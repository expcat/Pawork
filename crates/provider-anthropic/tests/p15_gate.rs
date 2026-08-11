//! P15-9 Phase 15 集中门禁 · Anthropic 子集（contract / golden / fuzz / compat）。
//!
//! 与 `scripts/p15-gate.sh` 对应：四类测试分别落在 `contract` / `golden` /
//! `fuzz` / `compat` 子模块，门禁脚本按模块名过滤执行。全部为确定性纯函数
//! 测试（不触网络、不读真实 Key），复用 `test_support::contract` 的 canonical
//! 断言，保证三家横向一致（ADR-015）。
//!
//! 覆盖 P15 场景：现代 Messages 传输归一、server tool（web_search / citation）
//! 往返、reasoning signature / redacted 只经 Protected Blob 引用往返（ADR-032）、
//! capability 协商降级。

use std::collections::BTreeSet;

use agent_domain::{
    ModelId, ProtectedBlobRef, ReasoningItem, ReasoningItemId, ServerToolEvent, Source,
    ThinkingContent, ToolCapabilityTag,
};
use model_registry::CapabilityEvidence;
use provider_anthropic::reasoning::{
    build_reasoning_item, extract_thinking_payload, reconstruct_block,
};
use provider_anthropic::server_tool::{
    citation_block_to_citation, server_tool_use_to_started, web_search_result_to_source,
    web_search_tool_result_to_events,
};
use provider_api::{
    CapabilityFallback, CapabilityRequirements, ModelCapabilities, ModelTransport, ReasoningConfig,
    ReasoningEffort,
};
use provider_runtime::negotiate::CapabilityNegotiator;
use proptest::prelude::*;
use serde_json::{json, Value};
use test_support::contract::{
    assert_capability_resolution_invariant, assert_citation_not_fabricated,
    assert_reasoning_item_protected_only_via_blob_ref, assert_server_tool_event_round_trip,
};

// ===== contract =====

mod contract {
    use super::*;

    /// Anthropic citation block 两类口径归一：char_location（Document）与
    /// web_search_result_location（WebSearch）。
    #[test]
    fn anthropic_citation_blocks_map_by_type() {
        let doc = citation_block_to_citation(&json!({
            "type": "char_location",
            "cited_text": "the cited passage",
            "document_index": 3,
        }))
        .unwrap();
        assert_eq!(doc.text.as_deref(), Some("the cited passage"));
        assert_eq!(doc.document_index, Some(3));
        assert_eq!(doc.source_kind, agent_domain::CitationSourceKind::Document);
        assert_citation_not_fabricated(&doc);

        let web = citation_block_to_citation(&json!({
            "type": "web_search_result_location",
            "url": "https://example.com/w",
            "title": "W",
            "cited_text": "web snippet",
        }))
        .unwrap();
        assert_eq!(web.url.as_deref(), Some("https://example.com/w"));
        assert_eq!(web.source_kind, agent_domain::CitationSourceKind::WebSearch);
        assert_citation_not_fabricated(&web);
    }

    /// `web_search_result` → Source，原始 metadata 保留。
    #[test]
    fn anthropic_web_search_result_maps_to_source() {
        let source: Source = web_search_result_to_source(&json!({
            "type": "web_search_result",
            "url": "https://example.com/r",
            "title": "R",
        }))
        .unwrap();
        assert_eq!(source.url.as_deref(), Some("https://example.com/r"));
        assert_eq!(source.title.as_deref(), Some("R"));
        assert!(source.raw_metadata.is_some());
    }

    /// `web_search_tool_result` block → 按序生命周期事件（SourceAdded ×N → Completed）。
    #[test]
    fn anthropic_web_search_tool_result_emits_ordered_events() {
        let events: Vec<ServerToolEvent> = web_search_tool_result_to_events(&json!({
            "type": "web_search_tool_result",
            "tool_use_id": "wsu-1",
            "content": [
                {"type":"web_search_result","url":"https://a.test","title":"A"},
                {"type":"web_search_result","url":"https://b.test","title":"B"},
            ],
        }))
        .unwrap();
        assert_eq!(events.len(), 3);
        // 前两条为 SourceAdded，末条为 Completed。
        for event in &events {
            assert_server_tool_event_round_trip(event);
        }
        assert!(matches!(events[0], ServerToolEvent::SourceAdded { .. }));
        assert!(matches!(*events.last().unwrap(), ServerToolEvent::Completed { .. }));
    }

