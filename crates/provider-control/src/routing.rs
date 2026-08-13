//! RoutingPolicy 候选排序策略链（P18-6，ADR-033）。
//!
//! 可组合、可解释、可确定性测试的路由策略链：固定过滤链
//! **capability（含 budget 容量）→ 注入的 tenant policy → health → 最高
//! priority bucket** 之后，由 [`RoutingStrategy`]（SingleCandidate / Priority /
//! round robin / smooth weighted round robin / FillFirst）选出候选；每个过滤与回退动作都记录
//! 在 [`RouteStep`] 解释中，决策不含明文 Secret、不按 Provider 名分支。
//!
//! 本模块不接触 Provider 调用、Secret 解析与租户策略实现：策略由宿主注入
//! （[`TenantPolicy`]），健康由 [`HealthView`] 注入（P18-5 [`HealthRuntime`]
//! 提供现成实现）；候选由宿主从账号仓库 / model registry 组装。

use std::collections::BTreeSet;

use agent_domain::{
    AccountId, AgentId, CredentialId, ModelId, PrincipalId, ProviderId, SessionId, TenantId,
    ToolCapabilityTag,
};
use provider_api::{ModelCapabilities, ReasoningStateDescriptor};
use serde::{Deserialize, Serialize};

use crate::health::{CooldownKey, FailureContext, HealthRuntime, HealthState};

/// 路由所需能力（canonical，可自 [`provider_api::ModelCapabilities`] 映射）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Text,
    ImageInput,
    ToolCalls,
    ParallelToolCalls,
    Thinking,
    StructuredOutput,
    PromptCache,
    Citations,
    /// reasoning continuation 维度（signature / encrypted / interleaved / effort）。
    ReasoningContinuation,
    /// Provider 服务端内置工具能力（P15-8 hosted tool tag）。
    HostedTool(ToolCapabilityTag),
}

/// 从 provider-api 的模型能力声明推导 canonical capability 集合。
///
/// 布尔字段 fail-closed（缺失即不支持）；`reasoning` 非默认即声明
/// [`Capability::ReasoningContinuation`]。本函数只做声明映射，不按 Provider 名分支。
pub fn capabilities_of(model: &ModelCapabilities) -> BTreeSet<Capability> {
    let mut set = BTreeSet::new();
    if model.text {
        set.insert(Capability::Text);
    }
    if model.image_input {
        set.insert(Capability::ImageInput);
    }
    if model.tool_calls {
        set.insert(Capability::ToolCalls);
    }
    if model.parallel_tool_calls {
        set.insert(Capability::ParallelToolCalls);
    }
    if model.thinking {
        set.insert(Capability::Thinking);
    }
    if model.structured_output {
        set.insert(Capability::StructuredOutput);
    }
    if model.prompt_cache {
        set.insert(Capability::PromptCache);
    }
    if model.citations {
        set.insert(Capability::Citations);
    }
    if model.reasoning.state != ReasoningStateDescriptor::default()
        || model.reasoning.supports_granular_effort
    {
        set.insert(Capability::ReasoningContinuation);
    }
    for tag in &model.hosted_tool_tags {
        set.insert(Capability::HostedTool(*tag));
    }
    set
}

/// 路由预算：只做容量过滤，不参与选择。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteBudget {
    /// 本次请求预估输入 token；超过候选 context window 即淘汰。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_input_tokens: Option<u64>,
    /// 本次请求所需最大输出 token；超过候选上限即淘汰。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_output_tokens: Option<u64>,
}

/// 一次 route 决策的输入上下文。**不含任何 Secret**。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteContext {
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub model_id: ModelId,
    /// 必须全部满足的能力；空集表示不限制。
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub required_capabilities: BTreeSet<Capability>,
    #[serde(default)]
    pub budget: RouteBudget,
    /// 加权轮询推进序号：相同 `(seed, round)` 必得相同结果；其它策略忽略。
    #[serde(default)]
    pub round: u64,
}

/// 路由候选：已由宿主组装（账号仓库 / model registry），本模块只排序与过滤。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteCandidate {
    pub account_id: AccountId,
    pub credential_id: CredentialId,
    pub provider_id: ProviderId,
    pub model_id: ModelId,
    /// 路由优先级（数字越小越优先）。
    pub priority: u32,
    /// 加权轮询权重（0 = 不参与加权；默认 1）。
    pub weight: u32,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub capabilities: BTreeSet<Capability>,
    pub context_window_tokens: u64,
    pub max_output_tokens: u64,
    /// 当前活跃 lease 数（FillFirst 判定；host 自 CredentialPool 填充）。
    #[serde(default)]
    pub active_leases: u64,
    /// 账号并发上限（P18-4 池准入语义）。
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: u64,
}

fn default_max_concurrency() -> u64 {
    1
}

impl RouteCandidate {
    /// 解释步骤引用的脱敏定位（只有 opaque id，无 Secret）。
    pub fn route_ref(&self) -> CandidateRef {
        CandidateRef {
            account_id: self.account_id.clone(),
            credential_id: self.credential_id.clone(),
            provider_id: self.provider_id.clone(),
            model_id: self.model_id.clone(),
        }
    }
}

/// 候选的 opaque 定位（解释步骤 / 审计用，绝不含明文）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateRef {
    pub account_id: AccountId,
    pub credential_id: CredentialId,
    pub provider_id: ProviderId,
    pub model_id: ModelId,
}

/// 租户策略拒绝结果（只携带脱敏原因）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDenial {
    pub reason: String,
}

/// 注入的租户策略：完整 RBAC 由 P18-9 接线，本任务只定义强制入口。
pub trait TenantPolicy: Send + Sync {
    /// 候选是否被允许参与路由；拒绝理由进入解释步骤。
    fn allows(
        &self,
        context: &RouteContext,
        candidate: &RouteCandidate,
    ) -> Result<(), PolicyDenial>;
}

/// P18-9 完整接线前的 `local/default` 策略：放行全部候选（ADR-033）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalDefaultPolicy;

impl TenantPolicy for LocalDefaultPolicy {
    fn allows(
        &self,
        _context: &RouteContext,
        _candidate: &RouteCandidate,
    ) -> Result<(), PolicyDenial> {
        Ok(())
    }
}

/// 健康过滤的裁决（admissible + 拒绝时的状态，供解释步骤）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthVerdict {
    pub admissible: bool,
    /// 拒绝时的健康状态；admissible 时可留 `None`。
    pub state: Option<HealthState>,
}

/// 注入的健康视图：路由链在此过滤健康候选（P18-5）。
pub trait HealthView: Send + Sync {
    fn verdict(&mut self, candidate: &RouteCandidate) -> HealthVerdict;
}

/// 放行视图：测试 / 健康未接线时使用。
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmitAllHealth;

impl HealthView for AdmitAllHealth {
    fn verdict(&mut self, _candidate: &RouteCandidate) -> HealthVerdict {
        HealthVerdict {
            admissible: true,
            state: None,
        }
    }
}

