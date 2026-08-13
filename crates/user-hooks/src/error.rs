//! User Hook 错误类型。
//!
//! 所有错误只携带可安全记录的信息；任何 Secret 明文不得进入 `Display` 或
//! `Debug` 输出，secret 相关失败统一归一为 [`HookError::SecretUnavailable`]。

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Hook 注册 / 派发期错误。
#[derive(Clone, Debug, Error)]
pub enum HookError {
    /// 配置 schema 无效（未知 handler 类型、缺字段、scope 非法等）。
    #[error("invalid hook config: {0}")]
    InvalidConfig(String),
    /// Handler 重复注册（同一 hook id 已存在）。
    #[error("hook {hook_id} is already registered")]
    Conflict { hook_id: String },
    /// Handler 未注册。
    #[error("hook {hook_id} is not registered")]
    NotFound { hook_id: String },
    /// 注入的执行器返回错误；message 已经过 redaction，可安全记录。
    #[error("executor failure for hook {hook_id}: {message}")]
    Executor { hook_id: String, message: String },
    /// Secret 引用无法解析（不存在 / 无权限）。不暴露引用细节。
    #[error("a required secret is unavailable for hook {hook_id}")]
    SecretUnavailable { hook_id: String },
    /// 策略拒绝继续执行。
    #[error("policy denied hook {hook_id}: {reason}")]
    PolicyDenied { hook_id: String, reason: String },
    /// 同步阻断 handler 超时。
    #[error("hook {hook_id} timed out after {timeout_ms}ms")]
    Timeout { hook_id: String, timeout_ms: u64 },
    /// 被取消。
    #[error("hook {hook_id} was cancelled")]
    Cancelled { hook_id: String },
}

impl HookError {
    /// 把任意执行器侧错误字符串归一为 [`HookError::Executor`]。
    pub fn executor(hook_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Executor {
            hook_id: hook_id.into(),
            message: message.into(),
        }
    }
}

/// 派发一条 hook 的结果状态（可序列化为 canonical 审计事件）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HookStatus {
    /// 成功完成（同步）或已成功投递（async fire-and-forget 后续回报）。
    Success,
    /// 执行失败但未中断 run loop（async 失败仅记录）。
    Failed(String),
    /// 策略拒绝。
    Denied(String),
    /// 超时降级。
    Timeout,
    /// 被取消。
    Cancelled,
}

impl HookStatus {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}
