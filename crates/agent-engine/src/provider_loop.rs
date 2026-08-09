//! Provider Loop（P3-3）—— Agent 循环的主干。
//!
//! 流式提交请求、解析 tool call、执行工具、回填 tool result、继续多轮，直到
//! 模型不再请求工具或达到最大迭代次数。本模块组合状态机（P3-1）、预算控制
//! （P3-6）、消息队列（P3-5）与事件广播（P3-9）。
//!
//! 工具执行与审批通过 trait 注入，既可接 `tool-runtime::ToolScheduler`（P3-4），
//! 也可在测试中用 Mock 注入，保持与调度器解耦。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use agent_domain::{
    CancellationToken, Message, MessageId, MessageMetadata, ModelId, RequestId, RunId,
};
use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};
use provider_api::{
    CanonicalModelRequest, ModelProvider, ModelResponseSummary, ProviderError, ProviderEventSink,
    ProviderStreamEvent,
};
use thiserror::Error;

use crate::appender::{AssembledTurn, ToolCallResult};
use crate::broadcast::EventBroadcaster;
use crate::budget::{BudgetController, BudgetDimension, BudgetReport};
use crate::cancel::{CancelHandle, CancelReason};
use crate::queue::MessageQueue;
use crate::retry::{RetryController, RetryDecision, RetryPolicy};
use crate::state::{EventHint, RunState, RunStateMachine, RunTransition, TransitionError};

/// Agent Loop 执行中需要的回调集合（由调用方/宿主注入）。
///
/// 所有持久化、工具执行、审批、ID 生成都经由此 trait，使 Provider Loop 与
/// SQLite/Tool Scheduler/Event Store 解耦，便于单测与替换。
#[async_trait::async_trait]
pub trait LoopContext: Send + Sync {
    /// 执行一批 tool call，返回对应结果（顺序与输入一致）。
    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        events: LoopEventEmitter,
        cancel: CancellationToken,
    ) -> Vec<ToolCallResult>;

    /// 请求用户审批一组 tool call；返回每个 call 的审批决策（顺序一致）。
    async fn request_approval(
        &self,
        calls: &[PendingToolInvocation],
        cancel: CancellationToken,
    ) -> Vec<ApprovalOutcome>;

    /// 生成新的 MessageId（保证唯一）。
    fn next_message_id(&self) -> MessageId;

    /// 生成新的 RequestId。
    fn next_request_id(&self) -> RequestId;
}

/// 待执行的一次工具调用（解析自本轮 tool call）。
#[derive(Clone, Debug)]
pub struct PendingToolInvocation {
    pub tool_call_id: agent_domain::ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// 审批结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// 放行该工具。
    Approved,
    /// 拒绝该工具（回填拒绝结果后继续循环）。
    Denied,
}

/// Provider Loop 错误。
#[derive(Debug, Error)]
pub enum LoopError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("budget exhausted: {0:?}")]
    BudgetExceeded(BudgetReport),
    #[error("illegal state transition: {0}")]
    State(#[from] TransitionError),
    #[error("run cancelled")]
    Cancelled,
    #[error("run failed: {0}")]
    Failed(String),
}

/// 单轮模型调用的产出。
#[derive(Clone, Debug)]
pub struct TurnOutcome {
    pub assistant_message: Message,
    pub tool_results: Vec<ToolCallResult>,
    pub summary: ModelResponseSummary,
    /// 该轮结束后的 Run 状态。
    pub state: RunState,
    /// 该轮结束后的预算报告。
    pub budget: BudgetReport,
}

impl TurnOutcome {
    /// 本轮是否请求了工具（循环据此决定是否继续）。
    pub fn requests_tools(&self) -> bool {
        !self.tool_results.is_empty()
    }
}

/// Provider Loop 配置。
#[derive(Clone, Debug)]
pub struct ProviderLoopConfig {
    pub session_id: agent_domain::SessionId,
    pub run_id: RunId,
    pub provider_id: agent_domain::ProviderId,
    pub model: ModelId,
    /// 工具定义（随每次请求带给 Provider）。
    pub tools: Vec<provider_api::ToolDefinition>,
    /// 初始对话历史（不含本轮触发消息）。
    pub initial_messages: Vec<Message>,
    /// 最大循环迭代次数（安全阀，防止模型无限请求工具）。
    pub max_iterations: u64,
    pub budget: crate::budget::BudgetLimits,
    pub retry: RetryPolicy,
    pub thinking: Option<provider_api::ThinkingConfig>,
}

/// Provider Loop：执行单次 Agent 循环（可含多轮工具）。
///
/// 调用 [`ProviderLoop::run`] 会流式提交、解析 tool call、审批、执行、回填并
/// 继续下一轮，直到完成、取消或预算耗尽。所有状态转换与消息落库通过
/// [`EventSink`] 回调持久化，并通过 [`EventBroadcaster`] 广播。
pub struct ProviderLoop {
    provider: Arc<dyn ModelProvider>,
    context: Arc<dyn LoopContext>,
    config: ProviderLoopConfig,
    state: RunStateMachine,
    budget: BudgetController,
    broadcaster: EventBroadcaster,
    /// 下一个事件序号（同一 Session 内严格递增）。
    next_sequence: Arc<AtomicU64>,
    /// 已提交的消息历史（每轮追加，供下一轮请求使用）。
    messages: Vec<Message>,
    started_at: Option<Instant>,
    warned_budget_dimensions: BTreeSet<BudgetDimension>,
}