/// 用 crate 自己的 [`HealthRuntime`]（P18-5）做健康过滤。
///
/// 报告的 state 为账号 / 凭据 / 模型 / Provider 四个 scope 中第一个不可准入者
/// （否则为最差可准入状态），保证解释步骤可定位真正拒绝的 scope。
impl HealthView for HealthRuntime {
    fn verdict(&mut self, candidate: &RouteCandidate) -> HealthVerdict {
        let keys = [
            CooldownKey::account(&candidate.account_id),
            CooldownKey::credential(&candidate.credential_id),
            CooldownKey::model(&candidate.model_id),
            CooldownKey::provider(&candidate.provider_id),
        ];
        let mut worst = HealthState::Healthy;
        for key in keys {
            let state = self.scope_state(&key);
            if !state.is_admissible() {
                return HealthVerdict {
                    admissible: false,
                    state: Some(state),
                };
            }
            if state == HealthState::Degraded {
                worst = HealthState::Degraded;
            }
        }
        let context = FailureContext::new(
            Some(candidate.account_id.clone()),
            Some(candidate.credential_id.clone()),
            Some(candidate.model_id.clone()),
            Some(candidate.provider_id.clone()),
        );
        // 路由规划只能观察健康状态；HalfOpen 探针须由最终 winner 在执行
        // Route → Lease 时通过 `is_admissible` 预留。
        let admissible = self.can_admit(&context);
        HealthVerdict {
            admissible,
            // records 均可用但 circuit 仍 Open / HalfOpen 已满时，用
            // CoolingDown 表达运行时拒绝，绝不生成 HealthRejected(Healthy)。
            state: Some(if admissible {
                worst
            } else {
                HealthState::CoolingDown
            }),
        }
    }
}

/// 显式回退动作种类（ADR-033 回退边界：可审计、可关闭）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackKind {
    /// 同一凭据重试。
    RetrySameCredential,
    /// 换凭据（同账号 / 同 Provider；客户端错误不得默认触发）。
    FailoverCredential,
    /// 换模型（同 Provider）。
    FallbackModel,
    /// 换 Provider。
    FallbackProvider,
    /// 换传输协议（如 ChatCompletions → Responses）。
    FallbackProtocol,
}

impl FallbackKind {
    /// 冻结的持久化字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RetrySameCredential => "retry_same_credential",
            Self::FailoverCredential => "failover_credential",
            Self::FallbackModel => "fallback_model",
            Self::FallbackProvider => "fallback_provider",
            Self::FallbackProtocol => "fallback_protocol",
        }
    }
}

/// 一条可审计的回退动作（种类 + 是否开启）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackAction {
    pub kind: FallbackKind,
    pub allowed: bool,
}

/// 回退计划：显式区分 same credential / credential / model / provider / protocol。
///
/// 默认全部关闭（fail-closed）：ADR-033 要求客户端错误（Cancelled /
/// InvalidRequest / ContextTooLarge / ProtocolIncompatible）不得默认触发
/// credential rotation；实际执行由宿主按 ErrorClassifier 分类决定（P18-9）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackPlan {
    #[serde(default)]
    pub retry_same_credential: bool,
    #[serde(default)]
    pub failover_credential: bool,
    #[serde(default)]
    pub fallback_model: bool,
    #[serde(default)]
    pub fallback_provider: bool,
    #[serde(default)]
    pub fallback_protocol: bool,
}

impl FallbackPlan {
    /// 按 canonical 顺序展开为可审计的动作列表
    /// （same credential → credential → model → provider → protocol）。
    pub fn actions(&self) -> Vec<FallbackAction> {
        vec![
            FallbackAction {
                kind: FallbackKind::RetrySameCredential,
                allowed: self.retry_same_credential,
            },
            FallbackAction {
                kind: FallbackKind::FailoverCredential,
                allowed: self.failover_credential,
            },
            FallbackAction {
                kind: FallbackKind::FallbackModel,
                allowed: self.fallback_model,
            },
            FallbackAction {
                kind: FallbackKind::FallbackProvider,
                allowed: self.fallback_provider,
            },
            FallbackAction {
                kind: FallbackKind::FallbackProtocol,
                allowed: self.fallback_protocol,
            },
        ]
    }

    /// 已开启动作的逗号分隔摘要（解释步骤用）。
    fn summary(&self) -> String {
        let names: Vec<&str> = self
            .actions()
            .iter()
            .filter(|action| action.allowed)
            .map(|action| action.kind.as_str())
            .collect();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(",")
        }
    }
}

/// 候选排序策略。
///
/// 冻结枚举：新增策略属于 schema 演进，必须同步 `app-database` 控制面迁移与
/// `as_db_str` 的持久化字符串映射。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// 单候选：仅有一个账号时使用（legacy 合成默认账号的默认策略）。
    SingleCandidate,
    /// 严格优先级：按配置的账号顺序选取首个健康候选。
    Priority,
    /// 轮询：在最高优先级桶内按确定性 seed 与 round 依次选择。
    RoundRobin,
    /// 加权轮询：按账号权重做加权 round robin。
    WeightedRoundRobin,
    /// 填满优先：优先打满首个账号并发，再启用下一个。
    FillFirst,
    /// 会话亲和：同一 session 尽量复用上一次的账号。
    SessionAffinity,
}

impl RoutingStrategy {
    /// 冻结的持久化字符串（与 `app-database` 控制面 schema 对齐）。
    pub fn as_db_str(self) -> &'static str {
        match self {
            RoutingStrategy::SingleCandidate => "single_candidate",
            RoutingStrategy::Priority => "priority",
            RoutingStrategy::RoundRobin => "round_robin",
            RoutingStrategy::WeightedRoundRobin => "weighted_round_robin",
            RoutingStrategy::FillFirst => "fill_first",
            RoutingStrategy::SessionAffinity => "session_affinity",
        }
    }

    /// 由持久化字符串反解；未知值返回 `None`（由调用方决定降级到 `SingleCandidate`）。
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "single_candidate" => Some(RoutingStrategy::SingleCandidate),
            "priority" => Some(RoutingStrategy::Priority),
            "round_robin" => Some(RoutingStrategy::RoundRobin),
            "weighted_round_robin" => Some(RoutingStrategy::WeightedRoundRobin),
            "fill_first" => Some(RoutingStrategy::FillFirst),
            "session_affinity" => Some(RoutingStrategy::SessionAffinity),
            _ => None,
        }
    }
}

impl Default for RoutingStrategy {
    /// legacy 合成默认账号的默认策略（ADR-033）。
    fn default() -> Self {
        RoutingStrategy::SingleCandidate
    }
}

/// 路由决策错误（记录在 [`RouteDecision::error`]，决策本身始终可解释）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoutingError {
    /// 过滤链后没有可准入候选。
    #[error("no admissible candidate")]
    NoCandidate,
    /// SingleCandidate 要求最高 priority bucket 恰好一个候选。
    #[error("single-candidate strategy requires exactly one candidate in the highest priority bucket; found {found}")]
    TooManyCandidates { found: usize },
    /// 策略未实现（P18-6 覆盖 single / priority / round-robin / weighted / fill-first）。
    #[error("routing strategy {strategy:?} is not implemented (P18-6 covers single/priority/round-robin/weighted/fill-first)")]
    UnsupportedStrategy { strategy: RoutingStrategy },
    /// 加权轮询要求至少一个 weight > 0 的候选。
    #[error("weighted round robin requires at least one candidate with weight > 0")]
    NoWeightedCandidate,
}

/// 过滤 / 选择 / 回退的解释步骤（无 Secret，按管道顺序记录）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteStep {
    /// capability 或 budget 容量不足。
    CapabilityEliminated {
        candidate: CandidateRef,
        capability: Capability,
        detail: String,
    },
    /// 被注入的 tenant policy 拒绝。
    PolicyDenied {
        candidate: CandidateRef,
        reason: String,
    },
    /// 健康过滤拒绝。
    HealthRejected {
        candidate: CandidateRef,
        state: HealthState,
    },
    /// FillFirst：并发已打满。
    ConcurrencyFull {
        candidate: CandidateRef,
        active: u64,
        max: u64,
    },
    /// 加权轮询：weight = 0 不参与。
    WeightZeroExcluded { candidate: CandidateRef },
    /// FillFirst 已命中更靠前且仍有容量的候选，后续候选本轮延后。
    FillFirstDeferred {
        candidate: CandidateRef,
        selected: CandidateRef,
    },
    /// 不在最高 priority bucket。
    BucketExcluded {
        candidate: CandidateRef,
        priority: u32,
        best_priority: u32,
    },
    /// 最终选中。
    Selected {
        candidate: CandidateRef,
        strategy: RoutingStrategy,
    },
    /// 无候选（含原因与回退摘要）。
    NoCandidate { reason: String },
}

