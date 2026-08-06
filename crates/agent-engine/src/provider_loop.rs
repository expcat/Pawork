//! Provider Loop（P3-3）—— Agent 循环的主干。
//!
//! 流式提交请求、解析 tool call、执行工具、回填 tool result、继续多轮，直到
//! 模型不再请求工具或达到最大迭代次数。本模块组合状态机（P3-1）、预算控制
//! （P3-6）、消息队列（P3-5）与事件广播（P3-9）。
//!
//! 工具执行与审批通过 trait 注入，既可接 `tool-runtime::ToolScheduler`（P3-4），
//! 也可在测试中用 Mock 注入，保持与调度器解耦。

use std::sync::Arc;

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
use crate::budget::{BudgetController, BudgetReport};
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
    next_sequence: u64,
    /// 已提交的消息历史（每轮追加，供下一轮请求使用）。
    messages: Vec<Message>,
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
            next_sequence: start_sequence.max(1),
            messages,
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
        cancel: CancellationToken,
    ) -> Result<(RunState, ModelResponseSummary), LoopError> {
        // Created → PreparingContext → WaitingForProvider
        self.transition(RunTransition::Begin)?;
        self.transition(RunTransition::ContextPrepared)?;

        loop {
            if cancel.is_cancelled() {
                self.transition(RunTransition::Cancel)?;
                return Err(LoopError::Cancelled);
            }

            let report = self.budget.tick_iteration();
            if report.must_stop() {
                self.transition(RunTransition::Fail)?;
                return Err(LoopError::BudgetExceeded(report));
            }

            // 执行一轮：WaitingForProvider → StreamingResponse → (CollectingToolCalls | Completed)
            let outcome = match self.run_turn(&cancel).await {
                Ok(outcome) => outcome,
                Err(LoopError::Cancelled) => {
                    self.transition(RunTransition::Cancel)?;
                    return Err(LoopError::Cancelled);
                }
                Err(LoopError::Provider(err))
                    if err.kind == provider_api::ProviderErrorKind::Cancelled =>
                {
                    self.transition(RunTransition::Cancel)?;
                    return Err(LoopError::Cancelled);
                }
                Err(err) => {
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

            if !requests_tools {
                // 无工具请求 → 完成。StreamFinished 已将状态推进到 Completed，
                // 这里仅幂等地补发 Complete（若尚未终态）。
                if !self.state.state().is_terminal() {
                    self.transition(RunTransition::Complete)?;
                }
                let usage = summary.usage.clone();
                self.emit_terminal_payload(AgentEvent::RunCompleted {
                    stop_reason: summary.stop_reason.clone(),
                    usage,
                });
                return Ok((self.state.state(), summary));
            }

            // 已请求工具：回填结果后进入下一轮（run_turn 内部已处理审批/执行/回填）。
        }
    }

    /// 执行单轮：提交 Provider → 收集 → 审批/执行工具 → 回填结果。
    async fn run_turn(&mut self, cancel: &CancellationToken) -> Result<TurnOutcome, LoopError> {
        // WaitingForProvider → StreamingResponse
        self.transition(RunTransition::ProviderStarted)?;

        let request = self.build_request();
        let sink = LoopSink::new(
            self.config.session_id.clone(),
            self.config.run_id.clone(),
            request.request_id.clone(),
        );
        self.emit_payload(AgentEvent::ProviderRequestStarted {
            request_id: request.request_id.clone(),
            provider_id: self.config.provider_id.clone(),
            model: self.config.model.as_str().to_string(),
        });

        let summary = self.provider.stream(request, &sink, cancel.clone()).await?;

        self.budget
            .record_tokens(summary.usage.input_tokens, summary.usage.output_tokens);

        // 把流式增量累积成一条助手消息。
        let mut turn = AssembledTurn::new(self.context.next_message_id());
        for event in sink.events() {
            turn.apply(&event);
        }
        turn.summary = Some(summary.clone());

        // StreamingResponse → CollectingToolCalls（有工具）或 Completed（无）
        self.transition(RunTransition::StreamFinished {
            has_tool_calls: turn.has_tool_calls(),
        })?;

        // 构建并提交助手消息。
        let metadata = MessageMetadata {
            usage: Some(summary.usage.clone()),
            stop_reason: Some(summary.stop_reason.clone()),
            provider: Some(self.config.provider_id.clone()),
            model: Some(self.config.model.clone()),
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
            .request_approval(&invocations, cancel.clone())
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
            let executed = self.context.execute_tools(approved, cancel.clone()).await;
            for (slot, r) in approved_slots.iter().zip(executed) {
                self.emit_payload(AgentEvent::ToolExecutionCompleted {
                    tool_call_id: r.tool_call_id.clone(),
                    result: tool_result_content_view(&r),
                });
                self.budget
                    .record_output(estimate_output_bytes(&r.result.content));
                results[*slot] = r;
            }
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

    fn next_envelope(&mut self, payload: AgentEvent) -> AgentEventEnvelope {
        let sequence = EventSequence::new(self.next_sequence);
        self.next_sequence += 1;
        AgentEventEnvelope::new(
            agent_domain::EventId::from(format!("evt-{}-{}", self.config.run_id, sequence.value())),
            self.config.session_id.clone(),
            self.config.run_id.clone(),
            sequence,
            agent_domain::Timestamp::from_unix_millis(unix_millis_now()),
            payload,
        )
    }

    fn emit_payload(&mut self, payload: AgentEvent) {
        let envelope = self.next_envelope(payload);
        // 广播忽略无订阅者错误（核心不应因此中断）。
        let _ = self.broadcaster.publish(envelope);
    }

    fn emit_terminal_payload(&mut self, payload: AgentEvent) {
        self.emit_payload(payload);
    }

    fn emit_message_committed(&mut self, message: &Message) {
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

/// 内部 sink：缓存 Provider 流式事件供 loop 累积。
struct LoopSink {
    events: std::sync::Mutex<Vec<ProviderStreamEvent>>,
}

impl LoopSink {
    fn new(_session_id: agent_domain::SessionId, _run_id: RunId, _request_id: RequestId) -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn events(&self) -> Vec<ProviderStreamEvent> {
        self.events.lock().expect("loop sink mutex").clone()
    }
}

#[async_trait::async_trait]
impl ProviderEventSink for LoopSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        self.events.lock().expect("loop sink mutex").push(event);
        Ok(())
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
            thinking: None,
        }
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

        let (state, summary) = engine.run(CancellationToken::new()).await.unwrap();
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
        let (state, summary) = engine.run(CancellationToken::new()).await.unwrap();
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
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("x")]),
            1,
            EventBroadcaster::new(),
        );

        let result = engine.run(cancel).await;
        assert!(matches!(result, Err(LoopError::Cancelled)));
        assert_eq!(engine.state(), RunState::Cancelled);
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
        let _ = engine.run(CancellationToken::new()).await;

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
        let _ = engine.run(CancellationToken::new()).await.unwrap();

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
}
