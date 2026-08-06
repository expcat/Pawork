//! 取消（P3-8）。
//!
//! 协调 Run 级别的取消：单一根令牌传播到 Provider 流与 Tool 执行；
//! 取消后触发进程树清理回调（由宿主注入 `ProcessTreeCleaner`，因为
//! `process-runtime` 在 Phase 4 才落地，这里只定义抽象边界，不直接 spawn/kill）。
//!
//! 与状态机配合：取消发生时 Run 转 `Cancelled`，并保证已启动的子进程被回收，
//! 不遗留运行进程（验收标准：Cancel 不留下运行进程）。

use std::sync::Arc;

use agent_domain::CancellationToken;

/// 进程树清理抽象：取消后由宿主实现，负责 kill 子进程组。
///
/// 本 crate 不直接依赖 process-runtime；具体清理（macOS/Windows/Linux 进程组
/// 终止）在 Phase 4 落地后注入。这里以 trait 注入，保证取消路径可测、可替换。
pub trait ProcessTreeCleaner: Send + Sync {
    /// 清理给定 run 关联的进程树。返回被终止的进程数（用于事件/审计）。
    fn cleanup(&self, run_id: &agent_domain::RunId) -> usize;
}

/// 无操作清理器（测试与未接入进程运行时时使用）。
#[derive(Debug, Default, Clone)]
pub struct NoopProcessTreeCleaner;

impl ProcessTreeCleaner for NoopProcessTreeCleaner {
    fn cleanup(&self, _run_id: &agent_domain::RunId) -> usize {
        0
    }
}

/// Run 级别的取消句柄。
///
/// 持有根 [`CancellationToken`]，并可选关联一个进程树清理器。调用 [`CancelHandle::cancel`]
/// 会：① 取消根令牌（传播到 Provider/Tool）② 触发进程树清理。
#[derive(Clone)]
pub struct CancelHandle {
    token: CancellationToken,
    run_id: agent_domain::RunId,
    cleaner: Arc<dyn ProcessTreeCleaner>,
    /// 取消门控：用 compare_exchange 保证仅一方进入 cleanup（避免并发 TOCTOU）。
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// 已终止的进程数（便于审计事件，无锁原子）。
    killed: Arc<std::sync::atomic::AtomicU64>,
}

impl std::fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelHandle")
            .field("run_id", &self.run_id)
            .field("cancelled", &self.token.is_cancelled())
            .field(
                "killed",
                &self.killed.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl CancelHandle {
    pub fn new(run_id: agent_domain::RunId, cleaner: Arc<dyn ProcessTreeCleaner>) -> Self {
        Self {
            token: CancellationToken::new(),
            run_id,
            cleaner,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            killed: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// 共享的取消令牌：传给 Provider stream 与 Tool execute。
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// 取消原因（仅用于事件/日志，不影响传播）。
    pub fn cancel(&self, _reason: CancelReason) -> CancelReceipt {
        // 原子门控：compare_exchange 保证仅一个调用方进入 cleanup 路径，
        // 避免并发取消（如用户 + 预算/关停）重复触发进程树清理（TOCTOU）。
        let already = self
            .cancelled
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err();
        if already {
            return CancelReceipt {
                already_cancelled: true,
                processes_killed: 0,
            };
        }
        // 1) 传播取消到 Provider/Tool。
        self.token.cancel();
        // 2) 进程树清理：确保不遗留子进程。
        let processes_killed = self.cleaner.cleanup(&self.run_id) as u64;
        self.killed
            .store(processes_killed, std::sync::atomic::Ordering::Release);
        CancelReceipt {
            already_cancelled: false,
            processes_killed,
        }
    }

    /// 注册一个子令牌，使根取消时一并传播（用于派生子任务）。
    pub fn child_token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// 已被清理的进程数（取消后）。
    pub fn processes_killed(&self) -> u64 {
        self.killed.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// 取消来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    User,
    Budget,
    System,
    Shutdown,
}

/// 取消结果回执。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelReceipt {
    /// 调用前是否已处于取消态（幂等）。
    pub already_cancelled: bool,
    /// 被终止的进程数（0 表示无子进程或清理器为 noop）。
    pub processes_killed: u64,
}

impl CancelReceipt {
    pub fn cleaned_up(&self) -> bool {
        !self.already_cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// 可记录调用次数的清理器。
    struct CountingCleaner {
        run: agent_domain::RunId,
        count: Arc<AtomicUsize>,
    }

    impl ProcessTreeCleaner for CountingCleaner {
        fn cleanup(&self, run_id: &agent_domain::RunId) -> usize {
            assert_eq!(run_id, &self.run, "清理器应收到正确的 run_id");
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            3 // 模拟终止 3 个进程
        }
    }

    #[test]
    fn cancel_propagates_to_token_and_runs_cleanup() {
        let cleaner = Arc::new(CountingCleaner {
            run: agent_domain::RunId::from("run-1"),
            count: Arc::new(AtomicUsize::new(0)),
        });
        let handle = CancelHandle::new(agent_domain::RunId::from("run-1"), cleaner.clone());

        let token = handle.token();
        assert!(!token.is_cancelled());
        assert!(!handle.is_cancelled());

        let receipt = handle.cancel(CancelReason::User);
        assert!(receipt.cleaned_up());
        assert_eq!(receipt.processes_killed, 3);
        assert!(token.is_cancelled());
        assert_eq!(handle.processes_killed(), 3);
        assert_eq!(cleaner.count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn cancel_is_idempotent() {
        let handle = CancelHandle::new(
            agent_domain::RunId::from("run-1"),
            Arc::new(NoopProcessTreeCleaner),
        );
        handle.cancel(CancelReason::User);
        let receipt = handle.cancel(CancelReason::System);
        assert!(!receipt.cleaned_up());
        assert!(receipt.already_cancelled);
    }

    #[tokio::test]
    async fn child_token_completes_when_root_cancelled() {
        let handle = CancelHandle::new(
            agent_domain::RunId::from("run-1"),
            Arc::new(NoopProcessTreeCleaner),
        );
        let child = handle.child_token();
        let wait = child.cancelled();
        tokio::pin!(wait);

        // 未取消前 pending
        tokio::task::yield_now().await;
        handle.cancel(CancelReason::User);

        use std::future::Future;
        use std::pin::Pin;
        let mut f: Pin<&mut _> = wait;
        use std::task::{Context, Poll};
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert_eq!(f.as_mut().poll(&mut cx), Poll::Ready(()));
    }
}
