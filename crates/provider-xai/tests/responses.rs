//! xAI Responses 适配 Mock smoke（P15-4）。
//!
//! 用 wiremock 锁定 Responses 路径的 item→event 归一、Live Search sources、
//! Web / X / Collection / Code / MCP 事件、reasoning Protected Blob 往返、双鉴权
//! 与降级行为。不触真实 API。

use std::collections::BTreeMap;

use agent_domain::{
    CancellationToken, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    ServerToolEvent, StopReason, TextContent, ToolCapabilityTag,
};
use provider_api::{
    CanonicalModelRequest, CredentialKind, HostedToolRequest, ModelProvider, PromptCachePreference,
    ProviderErrorKind, ProviderStreamEvent, ReasoningConfig, ReasoningEffort, RequestBudget,
    ResolvedCredential, ResponseFormat, ToolChoice,
};
use provider_xai::responses::{
    live_search_source_to_source, normalize_responses_error, requirements_from_request,
    to_responses_body, AcceptedResponsesTools, ResponsesAssemblyEvent, ResponsesStreamAssembler,
};
use provider_xai::{XaiConfig, XaiProvider};
use test_support::RecordingProviderSink;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn responses_request(model: &str) -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: agent_domain::RequestId::from("xai-resp-r1"),
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
        trace_id: Some("trace-xai-resp".into()),
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

fn sse_body(chunks: &[&str]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(chunk);
        body.push_str("\n\n");
    }
    body
}

fn canonical_only(events: Vec<ResponsesAssemblyEvent>) -> Vec<ProviderStreamEvent> {
    events
        .into_iter()
        .filter_map(|e| match e {
            ResponsesAssemblyEvent::Canonical(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn has_event(events: &[ProviderStreamEvent], pred: impl Fn(&ProviderStreamEvent) -> bool) -> bool {
    events.iter().any(pred)
}

#[tokio::test]
async fn responses_text_reasoning_function_and_completion_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer xai-key"))
        .and(body_partial_json(serde_json::json!({
            "model": "grok-4",
            "stream": true,
            "input": [{"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "hi"}
            ]}]
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"type":"response.created","response":{"id":"resp_xai_1"}}"#,
                    r#"{"type":"response.reasoning_summary_text.delta","delta":"thinking"}"#,
                    r#"{"type":"response.output_text.delta","delta":"Hello"}"#,
                    r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":""}}"#,
                    r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"p\":1}"}"#,
                    r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{\"p\":1}"}}"#,
                    r#"{"type":"response.completed","response":{"id":"resp_xai_1","status":"completed","usage":{"input_tokens":3,"output_tokens":2}}}"#,
                ])),
        )
        .mount(&server)
        .await;

    let adapter = provider(&server, CredentialKind::ApiKey, "xai-key");
    let sink = RecordingProviderSink::default();
    let summary = adapter
        .stream(responses_request("grok-4"), &sink, CancellationToken::new())
        .await
        .expect("xAI Responses stream succeeds");

    let events = sink.events();
    assert!(has_event(&events, |e| matches!(
        e,
        ProviderStreamEvent::ResponseStarted { response_id } if response_id.as_deref() == Some("resp_xai_1")
    )));
    assert!(has_event(&events, |e| matches!(
        e,
        ProviderStreamEvent::ThinkingDelta(t) if t == "thinking"
    )));
    assert!(has_event(&events, |e| matches!(
        e,
        ProviderStreamEvent::TextDelta(t) if t == "Hello"
    )));
    assert!(has_event(&events, |e| matches!(
        e,
        ProviderStreamEvent::ToolCallStarted { name, .. } if name == "read"
    )));
    assert!(has_event(&events, |e| matches!(
        e,
        ProviderStreamEvent::ToolCallCompleted { id } if id.as_str() == "call_1"
    )));
    assert!(has_event(&events, |e| matches!(
        e,
        ProviderStreamEvent::ResponseCompleted(StopReason::Completed)
    )));
    assert_eq!(summary.usage.input_tokens, 3);
    assert_eq!(summary.usage.output_tokens, 2);
    assert_eq!(summary.response_id.as_deref(), Some("resp_xai_1"));
}

