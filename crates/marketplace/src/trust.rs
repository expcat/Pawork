//! Trust 等级与配置（P17-3）。
//!
//! source / package 的 trust 等级决定安装审批：
//! - untrusted：默认拒装，需调用方显式批准（组织策略仍可否决）；
//! - verified：必须携带有效 Ed25519 签名（keyring 校验通过）；
//! - trusted：允许安装（若携带签名仍必须有效）。
//!
//! 有效 trust 取 source 信任与 package 覆盖中最保守者（fail-closed）。

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Trust 等级。声明顺序即严格程度顺序：Untrusted < Verified < Trusted。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// 不可信：默认拒装，需显式批准。
    #[default]
    Untrusted,
    /// 已验证：必须携带有效 Ed25519 签名。
    Verified,
    /// 可信：允许安装。
    Trusted,
}

impl fmt::Display for TrustLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Untrusted => "untrusted",
            Self::Verified => "verified",
            Self::Trusted => "trusted",
        })
    }
}

/// Trust 配置：source 级与 package 级覆盖。
///
/// 有效 trust = min(source 信任, package 覆盖)；package 覆盖缺省视为中性
/// （不压低 source 信任）。组织策略对 trust 的约束见 crate::policy::TeamPolicy，
/// 评估顺序优先于本配置与用户批准。
#[derive(Clone, Debug, Default)]
pub struct TrustConfig {
    /// source 名 → trust 覆盖（缺省用 SourceSpec 自带 trust）。
    pub source_overrides: BTreeMap<String, TrustLevel>,
    /// package id → trust 覆盖。
    pub package_overrides: BTreeMap<String, TrustLevel>,
}

impl TrustConfig {
    /// 计算某 source 上某 package 的有效 trust（取最保守）。
    pub fn effective(
        &self,
        source: &crate::source::SourceSpec,
        package_id: &plugin_package::PackageId,
    ) -> TrustLevel {
        let source_trust = self
            .source_overrides
            .get(&source.name)
            .copied()
            .unwrap_or(source.trust);
        match self.package_overrides.get(package_id.as_str()) {
            Some(level) => source_trust.min(*level),
            None => source_trust,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceSpec;
    use plugin_package::PackageId;

    #[test]
    fn effective_trust_takes_most_conservative() {
        let mut config = TrustConfig::default();
        let source = SourceSpec::registry("community", "https://example.com/index.json");
        let id = PackageId::new("acme.pkg").unwrap();
        // source 默认 untrusted。
        assert_eq!(config.effective(&source, &id), TrustLevel::Untrusted);

        config
            .source_overrides
            .insert("community".into(), TrustLevel::Trusted);
        assert_eq!(config.effective(&source, &id), TrustLevel::Trusted);

        config
            .package_overrides
            .insert("acme.pkg".into(), TrustLevel::Verified);
        assert_eq!(config.effective(&source, &id), TrustLevel::Verified);
    }

    #[test]
    fn ordering_is_by_strictness() {
        assert!(TrustLevel::Untrusted < TrustLevel::Verified);
        assert!(TrustLevel::Verified < TrustLevel::Trusted);
    }
}
