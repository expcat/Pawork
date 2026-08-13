//! Team policy（P17-3）：组织级安装控制。
//!
//! 组织策略优先于用户批准与 trust 配置评估，且 fail-closed：策略文本不可读时
//! 解析即拒绝（[TeamPolicy::from_json]），不给「按缺省放行」留口子。

use std::collections::{BTreeMap, BTreeSet};

use plugin_package::PackageId;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::error::MarketplaceError;
use crate::source::SourceSpec;
use crate::trust::TrustLevel;

/// 组织 / team 安装策略。
///
/// 字段缺省皆为最宽松（无名单、最低 trust 为 untrusted、不强制签名）；任一
/// 规则命中即拒绝，用户批准不能覆盖。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamPolicy {
    /// 禁止的 source 名（优先评估）。
    #[serde(default)]
    pub denied_sources: BTreeSet<String>,
    /// 允许的 source 名；None 表示不设白名单。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_sources: Option<BTreeSet<String>>,
    /// 最低可接受有效 trust。
    #[serde(default)]
    pub min_trust: TrustLevel,
    /// 强制要求有效 Ed25519 签名。
    #[serde(default)]
    pub require_signature: bool,
    /// package id -> 允许的版本范围。
    #[serde(default)]
    pub allowed_versions: BTreeMap<String, VersionReq>,
}

/// 策略评估输入。
pub struct PolicyInput<'a> {
    pub id: &'a PackageId,
    pub version: &'a Version,
    pub source: &'a SourceSpec,
    /// 有效 trust（source 与 package 覆盖取最保守）。
    pub effective_trust: TrustLevel,
    /// 签名是否存在且通过 keyring 校验。
    pub signature_valid: bool,
}

impl TeamPolicy {
    /// 拒绝一切安装（fail-closed 兜底 / 高安全默认）。
    pub fn deny_all() -> Self {
        Self {
            allowed_sources: Some(BTreeSet::new()),
            min_trust: TrustLevel::Trusted,
            require_signature: true,
            ..Self::default()
        }
    }

    /// 从 JSON 解析策略。不可读策略 fail-closed：直接返回 PolicyDenied。
    pub fn from_json(text: &str) -> Result<Self, MarketplaceError> {
        serde_json::from_str(text).map_err(|error| {
            MarketplaceError::PolicyDenied(format!(
                "team policy is unreadable (fail-closed): {error}"
            ))
        })
    }