/// 选中的路由（完整候选 + 策略 + 轮次，供审计）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedRoute {
    pub candidate: RouteCandidate,
    pub strategy: RoutingStrategy,
    pub round: u64,
}

/// 一次 route 决策：始终可解释（即使未选中）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteDecision {
    pub strategy: RoutingStrategy,
    pub seed: u64,
    pub round: u64,
    pub selected: Option<SelectedRoute>,
    pub error: Option<RoutingError>,
    /// 按管道顺序记录的过滤 / 选择 / 无候选解释。
    pub steps: Vec<RouteStep>,
    /// 回退计划（审计快照）。
    pub fallback: FallbackPlan,
    /// 回退计划的 canonical 展开（可审计每个动作的开关）。
    pub fallback_actions: Vec<FallbackAction>,
}

/// 路由策略配置：固定过滤链 + 选择策略 + 回退计划。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingPolicy {
    #[serde(default)]
    pub strategy: RoutingStrategy,
    /// 加权轮询确定性种子（同 seed 同输入必同序列）。
    #[serde(default)]
    pub seed: u64,
    #[serde(default)]
    pub fallback: FallbackPlan,
}

impl Default for RoutingPolicy {
    /// 旧配置默认：`SingleCandidate` + 全关回退（legacy 合成默认账号，ADR-033）。
    fn default() -> Self {
        Self {
            strategy: RoutingStrategy::SingleCandidate,
            seed: 0,
            fallback: FallbackPlan::default(),
        }
    }
}

impl RoutingPolicy {
    /// 运行固定过滤链（capability → tenant policy → health → 最高 priority
    /// bucket）后按策略选择，返回带完整解释的决策。
    ///
    /// 确定性：同一 `(context, candidates, tenant, health)` 输入必得同一决策；
    /// 加权轮询序列由 `(seed, context.round)` 完全决定，可固定 seed 重放。
    pub fn plan(
        &self,
        context: &RouteContext,
        candidates: &[RouteCandidate],
        tenant: &dyn TenantPolicy,
        health: &mut dyn HealthView,
    ) -> RouteDecision {
        let mut steps = Vec::new();

        // 1) capability（含 budget 容量）。
        let mut capability_ok = Vec::new();
        for candidate in candidates {
            if let Some(missing) = context
                .required_capabilities
                .iter()
                .find(|cap| !candidate.capabilities.contains(cap))
            {
                steps.push(RouteStep::CapabilityEliminated {
                    candidate: candidate.route_ref(),
                    capability: *missing,
                    detail: format!("required capability `{missing:?}` not supported"),
                });
                continue;
            }
            if let Some(required) = context.budget.required_input_tokens {
                if required > candidate.context_window_tokens {
                    steps.push(RouteStep::CapabilityEliminated {
                        candidate: candidate.route_ref(),
                        capability: Capability::Text,
                        detail: format!(
                            "required input tokens {required} exceed context window {}",
                            candidate.context_window_tokens
                        ),
                    });
                    continue;
                }
            }
            if let Some(required) = context.budget.required_output_tokens {
                if required > candidate.max_output_tokens {
                    steps.push(RouteStep::CapabilityEliminated {
                        candidate: candidate.route_ref(),
                        capability: Capability::Text,
                        detail: format!(
                            "required output tokens {required} exceed candidate max {}",
                            candidate.max_output_tokens
                        ),
                    });
                    continue;
                }
            }
            capability_ok.push(candidate.clone());
        }

        // 2) 注入的 tenant policy。
        let mut policy_ok = Vec::new();
        for candidate in capability_ok {
            match tenant.allows(context, &candidate) {
                Ok(()) => policy_ok.push(candidate),
                Err(denial) => steps.push(RouteStep::PolicyDenied {
                    candidate: candidate.route_ref(),
                    reason: denial.reason,
                }),
            }
        }

        // 3) health。
        let mut healthy = Vec::new();
        for candidate in policy_ok {
            let verdict = health.verdict(&candidate);
            if verdict.admissible {
                healthy.push(candidate);
            } else {
                steps.push(RouteStep::HealthRejected {
                    candidate: candidate.route_ref(),
                    state: verdict.state.unwrap_or(HealthState::Disabled),
                });
            }
        }

        // 4) priority bucket（数字最小者优先；FillFirst 按桶顺序在耗尽后下沉）。
        let best_priority = healthy.iter().map(|candidate| candidate.priority).min();
        let mut bucket = Vec::new();
        if let Some(best) = best_priority {
            if self.strategy == RoutingStrategy::FillFirst {
                bucket = healthy;
                // Stable sort preserves configured order inside each priority bucket.
                bucket.sort_by_key(|candidate| candidate.priority);
            } else {
                for candidate in healthy {
                    if candidate.priority == best {
                        bucket.push(candidate);
                    } else {
                        steps.push(RouteStep::BucketExcluded {
                            candidate: candidate.route_ref(),
                            priority: candidate.priority,
                            best_priority: best,
                        });
                    }
                }
            }
        }

        let fallback_actions = self.fallback.actions();
        let mut decision = RouteDecision {
            strategy: self.strategy,
            seed: self.seed,
            round: context.round,
            selected: None,
            error: None,
            steps,
            fallback: self.fallback.clone(),
            fallback_actions,
        };

        let Some(_best) = best_priority else {
            decision.error = Some(RoutingError::NoCandidate);
            decision.steps.push(RouteStep::NoCandidate {
                reason: format!(
                    "no candidate passed capability / tenant policy / health filters; \
                     fallback actions: {}",
                    self.fallback.summary()
                ),
            });
            return decision;
        };

        // 5) 选择策略（在最高 priority bucket 内）。
        match self.strategy {
            RoutingStrategy::SingleCandidate => match bucket.len() {
                0 => {
                    decision.error = Some(RoutingError::NoCandidate);
                    decision.steps.push(RouteStep::NoCandidate {
                        reason: "no candidate in the highest priority bucket".to_string(),
                    });
                }
                1 => select(
                    &mut decision,
                    bucket.pop().expect("len == 1"),
                    context.round,
                ),
                found => {
                    decision.error = Some(RoutingError::TooManyCandidates { found });
                }
            },
            RoutingStrategy::Priority => {
                if let Some(first) = bucket.into_iter().next() {
                    select(&mut decision, first, context.round);
                } else {
                    decision.error = Some(RoutingError::NoCandidate);
                    decision.steps.push(RouteStep::NoCandidate {
                        reason: "no candidate in the highest priority bucket".to_string(),
                    });
                }
            }
            RoutingStrategy::RoundRobin => {
                if let Some(winner) = round_robin_pick(bucket.len(), self.seed, context.round) {
                    select(&mut decision, bucket[winner].clone(), context.round);
                } else {
                    decision.error = Some(RoutingError::NoCandidate);
                    decision.steps.push(RouteStep::NoCandidate {
                        reason: "no candidate in the highest priority bucket".to_string(),
                    });
                }
            }
            RoutingStrategy::WeightedRoundRobin => {
                let mut weighted = Vec::new();
                for candidate in bucket {
                    if candidate.weight == 0 {
                        decision.steps.push(RouteStep::WeightZeroExcluded {
                            candidate: candidate.route_ref(),
                        });
                    } else {
                        weighted.push(candidate);
                    }
                }
                match smooth_weighted_pick(&weighted, self.seed, context.round) {
                    Some(winner) => select(&mut decision, weighted[winner].clone(), context.round),
                    None => {
                        decision.error = Some(RoutingError::NoWeightedCandidate);
                    }
                }
            }
            RoutingStrategy::FillFirst => {
                let mut full = true;
                let mut candidates = bucket.into_iter();
                while let Some(candidate) = candidates.next() {
                    if candidate.active_leases < candidate.max_concurrency {
                        let selected_ref = candidate.route_ref();
                        select(&mut decision, candidate, context.round);
                        for deferred in candidates {
                            decision.steps.push(RouteStep::FillFirstDeferred {
                                candidate: deferred.route_ref(),
                                selected: selected_ref.clone(),
                            });
                        }
                        full = false;
                        break;
                    }
                    decision.steps.push(RouteStep::ConcurrencyFull {
                        candidate: candidate.route_ref(),
                        active: candidate.active_leases,
                        max: candidate.max_concurrency,
                    });
                }
                if full {
                    decision.error = Some(RoutingError::NoCandidate);
                    decision.steps.push(RouteStep::NoCandidate {
                        reason: format!(
                            "all healthy candidates are at max concurrency; \
                             fallback actions: {}",
                            self.fallback.summary()
                        ),
                    });
                }
            }
            RoutingStrategy::SessionAffinity => {
                decision.error = Some(RoutingError::UnsupportedStrategy {
                    strategy: RoutingStrategy::SessionAffinity,
                });
                decision.steps.push(RouteStep::NoCandidate {
                    reason: "session affinity is not implemented in P18-6".to_string(),
                });
            }
        }

        decision
    }
}

