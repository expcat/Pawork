//! AgentSupervisor：spawn / 注册表 / 取消树 / 崩溃恢复（P12-1 + P12-6）。
//!
//! - 所有 worker 都必须经 [`AgentSupervisor::spawn`] 创建，禁止脱离监督的
//!   `tokio::spawn`；
//! - 生命周期全部事件化、可重放（[`crate::OrchestrationEvent`]）；
//! - 取消树：取消 parent 递归联动全部后代，lease 以
//!   [`provider_control::LeaseOutcome::Cancelled`] 幂等释放，**不惩罚账号健康**；
//! - 恢复：重放事件后，任何仍处于活动态且无存活运行时的 worker 一律标记
//!   `Failed`，不留悬挂 worker。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_domain::{AgentId, CancellationToken, ModelId, ProviderId};
use provider_control::{CredentialPool, LeaseGuard, LeaseOutcome};
use tenant_service::TenantPolicyEngine;
use usage_ledger::UsageLedger;

use crate::budget::{LedgerContext, WorkerBudgetController, WorkerBudgetLimits};
use crate::identity::{AgentInstance, WorkerRole};
use crate::lifecycle::{
    replay_workers, OrchestrationEvent, WorkerState, WorkerStateMachine, WorkerTransition,
};

/// 注册表中的单个 worker 条目。
pub struct WorkerEntry {
    /// 不可变身份。
    pub instance: AgentInstance,
    /// 生命周期状态机。
    pub state: WorkerStateMachine,
    /// 持有的 credential lease 守卫（未申请时为 `None`）。
    pub lease: Option<LeaseGuard>,
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
}

