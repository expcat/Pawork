//! Marketplace 错误类型（P17-3）。
//!
//! 所有安全相关失败（签名、哈希、策略、trust、回滚补偿）一律 fail-closed：
//! 调用方不得把 `Ok` 之外的任何结果解释为成功。

use thiserror::Error;

/// Marketplace 统一错误类型。
#[derive(Debug, Error)]
pub enum MarketplaceError {
    #[error("package format error: {0}")]
    Package(#[from] plugin_package::PackageError),

    #[error("source `{name}` I/O failed: {message}")]
    SourceIo { name: String, message: String },

    #[error("index of source `{name}` is invalid: {message}")]
    InvalidIndex { name: String, message: String },

    #[error("package `{id}` not found in any configured source")]
    PackageNotFound { id: String },

    #[error("no version of package `{id}` matches requirement `{requirement}`")]
    NoMatchingVersion { id: String, requirement: String },

    #[error("dependency resolution failed: {message}")]
    Resolution { message: String },

    #[error("denied by policy: {0}")]
    PolicyDenied(String),

    #[error("denied by trust gate for `{id}` (level `{level}`): {message}")]
    TrustDenied {
        id: String,
        level: String,
        message: String,
    },

    #[error("signature verification failed for `{id}@{version}`: {message}")]
    Signature {
        id: String,
        version: String,
        message: String,
    },

    #[error("package identity mismatch: expected `{expected}`, archive declares `{found}`")]
    PackageIdentityMismatch { expected: String, found: String },

    #[error("bundle hash mismatch for `{id}@{version}`: expected {expected}, found {found}")]
    BundleHashMismatch {
        id: String,
        version: String,
        expected: String,
        found: String,
    },

    #[error("hash pin mismatch for `{id}`: pinned {pinned}, found {found}")]
    HashPinMismatch {
        id: String,
        pinned: String,
        found: String,
    },

    #[error("version pin violation for `{id}`: pinned to {pinned}")]
    VersionPinViolation { id: String, pinned: String },

    #[error("package `{0}` is already installed (use update)")]
    AlreadyInstalled(String),

    #[error("package `{0}` is not installed")]
    NotInstalled(String),

    #[error("resource conflict: {kind} `{key}` collides with installed package `{package}`")]
    ResourceConflict {
        kind: String,
        key: String,
        package: String,
    },

    #[error("host operation `{op}` failed for `{resource}`: {message}")]
    Host {
        op: &'static str,
        resource: String,
        message: String,
    },

    #[error(
        "operation failed: {original}; rollback compensation failures: {compensation_failures:?}"
    )]
    RollbackFailed {
        original: String,
        compensation_failures: Vec<String>,
    },

    #[error("state/pin store I/O failed: {0}")]
    State(String),

    #[error("staging I/O failed: {0}")]
    Staging(String),
}
