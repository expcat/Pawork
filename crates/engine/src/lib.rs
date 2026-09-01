//! Agent Engine：组装 [`CanonicalModelRequest`]、调用 `ModelProvider::stream`，
//! 经 [`AgentEventSink`] 发射 `AgentEventEnvelope`，并经 [`LoopContext`] 跑工具循环。
//!
//! 本 crate 不重试、不落库、不按 Provider 名称分支。
//! 落库由调用方在 sink 里 persist-first。

mod appender;
mod cancel;
pub mod context;
mod event;
mod session_turn;
mod tool_loop;

use std::collections::BTreeMap;

use pawork_domain::{CancellationToken, Message, ModelId, RequestId};
use pawork_domain::{
    CanonicalModelRequest, ModelProvider, ModelResponseSummary, PromptCachePreference,
    ProviderError, ProviderEventSink, RequestBudget, ResponseFormat, ToolChoice, ToolDefinition,
};

pub use appender::{tool_results_message, AssembledTurn, PendingToolCall, ToolCallResult};
pub use cancel::{
    CancelHandle, CancelReason, CancelReceipt, NoopProcessTreeCleaner, ProcessTreeCleaner,
};
pub use context::{
    compute_compaction, AutoCompactionReason, CompactionReason, CompactionTrigger, ContextBudget,
    ContextBudgetBreakdown, ContextLimits, HeuristicEstimator, InjectedLayer, TokenEstimator,
    ToolSchema, TrimThresholds, TrimmedToolResult, TurnContext,
};
pub use event::{map_provider_event, AgentEventSink, EngineError, LoopEventEmitter};
pub use session_turn::{now_timestamp, run_session_turn, SessionTurn};
pub use tool_loop::{
    run_manual_compaction, run_session, ApprovalGate, CompactionOutcome, LoopContext,
    PendingToolInvocation, WriteCheckpoint, DEFAULT_MAX_TOOL_ROUNDS,
};

/// 用冻结契约的默认值填满 CanonicalModelRequest（tools/hosted/extensions 空，
/// thinking/reasoning None，temperature/max_output_tokens None，
/// tool_choice Auto，response_format Text，prompt_cache Automatic，
/// budget default，provider_options 空，trace_id None）。
pub fn assemble_request(
    request_id: RequestId,
    model: ModelId,
    messages: Vec<Message>,
) -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id,
        model,
        messages,
        tools: Vec::new(),
        hosted_tools: Vec::new(),
        extensions: Vec::new(),
        tool_choice: ToolChoice::Auto,
        thinking: None,
        reasoning: None,
        temperature: None,
        max_output_tokens: None,
        stop_sequences: Vec::new(),
        response_format: ResponseFormat::Text,
        prompt_cache: PromptCachePreference::Automatic,
        budget: RequestBudget::default(),
        provider_options: BTreeMap::new(),
        trace_id: None,
    }
}

/// 与 [`assemble_request`] 相同默认值，但 `tools` 使用入参。
pub fn assemble_request_with_tools(
    request_id: RequestId,
    model: ModelId,
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
) -> CanonicalModelRequest {
    let mut request = assemble_request(request_id, model, messages);
    request.tools = tools;
    request
}

