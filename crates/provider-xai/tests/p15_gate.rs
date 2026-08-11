//! P15-9 Phase 15 集中门禁 · xAI 子集（contract / golden / fuzz / compat）。
//!
//! 与 `scripts/p15-gate.sh` 对应：四类测试分别落在 `contract` / `golden` /
//! `fuzz` / `compat` 子模块，门禁脚本按模块名过滤执行。全部为确定性纯函数
//! 测试（不触网络、不读真实 Key），复用 `test_support::contract` 的 canonical
//! 断言，保证三家横向一致（ADR-015）。
//!
//! 覆盖 P15 场景：Responses 传输归一、Live Search sources / citation 归一、
//! reasoning encrypted_content 只经 Protected Blob 引用往返（ADR-032）、
//! capability 协商降级。

use std::collections::BTreeMap;

use agent_domain::{
    Citation, ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId,
    ProtectedBlobRef, ReasoningItem, ServerToolEvent, Source, TextContent, ToolCapabilityTag,
};
use model_registry::CapabilityEvidence;
use provider_api::{
    CapabilityFallback, CapabilityRequirements, HostedToolRequest, ModelCapabilities,
    ModelTransport, ReasoningConfig, ReasoningEffort, ResponseFormat, ToolChoice,
};
use provider_runtime::negotiate::CapabilityNegotiator;
use provider_xai::reasoning::{
    parse_responses_reasoning, to_reasoning_item, to_responses_input_reasoning,
};
use provider_xai::server_tool::{
    citation_url_to_citation, response_item_to_server_tool_event, url_citation_annotation_to_citation,
};
use provider_xai::live_search_source_to_source;
use provider_xai::responses::source_to_citation;
use proptest::prelude::*;
use serde_json::{json, Value};
use test_support::contract::{
    assert_capability_resolution_invariant, assert_citation_not_fabricated,
    assert_reasoning_item_protected_only_via_blob_ref, assert_server_tool_event_round_trip,
};

// ===== contract =====

mod contract {
    use super::*;

    /// xAI Live Search source 各类口径归一：纯 URL / web / x post / document。
    #[test]
    fn xai_live_search_sources_map_by_type() {
        // 纯字符串 URL。
        let s = live_search_source_to_source(&json!("https://e.test/a")).unwrap();
        assert_eq!(s.url.as_deref(), Some("https://e.test/a"));

        // web / url。
        let s = live_search_source_to_source(&json!({
            "type": "web", "url": "https://e.test/b", "title": "B", "snippet": "snip"
        }))
        .unwrap();
        assert_eq!(s.url.as_deref(), Some("https://e.test/b"));
        assert_eq!(s.snippet.as_deref(), Some("snip"));
        assert!(s.raw_metadata.is_some());

        // x post：保留 text + snippet。
        let s = live_search_source_to_source(&json!({
            "type": "x", "url": "https://x.test/p", "text": "post body"
        }))
        .unwrap();
        assert_eq!(s.text.as_deref(), Some("post body"));

        // document：保留 document_index + text。
        let s = live_search_source_to_source(&json!({
            "type": "document", "document_index": 2, "text": "doc body"
        }))
        .unwrap();
        assert_eq!(s.document_index, Some(2));
        assert_eq!(s.text.as_deref(), Some("doc body"));
    }

    /// 未知来源类型且无 url → None（fail-closed，不猜种类）。
    #[test]
    fn xai_unknown_source_without_url_is_fail_closed() {
        assert!(live_search_source_to_source(&json!({"type": "mystery"})).is_none());
        // 未知但有 url → 保留 raw metadata，不猜种类。
        let s = live_search_source_to_source(&json!({"type": "mystery", "url": "https://e.test"}))
            .unwrap();
        assert_eq!(s.url.as_deref(), Some("https://e.test"));
        assert!(s.raw_metadata.is_some());
    }

