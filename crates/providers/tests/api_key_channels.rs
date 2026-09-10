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
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

#[derive(Clone, Debug, Default)]
struct RecordingProviderSink(Arc<Mutex<Vec<ProviderStreamEvent>>>);

impl RecordingProviderSink {
    fn events(&self) -> Vec<ProviderStreamEvent> {
        self.0.lock().expect("provider sink mutex").clone()
    }
}

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

/// MOCK-7 合并用例：contract.rs 的 chat 文本流 / 单工具调用 / usage+stop
/// 三条等效契约改为五通道表驱动（ApiKeyChannelProvider 的 Chat Completions
/// 路径直接委派 OpenAiCompatibleProvider，样例与断言取自 tests/common，
/// 断言强度与原 contract 用例一致）。
#[tokio::test]
async fn chat_contract_facets_stream_over_all_channels() {
    for preset in api_key_presets() {
        for (facet, body) in [
            ("text_stream", common::chat_text_stream_body()),
            ("tool_call", common::chat_tool_call_body()),
            ("usage_and_stop", common::chat_usage_stop_body()),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/chat/completions"))
                .and(header("authorization", "Bearer sk-channel-test"))
                .and(header("x-trace-id", "trace-1"))
                .and(body_partial_json(serde_json::json!({
                    "stream_options": { "include_usage": true }
                })))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(body),
                )
                .expect(1)
                .mount(&server)
                .await;
            let config = config_for(preset, server.uri())
                .with_model_transport("test-model", ModelTransport::ChatCompletions);
            let provider =
                ApiKeyChannelProvider::new(config, Some(api_key())).expect("construct");
            let sink = RecordingProviderSink::default();
            let summary = provider
                .stream(request(), &sink, CancellationToken::new())
                .await
                .unwrap_or_else(|error| panic!("{facet} failed for {}: {error:?}", preset.id));
            let events = sink.events();
            match facet {
                "text_stream" => {
                    common::contract::assert_text_stream(&events);
                    assert_eq!(summary.stop_reason, StopReason::Completed);
                }
                "tool_call" => common::contract::assert_single_tool_call(&events),
                "usage_and_stop" => {
                    common::contract::assert_usage_and_stop(&events, StopReason::MaxTokens)
                }
                _ => unreachable!(),
            }
            server.verify().await;
        }
    }
}

