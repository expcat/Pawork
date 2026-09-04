//! 租户策略本体：`TenantPolicy` 与 [`TenantPolicyEngine`]（deny-first）。
//!
//! 默认身份为 `local/default` / `local/user`（禁止改成 quota 哨兵 `"local"`）。
//! 约束：无网络、无数据库、无 Secret 访问；同步互斥仅使用 `std::sync::Mutex`，
//! 锁从不跨越 `await` 持有。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::decision::PolicyDecisionEvent;
use crate::identity::{default_tenant, DEFAULT_TENANT};
use crate::rbac::{AuditExportPolicy, Permission, PermissionProfile, PrincipalRole};

pub use pawork_domain::{
    AccountId, AgentId, ModelId, PrincipalId, ProviderId, SessionId, TenantId,
};

/// 租户级策略：并发 / 预算 / 模型白名单。
///
/// 所有字段均为 `Option`，`None` 表示不限制（放行）；`Some` 时按 deny-first 执行。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TenantPolicy {
    /// 同一租户最大并发 agent 数。
    #[serde(default)]
    pub max_concurrent_agents: Option<u64>,
    /// 同一租户最大并发请求数。
    #[serde(default)]
    pub max_concurrent_requests: Option<u64>,
    /// 每日输入 token 预算。
    #[serde(default)]
    pub daily_input_token_budget: Option<u64>,
    /// 每日输出 token 预算。
    #[serde(default)]
    pub daily_output_token_budget: Option<u64>,
    /// 每日成本预算（micros，1e-6 美元）。
    #[serde(default)]
    pub daily_cost_micros_budget: Option<u64>,
    /// 允许使用的模型白名单；`None` 表示不限制，`Some([])` 拒绝全部
    /// （deny-first），`Some(非空)` 仅放行列表内模型。
    #[serde(default)]
    pub allowed_models: Option<Vec<ModelId>>,
    /// 允许使用的 Provider 白名单；`None` 表示不限制，`Some([])` 拒绝全部
    /// （deny-first），`Some(非空)` 仅放行列表内 Provider。
    #[serde(default)]
    pub allowed_providers: Option<Vec<ProviderId>>,
    /// 允许使用的账号白名单；`None` 表示不限制，`Some([])` 拒绝全部
    /// （deny-first），`Some(非空)` 仅放行列表内账号。
    #[serde(default)]
    pub allowed_accounts: Option<Vec<AccountId>>,
    /// 权限配置（角色与 per-principal 覆盖）；`None` 时按默认角色规则：
    /// `local/default` 租户视为 Admin（legacy 单用户兼容），其余未知租户
    /// 取最受限角色 Viewer（deny-first）。
    #[serde(default)]
    pub permission_profile: Option<PermissionProfile>,
    /// 审计/会话保留天数；`None` 表示永久保留。
    #[serde(default)]
    pub retention_days: Option<u64>,
    /// Audit 导出策略；`None` 或未启用一律拒绝导出（deny-first）。
    #[serde(default)]
    pub audit_export: Option<AuditExportPolicy>,
}

/// 租户策略决策结果（与冻结的 `pawork_policy::PolicyDecision` 不是同一类型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TenantPolicyDecision {
    /// 放行。
    Allow,
    /// 拒绝，并附原因。
    Deny { reason: String },
    /// 放行但受约束（如达到并发 / 预算边界），并附原因。
    Limit { reason: String },
    /// 放行但需要回退动作（如换 Provider / 模型），并附原因。
    Fallback { reason: String },
}

/// 并发维度：agent 并发或请求并发。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyKind {
    /// agent 并发。
    Agents,
    /// 请求并发。
    Requests,
}

/// 预算维度：Token 或 Cost。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDimension {
    /// Token 预算（输入 / 输出共用该维度）。
    Tokens,
    /// 成本预算（micros）。
    Cost,
}

