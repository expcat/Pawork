//! 首发 API-key 渠道契约：注册表行驱动、默认 id/URL、凭证 fail-closed、
//! Bearer 请求路径。
//!
//! 全程 wiremock，不接触真实网络与 Key。本文件依赖 pawork-providers 导出
//! api_key 类型；在 lib.rs 接线前本测试无法编译。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    StopReason, TextContent,
};
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelProvider, ModelTransport, PromptCachePreference,
    ProviderError, ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, RequestBudget,
    ResolvedCredential, ResponseFormat, ToolChoice,
};
use pawork_providers::channels::registry::{
    channel_preset, is_enabled, ChannelKind, ChannelPreset, CHANNEL_REGISTRY,
};
use pawork_providers::net::http::HttpClientConfig;
use pawork_providers::{ApiKeyChannelConfig, ApiKeyChannelProvider};
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
        session_id: None,
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

fn api_key_presets() -> Vec<&'static ChannelPreset> {
    CHANNEL_REGISTRY
        .iter()
        .filter(|preset| preset.kind == ChannelKind::ApiKey)
        .collect()
}

fn config_for(preset: &'static ChannelPreset, base_url: impl Into<String>) -> ApiKeyChannelConfig {
    ApiKeyChannelConfig::new(preset)
        .expect("api-key preset config")
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
        ("glm-coding", "https://api.z.ai/api/coding/paas/v4"),
        ("opencode-go", "https://opencode.ai/zen/go/v1"),
        (
            "qwen-token-plan",
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        ),
        ("deepseek", "https://api.deepseek.com"),
        ("kimi-platform", "https://api.moonshot.ai/v1"),
    ];

    let presets = api_key_presets();
    assert_eq!(presets.len(), expected.len());
    for (preset, (id, url)) in presets.into_iter().zip(expected) {
        assert_eq!(preset.id, id);
        assert_eq!(preset.default_base_url, url);
        assert!(is_enabled(preset), "{id} feature must be enabled here");

        let config = ApiKeyChannelConfig::new(preset).expect("config");
        assert_eq!(config.preset.id, id);
        assert_eq!(config.base_url, url);

        let provider = ApiKeyChannelProvider::new(config, Some(api_key())).expect("construct");
        assert_eq!(provider.id().as_str(), id);
    }
}

#[test]
fn non_api_key_preset_is_fail_closed() {
    let chatgpt = channel_preset("chatgpt").expect("chatgpt row");
    assert_ne!(chatgpt.kind, ChannelKind::ApiKey);
    let error = ApiKeyChannelConfig::new(chatgpt)
        .err()
        .expect("non-api-key preset must fail");
    assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
}

