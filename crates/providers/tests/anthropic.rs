//! Anthropic Messages 契约测试（S2 基线）。
//!
//! `RecordingProviderSink` 与断言 helper 就地复制自 `tests/contract.rs`，
//! 不依赖 V1 `test-support`。全程 wiremock，不接触真实网络与 Key。

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
use pawork_providers::{AnthropicConfig, AnthropicProvider, ANTHROPIC_VERSION};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

mod contract {
    use pawork_domain::{ProviderError, ProviderErrorKind, ProviderStreamEvent};

    pub fn assert_text_stream(events: &[ProviderStreamEvent]) {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderStreamEvent::TextDelta(t) if !t.is_empty())),
            "text 流应至少含一条非空 TextDelta，实际：{events:?}"
        );
        assert!(
            matches!(
                events.last(),
                Some(ProviderStreamEvent::ResponseCompleted(_))
            ),
            "文本流应以 ResponseCompleted 收尾，实际末尾：{:?}",
            events.last()
        );
    }

    pub fn assert_single_tool_call(events: &[ProviderStreamEvent]) {
        let started = events
            .iter()
            .find(|e| matches!(e, ProviderStreamEvent::ToolCallStarted { .. }));
        assert!(started.is_some(), "应存在 ToolCallStarted");
        let id = match started.unwrap() {
            ProviderStreamEvent::ToolCallStarted { id, .. } => id.clone(),
            _ => unreachable!(),
        };
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProviderStreamEvent::ToolCallCompleted { id: cid } if cid == &id)),
            "tool call {id} 应被 Completed 闭合"
        );
    }

    pub fn assert_parallel_tool_calls(events: &[ProviderStreamEvent]) {
        let started_ids: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ProviderStreamEvent::ToolCallStarted { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert!(
            started_ids.len() >= 2,
            "并行 tool call 应至少有两个 Started（实际 {}）",
            started_ids.len()
        );
        for id in &started_ids {
            assert!(
                events.iter().any(
                    |e| matches!(e, ProviderStreamEvent::ToolCallCompleted { id: cid } if cid == id)
                ),
                "tool call {id} 应被 Completed 闭合"
            );
        }
    }

    pub fn assert_error_kind(
        events: &[ProviderStreamEvent],
        stream_error: Option<&ProviderError>,
        kind: ProviderErrorKind,
    ) {
        let event_matches = events.iter().any(|e| match e {
            ProviderStreamEvent::Error(err) => err.kind == kind,
            _ => false,
        });
        let return_matches = stream_error.is_some_and(|error| error.kind == kind);
        assert!(
            event_matches || return_matches,
            "应存在 kind={kind:?} 的 Error 事件或 stream 返回错误，事件：{events:?}，返回错误：{stream_error:?}"
        );
    }
}

#[derive(Clone)]
struct CancelAfterTextSink {
    inner: RecordingProviderSink,
    cancel: CancellationToken,
}

#[async_trait]
impl ProviderEventSink for CancelAfterTextSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        let should_cancel = matches!(event, ProviderStreamEvent::TextDelta(_));
        self.inner.emit(event).await?;
        if should_cancel {
            self.cancel.cancel();
        }
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

fn request(model: &str) -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: pawork_domain::RequestId::from("r1"),
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
    let mut config = AnthropicConfig::new(server.uri()).with_provider_id("test");
    config.http = pawork_providers::net::http::HttpClientConfig::builder()
        .disable_system_proxy()
        .build();
    AnthropicProvider::new(
        config,
        Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-ant-test")),
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
        .and(header("anthropic-version", ANTHROPIC_VERSION))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
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
        .stream(request("claude-3-5-sonnet"), &sink, CancellationToken::new())
        .await
        .expect("stream ok");
    contract::assert_text_stream(&sink.events());
    assert_eq!(summary.stop_reason, StopReason::Completed);
    assert_eq!(p.id().as_str(), "test");
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
    p.stream(request("claude-3-5-sonnet"), &sink, CancellationToken::new())
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
    p.stream(request("claude-3-5-sonnet"), &sink, CancellationToken::new())
        .await
        .expect("stream ok");
    contract::assert_parallel_tool_calls(&sink.events());
}

#[tokio::test]
async fn contract_cancel_mid_stream() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"must-not-complete"}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let cancel = CancellationToken::new();
    let recording = RecordingProviderSink::default();
    let sink = CancelAfterTextSink {
        inner: recording.clone(),
        cancel: cancel.clone(),
    };
    let err = p
        .stream(request("claude-3-5-sonnet"), &sink, cancel)
        .await
        .expect_err("收到首个 delta 后取消应失败");
    contract::assert_error_kind(&recording.events(), Some(&err), ProviderErrorKind::Cancelled);
    assert!(recording
        .events()
        .iter()
        .any(|event| matches!(event, ProviderStreamEvent::TextDelta(text) if text == "first")));
}

