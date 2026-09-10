//! Provider Contract Tests：用 wiremock 驱动 OpenAI-compatible 适配器。
//!
//! 覆盖 ADR-015 用例集：text、tool call、multiple tool calls、usage+stop、
//! cancel、timeout、rate limit、malformed stream、partial JSON、reconnect、context overflow。
//! 全程不接触真实网络与真实 auth 文件。
//!
//! 流断言与 SSE 样例收敛于 `tests/common`（MOCK-7）；`RecordingProviderSink`
//! 就地复制自 V1 `test-support`，不依赖 V1 crate。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::ModelProvider;
use pawork_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    StopReason, TextContent,
};
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, PromptCachePreference, ProviderError, ProviderErrorKind,
    ProviderEventSink, ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat,
    ToolChoice,
};
use pawork_providers::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

mod common;

use common::contract;

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

/// 匹配「请求不带 Authorization 头」。
struct NoAuthorizationHeader;

impl Match for NoAuthorizationHeader {
    fn matches(&self, request: &Request) -> bool {
        request.headers.get("authorization").is_none()
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
        session_id: None,
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

fn provider(server: &MockServer, timeout: Option<Duration>) -> OpenAiCompatibleProvider {
    provider_at(server.uri(), timeout)
}

fn provider_at(base_url: impl Into<String>, timeout: Option<Duration>) -> OpenAiCompatibleProvider {
    let mut config = OpenAiCompatibleConfig::new(base_url).with_provider_id("test");
    // 测试环境禁用系统代理，避免 NO_PROXY/系统代理干扰本地 mock server
    config.http = pawork_providers::net::http::HttpClientConfig::builder()
        .disable_system_proxy()
        .build();
    if let Some(t) = timeout {
        config.request_timeout = Some(t);
    }
    OpenAiCompatibleProvider::new(
        config,
        Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-test")),
    )
    .expect("构造 adapter")
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

fn spawn_slow_chunked_server(
    chunk_delay: Duration,
) -> (String, thread::JoinHandle<std::io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow stream server");
    let address = listener.local_addr().expect("slow stream address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut request_bytes = [0_u8; 8192];
        let _ = stream.read(&mut request_bytes)?;
        stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )?;
        stream.flush()?;

        let chunks = [
            "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"c\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        ];
        for chunk in chunks {
            thread::sleep(chunk_delay);
            write!(stream, "{:X}\r\n", chunk.len())?;
            stream.write_all(chunk.as_bytes())?;
            stream.write_all(b"\r\n")?;
            stream.flush()?;
        }
        stream.write_all(b"0\r\n\r\n")?;
        Ok(())
    });
    (format!("http://{address}"), handle)
}

async fn mount_chat_ok(server: &MockServer, body: String) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .and(header("x-trace-id", "trace-1"))
        .and(body_partial_json(serde_json::json!({
            "stream_options": { "include_usage": true }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn contract_parallel_tool_calls() {
    let server = MockServer::start().await;
    let body = common::sse_body(&[
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c_a","function":{"name":"r","arguments":"{}"}}]} }]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c_b","function":{"name":"w","arguments":"{}"}}]} }]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    ]);
    mount_chat_ok(&server, body).await;

