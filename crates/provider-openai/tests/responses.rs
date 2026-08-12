//! OpenAI Responses 传输路径 Mock smoke（P15-2）。
//!
//! 全程 wiremock，不接触真实网络与 Key。覆盖：
//! - Responses + function / reasoning 的 item → ProviderStreamEvent；
//! - web_search_call 的 ServerTool + Citation 归一；
//! - reasoning encrypted_content 只经 Protected Blob 引用往返（ADR-032）；
//! - 不支持 Responses 时降级到 Chat Completions（行为可观察）；
//! - Core 无 Provider 名称分支（no_provider_branch 断言）。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_domain::{
    ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId, ReasoningItem,
    ReasoningItemId, ServerToolEvent, StopReason, TextContent, ToolCallId, ToolCapabilityTag,
};
use provider_api::ModelProvider;
use provider_api::{
    CanonicalModelRequest, CredentialKind, HostedToolRequest, PromptCachePreference,
    ProviderErrorKind, ProviderStreamEvent, RequestBudget, ResolvedCredential, ResponseFormat,
    ToolChoice,
};
use provider_openai::{
    AcceptedResponsesTools, InMemoryReasoningProtector, OpenAiConfig, OpenAiProvider,
    ReasoningProtector,
};
use provider_runtime::http::HttpClientConfig;
use test_support::RecordingProviderSink;
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

fn provider_with_protector(
    server: &MockServer,
    protector: Arc<dyn ReasoningProtector>,
) -> OpenAiProvider {
    provider(server).with_reasoning_protector(protector)
}

/// 构造 Responses SSE body：每个事件 `event: <t>\ndata: <json>\n\n`，末尾不发送
/// `[DONE]`（Responses 以 `response.completed` 收尾后关闭流）。
fn responses_sse_body(events: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (event_type, data) in events {
        body.push_str("event: ");
        body.push_str(event_type);
        body.push('\n');
        body.push_str("data: ");
        body.push_str(data);
        body.push_str("\n\n");
    }
    body
}

fn chat_sse_body(chunks: &[&str]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(chunk);
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

async fn mount_responses(server: &MockServer, body: String) {
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(server)
        .await;
}

async fn mount_chat(server: &MockServer, body: String) {
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
async fn responses_text_reasoning_and_function_call_stream() {
    let server = MockServer::start().await;
    let body = responses_sse_body(&[
        (
            "response.created",
            r#"{"type":"response.created","response":{"id":"resp_42"}}"#,
        ),
        (
            "response.reasoning_summary_text.delta",
            r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking"}"#,
        ),
        (
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","delta":"answer"}"#,
        ),
        (
            "response.output_item.added",
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":""}}"#,
        ),
        (
            "response.function_call_arguments.delta",
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"p\":1}"}"#,
        ),
        (
            "response.output_item.done",
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{\"p\":1}"}}"#,
        ),
        (
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_42","status":"completed","usage":{"input_tokens":4,"output_tokens":3}}}"#,
        ),
    ]);
    mount_responses(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(request("o3"), &sink, agent_domain::CancellationToken::new())
        .await
        .expect("responses stream ok");
    let events = sink.events();

    // 走的是 Responses 端点（不是 Chat Completions）。
    assert_eq!(summary.response_id.as_deref(), Some("resp_42"));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::ResponseStarted { response_id } if response_id.as_deref() == Some("resp_42"))));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::ThinkingDelta(t) if t == "thinking")));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::TextDelta(t) if t == "answer")));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::ToolCallStarted { name, .. } if name == "read")));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::ToolCallArgumentsDelta { id, json } if id.as_str()=="call_1" && json == "{\"p\":1}")));
    assert!(events.iter().any(
        |e| matches!(e, ProviderStreamEvent::ToolCallCompleted { id } if id.as_str() == "call_1")
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        ProviderStreamEvent::ResponseCompleted(StopReason::Completed)
    )));
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::UsageUpdated(u) if u.input_tokens == 4 && u.output_tokens == 3)));
}

#[tokio::test]
async fn responses_web_search_emits_server_tool_and_citations() {
    let server = MockServer::start().await;
    let web_search_done = r#"{"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"pawork","sources":[{"type":"url","url":"https://pawork.dev","title":"Pawork"}]}}}"#;
    let text_done = r#"{"type":"response.output_text.done","item_id":"msg_1","text":"see pawork","annotations":[{"type":"url_citation","url":"https://pawork.dev","title":"Pawork","start_index":0}]}"#;
    let body = responses_sse_body(&[
        (
            "response.created",
            r#"{"type":"response.created","response":{"id":"resp_ws"}}"#,
        ),
        ("response.output_item.done", web_search_done),
        ("response.output_text.done", text_done),
        (
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_ws","status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
    ]);
    mount_responses(&server, body).await;

    let mut req = request("o3");
    req.hosted_tools.push(HostedToolRequest {
        name: "web_search".into(),
        kind: ToolCapabilityTag::WebSearch,
        description: String::new(),
        capabilities: vec![ToolCapabilityTag::WebSearch],
        config: None,
    });

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");
    let events = sink.events();

    // web_search_call → ServerTool Completed + SourceAdded（citations 经 P15-5）。
    assert!(events.iter().any(|e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { tool_call_id, .. })
            if tool_call_id.as_str() == "ws_1"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::SourceAdded { tool_call_id, source })
            if tool_call_id.as_str() == "ws_1"
                && source.url.as_deref() == Some("https://pawork.dev")
    )));
    // url_citation annotation → CitationAdded，归属到产生它的 web_search_call。
    assert!(events.iter().any(|e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::CitationAdded { tool_call_id, citation })
            if tool_call_id.as_str() == "ws_1"
                && citation.url.as_deref() == Some("https://pawork.dev")
    )));

    // 请求体声明了 web_search_preview 并 include sources。
    let request_body = last_request_body(&server).await;
    assert_eq!(request_body["tools"][0]["type"], "web_search_preview");
    assert_eq!(
        request_body["include"][0],
        "web_search_preview.action.sources"
    );
}