impl ProviderLoop {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        context: Arc<dyn LoopContext>,
        mut config: ProviderLoopConfig,
        start_sequence: u64,
        broadcaster: EventBroadcaster,
    ) -> Self {
        let messages = config.initial_messages.clone();
        // 若 budget 未设迭代上限，用 config.max_iterations 作为安全阀，
        // 避免模型无限请求工具（与「预算控制」统一，不留两套并行死配置）。
        if config.budget.max_iterations.is_none() && config.max_iterations > 0 {
            config.budget.max_iterations = Some(config.max_iterations);
        }
        let budget = BudgetController::new(config.budget.clone());
        Self {
            provider,
            context,
            config,
            state: RunStateMachine::new(),
            budget,
            broadcaster,
            next_sequence: Arc::new(AtomicU64::new(start_sequence.max(1))),
            messages,
            started_at: None,
            warned_budget_dimensions: BTreeSet::new(),
        }
    }

    /// 当前 Run 状态。
    pub fn state(&self) -> RunState {
        self.state.state()
    }

    /// 当前消息历史快照。
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// 运行循环直到完成、取消或预算耗尽。
    pub async fn run(
        &mut self,
        queue: Arc<MessageQueue>,
        cancel: CancelHandle,
    ) -> Result<(RunState, ModelResponseSummary), LoopError> {
        self.started_at = Some(Instant::now());
        // Created → PreparingContext → WaitingForProvider
        self.transition(RunTransition::Begin)?;
        self.transition(RunTransition::ContextPrepared)?;

        loop {
            if cancel.is_cancelled() {
                self.transition(RunTransition::Cancel)?;
                cancel.cancel(CancelReason::System);
                self.emit_terminal_payload(AgentEvent::RunCancelled {
                    reason: Some("cancelled before provider request".into()),
                });
                return Err(LoopError::Cancelled);
            }

            self.update_elapsed();
            let report = self.budget.tick_iteration();
            self.emit_budget_warnings(&report);
            if report.must_stop() {
                cancel.cancel(CancelReason::Budget);
                self.transition(RunTransition::Fail)?;
                self.emit_terminal_payload(AgentEvent::RunFailed {
                    error: ProviderError::clone_for_event(&LoopError::BudgetExceeded(
                        report.clone(),
                    )),
                });
                return Err(LoopError::BudgetExceeded(report));
            }

            // 执行一轮：WaitingForProvider → StreamingResponse → (CollectingToolCalls | Completed)
            let outcome = match self.run_turn(&cancel).await {
                Ok(outcome) => outcome,
                Err(LoopError::Cancelled) => {
                    cancel.cancel(CancelReason::System);
                    self.transition(RunTransition::Cancel)?;
                    self.emit_terminal_payload(AgentEvent::RunCancelled {
                        reason: Some("cancelled during provider or tool execution".into()),
                    });
                    return Err(LoopError::Cancelled);
                }
                Err(LoopError::Provider(err))
                    if err.kind == provider_api::ProviderErrorKind::Cancelled =>
                {
                    cancel.cancel(CancelReason::System);
                    self.transition(RunTransition::Cancel)?;
                    self.emit_terminal_payload(AgentEvent::RunCancelled {
                        reason: Some("provider stream cancelled".into()),
                    });
                    return Err(LoopError::Cancelled);
                }
                Err(err) => {
                    let reason = if matches!(&err, LoopError::BudgetExceeded(_)) {
                        CancelReason::Budget
                    } else {
                        CancelReason::System
                    };
                    cancel.cancel(reason);
                    self.transition(RunTransition::Fail)?;
                    self.emit_terminal_payload(AgentEvent::RunFailed {
                        error: ProviderError::clone_for_event(&err),
                    });
                    return Err(err);
                }
            };

            // 先判断是否请求工具（借引用），再取走 summary。
            let requests_tools = outcome.requests_tools();
            let summary = outcome.summary;

            let queued = queue.drain_one().await;

            if !requests_tools {
                if let Some(queued) = queued {
                    self.transition(RunTransition::QueuedMessageAppended)?;
                    self.messages.push(queued.message.clone());
                    self.emit_message_committed(&queued.message);
                    continue;
                }
                self.transition(RunTransition::Complete)?;
                let usage = summary.usage.clone();
                self.emit_terminal_payload(AgentEvent::RunCompleted {
                    stop_reason: summary.stop_reason.clone(),
                    usage,
                });
                return Ok((self.state.state(), summary));
            }

            if let Some(queued) = queued {
                self.messages.push(queued.message.clone());
                self.emit_message_committed(&queued.message);
            }
            // 已请求工具：回填结果后进入下一轮（run_turn 内部已处理审批/执行/回填）。
        }
    }

    /// 执行单轮：提交 Provider → 收集 → 审批/执行工具 → 回填结果。
    async fn run_turn(&mut self, cancel: &CancelHandle) -> Result<TurnOutcome, LoopError> {
        // WaitingForProvider → StreamingResponse
        self.transition(RunTransition::ProviderStarted)?;

        let request = self.build_request();
        let assistant_message_id = self.context.next_message_id();
        self.emit_payload(AgentEvent::ProviderRequestStarted {
            request_id: request.request_id.clone(),
            provider_id: self.config.provider_id.clone(),
            model: self.config.model.as_str().to_string(),
        });

        let mut retry = RetryController::new(self.config.retry.clone());
        let (summary, sink) = loop {
            let sink = LoopSink::new(
                self.event_emitter(),
                assistant_message_id.clone(),
                request.request_id.clone(),
            );
            match self
                .provider
                .stream(request.clone(), &sink, cancel.token())
                .await
            {
                Ok(summary) => break (summary, sink),
                Err(err) => match retry.on_error(&err) {
                    RetryDecision::Retry {
                        attempt,
                        backoff,
                        reason,
                    } => {
                        self.emit_payload(AgentEvent::Diagnostic {
                            code: "provider_retry_attempt".into(),
                            details: serde_json::json!({
                                "attempt": attempt,
                                "reason": format!("{reason:?}"),
                                "backoff_ms": backoff.as_millis() as u64,
                                "request_id": request.request_id.as_str(),
                            }),
                        });
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = cancel.token().cancelled() => return Err(LoopError::Cancelled),
                        }
                    }
                    RetryDecision::Stop { .. } => return Err(LoopError::Provider(err)),
                },
            }
        };

        self.budget
            .record_tokens(summary.usage.input_tokens, summary.usage.output_tokens);
        let estimated_cost = model_registry::ModelRegistry::builtin()
            .estimate_cost(self.config.model.as_str(), &summary.usage);
        if let Some(cost) = &estimated_cost {
            self.budget.record_cost(cost.amount_micros);
        }
        self.check_budget()?;

        // 把流式增量累积成一条助手消息。
        let mut turn = AssembledTurn::new(assistant_message_id);
        for event in sink.drain_events() {
            turn.apply(&event);
        }
        turn.summary = Some(summary.clone());

        // 工具轮次立即进入 CollectingToolCalls；无工具轮次由 run 在检查消息队列后
        // 决定完成或继续，避免过早进入不可逆终态。
        if turn.has_tool_calls() {
            self.transition(RunTransition::StreamFinished {
                has_tool_calls: true,
            })?;
        }

        // 构建并提交助手消息。
        let metadata = MessageMetadata {
            usage: Some(summary.usage.clone()),
            stop_reason: Some(summary.stop_reason.clone()),
            provider: Some(self.config.provider_id.clone()),
            model: Some(self.config.model.clone()),
            cost: estimated_cost,
            ..MessageMetadata::default()
        };
        let assistant_message = turn.clone().into_message(metadata);
        self.messages.push(assistant_message.clone());
        self.emit_message_committed(&assistant_message);

        // 没有工具 → 返回（run 会完成）。
        if !turn.has_tool_calls() {
            return Ok(TurnOutcome {
                assistant_message,
                tool_results: Vec::new(),
                summary,
                state: self.state.state(),
                budget: self.budget.check(),
            });
        }

        // 收集待执行 tool call（保持到达顺序）。
        let invocations: Vec<PendingToolInvocation> = turn
            .tool_call_order
            .iter()
            .filter_map(|id| turn.tool_calls.get(id).map(|c| (id, c)))
            .map(|(id, c)| PendingToolInvocation {
                tool_call_id: id.clone(),
                name: c.name.clone(),
                arguments: c.arguments(),
            })
            .collect();

        // 审批：请求用户决策。
        self.transition(RunTransition::ApprovalRequested)?;
        for inv in &invocations {
            self.emit_payload(AgentEvent::ToolApprovalRequested {
                tool_call_id: inv.tool_call_id.clone(),
                reason: format!("tool `{}` requires approval", inv.name),
            });
        }
        let approvals = self
            .context
            .request_approval(&invocations, cancel.token())
            .await;

        // 按原序收集结果：拒绝的直接回填，通过的先占位，执行后回填到原位置，
        // 保证 results 与 invocations 同序（满足按序匹配 / 重放 / 审计一致性）。
        let mut results: Vec<ToolCallResult> = Vec::with_capacity(invocations.len());
        let mut approved_slots: Vec<usize> = Vec::new();
        for (inv, outcome) in invocations.iter().zip(approvals.iter()) {
            self.emit_payload(AgentEvent::ToolApprovalResponded {
                tool_call_id: inv.tool_call_id.clone(),
                decision: match outcome {
                    ApprovalOutcome::Approved => agent_events::ApprovalDecision::ApprovedOnce,
                    ApprovalOutcome::Denied => agent_events::ApprovalDecision::Denied,
                },
                comment: None,
            });
            match outcome {
                ApprovalOutcome::Approved => {
                    self.budget.record_tool_call();
                    approved_slots.push(results.len());
                    results.push(placeholder_tool_result(inv));
                }
                ApprovalOutcome::Denied => {
                    results.push(denied_tool_result(inv));
                }
            }
        }

        // 审批通过 → 执行工具（按原序），回填到占位位置以保持顺序。
        if !approved_slots.is_empty() {
            self.budget.set_concurrency(approved_slots.len() as u64);
            self.check_budget()?;
            self.transition(RunTransition::ApprovalGranted)?;
            let approved: Vec<PendingToolInvocation> = approved_slots
                .iter()
                .map(|&i| invocations[i].clone())
                .collect();
            for inv in &approved {
                self.emit_payload(AgentEvent::ToolExecutionStarted {
                    tool_call_id: inv.tool_call_id.clone(),
                });
            }
            let executed = self
                .context
                .execute_tools(approved, self.event_emitter(), cancel.token())
                .await;
            self.budget.set_concurrency(0);
            if cancel.is_cancelled() {
                return Err(LoopError::Cancelled);
            }
            for (slot, r) in approved_slots.iter().zip(executed) {
                self.emit_payload(AgentEvent::ToolExecutionCompleted {
                    tool_call_id: r.tool_call_id.clone(),
                    result: tool_result_content_view(&r),
                });
                self.budget
                    .record_output(estimate_output_bytes(&r.result.content));
                self.budget.record_artifact(
                    r.result
                        .artifacts
                        .iter()
                        .map(|artifact| artifact.byte_length)
                        .sum(),
                );
                results[*slot] = r;
            }
            self.check_budget()?;
            self.transition(RunTransition::ToolsCompleted)?;
        } else {
            // 全部拒绝时，状态从 WaitingForApproval → AppendingToolResults
            self.transition(RunTransition::ApprovalDenied)?;
        }

        // 回填 tool result 消息。
        let tool_message =
            crate::appender::tool_results_message(self.context.next_message_id(), results.clone());
        self.messages.push(tool_message.clone());
        self.emit_message_committed(&tool_message);

        // AppendingToolResults → WaitingForProvider（下一轮）
        self.transition(RunTransition::ResultsAppended)?;

        Ok(TurnOutcome {
            assistant_message,
            tool_results: results,
            summary,
            state: self.state.state(),
            budget: self.budget.check(),
        })
    }

    fn build_request(&self) -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: self.context.next_request_id(),
            model: self.config.model.clone(),
            messages: self.messages.clone(),
            tools: self.config.tools.clone(),
            tool_choice: provider_api::ToolChoice::Auto,
            thinking: self.config.thinking.clone(),
            temperature: None,
            max_output_tokens: self.config.budget.max_output_tokens,
            stop_sequences: Vec::new(),
            response_format: provider_api::ResponseFormat::Text,
            prompt_cache: provider_api::PromptCachePreference::Automatic,
            budget: provider_api::RequestBudget::default(),
            provider_options: std::collections::BTreeMap::new(),
            trace_id: None,
        }
    }

    fn transition(&mut self, t: RunTransition) -> Result<(RunState, EventHint), TransitionError> {
        let result = self.state.apply(t)?;
        // 按事件 hint 自动补发「每次转换都有事件」契约所要求的事件。
        // ProviderRequestStarted 等携带额外载荷的事件由调用点显式 emit，
        // 这里只补 RunStarted / ContextPrepared（循环此前遗漏的两个）。
        let (_state, hint) = result;
        match hint {
            EventHint::RunStarted => {
                self.emit_payload(AgentEvent::RunStarted {
                    trigger_message_id: self
                        .messages
                        .iter()
                        .rev()
                        .find(|m| m.role == agent_domain::MessageRole::User)
                        .map(|m| m.id.clone())
                        .unwrap_or_else(|| MessageId::from("trigger")),
                });
            }
            EventHint::ContextPrepared => {
                self.emit_payload(AgentEvent::ContextPrepared {
                    message_count: self.messages.len() as u64,
                    estimated_input_tokens: 0,
                });
            }
            EventHint::ProviderRequestStarted
            | EventHint::RunCompleted
            | EventHint::RunCancelled
            | EventHint::RunFailed
            | EventHint::MessageCommitted
            | EventHint::ToolApprovalRequested
            | EventHint::None => {}
        }
        Ok(result)
    }

    fn event_emitter(&self) -> LoopEventEmitter {
        LoopEventEmitter {
            session_id: self.config.session_id.clone(),
            run_id: self.config.run_id.clone(),
            broadcaster: self.broadcaster.clone(),
            next_sequence: self.next_sequence.clone(),
        }
    }

    fn update_elapsed(&mut self) {
        if let Some(started_at) = self.started_at {
            self.budget.set_elapsed(started_at.elapsed());
        }
    }

    fn check_budget(&mut self) -> Result<BudgetReport, LoopError> {
        self.update_elapsed();
        let report = self.budget.check();
        self.emit_budget_warnings(&report);
        if report.must_stop() {
            Err(LoopError::BudgetExceeded(report))
        } else {
            Ok(report)
        }
    }

    fn emit_budget_warnings(&mut self, report: &BudgetReport) {
        for dimension in &report.soft_warnings {
            if self.warned_budget_dimensions.insert(*dimension) {
                self.emit_payload(AgentEvent::Diagnostic {
                    code: "budget_soft_limit".into(),
                    details: serde_json::json!({
                        "dimension": dimension.as_str(),
                        "usage": self.budget.usage(),
                    }),
                });
            }
        }
    }

    fn next_envelope(&self, payload: AgentEvent) -> AgentEventEnvelope {
        let sequence = EventSequence::new(self.next_sequence.fetch_add(1, Ordering::SeqCst));
        AgentEventEnvelope::new(
            agent_domain::EventId::from(format!("evt-{}-{}", self.config.run_id, sequence.value())),
            self.config.session_id.clone(),
            self.config.run_id.clone(),
            sequence,
            agent_domain::Timestamp::from_unix_millis(unix_millis_now()),
            payload,
        )
    }

    fn emit_payload(&self, payload: AgentEvent) {
        let envelope = self.next_envelope(payload);
        // 广播忽略无订阅者错误（核心不应因此中断）。
        let _ = self.broadcaster.publish(envelope);
    }

    fn emit_terminal_payload(&self, payload: AgentEvent) {
        self.emit_payload(payload);
    }

    fn emit_message_committed(&self, message: &Message) {
        self.emit_payload(AgentEvent::MessageCommitted {
            message: message.clone(),
        });
    }
}

