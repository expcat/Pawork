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
    parent_workspace: Option<PathBuf>,
    worktree_allocator: Option<Arc<dyn WorktreeAllocator>>,
    task_graph: Option<Arc<TaskGraph>>,
    patch_merger: Option<Arc<PatchMerger>>,
    pending_patches: Arc<Mutex<BTreeMap<AgentId, PatchProposal>>>,
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
            parent_workspace: None,
            worktree_allocator: None,
            task_graph: None,
            patch_merger: None,
            pending_patches: Arc::new(Mutex::new(BTreeMap::new())),
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
            self.emit(OrchestrationEvent::ConcurrencyDenied {
                kind: "agents".to_string(),
                current: active_total,
                limit: self.config.max_agent_concurrency,
            });
            return Err(SupervisorError::PolicyDenied(format!(
                "agent concurrency limit reached: active {active_total} of max {}",
                self.config.max_agent_concurrency
            )));
        }

        // 3. 创建实例（worktree_path 初始来自请求，分配后可能被覆盖）。
        let agent_id = AgentId::new(format!(
            "agent-{}",
            self.next_agent_id.fetch_add(1, Ordering::Relaxed)
        ));
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
            Some(acquire) => match self.pool.acquire_guard(acquire.clone()).await {
                Ok(guard) => Some(guard),
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
            },
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
        self.workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(
                agent_id.clone(),
                WorkerEntry {
                    instance,
                    state: machine,
                    lease,
                    worktree: worktree_guard,
                    model: req.model.clone(),
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
    /// 累计用量 flush 到注入的 usage ledger（无用量时为空操作）。归属从
    /// lease（account / provider）与 spawn 请求（model）取真实值，不再
    /// 硬编码 `"unknown"`；worktree 显式释放；TaskGraph 推进为 Completed。
    pub async fn complete(&self, agent_id: &AgentId) -> Result<(), SupervisorError> {
        let (lease, parent, instance, controller, worktree, model) = {
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
            (
                entry.lease.take(),
                entry.instance.parent_id.clone(),
                instance,
                controller,
                entry.worktree.take(),
                model,
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
        // 读完归属后释放 lease（默认 outcome 即 Completed；Drop 触发同步幂等释放）。
        drop(lease);
        if let Some(controller) = controller {
            let ctx = LedgerContext {
                tenant_id: instance.tenant_id.clone(),
                principal_id: instance.principal_id.clone(),
                account_id,
                session_id: instance.session_id.clone(),
                agent_id: instance.agent_id.clone(),
                run_id: None,
                provider_id,
                model_id,
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
    /// `WorkerFailed` → 从父的活跃 children 中移除。worktree 显式释放；
    /// TaskGraph 推进为 Failed 并发出 TaskFailed。
    pub async fn fail(&self, agent_id: &AgentId, reason: String) -> Result<(), SupervisorError> {
        let (lease, parent, worktree) = {
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
            (
                entry.lease.take(),
                entry.instance.parent_id.clone(),
                entry.worktree.take(),
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
    /// worktree 显式释放（best-effort）；TaskGraph 推进为 Cancelled 并发出
    /// `TaskCancelled`。终态节点跳过；重复调用是幂等的（第二次不再取消
    /// 任何节点、不重复释放）。
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
            let (cancelled, lease, worktree) = {
                let mut workers = self
                    .workers
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let Some(entry) = workers.get_mut(&id) else {
                    continue;
                };
                if entry.state.state().is_terminal() {
                    (false, None, None)
                } else {
                    let _ = entry.state.apply(WorkerTransition::BeginCancel);
                    let _ = entry.state.apply(WorkerTransition::Cancel);
                    (true, entry.lease.take(), entry.worktree.take())
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

    /// 记录一次用量并检查预算（B1）：对硬超限维度发出 `BudgetExceeded`。
    ///
    /// 用量经该 worker 的 [`WorkerBudgetController`] 累加；`check()` 报告的
    /// 每个硬超限维度以当前用量与对应上限发出一个 `BudgetExceeded` 事件。
    pub async fn record_usage(
        &self,
        agent_id: &AgentId,
        input: u64,
        output: u64,
        cost_micros: u64,
    ) -> Result<(), SupervisorError> {
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
        let (used_input, used_output, used_cost) = controller.usage();
        let limits = controller.limits();
        for dimension in &report.hard_exceeded {
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
    use agent_domain::{ModelId, PrincipalId, SessionId, TenantId};
    use async_trait::async_trait;
    use provider_control::{AccountId, AcquireRequest, InMemoryCredentialPool};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Mutex;
    use tenant_service::{InMemoryTenantPolicyEngine, TenantPolicy};
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
            Arc::new(InMemoryTenantPolicyEngine::default()),
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
            Arc::new(InMemoryTenantPolicyEngine::default()),
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

        let records = ledger.query(&UsageQuery::default()).await;
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
            Arc::new(InMemoryTenantPolicyEngine::default()),
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
}
