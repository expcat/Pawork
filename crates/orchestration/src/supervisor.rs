//! AgentSupervisor：spawn / 注册表 / 取消树 / 崩溃恢复（P12-1 + P12-6）。
//!
//! - 所有 worker 都必须经 [`AgentSupervisor::spawn`] 创建，禁止脱离监督的
//!   `tokio::spawn`；
//! - 生命周期全部事件化、可重放（[`crate::OrchestrationEvent`]）；
//! - 取消树：取消 parent 递归联动全部后代，lease 以
//!   [`provider_control::LeaseOutcome::Cancelled`] 幂等释放，**不惩罚账号健康**；
//! - 恢复：重放事件后，任何仍处于活动态且无存活运行时的 worker 一律标记
//!   `Failed`，不留悬挂 worker。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_domain::{AgentId, CancellationToken, ModelId, ProviderId};
use provider_control::{CredentialPool, LeaseGuard, LeaseOutcome};
use tenant_service::{
    IdentityContext, Permission, PolicyDecisionEvent, PolicyDecisionKind, PolicyGate,
    TenantPolicyEngine,
};
#[cfg(test)]
use usage_ledger::{InMemoryUsageLedger, UsageLedgerError, UsageRecord, UsageTotals};
use usage_ledger::{UsageLedger, UsageQuery};

use crate::budget::{
    LedgerContext, WorkerBudgetController, WorkerBudgetLimits, DIM_COST_MICROS, DIM_INPUT_TOKENS,
    DIM_OUTPUT_TOKENS,
};
use crate::identity::{AgentInstance, WorkerRole};
use crate::lifecycle::{
    replay_workers, OrchestrationEvent, WorkerState, WorkerStateMachine, WorkerTransition,
};
use crate::merge::{
    ConflictReport, MergeDecision, MergeOutcome, PatchMerger, PatchProposal, WorkerPatch,
};
use crate::task_graph::{AgentTask, TaskGraph, TaskId, TaskState};
use crate::worktree::{WorktreeAllocator, WorktreeGuard};

/// 注册表中的单个 worker 条目。
pub struct WorkerEntry {
    /// 不可变身份。
    pub instance: AgentInstance,
    /// 生命周期状态机。
    pub state: WorkerStateMachine,
    /// 持有的 credential lease 守卫（未申请时为 `None`）。
    pub lease: Option<LeaseGuard>,
    /// 分配的 worktree 守卫（未分配时为 `None`）。
    pub worktree: Option<WorktreeGuard>,
    /// spawn 请求携带的模型（用于 ledger 归属）。
    pub model: Option<ModelId>,
}

/// Supervisor 配置。
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    /// 本 Supervisor 允许的最大 agent 并发（本地闸门，租户策略之外）。
    pub max_agent_concurrency: u64,
    /// 默认账号侧并发（创建 `CredentialPool` 时的建议值；本 Supervisor 不创建池）。
    pub default_pool_concurrency: u64,
    /// spawn 未显式携带预算时使用的默认预算。
    pub budget: WorkerBudgetLimits,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            max_agent_concurrency: 16,
            default_pool_concurrency: 4,
            budget: WorkerBudgetLimits::default(),
        }
    }
}

/// spawn 请求。
#[derive(Clone, Debug)]
pub struct SpawnRequest {
    /// 租户。
    pub tenant_id: agent_domain::TenantId,
    /// 主体。
    pub principal_id: agent_domain::PrincipalId,
    /// 父代理；`None` 表示创建根（Parent）。
    pub parent_id: Option<AgentId>,
    /// 会话。
    pub session_id: agent_domain::SessionId,
    /// 独立 worktree 路径（可选）。
    pub worktree_path: Option<PathBuf>,
    /// 预算覆盖（`None` 使用 Supervisor 默认预算）。
    pub budget: Option<WorkerBudgetLimits>,
    /// 模型（可选；提供时经租户策略模型白名单闸门）。
    pub model: Option<ModelId>,
    /// 申请 credential lease 的请求（`None` 不申请）。
    pub acquire: Option<provider_control::AcquireRequest>,
    /// 任务依赖（可选；配置 TaskGraph 时注册）。
    pub task_deps: Vec<TaskId>,
    /// 任务描述（可选）。
    pub task_description: Option<String>,
    /// 最大重试次数（可选）。
    pub task_max_retries: Option<u32>,
}

/// 取消树回执。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelTreeReceipt {
    /// 本次实际被取消（进入终态 Cancelled）的 agent 列表。
    pub cancelled_ids: Vec<AgentId>,
    /// 本次实际释放的 lease 数量。
    pub leases_released: u64,
}

/// 恢复报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReport {
    /// 重放后处于活动态、已被标记 Failed 的孤儿 worker。
    pub orphaned: Vec<AgentId>,
    /// 每个已知 worker 恢复后的最终状态（全部为终态）。
    pub recovered_states: BTreeMap<AgentId, WorkerState>,
}

/// Supervisor 错误。
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    /// 未知 agent。
    #[error("unknown agent: {0}")]
    UnknownAgent(AgentId),
    /// 生命周期转换非法。
    #[error("illegal lifecycle transition: {0}")]
    IllegalLifecycle(#[from] crate::lifecycle::LifecycleError),
    /// 策略拒绝。
    #[error("policy denied: {0}")]
    PolicyDenied(String),
    /// credential 池申请失败（错误归一为字符串，不泄漏内部细节）。
    #[error("credential pool acquire failed: {0}")]
    PoolAcquire(String),
    /// lease 相关错误。
    #[error("lease error: {0}")]
    LeaseError(String),
    /// patch 合并错误。
    #[error("merge error: {0}")]
    Merge(String),
    /// worker 已终态，拒绝再记录用量（终态后用量 flush 由 `flush_usage` 重试）。
    #[error("worker terminal, record_usage rejected: {0}")]
    WorkerTerminal(AgentId),
    /// 终态用量 flush 失败，controller 已保留在 budget 表中，可经 `flush_usage` 重试。
    #[error("usage flush pending for terminal worker: {0}")]
    UsageFlushPending(AgentId),
    /// worker 尚未终态，`flush_usage` 拒绝执行（终态后才允许 flush）。
    #[error("worker not terminal, flush_usage rejected: {0}")]
    FlushNotTerminal(AgentId),
    /// 终态 worker 的 flush 状态不一致：controller 存在但归属 ctx 缺失。
    #[error("flush context missing for terminal worker: {0}")]
    FlushContextMissing(AgentId),
    /// cancel_tree 已取消全部节点，但部分终态用量 flush 失败待重试。错误携带
    /// 本次取消结果与待重试 agent 列表，取消本身已完成、不吞 pending。
    #[error("cancel tree completed, usage flush pending for: {pending:?}")]
    CancelTreeFlushPending {
        /// 本次取消的实际结果（节点与 lease 计数）。
        receipt: CancelTreeReceipt,
        /// 终态用量 flush 失败、仍待经 `flush_usage` 重试的 agent 列表。
        pending: Vec<AgentId>,
    },
}

/// 用量 flush 在途标记（RAII）：进入 flush 前登记，结束时（含 future 被
/// drop / 取消）自动清除。仅在同步段操作，不跨 await 持有任何锁。
struct FlushTicket {
    inflight: Arc<Mutex<BTreeSet<AgentId>>>,
    agent_id: AgentId,
}

impl FlushTicket {
    /// 登记 `agent_id` 的在途标记并返回票据。
    fn issue(inflight: &Arc<Mutex<BTreeSet<AgentId>>>, agent_id: &AgentId) -> Self {
        inflight
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(agent_id.clone());
        Self {
            inflight: Arc::clone(inflight),
            agent_id: agent_id.clone(),
        }
    }
}

impl Drop for FlushTicket {
    fn drop(&mut self) {
        self.inflight
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.agent_id);
    }
}

/// 并发预约失败原因（区分全局本地闸门与租户策略闸门，便于审计）。
enum ConcurrencyReservationError {
    /// 全局 agent 并发上限（本地闸门）。
    Global { current: u64, limit: u64 },
    /// 租户 agent 并发上限（策略闸门）。
    Tenant { current: u64, max: u64 },
}

/// spawn 的在途并发槽位预约（RAII）。drop 时从 [`AgentSupervisor::reservations`]
/// 移除预约的 agent_id（幂等：成功兑现路径已移除，drop 再移除为 no-op）。
struct ConcurrencyReservation {
    reservations: Arc<Mutex<BTreeMap<AgentId, agent_domain::TenantId>>>,
    agent_id: AgentId,
}

impl Drop for ConcurrencyReservation {
    fn drop(&mut self) {
        self.reservations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.agent_id);
    }
}

/// 编排 Supervisor：集中拥有 spawn / assign / cancel_tree / 恢复。
pub struct AgentSupervisor {
    workers: Arc<Mutex<BTreeMap<AgentId, WorkerEntry>>>,
    cancel_tokens: Arc<Mutex<BTreeMap<AgentId, CancellationToken>>>,
    children: Arc<Mutex<BTreeMap<AgentId, Vec<AgentId>>>>,
    /// 并发 spawn 的在途槽位预约（agent_id → tenant_id）。与活动 worker 计数
    /// 在单一临界区内合并判定全局 / 租户并发，杜绝 spawn 的 check-then-act
    /// 超配；RAII [`ConcurrencyReservation`] 在 spawn 任一后续步骤失败时归还。
    reservations: Arc<Mutex<BTreeMap<AgentId, agent_domain::TenantId>>>,
    pool: Arc<dyn CredentialPool>,
    policy: Arc<dyn TenantPolicyEngine>,
    ledger: Arc<dyn UsageLedger>,
    event_log: Arc<Mutex<Vec<OrchestrationEvent>>>,
    next_agent_id: AtomicU64,
    budget: Arc<Mutex<BTreeMap<AgentId, WorkerBudgetController>>>,
    config: SupervisorConfig,
    parent_workspace: Option<PathBuf>,
    worktree_allocator: Option<Arc<dyn WorktreeAllocator>>,
    task_graph: Option<Arc<TaskGraph>>,
    patch_merger: Option<Arc<PatchMerger>>,
    pending_patches: Arc<Mutex<BTreeMap<AgentId, PatchProposal>>>,
    /// 终态 flush 失败时缓存的 ledger 归属上下文，供 `flush_usage` 重试复用，
    /// 保证重试不丢失 account / provider / model 归属（lease 已释放后仍可对账）。
    flush_ctx: Arc<Mutex<BTreeMap<AgentId, LedgerContext>>>,
    /// 用量 flush 在途标记（终态路径与 `flush_usage` 重试共用）：flush 在途
    /// 期间并发调用方收到 [`SupervisorError::UsageFlushPending`] 而非假成功。
    flush_in_flight: Arc<Mutex<BTreeSet<AgentId>>>,
}