    let p = provider(&server, None);
    let sink = RecordingProviderSink::default();
    p.stream(
        request("gpt-4o"),
        &sink,
        pawork_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    contract::assert_parallel_tool_calls(&sink.events());
}

/// MOCK-7 覆盖决策：文本流 / usage+stop / 工具调用三切面已合并至
/// api_key_channels 的五通道表驱动用例（需显式 features），此处引用
/// common 样例保持默认死表（无 features）的 happy-path 覆盖，不复制样例。
#[tokio::test]
async fn contract_chat_facets_default_coverage() {
    let server = MockServer::start().await;
    let p = provider(&server, None);

    // 文本流：非空 TextDelta + ResponseCompleted 收尾。
    mount_chat_ok(&server, common::chat_text_stream_body()).await;
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(
            request("gpt-4o"),
            &sink,
            pawork_domain::CancellationToken::new(),
        )
        .await
    .expect("stream ok");
    contract::assert_text_stream(&sink.events());
    assert_eq!(summary.stop_reason, StopReason::Completed);

    // usage + stop：usage chunk 在 finish 之后到达，独立断言。
    server.reset().await;
    mount_chat_ok(&server, common::chat_usage_stop_body()).await;
    let usage_sink = RecordingProviderSink::default();
    p.stream(
        request("gpt-4o"),
        &usage_sink,
        pawork_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    contract::assert_usage_and_stop(&usage_sink.events(), StopReason::MaxTokens);

    // 单工具调用：Started → Completed 闭合。
    server.reset().await;
    mount_chat_ok(&server, common::chat_tool_call_body()).await;
    let tool_sink = RecordingProviderSink::default();
    p.stream(
        request("gpt-4o"),
        &tool_sink,
        pawork_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    contract::assert_single_tool_call(&tool_sink.events());
}

#[tokio::test]
async fn contract_cancel_mid_stream() {
    let server = MockServer::start().await;
    let body = common::sse_body(&[
        r#"{"choices":[{"delta":{"content":"first"}}]}"#,
        r#"{"choices":[{"delta":{"content":"must-not-complete"}}]}"#,
    ]);
    mount_chat_ok(&server, body).await;

    let p = provider(&server, None);
    let cancel = CancellationToken::new();
    let recording = RecordingProviderSink::default();
    let sink = CancelAfterTextSink {
        inner: recording.clone(),
        cancel: cancel.clone(),
    };
    let err = p
        .stream(request("gpt-4o"), &sink, cancel)
        .await
        .expect_err("收到首个 delta 后取消应失败");
    contract::assert_error_kind(
        &recording.events(),
        Some(&err),
        ProviderErrorKind::Cancelled,
    );
    assert!(recording
        .events()
        .iter()
        .any(|event| matches!(event, ProviderStreamEvent::TextDelta(text) if text == "first")));
}

#[tokio::test]
async fn contract_pre_cancel_does_not_send_request() {
    let server = MockServer::start().await;
    mount_chat_ok(&server, common::sse_body(&[])).await;

    let p = provider(&server, None);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(request("gpt-4o"), &sink, cancel)
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
async fn contract_timeout_is_normalized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(250))
                .set_body_string(common::sse_body(&[])),
        )
        .mount(&server)
        .await;

    let p = provider(&server, Some(Duration::from_millis(50)));
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(request("gpt-4o"), &sink, CancellationToken::new())
        .await
        .expect_err("连续无响应应超时");
    contract::assert_error_kind(&sink.events(), Some(&err), ProviderErrorKind::Timeout);
}

#[tokio::test]
async fn contract_long_stream_resets_read_timeout_after_each_chunk() {
    let read_timeout = Duration::from_millis(250);
    let (base_url, server) = spawn_slow_chunked_server(Duration::from_millis(80));
    let p = provider_at(base_url, Some(read_timeout));
    let sink = RecordingProviderSink::default();

    let summary = p
        .stream(request("gpt-4o"), &sink, CancellationToken::new())
        .await
        .expect("总时长超过 read timeout、但每个 chunk 都及时到达时应成功");
    server
        .join()
        .expect("slow stream server thread")
        .expect("slow stream server IO");

    assert_eq!(summary.stop_reason, StopReason::Completed);
    contract::assert_text_stream(&sink.events());
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

    let p = provider(&server, None);
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(
            request("gpt-4o"),
            &sink,
            pawork_domain::CancellationToken::new(),
        )
        .await
        .expect_err("429 应失败");
    contract::assert_error_kind(&sink.events(), Some(&err), ProviderErrorKind::RateLimited);
    assert!(err.retryable);
    assert_eq!(err.retry_after_ms, Some(5_000));
}

#[tokio::test]
async fn contract_context_overflow_is_normalized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(413).set_body_string("too large"))
        .mount(&server)
        .await;

    let p = provider(&server, None);
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(
            request("gpt-4o"),
            &sink,
            pawork_domain::CancellationToken::new(),
        )
        .await
        .expect_err("413 应失败");
    contract::assert_error_kind(
        &sink.events(),
        Some(&err),
        ProviderErrorKind::ContextTooLarge,
    );
    assert!(!err.retryable);
}

#[tokio::test]
async fn contract_malformed_stream_is_interrupted() {
    let server = MockServer::start().await;
    // 只有文本 delta，无 finish_reason 也无 [DONE] → StreamInterrupted
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"),
        )
        .mount(&server)
        .await;

    let p = provider(&server, None);
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(
            request("gpt-4o"),
            &sink,
            pawork_domain::CancellationToken::new(),
        )
        .await
        .expect_err("缺 finish/DONE 应 StreamInterrupted");
    contract::assert_error_kind(
        &sink.events(),
        Some(&err),
        ProviderErrorKind::StreamInterrupted,
    );
}

