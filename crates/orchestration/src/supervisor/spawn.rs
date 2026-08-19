//! spawn：准入、lease / worktree 分配与 worker 注册。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use pawork_domain::{AgentId, CancellationToken, ModelId};
use pawork_control_plane::credential::LeaseOutcome;
use pawork_control_plane::{
    IdentityContext, Permission, PolicyDecisionEvent, PolicyDecisionKind, PolicyGate,
};
use pawork_control_plane::UsageQuery;

use crate::budget::WorkerBudgetController;
use crate::identity::{AgentInstance, WorkerRole};
use crate::lifecycle::{OrchestrationEvent, WorkerState, WorkerStateMachine, WorkerTransition};
use crate::task_graph::{AgentTask, TaskId, TaskState};
use crate::worktree::WorktreeGuard;
use crate::budget::WorkerBudgetLimits;

use super::{now_ms, AgentSupervisor, SupervisorError, WorkerEntry};

#[derive(Clone, Debug)]
pub struct SpawnRequest {
    /// 租户。
    pub tenant_id: pawork_domain::TenantId,
    /// 主体。
    pub principal_id: pawork_domain::PrincipalId,
    /// 父代理；`None` 表示创建根（Parent）。
    pub parent_id: Option<AgentId>,
    /// 会话。
    pub session_id: pawork_domain::SessionId,
    /// 独立 worktree 路径（可选）。
    pub worktree_path: Option<PathBuf>,
    /// 预算覆盖（`None` 使用 Supervisor 默认预算）。
    pub budget: Option<WorkerBudgetLimits>,
    /// 模型（可选；提供时经租户策略模型白名单闸门）。
    pub model: Option<ModelId>,
    /// 申请 credential lease 的请求（`None` 不申请）。
    pub acquire: Option<pawork_control_plane::credential::AcquireRequest>,
    /// 任务依赖（可选；配置 TaskGraph 时注册）。
    pub task_deps: Vec<TaskId>,
    /// 任务描述（可选）。
    pub task_description: Option<String>,
    /// 最大重试次数（可选）。
    pub task_max_retries: Option<u32>,
}


/// 并发预约失败原因（区分全局本地闸门与租户策略闸门，便于审计）。
pub(crate) enum ConcurrencyReservationError {
    /// 全局 agent 并发上限（本地闸门）。
    Global { current: u64, limit: u64 },
    /// 租户 agent 并发上限（策略闸门）。
    Tenant { current: u64, max: u64 },
}

/// spawn 的在途并发槽位预约（RAII）。drop 时从 [`AgentSupervisor::reservations`]
/// 移除预约的 agent_id（幂等：成功兑现路径已移除，drop 再移除为 no-op）。
pub(crate) struct ConcurrencyReservation {
    reservations: Arc<Mutex<BTreeMap<AgentId, pawork_domain::TenantId>>>,
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


impl AgentSupervisor {
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
        // 0. parent 准入：存在、同 tenant、同 session、状态可派生。失败不写
        //    children / workers，也不占用并发预约。
        self.validate_parent(&req)?;
        // 1. 并发预约（race-free）：与活动 worker 计数在单一临界区内合并判定
        //    全局 / 租户并发，杜绝 check-then-act 超配。预约以 RAII 归还——
        //    spawn 任一后续步骤失败（闸门拒绝 / worktree / lease / 注册）都自动
        //    归还槽位，绝不永久占用额度。agent_id 先于预约生成，使预约、worktree、
        //    lease、注册全程共享同一 canonical 身份。
        let agent_id = AgentId::new(format!(
            "agent-{}",
            self.next_agent_id.fetch_add(1, Ordering::Relaxed)
        ));
        if let Some(max_depth) = self.config.max_worker_depth {
            let depth = self.worker_depth(req.parent_id.as_ref());
            if depth > max_depth {
                let reason = format!(
                    "worker depth limit exceeded: depth={depth} max={max_depth}"
                );
                self.record_policy_denial(&req, PolicyGate::AgentSpawn, &reason);
                self.emit(OrchestrationEvent::ConcurrencyDenied {
                    kind: "depth".to_string(),
                    current: depth,
                    limit: max_depth,
                });
                return Err(SupervisorError::PolicyDenied(reason));
            }
        }
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
                            return Err(SupervisorError::PolicyDenied(reason_msg));
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


