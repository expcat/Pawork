//! 字节级契约 golden(R1 波 A 前置建立于 pawork-api、golden 先行,随后随类型
//! 整组平移至 `pawork-domain`,ADR-039)。锁定 `ProviderStreamEvent` 13 变体
//! (tag=`type`/content=`data`/snake_case)、`ProviderError`、
//! `CanonicalModelRequest`、`ToolResult` 的 JSON 形状;形状演进必须显式
//! 重建夹具:
//!
//! ```sh
//! GOLDEN_UPDATE=1 cargo test -p pawork-domain --test contract_golden
//! ```

use std::{collections::BTreeMap, path::PathBuf};

use pawork_domain::{
    ArtifactId, ArtifactReference, CanonicalModelRequest, ContentPart, ErrorCategory, ErrorContext,
    ExtensionToolRequest, HostedToolRequest, Message, MessageId, MessageMetadata, MessageRole,
    ModelId, PromptCachePreference, ProtectedBlobRef, ProviderError, ProviderErrorKind,
    ProviderStreamEvent, ProviderTranscriptEnvelope, ReasoningConfig, ReasoningEffort,
    ReasoningItem, ReasoningItemId, RequestBudget, RequestId, ResponseFormat, ServerToolEvent,
    StopReason, TextContent, ThinkingConfig, ThinkingLevel, TokenUsage, ToolCallId,
    ToolCapabilityTag, ToolChoice, ToolDefinition, ToolResult, TranscriptItem,
};
use serde::Serialize;
use serde_json::Value;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// 逐行比对(必要时重建)golden 文件:字节相等 + 反序列化回读相等。
/// serde_json 对同一结构的序列化输出是确定性的(字段按声明序、BTreeMap 排序)。
fn check_golden<T>(file: &str, label: &str, values: &[T])
where
    T: Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    let actual: Vec<String> = values
        .iter()
        .map(|v| serde_json::to_string(v).expect("serialize golden value"))
        .collect();
    let path = fixtures_dir().join(file);
    if std::env::var_os("GOLDEN_UPDATE").is_some() {
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("mkdir fixtures");
        std::fs::write(&path, actual.join("\n") + "\n").expect("write fixture");
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "read {} failed: {error}; 先以 GOLDEN_UPDATE=1 重建夹具",
            path.display()
        )
    });
    let expected_lines: Vec<&str> = expected.lines().collect();
    assert_eq!(
        expected_lines.len(),
        actual.len(),
        "{label}: 夹具行数与构造数不一致"
    );
    for (index, (expected_line, (actual_line, value))) in expected_lines
        .iter()
        .zip(actual.iter().zip(values.iter()))
        .enumerate()
    {
        assert_eq!(
            expected_line, actual_line,
            "{label}[{index}]: JSON 形状漂移——若属契约演进,须 ADR + 显式 GOLDEN_UPDATE"
        );
        let decoded: T = serde_json::from_str(expected_line).expect("deserialize fixture line");
        assert_eq!(&decoded, value, "{label}[{index}]: 夹具回读与构造值不相等");
    }
}

fn full_provider_error() -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::RateLimited,
        message: "slow down".into(),
        retryable: true,
        retry_after_ms: Some(250),
        provider_request_id: Some("req-upstream-9".into()),
        http_status: Some(429),
        redacted_details: Some("[REDACTED] upstream body".into()),
        diagnostics: BTreeMap::from([("upstream".to_owned(), "HTTP 429".to_owned())]),
    }
}

/// 13 变体顺序即声明序;增减变体时同步更新计数断言与夹具。
fn all_stream_events() -> Vec<ProviderStreamEvent> {
    vec![
        ProviderStreamEvent::ResponseStarted {
            response_id: Some("resp-1".into()),
        },
        ProviderStreamEvent::TextDelta("hello".into()),
        ProviderStreamEvent::ThinkingDelta("考虑中".into()),
        ProviderStreamEvent::ReasoningItem(ReasoningItem {
            id: ReasoningItemId::from("reasoning-1"),
            summary: Some("safe summary".into()),
            protected_blob_ref: ProtectedBlobRef::from("protected-1"),
            opaque_metadata: BTreeMap::from([("origin".to_owned(), Value::from("wire"))]),
            continuation_metadata: BTreeMap::new(),
        }),
        ProviderStreamEvent::ToolCallStarted {
            id: ToolCallId::from("tc-1"),
            name: "read_file".into(),
        },
        ProviderStreamEvent::ToolCallArgumentsDelta {
            id: ToolCallId::from("tc-1"),
            json: "{\"path\":".into(),
        },
        ProviderStreamEvent::ToolCallCompleted {
            id: ToolCallId::from("tc-1"),
        },
        ProviderStreamEvent::UsageUpdated(TokenUsage {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 1,
            cache_write_tokens: 2,
        }),
        ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse),
        ProviderStreamEvent::ProviderMetadata(serde_json::json!({"wire": "v1"})),
        ProviderStreamEvent::ServerTool(ServerToolEvent::Started {
            tool_call_id: ToolCallId::from("st-1"),
            name: "web_search".into(),
            arguments: Some(serde_json::json!({"query": "pawork"})),
        }),
        ProviderStreamEvent::TranscriptEnvelope(ProviderTranscriptEnvelope {
            items: vec![TranscriptItem::Text("final".into())],
            cursor: Some("cursor-1".into()),
            continuation_reference: Some("ref-1".into()),
        }),
        ProviderStreamEvent::Error(full_provider_error()),
    ]
}

