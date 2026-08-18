//! Provider 流事件的可复用最小断言。
//!
//! 不绑定具体 Provider：给定一组 [`ProviderStreamEvent`]，断言文本流与
//! tool-call 闭合形状。P15 capability / server_tool / citation / reasoning
//! 断言不在本波次迁入。

use pawork_domain::ProviderStreamEvent;

/// 断言文本流：至少含一条非空 TextDelta，并以 ResponseCompleted 收尾。
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

/// 统计某个事件变体的数量（测试辅助）。
pub fn count_variant<F>(events: &[ProviderStreamEvent], predicate: F) -> usize
where
    F: Fn(&ProviderStreamEvent) -> bool,
{
    events.iter().filter(|e| predicate(e)).count()
}

#[cfg(test)]
mod tests {
    use pawork_domain::{StopReason, ToolCallId};

    use super::*;

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
        assert_eq!(
            count_variant(&text_events(), |e| {
                matches!(e, ProviderStreamEvent::TextDelta(_))
            }),
            1
        );
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
}
