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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_domain::{
    CancellationToken, CommandId, ContentPart, CoreInstanceId, EventId, Message, MessageId,
    MessageMetadata, MessageRole, ModelId, ProviderId, RequestId, RunId, SessionId, TextContent,
    Timestamp,
};
use agent_engine::{
    ApprovalOutcome, CancelHandle, CancelReason, EventBroadcaster, LoopContext, LoopError,
    PendingToolInvocation, ProviderLoop, ProviderLoopConfig, ToolCallResult,
};
use agent_events::{AgentEvent, AgentEventEnvelope};
use async_trait::async_trait;
use core_api::{
    AppEvent, AppEventEnvelope, CommandSource, EventSource, EventStream, GlobalSequence,
    QuotaAlertKind, QuotaOverviewQuery, QuotaUnit, RunState, API_VERSION,
};
use model_registry::ModelRegistry;
use provider_api::ModelProvider;
use thiserror::Error;
use tool_api::ToolResult;

use crate::aggregate::AggregateState;
use crate::approval::{ApprovalRegistry, Registration};
use crate::error::now_timestamp;
use crate::rate_limit::RateLimiter;

/// 默认最大并发 run 数（有界性：超限的 RunStart 返回结构化错误）。
pub const DEFAULT_MAX_CONCURRENT_RUNS: usize = 8;

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
    pub provider_id: ProviderId,
    pub model: ModelId,
    pub source: CommandSource,
    pub command_id: CommandId,
    pub user_message: String,
    /// Run 前注入的供应商中立额度信号（P14-8）；None = 不注入。
    pub external_quota: Option<agent_engine::ExternalQuotaSignal>,
}