#[tokio::test]
async fn contract_pre_cancel_does_not_send_request() {
    let server = MockServer::start().await;
    mount_ok(&server, sse(&[])).await;

    let p = provider(&server);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(request("claude-3-5-sonnet"), &sink, cancel)
        .await
        .expect_err("预取消应在发送前失败");
    contract::assert_error_kind(&sink.events(), Some(&err), ProviderErrorKind::Cancelled);
    assert!(
        server
            .received_requests()
            .await
            .expect("request recording enabled")
            .is_empty(),
        "预取消不得命中远端"
    );
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
        .stream(request("claude-3-5-sonnet"), &sink, CancellationToken::new())
        .await
        .expect_err("429 应失败");
    contract::assert_error_kind(&sink.events(), Some(&err), ProviderErrorKind::RateLimited);
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
        .stream(request("claude-3-5-sonnet"), &sink, CancellationToken::new())
        .await
        .expect_err("缺 message_stop 应 StreamInterrupted");
    contract::assert_error_kind(
        &sink.events(),
        Some(&err),
        ProviderErrorKind::StreamInterrupted,
    );
}

#[tokio::test]
async fn list_models_is_static_and_does_not_hit_network() {
    let server = MockServer::start().await;
    let p = provider(&server);
    let models = p.list_models(None).await.expect("list models");
    assert!(models
        .iter()
        .any(|model| model.id == ModelId::new("claude-3-5-sonnet")));
    assert!(
        server
            .received_requests()
            .await
            .expect("request recording enabled")
            .is_empty(),
        "list_models 不得请求 /v1/models 或任何远端"
    );
}

#[tokio::test]
async fn contract_prompt_cache_and_thinking_are_written() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let mut req = request("claude-3-5-sonnet");
    req.temperature = Some(1.0);
    req.max_output_tokens = Some(2048);
    req.thinking = Some(pawork_domain::ThinkingConfig {
        level: pawork_domain::ThinkingLevel::High,
        budget_tokens: Some(1024),
    });
    req.prompt_cache = PromptCachePreference::Required;
    req.messages.insert(
        0,
        Message {
            id: MessageId::new("sys"),
            role: MessageRole::System,
            content: vec![ContentPart::Text(TextContent {
                text: "sys".into(),
            })],
            metadata: MessageMetadata::default(),
        },
    );
    p.stream(req, &sink, CancellationToken::new())
        .await
        .expect("stream ok");
    assert!(sink
        .events()
        .iter()
        .any(|event| matches!(event, ProviderStreamEvent::ThinkingDelta(text) if text == "plan")));
    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 1);
    let sent: serde_json::Value = received[0].body_json().expect("json body");
    assert_eq!(
        sent["thinking"],
        serde_json::json!({"type":"enabled","budget_tokens":1024})
    );
    assert_eq!(sent["system"]["cache_control"]["type"], "ephemeral");
}

#[tokio::test]
async fn hosted_tools_are_rejected_before_http() {
    let server = MockServer::start().await;
    mount_ok(&server, sse(&[])).await;
    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let mut req = request("claude-3-5-sonnet");
    req.hosted_tools.push(pawork_domain::HostedToolRequest {
        name: "web_search".into(),
        kind: pawork_domain::ToolCapabilityTag::WebSearch,
        description: "search".into(),
        capabilities: vec![pawork_domain::ToolCapabilityTag::WebSearch],
        config: None,
    });
    let err = p
        .stream(req, &sink, CancellationToken::new())
        .await
        .expect_err("undeclared hosted tools must fail closed");
    assert_eq!(err.kind, ProviderErrorKind::InvalidRequest);
    assert!(
        server
            .received_requests()
            .await
            .expect("request recording enabled")
            .is_empty(),
        "hosted tool reject must not hit HTTP"
    );
}

#[tokio::test]
async fn contract_thinking_signature_is_protected_not_emitted() {
    let server = MockServer::start().await;
    let body = sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","id":"th_1"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-secret"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hello"}}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ]);
    mount_ok(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let mut req = request("claude-3-5-sonnet");
    req.temperature = Some(1.0);
    req.max_output_tokens = Some(2048);
    req.thinking = Some(pawork_domain::ThinkingConfig {
        level: pawork_domain::ThinkingLevel::Low,
        budget_tokens: Some(1024),
    });
    p.stream(req, &sink, CancellationToken::new())
        .await
        .expect("stream ok");
    let events = sink.events();
    assert!(events.iter().any(|event| matches!(
        event,
        ProviderStreamEvent::ThinkingDelta(text) if text == "plan"
    )));
    let item = events.iter().find_map(|event| match event {
        ProviderStreamEvent::ReasoningItem(item) => Some(item),
        _ => None,
    });
    let item = item.expect("reasoning item");
    assert_eq!(item.id.as_str(), "th_1");
    assert!(!item.protected_blob_ref.as_str().is_empty());
    assert_eq!(
        item.continuation_metadata["provider_hints.anthropic.model"],
        serde_json::json!("claude-3-5-sonnet")
    );
    let dumped = format!("{events:?}");
    assert!(!dumped.contains("sig-secret"), "signature must not leak into events: {dumped}");
}