/// 租户策略执行错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TenantPolicyError {
    /// 并发限制被超出。
    #[error("并发限制被超出：kind={kind:?} current={current} max={max}")]
    ConcurrencyExceeded {
        /// 并发维度。
        kind: ConcurrencyKind,
        /// 当前值。
        current: u64,
        /// 上限。
        max: u64,
    },
    /// 预算限制被超出。
    #[error("预算限制被超出：dimension={dimension:?} used={used} limit={limit}")]
    BudgetExceeded {
        /// 预算维度。
        dimension: BudgetDimension,
        /// 已使用量。
        used: u64,
        /// 上限。
        limit: u64,
    },
    /// 模型不在允许列表内。
    #[error("模型不在允许列表内：model={model}")]
    ModelNotAllowed {
        /// 模型标识。
        model: String,
    },
    /// Provider 不在允许列表内。
    #[error("Provider 不在允许列表内：provider={provider}")]
    ProviderNotAllowed {
        /// Provider 标识。
        provider: String,
    },
    /// 账号不在允许列表内。
    #[error("账号不在允许列表内：account={account}")]
    AccountNotAllowed {
        /// 账号标识。
        account: String,
    },
    /// 主体角色缺少权限。
    #[error("主体缺少权限：principal={principal} permission={permission:?}")]
    PermissionDenied {
        /// 主体标识。
        principal: String,
        /// 被拒权限。
        permission: Permission,
    },
    /// Audit 导出被拒。
    #[error("Audit 导出被拒：{reason}")]
    AuditExportDenied {
        /// 脱敏原因。
        reason: String,
    },
}

/// 租户策略引擎：提供策略读取与 spawn / acquire / budget 闸口检查。
///
/// 所有 `check_*` 均为异步闸口；`current >= 上限` 时拒绝，`None` 表示不限制。
#[async_trait]
pub trait TenantPolicyEngine: Send + Sync {
    /// 返回指定租户的当前策略；未知租户返回全 `None` 的默认策略。
    fn policy(&self, tenant: &TenantId) -> TenantPolicy;

    /// 检查 agent 并发是否在限制内。
    async fn check_agent_concurrency(
        &self,
        tenant: &TenantId,
        current_active_agents: u64,
    ) -> Result<(), TenantPolicyError>;

    /// 检查请求并发是否在限制内。
    async fn check_request_concurrency(
        &self,
        tenant: &TenantId,
        current_active_requests: u64,
    ) -> Result<(), TenantPolicyError>;

    /// 检查模型是否在允许列表内。
    async fn check_model(
        &self,
        tenant: &TenantId,
        model: &ModelId,
    ) -> Result<(), TenantPolicyError>;

    /// 检查 Provider 是否在允许列表内（deny-first）。
    async fn check_provider(
        &self,
        tenant: &TenantId,
        provider: &ProviderId,
    ) -> Result<(), TenantPolicyError>;

    /// 检查账号是否在允许列表内（deny-first）。
    async fn check_account(
        &self,
        tenant: &TenantId,
        account: &AccountId,
    ) -> Result<(), TenantPolicyError>;

    /// 检查主体角色是否被授权指定权限（deny-first）。
    async fn check_permission(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        permission: Permission,
    ) -> Result<(), TenantPolicyError>;

    /// 检查 Audit 导出是否被允许：角色 + 导出策略 + 目标白名单。
    async fn check_audit_export(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        destination: &str,
    ) -> Result<(), TenantPolicyError>;

    /// 检查每日预算（输入 token / 输出 token / 成本 micros）是否在限制内。
    async fn check_budget(
        &self,
        tenant: &TenantId,
        used_input_tokens: u64,
        used_output_tokens: u64,
        used_cost_micros: u64,
    ) -> Result<(), TenantPolicyError>;

    /// 为指定租户设置策略（覆盖已有策略）。
    fn set_policy(&self, tenant: TenantId, policy: TenantPolicy);

    /// 返回指定租户当前策略版本；未知租户为 0，播种默认策略为 1，
    /// 每次 `set_policy` 递增。
    fn policy_version(&self, tenant: &TenantId) -> u64;

