//! env 门控真实 API 冒烟。不进默认测试路径。
//!
//! 需要：
//!   PAWORK_SMOKE_BASE_URL
//!   PAWORK_SMOKE_API_KEY
//!   PAWORK_SMOKE_MODEL
//!
//! 运行：`cargo test -p pawork-app --test smoke -- --ignored --nocapture`
//! 禁止把 key 打印到日志。

use std::sync::Mutex;

use async_trait::async_trait;
use pawork_api::{CredentialKind, ResolvedCredential};
use pawork_app::AppCore;
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, CancellationToken, ContentPart, Message, MessageId,
    MessageRole, ModelId, ProviderId, TextContent,
};
use pawork_engine::{AgentEventSink, EngineError};
use pawork_providers::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use pawork_session::SessionStore;

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
#[ignore]
async fn smoke_stream_receives_text_delta_and_completed() {
    let base_url = std::env::var("PAWORK_SMOKE_BASE_URL")
        .expect("PAWORK_SMOKE_BASE_URL is required for ignored smoke");
    let api_key = std::env::var("PAWORK_SMOKE_API_KEY")
        .expect("PAWORK_SMOKE_API_KEY is required for ignored smoke");
    let model = std::env::var("PAWORK_SMOKE_MODEL")
        .expect("PAWORK_SMOKE_MODEL is required for ignored smoke");

    let credential = ResolvedCredential::new(CredentialKind::ApiKey, api_key);
    let provider = OpenAiCompatibleProvider::new(
        OpenAiCompatibleConfig::new(base_url).with_provider_id("smoke"),
        Some(credential.clone()),
    )
    .expect("construct smoke provider");
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = AppCore::from_parts(
        std::sync::Arc::new(provider),
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
    assert!(
        events
            .iter()
            .any(|event| matches!(event.payload, AgentEvent::RunCompleted { .. })),
        "expected RunCompleted"
    );
    let _ = summary;
    core.shutdown().await.expect("shutdown");
}