/// 把 [`ProviderError`] 转成可放入事件的 [`ErrorContext`]（去敏感细节）。
///
/// `ProviderError` 实现了 `Clone`，这里复制一份用于事件化（循环的主错误路径
/// 仍返回原始 owned 错误给调用方）。
trait ProviderErrorExt {
    fn clone_for_event(err: &LoopError) -> agent_domain::ErrorContext;
}

impl ProviderErrorExt for ProviderError {
    fn clone_for_event(err: &LoopError) -> agent_domain::ErrorContext {
        match err {
            LoopError::Provider(e) => agent_domain::ErrorContext::from(e.clone()),
            LoopError::BudgetExceeded(report) => agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::ResourceExhausted,
                message: format!("budget exceeded: {:?}", report.hard_exceeded),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
            LoopError::State(e) => agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::Internal,
                message: e.to_string(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
            LoopError::Cancelled => agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::Cancelled,
                message: "run cancelled".into(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
            LoopError::Failed(msg) => agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::Internal,
                message: msg.clone(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
        }
    }
}

/// 粗估工具输出字节数（用于预算统计）。
fn estimate_output_bytes(content: &[agent_domain::ContentPart]) -> u64 {
    let serialized = serde_json::to_string(content).unwrap_or_default();
    serialized.len() as u64
}

/// 占位结果：保持 results 与 invocations 同序，执行后由真实结果回填。
fn placeholder_tool_result(inv: &PendingToolInvocation) -> ToolCallResult {
    ToolCallResult {
        tool_call_id: inv.tool_call_id.clone(),
        tool_name: inv.name.clone(),
        arguments: inv.arguments.clone(),
        result: tool_api::ToolResult::success(Vec::new()),
    }
}

/// 构造拒绝结果（不执行工具，直接回填错误结果）。
fn denied_tool_result(inv: &PendingToolInvocation) -> ToolCallResult {
    ToolCallResult {
        tool_call_id: inv.tool_call_id.clone(),
        tool_name: inv.name.clone(),
        arguments: inv.arguments.clone(),
        result: tool_api::ToolResult::failure(agent_domain::ErrorContext {
            category: agent_domain::ErrorCategory::Authorization,
            message: "tool call denied by user".into(),
            retryable: false,
            retry_after_ms: None,
            diagnostics: Default::default(),
        }),
    }
}

/// 从 [`ToolCallResult`] 构造用于事件的可序列化视图（借用，不 move）。
fn tool_result_content_view(r: &ToolCallResult) -> agent_domain::ToolResultContent {
    agent_domain::ToolResultContent {
        tool_call_id: r.tool_call_id.clone(),
        tool_name: Some(r.tool_name.clone()),
        content: r.result.content.clone(),
        is_error: r.result.is_error(),
        metadata: r.result.metadata.clone(),
    }
}

fn unix_millis_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 可克隆的 Loop 事件发射器；Provider 与 Tool 的流式 sink 共用同一序列源。
#[derive(Clone)]
pub struct LoopEventEmitter {
    session_id: agent_domain::SessionId,
    run_id: RunId,
    broadcaster: EventBroadcaster,
    next_sequence: Arc<AtomicU64>,
}

impl LoopEventEmitter {
    fn emit(&self, payload: AgentEvent) {
        let sequence = EventSequence::new(self.next_sequence.fetch_add(1, Ordering::SeqCst));
        let envelope = AgentEventEnvelope::new(
            agent_domain::EventId::from(format!("evt-{}-{}", self.run_id, sequence.value())),
            self.session_id.clone(),
            self.run_id.clone(),
            sequence,
            agent_domain::Timestamp::from_unix_millis(unix_millis_now()),
            payload,
        );
        let _ = self.broadcaster.publish(envelope);
    }

    pub fn emit_tool_event(
        &self,
        tool_call_id: agent_domain::ToolCallId,
        event: tool_api::ToolStreamEvent,
    ) {
        match event {
            tool_api::ToolStreamEvent::OutputDelta { channel, delta } => {
                let stream = match channel {
                    tool_api::ToolOutputChannel::Stdout => agent_events::ToolOutputStream::Stdout,
                    tool_api::ToolOutputChannel::Stderr => agent_events::ToolOutputStream::Stderr,
                    tool_api::ToolOutputChannel::Structured => {
                        agent_events::ToolOutputStream::Structured
                    }
                };
                self.emit(AgentEvent::ToolOutputDelta {
                    tool_call_id,
                    stream,
                    delta,
                });
            }
            tool_api::ToolStreamEvent::Progress { .. }
            | tool_api::ToolStreamEvent::ArtifactAvailable(_) => {}
        }
    }
}

/// 内部 sink：缓存 Provider 流式事件供 loop 累积，并同步广播 canonical delta。
struct LoopSink {
    events: std::sync::Mutex<Vec<ProviderStreamEvent>>,
    emitter: LoopEventEmitter,
    message_id: MessageId,
    _request_id: RequestId,
}

impl LoopSink {
    fn new(emitter: LoopEventEmitter, message_id: MessageId, request_id: RequestId) -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
            emitter,
            message_id,
            _request_id: request_id,
        }
    }

    fn drain_events(&self) -> Vec<ProviderStreamEvent> {
        std::mem::take(&mut *self.events.lock().expect("loop sink mutex"))
    }
}

#[async_trait::async_trait]
impl ProviderEventSink for LoopSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        let payload = match &event {
            ProviderStreamEvent::TextDelta(delta) => Some(AgentEvent::AssistantTextDelta {
                message_id: self.message_id.clone(),
                delta: delta.clone(),
            }),
            ProviderStreamEvent::ThinkingDelta(delta) => Some(AgentEvent::AssistantThinkingDelta {
                message_id: self.message_id.clone(),
                delta: delta.clone(),
            }),
            ProviderStreamEvent::ToolCallStarted { id, name } => {
                Some(AgentEvent::ToolCallStarted {
                    tool_call_id: id.clone(),
                    name: name.clone(),
                })
            }
            ProviderStreamEvent::ToolCallArgumentsDelta { id, json } => {
                Some(AgentEvent::ToolCallArgumentsDelta {
                    tool_call_id: id.clone(),
                    json_delta: json.clone(),
                })
            }
            _ => None,
        };
        if let Some(payload) = payload {
            self.emitter.emit(payload);
        }
        self.events.lock().expect("loop sink mutex").push(event);
        Ok(())
    }
}

