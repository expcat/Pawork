//! Provider Contract Test 可复用断言（P2-11）。
//!
//! 不绑定具体 Provider：给定一组 [`ProviderStreamEvent`]，提供行为可横向对比的
//! 断言函数。每个新增 Provider 须通过这些断言（覆盖 ADR-015 的用例集）。

use agent_domain::StopReason;
use agent_domain::{Citation, CitationSourceKind, ReasoningItem, ServerToolEvent};
use provider_api::{ProviderError, ProviderErrorKind, ProviderStreamEvent, ResolvedCapabilities};

/// 断言文本流：至少含一条 TextDelta，并以 ResponseCompleted 收尾。
pub fn assert_text_stream(events: &[ProviderStreamEvent]) {
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::TextDelta(t) if !t.is_empty())),
        "text 流应至少含一条非空 TextDelta，实际：{events:?}"
    );
    assert!(
        matches!(
            events.last(),
            Some(ProviderStreamEvent::ResponseCompleted(_))
        ),
        "文本流应以 ResponseCompleted 收尾，实际末尾：{:?}",
        events.last()
    );
}

/// 断言单个 tool call 闭合：Started → ArgumentsDelta(可多条) → Completed。
pub fn assert_single_tool_call(events: &[ProviderStreamEvent]) {
    let started = events
        .iter()
        .find(|e| matches!(e, ProviderStreamEvent::ToolCallStarted { .. }));
    assert!(started.is_some(), "应存在 ToolCallStarted");
    let id = match started.unwrap() {
        ProviderStreamEvent::ToolCallStarted { id, .. } => id.clone(),
        _ => unreachable!(),
    };
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::ToolCallCompleted { id: cid } if cid == &id)),
        "tool call {id} 应被 Completed 闭合"
    );
}

/// 断言两个 tool call 并行交错且各自闭合。
pub fn assert_parallel_tool_calls(events: &[ProviderStreamEvent]) {
    let started_ids: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ProviderStreamEvent::ToolCallStarted { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        started_ids.len() >= 2,
        "并行 tool call 应至少有两个 Started（实际 {}）",
        started_ids.len()
    );
    for id in &started_ids {
        assert!(
            events.iter().any(
                |e| matches!(e, ProviderStreamEvent::ToolCallCompleted { id: cid } if cid == id)
            ),
            "tool call {id} 应被 Completed 闭合"
        );
    }
}

/// 断言 usage 已归一且 stop reason 符合预期。
pub fn assert_usage_and_stop(events: &[ProviderStreamEvent], expected_stop: StopReason) {
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::UsageUpdated(u) if u.total_tokens() > 0)),
        "应有非零 UsageUpdated"
    );
    let actual_stop = events.iter().find_map(|e| match e {
        ProviderStreamEvent::ResponseCompleted(stop) => Some(stop.clone()),
        _ => None,
    });
    assert_eq!(actual_stop, Some(expected_stop), "stop reason 不符预期");
}

/// 断言错误事件或 `stream()` 返回错误至少有一处归一为指定类别。
pub fn assert_error_kind(
    events: &[ProviderStreamEvent],
    stream_error: Option<&ProviderError>,
    kind: ProviderErrorKind,
) {
    let event_matches = events.iter().any(|e| match e {
        ProviderStreamEvent::Error(err) => err.kind == kind,
        _ => false,
    });
    let return_matches = stream_error.is_some_and(|error| error.kind == kind);
    assert!(
        event_matches || return_matches,
        "应存在 kind={kind:?} 的 Error 事件或 stream 返回错误，事件：{events:?}，返回错误：{stream_error:?}"
    );
}

/// 统计某个事件变体的数量（测试辅助）。
pub fn count_variant<F>(events: &[ProviderStreamEvent], predicate: F) -> usize
where
    F: Fn(&ProviderStreamEvent) -> bool,
{
    events.iter().filter(|e| predicate(e)).count()
}

// ----- Phase 15（P15-9）共享断言：citation / reasoning / capability / server tool -----
//
// 以下断言不绑定具体 Provider：任意 adapter 归一后的 canonical 结构都应满足，
// 用于三家 p15_gate.rs 的 contract / fuzz / 兼容性门禁复用，保证 ADR-015 横向
// 一致与 ADR-032 受保护材料守护。

/// 断言能力协商结果满足 `requested == supported ∪ unsupported`：每项请求必须
/// 显式落到 supported 或 unsupported，禁止静默丢弃或伪造（P15-8 不变量）。
pub fn assert_capability_resolution_invariant(resolved: &ResolvedCapabilities) {
    let union: std::collections::BTreeSet<String> = resolved
        .supported
        .iter()
        .chain(resolved.unsupported.iter())
        .cloned()
        .collect();
    assert_eq!(
        resolved.requested, union,
        "requested 必须 == supported ∪ unsupported（禁止静默丢弃/伪造）"
    );
}