#[test]
fn missing_or_wrong_credential_is_fail_closed_for_all_channels() {
    for preset in api_key_presets() {
        let missing =
            ApiKeyChannelProvider::new(ApiKeyChannelConfig::new(preset).expect("config"), None)
                .err()
                .expect("missing credential must fail");
        assert_eq!(missing.kind, ProviderErrorKind::Authentication);

        let empty = ApiKeyChannelProvider::new(
            ApiKeyChannelConfig::new(preset).expect("config"),
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "  ")),
        )
        .err()
        .expect("empty API key must fail");
        assert_eq!(empty.kind, ProviderErrorKind::Authentication);

        for kind in [CredentialKind::OAuthBearer, CredentialKind::SessionToken] {
            let error = ApiKeyChannelProvider::new(
                ApiKeyChannelConfig::new(preset).expect("config"),
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
    for preset in api_key_presets() {
        let mut config = ApiKeyChannelConfig::new(preset).expect("config");
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
async fn bearer_session_headers_are_scoped_to_opencode_on_both_transports() {
    for preset in api_key_presets() {
        for transport in [ModelTransport::ChatCompletions, ModelTransport::Responses] {
            let server = MockServer::start().await;
            let (endpoint, body) = match transport {
                ModelTransport::ChatCompletions => (
                    "/chat/completions",
                    sse_body(&[
                        r#"{"choices":[{"delta":{"content":"ok"}}]}"#,
                        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                    ]),
                ),
                ModelTransport::Responses => (
                    "/responses",
                    sse_body(&[
                        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
                    ]),
                ),
                ModelTransport::Messages => unreachable!(),
            };
            Mock::given(method("POST"))
                .and(path(endpoint))
                .and(header("authorization", "Bearer sk-channel-test"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(body),
                )
                .expect(3)
                .mount(&server)
                .await;
            let config =
                config_for(preset, server.uri()).with_model_transport("test-model", transport);
            let provider = ApiKeyChannelProvider::new(config, Some(api_key())).unwrap();
            // 同一 adapter 跨会话使用，以及无会话请求，不得残留上一次身份。
            for session in [Some("session-one"), Some("session-two"), None] {
                let mut request = request();
                request.session_id = session.map(pawork_domain::SessionId::from);
                let summary = provider
                    .stream(
                        request,
                        &RecordingProviderSink::default(),
                        CancellationToken::new(),
                    )
                    .await
                    .expect("stream");
                assert_eq!(summary.stop_reason, StopReason::Completed);
            }
            let requests = server.received_requests().await.unwrap();
            for (request, session) in
                requests
                    .iter()
                    .zip([Some("session-one"), Some("session-two"), None])
            {
                let expected = if preset.id == "opencode-go" {
                    session
                } else {
                    None
                };
                assert_eq!(
                    request
                        .headers
                        .get("x-opencode-session")
                        .map(|value| value.to_str().unwrap()),
                    expected,
                    "{} {transport:?}",
                    preset.id,
                );
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                assert!(body.get("session_id").is_none());
                assert!(body.get("x-opencode-session").is_none());
            }
            server.verify().await;
        }
    }
}

#[tokio::test]
async fn invalid_opencode_session_header_fails_without_network_or_value_disclosure() {
    let server = MockServer::start().await;
    for transport in [ModelTransport::ChatCompletions, ModelTransport::Responses] {
        let config = config_for(channel_preset("opencode-go").unwrap(), server.uri())
            .with_model_transport("test-model", transport);
        let provider = ApiKeyChannelProvider::new(config, Some(api_key())).unwrap();
        let mut request = request();
        request.session_id = Some(pawork_domain::SessionId::from(
            "private-session\r\nx-injected: value",
        ));
        let error = provider
            .stream(
                request,
                &RecordingProviderSink::default(),
                CancellationToken::new(),
            )
            .await
            .expect_err("invalid header");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
        let error = format!("{error:?}");
        assert!(!error.contains("private-session"));
        assert!(!error.contains("sk-channel-test"));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn mixed_catalog_and_stream_share_documented_transports() {
    for (channel, chat_id, responses_id, excluded) in [
        (
            "opencode-go",
            "glm-5.3-flash",
            Some("grok-4.6"),
            "qwen3.8-max",
        ),
        ("qwen-token-plan", "qwen3.8-max", None, "wan2.7-image"),
    ] {
        let server = MockServer::start().await;
        let mut ids = vec![chat_id, excluded, "unknown-model"];
        ids.extend(responses_id);
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": ids.iter().map(|id| serde_json::json!({"id": id})).collect::<Vec<_>>()
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider = ApiKeyChannelProvider::new(
            config_for(channel_preset(channel).unwrap(), server.uri()),
            Some(api_key()),
        )
        .unwrap();
        let models = provider.list_models(None).await.unwrap();
        assert_eq!(models.len(), 1 + usize::from(responses_id.is_some()));
        assert_eq!(models[0].id.as_str(), chat_id);
        for model in models {
            let endpoint = match model.capabilities.transport {
                ModelTransport::ChatCompletions => "/chat/completions",
                ModelTransport::Responses => "/responses",
                ModelTransport::Messages => panic!("unsupported transport in runnable catalog"),
            };
            let body = if endpoint == "/responses" {
                sse_body(&[r#"{"type":"response.completed","response":{"status":"completed"}}"#])
            } else {
                sse_body(&[r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#])
            };
            Mock::given(method("POST"))
                .and(path(endpoint))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(body),
                )
                .expect(1)
                .mount(&server)
                .await;
            let mut req = request();
            req.model = model.id;
            provider
                .stream(
                    req,
                    &RecordingProviderSink::default(),
                    CancellationToken::new(),
                )
                .await
                .expect("documented route");
        }
        let before = server.received_requests().await.unwrap().len();
        for id in [excluded, "unknown-model"] {
            let mut req = request();
            req.model = ModelId::new(id);
            assert_eq!(
                provider
                    .stream(
                        req,
                        &RecordingProviderSink::default(),
                        CancellationToken::new()
                    )
                    .await
                    .unwrap_err()
                    .kind,
                ProviderErrorKind::InvalidRequest
            );
        }
        assert_eq!(server.received_requests().await.unwrap().len(), before);
        server.verify().await;
    }
}
