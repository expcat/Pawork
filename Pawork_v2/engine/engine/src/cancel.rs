//! Run 级取消：单一根令牌传到 Provider / Tool；`cancel()` 原子门控后
//! 再触发宿主注入的 [`ProcessTreeCleaner`]。
//!
//! 生产杀树靠同一把 token → `run_command` 桥 → `pawork-exec` 的
//! `ProcessTreeGuard`（波 A）。cleaner 是可测钩子，与 V1 同形；默认
//! [`NoopProcessTreeCleaner`]，不另建 `run_id → 进程` 登记表。

use std::sync::Arc;

use pawork_domain::{CancellationToken, RunId};

/// 进程树清理抽象：由宿主注入。engine 不依赖 `pawork-exec`。
pub trait ProcessTreeCleaner: Send + Sync {
    /// 清理该 run 关联的进程树。返回被终止的进程数（审计用）。
    fn cleanup(&self, run_id: &RunId) -> usize;
}

/// 无操作清理器（测试与未登记进程时使用）。
#[derive(Debug, Default, Clone)]
pub struct NoopProcessTreeCleaner;

impl ProcessTreeCleaner for NoopProcessTreeCleaner {
    fn cleanup(&self, _run_id: &RunId) -> usize {
        0
    }
}

/// Run 级取消句柄。
///
/// [`CancelHandle::cancel`]：① 取消根令牌 ② 触发进程树清理。并发调用幂等。
#[derive(Clone)]
pub struct CancelHandle {
    token: CancellationToken,
    run_id: RunId,
    cleaner: Arc<dyn ProcessTreeCleaner>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
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
    pub fn new(run_id: RunId, cleaner: Arc<dyn ProcessTreeCleaner>) -> Self {
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

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// `reason` 仅供调用方写入事件/日志；本方法不改信封形状。
    pub fn cancel(&self, _reason: CancelReason) -> CancelReceipt {
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
        self.token.cancel();
        let processes_killed = self.cleaner.cleanup(&self.run_id) as u64;
        self.killed
            .store(processes_killed, std::sync::atomic::Ordering::Release);
        CancelReceipt {
            already_cancelled: false,
            processes_killed,
        }
    }

    /// 与根令牌同一把（派生任务共用）。
    pub fn child_token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn processes_killed(&self) -> u64 {
        self.killed.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// 取消来源（运行时枚举，不进 `RunCancelled` 信封）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    User,
    Budget,
    System,
    Shutdown,
}

/// 取消回执。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelReceipt {
    pub already_cancelled: bool,
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

    struct CountingCleaner {
        run: RunId,
        count: Arc<AtomicUsize>,
    }

    impl ProcessTreeCleaner for CountingCleaner {
        fn cleanup(&self, run_id: &RunId) -> usize {
            assert_eq!(run_id, &self.run, "清理器应收到正确的 run_id");
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            3
        }
    }

    #[test]
    fn cancel_propagates_to_token_and_runs_cleanup() {
        let cleaner = Arc::new(CountingCleaner {
            run: RunId::from("run-1"),
            count: Arc::new(AtomicUsize::new(0)),
        });
        let handle = CancelHandle::new(RunId::from("run-1"), cleaner.clone());

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
        let handle = CancelHandle::new(RunId::from("run-1"), Arc::new(NoopProcessTreeCleaner));
        handle.cancel(CancelReason::User);
        let receipt = handle.cancel(CancelReason::System);
        assert!(!receipt.cleaned_up());
        assert!(receipt.already_cancelled);
    }

    #[tokio::test]
    async fn child_token_completes_when_root_cancelled() {
        let handle = CancelHandle::new(RunId::from("run-1"), Arc::new(NoopProcessTreeCleaner));
        let child = handle.child_token();
        handle.cancel(CancelReason::User);
        child.cancelled().await;
        assert!(child.is_cancelled());
    }
}
