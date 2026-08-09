//! Provider Contract Test 可复用断言（P2-11）。
//!
//! 不绑定具体 Provider：给定一组 [`ProviderStreamEvent`]，提供行为可横向对比的
//! 断言函数。每个新增 Provider 须通过这些断言（覆盖 ADR-015 的用例集）。

use agent_domain::StopReason;
use provider_api::{ProviderError, ProviderErrorKind, ProviderStreamEvent};

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
