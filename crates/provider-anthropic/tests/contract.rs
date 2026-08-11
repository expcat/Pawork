//! Anthropic 原生 Provider Contract Tests（P6-2）。
//!
//! 复用统一断言覆盖 ADR-015 核心用例，并额外验证 Anthropic 特性：thinking
//! 流（P6-5）、图片输入（P6-6）、prompt cache 命中（P6-7）、provider_options
//! 透传（P6-9）。全程 wiremock，不接触真实网络与 Key。

use std::collections::BTreeMap;

use agent_domain::{
    ContentPart, ImageContent, ImageSource, Message, MessageId, MessageMetadata, MessageRole,
    ModelId, StopReason, TextContent,
};
use provider_anthropic::{AnthropicConfig, AnthropicProvider};
use provider_api::ModelProvider;
use provider_api::{
    CanonicalModelRequest, CredentialKind, PromptCachePreference, ProviderErrorKind,
    ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat, ThinkingConfig,
    ThinkingLevel, ToolChoice, ToolDefinition,
};
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
        reasoning: None,
    }
}

fn provider(server: &MockServer) -> AnthropicProvider {
    let config = AnthropicConfig {
        base_url: server.uri(),
        http: HttpClientConfig::builder().disable_system_proxy().build(),
        ..AnthropicConfig::default()
    };
    AnthropicProvider::new(
        config,
        Some(ResolvedCredential::new(
            CredentialKind::ApiKey,
            "sk-ant-test",
        )),
    )
    .expect("构造 adapter")
}

fn sse(events: &[&str]) -> String {
    let mut body = String::new();
    for ev in events {
        body.push_str("event: message\n");
        body.push_str("data: ");
        body.push_str(ev);
        body.push_str("\n\n");
    }
    body
}

async fn mount_ok(server: &MockServer, body: String) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-test"))
        .and(header("anthropic-version", "2023-06-01"))
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
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(
            request("claude-3-5-sonnet"),
            &sink,
            agent_domain::CancellationToken::new(),
        )
        .await
        .expect("stream ok");
    contract::assert_text_stream(&sink.events());
    assert_eq!(summary.stop_reason, StopReason::Completed);
    assert_eq!(p.id().as_str(), "anthropic");
}

#[tokio::test]
async fn contract_thinking_delta_streams() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me think"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"text"}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}"#,
        r#"{"type":"content_block_stop","index":1}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(
        request("claude-3-5-sonnet"),
        &sink,
        agent_domain::CancellationToken::new(),
    )
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
    let block_start = serde_json::json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type":"tool_use","id":"toolu_1","name":"read"}
    })
    .to_string();
    let delta1 = serde_json::json!({
        "type":"content_block_delta","index":0,
        "delta":{"type":"input_json_delta","partial_json":"{\"p\":"}
    })
    .to_string();
    let delta2 = serde_json::json!({
        "type":"content_block_delta","index":0,
        "delta":{"type":"input_json_delta","partial_json":"\"a\"}"}
    })
    .to_string();
    let block_stop = serde_json::json!({"type":"content_block_stop","index":0}).to_string();

    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        &block_start,
        &delta1,
        &delta2,
        &block_stop,
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(
        request("claude-3-5-sonnet"),
        &sink,
        agent_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    contract::assert_single_tool_call(&sink.events());
}

#[tokio::test]
async fn contract_parallel_tool_calls() {
    let start_a = serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_a","name":"r"}}).to_string();
    let start_b = serde_json::json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_b","name":"w"}}).to_string();
    let stop_a = serde_json::json!({"type":"content_block_stop","index":0}).to_string();
    let stop_b = serde_json::json!({"type":"content_block_stop","index":1}).to_string();

    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        &start_a,
        &start_b,
        &stop_a,
        &stop_b,
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(
        request("claude-3-5-sonnet"),
        &sink,
        agent_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    contract::assert_parallel_tool_calls(&sink.events());
}

#[tokio::test]
async fn contract_usage_and_stop_reason_with_cache() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":80,"cache_creation_input_tokens":10}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(
        request("claude-3-5-sonnet"),
        &sink,
        agent_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    let events = sink.events();
    contract::assert_usage_and_stop(&events, StopReason::MaxTokens);
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::UsageUpdated(u) if u.cache_read_tokens == 80 && u.cache_write_tokens == 10)));
}

