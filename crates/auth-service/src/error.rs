//! auth-service 的错误类型。
//!
//! 所有错误均为 `Send + Sync`，且**不携带任何明文 secret**：keyring 返回的
//! 原始错误统一归一为 `Keychain(String)`，仅保留可读的归因描述。

use thiserror::Error;

/// 认证 / Secret 管理过程中可能出现的错误。
///
/// 任意变体的 `Display` 输出都不应包含明文 token；构造错误时严禁把 secret
/// 拼进 message。
#[derive(Debug, Error)]
pub enum AuthError {
    /// OS Keychain（或其它 `SecretBackend`）操作失败。
    #[error("keychain error: {0}")]
    Keychain(String),

    /// 指定 `(service, account)` 对应的条目不存在。
    #[error("credential not found")]
    NotFound,

    /// secret 本身非法（如为空、长度不足、格式不符）。
    #[error("invalid secret: {0}")]
    InvalidSecret(String),

    /// `StoredCredential` 元数据不完整或前后不一致。
    #[error("malformed credential metadata: {0}")]
    MalformedMetadata(String),
}
