//! 多轮工具循环：在 [`crate::run_turn`] 之上收集 tool call、经 [`LoopContext`]
//! 执行、回填 Tool 消息，直到本轮没有 tool call 或达到轮数上限。

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;

use async_trait::async_trait;
use pawork_api::{
    CanonicalModelRequest, ModelProvider, ModelResponseSummary, ProviderError, ToolResult,
};
use pawork_domain::{
    AgentEvent, CancellationToken, ErrorCategory, ErrorContext, MessageId, MessageMetadata,
    RequestId, TokenUsage, ToolCallId, ToolResultContent,
};

use crate::appender::{tool_results_message, AssembledTurn, ToolCallResult};
use crate::event::{AgentEventSink, EngineError, EventEmitter, LoopEventEmitter, LoopSink};
use crate::session_turn::SessionTurn;
use crate::run_turn;

/// 每 run 默认最大工具轮数（防失控）。达到后事件化终止，不再开下一轮 stream。
pub const DEFAULT_MAX_TOOL_ROUNDS: u64 = 20;

/// 待执行的一次工具调用（解析自本轮 tool call）。
#[derive(Clone, Debug)]
pub struct PendingToolInvocation {
    pub tool_call_id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Agent Loop 执行中需要的回调（由调用方注入）。
///
/// 本波次只含工具执行与 ID 生成；审批 / hook / scheduler 不在此 trait。
#[async_trait]
pub trait LoopContext: Send + Sync {
    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        events: LoopEventEmitter<'_>,
        cancel: CancellationToken,
    ) -> Vec<ToolCallResult>;

    fn next_message_id(&self) -> MessageId;

    fn next_request_id(&self) -> RequestId;
}

/// 多轮事件化：先发 `RunStarted` 与用户 `MessageCommitted`，再循环
/// provider → 组装助手 →（可选）执行工具并回填，直到无 tool call 或超限。
///
/// 是否继续以 [`AssembledTurn::has_tool_calls`] 为准，不看 `StopReason`。
/// persist 失败时不再补终态。不发审批事件，不按 Provider 名称分支。
pub async fn run_session(
    provider: &dyn ModelProvider,
    request: CanonicalModelRequest,
    turn: SessionTurn,
    events: &dyn AgentEventSink,
    cancel: CancellationToken,
    loop_ctx: &dyn LoopContext,
    max_tool_rounds: u64,
) -> Result<ModelResponseSummary, EngineError> {
    if turn.start_sequence == 0 {
        return Err(EngineError::sink(
            "start_sequence must be >= 1 (session_events CHECK)",
        ));
    }

    let next_sequence = AtomicU64::new(turn.start_sequence);
    let emitter = EventEmitter::new(
        turn.session_id.clone(),
        turn.run_id.clone(),
        &next_sequence,
        turn.timestamp,
        events,
    );
    let loop_events = LoopEventEmitter::new(emitter.clone());
    let trigger_id = turn.trigger_message.id.clone();
    let message_count = request.messages.len() as u64;

    emitter
        .emit(AgentEvent::RunStarted {
            trigger_message_id: trigger_id,
        })
        .await?;
    emitter
        .emit(AgentEvent::MessageCommitted {
            message: turn.trigger_message.clone(),
        })
        .await?;
    emitter
        .emit(AgentEvent::ContextPrepared {
            message_count,
            estimated_input_tokens: 0,
        })
        .await?;

    if cancel.is_cancelled() {
        return emit_cancelled(&emitter, "turn cancelled").await;
    }

    let mut current = request;
    let mut tool_rounds = 0_u64;
    let mut run_usage = TokenUsage::default();

    loop {
        if cancel.is_cancelled() {
            return emit_cancelled(&emitter, "turn cancelled").await;
        }

        emitter
            .emit(AgentEvent::ProviderRequestStarted {
                request_id: current.request_id.clone(),
                provider_id: turn.provider_id.clone(),
                model: turn.model.as_str().to_string(),
            })
            .await?;

        let assistant_id = loop_ctx.next_message_id();
        let sink = LoopSink::new(emitter.clone(), assistant_id.clone());
        let result = run_turn(provider, current.clone(), &sink, cancel.clone()).await;
        if let Some(error) = sink.take_persist_error() {
            return Err(error);
        }

        match result {
            Ok(summary) => {
                let mut assembled = AssembledTurn::new(assistant_id);
                for event in sink.drain_events() {
                    assembled.apply(&event);
                }
                assembled.summary = Some(summary.clone());

                let invocations = pending_invocations(&assembled);
                let has_tool_calls = assembled.has_tool_calls();
                let assistant = assembled.into_message(MessageMetadata {
                    usage: Some(summary.usage.clone()),
                    stop_reason: Some(summary.stop_reason.clone()),
                    provider: Some(turn.provider_id.clone()),
                    model: Some(turn.model.clone()),
                    ..MessageMetadata::default()
                });
                emitter
                    .emit(AgentEvent::MessageCommitted {
                        message: assistant.clone(),
                    })
                    .await?;

                run_usage = saturating_add_usage(&run_usage, &summary.usage);

                if !has_tool_calls {
                    let mut completed = summary;
                    completed.usage = run_usage.clone();
                    emitter
                        .emit(AgentEvent::RunCompleted {
                            stop_reason: completed.stop_reason.clone(),
                            usage: completed.usage.clone(),
                        })
                        .await?;
                    return Ok(completed);
                }

                for invocation in &invocations {
                    emitter
                        .emit(AgentEvent::ToolExecutionStarted {
                            tool_call_id: invocation.tool_call_id.clone(),
                        })
                        .await?;
                }

                let raw = loop_ctx
                    .execute_tools(invocations.clone(), loop_events.clone(), cancel.clone())
                    .await;
                let results = align_tool_results(&invocations, raw);

                for result in &results {
                    emitter
                        .emit(AgentEvent::ToolExecutionCompleted {
                            tool_call_id: result.tool_call_id.clone(),
                            result: tool_result_content(result),
                        })
                        .await?;
                }

                let tool_message =
                    tool_results_message(loop_ctx.next_message_id(), results);
                emitter
                    .emit(AgentEvent::MessageCommitted {
                        message: tool_message.clone(),
                    })
                    .await?;

                current.messages.push(assistant);
                current.messages.push(tool_message);
                current.request_id = loop_ctx.next_request_id();

                tool_rounds += 1;
                if tool_rounds >= max_tool_rounds {
                    let message =
                        format!("maximum tool rounds exceeded ({max_tool_rounds})");
                    emitter
                        .emit(AgentEvent::RunFailed {
                            error: ErrorContext {
                                category: ErrorCategory::ResourceExhausted,
                                message,
                                retryable: false,
                                retry_after_ms: None,
                                diagnostics: Default::default(),
                            },
                        })
                        .await?;
                    return Err(EngineError::MaxToolRounds(max_tool_rounds));
                }
            }
            Err(error) if error.kind == pawork_api::ProviderErrorKind::Cancelled => {
                return emit_cancelled(&emitter, error.message.clone()).await;
            }
            Err(error) => {
                let context = ErrorContext::from(error.clone());
                emitter
                    .emit(AgentEvent::RunFailed { error: context })
                    .await?;
                return Err(error.into());
            }
        }
    }
}