impl AgentSupervisor {
    /// 以注入的池 / 策略 / 账本与配置构造。
    pub fn new(
        pool: Arc<dyn CredentialPool>,
        policy: Arc<dyn TenantPolicyEngine>,
        ledger: Arc<dyn UsageLedger>,
        config: SupervisorConfig,
    ) -> Self {
        Self {
            workers: Arc::new(Mutex::new(BTreeMap::new())),
            cancel_tokens: Arc::new(Mutex::new(BTreeMap::new())),
            children: Arc::new(Mutex::new(BTreeMap::new())),
            reservations: Arc::new(Mutex::new(BTreeMap::new())),
            pool,
            policy,
            ledger,
            event_log: Arc::new(Mutex::new(Vec::new())),
            next_agent_id: AtomicU64::new(0),
            budget: Arc::new(Mutex::new(BTreeMap::new())),
            config,
            parent_workspace: None,
            worktree_allocator: None,
            task_graph: None,
            patch_merger: None,
            pending_patches: Arc::new(Mutex::new(BTreeMap::new())),
            flush_ctx: Arc::new(Mutex::new(BTreeMap::new())),
            flush_in_flight: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    /// 设置 parent 工作区（worktree 分配与 patch 合并的基准目录）。
    pub fn with_parent_workspace(mut self, path: PathBuf) -> Self {
        self.parent_workspace = Some(path);
        self
    }

    /// 注入 worktree 分配器：spawn 时按需分配隔离 worktree 并持有守卫。
    pub fn with_worktree_allocator(mut self, allocator: Arc<dyn WorktreeAllocator>) -> Self {
        self.worktree_allocator = Some(allocator);
        self
    }

    /// 注入任务依赖图：spawn / complete / fail / cancel 时注册并推进任务，
    /// 发出 Task* 事件。
    pub fn with_task_graph(mut self, graph: Arc<TaskGraph>) -> Self {
        self.task_graph = Some(graph);
        self
    }

    /// 注入 patch 合并器：`propose_patch` / `approve_patch` 使用。
    pub fn with_patch_merger(mut self, merger: Arc<PatchMerger>) -> Self {
        self.patch_merger = Some(merger);
        self
    }

    /// 创建并启动一个 worker。
    ///
    /// 流程：租户 agent 并发与模型白名单闸门（本地并发闸门拒绝时发出
    /// `ConcurrencyDenied`）→ 创建实例 → Admit → 按需分配 worktree（W1，
    /// 失败同 lease 失败处理）→ `WorkerCreated` / `WorkerAdmitted` → 申请
    /// lease（可选）→ Start → `WorkerStarted` → 注册 child 与取消令牌 →
    /// TaskGraph 注册（W2，发出 Task* 事件）→ 注册 worker 条目与预算控制器。
    /// lease / worktree 分配失败时把该 worker 标记 `Failed` 后返回错误，
    /// 保证事件流一致、恢复时不留悬挂 worker。
    pub async fn spawn(&self, req: SpawnRequest) -> Result<AgentId, SupervisorError> {
        // 1. 并发预约（race-free）：与活动 worker 计数在单一临界区内合并判定
        //    全局 / 租户并发，杜绝 check-then-act 超配。预约以 RAII 归还——
        //    spawn 任一后续步骤失败（闸门拒绝 / worktree / lease / 注册）都自动
        //    归还槽位，绝不永久占用额度。agent_id 先于预约生成，使预约、worktree、
        //    lease、注册全程共享同一 canonical 身份。
        let agent_id = AgentId::new(format!(
            "agent-{}",
            self.next_agent_id.fetch_add(1, Ordering::Relaxed)
        ));
        let _reservation = match self.reserve_concurrency(agent_id.clone(), &req.tenant_id) {
            Ok(reservation) => reservation,
            Err(ConcurrencyReservationError::Global { current, limit }) => {
                self.emit(OrchestrationEvent::ConcurrencyDenied {
                    kind: "agents".to_string(),
                    current,
                    limit,
                });
                return Err(SupervisorError::PolicyDenied(format!(
                    "agent concurrency limit reached: active {current} of max {limit}",
                )));
            }
            Err(ConcurrencyReservationError::Tenant { current, max }) => {
                let reason = format!("并发限制被超出：kind=Agents current={current} max={max}");
                self.record_policy_denial(&req, PolicyGate::AgentSpawn, &reason);
                return Err(SupervisorError::PolicyDenied(reason));
            }
        };
        // 2. 策略闸门（P18-9，deny-first，任何一层拒绝都不可被上层覆盖）：
        //    主体角色 AgentSpawn → 模型白名单 → lease 权限与 provider/account
        //    白名单 → 日 token/cost 预算（准入前）。租户 / 全局并发已由 reservation
        //    原子裁决（无 check-then-act 窗口），此处不再重复 concurrency 闸门。
        self.policy
            .check_permission(&req.tenant_id, &req.principal_id, Permission::AgentSpawn)
            .await
            .map_err(|error| {
                self.record_policy_denial(&req, PolicyGate::AgentSpawn, &error);
                SupervisorError::PolicyDenied(error.to_string())
            })?;
        if let Some(model) = &req.model {
            self.policy
                .check_model(&req.tenant_id, model)
                .await
                .map_err(|error| {
                    self.record_policy_denial(&req, PolicyGate::AgentSpawn, &error);
                    SupervisorError::PolicyDenied(error.to_string())
                })?;
        }
        if let Some(acquire) = &req.acquire {
            // P18-9：AcquireRequest 的 tenant / principal / session 必须与
            // SpawnRequest 外层一致，错配一律拒绝（不信任调用方拼接），
            // agent_id 由 supervisor 生成后覆写（见 lease 申请步骤）。
            let mismatched = acquire.tenant_id != req.tenant_id
                || acquire.principal_id != req.principal_id
                || acquire.session_id != req.session_id;
            if mismatched {
                let reason =
                    "AcquireRequest 与 SpawnRequest 的 tenant/principal/session 不一致".to_string();
                self.record_policy_denial(&req, PolicyGate::LeaseAcquire, &reason);
                return Err(SupervisorError::PolicyDenied(reason));
            }
            self.policy
                .check_permission(&req.tenant_id, &req.principal_id, Permission::LeaseAcquire)
                .await
                .map_err(|error| {
                    self.record_policy_denial(&req, PolicyGate::LeaseAcquire, &error);
                    SupervisorError::PolicyDenied(error.to_string())
                })?;
            if let Some(provider) = &acquire.provider_id {
                self.policy
                    .check_provider(&req.tenant_id, provider)
                    .await
                    .map_err(|error| {
                        self.record_policy_denial(&req, PolicyGate::LeaseAcquire, &error);
                        SupervisorError::PolicyDenied(error.to_string())
                    })?;
            }
            if let Some(account) = &acquire.account_id {
                self.policy
                    .check_account(&req.tenant_id, account)
                    .await
                    .map_err(|error| {
                        self.record_policy_denial(&req, PolicyGate::LeaseAcquire, &error);
                        SupervisorError::PolicyDenied(error.to_string())
                    })?;
            }
        }
        // 日 token/cost 预算在准入前执行：仅当租户策略配置了任一预算维度时
        // 才查询账本（fail-closed：账本不可用时拒绝准入，绝不静默放行）。
        let tenant_policy = self.policy.policy(&req.tenant_id);
        let budget_configured = tenant_policy.daily_input_token_budget.is_some()
            || tenant_policy.daily_output_token_budget.is_some()
            || tenant_policy.daily_cost_micros_budget.is_some();
        if budget_configured {
            let day_start = now_ms() - (now_ms() % MS_PER_DAY);
            let daily = UsageQuery {
                tenant_id: Some(req.tenant_id.clone()),
                occurred_at_start_ms: Some(day_start),
                ..UsageQuery::default()
            };
            match self.ledger.aggregate(&daily).await {
                Ok(totals) => {
                    self.policy
                        .check_budget(
                            &req.tenant_id,
                            totals.input_tokens,
                            totals.output_tokens,
                            totals.cost_micros,
                        )
                        .await
                        .map_err(|error| {
                            self.record_policy_denial(&req, PolicyGate::AgentSpawn, &error);
                            SupervisorError::PolicyDenied(error.to_string())
                        })?;
                }
                Err(error) => {
                    let reason = format!("日预算准入前查询账本失败（fail-closed）：{error}");
                    self.record_policy_denial(&req, PolicyGate::AgentSpawn, &reason);
                    return Err(SupervisorError::PolicyDenied(reason));
                }
            }
        }
        // 准入成功：记录 versioned 决策事件（审计事实源）。
        self.record_policy_allow(&req, PolicyGate::AgentSpawn, "agent spawn 准入");
        // 3. 创建实例（worktree_path 初始来自请求，分配后可能被覆盖）。
        let now = now_ms();
        let (role, mut instance) = match &req.parent_id {
            Some(parent) => (
                WorkerRole::Worker,
                AgentInstance::new_worker(
                    agent_id.clone(),
                    req.tenant_id.clone(),
                    req.principal_id.clone(),
                    parent.clone(),
                    req.session_id.clone(),
                    req.worktree_path.clone(),
                    now,
                ),
            ),
            None => (
                WorkerRole::Parent,
                AgentInstance::new_parent(
                    agent_id.clone(),
                    req.tenant_id.clone(),
                    req.principal_id.clone(),
                    req.session_id.clone(),
                    req.worktree_path.clone(),
                    now,
                ),
            ),
        };

        // 4. Admit（admit 折叠进 spawn）。
        let mut machine = WorkerStateMachine::from_state(WorkerState::Created);
        machine
            .apply(WorkerTransition::Admit)
            .map_err(SupervisorError::IllegalLifecycle)?;

        // 5. worktree 分配（W1）：Admit 后、申请 lease 前。
        //    仅当配置了分配器与 parent 工作区、且请求未自带 worktree 路径时
        //    按需分配；失败处理与 lease 失败一致（Failed + WorkerFailed + 注册）。
        let mut worktree_guard = None;
        if let (Some(allocator), Some(parent)) = (&self.worktree_allocator, &self.parent_workspace)
        {
            if req.worktree_path.is_none() {
                match allocator.allocate(parent, agent_id.as_str(), None).await {
                    Ok(worktree) => {
                        instance.worktree_path = Some(worktree.path.clone());
                        worktree_guard = Some(WorktreeGuard::new(worktree, allocator.clone()));
                    }
                    Err(error) => {
                        // 保持事件流一致：标记 Failed 并注册，再返回错误。
                        let _ = machine.apply(WorkerTransition::Fail);
                        self.emit(OrchestrationEvent::WorkerFailed {
                            agent_id: agent_id.clone(),
                            at_ms: now_ms(),
                            reason: error.to_string(),
                        });
                        let entry = WorkerEntry {
                            instance,
                            state: machine,
                            lease: None,
                            worktree: None,
                            model: req.model.clone(),
                        };
                        self.workers
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                            .insert(agent_id.clone(), entry);
                        return Err(SupervisorError::PoolAcquire(error.to_string()));
                    }
                }
            }
        }

        // 6. WorkerCreated（worktree_path 用分配后的真实路径）→ WorkerAdmitted。
        self.emit(OrchestrationEvent::WorkerCreated {
            agent_id: agent_id.clone(),
            tenant_id: req.tenant_id.clone(),
            parent_id: req.parent_id.clone(),
            role,
            session_id: req.session_id.clone(),
            worktree_path: instance
                .worktree_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            created_at_ms: now,
        });
        self.emit(OrchestrationEvent::WorkerAdmitted {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
        });

        // 7. 申请 lease（可选）。
        let lease = match &req.acquire {
            Some(acquire) => {
                // canonical AcquireRequest：agent_id 由 supervisor 用生成的
                // canonical id 覆写（P18-9），tenant/principal/session 同步
                // 取外层请求值（错配已在闸口拒绝），调用方提供的 agent_id
                // 不被信任。
                let mut canonical = acquire.clone();
                canonical.agent_id = agent_id.clone();
                canonical.tenant_id = req.tenant_id.clone();
                canonical.principal_id = req.principal_id.clone();
                canonical.session_id = req.session_id.clone();
                match self.pool.acquire_guard(canonical).await {
                    Ok(guard) => {
                        // P18-9 安全：不信任 pool 返回的 lease 内容。acquire 成功后
                        // 必须校验 lease 的 tenant/principal/session/agent 与本次
                        // spawn 的 canonical 请求一致，以及请求显式指定的
                        // provider/account 一致；任何错配（恶意 / 故障 pool）一律
                        // fail-closed：显式释放该 lease（Released，不惩罚账号健康）、
                        // 释放 worktree、标记 Failed 注册、归还并发预约后返回错误。
                        let lease_view = guard
                            .lease()
                            .expect("acquire_guard 刚返回的 lease 必须存在")
                            .clone();
                        if let Err(reason) =
                            validate_lease_scope(&lease_view, &req, &agent_id, acquire)
                        {
                            // 取走 lease，使 guard 的 Drop 不再触发释放副作用；
                            // 改由显式 release 以 Released 归还额度（恶意 lease 不得
                            // 继续占用账号并发，也不应记为 Failed 影响账号健康）。
                            let lease_id = lease_view.lease_id.clone();
                            let _ = guard.into_lease();
                            if let Err(release_error) =
                                self.pool.release(lease_id, LeaseOutcome::Released).await
                            {
                                tracing::warn!(
                                    %agent_id,
                                    %release_error,
                                    "failed to release lease after scope validation failure"
                                );
                            }
                            if let Some(wt_guard) = worktree_guard.take() {
                                if let Err(release_error) = wt_guard.release().await {
                                    tracing::warn!(
                                        %agent_id,
                                        %release_error,
                                        "failed to release worktree after lease scope failure"
                                    );
                                }
                            }
                            // 保持事件流一致：标记 Failed 并注册，再返回错误。
                            let _ = machine.apply(WorkerTransition::Fail);
                            self.emit(OrchestrationEvent::WorkerFailed {
                                agent_id: agent_id.clone(),
                                at_ms: now_ms(),
                                reason: reason.to_string(),
                            });
                            let entry = WorkerEntry {
                                instance,
                                state: machine,
                                lease: None,
                                worktree: None,
                                model: req.model.clone(),
                            };
                            self.workers
                                .lock()
                                .unwrap_or_else(|poison| poison.into_inner())
                                .insert(agent_id.clone(), entry);
                            let reason_msg = format!("lease scope validation failed: {reason}");
                            self.record_policy_denial(&req, PolicyGate::LeaseAcquire, &reason_msg);
                            return Err(SupervisorError::LeaseError(reason_msg));
                        }
                        Some(guard)
                    }
                    Err(error) => {
                        // 已分配的 worktree 显式释放，避免泄漏。
                        if let Some(guard) = worktree_guard.take() {
                            if let Err(release_error) = guard.release().await {
                                tracing::warn!(
                                    %agent_id,
                                    %release_error,
                                    "failed to release worktree after lease acquire failure"
                                );
                            }
                        }
                        // 保持事件流一致：标记 Failed 并注册，再返回错误。
                        let _ = machine.apply(WorkerTransition::Fail);
                        self.emit(OrchestrationEvent::WorkerFailed {
                            agent_id: agent_id.clone(),
                            at_ms: now_ms(),
                            reason: error.to_string(),
                        });
                        let entry = WorkerEntry {
                            instance,
                            state: machine,
                            lease: None,
                            worktree: None,
                            model: req.model.clone(),
                        };
                        self.workers
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                            .insert(agent_id.clone(), entry);
                        return Err(SupervisorError::PoolAcquire(error.to_string()));
                    }
                }
            }
            None => None,
        };

        // 8. Start → WorkerStarted。
        machine
            .apply(WorkerTransition::Start)
            .map_err(SupervisorError::IllegalLifecycle)?;
        self.emit(OrchestrationEvent::WorkerStarted {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
        });

        // 9. 注册 child 与取消令牌。
        if let Some(parent) = &req.parent_id {
            self.children
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .entry(parent.clone())
                .or_default()
                .push(agent_id.clone());
        }
        self.cancel_tokens
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(agent_id.clone(), CancellationToken::new());

        // 10. TaskGraph 注册（W2）：注册 child/取消令牌后、注册 WorkerEntry 前。
        if let Some(graph) = &self.task_graph {
            let task_id = TaskId::new(agent_id.as_str());
            let task = AgentTask {
                task_id: task_id.clone(),
                tenant_id: req.tenant_id.clone(),
                owner: agent_id.clone(),
                description: req.task_description.clone().unwrap_or_default(),
                depends_on: req.task_deps.clone(),
                retry_count: 0,
                max_retries: req.task_max_retries.unwrap_or(0),
                state: TaskState::Created,
            };
            graph
                .add_task(task)
                .map_err(|error| SupervisorError::PolicyDenied(error.to_string()))?;
            self.emit(OrchestrationEvent::TaskCreated {
                task_id: task_id.clone(),
                agent_id: agent_id.clone(),
                tenant_id: req.tenant_id.clone(),
            });
            // add_task 已按依赖完成度把状态置为 Ready / Blocked；无依赖（或
            // 依赖已全部完成）的任务直接 Ready，发出 TaskReady。
            if graph.state_of(&task_id) == Some(TaskState::Ready) {
                self.emit(OrchestrationEvent::TaskReady {
                    task_id: task_id.clone(),
                });
                // Ready 任务立刻指派并启动；Blocked 任务（依赖未完成）保持
                // Blocked，等待依赖 complete 后由 ready_tasks + mark_ready +
                // 外部 assign/start 推进——不在 spawn 中强制 assign，避免对
                // 合法前向依赖报 IllegalState 而破坏事件流一致性。
                graph
                    .assign(&task_id)
                    .map_err(|error| SupervisorError::PolicyDenied(error.to_string()))?;
                self.emit(OrchestrationEvent::TaskAssigned {
                    task_id: task_id.clone(),
                    agent_id: agent_id.clone(),
                });
                graph
                    .start(&task_id)
                    .map_err(|error| SupervisorError::PolicyDenied(error.to_string()))?;
            }
        }

        // 11. 注册 worker 条目与预算控制器。
        // 原子兑现并发预约：在同一临界区（先 reservations 后 workers，与
        // reserve_concurrency 一致）从 reservations 移除预约并插入活动 worker，
        // 杜绝并发 spawn 观察到「预约已撤但 worker 未注册」的中间态。
        {
            let mut reservations = self
                .reservations
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let mut workers = self
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            reservations.remove(&agent_id);
            workers.insert(
                agent_id.clone(),
                WorkerEntry {
                    instance,
                    state: machine,
                    lease,
                    worktree: worktree_guard,
                    model: req.model.clone(),
                },
            );
        }
        // reservation 的 RAII drop 在函数返回时再 remove 一次（幂等，no-op）。
        let limits = req.budget.unwrap_or_else(|| self.config.budget.clone());
        self.budget
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(agent_id.clone(), WorkerBudgetController::new(limits));

        Ok(agent_id)
    }

    /// Starting → Running，发出 `WorkerRunning`。
    pub async fn start_worker(&self, agent_id: &AgentId) -> Result<(), SupervisorError> {
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let entry = workers
            .get_mut(agent_id)
            .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
        entry
            .state
            .apply(WorkerTransition::BeginRunning)
            .map_err(SupervisorError::IllegalLifecycle)?;
        drop(workers);
        self.emit(OrchestrationEvent::WorkerRunning {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
        });
        Ok(())
    }

    /// 正常完成：释放 lease（`LeaseOutcome::Completed`，幂等）→ Complete →
    /// `WorkerCompleted` → 从父的活跃 children 中移除。同时把该 worker 的
    /// 累计用量 flush 到注入的 usage ledger（无用量时为空操作）。归属从
    /// lease（account / provider）与 spawn 请求（model）取真实值，不再
    /// 硬编码 `"unknown"`；worktree 显式释放；TaskGraph 推进为 Completed。
    pub async fn complete(&self, agent_id: &AgentId) -> Result<(), SupervisorError> {
        let (mut lease, parent, instance, controller, worktree, model, ticket) = {
            let mut workers = self
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let entry = workers
                .get_mut(agent_id)
                .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
            entry
                .state
                .apply(WorkerTransition::Complete)
                .map_err(SupervisorError::IllegalLifecycle)?;
            let instance = entry.instance.clone();
            let model = entry.model.clone();
            let controller = self
                .budget
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(agent_id);
            // 与 controller 移除同步登记 flush 在途标记：终态 flush 期间并发
            // `flush_usage` 收到 `UsageFlushPending`，不会误报成功。
            let ticket = controller
                .is_some()
                .then(|| FlushTicket::issue(&self.flush_in_flight, agent_id));
            (
                entry.lease.take(),
                entry.instance.parent_id.clone(),
                instance,
                controller,
                entry.worktree.take(),
                model,
                ticket,
            )
        };
        // 显式释放 worktree（best-effort）。
        if let Some(guard) = worktree {
            if let Err(error) = guard.release().await {
                tracing::warn!(%agent_id, %error, "failed to release worker worktree on complete");
            }
        }
        // TaskGraph：推进任务为 Completed 并发出 TaskCompleted。
        if let Some(graph) = &self.task_graph {
            let task_id = TaskId::new(agent_id.as_str());
            let _ = graph.complete(&task_id);
            self.emit(OrchestrationEvent::TaskCompleted { task_id });
        }
        // 真实归属：account / provider 取自 lease，model 取自 spawn 请求；
        // 无 lease / 无 model 时回退默认值（保持旧行为）。
        let (account_id, provider_id) = lease
            .as_ref()
            .and_then(|guard| guard.lease())
            .map(|l| (l.account_id.as_str().to_string(), l.provider_id.clone()))
            .unwrap_or_else(|| ("local/default".to_string(), ProviderId::new("local")));
        let model_id = model.unwrap_or_else(|| ModelId::new("unknown"));
        // 读完归属后释放 lease。LeaseGuard 默认 outcome 为 Failed（fail-safe：
        // 未显式标记不得计作成功），因此正常完成必须显式标记 Completed 后再
        // Drop（Drop 触发同步幂等释放）。
        if let Some(guard) = lease.as_mut() {
            *guard.outcome_mut() = LeaseOutcome::Completed;
        }
        drop(lease);
        let flush_outcome = self
            .flush_terminal_usage(
                agent_id,
                &instance,
                account_id,
                provider_id,
                model_id,
                controller,
            )
            .await;
        drop(ticket);
        self.emit(OrchestrationEvent::WorkerCompleted {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
        });
        self.remove_child(parent.as_ref(), agent_id);
        flush_outcome
    }

    /// 失败：释放 lease（`LeaseOutcome::Failed`，计入连续失败）→ Fail →
    /// `WorkerFailed` → 从父的活跃 children 中移除。worktree 显式释放；
    /// TaskGraph 推进为 Failed 并发出 TaskFailed。终态前把累计用量 flush 到
    /// ledger（与 complete 一致）；flush 失败保留 controller 与归属，可经
    /// [`AgentSupervisor::flush_usage`] 重试。
    pub async fn fail(&self, agent_id: &AgentId, reason: String) -> Result<(), SupervisorError> {
        let (lease, parent, worktree, instance, model, controller, ticket) = {
            let mut workers = self
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let entry = workers
                .get_mut(agent_id)
                .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
            entry
                .state
                .apply(WorkerTransition::Fail)
                .map_err(SupervisorError::IllegalLifecycle)?;
            let controller = self
                .budget
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(agent_id);
            // 与 controller 移除同步登记 flush 在途标记（见 complete / flush_usage）。
            let ticket = controller
                .is_some()
                .then(|| FlushTicket::issue(&self.flush_in_flight, agent_id));
            (
                entry.lease.take(),
                entry.instance.parent_id.clone(),
                entry.worktree.take(),
                entry.instance.clone(),
                entry.model.clone(),
                controller,
                ticket,
            )
        };
        if let Some(guard) = worktree {
            if let Err(error) = guard.release().await {
                tracing::warn!(%agent_id, %error, "failed to release worker worktree on fail");
            }
        }
        if let Some(graph) = &self.task_graph {
            let task_id = TaskId::new(agent_id.as_str());
            let _ = graph.fail(&task_id);
            self.emit(OrchestrationEvent::TaskFailed {
                task_id,
                reason: reason.clone(),
            });
        }
        // 真实归属：account / provider 取自 lease（释放前读取），model 取自 spawn 请求。
        let (account_id, provider_id) = lease
            .as_ref()
            .and_then(|guard| guard.lease())
            .map(|l| (l.account_id.as_str().to_string(), l.provider_id.clone()))
            .unwrap_or_else(|| ("local/default".to_string(), ProviderId::new("local")));
        let model_id = model.unwrap_or_else(|| ModelId::new("unknown"));
        if let Some(mut guard) = lease {
            *guard.outcome_mut() = LeaseOutcome::Failed;
            drop(guard);
        }
        let flush_outcome = self
            .flush_terminal_usage(
                agent_id,
                &instance,
                account_id,
                provider_id,
                model_id,
                controller,
            )
            .await;
        drop(ticket);
        self.emit(OrchestrationEvent::WorkerFailed {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
            reason,
        });
        self.remove_child(parent.as_ref(), agent_id);
        flush_outcome
    }

    /// 取消树：取消 `agent_id` 及其全部后代（BFS 遍历 children 图）。
    ///
    /// 每个节点：取消令牌 → `Cancelling`（`WorkerCancelling`）→ `Cancelled`
    /// （`WorkerCancelled`）→ 以 [`LeaseOutcome::Cancelled`] 幂等释放 lease。
    /// worktree 显式释放（best-effort）；TaskGraph 推进为 Cancelled 并发出
    /// `TaskCancelled`。终态节点跳过；重复调用是幂等的（第二次不再取消
    /// 任何节点、不重复释放）。
    ///
    /// 取消总是完成（所有非终态节点进入 `Cancelled`）；若任一节点的终态用量
    /// flush 失败，返回 [`SupervisorError::CancelTreeFlushPending`]——错误携带
    /// 完整 receipt 与待重试的 agent 列表，调用方可经
    /// [`AgentSupervisor::flush_usage`] 逐个重试，不吞掉 pending。
    pub async fn cancel_tree(
        &self,
        agent_id: &AgentId,
    ) -> Result<CancelTreeReceipt, SupervisorError> {
        {
            let workers = self
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if !workers.contains_key(agent_id) {
                return Err(SupervisorError::UnknownAgent(agent_id.clone()));
            }
        }

        let mut queue = vec![agent_id.clone()];
        let mut nodes = Vec::new();
        while let Some(id) = queue.pop() {
            nodes.push(id.clone());
            let kids = self
                .children
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get(&id)
                .cloned()
                .unwrap_or_default();
            queue.extend(kids);
        }

        let mut cancelled_ids = Vec::new();
        let mut leases_released = 0u64;
        let mut flush_pending = Vec::new();
        for id in nodes {
            if let Some(token) = self.cancel_token(&id) {
                token.cancel();
            }
            let (cancelled, lease, worktree, instance, model, controller, ticket) = {
                let mut workers = self
                    .workers
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let Some(entry) = workers.get_mut(&id) else {
                    continue;
                };
                if entry.state.state().is_terminal() {
                    (false, None, None, None, None, None, None)
                } else {
                    let _ = entry.state.apply(WorkerTransition::BeginCancel);
                    let _ = entry.state.apply(WorkerTransition::Cancel);
                    let controller = self
                        .budget
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .remove(&id);
                    // 与 controller 移除同步登记 flush 在途标记（见 flush_usage）。
                    let ticket = controller
                        .is_some()
                        .then(|| FlushTicket::issue(&self.flush_in_flight, &id));
                    (
                        true,
                        entry.lease.take(),
                        entry.worktree.take(),
                        Some(entry.instance.clone()),
                        entry.model.clone(),
                        controller,
                        ticket,
                    )
                }
            };
            if !cancelled {
                continue;
            }
            self.emit(OrchestrationEvent::WorkerCancelling {
                agent_id: id.clone(),
                at_ms: now_ms(),
            });
            self.emit(OrchestrationEvent::WorkerCancelled {
                agent_id: id.clone(),
                at_ms: now_ms(),
            });
            if let Some(guard) = worktree {
                if let Err(error) = guard.release().await {
                    tracing::warn!(%id, %error, "failed to release worktree on cancel");
                }
            }
            if let Some(graph) = &self.task_graph {
                let task_id = TaskId::new(id.as_str());
                let _ = graph.cancel(&task_id);
                self.emit(OrchestrationEvent::TaskCancelled { task_id });
            }
            // 真实归属：account / provider 取自 lease（释放前读取），model 取自 spawn 请求。
            let instance = instance.unwrap();
            let (account_id, provider_id) = lease
                .as_ref()
                .and_then(|guard| guard.lease())
                .map(|l| (l.account_id.as_str().to_string(), l.provider_id.clone()))
                .unwrap_or_else(|| ("local/default".to_string(), ProviderId::new("local")));
            let model_id = model.unwrap_or_else(|| ModelId::new("unknown"));
            if let Some(mut guard) = lease {
                *guard.outcome_mut() = LeaseOutcome::Cancelled;
                // Drop 触发同步幂等释放；Cancelled 只累加取消计数，
                // 不累加连续失败（不惩罚账号健康）。
                drop(guard);
                leases_released += 1;
            }
            // 终态前 flush（与 complete/fail 一致）；失败保留 controller 与归属，
            // 可经 `flush_usage` 重试。取消本身已完成：整体以
            // `CancelTreeFlushPending` 回报（携带 receipt 与待重试 agent 列表），
            // 不再吞掉 pending。
            if self
                .flush_terminal_usage(
                    &id,
                    &instance,
                    account_id,
                    provider_id,
                    model_id,
                    controller,
                )
                .await
                .is_err()
            {
                flush_pending.push(id.clone());
            }
            drop(ticket);
            cancelled_ids.push(id);
        }
        let receipt = CancelTreeReceipt {
            cancelled_ids,
            leases_released,
        };
        if flush_pending.is_empty() {
            Ok(receipt)
        } else {
            Err(SupervisorError::CancelTreeFlushPending {
                receipt,
                pending: flush_pending,
            })
        }
    }

    /// 记录一次用量并检查预算（B1）：对硬超限维度发出 `BudgetExceeded`。
    ///
    /// 用量经该 worker 的 [`WorkerBudgetController`] 累加；`check()` 报告中
    /// 「新进入硬超限且尚未发出过事件」的维度经 `diff_hard_exceeded` 去重后
    /// 以当前用量与对应上限发出一个 `BudgetExceeded` 事件（同一维度持续
    /// 超限只告警一次；用量回落到上限以下后该维度被「忘记」，恢复后可再告警）。
    ///
    /// worker 进入终态（Completed / Cancelled / Failed）后拒绝再记录用量：
    /// 终态用量已由 `complete` / `fail` / `cancel_tree` flush 到 ledger，
    /// 此后新增 record 会破坏「终态后不再变更用量」的不变式，因此返回
    /// [`SupervisorError::WorkerTerminal`]。终态 flush 若失败保留了 controller，
    /// 调用方应经 [`AgentSupervisor::flush_usage`] 重试。
    pub async fn record_usage(
        &self,
        agent_id: &AgentId,
        input: u64,
        output: u64,
        cost_micros: u64,
    ) -> Result<(), SupervisorError> {
        // 终态拒绝：worker 已终态时不得再累加用量（避免与终态 flush 竞争/重复）。
        let terminal = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(agent_id)
            .map(|entry| entry.state.state().is_terminal())
            .unwrap_or(false);
        if terminal {
            return Err(SupervisorError::WorkerTerminal(agent_id.clone()));
        }
        // 直接累加进注册表内的控制器（克隆会丢失写入）。
        let mut controllers = self
            .budget
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let controller = controllers
            .get_mut(agent_id)
            .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
        controller.record_tokens(input, output);
        controller.record_cost(cost_micros);
        let report = controller.check();
        // 持续超限去重：仅对「新进入硬超限」的维度发事件；恢复后可再告警。
        let newly_exceeded = controller.diff_hard_exceeded(&report);
        let (used_input, used_output, used_cost) = controller.usage();
        let limits = controller.limits().clone();
        for dimension in &newly_exceeded {
            let (used, limit) = match dimension.as_str() {
                DIM_INPUT_TOKENS => (used_input, limits.max_input_tokens.unwrap_or(0)),
                DIM_OUTPUT_TOKENS => (used_output, limits.max_output_tokens.unwrap_or(0)),
                DIM_COST_MICROS => (used_cost, limits.max_cost_micros.unwrap_or(0)),
                _ => continue,
            };
            self.emit(OrchestrationEvent::BudgetExceeded {
                agent_id: agent_id.clone(),
                dimension: dimension.clone(),
                used,
                limit,
            });
        }
        Ok(())
    }

    /// 显式重试终态 worker 的用量 flush。
    ///
    /// 仅允许终态 worker：活动 worker 误调用返回 [`SupervisorError::FlushNotTerminal`]，
    /// 且不会移除 / 丢弃其 controller。controller 与归属 ctx 必须成对存在：
    /// 不一致（controller 在而 ctx 缺失）时保留 controller 并返回
    /// [`SupervisorError::FlushContextMissing`]，不吞 pending。
    ///
    /// 并发安全：认领（校验在途标记、移除 controller / ctx）在同一临界区完成，
    /// 认领成功后登记在途标记；flush 在途期间其他调用方收到
    /// [`SupervisorError::UsageFlushPending`] 而非假成功。提交成功后才丢弃
    /// controller / ctx；失败时原样放回（放回先于在途标记清除），可重试。
    /// 账本写入由 controller 内部提交游标串行化，重试按相同 record 幂等重放，
    /// 不重复计账。controller 不存在时为空操作（无用量或已 flush）。
    pub async fn flush_usage(&self, agent_id: &AgentId) -> Result<(), SupervisorError> {
        // 仅允许终态：活动 worker 误调用直接拒绝，不触碰 budget / flush_ctx。
        let terminal = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(agent_id)
            .map(|entry| entry.state.state().is_terminal())
            .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
        if !terminal {
            return Err(SupervisorError::FlushNotTerminal(agent_id.clone()));
        }
        // 原子认领：controller 与 ctx 必须成对存在；认领成功即登记在途标记，
        // 并发 flush（终态路径或其他 flush_usage）期间本调用返回
        // `UsageFlushPending`，避免假成功。
        let (controller, ctx, _ticket) = {
            let mut budget = self
                .budget
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let mut flush_ctx = self
                .flush_ctx
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let mut inflight = self
                .flush_in_flight
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if inflight.contains(agent_id) {
                return Err(SupervisorError::UsageFlushPending(agent_id.clone()));
            }
            let controller = budget.remove(agent_id);
            let ctx = flush_ctx.remove(agent_id);
            match (controller, ctx) {
                (None, None) => (None, None, None),
                (Some(controller), Some(ctx)) => {
                    inflight.insert(agent_id.clone());
                    (
                        Some(controller),
                        Some(ctx),
                        Some(FlushTicket {
                            inflight: Arc::clone(&self.flush_in_flight),
                            agent_id: agent_id.clone(),
                        }),
                    )
                }
                (Some(controller), None) => {
                    // 不一致：controller 在而 ctx 缺失 → 保留 controller，不吞 pending。
                    budget.insert(agent_id.clone(), controller);
                    return Err(SupervisorError::FlushContextMissing(agent_id.clone()));
                }
                (None, Some(_ctx)) => (None, None, None),
            }
        };
        let Some(controller) = controller else {
            // 无可 flush：已提交或从未有 pending（残留 ctx 已随认领丢弃）。
            return Ok(());
        };
        let ctx = ctx.expect("controller 与 ctx 成对认领");
        match controller.flush_to_ledger(self.ledger.as_ref(), &ctx).await {
            Ok(()) => Ok(()),
            Err(error) => {
                // 失败：controller 与 ctx 放回表内（在途标记由票据 Drop 清除，
                // 且放回先于票据清除完成），等待下一次重试。
                self.budget
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(agent_id.clone(), controller);
                self.flush_ctx
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(agent_id.clone(), ctx);
                tracing::warn!(
                    %agent_id, %error,
                    "retry flush failed; controller and ctx retained"
                );
                Err(SupervisorError::UsageFlushPending(agent_id.clone()))
            }
        }
    }

    /// 终态 flush：把 controller 的累计用量写入 ledger。成功返回 `Ok`
    /// （controller 被消费）；失败时把 controller 与归属 ctx 放回 budget /
    /// flush_ctx 表，返回 [`SupervisorError::UsageFlushPending`]，调用方可经
    /// [`AgentSupervisor::flush_usage`] 重试。终态转换本身已完成，flush 失败
    /// 不回滚生命周期，仅保留用量可重试状态。
    ///
    /// 注：`std::sync::Mutex` 仅在构造 ctx 时短暂持有并立即 drop，不跨 await；
    /// ledger 调用期间不持有任何 `std::sync::Mutex`（保持无锁跨 await）。
    async fn flush_terminal_usage(
        &self,
        agent_id: &AgentId,
        instance: &AgentInstance,
        account_id: String,
        provider_id: ProviderId,
        model_id: ModelId,
        controller: Option<WorkerBudgetController>,
    ) -> Result<(), SupervisorError> {
        let Some(controller) = controller else {
            return Ok(());
        };
        let ctx = LedgerContext {
            credential_id: None,
            tenant_id: instance.tenant_id.clone(),
            principal_id: instance.principal_id.clone(),
            account_id,
            session_id: instance.session_id.clone(),
            agent_id: instance.agent_id.clone(),
            run_id: None,
            provider_id,
            model_id,
        };
        match controller.flush_to_ledger(self.ledger.as_ref(), &ctx).await {
            Ok(()) => Ok(()),
            Err(error) => {
                tracing::warn!(
                    %agent_id, %error,
                    "terminal usage flush failed; controller and ctx retained for retry"
                );
                self.budget
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(agent_id.clone(), controller);
                self.flush_ctx
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(agent_id.clone(), ctx);
                Err(SupervisorError::UsageFlushPending(agent_id.clone()))
            }
        }
    }

    /// 重试任务（W2）：仅复位 TaskGraph 中的任务状态并发出 `TaskRetried`。
    ///
    /// 注意：worker 生命周期仍是 `Failed` 终态，重跑需要新的 spawn；本方法
    /// 只把任务图状态复位（`Failed → Created`）并递增尝试计数。
    pub async fn retry_task(&self, agent_id: &AgentId) -> Result<u32, SupervisorError> {
        let Some(graph) = &self.task_graph else {
            return Err(SupervisorError::PolicyDenied(
                "task graph not configured".to_string(),
            ));
        };
        let task_id = TaskId::new(agent_id.as_str());
        let attempt = graph
            .retry(&task_id)
            .map_err(|error| SupervisorError::PolicyDenied(error.to_string()))?;
        self.emit(OrchestrationEvent::TaskRetried { task_id, attempt });
        Ok(attempt)
    }

    /// 收集并检测 patch 冲突（W3）：存入待审批表并发出 `PatchProposed`；
    /// 存在冲突时同时发出 `PatchConflict`。
    ///
    /// 要求已配置 [`PatchMerger`] 与 parent 工作区；否则返回
    /// `PolicyDenied`。
    pub async fn propose_patch(
        &self,
        agent_id: &AgentId,
        patch: WorkerPatch,
    ) -> Result<ConflictReport, SupervisorError> {
        let Some(merger) = &self.patch_merger else {
            return Err(SupervisorError::PolicyDenied(
                "patch merger not configured".to_string(),
            ));
        };
        let Some(parent_workspace) = &self.parent_workspace else {
            return Err(SupervisorError::PolicyDenied(
                "patch merger not configured".to_string(),
            ));
        };
        let proposal = merger
            .collect(&patch)
            .await
            .map_err(|error| SupervisorError::Merge(error.to_string()))?;
        let report = merger
            .detect_conflicts(&proposal, parent_workspace)
            .await
            .map_err(|error| SupervisorError::Merge(error.to_string()))?;
        self.emit(OrchestrationEvent::PatchProposed {
            agent_id: agent_id.clone(),
            files: proposal.files.clone(),
        });
        if report.has_conflicts() {
            self.emit(OrchestrationEvent::PatchConflict {
                agent_id: agent_id.clone(),
                files: report.conflicting_files.clone(),
            });
        }
        self.pending_patches
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(agent_id.clone(), proposal);
        Ok(report)
    }

    /// 依据 parent 决策执行合并（W3）：从待审批表取出提案，发出
    /// `PatchMerged` / `PatchConflict`。
    ///
    /// 无待审批提案返回 `UnknownAgent`；合并 / 冲突检测错误归一为
    /// `SupervisorError::Merge`。
    pub async fn approve_patch(
        &self,
        agent_id: &AgentId,
        decision: MergeDecision,
    ) -> Result<MergeOutcome, SupervisorError> {
        let Some(merger) = &self.patch_merger else {
            return Err(SupervisorError::PolicyDenied(
                "patch merger not configured".to_string(),
            ));
        };
        let Some(parent_workspace) = &self.parent_workspace else {
            return Err(SupervisorError::PolicyDenied(
                "patch merger not configured".to_string(),
            ));
        };
        let proposal = self
            .pending_patches
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(agent_id)
            .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
        let outcome = merger
            .merge(&proposal, parent_workspace, &decision)
            .await
            .map_err(|error| SupervisorError::Merge(error.to_string()))?;
        if !outcome.merged_files.is_empty() {
            self.emit(OrchestrationEvent::PatchMerged {
                agent_id: agent_id.clone(),
                files: outcome.merged_files.clone(),
            });
        }
        if !outcome.conflicts.is_empty() {
            self.emit(OrchestrationEvent::PatchConflict {
                agent_id: agent_id.clone(),
                files: outcome.conflicts.clone(),
            });
        }
        Ok(outcome)
    }

    /// 查询 agent 的取消令牌。
    pub fn cancel_token(&self, agent_id: &AgentId) -> Option<CancellationToken> {
        self.cancel_tokens
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(agent_id)
            .cloned()
    }

    /// 查询 agent 当前 worker 状态。
    pub fn state(&self, agent_id: &AgentId) -> Option<WorkerState> {
        self.workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(agent_id)
            .map(|entry| entry.state.state())
    }

    /// 事件快照（供重放 / 恢复）。
    pub fn events(&self) -> Vec<OrchestrationEvent> {
        self.event_log
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// 崩溃恢复：重放事件重建状态；任何重放后仍处于活动态且无存活运行时的
    /// worker 一律标记 `Failed`（不留悬挂 worker）。终态 worker 原样保留。
    pub async fn recover(&self, events: &[OrchestrationEvent]) -> RecoveryReport {
        let states = replay_workers(events);
        let mut orphaned = Vec::new();
        let mut recovered_states = BTreeMap::new();
        for (agent_id, state) in states {
            if state.is_terminal() {
                recovered_states.insert(agent_id.clone(), state);
                continue;
            }
            // 无存活运行时：活动态 → Failed。
            let mut machine = WorkerStateMachine::from_state(state);
            let _ = machine.apply(WorkerTransition::Fail);
            recovered_states.insert(agent_id.clone(), machine.state());
            orphaned.push(agent_id);
        }
        RecoveryReport {
            orphaned,
            recovered_states,
        }
    }

    /// 追加一条事件。
    fn emit(&self, event: OrchestrationEvent) {
        self.event_log
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
    }

    /// 记录一次策略拒绝决策（versioned，reason 统一脱敏）。
    fn record_policy_denial(
        &self,
        req: &SpawnRequest,
        gate: PolicyGate,
        reason: impl std::fmt::Display,
    ) {
        let identity = IdentityContext::new(req.tenant_id.clone(), req.principal_id.clone());
        self.policy.record_decision(PolicyDecisionEvent::new(
            self.policy.policy_version(&req.tenant_id),
            &identity,
            gate,
            PolicyDecisionKind::Deny,
            reason.to_string(),
            now_ms(),
        ));
    }

    /// 记录一次策略放行决策（versioned）。
    fn record_policy_allow(&self, req: &SpawnRequest, gate: PolicyGate, reason: &str) {
        let identity = IdentityContext::new(req.tenant_id.clone(), req.principal_id.clone());
        self.policy.record_decision(PolicyDecisionEvent::new(
            self.policy.policy_version(&req.tenant_id),
            &identity,
            gate,
            PolicyDecisionKind::Allow,
            reason,
            now_ms(),
        ));
    }

    /// 从父的 children 列表中移除（幂等）。
    fn remove_child(&self, parent: Option<&AgentId>, child: &AgentId) {
        let Some(parent) = parent else {
            return;
        };
        if let Some(kids) = self
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get_mut(parent)
        {
            kids.retain(|id| id != child);
        }
    }

    /// 测试辅助：活动 worker 与在途并发预约计数（`tenant = None` 时全局）。
    #[cfg(test)]
    fn active_worker_count(&self, tenant: Option<&agent_domain::TenantId>) -> u64 {
        let reservations = self
            .reservations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let workers = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let reserved = reservations
            .values()
            .filter(|tenant_id| tenant.is_none_or(|wanted| *tenant_id == wanted))
            .count() as u64;
        let active = workers
            .values()
            .filter(|entry| {
                entry.state.state().is_active()
                    && tenant.is_none_or(|tenant| entry.instance.tenant_id == *tenant)
            })
            .count() as u64;
        reserved + active
    }

    /// 原子并发预约：在单一临界区内把「活动 worker + 在途 reservations」合并
    /// 计数，校验全局本地上限与租户策略 `max_concurrent_agents`，通过后插入
    /// 一条 reservation 并返回 RAII 守卫。租户上限取自同步 `policy()`，全程
    /// 不跨 await，杜绝 spawn 的 check-then-act 超配（并发调用串行化在锁内）。
    fn reserve_concurrency(
        &self,
        agent_id: AgentId,
        tenant_id: &agent_domain::TenantId,
    ) -> Result<ConcurrencyReservation, ConcurrencyReservationError> {
        let mut reservations = self
            .reservations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let workers = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let count = |tid: Option<&agent_domain::TenantId>| -> u64 {
            let reserved = reservations
                .values()
                .filter(|stored| tid.is_none_or(|wanted| *stored == wanted))
                .count() as u64;
            let active = workers
                .values()
                .filter(|entry| {
                    entry.state.state().is_active()
                        && tid.is_none_or(|wanted| &entry.instance.tenant_id == wanted)
                })
                .count() as u64;
            reserved + active
        };
        // 全局本地闸门。
        let global_active = count(None);
        if global_active >= self.config.max_agent_concurrency {
            return Err(ConcurrencyReservationError::Global {
                current: global_active,
                limit: self.config.max_agent_concurrency,
            });
        }
        // 租户策略闸门（max_concurrent_agents 取自同步 policy，无需 await）。
        if let Some(max_t) = self.policy.policy(tenant_id).max_concurrent_agents {
            let tenant_active = count(Some(tenant_id));
            if tenant_active >= max_t {
                return Err(ConcurrencyReservationError::Tenant {
                    current: tenant_active,
                    max: max_t,
                });
            }
        }
        reservations.insert(agent_id.clone(), tenant_id.clone());
        Ok(ConcurrencyReservation {
            reservations: Arc::clone(&self.reservations),
            agent_id,
        })
    }
}

/// 校验 pool 返回的 lease 作用域与本次 spawn 的 canonical 请求一致。
///
/// 不信任调用方拼接的 [`provider_control::AcquireRequest`]（已在闸口拒绝错配），
/// 也不信任 pool 返回的 lease 内容：tenant / principal / session / agent 必须与
/// [`SpawnRequest`] 一致，请求显式指定的 provider / account 必须与 lease 一致。
/// 任何错配（恶意 / 故障 pool）返回原因串，调用方据此 fail-closed 释放 lease。
fn validate_lease_scope(
    lease: &provider_control::CredentialLease,
    req: &SpawnRequest,
    agent_id: &AgentId,
    acquire: &provider_control::AcquireRequest,
) -> Result<(), &'static str> {
    if lease.tenant_id != req.tenant_id {
        return Err("tenant mismatch");
    }
    if lease.principal_id != req.principal_id {
        return Err("principal mismatch");
    }
    if lease.session_id != req.session_id {
        return Err("session mismatch");
    }
    if &lease.agent_id != agent_id {
        return Err("agent mismatch");
    }
    if let Some(provider) = &acquire.provider_id {
        if &lease.provider_id != provider {
            return Err("provider mismatch");
        }
    }
    if let Some(account) = &acquire.account_id {
        if lease.account_id != *account {
            return Err("account mismatch");
        }
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 一天的毫秒数（日预算窗口按 UTC 日对齐）。
const MS_PER_DAY: u64 = 86_400_000;

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{ModelId, PrincipalId, SessionId, TenantId};
    use async_trait::async_trait;
    use provider_control::{AccountId, AcquireRequest, InMemoryCredentialPool};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Mutex;
    use tenant_service::{
        InMemoryTenantPolicyEngine, PermissionProfile, PolicyDecisionKind, PolicyGate,
        PrincipalRole, TenantPolicy,
    };
    use usage_ledger::{InMemoryUsageLedger, UsageQuery};

    use crate::merge::{DiffProvider, MergeError};
    use crate::worktree::{WorkerWorktree, WorktreeError};

    fn acquire_request(agent: &AgentId) -> AcquireRequest {
        AcquireRequest {
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-1"),
            session_id: SessionId::new("session-1"),
            agent_id: agent.clone(),
            provider_id: None,
            account_id: None,
            trace_id: None,
        }
    }

    fn harness(
        pool: Arc<InMemoryCredentialPool>,
        policy: Arc<InMemoryTenantPolicyEngine>,
    ) -> AgentSupervisor {
        // P18-9 deny-first：未知非 local/default 租户回落 Viewer，历史测试
        // 的 tenant-a / tenant-b 均视为「已配置租户」，缺 profile 时播种
        // 显式 Admin（仅在未配置时播种，避免覆盖测试自带 profile 或递增版本）。
        for tenant in ["tenant-a", "tenant-b"] {
            let tenant = TenantId::new(tenant);
            let current = policy.policy(&tenant);
            if current.permission_profile.is_none() {
                policy.set_policy(
                    tenant,
                    TenantPolicy {
                        permission_profile: Some(PermissionProfile {
                            default_role: Some(PrincipalRole::Admin),
                            ..PermissionProfile::default()
                        }),
                        ..current
                    },
                );
            }
        }
        AgentSupervisor::new(
            pool,
            policy,
            Arc::new(InMemoryUsageLedger::new()),
            SupervisorConfig::default(),
        )
    }

    fn spawn_request(agent_acquire: Option<AcquireRequest>) -> SpawnRequest {
        SpawnRequest {
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-1"),
            parent_id: None,
            session_id: SessionId::new("session-1"),
            worktree_path: None,
            budget: None,
            model: None,
            acquire: agent_acquire,
            task_deps: Vec::new(),
            task_description: None,
            task_max_retries: None,
        }
    }

    /// 测试用 worktree 分配器：每次分配创建独立临时目录并写 README；
    /// `release` 只记录路径、从不删除任何用户数据。
    pub struct FakeWt {
        tempdirs: Mutex<Vec<tempfile::TempDir>>,
        released: Mutex<Vec<PathBuf>>,
    }

    impl FakeWt {
        pub fn new() -> Self {
            Self {
                tempdirs: Mutex::new(Vec::new()),
                released: Mutex::new(Vec::new()),
            }
        }

        pub fn released(&self) -> Vec<PathBuf> {
            self.released
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone()
        }
    }

    impl Default for FakeWt {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl WorktreeAllocator for FakeWt {
        async fn allocate(
            &self,
            _parent_path: &Path,
            branch: &str,
            _start_point: Option<&str>,
        ) -> Result<WorkerWorktree, WorktreeError> {
            let dir = tempfile::tempdir().map_err(WorktreeError::Io)?;
            std::fs::write(dir.path().join("README.md"), "fake worktree\n")
                .map_err(WorktreeError::Io)?;
            let path = dir.path().to_path_buf();
            self.tempdirs
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(dir);
            Ok(WorkerWorktree {
                path,
                branch: branch.to_string(),
                managed: true,
            })
        }

        async fn release(&self, path: &Path) -> Result<(), WorktreeError> {
            // 绝不删除用户数据：只记录释放请求。
            self.released
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(path.to_path_buf());
            Ok(())
        }
    }

    /// 测试用 DiffProvider：脚本化 files/base（独立于 merge.rs 测试内的 fake）。
    #[derive(Clone)]
    pub struct FakeDiff {
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        base: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    }

    impl FakeDiff {
        pub fn new(files: BTreeMap<String, Vec<u8>>) -> Self {
            Self {
                files: Arc::new(Mutex::new(files)),
                base: Arc::new(Mutex::new(BTreeMap::new())),
            }
        }

        pub fn with_base(self, base: BTreeMap<String, Vec<u8>>) -> Self {
            *self
                .base
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = base;
            self
        }
    }

    #[async_trait]
    impl DiffProvider for FakeDiff {
        async fn changed_files(&self, _worktree_path: &Path) -> Result<Vec<String>, MergeError> {
            Ok(self
                .files
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .keys()
                .cloned()
                .collect())
        }

        async fn file_content(
            &self,
            _worktree_path: &Path,
            rel: &str,
        ) -> Result<Vec<u8>, MergeError> {
            self.files
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get(rel)
                .cloned()
                .ok_or_else(|| MergeError::Diff(format!("no such file {rel}")))
        }

        async fn base_content(
            &self,
            _parent_path: &Path,
            rel: &str,
        ) -> Result<Vec<u8>, MergeError> {
            self.base
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get(rel)
                .cloned()
                .ok_or_else(|| MergeError::Diff(format!("no base for {rel}")))
        }
    }

    fn events_contain(
        events: &[OrchestrationEvent],
        pred: impl Fn(&OrchestrationEvent) -> bool,
    ) -> bool {
        events.iter().any(pred)
    }

    #[tokio::test]
    async fn spawn_creates_worker_and_emits_lifecycle_events() {
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let agent_id = supervisor.spawn(spawn_request(None)).await.unwrap();
        assert!(agent_id.as_str().starts_with("agent-"));
        assert_eq!(supervisor.state(&agent_id), Some(WorkerState::Starting));

        let events = supervisor.events();
        let types: Vec<&str> = events
            .iter()
            .map(|event| match event {
                OrchestrationEvent::WorkerCreated { .. } => "created",
                OrchestrationEvent::WorkerAdmitted { .. } => "admitted",
                OrchestrationEvent::WorkerStarted { .. } => "started",
                _ => "other",
            })
            .collect();
        assert_eq!(types, vec!["created", "admitted", "started"]);

        supervisor.start_worker(&agent_id).await.unwrap();
        assert_eq!(supervisor.state(&agent_id), Some(WorkerState::Running));
        assert!(matches!(
            supervisor.events().last(),
            Some(OrchestrationEvent::WorkerRunning { .. })
        ));
    }

    #[tokio::test]
    async fn spawn_with_acquire_holds_lease_until_complete() {
        let pool = Arc::new(InMemoryCredentialPool::new(4));
        let supervisor = harness(
            pool.clone(),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let mut req = spawn_request(None);
        req.acquire = Some(acquire_request(&AgentId::new("placeholder")));
        let agent_id = supervisor.spawn(req).await.unwrap();

        let account = AccountId::new("local/default");
        assert_eq!(pool.active_count(&account), 1);

        supervisor.start_worker(&agent_id).await.unwrap();
        supervisor.complete(&agent_id).await.unwrap();
        assert_eq!(supervisor.state(&agent_id), Some(WorkerState::Completed));
        assert_eq!(pool.active_count(&account), 0);
        let health = pool.account_health(&account);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.cancelled_count, 0);
    }

    #[tokio::test]
    async fn fail_releases_lease_with_failed_outcome() {
        let pool = Arc::new(InMemoryCredentialPool::new(4));
        let supervisor = harness(
            pool.clone(),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let mut req = spawn_request(None);
        req.acquire = Some(acquire_request(&AgentId::new("placeholder")));
        let agent_id = supervisor.spawn(req).await.unwrap();
        supervisor.start_worker(&agent_id).await.unwrap();

        supervisor
            .fail(&agent_id, "boom".to_string())
            .await
            .unwrap();
        assert_eq!(supervisor.state(&agent_id), Some(WorkerState::Failed));
        let account = AccountId::new("local/default");
        assert_eq!(pool.active_count(&account), 0);
        assert_eq!(pool.account_health(&account).consecutive_failures, 1);
    }

    #[tokio::test]
    async fn cancel_tree_cancels_descendants_and_releases_leases_without_health_penalty() {
        let pool = Arc::new(InMemoryCredentialPool::new(8));
        let supervisor = harness(
            pool.clone(),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let account = AccountId::new("local/default");

        // parent（无 lease）+ 两个带 lease 的 child。
        let parent = supervisor.spawn(spawn_request(None)).await.unwrap();
        let mut child_req = SpawnRequest {
            parent_id: Some(parent.clone()),
            ..spawn_request(None)
        };
        child_req.acquire = Some(acquire_request(&AgentId::new("c1")));
        let child_a = supervisor.spawn(child_req.clone()).await.unwrap();
        child_req.acquire = Some(acquire_request(&AgentId::new("c2")));
        let child_b = supervisor.spawn(child_req).await.unwrap();
        supervisor.start_worker(&parent).await.unwrap();
        supervisor.start_worker(&child_a).await.unwrap();
        supervisor.start_worker(&child_b).await.unwrap();
        assert_eq!(pool.active_count(&account), 2);

        let receipt = supervisor.cancel_tree(&parent).await.unwrap();
        assert_eq!(receipt.cancelled_ids.len(), 3);
        assert!(receipt.cancelled_ids.contains(&parent));
        assert!(receipt.cancelled_ids.contains(&child_a));
        assert!(receipt.cancelled_ids.contains(&child_b));
        assert_eq!(receipt.leases_released, 2);

        assert_eq!(supervisor.state(&parent), Some(WorkerState::Cancelled));
        assert_eq!(supervisor.state(&child_a), Some(WorkerState::Cancelled));
        assert_eq!(supervisor.state(&child_b), Some(WorkerState::Cancelled));
        assert_eq!(pool.active_count(&account), 0, "lease 不得泄漏");

        // 取消不惩罚健康：cancelled 计数累加，连续失败保持 0。
        let health = pool.account_health(&account);
        assert_eq!(health.cancelled_count, 2);
        assert_eq!(health.consecutive_failures, 0);

        // 每个节点的取消令牌都已触发。
        for id in [&parent, &child_a, &child_b] {
            assert!(
                supervisor.cancel_token(id).unwrap().is_cancelled(),
                "token for {id} must be cancelled"
            );
        }
    }

    #[tokio::test]
    async fn cancel_tree_is_idempotent_on_repeat() {
        let pool = Arc::new(InMemoryCredentialPool::new(8));
        let supervisor = harness(
            pool.clone(),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let account = AccountId::new("local/default");
        let parent = supervisor.spawn(spawn_request(None)).await.unwrap();
        let mut child_req = SpawnRequest {
            parent_id: Some(parent.clone()),
            ..spawn_request(None)
        };
        child_req.acquire = Some(acquire_request(&AgentId::new("c1")));
        let child = supervisor.spawn(child_req).await.unwrap();

        let first = supervisor.cancel_tree(&parent).await.unwrap();
        assert_eq!(first.cancelled_ids.len(), 2);
        assert_eq!(first.leases_released, 1);
        assert_eq!(pool.account_health(&account).cancelled_count, 1);

        // 重复取消：幂等，无新增取消、无重复释放、健康计数不变。
        let second = supervisor.cancel_tree(&parent).await.unwrap();
        assert!(second.cancelled_ids.is_empty());
        assert_eq!(second.leases_released, 0);
        assert_eq!(pool.account_health(&account).cancelled_count, 1);
        assert_eq!(pool.account_health(&account).consecutive_failures, 0);
        assert_eq!(pool.active_count(&account), 0);
        assert_eq!(supervisor.state(&child), Some(WorkerState::Cancelled));
    }

    #[tokio::test]
    async fn cancel_tree_on_deep_tree_cancels_all() {
        let pool = Arc::new(InMemoryCredentialPool::new(16));
        let supervisor = harness(
            pool.clone(),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let root = supervisor.spawn(spawn_request(None)).await.unwrap();
        let mut mid_req = SpawnRequest {
            parent_id: Some(root.clone()),
            ..spawn_request(None)
        };
        let mid = supervisor.spawn(mid_req.clone()).await.unwrap();
        mid_req.parent_id = Some(mid.clone());
        let leaf = supervisor.spawn(mid_req).await.unwrap();

        let receipt = supervisor.cancel_tree(&root).await.unwrap();
        assert_eq!(receipt.cancelled_ids.len(), 3);
        for id in [&root, &mid, &leaf] {
            assert_eq!(supervisor.state(id), Some(WorkerState::Cancelled));
        }
    }

    #[tokio::test]
    async fn recover_marks_active_workers_failed_leaving_no_dangling_workers() {
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let tenant = TenantId::new("tenant-a");
        let created = |agent: &str, at: u64| OrchestrationEvent::WorkerCreated {
            agent_id: AgentId::new(agent),
            tenant_id: tenant.clone(),
            parent_id: None,
            role: WorkerRole::Parent,
            session_id: SessionId::new("s1"),
            worktree_path: None,
            created_at_ms: at,
        };
        let events = vec![
            // a：Running 中被中断 → 孤儿。
            created("a", 1),
            OrchestrationEvent::WorkerAdmitted {
                agent_id: AgentId::new("a"),
                at_ms: 2,
            },
            OrchestrationEvent::WorkerStarted {
                agent_id: AgentId::new("a"),
                at_ms: 3,
            },
            OrchestrationEvent::WorkerRunning {
                agent_id: AgentId::new("a"),
                at_ms: 4,
            },
            // b：Admitted 阶段被中断 → 孤儿。
            created("b", 5),
            OrchestrationEvent::WorkerAdmitted {
                agent_id: AgentId::new("b"),
                at_ms: 6,
            },
            // c：已完成（终态）→ 保持。
            created("c", 7),
            OrchestrationEvent::WorkerCompleted {
                agent_id: AgentId::new("c"),
                at_ms: 8,
            },
            // d：已失败（终态）→ 保持。
            created("d", 9),
            OrchestrationEvent::WorkerFailed {
                agent_id: AgentId::new("d"),
                at_ms: 10,
                reason: "earlier".into(),
            },
        ];

        let report = supervisor.recover(&events).await;
        assert_eq!(report.orphaned, vec![AgentId::new("a"), AgentId::new("b")]);
        assert_eq!(
            report.recovered_states[&AgentId::new("a")],
            WorkerState::Failed
        );
        assert_eq!(
            report.recovered_states[&AgentId::new("b")],
            WorkerState::Failed
        );
        assert_eq!(
            report.recovered_states[&AgentId::new("c")],
            WorkerState::Completed
        );
        assert_eq!(
            report.recovered_states[&AgentId::new("d")],
            WorkerState::Failed
        );
        // 不留悬挂 worker：所有恢复状态均为终态。
        assert!(report
            .recovered_states
            .values()
            .all(|state| state.is_terminal()));
    }

    #[tokio::test]
    async fn policy_denied_when_tenant_agent_concurrency_exceeded() {
        let policy = Arc::new(InMemoryTenantPolicyEngine::default());
        policy.set_policy(
            TenantId::new("tenant-a"),
            TenantPolicy {
                max_concurrent_agents: Some(1),
                ..TenantPolicy::default()
            },
        );
        let supervisor = harness(Arc::new(InMemoryCredentialPool::new(4)), policy);
        let first = supervisor.spawn(spawn_request(None)).await.unwrap();
        assert!(supervisor.state(&first).is_some());
        let err = supervisor.spawn(spawn_request(None)).await.unwrap_err();
        assert!(matches!(err, SupervisorError::PolicyDenied(_)));
    }

    #[tokio::test]
    async fn unknown_agent_operations_error() {
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let unknown = AgentId::new("nope");
        assert!(matches!(
            supervisor.complete(&unknown).await.unwrap_err(),
            SupervisorError::UnknownAgent(_)
        ));
        assert!(matches!(
            supervisor.cancel_tree(&unknown).await.unwrap_err(),
            SupervisorError::UnknownAgent(_)
        ));
        assert!(supervisor.state(&unknown).is_none());
        assert!(supervisor.cancel_token(&unknown).is_none());
    }

    #[tokio::test]
    async fn events_snapshot_roundtrips_through_replay() {
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let agent = supervisor.spawn(spawn_request(None)).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        let snapshot = supervisor.events();
        let states = replay_workers(&snapshot);
        assert_eq!(states[&agent], WorkerState::Running);
    }

    #[tokio::test]
    async fn spawn_with_allocator_assigns_isolated_worktree() {
        let parent_dir = tempfile::tempdir().unwrap();
        std::fs::write(parent_dir.path().join("notes.txt"), "parent content\n").unwrap();
        let allocator = Arc::new(FakeWt::new());
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        )
        .with_parent_workspace(parent_dir.path().to_path_buf())
        .with_worktree_allocator(allocator.clone());

        let agent = supervisor.spawn(spawn_request(None)).await.unwrap();
        // WorkerCreated 携带分配后的真实路径。
        let created = supervisor
            .events()
            .into_iter()
            .find_map(|event| match event {
                OrchestrationEvent::WorkerCreated {
                    agent_id: id,
                    worktree_path,
                    ..
                } if id == agent => worktree_path,
                _ => None,
            })
            .expect("WorkerCreated event");
        let worktree_path = PathBuf::from(created);
        assert!(worktree_path.join("README.md").exists());

        // worker 写入自己的 worktree 副本，不影响 parent 同名文件。
        std::fs::write(worktree_path.join("notes.txt"), "worker content\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(parent_dir.path().join("notes.txt")).unwrap(),
            "parent content\n",
            "worker 写入不得改变 parent 路径下的文件"
        );

        supervisor.start_worker(&agent).await.unwrap();
        supervisor.complete(&agent).await.unwrap();
        assert!(
            allocator.released().contains(&worktree_path),
            "complete 必须显式释放 worktree"
        );
    }

    #[tokio::test]
    async fn spawn_without_allocator_preserves_old_behavior() {
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let agent = supervisor.spawn(spawn_request(None)).await.unwrap();
        // 守卫限定在块内，确保在 start_worker / complete 前释放。
        {
            let workers = supervisor
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let entry = workers.get(&agent).unwrap();
            assert!(entry.instance.worktree_path.is_none());
            assert!(entry.worktree.is_none());
        }
        // WorkerCreated 的 worktree_path 为 None。
        assert!(supervisor.events().iter().all(|event| match event {
            OrchestrationEvent::WorkerCreated { worktree_path, .. } => worktree_path.is_none(),
            _ => true,
        }));

        supervisor.start_worker(&agent).await.unwrap();
        supervisor.complete(&agent).await.unwrap();
        assert_eq!(supervisor.state(&agent), Some(WorkerState::Completed));
    }

    #[tokio::test]
    async fn task_graph_wiring_emits_task_events() {
        let graph = Arc::new(TaskGraph::new());
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(8)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        )
        .with_task_graph(graph.clone());

        let completed = supervisor.spawn(spawn_request(None)).await.unwrap();
        supervisor.start_worker(&completed).await.unwrap();
        supervisor.complete(&completed).await.unwrap();
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::TaskCreated { task_id, .. }
                if *task_id == TaskId::new(completed.as_str())
        )));
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::TaskReady { task_id }
                if *task_id == TaskId::new(completed.as_str())
        )));
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::TaskAssigned { task_id, agent_id }
                if *task_id == TaskId::new(completed.as_str()) && *agent_id == completed
        )));
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::TaskCompleted { task_id }
                if *task_id == TaskId::new(completed.as_str())
        )));
        assert_eq!(
            graph.state_of(&TaskId::new(completed.as_str())),
            Some(TaskState::Completed)
        );

        let failed = supervisor.spawn(spawn_request(None)).await.unwrap();
        supervisor.start_worker(&failed).await.unwrap();
        supervisor.fail(&failed, "boom".to_string()).await.unwrap();
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::TaskFailed { task_id, reason }
                if *task_id == TaskId::new(failed.as_str()) && reason == "boom"
        )));

        let cancelled = supervisor.spawn(spawn_request(None)).await.unwrap();
        supervisor.cancel_tree(&cancelled).await.unwrap();
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::TaskCancelled { task_id }
                if *task_id == TaskId::new(cancelled.as_str())
        )));
    }

    #[tokio::test]
    async fn spawn_with_unmet_task_deps_stays_blocked_and_consistent() {
        // 前向依赖（TaskGraph 明确支持）：task 依赖尚未插入的 "dep"。
        // spawn 必须成功、任务保持 Blocked、不 emit TaskReady/TaskAssigned，
        // 且 worker 注册表与事件流一致（worker 已注册、状态 Starting）。
        let graph = Arc::new(TaskGraph::new());
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        )
        .with_task_graph(graph.clone());

        let mut req = spawn_request(None);
        req.task_deps = vec![TaskId::new("dep")];
        let agent_id = supervisor.spawn(req).await.unwrap();

        // worker 已注册、状态 Starting（spawn 成功路径）。
        assert_eq!(supervisor.state(&agent_id), Some(WorkerState::Starting));
        // 任务保持 Blocked。
        assert_eq!(
            graph.state_of(&TaskId::new(agent_id.as_str())),
            Some(TaskState::Blocked)
        );
        // 只 emit 了 TaskCreated，未 emit TaskReady/TaskAssigned。
        let events = supervisor.events();
        assert!(events_contain(&events, |event| matches!(
            event,
            OrchestrationEvent::TaskCreated { task_id, .. }
                if *task_id == TaskId::new(agent_id.as_str())
        )));
        assert!(!events_contain(&events, |event| matches!(
            event,
            OrchestrationEvent::TaskReady { task_id }
            | OrchestrationEvent::TaskAssigned { task_id, .. }
                if *task_id == TaskId::new(agent_id.as_str())
        )));
    }

    #[tokio::test]
    async fn retry_task_emits_task_retried() {
        let graph = Arc::new(TaskGraph::new());
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        )
        .with_task_graph(graph.clone());
        let mut req = spawn_request(None);
        req.task_max_retries = Some(2);
        let agent = supervisor.spawn(req).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        supervisor
            .fail(&agent, "retry me".to_string())
            .await
            .unwrap();

        let attempt = supervisor.retry_task(&agent).await.unwrap();
        assert_eq!(attempt, 1);
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::TaskRetried { task_id, attempt }
                if *task_id == TaskId::new(agent.as_str()) && *attempt == 1
        )));
        assert_eq!(
            graph.state_of(&TaskId::new(agent.as_str())),
            Some(TaskState::Created)
        );
    }

    #[tokio::test]
    async fn record_usage_emits_budget_exceeded() {
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            engine_for("tenant-a", TenantPolicy::default()),
            Arc::new(InMemoryUsageLedger::new()),
            SupervisorConfig {
                budget: WorkerBudgetLimits {
                    max_input_tokens: Some(10),
                    ..WorkerBudgetLimits::default()
                },
                ..SupervisorConfig::default()
            },
        );
        let agent = supervisor.spawn(spawn_request(None)).await.unwrap();
        supervisor.record_usage(&agent, 20, 0, 0).await.unwrap();
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::BudgetExceeded {
                agent_id,
                dimension,
                used,
                limit,
            } if *agent_id == agent && dimension == "input_tokens" && *used == 20 && *limit == 10
        )));
    }

    #[tokio::test]
    async fn complete_flushes_ledger_with_real_attribution() {
        let pool = Arc::new(InMemoryCredentialPool::new(4));
        let ledger = Arc::new(InMemoryUsageLedger::new());
        let supervisor = AgentSupervisor::new(
            pool.clone(),
            engine_for("tenant-a", TenantPolicy::default()),
            ledger.clone(),
            SupervisorConfig::default(),
        );
        let mut req = spawn_request(None);
        req.acquire = Some(acquire_request(&AgentId::new("placeholder")));
        req.model = Some(ModelId::new("mock-model"));
        let agent = supervisor.spawn(req).await.unwrap();
        supervisor.record_usage(&agent, 5, 3, 0).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        supervisor.complete(&agent).await.unwrap();

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].account_id, "local/default",
            "account 必须来自 lease 而非 unknown"
        );
        assert_eq!(
            records[0].provider_id,
            ProviderId::new("default"),
            "provider 必须来自 lease"
        );
        assert_eq!(
            records[0].model_id,
            ModelId::new("mock-model"),
            "model 必须来自 spawn 请求"
        );
    }

    #[tokio::test]
    async fn concurrency_denied_event_on_local_limit() {
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            engine_for("tenant-a", TenantPolicy::default()),
            Arc::new(InMemoryUsageLedger::new()),
            SupervisorConfig {
                max_agent_concurrency: 1,
                ..SupervisorConfig::default()
            },
        );
        let first = supervisor.spawn(spawn_request(None)).await.unwrap();
        assert!(supervisor.state(&first).is_some());
        let err = supervisor.spawn(spawn_request(None)).await.unwrap_err();
        assert!(matches!(err, SupervisorError::PolicyDenied(_)));
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::ConcurrencyDenied { kind, current, limit }
                if kind == "agents" && *current == 1 && *limit == 1
        )));
    }

    #[tokio::test]
    async fn propose_and_approve_patch_emits_events() {
        let parent_dir = tempfile::tempdir().unwrap();
        std::fs::write(parent_dir.path().join("a.txt"), b"base").unwrap();
        let diff = FakeDiff::new(BTreeMap::from([(
            "a.txt".to_string(),
            b"worker-v1".to_vec(),
        )]))
        .with_base(BTreeMap::from([("a.txt".to_string(), b"base".to_vec())]));
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        )
        .with_parent_workspace(parent_dir.path().to_path_buf())
        .with_patch_merger(Arc::new(PatchMerger::new(Arc::new(diff))));
        let agent = AgentId::new("agent-x");

        // 无冲突：propose → PatchProposed；approve(Merge) → PatchMerged。
        let patch = WorkerPatch {
            agent_id: agent.clone(),
            session_id: SessionId::new("session-1"),
            worktree_path: PathBuf::from("/wt"),
            changed_files: vec!["a.txt".to_string()],
        };
        let report = supervisor.propose_patch(&agent, patch).await.unwrap();
        assert!(!report.has_conflicts());
        let outcome = supervisor
            .approve_patch(&agent, MergeDecision::Merge)
            .await
            .unwrap();
        assert_eq!(outcome.merged_files, vec!["a.txt".to_string()]);
        let events = supervisor.events();
        assert!(events_contain(&events, |event| matches!(
            event,
            OrchestrationEvent::PatchProposed { agent_id, .. } if *agent_id == agent
        )));
        assert!(events_contain(&events, |event| matches!(
            event,
            OrchestrationEvent::PatchMerged { agent_id, files }
                if *agent_id == agent && files == &vec!["a.txt".to_string()]
        )));

        // 冲突用例：parent 当前内容与基准不一致 → propose 阶段 PatchConflict。
        std::fs::write(parent_dir.path().join("a.txt"), b"parent-edit").unwrap();
        let patch = WorkerPatch {
            agent_id: agent.clone(),
            session_id: SessionId::new("session-1"),
            worktree_path: PathBuf::from("/wt2"),
            changed_files: vec!["a.txt".to_string()],
        };
        let report = supervisor.propose_patch(&agent, patch).await.unwrap();
        assert!(report.has_conflicts());
        assert_eq!(report.conflicting_files, vec!["a.txt".to_string()]);
        assert!(events_contain(&supervisor.events(), |event| matches!(
            event,
            OrchestrationEvent::PatchConflict { agent_id, files }
                if *agent_id == agent && files == &vec!["a.txt".to_string()]
        )));
    }

    // ── P18-9 租户策略强制入口测试 ────────────────────────────────────────

    fn whitelisted_acquire(provider: Option<&str>, account: Option<&str>) -> AcquireRequest {
        AcquireRequest {
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-1"),
            session_id: SessionId::new("session-1"),
            agent_id: AgentId::new("placeholder"),
            provider_id: provider.map(ProviderId::new),
            account_id: account.map(AccountId::new),
            trace_id: None,
        }
    }

    fn engine_for(tenant: &str, policy: TenantPolicy) -> Arc<InMemoryTenantPolicyEngine> {
        let engine = Arc::new(InMemoryTenantPolicyEngine::default());
        // deny-first：测试聚焦白名单 / 预算 / 并发闸口时，缺 profile 会先被
        // Viewer 兜底拒绝；此处只在未显式配置 profile 时补 Admin，显式
        // Viewer / Service 场景保持原样。
        let policy = if policy.permission_profile.is_some() {
            policy
        } else {
            TenantPolicy {
                permission_profile: Some(PermissionProfile {
                    default_role: Some(PrincipalRole::Admin),
                    ..PermissionProfile::default()
                }),
                ..policy
            }
        };
        engine.set_policy(TenantId::new(tenant), policy);
        engine
    }

    #[tokio::test]
    async fn tenant_policy_provider_whitelist_denies_and_scopes_per_tenant() {
        let engine = engine_for(
            "tenant-a",
            TenantPolicy {
                allowed_providers: Some(vec![ProviderId::new("openai")]),
                ..TenantPolicy::default()
            },
        );
        let supervisor = harness(Arc::new(InMemoryCredentialPool::new(4)), engine.clone());
        let mut req = spawn_request(None);
        req.acquire = Some(whitelisted_acquire(Some("anthropic"), None));
        let err = supervisor.spawn(req).await.unwrap_err();
        assert!(matches!(err, SupervisorError::PolicyDenied(_)), "{err:?}");
        // 拒绝记为 versioned 决策事件（LeaseAcquire 闸口），deny 不被后续
        // 任何阶段覆盖（deny-first 短路：仅此一条事件）。
        let decisions = engine.decisions(&TenantId::new("tenant-a"));
        assert_eq!(decisions.len(), 1, "{decisions:?}");
        assert_eq!(decisions[0].gate, PolicyGate::LeaseAcquire);
        assert_eq!(decisions[0].decision, PolicyDecisionKind::Deny);
        assert_eq!(decisions[0].policy_version, 1);
        assert_eq!(decisions[0].tenant_id, TenantId::new("tenant-a"));
        assert_eq!(decisions[0].principal_id, PrincipalId::new("principal-1"));
        // 跨租户隔离：未配置租户 tenant-b 不继承 tenant-a 的 deny。
        let mut other = spawn_request(None);
        other.tenant_id = TenantId::new("tenant-b");
        // P18-9：AcquireRequest 的 tenant 必须与外层一致（错配拒绝）；
        // 测试夹具显式对齐 tenant-b 后放行。
        let mut other_acquire = whitelisted_acquire(Some("anthropic"), None);
        other_acquire.tenant_id = TenantId::new("tenant-b");
        other.acquire = Some(other_acquire);
        assert!(
            supervisor.spawn(other).await.is_ok(),
            "tenant-b must not inherit tenant-a deny policy"
        );
        let tenant_b_decisions = engine.decisions(&TenantId::new("tenant-b"));
        assert!(
            tenant_b_decisions
                .iter()
                .all(|event| event.decision == PolicyDecisionKind::Allow),
            "tenant-b spawn must be allowed (no inherited deny): {tenant_b_decisions:?}"
        );
    }

    #[tokio::test]
    async fn tenant_policy_account_whitelist_gates_lease_acquire() {
        let engine = engine_for(
            "tenant-a",
            TenantPolicy {
                allowed_accounts: Some(vec![AccountId::new("acct-a")]),
                ..TenantPolicy::default()
            },
        );
        let supervisor = harness(Arc::new(InMemoryCredentialPool::new(4)), engine.clone());

        let mut deny_req = spawn_request(None);
        deny_req.acquire = Some(whitelisted_acquire(None, Some("acct-b")));
        let err = supervisor.spawn(deny_req).await.unwrap_err();
        assert!(matches!(err, SupervisorError::PolicyDenied(_)), "{err:?}");

        let mut allow_req = spawn_request(None);
        allow_req.acquire = Some(whitelisted_acquire(None, Some("acct-a")));
        let agent = supervisor.spawn(allow_req).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        supervisor.complete(&agent).await.unwrap();

        let decisions = engine.decisions(&TenantId::new("tenant-a"));
        assert!(decisions
            .iter()
            .any(|event| event.gate == PolicyGate::LeaseAcquire
                && event.decision == PolicyDecisionKind::Deny));
        assert!(decisions
            .iter()
            .any(|event| event.gate == PolicyGate::AgentSpawn
                && event.decision == PolicyDecisionKind::Allow));
    }

    #[tokio::test]
    async fn tenant_policy_model_whitelist_denies_spawn() {
        let engine = engine_for(
            "tenant-a",
            TenantPolicy {
                allowed_models: Some(vec![ModelId::new("gpt-4o")]),
                ..TenantPolicy::default()
            },
        );
        let supervisor = harness(Arc::new(InMemoryCredentialPool::new(4)), engine.clone());
        let mut req = spawn_request(None);
        req.model = Some(ModelId::new("claude-3-5-sonnet"));
        let err = supervisor.spawn(req).await.unwrap_err();
        assert!(matches!(err, SupervisorError::PolicyDenied(_)), "{err:?}");
        assert_eq!(
            engine.decisions(&TenantId::new("tenant-a"))[0].gate,
            PolicyGate::AgentSpawn
        );

        // 命中白名单放行。
        let mut ok = spawn_request(None);
        ok.model = Some(ModelId::new("gpt-4o"));
        assert!(supervisor.spawn(ok).await.is_ok());
    }

    #[tokio::test]
    async fn tenant_policy_role_deny_first_only_core_policy_can_release() {
        let engine = engine_for(
            "tenant-a",
            TenantPolicy {
                permission_profile: Some(PermissionProfile {
                    default_role: Some(PrincipalRole::Viewer),
                    ..PermissionProfile::default()
                }),
                ..TenantPolicy::default()
            },
        );
        let supervisor = harness(Arc::new(InMemoryCredentialPool::new(4)), engine.clone());
        let err = supervisor.spawn(spawn_request(None)).await.unwrap_err();
        assert!(matches!(err, SupervisorError::PolicyDenied(_)), "{err:?}");
        assert_eq!(
            engine.decisions(&TenantId::new("tenant-a"))[0].decision,
            PolicyDecisionKind::Deny
        );

        // adapter / GUI / plugin 无法覆盖：只有 Core 策略更新（版本递增）后放行。
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
        let agent = supervisor.spawn(spawn_request(None)).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        supervisor.complete(&agent).await.unwrap();
        let decisions = engine.decisions(&TenantId::new("tenant-a"));
        assert_eq!(decisions[0].decision, PolicyDecisionKind::Deny);
        assert_eq!(decisions[1].policy_version, 2, "set_policy 递增版本");
        assert_eq!(decisions[1].decision, PolicyDecisionKind::Allow);
    }

    #[tokio::test]
    async fn tenant_policy_agent_concurrency_limits_per_tenant() {
        let engine = engine_for(
            "tenant-a",
            TenantPolicy {
                max_concurrent_agents: Some(1),
                ..TenantPolicy::default()
            },
        );
        let supervisor = harness(Arc::new(InMemoryCredentialPool::new(4)), engine.clone());
        let first = supervisor.spawn(spawn_request(None)).await.unwrap();
        let err = supervisor.spawn(spawn_request(None)).await.unwrap_err();
        assert!(matches!(err, SupervisorError::PolicyDenied(_)), "{err:?}");
        assert_eq!(
            engine
                .decisions(&TenantId::new("tenant-a"))
                .iter()
                .filter(|event| event.decision == PolicyDecisionKind::Deny)
                .count(),
            1
        );

        // 终态后并发恢复；同租户限制不泄漏到其它租户。
        supervisor.start_worker(&first).await.unwrap();
        supervisor.complete(&first).await.unwrap();
        assert!(
            supervisor.spawn(spawn_request(None)).await.is_ok(),
            "tenant concurrency must recover after terminal"
        );
        let mut other = spawn_request(None);
        other.tenant_id = TenantId::new("tenant-b");
        assert!(
            supervisor.spawn(other).await.is_ok(),
            "tenant-a concurrency limit must not leak to tenant-b"
        );
    }

    #[tokio::test]
    async fn tenant_policy_daily_token_budget_denies_spawn_before_admission() {
        let ledger = Arc::new(InMemoryUsageLedger::new());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64;
        ledger
            .record(UsageRecord {
                record_id: "budget-seed-1".into(),
                tenant_id: TenantId::new("tenant-a"),
                principal_id: PrincipalId::new("principal-1"),
                account_id: "local/default".into(),
                credential_id: None,
                session_id: SessionId::new("session-1"),
                agent_id: AgentId::new("seed-agent"),
                run_id: None,
                provider_id: ProviderId::new("local"),
                model_id: ModelId::new("unknown"),
                input_tokens: 101,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_micros: 0,
                currency: "USD".into(),
                occurred_at_ms: now,
                ..UsageRecord::default()
            })
            .await
            .expect("seed ledger");
        let engine = engine_for(
            "tenant-a",
            TenantPolicy {
                daily_input_token_budget: Some(100),
                ..TenantPolicy::default()
            },
        );
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            engine.clone(),
            ledger,
            SupervisorConfig::default(),
        );
        let err = supervisor.spawn(spawn_request(None)).await.unwrap_err();
        assert!(
            matches!(err, SupervisorError::PolicyDenied(ref reason) if reason.contains("预算")),
            "{err:?}"
        );
        let decisions = engine.decisions(&TenantId::new("tenant-a"));
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].gate, PolicyGate::AgentSpawn);
        assert_eq!(decisions[0].decision, PolicyDecisionKind::Deny);
    }

    #[tokio::test]
    async fn acquire_tenant_principal_session_mismatch_is_rejected() {
        let engine = engine_for("tenant-a", TenantPolicy::default());
        let supervisor = harness(Arc::new(InMemoryCredentialPool::new(4)), engine.clone());

        // 外层 tenant-a / principal-1 / session-1；acquire 逐个字段错配。
        let mismatched: Vec<AcquireRequest> = vec![
            AcquireRequest {
                tenant_id: TenantId::new("tenant-b"),
                ..acquire_request(&AgentId::new("placeholder"))
            },
            AcquireRequest {
                principal_id: PrincipalId::new("principal-other"),
                ..acquire_request(&AgentId::new("placeholder"))
            },
            AcquireRequest {
                session_id: SessionId::new("session-other"),
                ..acquire_request(&AgentId::new("placeholder"))
            },
        ];
        for acquire in mismatched {
            let mut req = spawn_request(None);
            req.acquire = Some(acquire);
            let err = supervisor.spawn(req).await.unwrap_err();
            assert!(
                matches!(err, SupervisorError::PolicyDenied(ref reason) if reason.contains("不一致")),
                "{err:?}"
            );
        }

        // 拒绝记录在 LeaseAcquire 闸口，且不产生任何 worker / 事件流。
        let decisions = engine.decisions(&TenantId::new("tenant-a"));
        assert_eq!(decisions.len(), 3, "{decisions:?}");
        assert!(decisions.iter().all(|event| {
            event.gate == PolicyGate::LeaseAcquire && event.decision == PolicyDecisionKind::Deny
        }));
        assert_eq!(supervisor.active_worker_count(None), 0);
        assert!(supervisor.events().is_empty());
    }

    #[tokio::test]
    async fn acquire_uses_supervisor_generated_canonical_agent_id() {
        let engine = engine_for("tenant-a", TenantPolicy::default());
        let pool = Arc::new(InMemoryCredentialPool::new(4));
        let supervisor = harness(pool.clone(), engine);

        let mut req = spawn_request(None);
        // 调用方试图自选 agent_id：supervisor 必须用生成的 canonical id 覆写。
        req.acquire = Some(acquire_request(&AgentId::new("attacker-chosen")));
        let agent = supervisor.spawn(req).await.unwrap();
        assert!(agent.as_str().starts_with("agent-"));
        assert_ne!(agent.as_str(), "attacker-chosen");

        let lease_agent = {
            let workers = supervisor
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let entry = workers.get(&agent).expect("worker registered");
            let guard = entry.lease.as_ref().expect("lease held");
            guard.lease().expect("lease present").agent_id.clone()
        };
        assert_eq!(
            lease_agent, agent,
            "lease 必须绑定 supervisor 生成的 canonical agent_id"
        );

        supervisor.complete(&agent).await.unwrap();
        assert_eq!(pool.active_count(&AccountId::new("local/default")), 0);
    }
}