    /// 返回主体在该租户的有效角色（deny-first，见 [`PermissionProfile`]）。
    fn principal_role(&self, tenant: &TenantId, principal: &PrincipalId) -> PrincipalRole;

    /// 记录一条版本化决策事件（reason 构造时统一脱敏）。
    fn record_decision(&self, event: PolicyDecisionEvent);

    /// 返回指定租户已记录的决策事件（审计读取）。
    fn decisions(&self, tenant: &TenantId) -> Vec<PolicyDecisionEvent>;
}

/// 内存版租户策略引擎：`BTreeMap<TenantId, TenantPolicy>` + `Arc<Mutex<_>>`。
///
/// 仅使用 `std::sync::Mutex`，且锁从不跨越 `await` 持有。
pub struct InMemoryTenantPolicyEngine {
    policies: Arc<Mutex<BTreeMap<TenantId, TenantPolicy>>>,
    versions: Arc<Mutex<BTreeMap<TenantId, u64>>>,
    decisions: Arc<Mutex<BTreeMap<TenantId, Vec<PolicyDecisionEvent>>>>,
}

/// 每租户保留的最大决策事件数（防内存无界增长）。
const MAX_DECISIONS_PER_TENANT: usize = 1024;

impl InMemoryTenantPolicyEngine {
    /// 以指定默认策略创建引擎，并将 `DEFAULT_TENANT` 播种为该策略。
    pub fn new(default_policy: TenantPolicy) -> Self {
        let mut policies = BTreeMap::new();
        policies.insert(default_tenant(), default_policy);
        Self {
            policies: Arc::new(Mutex::new(policies)),
            versions: Arc::new(Mutex::new(BTreeMap::from([(default_tenant(), 1)]))),
            decisions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl Default for InMemoryTenantPolicyEngine {
    /// 宽松默认策略：agent 并发 8、请求并发 16，预算与模型白名单不限制。
    fn default() -> Self {
        Self::new(TenantPolicy {
            max_concurrent_agents: Some(8),
            max_concurrent_requests: Some(16),
            ..TenantPolicy::default()
        })
    }
}

#[async_trait]
impl TenantPolicyEngine for InMemoryTenantPolicyEngine {
    fn policy(&self, tenant: &TenantId) -> TenantPolicy {
        self.policies
            .lock()
            .expect("租户策略锁中毒")
            .get(tenant)
            .cloned()
            .unwrap_or_default()
    }

    async fn check_agent_concurrency(
        &self,
        tenant: &TenantId,
        current_active_agents: u64,
    ) -> Result<(), TenantPolicyError> {
        let policy = self.policy(tenant);
        match policy.max_concurrent_agents {
            None => Ok(()),
            Some(max) if current_active_agents < max => Ok(()),
            Some(max) => Err(TenantPolicyError::ConcurrencyExceeded {
                kind: ConcurrencyKind::Agents,
                current: current_active_agents,
                max,
            }),
        }
    }

    async fn check_request_concurrency(
        &self,
        tenant: &TenantId,
        current_active_requests: u64,
    ) -> Result<(), TenantPolicyError> {
        let policy = self.policy(tenant);
        match policy.max_concurrent_requests {
            None => Ok(()),
            Some(max) if current_active_requests < max => Ok(()),
            Some(max) => Err(TenantPolicyError::ConcurrencyExceeded {
                kind: ConcurrencyKind::Requests,
                current: current_active_requests,
                max,
            }),
        }
    }

    async fn check_model(
        &self,
        tenant: &TenantId,
        model: &ModelId,
    ) -> Result<(), TenantPolicyError> {
        let policy = self.policy(tenant);
        match decide_model(model, policy.allowed_models.as_deref()) {
            TenantPolicyDecision::Allow => Ok(()),
            TenantPolicyDecision::Deny { .. }
            | TenantPolicyDecision::Limit { .. }
            | TenantPolicyDecision::Fallback { .. } => Err(TenantPolicyError::ModelNotAllowed {
                model: model.to_string(),
            }),
        }
    }

    async fn check_provider(
        &self,
        tenant: &TenantId,
        provider: &ProviderId,
    ) -> Result<(), TenantPolicyError> {
        let policy = self.policy(tenant);
        match decide_provider(provider, policy.allowed_providers.as_deref()) {
            TenantPolicyDecision::Allow => Ok(()),
            _ => Err(TenantPolicyError::ProviderNotAllowed {
                provider: provider.to_string(),
            }),
        }
    }

    async fn check_account(
        &self,
        tenant: &TenantId,
        account: &AccountId,
    ) -> Result<(), TenantPolicyError> {
        let policy = self.policy(tenant);
        match decide_account(account, policy.allowed_accounts.as_deref()) {
            TenantPolicyDecision::Allow => Ok(()),
            _ => Err(TenantPolicyError::AccountNotAllowed {
                account: account.to_string(),
            }),
        }
    }

    async fn check_permission(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        permission: Permission,
    ) -> Result<(), TenantPolicyError> {
        let role = self.principal_role(tenant, principal);
        match decide_permission(role, permission) {
            TenantPolicyDecision::Allow => Ok(()),
            _ => Err(TenantPolicyError::PermissionDenied {
                principal: principal.to_string(),
                permission,
            }),
        }
    }

    async fn check_audit_export(
        &self,
        tenant: &TenantId,
        principal: &PrincipalId,
        destination: &str,
    ) -> Result<(), TenantPolicyError> {
        let policy = self.policy(tenant);
        match decide_audit_export(
            self.principal_role(tenant, principal),
            policy.audit_export.as_ref(),
            destination,
        ) {
            TenantPolicyDecision::Allow => Ok(()),
            TenantPolicyDecision::Deny { reason } => Err(TenantPolicyError::AuditExportDenied { reason }),
            TenantPolicyDecision::Limit { reason } | TenantPolicyDecision::Fallback { reason } => {
                Err(TenantPolicyError::AuditExportDenied { reason })
            }
        }
    }

    async fn check_budget(
        &self,
        tenant: &TenantId,
        used_input_tokens: u64,
        used_output_tokens: u64,
        used_cost_micros: u64,
    ) -> Result<(), TenantPolicyError> {
        let policy = self.policy(tenant);
        if let Some(limit) = policy.daily_input_token_budget {
            if used_input_tokens >= limit {
                return Err(TenantPolicyError::BudgetExceeded {
                    dimension: BudgetDimension::Tokens,
                    used: used_input_tokens,
                    limit,
                });
            }
        }
        if let Some(limit) = policy.daily_output_token_budget {
            if used_output_tokens >= limit {
                return Err(TenantPolicyError::BudgetExceeded {
                    dimension: BudgetDimension::Tokens,
                    used: used_output_tokens,
                    limit,
                });
            }
        }
        if let Some(limit) = policy.daily_cost_micros_budget {
            if used_cost_micros >= limit {
                return Err(TenantPolicyError::BudgetExceeded {
                    dimension: BudgetDimension::Cost,
                    used: used_cost_micros,
                    limit,
                });
            }
        }
        Ok(())
    }

    fn set_policy(&self, tenant: TenantId, policy: TenantPolicy) {
        tracing::debug!(tenant = %tenant, "更新租户策略");
        let version = {
            let mut versions = self.versions.lock().expect("租户策略版本锁中毒");
            let next = versions.get(&tenant).copied().unwrap_or(0) + 1;
            versions.insert(tenant.clone(), next);
            next
        };
        self.policies
            .lock()
            .expect("租户策略锁中毒")
            .insert(tenant.clone(), policy);
        tracing::debug!(tenant = %tenant, version, "租户策略版本递增");
    }

    fn policy_version(&self, tenant: &TenantId) -> u64 {
        self.versions
            .lock()
            .expect("租户策略版本锁中毒")
            .get(tenant)
            .copied()
            .unwrap_or(0)
    }

    fn principal_role(&self, tenant: &TenantId, principal: &PrincipalId) -> PrincipalRole {
        match self.policy(tenant).permission_profile {
            Some(profile) => profile.effective_role(principal).unwrap_or_default(),
            None if tenant.as_str() == DEFAULT_TENANT => PrincipalRole::Admin,
            // deny-first：未知的非 local/default 租户不得默认 User，
            // 取最受限角色 Viewer。
            None => PrincipalRole::Viewer,
        }
    }

    fn record_decision(&self, event: PolicyDecisionEvent) {
        let mut decisions = self.decisions.lock().expect("决策事件锁中毒");
        let tenant_decisions = decisions.entry(event.tenant_id.clone()).or_default();
        if tenant_decisions.len() >= MAX_DECISIONS_PER_TENANT {
            tenant_decisions.remove(0);
        }
        tenant_decisions.push(event);
    }

    fn decisions(&self, tenant: &TenantId) -> Vec<PolicyDecisionEvent> {
        self.decisions
            .lock()
            .expect("决策事件锁中毒")
            .get(tenant)
            .cloned()
            .unwrap_or_default()
    }
}

/// 决策助手：判断 agent 并发是否放行（`None` 不限制，`current >= max` 拒绝）。
pub fn decide_agent_concurrency(current: u64, max: Option<u64>) -> TenantPolicyDecision {
    match max {
        None => TenantPolicyDecision::Allow,
        Some(max) if current >= max => TenantPolicyDecision::Deny {
            reason: format!("agent 并发已达上限 {max}（当前 {current}）"),
        },
        Some(_) => TenantPolicyDecision::Allow,
    }
}

/// 决策助手：判断请求并发是否放行（`None` 不限制，`current >= max` 拒绝）。
pub fn decide_request_concurrency(current: u64, max: Option<u64>) -> TenantPolicyDecision {
    match max {
        None => TenantPolicyDecision::Allow,
        Some(max) if current >= max => TenantPolicyDecision::Deny {
            reason: format!("请求并发已达上限 {max}（当前 {current}）"),
        },
        Some(_) => TenantPolicyDecision::Allow,
    }
}

/// 决策助手：判断模型是否放行（`None` 不限制；`Some([])` 拒绝全部；
/// 命中列表放行，否则拒绝）。
pub fn decide_model(model: &ModelId, allowed: Option<&[ModelId]>) -> TenantPolicyDecision {
    match allowed {
        None => TenantPolicyDecision::Allow,
        Some([]) => TenantPolicyDecision::Deny {
            reason: "模型白名单为空（拒绝全部）".to_string(),
        },
        Some(list) if list.contains(model) => TenantPolicyDecision::Allow,
        Some(_) => TenantPolicyDecision::Deny {
            reason: format!("模型 {model} 不在允许列表内"),
        },
    }
}

/// 决策助手：判断 Provider 是否放行（`None` 不限制；`Some([])` 拒绝全部；
/// 命中列表放行，否则拒绝）。
pub fn decide_provider(provider: &ProviderId, allowed: Option<&[ProviderId]>) -> TenantPolicyDecision {
    match allowed {
        None => TenantPolicyDecision::Allow,
        Some([]) => TenantPolicyDecision::Deny {
            reason: "Provider 白名单为空（拒绝全部）".to_string(),
        },
        Some(list) if list.contains(provider) => TenantPolicyDecision::Allow,
        Some(_) => TenantPolicyDecision::Deny {
            reason: format!("Provider {provider} 不在允许列表内"),
        },
    }
}

/// 决策助手：判断账号是否放行（`None` 不限制；`Some([])` 拒绝全部；
/// 命中列表放行，否则拒绝）。
pub fn decide_account(account: &AccountId, allowed: Option<&[AccountId]>) -> TenantPolicyDecision {
    match allowed {
        None => TenantPolicyDecision::Allow,
        Some([]) => TenantPolicyDecision::Deny {
            reason: "账号白名单为空（拒绝全部）".to_string(),
        },
        Some(list) if list.contains(account) => TenantPolicyDecision::Allow,
        Some(_) => TenantPolicyDecision::Deny {
            reason: format!("账号 {account} 不在允许列表内"),
        },
    }
}

/// 决策助手：判断角色是否被授权权限（deny-first）。
pub fn decide_permission(role: PrincipalRole, permission: Permission) -> TenantPolicyDecision {
    if role.allows(permission) {
        TenantPolicyDecision::Allow
    } else {
        TenantPolicyDecision::Deny {
            reason: format!("角色 {} 缺少权限 {}", role.as_str(), permission.as_str()),
        }
    }
}

/// 决策助手：判断记录是否仍在保留期内。
///
/// `None` 永久保留；`Some(days)` 且 `age_days > days` 时返回 `Limit`
/// （允许按保留期修剪），否则放行。
pub fn decide_retention(age_days: u64, retention_days: Option<u64>) -> TenantPolicyDecision {
    match retention_days {
        None => TenantPolicyDecision::Allow,
        Some(days) if age_days > days => TenantPolicyDecision::Limit {
            reason: format!("记录年龄 {age_days} 天超过保留期 {days} 天"),
        },
        Some(_) => TenantPolicyDecision::Allow,
    }
}

/// 决策助手：判断 Audit 导出是否放行（角色 + 导出策略 + 目标白名单，
/// 全部 deny-first）。
pub fn decide_audit_export(
    role: PrincipalRole,
    policy: Option<&AuditExportPolicy>,
    destination: &str,
) -> TenantPolicyDecision {
    if let TenantPolicyDecision::Deny { reason } = decide_permission(role, Permission::AuditExport) {
        return TenantPolicyDecision::Deny { reason };
    }
    let policy = match policy {
        Some(policy) if policy.enabled => policy,
        Some(_) => {
            return TenantPolicyDecision::Deny {
                reason: "租户未启用 Audit 导出".to_string(),
            }
        }
        None => {
            return TenantPolicyDecision::Deny {
                reason: "租户未配置 Audit 导出策略".to_string(),
            }
        }
    };
    if policy
        .allowed_destinations
        .iter()
        .any(|allowed| allowed == destination)
    {
        TenantPolicyDecision::Allow
    } else {
        TenantPolicyDecision::Deny {
            reason: format!("导出目标 {destination} 不在允许列表内"),
        }
    }
}

/// 决策助手：判断预算是否放行（任一维度 `used >= limit` 即拒绝——达到
/// 上限同样拒绝，按输入 / 输出 / 成本顺序检查）。
#[allow(clippy::too_many_arguments)]
pub fn decide_budget(
    used_input_tokens: u64,
    used_output_tokens: u64,
    used_cost_micros: u64,
    input_limit: Option<u64>,
    output_limit: Option<u64>,
    cost_limit: Option<u64>,
) -> TenantPolicyDecision {
    if let Some(limit) = input_limit {
        if used_input_tokens >= limit {
            return TenantPolicyDecision::Deny {
                reason: format!("输入 token 预算超限：used={used_input_tokens} limit={limit}"),
            };
        }
    }
    if let Some(limit) = output_limit {
        if used_output_tokens >= limit {
            return TenantPolicyDecision::Deny {
                reason: format!("输出 token 预算超限：used={used_output_tokens} limit={limit}"),
            };
        }
    }
    if let Some(limit) = cost_limit {
        if used_cost_micros >= limit {
            return TenantPolicyDecision::Deny {
                reason: format!("成本预算超限：used={used_cost_micros} limit={limit}"),
            };
        }
    }
    TenantPolicyDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{default_principal, DEFAULT_PRINCIPAL};

    #[test]
    fn default_tenant_is_local_default() {
        assert_eq!(DEFAULT_TENANT, "local/default");
        assert_eq!(DEFAULT_PRINCIPAL, "local/user");
        assert_eq!(default_tenant(), TenantId::new("local/default"));
        assert_eq!(default_principal(), PrincipalId::new("local/user"));
        // 默认引擎已为默认租户播种策略。
        let engine = InMemoryTenantPolicyEngine::default();
        assert_eq!(
            engine.policy(&default_tenant()).max_concurrent_agents,
            Some(8)
        );
    }

    #[tokio::test]
    async fn agent_concurrency_under_limit_allowed() {
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            max_concurrent_agents: Some(2),
            ..TenantPolicy::default()
        });
        assert!(engine
            .check_agent_concurrency(&default_tenant(), 0)
            .await
            .is_ok());
        assert!(engine
            .check_agent_concurrency(&default_tenant(), 1)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn agent_concurrency_over_limit_denied() {
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            max_concurrent_agents: Some(2),
            ..TenantPolicy::default()
        });
        let err = engine
            .check_agent_concurrency(&default_tenant(), 2)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            TenantPolicyError::ConcurrencyExceeded {
                kind: ConcurrencyKind::Agents,
                current: 2,
                max: 2,
            }
        ));
        assert!(engine
            .check_agent_concurrency(&default_tenant(), 3)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn request_concurrency_independent_from_agent_concurrency() {
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            max_concurrent_agents: Some(1),
            max_concurrent_requests: Some(5),
            ..TenantPolicy::default()
        });
        // agent 已达上限被拒，请求并发不受 agent 字段影响。
        assert!(engine
            .check_agent_concurrency(&default_tenant(), 1)
            .await
            .is_err());
        assert!(engine
            .check_request_concurrency(&default_tenant(), 1)
            .await
            .is_ok());
        assert!(engine
            .check_request_concurrency(&default_tenant(), 5)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn model_whitelist_empty_denies_all_but_matching_list_allows() {
        // `Some([])` 拒绝全部（deny-first；`None` 才是不限制）。
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            allowed_models: Some(vec![]),
            ..TenantPolicy::default()
        });
        let err = engine
            .check_model(&default_tenant(), &ModelId::new("gpt-4o"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            TenantPolicyError::ModelNotAllowed { ref model } if model == "gpt-4o"
        ));
        // `None` 才是不限制。
        let unlimited = InMemoryTenantPolicyEngine::new(TenantPolicy {
            allowed_models: None,
            ..TenantPolicy::default()
        });
        assert!(unlimited
            .check_model(&default_tenant(), &ModelId::new("any-model"))
            .await
            .is_ok());