#[tokio::test]
async fn contract_image_input_is_passed_through() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let mut req = request("claude-3-5-sonnet");
    req.messages[0]
        .content
        .push(ContentPart::Image(ImageContent {
            source: ImageSource::Base64("QkFTRQ==".into()),
            media_type: "image/png".into(),
            alt_text: None,
        }));

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    let blocks = body["messages"][0]["content"].as_array().expect("数组");
    assert!(blocks.iter().any(|b| {
        b["type"] == "image" && b["source"]["type"] == "base64" && b["source"]["data"] == "QkFTRQ=="
    }));
}

#[tokio::test]
async fn contract_thinking_request_enables_thinking() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let mut req = request("claude-3-5-sonnet");
    req.max_output_tokens = Some(4096);
    req.thinking = Some(ThinkingConfig {
        level: ThinkingLevel::Medium,
        budget_tokens: Some(2048),
    });

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 2048);
    assert!(
        body["thinking"]["budget_tokens"].as_u64().unwrap() < body["max_tokens"].as_u64().unwrap()
    );
}

#[tokio::test]
async fn contract_prompt_cache_marks_blocks() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let mut req = request("claude-3-5-sonnet");
    req.prompt_cache = PromptCachePreference::Required;
    req.messages.insert(
        0,
        Message {
            id: MessageId::new("sys"),
            role: MessageRole::System,
            content: vec![ContentPart::Text(TextContent { text: "sys".into() })],
            metadata: MessageMetadata::default(),
        },
    );
    req.messages.push(user("follow-up"));
    req.tools.push(ToolDefinition {
        name: "read_file".into(),
        description: "read a file".into(),
        input_schema: serde_json::json!({"type":"object"}),
    });

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(body["system"]["cache_control"]["type"], "ephemeral");
    let user_blocks = body["messages"][0]["content"].as_array().unwrap();
    let last_block = user_blocks.last().unwrap();
    assert_eq!(last_block["cache_control"]["type"], "ephemeral");
    assert!(body["messages"][1]["content"][0]
        .get("cache_control")
        .is_none());
    assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
    assert_eq!(body.to_string().matches("cache_control").count(), 3);
}

#[tokio::test]
async fn contract_structured_output_injects_schema_instruction() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let mut req = request("claude-3-5-sonnet");
    req.response_format = ResponseFormat::JsonSchema {
        name: "answer".into(),
        schema: serde_json::json!({"type":"object","required":["ok"]}),
    };

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    let instruction = body["system"]["text"].as_str().expect("system instruction");
    assert!(instruction.contains("JSON Schema named `answer`"));
    assert!(instruction.contains("\"required\":[\"ok\"]"));
}

#[tokio::test]
async fn contract_provider_options_pass_through() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let mut req = request("claude-3-5-sonnet");
    req.provider_options
        .insert("top_k".into(), serde_json::json!(40));

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    let body = last_request_body(&server).await;
    assert_eq!(body["top_k"], 40);
}

#[tokio::test]
async fn contract_cancel_returns_cancelled() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let cancel = agent_domain::CancellationToken::new();
    cancel.cancel();
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(request("claude-3-5-sonnet"), &sink, cancel)
        .await
        .expect_err("取消应失败");
    assert_eq!(err.kind, ProviderErrorKind::Cancelled);
}

#[tokio::test]
async fn contract_rate_limit_is_normalized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
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
            request("claude-3-5-sonnet"),
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
async fn contract_missing_message_stop_is_interrupted() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(
            request("claude-3-5-sonnet"),
            &sink,
            agent_domain::CancellationToken::new(),
        )
        .await
        .expect_err("缺 message_stop 应 StreamInterrupted");
    assert_eq!(err.kind, ProviderErrorKind::StreamInterrupted);
}

#[tokio::test]
async fn contract_list_models_returns_native_catalog() {
    let server = MockServer::start().await;
    let p = provider(&server);
    let models = p.list_models(None).await.expect("list models");
    assert!(models
        .iter()
        .any(|m| m.id == ModelId::new("claude-3-5-sonnet") && m.capabilities.thinking));
}
