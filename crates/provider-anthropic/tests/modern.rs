//! P15-3 现代 Messages Mock smoke（recorded fixture JSON，不触真实 API）。
//!
//! 覆盖：structured output + effort + adaptive thinking 原生映射、server tool
//! 结果归一（web_search / code_execution）、thinking signature 不透明往返、
//! interleaved 顺序、降级到 P6-2 基线（不报错）、ProviderTranscript 续接与
//! citation 归一。全部经 wiremock + 本地 fixture，不触真实网络与 Key。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_domain::ToolCapabilityTag;
use agent_domain::{
    ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId, ProtectedBlobRef,
    StopReason, TextContent, ThinkingContent,
};
use provider_anthropic::{
    modern::ContinuationStoreError, AnthropicConfig, AnthropicProvider, ReasoningContinuationStore,
};
use provider_api::{
    CanonicalModelRequest, CredentialKind, HostedToolRequest, ModelProvider, PromptCachePreference,
    ProviderStreamEvent, ReasoningConfig, ReasoningEffort, RequestBudget, ResolvedCredential,
    ResponseFormat, ToolChoice, ToolDefinition,
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
        reasoning: None,
        temperature: Some(0.0),
        max_output_tokens: Some(256),
        stop_sequences: Vec::new(),
        response_format: ResponseFormat::Text,
        prompt_cache: PromptCachePreference::Automatic,
        budget: RequestBudget::default(),
        provider_options: BTreeMap::new(),
        trace_id: Some("trace-modern".into()),
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

/// 内存不透明续传 store（Mock 替代 ReasoningStateBridge；加密属 P15-7 已测域）。
#[derive(Clone, Default)]
struct MemoryStore(Arc<StoreState>);

#[derive(Default)]
struct StoreState {
    inner: std::sync::Mutex<(BTreeMap<String, Vec<u8>>, usize)>,
}

impl MemoryStore {
    fn new() -> Self {
        Self(Arc::new(StoreState {
            inner: std::sync::Mutex::new((BTreeMap::new(), 0)),
        }))
    }

    fn handle(&self) -> ReasoningContinuationStore {
        let protect_store = self.clone();
        let protect = move |payload: Vec<u8>| -> futures::future::BoxFuture<
            'static,
            Result<ProtectedBlobRef, ContinuationStoreError>,
        > {
            let store = protect_store.clone();
            Box::pin(async move {
                let mut guard = store.0.inner.lock().expect("store lock");
                guard.1 += 1;
                let key = format!("blob-{}", guard.1);
                guard.0.insert(key.clone(), payload);
                Ok(ProtectedBlobRef::from(key))
            })
        };
        let resolve_store = self.clone();
        let resolve = move |blob_ref: ProtectedBlobRef| -> futures::future::BoxFuture<
            'static,
            Result<Vec<u8>, ContinuationStoreError>,
        > {
            let store = resolve_store.clone();
            Box::pin(async move {
                let guard = store.0.inner.lock().expect("store lock");
                guard
                    .0
                    .get(blob_ref.as_str())
                    .cloned()
                    .ok_or_else(|| ContinuationStoreError::new("blob missing"))
            })
        };
        ReasoningContinuationStore::new(protect, resolve)
    }

    fn blobs(&self) -> BTreeMap<String, Vec<u8>> {
        self.0.inner.lock().expect("store lock").0.clone()
    }
}

fn sse(events: &[&str]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("event: message\n");
        body.push_str("data: ");
        body.push_str(event);
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
    let requests = server.received_requests().await.expect("received requests");
    let last = requests.last().expect("at least one request");
    serde_json::from_slice(&last.body).expect("request body json")
}

fn text_fixture(text: &str) -> String {
    sse(&[
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
        &format!(
            r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{text}"}}}}"#
        ),
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        r#"{"type":"message_stop"}"#,
    ])
}

#[tokio::test]
async fn structured_output_effort_and_adaptive_thinking_map_natively() {
    let server = MockServer::start().await;
    mount_ok(&server, text_fixture("ok")).await;
    let mut req = request("claude-sonnet-4-5");
    req.response_format = ResponseFormat::JsonSchema {
        name: "answer".into(),
        schema: serde_json::json!({"type":"object","required":["ok"]}),
    };
    req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::High));

    let store = MemoryStore::new();
    let provider = provider(&server).with_reasoning_continuation(store.handle());
    let sink = RecordingProviderSink::default();
    let summary = provider
        .stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("modern stream succeeds");
    assert_eq!(summary.stop_reason, StopReason::Completed);

    let body = last_request_body(&server).await;
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "high");
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(body["output_config"]["format"]["name"], "answer");
    // 原生 schema，不再注入 system JSON 指令。
    assert!(body.get("system").is_none());
    // 无降级 note。
    assert!(!sink.events().iter().any(|event| {
        matches!(event, ProviderStreamEvent::ProviderMetadata(metadata)
            if metadata.get("degradation").is_some())
    }));
}

