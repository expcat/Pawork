//! P15-9 Phase 15 集中门禁 · OpenAI 子集（contract / golden / fuzz / compat）。
//!
//! 与 `scripts/p15-gate.sh` 对应：四类测试分别落在 `contract` / `golden` /
//! `fuzz` / `compat` 子模块，门禁脚本按模块名过滤执行。全部为确定性纯函数
//! 测试（不触网络、不读真实 Key），复用 `test_support::contract` 的 canonical
//! 断言，保证三家横向一致（ADR-015）。
//!
//! 覆盖 P15 场景：Responses 传输归一、server tool（web_search）往返、reasoning
//! 加密凭证只经 Protected Blob 引用往返（ADR-032）、capability 协商降级。

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
use provider_openai::reasoning::{
    canonical_reasoning_to_responses_input, extract_encrypted_content,
    responses_reasoning_to_canonical, EncryptedContent,
};
use provider_openai::server_tool::{
    response_item_to_server_tool_event, url_citation_annotation_to_citation,
    web_search_source_to_source,
};
use provider_runtime::negotiate::CapabilityNegotiator;
use proptest::prelude::*;
use serde_json::{json, Value};
use test_support::contract::{
    assert_capability_resolution_invariant, assert_citation_not_fabricated,
    assert_reasoning_item_protected_only_via_blob_ref, assert_server_tool_event_round_trip,
};

mod common {
    use super::*;

    pub(crate) fn request_with_hosted_web_search_and_reasoning()
        -> provider_api::CanonicalModelRequest
    {
        provider_api::CanonicalModelRequest {
            request_id: agent_domain::RequestId::from("p15-r1"),
            model: ModelId::from("o3"),
            messages: vec![Message {
                id: MessageId::new("m1"),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent { text: "search pawork".into() })],
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
            trace_id: Some("trace-p15-openai".into()),
        }
    }
}

// ===== contract =====

mod contract {
    use super::*;

    /// Responses `web_search_call` item → canonical ServerTool 生命周期事件往返。
    #[test]
    fn openai_web_search_item_round_trips_through_server_tool_event() {
        for (status, expected_type) in [
            ("searching", "server_tool_started"),
            ("completed", "server_tool_completed"),
            ("failed", "server_tool_failed"),
        ] {
            let item = json!({
                "type": "web_search_call",
                "id": "ws-1",
                "status": status,
                "action": {"query": "pawork"},
                "error": {"code": "ECONNREFUSED", "message": "boom"},
            });
            let event = response_item_to_server_tool_event(&item)
                .unwrap_or_else(|err| panic!("web_search_call `{status}` 应可映射：{err:?}"));
            assert_eq!(event.type_name(), expected_type);
            assert_server_tool_event_round_trip(&event);
        }
    }

    /// `url_citation` annotation 归一：定位 index 来自 `start_index`，缺省不猜。
    #[test]
    fn openai_url_citation_maps_index_and_kind() {
        let annotation = json!({
            "type": "url_citation",
            "url": "https://example.com/a",
            "title": "Example",
            "start_index": 42,
        });
        let citation: Citation = url_citation_annotation_to_citation(&annotation).unwrap();
        assert_eq!(citation.url.as_deref(), Some("https://example.com/a"));
        assert_eq!(citation.title.as_deref(), Some("Example"));
        assert_eq!(citation.index, Some(42));
        assert_eq!(citation.source_kind, agent_domain::CitationSourceKind::Url);
        assert_citation_not_fabricated(&citation);
    }

    /// `web_search_call.action.sources[]` → Source，原始 metadata 保真保留。
    #[test]
    fn openai_web_search_source_preserves_raw_metadata() {
        let wire = json!({
            "url": "https://example.com/s",
            "title": "S",
            "snippet": "snippet text",
            "extra": {"keep": true},
        });
        let source: Source = web_search_source_to_source(&wire).unwrap();
        assert_eq!(source.url.as_deref(), Some("https://example.com/s"));
        assert_eq!(source.snippet.as_deref(), Some("snippet text"));
        let raw = source.raw_metadata.expect("raw_metadata 保留");
        assert_eq!(raw["extra"]["keep"], json!(true));
    }

