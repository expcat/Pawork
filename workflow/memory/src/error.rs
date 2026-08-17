//! Memory service 错误类型。
//!
//! 刻意不暴露 [`MemoryId`](pawork_domain::MemoryId) 引用，保持错误为纯值类型，
//! 避免 service 层把 canonical ID 语义泄漏到无关位置。

use pawork_api::ProviderError;
use thiserror::Error;

/// 长期记忆操作错误。
#[derive(Debug, Error)]
pub enum MemoryError {
    /// 候选 / 记忆文本命中 Secret 启发式，被拒绝进入记忆。
    #[error("memory rejected: summary contains sensitive material")]
    SecretDetected,
    /// 记忆为空或不可用。
    #[error("memory rejected: empty summary")]
    EmptySummary,
    /// 指定 memory 不存在。
    #[error("memory not found: {0}")]
    NotFound(String),
    /// 记忆已被失效，不可重复失效。
    #[error("memory already invalidated: {0}")]
    AlreadyInvalidated(String),
    /// 嵌入调用失败（透传 canonical Provider 错误，不感知 Provider 名）。
    #[error("embedding provider error")]
    Embedding(#[from] ProviderError),
}