#[tokio::test]
async fn contract_reconnect_after_interrupted_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"cut\"}}]}\n\n"),
        )
        .mount(&server)
        .await;

    let p = provider(&server, None);
    let first_sink = RecordingProviderSink::default();
    let first_error = p
        .stream(request("gpt-4o"), &first_sink, CancellationToken::new())
        .await
        .expect_err("首次断流应失败");
    contract::assert_error_kind(
        &first_sink.events(),
        Some(&first_error),
        ProviderErrorKind::StreamInterrupted,
    );

    server.reset().await;
    mount_chat_ok(
        &server,
        common::sse_body(&[
            r#"{"choices":[{"delta":{"content":"reconnected"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]),
    )
    .await;
    let second_sink = RecordingProviderSink::default();
    let summary = p
        .stream(request("gpt-4o"), &second_sink, CancellationToken::new())
        .await
        .expect("断流后的下一次连接应成功");

    assert_eq!(summary.stop_reason, StopReason::Completed);
    contract::assert_text_stream(&second_sink.events());
}

#[tokio::test]
async fn contract_done_without_finish_reason_is_completed() {
    let server = MockServer::start().await;
    let body = common::sse_body(&[r#"{"choices":[{"delta":{"content":"done"}}]}"#]);
    mount_chat_ok(&server, body).await;

    let p = provider(&server, None);
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(
            request("gpt-4o"),
            &sink,
            pawork_domain::CancellationToken::new(),
        )
        .await
        .expect("[DONE] 应正常结束");

    assert_eq!(summary.stop_reason, StopReason::Completed);
}

#[tokio::test]
async fn contract_partial_json_tool_arguments() {
    let server = MockServer::start().await;
    // tool arguments 被切成不完整的 JSON 片段，最后用 finish 闭合
    let body = common::sse_body(&[
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read","arguments":"{\"path\":"}}]} }]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.txt\"}"}}]} }]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    ]);
    mount_chat_ok(&server, body).await;

    let p = provider(&server, None);
    let sink = RecordingProviderSink::default();
    p.stream(
        request("gpt-4o"),
        &sink,
        pawork_domain::CancellationToken::new(),
    )
    .await
    .expect("stream ok");
    // 事件流里应有 ArgumentsDelta 两段
    let arg_deltas: Vec<_> = sink
        .events()
        .iter()
        .filter_map(|e| match e {
            pawork_domain::ProviderStreamEvent::ToolCallArgumentsDelta { json, .. } => {
                Some(json.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        arg_deltas,
        vec!["{\"path\":".to_string(), "\"a.txt\"}".to_string()]
    );
    contract::assert_single_tool_call(&sink.events());
}

#[tokio::test]
async fn contract_list_models() {
    let server = MockServer::start().await;
    let p = provider(&server, None);
    for (payload, expected_count) in [
        (
            serde_json::json!({"data": [{"id": "gpt-4o"}, {"id": "llama-3"}]}),
            Some(2),
        ),
        (serde_json::json!({"data": []}), Some(0)),
        (serde_json::json!({}), None),
        (serde_json::json!({"data": null}), None),
        (serde_json::json!({"data": [], "has_more": true}), None),
        (serde_json::json!({"data": {}}), None),
        (serde_json::json!({"data": [{}]}), None),
        (serde_json::json!({"data": [{"id": ""}]}), None),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(payload))
            .expect(1)
            .mount(&server)
            .await;
        let result = p.list_models(None).await;
        if let Some(count) = expected_count {
            let models = result.expect("valid list");
            assert_eq!(models.len(), count);
            for model in models {
                assert_eq!(model.context_window_tokens, 0);
                assert_eq!(model.max_output_tokens, 0);
                assert!(!model.capabilities.tool_calls);
            }
        } else {
            assert_eq!(result.unwrap_err().kind, ProviderErrorKind::InvalidRequest);
        }
        server.verify().await;
    }
}

#[tokio::test]
async fn contract_no_authorization_when_credential_none() {
    let server = MockServer::start().await;
    let body = common::chat_minimal_ok_body();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(NoAuthorizationHeader)
        .and(header("x-trace-id", "trace-1"))
        .and(body_partial_json(serde_json::json!({
            "stream_options": { "include_usage": true }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    let mut config = OpenAiCompatibleConfig::new(server.uri()).with_provider_id("test");
    config.http = pawork_providers::net::http::HttpClientConfig::builder()
        .disable_system_proxy()
        .build();
    let p = OpenAiCompatibleProvider::new(config, None).expect("构造 adapter");
    let sink = RecordingProviderSink::default();
    p.stream(
        request("gpt-4o"),
        &sink,
        pawork_domain::CancellationToken::new(),
    )
    .await
    .expect("credential=None 应能完成流式请求");
    contract::assert_text_stream(&sink.events());
}