/// 失败计数测试专用：包装 [`InMemoryUsageLedger`]，前 `fail_until` 次 `record`
/// 返回错误（模拟 ledger 暂时不可用），之后放行。统计真实写入与重试次数。
#[cfg(test)]
struct FailingLedger {
    inner: InMemoryUsageLedger,
    fail_until: Mutex<usize>,
    record_calls: Mutex<usize>,
}

#[cfg(test)]
impl FailingLedger {
    fn fail_first(fail_until: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryUsageLedger::new(),
            fail_until: Mutex::new(fail_until),
            record_calls: Mutex::new(0),
        })
    }

    fn record_calls(&self) -> usize {
        *self.record_calls.lock().unwrap()
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl usage_ledger::UsageLedger for FailingLedger {
    async fn record(&self, record: UsageRecord) -> Result<(), UsageLedgerError> {
        let n = {
            let mut calls = self.record_calls.lock().unwrap();
            *calls += 1;
            *calls
        };
        // 在任何 await 前释放 std::sync::MutexGuard，保持 future Send。
        let should_fail = {
            let mut fail_until = self.fail_until.lock().unwrap();
            let fail = n <= *fail_until;
            // 失败名额耗尽后清零，避免后续重试仍被挡。
            if !fail {
                *fail_until = 0;
            }
            fail
        };
        if should_fail {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "ledger temporarily unavailable".to_string(),
            });
        }
        self.inner.record(record).await
    }

    async fn query(&self, query: &UsageQuery) -> Result<Vec<UsageRecord>, UsageLedgerError> {
        Ok(self.inner.query(query).await?)
    }

    async fn aggregate(&self, query: &UsageQuery) -> Result<UsageTotals, UsageLedgerError> {
        self.inner.aggregate(query).await
    }
}

