//! 重试（P3-7）。
//!
//! Agent 层重试三种粒度：
//! - **断流重试**：Provider 流中途断开（`StreamInterrupted`/`Network`/`Timeout`），
//!   在同一次模型调用内重发请求，保持上下文（历史消息）不变。
//! - **retry last call**：丢弃上一次不完整助手消息，用相同上下文重试上一次模型调用。
//! - **retry run**：从某个事件点重跑整个 Run。
//!
//! 每次重试产生可追溯信号（`RetryAttempt`），由调用方翻译为 `Diagnostic` 事件，
//! 保证「重试与事件一致性：可追溯」。
//!
//! 重试策略基于 `ProviderError` 的 `retryable` 与 `retry_after_ms`（P2-10 已归一），
//! 退避用指数退避（jitter 可选），在 Agent Engine 安全关键路径内保持单一实现。

use std::time::Duration;

use provider_api::{ProviderError, ProviderErrorKind};

/// 一次重试的记录（用于事件/审计）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryAttempt {
    /// 第几次重试（1 起）。
    pub attempt: u32,
    /// 触发重试的错误类别。
    pub reason: RetryReason,
    /// 本次重试前的退避等待。
    pub backoff_ms: u64,
}

/// 重试触发原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryReason {
    StreamInterrupted,
    Network,
    Timeout,
    RateLimited,
    ProviderUnavailable,
    Other,
}

impl RetryReason {
    pub fn from_error(err: &ProviderError) -> Self {
        match err.kind {
            ProviderErrorKind::StreamInterrupted => Self::StreamInterrupted,
            ProviderErrorKind::Network => Self::Network,
            ProviderErrorKind::Timeout => Self::Timeout,
            ProviderErrorKind::RateLimited => Self::RateLimited,
            ProviderErrorKind::ProviderUnavailable => Self::ProviderUnavailable,
            _ => Self::Other,
        }
    }
}

/// 重试决策。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryDecision {
    /// 可以重试：等待 `backoff` 后再试。
    Retry {
        attempt: u32,
        backoff: Duration,
        reason: RetryReason,
    },
    /// 不再重试（已达上限或错误不可重试）。
    Stop { reason: RetryReason },
}

/// 重试策略配置。
#[derive(Clone, Debug)]
pub struct RetryPolicy {
    /// 最大重试次数（不含首次调用）。
    pub max_attempts: u32,
    /// 初始退避。
    pub initial_backoff: Duration,
    /// 退避倍率。
    pub multiplier: f64,
    /// 最大退避上限。
    pub max_backoff: Duration,
    /// 退避抖动比例（0..=1，0 关闭）。
    pub jitter: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(500),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(10),
            jitter: 0.2,
        }
    }
}

impl RetryPolicy {
    /// 判断一个错误是否可重试。
    pub fn is_retryable(err: &ProviderError) -> bool {
        err.retryable
            && matches!(
                err.kind,
                ProviderErrorKind::StreamInterrupted
                    | ProviderErrorKind::Network
                    | ProviderErrorKind::Timeout
                    | ProviderErrorKind::RateLimited
                    | ProviderErrorKind::ProviderUnavailable
            )
    }

    /// 给定当前已尝试次数（1 起）与错误，返回下一步决策。
    pub fn decide(&self, attempt: u32, err: &ProviderError) -> RetryDecision {
        let reason = RetryReason::from_error(err);
        if !Self::is_retryable(err) || attempt > self.max_attempts {
            return RetryDecision::Stop { reason };
        }
        // 优先尊重 Provider 给出的 retry_after（如限流）。
        let backoff = if let Some(after_ms) = err.retry_after_ms {
            Duration::from_millis(after_ms)
        } else {
            self.compute_backoff(attempt)
        };
        RetryDecision::Retry {
            attempt,
            backoff,
            reason,
        }
    }

    /// 计算第 `attempt` 次重试的退避（含抖动）。
    ///
    /// 注意：抖动用于实际等待；为可测性，本函数仅返回确定值，抖动由调用方
    /// 在 sleep 前应用（或测试中固定 seed）。
    pub fn compute_backoff(&self, attempt: u32) -> Duration {
        let exp = self.multiplier.powi((attempt.saturating_sub(1)) as i32);
        let raw = self.initial_backoff.as_millis() as f64 * exp;
        let capped = raw.min(self.max_backoff.as_millis() as f64);
        Duration::from_millis(capped as u64)
    }

    /// 生成退避序列（测试用，不含抖动）。
    pub fn backoff_schedule(&self) -> Vec<Duration> {
        (1..=self.max_attempts)
            .map(|a| self.compute_backoff(a))
            .collect()
    }
}

/// 重试控制器：跟踪当前尝试次数与历史，产出 [`RetryAttempt`] 供事件化。
#[derive(Clone, Debug)]
pub struct RetryController {
    policy: RetryPolicy,
    attempts: u32,
    history: Vec<RetryAttempt>,
}