/// 将 [`tool_runtime::ToolScheduler`] 适配为 Provider Loop 的工具执行上下文。
pub struct SchedulerLoopContext {
    scheduler: Arc<tool_runtime::ToolScheduler>,
    execution_context: tool_api::ToolExecutionContext,
    approval: Arc<dyn tool_runtime::ApprovalResolver>,
    msg_counter: AtomicU64,
    req_counter: AtomicU64,
}

impl SchedulerLoopContext {
    pub fn new(
        scheduler: Arc<tool_runtime::ToolScheduler>,
        execution_context: tool_api::ToolExecutionContext,
        approval: Arc<dyn tool_runtime::ApprovalResolver>,
    ) -> Self {
        Self {
            scheduler,
            execution_context,
            approval,
            msg_counter: AtomicU64::new(0),
            req_counter: AtomicU64::new(0),
        }
    }

    pub fn execution_context(&self) -> &tool_api::ToolExecutionContext {
        &self.execution_context
    }
}

struct SchedulerToolSink {
    tool_call_id: agent_domain::ToolCallId,
    events: LoopEventEmitter,
}

#[async_trait::async_trait]
impl tool_api::ToolEventSink for SchedulerToolSink {
    async fn emit(&self, event: tool_api::ToolStreamEvent) -> Result<(), tool_api::ToolError> {
        self.events
            .emit_tool_event(self.tool_call_id.clone(), event);
        Ok(())
    }
}

