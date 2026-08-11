//! Google Gemini Provider Contract Tests（P6-3）。
//!
//! 复用统一断言覆盖 ADR-015 核心用例，并额外验证 Gemini 系特性：thinking
//! 流（P6-5）、图片输入透传（P6-6）、结构化输出（P6-8）、provider_options
//! 透传（P6-9）、prompt cache 命中（P6-7），以及 Google 特有的 header 认证。
//! 全程 wiremock，不接触真实网络与 Key。

use std::collections::BTreeMap;

use agent_domain::{
    CancellationToken, ContentPart, ImageContent, ImageSource, Message, MessageId, MessageMetadata,
    MessageRole, ModelId, StopReason, TextContent,
};
use provider_api::{
    CanonicalModelRequest, CredentialKind, ModelProvider, PromptCachePreference, ProviderErrorKind,
    ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat, ThinkingConfig,
    ThinkingLevel, ToolChoice,
};
use provider_google::{GoogleConfig, GoogleProvider};
use provider_runtime::http::HttpClientConfig;
use test_support::{contract, RecordingProviderSink};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "gemini-2.5-pro";

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
            reasoning: None,
    }
}

fn provider(server: &MockServer) -> GoogleProvider {
    let config = GoogleConfig {
        base_url: server.uri(),
        http: HttpClientConfig::builder().disable_system_proxy().build(),
        ..GoogleConfig::default()
    };
    GoogleProvider::new(
        config,
        Some(ResolvedCredential::new(CredentialKind::ApiKey, "test-key")),
    )
    .expect("构造 adapter")
}

/// 拼装 Gemini 风格 SSE 正文（无 [DONE]；末尾由 finishReason 收尾）。
fn sse_body(chunks: &[&str]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(chunk);
        body.push_str("\n\n");
    }
    body
}

fn stream_path(model: &str) -> String {
    format!("/v1beta/models/{model}:streamGenerateContent")
}

async fn mount_stream_ok(server: &MockServer, model: &str, body: String) {
    Mock::given(method("POST"))
        .and(path(stream_path(model)))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "test-key"))
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

async fn last_request_url(server: &MockServer) -> String {
    let requests = server
        .received_requests()
        .await
        .expect("wiremock records requests");
    requests
        .last()
        .expect("at least one request")
        .url
        .as_str()
        .to_string()
}

#[tokio::test]
async fn contract_text_stream() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":" world"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(request(MODEL), &sink, CancellationToken::new())
        .await
        .expect("stream ok");
    contract::assert_text_stream(&sink.events());
    assert_eq!(summary.stop_reason, StopReason::Completed);
    assert_eq!(p.id().as_str(), "google");
}

#[tokio::test]
async fn contract_auth_key_in_header_and_absent_from_url() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(request(MODEL), &sink, CancellationToken::new())
        .await
        .expect("stream ok");

    let url = last_request_url(&server).await;
    assert!(url.contains("alt=sse"), "URL 缺少 alt=sse：{url}");
    assert!(!url.contains("key="), "URL 不应包含 API key：{url}");
}

#[tokio::test]
async fn contract_thinking_via_thought_part() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"thought":true,"text":"let me think"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"answer"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(request(MODEL), &sink, CancellationToken::new())
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
    // 用 serde_json::json! 构造 chunk，保证 function_call 的 args 始终是合法 JSON。
    let chunk1 = serde_json::json!({
        "candidates": [{"content": {"role": "model", "parts": [{"functionCall": {"name": "read", "args": {"path": "a.txt"}}}]}}]
    })
    .to_string();
    let chunk2 = serde_json::json!({
        "candidates": [{"content": {"role": "model", "parts": []}, "finishReason": "STOP"}]
    })
    .to_string();

    let server = MockServer::start().await;
    let body = sse_body(&[&chunk1, &chunk2]);
    mount_stream_ok(&server, MODEL, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(request(MODEL), &sink, CancellationToken::new())
        .await
        .expect("stream ok");
    contract::assert_single_tool_call(&sink.events());
    // 有 functionCall 时 stop 应归一为 ToolUse。
    assert_eq!(summary.stop_reason, StopReason::ToolUse);
}

