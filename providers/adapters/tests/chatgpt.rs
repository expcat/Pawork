use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelProvider, PromptCachePreference, ProviderError,
    ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, RequestBudget, ResolvedCredential,
    ResponseFormat, ToolChoice,
};
use pawork_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    StopReason, TextContent,
};
use pawork_providers::net::http::HttpClientConfig;
use pawork_providers::{ChatGptConfig, ChatGptProvider};
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct Sink(Arc<Mutex<Vec<ProviderStreamEvent>>>);

#[async_trait]
impl ProviderEventSink for Sink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

fn request() -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: pawork_domain::RequestId::new("r1"),
        model: ModelId::new("codex-test"),
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
        max_output_tokens: Some(128),
        stop_sequences: Vec::new(),
        response_format: ResponseFormat::Text,
        prompt_cache: PromptCachePreference::Automatic,
        budget: RequestBudget::default(),
        provider_options: BTreeMap::new(),
        trace_id: Some("trace-1".into()),
    }
}

fn provider(server: &MockServer) -> ChatGptProvider {
    let mut config = ChatGptConfig::new("acct-test").with_base_url(server.uri());
    config.client_version = "1.2.3".into();
    config.http = HttpClientConfig::builder().disable_system_proxy().build();
    ChatGptProvider::new(
        config,
        Some(ResolvedCredential::new(
            CredentialKind::OAuthBearer,
            "oauth-test",
        )),
    )
    .unwrap()
}

#[tokio::test]
async fn oauth_headers_models_and_responses_path_are_wired() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(query_param("client_version", "1.2.3"))
        .and(header("authorization", "Bearer oauth-test"))
        .and(header("chatgpt-account-id", "acct-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [{"slug": "codex-test", "display_name": "Codex Test", "context_window": 200000}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer oauth-test"))
        .and(header("chatgpt-account-id", "acct-test"))
        .and(header("originator", "codex_cli_rs"))
        .and(body_partial_json(serde_json::json!({
            "model": "codex-test",
            "stream": true,
            "store": false
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(concat!(
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n"
                )),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = provider(&server);
    let models = provider.list_models(None).await.unwrap();
    assert_eq!(models[0].id.as_str(), "codex-test");
    let sink = Sink::default();
    let summary = provider
        .stream(request(), &sink, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(summary.stop_reason, StopReason::Completed);
    assert_eq!(summary.response_id.as_deref(), Some("resp_1"));
    server.verify().await;
}

#[tokio::test]
async fn malformed_responses_event_fails_even_if_completion_follows() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(concat!(
                    "data: not-json\n\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n"
                )),
        )
        .expect(1)
        .mount(&server)
        .await;

    let error = provider(&server)
        .stream(request(), &Sink::default(), CancellationToken::new())
        .await
        .expect_err("malformed event must terminate the stream");
    assert_eq!(error.kind, ProviderErrorKind::MalformedResponse);
    server.verify().await;
}