impl RetryController {
    pub fn new(policy: RetryPolicy) -> Self {
        Self {
            policy,
            attempts: 0,
            history: Vec::new(),
        }
    }

    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn history(&self) -> &[RetryAttempt] {
        &self.history
    }

    /// 给定错误，决定是否重试并记录一次尝试。返回决策与应等待时长。
    pub fn on_error(&mut self, err: &ProviderError) -> RetryDecision {
        self.attempts += 1;
        let decision = self.policy.decide(self.attempts, err);
        match &decision {
            RetryDecision::Retry {
                attempt,
                backoff,
                reason,
            } => {
                self.history.push(RetryAttempt {
                    attempt: *attempt,
                    reason: *reason,
                    backoff_ms: backoff.as_millis() as u64,
                });
            }
            RetryDecision::Stop { .. } => {}
        }
        decision
    }

    /// 重置（成功后或 retry last call/run 时调用）。
    pub fn reset(&mut self) {
        self.attempts = 0;
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(kind: ProviderErrorKind, retryable: bool) -> ProviderError {
        let mut e = ProviderError::new(kind, "boom");
        e.retryable = retryable;
        e
    }

    #[test]
    fn is_retryable_only_for_transient_kinds() {
        assert!(RetryPolicy::is_retryable(&err(
            ProviderErrorKind::StreamInterrupted,
            true
        )));
        assert!(RetryPolicy::is_retryable(&err(
            ProviderErrorKind::RateLimited,
            true
        )));
        // retryable=false 即便类别瞬时也不重试
        assert!(!RetryPolicy::is_retryable(&err(
            ProviderErrorKind::Network,
            false
        )));
        // 非瞬时类别不可重试
        assert!(!RetryPolicy::is_retryable(&err(
            ProviderErrorKind::InvalidRequest,
            true
        )));
    }

    #[test]
    fn decide_retries_then_stops_after_max() {
        let policy = RetryPolicy {
            max_attempts: 2,
            initial_backoff: Duration::from_millis(100),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(5),
            jitter: 0.0,
        };
        let mut ctrl = RetryController::new(policy);

        let e = err(ProviderErrorKind::StreamInterrupted, true);
        match ctrl.on_error(&e) {
            RetryDecision::Retry {
                attempt, backoff, ..
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(backoff, Duration::from_millis(100));
            }
            other => panic!("expected retry, got {other:?}"),
        }
        match ctrl.on_error(&e) {
            RetryDecision::Retry {
                attempt, backoff, ..
            } => {
                assert_eq!(attempt, 2);
                assert_eq!(backoff, Duration::from_millis(200));
            }
            other => panic!("expected retry, got {other:?}"),
        }
        // 第三次超过 max_attempts=2 → Stop
        assert!(matches!(ctrl.on_error(&e), RetryDecision::Stop { .. }));
        assert_eq!(ctrl.history().len(), 2);
    }

    #[test]
    fn non_retryable_error_stops_immediately() {
        let mut ctrl = RetryController::new(RetryPolicy::default());
        let e = err(ProviderErrorKind::InvalidRequest, false);
        assert!(matches!(ctrl.on_error(&e), RetryDecision::Stop { .. }));
        assert!(ctrl.history().is_empty());
    }

    #[test]
    fn respects_retry_after_for_rate_limit() {
        let policy = RetryPolicy::default();
        let mut e = err(ProviderErrorKind::RateLimited, true);
        e.retry_after_ms = Some(1234);
        let dec = policy.decide(1, &e);
        match dec {
            RetryDecision::Retry {
                backoff, reason, ..
            } => {
                assert_eq!(backoff, Duration::from_millis(1234));
                assert_eq!(reason, RetryReason::RateLimited);
            }
            _ => panic!("expected retry"),
        }
    }

    #[test]
    fn backoff_caps_at_max() {
        let policy = RetryPolicy {
            max_attempts: 10,
            initial_backoff: Duration::from_millis(100),
            multiplier: 10.0,
            max_backoff: Duration::from_millis(500),
            jitter: 0.0,
        };
        // 任何 attempt 都不应超过 max_backoff
        for b in policy.backoff_schedule() {
            assert!(b <= Duration::from_millis(500));
        }
    }

    #[test]
    fn reset_clears_history() {
        let mut ctrl = RetryController::new(RetryPolicy {
            max_attempts: 5,
            ..RetryPolicy::default()
        });
        ctrl.on_error(&err(ProviderErrorKind::Network, true));
        ctrl.on_error(&err(ProviderErrorKind::Network, true));
        assert_eq!(ctrl.attempts(), 2);
        ctrl.reset();
        assert_eq!(ctrl.attempts(), 0);
        assert!(ctrl.history().is_empty());
    }
}