#[tokio::test]
async fn responses_reasoning_protected_blob_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"step"}],"encrypted_content":"SECRET-OPAQUE-BYTES"}}"#,
                    r#"{"type":"response.output_text.delta","delta":"answer"}"#,
                    r#"{"type":"response.completed","response":{"id":"resp_xai_2","status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
                ])),
        )
        .mount(&server)
        .await;

    let adapter = provider(&server, CredentialKind::ApiKey, "xai-key");
    let sink = RecordingProviderSink::default();
    let summary = adapter
        .stream(responses_request("grok-4"), &sink, CancellationToken::new())
        .await
        .expect("reasoning stream succeeds");

    let events = sink.events();
    let reasoning = events
        .iter()
        .find_map(|e| match e {
            ProviderStreamEvent::ReasoningItem(item) => Some(item.clone()),
            _ => None,
        })
        .expect("reasoning item emitted");
    assert_eq!(reasoning.id.as_str(), "rs_1");
    assert_eq!(reasoning.summary.as_deref(), Some("step"));
    // 事件只携带 Protected Blob 引用，绝不携带明文凭证。
    let encoded = serde_json::to_string(&reasoning).expect("serialize reasoning item");
    assert!(!encoded.contains("SECRET-OPAQUE-BYTES"));
    assert!(reasoning.protected_blob_ref.as_str().starts_with("mem_"));

    // 第二轮：同一 provider 实例应复用默认 protector，把上轮引用解密回灌。
    let mut round2 = responses_request("grok-4");
    round2.messages[0]
        .content
        .push(ContentPart::Reasoning(reasoning));
    adapter
        .stream(
            round2,
            &RecordingProviderSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect("second reasoning stream succeeds");

    let requests = server.received_requests().await.expect("recorded requests");
    let responses_requests: Vec<_> = requests
        .iter()
        .filter(|request| request.url.path() == "/responses")
        .collect();
    assert_eq!(responses_requests.len(), 2);
    let second_body: serde_json::Value =
        serde_json::from_slice(&responses_requests[1].body).expect("second request body is JSON");
    assert_eq!(
        second_body["input"][0],
        serde_json::json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "summary_text", "text": "step"}],
            "encrypted_content": "SECRET-OPAQUE-BYTES"
        })
    );
    assert_eq!(summary.stop_reason, StopReason::Completed);
}

#[tokio::test]
async fn responses_web_search_live_sources_normalize_to_source() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"pawork","sources":[{"type":"url","url":"https://pawork.dev","title":"Pawork","snippet":"an agent workspace"}]}}}"#,
                    r#"{"type":"response.completed","response":{"id":"resp_xai_3","status":"completed","usage":{"input_tokens":2,"output_tokens":1}}}"#,
                ])),
        )
        .mount(&server)
        .await;

    let sink = RecordingProviderSink::default();
    provider(&server, CredentialKind::ApiKey, "xai-key")
        .stream(responses_request("grok-4"), &sink, CancellationToken::new())
        .await
        .expect("web search stream succeeds");

    let events = sink.events();
    assert!(has_event(&events, |e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { tool_call_id, .. })
            if tool_call_id.as_str() == "ws_1"
    )));
    assert!(has_event(&events, |e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::SourceAdded { tool_call_id, source })
            if tool_call_id.as_str() == "ws_1"
                && source.url.as_deref() == Some("https://pawork.dev")
                && source.title.as_deref() == Some("Pawork")
    )));
}

#[tokio::test]
async fn responses_x_collection_code_mcp_events_normalize() {
    // 纯函数层覆盖 X / Collection / Code / MCP 的 item→event 归一（不触网）。
    let mut assembler = ResponsesStreamAssembler::new();

    let x_events = canonical_only(assembler.feed(
        r#"{"type":"response.output_item.done","item":{"type":"x_search_call","id":"xs_1","status":"completed","sources":[{"type":"x","url":"https://x.com/p/status/1","text":"a post"}]}}"#,
    ));
    assert!(has_event(&x_events, |e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::SourceAdded { tool_call_id, source })
            if tool_call_id.as_str() == "xs_1"
                && source.url.as_deref() == Some("https://x.com/p/status/1")
    )));

    let col_events = canonical_only(assembler.feed(
        r#"{"type":"response.output_item.done","item":{"type":"file_search_call","id":"fs_1","status":"completed","results":[{"type":"document","title":"Doc A","text":"snippet","document_index":2}]}}"#,
    ));
    assert!(has_event(&col_events, |e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::SourceAdded { tool_call_id, source })
            if tool_call_id.as_str() == "fs_1" && source.document_index == Some(2)
    )));
    // hosted tool 续接：file_search 的结果不应映射为客户端 function_call_output。
    let serialized = serde_json::to_string(&col_events).expect("serialize");
    assert!(
        !serialized.contains("function_call_output"),
        "server tool continuation must never emit function_call_output: {serialized}"
    );

    let code_events = canonical_only(assembler.feed(
        r#"{"type":"response.output_item.done","item":{"type":"code_interpreter_call","id":"ci_1","status":"completed","code":"print(1)","outputs":[{"logs":"1\n"}]}}"#,
    ));
    assert!(has_event(&code_events, |e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramStarted { tool_call_id, .. })
            if tool_call_id.as_str() == "ci_1"
    )));
    assert!(has_event(&code_events, |e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramOutput { delta: Some(_), .. })
    )));

    let mcp_events = canonical_only(assembler.feed(
        r#"{"type":"response.output_item.done","item":{"type":"mcp_call","id":"mcp_1","name":"search","status":"completed","output":"ok"}}"#,
    ));
    assert!(has_event(&mcp_events, |e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::Started { name, .. }) if name == "mcp:search"
    )));
    assert!(has_event(&mcp_events, |e| matches!(
        e,
        ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { summary, .. })
            if summary.as_deref() == Some("ok")
    )));
}

