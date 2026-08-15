use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_api::{
    CanonicalModelRequest, CredentialKind, ModelProvider, PromptCachePreference, ProviderError,
    ProviderEventSink, ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat,
    ToolChoice,
};
use pawork_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    TextContent,
};
use pawork_net::http::HttpClientConfig;
use pawork_providers::{XaiConfig, XaiProvider};
use wiremock::matchers::{header, method, path};
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

fn request(model: &str) -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: pawork_domain::RequestId::new("r1"),
        model: ModelId::new(model),
        messages: vec![Message {
            id: MessageId::new("m1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: "hi".into() })],
            metadata: MessageMetadata::default(),
        }],
        tools: Vec::new(), hosted_tools: Vec::new(), extensions: Vec::new(),
        tool_choice: ToolChoice::Auto, thinking: None, reasoning: None,
        temperature: None, max_output_tokens: None, stop_sequences: Vec::new(),
        response_format: ResponseFormat::Text,
        prompt_cache: PromptCachePreference::Automatic,
        budget: RequestBudget::default(), provider_options: BTreeMap::new(), trace_id: None,
    }
}

fn provider(server: &MockServer) -> XaiProvider {
    let mut config = XaiConfig::new(server.uri());
    config.http = HttpClientConfig::builder().disable_system_proxy().build();
    XaiProvider::new(
        config,
        Some(ResolvedCredential::new(CredentialKind::OAuthBearer, "oauth-xai")),
    ).unwrap()
}

#[tokio::test]
async fn model_capability_selects_responses_or_chat() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer oauth-xai"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n"
        )).expect(1).mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer oauth-xai"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_string(
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
        )).expect(1).mount(&server).await;

    let provider = provider(&server);
    provider.stream(request("grok-4"), &Sink::default(), CancellationToken::new()).await.unwrap();
    provider.stream(request("grok-3"), &Sink::default(), CancellationToken::new()).await.unwrap();
    server.verify().await;
}
