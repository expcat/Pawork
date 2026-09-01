//! AgentSupervisor：spawn / 注册表 / 取消树 / 崩溃恢复（P12-1 + P12-6）。
//!
//! - 所有 worker 都必须经 [`AgentSupervisor::spawn`] 创建，禁止脱离监督的
//!   `tokio::spawn`；
//! - 生命周期全部事件化、可重放（[`crate::OrchestrationEvent`]）；
//! - 取消树：取消 parent 递归联动全部后代，lease 以
//!   [`pawork_control_plane::credential::LeaseOutcome::Cancelled`] 幂等释放，**不惩罚账号健康**；
//! - 恢复：[`AgentSupervisor::recover_report`] 为 report-only——重放事件后把
//!   活动孤儿在**报告**中记为 `Failed`，不重建 `WorkerEntry` / children /
//!   cancel token，也不 emit 恢复事件。

mod budget_gate;
mod cancel_tree;
mod recovery;
mod registry;
mod spawn;

pub use cancel_tree::CancelTreeReceipt;
pub use recovery::RecoveryReport;
pub use registry::WorkerEntry;
pub use spawn::SpawnRequest;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use pawork_control_plane::credential::{CredentialPool, LeaseOutcome};
use pawork_control_plane::{TenantPolicyEngine, UsageLedger};
use pawork_domain::{AgentId, CancellationToken, ModelId, ProviderId};

#[cfg(test)]
use pawork_control_plane::UsageQuery;
#[cfg(test)]
use pawork_control_plane::{InMemoryUsageLedger, UsageLedgerError, UsageRecord, UsageTotals};

use crate::budget::{LedgerContext, WorkerBudgetController, WorkerBudgetLimits};
#[cfg(test)]
use crate::identity::WorkerRole;
#[cfg(test)]
use crate::lifecycle::{replay_workers, WorkerState};
use crate::lifecycle::{OrchestrationEvent, WorkerTransition};
use crate::merge::{
    ConflictReport, MergeDecision, MergeOutcome, PatchMerger, PatchProposal, WorkerPatch,
};
#[cfg(test)]
use crate::task_graph::TaskState;
use crate::task_graph::{TaskGraph, TaskId};
use crate::worktree::WorktreeAllocator;

use registry::TerminalTake;

/// Supervisor 配置。
#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    /// 本 Supervisor 允许的最大 agent 并发（本地闸门，租户策略之外）。
    pub max_agent_concurrency: u64,
    /// 默认账号侧并发（创建 `CredentialPool` 时的建议值；本 Supervisor 不创建池）。
    pub default_pool_concurrency: u64,
    /// spawn 未显式携带预算时使用的默认预算。
    pub budget: WorkerBudgetLimits,
    /// 沿 parent_id 的最大 worker 深度；`None` 表示不限制（与 V1 默认行为一致）。
    pub max_worker_depth: Option<u64>,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            max_agent_concurrency: 16,
            default_pool_concurrency: 4,
            budget: WorkerBudgetLimits::default(),
            max_worker_depth: None,
        }
    }
}

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

/// 编排 Supervisor：集中拥有 spawn / assign / cancel_tree / 恢复。
pub struct AgentSupervisor {
    pub(crate) workers: Arc<Mutex<BTreeMap<AgentId, WorkerEntry>>>,
    pub(crate) cancel_tokens: Arc<Mutex<BTreeMap<AgentId, CancellationToken>>>,
    pub(crate) children: Arc<Mutex<BTreeMap<AgentId, Vec<AgentId>>>>,
    /// 并发 spawn 的在途槽位预约（agent_id → tenant_id）。与活动 worker 计数
    /// 在单一临界区内合并判定全局 / 租户并发，杜绝 spawn 的 check-then-act
    /// 超配；RAII [`spawn::ConcurrencyReservation`] 在 spawn 任一后续步骤失败时归还。
    pub(crate) reservations: Arc<Mutex<BTreeMap<AgentId, pawork_domain::TenantId>>>,
    pub(crate) pool: Arc<dyn CredentialPool>,
    pub(crate) policy: Arc<dyn TenantPolicyEngine>,
    pub(crate) ledger: Arc<dyn UsageLedger>,
    pub(crate) event_log: Arc<Mutex<Vec<OrchestrationEvent>>>,
    pub(crate) next_agent_id: AtomicU64,
    pub(crate) budget: Arc<Mutex<BTreeMap<AgentId, WorkerBudgetController>>>,
    pub(crate) config: SupervisorConfig,
    pub(crate) parent_workspace: Option<PathBuf>,
    pub(crate) worktree_allocator: Option<Arc<dyn WorktreeAllocator>>,
    pub(crate) task_graph: Option<Arc<TaskGraph>>,
    pub(crate) patch_merger: Option<Arc<PatchMerger>>,
    pub(crate) pending_patches: Arc<Mutex<BTreeMap<AgentId, PatchProposal>>>,
    /// 终态 flush 失败时缓存的 ledger 归属上下文，供 `flush_usage` 重试复用，
    /// 保证重试不丢失 account / provider / model 归属（lease 已释放后仍可对账）。
    pub(crate) flush_ctx: Arc<Mutex<BTreeMap<AgentId, LedgerContext>>>,
    /// 用量 flush 在途标记（终态路径与 `flush_usage` 重试共用）：flush 在途
    /// 期间并发调用方收到 [`SupervisorError::UsageFlushPending`] 而非假成功。
    pub(crate) flush_in_flight: Arc<Mutex<BTreeSet<AgentId>>>,
}