/// 阻塞式失败注入 ledger：前 `fail_first` 次 `record` 失败，下一次 `record`
/// 在进入 ledger 前发出 `entered` 信号并等待 `release`（模拟慢 ledger），
/// 之后正常写入。用于验证并发 flush 的在途标记、假成功拦截与不重复计账。
#[cfg(test)]
struct BlockingLedger {
    inner: InMemoryUsageLedger,
    fail_first: Mutex<usize>,
    record_calls: Mutex<usize>,
    entered_tx: tokio::sync::watch::Sender<bool>,
    release_tx: tokio::sync::watch::Sender<bool>,
}

#[cfg(test)]
impl BlockingLedger {
    fn new(fail_first: usize) -> Arc<Self> {
        let (entered_tx, _) = tokio::sync::watch::channel(false);
        let (release_tx, _) = tokio::sync::watch::channel(false);
        Arc::new(Self {
            inner: InMemoryUsageLedger::new(),
            fail_first: Mutex::new(fail_first),
            record_calls: Mutex::new(0),
            entered_tx,
            release_tx,
        })
    }

    fn record_calls(&self) -> usize {
        *self.record_calls.lock().unwrap()
    }

    fn entered_rx(&self) -> tokio::sync::watch::Receiver<bool> {
        self.entered_tx.subscribe()
    }