#[async_trait::async_trait]
impl LoopContext for SchedulerLoopContext {
    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        events: LoopEventEmitter,
        cancel: CancellationToken,
    ) -> Vec<ToolCallResult> {
        let futures = calls.into_iter().map(|call| {
            let scheduler = self.scheduler.clone();
            let context = self.execution_context.clone();
            let approval = self.approval.clone();
            let cancel = cancel.clone();
            let events = events.clone();
            async move {
                let request = tool_api::ToolRequest {
                    tool_call_id: call.tool_call_id.clone(),
                    input: call.arguments.clone(),
                };
                let sink = SchedulerToolSink {
                    tool_call_id: call.tool_call_id.clone(),
                    events,
                };
                let result = scheduler
                    .execute_named(
                        &call.name,
                        request,
                        context,
                        cancel,
                        approval.as_ref(),
                        &sink,
                    )
                    .await
                    .unwrap_or_else(|error| {
                        tool_api::ToolResult::failure(agent_domain::ErrorContext::from(error))
                    });
                ToolCallResult {
                    tool_call_id: call.tool_call_id,
                    tool_name: call.name,
                    arguments: call.arguments,
                    result,
                }
            }
        });
        futures::future::join_all(futures).await
    }

    async fn request_approval(
        &self,
        calls: &[PendingToolInvocation],
        _cancel: CancellationToken,
    ) -> Vec<ApprovalOutcome> {
        calls.iter().map(|_| ApprovalOutcome::Approved).collect()
    }

    fn next_message_id(&self) -> MessageId {
        let value = self.msg_counter.fetch_add(1, Ordering::Relaxed);
        MessageId::from(format!("{}-message-{value}", self.execution_context.run_id))
    }

    fn next_request_id(&self) -> RequestId {
        let value = self.req_counter.fetch_add(1, Ordering::Relaxed);
        RequestId::from(format!("{}-request-{value}", self.execution_context.run_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use agent_domain::{ContentPart, TextContent};
    use agent_domain::{StopReason, TokenUsage};
    use test_support::{MockProvider, MockScript, MockTool};
    use tool_api::AgentTool;
    use tool_api::ToolResult;

    #[derive(Clone)]
    struct SequenceProvider {
        phases: Arc<Vec<Arc<MockProvider>>>,
        calls: Arc<AtomicU64>,
        requests: Arc<Mutex<Vec<CanonicalModelRequest>>>,
    }

    impl SequenceProvider {
        fn new(scripts: Vec<MockScript>) -> Self {
            Self {
                phases: Arc::new(
                    scripts
                        .into_iter()
                        .map(|script| Arc::new(MockProvider::new(script)))
                        .collect(),
                ),
                calls: Arc::new(AtomicU64::new(0)),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Vec<CanonicalModelRequest> {
            self.requests.lock().expect("requests").clone()
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for SequenceProvider {
        fn id(&self) -> agent_domain::ProviderId {
            agent_domain::ProviderId::from("sequence")
        }

        async fn list_models(
            &self,
            _credential: Option<&provider_api::ResolvedCredential>,
        ) -> Result<Vec<provider_api::ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            let index = self.calls.fetch_add(1, Ordering::SeqCst) as usize;
            let phase = self
                .phases
                .get(index)
                .or_else(|| self.phases.last())
                .expect("sequence provider requires at least one phase");
            phase.stream(request, sink, cancel).await
        }
    }

    /// 测试用 LoopContext：自动审批、直接执行内置 MockTool。
    struct TestContext {
        tools: Mutex<Vec<Arc<MockTool>>>,
        msg_counter: AtomicU64,
        req_counter: AtomicU64,
    }

    impl TestContext {
        fn new(tools: Vec<MockTool>) -> Self {
            Self {
                tools: Mutex::new(tools.into_iter().map(Arc::new).collect()),
                req_counter: AtomicU64::new(0),
                msg_counter: AtomicU64::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl LoopContext for TestContext {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            _events: LoopEventEmitter,
            _cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            let tools = self.tools.lock().expect("tools").clone();
            let mut results = Vec::new();
            for call in calls {
                let tool = tools
                    .iter()
                    .find(|t| t.descriptor().name == call.name)
                    .cloned();
                let result = if let Some(tool) = tool {
                    let req = tool_api::ToolRequest {
                        tool_call_id: call.tool_call_id.clone(),
                        input: call.arguments.clone(),
                    };
                    let ctx = tool_api::ToolExecutionContext {
                        workspace_id: agent_domain::WorkspaceId::from("ws"),
                        run_id: RunId::from("run"),
                        working_directory: None,
                    };
                    let sink = test_support::RecordingToolSink::default();
                    tool.execute(req, ctx, &sink, CancellationToken::new())
                        .await
                        .unwrap_or_else(|e| {
                            ToolResult::failure(agent_domain::ErrorContext::from(e))
                        })
                } else {
                    ToolResult::failure(agent_domain::ErrorContext {
                        category: agent_domain::ErrorCategory::NotFound,
                        message: format!("unknown tool {}", call.name),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    })
                };
                results.push(ToolCallResult {
                    tool_call_id: call.tool_call_id,
                    tool_name: call.name,
                    arguments: call.arguments,
                    result,
                });
            }
            results
        }

        async fn request_approval(
            &self,
            _calls: &[PendingToolInvocation],
            _cancel: CancellationToken,
        ) -> Vec<ApprovalOutcome> {
            // 测试默认全部放行。
            _calls.iter().map(|_| ApprovalOutcome::Approved).collect()
        }

        fn next_message_id(&self) -> MessageId {
            let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
            MessageId::from(format!("msg-{n}"))
        }

        fn next_request_id(&self) -> RequestId {
            let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
            RequestId::from(format!("req-{n}"))
        }
    }

    fn config(messages: Vec<Message>) -> ProviderLoopConfig {
        ProviderLoopConfig {
            session_id: agent_domain::SessionId::from("session-1"),
            run_id: RunId::from("run-1"),
            provider_id: agent_domain::ProviderId::from("mock"),
            model: ModelId::from("mock-model"),
            tools: Vec::new(),
            initial_messages: messages,
            max_iterations: 10,
            budget: crate::budget::BudgetLimits {
                max_iterations: Some(10),
                ..Default::default()
            },
            retry: RetryPolicy {
                initial_backoff: std::time::Duration::ZERO,
                max_backoff: std::time::Duration::ZERO,
                jitter: 0.0,
                ..RetryPolicy::default()
            },
            thinking: None,
        }
    }

    fn run_cancel() -> CancelHandle {
        CancelHandle::new(
            RunId::from("run-1"),
            Arc::new(crate::NoopProcessTreeCleaner),
        )
    }

    fn message_queue() -> Arc<MessageQueue> {
        Arc::new(MessageQueue::new())
    }

    fn user_message(text: &str) -> Message {
        Message {
            id: MessageId::from("user-1"),
            role: agent_domain::MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: text.into() })],
            metadata: MessageMetadata::default(),
        }
    }

    #[tokio::test]
    async fn mock_provider_completes_without_tools() {
        let script = MockScript::new()
            .text("Hello!")
            .usage(TokenUsage {
                input_tokens: 5,
                output_tokens: 3,
                ..Default::default()
            })
            .complete();
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(script));
        let context: Arc<dyn LoopContext> = Arc::new(TestContext::new(Vec::new()));
        let broadcaster = EventBroadcaster::new();
        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("hi")]),
            1,
            broadcaster,
        );

        let (state, summary) = engine.run(message_queue(), run_cancel()).await.unwrap();
        assert_eq!(state, RunState::Completed);
        assert_eq!(summary.stop_reason, StopReason::Completed);
        // 历史：user + assistant
        assert_eq!(engine.messages().len(), 2);
    }

    #[tokio::test]
    async fn mock_provider_completes_multi_turn_tool_loop() {
        // 第一轮请求工具，第二轮无工具直接完成。
        // MockProvider 每次 stream 调用重放同一脚本；用两阶段 provider 区分两轮。
        let first = MockScript::new()
            .tool_call("echo", serde_json::json!({"text": "hi"}))
            .usage(TokenUsage {
                input_tokens: 10,
                output_tokens: 2,
                ..Default::default()
            })
            .complete_with(StopReason::ToolUse);
        let tool = MockTool::new(
            "echo",
            ToolResult::success(vec![ContentPart::Text(TextContent { text: "hi".into() })]),
        );
        let tool = Arc::new(tool);

        // 两阶段 provider：第一次调用产工具，第二次产纯文本。
        #[derive(Clone)]
        struct TwoPhase {
            first: Arc<MockProvider>,
            second: Arc<MockProvider>,
            calls: Arc<std::sync::atomic::AtomicU64>,
        }
        #[async_trait::async_trait]
        impl ModelProvider for TwoPhase {
            fn id(&self) -> agent_domain::ProviderId {
                self.first.id()
            }
            async fn list_models(
                &self,
                cred: Option<&provider_api::ResolvedCredential>,
            ) -> Result<Vec<provider_api::ModelDefinition>, ProviderError> {
                self.first.list_models(cred).await
            }
            async fn stream(
                &self,
                request: CanonicalModelRequest,
                sink: &dyn ProviderEventSink,
                cancel: CancellationToken,
            ) -> Result<ModelResponseSummary, ProviderError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    self.first.stream(request, sink, cancel).await
                } else {
                    self.second.stream(request, sink, cancel).await
                }
            }
        }
        let provider: Arc<dyn ModelProvider> = Arc::new(TwoPhase {
            first: Arc::new(MockProvider::new(first)),
            second: Arc::new(MockProvider::new(
                MockScript::new()
                    .text("done")
                    .usage(TokenUsage {
                        input_tokens: 20,
                        output_tokens: 1,
                        ..Default::default()
                    })
                    .complete(),
            )),
            calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        });

        // TestContext 需要在两轮间共享 tool。
        struct SharedToolContext {
            tool: Arc<MockTool>,
            msg_counter: AtomicU64,
            req_counter: AtomicU64,
        }
        #[async_trait::async_trait]
        impl LoopContext for SharedToolContext {
            async fn execute_tools(
                &self,
                calls: Vec<PendingToolInvocation>,
                _events: LoopEventEmitter,
                cancel: CancellationToken,
            ) -> Vec<ToolCallResult> {
                let mut results = Vec::new();
                for call in calls {
                    let req = tool_api::ToolRequest {
                        tool_call_id: call.tool_call_id.clone(),
                        input: call.arguments.clone(),
                    };
                    let ctx = tool_api::ToolExecutionContext {
                        workspace_id: agent_domain::WorkspaceId::from("ws"),
                        run_id: RunId::from("run"),
                        working_directory: None,
                    };
                    let sink = test_support::RecordingToolSink::default();
                    let result = self
                        .tool
                        .execute(req, ctx, &sink, cancel.clone())
                        .await
                        .unwrap_or_else(|e| {
                            ToolResult::failure(agent_domain::ErrorContext::from(e))
                        });
                    results.push(ToolCallResult {
                        tool_call_id: call.tool_call_id,
                        tool_name: call.name,
                        arguments: call.arguments,
                        result,
                    });
                }
                results
            }
            async fn request_approval(
                &self,
                calls: &[PendingToolInvocation],
                _cancel: CancellationToken,
            ) -> Vec<ApprovalOutcome> {
                calls.iter().map(|_| ApprovalOutcome::Approved).collect()
            }
            fn next_message_id(&self) -> MessageId {
                let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
                MessageId::from(format!("msg-{n}"))
            }
            fn next_request_id(&self) -> RequestId {
                let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
                RequestId::from(format!("req-{n}"))
            }
        }
        let context: Arc<dyn LoopContext> = Arc::new(SharedToolContext {
            tool,
            msg_counter: AtomicU64::new(0),
            req_counter: AtomicU64::new(0),
        });

        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("echo")]),
            1,
            EventBroadcaster::new(),
        );
        let (state, summary) = engine.run(message_queue(), run_cancel()).await.unwrap();
        assert_eq!(state, RunState::Completed);
        assert_eq!(summary.stop_reason, StopReason::Completed);
        // 历史：user + assistant(tool call) + tool result + assistant(text) = 4
        assert_eq!(engine.messages().len(), 4);
    }

    #[tokio::test]
    async fn cancelled_run_emits_cancelled_and_returns_error() {
        let provider: Arc<dyn ModelProvider> =
            Arc::new(MockProvider::new(MockScript::new().wait_for_cancellation()));
        let context: Arc<dyn LoopContext> = Arc::new(TestContext::new(Vec::new()));
        let cancel = run_cancel();
        cancel.cancel(CancelReason::User);
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("x")]),
            1,
            broadcaster,
        );

        let result = engine.run(message_queue(), cancel).await;
        assert!(matches!(result, Err(LoopError::Cancelled)));
        assert_eq!(engine.state(), RunState::Cancelled);
        let mut saw_cancelled = false;
        while let Ok(Some(event)) = sub.try_recv() {
            saw_cancelled |= matches!(event.payload, AgentEvent::RunCancelled { .. });
        }
        assert!(saw_cancelled, "取消路径必须广播 RunCancelled");
    }

    #[tokio::test]
    async fn streaming_cancel_runs_process_cleanup_and_emits_terminal_event() {
        struct Cleaner(Arc<AtomicU64>);
        impl crate::ProcessTreeCleaner for Cleaner {
            fn cleanup(&self, run_id: &RunId) -> usize {
                assert_eq!(run_id.as_str(), "run-1");
                self.0.fetch_add(1, Ordering::SeqCst);
                1
            }
        }

        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let cleaned = Arc::new(AtomicU64::new(0));
        let cancel = CancelHandle::new(RunId::from("run-1"), Arc::new(Cleaner(cleaned.clone())));
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            trigger.cancel(CancelReason::User);
        });
        let mut engine = ProviderLoop::new(
            Arc::new(MockProvider::new(MockScript::new().wait_for_cancellation())),
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("cancel")]),
            1,
            broadcaster,
        );

        assert!(matches!(
            engine.run(message_queue(), cancel).await,
            Err(LoopError::Cancelled)
        ));
        assert_eq!(cleaned.load(Ordering::SeqCst), 1);
        let mut terminal = false;
        while let Ok(Some(event)) = sub.try_recv() {
            terminal |= matches!(event.payload, AgentEvent::RunCancelled { .. });
        }
        assert!(terminal);
    }

    /// 验证混合审批（A 通过、B 拒绝、C 通过）下 tool result 仍按原序排列。
    #[tokio::test]
    async fn mixed_approval_preserves_result_order() {
        // 单轮发三个 tool call，按 B 拒绝、其余通过审批。
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(
            MockScript::new()
                .tool_call("a", serde_json::json!({}))
                .tool_call("b", serde_json::json!({}))
                .tool_call("c", serde_json::json!({}))
                .complete_with(StopReason::ToolUse),
        ));
        let tool_a = MockTool::new(
            "a",
            ToolResult::success(vec![ContentPart::Text(TextContent { text: "A".into() })]),
        );
        let tool_c = MockTool::new(
            "c",
            ToolResult::success(vec![ContentPart::Text(TextContent { text: "C".into() })]),
        );

        struct SelectiveApproval {
            deny: &'static str,
            tools: Arc<Mutex<Vec<Arc<MockTool>>>>,
            msg_counter: AtomicU64,
            req_counter: AtomicU64,
        }
        #[async_trait::async_trait]
        impl LoopContext for SelectiveApproval {
            async fn execute_tools(
                &self,
                calls: Vec<PendingToolInvocation>,
                _events: LoopEventEmitter,
                _cancel: CancellationToken,
            ) -> Vec<ToolCallResult> {
                let tools = self.tools.lock().expect("tools").clone();
                let mut out = Vec::new();
                for call in calls {
                    let result =
                        if let Some(t) = tools.iter().find(|t| t.descriptor().name == call.name) {
                            t.execute(
                                tool_api::ToolRequest {
                                    tool_call_id: call.tool_call_id.clone(),
                                    input: call.arguments.clone(),
                                },
                                tool_api::ToolExecutionContext {
                                    workspace_id: agent_domain::WorkspaceId::from("ws"),
                                    run_id: RunId::from("run"),
                                    working_directory: None,
                                },
                                &test_support::RecordingToolSink::default(),
                                CancellationToken::new(),
                            )
                            .await
                            .unwrap_or_else(|e| {
                                ToolResult::failure(agent_domain::ErrorContext::from(e))
                            })
                        } else {
                            ToolResult::failure(agent_domain::ErrorContext {
                                category: agent_domain::ErrorCategory::NotFound,
                                message: format!("unknown tool {}", call.name),
                                retryable: false,
                                retry_after_ms: None,
                                diagnostics: Default::default(),
                            })
                        };
                    out.push(ToolCallResult {
                        tool_call_id: call.tool_call_id,
                        tool_name: call.name,
                        arguments: call.arguments,
                        result,
                    });
                }
                out
            }
            async fn request_approval(
                &self,
                calls: &[PendingToolInvocation],
                _cancel: CancellationToken,
            ) -> Vec<ApprovalOutcome> {
                calls
                    .iter()
                    .map(|c| {
                        if c.name == self.deny {
                            ApprovalOutcome::Denied
                        } else {
                            ApprovalOutcome::Approved
                        }
                    })
                    .collect()
            }
            fn next_message_id(&self) -> MessageId {
                let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
                MessageId::from(format!("msg-{n}"))
            }
            fn next_request_id(&self) -> RequestId {
                let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
                RequestId::from(format!("req-{n}"))
            }
        }
        let context: Arc<dyn LoopContext> = Arc::new(SelectiveApproval {
            deny: "b",
            tools: Arc::new(Mutex::new(vec![Arc::new(tool_a), Arc::new(tool_c)])),
            msg_counter: AtomicU64::new(0),
            req_counter: AtomicU64::new(0),
        });
        let mut cfg = config(vec![user_message("go")]);
        // 预算=2：第 1 轮执行工具，第 2 轮触发预算停止（确保工具已执行并回填）。
        cfg.budget.max_iterations = Some(2);
        let mut engine = ProviderLoop::new(provider, context, cfg, 1, EventBroadcaster::new());
        // 第一轮：三个工具，B 被拒；预算=1 让循环停下。
        let _ = engine.run(message_queue(), run_cancel()).await;

        // 取回填的 Tool 消息（最后一条），其 content 应含三条 tool result，且按 a,b,c 序。
        let tool_msg = engine
            .messages()
            .iter()
            .rev()
            .find(|m| m.role == agent_domain::MessageRole::Tool)
            .expect("应有 Tool 角色消息");
        let results: Vec<&agent_domain::ContentPart> = tool_msg
            .content
            .iter()
            .filter(|p| matches!(p, agent_domain::ContentPart::ToolResult(_)))
            .collect();
        assert_eq!(results.len(), 3, "应回填三条 tool result");
        // 中间那条（b）应为错误（被拒）。
        if let agent_domain::ContentPart::ToolResult(tr) = results[1] {
            assert!(tr.is_error, "被拒工具 b 的结果应为错误");
        } else {
            panic!("第二条应为 ToolResult");
        }
        // 第一条与第三条应非错误。
        for (idx, part) in results.iter().enumerate() {
            if let agent_domain::ContentPart::ToolResult(tr) = part {
                let expected_error = idx == 1;
                assert_eq!(tr.is_error, expected_error, "第 {idx} 条 is_error 不符预期");
            }
        }
    }

    /// 验证 RunStarted / ContextPrepared 事件被广播（修复「每次转换都有事件」契约）。
    #[tokio::test]
    async fn run_started_and_context_prepared_are_broadcast() {
        let provider: Arc<dyn ModelProvider> =
            Arc::new(MockProvider::new(MockScript::new().text("ok").complete()));
        let context: Arc<dyn LoopContext> = Arc::new(TestContext::new(Vec::new()));
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("hi")]),
            1,
            broadcaster,
        );
        let _ = engine.run(message_queue(), run_cancel()).await.unwrap();

        let mut saw_run_started = false;
        let mut saw_context_prepared = false;
        for _ in 0..32 {
            match tokio::time::timeout(std::time::Duration::from_millis(50), sub.recv()).await {
                Ok(Ok(env)) => match env.payload {
                    AgentEvent::RunStarted { .. } => saw_run_started = true,
                    AgentEvent::ContextPrepared { .. } => saw_context_prepared = true,
                    _ => {}
                },
                _ => break,
            }
        }
        assert!(saw_run_started, "应广播 RunStarted 事件");
        assert!(saw_context_prepared, "应广播 ContextPrepared 事件");
    }

    #[tokio::test]
    async fn provider_deltas_are_broadcast_while_streaming() {
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(
            MockScript::new().thinking("plan").text("answer").complete(),
        ));
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            provider,
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("hi")]),
            1,
            broadcaster,
        );

        engine.run(message_queue(), run_cancel()).await.unwrap();
        let mut text = false;
        let mut thinking = false;
        while let Ok(Some(event)) = sub.try_recv() {
            match event.payload {
                AgentEvent::AssistantTextDelta { delta, .. } if delta == "answer" => text = true,
                AgentEvent::AssistantThinkingDelta { delta, .. } if delta == "plan" => {
                    thinking = true
                }
                _ => {}
            }
        }
        assert!(text && thinking, "文本与 thinking delta 都应实时广播");
    }

    #[tokio::test]
    async fn loop_scheduler_bridge_serializes_capability_and_streams_tool_output() {
        struct SchedulerProbeTool {
            name: &'static str,
            current: Arc<AtomicU64>,
            peak: Arc<AtomicU64>,
            contexts: Arc<Mutex<Vec<tool_api::ToolExecutionContext>>>,
        }

        #[async_trait::async_trait]
        impl AgentTool for SchedulerProbeTool {
            fn descriptor(&self) -> tool_api::ToolDescriptor {
                tool_api::ToolDescriptor {
                    name: self.name.into(),
                    description: "scheduler bridge probe".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    capability: tool_api::ToolCapability::WorkspaceWrite,
                    read_only: false,
                    supports_concurrency: false,
                    default_timeout_ms: Some(1_000),
                    max_output_bytes: 1024,
                    allowed_in_untrusted_workspace: true,
                }
            }

            async fn execute(
                &self,
                _request: tool_api::ToolRequest,
                context: tool_api::ToolExecutionContext,
                sink: &dyn tool_api::ToolEventSink,
                _cancel: CancellationToken,
            ) -> Result<ToolResult, tool_api::ToolError> {
                self.contexts.lock().expect("contexts").push(context);
                let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(current, Ordering::SeqCst);
                sink.emit(tool_api::ToolStreamEvent::OutputDelta {
                    channel: tool_api::ToolOutputChannel::Stdout,
                    delta: self.name.into(),
                })
                .await?;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                self.current.fetch_sub(1, Ordering::SeqCst);
                Ok(ToolResult::success(Vec::new()))
            }
        }

        let current = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        let contexts = Arc::new(Mutex::new(Vec::new()));
        let tools: Vec<Arc<dyn AgentTool>> = ["write_a", "write_b"]
            .into_iter()
            .map(|name| {
                Arc::new(SchedulerProbeTool {
                    name,
                    current: current.clone(),
                    peak: peak.clone(),
                    contexts: contexts.clone(),
                }) as Arc<dyn AgentTool>
            })
            .collect();
        let mut registry = tool_runtime::ToolRegistry::new();
        registry.extend(tools);
        let scheduler = Arc::new(tool_runtime::ToolScheduler::new(
            registry,
            tool_runtime::ToolSchedulerConfig {
                max_concurrent: 2,
                approval_mode: tool_runtime::ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        ));
        let execution_context = tool_api::ToolExecutionContext {
            workspace_id: agent_domain::WorkspaceId::from("workspace-e2e"),
            run_id: RunId::from("run-e2e"),
            working_directory: Some("repo".into()),
        };
        let context: Arc<dyn LoopContext> = Arc::new(SchedulerLoopContext::new(
            scheduler,
            execution_context.clone(),
            Arc::new(tool_runtime::AutoApproveResolver),
        ));
        let provider = SequenceProvider::new(vec![
            MockScript::new()
                .tool_call("write_a", serde_json::json!({"path": "a"}))
                .tool_call("write_b", serde_json::json!({"path": "b"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]);
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("write")]);
        cfg.run_id = RunId::from("run-e2e");
        let mut engine = ProviderLoop::new(Arc::new(provider), context, cfg, 1, broadcaster);
        let cancel = CancelHandle::new(
            RunId::from("run-e2e"),
            Arc::new(crate::NoopProcessTreeCleaner),
        );

        engine.run(message_queue(), cancel).await.unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), 1, "WorkspaceWrite 必须串行");
        let seen = contexts.lock().expect("contexts");
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|context| context == &execution_context));
        drop(seen);
        let mut tool_deltas = 0;
        let mut tool_started = 0;
        let mut argument_deltas = 0;
        while let Ok(Some(event)) = sub.try_recv() {
            match event.payload {
                AgentEvent::ToolOutputDelta { .. } => tool_deltas += 1,
                AgentEvent::ToolCallStarted { .. } => tool_started += 1,
                AgentEvent::ToolCallArgumentsDelta { .. } => argument_deltas += 1,
                _ => {}
            }
        }
        assert_eq!(tool_deltas, 2);
        assert_eq!(tool_started, 2);
        assert_eq!(argument_deltas, 2);
    }

    #[tokio::test]
    async fn scheduler_loop_context_uses_explicit_policy_resolver_once() {
        struct PolicyProbe {
            capability: tool_api::ToolCapability,
            calls: Arc<AtomicU64>,
        }

        #[async_trait::async_trait]
        impl AgentTool for PolicyProbe {
            fn descriptor(&self) -> tool_api::ToolDescriptor {
                tool_api::ToolDescriptor {
                    name: "policy_probe".into(),
                    description: "policy bridge probe".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    capability: self.capability.clone(),
                    read_only: false,
                    supports_concurrency: false,
                    default_timeout_ms: None,
                    max_output_bytes: 1024,
                    allowed_in_untrusted_workspace: false,
                }
            }

            async fn execute(
                &self,
                _request: tool_api::ToolRequest,
                _context: tool_api::ToolExecutionContext,
                _sink: &dyn tool_api::ToolEventSink,
                _cancel: CancellationToken,
            ) -> Result<ToolResult, tool_api::ToolError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult::success(Vec::new()))
            }
        }

        struct ExplicitResolver {
            outcome: tool_runtime::ApprovalOutcome,
            calls: Arc<AtomicU64>,
        }

        #[async_trait::async_trait]
        impl tool_runtime::ApprovalResolver for ExplicitResolver {
            async fn resolve(
                &self,
                requests: &[tool_api::ToolRequest],
            ) -> Vec<tool_runtime::ApprovalOutcome> {
                self.calls
                    .fetch_add(requests.len() as u64, Ordering::SeqCst);
                requests.iter().map(|_| self.outcome).collect()
            }
        }

        async fn run_case(
            mode: tool_runtime::ApprovalMode,
            capability: tool_api::ToolCapability,
            input: serde_json::Value,
            outcome: tool_runtime::ApprovalOutcome,
        ) -> (u64, u64) {
            let tool_calls = Arc::new(AtomicU64::new(0));
            let approval_calls = Arc::new(AtomicU64::new(0));
            let mut registry = tool_runtime::ToolRegistry::new();
            registry.register(Arc::new(PolicyProbe {
                capability,
                calls: tool_calls.clone(),
            }));
            let scheduler = Arc::new(tool_runtime::ToolScheduler::new(
                registry,
                tool_runtime::ToolSchedulerConfig {
                    max_concurrent: 1,
                    approval_mode: mode,
                    workspace_trusted: true,
                },
            ));
            let context: Arc<dyn LoopContext> = Arc::new(SchedulerLoopContext::new(
                scheduler,
                tool_api::ToolExecutionContext {
                    workspace_id: agent_domain::WorkspaceId::from("workspace-policy"),
                    run_id: RunId::from("run-policy"),
                    working_directory: None,
                },
                Arc::new(ExplicitResolver {
                    outcome,
                    calls: approval_calls.clone(),
                }),
            ));
            let provider = SequenceProvider::new(vec![
                MockScript::new()
                    .tool_call("policy_probe", input)
                    .complete_with(StopReason::ToolUse),
                MockScript::new().text("done").complete(),
            ]);
            let mut cfg = config(vec![user_message("policy")]);
            cfg.run_id = RunId::from("run-policy");
            let mut engine =
                ProviderLoop::new(Arc::new(provider), context, cfg, 1, EventBroadcaster::new());
            engine
                .run(
                    message_queue(),
                    CancelHandle::new(
                        RunId::from("run-policy"),
                        Arc::new(crate::NoopProcessTreeCleaner),
                    ),
                )
                .await
                .expect("provider loop");
            (
                tool_calls.load(Ordering::SeqCst),
                approval_calls.load(Ordering::SeqCst),
            )
        }

        assert_eq!(
            run_case(
                tool_runtime::ApprovalMode::AskForWrites,
                tool_api::ToolCapability::WorkspaceWrite,
                serde_json::json!({"path": "a.txt"}),
                tool_runtime::ApprovalOutcome::Denied,
            )
            .await,
            (0, 1),
            "明确拒绝不得执行"
        );
        assert_eq!(
            run_case(
                tool_runtime::ApprovalMode::AskForWrites,
                tool_api::ToolCapability::WorkspaceWrite,
                serde_json::json!({"path": "a.txt"}),
                tool_runtime::ApprovalOutcome::Approved,
            )
            .await,
            (1, 1),
            "明确批准应只提示一次并执行"
        );
        assert_eq!(
            run_case(
                tool_runtime::ApprovalMode::NeverAsk,
                tool_api::ToolCapability::Process,
                serde_json::json!({"command": "rm", "args": ["-rf", "/"]}),
                tool_runtime::ApprovalOutcome::Approved,
            )
            .await,
            (0, 0),
            "灾难命令地板应在 resolver 前直接拒绝"
        );
    }

    #[tokio::test]
    async fn interrupted_stream_retries_with_unchanged_messages() {
        let mut interrupted = ProviderError::new(
            provider_api::ProviderErrorKind::StreamInterrupted,
            "connection reset",
        );
        interrupted.retryable = true;
        interrupted.retry_after_ms = Some(0);
        let provider = SequenceProvider::new(vec![
            MockScript::new().text("partial").fail(interrupted),
            MockScript::new().text("final").complete(),
        ]);
        let provider_view = provider.clone();
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            Arc::new(provider),
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("retry")]),
            1,
            broadcaster,
        );

        engine.run(message_queue(), run_cancel()).await.unwrap();
        let requests = provider_view.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].messages, requests[1].messages);
        assert_eq!(requests[0].request_id, requests[1].request_id);
        assert!(engine.messages()[1]
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::Text(text) if text.text == "final")));
        let mut retry_diagnostic = false;
        while let Ok(Some(event)) = sub.try_recv() {
            retry_diagnostic |= matches!(
                event.payload,
                AgentEvent::Diagnostic { ref code, .. } if code == "provider_retry_attempt"
            );
        }
        assert!(retry_diagnostic, "每次重试必须产生 Diagnostic");
    }

    #[tokio::test]
    async fn queued_message_is_consumed_before_follow_up_turn() {
        let provider = SequenceProvider::new(vec![
            MockScript::new().text("first").complete(),
            MockScript::new().text("second").complete(),
        ]);
        let provider_view = provider.clone();
        let queue = message_queue();
        let queued = Message {
            id: MessageId::from("queued-user"),
            role: agent_domain::MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "follow up".into(),
            })],
            metadata: MessageMetadata::default(),
        };
        queue.enqueue(queued).await;
        let mut engine = ProviderLoop::new(
            Arc::new(provider),
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("first")]),
            1,
            EventBroadcaster::new(),
        );

        engine.run(queue, run_cancel()).await.unwrap();
        let requests = provider_view.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1]
            .messages
            .iter()
            .any(|message| message.id.as_str() == "queued-user"));
    }

    #[tokio::test]
    async fn budget_soft_warning_is_emitted_once_and_hard_limit_fails_terminally() {
        let provider = SequenceProvider::new(vec![MockScript::new().text("ok").complete()]);
        let queue = message_queue();
        for index in 0..3 {
            queue
                .enqueue(Message {
                    id: MessageId::from(format!("queued-{index}")),
                    role: agent_domain::MessageRole::User,
                    content: vec![ContentPart::Text(TextContent { text: "x".into() })],
                    metadata: MessageMetadata::default(),
                })
                .await;
        }
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("start")]);
        cfg.budget.max_iterations = Some(5);
        let mut engine = ProviderLoop::new(
            Arc::new(provider),
            Arc::new(TestContext::new(Vec::new())),
            cfg,
            1,
            broadcaster,
        );
        engine.run(queue, run_cancel()).await.unwrap();
        let mut warnings = 0;
        while let Ok(Some(event)) = sub.try_recv() {
            if matches!(event.payload, AgentEvent::Diagnostic { ref code, .. } if code == "budget_soft_limit")
            {
                warnings += 1;
            }
        }
        assert_eq!(warnings, 1, "同一预算维度只警告一次");

        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("hard")]);
        cfg.budget.max_iterations = Some(1);
        let mut engine = ProviderLoop::new(
            Arc::new(MockProvider::new(MockScript::new().complete())),
            Arc::new(TestContext::new(Vec::new())),
            cfg,
            1,
            broadcaster,
        );
        assert!(matches!(
            engine.run(message_queue(), run_cancel()).await,
            Err(LoopError::BudgetExceeded(_))
        ));
        let mut failed = false;
        while let Ok(Some(event)) = sub.try_recv() {
            failed |= matches!(event.payload, AgentEvent::RunFailed { .. });
        }
        assert!(failed, "预算硬上限必须广播 RunFailed");
    }

    #[tokio::test]
    async fn loop_records_cost_duration_concurrency_and_artifact_budgets() {
        let mut cost_cfg = config(vec![user_message("cost")]);
        cost_cfg.model = ModelId::from("gpt-4o");
        cost_cfg.budget.max_cost_micros = Some(1);
        let mut cost_engine = ProviderLoop::new(
            Arc::new(MockProvider::new(
                MockScript::new()
                    .usage(TokenUsage {
                        input_tokens: 1_000_000,
                        ..TokenUsage::default()
                    })
                    .complete(),
            )),
            Arc::new(TestContext::new(Vec::new())),
            cost_cfg,
            1,
            EventBroadcaster::new(),
        );
        let cost_error = cost_engine
            .run(message_queue(), run_cancel())
            .await
            .unwrap_err();
        assert!(matches!(
            cost_error,
            LoopError::BudgetExceeded(ref report)
                if report.hard_exceeded.contains(&BudgetDimension::Cost)
        ));

        #[derive(Clone)]
        struct DelayedProvider(Arc<MockProvider>);
        #[async_trait::async_trait]
        impl ModelProvider for DelayedProvider {
            fn id(&self) -> agent_domain::ProviderId {
                self.0.id()
            }
            async fn list_models(
                &self,
                credential: Option<&provider_api::ResolvedCredential>,
            ) -> Result<Vec<provider_api::ModelDefinition>, ProviderError> {
                self.0.list_models(credential).await
            }
            async fn stream(
                &self,
                request: CanonicalModelRequest,
                sink: &dyn ProviderEventSink,
                cancel: CancellationToken,
            ) -> Result<ModelResponseSummary, ProviderError> {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                self.0.stream(request, sink, cancel).await
            }
        }
        let mut duration_cfg = config(vec![user_message("duration")]);
        duration_cfg.budget.max_duration_ms = Some(1);
        let mut duration_engine = ProviderLoop::new(
            Arc::new(DelayedProvider(Arc::new(MockProvider::new(
                MockScript::new().complete(),
            )))),
            Arc::new(TestContext::new(Vec::new())),
            duration_cfg,
            1,
            EventBroadcaster::new(),
        );
        let duration_error = duration_engine
            .run(message_queue(), run_cancel())
            .await
            .unwrap_err();
        assert!(matches!(
            duration_error,
            LoopError::BudgetExceeded(ref report)
                if report.hard_exceeded.contains(&BudgetDimension::Duration)
        ));

        let mut concurrency_cfg = config(vec![user_message("concurrency")]);
        concurrency_cfg.budget.max_concurrency = Some(1);
        let mut concurrency_engine = ProviderLoop::new(
            Arc::new(MockProvider::new(
                MockScript::new()
                    .tool_call("unknown", serde_json::json!({}))
                    .complete_with(StopReason::ToolUse),
            )),
            Arc::new(TestContext::new(Vec::new())),
            concurrency_cfg,
            1,
            EventBroadcaster::new(),
        );
        let concurrency_error = concurrency_engine
            .run(message_queue(), run_cancel())
            .await
            .unwrap_err();
        assert!(matches!(
            concurrency_error,
            LoopError::BudgetExceeded(ref report)
                if report.hard_exceeded.contains(&BudgetDimension::Concurrency)
        ));

        let mut artifact_result = ToolResult::success(Vec::new());
        artifact_result
            .artifacts
            .push(agent_domain::ArtifactReference {
                id: agent_domain::ArtifactId::from("artifact-1"),
                media_type: "application/octet-stream".into(),
                byte_length: 10,
                content_hash: None,
                label: None,
            });
        let artifact_provider = SequenceProvider::new(vec![
            MockScript::new()
                .tool_call("artifact", serde_json::json!({}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().complete(),
        ]);
        let mut artifact_cfg = config(vec![user_message("artifact")]);
        artifact_cfg.budget.max_artifact_bytes = Some(10);
        let mut artifact_engine = ProviderLoop::new(
            Arc::new(artifact_provider),
            Arc::new(TestContext::new(vec![MockTool::new(
                "artifact",
                artifact_result,
            )])),
            artifact_cfg,
            1,
            EventBroadcaster::new(),
        );
        let artifact_error = artifact_engine
            .run(message_queue(), run_cancel())
            .await
            .unwrap_err();
        assert!(matches!(
            artifact_error,
            LoopError::BudgetExceeded(ref report)
                if report.hard_exceeded.contains(&BudgetDimension::ArtifactBytes)
        ));
    }

    #[tokio::test]
    async fn non_retryable_stream_error_emits_run_failed() {
        let error = ProviderError::new(provider_api::ProviderErrorKind::InvalidRequest, "bad");
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            Arc::new(MockProvider::new(MockScript::new().fail(error))),
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("bad")]),
            1,
            broadcaster,
        );
        assert!(matches!(
            engine.run(message_queue(), run_cancel()).await,
            Err(LoopError::Provider(_))
        ));
        let mut failed = false;
        while let Ok(Some(event)) = sub.try_recv() {
            failed |= matches!(event.payload, AgentEvent::RunFailed { .. });
        }
        assert!(failed);
    }
}
