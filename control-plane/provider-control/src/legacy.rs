//! legacy 单凭据回退：`account-control-v1` 关闭时的运行时回退路径（ADR-033/P18-1）。
//!
//! 即便 `account-control-v1` feature 关闭，消费方仍可通过本模块取得 ADR-033
//! 定义的 legacy 作用域（tenant `local/default`、account `local/default`、principal
//! `local/user`）与合成默认账号描述（`ProviderAccount(default)` /
//! `Credential(default)`，路由策略 `single_candidate`），保证旧数据库与未升级
//! 部署可独立运行。
//!
//! 本模块不依赖网络、数据库与 Secret 存储，仅产出脱敏的定位描述。

use pawork_domain::{AccountId, CredentialId, PrincipalId, ProviderId, TenantId};

/// Legacy 作用域常量（与 `core-api` `DEFAULT_QUOTA_*` / `DEFAULT_CONTROL_PLANE_*` 对齐）。
pub const LEGACY_TENANT: &str = "local/default";
pub const LEGACY_ACCOUNT: &str = "local/default";
pub const LEGACY_PRINCIPAL: &str = "local/user";
pub const LEGACY_PROVIDER: &str = "default";
pub const LEGACY_CREDENTIAL: &str = "default";

/// Legacy 路由策略（冻结字符串，与 `app-database` 控制面 schema `single_candidate` 对齐）。
pub const LEGACY_ROUTING_STRATEGY: &str = "single_candidate";

/// `account-control-v1` 关闭时的合成默认账号描述（**不含任何 secret 字段**）。
///
/// 对应 ADR-033：旧单 credential 配置自动包装为 synthetic
/// `ProviderAccount(default)` / `Credential(default)`，默认策略 `SingleCandidate`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticDefaultAccount {
    /// 租户（`local/default`）。
    pub tenant_id: TenantId,
    /// 账号（`local/default`）。
    pub account_id: AccountId,
    /// 发起主体（`local/user`）。
    pub principal_id: PrincipalId,
    /// Provider（`default`）。
    pub provider_id: ProviderId,
    /// 凭据元数据标识（`default`，绝非凭据值）。
    pub credential_id: CredentialId,
    /// 路由策略（`single_candidate`）。
    pub routing_strategy: &'static str,
    /// 是否为迁移自动包装的合成账号。
    pub synthetic: bool,
}

impl SyntheticDefaultAccount {
    /// 构造 ADR-033 定义的 legacy 合成默认账号。
    pub fn legacy_default() -> Self {
        Self {
            tenant_id: TenantId::new(LEGACY_TENANT),
            account_id: AccountId::new(LEGACY_ACCOUNT),
            principal_id: PrincipalId::new(LEGACY_PRINCIPAL),
            provider_id: ProviderId::new(LEGACY_PROVIDER),
            credential_id: CredentialId::new(LEGACY_CREDENTIAL),
            routing_strategy: LEGACY_ROUTING_STRATEGY,
            synthetic: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_default_matches_adr_033_scope() {
        let account = SyntheticDefaultAccount::legacy_default();
        assert_eq!(account.tenant_id.as_str(), "local/default");
        assert_eq!(account.account_id.as_str(), "local/default");
        assert_eq!(account.principal_id.as_str(), "local/user");
        assert_eq!(account.provider_id.as_str(), "default");
        assert_eq!(account.credential_id.as_str(), "default");
        assert_eq!(account.routing_strategy, "single_candidate");
        assert!(account.synthetic);
    }

    #[test]
    fn legacy_default_carries_no_secret_field() {
        // 编译期穷尽：SyntheticDefaultAccount 只有 7 个脱敏定位字段，无 secret/token。
        let account = SyntheticDefaultAccount::legacy_default();
        let SyntheticDefaultAccount {
            tenant_id,
            account_id,
            principal_id,
            provider_id,
            credential_id,
            routing_strategy,
            synthetic,
        } = &account;
        assert_eq!(tenant_id.as_str(), "local/default");
        assert_eq!(account_id.as_str(), "local/default");
        assert_eq!(principal_id.as_str(), "local/user");
        assert_eq!(provider_id.as_str(), "default");
        assert_eq!(credential_id.as_str(), "default");
        assert_eq!(*routing_strategy, "single_candidate");
        assert!(*synthetic);
    }
}
