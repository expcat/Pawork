//! 单轮会话：发 AgentEvent、调用 [`crate::run_turn`]、组装助手消息。

use std::sync::atomic::AtomicU64;

use pawork_api::{
    CanonicalModelRequest, ModelProvider, ModelResponseSummary, ProviderError, ProviderStreamEvent,
};
use pawork_domain::{
    AgentEvent, CancellationToken, ErrorContext, Message, MessageId, MessageMetadata, ModelId,
    ProviderId, RunId, SessionId, Timestamp, TokenUsage,
};

use crate::appender::AssembledTurn;
use crate::event::{AgentEventSink, EngineError, EventEmitter, LoopSink};
use crate::run_turn;

/// 一次会话轮次的标识与起始 sequence。
pub struct SessionTurn {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub provider_id: ProviderId,
    pub model: ModelId,
    pub start_sequence: u64,
    pub trigger_message: Message,
    pub timestamp: Timestamp,
}

impl SessionTurn {
    pub fn new(
        session_id: SessionId,
        run_id: RunId,
        provider_id: ProviderId,
        model: ModelId,
        start_sequence: u64,
        trigger_message: Message,
    ) -> Self {
        Self {
            session_id,
            run_id,
            provider_id,
            model,
            start_sequence,
            trigger_message,
            timestamp: now_timestamp(),
        }
    }
}

pub fn now_timestamp() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    Timestamp::from_unix_millis(millis)
}

/// 单轮事件化：先发 `RunStarted` 与用户 `MessageCommitted`，再跑 provider，最后发助手提交与终态。
///
/// 半轮取消/失败不把未完成的助手消息当成 `MessageCommitted`。
/// persist 失败时不再补终态（磁盘已停在最后一条成功 append）。
pub async fn run_session_turn(
    provider: &dyn ModelProvider,
    request: CanonicalModelRequest,
    turn: SessionTurn,
    events: &dyn AgentEventSink,
    cancel: CancellationToken,
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
    let trigger_id = turn.trigger_message.id.clone();
    let assistant_id = MessageId::from(format!("asst-{}", turn.run_id));
    let message_count = request.messages.len() as u64;
    let request_id = request.request_id.clone();

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
        emitter
            .emit(AgentEvent::RunCancelled {
                reason: Some("turn cancelled".into()),
                usage: None,
            })
            .await?;
        return Err(ProviderError::cancelled("turn cancelled").into());
    }

    emitter
        .emit(AgentEvent::ProviderRequestStarted {
            request_id,
            provider_id: turn.provider_id.clone(),
            model: turn.model.as_str().to_string(),
        })
        .await?;

    let sink = LoopSink::new(emitter.clone(), assistant_id.clone());

    let result = run_turn(provider, request, &sink, cancel).await;
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
            let assistant = assembled.into_message(MessageMetadata {
                usage: Some(summary.usage.clone()),
                stop_reason: Some(summary.stop_reason.clone()),
                provider: Some(turn.provider_id.clone()),
                model: Some(turn.model.clone()),
                ..MessageMetadata::default()
            });
            emitter
                .emit(AgentEvent::MessageCommitted { message: assistant })
                .await?;
            emitter
                .emit(AgentEvent::RunCompleted {
                    stop_reason: summary.stop_reason.clone(),
                    usage: summary.usage.clone(),
                })
                .await?;
            Ok(summary)
        }
        Err(error) if error.kind == pawork_api::ProviderErrorKind::Cancelled => {
            let usage = last_stream_usage(&sink.drain_events());
            emitter
                .emit(AgentEvent::RunCancelled {
                    reason: Some(error.message.clone()),
                    usage: optional_usage(&usage),
                })
                .await?;
            Err(error.into())
        }
        Err(error) => {
            let usage = last_stream_usage(&sink.drain_events());
            let context = ErrorContext::from(error.clone());
            emitter
                .emit(AgentEvent::RunFailed {
                    error: context,
                    usage: optional_usage(&usage),
                })
                .await?;
            Err(error.into())
        }
    }
}

fn optional_usage(usage: &TokenUsage) -> Option<TokenUsage> {
    if usage.is_zero() {
        None
    } else {
        Some(usage.clone())
    }
}