    fn release(&self) {
        let _ = self.release_tx.send(true);
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl usage_ledger::UsageLedger for BlockingLedger {
    async fn record(&self, record: UsageRecord) -> Result<(), UsageLedgerError> {
        let n = {
            let mut calls = self.record_calls.lock().unwrap();
            *calls += 1;
            *calls
        };
        // 在任何 await 前释放 std::sync::MutexGuard，保持 future Send。
        let should_fail = {
            let mut fail_until = self.fail_first.lock().unwrap();
            let fail = n <= *fail_until;
            if !fail {
                *fail_until = 0;
            }
            fail
        };
        if should_fail {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "ledger temporarily unavailable".to_string(),
            });
        }
        // 发出「已进入 ledger」信号并等待放行（watch 保证不丢信号）。
        let _ = self.entered_tx.send(true);
        let mut rx = self.release_tx.subscribe();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        self.inner.record(record).await
    }

    async fn query(&self, query: &UsageQuery) -> Result<Vec<UsageRecord>, UsageLedgerError> {
        Ok(self.inner.query(query).await?)
    }

    async fn aggregate(&self, query: &UsageQuery) -> Result<UsageTotals, UsageLedgerError> {
        self.inner.aggregate(query).await
    }
}

#[cfg(test)]
mod terminal_flush_tests {
    use super::*;
    use agent_domain::{PrincipalId, SessionId, TenantId};
    use provider_control::{AcquireRequest, InMemoryCredentialPool};
    use std::collections::BTreeSet;
    use tenant_service::{
        InMemoryTenantPolicyEngine, PermissionProfile, PrincipalRole, TenantPolicy,
    };
    // usage_ledger 类型（InMemoryUsageLedger / UsageQuery / UsageRecord 等）经
    // 文件级 cfg(test) `use` + `super::*` 可见。

