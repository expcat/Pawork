//! xAI Chat Completions contract and OpenAI-compatible differential tests.

use std::collections::BTreeMap;

use agent_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    ProviderId, StopReason, TextContent,
};
use provider_api::{
    CanonicalModelRequest, CredentialKind, ModelProvider, PromptCachePreference, ProviderErrorKind,
    ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat, ToolChoice,
};
use provider_openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use provider_xai::{XaiConfig, XaiProvider};
use test_support::{contract, RecordingProviderSink};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request(model: &str) -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: agent_domain::RequestId::from("xai-r1"),
        model: ModelId::from(model),
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
        temperature: Some(0.0),
        max_output_tokens: Some(128),
        stop_sequences: Vec::new(),
        response_format: ResponseFormat::Text,
        prompt_cache: PromptCachePreference::Automatic,
        budget: RequestBudget::default(),
        provider_options: BTreeMap::new(),
        trace_id: Some("trace-xai".into()),
        reasoning: None,
    }
}

fn provider(server: &MockServer, kind: CredentialKind, secret: &str) -> XaiProvider {
    let mut config = XaiConfig::new(server.uri());
    config.http = provider_runtime::http::HttpClientConfig::builder()
        .disable_system_proxy()
        .build();
    XaiProvider::new(config, Some(ResolvedCredential::new(kind, secret)))
        .expect("build xAI adapter")
}

fn baseline_provider(server: &MockServer) -> OpenAiCompatibleProvider {
    let mut config = OpenAiCompatibleConfig::new(server.uri()).with_provider_id("baseline");
    config.http = provider_runtime::http::HttpClientConfig::builder()
        .disable_system_proxy()
        .build();
    OpenAiCompatibleProvider::new(
        config,
        Some(ResolvedCredential::new(CredentialKind::ApiKey, "xai-key")),
    )
    .expect("build baseline adapter")
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

#[tokio::test]
async fn unified_contract_api_key_text_reasoning_usage_and_stop() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer xai-key"))
        .and(body_partial_json(serde_json::json!({
            "model": "grok-2",
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": [{"role": "user", "content": "hi"}]
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"choices":[{"delta":{"reasoning_content":"think"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"answer"}}]}"#,
                    r#"{"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":4}}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                ])),
        )
        .mount(&server)
        .await;

    let adapter = provider(&server, CredentialKind::ApiKey, "xai-key");
    let sink = RecordingProviderSink::default();
    let summary = adapter
        .stream(request("grok-2"), &sink, CancellationToken::new())
        .await
        .expect("xAI stream succeeds");

    assert_eq!(adapter.id(), ProviderId::new("xai"));
    contract::assert_text_stream(&sink.events());
    contract::assert_usage_and_stop(&sink.events(), StopReason::Completed);
    assert!(sink
        .events()
        .iter()
        .any(|event| matches!(event, ProviderStreamEvent::ThinkingDelta(text) if text == "think")));
    assert_eq!(summary.usage.input_tokens, 9);
    assert_eq!(summary.usage.output_tokens, 4);
}

#[tokio::test]
async fn oauth_credential_is_sent_as_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer oauth-access"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"choices":[{"delta":{"content":"ok"}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                ])),
        )
        .mount(&server)
        .await;

    let adapter = provider(&server, CredentialKind::OAuthBearer, "oauth-access");
    adapter
        .stream(
            request("grok-2"),
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect("OAuth bearer stream succeeds");
}

#[tokio::test]
async fn unified_contract_tool_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"p\":"}}]}}]}"#,
                    r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
                ])),
        )
        .mount(&server)
        .await;

    let sink = RecordingProviderSink::default();
    provider(&server, CredentialKind::ApiKey, "xai-key")
        .stream(request("grok-2"), &sink, CancellationToken::new())
        .await
        .expect("tool stream succeeds");
    contract::assert_single_tool_call(&sink.events());
}

#[tokio::test]
async fn xai_errors_are_normalized_independently() {
    for (status, expected, retry_after) in [
        (401, ProviderErrorKind::Authentication, None),
        (403, ProviderErrorKind::Authorization, None),
        (429, ProviderErrorKind::RateLimited, Some("3")),
    ] {
        let server = MockServer::start().await;
        let mut response = ResponseTemplate::new(status).set_body_string("xAI error");
        if let Some(value) = retry_after {
            response = response.insert_header("retry-after", value);
        }
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(response)
            .mount(&server)
            .await;

        let sink = RecordingProviderSink::default();
        let error = provider(&server, CredentialKind::ApiKey, "xai-key")
            .stream(request("grok-2"), &sink, CancellationToken::new())
            .await
            .expect_err("HTTP error must be normalized");
        contract::assert_error_kind(&sink.events(), Some(&error), expected.clone());
        if status == 429 {
            assert!(error.retryable);
            assert_eq!(error.retry_after_ms, Some(3_000));
        } else {
            assert!(!error.retryable);
        }
    }
}

#[tokio::test]
async fn differential_matches_openai_compatible_baseline() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"choices":[{"delta":{"reasoning_content":"r"}}]}"#,
                    r#"{"choices":[{"delta":{"content":"same"}}]}"#,
                    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                ])),
        )
        .mount(&server)
        .await;

    let xai_sink = RecordingProviderSink::default();
    let baseline_sink = RecordingProviderSink::default();
    let xai_summary = provider(&server, CredentialKind::ApiKey, "xai-key")
        .stream(request("grok-2"), &xai_sink, CancellationToken::new())
        .await
        .expect("xAI stream succeeds");
    let baseline_summary = baseline_provider(&server)
        .stream(request("grok-2"), &baseline_sink, CancellationToken::new())
        .await
        .expect("baseline stream succeeds");

    assert_eq!(xai_sink.events(), baseline_sink.events());
    assert_eq!(xai_summary, baseline_summary);
    let requests = server.received_requests().await.expect("record requests");
    assert_eq!(requests.len(), 2);
    let first: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    let second: serde_json::Value = serde_json::from_slice(&requests[1].body).expect("JSON body");
    assert_eq!(first, second);
}
