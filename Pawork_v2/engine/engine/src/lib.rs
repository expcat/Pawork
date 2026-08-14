//! 最小单轮 Agent Engine：组装 [`CanonicalModelRequest`] 并调用 `ModelProvider::stream`。
//!
//! 本 crate 不重试、不落库、不跑工具循环、不按 Provider 名称分支，
//! 也不把 `ProviderStreamEvent` 改写成 AgentEvent。

use std::collections::BTreeMap;

use pawork_api::{
    CanonicalModelRequest, ModelProvider, ModelResponseSummary, PromptCachePreference,
    ProviderError, ProviderEventSink, RequestBudget, ResponseFormat, ToolChoice,
};
use pawork_domain::{CancellationToken, Message, ModelId, RequestId};

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

/// 单轮：若 cancel 已取消则不调用 provider，返回 ProviderError::cancelled。
/// 否则把 request / sink / cancel 交给 provider.stream。
/// 不重试、不落库、不跑工具循环、不按 provider 名分支、不把事件改写成 AgentEvent。
/// ProviderStreamEvent 13 变体全部由 provider 发射、sink 原样接收；engine 不滤不删。
pub async fn run_turn(
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
    use pawork_api::{
        ModelDefinition, ProviderErrorKind, ProviderStreamEvent, ResolvedCredential,
    };
    use pawork_domain::{
        ContentPart, MessageId, MessageRole, ProviderId, StopReason, TextContent, TokenUsage,
        ToolCallId,
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

        let error = run_turn(
            &PanicIfCalledProvider,
            sample_request(),
            &sink,
            token,
        )
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