    /// P18-9 deny-first：未知 tenant-a 回落 Viewer，本模块的租户按「已配置」
    /// 处理（显式 Admin），聚焦 terminal flush 行为本身。
    fn test_engine() -> Arc<InMemoryTenantPolicyEngine> {
        let engine = Arc::new(InMemoryTenantPolicyEngine::default());
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
        engine
    }

    fn spawn_req(input_limit: Option<u64>, model: Option<ModelId>) -> SpawnRequest {
        SpawnRequest {
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-1"),
            parent_id: None,
            session_id: SessionId::new("session-1"),
            worktree_path: None,
            budget: Some(WorkerBudgetLimits {
                max_input_tokens: input_limit,
                ..WorkerBudgetLimits::default()
            }),
            model,
            acquire: None,
            task_deps: Vec::new(),
            task_description: None,
            task_max_retries: None,
        }
    }

    fn acquire_req() -> AcquireRequest {
        AcquireRequest {
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-1"),
            session_id: SessionId::new("session-1"),
            agent_id: AgentId::new("placeholder"),
            provider_id: None,
            account_id: None,
            trace_id: None,
        }
    }

    /// 终态触发方式（fail / cancel）。
    enum TerminalKind {
        Fail,
        Cancel,
    }

    impl TerminalKind {
        async fn finalize(self, supervisor: &AgentSupervisor, agent: &AgentId) {
            match self {
                TerminalKind::Fail => supervisor.fail(agent, "done".into()).await.unwrap(),
                TerminalKind::Cancel => {
                    supervisor.cancel_tree(agent).await.unwrap();
                }
            }
        }
    }

