//! Run 监督器（P13-1）。
//!
//! 登记 run 与 [`CancelHandle`]，提供幂等的 `cancel` / `retry`；`RunStart` 经
//! [`agent_engine::ProviderLoop`] 执行真实 Agent 循环（测试注入
//! `test-support::MockProvider`）。GUI 断线只更新连接记录，绝不取消 Run——
//! 取消唯一入口是 `RunCancel` 命令。
//!
//! 每个 run 一个后台任务：订阅 [`EventBroadcaster`]，把
//! [`AgentEventEnvelope`](agent_events::AgentEventEnvelope) 翻译为
//! [`AppEventEnvelope`](core_api::AppEventEnvelope)（含 stream/global 序号），
//! 同步聚合状态并推入 [`RateLimiter`] 按 stream 合并增量。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_domain::{
    AgentId, BackgroundTaskId, CancellationToken, CommandId, ContentPart, CoreInstanceId, EventId,
    Message, MessageId, MessageMetadata, MessageRole, ModelId, ProfileIsolation, ProfileToolRules,
    ProviderId, RequestId, RunId, SessionId, TaskKind, TaskStatus, TextContent, Timestamp,
    WorkspaceId,
};
use agent_engine::{
    ApprovalOutcome, CancelHandle, CancelReason, EventBroadcaster, LoopContext, LoopError,
    PendingToolInvocation, ProviderLoop, ProviderLoopConfig, ToolCallResult,
};
use agent_events::{AgentEvent, AgentEventEnvelope};
use async_trait::async_trait;
use audit_log::{AuditAction, AuditDecision, AuditDimensions, AuditTargetKind};
use core_api::{
    AppEvent, AppEventEnvelope, ClientContextSnapshot, ClientDiagnosticSeverity, CommandSource,
    EventSource, EventStream, GlobalSequence, QuotaAlertKind, QuotaOverviewQuery, QuotaUnit,
    RunState, API_VERSION,
};
use model_registry::ModelRegistry;
use provider_api::ModelProvider;
use provider_control::{AcquireRequest, CredentialLease, CredentialPool, LeaseGuard, LeaseOutcome};
use tenant_service::{
    decide_budget, decide_request_concurrency, IdentityContext, Permission, PolicyDecision,
    PolicyDecisionKind, PolicyGate,
};
use thiserror::Error;
use tool_api::ToolResult;
use usage_ledger::UsageQuery;

use crate::aggregate::AggregateState;
use crate::approval::{ApprovalRegistry, Registration};
use crate::error::now_timestamp;
use crate::policy::TenantPolicyGate;
use crate::rate_limit::RateLimiter;
use crate::user_hook::UserHookHost;

/// 默认最大并发 run 数（有界性：超限的 RunStart 返回结构化错误）。
pub const DEFAULT_MAX_CONCURRENT_RUNS: usize = 8;

/// 会话作用域的 canonical root AgentId（P18-4 审查补救）。
///
/// 由 `session_id` 确定性派生：同一 session 的所有 run 与每次 retry attempt
/// 共享同一 root agent 身份，跨进程重放 / 重启稳定；任何 session（含默认值）
/// 都产生非空 id。**客户端不可选择 agent 或 credential**：命令入口不暴露
/// agent/credential 输入，credential 只能来自 acquire 得到的真实
/// [`CredentialLease`]。派生格式 `root-<session_id>` 是契约
/// （`app-service/tests/credential_lease.rs` 断言）。
pub fn canonical_root_agent_id(session_id: &SessionId) -> AgentId {
    AgentId::new(format!("root-{}", session_id.as_str()))
}

#[derive(Debug, Error)]
pub enum SuperviseError {
    #[error("run not found: {0}")]
    NotFound(String),
    #[error("run {0} is already registered")]
    AlreadyExists(String),
    #[error("run {0} is still active")]
    StillActive(String),
    #[error("run {0} already completed; retry only applies to failed or cancelled runs")]
    Completed(String),
    #[error("max concurrent runs reached ({0})")]
    Capacity(usize),
    #[error("tenant policy denied run admission: {0}")]
    PolicyDenied(String),
    #[error("background run requires a TaskManager: {0}")]
    BackgroundUnavailable(String),
}

/// 取消结果回执（幂等）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelOutcome {
    /// 调用前已处于取消态或终态（本次调用未触发新取消）。
    pub already_cancelled: bool,
}

/// 监督器统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunSupervisorStats {
    pub started: u64,
    pub retried: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub active: usize,
    pub total: usize,
}

/// RunStart 的监督请求。
#[derive(Clone, Debug)]
pub struct RunRequest {
    pub run_id: RunId,
    pub session_id: SessionId,
    /// run 所属 workspace（session 记录聚合而来）；user hooks 按 workspace
    /// 作用域匹配触发。旧构造点未提供时为 `None`（仅 global hooks 生效）。
    pub workspace_id: Option<WorkspaceId>,
    /// P18-2 身份上下文：usage 记账与 run 归属的真实 tenant/principal。
    /// 由 router 在命令入口 fail-closed 解析，缺失身份的 run 不会到达这里。
    pub identity: tenant_service::IdentityContext,
    pub provider_id: ProviderId,
    pub model: ModelId,
    pub source: CommandSource,
    pub command_id: CommandId,
    pub user_message: String,
    /// Run 前注入的供应商中立额度信号（P14-8）；None = 不注入。
    pub external_quota: Option<agent_engine::ExternalQuotaSignal>,
    /// P17-5：可选 Agent Profile v2（loader 已校验的不可变配置）。命中时其
    /// prompt / canonical effort / tools / max_turns / background / isolation
    /// 成为该 run 的权威配置；retry 沿用同一不可变实例。
    pub profile: Option<crate::profile_resolver::ResolvedRunProfile>,
}

impl RunRequest {
    /// 本 run 的 canonical root AgentId（session 作用域，P18-4 审查补救）。
    ///
    /// 由 [`canonical_root_agent_id`] 从 `session_id` 确定性派生：同一 session
    /// 的首跑与每次 retry attempt 身份一致；客户端不可选择（命令入口不暴露
    /// agent/credential 输入，credential 只能来自 acquire 的真实 lease）。
    pub fn agent_id(&self) -> AgentId {
        canonical_root_agent_id(&self.session_id)
    }
}

struct RunTask {
    run_id: RunId,
    session_id: SessionId,
    /// session 作用域的 canonical root AgentId（P18-4 审查补救）：首跑与 retry
    /// 各 attempt 共享同一身份，经 [`RunRequest::agent_id`] 派生，客户端不可选。
    agent_id: AgentId,
    identity: tenant_service::IdentityContext,
    provider_id: ProviderId,
    model: ModelId,
    source: CommandSource,
    user_message: String,
    cancel: CancelHandle,
    state: Arc<Mutex<RunState>>,
    join: tokio::task::JoinHandle<()>,
    provider: Arc<dyn ModelProvider>,
    config: ProviderLoopConfig,
    /// 同一 run 的消费序号：首次执行为 0，每次成功 retry 前递增。
    attempt: u64,
    /// P17-5：run 绑定的不可变 profile（retry 沿用）。
    profile: Option<crate::profile_resolver::ResolvedRunProfile>,
    /// P17-5：background=true 时注册的 TaskKind::Agent id；终态时 finish/cancel。
    background_task_id: Option<BackgroundTaskId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TerminalCounters {
    completed: u64,
    cancelled: u64,
    failed: u64,
}

struct Inner {
    tasks: BTreeMap<RunId, RunTask>,
    started: u64,
    retried: u64,
}

/// Run 监督器：登记、取消、重试与终态收尾。
pub struct RunSupervisor {
    max_concurrent: usize,
    inner: Mutex<Inner>,
    aggregate: Arc<AggregateState>,
    approvals: Arc<ApprovalRegistry>,
    limiter: Arc<RateLimiter>,
    broadcaster: EventBroadcaster,
    instance_id: CoreInstanceId,
    global_sequence: Arc<AtomicU64>,
    terminal_counters: Arc<Mutex<TerminalCounters>>,
    /// P14-8 共享 Quota 运行时（注入后用于成功 run 幂等记账 + run 前信号查询）。
    quota_runtime: Mutex<Option<Arc<crate::QuotaRuntime>>>,
    /// P17-5 后台任务管理器：background=true 的 run 经它注册 / 启动 / 完成 /
    /// 取消一个 TaskKind::Agent，复用既有状态机。未注入时 background run fail-closed。
    task_manager: Mutex<Option<Arc<task_manager::TaskManager>>>,
    /// P14 告警桥（长期持有）：把 quota-service 的脱敏 Alert 映射为 Global
    /// stream 的 `QuotaAlert` 事件；与 run 事件共享 limiter/global_sequence，
    /// 独立维护 Global 流序号。供 RefreshScheduler 经 [`Self::alert_sink`] 注入。
    alert_sink: Arc<dyn quota_service::refresh::AlertSink>,
    /// Canonical quota audit bridge. It resolves the current shared TenantPolicyGate at emit
    /// time, so composition can wire policy after constructing the supervisor.
    quota_audit_sink: Arc<dyn quota_service::refresh::AuditSink>,
    /// P17-6 Team 桥（长期持有）：把 canonical `teams::TeamEvent` 映射为
    /// `AppEvent::TeamEvent` typed 镜像，与 quota 告警共享 limiter /
    /// global_sequence / Global 流序号（跨流连续由 EventHub 收口），推入共享
    /// limiter 后经 EventPump 发布到唯一 EventHub。供 TeamService 经
    /// [`Self::team_sink`] 注入。
    team_sink: Arc<dyn teams::TeamEventSink>,
    /// P17-1 User Hooks 宿主（注入后 run 的 pre-prompt / pre-tool 权威位点
    /// 回灌 hooks 结果；未注入时行为与既往完全一致）。
    user_hooks: Mutex<Option<Arc<UserHookHost>>>,
    /// run 的 workspace roots（P17-1）：传给 UserHookHost 的 pre-prompt /
    /// pre-tool 位点；宿主装配时注入，未注入为空（仅 global hooks 生效）。
    workspace_roots: Mutex<Vec<PathBuf>>,
    /// P18-4 共享 CredentialPool（注入后每个 run attempt 在 provider 调用前
    /// 异步 acquire 并持有 LeaseGuard 至终态；未注入时为 `None`，走 legacy
    /// 过渡路径——不 acquire、attribution 无 credential）。
    credential_pool: Mutex<Option<Arc<dyn CredentialPool>>>,
    /// P18-9 租户策略闸口（与 router / AppService 共享同一实例）：lease
    /// 取得后强制 LeaseAcquire + account 白名单，并用唯一 UsageLedger 做
    /// 预算 admission；拒绝时释放 lease、run fail-closed。由
    /// [`crate::router::CommandRouter`] 构造时注入。
    tenant_policy: Arc<Mutex<Option<Arc<TenantPolicyGate>>>>,
}

impl RunSupervisor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_concurrent: usize,
        aggregate: Arc<AggregateState>,
        approvals: Arc<ApprovalRegistry>,
        limiter: Arc<RateLimiter>,
        broadcaster: EventBroadcaster,
        instance_id: CoreInstanceId,
    ) -> Self {
        let global_sequence = Arc::new(AtomicU64::new(0));
        // Global stream 是 quota 告警与 team 事件的唯一共享流：两者必须共享
        // 同一 stream_sequence 计数器，保证交错发布时流内序号连续。
        let stream_sequence = Arc::new(AtomicU64::new(0));
        let alert_sink: Arc<dyn quota_service::refresh::AlertSink> = Arc::new(AppQuotaAlertSink {
            limiter: Arc::clone(&limiter),
            global_sequence: Arc::clone(&global_sequence),
            stream_sequence: Arc::clone(&stream_sequence),
            instance_id: instance_id.clone(),
        });
        let policy_slot = Arc::new(Mutex::new(None));
        let quota_audit_sink: Arc<dyn quota_service::refresh::AuditSink> =
            Arc::new(AppQuotaAuditSink {
                tenant_policy: Arc::clone(&policy_slot),
            });
        let team_sink: Arc<dyn teams::TeamEventSink> = Arc::new(AppTeamEventSink {
            limiter: Arc::clone(&limiter),
            global_sequence: Arc::clone(&global_sequence),
            stream_sequence,
            instance_id: instance_id.clone(),
        });
        Self {
            max_concurrent: max_concurrent.max(1),
            inner: Mutex::new(Inner {
                tasks: BTreeMap::new(),
                started: 0,
                retried: 0,
            }),
            aggregate,
            approvals,
            limiter,
            broadcaster,
            instance_id,
            global_sequence,
            terminal_counters: Arc::new(Mutex::new(TerminalCounters::default())),
            quota_runtime: Mutex::new(None),
            task_manager: Mutex::new(None),
            alert_sink,
            quota_audit_sink,
            team_sink,
            user_hooks: Mutex::new(None),
            workspace_roots: Mutex::new(Vec::new()),
            credential_pool: Mutex::new(None),
            tenant_policy: policy_slot,
        }
    }

    /// 注入共享 User Hooks 宿主（P17-1）。同一实例重复注入为 no-op；
    /// [`Self::new`] 签名保持不变，未注入时 run loop 不回调 hooks。
    pub fn set_user_hooks(&self, host: Arc<UserHookHost>) {
        let mut guard = self.user_hooks.lock().expect("user_hooks mutex");
        let already = guard
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &host));
        if !already {
            *guard = Some(host);
        }
    }

    fn user_hooks(&self) -> Option<Arc<UserHookHost>> {
        self.user_hooks.lock().expect("user_hooks mutex").clone()
    }

    /// 注入 run 的 workspace roots（P17-1）。同一实例重复注入为 no-op。
    pub fn set_workspace_roots(&self, roots: Vec<PathBuf>) {
        let mut guard = self.workspace_roots.lock().expect("workspace_roots mutex");
        if *guard != roots {
            *guard = roots;
        }
    }

    /// 当前注入的 run workspace roots（P17-1；宿主装配 / 诊断用）。
    pub fn workspace_roots(&self) -> Vec<PathBuf> {
        self.workspace_roots
            .lock()
            .expect("workspace_roots mutex")
            .clone()
    }

    /// 是否已注入共享 User Hooks 宿主（宿主装配 / 诊断用）。
    pub fn user_hooks_active(&self) -> bool {
        self.user_hooks.lock().expect("user_hooks mutex").is_some()
    }

    /// 注入共享租户策略闸口（P18-9）：与 router / AppService 同一实例，
    /// 裁决只记录一次（不可双记）。幂等：同一实例重复注入为 no-op。
    pub fn set_tenant_policy(&self, gate: Arc<TenantPolicyGate>) {
        let mut guard = self.tenant_policy.lock().expect("tenant_policy mutex");
        let already = guard
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &gate));
        if !already {
            *guard = Some(gate);
        }
    }

    fn tenant_policy(&self) -> Option<Arc<TenantPolicyGate>> {
        self.tenant_policy
            .lock()
            .expect("tenant_policy mutex")
            .clone()
    }

    /// 指定租户当前活跃（非终态）run 数（P18-9 请求并发准入）。
    pub fn active_for_tenant(&self, tenant: &agent_domain::TenantId) -> u64 {
        let inner = lock(&self.inner);
        active_for_tenant(&inner, tenant)
    }

    /// 注入共享 CredentialPool（P18-4）：每个 run attempt 在 provider 调用前
    /// 异步 acquire，持有 LeaseGuard 至终态；acquire 失败 fail-closed（run
    /// 进入 Failed，不调用 provider）。幂等：同一实例重复注入为 no-op。
    pub fn set_credential_pool(&self, pool: Arc<dyn CredentialPool>) {
        let mut guard = self.credential_pool.lock().expect("credential_pool mutex");
        let already = guard
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &pool));
        if !already {
            *guard = Some(pool);
        }
    }

    fn credential_pool(&self) -> Option<Arc<dyn CredentialPool>> {
        self.credential_pool
            .lock()
            .expect("credential_pool mutex")
            .clone()
    }

    /// 注入共享 Quota 运行时（P14-8）：成功 run 完成后向同一 ledger 幂等记账。
    /// 幂等：同一实例重复注入为 no-op。既有 [`Self::new`] 签名保持不变。
    pub fn set_quota_runtime(&self, runtime: Arc<crate::QuotaRuntime>) {
        let mut guard = self.quota_runtime.lock().expect("quota_runtime mutex");
        let already = guard
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &runtime));
        if !already {
            *guard = Some(runtime);
        }
    }

    fn quota_runtime(&self) -> Option<Arc<crate::QuotaRuntime>> {
        self.quota_runtime
            .lock()
            .expect("quota_runtime mutex")
            .clone()
    }

    /// 注入 P17-5 后台任务管理器：background=true 的 run 经它注册 / 启动 /
    /// 完成 / 取消一个 TaskKind::Agent。幂等：同一实例重复注入为 no-op。
    pub fn set_task_manager(&self, manager: Arc<task_manager::TaskManager>) {
        let mut guard = self.task_manager.lock().expect("task_manager mutex");
        let already = guard
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &manager));
        if !already {
            *guard = Some(manager);
        }
    }

    fn task_manager(&self) -> Option<Arc<task_manager::TaskManager>> {
        self.task_manager
            .lock()
            .expect("task_manager mutex")
            .clone()
    }

    /// 登记并启动一个 run。需要 tokio 运行时；无运行时返回错误（结构化，不 panic）。
    pub fn start(
        &self,
        request: RunRequest,
        provider: Arc<dyn ModelProvider>,
    ) -> Result<(), SuperviseError> {
        self.start_inner(request, provider, false)
    }

    /// 经 P18-9 tenant policy 原子准入后登记并启动 run。策略检查与任务插入
    /// 共用 `inner` 临界区，避免并发 RunStart 在 check-then-act 窗口同时越过
    /// `max_concurrent_requests`。
    pub fn start_with_policy(
        &self,
        request: RunRequest,
        provider: Arc<dyn ModelProvider>,
    ) -> Result<(), SuperviseError> {
        self.start_inner(request, provider, true)
    }

    fn start_inner(
        &self,
        request: RunRequest,
        provider: Arc<dyn ModelProvider>,
        enforce_policy: bool,
    ) -> Result<(), SuperviseError> {
        let mut inner = lock(&self.inner);
        if inner.tasks.contains_key(&request.run_id) {
            return Err(SuperviseError::AlreadyExists(request.run_id.to_string()));
        }
        if enforce_policy {
            self.enforce_run_admission(
                &inner,
                &request.identity,
                &request.provider_id,
                &request.model,
            )?;
        }
        let active = inner
            .tasks
            .values()
            .filter(|task| {
                let state = task.state.lock().expect("run task state");
                !terminal(&state)
            })
            .count();
        if active >= self.max_concurrent {
            return Err(SuperviseError::Capacity(self.max_concurrent));
        }

        let cancel = CancelHandle::new(
            request.run_id.clone(),
            Arc::new(agent_engine::NoopProcessTreeCleaner),
        );
        let state = Arc::new(Mutex::new(RunState::Created));
        let (config, queue) = self.build_config(&request);
        // P17-5：profile 的 tool_rules（deny-first）与 isolation（不可变要求）随
        // run 携带到 AppLoopContext（pre_tool 权威过滤 + 策略上下文）。
        let (tool_rules, isolation) = request
            .profile
            .as_ref()
            .map(|resolved| (resolved.profile.tools.clone(), resolved.profile.isolation))
            .unwrap_or_default();
        // P17-5：background=true 经 TaskManager 注册 / 启动一个 TaskKind::Agent，
        // 复用既有状态机；未注入 TaskManager 时 fail-closed。终态 finish/cancel
        // 在 run 任务内完成（见 spawn_run_task）。
        let task_manager = self.task_manager();
        let background_task_id = if request
            .profile
            .as_ref()
            .is_some_and(|resolved| resolved.profile.background)
        {
            let manager = task_manager.as_ref().ok_or_else(|| {
                SuperviseError::BackgroundUnavailable(format!(
                    "background run `{}` requires a TaskManager",
                    request.run_id
                ))
            })?;
            let task_id = manager
                .register(TaskKind::Agent, None)
                .map_err(|error| SuperviseError::BackgroundUnavailable(error.to_string()))?;
            manager
                .start(&task_id)
                .map_err(|error| SuperviseError::BackgroundUnavailable(error.to_string()))?;
            Some(task_id)
        } else {
            None
        };
        let created_at = match self
            .aggregate
            .get_run(&request.run_id, &request.identity.tenant_id)
        {
            Some(record) => record.created_at,
            None => now_timestamp(),
        };
        let quota_runtime = self.quota_runtime();
        let credential_pool = self.credential_pool();
        let tenant_policy = self.tenant_policy();
        let user_hooks = self.user_hooks();
        let workspace_roots = self.workspace_roots();
        let agent_id = request.agent_id();
        let task = spawn_run_task(
            request.run_id.clone(),
            request.session_id.clone(),
            request.workspace_id.clone(),
            request.identity.clone(),
            agent_id.clone(),
            request.source.clone(),
            request.command_id.clone(),
            Arc::clone(&self.aggregate),
            Arc::clone(&self.approvals),
            Arc::clone(&self.limiter),
            self.broadcaster.clone(),
            self.instance_id.clone(),
            Arc::clone(&self.global_sequence),
            Arc::clone(&self.terminal_counters),
            provider.clone(),
            config.clone(),
            queue,
            cancel.clone(),
            Arc::clone(&state),
            request.provider_id.clone(),
            request.model.clone(),
            0,
            created_at,
            request.external_quota,
            quota_runtime,
            user_hooks,
            workspace_roots,
            tool_rules,
            isolation,
            background_task_id.clone(),
            task_manager,
            credential_pool,
            tenant_policy,
        );
        inner.started += 1;
        inner.tasks.insert(
            request.run_id.clone(),
            RunTask {
                run_id: request.run_id,
                session_id: request.session_id,
                agent_id,
                identity: request.identity,
                provider_id: request.provider_id,
                model: request.model,
                source: request.source,
                user_message: request.user_message,
                cancel,
                state,
                join: task,
                provider,
                config,
                attempt: 0,
                profile: request.profile,
                background_task_id,
            },
        );
        Ok(())
    }

    fn enforce_run_admission(
        &self,
        inner: &Inner,
        identity: &IdentityContext,
        provider_id: &ProviderId,
        model: &ModelId,
    ) -> Result<(), SuperviseError> {
        let gate = self.tenant_policy().ok_or_else(|| {
            SuperviseError::PolicyDenied("tenant policy gate is not configured".into())
        })?;
        gate.check_permission(identity, Permission::AgentSpawn)
            .map_err(|error| SuperviseError::PolicyDenied(error.to_string()))?;
        gate.check_model(&identity.tenant_id, model)
            .map_err(|error| SuperviseError::PolicyDenied(error.to_string()))?;
        gate.check_provider(&identity.tenant_id, provider_id)
            .map_err(|error| SuperviseError::PolicyDenied(error.to_string()))?;

        let current = active_for_tenant(inner, &identity.tenant_id);
        let max = gate
            .engine()
            .policy(&identity.tenant_id)
            .max_concurrent_requests;
        match decide_request_concurrency(current, max) {
            PolicyDecision::Allow => Ok(()),
            PolicyDecision::Deny { reason } => Err(SuperviseError::PolicyDenied(reason)),
            other => Err(SuperviseError::PolicyDenied(format!(
                "request admission returned {other:?}; denied by default"
            ))),
        }
    }

    /// 取消 run（幂等）：未登记返回 NotFound；已取消或已终态为 no-op。
    pub fn cancel(&self, run_id: &RunId) -> Result<CancelOutcome, SuperviseError> {
        self.cancel_for_tenant(
            run_id,
            &agent_domain::TenantId::new(core_api::DEFAULT_CONTROL_PLANE_TENANT),
        )
    }

    /// tenant-scoped 取消：跨租户 run 视同不存在，不泄漏状态。
    pub fn cancel_for_tenant(
        &self,
        run_id: &RunId,
        tenant_id: &agent_domain::TenantId,
    ) -> Result<CancelOutcome, SuperviseError> {
        let inner = lock(&self.inner);
        let task = inner
            .tasks
            .get(run_id)
            .ok_or_else(|| SuperviseError::NotFound(run_id.to_string()))?;
        if task.identity.tenant_id != *tenant_id {
            return Err(SuperviseError::NotFound(run_id.to_string()));
        }
        let state = task.state.lock().expect("run task state").clone();
        if task.cancel.is_cancelled() || terminal(&state) {
            return Ok(CancelOutcome {
                already_cancelled: true,
            });
        }
        task.cancel.cancel(CancelReason::User);
        // 立即反映到聚合，避免查询滞后；后台任务随后广播 RunCancelled。
        let _ = self.aggregate.set_run_state(run_id, RunState::Cancelled);
        Ok(CancelOutcome {
            already_cancelled: false,
        })
    }

    /// 重试 run（幂等）：仅 Failed / Cancelled / Interrupted 可重开；Completed 与
    /// 活跃 run 返回结构化错误。
    pub fn retry(&self, run_id: &RunId) -> Result<(), SuperviseError> {
        self.retry_for_tenant(
            run_id,
            &agent_domain::TenantId::new(core_api::DEFAULT_CONTROL_PLANE_TENANT),
        )
    }

    /// tenant-scoped 重试：跨租户 run 视同不存在，不泄漏状态。
    pub fn retry_for_tenant(
        &self,
        run_id: &RunId,
        tenant_id: &agent_domain::TenantId,
    ) -> Result<(), SuperviseError> {
        self.retry_inner(run_id, tenant_id, None)
    }

    /// tenant-scoped policy-aware retry。每次 attempt 都重新执行 AgentSpawn、
    /// model/provider 白名单与请求并发准入，避免策略更新后以 Retry 绕过。
    pub fn retry_for_identity(
        &self,
        run_id: &RunId,
        identity: &IdentityContext,
    ) -> Result<(), SuperviseError> {
        self.retry_inner(run_id, &identity.tenant_id, Some(identity))
    }

    fn retry_inner(
        &self,
        run_id: &RunId,
        tenant_id: &agent_domain::TenantId,
        policy_identity: Option<&IdentityContext>,
    ) -> Result<(), SuperviseError> {
        let mut inner = lock(&self.inner);
        let (current, provider_id, model) = {
            let task = inner
                .tasks
                .get(run_id)
                .ok_or_else(|| SuperviseError::NotFound(run_id.to_string()))?;
            if task.identity.tenant_id != *tenant_id {
                return Err(SuperviseError::NotFound(run_id.to_string()));
            }
            (
                task.state.lock().expect("run task state").clone(),
                task.provider_id.clone(),
                task.model.clone(),
            )
        };
        if current == RunState::Completed {
            return Err(SuperviseError::Completed(run_id.to_string()));
        }
        if !terminal(&current) {
            return Err(SuperviseError::StillActive(run_id.to_string()));
        }
        if let Some(identity) = policy_identity {
            self.enforce_run_admission(&inner, identity, &provider_id, &model)?;
        }
        let active = inner
            .tasks
            .values()
            .filter(|task| {
                let state = task.state.lock().expect("run task state");
                !terminal(&state)
            })
            .count();
        if active >= self.max_concurrent {
            return Err(SuperviseError::Capacity(self.max_concurrent));
        }
        let task = inner
            .tasks
            .get(run_id)
            .expect("run task remains registered while inner is locked");
        let attempt = task
            .attempt
            .checked_add(1)
            .expect("run retry attempt counter overflow");

        let request = RunRequest {
            run_id: task.run_id.clone(),
            session_id: task.session_id.clone(),
            // retry 复用 session 聚合的 workspace 归属，保证 hooks 的
            // workspace 作用域在重试后依然正确。
            workspace_id: self
                .aggregate
                .get_session(&task.session_id, &task.identity.tenant_id)
                .map(|session| session.workspace_id),
            identity: task.identity.clone(),
            provider_id: task.provider_id.clone(),
            model: task.model.clone(),
            source: task.source.clone(),
            command_id: CommandId::from(format!("retry-{}", task.run_id)),
            user_message: task.user_message.clone(),
            external_quota: None,
            // P17-5：retry 沿用同一不可变 profile（retry 保持 profile）。
            profile: task.profile.clone(),
        };
        let cancel = CancelHandle::new(
            task.run_id.clone(),
            Arc::new(agent_engine::NoopProcessTreeCleaner),
        );
        let new_state = Arc::new(Mutex::new(RunState::Created));
        let (config, queue) = self.build_config(&request);
        // P17-5：retry 沿用同一不可变 profile；tool_rules / isolation 继续随 run
        // 携带，background 经 TaskManager 重新注册 / 启动一个 TaskKind::Agent
        // （上一 attempt 已终态，复用既有生命周期而非新建状态机）。
        let (tool_rules, isolation) = request
            .profile
            .as_ref()
            .map(|resolved| (resolved.profile.tools.clone(), resolved.profile.isolation))
            .unwrap_or_default();
        let task_manager = self.task_manager();
        let background_task_id = if request
            .profile
            .as_ref()
            .is_some_and(|resolved| resolved.profile.background)
        {
            let manager = task_manager.as_ref().ok_or_else(|| {
                SuperviseError::BackgroundUnavailable(format!(
                    "background retry of `{}` requires a TaskManager",
                    request.run_id
                ))
            })?;
            let task_id = manager
                .register(TaskKind::Agent, None)
                .map_err(|error| SuperviseError::BackgroundUnavailable(error.to_string()))?;
            manager
                .start(&task_id)
                .map_err(|error| SuperviseError::BackgroundUnavailable(error.to_string()))?;
            Some(task_id)
        } else {
            None
        };
        let created_at = match self.aggregate.get_run(run_id, &task.identity.tenant_id) {
            Some(record) => record.created_at,
            None => now_timestamp(),
        };
        let quota_runtime = self.quota_runtime();
        let credential_pool = self.credential_pool();
        let tenant_policy = self.tenant_policy();
        let user_hooks = self.user_hooks();
        let workspace_roots = self.workspace_roots();
        let join = spawn_run_task(
            task.run_id.clone(),
            task.session_id.clone(),
            request.workspace_id.clone(),
            task.identity.clone(),
            task.agent_id.clone(),
            task.source.clone(),
            request.command_id,
            Arc::clone(&self.aggregate),
            Arc::clone(&self.approvals),
            Arc::clone(&self.limiter),
            self.broadcaster.clone(),
            self.instance_id.clone(),
            Arc::clone(&self.global_sequence),
            Arc::clone(&self.terminal_counters),
            Arc::clone(&task.provider),
            config.clone(),
            queue,
            cancel.clone(),
            Arc::clone(&new_state),
            request.provider_id.clone(),
            request.model.clone(),
            attempt,
            created_at,
            request.external_quota,
            quota_runtime,
            user_hooks,
            workspace_roots,
            tool_rules,
            isolation,
            background_task_id.clone(),
            task_manager,
            credential_pool,
            tenant_policy,
        );
        let _ = self.aggregate.set_run_state(run_id, RunState::Created);
        if let Some(task) = inner.tasks.get_mut(run_id) {
            task.cancel = cancel;
            task.state = new_state;
            task.join = join;
            task.config = config;
            task.attempt = attempt;
            task.background_task_id = background_task_id;
        }
        inner.retried += 1;
        Ok(())
    }

    /// 当前 run 是否活跃（未取消、非终态）。
    pub fn is_active(&self, run_id: &RunId) -> bool {
        lock(&self.inner).tasks.get(run_id).is_some_and(|task| {
            let state = task.state.lock().expect("run task state").clone();
            !task.cancel.is_cancelled() && !terminal(&state)
        })
    }

    /// 当前登记的 run 数（含终态，供幂等测试验证不重复建 Run）。
    pub fn total(&self) -> usize {
        lock(&self.inner).tasks.len()
    }

    pub fn stats(&self) -> RunSupervisorStats {
        let inner = lock(&self.inner);
        let counters = lock(&self.terminal_counters);
        RunSupervisorStats {
            started: inner.started,
            retried: inner.retried,
            completed: counters.completed,
            cancelled: counters.cancelled,
            failed: counters.failed,
            active: inner
                .tasks
                .values()
                .filter(|task| {
                    let state = task.state.lock().expect("run task state");
                    !terminal(&state)
                })
                .count(),
            total: inner.tasks.len(),
        }
    }

    /// 冲刷并取回已限流合并的应用事件（供 GUI 协议 / 测试消费）。
    pub fn drain_events(&self) -> Vec<AppEventEnvelope> {
        self.limiter.flush()
    }

    /// 返回共享告警桥（`quota_service::refresh::AlertSink` trait 对象），供
    /// RefreshScheduler 注入。同一 supervisor 每次调用返回同一实例：桥长期持有
    /// 于 [`Self::new`]，Global 流序号持续递增，多次调用不会重置序列。
    pub fn alert_sink(&self) -> Arc<dyn quota_service::refresh::AlertSink> {
        Arc::clone(&self.alert_sink)
    }

    /// Returns the canonical quota refresh audit bridge for `RefreshScheduler`.
    pub fn quota_audit_sink(&self) -> Arc<dyn quota_service::refresh::AuditSink> {
        Arc::clone(&self.quota_audit_sink)
    }

    /// 返回共享 Team 事件桥（`teams::TeamEventSink` trait 对象），供
    /// app-service 装配 TeamService 时注入。同一 supervisor 每次调用返回
    /// 同一实例：Global 流序号持续递增，多次调用不会重置序列。
    pub fn team_sink(&self) -> Arc<dyn teams::TeamEventSink> {
        Arc::clone(&self.team_sink)
    }

    fn build_config(
        &self,
        request: &RunRequest,
    ) -> (ProviderLoopConfig, Arc<agent_engine::MessageQueue>) {
        let message = Message {
            id: MessageId::from(format!("user-{}", request.run_id)),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: request.user_message.clone(),
            })],
            metadata: MessageMetadata::default(),
        };
        // P17-5：profile 命中时其不可变配置成为权威来源——
        // - prompt.system + instructions 作为 canonical 初始 system message；
        // - canonical effort 经 ReasoningConfig 流入 CapabilityNegotiator；
        // - max_turns 成为 ProviderLoop 迭代硬上限。
        let (initial_messages, reasoning, max_iterations) = match request.profile.as_ref() {
            Some(resolved) => {
                let system_text = match resolved.profile.prompt.instructions.as_deref() {
                    Some(instructions) if !instructions.trim().is_empty() => {
                        format!("{}\n\n{instructions}", resolved.profile.prompt.system)
                    }
                    _ => resolved.profile.prompt.system.clone(),
                };
                let system = Message {
                    id: MessageId::from(format!("system-{}", request.run_id)),
                    role: MessageRole::System,
                    content: vec![ContentPart::Text(TextContent { text: system_text })],
                    metadata: MessageMetadata::default(),
                };
                let reasoning = provider_api::ReasoningConfig::new(resolved.profile.effort);
                let max_iterations = resolved.profile.max_turns.unwrap_or(16);
                (vec![system, message], Some(reasoning), max_iterations)
            }
            None => (vec![message], None, 16),
        };
        let config = ProviderLoopConfig {
            session_id: request.session_id.clone(),
            run_id: request.run_id.clone(),
            provider_id: request.provider_id.clone(),
            model: request.model.clone(),
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            initial_messages,
            max_iterations,
            budget: agent_engine::BudgetLimits::default(),
            retry: agent_engine::RetryPolicy::default(),
            thinking: None,
            reasoning,
        };
        (config, Arc::new(agent_engine::MessageQueue::new()))
    }
}

