//! providers 集成测试共享件：SSE wire 样例与流断言的单一来源（MOCK-7）。
//!
//! 各集成测试二进制以 `mod common;` 引入。本模块只依赖 `pawork-domain`
//! 与纯字符串拼装，不触碰 feature 门控 API；不同二进制按需取用。

#![allow(dead_code)]

use pawork_domain::{ProviderError, ProviderErrorKind, ProviderStreamEvent, StopReason};

/// 拼装 SSE 数据帧（`data: {chunk}\n\n`），不追加终止帧。
pub fn sse_frames(chunks: &[&str]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(chunk);
        body.push_str("\n\n");
    }
    body
}

/// Chat Completions SSE 响应体：数据帧 + 终止 `data: [DONE]\n\n`。
pub fn sse_body(chunks: &[&str]) -> String {
    let mut body = sse_frames(chunks);
    body.push_str("data: [DONE]\n\n");
    body
}

/// chat 文本流样例：两段文本 delta + `finish_reason=stop`。
pub fn chat_text_stream_body() -> String {
    sse_body(&[
        r#"{"choices":[{"delta":{"content":"Hello"}}]}"#,
        r#"{"choices":[{"delta":{"content":" world"}}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    ])
}

/// chat 单工具调用样例：arguments JSON 跨两段 delta + `finish_reason=tool_calls`。
pub fn chat_tool_call_body() -> String {
    sse_body(&[
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"p\":"}}]} }]}"#,
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]} }]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
    ])
}

/// chat usage + 截断样例：文本 delta + `finish_reason=length` + 独立 usage chunk。
pub fn chat_usage_stop_body() -> String {
    sse_body(&[
        r#"{"choices":[{"delta":{"content":"x"}}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
        r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
    ])
}

/// chat 最小成功样例：单条文本 delta + `finish_reason=stop`。
pub fn chat_minimal_ok_body() -> String {
    sse_body(&[
        r#"{"choices":[{"delta":{"content":"ok"}}]}"#,
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    ])
}

/// chat 仅收尾样例：无内容 delta，只有 `finish_reason=stop`（路由接线断言用）。
pub fn chat_finish_only_body() -> String {
    sse_body(&[r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#])
}

/// Responses 最小完成样例（无 usage，不带 `[DONE]`，与 Responses wire 一致）。
pub fn responses_completed_body() -> String {
    sse_frames(&[r#"{"type":"response.completed","response":{"status":"completed"}}"#])
}

/// Responses 文本流样例：created + N 段文本 delta + completed（含 usage）。
pub fn responses_text_stream_body(
    response_id: &str,
    deltas: &[&str],
    usage: (u64, u64),
) -> String {
    let mut events = vec![format!(
        r#"{{"type":"response.created","response":{{"id":"{response_id}"}}}}"#
    )];
    for delta in deltas {
        events.push(format!(
            r#"{{"type":"response.output_text.delta","delta":"{delta}"}}"#
        ));
    }
    let (input_tokens, output_tokens) = usage;
    events.push(format!(
        r#"{{"type":"response.completed","response":{{"id":"{response_id}","status":"completed","usage":{{"input_tokens":{input_tokens},"output_tokens":{output_tokens}}}}}}}"#
    ));
    sse_frames(&events.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Provider 流断言：与 contract 契约测试同强度，供 contract.rs 与
/// api_key_channels.rs 的合并用例共用。
pub mod contract {
    use super::{ProviderError, ProviderErrorKind, ProviderStreamEvent, StopReason};

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
            events.iter().any(
                |e| matches!(e, ProviderStreamEvent::ToolCallCompleted { id: cid } if cid == &id)
            ),
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
}