    fn exceeded_count(supervisor: &AgentSupervisor) -> usize {
        supervisor
            .events()
            .iter()
            .filter(|event| {
                matches!(event, OrchestrationEvent::BudgetExceeded { dimension, .. } if dimension == "input_tokens")
            })
            .count()
    }

    #[tokio::test]
    async fn fail_and_cancel_flush_ledger_like_complete() {
        for (label, terminal) in [
            ("fail", TerminalKind::Fail),
            ("cancel", TerminalKind::Cancel),
        ] {
            let ledger = Arc::new(InMemoryUsageLedger::new());
            let supervisor = AgentSupervisor::new(
                Arc::new(InMemoryCredentialPool::new(4)),
                test_engine(),
                ledger.clone(),
                SupervisorConfig::default(),
            );
            let mut req = spawn_req(None, Some(ModelId::new("mock-model")));
            req.acquire = Some(acquire_req());
            let agent = supervisor.spawn(req).await.unwrap();
            supervisor.record_usage(&agent, 7, 3, 0).await.unwrap();
            supervisor.start_worker(&agent).await.unwrap();

            let _ = terminal.finalize(&supervisor, &agent).await;

            let records = ledger.query(&UsageQuery::default()).await.unwrap();
            assert_eq!(
                records.len(),
                1,
                "{label}: fail/cancel 必须与 complete 一样 flush ledger"
            );
            assert_eq!(records[0].input_tokens, 7);
            assert_eq!(records[0].output_tokens, 3);
            assert_eq!(
                records[0].account_id, "local/default",
                "{label}: 归属必须来自 lease"
            );
        }
    }