/// 普通轮询在 seed 洗牌后的最高优先级桶上按 round 推进。
fn round_robin_pick(candidate_count: usize, seed: u64, round: u64) -> Option<usize> {
    if candidate_count == 0 {
        return None;
    }
    let mut order: Vec<usize> = (0..candidate_count).collect();
    shuffle(&mut order, seed);
    Some(order[(round % candidate_count as u64) as usize])
}

fn select(decision: &mut RouteDecision, candidate: RouteCandidate, round: u64) {
    let route_ref = candidate.route_ref();
    decision.steps.push(RouteStep::Selected {
        candidate: route_ref,
        strategy: decision.strategy,
    });
    decision.selected = Some(SelectedRoute {
        candidate,
        strategy: decision.strategy,
        round,
    });
}

/// smooth weighted round robin 选择器（Nginx 算法，可跨调用推进的状态机）。
///
/// 构造时按 seed 确定性洗牌候选（weight = 0 不参与）；序列由
/// `(候选, seed)` 完全决定，可固定 seed 重放。与 [`RoutingPolicy::plan`] 的
/// 加权分支共享同一 [`smooth_step`] 状态转移，行为一致。
#[derive(Clone, Debug)]
pub struct SmoothWeightedPicker {
    order: Vec<RouteCandidate>,
    weights: Vec<u32>,
    current: Vec<i64>,
    total: i64,
}

impl SmoothWeightedPicker {
    /// 构造：先剔除 weight = 0，再按 seed 确定性洗牌。
    pub fn new(seed: u64, candidates: Vec<RouteCandidate>) -> Self {
        let mut order: Vec<RouteCandidate> = candidates
            .into_iter()
            .filter(|candidate| candidate.weight > 0)
            .collect();
        shuffle(&mut order, seed);
        let weights: Vec<u32> = order.iter().map(|candidate| candidate.weight).collect();
        let total: i64 = weights.iter().map(|&w| i64::from(w)).sum();
        let current = vec![0i64; weights.len()];
        Self {
            order,
            weights,
            current,
            total,
        }
    }

    /// 取下一个候选；候选耗尽时返回 `None`。
    pub fn pick(&mut self) -> Option<RouteCandidate> {
        if self.order.is_empty() {
            return None;
        }
        let winner = smooth_step(&mut self.current, &self.weights, self.total);
        Some(self.order[winner].clone())
    }

    /// 当前剩余的候选数。
    pub fn remaining(&self) -> usize {
        self.order.len()
    }
}

/// SWRR 单步状态转移：`current += weight`，取最大（平局取序小者），胜者减 total。
fn smooth_step(current: &mut [i64], weights: &[u32], total: i64) -> usize {
    for (slot, weight) in current.iter_mut().zip(weights) {
        *slot += i64::from(*weight);
    }
    let winner = (1..current.len()).fold(0, |best, index| {
        if current[index] > current[best] {
            index
        } else {
            best
        }
    });
    current[winner] -= total;
    winner
}

/// SWRR 序列在 `round` 处的胜者（在 seed 洗牌后的顺序上）。
///
/// SWRR 状态以 `total / gcd(weights)` 为周期回到零状态，故先用 round 对周期
/// 取模再推进，避免大 round 的线性模拟（周期性质已由数值验证覆盖）。
fn smooth_weighted_pick(candidates: &[RouteCandidate], seed: u64, round: u64) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    let mut order: Vec<usize> = (0..candidates.len()).collect();
    shuffle(&mut order, seed);
    let weights: Vec<u32> = order.iter().map(|&i| candidates[i].weight).collect();
    let total: u64 = weights.iter().map(|&w| u64::from(w)).sum();
    let mut divisor = total;
    for &w in &weights {
        divisor = gcd(divisor, u64::from(w));
    }
    let period = (total / divisor).max(1);
    let mut current = vec![0i64; weights.len()];
    let mut winner = 0usize;
    for _ in 0..=round % period {
        winner = smooth_step(&mut current, &weights, total as i64);
    }
    Some(order[winner])
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

/// 确定性 Fisher–Yates 洗牌（SplitMix64，与 seed 一一对应）。
fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut rng = SplitMix64::new(seed);
    for index in (1..items.len()).rev() {
        let other = (rng.next_u64() % (index as u64 + 1)) as usize;
        items.swap(index, other);
    }
}

