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
    AppEvent, AppEventEnvelope, CommandSource, EventSource, EventStream, GlobalSequence, RunState,
    API_VERSION,
};
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
            global_sequence: Arc::new(AtomicU64::new(0)),
            terminal_counters: Arc::new(Mutex::new(TerminalCounters::default())),
        }
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

        let request = RunRequest {
            run_id: task.run_id.clone(),
            session_id: task.session_id.clone(),
            provider_id: task.provider_id.clone(),
            model: task.model.clone(),
            source: task.source.clone(),
            command_id: CommandId::from(format!("retry-{}", task.run_id)),
            user_message: task.user_message.clone(),
        };
        let cancel = CancelHandle::new(
            task.run_id.clone(),
            Arc::new(agent_engine::NoopProcessTreeCleaner),
        );
        let new_state = Arc::new(Mutex::new(RunState::Created));
        let (config, queue) = self.build_config(&request);
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
        );
        let _ = self.aggregate.set_run_state(run_id, RunState::Created);
        if let Some(task) = inner.tasks.get_mut(run_id) {
            task.cancel = cancel;
            task.state = new_state;
            task.join = join;
            task.config = config;
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
            initial_messages: vec![message],
            max_iterations: 16,
            budget: agent_engine::BudgetLimits::default(),
            retry: agent_engine::RetryPolicy::default(),
            thinking: None,
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

#[allow(clippy::too_many_arguments)]
fn spawn_run_task(
    run_id: RunId,
    _session_id: SessionId,
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
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let context = Arc::new(AppLoopContext::new(
            run_id.clone(),
            Arc::clone(&approvals),
            cancel.clone(),
        ));
        let mut engine = ProviderLoop::new(provider, context, config, 1, broadcaster.clone());
        let mut subscriber = broadcaster.subscribe();
        let mut last_state = RunState::Created;
        let stream_sequence = AtomicU64::new(0);

        let engine_future = engine.run(queue, cancel.clone());
        tokio::pin!(engine_future);
        let outcome = loop {
            tokio::select! {
                event = subscriber.recv() => {
                    match event {
                        Ok(envelope) => {
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
        while let Ok(Some(envelope)) = subscriber.try_recv() {
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
        | AgentEvent::CompactionStarted { .. }
        | AgentEvent::CompactionCompleted { .. }
        | AgentEvent::CheckpointCreated { .. }
        | AgentEvent::CheckpointRolledBack { .. }
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
        | AgentEvent::RunCompleted { .. }
        | AgentEvent::RunCancelled { .. }
        | AgentEvent::RunFailed { .. }
        | AgentEvent::CompactionStarted { .. }
        | AgentEvent::CompactionCompleted { .. }
        | AgentEvent::CheckpointCreated { .. }
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
}