    /// 评估一次安装请求；任一规则命中返回 PolicyDenied。
    ///
    /// 评估顺序：source 黑名单 -> source 白名单 -> 版本白名单 -> 最低 trust ->
    /// 强制签名。全部先于用户批准与 trust gate（组织策略优先）。
    pub fn evaluate(&self, input: &PolicyInput<'_>) -> Result<(), MarketplaceError> {
        if self.denied_sources.contains(&input.source.name) {
            return Err(MarketplaceError::PolicyDenied(format!(
                "source {} is denied by team policy",
                input.source.name
            )));
        }
        if let Some(allowed) = &self.allowed_sources {
            if !allowed.contains(&input.source.name) {
                return Err(MarketplaceError::PolicyDenied(format!(
                    "source {} is not in the team policy allowlist",
                    input.source.name
                )));
            }
        }
        if let Some(requirement) = self.allowed_versions.get(input.id.as_str()) {
            if !requirement.matches(input.version) {
                return Err(MarketplaceError::PolicyDenied(format!(
                    "version {} of {} is not allowed by team policy (requirement {requirement})",
                    input.version,
                    input.id.as_str()
                )));
            }
        }
        if input.effective_trust < self.min_trust {
            return Err(MarketplaceError::PolicyDenied(format!(
                "effective trust {} is below team policy minimum {}",
                input.effective_trust, self.min_trust
            )));
        }
        if self.require_signature && !input.signature_valid {
            return Err(MarketplaceError::PolicyDenied(format!(
                "team policy requires a valid signature for {}",
                input.id.as_str()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(
        id: &'a PackageId,
        version: &'a Version,
        source: &'a SourceSpec,
        effective_trust: TrustLevel,
        signature_valid: bool,
    ) -> PolicyInput<'a> {
        PolicyInput {
            id,
            version,
            source,
            effective_trust,
            signature_valid,
        }
    }

    #[test]
    fn default_policy_is_permissive() {
        let policy = TeamPolicy::default();
        let id = PackageId::new("acme.pkg").unwrap();
        let version = Version::new(1, 0, 0);
        let source = SourceSpec::registry("community", "https://example.com");
        policy
            .evaluate(&input(&id, &version, &source, TrustLevel::Untrusted, false))
            .expect("default policy permits");
    }

    #[test]
    fn denied_and_allowlisted_sources() {
        let id = PackageId::new("acme.pkg").unwrap();
        let version = Version::new(1, 0, 0);
        let denied_source = SourceSpec::registry("rogue", "https://rogue.example");
        let mut policy = TeamPolicy::default();
        policy.denied_sources.insert("rogue".into());
        let error = policy
            .evaluate(&input(
                &id,
                &version,
                &denied_source,
                TrustLevel::Trusted,
                true,
            ))
            .unwrap_err();
        assert!(error.to_string().contains("denied by team policy"));

        let other_source = SourceSpec::registry("community", "https://example.com");
        policy.allowed_sources = Some(BTreeSet::from(["official".to_string()]));
        let error = policy
            .evaluate(&input(
                &id,
                &version,
                &other_source,
                TrustLevel::Trusted,
                true,
            ))
            .unwrap_err();
        assert!(error.to_string().contains("allowlist"));
    }

    #[test]
    fn min_trust_and_require_signature_are_enforced() {
        let id = PackageId::new("acme.pkg").unwrap();
        let version = Version::new(1, 0, 0);
        let source = SourceSpec::registry("community", "https://example.com");
        let policy = TeamPolicy {
            min_trust: TrustLevel::Verified,
            require_signature: true,
            ..TeamPolicy::default()
        };
        assert!(policy
            .evaluate(&input(&id, &version, &source, TrustLevel::Untrusted, true))
            .is_err());
        assert!(policy
            .evaluate(&input(&id, &version, &source, TrustLevel::Verified, false))
            .is_err());
        policy
            .evaluate(&input(&id, &version, &source, TrustLevel::Verified, true))
            .expect("trusted + signed passes");
    }

    #[test]
    fn allowed_versions_are_enforced() {
        let id = PackageId::new("acme.pkg").unwrap();
        let source = SourceSpec::registry("community", "https://example.com");
        let policy = TeamPolicy {
            allowed_versions: BTreeMap::from([(
                "acme.pkg".to_string(),
                VersionReq::parse("^1").unwrap(),
            )]),
            ..TeamPolicy::default()
        };
        assert!(policy
            .evaluate(&input(
                &id,
                &Version::new(2, 0, 0),
                &source,
                TrustLevel::Trusted,
                false
            ))
            .is_err());
        policy
            .evaluate(&input(
                &id,
                &Version::new(1, 2, 0),
                &source,
                TrustLevel::Trusted,
                false,
            ))
            .expect("version in range passes");
    }

    #[test]
    fn unreadable_policy_fails_closed() {
        let error = TeamPolicy::from_json("{ not json").unwrap_err();
        assert!(matches!(error, MarketplaceError::PolicyDenied(_)));
        let parsed =
            TeamPolicy::from_json(r#"{"min_trust": "verified", "require_signature": true}"#)
                .unwrap();
        assert_eq!(parsed.min_trust, TrustLevel::Verified);
        assert!(parsed.require_signature);
    }

    #[test]
    fn deny_all_rejects_everything() {
        let policy = TeamPolicy::deny_all();
        let id = PackageId::new("acme.pkg").unwrap();
        let version = Version::new(1, 0, 0);
        let source = SourceSpec::registry("community", "https://example.com");
        assert!(policy
            .evaluate(&input(&id, &version, &source, TrustLevel::Trusted, true))
            .is_err());
    }
}
