//! 不透明 secret 定位符（P17-2）。
//!
//! Package manifest 中 MCP 的 env / headers 值一律是 [`SecretRef`] 定位符——
//! 只持久化 `(service, account)` 键名，不携带明文。`Debug` / `Serialize` /
//! roundtrip 因此不可能泄漏 token；明文由宿主在安装时绑定到
//! `auth_service::SecretBackend` 并即时解析（与 `mcp-client::security::SecretRef`
//! 模型一致；plugin-package 不依赖 mcp-client，故各自保留同构类型）。
//!
//! 定位符本身受严格校验（P17-2 安全复审）：非空、长度受限、ASCII 字符集、
//! 拒绝明显 token 形态（`sk-`、`ghp_`、`Bearer `、JWT 前缀等）。校验在
//! `PackageManifest::validate`（经 `McpServerDeclaration::validate` 的 env /
//! headers 真实字段）中强制执行。

use serde::{Deserialize, Serialize};

use crate::error::PackageError;

/// SecretRef 定位符的最大长度（service / account 各自）。
pub const MAX_SECRET_REF_LEN: usize = 128;

/// 指向 `SecretBackend` 中一条 secret 的定位符。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SecretRef {
    service: String,
    account: String,
}

impl SecretRef {
    /// 从 backend 定位符构造引用（定位符本身不是 secret）。
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    /// Backend `service`（keychain 命名空间）。
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Backend `account` 键。
    pub fn account(&self) -> &str {
        &self.account
    }

    /// 校验定位符自身：非空、长度 ≤ [`MAX_SECRET_REF_LEN`]、ASCII 字符集、拒绝
    /// 明显 token 形态（定位符只应携带 backend 键名，不应携带明文凭证）。
    ///
    /// 由 `McpServerDeclaration::validate` 在 env / headers 真实字段上强制调用
    /// （即 `PackageManifest::validate` 链）。
    pub fn validate(&self) -> Result<(), PackageError> {
        validate_locator("service", &self.service)?;
        validate_locator("account", &self.account)
    }
}

fn validate_locator(field: &str, value: &str) -> Result<(), PackageError> {
    if value.is_empty() {
        return Err(PackageError::field(
            field,
            "secret ref locator must not be empty",
        ));
    }
    if value.len() > MAX_SECRET_REF_LEN {
        return Err(PackageError::field(
            field,
            format!("secret ref locator must not exceed {MAX_SECRET_REF_LEN} characters"),
        ));
    }
    let valid_chars = value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/' | '@' | '+' | '%')
    });
    if !valid_chars {
        return Err(PackageError::field(
            field,
            "secret ref locator may only contain ASCII alphanumerics and `-_.:/@+%`",
        ));
    }
    // 明显 token 形态（大小写不敏感）：定位符不应携带任何明文凭证特征。
    const TOKEN_MARKERS: &[&str] = &[
        "sk-",
        "sk_",
        "pk-",
        "pk_",
        "ghp_",
        "gho_",
        "github_pat_",
        "glpat-",
        "glptt-",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxr-",
        "akia",
        "asia",
        "eyj",
        "ya29.",
        "aiza",
        "bearer ",
        "-----begin",
    ];
    let lowered = value.to_ascii_lowercase();
    if TOKEN_MARKERS.iter().any(|marker| lowered.contains(marker)) {
        return Err(PackageError::field(
            field,
            "secret ref locator must not contain secret material (obvious token markers are rejected)",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_only_locators_and_round_trips() {
        const TOKEN: &str = "sk-package-supersecret-abcdef";
        let reference = SecretRef::new("pawork.mcp.fs", "cred-1");
        let json = serde_json::to_string(&reference).expect("serialize");
        assert!(json.contains("pawork.mcp.fs"));
        assert!(!json.contains(TOKEN));

        let back: SecretRef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, reference);
    }

    #[test]
    fn debug_contains_only_locators() {
        const TOKEN: &str = "sk-package-supersecret-abcdef";
        let reference = SecretRef::new("pawork.mcp", "cred-1");
        let rendered = format!("{reference:?}");
        assert!(rendered.contains("pawork.mcp"));
        assert!(!rendered.contains(TOKEN));
    }

    #[test]
    fn validates_locators() {
        assert!(SecretRef::new("pawork.mcp.fs", "cred-1").validate().is_ok());
        assert!(SecretRef::new("pawork.mcp", "user@example.com")
            .validate()
            .is_ok());
    }

    #[test]
    fn rejects_empty_locator() {
        assert!(SecretRef::new("", "cred-1").validate().is_err());
        assert!(SecretRef::new("pawork.mcp", "").validate().is_err());
    }

    #[test]
    fn rejects_overlong_locator() {
        assert!(SecretRef::new("x".repeat(MAX_SECRET_REF_LEN + 1), "cred-1")
            .validate()
            .is_err());
        assert!(
            SecretRef::new("pawork.mcp", "y".repeat(MAX_SECRET_REF_LEN + 1))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn rejects_bad_charset() {
        assert!(SecretRef::new("bad service", "cred-1").validate().is_err());
        assert!(SecretRef::new("pawork.mcp", "cred=1").validate().is_err());
        assert!(SecretRef::new("服务", "cred-1").validate().is_err());
        assert!(SecretRef::new("pawork.mcp", "cred 1").validate().is_err());
    }

    #[test]
    fn rejects_obvious_token_material() {
        for token in [
            "sk-live-token-0123456789",
            "sk_secret",
            "ghp_abcdefghijklmnopqrstuvwxyz",
            "github_pat_1234",
            "xoxb-1234-5678",
            "Bearer eyJhbGciOiJIUzI1NiJ9",
            "eyJhbGciOiJSUzI1NiIsImtpZCI6InNvbWUta2V5LWlkIn0",
            "AKIAIOSFODNN7EXAMPLE",
            "-----BEGIN RSA PRIVATE KEY-----",
        ] {
            assert!(
                SecretRef::new(token, "cred-1").validate().is_err(),
                "service must reject {token}"
            );
            assert!(
                SecretRef::new("pawork.mcp", token).validate().is_err(),
                "account must reject {token}"
            );
        }
    }
}