    /// `server_tool_use` 仅在声明白名单内才归一为 Started。
    #[test]
    fn anthropic_server_tool_use_requires_whitelist() {
        let event = server_tool_use_to_started(
            &json!({"type":"server_tool_use","id":"stu-1","name":"web_search","input":{"query":"q"}}),
            &["web_search"],
        )
        .unwrap();
        assert_server_tool_event_round_trip(&event);
        // 未声明的 server tool 名 → fail-closed。
        let err = server_tool_use_to_started(
            &json!({"type":"server_tool_use","id":"stu-2","name":"web_search"}),
            &["code_execution"],
        );
        assert!(err.is_err());
    }

    /// Anthropic signature / redacted_thinking 续传凭证只进 Protected Blob；
    /// canonical item 不含 signature/data，重建的 wire block 仍含原文。
    #[test]
    fn anthropic_thinking_signature_round_trips_via_blob_ref() {
        let block = json!({
            "type": "thinking",
            "thinking": "visible reasoning text",
            "signature": "SIG-PAYLOAD",
        });
        let payload = extract_thinking_payload(&block).unwrap();
        let canonical: ReasoningItem = build_reasoning_item(
            ReasoningItemId::new("rs-anthropic-1"),
            ProtectedBlobRef::from("blob-anthropic-1"),
            &payload,
        );
        assert_eq!(canonical.protected_blob_ref.as_str(), "blob-anthropic-1");
        assert_reasoning_item_protected_only_via_blob_ref(&canonical);

        let thinking = ThinkingContent {
            text: "visible reasoning text".into(),
            reasoning_item_id: Some(ReasoningItemId::new("rs-anthropic-1")),
            redacted: false,
        };
        let reconstructed = reconstruct_block(&canonical, Some(&thinking), &payload).unwrap();
        let wire = reconstructed.to_string();
        assert!(wire.contains("SIG-PAYLOAD"), "重建 wire 应含 signature 原文");
        assert!(wire.contains("\"thinking\":\"visible reasoning text\""));
    }

    /// redacted_thinking 块同样只经 blob ref 往返，无关联文本。
    #[test]
    fn anthropic_redacted_thinking_round_trips_via_blob_ref() {
        let block = json!({"type":"redacted_thinking","data":"REDACTED-DATA"});
        let payload = extract_thinking_payload(&block).unwrap();
        let canonical = build_reasoning_item(
            ReasoningItemId::new("rs-anthropic-2"),
            ProtectedBlobRef::from("blob-anthropic-2"),
            &payload,
        );
        assert_reasoning_item_protected_only_via_blob_ref(&canonical);
        let reconstructed = reconstruct_block(&canonical, None, &payload).unwrap();
        assert!(reconstructed.to_string().contains("REDACTED-DATA"));
    }
}

// ===== golden =====

mod golden {
    use super::*;

    #[test]
    fn anthropic_citation_char_location_snapshot() {
        let citation = citation_block_to_citation(&json!({
            "type": "char_location",
            "cited_text": "the cited passage",
            "document_index": 3,
        }))
        .unwrap();
        insta::assert_json_snapshot!("anthropic_citation_char_location", citation);
    }

    #[test]
    fn anthropic_web_search_result_source_snapshot() {
        let source = web_search_result_to_source(&json!({
            "type": "web_search_result",
            "url": "https://example.com/r",
            "title": "R",
        }))
        .unwrap();
        insta::assert_json_snapshot!("anthropic_web_search_result_source", source);
    }