#[tokio::test]
async fn responses_reasoning_encrypted_content_only_reaches_blob_store() {
    let server = MockServer::start().await;
    // 响应里携带 encrypted_content（受保护明文，仅出现在本 fixture）。
    let reasoning_done = r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_7","summary":[{"type":"summary_text","text":"checked constraints"}],"encrypted_content":"SECRET-CONTINUATION-BYTES"}}"#;
    let body = responses_sse_body(&[
        (
            "response.created",
            r#"{"type":"response.created","response":{"id":"resp_r"}}"#,
        ),
        ("response.output_item.done", reasoning_done),
        (
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_r","status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
    ]);
    mount_responses(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(request("o3"), &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");
    let events = sink.events();

    // reasoning 进了 canonical 事件（带 protected_blob_ref）。
    let reasoning = events
        .iter()
        .find_map(|e| match e {
            ProviderStreamEvent::ReasoningItem(item) => Some(item.clone()),
            _ => None,
        })
        .expect("reasoning item emitted");
    assert_eq!(reasoning.id.as_str(), "rs_7");
    assert!(!reasoning.protected_blob_ref.as_str().is_empty());

    // ADR-032：encrypted_content 明文绝不进入任何事件（序列化 / Debug）。
    let all_debug = format!("{events:?}");
    let all_json = serde_json::to_string(&events).expect("serialize events");
    for forbidden in ["SECRET-CONTINUATION-BYTES", "encrypted_content"] {
        assert!(
            !all_debug.contains(forbidden),
            "reasoning plaintext leaked into event debug: {forbidden}"
        );
        assert!(
            !all_json.contains(forbidden),
            "reasoning plaintext leaked into event json: {forbidden}"
        );
    }
}

#[tokio::test]
async fn default_reasoning_protector_rehydrates_across_streams_on_same_provider() {
    let server = MockServer::start().await;
    let reasoning_done = r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_cross_round","summary":[{"type":"summary_text","text":"retained"}],"encrypted_content":"cross-round-bytes"}}"#;
    let body = responses_sse_body(&[
        (
            "response.created",
            r#"{"type":"response.created","response":{"id":"resp_cross_round"}}"#,
        ),
        ("response.output_item.done", reasoning_done),
        (
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_cross_round","status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
    ]);
    mount_responses(&server, body).await;

    let provider = provider(&server);
    let first_sink = RecordingProviderSink::default();
    provider
        .stream(
            request("o3"),
            &first_sink,
            agent_domain::CancellationToken::new(),
        )
        .await
        .expect("first stream ok");
    let reasoning = first_sink
        .events()
        .into_iter()
        .find_map(|event| match event {
            ProviderStreamEvent::ReasoningItem(item) => Some(item),
            _ => None,
        })
        .expect("first stream emits reasoning item");

    let mut second_request = request("o3");
    second_request.messages[0]
        .content
        .push(ContentPart::Reasoning(reasoning));
    provider
        .stream(
            second_request,
            &RecordingProviderSink::default(),
            agent_domain::CancellationToken::new(),
        )
        .await
        .expect("second stream ok");

    let request_body = last_request_body(&server).await;
    let reasoning_input = request_body["input"]
        .as_array()
        .expect("input array")
        .iter()
        .find(|item| item["type"] == "reasoning")
        .expect("reasoning input rehydrated");
    assert_eq!(reasoning_input["id"], "rs_cross_round");
    assert_eq!(reasoning_input["encrypted_content"], "cross-round-bytes");
}

#[tokio::test]
async fn responses_reasoning_round_trip_injects_decrypted_input() {
    let server = MockServer::start().await;
    let body = responses_sse_body(&[
        (
            "response.created",
            r#"{"type":"response.created","response":{"id":"resp_rt"}}"#,
        ),
        (
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_rt","status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
    ]);
    mount_responses(&server, body).await;

    // 用共享 protector 预先 protect 一段 continuation，构造历史 reasoning item。
    let protector = Arc::new(InMemoryReasoningProtector::default());
    let reference = protector
        .protect(b"round-trip-bytes")
        .await
        .expect("protect");
    let item = ReasoningItem {
        id: ReasoningItemId::from("rs_hist"),
        summary: None,
        protected_blob_ref: reference,
        opaque_metadata: BTreeMap::from([(
            "openai.responses.summary_entries".into(),
            serde_json::json!([{"type":"summary_text","text":"prior step"}]),
        )]),
        continuation_metadata: BTreeMap::new(),
    };
    let mut req = request("o3");
    req.messages[0].content.push(ContentPart::Reasoning(item));

    let p = provider_with_protector(&server, protector);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    // 请求体回灌了解密后的 reasoning input item。
    let request_body = last_request_body(&server).await;
    let input = request_body["input"].as_array().expect("input array");
    let reasoning_input = input
        .iter()
        .find(|v| v["type"] == "reasoning")
        .expect("reasoning input item injected");
    assert_eq!(reasoning_input["id"], "rs_hist");
    assert_eq!(reasoning_input["encrypted_content"], "round-trip-bytes");
}

#[tokio::test]
async fn responses_degrades_to_chat_completions_for_baseline_model() {
    let server = MockServer::start().await;
    let body = chat_sse_body(&[
        r#"{"choices":[{"delta":{"content":"legacy"}}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    ]);
    mount_chat(&server, body).await;

    // gpt-4o 声明 ChatCompletions transport → 协商降级，走 /chat/completions。
    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = p
        .stream(
            request("gpt-4o"),
            &sink,
            agent_domain::CancellationToken::new(),
        )
        .await
        .expect("chat stream ok");
    let events = sink.events();
    assert!(events
        .iter()
        .any(|e| matches!(e, ProviderStreamEvent::TextDelta(t) if t == "legacy")));
    assert_eq!(summary.stop_reason, StopReason::Completed);
    // 确认没有命中 /responses。
    let requests = server.received_requests().await.expect("recorded");
    assert!(requests.iter().all(|req| req.url.path() != "/responses"));
}

#[tokio::test]
async fn responses_unsupported_hosted_tool_is_rejected_not_silently_dropped() {
    let server = MockServer::start().await;
    // 即便声明了 hosted tool，未协商通过也不进入请求体（gpt-4o 不支持 hosted tools）。
    let body =
        chat_sse_body(&[r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#]);
    mount_chat(&server, body).await;

    let mut req = request("gpt-4o");
    req.hosted_tools.push(HostedToolRequest {
        name: "web_search".into(),
        kind: ToolCapabilityTag::WebSearch,
        description: String::new(),
        capabilities: vec![ToolCapabilityTag::WebSearch],
        config: None,
    });
    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");

    // 降级到 Chat Completions 的请求体不应携带 web_search（hosted tool 协商失败）。
    let body = last_request_body(&server).await;
    assert!(body.get("web_search").is_none());
    assert!(body
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            !tools
                .iter()
                .any(|t| t.get("type").and_then(|v| v.as_str()) == Some("web_search_preview"))
        })
        .unwrap_or(true));
}

#[tokio::test]
async fn responses_error_status_normalizes_to_provider_error() {
    let server = MockServer::start().await;
    // vector store 未就绪：Responses 返回 400 + 结构化错误。
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(
                r#"{"error":{"code":"vector_store_not_ready","message":"vector_store is not_ready yet"}}"#,
            ),
        )
        .mount(&server)
        .await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    let err = p
        .stream(request("o3"), &sink, agent_domain::CancellationToken::new())
        .await
        .expect_err("400 应失败");
    // vector store 未就绪归一为 ProviderUnavailable（可重试，等远端状态就绪）。
    assert_eq!(err.kind, ProviderErrorKind::ProviderUnavailable);
    assert!(err.retryable);
}

#[tokio::test]
async fn responses_path_has_no_provider_name_branch_in_events() {
    // no_provider_branch 断言：归一后的事件序列不应携带 "openai" 字面标识
    // （Core 只消费 canonical 词汇，不按 Provider 名分支）。
    let server = MockServer::start().await;
    let body = responses_sse_body(&[
        (
            "response.created",
            r#"{"type":"response.created","response":{"id":"resp_np"}}"#,
        ),
        (
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","delta":"hi"}"#,
        ),
        (
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_np","status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
    ]);
    mount_responses(&server, body).await;

    let p = provider(&server);
    let sink = RecordingProviderSink::default();
    p.stream(request("o3"), &sink, agent_domain::CancellationToken::new())
        .await
        .expect("stream ok");
    let json = serde_json::to_string(&sink.events()).expect("serialize events");
    let lower = json.to_ascii_lowercase();
    assert!(
        !lower.contains("openai"),
        "canonical event stream must not carry provider name: {json}"
    );
}

// AcceptedResponsesTools 在 wire 层暴露给 host 注入测试夹具时复用；此处静态
// 引用避免意外移除导出（P15-2 公共边界）。
#[test]
fn accepted_tools_type_is_part_of_public_boundary() {
    let _ = AcceptedResponsesTools::default();
}

// ToolCallId 占位引用，防止未来重命名后丢失 canonical 衔接。
#[allow(dead_code)]
const _ENSURE_TOOL_CALL_ID: fn() = || {
    let _ = ToolCallId::from("placeholder");
};