/// 内部单轮原语：若 cancel 已取消则不调用 provider，返回 ProviderError::cancelled。
/// 否则把 request / sink / cancel 交给 provider.stream。
/// 不重试、不落库、不跑工具循环、不按 provider 名分支、不把事件改写成 AgentEvent。
/// ProviderStreamEvent 13 变体全部由 provider 发射、sink 原样接收；engine 不滤不删。
/// 公开面已收口；crate 内 `session_turn` / `tool_loop` 继续走此入口。
pub(crate) async fn run_turn(
    provider: &dyn ModelProvider,
    request: CanonicalModelRequest,
    sink: &dyn ProviderEventSink,
    cancel: CancellationToken,
) -> Result<ModelResponseSummary, ProviderError> {
    if cancel.is_cancelled() {
        return Err(ProviderError::cancelled("turn cancelled"));
    }
    provider.stream(request, sink, cancel).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use pawork_domain::{
        ContentPart, MessageId, MessageRole, ProviderId, StopReason, TextContent, TokenUsage,
        ToolCallId,
    };
    use pawork_domain::{
        ModelDefinition, ProviderErrorKind, ProviderStreamEvent, ResolvedCredential, ToolDefinition,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<ProviderStreamEvent>>);

    impl RecordingSink {
        fn snapshot(&self) -> Vec<ProviderStreamEvent> {
            self.0.lock().expect("recording sink mutex").clone()
        }
    }

    #[async_trait]
    impl ProviderEventSink for RecordingSink {
        async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
            self.0.lock().expect("recording sink mutex").push(event);
            Ok(())
        }
    }

    struct ScriptedProvider {
        events: Vec<ProviderStreamEvent>,
        summary: ModelResponseSummary,
    }

    #[async_trait]
    impl ModelProvider for ScriptedProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            for event in &self.events {
                sink.emit(event.clone()).await?;
            }
            Ok(self.summary.clone())
        }
    }

    struct PanicIfCalledProvider;

    #[async_trait]
    impl ModelProvider for PanicIfCalledProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            _sink: &dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            panic!("stream must not be called when the turn is already cancelled");
        }
    }

    struct CancelAfterDeltaProvider;

    #[async_trait]
    impl ModelProvider for CancelAfterDeltaProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            sink.emit(ProviderStreamEvent::TextDelta("partial".into()))
                .await?;
            if cancel.is_cancelled() {
                return Err(ProviderError::cancelled("turn cancelled"));
            }
            cancel.cancelled().await;
            Err(ProviderError::cancelled("turn cancelled"))
        }
    }

    fn sample_messages() -> Vec<Message> {
        vec![Message {
            id: MessageId::from("message-1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "hello".into(),
            })],
            metadata: Default::default(),
        }]
    }

    fn sample_request() -> CanonicalModelRequest {
        assemble_request(
            RequestId::from("request-1"),
            ModelId::from("model-1"),
            sample_messages(),
        )
    }

    fn completed_summary() -> ModelResponseSummary {
        ModelResponseSummary {
            stop_reason: StopReason::Completed,
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 4,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
            response_id: Some("resp-1".into()),
            provider_metadata: Default::default(),
        }
    }

    fn happy_path_events() -> Vec<ProviderStreamEvent> {
        vec![
            ProviderStreamEvent::TextDelta("hello".into()),
            ProviderStreamEvent::UsageUpdated(TokenUsage {
                input_tokens: 10,
                output_tokens: 4,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            }),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ]
    }

    #[test]
    fn assemble_request_fills_idle_fields_with_defaults() {
        let messages = sample_messages();
        let request = assemble_request(
            RequestId::from("request-1"),
            ModelId::from("model-1"),
            messages.clone(),
        );

        assert_eq!(
            request,
            CanonicalModelRequest {
                request_id: RequestId::from("request-1"),
                model: ModelId::from("model-1"),
                messages,
                tools: Vec::new(),
                hosted_tools: Vec::new(),
                extensions: Vec::new(),
                tool_choice: ToolChoice::Auto,
                thinking: None,
                reasoning: None,
                temperature: None,
                max_output_tokens: None,
                stop_sequences: Vec::new(),
                response_format: ResponseFormat::Text,
                prompt_cache: PromptCachePreference::Automatic,
                budget: RequestBudget::default(),
                provider_options: BTreeMap::new(),
                trace_id: None,
            }
        );
    }

    #[test]
    fn assemble_request_with_tools_keeps_defaults_and_sets_tools() {
        let tools = vec![ToolDefinition {
            name: "echo".into(),
            description: "echo".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let request = assemble_request_with_tools(
            RequestId::from("request-1"),
            ModelId::from("model-1"),
            sample_messages(),
            tools.clone(),
        );
        let mut expected = assemble_request(
            RequestId::from("request-1"),
            ModelId::from("model-1"),
            sample_messages(),
        );
        expected.tools = tools;
        assert_eq!(request, expected);
    }

    #[tokio::test]
    async fn run_turn_forwards_text_usage_and_completed_summary() {
        let events = happy_path_events();
        let summary = completed_summary();
        let provider = ScriptedProvider {
            events: events.clone(),
            summary: summary.clone(),
        };
        let sink = RecordingSink::default();

        let result = run_turn(&provider, sample_request(), &sink, CancellationToken::new())
            .await
            .expect("happy-path turn");

        assert_eq!(result, summary);
        assert_eq!(result.stop_reason, StopReason::Completed);
        assert_eq!(sink.snapshot(), events);
    }

    #[tokio::test]
    async fn run_turn_forwards_unconsumed_stream_variants() {
        let mut events = happy_path_events();
        events.insert(1, ProviderStreamEvent::ThinkingDelta("think".into()));
        events.insert(
            2,
            ProviderStreamEvent::ToolCallStarted {
                id: ToolCallId::from("call-1"),
                name: "read_file".into(),
            },
        );
        let provider = ScriptedProvider {
            events: events.clone(),
            summary: completed_summary(),
        };
        let sink = RecordingSink::default();

        run_turn(&provider, sample_request(), &sink, CancellationToken::new())
            .await
            .expect("turn with extra variants");

        let received = sink.snapshot();
        assert!(received.contains(&ProviderStreamEvent::ThinkingDelta("think".into())));
        assert!(received.contains(&ProviderStreamEvent::ToolCallStarted {
            id: ToolCallId::from("call-1"),
            name: "read_file".into(),
        }));
        assert_eq!(received, events);
    }

    #[tokio::test]
    async fn pre_cancelled_turn_does_not_call_provider() {
        let token = CancellationToken::new();
        token.cancel();
        let sink = RecordingSink::default();

        let error = run_turn(&PanicIfCalledProvider, sample_request(), &sink, token)
            .await
            .expect_err("pre-cancelled turn must fail");

        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
        assert!(sink.snapshot().is_empty());
    }

    #[tokio::test]
    async fn mid_stream_cancel_returns_cancelled() {
        let token = CancellationToken::new();
        let sink = RecordingSink::default();
        let run = run_turn(
            &CancelAfterDeltaProvider,
            sample_request(),
            &sink,
            token.clone(),
        );
        let cancel_after_delta = async {
            loop {
                if !sink.snapshot().is_empty() {
                    token.cancel();
                    break;
                }
                tokio::task::yield_now().await;
            }
        };

        let (result, ()) = tokio::join!(run, cancel_after_delta);
        let error = result.expect_err("mid-stream cancel must fail");
        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
        assert_eq!(
            sink.snapshot(),
            vec![ProviderStreamEvent::TextDelta("partial".into())]
        );
    }
}