    /// reasoning encrypted_content 只进 Protected Blob；canonical item 不含凭证，
    /// 回灌重建的 wire 仍含 encrypted_content（仅在 wire 层）。
    #[test]
    fn openai_reasoning_encrypted_round_trips_via_blob_ref() {
        let item = json!({
            "type": "reasoning",
            "id": "rs-1",
            "summary": [{"type":"summary_text","text":"checked constraints"}],
            "encrypted_content": "ENCRYPTED-BLOB-PAYLOAD",
        });
        let extracted: EncryptedContent =
            extract_encrypted_content(&item).unwrap().expect("应抽出 encrypted_content");
        let plaintext = extracted.into_inner();
        let canonical: ReasoningItem = responses_reasoning_to_canonical(
            &item,
            ProtectedBlobRef::from("blob-openai-1"),
        )
        .expect("归一 reasoning item");
        assert_eq!(canonical.protected_blob_ref.as_str(), "blob-openai-1");
        assert_reasoning_item_protected_only_via_blob_ref(&canonical);

        let wire =
            canonical_reasoning_to_responses_input(&canonical, &plaintext).expect("回灌重建");
        let wire_str = wire.to_string();
        assert!(wire_str.contains("encrypted_content"), "wire 应含 encrypted_content");
        assert!(
            wire_str.contains("ENCRYPTED-BLOB-PAYLOAD"),
            "回灌应逐字节还原加密载荷"
        );
        assert!(wire_str.contains("\"id\":\"rs-1\""));
    }

    /// ServerToolEvent::CitationAdded 也可持久化往返。
    #[test]
    fn openai_citation_event_round_trip() {
        let id = agent_domain::ToolCallId::from("ws-1");
        let citation_event = ServerToolEvent::CitationAdded {
            tool_call_id: id,
            citation: Citation {
                url: Some("https://example.com".into()),
                source_kind: agent_domain::CitationSourceKind::WebSearch,
                ..Citation::empty()
            },
        };
        assert_server_tool_event_round_trip(&citation_event);
    }
}

// ===== golden =====

mod golden {
    use super::*;

    #[test]
    fn openai_url_citation_snapshot() {
        let citation = url_citation_annotation_to_citation(&json!({
            "type": "url_citation",
            "url": "https://example.com/a",
            "title": "Example",
            "start_index": 42,
        }))
        .unwrap();
        insta::assert_json_snapshot!("openai_url_citation", citation);
    }

    #[test]
    fn openai_web_search_source_snapshot() {
        let source = web_search_source_to_source(&json!({
            "url": "https://example.com/s",
            "title": "S",
            "snippet": "snippet text",
        }))
        .unwrap();
        insta::assert_json_snapshot!("openai_web_search_source", source);
    }