/// 断言 [`ServerToolEvent`] 经 JSON round-trip 后 `type_name` 与 `tool_call_id`
/// 保持稳定——可持久化、可重放（ADR-016）。
pub fn assert_server_tool_event_round_trip(event: &ServerToolEvent) {
    let type_name = event.type_name();
    let id = event.tool_call_id().clone();
    let value = serde_json::to_value(event).expect("serialize server tool event");
    let decoded: ServerToolEvent =
        serde_json::from_value(value).expect("deserialize server tool event");
    assert_eq!(
        decoded.type_name(),
        type_name,
        "type_name 应在 round-trip 后稳定"
    );
    assert_eq!(
        decoded.tool_call_id(),
        &id,
        "tool_call_id 应在 round-trip 后稳定"
    );
}

/// 断言 [`Citation`] 不在 `source_kind == Unknown` 的同时伪造 url/title——
/// 缺省字段保持空而非猜值（P15-5 归一契约）。
pub fn assert_citation_not_fabricated(citation: &Citation) {
    if citation.source_kind == CitationSourceKind::Unknown {
        assert!(
            citation.url.is_none() && citation.title.is_none(),
            "Unknown citation 不应伪造 url/title：{citation:?}"
        );
    }
}

/// 断言受保护 reasoning 凭证只经 `protected_blob_ref` 引用：canonical
/// [`ReasoningItem`] 的 JSON 序列化中绝不出现受保护明文材料（ADR-032）。
///
/// 仅禁止凭证专有字段名——`encrypted_content`（OpenAI/xAI 密文）、`signature`
/// （Anthropic 签名）、`reasoning_content`（通用）与密钥前缀 `sk-`。结构性
/// 翻译提示（Anthropic `anthropic_block_kind` = `"thinking"` / `"redacted_thinking"`，
/// OpenAI `openai.responses.summary_entries`）是非敏感 kind/条目镜像，放行。
pub fn assert_reasoning_item_protected_only_via_blob_ref(item: &ReasoningItem) {
    let json = serde_json::to_string(item).expect("serialize reasoning item");
    assert!(
        json.contains("protected_blob_ref"),
        "ReasoningItem 应携带 protected_blob_ref 引用，实际：{json}"
    );
    for forbidden in ["encrypted_content", "signature", "reasoning_content", "sk-"] {
        assert!(
            !json.contains(forbidden),
            "ReasoningItem 序列化不得出现受保护材料 `{forbidden}`（ADR-032），实际：{json}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{TokenUsage, ToolCallId};

    fn text_events() -> Vec<ProviderStreamEvent> {
        vec![
            ProviderStreamEvent::ResponseStarted { response_id: None },
            ProviderStreamEvent::TextDelta("hi".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ]
    }

    #[test]
    fn text_assertions_pass_on_valid_stream() {
        assert_text_stream(&text_events());
    }

    #[test]
    fn single_tool_call_assertion_passes() {
        let id = ToolCallId::new("c1");
        let events = vec![
            ProviderStreamEvent::ToolCallStarted {
                id: id.clone(),
                name: "x".into(),
            },
            ProviderStreamEvent::ToolCallArgumentsDelta {
                id: id.clone(),
                json: "{}".into(),
            },
            ProviderStreamEvent::ToolCallCompleted { id },
            ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse),
        ];
        assert_single_tool_call(&events);
    }

    #[test]
    fn parallel_tool_call_assertion_passes() {
        let a = ToolCallId::new("a");
        let b = ToolCallId::new("b");
        let events = vec![
            ProviderStreamEvent::ToolCallStarted {
                id: a.clone(),
                name: "x".into(),
            },
            ProviderStreamEvent::ToolCallStarted {
                id: b.clone(),
                name: "y".into(),
            },
            ProviderStreamEvent::ToolCallCompleted { id: a },
            ProviderStreamEvent::ToolCallCompleted { id: b },
        ];
        assert_parallel_tool_calls(&events);
    }

    #[test]
    fn usage_and_stop_assertion_passes() {
        let events = vec![
            ProviderStreamEvent::UsageUpdated(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            }),
            ProviderStreamEvent::ResponseCompleted(StopReason::MaxTokens),
        ];
        assert_usage_and_stop(&events, StopReason::MaxTokens);
    }

    #[test]
    fn error_kind_accepts_matching_stream_error() {
        let error = ProviderError::new(ProviderErrorKind::Timeout, "timed out");
        assert_error_kind(&[], Some(&error), ProviderErrorKind::Timeout);
    }

    #[test]
    #[should_panic(expected = "应存在")]
    fn error_kind_rejects_empty_evidence() {
        assert_error_kind(&[], None, ProviderErrorKind::Timeout);
    }
}
