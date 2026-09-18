use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    StopReason, TextContent,
};
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelProvider, PromptCachePreference, ProviderError,
    ProviderEventSink, ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat,
    ToolChoice,
};
use pawork_providers::net::http::HttpClientConfig;
use pawork_providers::{XaiConfig, XaiProvider};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

#[derive(Default)]
struct Sink(Arc<Mutex<Vec<ProviderStreamEvent>>>);

#[async_trait]
impl ProviderEventSink for Sink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

fn request(model: &str) -> CanonicalModelRequest {
    CanonicalModelRequest {
        session_id: Some(pawork_domain::SessionId::from("session-other-provider")),
        request_id: pawork_domain::RequestId::new("r1"),
        model: ModelId::new(model),
        messages: vec![Message {
            id: MessageId::new("m1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: "hi".into() })],
            metadata: MessageMetadata::default(),
        }],
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

fn provider(server: &MockServer) -> XaiProvider {
    let mut config = XaiConfig::new(server.uri());
    config.http = HttpClientConfig::builder().disable_system_proxy().build();
    XaiProvider::new(
        config,
        Some(ResolvedCredential::new(
            CredentialKind::OAuthBearer,
            "oauth-xai",
        )),
    )
    .unwrap()
}

#[tokio::test]
async fn model_capability_selects_responses_or_chat() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer oauth-xai"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(common::responses_completed_body()),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer oauth-xai"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(common::chat_finish_only_body()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = provider(&server);
    let mut search = request("grok-4");
    search.hosted_tools.push(pawork_domain::HostedToolRequest {
        kind: pawork_domain::ToolCapabilityTag::WebSearch,
        name: "web_search".into(),
        description: String::new(),
        capabilities: Vec::new(),
        config: None,
    });
    provider
        .stream(&search, &Sink::default(), CancellationToken::new())
        .await
        .unwrap();
    provider
        .stream(
            &request("grok-3"),
            &Sink::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| !request.headers.contains_key("x-opencode-session")));
    search.model = ModelId::new("grok-3");
    let error = provider
        .stream(&search, &Sink::default(), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind, pawork_domain::ProviderErrorKind::InvalidRequest);
    let sent = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["tools"][0]["type"], "web_search");
    assert!(body.get("search_parameters").is_none());
    assert!(pawork_providers::xai_builtin_models()
        .iter()
        .all(|model| model
            .capabilities
            .hosted_tool_tags
            .contains(&pawork_domain::ToolCapabilityTag::WebSearch)
            == (model.capabilities.transport == pawork_domain::ModelTransport::Responses)));
    server.verify().await;
}

#[tokio::test]
async fn grok4_responses_round_trip_streams_events_with_oauth_bearer() {
    let server = MockServer::start().await;
    let sse = common::responses_text_stream_body("resp_xai_1", &["grok ", "works"], (11, 7));
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer oauth-xai"))
        .and(body_string_contains("grok-4"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = provider(&server);
    let sink = Sink::default();
    let summary = provider
        .stream(&request("grok-4"), &sink, CancellationToken::new())
        .await
        .unwrap();

    let events = sink.0.lock().unwrap().clone();
    let deltas = events
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::TextDelta(delta) => Some(delta.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(deltas, vec!["grok ", "works"]);
    assert!(matches!(
        &events[..],
        [ProviderStreamEvent::ResponseStarted { response_id: Some(id) }, ..] if id == "resp_xai_1"
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderStreamEvent::UsageUpdated(usage) if usage.input_tokens == 11 && usage.output_tokens == 7
    )));
    assert_eq!(summary.stop_reason, StopReason::Completed);
    assert_eq!(summary.usage.input_tokens, 11);
    assert_eq!(summary.usage.output_tokens, 7);
    assert_eq!(summary.response_id.as_deref(), Some("resp_xai_1"));
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| !request.headers.contains_key("x-opencode-session")));
    server.verify().await;
}