impl std::fmt::Debug for RunSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunSupervisor")
            .field("max_concurrent", &self.max_concurrent)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

/// P14 告警事件桥：把 quota-service 的脱敏 [`Alert`] 安全映射为
/// [`AppEvent::QuotaAlert`] 并推入共享限流器。
///
/// - 信封：`Core` source、`Global` stream；`global_sequence` 与 run 事件共享
///   （原子递增保持跨流连续），`stream_sequence` 独立维护（Global 流唯一持有者）。
/// - 负载：稳定 `kind`（[`core_api::QuotaAlertKind`]，与 quota-service
///   `AlertKind` 1:1 镜像）、稳定 severity（kind → severity 映射稳定，其中
///   Threshold 依据 `advisory` 区分 Critical/Warning）、window/unit 1:1 镜像、
///   model 原样透传、`source` 经
///   [`quota_service::adapters::http_util::redact_secrets`] 二次脱敏后透传、
///   `credential_hint` 经 [`core_api::mask_credential_hint`] 脱敏；`snapshot`
///   恒为 `None`（Alert 不携带快照）。消息只含 kind 与剩余百分比，永不
///   包含 source 或任何凭据。
struct AppQuotaAlertSink {
    limiter: Arc<RateLimiter>,
    global_sequence: Arc<AtomicU64>,
    /// Global stream 独立消费序号（桥是 Global 流唯一事件源）。
    stream_sequence: Arc<AtomicU64>,
    instance_id: CoreInstanceId,
}

struct AppQuotaAuditSink {
    tenant_policy: Arc<Mutex<Option<Arc<TenantPolicyGate>>>>,
}

#[async_trait]
impl quota_service::refresh::AuditSink for AppQuotaAuditSink {
    async fn record(&self, entry: quota_service::refresh::AuditEntry) {
        let gate = self
            .tenant_policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(gate) = gate else {
            tracing::warn!("quota audit dropped because tenant policy gate is not configured");
            return;
        };
        let identity = IdentityContext::new(
            entry.scope.tenant_id.clone(),
            agent_domain::PrincipalId::new("local/system"),
        );
        gate.record_control_event(
            &identity,
            AuditAction::QuotaRefreshed,
            AuditTargetKind::Quota,
            if entry.failures == 0 {
                AuditDecision::Observe
            } else {
                AuditDecision::Error
            },
            if entry.served_stale {
                "quota_refresh_stale"
            } else if entry.failures == 0 {
                "quota_refresh_succeeded"
            } else {
                "quota_refresh_partial_failure"
            },
            AuditDimensions {
                provider_id: Some(entry.scope.provider_id.clone()),
                account_id: Some(agent_domain::AccountId::from(
                    entry.scope.account_id.as_str(),
                )),
                ..AuditDimensions::default()
            },
            entry.at_ms,
        );
    }
}

#[async_trait]
impl quota_service::refresh::AlertSink for AppQuotaAlertSink {
    async fn emit(&self, alert: quota_service::refresh::Alert) {
        let sequence = self.stream_sequence.fetch_add(1, Ordering::SeqCst);
        let envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: self.instance_id.clone(),
            event_id: EventId::from(format!("app-evt-quota-alert-{sequence}")),
            global_sequence: GlobalSequence(self.global_sequence.fetch_add(1, Ordering::SeqCst)),
            stream: EventStream::Global,
            stream_sequence: sequence + 1,
            timestamp: Timestamp::from_unix_millis(alert.at_ms),
            source: EventSource::Core,
            payload: AppEvent::QuotaAlert {
                alert: Box::new(quota_alert_from(&alert)),
            },
        };
        self.limiter.push(envelope);
    }
}

/// P17-6 Team 事件桥：把 canonical [`teams::TeamEventEnvelope`] 映射为
/// [`AppEvent::TeamEvent`] typed 镜像并推入共享限流器（EventPump 轮询后
/// 发布到唯一 EventHub，ADR-024）。
///
/// - 信封：`Core` source、`Global` stream；`global_sequence` 与 run 事件 /
///   quota 告警共享（原子递增保持跨流连续），`stream_sequence` 与 quota
///   告警桥共享同一计数器（Global 流唯一持有者，交错发布仍连续）。
/// - 负载：`crate::team::to_app_event` 1:1 镜像 `teams::TeamEvent`，无 secret
///   字段（事件正文由团队语义决定，镜像不做额外脱敏）。
/// - 幂等可重入：镜像在 durable append 成功后才被调用（persist-first，
///   见 `teams::service`），hub 镜像失败不影响已持久化事实。
struct AppTeamEventSink {
    limiter: Arc<RateLimiter>,
    global_sequence: Arc<AtomicU64>,
    /// Global 流共享消费序号（与 AppQuotaAlertSink 同一计数器）。
    stream_sequence: Arc<AtomicU64>,
    instance_id: CoreInstanceId,
}

impl teams::TeamEventSink for AppTeamEventSink {
    fn record(&self, envelope: teams::TeamEventEnvelope) {
        let sequence = self.stream_sequence.fetch_add(1, Ordering::SeqCst);
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: self.instance_id.clone(),
            // 复用 team 事件自身的 event_id，GUI / 重放可跨流关联同一事实。
            event_id: envelope.event_id.clone(),
            global_sequence: GlobalSequence(self.global_sequence.fetch_add(1, Ordering::SeqCst)),
            stream: EventStream::Global,
            stream_sequence: sequence + 1,
            timestamp: envelope.timestamp,
            source: EventSource::Core,
            payload: AppEvent::TeamEvent {
                event: Box::new(crate::team::to_app_event(&envelope.payload)),
            },
        };
        self.limiter.push(app_envelope);
    }
}

/// 脱敏 Alert → core_api 安全视图：字段 1:1 镜像，凭据只留掩码提示。
fn quota_alert_from(alert: &quota_service::refresh::Alert) -> core_api::QuotaAlert {
    core_api::QuotaAlert {
        tenant_id: alert.scope.tenant_id.clone(),
        account_id: alert.scope.account_id.as_str().to_string(),
        provider_id: alert.scope.provider_id.clone(),
        model_id: alert.scope.model_id.clone(),
        window: quota_window_from(alert.window),
        unit: quota_unit_from(&alert.unit),
        // 新事件总是带完整归属（Some）；None 仅用于旧持久化 JSON 的重放。
        kind: Some(quota_alert_kind_from(alert.kind)),
        severity: alert_severity_from(alert),
        // 生产路径的 Alert.source 已由 quota-service 脱敏；此处再用现有公开
        // helper 二次脱敏（最后防线），确保异常构造的 source 也不会把
        // query/fragment/secret 带进可持久化事件。
        source: Some(quota_service::adapters::http_util::redact_secrets(
            &alert.source,
        )),
        message: alert_message(alert),
        snapshot: None,
        credential_hint: alert
            .scope
            .credential_id
            .as_deref()
            .and_then(core_api::mask_credential_hint),
    }
}

/// 稳定 kind 映射：与 quota-service `AlertKind` 1:1 镜像，枚举形态冻结。
fn quota_alert_kind_from(kind: quota_service::refresh::AlertKind) -> QuotaAlertKind {
    match kind {
        quota_service::refresh::AlertKind::Threshold => QuotaAlertKind::Threshold,
        quota_service::refresh::AlertKind::Recovered => QuotaAlertKind::Recovered,
        quota_service::refresh::AlertKind::Stale => QuotaAlertKind::Stale,
        quota_service::refresh::AlertKind::ReauthorizationRequired => {
            QuotaAlertKind::ReauthorizationRequired
        }
        quota_service::refresh::AlertKind::PartialFailure => QuotaAlertKind::PartialFailure,
    }
}

/// 稳定 severity：可重放、不依赖时间/上下文。仅 Threshold 区分
/// `advisory`：真实（非 advisory）阈值告警为 Critical，其余（advisory 的
/// 抓取/估算阈值、Stale、PartialFailure）保持 Warning。
fn alert_severity_from(alert: &quota_service::refresh::Alert) -> core_api::QuotaAlertSeverity {
    match alert.kind {
        quota_service::refresh::AlertKind::ReauthorizationRequired => {
            core_api::QuotaAlertSeverity::Critical
        }
        quota_service::refresh::AlertKind::Recovered => core_api::QuotaAlertSeverity::Info,
        quota_service::refresh::AlertKind::Threshold if !alert.advisory => {
            core_api::QuotaAlertSeverity::Critical
        }
        quota_service::refresh::AlertKind::Threshold
        | quota_service::refresh::AlertKind::Stale
        | quota_service::refresh::AlertKind::PartialFailure => {
            core_api::QuotaAlertSeverity::Warning
        }
    }
}

/// 稳定消息：只含 kind 与剩余百分比（如有）；绝不拼入 `source` 标签或凭据。
fn alert_message(alert: &quota_service::refresh::Alert) -> String {
    match alert.kind {
        quota_service::refresh::AlertKind::Threshold => format!(
            "quota threshold breached: remaining {pct}%",
            pct = alert.remaining_percent.unwrap_or(0)
        ),
        quota_service::refresh::AlertKind::Recovered => format!(
            "quota recovered: remaining {pct}%",
            pct = alert.remaining_percent.unwrap_or(100)
        ),
        quota_service::refresh::AlertKind::Stale => {
            "quota data stale: fresh fetch failed, serving cached snapshot".to_string()
        }
        quota_service::refresh::AlertKind::ReauthorizationRequired => {
            "credential requires reauthorization".to_string()
        }
        quota_service::refresh::AlertKind::PartialFailure => {
            "quota refresh partially failed: some sources unavailable".to_string()
        }
    }
}