struct RunTask {
    run_id: RunId,
    session_id: SessionId,
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
    /// P14 告警桥（长期持有）：把 quota-service 的脱敏 Alert 映射为 Global
    /// stream 的 `QuotaAlert` 事件；与 run 事件共享 limiter/global_sequence，
    /// 独立维护 Global 流序号。供 RefreshScheduler 经 [`Self::alert_sink`] 注入。
    alert_sink: Arc<dyn quota_service::refresh::AlertSink>,
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
        let alert_sink: Arc<dyn quota_service::refresh::AlertSink> = Arc::new(AppQuotaAlertSink {
            limiter: Arc::clone(&limiter),
            global_sequence: Arc::clone(&global_sequence),
            stream_sequence: AtomicU64::new(0),
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
            alert_sink,
        }
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

    /// 登记并启动一个 run。需要 tokio 运行时；无运行时返回错误（结构化，不 panic）。
    pub fn start(
        &self,
        request: RunRequest,
        provider: Arc<dyn ModelProvider>,
    ) -> Result<(), SuperviseError> {
        let mut inner = lock(&self.inner);
        if inner.tasks.contains_key(&request.run_id) {
            return Err(SuperviseError::AlreadyExists(request.run_id.to_string()));
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
        let created_at = match self.aggregate.get_run(&request.run_id) {
            Some(record) => record.created_at,
            None => now_timestamp(),
        };
        let quota_runtime = self.quota_runtime();
        let task = spawn_run_task(
            request.run_id.clone(),
            request.session_id.clone(),
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
        );
        inner.started += 1;
        inner.tasks.insert(
            request.run_id.clone(),
            RunTask {
                run_id: request.run_id,
                session_id: request.session_id,
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
            },
        );
        Ok(())
    }

    /// 取消 run（幂等）：未登记返回 NotFound；已取消或已终态为 no-op。
    pub fn cancel(&self, run_id: &RunId) -> Result<CancelOutcome, SuperviseError> {
        let inner = lock(&self.inner);
        let task = inner
            .tasks
            .get(run_id)
            .ok_or_else(|| SuperviseError::NotFound(run_id.to_string()))?;
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
        let mut inner = lock(&self.inner);
        let task = inner
            .tasks
            .get(run_id)
            .ok_or_else(|| SuperviseError::NotFound(run_id.to_string()))?;
        let current = task.state.lock().expect("run task state").clone();
        if current == RunState::Completed {
            return Err(SuperviseError::Completed(run_id.to_string()));
        }
        if !terminal(&current) {
            return Err(SuperviseError::StillActive(run_id.to_string()));
        }
        let attempt = task
            .attempt
            .checked_add(1)
            .expect("run retry attempt counter overflow");

        let request = RunRequest {
            run_id: task.run_id.clone(),
            session_id: task.session_id.clone(),
            provider_id: task.provider_id.clone(),
            model: task.model.clone(),
            source: task.source.clone(),
            command_id: CommandId::from(format!("retry-{}", task.run_id)),
            user_message: task.user_message.clone(),
            external_quota: None,
        };
        let cancel = CancelHandle::new(
            task.run_id.clone(),
            Arc::new(agent_engine::NoopProcessTreeCleaner),
        );
        let new_state = Arc::new(Mutex::new(RunState::Created));
        let (config, queue) = self.build_config(&request);
        let created_at = match self.aggregate.get_run(run_id) {
            Some(record) => record.created_at,
            None => now_timestamp(),
        };
        let quota_runtime = self.quota_runtime();
        let join = spawn_run_task(
            task.run_id.clone(),
            task.session_id.clone(),
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
        );
        let _ = self.aggregate.set_run_state(run_id, RunState::Created);
        if let Some(task) = inner.tasks.get_mut(run_id) {
            task.cancel = cancel;
            task.state = new_state;
            task.join = join;
            task.config = config;
            task.attempt = attempt;
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
        let config = ProviderLoopConfig {
            session_id: request.session_id.clone(),
            run_id: request.run_id.clone(),
            provider_id: request.provider_id.clone(),
            model: request.model.clone(),
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            initial_messages: vec![message],
            max_iterations: 16,
            budget: agent_engine::BudgetLimits::default(),
            retry: agent_engine::RetryPolicy::default(),
            thinking: None,
            reasoning: None,
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
    stream_sequence: AtomicU64,
    instance_id: CoreInstanceId,
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

/// 终态幂等记账：`record_id` 由 (run, session, provider) 确定性派生（session 防
/// 跨 session 冲突），相同内容重复写入为重放成功（usage-ledger 幂等语义），不会
/// 重复计数。
///
/// 归属字段取真实值：`session_id` 来自 run 请求（非默认），`occurred_at_ms` 取
/// 用量观测/终态事件的真实时间戳（非 run 创建时间）。`principal_id` / `agent_id`
/// 在当前 run 上下文不可得，留默认（账本不校验其非空）。费用按 builtin 定价
/// 估算（`cost_micros`/`currency`）；未知模型/无定价回退 0/USD，不影响记账。
fn record_run_usage(
    run_id: &RunId,
    session_id: &SessionId,
    provider_id: &ProviderId,
    model: &ModelId,
    attempt: u64,
    occurred_at_ms: u64,
    usage: &agent_domain::TokenUsage,
) -> usage_ledger::UsageRecord {
    let estimated = ModelRegistry::builtin().estimate_cost(model.as_str(), usage);
    let (cost_micros, currency) = match &estimated {
        Some(cost) => (cost.amount_micros, cost.currency.clone()),
        None => (0, "USD".to_string()),
    };
    usage_ledger::UsageRecord {
        record_id: format!(
            "auto-rec-run-{}-session-{}-{}-attempt-{attempt}",
            run_id,
            session_id,
            provider_id.as_str(),
        ),
        tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
        principal_id: agent_domain::PrincipalId::default(),
        account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
        credential_id: None,
        session_id: session_id.clone(),
        agent_id: agent_domain::AgentId::default(),
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
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_run_task(
    run_id: RunId,
    session_id: SessionId,
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
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let context = Arc::new(AppLoopContext::new(
            run_id.clone(),
            Arc::clone(&approvals),
            cancel.clone(),
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
        let stream_sequence = AtomicU64::new(0);

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
        // P14-8：终态幂等记账——Completed/Failed/Cancelled（含 Interrupted）只写一次。
        // 成功以 engine Ok summary 的 run 级累计 usage 权威记账，时间取终态观测
        // 时间、缺失时回退当前时间；失败/取消仍用已观测快照，不丢已发生用量。
        // record_id 由 (run, session, provider) 确定性派生，重放内容稳定，ledger
        // 幂等语义保证不重复计数。记账成功后才刷新本地额度缓存（四窗口
        // Token/Cost）；账本失败不发布缓存，保证缓存与账本一致。
        if terminal(&final_state) {
            if let Some(runtime) = quota_runtime.as_ref() {
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
                        &provider_id,
                        &model,
                        attempt,
                        occurred_at_ms,
                        &usage,
                    );
                    // 不静默：错误体（InvalidRecord/Conflict/MixedCurrencies）不含密钥或凭据，
                    // 结构化上报便于诊断；不影响 run 终态语义。
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
                    } else {
                        // 记账成功后才刷新本地缓存，保证缓存与账本同源；失败只
                        // 上报失败键数量与本地 scope 标记（可诊断，不输出 scope
                        // 明细或凭据等潜在 secret），不改变 run 终态语义。
                        if let Err(failures) = runtime.refresh_local_cache(&record).await {
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
                        } else {
                            // 记账 + 缓存刷新都成功：按该 record 的完整
                            // model/credential scope 构建 Token overview，经共享
                            // push_event 发布 QuotaChanged（run 流）。任一前置
                            // 步骤失败都不会走到这里，也就不发事件。
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
                                &limiter,
                                &instance_id,
                                &global_sequence,
                                &stream_sequence,
                                &source,
                                &command_id,
                                &run_id,
                                AppEvent::QuotaChanged {
                                    view: Box::new(view),
                                },
                                now_timestamp(),
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
        | AgentEvent::CheckpointRolledBack { .. } => None,
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

fn lock<T>(inner: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Provider Loop 回调适配：审批经 [`ApprovalRegistry`] 等待通道，工具执行在
/// P13-1 为最小 no-op 实现（返回成功结果，供审批→执行→回填链路闭环）。
pub struct AppLoopContext {
    run_id: RunId,
    approvals: Arc<ApprovalRegistry>,
    cancel: CancelHandle,
    next_message: AtomicU64,
    next_request: AtomicU64,
}

impl AppLoopContext {
    pub fn new(run_id: RunId, approvals: Arc<ApprovalRegistry>, cancel: CancelHandle) -> Self {
        Self {
            run_id,
            approvals,
            cancel,
            next_message: AtomicU64::new(0),
            next_request: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl LoopContext for AppLoopContext {
    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        _events: agent_engine::LoopEventEmitter,
        _cancel: CancellationToken,
    ) -> Vec<ToolCallResult> {
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
        RequestId::from(format!("req-{}-{}", self.run_id, sequence))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{ModelId, ProviderId};
    use usage_ledger::InMemoryUsageLedger;

    #[derive(Clone)]
    struct SequenceProvider {
        phases: Arc<Vec<Arc<test_support::MockProvider>>>,
        calls: Arc<AtomicU64>,
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
            }
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

        async fn query(&self, query: &usage_ledger::UsageQuery) -> Vec<usage_ledger::UsageRecord> {
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
        let _ = aggregate.create_session(workspace_id, "s".into(), now_timestamp());

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
                    provider_id: ProviderId::from("mock"),
                    model: ModelId::from("mock-model"),
                    source: CommandSource::Automation,
                    command_id: CommandId::from("cmd-1"),
                    user_message: "hello".into(),
                    external_quota: None,
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
        let _ = aggregate.create_session(workspace_id, "s".into(), now_timestamp());
        let provider: Arc<dyn ModelProvider> = Arc::new(
            test_support::MockProvider::new(test_support::MockScript::new().complete())
                .with_id(ProviderId::from("mock")),
        );
        supervisor
            .start(
                RunRequest {
                    run_id: run_id.clone(),
                    session_id,
                    provider_id: ProviderId::from("mock"),
                    model: ModelId::from("mock-model"),
                    source: CommandSource::Automation,
                    command_id: CommandId::from("cmd-2"),
                    user_message: "hi".into(),
                    external_quota: None,
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
            provider_id: ProviderId::from("mock"),
            model: ModelId::from("mock-model"),
            source: CommandSource::Automation,
            command_id: CommandId::from("cmd"),
            user_message: "hi".into(),
            external_quota: None,
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
        let _ = aggregate.create_session(workspace_id, "s".into(), now_timestamp());
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
            if let Some(record) = ledger.query(&query).await.into_iter().next() {
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
                .await;
            if records.len() >= expected {
                return records;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        ledger
            .query(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await
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
            &provider,
            &model,
            0,
            occurred_at_ms,
            &usage,
        );
        // 真实 session_id，非默认。
        assert_eq!(record.session_id, session);
        assert_ne!(record.session_id, SessionId::default());
        // 真实 usage/终态时间，非 0。
        assert_eq!(record.occurred_at_ms, occurred_at_ms);
        // record_id 确定性派生：重试同 record 内容稳定。
        let again = record_run_usage(
            &run_id,
            &session,
            &provider,
            &model,
            0,
            occurred_at_ms,
            &usage,
        );
        assert_eq!(record.record_id, again.record_id);
        assert!(record.record_id.ends_with("attempt-0"));
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
            &provider,
            &model,
            0,
            1_700_000_000_000,
            &tokens(),
        );
        // 同一 record 重复写入：重放成功，不重复计数。
        ledger.record(record.clone()).await.expect("first record");
        ledger.record(record.clone()).await.expect("replay record");
        let stored = ledger
            .query(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await;
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
            &ProviderId::from("mock"),
            &ModelId::from("gpt-4o"),
            0,
            1_700_000_000_000,
            &tokens(),
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
            &ProviderId::from("mock"),
            &ModelId::from("no-such-model"),
            0,
            1_700_000_000_000,
            &tokens(),
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
            .await;
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

        let record = await_usage_record(&ledger, &SessionId::from("session-multi-turn"))
            .await
            .expect("multi-turn usage");
        // 第一轮只取 12/2，第二轮只取 25/4；同轮旧快照不重复累计。
        assert_eq!((record.input_tokens, record.output_tokens), (37, 6));
        // RunCompleted 的 usage 是 run 级累计值：覆盖总量、不与已提交 turn 相加
        // （否则双计为 49/8）；成功可记账且只写一条，record_id 含 session 防冲突。
        let session = SessionId::from("session-multi-turn");
        let all = ledger
            .query(&usage_ledger::UsageQuery::by_session(session.clone()))
            .await;
        assert_eq!(all.len(), 1, "成功终态只记账一次，不双计");
        assert!(
            all.iter()
                .all(|r| r.record_id.contains("session-multi-turn")),
            "record_id 必须含 session_id，避免跨 session 冲突"
        );
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
            .await;
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
        assert!(record_ids.iter().any(|id| id.ends_with("attempt-0")));
        assert!(record_ids.iter().any(|id| id.ends_with("attempt-1")));
        assert!(record_ids.iter().any(|id| id.ends_with("attempt-2")));
        let totals = ledger
            .aggregate(&usage_ledger::UsageQuery::by_session(session_id))
            .await
            .expect("aggregate attempts");
        assert_eq!((totals.input_tokens, totals.output_tokens), (300, 150));
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
            .await;
        assert_eq!(all.len(), 1);

        // 记账成功后才刷新本地缓存：等待四窗口 × (Token + Cost<USD>) 全部
        // cache-only 命中，且缓存值等于账本实际值（不触发任何 adapter/网络）。
        let scope = quota_service::QuotaScope {
            tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
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
                    tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
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
}
