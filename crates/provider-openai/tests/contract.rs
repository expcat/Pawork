//! OpenAI 原生 Provider Contract Tests（P6-1）。
//!
//! 复用统一断言覆盖 ADR-015 核心用例，并额外验证 OpenAI 系特性：
//! reasoning 流（P6-5）、图片输入（P6-6）、结构化输出（P6-8）、provider_options
//! 透传（P6-9）。全程 wiremock，不接触真实网络与 Key。

use std::collections::BTreeMap;

use agent_domain::{
    ContentPart, ImageContent, ImageSource, Message, MessageId, MessageMetadata, MessageRole,
    ModelId, StopReason, TextContent,
};
use provider_api::ModelProvider;
use provider_api::{
    CanonicalModelRequest, CredentialKind, PromptCachePreference, ProviderErrorKind,
    ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat, ThinkingConfig,
    ThinkingLevel, ToolChoice,
};
use provider_openai::{OpenAiConfig, OpenAiProvider};
use provider_runtime::http::HttpClientConfig;
use test_support::{contract, RecordingProviderSink};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn user(text: &str) -> Message {
    Message {
        id: MessageId::new("m1"),
        role: MessageRole::User,
        content: vec![ContentPart::Text(TextContent { text: text.into() })],
        metadata: MessageMetadata::default(),
    }
}

fn request(model: &str) -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: agent_domain::RequestId::from("r1"),
        model: ModelId::from(model),
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
    }
}

fn provider(server: &MockServer) -> OpenAiProvider {
    let config = OpenAiConfig {
        base_url: server.uri(),
        http: HttpClientConfig::builder().disable_system_proxy().build(),
        ..OpenAiConfig::default()
    };
    OpenAiProvider::new(
        config,
        Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-test")),
    )
    .expect("构造 adapter")
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

async fn mount_chat_ok(server: &MockServer, body: String) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

async fn last_request_body(server: &MockServer) -> serde_json::Value {
    let requests = server
        .received_requests()
        .await
        .expect("wiremock records requests");
    let body = &requests.last().expect("at least one request").body;
    serde_json::from_slice(body).expect("请求体为 JSON")
}

#[tokio::test]
async fn contract_text_stream() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"choices":[{"delta":{"content":"Hello"}}]}"#,
        r#"{"choices":[{"delta":{"content":" world"}}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    ]);
    mount_chat_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(
            request("gpt-4o"),
            &sink,
            agent_domain::CancellationToken::new(),
        )
        .await
        .expect("stream ok");
    contract::assert_text_stream(&sink.events());
    assert_eq!(summary.stop_reason, StopReason::Completed);
    assert_eq!(p.id().as_str(), "openai");
}

#[tokio::test]
async fn contract_reasoning_streams_thinking_delta() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"choices":[{"delta":{"reasoning_content":"let me think"}}]}"#,
        r#"{"choices":[{"delta":{"content":"answer"}}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    ]);
    mount_chat_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(request("o1"), &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");
    let events = sink.events();
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::ThinkingDelta(t) if t == "let me think")));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::TextDelta(t) if t == "answer")));
}

#[tokio::test]
async fn contract_single_tool_call() {
    // 用 serde_json::json! 构造，保证跨 chunk 的 tool arguments 始终是合法 JSON。
    let chunk1 = serde_json::json!({
        "choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "read", "arguments": "{\"p\":"}}]}}]
    })
    .to_string();
    let chunk2 = serde_json::json!({
        "choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"a\"}"}}]}}]
    })
    .to_string();
    let chunk3 = serde_json::json!({
        "choices": [{"delta": {}, "finish_reason": "tool_calls"}]
    })
    .to_string();

    let server = MockServer::start().await;
    let body = sse_body(&[&chunk1, &chunk2, &chunk3]);
    mount_chat_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(
        request("gpt-4o"),
        &sink,
        agent_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    contract::assert_single_tool_call(&sink.events());
}

#[tokio::test]
async fn contract_usage_and_stop_reason() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"choices":[{"delta":{"content":"x"}}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"length"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":4}}}"#,
    ]);
    mount_chat_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(
        request("gpt-4o"),
        &sink,
        agent_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    // P6-7：OpenAI 自动命中缓存应在 usage 的 cache_read_tokens 体现
    let events = sink.events();
    contract::assert_usage_and_stop(&events, StopReason::MaxTokens);
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::UsageUpdated(u) if u.cache_read_tokens == 4)));
}

#[tokio::test]
async fn contract_image_input_is_passed_through() {
    let server = MockServer::start().await;
    let body = sse_body(&[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#]);
    mount_chat_ok(&server, body).await;

    let mut req = request("gpt-4o");
    req.messages[0]
        .content
        .push(ContentPart::Image(ImageContent {
            source: ImageSource::Url("https://example.com/x.png".into()),
            media_type: "image/png".into(),
            alt_text: None,
        }));

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    let content = body["messages"][0]["content"].as_array().expect("数组");
    assert!(content.iter().any(|p| {
        p["type"] == "image_url" && p["image_url"]["url"] == "https://example.com/x.png"
    }));
}

#[tokio::test]
async fn contract_provider_options_pass_through() {
    let server = MockServer::start().await;
    let body = sse_body(&[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#]);
    mount_chat_ok(&server, body).await;

    let mut req = request("gpt-4o");
    req.provider_options
        .insert("seed".into(), serde_json::json!(42));
    req.provider_options
        .insert("service_tier".into(), serde_json::json!("default"));

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(body["seed"], 42);
    assert_eq!(body["service_tier"], "default");
}

#[tokio::test]
async fn contract_structured_output_json_schema() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"choices":[{"delta":{"content":"{\"ok\":true}"},"finish_reason":"stop"}]}"#,
    ]);
    mount_chat_ok(&server, body).await;

    let mut req = request("gpt-4o");
    req.response_format = ResponseFormat::JsonSchema {
        name: "result".into(),
        schema: serde_json::json!({"type": "object"}),
    };

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["name"], "result");
}

#[tokio::test]
async fn contract_thinking_level_maps_to_reasoning_effort() {
    let server = MockServer::start().await;
    let body = sse_body(&[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#]);
    mount_chat_ok(&server, body).await;

    let mut req = request("o1");
    req.thinking = Some(ThinkingConfig {
        level: ThinkingLevel::High,
        budget_tokens: None,
    });

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(body["reasoning_effort"], "high");
}

#[tokio::test]
async fn contract_rate_limit_is_normalized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "5")
                .set_body_string("slow down"),
        )
        .mount(&server)
        .await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(
            request("gpt-4o"),
            &sink,
            agent_domain::CancellationToken::new(),
        )
        .await
        .expect_err("429 应失败");
    assert_eq!(err.kind, ProviderErrorKind::RateLimited);
    assert!(err.retryable);
    assert_eq!(err.retry_after_ms, Some(5_000));
}

#[tokio::test]
async fn contract_list_models_returns_native_catalog() {
    let server = MockServer::start().await;
    let p = provider(&server);
    let models = p.list_models(None).await.expect("list models");
    assert!(models.iter().any(|m| m.id == ModelId::new("gpt-4o")));
    assert!(models
        .iter()
        .any(|m| m.id == ModelId::new("o1") && m.capabilities.thinking));
}