    #[test]
    fn openai_reasoning_item_snapshot() {
        let canonical = responses_reasoning_to_canonical(
            &json!({
                "type": "reasoning",
                "id": "rs-1",
                "summary": [{"type":"summary_text","text":"checked constraints"}],
                "encrypted_content": "ENCRYPTED-BLOB-PAYLOAD",
            }),
            ProtectedBlobRef::from("blob-openai-1"),
        )
        .unwrap();
        insta::assert_json_snapshot!("openai_reasoning_item", canonical);
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
        fn openai_url_citation_never_panics(annotation in arb_json_object(vec![
            "type".into(), "url".into(), "title".into(), "start_index".into(),
        ])) {
        if let Ok(citation) = url_citation_annotation_to_citation(&annotation) {
            assert_citation_not_fabricated(&citation);
        }
    }

        #[test]
        fn openai_web_search_source_never_panics(source in arb_json_object(vec![
            "type".into(), "url".into(), "title".into(), "snippet".into(),
        ])) {
            let _ = web_search_source_to_source(&source);
        }

        #[test]
        fn openai_reasoning_never_panics_and_protected(
            item in arb_json_object(vec![
                "type".into(), "id".into(), "summary".into(), "encrypted_content".into(),
            ])
        ) {
            if let Ok(Some(content)) = extract_encrypted_content(&item) {
                let plaintext = content.into_inner();
                if let Ok(canonical) = responses_reasoning_to_canonical(
                    &item,
                    ProtectedBlobRef::from("blob-fuzz"),
                ) {
                    assert_reasoning_item_protected_only_via_blob_ref(&canonical);
                    if let Ok(wire) = canonical_reasoning_to_responses_input(&canonical, &plaintext) {
                        prop_assert!(wire.to_string().contains("encrypted_content"));
                    }
                }
            }
        }

        #[test]
        fn openai_interleaved_server_tools_preserve_count(
            seq in prop::collection::vec(
                prop::sample::select(vec!["web_search".to_string(), "citation".to_string(), "source".to_string()]),
                0..16,
            )
        ) {
            let mut mapped = 0usize;
            for tag in &seq {
                let ok = match tag.as_str() {
                    "web_search" => response_item_to_server_tool_event(&json!({
                        "type":"web_search_call","id":"ws-1","status":"completed"
                    })).is_ok(),
                    "citation" => url_citation_annotation_to_citation(&json!({
                        "type":"url_citation","url":"https://e.test","title":"t"
                    })).is_ok(),
                    "source" => web_search_source_to_source(&json!({"url":"https://e.test"})).is_ok(),
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
    use super::common::request_with_hosted_web_search_and_reasoning;
    use super::*;

    fn evidence(transport: ModelTransport) -> CapabilityEvidence {
        CapabilityEvidence {
            model: ModelId::from("o3"),
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

    /// 同一 canonical 请求（hosted web_search + reasoning High）经两路协商：
    /// 现代 Responses 模型下 web_search / citations / reasoning 全 supported；
    /// 仅 ChatCompletions 模型（P6 基线）协商降级到 ChatCompletions，citations 进
    /// unsupported（Reject）、transport 记 LegacyTransport，reasoning 仍由本地
    /// thinking 承接（不丢弃）。两路均满足 `requested == supported ∪ unsupported`。
    #[test]
    fn openai_modern_request_degrades_consistently_on_legacy_only_model() {
        let request = request_with_hosted_web_search_and_reasoning();
        let requirements: CapabilityRequirements =
            provider_openai::requirements_from_request(&request);
        // 请求层偏好 Responses。
        assert!(requirements.transport_pref.contains(&ModelTransport::Responses));

        // 现代路径：模型声明 Responses，全部 supported。
        let modern = CapabilityNegotiator::negotiate(&evidence(ModelTransport::Responses), &requirements);
        assert_eq!(modern.chosen_transport, ModelTransport::Responses);
        assert_capability_resolution_invariant(&modern);
        assert!(modern.supported.contains("citations"));
        assert!(modern.supported.contains("reasoning"));
        assert!(modern.unsupported.is_empty());

        // 旧路径：模型仅 ChatCompletions（P6 基线），降级但保持不变量。
        let legacy = CapabilityNegotiator::negotiate(&evidence(ModelTransport::ChatCompletions), &requirements);
        assert_eq!(legacy.chosen_transport, ModelTransport::ChatCompletions);
        assert_capability_resolution_invariant(&legacy);
        assert_eq!(
            legacy.fallback.get("transport"),
            Some(&CapabilityFallback::LegacyTransport)
        );
        // 不支持的 citations 显式进 unsupported（不静默丢弃）。
        assert!(legacy.unsupported.contains("citations"));
        // reasoning 由 thinking=true 承接，仍 supported。
        assert!(legacy.supported.contains("reasoning"));
    }

    /// `XHigh` 在不支持细粒度 effort 的旧模型上 clamp 为 High，记录
    /// `ClampedEffort`，不形成双轨。
    #[test]
    fn openai_xhigh_effort_clamps_on_legacy_model() {
        let mut request = request_with_hosted_web_search_and_reasoning();
        request.reasoning = Some(ReasoningConfig {
            effort: ReasoningEffort::XHigh,
            ..ReasoningConfig::default()
        });
        let requirements = provider_openai::requirements_from_request(&request);
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
