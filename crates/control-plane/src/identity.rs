//! Canonical identity（P18-2）：`IdentityContext`。
//!
//! `IdentityContext` 是跨模块统一键（tenant + principal），Session / Agent /
//! Usage / Audit 的创建与查询都必须携带它；`TenantId` 表示组织 / 逻辑租户，
//! `PrincipalId` 表示当前用户或服务账号，两者不得由 API key hash 代替。
//!
//! 未配置 tenant 的本地用户固定映射 `tenant_id = local/default`、
//! `principal_id = local/user`（ADR-033），由 [`IdentityContext::local`] /
//! [`default_tenant`] / [`default_principal`] 提供。空 tenant / principal 经
//! [`IdentityContext::validate`] 拒绝。

use serde::{Deserialize, Serialize};

pub use pawork_domain::{PrincipalId, TenantId};

/// 默认租户标识字符串（禁止改成 quota 哨兵 `"local"`）。
pub const DEFAULT_TENANT: &str = "local/default";
/// 默认主体标识字符串。
pub const DEFAULT_PRINCIPAL: &str = "local/user";

/// 返回默认租户身份。
pub fn default_tenant() -> TenantId {
    TenantId::new(DEFAULT_TENANT)
}

/// 返回默认主体身份。
pub fn default_principal() -> PrincipalId {
    PrincipalId::new(DEFAULT_PRINCIPAL)
}

/// 一次请求 / 一条持久记录的身份上下文：租户 + 主体。
///
/// 值类型：`Clone + Eq`，可序列化进入 canonical event / 持久化实体；不可变，
/// 创建后不修改。空 tenant / principal 必须经 [`Self::validate`] 拒绝，不得把
/// 空白身份写入持久记录。测试与 legacy 单用户回退使用 [`Self::local`]。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityContext {
    /// 组织 / 逻辑租户。
    pub tenant_id: TenantId,
    /// 当前用户或服务账号。
    pub principal_id: PrincipalId,
}

impl IdentityContext {
    /// 未配置 tenant 的本地默认身份（`local/default` / `local/user`）。
    pub fn local() -> Self {
        Self {
            tenant_id: default_tenant(),
            principal_id: default_principal(),
        }
    }

    /// 显式构造身份上下文（供测试与多租户宿主使用）。
    pub fn new(tenant_id: TenantId, principal_id: PrincipalId) -> Self {
        Self {
            tenant_id,
            principal_id,
        }
    }

    /// 显式构造并校验身份上下文，供不可信输入的边界使用。
    pub fn try_new(tenant_id: TenantId, principal_id: PrincipalId) -> Result<Self, IdentityError> {
        let identity = Self::new(tenant_id, principal_id);
        identity.validate()?;
        Ok(identity)
    }

    /// 校验 tenant / principal 均包含非空白字符。
    pub fn validate(&self) -> Result<(), IdentityError> {
        if self.tenant_id.as_str().trim().is_empty() {
            return Err(IdentityError::EmptyTenant);
        }
        if self.principal_id.as_str().trim().is_empty() {
            return Err(IdentityError::EmptyPrincipal);
        }
        Ok(())
    }

    /// 是否落在默认本地身份（`local/default` / `local/user`）。
    pub fn is_local_default(&self) -> bool {
        self.tenant_id.as_str() == DEFAULT_TENANT && self.principal_id.as_str() == DEFAULT_PRINCIPAL
    }
}

impl Default for IdentityContext {
    /// 本地默认身份。
    ///
    /// 注意：生产请求路径禁止用 `Default` 冒充真实身份；只有测试夹具与
    /// legacy 回退（未配置租户的旧单用户部署）才允许依赖此默认值。
    fn default() -> Self {
        Self::local()
    }
}

/// 身份解析错误（fail-closed：任何错误都不得放行）。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    /// 请求没有可解析的主体（缺失身份，fail-closed）。
    #[error("missing identity: {0}")]
    MissingIdentity(String),
    /// 主体标识为空字符串。
    #[error("identity principal is empty")]
    EmptyPrincipal,
    /// 解析出的租户标识为空字符串。
    #[error("identity tenant is empty")]
    EmptyTenant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_identity_matches_frozen_defaults() {
        let identity = IdentityContext::local();
        assert_eq!(identity.tenant_id.as_str(), "local/default");
        assert_eq!(identity.principal_id.as_str(), "local/user");
        assert!(identity.is_local_default());
        assert_eq!(IdentityContext::default(), identity);
        // 显式构造的非默认身份不被误判为本地默认。
        let remote =
            IdentityContext::new(TenantId::new("tenant-a"), PrincipalId::new("principal-a"));
        assert!(!remote.is_local_default());
    }

    #[test]
    fn identity_context_round_trips_through_json() {
        let identity =
            IdentityContext::new(TenantId::new("tenant-a"), PrincipalId::new("principal-a"));
        let json = serde_json::to_string(&identity).expect("serialize");
        let decoded: IdentityContext = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, identity);
    }

    #[test]
    fn identity_context_rejects_blank_tenant_and_principal() {
        assert_eq!(
            IdentityContext::new(TenantId::new(" \t"), PrincipalId::new("local/user")).validate(),
            Err(IdentityError::EmptyTenant)
        );
        assert_eq!(
            IdentityContext::try_new(TenantId::new("local/default"), PrincipalId::new("\n")),
            Err(IdentityError::EmptyPrincipal)
        );
    }
}
