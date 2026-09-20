//! env 门控真实 API 冒烟。不进默认测试路径。
//!
//! 需要：
//!   PAWORK_SMOKE_BASE_URL
//!   PAWORK_SMOKE_API_KEY
//!   PAWORK_SMOKE_MODEL
//!   PAWORK_SMOKE_PROTOCOL（可选：`chat_completions` 默认 / `messages`）
//!
//! 运行：`cargo test -p pawork-app --offline --features live-smoke --test smoke -- --nocapture`
//! `live-smoke` 是显式 feature：缺环境变量必须失败，不得 ignore 记绿。
//! 禁止把 key 打印到日志。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_app::AppCore;
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, CancellationToken, ContentPart, Message, MessageId,
    MessageRole, ModelId, ProviderId, StopReason, TextContent,
};
use pawork_domain::{CredentialKind, ModelProvider, ResolvedCredential};
use pawork_engine::{AgentEventSink, EngineError};
use pawork_providers::{
    AnthropicConfig, AnthropicProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use pawork_storage::session::SessionStore;

#[derive(Default)]
struct RecordingEvents(Mutex<Vec<AgentEventEnvelope>>);

#[async_trait]
impl AgentEventSink for RecordingEvents {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        self.0.lock().expect("sink mutex").push(envelope);
        Ok(())
    }
}

#[tokio::test]
async fn smoke_stream_receives_text_delta_and_completed() {
    let base_url = std::env::var("PAWORK_SMOKE_BASE_URL")
        .expect("PAWORK_SMOKE_BASE_URL is required for live-smoke");
    let api_key = std::env::var("PAWORK_SMOKE_API_KEY")
        .expect("PAWORK_SMOKE_API_KEY is required for live-smoke");
    let model =
        std::env::var("PAWORK_SMOKE_MODEL").expect("PAWORK_SMOKE_MODEL is required for live-smoke");

    let protocol =
        std::env::var("PAWORK_SMOKE_PROTOCOL").unwrap_or_else(|_| "chat_completions".into());
    let credential = ResolvedCredential::new(CredentialKind::ApiKey, api_key);
    let provider: Arc<dyn ModelProvider> = match protocol.as_str() {
        "messages" | "anthropic-messages" => Arc::new(
            AnthropicProvider::new(
                AnthropicConfig::new(base_url).with_provider_id("smoke"),
                Some(credential.clone()),
            )
            .expect("construct anthropic smoke provider"),
        ),
        "chat_completions" | "openai-compatible" => Arc::new(
            OpenAiCompatibleProvider::new(
                OpenAiCompatibleConfig::new(base_url).with_provider_id("smoke"),
                Some(credential.clone()),
            )
            .expect("construct openai-compatible smoke provider"),
        ),
        other => panic!("unsupported PAWORK_SMOKE_PROTOCOL: {other}"),
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = AppCore::from_parts(
        provider,
        Some(credential),
        ModelId::from(model.as_str()),
        ProviderId::from("smoke"),
        Some(store),
    );
    let session = core.create_session("smoke").await.expect("session");

    let sink = RecordingEvents::default();
    let messages = vec![Message {
        id: MessageId::from("smoke-1"),
        role: MessageRole::User,
        content: vec![ContentPart::Text(TextContent {
            text: "Reply with the single word pong.".into(),
        })],
        metadata: Default::default(),
    }];

    let summary = core
        .chat_turn(&session, messages, &sink, CancellationToken::new())
        .await
        .expect("smoke turn");

    let events = sink.0.lock().expect("sink mutex").clone();
    assert!(
        events.iter().any(|event| matches!(
            event.payload,
            AgentEvent::AssistantTextDelta { ref delta, .. } if !delta.is_empty()
        )),
        "expected AssistantTextDelta"
    );
    assert_eq!(summary.stop_reason, StopReason::Completed);
    let terminal: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event.payload,
                AgentEvent::RunCompleted { .. }
                    | AgentEvent::RunFailed { .. }
                    | AgentEvent::RunCancelled { .. }
            )
        })
        .collect();
    assert_eq!(terminal.len(), 1, "expected exactly one terminal event");
    assert!(matches!(
        terminal[0].payload,
        AgentEvent::RunCompleted { .. }
    ));
    let restored = core
        .resume_messages(&session)
        .await
        .expect("persisted conversation");
    assert!(
        restored
            .iter()
            .any(|message| message.role == MessageRole::Assistant
                && message.content.iter().any(|part| matches!(part,
                    ContentPart::Text(text) if !text.text.trim().is_empty()
                ))),
        "completed response must be available to resume"
    );
    core.shutdown().await.expect("shutdown");
}