fn quota_window_from(window: quota_service::QuotaWindow) -> core_api::QuotaWindow {
    match window {
        quota_service::QuotaWindow::Overall => core_api::QuotaWindow::Overall,
        quota_service::QuotaWindow::Rolling5h => core_api::QuotaWindow::Rolling5h,
        quota_service::QuotaWindow::Weekly => core_api::QuotaWindow::Weekly,
        quota_service::QuotaWindow::Monthly => core_api::QuotaWindow::Monthly,
    }
}

fn quota_unit_from(unit: &quota_service::QuotaUnit) -> core_api::QuotaUnit {
    match unit {
        quota_service::QuotaUnit::Count => core_api::QuotaUnit::Count,
        quota_service::QuotaUnit::Token => core_api::QuotaUnit::Token,
        quota_service::QuotaUnit::Cost { currency } => core_api::QuotaUnit::Cost {
            currency: currency.clone(),
        },
    }
}

/// 从 Agent 事件中提取「最近一次观测到的用量快照」。
///
/// `UsageUpdated` 是 Provider 流式用量在 canonical 事件流上的投影（LoopSink
/// 广播），`RunCompleted` 携带 run 级累计最终用量；二者都刷新快照，供终态记账使用。
/// `RunFailed` / `RunCancelled` 本身不携带用量，失败/取消的已发生用量依赖在此
/// 之前观测到的 `UsageUpdated`——这是「失败/取消不丢已发生用量」的依据。
fn usage_from_event(event: &AgentEvent) -> Option<agent_domain::TokenUsage> {
    match event {
        AgentEvent::UsageUpdated { usage } => Some(usage.clone()),
        AgentEvent::RunCompleted { usage, .. } => Some(usage.clone()),
        _ => None,
    }
}

/// 单次消费（原始 run 或某次 retry）的 Provider 用量累计器。
///
/// `ProviderRequestStarted` 划分 Provider turn：新 turn 开始时把上一 turn 的最新
/// 快照提交到累计值；同一 turn 内多个 `UsageUpdated` 只覆盖当前快照。`RunCompleted`
/// 的 usage 是 run 级累计值，作为终态权威总量直接覆盖而非相加（避免多轮双计）。
/// 失败/取消没有终态 usage 时，当前 turn 已观测到的最后快照仍会进入最终总量。
#[derive(Debug, Default)]
struct AttemptUsage {
    completed_turns: agent_domain::TokenUsage,
    current_turn: Option<agent_domain::TokenUsage>,
    /// RunCompleted 携带的 run 级累计用量（终态权威总量）。
    run_terminal: Option<agent_domain::TokenUsage>,
    occurred_at_ms: Option<u64>,
}

impl AttemptUsage {
    fn observe(&mut self, event: &AgentEvent, timestamp: Timestamp) {
        if matches!(event, AgentEvent::ProviderRequestStarted { .. }) {
            self.finish_current_turn();
        }
        match event {
            AgentEvent::RunCompleted { usage, .. } => {
                // RunCompleted 的 usage 是 run 级累计值：覆盖总量，不再与
                // completed_turns 相加（否则多轮 run 双计）。
                self.run_terminal = Some(usage.clone());
                self.current_turn = None;
                self.occurred_at_ms = Some(timestamp.as_unix_millis());
            }
            _ => {
                if let Some(usage) = usage_from_event(event) {
                    self.current_turn = Some(usage);
                    self.occurred_at_ms = Some(timestamp.as_unix_millis());
                }
            }
        }
    }

    fn snapshot(&self) -> Option<(agent_domain::TokenUsage, u64)> {
        let occurred_at_ms = self.occurred_at_ms?;
        let total = match &self.run_terminal {
            Some(terminal) => terminal.clone(),
            None => {
                let mut total = self.completed_turns.clone();
                if let Some(current) = &self.current_turn {
                    add_usage(&mut total, current);
                }
                total
            }
        };
        Some((total, occurred_at_ms))
    }

    fn finish_current_turn(&mut self) {
        if let Some(current) = self.current_turn.take() {
            add_usage(&mut self.completed_turns, &current);
        }
    }
}

fn add_usage(total: &mut agent_domain::TokenUsage, usage: &agent_domain::TokenUsage) {
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    total.cache_write_tokens = total
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
}

fn envelope_matches_run(
    envelope: &AgentEventEnvelope,
    run_id: &RunId,
    session_id: &SessionId,
) -> bool {
    envelope.run_id == *run_id && envelope.session_id == *session_id
}