async fn emit_cancelled(
    emitter: &EventEmitter<'_>,
    reason: impl Into<String>,
) -> Result<ModelResponseSummary, EngineError> {
    let reason = reason.into();
    emitter
        .emit(AgentEvent::RunCancelled {
            reason: Some(reason.clone()),
        })
        .await?;
    Err(ProviderError::cancelled(reason).into())
}

fn pending_invocations(assembled: &AssembledTurn) -> Vec<PendingToolInvocation> {
    assembled
        .tool_call_order
        .iter()
        .filter_map(|id| {
            assembled.tool_calls.get(id).map(|call| PendingToolInvocation {
                tool_call_id: id.clone(),
                name: call.name.clone(),
                arguments: call.arguments(),
            })
        })
        .collect()
}

fn align_tool_results(
    invocations: &[PendingToolInvocation],
    results: Vec<ToolCallResult>,
) -> Vec<ToolCallResult> {
    let mut by_id: BTreeMap<ToolCallId, ToolCallResult> = results
        .into_iter()
        .map(|result| (result.tool_call_id.clone(), result))
        .collect();
    invocations
        .iter()
        .map(|invocation| {
            by_id
                .remove(&invocation.tool_call_id)
                .unwrap_or_else(|| ToolCallResult {
                    tool_call_id: invocation.tool_call_id.clone(),
                    tool_name: invocation.name.clone(),
                    arguments: invocation.arguments.clone(),
                    result: ToolResult::failure(ErrorContext {
                        category: ErrorCategory::NotFound,
                        message: "missing tool result".into(),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    }),
                })
        })
        .collect()
}

fn tool_result_content(result: &ToolCallResult) -> ToolResultContent {
    ToolResultContent {
        tool_call_id: result.tool_call_id.clone(),
        tool_name: Some(result.tool_name.clone()),
        content: result.result.content.clone(),
        is_error: result.result.is_error(),
        metadata: result.result.metadata.clone(),
    }
}