    #[tokio::test]
    async fn terminal_flush_failure_retains_controller_and_is_retryable() {
        let ledger = FailingLedger::fail_first(1);
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            test_engine(),
            ledger.clone() as Arc<dyn UsageLedger>,
            SupervisorConfig::default(),
        );
        let agent = supervisor.spawn(spawn_req(None, None)).await.unwrap();
        supervisor.record_usage(&agent, 5, 0, 0).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();

        let err = supervisor.complete(&agent).await.unwrap_err();
        assert!(matches!(err, SupervisorError::UsageFlushPending(_)));
        assert_eq!(
            supervisor.state(&agent),
            Some(WorkerState::Completed),
            "终态转换已完成，flush 失败不回滚生命周期"
        );
        assert_eq!(ledger.record_calls(), 1);
        assert_eq!(
            ledger.query(&UsageQuery::default()).await.unwrap().len(),
            0,
            "flush 失败时账本不得写入"
        );

        supervisor.flush_usage(&agent).await.unwrap();
        assert_eq!(ledger.record_calls(), 2);
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 5);

        supervisor.flush_usage(&agent).await.unwrap();
        assert_eq!(
            ledger.record_calls(),
            2,
            "controller 已丢弃后不得重复 flush"
        );
    }

    #[tokio::test]
    async fn record_usage_rejected_after_terminal() {
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            test_engine(),
            Arc::new(InMemoryUsageLedger::new()),
            SupervisorConfig::default(),
        );
        let agent = supervisor.spawn(spawn_req(None, None)).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        supervisor.record_usage(&agent, 1, 0, 0).await.unwrap();
        supervisor.complete(&agent).await.unwrap();

        let err = supervisor
            .record_usage(&agent, 100, 0, 0)
            .await
            .unwrap_err();
        assert!(
            matches!(err, SupervisorError::WorkerTerminal(_)),
            "终态后 record_usage 必须被拒绝: {err:?}"
        );
    }

    #[tokio::test]
    async fn budget_exceeded_deduped_across_record_usage() {
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            test_engine(),
            Arc::new(InMemoryUsageLedger::new()),
            SupervisorConfig::default(),
        );
        let agent = supervisor.spawn(spawn_req(Some(10), None)).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();

        supervisor.record_usage(&agent, 20, 0, 0).await.unwrap();
        assert_eq!(exceeded_count(&supervisor), 1, "首次超限应告警");
        supervisor.record_usage(&agent, 5, 0, 0).await.unwrap();
        assert_eq!(exceeded_count(&supervisor), 1, "持续超限去重，不重复告警");
    }

    #[tokio::test]
    async fn diff_hard_exceeded_re_alarms_after_recovery() {
        let mk = || {
            WorkerBudgetController::new(WorkerBudgetLimits {
                max_input_tokens: Some(10),
                ..WorkerBudgetLimits::default()
            })
        };
        let expected = std::iter::once("input_tokens".to_string()).collect::<BTreeSet<String>>();
        let ctrl = mk();
        ctrl.record_tokens(20, 0);
        let over = ctrl.check();
        assert_eq!(ctrl.diff_hard_exceeded(&over), expected);
        assert!(ctrl.diff_hard_exceeded(&over).is_empty(), "持续超限去重");

        let ctrl2 = mk();
        ctrl2.record_tokens(5, 0);
        assert!(
            ctrl2.diff_hard_exceeded(&ctrl2.check()).is_empty(),
            "未超限不发"
        );
        ctrl2.record_tokens(15, 0);
        assert_eq!(
            ctrl2.diff_hard_exceeded(&ctrl2.check()),
            expected,
            "恢复后再次超限应重新告警"
        );
    }

    #[tokio::test]
    async fn hard_limit_is_ge_boundary() {
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            test_engine(),
            Arc::new(InMemoryUsageLedger::new()),
            SupervisorConfig::default(),
        );
        let agent = supervisor.spawn(spawn_req(Some(10), None)).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        supervisor.record_usage(&agent, 10, 0, 0).await.unwrap();
        assert!(
            supervisor.events().iter().any(|event| {
                matches!(event, OrchestrationEvent::BudgetExceeded { dimension, used, limit, .. } if dimension == "input_tokens" && *used == 10 && *limit == 10)
            }),
            "used == limit 必须判定为硬超限（>= 语义）"
        );
    }

    #[tokio::test]
    async fn cancel_tree_surfaces_pending_flush_with_receipt() {
        let ledger = FailingLedger::fail_first(2);
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(8)),
            test_engine(),
            ledger.clone() as Arc<dyn UsageLedger>,
            SupervisorConfig::default(),
        );
        let parent = supervisor.spawn(spawn_req(None, None)).await.unwrap();
        let child_req = SpawnRequest {
            parent_id: Some(parent.clone()),
            ..spawn_req(None, None)
        };
        let child = supervisor.spawn(child_req).await.unwrap();
        supervisor.record_usage(&parent, 3, 0, 0).await.unwrap();
        supervisor.record_usage(&child, 4, 0, 0).await.unwrap();
        supervisor.start_worker(&parent).await.unwrap();
        supervisor.start_worker(&child).await.unwrap();

        let err = supervisor.cancel_tree(&parent).await.unwrap_err();
        let SupervisorError::CancelTreeFlushPending { receipt, pending } = &err else {
            panic!("expected CancelTreeFlushPending, got {err:?}");
        };
        // 取消仍完成：全部节点 Cancelled、receipt 完整、lease 无泄漏。
        assert_eq!(receipt.cancelled_ids.len(), 2);
        assert!(receipt.cancelled_ids.contains(&parent));
        assert!(receipt.cancelled_ids.contains(&child));
        assert_eq!(supervisor.state(&parent), Some(WorkerState::Cancelled));
        assert_eq!(supervisor.state(&child), Some(WorkerState::Cancelled));
        // pending 必须含 agent id，且失败期间账本不得写入。
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&parent));
        assert!(pending.contains(&child));
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 0);

        // 逐节点重试：全部落账，不重复。
        supervisor.flush_usage(&parent).await.unwrap();
        supervisor.flush_usage(&child).await.unwrap();
        supervisor.flush_usage(&parent).await.unwrap();
        supervisor.flush_usage(&child).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 2);
        let inputs: BTreeSet<u64> = records.iter().map(|r| r.input_tokens).collect();
        assert_eq!(inputs, BTreeSet::from([3, 4]));
        assert_eq!(ledger.record_calls(), 4, "2 次失败 + 2 次重试后不得再写");
    }

    #[tokio::test]
    async fn flush_usage_rejected_for_active_worker_keeps_controller() {
        let ledger = Arc::new(InMemoryUsageLedger::new());
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            test_engine(),
            ledger.clone(),
            SupervisorConfig::default(),
        );
        let agent = supervisor.spawn(spawn_req(None, None)).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        supervisor.record_usage(&agent, 9, 1, 0).await.unwrap();

        // 活动 worker 误调用：拒绝且不得移除 controller。
        let err = supervisor.flush_usage(&agent).await.unwrap_err();
        assert!(
            matches!(err, SupervisorError::FlushNotTerminal(_)),
            "{err:?}"
        );

        // controller 未被移除：complete 仍能完整 flush（用量不丢）。
        supervisor.complete(&agent).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 9);
        assert_eq!(records[0].output_tokens, 1);
    }

    #[tokio::test]
    async fn flush_usage_requires_ctx_controller_pair() {
        let ledger = FailingLedger::fail_first(1);
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            test_engine(),
            ledger.clone() as Arc<dyn UsageLedger>,
            SupervisorConfig::default(),
        );
        let agent = supervisor.spawn(spawn_req(None, None)).await.unwrap();
        supervisor.record_usage(&agent, 5, 0, 0).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();
        let err = supervisor.complete(&agent).await.unwrap_err();
        assert!(matches!(err, SupervisorError::UsageFlushPending(_)));

        // 破坏成对性：controller 在、ctx 缺失（内部不一致的防御路径）。
        supervisor
            .flush_ctx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&agent);
        let err = supervisor.flush_usage(&agent).await.unwrap_err();
        assert!(
            matches!(err, SupervisorError::FlushContextMissing(_)),
            "{err:?}"
        );
        // controller 必须被保留（不吞 pending、不丢账）。
        assert!(supervisor
            .budget
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .contains_key(&agent));

        // 恢复 ctx 后重试成功。
        supervisor
            .flush_ctx
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(
                agent.clone(),
                LedgerContext {
                    credential_id: None,
                    tenant_id: TenantId::new("tenant-a"),
                    principal_id: PrincipalId::new("principal-1"),
                    account_id: "local/default".to_string(),
                    session_id: SessionId::new("session-1"),
                    agent_id: agent.clone(),
                    run_id: None,
                    provider_id: ProviderId::new("local"),
                    model_id: ModelId::new("unknown"),
                },
            );
        supervisor.flush_usage(&agent).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 5);
    }

    #[tokio::test]
    async fn concurrent_flush_usage_no_false_success_or_double_count() {
        let ledger = BlockingLedger::new(1);
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            test_engine(),
            ledger.clone() as Arc<dyn UsageLedger>,
            SupervisorConfig::default(),
        );
        let agent = supervisor.spawn(spawn_req(None, None)).await.unwrap();
        supervisor.record_usage(&agent, 5, 0, 0).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();

        // 制造 pending：终态 flush 失败一次，controller + ctx 保留。
        let err = supervisor.complete(&agent).await.unwrap_err();
        assert!(matches!(err, SupervisorError::UsageFlushPending(_)));

        let entered_rx = ledger.entered_rx();
        let flush_a = supervisor.flush_usage(&agent);
        let flush_b = async {
            // 等第一个 flush 进入 ledger 调用（在途标记已登记）后再并发调用。
            let mut rx = entered_rx;
            while !*rx.borrow() {
                rx.changed().await.unwrap();
            }
            let err = supervisor.flush_usage(&agent).await.unwrap_err();
            assert!(
                matches!(err, SupervisorError::UsageFlushPending(_)),
                "并发 flush 期间不得假成功: {err:?}"
            );
            ledger.release();
        };
        let (first, _) = tokio::join!(flush_a, flush_b);
        assert!(
            first.is_ok(),
            "第一个 flush 在 ledger 恢复后必须成功: {first:?}"
        );

        // 收尾 flush 为空操作；账本仅一条、不重复计账。
        supervisor.flush_usage(&agent).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 5);
        assert_eq!(
            ledger.record_calls(),
            2,
            "1 次失败 + 1 次并发成功，不得重复写入"
        );
    }

    #[tokio::test]
    async fn flush_usage_during_terminal_flush_reports_pending_not_success() {
        let ledger = BlockingLedger::new(0);
        let supervisor = AgentSupervisor::new(
            Arc::new(InMemoryCredentialPool::new(4)),
            test_engine(),
            ledger.clone() as Arc<dyn UsageLedger>,
            SupervisorConfig::default(),
        );
        let agent = supervisor.spawn(spawn_req(None, None)).await.unwrap();
        supervisor.record_usage(&agent, 5, 0, 0).await.unwrap();
        supervisor.start_worker(&agent).await.unwrap();

        let entered_rx = ledger.entered_rx();
        let complete_fut = supervisor.complete(&agent);
        let check_fut = async {
            // 终态 flush 在途（controller 已移出 budget、尚未提交）期间，
            // flush_usage 必须回报 pending 而非假成功。
            let mut rx = entered_rx;
            while !*rx.borrow() {
                rx.changed().await.unwrap();
            }
            let err = supervisor.flush_usage(&agent).await.unwrap_err();
            assert!(
                matches!(err, SupervisorError::UsageFlushPending(_)),
                "终态 flush 在途时不得假成功: {err:?}"
            );
            ledger.release();
        };
        let (completed, _) = tokio::join!(complete_fut, check_fut);
        assert!(
            completed.is_ok(),
            "ledger 恢复后 complete 必须成功: {completed:?}"
        );

        // 已提交：后续 flush_usage 为空操作，账本仅一条。
        supervisor.flush_usage(&agent).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 5);
    }

    // UsageRecord 由 ledger 内部产生；保留导入以表明 query 返回类型。
    const _: fn(&UsageRecord) = |_| {};
}