/// 幂等记账：`record_id` 由 (run, session, provider) 确定性派生（session 防
/// 跨 session 冲突），相同内容重复写入为重放成功（usage-ledger 幂等语义），
/// 不会重复计数。
///
/// 归属字段取调用方注入的 [`usage_ledger::UsageAttribution`]
/// （P18-8 review：`record_run_usage` 不再内部派生 synthetic 身份/账号）：
/// tenant/principal 来自 run 请求的 [`tenant_service::IdentityContext`]
/// （router 入口 fail-closed 解析）。P18-4 生产接线后：account/credential
/// 由 [`spawn_run_task`] 在 acquire 成功后从真实 [`CredentialLease`] 构造
/// （[`attribution_from_lease`]），trace 取 acquire 请求的 trace_id；未注入
/// 池的测试 / 嵌入式路径保留 legacy 过渡派生（account 默认哨兵、credential
/// 为 `None`）。P18-4 审查补救：`agent_id` 为 session 作用域 canonical root
/// AgentId（[`RunRequest::agent_id`]），UsageRecord 不再写默认空值。本函数
/// 契约不变。
/// `session_id` 来自 run 请求（非默认），`occurred_at_ms` 取用量观测/终态
/// 事件的真实时间戳（非 run 创建时间）。费用按 builtin 定价估算
/// （`cost_micros`/`currency`）；未知模型/无定价回退 0/USD，不影响记账。
#[allow(clippy::too_many_arguments)]
fn record_run_usage(
    run_id: &RunId,
    session_id: &SessionId,
    agent_id: &AgentId,
    provider_id: &ProviderId,
    model: &ModelId,
    attempt: u64,
    occurred_at_ms: u64,
    usage: &agent_domain::TokenUsage,
    attribution: &usage_ledger::UsageAttribution,
) -> usage_ledger::UsageRecord {
    let (cost_micros, currency, cost_confidence, cost_provenance) =
        estimate_cost_fields(model, usage);
    usage_ledger::UsageRecord {
        record_id: format!(
            "auto-rec-run-{}-session-{}-{}-attempt-{attempt}",
            run_id,
            session_id,
            provider_id.as_str(),
        ),
        version: usage_ledger::RECORD_VERSION,
        tenant_id: attribution.tenant_id.clone(),
        principal_id: attribution.principal_id.clone(),
        account_id: attribution.account_id.clone(),
        credential_id: attribution.credential_id.clone(),
        session_id: session_id.clone(),
        agent_id: agent_id.clone(),
        run_id: Some(run_id.clone()),
        provider_id: provider_id.clone(),
        model_id: model.clone(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        cost_micros,
        // currency 是必填校验字段（3 位大写）；未知模型/无定价回退中性 USD。
        currency,
        occurred_at_ms,
        // P18-8 v2：trace 归属每次实际上游 attempt；定价快照来自 builtin
        // rate card（历史费用不随价格更新漂移）。终态兜底记录无单次上游
        // request/event 来源，置 None（逐上游调用记账见 spawn_run_task
        // 的 per-call 路径，带 request_id/event_id/attempt）。
        request_id: None,
        event_id: None,
        upstream_attempt: Some(attempt),
        trace_id: attribution.trace_id.clone(),
        rate_card: Some(model_registry::BUILTIN_RATE_CARD.to_string()),
        rate_version: Some(model_registry::BUILTIN_RATE_VERSION.to_string()),
        cost_confidence,
        cost_provenance,
    }
}

/// 单次实际上游调用（provider turn）的用量观测，供逐调用独立记账。
#[derive(Clone, Debug, Default)]
struct TurnObservation {
    request_id: RequestId,
    usage: agent_domain::TokenUsage,
    event_id: EventId,
    occurred_at_ms: u64,
    /// 该 run 内第几次上游调用（1 基）。retry/failover 每次新
    /// `ProviderRequestStarted` 递增。
    turn_index: u64,
}

/// 事件循环内逐 turn 收口：`ProviderRequestStarted` 开启新 turn，同一 turn
/// 内最后一次 `UsageUpdated` 快照为该调用归属（`ProviderStreamEvent` 的
/// usage 是单次请求累计值，见 AttemptUsage 语义）；无用量观测的调用不产生
/// 零成本噪声记录。
#[derive(Debug, Default)]
struct TurnObserver {
    current_request: Option<RequestId>,
    current_usage: Option<agent_domain::TokenUsage>,
    current_event: Option<EventId>,
    current_occurred: Option<u64>,
    turn_index: u64,
    turns: Vec<TurnObservation>,
}

impl TurnObserver {
    fn observe(&mut self, event: &AgentEvent, envelope: &AgentEventEnvelope) {
        match event {
            AgentEvent::ProviderRequestStarted { request_id, .. } => {
                self.close_turn();
                self.current_request = Some(request_id.clone());
                self.turn_index = self.turn_index.saturating_add(1);
            }
            AgentEvent::UsageUpdated { usage } => {
                self.current_usage = Some(usage.clone());
                self.current_event = Some(envelope.event_id.clone());
                self.current_occurred = Some(envelope.timestamp.as_unix_millis());
            }
            _ => {}
        }
    }

    fn close_turn(&mut self) {
        if let (Some(request_id), Some(usage), Some(event_id), Some(occurred_at_ms)) = (
            self.current_request.as_ref(),
            self.current_usage.as_ref(),
            self.current_event.as_ref(),
            self.current_occurred,
        ) {
            self.turns.push(TurnObservation {
                request_id: request_id.clone(),
                usage: usage.clone(),
                event_id: event_id.clone(),
                occurred_at_ms,
                turn_index: self.turn_index,
            });
        }
        self.current_request = None;
        self.current_usage = None;
        self.current_event = None;
        self.current_occurred = None;
    }
}

/// 逐上游调用记账记录：`record_id` 由 (run, request, turn) 确定性派生，
/// request_id/event_id/attempt 随记录持久化，账本层按
/// (tenant, account, request, attempt) 去重。
fn record_run_usage_per_call(
    run_id: &RunId,
    session_id: &SessionId,
    agent_id: &AgentId,
    provider_id: &ProviderId,
    model: &ModelId,
    turn: &TurnObservation,
    attribution: &usage_ledger::UsageAttribution,
) -> usage_ledger::UsageRecord {
    let (cost_micros, currency, cost_confidence, cost_provenance) =
        estimate_cost_fields(model, &turn.usage);
    usage_ledger::UsageRecord {
        record_id: format!(
            "auto-rec-run-{}-request-{}-attempt-{}",
            run_id, turn.request_id, turn.turn_index,
        ),
        version: usage_ledger::RECORD_VERSION,
        tenant_id: attribution.tenant_id.clone(),
        principal_id: attribution.principal_id.clone(),
        account_id: attribution.account_id.clone(),
        credential_id: attribution.credential_id.clone(),
        session_id: session_id.clone(),
        agent_id: agent_id.clone(),
        run_id: Some(run_id.clone()),
        provider_id: provider_id.clone(),
        model_id: model.clone(),
        input_tokens: turn.usage.input_tokens,
        output_tokens: turn.usage.output_tokens,
        cache_read_tokens: turn.usage.cache_read_tokens,
        cache_write_tokens: turn.usage.cache_write_tokens,
        cost_micros,
        currency,
        occurred_at_ms: turn.occurred_at_ms,
        request_id: Some(turn.request_id.clone()),
        event_id: Some(turn.event_id.clone()),
        upstream_attempt: Some(turn.turn_index),
        trace_id: attribution.trace_id.clone(),
        rate_card: Some(model_registry::BUILTIN_RATE_CARD.to_string()),
        rate_version: Some(model_registry::BUILTIN_RATE_VERSION.to_string()),
        cost_confidence,
        cost_provenance,
    }
}

/// 按 builtin rate card 估算费用与 provenance（终态兜底与逐调用记账共用，
/// 保证同模型同用量同价）。未知模型/无定价回退 0/USD，不影响记账。
fn estimate_cost_fields(
    model: &ModelId,
    usage: &agent_domain::TokenUsage,
) -> (
    u64,
    String,
    Option<usage_ledger::CostConfidence>,
    Option<String>,
) {
    let estimated = ModelRegistry::builtin().estimate_cost(model.as_str(), usage);
    let (cost_micros, currency) = match &estimated {
        Some(cost) => (cost.amount_micros, cost.currency.clone()),
        None => (0, "USD".to_string()),
    };
    let (cost_confidence, cost_provenance) = match &estimated {
        Some(_) => (
            Some(usage_ledger::CostConfidence::Estimated),
            Some(format!(
                "{}:{}:estimate",
                model_registry::BUILTIN_RATE_CARD,
                model_registry::BUILTIN_RATE_VERSION
            )),
        ),
        None => (
            Some(usage_ledger::CostConfidence::Unknown),
            Some("no-pricing:unknown-model".to_string()),
        ),
    };
    (cost_micros, currency, cost_confidence, cost_provenance)
}

/// 记账 + 本地缓存刷新（P18-8 逐调用与终态兜底共用）：记账成功后才刷新
/// 缓存，账本失败不发布缓存，保证缓存与账本一致。失败只上报结构化摘要
/// （不含密钥/凭据），不改变 run 终态语义。返回是否全部成功。
async fn record_usage_and_refresh(
    runtime: &Arc<crate::QuotaRuntime>,
    record: &usage_ledger::UsageRecord,
    run_id: &RunId,
    session_id: &SessionId,
    provider_id: &ProviderId,
    model: &ModelId,
    attempt: u64,
) -> bool {
    if let Err(error) = runtime.ledger.record(record.clone()).await {
        tracing::warn!(
            run_id = %run_id,
            session_id = %session_id,
            provider_id = %provider_id.as_str(),
            model = %model.as_str(),
            attempt,
            error = %error,
            "usage ledger record failed; run usage not persisted",
        );
        return false;
    }
    if let Err(failures) = runtime.refresh_local_cache(record).await {
        tracing::warn!(
            run_id = %run_id,
            session_id = %session_id,
            provider_id = %provider_id.as_str(),
            model = %model.as_str(),
            attempt,
            failed_keys = failures.len(),
            scope = "local-ledger",
            "local usage cache refresh failed; cache may be stale",
        );
        return false;
    }
    true
}

/// 按该 record 的完整 model/credential scope 构建 Token overview，经共享
/// push_event 发布一次 QuotaChanged（run 流）。仅记账 + 缓存刷新都成功的
/// record 走这里（调用方保证），失败路径不发布。
#[allow(clippy::too_many_arguments)]
fn push_quota_changed(
    limiter: &RateLimiter,
    instance_id: &CoreInstanceId,
    global_sequence: &AtomicU64,
    stream_sequence: &AtomicU64,
    source: &CommandSource,
    command_id: &CommandId,
    run_id: &RunId,
    runtime: &Arc<crate::QuotaRuntime>,
    record: &usage_ledger::UsageRecord,
) {
    let query = QuotaOverviewQuery {
        tenant_id: record.tenant_id.clone(),
        account_id: record.account_id.clone(),
        provider_id: Some(record.provider_id.clone()),
        credential_id: record.credential_id.clone(),
        model_id: Some(record.model_id.clone()),
        windows: Vec::new(),
        unit: Some(QuotaUnit::Token),
    };
    let view = crate::router::CommandRouter::cached_quota_overview(
        runtime,
        &query,
        record.provider_id.clone(),
    );
    push_event(
        limiter,
        instance_id,
        global_sequence,
        stream_sequence,
        source,
        command_id,
        run_id,
        AppEvent::QuotaChanged {
            view: Box::new(view),
        },
        now_timestamp(),
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_run_task(
    run_id: RunId,
    session_id: SessionId,
    workspace_id: Option<WorkspaceId>,
    identity: tenant_service::IdentityContext,
    agent_id: AgentId,
    source: CommandSource,
    command_id: CommandId,
    aggregate: Arc<AggregateState>,
    approvals: Arc<ApprovalRegistry>,
    limiter: Arc<RateLimiter>,
    broadcaster: EventBroadcaster,
    instance_id: CoreInstanceId,
    global_sequence: Arc<AtomicU64>,
    terminal_counters: Arc<Mutex<TerminalCounters>>,
    provider: Arc<dyn ModelProvider>,
    config: ProviderLoopConfig,
    queue: Arc<agent_engine::MessageQueue>,
    cancel: CancelHandle,
    task_state: Arc<Mutex<RunState>>,
    provider_id: ProviderId,
    model: ModelId,
    attempt: u64,
    _created_at: Timestamp,
    external_quota: Option<agent_engine::ExternalQuotaSignal>,
    quota_runtime: Option<Arc<crate::QuotaRuntime>>,
    user_hooks: Option<Arc<UserHookHost>>,
    workspace_roots: Vec<PathBuf>,
    tool_rules: ProfileToolRules,
    isolation: ProfileIsolation,
    background_task_id: Option<BackgroundTaskId>,
    task_manager: Option<Arc<task_manager::TaskManager>>,
    credential_pool: Option<Arc<dyn CredentialPool>>,
    tenant_policy: Option<Arc<TenantPolicyGate>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // P18-4：每个 run attempt 在 provider 调用前异步 acquire 一次 lease，
        // 持有 LeaseGuard 至终态（Drop 时以 outcome 释放）。retry 生成新
        // attempt → 重新 acquire。acquire 失败 fail-closed：run 直接进入
        // Failed，绝不调用 provider（并发额度是账号侧硬闸门，不降级为无租约运行）。
        let stream_sequence = AtomicU64::new(0);
        let mut lease_guard: Option<LeaseGuard> = None;
        // P18-9 fail-closed 收尾：策略拒绝（lease 权限 / account / 预算）→
        // run Failed、释放 lease、绝不调用 provider。
        let fail_policy = |reason: String| {
            tracing::error!(
                run_id = %run_id,
                session_id = %session_id,
                provider_id = %provider_id.as_str(),
                attempt,
                reason,
                "tenant policy denied; run fails closed without provider call"
            );
            let _ = aggregate.set_run_state(&run_id, RunState::Failed);
            push_run_changed(
                &limiter,
                &instance_id,
                &global_sequence,
                &stream_sequence,
                &source,
                &command_id,
                &run_id,
                RunState::Failed,
            );
            *task_state.lock().expect("run task state") = RunState::Failed;
            lock(&terminal_counters).failed += 1;
        };
        let attribution = match credential_pool.as_ref() {
            Some(pool) => {
                let trace_id = format!("run:{run_id}:attempt:{attempt}");
                let request = AcquireRequest {
                    tenant_id: identity.tenant_id.clone(),
                    principal_id: identity.principal_id.clone(),
                    session_id: session_id.clone(),
                    agent_id: agent_id.clone(),
                    provider_id: Some(provider_id.clone()),
                    account_id: None,
                    trace_id: Some(trace_id.clone()),
                };
                match pool.acquire_guard(request).await {
                    Ok(mut guard) => {
                        // P18-9：lease 取得后强制 LeaseAcquire 权限 + account
                        // 白名单（真实 lease account），拒绝时释放 lease、
                        // run fail-closed。
                        let lease = guard
                            .lease()
                            .expect("freshly acquired guard must hold a lease");
                        if let Some(gate) = tenant_policy.as_ref() {
                            if let Err(error) =
                                gate.check_permission(&identity, Permission::LeaseAcquire)
                            {
                                gate.record_decision(
                                    &identity,
                                    PolicyGate::LeaseAcquire,
                                    PolicyDecisionKind::Deny,
                                    error.to_string(),
                                );
                                *guard.outcome_mut() = LeaseOutcome::Released;
                                fail_policy(error.to_string());
                                return;
                            }
                            if let Err(error) =
                                gate.check_account(&identity.tenant_id, &lease.account_id)
                            {
                                gate.record_decision(
                                    &identity,
                                    PolicyGate::LeaseAcquire,
                                    PolicyDecisionKind::Deny,
                                    error.to_string(),
                                );
                                *guard.outcome_mut() = LeaseOutcome::Released;
                                fail_policy(error.to_string());
                                return;
                            }
                            gate.record_decision_scoped(
                                &identity,
                                PolicyGate::LeaseAcquire,
                                PolicyDecisionKind::Allow,
                                "lease 准入放行",
                                AuditDimensions {
                                    session_id: Some(session_id.clone()),
                                    agent_id: Some(agent_id.clone()),
                                    provider_id: Some(provider_id.clone()),
                                    account_id: Some(lease.account_id.clone()),
                                    trace_id: Some(trace_id.clone()),
                                    ..AuditDimensions::default()
                                },
                            );
                        }
                        // UsageAttribution 从真实 CredentialLease 得到：account/
                        // credential 一律取 lease 值，客户端（RunRequest）不可
                        // 传入 credential；trace 取 acquire 请求的 trace_id。
                        let attribution = match attribution_from_lease(
                            lease,
                            &identity,
                            &session_id,
                            &agent_id,
                            &provider_id,
                            trace_id,
                        ) {
                            Ok(attribution) => attribution,
                            Err(reason) => {
                                tracing::error!(
                                    run_id = %run_id,
                                    session_id = %session_id,
                                    provider_id = %provider_id.as_str(),
                                    attempt,
                                    reason,
                                    "credential lease scope mismatch; run fails closed without provider call"
                                );
                                *guard.outcome_mut() = LeaseOutcome::Released;
                                let _ = aggregate.set_run_state(&run_id, RunState::Failed);
                                push_run_changed(
                                    &limiter,
                                    &instance_id,
                                    &global_sequence,
                                    &stream_sequence,
                                    &source,
                                    &command_id,
                                    &run_id,
                                    RunState::Failed,
                                );
                                *task_state.lock().expect("run task state") = RunState::Failed;
                                lock(&terminal_counters).failed += 1;
                                return;
                            }
                        };
                        lease_guard = Some(guard);
                        if let Some(gate) = tenant_policy.as_ref() {
                            gate.record_control_event(
                                &identity,
                                AuditAction::AgentLifecycle,
                                AuditTargetKind::Agent,
                                AuditDecision::Allow,
                                "agent_run_started",
                                AuditDimensions {
                                    session_id: Some(session_id.clone()),
                                    agent_id: Some(agent_id.clone()),
                                    provider_id: Some(provider_id.clone()),
                                    account_id: Some(attribution.account_id.clone().into()),
                                    trace_id: attribution.trace_id.clone(),
                                    ..AuditDimensions::default()
                                },
                                attempt,
                            );
                        }
                        attribution
                    }
                    Err(error) => {
                        tracing::error!(
                            run_id = %run_id,
                            session_id = %session_id,
                            provider_id = %provider_id.as_str(),
                            attempt,
                            error = %error,
                            "credential lease acquire failed; run fails closed without \
                             provider call"
                        );
                        let _ = aggregate.set_run_state(&run_id, RunState::Failed);
                        push_run_changed(
                            &limiter,
                            &instance_id,
                            &global_sequence,
                            &stream_sequence,
                            &source,
                            &command_id,
                            &run_id,
                            RunState::Failed,
                        );
                        {
                            let mut guard = task_state.lock().expect("run task state");
                            *guard = RunState::Failed;
                        }
                        {
                            let mut guard = lock(&terminal_counters);
                            guard.failed += 1;
                        }
                        return;
                    }
                }
            }
            // 未注入池（测试 / 嵌入式）：保留 legacy 过渡归属（无 credential）。
            None => usage_ledger::UsageAttribution {
                tenant_id: identity.tenant_id.clone(),
                principal_id: identity.principal_id.clone(),
                account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
                credential_id: None,
                trace_id: None,
            },
        };
        // P18-9：预算 admission 在 provider 调用前执行，数据源是唯一共享
        // UsageLedger（quota_runtime.ledger）。仅当租户策略配置了任一预算
        // 维度时检查；账本缺失或查询失败 fail-closed（拒绝时释放 lease）。
        if let Some(gate) = tenant_policy.as_ref() {
            let policy = gate.engine().policy(&identity.tenant_id);
            let budget_configured = policy.daily_input_token_budget.is_some()
                || policy.daily_output_token_budget.is_some()
                || policy.daily_cost_micros_budget.is_some();
            if budget_configured {
                let ledger = match quota_runtime.as_ref() {
                    Some(runtime) => Arc::clone(&runtime.ledger),
                    None => {
                        let reason =
                            "预算已配置但无共享 UsageLedger，预算 admission 无法执行（fail-closed）"
                                .to_string();
                        gate.record_decision(
                            &identity,
                            PolicyGate::RequestAdmission,
                            PolicyDecisionKind::Deny,
                            reason.clone(),
                        );
                        release_lease(&mut lease_guard);
                        fail_policy(reason);
                        return;
                    }
                };
                let day_start = epoch_millis() - (epoch_millis() % MS_PER_DAY);
                let query = UsageQuery {
                    tenant_id: Some(identity.tenant_id.clone()),
                    occurred_at_start_ms: Some(day_start),
                    ..UsageQuery::default()
                };
                match ledger.aggregate(&query).await {
                    Ok(totals) => {
                        let decision = decide_budget(
                            totals.input_tokens,
                            totals.output_tokens,
                            totals.cost_micros,
                            policy.daily_input_token_budget,
                            policy.daily_output_token_budget,
                            policy.daily_cost_micros_budget,
                        );
                        match decision {
                            PolicyDecision::Allow => {
                                gate.record_decision(
                                    &identity,
                                    PolicyGate::RequestAdmission,
                                    PolicyDecisionKind::Allow,
                                    "预算 admission 放行",
                                );
                            }
                            PolicyDecision::Deny { reason } => {
                                gate.record_decision(
                                    &identity,
                                    PolicyGate::RequestAdmission,
                                    PolicyDecisionKind::Deny,
                                    reason.clone(),
                                );
                                release_lease(&mut lease_guard);
                                fail_policy(reason);
                                return;
                            }
                            other => {
                                let reason =
                                    format!("预算 admission 裁决异常：{other:?}（deny-first）");
                                gate.record_decision(
                                    &identity,
                                    PolicyGate::RequestAdmission,
                                    PolicyDecisionKind::Deny,
                                    reason.clone(),
                                );
                                release_lease(&mut lease_guard);
                                fail_policy(reason);
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let reason = format!("预算 admission 查询账本失败（fail-closed）：{error}");
                        gate.record_decision(
                            &identity,
                            PolicyGate::RequestAdmission,
                            PolicyDecisionKind::Deny,
                            reason.clone(),
                        );
                        release_lease(&mut lease_guard);
                        fail_policy(reason);
                        return;
                    }
                }
            }
        }
        let context = Arc::new(AppLoopContext::new(
            run_id.clone(),
            session_id.clone(),
            Arc::clone(&aggregate),
            workspace_id,
            workspace_roots,
            Arc::clone(&approvals),
            cancel.clone(),
            user_hooks,
            tool_rules,
            isolation,
            attempt,
        ));
        let mut engine = ProviderLoop::new_with_external_quota(
            provider,
            context,
            config,
            1,
            broadcaster.clone(),
            external_quota,
        );
        let mut subscriber = broadcaster.subscribe();
        let mut last_state = RunState::Created;
        let mut attempt_usage = AttemptUsage::default();
        // P18-8：归属在 run 生命周期派生。P18-4 稳定后：注入池时 account/
        // credential 来自真实 CredentialLease（见上方 acquire 分支），本处不再
        // 派生默认账号；账本契约本身只接受调用方注入的归属，绝不回退到账本
        // 内部猜默认账号。未注入池的 legacy 分支仅为测试 / 嵌入式保留。
        // 逐上游调用记账观测：`ProviderRequestStarted` 开启新 turn（retry/
        // failover 同 turn 内以最后一次 `UsageUpdated` 快照归属），每次
        // turn 收口为一条独立不可变记录（request_id + event_id + attempt）。
        let mut turn_observer = TurnObserver::default();

        let engine_future = engine.run(queue, cancel.clone());
        tokio::pin!(engine_future);
        let outcome = loop {
            tokio::select! {
                event = subscriber.recv() => {
                    match event {
                        Ok(envelope) => {
                            // broadcaster 为全局共享：任何状态/用量更新前必须同时校验
                            // run_id 与 session_id，避免并发 run 相互串账或串状态。
                            if !envelope_matches_run(&envelope, &run_id, &session_id) {
                                continue;
                            }
                            turn_observer.observe(&envelope.payload, &envelope);
                            attempt_usage.observe(&envelope.payload, envelope.timestamp);
                            last_state = apply_agent_event(
                                &aggregate,
                                &limiter,
                                &instance_id,
                                &global_sequence,
                                &stream_sequence,
                                &source,
                                &command_id,
                                envelope,
                                last_state,
                            );
                        }
                        Err(agent_engine::BroadcastError::Lagged { .. }) => continue,
                        Err(agent_engine::BroadcastError::Closed) => break None,
                        Err(agent_engine::BroadcastError::NoSubscribers) => continue,
                    }
                }
                result = &mut engine_future => break Some(result),
            }
        };

        // 冲刷引擎完成前后可能残留的事件。
        loop {
            match subscriber.try_recv() {
                Ok(Some(envelope)) => {
                    if !envelope_matches_run(&envelope, &run_id, &session_id) {
                        continue;
                    }
                    turn_observer.observe(&envelope.payload, &envelope);
                    attempt_usage.observe(&envelope.payload, envelope.timestamp);
                    last_state = apply_agent_event(
                        &aggregate,
                        &limiter,
                        &instance_id,
                        &global_sequence,
                        &stream_sequence,
                        &source,
                        &command_id,
                        envelope,
                        last_state,
                    );
                }
                // Lagged 只表示旧事件已被丢弃，receiver 仍可继续读取最新事件。
                Err(agent_engine::BroadcastError::Lagged { .. }) => continue,
                Ok(None)
                | Err(agent_engine::BroadcastError::Closed)
                | Err(agent_engine::BroadcastError::NoSubscribers) => break,
            }
        }
        turn_observer.close_turn();

        // 成功时保留 engine Ok 的 summary：其 usage 是 run 级累计值（ProviderLoop
        // 在发出 RunCompleted 前已饱和累加），用作终态权威记账，即使广播丢失
        // （Lagged）也可据此记账。
        let engine_summary = match &outcome {
            Some(Ok((_, summary))) => Some(summary.clone()),
            _ => None,
        };
        let final_state = match outcome {
            Some(Ok((engine_state, _))) => to_core_state(engine_state),
            Some(Err(LoopError::Cancelled)) => RunState::Cancelled,
            Some(Err(_)) => RunState::Failed,
            None => RunState::Failed,
        };
        if !terminal(&last_state) {
            let _ = aggregate.set_run_state(&run_id, final_state.clone());
            if final_state != last_state {
                push_run_changed(
                    &limiter,
                    &instance_id,
                    &global_sequence,
                    &stream_sequence,
                    &source,
                    &command_id,
                    &run_id,
                    final_state.clone(),
                );
            }
        }
        approvals.clear_run(&run_id);
        if let Some(gate) = tenant_policy.as_ref() {
            let (decision, reason_code) = match final_state {
                RunState::Completed => (AuditDecision::Allow, "agent_run_completed"),
                RunState::Cancelled => (AuditDecision::Observe, "agent_run_cancelled"),
                RunState::Failed | RunState::Interrupted => {
                    (AuditDecision::Error, "agent_run_failed")
                }
                _ => (AuditDecision::Error, "agent_run_invalid_terminal"),
            };
            gate.record_control_event(
                &identity,
                AuditAction::AgentLifecycle,
                AuditTargetKind::Agent,
                decision,
                reason_code,
                AuditDimensions {
                    session_id: Some(session_id.clone()),
                    agent_id: Some(agent_id.clone()),
                    provider_id: Some(provider_id.clone()),
                    account_id: Some(attribution.account_id.clone().into()),
                    trace_id: attribution.trace_id.clone(),
                    ..AuditDimensions::default()
                },
                attempt,
            );
        }
        // P18-8：终态记账——每次实际上游 call/retry/failover 以 provider
        // request/event id + attempt 独立不可变记录（`TurnObserver` 在事件
        // 循环中逐 turn 收口），记录按 request/attempt 在账本层去重；全部
        // 写完后以最后一条成功记录的 scope 发布一次终态 QuotaChanged。
        // 仅在没有任何 per-call 记录（如事件被 Lagged 全部丢弃）时才以
        // run 终态汇总单条兜底，二者互斥防双计。记账成功后才刷新本地额度
        // 缓存（四窗口 Token/Cost）；账本失败不发布缓存，缓存与账本一致。
        if terminal(&final_state) {
            if let Some(runtime) = quota_runtime.as_ref() {
                let per_call_records: Vec<usage_ledger::UsageRecord> = turn_observer
                    .turns
                    .iter()
                    .map(|turn| {
                        record_run_usage_per_call(
                            &run_id,
                            &session_id,
                            &agent_id,
                            &provider_id,
                            &model,
                            turn,
                            &attribution,
                        )
                    })
                    .collect();
                if !per_call_records.is_empty() {
                    let mut last_ok: Option<usage_ledger::UsageRecord> = None;
                    for record in &per_call_records {
                        if record_usage_and_refresh(
                            runtime,
                            record,
                            &run_id,
                            &session_id,
                            &provider_id,
                            &model,
                            attempt,
                        )
                        .await
                        {
                            last_ok = Some(record.clone());
                        }
                    }
                    if let Some(record) = last_ok {
                        push_quota_changed(
                            &limiter,
                            &instance_id,
                            &global_sequence,
                            &stream_sequence,
                            &source,
                            &command_id,
                            &run_id,
                            runtime,
                            &record,
                        );
                    }
                } else {
                    // 兜底：无任何 per-call 记录时以 run 终态汇总单条记账。
                    // 成功以 engine Ok summary 的 run 级累计 usage 权威记账，
                    // 时间取终态观测时间、缺失时回退当前时间；失败/取消仍用
                    // 已观测快照，不丢已发生用量。record_id 由 (run, session,
                    // provider) 确定性派生，重放内容稳定，ledger 幂等语义
                    // 保证不重复计数。
                    let finalized = match &engine_summary {
                        Some(summary) => Some((
                            summary.usage.clone(),
                            attempt_usage
                                .occurred_at_ms
                                .unwrap_or_else(|| now_timestamp().as_unix_millis()),
                        )),
                        None => attempt_usage.snapshot(),
                    };
                    if let Some((usage, occurred_at_ms)) = finalized {
                        let record = record_run_usage(
                            &run_id,
                            &session_id,
                            &agent_id,
                            &provider_id,
                            &model,
                            attempt,
                            occurred_at_ms,
                            &usage,
                            &attribution,
                        );
                        if record_usage_and_refresh(
                            runtime,
                            &record,
                            &run_id,
                            &session_id,
                            &provider_id,
                            &model,
                            attempt,
                        )
                        .await
                        {
                            push_quota_changed(
                                &limiter,
                                &instance_id,
                                &global_sequence,
                                &stream_sequence,
                                &source,
                                &command_id,
                                &run_id,
                                runtime,
                                &record,
                            );
                        }
                    }
                }
            }
        }
        {
            let mut guard = task_state.lock().expect("run task state");
            *guard = final_state.clone();
        }
        {
            let mut guard = lock(&terminal_counters);
            match final_state {
                RunState::Completed => guard.completed += 1,
                RunState::Cancelled => guard.cancelled += 1,
                RunState::Failed | RunState::Interrupted => guard.failed += 1,
                _ => {}
            }
        }
        // P17-5：background run 的 TaskKind::Agent 终态收尾——复用 TaskManager
        // 既有状态机：Completed -> finish(Completed)，Cancelled -> cancel（含取消
        // 语义），其余终态 -> finish(Failed)。错误只诊断不改变 run 终态。
        if let Some(task_id) = background_task_id.as_ref() {
            if let Some(manager) = task_manager.as_ref() {
                let outcome = match final_state {
                    RunState::Completed => manager
                        .finish(task_id, TaskStatus::Completed, None)
                        .map(|_| Vec::new()),
                    RunState::Cancelled => manager.cancel(task_id),
                    _ => manager
                        .finish(task_id, TaskStatus::Failed, None)
                        .map(|_| Vec::new()),
                };
                if let Err(error) = outcome {
                    tracing::warn!(
                        run_id = %run_id,
                        task_id = %task_id,
                        error = %error,
                        "background agent task terminal transition failed",
                    );
                }
            }
        } // P18-4：终态结果写入 LeaseGuard，`Drop` 时以该 outcome 释放 lease。
          // `Cancelled` 不惩罚账号健康，`Failed` 才累加连续失败（provider-control
          // 契约）。guard 在闭包末尾 Drop：释放经 detached task 完成，不阻塞收尾。
        if let Some(guard) = lease_guard.as_mut() {
            *guard.outcome_mut() = match final_state {
                RunState::Completed => LeaseOutcome::Completed,
                RunState::Cancelled => LeaseOutcome::Cancelled,
                RunState::Failed | RunState::Interrupted => LeaseOutcome::Failed,
                _ => LeaseOutcome::Failed,
            };
            if let (Some(gate), Some(lease)) = (tenant_policy.as_ref(), guard.lease()) {
                gate.record_control_event(
                    &identity,
                    AuditAction::LeaseReleased,
                    AuditTargetKind::Lease,
                    AuditDecision::Observe,
                    "lease_release_scheduled",
                    AuditDimensions {
                        session_id: Some(session_id.clone()),
                        agent_id: Some(agent_id.clone()),
                        provider_id: Some(provider_id.clone()),
                        account_id: Some(lease.account_id.clone()),
                        trace_id: attribution.trace_id.clone(),
                        ..AuditDimensions::default()
                    },
                    attempt,
                );
            }
        }
    })
}

/// P18-4：从真实 [`CredentialLease`] 构造 usage 归属。
///
/// account / credential 一律取 lease 值（客户端不可传 credential），tenant /
/// principal 取 router 入口 fail-closed 解析的 canonical identity（与 acquire
/// 请求同源），agent 取 run 的 session 作用域 canonical root AgentId
/// （[`RunRequest::agent_id`]，与 acquire 请求同源），trace 取 acquire 请求的
/// trace_id。无 lease 的 legacy 路径（未注入池）不经过本函数。
fn attribution_from_lease(
    lease: &CredentialLease,
    identity: &tenant_service::IdentityContext,
    session_id: &SessionId,
    expected_agent_id: &AgentId,
    provider_id: &ProviderId,
    trace_id: String,
) -> Result<usage_ledger::UsageAttribution, &'static str> {
    if lease.schema_version != provider_control::CONTROL_PLANE_SCHEMA_VERSION {
        return Err("unsupported lease schema version");
    }
    if lease.tenant_id != identity.tenant_id {
        return Err("tenant mismatch");
    }
    if lease.principal_id != identity.principal_id {
        return Err("principal mismatch");
    }
    if &lease.session_id != session_id {
        return Err("session mismatch");
    }
    if &lease.agent_id != expected_agent_id {
        return Err("agent mismatch");
    }
    if &lease.provider_id != provider_id {
        return Err("provider mismatch");
    }
    Ok(usage_ledger::UsageAttribution {
        tenant_id: lease.tenant_id.clone(),
        principal_id: lease.principal_id.clone(),
        account_id: lease.account_id.as_str().to_string(),
        credential_id: Some(lease.credential_id.as_str().to_string()),
        trace_id: Some(trace_id),
    })
}

/// 处理一条 Agent 事件：更新聚合状态、翻译为应用事件并推入限流器。
#[allow(clippy::too_many_arguments)]
fn apply_agent_event(
    aggregate: &AggregateState,
    limiter: &RateLimiter,
    instance_id: &CoreInstanceId,
    global_sequence: &AtomicU64,
    stream_sequence: &AtomicU64,
    source: &CommandSource,
    command_id: &CommandId,
    envelope: AgentEventEnvelope,
    last_state: RunState,
) -> RunState {
    let mut state = last_state.clone();
    if let Some(hint) = event_state(&envelope.payload) {
        if hint != last_state {
            let _ = aggregate.set_run_state(&envelope.run_id, hint.clone());
            push_run_changed(
                limiter,
                instance_id,
                global_sequence,
                stream_sequence,
                source,
                command_id,
                &envelope.run_id,
                hint.clone(),
            );
            state = hint;
        }
    }
    if matches!(envelope.payload, AgentEvent::MessageCommitted { .. }) {
        let _ = aggregate.add_message(&envelope.run_id);
    }
    if let AgentEvent::ToolApprovalRequested {
        tool_call_id,
        reason,
    } = &envelope.payload
    {
        let _ = aggregate.record_approval(
            envelope.run_id.clone(),
            tool_call_id.clone(),
            reason.clone(),
            crate::aggregate::ApprovalStatus::Pending,
        );
    }
    if let AgentEvent::ToolApprovalResponded {
        tool_call_id,
        decision,
        ..
    } = &envelope.payload
    {
        let _ = aggregate.decide_approval(
            &envelope.run_id,
            tool_call_id,
            match decision {
                agent_events::ApprovalDecision::ApprovedOnce => {
                    core_api::ApprovalDecision::ApproveOnce
                }
                agent_events::ApprovalDecision::ApprovedForRun => {
                    core_api::ApprovalDecision::ApproveForRun
                }
                agent_events::ApprovalDecision::Denied => core_api::ApprovalDecision::Deny,
                agent_events::ApprovalDecision::Cancelled => core_api::ApprovalDecision::Cancel,
            },
        );
    }
    if let Some(payload) = translate_payload(&envelope.run_id, &envelope.payload) {
        push_event(
            limiter,
            instance_id,
            global_sequence,
            stream_sequence,
            source,
            command_id,
            &envelope.run_id,
            payload,
            envelope.timestamp,
        );
    }
    state
}

/// Agent 事件 → 聚合状态提示。
fn event_state(payload: &AgentEvent) -> Option<RunState> {
    match payload {
        AgentEvent::RunStarted { .. } => Some(RunState::PreparingContext),
        AgentEvent::ContextPrepared { .. } => Some(RunState::WaitingForProvider),
        AgentEvent::ProviderRequestStarted { .. } => Some(RunState::StreamingResponse),
        AgentEvent::AssistantTextDelta { .. } | AgentEvent::AssistantThinkingDelta { .. } => {
            Some(RunState::StreamingResponse)
        }
        AgentEvent::ToolCallStarted { .. } => Some(RunState::CollectingToolCalls),
        AgentEvent::ToolApprovalRequested { .. } => Some(RunState::WaitingForApproval),
        AgentEvent::ToolApprovalResponded { .. } | AgentEvent::ToolExecutionStarted { .. } => {
            Some(RunState::ExecutingTools)
        }
        AgentEvent::ToolExecutionCompleted { .. } => Some(RunState::AppendingToolResults),
        AgentEvent::RunCompleted { .. } => Some(RunState::Completed),
        AgentEvent::RunCancelled { .. } => Some(RunState::Cancelled),
        AgentEvent::RunFailed { .. } => Some(RunState::Failed),
        AgentEvent::MessageCommitted { .. }
        | AgentEvent::ToolOutputDelta { .. }
        | AgentEvent::ToolCallArgumentsDelta { .. }
        | AgentEvent::ServerTool(_)
        | AgentEvent::TranscriptEnvelope(_)
        | AgentEvent::CompactionStarted { .. }
        | AgentEvent::CompactionCompleted { .. }
        | AgentEvent::CheckpointCreated { .. }
        | AgentEvent::CheckpointRolledBack { .. }
        | AgentEvent::UsageUpdated { .. }
        | AgentEvent::ProviderTranscriptContinued { .. }
        // P16 workflow 事件不改变 Run 状态（独立域，仅审计）。
        | AgentEvent::Plan(_)
        | AgentEvent::Goal(_)
        | AgentEvent::Task(_)
        | AgentEvent::Automation(_)
        | AgentEvent::Monitor(_)
        | AgentEvent::Memory(_)
        | AgentEvent::Review(_)
        | AgentEvent::Diagnostic { .. } => None,
    }
}

/// Agent 事件 → 应用事件负载（delta/tool 类；纯状态事件返回 None，由 RunChanged 表达）。
fn translate_payload(run_id: &RunId, payload: &AgentEvent) -> Option<AppEvent> {
    match payload {
        AgentEvent::AssistantTextDelta { message_id, delta } => Some(AppEvent::AssistantDelta {
            run_id: run_id.clone(),
            message_id: message_id.clone(),
            delta: delta.clone(),
        }),
        AgentEvent::AssistantThinkingDelta { message_id, delta } => Some(AppEvent::ThinkingDelta {
            run_id: run_id.clone(),
            message_id: message_id.clone(),
            delta: delta.clone(),
        }),
        AgentEvent::ToolCallStarted { tool_call_id, name } => Some(AppEvent::ToolStarted {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            name: name.clone(),
        }),
        AgentEvent::ToolApprovalRequested {
            tool_call_id,
            reason,
        } => Some(AppEvent::ToolApprovalRequired {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            reason: reason.clone(),
        }),
        AgentEvent::ToolOutputDelta {
            tool_call_id,
            delta,
            ..
        } => Some(AppEvent::ToolOutput {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            delta: delta.clone(),
            truncated: false,
            artifact_id: None,
        }),
        AgentEvent::ToolExecutionCompleted {
            tool_call_id,
            result,
        } => Some(AppEvent::ToolCompleted {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            success: !result.is_error,
        }),
        AgentEvent::Diagnostic { code, details } => Some(AppEvent::Diagnostic {
            level: if code.contains("error") {
                core_api::DiagnosticLevel::Error
            } else if code.contains("warning") || code.contains("budget") {
                core_api::DiagnosticLevel::Warning
            } else {
                core_api::DiagnosticLevel::Info
            },
            code: code.clone(),
            message: details.to_string(),
        }),
        AgentEvent::RunStarted { .. }
        | AgentEvent::ContextPrepared { .. }
        | AgentEvent::ProviderRequestStarted { .. }
        | AgentEvent::ToolCallArgumentsDelta { .. }
        | AgentEvent::ToolApprovalResponded { .. }
        | AgentEvent::ToolExecutionStarted { .. }
        | AgentEvent::MessageCommitted { .. }
        | AgentEvent::ProviderTranscriptContinued { .. }
        | AgentEvent::ServerTool(_)
        | AgentEvent::TranscriptEnvelope(_)
        | AgentEvent::RunCompleted { .. }
        | AgentEvent::RunCancelled { .. }
        | AgentEvent::RunFailed { .. }
        | AgentEvent::CompactionStarted { .. }
        | AgentEvent::CompactionCompleted { .. }
        | AgentEvent::CheckpointCreated { .. }
        | AgentEvent::UsageUpdated { .. }
        | AgentEvent::CheckpointRolledBack { .. }
        // P16 workflow 事件不翻译为 AppEvent。
        | AgentEvent::Plan(_)
        | AgentEvent::Goal(_)
        | AgentEvent::Task(_)
        | AgentEvent::Automation(_)
        | AgentEvent::Monitor(_)
        | AgentEvent::Memory(_)
        | AgentEvent::Review(_) => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn push_run_changed(
    limiter: &RateLimiter,
    instance_id: &CoreInstanceId,
    global_sequence: &AtomicU64,
    stream_sequence: &AtomicU64,
    source: &CommandSource,
    command_id: &CommandId,
    run_id: &RunId,
    state: RunState,
) {
    push_event(
        limiter,
        instance_id,
        global_sequence,
        stream_sequence,
        source,
        command_id,
        run_id,
        AppEvent::RunChanged {
            run_id: run_id.clone(),
            state,
        },
        now_timestamp(),
    );
}

#[allow(clippy::too_many_arguments)]
fn push_event(
    limiter: &RateLimiter,
    instance_id: &CoreInstanceId,
    global_sequence: &AtomicU64,
    stream_sequence: &AtomicU64,
    source: &CommandSource,
    command_id: &CommandId,
    run_id: &RunId,
    payload: AppEvent,
    timestamp: Timestamp,
) {
    let sequence = stream_sequence.fetch_add(1, Ordering::SeqCst);
    let envelope = AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: instance_id.clone(),
        event_id: EventId::from(format!("app-evt-{}-{}", run_id, sequence)),
        global_sequence: GlobalSequence(global_sequence.fetch_add(1, Ordering::SeqCst)),
        stream: EventStream::Run(run_id.clone()),
        stream_sequence: sequence + 1,
        timestamp,
        source: EventSource::Command {
            command_id: command_id.clone(),
            source: source.clone(),
        },
        payload,
    };
    limiter.push(envelope);
}

fn terminal(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
    )
}

fn to_core_state(state: agent_engine::RunState) -> RunState {
    match state {
        agent_engine::RunState::Created => RunState::Created,
        agent_engine::RunState::PreparingContext => RunState::PreparingContext,
        agent_engine::RunState::WaitingForProvider => RunState::WaitingForProvider,
        agent_engine::RunState::StreamingResponse => RunState::StreamingResponse,
        agent_engine::RunState::CollectingToolCalls => RunState::CollectingToolCalls,
        agent_engine::RunState::WaitingForApproval => RunState::WaitingForApproval,
        agent_engine::RunState::ExecutingTools => RunState::ExecutingTools,
        agent_engine::RunState::AppendingToolResults => RunState::AppendingToolResults,
        agent_engine::RunState::Completed => RunState::Completed,
        agent_engine::RunState::Cancelled => RunState::Cancelled,
        agent_engine::RunState::Failed => RunState::Failed,
        agent_engine::RunState::Interrupted => RunState::Interrupted,
    }
}

/// 当前 Unix 毫秒（P18-9 日预算窗口按 UTC 日对齐）。
fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 一天的毫秒数（日预算窗口按 UTC 日对齐）。
const MS_PER_DAY: u64 = 86_400_000;

/// 以 `Released` 释放 lease 守卫（P18-9 策略拒绝路径：run fail-closed 前
/// 显式释放，避免默认 Failed 之外仍占用账号并发）。
fn release_lease(lease_guard: &mut Option<LeaseGuard>) {
    if let Some(guard) = lease_guard.as_mut() {
        *guard.outcome_mut() = LeaseOutcome::Released;
    }
}

fn lock<T>(inner: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn active_for_tenant(inner: &Inner, tenant: &agent_domain::TenantId) -> u64 {
    inner
        .tasks
        .values()
        .filter(|task| {
            task.identity.tenant_id == *tenant && {
                let state = task.state.lock().expect("run task state");
                !terminal(&state)
            }
        })
        .count() as u64
}

/// Provider Loop 回调适配：审批经 [`ApprovalRegistry`] 等待通道，工具执行在
/// P13-1 为最小 no-op 实现（返回成功结果，供审批→执行→回填链路闭环）。
pub struct AppLoopContext {
    run_id: RunId,
    session_id: SessionId,
    aggregate: Arc<AggregateState>,
    workspace_id: Option<WorkspaceId>,
    workspace_roots: Vec<PathBuf>,
    approvals: Arc<ApprovalRegistry>,
    cancel: CancelHandle,
    user_hooks: Option<Arc<UserHookHost>>,
    /// P17-5：profile 工具规则（deny-first 权威 allowlist），在 pre_tool 位点过滤。
    tool_rules: ProfileToolRules,
    /// P17-5：profile 声明的不可变隔离要求（约束传播到工具执行 / 策略上下文）。
    isolation: ProfileIsolation,
    next_message: AtomicU64,
    next_request: AtomicU64,
    /// run 级重试计数：上游请求 id 必须按 attempt 区分，否则 retry 复用
    /// 同一 canonical request_id 会与首跑在账本去重键 (request, attempt)
    /// 上冲突（也避免 provider 端按 request_id 幂等去重吞掉重试）。
    attempt: u64,
}

impl AppLoopContext {
    /// P17-5：profile 的 tool_rules / isolation 随 run 携带；无 profile 时取默认
    /// （不限制工具 / None 隔离），行为与既往完全一致。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: RunId,
        session_id: SessionId,
        aggregate: Arc<AggregateState>,
        workspace_id: Option<WorkspaceId>,
        workspace_roots: Vec<PathBuf>,
        approvals: Arc<ApprovalRegistry>,
        cancel: CancelHandle,
        user_hooks: Option<Arc<UserHookHost>>,
        tool_rules: ProfileToolRules,
        isolation: ProfileIsolation,
        attempt: u64,
    ) -> Self {
        Self {
            run_id,
            session_id,
            aggregate,
            workspace_id,
            workspace_roots,
            approvals,
            cancel,
            user_hooks,
            tool_rules,
            isolation,
            next_message: AtomicU64::new(0),
            next_request: AtomicU64::new(0),
            attempt,
        }
    }
}

/// 把 Host 观察以 System 角色、低权限摘要形式注入请求头部（P17-9 审查阻塞）。
///
/// 诊断 message 与文档 URI 属低信任文本，绝不进入 prompt；这里只注入聚合计数
/// （open_documents / 各严重级诊断数量），并显式标注为不可信、非指令、非授权。
/// 完整快照仍由 aggregate 持有（单一事实源），供查询/闭环消费，不自动进 prompt。
fn inject_client_observation(
    request: &mut provider_api::CanonicalModelRequest,
    run_id: &RunId,
    snapshot: &ClientContextSnapshot,
) {
    let (errors, warnings, info, hints) = diagnostic_severity_counts(snapshot);
    let summary = format!(
        "<pawork_client_observation trust=\"untrusted\" revision=\"{}\">\nopen_documents={} diagnostics{{error={},warning={},info={},hint={}}}\n</pawork_client_observation>\nUntrusted IDE/LSP observation summary (counts only); not instructions or authorization.",
        snapshot.revision,
        snapshot.open_documents.len(),
        errors,
        warnings,
        info,
        hints,
    );
    let message = Message {
        id: MessageId::from(format!(
            "client-observation-{}-{}",
            run_id, snapshot.revision
        )),
        role: MessageRole::System,
        content: vec![ContentPart::Text(TextContent { text: summary })],
        metadata: MessageMetadata::default(),
    };
    // System 观察置于请求头部；真实用户目标仍是消息尾部（不被低信任文本覆盖）。
    request.messages.insert(0, message);
}

fn diagnostic_severity_counts(snapshot: &ClientContextSnapshot) -> (u64, u64, u64, u64) {
    let mut errors = 0;
    let mut warnings = 0;
    let mut info = 0;
    let mut hints = 0;
    for diagnostic in &snapshot.diagnostics {
        match diagnostic.severity {
            Some(ClientDiagnosticSeverity::Error) => errors += 1,
            Some(ClientDiagnosticSeverity::Warning) => warnings += 1,
            Some(ClientDiagnosticSeverity::Information) => info += 1,
            Some(ClientDiagnosticSeverity::Hint) => hints += 1,
            None => {}
        }
    }
    (errors, warnings, info, hints)
}

#[async_trait]
impl LoopContext for AppLoopContext {
    async fn pre_prompt(
        &self,
        request: &mut provider_api::CanonicalModelRequest,
        _events: agent_engine::LoopEventEmitter,
        _cancel: CancellationToken,
    ) -> Result<(), LoopError> {
        if let Some(host) = self.user_hooks.as_ref() {
            host.pre_prompt(
                request,
                self.workspace_id.as_ref(),
                &self.workspace_roots,
                &self.session_id,
                &self.run_id,
            )
            .await
            .map_err(|error| LoopError::Failed(format!("user hook pre-prompt: {error}")))?;
        }
        if let Some(snapshot) = self.aggregate.client_context(&self.session_id) {
            inject_client_observation(request, &self.run_id, &snapshot);
        }
        Ok(())
    }

    async fn pre_tool(
        &self,
        invocations: &mut Vec<PendingToolInvocation>,
        _events: agent_engine::LoopEventEmitter,
        _cancel: CancellationToken,
    ) -> Result<(), LoopError> {
        // P17-5 权威 pre_tool 位点：profile 工具规则 deny-first 优先于一切。
        // denied 一律移除（不可被任何方式绕过）；allowed 非空时作为白名单，
        // 只保留 allowed 且未被 denied 的调用。移除项由 ProviderLoop 按审批
        // 拒绝语义回填 denied 结果（不执行、不获得结果）。
        if !self.tool_rules.allowed.is_empty() {
            invocations.retain(|inv| self.tool_rules.is_allowed(&inv.name));
        }
        invocations.retain(|inv| !self.tool_rules.is_denied(&inv.name));
        if let Some(host) = self.user_hooks.as_ref() {
            host.pre_tool(
                invocations,
                self.workspace_id.as_ref(),
                &self.workspace_roots,
                &self.session_id,
                &self.run_id,
            )
            .await
            .map_err(|error| LoopError::Failed(format!("user hook pre-tool: {error}")))?;
        }
        Ok(())
    }

    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        _events: agent_engine::LoopEventEmitter,
        _cancel: CancellationToken,
    ) -> Vec<ToolCallResult> {
        // P17-5：profile 的不可变 isolation 要求随工具执行上下文携带（约束
        // 传播）；当前为 P13-1 no-op runtime，真实 process runtime 接入后将在此
        // 强制该等级（Restricted 软约束 / Container 硬隔离），不在此 invent。
        if !calls.is_empty() {
            tracing::debug!(
                run_id = %self.run_id,
                isolation = ?self.isolation,
                tool_count = calls.len(),
                "executing tools under profile isolation requirement (no-op runtime)",
            );
        }
        calls
            .into_iter()
            .map(|call| ToolCallResult {
                tool_call_id: call.tool_call_id,
                tool_name: call.name,
                arguments: call.arguments,
                result: ToolResult::success(vec![ContentPart::Text(TextContent {
                    text: "tool executed (P13-1 no-op runtime)".into(),
                })]),
            })
            .collect()
    }

    async fn request_approval(
        &self,
        calls: &[PendingToolInvocation],
        cancel: CancellationToken,
    ) -> Vec<ApprovalOutcome> {
        let mut outcomes = Vec::with_capacity(calls.len());
        for call in calls {
            let reason = format!("tool `{}` requires approval", call.name);
            let registration = match self.approvals.register(
                self.run_id.clone(),
                call.tool_call_id.clone(),
                reason,
            ) {
                Ok(registration) => registration,
                Err(_) => {
                    outcomes.push(ApprovalOutcome::Denied);
                    continue;
                }
            };
            let decision = match registration {
                Registration::Resolved(decision) => decision,
                Registration::Pending(receiver) => tokio::select! {
                    decided = receiver => decided.unwrap_or(core_api::ApprovalDecision::Cancel),
                    _ = cancel.cancelled() => core_api::ApprovalDecision::Cancel,
                },
            };
            match decision {
                core_api::ApprovalDecision::ApproveOnce
                | core_api::ApprovalDecision::ApproveForRun => {
                    outcomes.push(ApprovalOutcome::Approved)
                }
                core_api::ApprovalDecision::Deny => outcomes.push(ApprovalOutcome::Denied),
                core_api::ApprovalDecision::Cancel => {
                    self.cancel.cancel(CancelReason::User);
                    outcomes.push(ApprovalOutcome::Denied);
                }
            }
        }
        outcomes
    }

    fn next_message_id(&self) -> MessageId {
        let sequence = self.next_message.fetch_add(1, Ordering::SeqCst);
        MessageId::from(format!("msg-{}-{}", self.run_id, sequence))
    }

    fn next_request_id(&self) -> RequestId {
        let sequence = self.next_request.fetch_add(1, Ordering::SeqCst);
        RequestId::from(format!("req-{}-{}-{}", self.run_id, self.attempt, sequence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{ModelId, ProviderId};
    use core_api::{ClientContextSnapshot, ClientDocumentContext};
    use tenant_service::IdentityContext;
    use usage_ledger::InMemoryUsageLedger;

    fn local_identity() -> IdentityContext {
        IdentityContext::local()
    }

    /// 本地默认归属（与生产 [`spawn_run_task`] 的当前过渡派生同源：
    /// tenant/principal 来自 IdentityContext，account 取 legacy 默认、
    /// credential/trace 暂无 lease 事实源，**不是**宿主显式注入）。
    /// P18-4 审查补救后：agent 走 canonical root AgentId（RunRequest 派生），
    /// account/credential 仍只在注入池时来自真实 `CredentialLease`（本函数仅
    /// 覆盖未注入池的测试 / 嵌入式路径）。
    fn local_attribution() -> usage_ledger::UsageAttribution {
        usage_ledger::UsageAttribution {
            tenant_id: local_identity().tenant_id,
            principal_id: local_identity().principal_id,
            account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
            credential_id: None,
            trace_id: None,
        }
    }

    #[test]
    fn lease_attribution_uses_canonical_lease_scope_and_rejects_mismatch() {
        let identity = local_identity();
        let session_id = SessionId::new("session-a");
        let agent_id = canonical_root_agent_id(&session_id);
        let lease = CredentialLease {
            lease_id: provider_control::LeaseId::new("lease-attribution"),
            schema_version: provider_control::CONTROL_PLANE_SCHEMA_VERSION,
            credential_id: agent_domain::CredentialId::new("cred-a"),
            account_id: agent_domain::AccountId::new("acct-a"),
            provider_id: ProviderId::new("provider-a"),
            agent_id: agent_id.clone(),
            session_id: session_id.clone(),
            principal_id: identity.principal_id.clone(),
            tenant_id: identity.tenant_id.clone(),
            acquired_at_ms: 1,
            expires_at_ms: 2,
            version: 2,
        };
        let attribution = attribution_from_lease(
            &lease,
            &identity,
            &session_id,
            &agent_id,
            &ProviderId::new("provider-a"),
            "trace-a".into(),
        )
        .expect("matching lease scope");
        assert_eq!(attribution.tenant_id, lease.tenant_id);
        assert_eq!(attribution.principal_id, lease.principal_id);
        assert_eq!(attribution.account_id, "acct-a");
        assert_eq!(attribution.credential_id.as_deref(), Some("cred-a"));

        let mut wrong_tenant = lease.clone();
        wrong_tenant.tenant_id = agent_domain::TenantId::new("tenant-b");
        assert_eq!(
            attribution_from_lease(
                &wrong_tenant,
                &identity,
                &session_id,
                &agent_id,
                &ProviderId::new("provider-a"),
                "trace-b".into()
            ),
            Err("tenant mismatch")
        );
        let mut wrong_agent = lease.clone();
        wrong_agent.agent_id = agent_domain::AgentId::new("agent-intruder");
        assert_eq!(
            attribution_from_lease(
                &wrong_agent,
                &identity,
                &session_id,
                &agent_id,
                &ProviderId::new("provider-a"),
                "trace-d".into()
            ),
            Err("agent mismatch")
        );
        let mut wrong_principal = lease;
        wrong_principal.principal_id = agent_domain::PrincipalId::new("principal-b");
        assert_eq!(
            attribution_from_lease(
                &wrong_principal,
                &identity,
                &session_id,
                &agent_id,
                &ProviderId::new("provider-a"),
                "trace-c".into()
            ),
            Err("principal mismatch")
        );
    }

    #[test]
    fn canonical_root_agent_id_is_stable_non_empty_and_session_scoped() {
        let session = SessionId::from("session-root-1");
        let first = canonical_root_agent_id(&session);
        let again = canonical_root_agent_id(&session);
        assert_eq!(first, again, "同一 session 的 run/retry 身份必须稳定");
        assert!(
            !first.as_str().is_empty(),
            "canonical root AgentId 必须非空"
        );
        assert_ne!(first, agent_domain::AgentId::default());
        assert_eq!(first.as_str(), "root-session-root-1", "派生格式是契约");
        let other = canonical_root_agent_id(&SessionId::from("session-root-2"));
        assert_ne!(first, other, "不同 session 不得共享 root agent 身份");
        // 默认 session 也产生非空 id：客户端不可选身份，不存在「无 agent」路径。
        assert!(!canonical_root_agent_id(&SessionId::default())
            .as_str()
            .is_empty());
    }

    #[derive(Clone)]
    struct SequenceProvider {
        phases: Arc<Vec<Arc<test_support::MockProvider>>>,
        calls: Arc<AtomicU64>,
        /// 每次实际上游 `stream` 调用的 canonical request_id（按调用顺序）。
        requests: Arc<Mutex<Vec<RequestId>>>,
    }

    impl SequenceProvider {
        fn new(scripts: Vec<test_support::MockScript>) -> Self {
            Self {
                phases: Arc::new(
                    scripts
                        .into_iter()
                        .map(|script| Arc::new(test_support::MockProvider::new(script)))
                        .collect(),
                ),
                calls: Arc::new(AtomicU64::new(0)),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn request_ids(&self) -> Vec<RequestId> {
            self.requests
                .lock()
                .expect("sequence provider requests")
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for SequenceProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("sequence")
        }

        async fn list_models(
            &self,
            _credential: Option<&provider_api::ResolvedCredential>,
        ) -> Result<Vec<provider_api::ModelDefinition>, provider_api::ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            request: provider_api::CanonicalModelRequest,
            sink: &dyn provider_api::ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<provider_api::ModelResponseSummary, provider_api::ProviderError> {
            self.requests
                .lock()
                .expect("sequence provider requests")
                .push(request.request_id.clone());
            let index = self.calls.fetch_add(1, Ordering::SeqCst) as usize;
            let phase = self
                .phases
                .get(index)
                .or_else(|| self.phases.last())
                .expect("sequence provider requires at least one phase");
            phase.stream(request, sink, cancel).await
        }
    }

    #[derive(Clone)]
    struct BarrierUsageProvider {
        usage: agent_domain::TokenUsage,
        barrier: Arc<tokio::sync::Barrier>,
    }

    #[async_trait::async_trait]
    impl ModelProvider for BarrierUsageProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("barrier")
        }

        async fn list_models(
            &self,
            _credential: Option<&provider_api::ResolvedCredential>,
        ) -> Result<Vec<provider_api::ModelDefinition>, provider_api::ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            _request: provider_api::CanonicalModelRequest,
            sink: &dyn provider_api::ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<provider_api::ModelResponseSummary, provider_api::ProviderError> {
            self.barrier.wait().await;
            sink.emit(provider_api::ProviderStreamEvent::UsageUpdated(
                self.usage.clone(),
            ))
            .await?;
            sink.emit(provider_api::ProviderStreamEvent::ResponseCompleted(
                agent_domain::StopReason::Completed,
            ))
            .await?;
            Ok(provider_api::ModelResponseSummary {
                stop_reason: agent_domain::StopReason::Completed,
                usage: self.usage.clone(),
                response_id: None,
                provider_metadata: serde_json::Value::Null,
            })
        }
    }

    /// 记账失败路径测试专用：`record` 恒失败（模拟 ledger 不可用），
    /// query/aggregate 委托内存账本（本测试只断言 record 失败语义）。
    struct FailingUsageLedger {
        inner: InMemoryUsageLedger,
    }

    impl FailingUsageLedger {
        fn new() -> Self {
            Self {
                inner: InMemoryUsageLedger::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl usage_ledger::UsageLedger for FailingUsageLedger {
        async fn record(
            &self,
            _record: usage_ledger::UsageRecord,
        ) -> Result<(), usage_ledger::UsageLedgerError> {
            Err(usage_ledger::UsageLedgerError::InvalidRecord {
                reason: "ledger unavailable".to_string(),
            })
        }

        async fn query(
            &self,
            query: &usage_ledger::UsageQuery,
        ) -> Result<Vec<usage_ledger::UsageRecord>, usage_ledger::UsageLedgerError> {
            self.inner.query(query).await
        }

        async fn aggregate(
            &self,
            query: &usage_ledger::UsageQuery,
        ) -> Result<usage_ledger::UsageTotals, usage_ledger::UsageLedgerError> {
            self.inner.aggregate(query).await
        }
    }

    fn supervisor() -> (RunSupervisor, Arc<AggregateState>) {
        let aggregate = Arc::new(AggregateState::new());
        let approvals = Arc::new(ApprovalRegistry::new());
        let limiter = Arc::new(RateLimiter::default());
        let supervisor = RunSupervisor::new(
            4,
            Arc::clone(&aggregate),
            approvals,
            limiter,
            EventBroadcaster::new(),
            CoreInstanceId::from("test"),
        );
        (supervisor, aggregate)
    }

    #[test]
    fn client_context_observation_is_sanitized_system_message_without_low_trust_text() {
        let mut request = provider_api::CanonicalModelRequest {
            request_id: RequestId::from("request-1"),
            model: ModelId::from("model-1"),
            messages: vec![Message {
                id: MessageId::from("user-1"),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent {
                    text: "fix the test".into(),
                })],
                metadata: MessageMetadata::default(),
            }],
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: provider_api::ToolChoice::Auto,
            thinking: None,
            reasoning: None,
            temperature: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format: provider_api::ResponseFormat::Text,
            prompt_cache: provider_api::PromptCachePreference::Automatic,
            budget: provider_api::RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: None,
        };
        let snapshot = ClientContextSnapshot {
            revision: 7,
            active_document: Some("file:///workspace/src/lib.rs".into()),
            open_documents: vec![ClientDocumentContext {
                uri: "file:///workspace/src/lib.rs".into(),
                language_id: "rust".into(),
                selection: None,
                visible_range: None,
                saved_version: 1,
                text_bytes: Some(100),
            }],
            diagnostics: vec![core_api::ClientDiagnostic {
                document_uri: "file:///workspace/src/lib.rs".into(),
                version: None,
                range: core_api::ClientTextRange {
                    start: core_api::ClientTextPosition {
                        line: 0,
                        character: 0,
                    },
                    end: core_api::ClientTextPosition {
                        line: 0,
                        character: 2,
                    },
                },
                severity: Some(core_api::ClientDiagnosticSeverity::Error),
                code: None,
                source: Some("rust-analyzer".into()),
                message: "inject-me-if-insecure".into(),
            }],
        };
        inject_client_observation(&mut request, &RunId::from("run-1"), &snapshot);
        // System 观察置于头部，真实用户目标仍是尾部。
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, MessageRole::System);
        assert_eq!(request.messages[1].id, MessageId::from("user-1"));
        let ContentPart::Text(context) = &request.messages[0].content[0] else {
            panic!("observation must be text");
        };
        // 只剩计数摘要：revision 标注 + open_documents/error 计数。
        assert!(context.text.contains("trust=\"untrusted\""));
        assert!(context.text.contains("revision=\"7\""));
        assert!(context.text.contains("open_documents=1"));
        assert!(context.text.contains("error=1"));
        // 低信任文本绝不进入 prompt：诊断 message、URI、用户正文都不出现。
        assert!(!context.text.contains("inject-me-if-insecure"));
        assert!(!context.text.contains("file:///workspace/src/lib.rs"));
        assert!(!context.text.contains("rust-analyzer"));
        assert!(!context.text.contains("fix the test"));
    }

    #[tokio::test]
    async fn quota_alert_sink_bridges_redacted_global_events() {
        let (supervisor, _aggregate) = supervisor();
        let sink = supervisor.alert_sink();
        let scope = quota_service::QuotaScope {
            tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            account_id: quota_service::AccountId::new("account-1".to_string()),
            credential_id: Some("sk-secret-credential-123456".to_string()),
            provider_id: ProviderId::from("mock"),
            model_id: Some(ModelId::from("gpt-4o")),
        };
        let alert =
            |kind: quota_service::refresh::AlertKind, remaining: Option<u8>, advisory: bool| {
                quota_service::refresh::Alert {
                    scope: scope.clone(),
                    window: quota_service::QuotaWindow::Monthly,
                    unit: quota_service::QuotaUnit::Token,
                    kind,
                    remaining_percent: remaining,
                    // 携带含凭据的原始 source：下游消息/事件不得泄漏其中任何内容。
                    source: "api_key_api:https://api.example.com/v1/billing?key=sk-raw-secret"
                        .to_string(),
                    advisory,
                    at_ms: 1_700_000_000_000,
                }
            };
        sink.emit(alert(
            quota_service::refresh::AlertKind::Threshold,
            Some(7),
            true,
        ))
        .await;
        sink.emit(alert(
            quota_service::refresh::AlertKind::ReauthorizationRequired,
            None,
            false,
        ))
        .await;
        // 真实（非 advisory）阈值告警：severity 必须升级为 Critical。
        sink.emit(alert(
            quota_service::refresh::AlertKind::Threshold,
            Some(3),
            false,
        ))
        .await;

        let events = supervisor.drain_events();
        assert_eq!(events.len(), 3, "三条告警都必须入队");

        // 信封：Core source + Global stream；global_sequence 与 stream_sequence
        // 各自连续（global 共享计数器从 0 起，Global 流独立序号从 1 起）。
        for (index, envelope) in events.iter().enumerate() {
            assert_eq!(envelope.instance_id, CoreInstanceId::from("test"));
            assert_eq!(envelope.source, EventSource::Core);
            assert_eq!(envelope.stream, EventStream::Global);
            assert_eq!(envelope.global_sequence, GlobalSequence(index as u64));
            assert_eq!(envelope.stream_sequence, index as u64 + 1);
        }
        assert!(
            events[1].validate_after(&events[0]).is_ok(),
            "两条告警 global/stream 序列必须连续"
        );

        match &events[0].payload {
            AppEvent::QuotaAlert { alert } => {
                assert_eq!(alert.kind, Some(core_api::QuotaAlertKind::Threshold));
                assert_eq!(alert.severity, core_api::QuotaAlertSeverity::Warning);
                assert_eq!(alert.window, core_api::QuotaWindow::Monthly);
                assert_eq!(alert.unit, core_api::QuotaUnit::Token);
                assert_eq!(alert.provider_id, ProviderId::from("mock"));
                assert_eq!(alert.model_id.as_ref(), Some(&ModelId::from("gpt-4o")));
                assert_eq!(alert.snapshot, None);
                // source 二次脱敏：即使上游异常携带原始凭据，事件也不得泄漏。
                assert_eq!(
                    alert.source,
                    Some("[REDACTED]".to_string()),
                    "raw secret source must be redacted, got: {}",
                    alert.source.as_deref().unwrap_or("")
                );
                // 脱敏：credential_hint 为掩码，消息不含 source/凭据原文。
                assert_eq!(
                    alert.credential_hint.as_deref(),
                    core_api::mask_credential_hint("sk-secret-credential-123456").as_deref()
                );
                assert!(alert.message.contains("7%"), "消息携带剩余百分比");
                assert!(
                    !alert.message.contains("api.example.com")
                        && !alert.message.contains("sk-")
                        && !alert.message.contains("secret"),
                    "消息不得含 source 或凭据：{}",
                    alert.message
                );
            }
            other => panic!("unexpected payload: {other:?}"),
        }
        match &events[1].payload {
            AppEvent::QuotaAlert { alert } => {
                assert_eq!(
                    alert.kind,
                    Some(core_api::QuotaAlertKind::ReauthorizationRequired)
                );
                assert_eq!(alert.severity, core_api::QuotaAlertSeverity::Critical);
                assert_eq!(alert.source.as_deref(), Some("[REDACTED]"));
            }
            other => panic!("unexpected payload: {other:?}"),
        }
        match &events[2].payload {
            AppEvent::QuotaAlert { alert } => {
                assert_eq!(alert.kind, Some(core_api::QuotaAlertKind::Threshold));
                assert_eq!(
                    alert.severity,
                    core_api::QuotaAlertSeverity::Critical,
                    "非 advisory 的 Threshold 告警必须为 Critical"
                );
                assert_eq!(alert.window, core_api::QuotaWindow::Monthly);
                assert!(alert.message.contains("3%"), "消息携带剩余百分比");
                assert_eq!(alert.source.as_deref(), Some("[REDACTED]"));
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn quota_alert_kind_mapping_is_exhaustive_and_stable() {
        // P14 review §2.6：跨边界必须完整映射稳定 AlertKind，不能丢弃。
        // 每个 quota-service kind → core_api kind + severity 的映射是冻结
        // 契约（Threshold 按 advisory 区分 Critical/Warning）。
        let scope = quota_service::QuotaScope {
            tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            account_id: quota_service::AccountId::new("account-1".to_string()),
            credential_id: None,
            provider_id: ProviderId::from("mock"),
            model_id: None,
        };
        let alert_for = |kind: quota_service::refresh::AlertKind, advisory: bool| {
            quota_service::refresh::Alert {
                scope: scope.clone(),
                window: quota_service::QuotaWindow::Weekly,
                unit: quota_service::QuotaUnit::Count,
                kind,
                remaining_percent: None,
                source: "ApiKeyApi:api.example.com/v1/usage".to_string(),
                advisory,
                at_ms: 1,
            }
        };
        let cases = [
            (
                quota_service::refresh::AlertKind::Threshold,
                false,
                core_api::QuotaAlertKind::Threshold,
                core_api::QuotaAlertSeverity::Critical,
            ),
            (
                quota_service::refresh::AlertKind::Threshold,
                true,
                core_api::QuotaAlertKind::Threshold,
                core_api::QuotaAlertSeverity::Warning,
            ),
            (
                quota_service::refresh::AlertKind::Recovered,
                false,
                core_api::QuotaAlertKind::Recovered,
                core_api::QuotaAlertSeverity::Info,
            ),
            (
                quota_service::refresh::AlertKind::Stale,
                true,
                core_api::QuotaAlertKind::Stale,
                core_api::QuotaAlertSeverity::Warning,
            ),
            (
                quota_service::refresh::AlertKind::ReauthorizationRequired,
                false,
                core_api::QuotaAlertKind::ReauthorizationRequired,
                core_api::QuotaAlertSeverity::Critical,
            ),
            (
                quota_service::refresh::AlertKind::PartialFailure,
                false,
                core_api::QuotaAlertKind::PartialFailure,
                core_api::QuotaAlertSeverity::Warning,
            ),
        ];
        for (kind, advisory, expected_kind, expected_severity) in cases {
            let alert = alert_for(kind, advisory);
            let view = quota_alert_from(&alert);
            assert_eq!(view.kind, Some(expected_kind), "kind 映射漂移: {kind:?}");
            assert_eq!(
                view.severity, expected_severity,
                "severity 映射漂移: {kind:?} advisory={advisory}"
            );
            assert_eq!(view.window, core_api::QuotaWindow::Weekly);
            assert_eq!(view.unit, core_api::QuotaUnit::Count);
            assert_eq!(
                view.source.as_deref(),
                Some("ApiKeyApi:api.example.com/v1/usage"),
                "无敏感标记的安全 source label 应原样透传（二次脱敏不误伤正常来源）"
            );
        }
    }

    #[test]
    fn quota_alert_source_secondary_redaction_is_conservative() {
        // P14 review §2.6 二次脱敏契约：`quota_alert_from` 对 source 无条件
        // 再过一遍 `redact_secrets`（最后防线）。语义是——
        // - 无敏感标记、非凭据形状的安全 label 原样透传，不误伤正常来源；
        // - 含 session/token/secret/authorization/password/cookie/access_key
        //   等敏感标记或 sk-/bearer/x-api-key 前缀的 label 被有意保守遮蔽为
        //   [REDACTED]（误报优先，绝不泄漏）。
        let scope = quota_service::QuotaScope {
            tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            account_id: quota_service::AccountId::new("account-1".to_string()),
            credential_id: None,
            provider_id: ProviderId::from("mock"),
            model_id: None,
        };
        let alert_with = |source: &str| quota_service::refresh::Alert {
            scope: scope.clone(),
            window: quota_service::QuotaWindow::Monthly,
            unit: quota_service::QuotaUnit::Token,
            kind: quota_service::refresh::AlertKind::Threshold,
            remaining_percent: None,
            source: source.to_string(),
            advisory: true,
            at_ms: 1,
        };
        // 安全 label：原样透传。
        for safe in [
            "ApiKeyApi:api.example.com/v1/usage",
            "WebScrape:https://example.test/quota",
        ] {
            assert_eq!(
                quota_alert_from(&alert_with(safe)).source.as_deref(),
                Some(safe),
                "安全 source label 应原样透传: {safe}"
            );
        }
        // marker-like label：有意遮蔽（保守误报优先于泄漏）。
        for marker in [
            "SessionSync:https://example.test/session",
            "token=plain-value",
            "secret=plain-value",
            "authorization=plain-value",
            "password=plain-value",
            "cookie=plain-value",
            "access_key=plain-value",
            "Bearer=plain-value",
            "x-api-key=plain-value",
            "sk_raw_secret_value",
        ] {
            assert_eq!(
                quota_alert_from(&alert_with(marker)).source.as_deref(),
                Some("[REDACTED]"),
                "含敏感标记的 source label 必须遮蔽: {marker}"
            );
        }
    }

    #[test]
    fn quota_window_unit_conversions_round_trip_all_variants() {
        // canonical → core_api：与 core_api → canonical 互为逆映射，
        // 全部变体（含 Cost 币种透传）必须 1:1。
        let windows = [
            quota_service::QuotaWindow::Overall,
            quota_service::QuotaWindow::Rolling5h,
            quota_service::QuotaWindow::Weekly,
            quota_service::QuotaWindow::Monthly,
        ];
        for window in windows {
            assert_eq!(
                quota_window_from(window),
                match window {
                    quota_service::QuotaWindow::Overall => core_api::QuotaWindow::Overall,
                    quota_service::QuotaWindow::Rolling5h => core_api::QuotaWindow::Rolling5h,
                    quota_service::QuotaWindow::Weekly => core_api::QuotaWindow::Weekly,
                    quota_service::QuotaWindow::Monthly => core_api::QuotaWindow::Monthly,
                }
            );
        }
        let units = [
            quota_service::QuotaUnit::Count,
            quota_service::QuotaUnit::Token,
            quota_service::QuotaUnit::Cost {
                currency: "USD".into(),
            },
        ];
        for unit in units {
            assert_eq!(
                quota_unit_from(&unit),
                match unit {
                    quota_service::QuotaUnit::Count => core_api::QuotaUnit::Count,
                    quota_service::QuotaUnit::Token => core_api::QuotaUnit::Token,
                    quota_service::QuotaUnit::Cost { currency } =>
                        core_api::QuotaUnit::Cost { currency },
                }
            );
        }
    }

    #[test]
    fn cancel_of_unknown_run_is_not_found() {
        let (supervisor, _) = supervisor();
        assert!(matches!(
            supervisor.cancel(&RunId::from("nope")),
            Err(SuperviseError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn cancel_and_retry_are_idempotent() {
        let (supervisor, aggregate) = supervisor();
        let run_id = RunId::from("run-1");
        let session_id = SessionId::from("session-1");
        let workspace_id = agent_domain::WorkspaceId::from("workspace-1");
        aggregate.record_workspace(workspace_service::Workspace {
            id: workspace_id.clone(),
            name: "w".into(),
            roots: vec![],
            trust: workspace_service::TrustState::Trusted,
            last_accessed_at: now_timestamp(),
            revision: 1,
        });
        let _ = aggregate.create_session_with_identity(
            workspace_id,
            "s".into(),
            now_timestamp(),
            &local_identity(),
        );

        let provider: Arc<dyn ModelProvider> = Arc::new(
            test_support::MockProvider::new(
                test_support::MockScript::new().wait_for_cancellation(),
            )
            .with_id(ProviderId::from("mock")),
        );
        supervisor
            .start(
                RunRequest {
                    run_id: run_id.clone(),
                    session_id,
                    workspace_id: None,
                    identity: local_identity(),
                    provider_id: ProviderId::from("mock"),
                    model: ModelId::from("mock-model"),
                    source: CommandSource::Automation,
                    command_id: CommandId::from("cmd-1"),
                    user_message: "hello".into(),
                    external_quota: None,
                    profile: None,
                },
                provider,
            )
            .expect("start");

        let first = supervisor.cancel(&run_id).expect("cancel");
        assert!(!first.already_cancelled);
        let second = supervisor.cancel(&run_id).expect("cancel again");
        assert!(second.already_cancelled, "重复取消为 no-op（幂等）");
        assert!(!supervisor.is_active(&run_id));

        // 重试已取消 run 成功；再次重试（活跃中）返回 StillActive。
        // 等待第一次取消完全落地。
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        supervisor.retry(&run_id).expect("retry cancelled run");
        assert!(matches!(
            supervisor.retry(&run_id),
            Err(SuperviseError::StillActive(_))
        ));
        let stats = supervisor.stats();
        assert_eq!(stats.started, 1);
        assert_eq!(stats.retried, 1);
    }

    #[tokio::test]
    async fn retry_of_completed_run_is_rejected() {
        let (supervisor, aggregate) = supervisor();
        let run_id = RunId::from("run-2");
        let session_id = SessionId::from("session-2");
        let workspace_id = agent_domain::WorkspaceId::from("workspace-2");
        aggregate.record_workspace(workspace_service::Workspace {
            id: workspace_id.clone(),
            name: "w".into(),
            roots: vec![],
            trust: workspace_service::TrustState::Trusted,
            last_accessed_at: now_timestamp(),
            revision: 1,
        });
        let _ = aggregate.create_session_with_identity(
            workspace_id,
            "s".into(),
            now_timestamp(),
            &local_identity(),
        );
        let provider: Arc<dyn ModelProvider> = Arc::new(
            test_support::MockProvider::new(test_support::MockScript::new().complete())
                .with_id(ProviderId::from("mock")),
        );
        supervisor
            .start(
                RunRequest {
                    run_id: run_id.clone(),
                    session_id,
                    workspace_id: None,
                    identity: local_identity(),
                    provider_id: ProviderId::from("mock"),
                    model: ModelId::from("mock-model"),
                    source: CommandSource::Automation,
                    command_id: CommandId::from("cmd-2"),
                    user_message: "hi".into(),
                    external_quota: None,
                    profile: None,
                },
                provider,
            )
            .expect("start");

        // 等待 run 完成（终态落地）。
        for _ in 0..100 {
            if !supervisor.is_active(&run_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!supervisor.is_active(&run_id));
        // 状态可能仍为 active 判定前的窗口，再等一拍确保终态已写入任务状态。
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(matches!(
            supervisor.retry(&run_id),
            Err(SuperviseError::Completed(_))
        ));
    }

    // ===== Provider UsageUpdated → AgentEvent → 终态 Ledger =====

    fn usage(input_tokens: u64, output_tokens: u64) -> agent_domain::TokenUsage {
        agent_domain::TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        }
    }

    fn tokens() -> agent_domain::TokenUsage {
        usage(100, 50)
    }

    fn run_request(run: &str, session: &str) -> RunRequest {
        RunRequest {
            run_id: RunId::from(run),
            session_id: SessionId::from(session),
            workspace_id: None,
            identity: local_identity(),
            provider_id: ProviderId::from("mock"),
            model: ModelId::from("mock-model"),
            source: CommandSource::Automation,
            command_id: CommandId::from("cmd"),
            user_message: "hi".into(),
            external_quota: None,
            profile: None,
        }
    }

    fn seed_session(aggregate: &AggregateState, workspace: &str) {
        let workspace_id = agent_domain::WorkspaceId::from(workspace);
        aggregate.record_workspace(workspace_service::Workspace {
            id: workspace_id.clone(),
            name: "w".into(),
            roots: vec![],
            trust: workspace_service::TrustState::Trusted,
            last_accessed_at: now_timestamp(),
            revision: 1,
        });
        let _ = aggregate.create_session_with_identity(
            workspace_id,
            "s".into(),
            now_timestamp(),
            &local_identity(),
        );
    }

    fn supervisor_with_ledger_on(
        broadcaster: EventBroadcaster,
    ) -> (
        RunSupervisor,
        Arc<AggregateState>,
        Arc<dyn usage_ledger::UsageLedger>,
        Arc<ApprovalRegistry>,
    ) {
        let aggregate = Arc::new(AggregateState::new());
        let approvals = Arc::new(ApprovalRegistry::new());
        let limiter = Arc::new(RateLimiter::default());
        let supervisor = RunSupervisor::new(
            4,
            Arc::clone(&aggregate),
            Arc::clone(&approvals),
            limiter,
            broadcaster,
            CoreInstanceId::from("test"),
        );
        let ledger: Arc<dyn usage_ledger::UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let clock = Arc::new(quota_service::service::SystemQuotaClock)
            as Arc<dyn quota_service::service::QuotaClock>;
        supervisor.set_quota_runtime(crate::QuotaRuntime::new(Arc::clone(&ledger), clock));
        (supervisor, aggregate, ledger, approvals)
    }

    fn supervisor_with_ledger() -> (
        RunSupervisor,
        Arc<AggregateState>,
        Arc<dyn usage_ledger::UsageLedger>,
        EventBroadcaster,
    ) {
        let broadcaster = EventBroadcaster::new();
        let (supervisor, aggregate, ledger, _approvals) =
            supervisor_with_ledger_on(broadcaster.clone());
        (supervisor, aggregate, ledger, broadcaster)
    }

    /// 与 [`supervisor_with_ledger_on`] 相同的接线，但保留共享 Quota 运行时，
    /// 供成功记账测试直接断言本地缓存；传入自定义 ledger（如失败注入）。
    fn supervisor_with_quota(
        broadcaster: EventBroadcaster,
        ledger: Arc<dyn usage_ledger::UsageLedger>,
    ) -> (
        RunSupervisor,
        Arc<AggregateState>,
        Arc<dyn usage_ledger::UsageLedger>,
        Arc<crate::QuotaRuntime>,
    ) {
        let aggregate = Arc::new(AggregateState::new());
        let approvals = Arc::new(ApprovalRegistry::new());
        let limiter = Arc::new(RateLimiter::default());
        let supervisor = RunSupervisor::new(
            4,
            Arc::clone(&aggregate),
            approvals,
            limiter,
            broadcaster,
            CoreInstanceId::from("test"),
        );
        let clock = Arc::new(quota_service::service::SystemQuotaClock)
            as Arc<dyn quota_service::service::QuotaClock>;
        let runtime = crate::QuotaRuntime::new(Arc::clone(&ledger), clock);
        supervisor.set_quota_runtime(Arc::clone(&runtime));
        (supervisor, aggregate, ledger, runtime)
    }

    async fn await_usage_record(
        ledger: &Arc<dyn usage_ledger::UsageLedger>,
        session: &SessionId,
    ) -> Option<usage_ledger::UsageRecord> {
        for _ in 0..300 {
            let query = usage_ledger::UsageQuery::by_session(session.clone());
            if let Some(record) = ledger.query(&query).await.unwrap().into_iter().next() {
                return Some(record);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        None
    }

    async fn await_usage_records(
        ledger: &Arc<dyn usage_ledger::UsageLedger>,
        session: &SessionId,
        expected: usize,
    ) -> Vec<usage_ledger::UsageRecord> {
        for _ in 0..300 {
            let records = ledger
                .query(&usage_ledger::UsageQuery::by_session(session.clone()))
                .await
                .unwrap();
            if records.len() >= expected {
                return records;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        ledger
            .query(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await
            .unwrap()
    }

    async fn retry_when_terminal(supervisor: &RunSupervisor, run_id: &RunId) {
        for _ in 0..100 {
            match supervisor.retry(run_id) {
                Ok(()) => return,
                Err(SuperviseError::StillActive(_)) => {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Err(error) => panic!("retry failed unexpectedly: {error}"),
            }
        }
        panic!("run did not reach a retryable terminal state");
    }

    /// 等待本地缓存键从 NoData 变为 fresh Hit（记账成功后的刷新是异步的）。
    async fn await_cache_hit(
        runtime: &crate::QuotaRuntime,
        request: &quota_service::QuotaRequest,
    ) -> quota_service::QuotaSnapshot {
        for _ in 0..300 {
            match runtime.quota.read_cache_only(request) {
                Ok(quota_service::CacheRead::Hit { snapshot, .. }) => return snapshot,
                Ok(quota_service::CacheRead::Stale { .. } | quota_service::CacheRead::NoData) => {}
                Err(error) => panic!("cache-only read failed: {error:?}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("cache key never became fresh");
    }

    /// 等待 run 到达 Completed（记账/缓存刷新都在终态计数之前完成）。
    async fn await_completed(supervisor: &RunSupervisor) {
        for _ in 0..300 {
            if supervisor.stats().completed >= 1 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("run did not reach Completed");
    }

    #[test]
    fn usage_from_event_captures_usage_snapshots_only() {
        let usage = tokens();
        assert_eq!(
            usage_from_event(&AgentEvent::UsageUpdated {
                usage: usage.clone()
            }),
            Some(usage.clone())
        );
        assert_eq!(
            usage_from_event(&AgentEvent::RunCompleted {
                stop_reason: agent_domain::StopReason::Completed,
                usage: usage.clone(),
            }),
            Some(usage)
        );
        // RunCancelled / RunFailed 不携带用量，依赖此前观测到的 UsageUpdated。
        assert_eq!(
            usage_from_event(&AgentEvent::RunCancelled { reason: None }),
            None
        );
    }

    #[tokio::test]
    async fn record_run_usage_uses_real_session_stable_id_and_real_time() {
        let run_id = RunId::from("run-x");
        let session = SessionId::from("session-real");
        let provider = ProviderId::from("mock");
        let model = ModelId::from("mock-model");
        let usage = tokens();
        // 用量观测/终态事件时间（远大于 0，且非 run 创建时间默认）。
        let occurred_at_ms: u64 = 1_700_000_000_000;

        let record = record_run_usage(
            &run_id,
            &session,
            &canonical_root_agent_id(&session),
            &provider,
            &model,
            0,
            occurred_at_ms,
            &usage,
            &local_attribution(),
        );
        // 真实 session_id，非默认。
        assert_eq!(record.session_id, session);
        assert_ne!(record.session_id, SessionId::default());
        // P18-4 审查补救：agent 为 session 作用域 canonical root AgentId（非默认）。
        assert_eq!(record.agent_id, canonical_root_agent_id(&session));
        assert_ne!(record.agent_id, agent_domain::AgentId::default());
        // 真实身份：local/default + local/user，不再硬编码 quota 默认。
        assert_eq!(
            record.tenant_id,
            tenant_service::IdentityContext::local().tenant_id
        );
        assert_eq!(
            record.principal_id,
            tenant_service::IdentityContext::local().principal_id
        );
        // P18-8：归属来自显式注入的 UsageAttribution（含 account/credential/
        // trace），record_run_usage 不再内部硬编码 synthetic 账号。
        assert_eq!(record.account_id, core_api::DEFAULT_QUOTA_ACCOUNT);
        assert_eq!(record.credential_id, None);
        assert_eq!(record.trace_id, None);
        // 真实 usage/终态时间，非 0。
        assert_eq!(record.occurred_at_ms, occurred_at_ms);
        // record_id 确定性派生：重试同 record 内容稳定。
        let again = record_run_usage(
            &run_id,
            &session,
            &canonical_root_agent_id(&session),
            &provider,
            &model,
            0,
            occurred_at_ms,
            &usage,
            &local_attribution(),
        );
        assert_eq!(record.record_id, again.record_id);
        assert!(record.record_id.ends_with("attempt-0"));
        // P18-8 v2：trace 与定价快照随记录持久化。
        assert_eq!(record.version, usage_ledger::RECORD_VERSION);
        assert_eq!(record.upstream_attempt, Some(0));
        assert_eq!(
            record.rate_card.as_deref(),
            Some(model_registry::BUILTIN_RATE_CARD)
        );
        assert_eq!(
            record.rate_version.as_deref(),
            Some(model_registry::BUILTIN_RATE_VERSION)
        );
        // mock-model 不在内置目录：回退 Unknown + no-pricing provenance，
        // 记账不受影响。
        assert_eq!(
            record.cost_confidence,
            Some(usage_ledger::CostConfidence::Unknown)
        );
        assert_eq!(
            record.cost_provenance.as_deref(),
            Some("no-pricing:unknown-model")
        );

        // 内置模型（gpt-4o）走 estimate 分支：快照完整可追溯。
        let estimated = record_run_usage(
            &run_id,
            &session,
            &canonical_root_agent_id(&session),
            &provider,
            &ModelId::from("gpt-4o"),
            0,
            occurred_at_ms,
            &usage,
            &local_attribution(),
        );
        assert_eq!(
            estimated.cost_confidence,
            Some(usage_ledger::CostConfidence::Estimated)
        );
        assert_eq!(
            estimated.cost_provenance.as_deref(),
            Some("builtin:2026-08-01:estimate")
        );
        assert_eq!(estimated.rate_card.as_deref(), Some("builtin"));
    }

    #[tokio::test]
    async fn record_run_usage_per_call_carries_request_event_attempt_and_trace() {
        let mut attribution = local_attribution();
        attribution.trace_id = Some("trace-run-42".to_string());
        attribution.credential_id = Some("cred-leased-7".to_string());
        let turn = TurnObservation {
            request_id: RequestId::from("req-upstream-9"),
            usage: tokens(),
            event_id: EventId::from("evt-usage-3"),
            occurred_at_ms: 1_700_000_000_001,
            turn_index: 2,
        };
        let record = record_run_usage_per_call(
            &RunId::from("run-per-call"),
            &SessionId::from("session-per-call"),
            &canonical_root_agent_id(&SessionId::from("session-per-call")),
            &ProviderId::from("mock"),
            &ModelId::from("gpt-4o"),
            &turn,
            &attribution,
        );
        assert_eq!(
            record.record_id,
            "auto-rec-run-run-per-call-request-req-upstream-9-attempt-2"
        );
        assert_eq!(
            record.request_id.as_ref().map(|id| id.as_str()),
            Some("req-upstream-9")
        );
        assert_eq!(
            record.event_id.as_ref().map(|id| id.as_str()),
            Some("evt-usage-3")
        );
        assert_eq!(record.upstream_attempt, Some(2));
        assert_eq!(record.trace_id.as_deref(), Some("trace-run-42"));
        assert_eq!(record.credential_id.as_deref(), Some("cred-leased-7"));
        assert_eq!(record.account_id, core_api::DEFAULT_QUOTA_ACCOUNT);
        assert_eq!(
            record.agent_id,
            canonical_root_agent_id(&SessionId::from("session-per-call"))
        );
        // 定价快照与终态记账同源（gpt-4o estimate）。
        assert_eq!(record.cost_micros, 750);
        assert_eq!(
            record.cost_confidence,
            Some(usage_ledger::CostConfidence::Estimated)
        );

        // 幂等重放：同记录重复写入不重复计数。
        let ledger: Arc<dyn usage_ledger::UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        ledger.record(record.clone()).await.expect("first record");
        ledger.record(record.clone()).await.expect("replay record");
        let stored = ledger
            .query(&usage_ledger::UsageQuery::by_session(SessionId::from(
                "session-per-call",
            )))
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
    }

    #[tokio::test]
    async fn record_run_usage_replay_is_idempotent() {
        let ledger: Arc<dyn usage_ledger::UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let run_id = RunId::from("run-replay");
        let session = SessionId::from("session-replay");
        let provider = ProviderId::from("mock");
        let model = ModelId::from("mock-model");
        let record = record_run_usage(
            &run_id,
            &session,
            &canonical_root_agent_id(&session),
            &provider,
            &model,
            0,
            1_700_000_000_000,
            &tokens(),
            &local_attribution(),
        );
        // 同一 record 重复写入：重放成功，不重复计数。
        ledger.record(record.clone()).await.expect("first record");
        ledger.record(record.clone()).await.expect("replay record");
        let stored = ledger
            .query(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await
            .unwrap();
        assert_eq!(stored.len(), 1, "重放不得产生第二条记录");
        let totals = ledger
            .aggregate(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await
            .expect("aggregate");
        assert_eq!(totals.input_tokens, 100);
        assert_eq!(totals.output_tokens, 50);
    }

    #[tokio::test]
    async fn record_run_usage_estimates_cost_for_known_model() {
        // gpt-4o 定价：input $2.5/M、output $10/M；usage(100, 50) => 750 micros。
        let record = record_run_usage(
            &RunId::from("run-cost"),
            &SessionId::from("session-cost"),
            &canonical_root_agent_id(&SessionId::from("session-cost")),
            &ProviderId::from("mock"),
            &ModelId::from("gpt-4o"),
            0,
            1_700_000_000_000,
            &tokens(),
            &local_attribution(),
        );
        assert_eq!(record.cost_micros, 750, "已知模型成本非零且数值精确");
        assert_eq!(record.currency, "USD");
        assert_eq!((record.input_tokens, record.output_tokens), (100, 50));
    }

    #[tokio::test]
    async fn record_run_usage_unknown_model_falls_back_to_zero_usd() {
        let ledger: Arc<dyn usage_ledger::UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let record = record_run_usage(
            &RunId::from("run-unknown"),
            &SessionId::from("session-unknown"),
            &canonical_root_agent_id(&SessionId::from("session-unknown")),
            &ProviderId::from("mock"),
            &ModelId::from("no-such-model"),
            0,
            1_700_000_000_000,
            &tokens(),
            &local_attribution(),
        );
        // 未知模型：费用回退 0/USD，记账不受影响（tokens 仍真实写入）。
        assert_eq!(record.cost_micros, 0);
        assert_eq!(record.currency, "USD");
        ledger
            .record(record.clone())
            .await
            .expect("未知模型仍可记账");
        let stored = ledger
            .query(&usage_ledger::UsageQuery::by_session(SessionId::from(
                "session-unknown",
            )))
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!((stored[0].input_tokens, stored[0].output_tokens), (100, 50));
    }

    #[tokio::test]
    async fn concurrent_runs_do_not_cross_usage_on_global_broadcaster() {
        let (supervisor, aggregate, ledger, _broadcaster) = supervisor_with_ledger();
        seed_session(&aggregate, "ws-concurrent-a");
        seed_session(&aggregate, "ws-concurrent-b");
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let provider_a: Arc<dyn ModelProvider> = Arc::new(BarrierUsageProvider {
            usage: usage(11, 1),
            barrier: Arc::clone(&barrier),
        });
        let provider_b: Arc<dyn ModelProvider> = Arc::new(BarrierUsageProvider {
            usage: usage(22, 2),
            barrier,
        });

        supervisor
            .start(
                run_request("run-concurrent-a", "session-concurrent-a"),
                provider_a,
            )
            .expect("start run a");
        supervisor
            .start(
                run_request("run-concurrent-b", "session-concurrent-b"),
                provider_b,
            )
            .expect("start run b");

        let session_a = SessionId::from("session-concurrent-a");
        let session_b = SessionId::from("session-concurrent-b");
        let record_a = await_usage_record(&ledger, &session_a)
            .await
            .expect("run a usage");
        let record_b = await_usage_record(&ledger, &session_b)
            .await
            .expect("run b usage");
        assert_eq!((record_a.input_tokens, record_a.output_tokens), (11, 1));
        assert_eq!((record_b.input_tokens, record_b.output_tokens), (22, 2));
        assert_eq!(
            record_a.run_id.as_ref(),
            Some(&RunId::from("run-concurrent-a"))
        );
        assert_eq!(
            record_b.run_id.as_ref(),
            Some(&RunId::from("run-concurrent-b"))
        );
    }

    #[tokio::test]
    async fn multi_turn_usage_sums_latest_snapshot_from_each_provider_turn() {
        let broadcaster = EventBroadcaster::new();
        let (supervisor, aggregate, ledger, approvals) = supervisor_with_ledger_on(broadcaster);
        seed_session(&aggregate, "ws-multi-turn");
        let run_id = RunId::from("run-multi-turn");
        approvals
            .decide(
                &run_id,
                &agent_domain::ToolCallId::from("mock-tool-call-0"),
                core_api::ApprovalDecision::ApproveForRun,
            )
            .expect("queue tool approval");
        let provider: Arc<dyn ModelProvider> = Arc::new(SequenceProvider::new(vec![
            test_support::MockScript::new()
                .tool_call("echo", serde_json::json!({"text": "hi"}))
                .usage(usage(10, 1))
                .usage(usage(12, 2))
                .complete_with(agent_domain::StopReason::ToolUse),
            test_support::MockScript::new()
                .text("done")
                .usage(usage(20, 3))
                .usage(usage(25, 4))
                .complete(),
        ]));
        supervisor
            .start(
                run_request("run-multi-turn", "session-multi-turn"),
                provider,
            )
            .expect("start multi-turn run");

        let session = SessionId::from("session-multi-turn");
        let all = await_usage_records(&ledger, &session, 2).await;
        // P18-8 逐上游调用记账：每轮 provider request 一条独立不可变记录，
        // 同轮旧快照不重复累计（第一轮 12/2、第二轮 25/4），request/event
        // id + attempt 随记录持久化。
        assert_eq!(all.len(), 2, "每次实际上游调用独立记账");
        assert!(
            all.iter().all(|r| r.request_id.is_some()),
            "per-call 记录必须携带 request_id"
        );
        assert!(
            all.iter().all(|r| r.event_id.is_some()),
            "per-call 记录必须携带 event_id"
        );
        assert!(
            all.iter().all(|r| r.upstream_attempt.is_some()),
            "per-call 记录必须携带 attempt"
        );
        let mut usage: Vec<_> = all
            .iter()
            .map(|r| (r.input_tokens, r.output_tokens))
            .collect();
        usage.sort_unstable();
        assert_eq!(usage, vec![(12, 2), (25, 4)], "每轮只取最后一次快照");
        let totals = ledger
            .aggregate(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await
            .expect("aggregate multi-turn");
        assert_eq!(
            (totals.input_tokens, totals.output_tokens),
            (37, 6),
            "逐调用记账后聚合不双计"
        );
        // 终态兜底与 per-call 互斥：不得再写 run 汇总单条。
        let all = ledger
            .query(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await
            .unwrap();
        assert_eq!(all.len(), 2, "per-call 记账后不得叠加终态汇总");
    }

    #[tokio::test]
    async fn try_recv_lagged_continues_draining_to_terminal_usage() {
        let broadcaster = EventBroadcaster::with_capacity(1);
        let (supervisor, aggregate, ledger, _approvals) = supervisor_with_ledger_on(broadcaster);
        seed_session(&aggregate, "ws-lagged");
        let provider: Arc<dyn ModelProvider> = Arc::new(test_support::MockProvider::new(
            test_support::MockScript::new()
                .text("a")
                .text("b")
                .usage(tokens())
                .complete(),
        ));
        supervisor
            .start(run_request("run-lagged", "session-lagged"), provider)
            .expect("start lagged run");

        let record = await_usage_record(&ledger, &SessionId::from("session-lagged"))
            .await
            .expect("Lagged drain must continue to RunCompleted");
        assert_eq!((record.input_tokens, record.output_tokens), (100, 50));
        // Lagged 丢弃旧事件后，终态仍以 engine summary 的 run 累计值兜底记账：
        // 只写一条、聚合不双计。
        let session = SessionId::from("session-lagged");
        let all = ledger
            .query(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await
            .unwrap();
        assert_eq!(all.len(), 1, "Lagged 兜底也不得重复记账");
        let totals = ledger
            .aggregate(&usage_ledger::UsageQuery::by_session(session))
            .await
            .expect("aggregate lagged");
        assert_eq!(
            (totals.input_tokens, totals.output_tokens),
            (100, 50),
            "Lagged 兜底后聚合不双计"
        );
    }

    #[tokio::test]
    async fn retries_use_distinct_attempt_records_and_each_attempt_is_counted() {
        let (supervisor, aggregate, ledger, _broadcaster) = supervisor_with_ledger();
        seed_session(&aggregate, "ws-retry-attempt");
        let run_id = RunId::from("run-retry-attempt");
        let session_id = SessionId::from("session-retry-attempt");
        let provider: Arc<dyn ModelProvider> = Arc::new(test_support::MockProvider::new(
            test_support::MockScript::new()
                .usage(tokens())
                .fail(provider_api::ProviderError::new(
                    provider_api::ProviderErrorKind::InvalidRequest,
                    "expected failure",
                )),
        ));
        supervisor
            .start(
                run_request("run-retry-attempt", "session-retry-attempt"),
                provider,
            )
            .expect("start original attempt");
        assert_eq!(await_usage_records(&ledger, &session_id, 1).await.len(), 1);

        retry_when_terminal(&supervisor, &run_id).await;
        assert_eq!(await_usage_records(&ledger, &session_id, 2).await.len(), 2);
        retry_when_terminal(&supervisor, &run_id).await;
        let records = await_usage_records(&ledger, &session_id, 3).await;
        assert_eq!(records.len(), 3);
        let mut record_ids: Vec<_> = records
            .iter()
            .map(|record| record.record_id.as_str())
            .collect();
        record_ids.sort_unstable();
        // P18-8 逐上游调用记账：每次 run 消费（首次 + 2 次 retry）各一次
        // 上游调用，记录以 request_id + turn attempt 独立去重；每次 retry
        // 是新请求（新 request_id），因此 3 条记录两两不同。
        assert_eq!(record_ids.len(), 3);
        assert!(
            record_ids.iter().all(|id| id.contains("-request-")),
            "per-call 记录 ID 必须含 request_id：{record_ids:?}"
        );
        assert!(
            record_ids.iter().all(|id| id.ends_with("-attempt-1")),
            "单次调用 turn index 为 1：{record_ids:?}"
        );
        assert!(
            records
                .iter()
                .all(|record| record.upstream_attempt == Some(1)),
            "每次上游调用 attempt 独立持久化"
        );
        // 三次消费的 request_id 互不相同（retry 是新的上游调用）。
        let request_ids: std::collections::BTreeSet<_> = records
            .iter()
            .filter_map(|record| record.request_id.as_ref().map(|id| id.as_str()))
            .collect();
        assert_eq!(request_ids.len(), 3, "retry 必须产生新的 request_id");
        let totals = ledger
            .aggregate(&usage_ledger::UsageQuery::by_session(session_id))
            .await
            .expect("aggregate attempts");
        assert_eq!((totals.input_tokens, totals.output_tokens), (300, 150));
    }

    /// P18-8 review：transport retry（同一 run 内 engine `RetryController`
    /// 驱动的断流重试）的每次实际上游 `stream` 调用都必须以独立不可变
    /// ledger 记录收口——request_id / event_id / upstream_attempt 逐调用
    /// 持久化，失败前已观测的 UsageUpdated 单独收口，聚合只累计一次。
    #[tokio::test]
    async fn transport_retry_records_each_stream_attempt_in_ledger() {
        let (supervisor, aggregate, ledger, _broadcaster) = supervisor_with_ledger();
        seed_session(&aggregate, "ws-transport-retry");
        let session_id = SessionId::from("session-transport-retry");
        let flaky = |kind: provider_api::ProviderErrorKind| {
            let mut error = provider_api::ProviderError::new(kind, "flaky transport");
            error.retryable = true;
            error.retry_after_ms = Some(0);
            error
        };
        let usage = |input: u64, output: u64| agent_domain::TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        // 三次实际 stream 调用：前两次 transport 失败（失败前均已观测
        // UsageUpdated），第三次成功；engine 在同一 run 内自动重试。
        let sequence = SequenceProvider::new(vec![
            test_support::MockScript::new()
                .usage(usage(11, 1))
                .fail(flaky(provider_api::ProviderErrorKind::Network)),
            test_support::MockScript::new()
                .usage(usage(22, 2))
                .fail(flaky(provider_api::ProviderErrorKind::Timeout)),
            test_support::MockScript::new()
                .usage(usage(33, 3))
                .complete(),
        ]);
        let provider: Arc<dyn ModelProvider> = Arc::new(sequence.clone());
        supervisor
            .start(
                run_request("run-transport-retry", "session-transport-retry"),
                provider,
            )
            .expect("start transport-retry run");

        // 第三次调用成功 → run Completed；逐调用记账在终态计数之前完成。
        await_completed(&supervisor).await;
        let records = await_usage_records(&ledger, &session_id, 3).await;
        // 再等一拍确认没有兜底记录混入（per-call 与 run 终态兜底互斥）。
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let all = ledger
            .query(&usage_ledger::UsageQuery::by_session(session_id.clone()))
            .await
            .unwrap();
        assert_eq!(
            all.len(),
            3,
            "每次实际上游调用恰好一条记录，不得双计：{all:?}"
        );
        assert_eq!(records.len(), 3);

        // 与 provider 实际观察到的三次调用 request_id 一一对应（按顺序）。
        let observed = sequence
            .request_ids()
            .into_iter()
            .map(|id| id.as_str().to_string())
            .collect::<Vec<_>>();
        assert_eq!(observed.len(), 3, "三次实际上游 stream 调用");
        let mut sorted_observed = observed.clone();
        sorted_observed.sort_unstable();
        sorted_observed.dedup();
        assert_eq!(
            sorted_observed.len(),
            3,
            "transport retry 每次调用必须生成新 request_id"
        );

        let mut sorted_records = records.clone();
        sorted_records.sort_by_key(|record| record.upstream_attempt);
        let attempts: Vec<u64> = sorted_records
            .iter()
            .map(|record| record.upstream_attempt.expect("attempt"))
            .collect();
        assert_eq!(attempts, vec![1, 2, 3], "attempt 逐调用独立 1 基编号");
        let record_request_ids: Vec<String> = sorted_records
            .iter()
            .map(|record| {
                record
                    .request_id
                    .as_ref()
                    .expect("request_id")
                    .as_str()
                    .to_string()
            })
            .collect();
        assert_eq!(
            record_request_ids, observed,
            "ledger request_id 必须与 provider 观察到的每次调用一一对应"
        );
        // 每条记录都带独立的 UsageUpdated 事件 id（失败前观测也单独收口）。
        let event_ids: std::collections::BTreeSet<_> = sorted_records
            .iter()
            .map(|record| record.event_id.as_ref().expect("event_id").as_str())
            .collect();
        assert_eq!(event_ids.len(), 3, "每次调用的 UsageUpdated 事件 id 独立");
        assert!(
            sorted_records
                .iter()
                .all(|record| record.record_id.contains("-request-")
                    && record.record_id.contains("-attempt-")),
            "记录 ID 由 (run, request, attempt) 确定性派生"
        );
        // 失败前观测的用量完整计入聚合，只累计一次。
        let totals = ledger
            .aggregate(&usage_ledger::UsageQuery::by_session(session_id))
            .await
            .expect("aggregate transport retry");
        assert_eq!(
            (totals.input_tokens, totals.output_tokens),
            (66, 6),
            "三次调用用量 (11+22+33, 1+2+3) 各累计一次"
        );

        // 幂等重放：逐条重放不重复记账（账本按 request/attempt 去重）。
        for record in &records {
            ledger.record(record.clone()).await.expect("replay record");
        }
        let after_replay = ledger
            .query(&usage_ledger::UsageQuery::by_session(SessionId::from(
                "session-transport-retry",
            )))
            .await
            .unwrap();
        assert_eq!(after_replay.len(), 3, "重放不重复累计");
    }

    /// P18-8 review：最后一次上游调用也失败（retry 全部耗尽 → run Failed）
    /// 时，失败前已观测的 UsageUpdated 仍单独收口，不因 run 失败丢失。
    #[tokio::test]
    async fn transport_retry_failed_run_closes_out_observed_usage_per_attempt() {
        let (supervisor, aggregate, ledger, _broadcaster) = supervisor_with_ledger();
        seed_session(&aggregate, "ws-transport-fail");
        let session_id = SessionId::from("session-transport-fail");
        let flaky = || {
            let mut error =
                provider_api::ProviderError::new(provider_api::ProviderErrorKind::Network, "down");
            error.retryable = true;
            error.retry_after_ms = Some(0);
            error
        };
        let usage = |input: u64| agent_domain::TokenUsage {
            input_tokens: input,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        // 默认 RetryPolicy(max_attempts=3)：1 次首调 + 3 次重试 = 4 次
        // 实际上游调用，全部失败，每次失败前都观测到用量。
        let provider: Arc<dyn ModelProvider> = Arc::new(SequenceProvider::new(vec![
            test_support::MockScript::new()
                .usage(usage(7))
                .fail(flaky()),
            test_support::MockScript::new()
                .usage(usage(8))
                .fail(flaky()),
            test_support::MockScript::new()
                .usage(usage(9))
                .fail(flaky()),
            test_support::MockScript::new()
                .usage(usage(10))
                .fail(flaky()),
        ]));
        supervisor
            .start(
                run_request("run-transport-fail", "session-transport-fail"),
                provider,
            )
            .expect("start failing run");

        // 等 run 到达 Failed 终态（终态计数在记账之后递增）。
        let mut failed = false;
        for _ in 0..300 {
            if supervisor.stats().failed >= 1 {
                failed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(failed, "run 必须失败");
        let records = await_usage_records(&ledger, &session_id, 4).await;
        let mut attempts: Vec<u64> = records
            .iter()
            .map(|record| record.upstream_attempt.expect("attempt"))
            .collect();
        attempts.sort_unstable();
        assert_eq!(attempts, vec![1, 2, 3, 4], "每次失败调用独立收口");
        let request_ids: std::collections::BTreeSet<_> = records
            .iter()
            .filter_map(|record| record.request_id.as_ref().map(|id| id.as_str()))
            .collect();
        assert_eq!(request_ids.len(), 4, "每次失败调用独立 request_id");
        let totals = ledger
            .aggregate(&usage_ledger::UsageQuery::by_session(session_id))
            .await
            .expect("aggregate failed attempts");
        assert_eq!(totals.input_tokens, 34, "7+8+9+10 各累计一次，不丢不重");
    }

    #[tokio::test]
    async fn terminal_usage_persisted_on_success() {
        let broadcaster = EventBroadcaster::new();
        let ledger: Arc<dyn usage_ledger::UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        let (supervisor, aggregate, ledger, runtime) = supervisor_with_quota(broadcaster, ledger);
        seed_session(&aggregate, "ws-ok");
        let session = SessionId::from("session-ok");
        let provider: Arc<dyn ModelProvider> = Arc::new(
            test_support::MockProvider::new(
                test_support::MockScript::new().usage(tokens()).complete(),
            )
            .with_id(ProviderId::from("mock")),
        );
        // gpt-4o 已知定价：验证记账、费用估算与 Cost 缓存值端到端一致。
        let request = RunRequest {
            model: ModelId::from("gpt-4o"),
            ..run_request("run-ok", "session-ok")
        };
        supervisor.start(request, provider).expect("start");

        let record = await_usage_record(&ledger, &session)
            .await
            .expect("success must persist usage");
        assert_eq!(record.session_id, session, "真实 session_id");
        assert_eq!(record.run_id.as_ref(), Some(&RunId::from("run-ok")));
        assert_eq!(record.input_tokens, 100);
        assert_eq!(record.output_tokens, 50);
        assert!(record.occurred_at_ms > 0, "真实 usage/终态时间");
        // 单一 ledger 条目（终态只写一次）。
        let all = ledger
            .query(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await
            .unwrap();
        assert_eq!(all.len(), 1);

        // 记账成功后才刷新本地缓存：等待四窗口 × (Token + Cost<USD>) 全部
        // cache-only 命中，且缓存值等于账本实际值（不触发任何 adapter/网络）。
        let scope = quota_service::QuotaScope {
            tenant_id: tenant_service::IdentityContext::local().tenant_id,
            account_id: quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT.to_string()),
            credential_id: None,
            provider_id: ProviderId::from("mock"),
            model_id: Some(ModelId::from("gpt-4o")),
        };
        for window in [
            quota_service::QuotaWindow::Overall,
            quota_service::QuotaWindow::Rolling5h,
            quota_service::QuotaWindow::Weekly,
            quota_service::QuotaWindow::Monthly,
        ] {
            for (unit, used) in [
                (quota_service::QuotaUnit::Token, 150u64),
                (
                    quota_service::QuotaUnit::Cost {
                        currency: "USD".into(),
                    },
                    750u64,
                ),
            ] {
                let snapshot = await_cache_hit(
                    &runtime,
                    &quota_service::QuotaRequest {
                        scope: scope.clone(),
                        window,
                        unit: unit.clone(),
                    },
                )
                .await;
                assert_eq!(
                    snapshot.values.used,
                    quota_service::QuotaMeasure::Exact(used),
                    "{window:?} {unit:?} 缓存值必须等于账本实际值"
                );
                assert_eq!(
                    snapshot.provenance.adapter_kind,
                    quota_service::AdapterKind::LocalLedger,
                    "{window:?} {unit:?} 必须来自本地 ledger 派生"
                );
            }
        }

        // 记账 + 缓存刷新成功后，必须按该 record 的完整 model scope 发布一次
        // QuotaChanged（Token 总览，run 流）；失败路径不发（见
        // terminal_usage_ledger_failure_does_not_publish_cache）。终态计数在
        // 记账/刷新/发布全部完成之后才递增，故到达 Completed 后冲刷是竞态安全的。
        await_completed(&supervisor).await;
        let events = supervisor.drain_events();
        let changed: Vec<&core_api::AppEventEnvelope> = events
            .iter()
            .filter(|envelope| matches!(envelope.payload, AppEvent::QuotaChanged { .. }))
            .collect();
        assert_eq!(changed.len(), 1, "成功记账必须恰好发布一次 QuotaChanged");
        assert_eq!(
            changed[0].stream,
            EventStream::Run(RunId::from("run-ok")),
            "QuotaChanged 走 run 流"
        );
        match &changed[0].payload {
            AppEvent::QuotaChanged { view } => {
                assert_eq!(view.scope.provider_id, ProviderId::from("mock"));
                assert_eq!(
                    view.scope.model_id.as_ref(),
                    Some(&ModelId::from("gpt-4o")),
                    "必须覆盖该 record 的完整 model scope"
                );
                assert_eq!(view.scope.credential_hint, None, "record 无 credential");
                assert_eq!(view.windows.len(), 4, "默认全部窗口");
                assert!(view.from_cache, "刷新成功后必须来自本地缓存");
                for entry in &view.windows {
                    match &entry.read {
                        core_api::WindowReadView::Ok { snapshot, .. } => {
                            assert_eq!(
                                snapshot.unit,
                                core_api::QuotaUnit::Token,
                                "QuotaChanged 只发布 Token 单位"
                            );
                            assert_eq!(
                                snapshot.scope.model_id.as_ref(),
                                Some(&ModelId::from("gpt-4o"))
                            );
                        }
                        other => panic!("每个窗口都必须是缓存命中：{:?} {other:?}", entry.window),
                    }
                }
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[tokio::test]
    async fn terminal_usage_ledger_failure_does_not_publish_cache() {
        let broadcaster = EventBroadcaster::new();
        let ledger: Arc<dyn usage_ledger::UsageLedger> = Arc::new(FailingUsageLedger::new());
        let (supervisor, aggregate, _ledger, runtime) = supervisor_with_quota(broadcaster, ledger);
        seed_session(&aggregate, "ws-fail-cache");
        let provider: Arc<dyn ModelProvider> = Arc::new(
            test_support::MockProvider::new(
                test_support::MockScript::new().usage(tokens()).complete(),
            )
            .with_id(ProviderId::from("mock")),
        );
        supervisor
            .start(
                run_request("run-fail-cache", "session-fail-cache"),
                provider,
            )
            .expect("start");

        // 等待 run 到达 Completed：记账尝试与（不应发生的）缓存刷新都在终态
        // 计数之前完成，故终态到达后断言缓存为空是竞态安全的。
        await_completed(&supervisor).await;
        assert_eq!(supervisor.stats().completed, 1, "账本失败不影响 run 终态");
        assert_eq!(
            runtime.quota.cache_size(),
            0,
            "ledger.record 失败不得发布任何缓存键"
        );
        let read = runtime
            .quota
            .read_cache_only(&quota_service::QuotaRequest {
                scope: quota_service::QuotaScope {
                    tenant_id: tenant_service::IdentityContext::local().tenant_id,
                    account_id: quota_service::AccountId::new(
                        core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
                    ),
                    credential_id: None,
                    provider_id: ProviderId::from("mock"),
                    model_id: Some(ModelId::from("mock-model")),
                },
                window: quota_service::QuotaWindow::Overall,
                unit: quota_service::QuotaUnit::Token,
            })
            .expect("cache-only read must not touch adapters");
        assert!(
            matches!(&read, quota_service::CacheRead::NoData),
            "账本失败路径不得发布缓存：{read:?}"
        );
        let events = supervisor.drain_events();
        assert!(
            !events
                .iter()
                .any(|envelope| matches!(envelope.payload, AppEvent::QuotaChanged { .. })),
            "ledger.record 失败不得发布 QuotaChanged"
        );
    }

    #[tokio::test]
    async fn terminal_usage_persisted_on_failure() {
        let (supervisor, aggregate, ledger, _broadcaster) = supervisor_with_ledger();
        seed_session(&aggregate, "ws-fail");
        let session = SessionId::from("session-fail");
        // 用量先于失败发生：UsageUpdated 已广播，失败不得丢失已发生用量。
        let provider: Arc<dyn ModelProvider> = Arc::new(
            test_support::MockProvider::new(test_support::MockScript::new().usage(tokens()).fail(
                provider_api::ProviderError::new(
                    provider_api::ProviderErrorKind::InvalidRequest,
                    "boom",
                ),
            ))
            .with_id(ProviderId::from("mock")),
        );
        supervisor
            .start(run_request("run-fail", "session-fail"), provider)
            .expect("start");

        let record = await_usage_record(&ledger, &session)
            .await
            .expect("failure must persist already-occurred usage");
        assert_eq!(record.session_id, session);
        assert_eq!(record.input_tokens, 100);
        assert!(record.occurred_at_ms > 0);
    }

    #[tokio::test]
    async fn terminal_usage_persisted_on_cancel() {
        let (supervisor, aggregate, ledger, broadcaster) = supervisor_with_ledger();
        seed_session(&aggregate, "ws-cancel");
        let run_id = RunId::from("run-cancel");
        let session = SessionId::from("session-cancel");
        let provider: Arc<dyn ModelProvider> = Arc::new(
            test_support::MockProvider::new(
                test_support::MockScript::new()
                    .usage(tokens())
                    .wait_for_cancellation(),
            )
            .with_id(ProviderId::from("mock")),
        );
        // 订阅须在 start 前建立，确保不漏 UsageUpdated。
        let mut sub = broadcaster.subscribe();
        supervisor
            .start(run_request("run-cancel", "session-cancel"), provider)
            .expect("start");
        // 先观测到 UsageUpdated 再取消，确保已发生用量被捕获。
        let mut observed = false;
        loop {
            match sub.recv().await {
                Ok(envelope) => {
                    if matches!(envelope.payload, AgentEvent::UsageUpdated { .. }) {
                        observed = true;
                        break;
                    }
                }
                Err(agent_engine::BroadcastError::Lagged { .. }) => continue,
                Err(_) => break,
            }
        }
        assert!(observed, "取消前必须已广播 UsageUpdated");
        supervisor.cancel(&run_id).expect("cancel");

        let record = await_usage_record(&ledger, &session)
            .await
            .expect("cancel must persist already-occurred usage");
        assert_eq!(record.session_id, session);
        assert_eq!(record.input_tokens, 100);
        assert!(record.occurred_at_ms > 0);
    }

    #[test]
    fn workflow_events_do_not_change_run_state_or_emit_app_events() {
        use agent_domain::{
            AutomationEvent, AutomationId, BackgroundTaskId, GoalEvent, GoalId, MemoryEvent,
            MemoryId, MonitorEvent, MonitorId, PlanEvent, PlanId, PlanVersionId, ReviewEvent,
            ReviewSessionId, TaskEvent,
        };

        let aggregate = AggregateState::new();
        let limiter = RateLimiter::new(std::time::Duration::from_secs(60), 1024);
        let global = AtomicU64::new(0);
        let stream = AtomicU64::new(0);
        let source = CommandSource::Automation;
        let command_id = CommandId::from("cmd-wf");
        let instance_id = CoreInstanceId::from("test");
        let run_id = RunId::from("run-wf");
        let session_id = SessionId::from("s-1");
        let envelope = |seq: u64, payload: AgentEvent| {
            AgentEventEnvelope::new(
                EventId::from(format!("e-{seq}")),
                session_id.clone(),
                run_id.clone(),
                agent_events::EventSequence::new(seq),
                Timestamp::from_unix_millis(seq),
                payload,
            )
        };

        // 基线：RunStarted 把状态迁移到 PreparingContext，并产生一条 RunChanged。
        let mut state = apply_agent_event(
            &aggregate,
            &limiter,
            &instance_id,
            &global,
            &stream,
            &source,
            &command_id,
            envelope(
                1,
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("m-1"),
                },
            ),
            RunState::Created,
        );
        assert_eq!(state, RunState::PreparingContext);
        assert_eq!(global.load(Ordering::SeqCst), 1);
        assert_eq!(stream.load(Ordering::SeqCst), 1);

        let workflow_events = [
            AgentEvent::Plan(PlanEvent::ReviewRequested {
                plan_id: PlanId::from("p-1"),
                version: PlanVersionId::from("v-1"),
            }),
            AgentEvent::Goal(GoalEvent::Paused {
                goal_id: GoalId::from("g-1"),
            }),
            AgentEvent::Task(TaskEvent::Suspended {
                task_id: BackgroundTaskId::from("t-1"),
            }),
            AgentEvent::Automation(AutomationEvent::Triggered {
                automation_id: AutomationId::from("a-1"),
                task_id: BackgroundTaskId::from("t-1"),
            }),
            AgentEvent::Monitor(MonitorEvent::Stopped {
                monitor_id: MonitorId::from("m-1"),
                reason: None,
            }),
            AgentEvent::Memory(MemoryEvent::Invalidated {
                memory_id: MemoryId::from("mem-1"),
                reason: "stale".into(),
            }),
            AgentEvent::Review(ReviewEvent::SessionCreated {
                session_id: ReviewSessionId::from("r-1"),
                workspace_id: None,
            }),
        ];

        for (index, payload) in workflow_events.into_iter().enumerate() {
            state = apply_agent_event(
                &aggregate,
                &limiter,
                &instance_id,
                &global,
                &stream,
                &source,
                &command_id,
                envelope(100 + index as u64, payload),
                state,
            );
            assert_eq!(
                state,
                RunState::PreparingContext,
                "workflow event {index} 不得改变 Run 状态"
            );
        }

        // 序号停在基线值；限流器中只有基线的 RunChanged，无任何 workflow 应用事件。
        assert_eq!(global.load(Ordering::SeqCst), 1);
        assert_eq!(stream.load(Ordering::SeqCst), 1);
        let flushed = limiter.flush();
        assert_eq!(flushed.len(), 1, "workflow 事件不得产生应用事件");
        assert!(matches!(&flushed[0].payload, AppEvent::RunChanged { .. }));
    }

    #[test]
    fn team_sink_mirrors_typed_events_on_shared_global_stream() {
        let limiter = Arc::new(RateLimiter::new(
            std::time::Duration::from_secs(60),
            crate::rate_limit::DEFAULT_RATE_LIMIT_BUFFER,
        ));
        let global_sequence = Arc::new(AtomicU64::new(0));
        // 与 quota 告警桥共享同一 Global 流序号（交错发布仍连续）。
        let stream_sequence = Arc::new(AtomicU64::new(0));
        let sink = AppTeamEventSink {
            limiter: Arc::clone(&limiter),
            global_sequence: Arc::clone(&global_sequence),
            stream_sequence: Arc::clone(&stream_sequence),
            instance_id: CoreInstanceId::from("inst"),
        };

        let team = teams::TeamId::from("team-1");
        let first = teams::TeamEventEnvelope::new(
            team.clone(),
            teams::TeamEventSequence::new(1),
            EventId::from("team-1-evt-1"),
            now_timestamp(),
            teams::TeamEvent::TeamCreated {
                team_id: team.clone(),
                tenant_id: agent_domain::TenantId::from("ten"),
                supervisor: agent_domain::AgentId::from("sup"),
                name: "T".into(),
            },
        );
        let second = teams::TeamEventEnvelope::new(
            team.clone(),
            teams::TeamEventSequence::new(2),
            EventId::from("team-1-evt-2"),
            now_timestamp(),
            teams::TeamEvent::MemberAdded {
                team_id: team.clone(),
                agent_id: agent_domain::AgentId::from("w1"),
                role: teams::MemberRole::Worker,
            },
        );
        use teams::TeamEventSink as _;
        sink.record(first);
        sink.record(second);

        let drained = limiter.flush();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].stream_sequence, 1);
        assert_eq!(drained[1].stream_sequence, 2);
        // 本地 global 序号由 fetch_add 前值分配（0,1）；EventHub 发布时统一
        // 重写为全局连续序列，本地值仅保证单调递增。
        assert_eq!(drained[0].global_sequence.0, 0);
        assert_eq!(drained[1].global_sequence.0, 1);
        assert_eq!(drained[0].event_id.as_str(), "team-1-evt-1");
        match &drained[0].payload {
            AppEvent::TeamEvent { event } => {
                assert_eq!(event.kind(), "team_created");
                assert_eq!(event.team_id().as_str(), "team-1");
            }
            other => panic!("expected team event mirror, got {other:?}"),
        }
        match &drained[1].payload {
            AppEvent::TeamEvent { event } => assert_eq!(event.kind(), "member_added"),
            other => panic!("expected team event mirror, got {other:?}"),
        }
    }
}