fn full_request() -> CanonicalModelRequest {
    CanonicalModelRequest {
        request_id: RequestId::from("request-1"),
        model: ModelId::from("glm-4.7"),
        messages: vec![Message {
            id: MessageId::from("message-1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: "hi".into() })],
            metadata: MessageMetadata::default(),
        }],
        tools: vec![ToolDefinition {
            name: "read_file".into(),
            description: "read".into(),
            // 键序须按字典序书写:自由形 Value 的序列化键序随 preserve_order
            // 特性统一解析而变(开=插入序/关=BTreeMap 排序);按字典序构造则
            // 两种解析下字节一致,byte golden 与调用方分组无关(R1 波 E 收口发现)。
            input_schema: serde_json::json!({"properties": {"path": {"type": "string"}}, "type": "object"}),
        }],
        hosted_tools: vec![HostedToolRequest {
            name: "web_search".into(),
            kind: ToolCapabilityTag::WebSearch,
            description: "search the web".into(),
            capabilities: vec![ToolCapabilityTag::WebSearch],
            config: Some(serde_json::json!({"max_results": 5})),
        }],
        extensions: vec![ExtensionToolRequest {
            name: "remote_mcp".into(),
            reference: "mcp://connector/search".into(),
            description: String::new(),
            capabilities: Vec::new(),
            requires_approval: true,
        }],
        tool_choice: ToolChoice::Named("read_file".into()),
        thinking: Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(1024),
        }),
        reasoning: Some(ReasoningConfig::new(ReasoningEffort::High)),
        temperature: Some(0.5),
        max_output_tokens: Some(4096),
        stop_sequences: vec!["END".into()],
        response_format: ResponseFormat::JsonSchema {
            name: "answer".into(),
            schema: serde_json::json!({"type": "object"}),
        },
        prompt_cache: PromptCachePreference::Required,
        budget: RequestBudget {
            timeout_ms: Some(30_000),
            max_cost_micros: Some(1500),
            max_input_tokens: Some(8192),
        },
        provider_options: BTreeMap::from([(
            "custom".to_owned(),
            serde_json::json!({"enabled": true}),
        )]),
        trace_id: Some("trace-1".into()),
    }
}

fn tool_results() -> Vec<ToolResult> {
    vec![
        ToolResult {
            content: vec![ContentPart::Text(TextContent {
                text: "done".into(),
            })],
            artifacts: vec![ArtifactReference {
                id: ArtifactId::from("artifact-1"),
                media_type: "text/plain".into(),
                byte_length: 42,
                content_hash: Some("sha256:abc".into()),
                label: Some("log".into()),
            }],
            metadata: serde_json::json!({"elapsed_ms": 7}),
            truncated: true,
            success: true,
            error: None,
        },
        ToolResult::failure(ErrorContext {
            category: ErrorCategory::Timeout,
            message: "tool timed out".into(),
            retryable: true,
            retry_after_ms: Some(100),
            diagnostics: BTreeMap::from([("tool".to_owned(), "run_command".to_owned())]),
        }),
    ]
}

#[test]
fn provider_stream_event_13_variants_byte_golden() {
    let events = all_stream_events();
    assert_eq!(events.len(), 13, "ProviderStreamEvent 变体数锁定为 13");
    check_golden(
        "provider_stream_event_13.jsonl",
        "ProviderStreamEvent",
        &events,
    );
}

#[test]
fn provider_error_byte_golden() {
    check_golden(
        "provider_error_full.json",
        "ProviderError",
        &[full_provider_error()],
    );
}

#[test]
fn canonical_model_request_byte_golden() {
    check_golden(
        "canonical_model_request_full.json",
        "CanonicalModelRequest",
        &[full_request()],
    );
}

#[test]
fn tool_result_byte_golden() {
    check_golden("tool_result_pair.jsonl", "ToolResult", &tool_results());
}