#[tokio::test]
async fn server_tools_normalize_results_citations_and_transcript() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[
            r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{"query":"pawork"}}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","title":"Pawork","url":"https://pawork.dev"},{"type":"web_search_result","title":"Docs","url":"https://docs.pawork.dev"}]}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"server_tool_use","id":"srvtoolu_2","name":"code_execution","input":{"code":"print(1)"}}}"#,
            r#"{"type":"content_block_stop","index":2}"#,
            r#"{"type":"content_block_start","index":3,"content_block":{"type":"code_execution_tool_result","tool_use_id":"srvtoolu_2","content":[{"type":"text","text":"1"}]}}"#,
            r#"{"type":"content_block_stop","index":3}"#,
            r#"{"type":"content_block_start","index":4,"content_block":{"type":"text","citations":[{"type":"web_search_result_location","url":"https://pawork.dev","title":"Pawork"}]}}"#,
            r#"{"type":"content_block_delta","index":4,"delta":{"type":"text_delta","text":"sources"}}"#,
            r#"{"type":"content_block_stop","index":4}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#,
            r#"{"type":"message_stop"}"#,
        ]),
    )
    .await;

    let mut req = request("claude-sonnet-4-5");
    req.hosted_tools.push(HostedToolRequest {
        name: "web_search".into(),
        kind: ToolCapabilityTag::WebSearch,
        description: String::new(),
        capabilities: Vec::new(),
        config: None,
    });
    req.hosted_tools.push(HostedToolRequest {
        name: "code_execution".into(),
        kind: ToolCapabilityTag::CodeExecution,
        description: String::new(),
        capabilities: Vec::new(),
        config: None,
    });

    let provider = provider(&server);
    let sink = RecordingProviderSink::default();
    provider
        .stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("server tool stream succeeds");

    use agent_domain::ServerToolEvent;
    let kinds: Vec<String> = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::ServerTool(server_event) => Some(match server_event {
                ServerToolEvent::Started { name, .. } => format!("started:{name}"),
                ServerToolEvent::SourceAdded { .. } => "source_added".into(),
                ServerToolEvent::Completed { .. } => "completed".into(),
                ServerToolEvent::ProgramStarted { .. } => "program_started".into(),
                ServerToolEvent::ProgramOutput { .. } => "program_output".into(),
                ServerToolEvent::CitationAdded { .. } => "citation_added".into(),
                _ => "other".into(),
            }),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "started:web_search".to_string(),
            "source_added".to_string(),
            "source_added".to_string(),
            "completed".to_string(),
            "started:code_execution".to_string(),
            "program_started".to_string(),
            "program_output".to_string(),
            "completed".to_string(),
            "citation_added".to_string(),
        ]
    );

    // ProviderTranscript 续接信封（provider-neutral）。
    let envelope = sink
        .events()
        .iter()
        .find_map(|event| match event {
            ProviderStreamEvent::TranscriptEnvelope(envelope) => Some(envelope.clone()),
            _ => None,
        })
        .expect("transcript envelope emitted");
    let encoded = serde_json::to_string(&envelope).expect("serialize envelope");
    for forbidden in [
        "anthropic",
        "web_search_20250305",
        "code_execution_20250522",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "transcript envelope must not carry provider wire names: {encoded}"
        );
    }
}

