//! OpenAI 流式 chunk → canonical ProviderStreamEvent 的映射。
//!
//! 每个 `data: {json}` 对应一个 OpenAI Chat Completions 流式 chunk。本模块把
//! delta（文本 / tool_calls）、usage 与 finish_reason 映射为 canonical 事件。

use crate::usage::{map_stop_reason, normalize_usage};
use pawork_domain::ProviderStreamEvent;
use pawork_domain::{ProviderError, ProviderErrorKind, TokenUsage, ToolCallId};
use serde_json::Value;
use std::collections::HashMap;

/// 流错误入口统一安全文案（R-02）：不带上游 message 原文（可能回显敏感
/// 文本），只保留白名单诊断字段（错误 type/code，限 ASCII 标识符字符与
/// 长度）——与 HTTP 层不把响应正文写入错误文案（net/retry.rs）同口径。
pub(crate) fn stream_error_message(context: &str, field: &str, value: Option<&str>) -> String {
    match value.filter(|value| is_safe_diagnostic_field(value)) {
        Some(value) => format!("{context} ({field}={value})"),
        None => context.to_string(),
    }
}

fn is_safe_diagnostic_field(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

/// 解析单条 SSE data 行的 JSON，返回该 chunk 应发射的事件。
///
/// `pending` 记录 index→id 映射，后续 chunk 仅带 index + arguments 片段时能补齐 id。
pub fn chunk_to_events(data: &str, pending: &mut ChunkState) -> Vec<ProviderStreamEvent> {
    let value: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => {
            // R-08：畸形 chunk 不得静默忽略——正文或工具参数可能已缺失，
            // 明确报 MalformedResponse，由驱动层终止流（不伪造成功）。
            return vec![ProviderStreamEvent::Error(ProviderError::new(
                ProviderErrorKind::MalformedResponse,
                "invalid chat completions SSE chunk JSON",
            ))];
        }
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

    // reasoning / thinking delta（OpenAI o-series：delta.reasoning_content / reasoning）
    if let Some(reasoning) = delta
        .and_then(|d| d.get("reasoning_content").or_else(|| d.get("reasoning")))
        .and_then(|c| c.as_str())
    {
        if !reasoning.is_empty() {
            events.push(ProviderStreamEvent::ThinkingDelta(reasoning.to_string()));
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
    use pawork_domain::ProviderStreamEvent;
    use pawork_domain::StopReason;

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

    /// R-08：畸形 chunk 不再静默吞掉，映射为 MalformedResponse 错误事件。
    #[test]
    fn malformed_chunk_yields_malformed_error() {
        let mut state = ChunkState::default();
        let events = chunk_to_events("not-json", &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::Error(err)
                if err.kind == pawork_domain::ProviderErrorKind::MalformedResponse
        ));
    }

    /// R-02：流错误文案只保留白名单诊断字段——安全字符的 code/type 保留，
    /// 含自由文本（可能回显 Secret）的值整体丢弃。
    #[test]
    fn stream_error_message_keeps_only_whitelisted_field() {
        assert_eq!(
            stream_error_message("anthropic stream error", "type", Some("overloaded_error")),
            "anthropic stream error (type=overloaded_error)"
        );
        assert_eq!(
            stream_error_message("Responses request failed", "code", None),
            "Responses request failed"
        );
        // 自由文本（含空格 / 中文）与超长值不作为诊断字段保留。
        assert_eq!(
            stream_error_message("ctx", "code", Some("key sk-FAKE-SECRET rejected")),
            "ctx"
        );
        assert_eq!(
            stream_error_message("ctx", "code", Some(&"a".repeat(65))),
            "ctx"
        );
    }

    #[test]
    fn is_done_detects_marker() {
        assert!(is_done("[DONE]"));
        assert!(is_done(" [DONE] "));
        assert!(!is_done("data"));
    }

    #[test]
    fn reasoning_delta_maps_to_thinking() {
        let mut state = ChunkState::default();
        let events = chunk_to_events(
            r#"{"choices":[{"delta":{"reasoning_content":"thinking hard"}}]}"#,
            &mut state,
        );
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ThinkingDelta(t) if t == "thinking hard"
        )));

        // 兼容部分兼容服务使用 `reasoning` 字段名
        let events2 = chunk_to_events(
            r#"{"choices":[{"delta":{"reasoning":"more"}}]}"#,
            &mut state,
        );
        assert!(events2.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ThinkingDelta(t) if t == "more"
        )));
    }
}