impl AgentSupervisor {
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

    /// 正常完成：释放 lease（`LeaseOutcome::Completed`，幂等）→ Complete →
    /// `WorkerCompleted` → 从父的活跃 children 中移除。同时把该 worker 的
    /// 累计用量 flush 到注入的 usage ledger（无用量时为空操作）。归属从
    /// lease（account / provider）与 spawn 请求（model）取真实值，不再
    /// 硬编码 `"unknown"`；worktree 显式释放；TaskGraph 推进为 Completed。
    pub async fn complete(&self, agent_id: &AgentId) -> Result<(), SupervisorError> {
        let TerminalTake {
            mut lease,
            parent,
            instance,
            controller,
            worktree,
            model,
            ticket,
        } = self.apply_terminal_and_take(agent_id, WorkerTransition::Complete)?;
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
        let TerminalTake {
            lease,
            parent,
            instance,
            controller,
            worktree,
            model,
            ticket,
        } = self.apply_terminal_and_take(agent_id, WorkerTransition::Fail)?;
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
    use async_trait::async_trait;
    use pawork_control_plane::credential::{
        AccountId, AcquireRequest, CredentialLease, CredentialPool, InMemoryCredentialPool,
        LeaseGuard, LeaseId, LeaseOutcome, PoolError, ReleaseReceipt,
    };
    use pawork_control_plane::{
        InMemoryTenantPolicyEngine, PermissionProfile, PolicyDecisionKind, PolicyGate,
        PrincipalRole, TenantPolicy,
    };
    use pawork_control_plane::{InMemoryUsageLedger, UsageQuery};
    use pawork_domain::{ModelId, PrincipalId, SessionId, TenantId};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Mutex;

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
        pool: Arc<dyn CredentialPool>,
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
        harness_with_config(pool, policy, SupervisorConfig::default())
    }

    fn harness_with_config(
        pool: Arc<dyn CredentialPool>,
        policy: Arc<InMemoryTenantPolicyEngine>,
        config: SupervisorConfig,
    ) -> AgentSupervisor {
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
        AgentSupervisor::new(pool, policy, Arc::new(InMemoryUsageLedger::new()), config)
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
    async fn recover_report_is_report_only_and_does_not_rebuild_operable_state() {
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

        let report = supervisor.recover_report(&events).await;
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
        assert!(report
            .recovered_states
            .values()
            .all(|state| state.is_terminal()));
        // report-only：Supervisor 自身不被重建，不能据此 cancel / assign / flush。
        assert!(supervisor.state(&AgentId::new("a")).is_none());
        assert!(supervisor.state(&AgentId::new("b")).is_none());
        assert!(supervisor.state(&AgentId::new("c")).is_none());
        assert!(supervisor.cancel_token(&AgentId::new("a")).is_none());
        assert!(supervisor.events().is_empty());
        assert!(supervisor
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty());
        assert_eq!(supervisor.active_worker_count(None), 0);
    }

    #[tokio::test]
    async fn spawn_rejects_missing_parent_without_writing_children_or_workers() {
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let mut req = spawn_request(None);
        req.parent_id = Some(AgentId::new("no-such-parent"));
        let err = supervisor.spawn(req).await.unwrap_err();
        assert!(
            matches!(err, SupervisorError::PolicyDenied(ref reason) if reason.contains("does not exist")),
            "{err:?}"
        );
        assert!(supervisor.state(&AgentId::new("no-such-parent")).is_none());
        assert_eq!(supervisor.active_worker_count(None), 0);
        assert!(supervisor.events().is_empty());
        assert!(supervisor
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty());
        assert!(supervisor
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_empty());
    }

    #[tokio::test]
    async fn spawn_rejects_cross_tenant_and_cross_session_parent() {
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(8)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let parent = supervisor.spawn(spawn_request(None)).await.unwrap();
        let before_events = supervisor.events().len();

        let cross_tenant = SpawnRequest {
            parent_id: Some(parent.clone()),
            tenant_id: TenantId::new("tenant-b"),
            ..spawn_request(None)
        };
        let err = supervisor.spawn(cross_tenant).await.unwrap_err();
        assert!(
            matches!(err, SupervisorError::PolicyDenied(ref reason) if reason.contains("tenant mismatch")),
            "{err:?}"
        );

        let cross_session = SpawnRequest {
            parent_id: Some(parent.clone()),
            session_id: SessionId::new("session-other"),
            ..spawn_request(None)
        };
        let err = supervisor.spawn(cross_session).await.unwrap_err();
        assert!(
            matches!(err, SupervisorError::PolicyDenied(ref reason) if reason.contains("session mismatch")),
            "{err:?}"
        );

        assert_eq!(supervisor.state(&parent), Some(WorkerState::Starting));
        assert_eq!(supervisor.active_worker_count(None), 1);
        assert!(supervisor
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&parent)
            .is_none());
        assert_eq!(
            supervisor
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .len(),
            1
        );
        assert_eq!(
            supervisor.events().len(),
            before_events,
            "rejected spawn must not emit worker lifecycle events"
        );
    }