#[tokio::test]
async fn contract_parallel_tool_calls() {
    // 一个 chunk 内含两个 functionCall（Gemini 常见并行模式）。
    let chunk1 = serde_json::json!({
        "candidates": [{"content": {"role": "model", "parts": [
            {"functionCall": {"name": "read", "args": {}}},
            {"functionCall": {"name": "write", "args": {}}}
        ]}}]
    })
    .to_string();
    let chunk2 = serde_json::json!({
        "candidates": [{"content": {"role": "model", "parts": []}, "finishReason": "STOP"}]
    })
    .to_string();

    let server = MockServer::start().await;
    let body = sse_body(&[&chunk1, &chunk2]);
    mount_stream_ok(&server, MODEL, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(request(MODEL), &sink, CancellationToken::new())
        .await
        .expect("stream ok");
    contract::assert_parallel_tool_calls(&sink.events());
    let calls = summary.provider_metadata["toolCalls"]
        .as_array()
        .expect("保留 toolCalls 元数据");
    assert_eq!(calls[0]["id"], "gemini-call-0");
    assert_eq!(calls[0]["name"], "read");
    assert_eq!(calls[0]["ordinal"], 0);
    assert_eq!(calls[1]["id"], "gemini-call-1");
    assert_eq!(calls[1]["name"], "write");
    assert_eq!(calls[1]["ordinal"], 1);
}

#[tokio::test]
async fn contract_usage_and_stop_reason_with_cache() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"x"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"cachedContentTokenCount":3}}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(request(MODEL), &sink, CancellationToken::new())
        .await
        .expect("stream ok");
    let events = sink.events();
    contract::assert_usage_and_stop(&events, StopReason::MaxTokens);
    // P6-7：cachedContentTokenCount 应体现在 cache_read_tokens。
    let usage = events
        .iter()
        .find_map(|e| match e {
            ProviderStreamEvent::UsageUpdated(u) => Some(u.clone()),
            _ => None,
        })
        .expect("有 UsageUpdated");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(usage.cache_read_tokens, 3);
}

#[tokio::test]
async fn contract_cancel_returns_cancelled() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"partial"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = p
        .stream(request(MODEL), &sink, cancel)
        .await
        .expect_err("取消应失败");
    assert_eq!(err.kind, ProviderErrorKind::Cancelled);
}

#[tokio::test]
async fn contract_rate_limit_is_normalized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(stream_path(MODEL)))
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
        .stream(request(MODEL), &sink, CancellationToken::new())
        .await
        .expect_err("429 应失败");
    assert_eq!(err.kind, ProviderErrorKind::RateLimited);
    assert!(err.retryable);
    assert_eq!(err.retry_after_ms, Some(5_000));
}

#[tokio::test]
async fn contract_missing_finish_returns_stream_interrupted() {
    let server = MockServer::start().await;
    // 缺少 finishReason，流即结束 → StreamInterrupted。
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"partial"}]}}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(request(MODEL), &sink, CancellationToken::new())
        .await
        .expect_err("缺 finishReason 应失败");
    assert_eq!(err.kind, ProviderErrorKind::StreamInterrupted);
}

#[tokio::test]
async fn contract_image_input_is_passed_through() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let mut req = request(MODEL);
    req.messages[0]
        .content
        .push(ContentPart::Image(ImageContent {
            source: ImageSource::Base64("QkFTRTY0".into()),
            media_type: "image/png".into(),
            alt_text: None,
        }));

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    let parts = body["contents"][0]["parts"].as_array().expect("parts 数组");
    assert!(parts.iter().any(|p| {
        p["inlineData"]["mimeType"] == "image/png" && p["inlineData"]["data"] == "QkFTRTY0"
    }));
}

#[tokio::test]
async fn contract_structured_output_json_schema() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"{\"ok\":true}"}]},"finishReason":"STOP"}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let mut req = request(MODEL);
    req.response_format = ResponseFormat::JsonSchema {
        name: "result".into(),
        schema: serde_json::json!({"type": "object"}),
    };

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(body["generationConfig"]["responseSchema"]["type"], "object");
}

#[tokio::test]
async fn contract_thinking_level_maps_to_budget() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let mut req = request(MODEL);
    req.thinking = Some(ThinkingConfig {
        level: ThinkingLevel::High,
        budget_tokens: None,
    });

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
        8192
    );
}

#[tokio::test]
async fn contract_provider_options_pass_through() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"ok"}]},"finishReason":"STOP"}]}"#,
    ]);
    mount_stream_ok(&server, MODEL, body).await;

    let mut req = request(MODEL);
    req.provider_options
        .insert("topP".into(), serde_json::json!(0.9));
    req.provider_options
        .insert("topK".into(), serde_json::json!(40));

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(body["generationConfig"]["topP"], 0.9);
    assert_eq!(body["generationConfig"]["topK"], 40);
}

#[tokio::test]
async fn contract_list_models_returns_native_catalog() {
    let server = MockServer::start().await;
    let p = provider(&server);
    let models = p.list_models(None).await.expect("list models");
    assert!(models
        .iter()
        .any(|m| m.id == ModelId::new(MODEL) && m.capabilities.thinking));
    assert!(models.iter().any(|m| m.capabilities.image_input));
}