#[tokio::test]
async fn thinking_signature_round_trips_through_opaque_store() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[
            r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"let me think"}}"#,
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"let me think","signature":"SIG-FIXTURE-1"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
            r#"{"type":"message_stop"}"#,
        ]),
    )
    .await;

    let mut req = request("claude-sonnet-4-5");
    req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::Medium));
    let store = MemoryStore::new();
    let provider = provider(&server).with_reasoning_continuation(store.handle());
    let sink = RecordingProviderSink::default();
    provider
        .stream(req.clone(), &sink, agent_domain::CancellationToken::new())
        .await
        .expect("thinking stream succeeds");

    // 事件只带安全引用，signature 不进事件流。
    let items: Vec<_> = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::ReasoningItem(item) => Some(item.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(items.len(), 1);
    let item = items[0].clone();
    let encoded_event = serde_json::to_string(&sink.events()).expect("serialize events");
    assert!(!encoded_event.contains("SIG-FIXTURE-1"));
    // 原文只在不透明 store 中（此处为内存 Mock；真实路径为加密 blob store）。
    let blobs = store.blobs();
    assert!(blobs
        .values()
        .any(|bytes| { String::from_utf8_lossy(bytes).contains("SIG-FIXTURE-1") }));

    // 第二轮：会话回灌 → 重建带 signature 的 thinking 块。
    req.messages.push(Message {
        id: MessageId::new("a1"),
        role: MessageRole::Assistant,
        content: vec![
            ContentPart::Thinking(ThinkingContent {
                text: "let me think".into(),
                reasoning_item_id: Some(item.id.clone()),
                redacted: false,
            }),
            ContentPart::Reasoning(item.clone()),
            ContentPart::Text(TextContent {
                text: "answer".into(),
            }),
        ],
        metadata: MessageMetadata::default(),
    });
    provider
        .stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("second turn succeeds");
    let body = last_request_body(&server).await;
    let assistant = &body["messages"][1];
    assert_eq!(assistant["content"][0]["type"], "thinking");
    assert_eq!(assistant["content"][0]["thinking"], "let me think");
    assert_eq!(assistant["content"][0]["signature"], "SIG-FIXTURE-1");
    assert_eq!(assistant["content"][1]["type"], "text");
}

#[tokio::test]
async fn interleaved_thinking_and_tools_keep_wire_order() {
    let server = MockServer::start().await;
    mount_ok(
        &server,
        sse(&[
            r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step one"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step two"}}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{"query":"x"}}}"#,
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"step one step two","signature":"SIG-INTERLEAVED"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"p\":1}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"content_block_start","index":3,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","url":"https://pawork.dev"}]}}"#,
            r#"{"type":"content_block_stop","index":3}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":6}}"#,
            r#"{"type":"message_stop"}"#,
        ]),
    )
    .await;

    let mut req = request("claude-sonnet-4-5");
    req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::High));
    req.hosted_tools.push(HostedToolRequest {
        name: "web_search".into(),
        kind: ToolCapabilityTag::WebSearch,
        description: String::new(),
        capabilities: Vec::new(),
        config: None,
    });
    req.tools.push(ToolDefinition {
        name: "read_file".into(),
        description: "read".into(),
        input_schema: serde_json::json!({"type":"object"}),
    });

    let store = MemoryStore::new();
    let provider = provider(&server).with_reasoning_continuation(store.handle());
    let sink = RecordingProviderSink::default();
    provider
        .stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("interleaved stream succeeds");

    use agent_domain::ServerToolEvent;
    let kinds: Vec<String> = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::ThinkingDelta(_) => Some("thinking".into()),
            ProviderStreamEvent::ReasoningItem(_) => Some("reasoning_item".into()),
            ProviderStreamEvent::ToolCallStarted { .. } => Some("tool_start".into()),
            ProviderStreamEvent::ToolCallArgumentsDelta { .. } => Some("tool_args".into()),
            ProviderStreamEvent::ToolCallCompleted { .. } => Some("tool_completed".into()),
            ProviderStreamEvent::ServerTool(ServerToolEvent::Started { .. }) => {
                Some("server_start".into())
            }
            ProviderStreamEvent::ServerTool(ServerToolEvent::SourceAdded { .. }) => {
                Some("server_source".into())
            }
            ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { .. }) => {
                Some("server_completed".into())
            }
            ProviderStreamEvent::ResponseCompleted(_) => Some("completed".into()),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "thinking".to_string(),
            "tool_start".to_string(),
            "thinking".to_string(),
            "server_start".to_string(),
            "reasoning_item".to_string(),
            "tool_args".to_string(),
            "tool_completed".to_string(),
            "server_source".to_string(),
            "server_completed".to_string(),
            "completed".to_string(),
        ]
    );
}