    #[tokio::test]
    async fn spawn_rejects_terminal_parent() {
        let supervisor = harness(
            Arc::new(InMemoryCredentialPool::new(4)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
        );
        let parent = supervisor.spawn(spawn_request(None)).await.unwrap();
        supervisor.start_worker(&parent).await.unwrap();
        supervisor.complete(&parent).await.unwrap();
        let req = SpawnRequest {
            parent_id: Some(parent.clone()),
            ..spawn_request(None)
        };
        let err = supervisor.spawn(req).await.unwrap_err();
        assert!(
            matches!(err, SupervisorError::PolicyDenied(ref reason) if reason.contains("cannot spawn")),
            "{err:?}"
        );
        assert!(supervisor
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&parent)
            .is_none());
        assert_eq!(supervisor.active_worker_count(None), 0);
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

    /// 并行 `spawn` 压力：`n` 路 `tokio::spawn` 同时冲并发闸门。
    async fn join_spawns(
        supervisor: &Arc<AgentSupervisor>,
        n: usize,
        with_acquire: bool,
    ) -> (Vec<AgentId>, Vec<SupervisorError>) {
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..n {
            let supervisor = Arc::clone(supervisor);
            set.spawn(async move {
                let acquire = with_acquire.then(|| acquire_request(&AgentId::new("placeholder")));
                supervisor.spawn(spawn_request(acquire)).await
            });
        }
        let mut admitted = Vec::new();
        let mut errors = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined.expect("parallel spawn task") {
                Ok(id) => admitted.push(id),
                Err(err) => errors.push(err),
            }
        }
        (admitted, errors)
    }

    /// 恶意 / 故障 pool：acquire 成功，但返回的 lease 作用域与 canonical 请求错配。
    #[derive(Clone, Copy, Debug)]
    enum LeaseScopeMismatch {
        Tenant,
        Principal,
        Session,
        Agent,
        Provider,
        Account,
    }