#[tokio::test]
async fn responses_oauth_bearer_credential_is_sent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer oauth-access"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"type":"response.output_text.delta","delta":"ok"}"#,
                    r#"{"type":"response.completed","response":{"id":"resp_oauth","status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
                ])),
        )
        .mount(&server)
        .await;

    let adapter = provider(&server, CredentialKind::OAuthBearer, "oauth-access");
    let sink = RecordingProviderSink::default();
    adapter
        .stream(responses_request("grok-4"), &sink, CancellationToken::new())
        .await
        .expect("OAuth bearer Responses stream succeeds");
    assert!(has_event(&sink.events(), |e| matches!(
        e,
        ProviderStreamEvent::TextDelta(t) if t == "ok"
    )));
}

#[tokio::test]
async fn responses_degrades_to_chat_completions_for_baseline_model() {
    // grok-2 声明 ChatCompletions transport → 不打 /responses，打 /chat/completions。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string({
                    let mut body = String::new();
                    body.push_str("data: {\"choices\":[{\"delta\":{\"content\":\"legacy\"}}]}\n\n");
                    body.push_str(
                        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    );
                    body.push_str("data: [DONE]\n\n");
                    body
                }),
        )
        .mount(&server)
        .await;

    let sink = RecordingProviderSink::default();
    provider(&server, CredentialKind::ApiKey, "xai-key")
        .stream(responses_request("grok-2"), &sink, CancellationToken::new())
        .await
        .expect("degraded Chat Completions stream succeeds");
    assert!(has_event(&sink.events(), |e| matches!(
        e,
        ProviderStreamEvent::TextDelta(t) if t == "legacy"
    )));
    // 确认没有请求过 /responses（降级路径）。
    let requests = server.received_requests().await.expect("recorded");
    assert!(requests.iter().all(|r| r.url.path() != "/responses"));
}

#[tokio::test]
async fn responses_error_normalization_maps_xai_codes() {
    // Live Search quota → RateLimited + retryable。
    let quota = provider_api::ProviderError::new(
        ProviderErrorKind::InvalidRequest,
        "live_search quota exceeded",
    );
    let normalized = normalize_responses_error(quota);
    assert_eq!(normalized.kind, ProviderErrorKind::RateLimited);
    assert!(normalized.retryable);

    // MCP unauthorized → Authorization + non-retryable。
    let mcp = provider_api::ProviderError::new(
        ProviderErrorKind::InvalidRequest,
        "mcp connector unauthorized",
    );
    let normalized = normalize_responses_error(mcp);
    assert_eq!(normalized.kind, ProviderErrorKind::Authorization);
    assert!(!normalized.retryable);
}

#[tokio::test]
async fn responses_http_error_is_normalized_and_emitted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
        .mount(&server)
        .await;

    let sink = RecordingProviderSink::default();
    let error = provider(&server, CredentialKind::ApiKey, "xai-key")
        .stream(responses_request("grok-4"), &sink, CancellationToken::new())
        .await
        .expect_err("429 must be normalized");
    assert_eq!(error.kind, ProviderErrorKind::RateLimited);
    assert!(error.retryable);
}

