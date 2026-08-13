//! Canonical identity（P18-2）：`IdentityContext` 与身份解析器。
//!
//! `IdentityContext` 是跨模块统一键（tenant + principal），Session / Agent /
//! Usage / Audit 的创建与查询都必须携带它；`TenantId` 表示组织 / 逻辑租户，
//! `PrincipalId` 表示当前用户或服务账号，两者不得由 API key hash 代替
//! （[tenant-audit](../../../docs/features/tenant-audit.md)）。
//!
//! 未配置 tenant 的本地用户固定映射 `tenant_id = local/default`、
//! `principal_id = local/user`（ADR-033），由 [`LocalIdentityResolver`] 提供。
//! **缺失身份 fail-closed**：解析器收到 `None` / 空主体时返回错误，调用方必须
//! 拒绝操作，不得静默落入默认身份。

use serde::{Deserialize, Serialize};

use crate::{default_principal, default_tenant, DEFAULT_PRINCIPAL, DEFAULT_TENANT};

pub use agent_domain::{PrincipalId, TenantId};

/// 一次请求 / 一条持久记录的身份上下文：租户 + 主体。
///
/// 值类型：`Clone + Eq`，可序列化进入 canonical event / 持久化实体；不可变，
/// 创建后不修改。生产路径必须经 [`IdentityResolver`] 解析获得，禁止从默认值
/// 直接构造后冒充真实身份（测试与 legacy 回退除外，见 [`Self::local`]）。
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

/// 身份解析器：把请求携带的主体键解析为 [`IdentityContext`]。
///
/// `principal` 是请求协议层（如 `core_api::ActorIdentity`）映射出的主体键；
/// `None` 表示请求未携带可解析身份，解析器必须返回错误（fail-closed），
/// 不得静默使用默认身份。
pub trait IdentityResolver: Send + Sync {
    /// 解析主体键为身份上下文；缺失 / 空主体返回 [`IdentityError`]。
    fn resolve(&self, principal: Option<&str>) -> Result<IdentityContext, IdentityError>;
}

/// 本地默认解析器：tenant 固定为 `local/default`，principal 保留协议层提供的
/// canonical 主体。`LocalUser` 由协议层映射为 `local/user`，其它已认证主体保持
/// 稳定区分，避免不同调用者共享 usage / session principal。
///
/// 缺失（`None`）或空主体一律拒绝（fail-closed），保证升级后无 tenant 归属
/// 的持久记录不可能从新代码路径产生。
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalIdentityResolver;

impl IdentityResolver for LocalIdentityResolver {
    fn resolve(&self, principal: Option<&str>) -> Result<IdentityContext, IdentityError> {
        match principal {
            None => Err(IdentityError::MissingIdentity(
                "request carries no resolvable principal".into(),
            )),
            Some(principal) if principal.trim().is_empty() => Err(IdentityError::EmptyPrincipal),
            Some(principal) => {
                IdentityContext::try_new(default_tenant(), PrincipalId::new(principal.trim()))
            }
        }
    }
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
    fn local_resolver_preserves_canonical_principal() {
        let resolver = LocalIdentityResolver;
        let identity = resolver.resolve(Some("local/user")).expect("resolve");
        assert!(identity.is_local_default());
        let automation = resolver
            .resolve(Some("automation:scheduler"))
            .expect("resolve");
        assert_eq!(automation.tenant_id.as_str(), "local/default");
        assert_eq!(automation.principal_id.as_str(), "automation:scheduler");
    }

    #[test]
    fn missing_identity_fails_closed() {
        let resolver = LocalIdentityResolver;
        assert_eq!(
            resolver.resolve(None),
            Err(IdentityError::MissingIdentity(
                "request carries no resolvable principal".into()
            ))
        );
        assert_eq!(
            resolver.resolve(Some("")),
            Err(IdentityError::EmptyPrincipal)
        );
        assert_eq!(
            resolver.resolve(Some("   ")),
            Err(IdentityError::EmptyPrincipal)
        );
        // 空 tenant / 空 principal 的上下文本身不可构造成功（类型安全），
        // 此处验证解析器不会产生空值身份。
        assert!(!resolver
            .resolve(Some("x"))
            .expect("resolve")
            .tenant_id
            .as_str()
            .is_empty());
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