fn last_stream_usage(events: &[ProviderStreamEvent]) -> TokenUsage {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            ProviderStreamEvent::UsageUpdated(usage) => Some(usage.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use pawork_api::{
        ModelDefinition, ProviderErrorKind, ProviderEventSink, ProviderStreamEvent,
        ResolvedCredential,
    };
    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, ContentPart, EventSequence, MessageId, MessageRole,
        RequestId, StopReason, TextContent, TokenUsage,
    };

    use crate::assemble_request;

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

    struct FailAfter {
        inner: RecordingEvents,
        succeed: usize,
    }

    #[async_trait]
    impl AgentEventSink for FailAfter {
        async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
            let count = self.inner.snapshot().len();
            if count >= self.succeed {
                return Err(EngineError::sink("injected persist failure"));
            }
            self.inner.emit(envelope).await
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
            cancel.cancelled().await;
            Err(ProviderError::cancelled("turn cancelled"))
        }
    }

    struct FailingProvider;

    #[async_trait]
    impl ModelProvider for FailingProvider {
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
            Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "boom",
            ))
        }
    }

    fn user_hello() -> Message {
        Message {
            id: MessageId::from("msg-1"),
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

    fn sample_request() -> CanonicalModelRequest {
        assemble_request(
            RequestId::from("request-1"),
            ModelId::from("model-1"),
            vec![user_hello()],
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

    fn event_type(event: &AgentEvent) -> &'static str {
        match event {
            AgentEvent::MessageCommitted { message } if message.role == MessageRole::User => {
                "MessageCommitted.user"
            }
            AgentEvent::MessageCommitted { .. } => "MessageCommitted.assistant",
            AgentEvent::RunStarted { .. } => "RunStarted",
            AgentEvent::ContextPrepared { .. } => "ContextPrepared",
            AgentEvent::ProviderRequestStarted { .. } => "ProviderRequestStarted",
            AgentEvent::AssistantTextDelta { .. } => "AssistantTextDelta",
            AgentEvent::AssistantThinkingDelta { .. } => "AssistantThinkingDelta",
            AgentEvent::UsageUpdated { .. } => "UsageUpdated",
            AgentEvent::RunCompleted { .. } => "RunCompleted",
            AgentEvent::RunCancelled { .. } => "RunCancelled",
            AgentEvent::RunFailed { .. } => "RunFailed",
            _ => "other",
        }
    }

    #[tokio::test]
    async fn happy_path_emits_session_turn_sequence() {
        let provider = ScriptedProvider {
            events: vec![
                ProviderStreamEvent::ThinkingDelta("think".into()),
                ProviderStreamEvent::TextDelta("hello".into()),
                ProviderStreamEvent::UsageUpdated(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 4,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                }),
                ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
            ],
            summary: completed_summary(),
        };
        let sink = RecordingEvents::default();
        let summary = run_session_turn(
            &provider,
            sample_request(),
            sample_turn(),
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        assert_eq!(summary.stop_reason, StopReason::Completed);
        assert_eq!(
            sink.types(),
            vec![
                "RunStarted",
                "MessageCommitted.user",
                "ContextPrepared",
                "ProviderRequestStarted",
                "AssistantThinkingDelta",
                "AssistantTextDelta",
                "UsageUpdated",
                "MessageCommitted.assistant",
                "RunCompleted",
            ]
        );
        let envelopes = sink.snapshot();
        for (index, envelope) in envelopes.iter().enumerate() {
            assert_eq!(envelope.sequence, EventSequence::new((index + 1) as u64));
            assert_eq!(envelope.event_id.as_str(), format!("evt-run-1-{}", index + 1));
        }
        let assistant = envelopes
            .iter()
            .find_map(|envelope| match &envelope.payload {
                AgentEvent::MessageCommitted { message }
                    if message.role == MessageRole::Assistant =>
                {
                    Some(message)
                }
                _ => None,
            })
            .expect("assistant");
        assert!(matches!(
            &assistant.content[0],
            ContentPart::Thinking(thinking) if thinking.text == "think"
        ));
        assert!(matches!(
            &assistant.content[1],
            ContentPart::Text(text) if text.text == "hello"
        ));
    }

    #[tokio::test]
    async fn pre_cancelled_emits_run_cancelled_without_provider_request() {
        let token = CancellationToken::new();
        token.cancel();
        let sink = RecordingEvents::default();
        let error = run_session_turn(
            &FailingProvider,
            sample_request(),
            sample_turn(),
            &sink,
            token,
        )
        .await
        .expect_err("cancelled");
        assert!(error.is_cancelled());
        assert_eq!(
            sink.types(),
            vec![
                "RunStarted",
                "MessageCommitted.user",
                "ContextPrepared",
                "RunCancelled",
            ]
        );
    }

    #[tokio::test]
    async fn mid_stream_cancel_keeps_delta_and_does_not_commit_assistant() {
        let token = CancellationToken::new();
        let sink = RecordingEvents::default();
        let run = run_session_turn(
            &CancelAfterDeltaProvider,
            sample_request(),
            sample_turn(),
            &sink,
            token.clone(),
        );
        let cancel_after_delta = async {
            loop {
                if sink.types().contains(&"AssistantTextDelta") {
                    token.cancel();
                    break;
                }
                tokio::task::yield_now().await;
            }
        };
        let (result, ()) = tokio::join!(run, cancel_after_delta);
        assert!(result.expect_err("cancelled").is_cancelled());
        let types = sink.types();
        assert!(types.contains(&"AssistantTextDelta"));
        assert!(types.contains(&"RunCancelled"));
        assert!(!types.contains(&"MessageCommitted.assistant"));
        assert!(!types.contains(&"RunCompleted"));
    }

    #[tokio::test]
    async fn provider_error_emits_run_failed() {
        let sink = RecordingEvents::default();
        let error = run_session_turn(
            &FailingProvider,
            sample_request(),
            sample_turn(),
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect_err("failed");
        assert!(!error.is_cancelled());
        assert_eq!(
            sink.types(),
            vec![
                "RunStarted",
                "MessageCommitted.user",
                "ContextPrepared",
                "ProviderRequestStarted",
                "RunFailed",
            ]
        );
    }

    #[tokio::test]
    async fn persist_failure_mid_turn_stops_without_terminal_and_resume_continues_sequence() {
        let provider = ScriptedProvider {
            events: vec![
                ProviderStreamEvent::TextDelta("hello".into()),
                ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
            ],
            summary: completed_summary(),
        };
        let first = FailAfter {
            inner: RecordingEvents::default(),
            succeed: 5,
        };
        let error = run_session_turn(
            &provider,
            sample_request(),
            sample_turn(),
            &first,
            CancellationToken::new(),
        )
        .await
        .expect_err("persist fail");
        assert!(matches!(error, EngineError::Sink(_)));
        let persisted = first.inner.snapshot();
        assert_eq!(persisted.len(), 5);
        assert_eq!(
            first.inner.types(),
            vec![
                "RunStarted",
                "MessageCommitted.user",
                "ContextPrepared",
                "ProviderRequestStarted",
                "AssistantTextDelta",
            ]
        );
        assert!(!first.inner.types().contains(&"MessageCommitted.assistant"));

        let committed: Vec<Message> = persisted
            .iter()
            .filter_map(|envelope| match &envelope.payload {
                AgentEvent::MessageCommitted { message } => Some(message.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].role, MessageRole::User);

        let resume_sink = RecordingEvents::default();
        let mut resume = sample_turn();
        resume.run_id = RunId::from("run-2");
        resume.start_sequence = persisted.last().unwrap().sequence.value() + 1;
        resume.trigger_message = Message {
            id: MessageId::from("msg-2"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "again".into(),
            })],
            metadata: Default::default(),
        };
        run_session_turn(
            &provider,
            assemble_request(
                RequestId::from("request-2"),
                ModelId::from("model-1"),
                {
                    let mut history = committed;
                    history.push(resume.trigger_message.clone());
                    history
                },
            ),
            resume,
            &resume_sink,
            CancellationToken::new(),
        )
        .await
        .expect("resume");
        assert_eq!(resume_sink.snapshot()[0].sequence, EventSequence::new(6));
        assert_eq!(
            resume_sink.snapshot()[0].event_id.as_str(),
            "evt-run-2-6"
        );
        assert_eq!(
            resume_sink
                .types()
                .into_iter()
                .filter(|name| *name == "MessageCommitted.user")
                .count(),
            1
        );
    }
}