        // 命中列表放行。
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            allowed_models: Some(vec![
                ModelId::new("gpt-4o"),
                ModelId::new("claude-3-5-sonnet"),
            ]),
            ..TenantPolicy::default()
        });
        assert!(engine
            .check_model(&default_tenant(), &ModelId::new("gpt-4o"))
            .await
            .is_ok());
        assert!(engine
            .check_model(&default_tenant(), &ModelId::new("claude-3-5-sonnet"))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn model_denied_when_not_in_allowlist() {
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            allowed_models: Some(vec![ModelId::new("gpt-4o")]),
            ..TenantPolicy::default()
        });
        let err = engine
            .check_model(&default_tenant(), &ModelId::new("deepseek-v3"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            TenantPolicyError::ModelNotAllowed { ref model } if model == "deepseek-v3"
        ));
    }

    #[tokio::test]
    async fn provider_and_account_empty_whitelists_deny_all() {
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            allowed_providers: Some(vec![]),
            allowed_accounts: Some(vec![]),
            ..TenantPolicy::default()
        });
        assert!(engine
            .check_provider(&default_tenant(), &ProviderId::new("openai"))
            .await
            .is_err());
        assert!(engine
            .check_account(&default_tenant(), &AccountId::new("acct-a"))
            .await
            .is_err());
        // `None` 才是不限制。
        let unlimited = InMemoryTenantPolicyEngine::default();
        assert!(unlimited
            .check_provider(&default_tenant(), &ProviderId::new("openai"))
            .await
            .is_ok());
        assert!(unlimited
            .check_account(&default_tenant(), &AccountId::new("acct-a"))
            .await
            .is_ok());
    }

    #[test]
    fn unknown_non_default_tenant_falls_back_to_most_restricted_role() {
        let engine = InMemoryTenantPolicyEngine::default();
        // 未配置的 local/default 保持 Admin（legacy 兼容）。
        assert_eq!(
            engine.principal_role(&default_tenant(), &PrincipalId::new("anyone")),
            PrincipalRole::Admin
        );
        // 未知的非 local/default 租户取最受限 Viewer，绝不默认 User。
        assert_eq!(
            engine.principal_role(&TenantId::new("tenant-a"), &PrincipalId::new("principal-1")),
            PrincipalRole::Viewer
        );
        // 显式 profile 时按 profile 生效（此处 Admin 覆盖 fallback）。
        engine.set_policy(
            TenantId::new("tenant-a"),
            TenantPolicy {
                permission_profile: Some(PermissionProfile {
                    default_role: Some(PrincipalRole::Admin),
                    ..PermissionProfile::default()
                }),
                ..TenantPolicy::default()
            },
        );
        assert_eq!(
            engine.principal_role(&TenantId::new("tenant-a"), &PrincipalId::new("principal-1")),
            PrincipalRole::Admin
        );
    }

    #[tokio::test]
    async fn budget_exceeded_on_cost() {
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            daily_input_token_budget: Some(1000),
            daily_output_token_budget: Some(1000),
            daily_cost_micros_budget: Some(100),
            ..TenantPolicy::default()
        });
        // token 未超、cost 超限 => Cost 维度拒绝。
        let err = engine
            .check_budget(&default_tenant(), 900, 900, 101)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            TenantPolicyError::BudgetExceeded {
                dimension: BudgetDimension::Cost,
                used: 101,
                limit: 100,
            }
        ));
        // used == limit 同样拒绝（deny-first 边界：达到上限即拒绝）。
        let err = engine
            .check_budget(&default_tenant(), 1000, 1000, 100)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            TenantPolicyError::BudgetExceeded {
                dimension: BudgetDimension::Tokens,
                used: 1000,
                limit: 1000,
            }
        ));
        // 各维度边界：used == limit 拒绝，used < limit 放行。
        assert!(engine
            .check_budget(&default_tenant(), 999, 999, 99)
            .await
            .is_ok());
        assert!(engine
            .check_budget(&default_tenant(), 1000, 0, 0)
            .await
            .is_err());
        assert!(engine
            .check_budget(&default_tenant(), 0, 1000, 0)
            .await
            .is_err());
        assert!(engine
            .check_budget(&default_tenant(), 0, 0, 100)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn budget_none_means_unlimited() {
        // 默认引擎预算字段全为 None，任何用量都放行。
        let engine = InMemoryTenantPolicyEngine::default();
        assert!(engine
            .check_budget(&default_tenant(), u64::MAX, u64::MAX, u64::MAX)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn set_policy_overrides_for_tenant() {
        let engine = InMemoryTenantPolicyEngine::default();
        let tenant = TenantId::new("tenant-a");
        engine.set_policy(
            tenant.clone(),
            TenantPolicy {
                max_concurrent_agents: Some(1),
                ..TenantPolicy::default()
            },
        );
        assert!(engine.check_agent_concurrency(&tenant, 1).await.is_err());
        assert!(engine.check_agent_concurrency(&tenant, 0).await.is_ok());
        // 默认租户策略不受影响。
        assert_eq!(
            engine.policy(&default_tenant()).max_concurrent_agents,
            Some(8)
        );
        assert_eq!(engine.policy(&tenant).max_concurrent_agents, Some(1));
    }
}