/// 编排 Supervisor：集中拥有 spawn / assign / cancel_tree / 恢复。
pub struct AgentSupervisor {
    workers: Arc<Mutex<BTreeMap<AgentId, WorkerEntry>>>,
    cancel_tokens: Arc<Mutex<BTreeMap<AgentId, CancellationToken>>>,
    children: Arc<Mutex<BTreeMap<AgentId, Vec<AgentId>>>>,
    pool: Arc<dyn CredentialPool>,
    policy: Arc<dyn TenantPolicyEngine>,
    ledger: Arc<dyn UsageLedger>,
    event_log: Arc<Mutex<Vec<OrchestrationEvent>>>,
    next_agent_id: AtomicU64,
    budget: Arc<Mutex<BTreeMap<AgentId, WorkerBudgetController>>>,
    config: SupervisorConfig,
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
            pool,
            policy,
            ledger,
            event_log: Arc::new(Mutex::new(Vec::new())),
            next_agent_id: AtomicU64::new(0),
            budget: Arc::new(Mutex::new(BTreeMap::new())),
            config,
        }
    }

    /// 根 Supervisor 身份（所有 parent 的公共 owner）。
    pub fn parent_id(&self) -> AgentId {
        AgentId::new("supervisor")
    }

    /// 创建并启动一个 worker。
    ///
    /// 流程：租户 agent 并发与模型白名单闸门 → 创建实例 → `WorkerCreated` →
    /// Admit → `WorkerAdmitted` → 申请 lease（可选）→ Start → `WorkerStarted`
    /// → 注册 child 与取消令牌。lease 申请失败时把该 worker 标记 `Failed`
    /// 后返回错误，保证事件流一致、恢复时不留悬挂 worker。
    pub async fn spawn(&self, req: SpawnRequest) -> Result<AgentId, SupervisorError> {
        // 1. 策略闸门：租户 agent 并发 + 模型白名单。
        let active_for_tenant = self.active_worker_count(Some(&req.tenant_id));
        self.policy
            .check_agent_concurrency(&req.tenant_id, active_for_tenant)
            .await
            .map_err(|error| SupervisorError::PolicyDenied(error.to_string()))?;
        if let Some(model) = &req.model {
            self.policy
                .check_model(&req.tenant_id, model)
                .await
                .map_err(|error| SupervisorError::PolicyDenied(error.to_string()))?;
        }
        // 2. 本地并发闸门（与租户策略相互独立）。
        let active_total = self.active_worker_count(None);
        if active_total >= self.config.max_agent_concurrency {
            return Err(SupervisorError::PolicyDenied(format!(
                "agent concurrency limit reached: active {active_total} of max {}",
                self.config.max_agent_concurrency
            )));
        }

        // 3. 创建实例并发出 WorkerCreated。
        let agent_id = AgentId::new(format!(
            "agent-{}",
            self.next_agent_id.fetch_add(1, Ordering::Relaxed)
        ));
        let now = now_ms();
        let worktree_path = req
            .worktree_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned());
        let (role, instance) = match &req.parent_id {
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
        self.emit(OrchestrationEvent::WorkerCreated {
            agent_id: agent_id.clone(),
            tenant_id: req.tenant_id.clone(),
            parent_id: req.parent_id.clone(),
            role,
            session_id: req.session_id.clone(),
            worktree_path,
            created_at_ms: now,
        });

        // 4. Admit（admit 折叠进 spawn）。
        let mut machine = WorkerStateMachine::from_state(WorkerState::Created);
        machine
            .apply(WorkerTransition::Admit)
            .map_err(SupervisorError::IllegalLifecycle)?;
        self.emit(OrchestrationEvent::WorkerAdmitted {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
        });

        // 5. 申请 lease（可选）。
        let lease = match &req.acquire {
            Some(acquire) => match self.pool.acquire_guard(acquire.clone()).await {
                Ok(guard) => Some(guard),
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
                    };
                    self.workers
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .insert(agent_id.clone(), entry);
                    return Err(SupervisorError::PoolAcquire(error.to_string()));
                }
            },
            None => None,
        };

        // 6. Start → WorkerStarted。
        machine
            .apply(WorkerTransition::Start)
            .map_err(SupervisorError::IllegalLifecycle)?;
        self.emit(OrchestrationEvent::WorkerStarted {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
        });

        // 7. 注册 child 与取消令牌。
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

        // 8. 注册 worker 条目与预算控制器。
        self.workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(
                agent_id.clone(),
                WorkerEntry {
                    instance,
                    state: machine,
                    lease,
                },
            );
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
    /// 累计用量 flush 到注入的 usage ledger（无用量时为空操作）。
    pub async fn complete(&self, agent_id: &AgentId) -> Result<(), SupervisorError> {
        let (lease, parent, instance, controller) = {
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
            let controller = self
                .budget
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(agent_id);
            (
                entry.lease.take(),
                entry.instance.parent_id.clone(),
                instance,
                controller,
            )
        };
        // 默认 outcome 即 Completed；Drop 触发同步幂等释放。
        drop(lease);
        if let Some(controller) = controller {
            let ctx = LedgerContext {
                tenant_id: instance.tenant_id.clone(),
                principal_id: instance.principal_id.clone(),
                account_id: "unknown".to_string(),
                session_id: instance.session_id.clone(),
                agent_id: instance.agent_id.clone(),
                run_id: None,
                provider_id: ProviderId::new("unknown"),
                model_id: ModelId::new("unknown"),
            };
            if let Err(error) = controller.flush_to_ledger(self.ledger.as_ref(), &ctx).await {
                tracing::warn!(%agent_id, %error, "failed to flush worker usage to ledger");
            }
        }
        self.emit(OrchestrationEvent::WorkerCompleted {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
        });
        self.remove_child(parent.as_ref(), agent_id);
        Ok(())
    }

    /// 失败：释放 lease（`LeaseOutcome::Failed`，计入连续失败）→ Fail →
    /// `WorkerFailed` → 从父的活跃 children 中移除。
    pub async fn fail(&self, agent_id: &AgentId, reason: String) -> Result<(), SupervisorError> {
        let (lease, parent) = {
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
            (entry.lease.take(), entry.instance.parent_id.clone())
        };
        if let Some(mut guard) = lease {
            *guard.outcome_mut() = LeaseOutcome::Failed;
            drop(guard);
        }
        self.emit(OrchestrationEvent::WorkerFailed {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
            reason,
        });
        self.remove_child(parent.as_ref(), agent_id);
        Ok(())
    }

    /// 取消树：取消 `agent_id` 及其全部后代（BFS 遍历 children 图）。
    ///
    /// 每个节点：取消令牌 → `Cancelling`（`WorkerCancelling`）→ `Cancelled`
    /// （`WorkerCancelled`）→ 以 [`LeaseOutcome::Cancelled`] 幂等释放 lease。
    /// 终态节点跳过；重复调用是幂等的（第二次不再取消任何节点、不重复释放）。
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
        for id in nodes {
            if let Some(token) = self.cancel_token(&id) {
                token.cancel();
            }
            let (cancelled, lease) = {
                let mut workers = self
                    .workers
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let Some(entry) = workers.get_mut(&id) else {
                    continue;
                };
                if entry.state.state().is_terminal() {
                    (false, None)
                } else {
                    let _ = entry.state.apply(WorkerTransition::BeginCancel);
                    let _ = entry.state.apply(WorkerTransition::Cancel);
                    (true, entry.lease.take())
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
            if let Some(mut guard) = lease {
                *guard.outcome_mut() = LeaseOutcome::Cancelled;
                // Drop 触发同步幂等释放；Cancelled 只累加取消计数，
                // 不累加连续失败（不惩罚账号健康）。
                drop(guard);
                leases_released += 1;
            }
            cancelled_ids.push(id);
        }
        Ok(CancelTreeReceipt {
            cancelled_ids,
            leases_released,
        })
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

    /// 活动 worker 计数（`tenant = None` 时全局）。
    fn active_worker_count(&self, tenant: Option<&agent_domain::TenantId>) -> u64 {
        self.workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .values()
            .filter(|entry| {
                entry.state.state().is_active()
                    && tenant.is_none_or(|tenant| entry.instance.tenant_id == *tenant)
            })
            .count() as u64
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{PrincipalId, SessionId, TenantId};
    use provider_control::{AccountId, AcquireRequest, InMemoryCredentialPool};
    use tenant_service::{InMemoryTenantPolicyEngine, TenantPolicy};
    use usage_ledger::InMemoryUsageLedger;

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
        }
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
}