    /// source_to_citation 折叠保留可定位字段与来源种类。
    #[test]
    fn xai_source_folds_to_citation_preserving_kind() {
        let source = live_search_source_to_source(&json!({
            "type": "web", "url": "https://e.test/c", "title": "C", "snippet": "x"
        }))
        .unwrap();
        let citation: Citation = source_to_citation(&source, agent_domain::CitationSourceKind::WebSearch);
        assert_eq!(citation.url.as_deref(), Some("https://e.test/c"));
        assert_eq!(citation.source_kind, agent_domain::CitationSourceKind::WebSearch);
        assert_citation_not_fabricated(&citation);
    }

    /// 顶层 citations[] URL 与 annotations url_citation 双口径归一。
    #[test]
    fn xai_citation_url_and_annotation_map() {
        let from_url = citation_url_to_citation(&json!("https://e.test/u")).unwrap();
        assert_eq!(from_url.url.as_deref(), Some("https://e.test/u"));
        assert_eq!(from_url.source_kind, agent_domain::CitationSourceKind::Url);

        let from_ann = url_citation_annotation_to_citation(&json!({
            "type": "url_citation", "url": "https://e.test/v", "title": "V", "start_index": 7
        }))
        .unwrap();
        assert_eq!(from_ann.url.as_deref(), Some("https://e.test/v"));
        assert_eq!(from_ann.index, Some(7));
    }

    /// xAI server tool items（web/x/code/file/mcp）按 status 归一为生命周期事件。
    #[test]
    fn xai_server_tool_items_round_trip() {
        for item_type in ["web_search_call", "x_search_call", "code_interpreter_call", "file_search_call", "mcp_call"] {
            let item = json!({"type": item_type, "id": "st-1", "status": "completed"});
            let event = response_item_to_server_tool_event(&item)
                .unwrap_or_else(|err| panic!("`{item_type}` 应可映射：{err:?}"));
            assert_server_tool_event_round_trip(&event);
        }
        // 未知 item type → fail-closed。
        assert!(response_item_to_server_tool_event(&json!({"type":"mystery","id":"x","status":"completed"})).is_err());
    }

    /// reasoning encrypted_content 只进 Protected Blob；canonical item 不含凭证，
    /// 回灌重建的 wire 仍含 encrypted_content（仅在 wire 层）。
    #[test]
    fn xai_reasoning_encrypted_round_trips_via_blob_ref() {
        let item = json!({
            "type": "reasoning",
            "id": "rs-xai-1",
            "summary": [{"type":"summary_text","text":"reasoned briefly"}],
            "encrypted_content": "XAI-ENCRYPTED-PAYLOAD",
        });
        let parsed = parse_responses_reasoning(&item).unwrap();
        assert!(parsed.protected().is_some());
        let plaintext = parsed.protected().unwrap().as_str().to_owned();
        let canonical: ReasoningItem =
            to_reasoning_item(parsed, ProtectedBlobRef::from("blob-xai-1")).unwrap();
        assert_eq!(canonical.protected_blob_ref.as_str(), "blob-xai-1");
        assert_eq!(canonical.summary.as_deref(), Some("reasoned briefly"));
        assert_reasoning_item_protected_only_via_blob_ref(&canonical);

        let wire = to_responses_input_reasoning(&canonical, &plaintext);
        let wire_str = wire.to_string();
        assert!(wire_str.contains("encrypted_content"), "wire 应含 encrypted_content");
        assert!(wire_str.contains("XAI-ENCRYPTED-PAYLOAD"), "回灌应逐字节还原加密载荷");
    }

    /// ServerToolEvent::SourceAdded 可持久化往返。
    #[test]
    fn xai_source_event_round_trip() {
        let event = ServerToolEvent::SourceAdded {
            tool_call_id: agent_domain::ToolCallId::from("ls-1"),
            source: Source {
                url: Some("https://e.test".into()),
                ..Source::default()
            },
        };
        assert_server_tool_event_round_trip(&event);
    }
}

// ===== golden =====

mod golden {
    use super::*;

