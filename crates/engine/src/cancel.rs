//! Run 级取消：单一根令牌传到 Provider / Tool。`cancel()` 只取消该令牌。
//!
//! 生产杀树由同一把令牌桥到 `run_command` / `pawork-exec` 的 `ProcessTreeGuard`。
//! engine 不依赖 `pawork-exec`，也不另建 `run_id → 进程` 登记表。

use std::sync::Arc;

use pawork_domain::{CancellationToken, RunId};

/// Run 级取消句柄。
///
/// [`CancelHandle::cancel`] 取消根令牌。并发调用幂等。
#[derive(Clone)]
pub struct CancelHandle {
    token: CancellationToken,
    run_id: RunId,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl std::fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelHandle")
            .field("run_id", &self.run_id)
            .field("cancelled", &self.token.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl CancelHandle {
    pub fn new(run_id: RunId) -> Self {
        Self {
            token: CancellationToken::new(),
            run_id,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            };
        }
        self.token.cancel();
        CancelReceipt {
            already_cancelled: false,
        }
    }

    /// 与根令牌同一把（派生任务共用）。
    pub fn child_token(&self) -> CancellationToken {
        self.token.clone()
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
}

impl CancelReceipt {
    pub fn cleaned_up(&self) -> bool {
        !self.already_cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_idempotent() {
        let handle = CancelHandle::new(RunId::from("run-1"));
        let token = handle.token();
        assert!(!token.is_cancelled());
        let first = handle.cancel(CancelReason::User);
        assert!(first.cleaned_up());
        assert!(token.is_cancelled());
        let receipt = handle.cancel(CancelReason::System);
        assert!(!receipt.cleaned_up());
        assert!(receipt.already_cancelled);
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn child_token_completes_when_root_cancelled() {
        let handle = CancelHandle::new(RunId::from("run-1"));
        let child = handle.child_token();
        handle.cancel(CancelReason::User);
        child.cancelled().await;
        assert!(child.is_cancelled());
    }
}