    /// 原子并发预约：在单一临界区内把「活动 worker + 在途 reservations」合并
    /// 计数，校验全局本地上限与租户策略 `max_concurrent_agents`，通过后插入
    /// 一条 reservation 并返回 RAII 守卫。租户上限取自同步 `policy()`，全程
    /// 不跨 await，杜绝 spawn 的 check-then-act 超配（并发调用串行化在锁内）。
    fn reserve_concurrency(
        &self,
        agent_id: AgentId,
        tenant_id: &pawork_domain::TenantId,
    ) -> Result<ConcurrencyReservation, ConcurrencyReservationError> {
        let mut reservations = self
            .reservations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let workers = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let count = |tid: Option<&pawork_domain::TenantId>| -> u64 {
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



    /// parent 必须存在于 workers、与本次请求同 tenant / session，且状态允许派生
    ///（活动且非 Cancelling）。失败返回 PolicyDenied，调用方不得写 children / workers。
    fn validate_parent(&self, req: &SpawnRequest) -> Result<(), SupervisorError> {
        let Some(parent_id) = req.parent_id.as_ref() else {
            return Ok(());
        };
        let reason = {
            let workers = self
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            match workers.get(parent_id) {
                None => format!("parent agent {parent_id} does not exist"),
                Some(parent) if parent.instance.tenant_id != req.tenant_id => {
                    format!(
                        "parent agent {parent_id} tenant mismatch: parent={} request={}",
                        parent.instance.tenant_id, req.tenant_id
                    )
                }
                Some(parent) if parent.instance.session_id != req.session_id => {
                    format!(
                        "parent agent {parent_id} session mismatch: parent={} request={}",
                        parent.instance.session_id, req.session_id
                    )
                }
                Some(parent)
                    if !parent.state.state().is_active()
                        || parent.state.state() == WorkerState::Cancelling =>
                {
                    format!(
                        "parent agent {parent_id} state {} cannot spawn children",
                        parent.state.state()
                    )
                }
                Some(_) => return Ok(()),
            }
        };
        self.record_policy_denial(req, PolicyGate::AgentSpawn, &reason);
        Err(SupervisorError::PolicyDenied(reason))
    }

    /// 沿 parent_id 计深度：根（无 parent）为 0，子代为 1，孙代为 2。
    fn worker_depth(&self, parent_id: Option<&AgentId>) -> u64 {
        let mut depth = 0u64;
        let mut current = parent_id.cloned();
        let workers = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = current {
            if !seen.insert(id.clone()) {
                break;
            }
            depth = depth.saturating_add(1);
            current = workers.get(&id).and_then(|entry| entry.instance.parent_id.clone());
        }
        depth
    }

}

/// 校验 pool 返回的 lease 作用域与本次 spawn 的 canonical 请求一致。
///
/// 不信任调用方拼接的 [`pawork_control_plane::credential::AcquireRequest`]（已在闸口拒绝错配），
/// 也不信任 pool 返回的 lease 内容：tenant / principal / session / agent 必须与
/// [`SpawnRequest`] 一致，请求显式指定的 provider / account 必须与 lease 一致。
/// 任何错配（恶意 / 故障 pool）返回原因串，调用方据此 fail-closed 释放 lease。
fn validate_lease_scope(
    lease: &pawork_control_plane::credential::CredentialLease,
    req: &SpawnRequest,
    agent_id: &AgentId,
    acquire: &pawork_control_plane::credential::AcquireRequest,
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


/// 一天的毫秒数（日预算窗口按 UTC 日对齐）。
const MS_PER_DAY: u64 = 86_400_000;