    #[test]
    fn xai_live_search_web_source_snapshot() {
        let source = live_search_source_to_source(&json!({
            "type": "web", "url": "https://example.com/w", "title": "W", "snippet": "snip"
        }))
        .unwrap();
        insta::assert_json_snapshot!("xai_live_search_web_source", source);
    }

    #[test]
    fn xai_live_search_document_source_snapshot() {
        let source = live_search_source_to_source(&json!({
            "type": "document", "document_index": 2, "title": "D", "text": "doc body"
        }))
        .unwrap();
        insta::assert_json_snapshot!("xai_live_search_document_source", source);
    }

    #[test]
    fn xai_reasoning_item_snapshot() {
        let parsed = parse_responses_reasoning(&json!({
            "type": "reasoning",
            "id": "rs-xai-1",
            "summary": [{"type":"summary_text","text":"reasoned briefly"}],
            "encrypted_content": "XAI-ENCRYPTED-PAYLOAD",
        }))
        .unwrap();
        let canonical = to_reasoning_item(parsed, ProtectedBlobRef::from("blob-xai-1")).unwrap();
        insta::assert_json_snapshot!("xai_reasoning_item", canonical);
    }
}

// ===== fuzz =====

fn arb_json_object(keys: Vec<String>) -> impl Strategy<Value = Value> {
    prop::collection::vec(
        (
            prop::sample::select(keys.clone()),
            prop_oneof![
                Just(Value::Null),
                any::<String>().prop_map(Value::String),
                (0u32..8).prop_map(|i| Value::from(i as u64)),
                Just(Value::Array(vec![])),
                Just(Value::Object(serde_json::Map::new())),
            ],
        ),
        0..8,
    )
    .prop_map(move |pairs| {
        let mut map = serde_json::Map::new();
        for (k, v) in pairs {
            map.insert(k, v);
        }
        Value::Object(map)
    })
}

fn arb_json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<String>().prop_map(Value::String),
        arb_json_object(vec![
            "type".into(), "url".into(), "title".into(), "snippet".into(),
            "text".into(), "document_index".into(), "index".into(),
            "id".into(), "summary".into(), "encrypted_content".into(),
        ]),
    ]
}

mod fuzz {
    use super::*;

    proptest! {
        #[test]
        fn xai_live_search_source_never_panics(value in arb_json_value()) {
            // 任意输入（字符串 / 对象 / null）都不 panic；结果 Ok/None 任一。
            let _ = live_search_source_to_source(&value);
        }

        #[test]
        fn xai_citation_url_never_panics(value in arb_json_value()) {
            let _ = citation_url_to_citation(&value);
        }

        #[test]
        fn xai_url_citation_annotation_never_panics(annotation in arb_json_object(vec![
            "type".into(), "url".into(), "title".into(), "start_index".into(),
        ])) {
            if let Ok(citation) = url_citation_annotation_to_citation(&annotation) {
                assert_citation_not_fabricated(&citation);
            }
        }

        #[test]
        fn xai_reasoning_never_panics_and_protected(
            item in arb_json_object(vec![
                "type".into(), "id".into(), "summary".into(), "encrypted_content".into(),
            ])
        ) {
            if let Ok(parsed) = parse_responses_reasoning(&item) {
                if let Some(protected) = parsed.protected() {
                    let plaintext = protected.as_str().to_owned();
                    if let Ok(canonical) = to_reasoning_item(parsed, ProtectedBlobRef::from("blob-fuzz")) {
                        assert_reasoning_item_protected_only_via_blob_ref(&canonical);
                        let wire = to_responses_input_reasoning(&canonical, &plaintext);
                        prop_assert!(wire.to_string().contains("encrypted_content"));
                    }
                }
            }
            // 任意 reasoning-shaped 输入都不 panic。
        }

        #[test]
        fn xai_interleaved_sources_and_tools_preserve_count(
            seq in prop::collection::vec(
                prop::sample::select(vec!["source".to_string(), "citation".to_string(), "tool".to_string()]),
                0..16,
            )
        ) {
            let mut mapped = 0usize;
            for tag in &seq {
                let ok = match tag.as_str() {
                    "source" => live_search_source_to_source(&json!({"type":"web","url":"https://e.test"})).is_some(),
                    "citation" => citation_url_to_citation(&json!("https://e.test")).is_ok(),
                    "tool" => response_item_to_server_tool_event(&json!({
                        "type":"web_search_call","id":"st-1","status":"completed"
                    })).is_ok(),
                    _ => false,
                };
                if ok { mapped += 1; }
            }
            // 全部 well-formed → 全部归一，interleaved 顺序不丢条目。
            prop_assert_eq!(mapped, seq.len());
        }
    }
}