    impl LeaseScopeMismatch {
        fn reason(self) -> &'static str {
            match self {
                Self::Tenant => "tenant mismatch",
                Self::Principal => "principal mismatch",
                Self::Session => "session mismatch",
                Self::Agent => "agent mismatch",
                Self::Provider => "provider mismatch",
                Self::Account => "account mismatch",
            }
        }
    }

    struct MismatchingPool {
        inner: InMemoryCredentialPool,
        kind: LeaseScopeMismatch,
        releases: Mutex<Vec<(LeaseId, LeaseOutcome)>>,
    }

    impl MismatchingPool {
        fn new(kind: LeaseScopeMismatch) -> Self {
            Self {
                inner: InMemoryCredentialPool::new(4),
                kind,
                releases: Mutex::new(Vec::new()),
            }
        }

        fn forge(&self, mut req: AcquireRequest) -> AcquireRequest {
            match self.kind {
                LeaseScopeMismatch::Tenant => {
                    req.tenant_id = TenantId::new("tenant-evil");
                }
                LeaseScopeMismatch::Principal => {
                    req.principal_id = PrincipalId::new("principal-evil");
                }
                LeaseScopeMismatch::Session => {
                    req.session_id = SessionId::new("session-evil");
                }
                LeaseScopeMismatch::Agent => {
                    req.agent_id = AgentId::new("agent-evil");
                }
                LeaseScopeMismatch::Provider => {
                    req.provider_id = Some(ProviderId::new("evil-provider"));
                }
                LeaseScopeMismatch::Account => {
                    req.account_id = Some(AccountId::new("evil-account"));
                }
            }
            req
        }

        fn released(&self) -> Vec<(LeaseId, LeaseOutcome)> {
            self.releases
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl CredentialPool for MismatchingPool {
        async fn acquire(&self, req: AcquireRequest) -> Result<CredentialLease, PoolError> {
            self.inner.acquire(self.forge(req)).await
        }

        async fn acquire_guard(&self, req: AcquireRequest) -> Result<LeaseGuard, PoolError> {
            self.inner.acquire_guard(self.forge(req)).await
        }

        async fn release(
            &self,
            lease_id: LeaseId,
            outcome: LeaseOutcome,
        ) -> Result<ReleaseReceipt, PoolError> {
            self.releases
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push((lease_id.clone(), outcome));
            self.inner.release(lease_id, outcome).await
        }

        fn active_count(&self, account: &AccountId) -> u64 {
            self.inner.active_count(account)
        }

        fn account_health(
            &self,
            account: &AccountId,
        ) -> pawork_control_plane::credential::AccountHealth {
            self.inner.account_health(account)
        }
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
    async fn concurrent_spawn_never_exceeds_max_concurrent_agents() {
        const CAP: u64 = 2;
        const PRESSURE: usize = 16;
        let engine = engine_for(
            "tenant-a",
            TenantPolicy {
                max_concurrent_agents: Some(CAP),
                ..TenantPolicy::default()
            },
        );
        let supervisor = Arc::new(harness(Arc::new(InMemoryCredentialPool::new(8)), engine));

        let (admitted, errors) = join_spawns(&supervisor, PRESSURE, false).await;
        assert_eq!(
            admitted.len() as u64,
            CAP,
            "admitted {} workers under cap {CAP}: {admitted:?}",
            admitted.len()
        );
        assert_eq!(errors.len(), PRESSURE - CAP as usize);
        assert!(
            errors
                .iter()
                .all(|err| matches!(err, SupervisorError::PolicyDenied(_))),
            "extras must be PolicyDenied, got {errors:?}"
        );
        assert_eq!(
            supervisor.active_worker_count(Some(&TenantId::new("tenant-a"))),
            CAP
        );

        // 预约必须归还：完成一名 agent 后应能再准入一名。
        let first = admitted[0].clone();
        supervisor.start_worker(&first).await.unwrap();
        supervisor.complete(&first).await.unwrap();
        let replacement = supervisor.spawn(spawn_request(None)).await.unwrap();
        assert_eq!(
            supervisor.active_worker_count(Some(&TenantId::new("tenant-a"))),
            CAP
        );
        assert!(supervisor.state(&replacement).is_some());
    }

    #[tokio::test]
    async fn concurrent_spawn_never_exceeds_pool_concurrency() {
        const CAP: u64 = 2;
        const PRESSURE: usize = 12;
        let pool = Arc::new(InMemoryCredentialPool::new(CAP));
        let supervisor = Arc::new(harness(
            pool.clone(),
            engine_for("tenant-a", TenantPolicy::default()),
        ));

        let (admitted, errors) = join_spawns(&supervisor, PRESSURE, true).await;
        assert_eq!(
            admitted.len() as u64,
            CAP,
            "pool over-admitted {} leases under cap {CAP}",
            admitted.len()
        );
        assert_eq!(errors.len(), PRESSURE - CAP as usize);
        assert!(
            errors
                .iter()
                .all(|err| matches!(err, SupervisorError::PoolAcquire(_))),
            "pool extras must be PoolAcquire, got {errors:?}"
        );
        let account = AccountId::new("local/default");
        assert_eq!(pool.active_count(&account), CAP);

        let first = admitted[0].clone();
        supervisor.start_worker(&first).await.unwrap();
        supervisor.complete(&first).await.unwrap();
        assert_eq!(pool.active_count(&account), CAP - 1);

        let mut req = spawn_request(None);
        req.acquire = Some(acquire_request(&AgentId::new("placeholder")));
        let replacement = supervisor.spawn(req).await.unwrap();
        assert_eq!(pool.active_count(&account), CAP);
        assert!(supervisor.state(&replacement).is_some());
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
    async fn malicious_pool_lease_scope_mismatch_is_rejected_and_released() {
        let kinds = [
            LeaseScopeMismatch::Tenant,
            LeaseScopeMismatch::Principal,
            LeaseScopeMismatch::Session,
            LeaseScopeMismatch::Agent,
            LeaseScopeMismatch::Provider,
            LeaseScopeMismatch::Account,
        ];
        for kind in kinds {
            let pool = Arc::new(MismatchingPool::new(kind));
            let engine = engine_for(
                "tenant-a",
                TenantPolicy {
                    max_concurrent_agents: Some(1),
                    ..TenantPolicy::default()
                },
            );
            let supervisor = harness(pool.clone(), engine);

            let mut acquire = acquire_request(&AgentId::new("placeholder"));
            match kind {
                LeaseScopeMismatch::Provider => {
                    acquire.provider_id = Some(ProviderId::new("requested-provider"));
                }
                LeaseScopeMismatch::Account => {
                    acquire.account_id = Some(AccountId::new("requested-account"));
                }
                _ => {}
            }
            let mut req = spawn_request(None);
            req.acquire = Some(acquire);
            let err = supervisor.spawn(req).await.unwrap_err();
            assert!(
                matches!(
                    err,
                    SupervisorError::PolicyDenied(ref reason)
                        if reason.contains("lease scope validation")
                            && reason.contains(kind.reason())
                ),
                "{kind:?}: expected PolicyDenied with lease scope validation, got {err:?}"
            );

            let released = pool.released();
            assert_eq!(released.len(), 1, "{kind:?}: {released:?}");
            assert_eq!(
                released[0].1,
                LeaseOutcome::Released,
                "{kind:?}: must release with Released, not Failed"
            );

            let health_accounts = [
                AccountId::new("local/default"),
                AccountId::new("requested-account"),
                AccountId::new("evil-account"),
            ];
            for account in health_accounts {
                assert_eq!(
                    pool.active_count(&account),
                    0,
                    "{kind:?}: dangling lease on {account}"
                );
                assert_eq!(
                    pool.account_health(&account).consecutive_failures,
                    0,
                    "{kind:?}: Released must not punish account health"
                );
            }
            assert_eq!(
                supervisor.active_worker_count(None),
                0,
                "{kind:?}: must not leave an active worker or reservation"
            );

            // cap=1：若预约泄漏，下一次 spawn 会被并发闸门拒绝。
            let recovered = supervisor.spawn(spawn_request(None)).await.unwrap();
            assert!(supervisor.state(&recovered).is_some(), "{kind:?}");
            supervisor.start_worker(&recovered).await.unwrap();
            supervisor.complete(&recovered).await.unwrap();
        }
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

    #[tokio::test]
    async fn max_worker_depth_allows_siblings_and_denies_grandchild() {
        let mut config = SupervisorConfig::default();
        config.max_worker_depth = Some(1);
        let supervisor = harness_with_config(
            Arc::new(InMemoryCredentialPool::new(8)),
            Arc::new(InMemoryTenantPolicyEngine::default()),
            config,
        );
        let parent = supervisor.spawn(spawn_request(None)).await.unwrap();
        let mut child_req = SpawnRequest {
            parent_id: Some(parent.clone()),
            ..spawn_request(None)
        };
        let sibling_a = supervisor.spawn(child_req.clone()).await.unwrap();
        let sibling_b = supervisor.spawn(child_req.clone()).await.unwrap();
        assert_eq!(supervisor.state(&sibling_a), Some(WorkerState::Starting));
        assert_eq!(supervisor.state(&sibling_b), Some(WorkerState::Starting));

        child_req.parent_id = Some(sibling_a.clone());
        let err = supervisor.spawn(child_req).await.unwrap_err();
        assert!(
            matches!(err, SupervisorError::PolicyDenied(ref reason) if reason.contains("depth")),
            "grandchild must be PolicyDenied, got {err:?}"
        );
        assert!(events_contain(&supervisor.events(), |event| {
            matches!(
                event,
                OrchestrationEvent::ConcurrencyDenied { kind, limit, .. }
                    if kind == "depth" && *limit == 1
            )
        }));
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
impl pawork_control_plane::UsageLedger for FailingLedger {
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
impl pawork_control_plane::UsageLedger for BlockingLedger {
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
    use pawork_control_plane::credential::{AcquireRequest, InMemoryCredentialPool};
    use pawork_control_plane::{
        InMemoryTenantPolicyEngine, PermissionProfile, PrincipalRole, TenantPolicy,
    };
    use pawork_domain::{PrincipalId, SessionId, TenantId};
    use std::collections::BTreeSet;
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