#[test]
fn responses_request_body_and_requirements_shape() {
    let mut request = responses_request("grok-4");
    request.hosted_tools.push(HostedToolRequest {
        name: "web_search".into(),
        kind: ToolCapabilityTag::WebSearch,
        description: String::new(),
        capabilities: vec![ToolCapabilityTag::WebSearch],
        config: None,
    });
    request.reasoning = Some(ReasoningConfig::new(ReasoningEffort::XHigh));
    request.provider_options.insert(
        "previous_response_id".into(),
        serde_json::json!("resp_prev"),
    );

    let body = to_responses_body(
        &request,
        Vec::new(),
        &AcceptedResponsesTools {
            web_search: true,
            ..AcceptedResponsesTools::default()
        },
    );
    assert_eq!(body["model"], "grok-4");
    assert_eq!(body["stream"], true);
    assert_eq!(body["tools"][0]["type"], "web_search");
    assert_eq!(body["include"][0], "web_search.sources");
    // XHigh 被 clamp 为 high（Responses 暂无 xhigh/max）。
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["previous_response_id"], "resp_prev");

    let requirements = requirements_from_request(&request);
    assert!(requirements
        .required_tools
        .contains(&ToolCapabilityTag::WebSearch));
    assert!(requirements.citations);
    assert_eq!(
        requirements.transport_pref,
        vec![provider_api::ModelTransport::Responses]
    );
}

#[test]
fn live_search_source_normalization_covers_url_x_and_document() {
    let web = live_search_source_to_source(&serde_json::json!({
        "type": "url",
        "url": "https://pawork.dev",
        "title": "Pawork"
    }))
    .expect("web source");
    assert_eq!(web.url.as_deref(), Some("https://pawork.dev"));
    assert_eq!(web.title.as_deref(), Some("Pawork"));

    let x = live_search_source_to_source(&serde_json::json!({
        "type": "x",
        "url": "https://x.com/p/1",
        "text": "post body"
    }))
    .expect("x source");
    assert_eq!(x.url.as_deref(), Some("https://x.com/p/1"));
    assert_eq!(x.text.as_deref(), Some("post body"));

    let doc = live_search_source_to_source(&serde_json::json!({
        "type": "document",
        "title": "Doc",
        "text": "snippet",
        "document_index": 5
    }))
    .expect("document source");
    assert_eq!(doc.document_index, Some(5));
    assert_eq!(doc.title.as_deref(), Some("Doc"));

    // 纯字符串 URL（xAI top-level citations 紧凑形式）。
    let url = live_search_source_to_source(&serde_json::json!("https://example.com/x"))
        .expect("url string");
    assert_eq!(url.url.as_deref(), Some("https://example.com/x"));
}

#[tokio::test]
async fn responses_hosted_tools_only_sent_when_accepted() {
    let server = MockServer::start().await;
    // 期望 body 里包含 web_search 工具声明（协商通过）。
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(body_partial_json(serde_json::json!({
            "tools": [{"type": "web_search"}]
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    r#"{"type":"response.completed","response":{"id":"resp_tools","status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#,
                ])),
        )
        .mount(&server)
        .await;

    let mut request = responses_request("grok-4");
    request.hosted_tools.push(HostedToolRequest {
        name: "web_search".into(),
        kind: ToolCapabilityTag::WebSearch,
        description: String::new(),
        capabilities: vec![ToolCapabilityTag::WebSearch],
        config: None,
    });

    let sink = RecordingProviderSink::default();
    provider(&server, CredentialKind::ApiKey, "xai-key")
        .stream(request, &sink, CancellationToken::new())
        .await
        .expect("hosted tool stream succeeds");
}

#[tokio::test]
async fn responses_no_provider_branch_assertion() {
    // 协商器只消费证据 + 要求；即使 provider id 为 xai，协商结果只由模型能力
    // 声明决定，证明不在 Core 走 xAI 名称分支。
    use model_registry::CapabilityEvidence;
    use provider_api::{CapabilityRequirements, ModelCapabilities, ModelTransport};
    use provider_runtime::negotiate::CapabilityNegotiator;

    let evidence = CapabilityEvidence {
        model: ModelId::new("grok-4"),
        provider: Some(agent_domain::ProviderId::new("xai")),
        static_declared: Some(ModelCapabilities {
            text: true,
            thinking: true,
            transport: ModelTransport::Responses,
            ..ModelCapabilities::default()
        }),
        probe_declared: None,
        override_declared: None,
    };
    let requirements = CapabilityRequirements {
        transport_pref: vec![ModelTransport::Responses],
        ..CapabilityRequirements::default()
    };
    let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
    assert_eq!(resolved.chosen_transport, ModelTransport::Responses);
    // 证据层 provider 字段不影响选择（纯函数不读名称）。
    let evidence_no_provider = CapabilityEvidence {
        provider: None,
        ..evidence
    };
    let resolved2 = CapabilityNegotiator::negotiate(&evidence_no_provider, &requirements);
    assert_eq!(resolved2.chosen_transport, resolved.chosen_transport);
}