    #[test]
    fn anthropic_reasoning_item_snapshot() {
        let payload = extract_thinking_payload(&json!({
            "type": "thinking",
            "thinking": "visible reasoning text",
            "signature": "SIG-PAYLOAD",
        }))
        .unwrap();
        let canonical = build_reasoning_item(
            ReasoningItemId::new("rs-anthropic-1"),
            ProtectedBlobRef::from("blob-anthropic-1"),
            &payload,
        );
        insta::assert_json_snapshot!("anthropic_reasoning_item", canonical);
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

mod fuzz {
    use super::*;

    proptest! {
        #[test]
        fn anthropic_citation_block_never_panics(block in arb_json_object(vec![
            "type".into(), "cited_text".into(), "document_index".into(),
            "url".into(), "title".into(),
        ])) {
            if let Ok(citation) = citation_block_to_citation(&block) {
                assert_citation_not_fabricated(&citation);
            }
        }

        #[test]
        fn anthropic_web_search_result_never_panics(result in arb_json_object(vec![
            "type".into(), "url".into(), "title".into(),
        ])) {
            let _ = web_search_result_to_source(&result);
        }

        #[test]
        fn anthropic_thinking_block_never_panics_and_protected(
            block in arb_json_object(vec![
                "type".into(), "thinking".into(), "signature".into(), "data".into(),
            ])
        ) {
            if let Ok(payload) = extract_thinking_payload(&block) {
                let canonical = build_reasoning_item(
                    ReasoningItemId::new("rs-fuzz"),
                    ProtectedBlobRef::from("blob-fuzz"),
                    &payload,
                );
                assert_reasoning_item_protected_only_via_blob_ref(&canonical);
            }
            // 任意 thinking-shaped 输入都不 panic（redacted 无关联文本，重建可能 Err）。
        }

        #[test]
        fn anthropic_interleaved_citations_and_tools_preserve_count(
            seq in prop::collection::vec(
                prop::sample::select(vec!["citation".to_string(), "source".to_string(), "result".to_string()]),
                0..16,
            )
        ) {
            let mut mapped = 0usize;
            for tag in &seq {
                let ok = match tag.as_str() {
                    "citation" => citation_block_to_citation(&json!({
                        "type":"char_location","cited_text":"t","document_index":0
                    })).is_ok(),
                    "source" => web_search_result_to_source(&json!({
                        "type":"web_search_result","url":"https://e.test"
                    })).is_ok(),
                    "result" => web_search_tool_result_to_events(&json!({
                        "type":"web_search_tool_result","tool_use_id":"wsu-1","content":[]
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

    fn requirements() -> CapabilityRequirements {
        // Anthropic 现代 Messages 请求：web_search hosted tool + reasoning High + citations。
        let mut required_tools: BTreeSet<ToolCapabilityTag> = BTreeSet::new();
        required_tools.insert(ToolCapabilityTag::WebSearch);
        CapabilityRequirements {
            transport_pref: vec![ModelTransport::Messages],
            required_tools,
            reasoning: Some(ReasoningConfig::new(ReasoningEffort::High)),
            citations: true,
        }
    }

    fn evidence(transport: ModelTransport) -> CapabilityEvidence {
        CapabilityEvidence {
            model: ModelId::from("claude-test"),
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

    /// 同一 canonical 请求：现代 Messages 模型全 supported；仅 ChatCompletions 模型
    /// （P6-2 基线降级）→ transport 降级、citations 显式 unsupported、reasoning 仍
    /// 由本地 thinking 承接。两路均满足 `requested == supported ∪ unsupported`。
    #[test]
    fn anthropic_modern_request_degrades_consistently_on_legacy_only_model() {
        let requirements = requirements();

        let modern = CapabilityNegotiator::negotiate(&evidence(ModelTransport::Messages), &requirements);
        assert_eq!(modern.chosen_transport, ModelTransport::Messages);
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

    /// `Max` 在不支持细粒度 effort 的旧模型上 clamp 为 High（ClampedEffort）。
    #[test]
    fn anthropic_max_effort_clamps_on_legacy_model() {
        let mut requirements = requirements();
        requirements.reasoning = Some(ReasoningConfig {
            effort: ReasoningEffort::Max,
            ..ReasoningConfig::default()
        });
        let resolved = CapabilityNegotiator::negotiate(
            &evidence(ModelTransport::ChatCompletions),
            &requirements,
        );
        assert_capability_resolution_invariant(&resolved);
        assert_eq!(
            resolved.fallback.get("reasoning.effort"),
            Some(&CapabilityFallback::ClampedEffort)
        );
        assert!(resolved.supported.contains("reasoning"));
    }
}