fn saturating_add_usage(acc: &TokenUsage, round: &TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: acc.input_tokens.saturating_add(round.input_tokens),
        output_tokens: acc.output_tokens.saturating_add(round.output_tokens),
        cache_read_tokens: acc
            .cache_read_tokens
            .saturating_add(round.cache_read_tokens),
        cache_write_tokens: acc
            .cache_write_tokens
            .saturating_add(round.cache_write_tokens),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use pawork_api::{
        AgentTool, CanonicalModelRequest, ModelProvider, ModelResponseSummary, ProviderError,
        ProviderEventSink, ToolDefinition, ToolError, ToolErrorKind, ToolEventSink,
        ToolExecutionContext, ToolRequest, ToolResult, ToolStreamEvent,
    };
    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, CancellationToken, ContentPart, ErrorCategory, Message,
        MessageId, MessageRole, ModelId, ProviderId, RequestId, RunId, SessionId, StopReason,
        TextContent, Timestamp, TokenUsage, ToolCallId, WorkspaceId,
    };
    use pawork_testkit::{MockProvider, MockScript, MockTool};

    use crate::assemble_request_with_tools;
    use crate::event::{AgentEventSink, EngineError, LoopEventEmitter};
    use crate::session_turn::SessionTurn;

    use super::*;

    #[derive(Default)]
    struct RecordingEvents(Mutex<Vec<AgentEventEnvelope>>);

    impl RecordingEvents {
        fn snapshot(&self) -> Vec<AgentEventEnvelope> {
            self.0.lock().expect("events mutex").clone()
        }

        fn types(&self) -> Vec<&'static str> {
            self.snapshot()
                .into_iter()
                .map(|envelope| event_type(&envelope.payload))
                .collect()
        }
    }

    #[async_trait]
    impl AgentEventSink for RecordingEvents {
        async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
            self.0.lock().expect("events mutex").push(envelope);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct RecordingProvider {
        inner: MockProvider,
        requests: Arc<Mutex<Vec<CanonicalModelRequest>>>,
    }

    impl RecordingProvider {
        fn new(inner: MockProvider) -> Self {
            Self {
                inner,
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Vec<CanonicalModelRequest> {
            self.requests.lock().expect("requests mutex").clone()
        }
    }

    #[async_trait]
    impl ModelProvider for RecordingProvider {
        fn id(&self) -> ProviderId {
            self.inner.id()
        }

        async fn list_models(
            &self,
            credential: Option<&pawork_api::ResolvedCredential>,
        ) -> Result<Vec<pawork_api::ModelDefinition>, ProviderError> {
            self.inner.list_models(credential).await
        }

        async fn stream(
            &self,
            request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            self.requests
                .lock()
                .expect("requests mutex")
                .push(request.clone());
            self.inner.stream(request, sink, cancel).await
        }
    }

    struct TestContext {
        tools: Vec<MockTool>,
        msg_counter: AtomicU64,
        req_counter: AtomicU64,
    }

    impl TestContext {
        fn new(tools: Vec<MockTool>) -> Self {
            Self {
                tools,
                msg_counter: AtomicU64::new(0),
                req_counter: AtomicU64::new(0),
            }
        }
    }

    struct ForwardingSink<'a> {
        tool_call_id: ToolCallId,
        events: LoopEventEmitter<'a>,
    }

    #[async_trait]
    impl ToolEventSink for ForwardingSink<'_> {
        async fn emit(&self, event: ToolStreamEvent) -> Result<(), ToolError> {
            self.events
                .emit_tool_event(self.tool_call_id.clone(), event)
                .await
                .map_err(|error| ToolError {
                    kind: ToolErrorKind::Internal,
                    message: error.to_string(),
                    retryable: false,
                    retry_after_ms: None,
                })
        }
    }

    async fn execute_one(
        tools: &[MockTool],
        call: PendingToolInvocation,
        events: LoopEventEmitter<'_>,
        cancel: CancellationToken,
    ) -> ToolCallResult {
        let tool = tools
            .iter()
            .find(|tool| tool.descriptor().name == call.name)
            .cloned();
        let result = if let Some(tool) = tool {
            let request = ToolRequest {
                tool_call_id: call.tool_call_id.clone(),
                input: call.arguments.clone(),
            };
            let context = ToolExecutionContext {
                workspace_id: WorkspaceId::from("ws"),
                run_id: RunId::from("run"),
                working_directory: None,
            };
            let sink = ForwardingSink {
                tool_call_id: call.tool_call_id.clone(),
                events,
            };
            tool.execute(request, context, &sink, cancel)
                .await
                .unwrap_or_else(|error| ToolResult::failure(ErrorContext::from(error)))
        } else {
            ToolResult::failure(ErrorContext {
                category: ErrorCategory::NotFound,
                message: format!("unknown tool {}", call.name),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            })
        };
        ToolCallResult {
            tool_call_id: call.tool_call_id,
            tool_name: call.name,
            arguments: call.arguments,
            result,
        }
    }

    #[async_trait]
    impl LoopContext for TestContext {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            events: LoopEventEmitter<'_>,
            cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            let tools = self.tools.clone();
            let jobs = calls.into_iter().map(|call| {
                let events = events.clone();
                let tools = tools.clone();
                let cancel = cancel.clone();
                async move { execute_one(&tools, call, events, cancel).await }
            });
            futures::future::join_all(jobs).await
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

    fn user_hello() -> Message {
        Message {
            id: MessageId::from("msg-user"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "hello".into(),
            })],
            metadata: Default::default(),
        }
    }

    fn sample_turn() -> SessionTurn {
        SessionTurn {
            session_id: SessionId::from("ses-1"),
            run_id: RunId::from("run-1"),
            provider_id: ProviderId::from("mock"),
            model: ModelId::from("model-1"),
            start_sequence: 1,
            trigger_message: user_hello(),
            timestamp: Timestamp::from_unix_millis(1),
        }
    }

    fn echo_tool_def() -> ToolDefinition {
        ToolDefinition {
            name: "echo".into(),
            description: "echo".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn sample_request(tools: Vec<ToolDefinition>) -> CanonicalModelRequest {
        assemble_request_with_tools(
            RequestId::from("request-1"),
            ModelId::from("model-1"),
            vec![user_hello()],
            tools,
        )
    }

    fn event_type(event: &AgentEvent) -> &'static str {
        match event {
            AgentEvent::MessageCommitted { message } => match message.role {
                MessageRole::User => "MessageCommitted.user",
                MessageRole::Assistant => "MessageCommitted.assistant",
                MessageRole::Tool => "MessageCommitted.tool",
                MessageRole::System => "MessageCommitted.system",
            },
            AgentEvent::RunStarted { .. } => "RunStarted",
            AgentEvent::ContextPrepared { .. } => "ContextPrepared",
            AgentEvent::ProviderRequestStarted { .. } => "ProviderRequestStarted",
            AgentEvent::AssistantTextDelta { .. } => "AssistantTextDelta",
            AgentEvent::ToolCallStarted { .. } => "ToolCallStarted",
            AgentEvent::ToolCallArgumentsDelta { .. } => "ToolCallArgumentsDelta",
            AgentEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
            AgentEvent::ToolExecutionCompleted { .. } => "ToolExecutionCompleted",
            AgentEvent::ToolOutputDelta { .. } => "ToolOutputDelta",
            AgentEvent::UsageUpdated { .. } => "UsageUpdated",
            AgentEvent::RunCompleted { .. } => "RunCompleted",
            AgentEvent::RunCancelled { .. } => "RunCancelled",
            AgentEvent::RunFailed { .. } => "RunFailed",
            _ => "other",
        }
    }

    fn committed_roles(sink: &RecordingEvents) -> Vec<MessageRole> {
        sink.snapshot()
            .into_iter()
            .filter_map(|envelope| match envelope.payload {
                AgentEvent::MessageCommitted { message } => Some(message.role),
                _ => None,
            })
            .collect()
    }

    fn tool_messages(sink: &RecordingEvents) -> Vec<Message> {
        sink.snapshot()
            .into_iter()
            .filter_map(|envelope| match envelope.payload {
                AgentEvent::MessageCommitted { message }
                    if message.role == MessageRole::Tool =>
                {
                    Some(message)
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn mock_provider_completes_multi_turn_tool_loop() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("echo", serde_json::json!({"text": "hi"}))
                .usage(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    cache_read_tokens: 100,
                    cache_write_tokens: 4,
                })
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .text("done")
                .usage(TokenUsage {
                    input_tokens: 20,
                    output_tokens: 1,
                    cache_read_tokens: 5,
                    cache_write_tokens: 8,
                })
                .complete(),
        ]));
        let echo = MockTool::new(
            "echo",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "hi".into(),
            })]),
        );
        let ctx = TestContext::new(vec![echo]);
        let sink = RecordingEvents::default();

        let summary = run_session(
            &provider,
            sample_request(vec![echo_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
        )
        .await
        .expect("multi-turn loop");

        assert_eq!(summary.stop_reason, StopReason::Completed);
        assert_eq!(summary.usage.input_tokens, 30);
        assert_eq!(summary.usage.output_tokens, 3);
        assert_eq!(summary.usage.cache_read_tokens, 105);
        assert_eq!(summary.usage.cache_write_tokens, 12);
        assert_eq!(
            committed_roles(&sink),
            vec![
                MessageRole::User,
                MessageRole::Assistant,
                MessageRole::Tool,
                MessageRole::Assistant,
            ]
        );
        let types = sink.types();
        assert!(types.contains(&"ToolExecutionStarted"));
        assert!(types.contains(&"ToolExecutionCompleted"));
        assert!(types.contains(&"MessageCommitted.tool"));
        assert!(types.contains(&"RunCompleted"));
        assert!(!types.contains(&"RunFailed"));

        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| message.role == MessageRole::Tool),
            "second-round request must include a Tool-role message"
        );
        assert_eq!(requests[1].tools, vec![echo_tool_def()]);
    }

    #[tokio::test]
    async fn parallel_readonly_tools_are_dispatched_together() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("read_file", serde_json::json!({"path": "a"}))
                .tool_call("list_directory", serde_json::json!({"path": "."}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let read_file = MockTool::new(
            "read_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "a".into(),
            })]),
        );
        let list_directory = MockTool::new(
            "list_directory",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: ".".into(),
            })]),
        );
        let ctx = TestContext::new(vec![read_file.clone(), list_directory.clone()]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![
                ToolDefinition {
                    name: "read_file".into(),
                    description: "read".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDefinition {
                    name: "list_directory".into(),
                    description: "list".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
        )
        .await
        .expect("parallel tools");

        assert_eq!(read_file.calls().len(), 1);
        assert_eq!(list_directory.calls().len(), 1);
        let second = &provider.requests()[1];
        let tool_parts: Vec<_> = second
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .flat_map(|message| message.content.iter())
            .filter(|part| matches!(part, ContentPart::ToolResult(_)))
            .collect();
        assert_eq!(tool_parts.len(), 2);
        assert_eq!(
            sink.types()
                .into_iter()
                .filter(|name| *name == "ToolExecutionStarted")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn tool_failure_is_filled_back_and_loop_continues() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("broken", serde_json::json!({"x": 1}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("recovered").complete(),
        ]));
        let broken = MockTool::failing(
            "broken",
            ToolError {
                kind: ToolErrorKind::ExecutionFailed,
                message: "boom".into(),
                retryable: false,
                retry_after_ms: None,
            },
        );
        let ctx = TestContext::new(vec![broken]);
        let sink = RecordingEvents::default();

        let summary = run_session(
            &provider,
            sample_request(vec![ToolDefinition {
                name: "broken".into(),
                description: "broken".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
        )
        .await
        .expect("loop continues after tool failure");

        assert_eq!(summary.stop_reason, StopReason::Completed);
        assert_eq!(provider.requests().len(), 2);
        let tool = &tool_messages(&sink)[0];
        assert!(matches!(
            &tool.content[0],
            ContentPart::ToolResult(result) if result.is_error
        ));
        assert!(sink.types().contains(&"RunCompleted"));
    }

    #[tokio::test]
    async fn max_tool_rounds_emits_run_failed_without_extra_stream() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("echo", serde_json::json!({"n": 1}))
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .tool_call("echo", serde_json::json!({"n": 2}))
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .tool_call("echo", serde_json::json!({"n": 3}))
                .complete_with(StopReason::ToolUse),
        ]));
        let echo = MockTool::new("echo", ToolResult::success(Vec::new()));
        let ctx = TestContext::new(vec![echo]);
        let sink = RecordingEvents::default();

        let error = run_session(
            &provider,
            sample_request(vec![echo_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            2,
        )
        .await
        .expect_err("max tool rounds");

        assert!(matches!(error, EngineError::MaxToolRounds(2)));
        assert!(!error.is_cancelled());
        assert_eq!(provider.requests().len(), 2);
        assert_eq!(provider.inner.calls().len(), 2);
        let types = sink.types();
        assert!(types.contains(&"RunFailed"));
        assert!(!types.contains(&"RunCompleted"));
        let failed = sink
            .snapshot()
            .into_iter()
            .find_map(|envelope| match envelope.payload {
                AgentEvent::RunFailed { error } => Some(error),
                _ => None,
            })
            .expect("RunFailed");
        assert_eq!(failed.category, ErrorCategory::ResourceExhausted);
        assert!(failed.message.contains("maximum tool rounds exceeded"));
        assert!(failed.message.contains('2'));
    }
}