#[tokio::test]
async fn baseline_model_degrades_to_legacy_without_error() {
    let server = MockServer::start().await;
    mount_ok(&server, text_fixture("ok")).await;

    // claude-3-5-sonnet 只声明基线 transport：现代字段全部显式降级，不报错。
    let mut req = request("claude-3-5-sonnet");
    req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::XHigh));
    req.response_format = ResponseFormat::JsonSchema {
        name: "answer".into(),
        schema: serde_json::json!({"type":"object"}),
    };
    req.hosted_tools.push(HostedToolRequest {
        name: "web_search".into(),
        kind: ToolCapabilityTag::WebSearch,
        description: String::new(),
        capabilities: Vec::new(),
        config: None,
    });
    req.tools.push(ToolDefinition {
        name: "read_file".into(),
        description: "read".into(),
        input_schema: serde_json::json!({"type":"object"}),
    });

    let provider = provider(&server);
    let sink = RecordingProviderSink::default();
    let summary = provider
        .stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("degraded stream must not error");
    assert_eq!(summary.stop_reason, StopReason::Completed);

    // 降级可观察（ProviderMetadata），不静默。
    let degradations: Vec<String> = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::ProviderMetadata(metadata) => metadata
                .get("degradation")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            _ => None,
        })
        .collect();
    assert!(!degradations.is_empty(), "降级必须可观察");
    assert!(degradations
        .iter()
        .any(|note| note.contains("legacy baseline")));
    assert!(!sink
        .events()
        .iter()
        .any(|event| { matches!(event, ProviderStreamEvent::Error(_)) }));

    // legacy body：effort clamp 为 budget thinking（XHigh → High），
    // 结构化输出退回 system 指令，hosted tool 降级为 function calling。
    let body = last_request_body(&server).await;
    assert!(body.get("output_config").is_none());
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 255);
    assert!(body["system"]["text"]
        .as_str()
        .expect("system text")
        .contains("JSON Schema named `answer`"));
    assert_eq!(body["tools"][0]["name"], "read_file");
    assert_eq!(body["tools"].as_array().expect("tools").len(), 1);
}

#[tokio::test]
async fn unsupported_server_tool_kind_degrades_with_note_not_error() {
    let server = MockServer::start().await;
    mount_ok(&server, text_fixture("ok")).await;

    let mut req = request("claude-sonnet-4-5");
    req.hosted_tools.push(HostedToolRequest {
        name: "image_gen".into(),
        kind: ToolCapabilityTag::ImageGeneration,
        description: String::new(),
        capabilities: Vec::new(),
        config: None,
    });
    req.hosted_tools.push(HostedToolRequest {
        name: "web_search".into(),
        kind: ToolCapabilityTag::WebSearch,
        description: String::new(),
        capabilities: Vec::new(),
        config: None,
    });

    let provider = provider(&server);
    let sink = RecordingProviderSink::default();
    provider
        .stream(req, &sink, agent_domain::CancellationToken::new())
        .await
        .expect("modern stream succeeds");

    let body = last_request_body(&server).await;
    let tools = body["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1, "image_gen 降级，web_search 保留");
    assert_eq!(tools[0]["type"], "web_search_20250305");
    let degradations: Vec<String> = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::ProviderMetadata(metadata) => metadata
                .get("degradation")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            _ => None,
        })
        .collect();
    assert!(degradations.iter().any(|note| note.contains("image_gen")));
    assert!(!sink
        .events()
        .iter()
        .any(|event| { matches!(event, ProviderStreamEvent::Error(_)) }));
}
