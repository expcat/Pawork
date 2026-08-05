//! OpenAI 流式 chunk → canonical ProviderStreamEvent 的映射。
//!
//! 每个 `data: {json}` 对应一个 OpenAI Chat Completions 流式 chunk。本模块把
//! delta（文本 / tool_calls）、usage 与 finish_reason 映射为 canonical 事件。

use agent_domain::{TokenUsage, ToolCallId};
use provider_api::ProviderStreamEvent;
use provider_runtime::usage::{map_stop_reason, normalize_usage};
use serde_json::Value;
use std::collections::HashMap;

/// 解析单条 SSE data 行的 JSON，返回该 chunk 应发射的事件。
///
/// `pending` 记录 index→id 映射，后续 chunk 仅带 index + arguments 片段时能补齐 id。
pub fn chunk_to_events(data: &str, pending: &mut ChunkState) -> Vec<ProviderStreamEvent> {
    let value: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut events = Vec::new();

    // usage（部分 provider 在最后 chunk 带完整 usage）
    if value.get("usage").is_some() {
        let usage = normalize_usage(&value);
        if usage != TokenUsage::default() {
            events.push(ProviderStreamEvent::UsageUpdated(usage));
        }
    }

    let choices = match value.get("choices").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return events,
    };
    let Some(choice) = choices.first() else {
        return events;
    };
    let delta = choice.get("delta");

    // text delta
    if let Some(content) = delta
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())
    {
        if !content.is_empty() {
            events.push(ProviderStreamEvent::TextDelta(content.to_string()));
        }
    }

    // tool_calls delta（OpenAI 用 index 标识并行 tool call）
    if let Some(tool_calls) = delta
        .and_then(|d| d.get("tool_calls"))
        .and_then(|t| t.as_array())
    {
        for tc in tool_calls {
            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            let function = tc.get("function");
            let id = tc.get("id").and_then(|i| i.as_str()).map(|s| s.to_string());
            let name = function
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|s| s.to_string());
            let args = function
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or("");

            let is_new = !pending.tool_ids.contains_key(&index);
            if is_new {
                let call_id = id.clone().unwrap_or_else(|| format!("call-{index}"));
                let call_name = name.clone().unwrap_or_default();
                pending.tool_ids.insert(index, call_id.clone());
                events.push(ProviderStreamEvent::ToolCallStarted {
                    id: ToolCallId::new(call_id.clone()),
                    name: call_name,
                });
                if !args.is_empty() {
                    events.push(ProviderStreamEvent::ToolCallArgumentsDelta {
                        id: ToolCallId::new(call_id),
                        json: args.to_string(),
                    });
                }
            } else if let Some(existing_id) = pending.tool_ids.get(&index).cloned() {
                events.push(ProviderStreamEvent::ToolCallArgumentsDelta {
                    id: ToolCallId::new(existing_id),
                    json: args.to_string(),
                });
            }
        }
    }

    // finish_reason
    if let Some(finish) = choice.get("finish_reason").and_then(|f| f.as_str()) {
        // 发射已开始 tool call 的 Completed 事件
        let ids: Vec<String> = pending.tool_ids.values().cloned().collect();
        for id in ids {
            events.push(ProviderStreamEvent::ToolCallCompleted {
                id: ToolCallId::new(id),
            });
        }
        pending.has_tool_calls = !pending.tool_ids.is_empty();
        let stop = map_stop_reason(Some(finish), pending.has_tool_calls);
        events.push(ProviderStreamEvent::ResponseCompleted(stop));
    }

    events
}

/// 解析流期间需在 chunk 间保持的状态（tool index→id 映射等）。
#[derive(Default)]
pub struct ChunkState {
    pub tool_ids: HashMap<usize, String>,
    pub has_tool_calls: bool,
}

/// 判断某 data 是否为流的结束标记 `[DONE]`。
pub fn is_done(data: &str) -> bool {
    data.trim() == "[DONE]"
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::StopReason;
    use provider_api::ProviderStreamEvent;

    #[test]
    fn text_delta_maps() {
        let data = r#"{"choices":[{"delta":{"content":"Hi"}}]}"#;
        let mut state = ChunkState::default();
        let events = chunk_to_events(data, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::TextDelta(t) if t == "Hi"
        ));
    }

    #[test]
    fn finish_reason_completes_with_tool_calls_priority() {
        let data = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        let mut state = ChunkState::default();
        let events = chunk_to_events(data, &mut state);
        // 无 tool call 开始过，直接 ResponseCompleted(ToolUse)
        assert!(matches!(
            events.last(),
            Some(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse))
        ));
    }

    #[test]
    fn parallel_tool_calls_across_chunks() {
        let mut state = ChunkState::default();
        let first = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-a","function":{"name":"read","arguments":"{\"path\":"}}]}}]}"#;
        let second = r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call-b","function":{"name":"write","arguments":"{\"x\":1}"}}]}}]}"#;
        let third = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]}}]}"#;
        let done = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;

        let e1 = chunk_to_events(first, &mut state);
        assert!(e1.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ToolCallStarted { name, .. } if name == "read"
        )));
        chunk_to_events(second, &mut state);
        let e3 = chunk_to_events(third, &mut state);
        // 第三段补 call-a 的 arguments 片段
        assert!(e3
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::ToolCallArgumentsDelta { id, .. } if id.as_str() == "call-a")));

        let e4 = chunk_to_events(done, &mut state);
        // 收尾：两个 Completed + ResponseCompleted
        assert!(
            e4.iter()
                .filter(|e| matches!(e, ProviderStreamEvent::ToolCallCompleted { .. }))
                .count()
                >= 2
        );
        assert!(matches!(
            e4.last(),
            Some(ProviderStreamEvent::ResponseCompleted(_))
        ));
    }

    #[test]
    fn usage_emitted_when_present() {
        let data = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let mut state = ChunkState::default();
        let events = chunk_to_events(data, &mut state);
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::UsageUpdated(u) if u.input_tokens == 10)));
    }

    #[test]
    fn is_done_detects_marker() {
        assert!(is_done("[DONE]"));
        assert!(is_done(" [DONE] "));
        assert!(!is_done("data"));
    }
}
