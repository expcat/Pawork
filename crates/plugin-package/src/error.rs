//! Package 加载、校验、归档校验与分发的错误类型。

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("package manifest is not valid TOML: {0}")]
    ManifestToml(String),
    #[error("package manifest is not valid JSON: {0}")]
    ManifestJson(String),
    #[error("package manifest field `{field}` is invalid: {message}")]
    ManifestField { field: String, message: String },
    #[error("package manifest schema version {found} is unsupported (expected {expected})")]
    UnsupportedSchemaVersion { found: u32, expected: u32 },
    #[error("package dependency requirement is invalid: {0}")]
    DependencyRequirement(String),
    #[error("package contains a conflict: {0}")]
    Conflict(String),
    #[error("package archive I/O failed: {0}")]
    ArchiveIo(#[from] std::io::Error),
    #[error("package archive path escapes the package root: {0}")]
    PathEscape(PathBuf),
    #[error("package archive is missing required file: {0}")]
    MissingFile(PathBuf),
    #[error("package content hash mismatch for `{path}`: expected {expected}, found {found}")]
    HashMismatch {
        path: PathBuf,
        expected: String,
        found: String,
    },
    #[error("package content manifest lists a path that is not a regular file: {0}")]
    NotRegularFile(PathBuf),
    #[error("package content manifest contains an invalid blake3 hash at line {line}: {value}")]
    InvalidContentHash { line: usize, value: String },
    #[error("package content manifest contains a duplicate path: {0}")]
    DuplicateContentPath(PathBuf),
    #[error("package archive contains a regular file not listed in contents.b3: {0}")]
    UnlistedFile(PathBuf),
    #[error("package archive exceeds {resource} limit of {limit}, found {found}")]
    ResourceLimit {
        resource: &'static str,
        limit: u64,
        found: u64,
    },
    #[error("package dispatch to `{sink}` failed for `{resource}`: {message}")]
    Dispatch {
        sink: &'static str,
        resource: String,
        message: String,
    },
}

impl PackageError {
    /// 构造一个字段级 manifest 校验错误。
    pub(crate) fn field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ManifestField {
            field: field.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_redact_no_secret_assumption() {
        // 错误只携带结构性信息（路径、哈希、字段名），不强制回显子资源正文。
        let err = PackageError::HashMismatch {
            path: PathBuf::from("skills/x/manifest.toml"),
            expected: "abc".into(),
            found: "def".into(),
        };
        assert!(err.to_string().contains("skills/x/manifest.toml"));
        assert!(err.to_string().contains("abc"));
        assert!(err.to_string().contains("def"));
    }
}