// ===== compat（兼容性差分：协商降级不变量）=====

mod compat {
    use super::*;

    fn request_with_live_search_and_reasoning() -> provider_api::CanonicalModelRequest {
        provider_api::CanonicalModelRequest {
            request_id: agent_domain::RequestId::from("p15-xai-r1"),
            model: ModelId::from("grok-test"),
            messages: vec![Message {
                id: MessageId::new("m1"),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent { text: "live search pawork".into() })],
                metadata: MessageMetadata::default(),
            }],
            tools: Vec::new(),
            hosted_tools: vec![HostedToolRequest {
                name: "web_search".into(),
                kind: ToolCapabilityTag::WebSearch,
                description: String::new(),
                capabilities: Vec::new(),
                config: None,
            }],
            extensions: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            reasoning: Some(ReasoningConfig::new(ReasoningEffort::High)),
            temperature: Some(0.0),
            max_output_tokens: Some(256),
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: provider_api::PromptCachePreference::Automatic,
            budget: provider_api::RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: Some("trace-p15-xai".into()),
        }
    }

    fn evidence(transport: ModelTransport) -> CapabilityEvidence {
        CapabilityEvidence {
            model: ModelId::from("grok-test"),
            provider: None,
            static_declared: Some(ModelCapabilities {
                text: true,
                tool_calls: true,
                parallel_tool_calls: true,
                thinking: true,
                transport,
                hosted_tool_tags: [ToolCapabilityTag::WebSearch].into_iter().collect(),
                citations: transport != ModelTransport::ChatCompletions,
                ..ModelCapabilities::default()
            }),
            probe_declared: None,
            override_declared: None,
        }
    }

    /// 同一 canonical 请求：现代 Responses 模型全 supported；仅 ChatCompletions 模型
    /// （P6-10 基线降级）→ transport 降级、citations 显式 unsupported、reasoning 仍
    /// 由本地 thinking 承接。两路均满足 `requested == supported ∪ unsupported`。
    #[test]
    fn xai_modern_request_degrades_consistently_on_legacy_only_model() {
        let request = request_with_live_search_and_reasoning();
        let requirements: CapabilityRequirements = provider_xai::requirements_from_request(&request);
        assert!(requirements.transport_pref.contains(&ModelTransport::Responses));

        let modern = CapabilityNegotiator::negotiate(&evidence(ModelTransport::Responses), &requirements);
        assert_eq!(modern.chosen_transport, ModelTransport::Responses);
        assert_capability_resolution_invariant(&modern);
        assert!(modern.supported.contains("citations"));
        assert!(modern.supported.contains("reasoning"));
        assert!(modern.unsupported.is_empty());

        let legacy = CapabilityNegotiator::negotiate(&evidence(ModelTransport::ChatCompletions), &requirements);
        assert_eq!(legacy.chosen_transport, ModelTransport::ChatCompletions);
        assert_capability_resolution_invariant(&legacy);
        assert_eq!(
            legacy.fallback.get("transport"),
            Some(&CapabilityFallback::LegacyTransport)
        );
        assert!(legacy.unsupported.contains("citations"));
        assert!(legacy.supported.contains("reasoning"));
    }
}