/// 确定性 PRNG（SplitMix64）：无全局状态，可重放。
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use proptest::prelude::*;

    use crate::account::Clock;
    use crate::classifier::{ErrorClassifier, HttpErrorClassifier};
    use crate::health::{BackoffPolicy, CircuitConfig};

    /// 可变时钟：测试推进时间用（与 health 模块测试一致）。
    #[derive(Clone)]
    struct MutableClock(Arc<AtomicU64>);

    impl MutableClock {
        fn new(now_ms: u64) -> Self {
            Self(Arc::new(AtomicU64::new(now_ms)))
        }

        fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::Relaxed);
        }
    }

    impl Clock for MutableClock {
        fn now(&self) -> agent_domain::Timestamp {
            agent_domain::Timestamp::from_unix_millis(self.0.load(Ordering::Relaxed))
        }
    }

    fn context(model: &str) -> RouteContext {
        RouteContext {
            tenant_id: TenantId::new("tenant-1"),
            principal_id: PrincipalId::new("principal-1"),
            session_id: SessionId::new("session-1"),
            agent_id: AgentId::new("agent-1"),
            model_id: ModelId::new(model),
            required_capabilities: BTreeSet::new(),
            budget: RouteBudget::default(),
            round: 0,
        }
    }

    fn candidate(account: &str, credential: &str, priority: u32, weight: u32) -> RouteCandidate {
        RouteCandidate {
            account_id: AccountId::new(account),
            credential_id: CredentialId::new(credential),
            provider_id: ProviderId::new("provider-a"),
            model_id: ModelId::new("model-a"),
            priority,
            weight,
            capabilities: BTreeSet::from([Capability::Text]),
            context_window_tokens: 128_000,
            max_output_tokens: 16_000,
            active_leases: 0,
            max_concurrency: 1,
        }
    }

    fn priority_policy() -> RoutingPolicy {
        RoutingPolicy {
            strategy: RoutingStrategy::Priority,
            ..RoutingPolicy::default()
        }
    }

    /// 按账号名拒绝的健康视图（测试用）。
    struct RejectAccounts(Vec<String>);

    impl HealthView for RejectAccounts {
        fn verdict(&mut self, candidate: &RouteCandidate) -> HealthVerdict {
            let admissible = !self
                .0
                .iter()
                .any(|name| name == candidate.account_id.as_str());
            HealthVerdict {
                admissible,
                state: if admissible {
                    None
                } else {
                    Some(HealthState::BillingBlocked)
                },
            }
        }
    }

    /// 按账号名拒绝的租户策略（测试用）。
    struct DenyAccounts(Vec<String>);

    impl TenantPolicy for DenyAccounts {
        fn allows(
            &self,
            _context: &RouteContext,
            candidate: &RouteCandidate,
        ) -> Result<(), PolicyDenial> {
            if self
                .0
                .iter()
                .any(|name| name == candidate.account_id.as_str())
            {
                Err(PolicyDenial {
                    reason: format!(
                        "account {} excluded by injected policy",
                        candidate.account_id
                    ),
                })
            } else {
                Ok(())
            }
        }
    }

    fn step_mentions(step: &RouteStep, account: &str) -> bool {
        match step {
            RouteStep::CapabilityEliminated { candidate, .. }
            | RouteStep::PolicyDenied { candidate, .. }
            | RouteStep::HealthRejected { candidate, .. }
            | RouteStep::ConcurrencyFull { candidate, .. }
            | RouteStep::WeightZeroExcluded { candidate }
            | RouteStep::FillFirstDeferred { candidate, .. }
            | RouteStep::BucketExcluded { candidate, .. }
            | RouteStep::Selected { candidate, .. } => candidate.account_id.as_str() == account,
            RouteStep::NoCandidate { .. } => false,
        }
    }

    #[test]
    fn default_policy_is_single_candidate_and_selects_sole_candidate() {
        let policy = RoutingPolicy::default();
        assert_eq!(policy.strategy, RoutingStrategy::SingleCandidate);
        let decision = policy.plan(
            &context("m"),
            &[candidate("acct-a", "cred-a", 0, 1)],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        let selected = decision.selected.expect("sole candidate must be selected");
        assert_eq!(selected.candidate.account_id, AccountId::new("acct-a"));
        assert!(decision
            .steps
            .iter()
            .any(|step| matches!(step, RouteStep::Selected { .. })));
    }

    #[test]
    fn single_candidate_rejects_multiple_in_top_bucket() {
        let policy = RoutingPolicy::default();
        let decision = policy.plan(
            &context("m"),
            &[
                candidate("acct-a", "cred-a", 0, 1),
                candidate("acct-b", "cred-b", 0, 1),
            ],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        assert_eq!(
            decision.error,
            Some(RoutingError::TooManyCandidates { found: 2 })
        );
        assert!(decision.selected.is_none());
    }

    #[test]
    fn priority_selects_min_priority_and_explains_bucket_exclusion() {
        let policy = priority_policy();
        let decision = policy.plan(
            &context("m"),
            &[
                candidate("acct-low", "cred-low", 1, 1),
                candidate("acct-hi1", "cred-hi1", 0, 1),
                candidate("acct-hi2", "cred-hi2", 0, 1),
            ],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        let selected = decision.selected.expect("must select from top bucket");
        assert_eq!(selected.candidate.priority, 0);
        assert!(matches!(
            selected.candidate.account_id.as_str(),
            "acct-hi1" | "acct-hi2"
        ));
        assert!(decision.steps.iter().any(|step| matches!(
            step,
            RouteStep::BucketExcluded {
                candidate,
                priority: 1,
                best_priority: 0,
            } if candidate.account_id.as_str() == "acct-low"
        )));
    }

    #[test]
    fn lower_priority_becomes_eligible_when_higher_is_unhealthy() {
        let policy = priority_policy();
        let decision = policy.plan(
            &context("m"),
            &[
                candidate("acct-hi", "cred-hi", 0, 1),
                candidate("acct-lo", "cred-lo", 1, 1),
            ],
            &LocalDefaultPolicy,
            &mut RejectAccounts(vec!["acct-hi".to_string()]),
        );
        let selected = decision.selected.expect("lower priority must be eligible");
        assert_eq!(selected.candidate.account_id, AccountId::new("acct-lo"));
        assert!(decision.steps.iter().any(|step| matches!(
            step,
            RouteStep::HealthRejected {
                candidate,
                state: HealthState::BillingBlocked,
            } if candidate.account_id.as_str() == "acct-hi"
        )));

        let none = policy.plan(
            &context("m"),
            &[
                candidate("acct-hi", "cred-hi", 0, 1),
                candidate("acct-lo", "cred-lo", 1, 1),
            ],
            &LocalDefaultPolicy,
            &mut RejectAccounts(vec!["acct-hi".to_string(), "acct-lo".to_string()]),
        );
        assert_eq!(none.error, Some(RoutingError::NoCandidate));
        assert!(none.selected.is_none());
    }

    #[test]
    fn capability_filter_eliminates_unsupported_candidates() {
        let policy = priority_policy();
        let mut ctx = context("m");
        ctx.required_capabilities = BTreeSet::from([Capability::ToolCalls]);
        let decision = policy.plan(
            &ctx,
            &[candidate("acct-a", "cred-a", 0, 1)],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        assert_eq!(decision.error, Some(RoutingError::NoCandidate));
        assert!(matches!(
            &decision.steps[0],
            RouteStep::CapabilityEliminated {
                capability: Capability::ToolCalls,
                ..
            }
        ));
    }

    #[test]
    fn budget_capacity_eliminates_undersized_candidates() {
        let policy = priority_policy();
        let mut ctx = context("m");
        ctx.budget = RouteBudget {
            required_input_tokens: Some(200_000),
            required_output_tokens: Some(32_000),
        };
        let decision = policy.plan(
            &ctx,
            &[candidate("acct-a", "cred-a", 0, 1)],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        assert_eq!(decision.error, Some(RoutingError::NoCandidate));
        assert!(decision.steps.iter().any(|step| matches!(
            step,
            RouteStep::CapabilityEliminated { detail, .. }
                if detail.contains("200000") || detail.contains("32000")
        )));
    }

    #[test]
    fn tenant_policy_denial_is_recorded_and_denied_never_selected() {
        let policy = priority_policy();
        let decision = policy.plan(
            &context("m"),
            &[
                candidate("acct-a", "cred-a", 0, 1),
                candidate("acct-b", "cred-b", 0, 1),
            ],
            &DenyAccounts(vec!["acct-a".to_string()]),
            &mut AdmitAllHealth,
        );
        let selected = decision.selected.expect("acct-b must be selected");
        assert_eq!(selected.candidate.account_id, AccountId::new("acct-b"));
        assert!(decision.steps.iter().any(|step| matches!(
            step,
            RouteStep::PolicyDenied {
                candidate,
                reason,
            } if candidate.account_id.as_str() == "acct-a" && reason.contains("injected policy")
        )));
    }

    #[test]
    fn plan_integrates_with_health_runtime_cooldown() {
        let clock = Arc::new(MutableClock::new(1_000));
        let mut runtime = HealthRuntime::new(clock.clone());
        let cand = candidate("acct-a", "cred-a", 0, 1);
        let failure = FailureContext::new(
            Some(cand.account_id.clone()),
            Some(cand.credential_id.clone()),
            Some(cand.model_id.clone()),
            Some(cand.provider_id.clone()),
        );
        runtime.record_failure(
            &failure,
            HttpErrorClassifier.classify_http(429, None),
            Some(5_000),
        );

        let policy = priority_policy();
        let rejected = policy.plan(
            &context("m"),
            std::slice::from_ref(&cand),
            &LocalDefaultPolicy,
            &mut runtime,
        );
        assert_eq!(rejected.error, Some(RoutingError::NoCandidate));
        assert!(rejected.steps.iter().any(|step| matches!(
            step,
            RouteStep::HealthRejected {
                candidate,
                state: HealthState::CoolingDown,
            } if candidate.account_id.as_str() == "acct-a"
        )));

        clock.advance(5_000);
        let admitted = policy.plan(&context("m"), &[cand], &LocalDefaultPolicy, &mut runtime);
        assert!(admitted.selected.is_some(), "cooldown 到期后必须恢复准入");
    }

    #[test]
    fn route_planning_does_not_reserve_half_open_probe_slots() {
        let clock = Arc::new(MutableClock::new(1_000));
        let mut runtime = HealthRuntime::with_config(
            clock.clone(),
            BackoffPolicy::default(),
            CircuitConfig {
                failure_threshold: 1,
                open_timeout_ms: 1_000,
                half_open_max_probes: 1,
                success_threshold: 1,
            },
        );
        let cand = candidate("acct-a", "cred-a", 0, 1);
        let failure = FailureContext::new(
            Some(cand.account_id.clone()),
            Some(cand.credential_id.clone()),
            Some(cand.model_id.clone()),
            Some(cand.provider_id.clone()),
        );
        runtime.record_failure(
            &failure,
            HttpErrorClassifier.classify_http(503, None),
            Some(1_000),
        );
        clock.advance(1_000);

        let policy = priority_policy();
        for _ in 0..3 {
            let decision = policy.plan(
                &context("m"),
                std::slice::from_ref(&cand),
                &LocalDefaultPolicy,
                &mut runtime,
            );
            assert!(decision.selected.is_some(), "只读 plan 应持续看见可用探针");
        }

        assert!(runtime.is_admissible(&failure), "winner 应能预留唯一探针");
        assert!(
            !runtime.is_admissible(&failure),
            "唯一 HalfOpen 探针被执行准入预留后才应耗尽"
        );
    }

    #[test]
    fn open_circuit_rejection_is_never_explained_as_healthy() {
        let clock = Arc::new(MutableClock::new(1_000));
        let mut runtime = HealthRuntime::with_config(
            clock.clone(),
            BackoffPolicy::default(),
            CircuitConfig {
                failure_threshold: 1,
                open_timeout_ms: 30_000,
                half_open_max_probes: 1,
                success_threshold: 1,
            },
        );
        let cand = candidate("acct-a", "cred-a", 0, 1);
        let failure = FailureContext::new(
            Some(cand.account_id.clone()),
            Some(cand.credential_id.clone()),
            Some(cand.model_id.clone()),
            Some(cand.provider_id.clone()),
        );
        runtime.record_failure(
            &failure,
            HttpErrorClassifier.classify_http(503, None),
            Some(1_000),
        );
        clock.advance(1_000);

        let decision =
            priority_policy().plan(&context("m"), &[cand], &LocalDefaultPolicy, &mut runtime);
        assert_eq!(decision.error, Some(RoutingError::NoCandidate));
        assert!(decision.steps.iter().any(|step| matches!(
            step,
            RouteStep::HealthRejected {
                state: HealthState::CoolingDown,
                ..
            }
        )));
        assert!(!decision.steps.iter().any(|step| matches!(
            step,
            RouteStep::HealthRejected {
                state: HealthState::Healthy,
                ..
            }
        )));
    }

    #[test]
    fn round_robin_cycles_deterministically_inside_best_priority_bucket() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::RoundRobin,
            seed: 19,
            ..RoutingPolicy::default()
        };
        let candidates = vec![
            candidate("acct-a", "cred-a", 0, 1),
            candidate("acct-b", "cred-b", 0, 1),
            candidate("acct-c", "cred-c", 0, 1),
            candidate("acct-low", "cred-low", 1, 1),
        ];
        let run = || -> Vec<String> {
            (0..6)
                .map(|round| {
                    let mut ctx = context("m");
                    ctx.round = round;
                    policy
                        .plan(&ctx, &candidates, &LocalDefaultPolicy, &mut AdmitAllHealth)
                        .selected
                        .expect("best bucket is nonempty")
                        .candidate
                        .account_id
                        .as_str()
                        .to_string()
                })
                .collect()
        };
        let first = run();
        assert_eq!(first, run(), "同 seed 与输入必须重放同一轮询序列");
        assert_eq!(&first[..3], &first[3..], "一个周期后应回到相同顺序");
        assert!(!first.iter().any(|account| account == "acct-low"));
        assert_eq!(first[..3].iter().collect::<BTreeSet<_>>().len(), 3);
    }

    #[test]
    fn weighted_rr_plan_matches_picker_and_records_selected_steps() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::WeightedRoundRobin,
            seed: 7,
            ..RoutingPolicy::default()
        };
        let candidates = vec![
            candidate("acct-a", "cred-a", 0, 3),
            candidate("acct-b", "cred-b", 0, 1),
        ];
        let mut picker = SmoothWeightedPicker::new(policy.seed, candidates.clone());
        let expected: Vec<String> = (0..12)
            .map(|_| {
                picker
                    .pick()
                    .expect("picker nonempty")
                    .account_id
                    .as_str()
                    .to_string()
            })
            .collect();
        let actual: Vec<String> = (0..12)
            .map(|round| {
                let mut ctx = context("m");
                ctx.round = round;
                policy
                    .plan(&ctx, &candidates, &LocalDefaultPolicy, &mut AdmitAllHealth)
                    .selected
                    .expect("must select")
                    .candidate
                    .account_id
                    .as_str()
                    .to_string()
            })
            .collect();
        assert_eq!(actual, expected, "plan 的加权序列必须与 picker 一致");
    }

    #[test]
    fn weighted_rr_distribution_is_exact_at_cycle_boundaries() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::WeightedRoundRobin,
            seed: 7,
            ..RoutingPolicy::default()
        };
        let candidates = vec![
            candidate("acct-a", "cred-a", 0, 3),
            candidate("acct-b", "cred-b", 0, 1),
        ];
        let mut picker = SmoothWeightedPicker::new(policy.seed, candidates);
        let mut counts = [0u64; 2];
        for _ in 0..4_000 {
            let picked = picker.pick().expect("picker nonempty");
            counts[if picked.account_id.as_str() == "acct-a" {
                0
            } else {
                1
            }] += 1;
        }
        assert_eq!(counts, [3_000, 1_000], "整周期边界必须精确等于权重比");
    }

    #[test]
    fn weighted_rr_same_seed_replays_and_different_seed_diverges() {
        let candidates = vec![
            candidate("acct-a", "cred-a", 0, 2),
            candidate("acct-b", "cred-b", 0, 1),
            candidate("acct-c", "cred-c", 0, 1),
        ];
        let run = |seed: u64| -> Vec<String> {
            let policy = RoutingPolicy {
                strategy: RoutingStrategy::WeightedRoundRobin,
                seed,
                ..RoutingPolicy::default()
            };
            (0..24)
                .map(|round| {
                    let mut ctx = context("m");
                    ctx.round = round;
                    policy
                        .plan(&ctx, &candidates, &LocalDefaultPolicy, &mut AdmitAllHealth)
                        .selected
                        .expect("must select")
                        .candidate
                        .account_id
                        .as_str()
                        .to_string()
                })
                .collect()
        };
        assert_eq!(run(42), run(42), "相同 seed 必须完整重放");
        assert_ne!(run(1), run(2), "不同 seed 应产生不同初始顺序");
    }

    #[test]
    fn weighted_rr_min_weight_candidates_never_repeat_consecutively() {
        for weights in [
            &[3u32, 1][..],
            &[2, 1, 1][..],
            &[5, 3, 2][..],
            &[7, 1, 1, 1][..],
        ] {
            let candidates: Vec<RouteCandidate> = weights
                .iter()
                .enumerate()
                .map(|(index, &weight)| {
                    candidate(
                        &format!("acct-{index}"),
                        &format!("cred-{index}"),
                        0,
                        weight,
                    )
                })
                .collect();
            let mut picker = SmoothWeightedPicker::new(1, candidates);
            let min_weight = *weights.iter().min().expect("nonempty");
            let mut previous: Option<String> = None;
            for _ in 0..2_000 {
                let picked = picker.pick().expect("picker nonempty");
                if picked.weight == min_weight {
                    assert_ne!(
                        Some(picked.account_id.as_str().to_string()),
                        previous,
                        "min-weight 候选不得连续出现（weights {weights:?}）"
                    );
                }
                previous = Some(picked.account_id.as_str().to_string());
            }
        }
    }

    #[test]
    fn weighted_rr_zero_weight_candidates_are_excluded_and_explained() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::WeightedRoundRobin,
            seed: 3,
            ..RoutingPolicy::default()
        };
        let candidates = vec![
            candidate("acct-zero", "cred-zero", 0, 0),
            candidate("acct-active", "cred-active", 0, 2),
        ];
        let decision = policy.plan(
            &context("m"),
            &candidates,
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        let selected = decision
            .selected
            .expect("weighted candidate must be selected");
        assert_eq!(selected.candidate.account_id, AccountId::new("acct-active"));
        assert!(decision.steps.iter().any(|step| matches!(
            step,
            RouteStep::WeightZeroExcluded { candidate }
                if candidate.account_id.as_str() == "acct-zero"
        )));

        let all_zero = policy.plan(
            &context("m"),
            &[candidate("acct-zero", "cred-zero", 0, 0)],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        assert_eq!(all_zero.error, Some(RoutingError::NoWeightedCandidate));
    }

    #[test]
    fn fill_first_fills_capacity_before_moving_on() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::FillFirst,
            ..RoutingPolicy::default()
        };
        let mut first = candidate("acct-a", "cred-a", 0, 1);
        first.max_concurrency = 2;
        first.active_leases = 2;
        let mut second = candidate("acct-b", "cred-b", 0, 1);
        second.max_concurrency = 2;
        second.active_leases = 1;

        let decision = policy.plan(
            &context("m"),
            &[first.clone(), second.clone()],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        let selected = decision.selected.expect("acct-b has capacity");
        assert_eq!(selected.candidate.account_id, AccountId::new("acct-b"));
        assert!(decision.steps.iter().any(|step| matches!(
            step,
            RouteStep::ConcurrencyFull {
                candidate,
                active: 2,
                max: 2,
            } if candidate.account_id.as_str() == "acct-a"
        )));

        let mut full_a = first;
        full_a.active_leases = 2;
        let mut full_b = second;
        full_b.active_leases = 2;
        let exhausted = policy.plan(
            &context("m"),
            &[full_a, full_b],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        assert_eq!(exhausted.error, Some(RoutingError::NoCandidate));
        assert!(exhausted.selected.is_none());
    }

    #[test]
    fn fill_first_descends_only_after_higher_priority_is_exhausted() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::FillFirst,
            ..RoutingPolicy::default()
        };
        let mut high = candidate("acct-high", "cred-high", 0, 1);
        high.max_concurrency = 2;
        let mut low = candidate("acct-low", "cred-low", 1, 1);
        low.max_concurrency = 2;

        let high_available = policy.plan(
            &context("m"),
            &[low.clone(), high.clone()],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        assert_eq!(
            high_available
                .selected
                .expect("high priority has capacity")
                .candidate
                .account_id,
            AccountId::new("acct-high")
        );
        assert!(high_available.steps.iter().any(|step| matches!(
            step,
            RouteStep::FillFirstDeferred {
                candidate,
                selected,
            } if candidate.account_id.as_str() == "acct-low"
                && selected.account_id.as_str() == "acct-high"
        )));

        high.active_leases = high.max_concurrency;
        let descended = policy.plan(
            &context("m"),
            &[low, high],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        assert_eq!(
            descended
                .selected
                .expect("low priority becomes eligible after exhaustion")
                .candidate
                .account_id,
            AccountId::new("acct-low")
        );
        assert!(descended.steps.iter().any(|step| matches!(
            step,
            RouteStep::ConcurrencyFull { candidate, .. }
                if candidate.account_id.as_str() == "acct-high"
        )));
    }

    #[test]
    fn fallback_plan_defaults_fail_closed_and_actions_are_ordered() {
        let plan = FallbackPlan::default();
        assert!(!plan.retry_same_credential && !plan.failover_credential);
        assert!(!plan.fallback_model && !plan.fallback_provider && !plan.fallback_protocol);
        let kinds: Vec<FallbackKind> = plan.actions().iter().map(|action| action.kind).collect();
        assert_eq!(
            kinds,
            vec![
                FallbackKind::RetrySameCredential,
                FallbackKind::FailoverCredential,
                FallbackKind::FallbackModel,
                FallbackKind::FallbackProvider,
                FallbackKind::FallbackProtocol,
            ]
        );
        assert!(plan.actions().iter().all(|action| !action.allowed));

        let enabled = FallbackPlan {
            retry_same_credential: true,
            fallback_provider: true,
            ..FallbackPlan::default()
        };
        let allowed: Vec<&str> = enabled
            .actions()
            .iter()
            .filter(|action| action.allowed)
            .map(|action| action.kind.as_str())
            .collect();
        assert_eq!(allowed, vec!["retry_same_credential", "fallback_provider"]);

        let wire = serde_json::to_string(&enabled).expect("serialize");
        let decoded: FallbackPlan = serde_json::from_str(&wire).expect("decode");
        assert_eq!(decoded, enabled);
    }

    #[test]
    fn policy_serde_missing_fields_default_to_single_candidate() {
        let empty: RoutingPolicy = serde_json::from_str("{}").expect("old config parses");
        assert_eq!(empty, RoutingPolicy::default());

        let priority: RoutingPolicy =
            serde_json::from_str(r#"{"strategy":"priority"}"#).expect("partial config parses");
        assert_eq!(priority.strategy, RoutingStrategy::Priority);
        assert_eq!(priority.seed, 0);
        assert_eq!(priority.fallback, FallbackPlan::default());
    }

    #[test]
    fn session_affinity_reports_unsupported_strategy() {
        let policy = RoutingPolicy {
            strategy: RoutingStrategy::SessionAffinity,
            ..RoutingPolicy::default()
        };
        let decision = policy.plan(
            &context("m"),
            &[candidate("acct-a", "cred-a", 0, 1)],
            &LocalDefaultPolicy,
            &mut AdmitAllHealth,
        );
        assert_eq!(
            decision.error,
            Some(RoutingError::UnsupportedStrategy {
                strategy: RoutingStrategy::SessionAffinity,
            })
        );
        assert!(decision.selected.is_none());
    }

    #[test]
    fn decision_serialization_contains_no_secret_fields() {
        let policy = priority_policy();
        let decision = policy.plan(
            &context("m"),
            &[
                candidate("acct-a", "cred-a", 0, 1),
                candidate("acct-b", "cred-b", 1, 1),
            ],
            &DenyAccounts(vec!["acct-a".to_string()]),
            &mut AdmitAllHealth,
        );
        let value = serde_json::to_value(&decision).expect("serialize decision");
        let mut stack = vec![value];
        while let Some(node) = stack.pop() {
            match node {
                serde_json::Value::Object(map) => {
                    for (key, child) in map {
                        assert!(
                            !matches!(key.as_str(), "secret" | "token" | "api_key" | "password"),
                            "decision must not carry secret fields, found `{key}`"
                        );
                        stack.push(child);
                    }
                }
                serde_json::Value::Array(items) => stack.extend(items),
                _ => {}
            }
        }
    }

    #[test]
    fn capabilities_of_maps_model_capabilities_fail_closed() {
        let model = ModelCapabilities {
            text: true,
            tool_calls: true,
            ..ModelCapabilities::default()
        };
        let set = capabilities_of(&model);
        assert!(set.contains(&Capability::Text));
        assert!(set.contains(&Capability::ToolCalls));
        assert!(!set.contains(&Capability::Thinking));
        assert!(!set.contains(&Capability::ReasoningContinuation));

        let extended = ModelCapabilities {
            thinking: true,
            citations: true,
            ..model
        };
        let set = capabilities_of(&extended);
        assert!(set.contains(&Capability::Thinking));
        assert!(set.contains(&Capability::Citations));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// 属性：Priority 策略绝不选中被过滤（能力 / 策略 / 健康）候选，
        /// 也绝不选中非最高 priority bucket；每个淘汰都有解释步骤。
        #[test]
        fn priority_never_selects_eliminated_or_lower_priority_candidate(
            specs in prop::collection::vec((0u32..5, any::<bool>()), 1..8),
        ) {
            let candidates: Vec<RouteCandidate> = specs
                .iter()
                .enumerate()
                .map(|(index, (priority, _))| {
                    candidate(&format!("acct-{index}"), &format!("cred-{index}"), *priority, 1)
                })
                .collect();
            let rejected: Vec<String> = specs
                .iter()
                .enumerate()
                .filter(|(_, (_, healthy))| !healthy)
                .map(|(index, _)| format!("acct-{index}"))
                .collect();
            let policy = priority_policy();
            let decision = policy.plan(
                &context("m"),
                &candidates,
                &LocalDefaultPolicy,
                &mut RejectAccounts(rejected.clone()),
            );
            let healthy_min = specs
                .iter()
                .filter(|(_, healthy)| *healthy)
                .map(|(priority, _)| *priority)
                .min();
            match healthy_min {
                None => {
                    prop_assert!(decision.selected.is_none());
                    prop_assert_eq!(decision.error, Some(RoutingError::NoCandidate));
                }
                Some(min) => {
                    let selected = decision.selected.as_ref().expect("must select");
                    prop_assert_eq!(selected.candidate.priority, min);
                    prop_assert!(
                        !rejected.contains(&selected.candidate.account_id.as_str().to_string()),
                        "被健康拒绝的候选不得被选中"
                    );
                    for (index, (priority, healthy)) in specs.iter().enumerate() {
                        let account = format!("acct-{index}");
                        if !healthy || *priority != min {
                            prop_assert!(
                                decision.steps.iter().any(|step| step_mentions(step, &account)),
                                "eliminated candidate {account} must have an explanation step"
                            );
                        }
                    }
                }
            }
        }

        /// 属性：SWRR 在整周期边界精确等于权重比（周期 = total / gcd）。
        #[test]
        fn weighted_rr_prop_distribution_is_exact_at_cycle_boundaries(
            weights in prop::collection::vec(1u32..6, 1..4),
        ) {
            let total: u32 = weights.iter().sum();
            let candidates: Vec<RouteCandidate> = weights
                .iter()
                .enumerate()
                .map(|(index, &weight)| {
                    candidate(&format!("acct-{index}"), &format!("cred-{index}"), 0, weight)
                })
                .collect();
            let mut picker = SmoothWeightedPicker::new(9, candidates.clone());
            let cycles = 300u64;
            let mut counts = vec![0u64; weights.len()];
            for _ in 0..(u64::from(total) * cycles) {
                let picked = picker.pick().expect("picker nonempty");
                let index = candidates
                    .iter()
                    .position(|c| c.account_id == picked.account_id)
                    .expect("picked from candidates");
                counts[index] += 1;
            }
            for (index, &weight) in weights.iter().enumerate() {
                prop_assert_eq!(
                    counts[index],
                    u64::from(weight) * cycles,
                    "整周期边界分布必须精确等于权重"
                );
            }
        }

        /// 属性：相同输入 + 相同 seed 的 plan 决策完全重放（无隐藏状态）。
        #[test]
        fn weighted_rr_plan_replays_with_same_seed(
            weights in prop::collection::vec(1u32..6, 1..4),
            seed in any::<u64>(),
        ) {
            let candidates: Vec<RouteCandidate> = weights
                .iter()
                .enumerate()
                .map(|(index, &weight)| {
                    candidate(&format!("acct-{index}"), &format!("cred-{index}"), 0, weight)
                })
                .collect();
            let policy = RoutingPolicy {
                strategy: RoutingStrategy::WeightedRoundRobin,
                seed,
                ..RoutingPolicy::default()
            };
            let run = || -> Vec<RouteDecision> {
                (0..20u64)
                    .map(|round| {
                        let mut ctx = context("m");
                        ctx.round = round;
                        policy.plan(&ctx, &candidates, &LocalDefaultPolicy, &mut AdmitAllHealth)
                    })
                    .collect()
            };
            prop_assert_eq!(run(), run(), "相同 seed 的决策序列必须完全一致");
        }
    }

    #[test]
    fn db_string_round_trip_is_stable() {
        for strategy in [
            RoutingStrategy::SingleCandidate,
            RoutingStrategy::Priority,
            RoutingStrategy::RoundRobin,
            RoutingStrategy::WeightedRoundRobin,
            RoutingStrategy::FillFirst,
            RoutingStrategy::SessionAffinity,
        ] {
            let wire = strategy.as_db_str();
            assert_eq!(RoutingStrategy::from_db_str(wire), Some(strategy));
        }
    }

    #[test]
    fn unknown_db_string_falls_back_to_none() {
        assert_eq!(RoutingStrategy::from_db_str("unknown"), None);
        assert_eq!(RoutingStrategy::default(), RoutingStrategy::SingleCandidate);
    }

    #[test]
    fn serde_wire_values_are_stable_and_unknown_values_fail_closed() {
        for (strategy, wire) in [
            (RoutingStrategy::SingleCandidate, "single_candidate"),
            (RoutingStrategy::Priority, "priority"),
            (RoutingStrategy::RoundRobin, "round_robin"),
            (RoutingStrategy::WeightedRoundRobin, "weighted_round_robin"),
            (RoutingStrategy::FillFirst, "fill_first"),
            (RoutingStrategy::SessionAffinity, "session_affinity"),
        ] {
            let json = serde_json::to_string(&strategy).expect("serialize strategy");
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(
                serde_json::from_str::<RoutingStrategy>(&json).expect("decode strategy"),
                strategy
            );
        }
        assert!(serde_json::from_str::<RoutingStrategy>("\"future_strategy\"").is_err());
    }
}
