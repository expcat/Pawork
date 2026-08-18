//! 首发 API-key 渠道契约：默认 id/URL、凭证 fail-closed、Bearer 请求路径。
//!
//! 全程 wiremock，不接触真实网络与 Key。本文件依赖 pawork-providers 导出
//! api_key 类型；在 lib.rs 接线前本测试无法编译。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelProvider, ModelTransport, PromptCachePreference,
    ProviderError, ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, RequestBudget,
    ResolvedCredential, ResponseFormat, ToolChoice,
};
use pawork_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    StopReason, TextContent,
};
use pawork_net::http::HttpClientConfig;
use pawork_providers::{ApiKeyChannel, ApiKeyChannelConfig, ApiKeyChannelProvider};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Debug, Default)]
struct RecordingProviderSink(Arc<Mutex<Vec<ProviderStreamEvent>>>);

#[async_trait]
impl ProviderEventSink for RecordingProviderSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        self.0.lock().expect("provider sink mutex").push(event);
        Ok(())
    }
}

fn user(text: &str) -> Message {
    Message {
        id: MessageId::new("m1"),
        role: MessageRole::User,
        content: vec![ContentPart::Text(TextContent { text: text.into() })],
        metadata: MessageMetadata::default(),
    }
}

fn request() -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: pawork_domain::RequestId::from("r1"),
        model: ModelId::from("test-model"),
        messages: vec![user("hi")],
        tools: Vec::new(),
        hosted_tools: Vec::new(),
        extensions: Vec::new(),
        tool_choice: ToolChoice::Auto,
        thinking: None,
        temperature: Some(0.0),
        max_output_tokens: Some(128),
        stop_sequences: Vec::new(),
        response_format: ResponseFormat::Text,
        prompt_cache: PromptCachePreference::Automatic,
        budget: RequestBudget::default(),
        provider_options: BTreeMap::new(),
        trace_id: Some("trace-1".into()),
        reasoning: None,
    }
}

fn api_key() -> ResolvedCredential {
    ResolvedCredential::new(CredentialKind::ApiKey, "sk-channel-test")
}

fn config_for(channel: ApiKeyChannel, base_url: impl Into<String>) -> ApiKeyChannelConfig {
    ApiKeyChannelConfig::new(channel)
        .with_base_url(base_url)
        .with_http(HttpClientConfig::builder().disable_system_proxy().build())
}

fn sse_body(chunks: &[&str]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(chunk);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

#[test]
fn default_ids_and_base_urls_cover_all_channels() {
    let expected = [
        (
            ApiKeyChannel::GlmCoding,
            "glm-coding",
            "https://api.z.ai/api/coding/paas/v4",
        ),
        (
            ApiKeyChannel::OpenCodeGo,
            "opencode-go",
            "https://opencode.ai/zen/go/v1",
        ),
        (
            ApiKeyChannel::QwenTokenPlan,
            "qwen-token-plan",
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        ),
        (
            ApiKeyChannel::DeepSeek,
            "deepseek",
            "https://api.deepseek.com",
        ),
    ];

    assert_eq!(ApiKeyChannel::ALL.len(), expected.len());
    for (channel, id, url) in expected {
        assert_eq!(channel.as_str(), id);
        assert_eq!(channel.default_base_url(), url);
        assert_eq!(channel.provider_id().as_str(), id);

        let config = ApiKeyChannelConfig::new(channel);
        assert_eq!(config.channel, channel);
        assert_eq!(config.base_url, url);

        let provider = ApiKeyChannelProvider::new(config, Some(api_key())).expect("construct");
        assert_eq!(provider.id().as_str(), id);
    }
}

#[test]
fn missing_or_wrong_credential_is_fail_closed_for_all_channels() {
    for channel in ApiKeyChannel::ALL.iter().copied() {
        let missing = ApiKeyChannelProvider::new(ApiKeyChannelConfig::new(channel), None)
            .err()
            .expect("missing credential must fail");
        assert_eq!(missing.kind, ProviderErrorKind::Authentication);

        let empty = ApiKeyChannelProvider::new(
            ApiKeyChannelConfig::new(channel),
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "  ")),
        )
        .err()
        .expect("empty API key must fail");
        assert_eq!(empty.kind, ProviderErrorKind::Authentication);

        for kind in [CredentialKind::OAuthBearer, CredentialKind::SessionToken] {
            let error = ApiKeyChannelProvider::new(
                ApiKeyChannelConfig::new(channel),
                Some(ResolvedCredential::new(kind, "not-an-api-key")),
            )
            .err()
            .expect("non-API-key credential must fail");
            assert_eq!(error.kind, ProviderErrorKind::Authentication);
        }
    }
}

#[test]
fn fixed_credential_headers_are_rejected_for_all_channels() {
    for channel in ApiKeyChannel::ALL.iter().copied() {
        let mut config = ApiKeyChannelConfig::new(channel);
        config
            .http
            .extra_headers
            .push(("Authorization".into(), "Bearer attacker".into()));
        let error = ApiKeyChannelProvider::new(config, Some(api_key()))
            .err()
            .expect("duplicate credential header must fail");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
    }
}

#[tokio::test]
async fn bearer_chat_path_covers_all_channels() {
    for channel in ApiKeyChannel::ALL.iter().copied() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer sk-channel-test"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body(&[
                        r#"{"choices":[{"delta":{"content":"ok"}}]}"#,
                        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                    ])),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider =
            ApiKeyChannelProvider::new(config_for(channel, server.uri()), Some(api_key()))
                .expect("construct");
        assert_eq!(provider.id().as_str(), channel.as_str());

        let sink = RecordingProviderSink::default();
        let summary = provider
            .stream(request(), &sink, CancellationToken::new())
            .await
            .expect("stream");
        assert_eq!(summary.stop_reason, StopReason::Completed);
        server.verify().await;
    }
}

#[tokio::test]
async fn declared_model_transport_selects_responses_without_channel_branching() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer sk-channel-test"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
                ),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = config_for(ApiKeyChannel::OpenCodeGo, server.uri())
        .with_model_transport("test-model", ModelTransport::Responses);
    let provider = ApiKeyChannelProvider::new(config, Some(api_key())).unwrap();
    provider
        .stream(request(), &RecordingProviderSink::default(), CancellationToken::new())
        .await
        .unwrap();
    server.verify().await;
}
