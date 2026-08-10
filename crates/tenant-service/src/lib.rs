//! # tenant-service
//!
//! 租户 / 主体身份默认值与 `TenantPolicy` 执行（P18-2 / P18-9 最小契约，供 Phase 12 编排消费）。
//!
//! 本 crate 只做三件事：
//! - 提供租户 / 主体的默认身份（`local/default`、`local/user`）；
//! - 定义 `TenantPolicy`：代理并发、请求并发、每日 Token / Cost 预算、允许模型列表；
//! - 通过 `TenantPolicyEngine` 在 spawn / acquire / budget 闸口强制执行，默认 deny-first。
//!
//! 约束：无网络、无数据库、无 Secret 访问，全部基于内存状态；
//! 同步互斥仅使用 `std::sync::Mutex`，锁从不跨越 `await` 持有。
//! 类型名与字段名保持英文，文档与错误文案使用中文。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use agent_domain::{AgentId, ModelId, PrincipalId, SessionId, TenantId};

/// 默认租户标识字符串。
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
    /// 允许使用的模型白名单；`Some` 且非空时仅放行列表内模型，空列表视为不限制。
    #[serde(default)]
    pub allowed_models: Option<Vec<ModelId>>,
}

/// 策略决策结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyDecision {
    /// 放行。
    Allow,
    /// 拒绝，并附原因。
    Deny { reason: String },
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
}

/// 内存版租户策略引擎：`BTreeMap<TenantId, TenantPolicy>` + `Arc<Mutex<_>>`。
///
/// 仅使用 `std::sync::Mutex`，且锁从不跨越 `await` 持有。
pub struct InMemoryTenantPolicyEngine {
    policies: Arc<Mutex<BTreeMap<TenantId, TenantPolicy>>>,
}

impl InMemoryTenantPolicyEngine {
    /// 以指定默认策略创建引擎，并将 `DEFAULT_TENANT` 播种为该策略。
    pub fn new(default_policy: TenantPolicy) -> Self {
        let mut policies = BTreeMap::new();
        policies.insert(default_tenant(), default_policy);
        Self {
            policies: Arc::new(Mutex::new(policies)),
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
            PolicyDecision::Allow => Ok(()),
            PolicyDecision::Deny { .. } => Err(TenantPolicyError::ModelNotAllowed {
                model: model.to_string(),
            }),
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
            if used_input_tokens > limit {
                return Err(TenantPolicyError::BudgetExceeded {
                    dimension: BudgetDimension::Tokens,
                    used: used_input_tokens,
                    limit,
                });
            }
        }
        if let Some(limit) = policy.daily_output_token_budget {
            if used_output_tokens > limit {
                return Err(TenantPolicyError::BudgetExceeded {
                    dimension: BudgetDimension::Tokens,
                    used: used_output_tokens,
                    limit,
                });
            }
        }
        if let Some(limit) = policy.daily_cost_micros_budget {
            if used_cost_micros > limit {
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
        self.policies
            .lock()
            .expect("租户策略锁中毒")
            .insert(tenant, policy);
    }
}

/// 决策助手：判断 agent 并发是否放行（`None` 不限制，`current >= max` 拒绝）。
pub fn decide_agent_concurrency(current: u64, max: Option<u64>) -> PolicyDecision {
    match max {
        None => PolicyDecision::Allow,
        Some(max) if current >= max => PolicyDecision::Deny {
            reason: format!("agent 并发已达上限 {max}（当前 {current}）"),
        },
        Some(_) => PolicyDecision::Allow,
    }
}

/// 决策助手：判断请求并发是否放行（`None` 不限制，`current >= max` 拒绝）。
pub fn decide_request_concurrency(current: u64, max: Option<u64>) -> PolicyDecision {
    match max {
        None => PolicyDecision::Allow,
        Some(max) if current >= max => PolicyDecision::Deny {
            reason: format!("请求并发已达上限 {max}（当前 {current}）"),
        },
        Some(_) => PolicyDecision::Allow,
    }
}

/// 决策助手：判断模型是否放行（`None` 或空列表不限制，命中列表放行，否则拒绝）。
pub fn decide_model(model: &ModelId, allowed: Option<&[ModelId]>) -> PolicyDecision {
    match allowed {
        None => PolicyDecision::Allow,
        Some([]) => PolicyDecision::Allow,
        Some(list) if list.contains(model) => PolicyDecision::Allow,
        Some(_) => PolicyDecision::Deny {
            reason: format!("模型 {model} 不在允许列表内"),
        },
    }
}

/// 决策助手：判断预算是否放行（任一维度 `used > limit` 即拒绝，按输入 / 输出 / 成本顺序检查）。
#[allow(clippy::too_many_arguments)]
pub fn decide_budget(
    used_input_tokens: u64,
    used_output_tokens: u64,
    used_cost_micros: u64,
    input_limit: Option<u64>,
    output_limit: Option<u64>,
    cost_limit: Option<u64>,
) -> PolicyDecision {
    if let Some(limit) = input_limit {
        if used_input_tokens > limit {
            return PolicyDecision::Deny {
                reason: format!("输入 token 预算超限：used={used_input_tokens} limit={limit}"),
            };
        }
    }
    if let Some(limit) = output_limit {
        if used_output_tokens > limit {
            return PolicyDecision::Deny {
                reason: format!("输出 token 预算超限：used={used_output_tokens} limit={limit}"),
            };
        }
    }
    if let Some(limit) = cost_limit {
        if used_cost_micros > limit {
            return PolicyDecision::Deny {
                reason: format!("成本预算超限：used={used_cost_micros} limit={limit}"),
            };
        }
    }
    PolicyDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn model_allowed_when_list_empty_or_matches() {
        // 空列表视为不限制。
        let engine = InMemoryTenantPolicyEngine::new(TenantPolicy {
            allowed_models: Some(vec![]),
            ..TenantPolicy::default()
        });
        assert!(engine
            .check_model(&default_tenant(), &ModelId::new("gpt-4o"))
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
        // 恰好等于上限仍放行。
        assert!(engine
            .check_budget(&default_tenant(), 1000, 1000, 100)
            .await
            .is_ok());
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