#[tokio::test]
async fn bearer_session_headers_are_scoped_to_opencode_on_both_transports() {
    for preset in api_key_presets() {
        for transport in [ModelTransport::ChatCompletions, ModelTransport::Responses] {
            let server = MockServer::start().await;
            let (endpoint, body) = match transport {
                ModelTransport::ChatCompletions => {
                    ("/chat/completions", common::chat_minimal_ok_body())
                }
                ModelTransport::Responses => ("/responses", common::responses_completed_body()),
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
                common::responses_completed_body()
            } else {
                common::chat_finish_only_body()
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

fn go_usage_body() -> serde_json::Value {
    serde_json::json!({"usage": {
        "rolling": {"status": "ok", "percent": 0, "resetsAt": "1970-01-01T00:00:00.001Z"},
        "weekly": {"status": "ok", "percent": 99, "resetsAt": "2000-02-29T12:34:56.789Z"},
        "monthly": {"status": "rate-limited", "percent": 100, "resetsAt": "2026-09-09T00:00:00.000Z"}
    }})
}

#[tokio::test]
async fn go_usage_reads_three_windows_with_one_authenticated_get() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/usage"))
        .and(header("authorization", "Bearer sk-channel-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(go_usage_body()))
        .expect(1)
        .mount(&server)
        .await;
    let usage = pawork_providers::fetch_go_usage(
        config_for(channel_preset("opencode-go").unwrap(), server.uri()),
        &api_key(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(usage.rolling.unwrap().resets_at.as_unix_millis(), 1);
    let weekly = usage.weekly.unwrap();
    assert_eq!(weekly.used_percent, 99);
    assert_eq!(weekly.resets_at.as_unix_millis(), 951_827_696_789);
    assert_eq!(usage.monthly.unwrap().used_percent, 100);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    server.verify().await;
}

#[tokio::test]
async fn go_usage_rejects_malformed_windows_and_preserves_auth_boundaries() {
    use pawork_providers::{fetch_go_usage, verify_api_key};
    use serde_json::json;
    let server = MockServer::start().await;
    let config = || config_for(channel_preset("opencode-go").unwrap(), server.uri());
    // 不合法字段均只损坏所在窗口；同一解析器也禁止验证入口接受这些响应。
    let mut malformed = vec![
        ("percent", json!(-1)),
        ("percent", json!(101)),
        ("percent", json!(1.5)),
        ("percent", json!(1.0)),
        ("percent", json!("1")),
        ("percent", json!(100)),
        ("status", json!("rate-limited")),
        ("status", json!("sk-channel-test")),
    ];
    for reset in [
        "2026-02-29T00:00:00.000Z",
        "2100-02-29T00:00:00.000Z",
        "2026-04-31T00:00:00.000Z",
        "2026-00-01T00:00:00.000Z",
        "2026-01-00T00:00:00.000Z",
        "2026-01-01T24:00:00.000Z",
        "2026-01-01T00:60:00.000Z",
        "2026-01-01T00:00:60.000Z",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00.000+00:00",
        "1969-12-31T23:59:59.999Z",
        "sk-channel-test",
    ] {
        malformed.push(("resetsAt", json!(reset)));
    }
    for (field, value) in malformed {
        let mut body = go_usage_body();
        body["usage"]["rolling"][field] = value;
        Mock::given(method("GET"))
            .and(path("/usage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(2)
            .mount(&server)
            .await;
        let usage = fetch_go_usage(config(), &api_key(), CancellationToken::new())
            .await
            .unwrap();
        let error = usage.rolling.unwrap_err();
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
        assert!(!format!("{error:?}").contains("sk-channel-test"));
        assert!(usage.weekly.is_ok() && usage.monthly.is_ok());
        assert!(verify_api_key(config(), "sk-channel-test").await.is_err());
        server.verify().await;
        server.reset().await;
    }
    for body in [json!({}), json!({"usage": []})] {
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            fetch_go_usage(config(), &api_key(), CancellationToken::new())
                .await
                .unwrap_err()
                .kind,
            ProviderErrorKind::InvalidRequest
        );
        server.verify().await;
        server.reset().await;
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        fetch_go_usage(config(), &api_key(), cancel)
            .await
            .unwrap_err()
            .kind,
        ProviderErrorKind::Cancelled
    );
    let wrong_key = ResolvedCredential::new(CredentialKind::OAuthBearer, "sk-channel-test");
    assert_eq!(
        fetch_go_usage(config(), &wrong_key, CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        ProviderErrorKind::Authentication
    );
    assert_eq!(
        fetch_go_usage(
            config_for(channel_preset("deepseek").unwrap(), server.uri()),
            &api_key(),
            CancellationToken::new()
        )
        .await
        .unwrap_err()
        .kind,
        ProviderErrorKind::InvalidRequest
    );
    let mut fixed = config();
    fixed
        .http
        .extra_headers
        .push(("Authorization".into(), "sk-channel-test".into()));
    assert_eq!(
        fetch_go_usage(fixed, &api_key(), CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        ProviderErrorKind::InvalidRequest
    );
    assert!(server.received_requests().await.unwrap().is_empty());
    for status in [401, 403, 302] {
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("location", format!("{}/redirect", server.uri()))
                    .set_body_string("sk-channel-test"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let error = fetch_go_usage(config(), &api_key(), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("sk-channel-test"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        server.verify().await;
        server.reset().await;
    }
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(go_usage_body())
                .set_delay(std::time::Duration::from_secs(2)),
        )
        .mount(&server)
        .await;
    let cancel = CancellationToken::new();
    let credential = api_key();
    let fetch = fetch_go_usage(config(), &credential, cancel.clone());
    let cancel_later = async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();
    };
    let (result, _) = tokio::join!(fetch, cancel_later);
    assert_eq!(result.unwrap_err().kind, ProviderErrorKind::Cancelled);

    // 正文每 20ms 都有数据，不能靠 100ms read_timeout 结束；总期限必须生效。
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let drip = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n").unwrap();
        for _ in 0..25 {
            if stream.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    let config = config_for(
        channel_preset("opencode-go").unwrap(),
        format!("http://{address}"),
    )
    .with_request_timeout(std::time::Duration::from_millis(100));
    let error = fetch_go_usage(config, &credential, CancellationToken::new())
        .await
        .unwrap_err();
    drip.join().unwrap();
    assert_eq!(error.kind, ProviderErrorKind::Timeout);
    assert_eq!(error.message, "Go usage request exceeded total timeout");
}
