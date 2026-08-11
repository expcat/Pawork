//! Qwen unified contract and OpenAI-compatible differential tests (P6-12).

use std::collections::BTreeMap;

use agent_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    StopReason, TextContent,
};
use provider_api::{
    CanonicalModelRequest, CredentialKind, ModelProvider, PromptCachePreference, ProviderErrorKind,
    ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat, ThinkingConfig,
    ThinkingLevel, ToolChoice,
};
use provider_openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use provider_qwen::{QwenConfig, QwenProvider};
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

fn request() -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: agent_domain::RequestId::from("r1"),
        model: ModelId::from("qwen3-max"),
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

fn provider(server: &MockServer) -> QwenProvider {
    QwenProvider::new(
        QwenConfig {
            base_url: server.uri(),
            http: HttpClientConfig::builder().disable_system_proxy().build(),
            ..QwenConfig::default()
        },
        Some(ResolvedCredential::new(
            CredentialKind::ApiKey,
            "sk-dashscope-test",
        )),
    )
    .expect("Qwen adapter")
}

fn baseline(server: &MockServer) -> OpenAiCompatibleProvider {
    let mut config = OpenAiCompatibleConfig::new(server.uri()).with_provider_id("qwen-baseline");
    config.http = HttpClientConfig::builder().disable_system_proxy().build();
    OpenAiCompatibleProvider::new(
        config,
        Some(ResolvedCredential::new(
            CredentialKind::ApiKey,
            "sk-dashscope-test",
        )),
    )
    .expect("baseline adapter")
}

fn sse(chunks: &[&str]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(chunk);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

async fn mount_ok(server: &MockServer, body: String) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-dashscope-test"))
        .and(header("x-trace-id", "trace-1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

async fn request_bodies(server: &MockServer) -> Vec<serde_json::Value> {
    server
        .received_requests()
        .await
        .expect("recorded requests")
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("JSON request body"))
        .collect()
}

#[tokio::test]
async fn contract_text_stream_and_provider_id() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[
            r#"{"choices":[{"delta":{"content":"Hello"}}]}"#,
            r#"{"choices":[{"delta":{"content":" world"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]),
    )
    .await;

    let provider = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = provider
        .stream(request(), &sink, CancellationToken::new())
        .await
        .expect("stream");
    contract::assert_text_stream(&sink.events());
    assert_eq!(summary.stop_reason, StopReason::Completed);
    assert_eq!(provider.id().as_str(), "qwen");
}

#[tokio::test]
async fn contract_single_and_parallel_tool_calls() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c2","function":{"name":"write","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ]),
    )
    .await;

    let sink = RecordingProviderSink::default();
    provider(&server)
        .stream(request(), &sink, CancellationToken::new())
        .await
        .expect("stream");
    contract::assert_single_tool_call(&sink.events());
    contract::assert_parallel_tool_calls(&sink.events());
}

#[tokio::test]
async fn contract_usage_stop_and_thinking_delta() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[
            r#"{"choices":[{"delta":{"reasoning_content":"think"}}]}"#,
            r#"{"choices":[{"delta":{"content":"answer"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"length"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        ]),
    )
    .await;

    let sink = RecordingProviderSink::default();
    provider(&server)
        .stream(request(), &sink, CancellationToken::new())
        .await
        .expect("stream");
    let events = sink.events();
    contract::assert_usage_and_stop(&events, StopReason::MaxTokens);
    assert!(events
        .iter()
        .any(|event| matches!(event, ProviderStreamEvent::ThinkingDelta(text) if text == "think")));
}

#[tokio::test]
async fn qwen_enable_thinking_passes_through_provider_options_only_when_supplied() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#]),
    )
    .await;

    let mut enabled = request();
    enabled
        .provider_options
        .insert("enable_thinking".into(), serde_json::json!(true));
    provider(&server)
        .stream(
            enabled,
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream");

    provider(&server)
        .stream(
            request(),
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect("stream");

    let bodies = request_bodies(&server).await;
    assert_eq!(bodies[0]["enable_thinking"], true);
    assert!(bodies[1].get("enable_thinking").is_none());
}

#[tokio::test]
async fn canonical_thinking_maps_to_qwen_switch_only_for_capable_models() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#]),
    )
    .await;

    let mut enabled = request();
    enabled.thinking = Some(ThinkingConfig {
        level: ThinkingLevel::High,
        budget_tokens: None,
    });
    provider(&server)
        .stream(
            enabled,
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect("thinking stream");

    let mut unsupported = request();
    unsupported.model = ModelId::new("qwen-turbo");
    unsupported.thinking = Some(ThinkingConfig {
        level: ThinkingLevel::High,
        budget_tokens: None,
    });
    provider(&server)
        .stream(
            unsupported,
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect("non-thinking stream");

    let bodies = request_bodies(&server).await;
    assert_eq!(bodies[0]["enable_thinking"], true);
    assert!(bodies[0].get("reasoning_effort").is_none());
    assert!(bodies[1].get("enable_thinking").is_none());
    assert!(bodies[1].get("reasoning_effort").is_none());
}

#[tokio::test]
async fn contract_cancellation_and_errors_are_normalized() {
    let server = MockServer::start().await;
    let provider = provider(&server);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let sink = RecordingProviderSink::default();
    let error = provider
        .stream(request(), &sink, cancel)
        .await
        .expect_err("cancelled");
    contract::assert_error_kind(&sink.events(), Some(&error), ProviderErrorKind::Cancelled);
    assert!(server
        .received_requests()
        .await
        .expect("recorded requests")
        .is_empty());

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "3")
                .set_body_string("Throttling.RateQuota"),
        )
        .mount(&server)
        .await;
    let error = provider
        .stream(
            request(),
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect_err("rate limited");
    assert_eq!(error.kind, ProviderErrorKind::RateLimited);
    assert_eq!(error.retry_after_ms, Some(3_000));

    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string("DataInspectionFailed"))
        .mount(&server)
        .await;
    let error = provider
        .stream(
            request(),
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect_err("filtered");
    assert_eq!(error.kind, ProviderErrorKind::ContentFiltered);
}

#[tokio::test]
async fn contract_interrupted_stream_is_normalized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"),
        )
        .mount(&server)
        .await;
    let error = provider(&server)
        .stream(
            request(),
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect_err("interrupted");
    assert_eq!(error.kind, ProviderErrorKind::StreamInterrupted);
}

#[tokio::test]
async fn differential_matches_openai_compatible_request_events_and_summary() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[
            r#"{"choices":[{"delta":{"reasoning_content":"think"}}]}"#,
            r#"{"choices":[{"delta":{"content":"answer"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":2}}"#,
        ]),
    )
    .await;
    let mut input = request();
    input
        .provider_options
        .insert("enable_thinking".into(), serde_json::json!(true));
    input
        .provider_options
        .insert("enable_search".into(), serde_json::json!(false));

    let qwen_sink = RecordingProviderSink::default();
    let qwen_summary = provider(&server)
        .stream(input.clone(), &qwen_sink, CancellationToken::new())
        .await
        .expect("Qwen stream");
    let baseline_sink = RecordingProviderSink::default();
    let baseline_summary = baseline(&server)
        .stream(input, &baseline_sink, CancellationToken::new())
        .await
        .expect("baseline stream");

    assert_eq!(qwen_sink.events(), baseline_sink.events());
    assert_eq!(qwen_summary.stop_reason, baseline_summary.stop_reason);
    assert_eq!(qwen_summary.usage, baseline_summary.usage);
    let bodies = request_bodies(&server).await;
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0], bodies[1]);
}
