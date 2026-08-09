//! Gemini 流式 chunk → canonical [`ProviderStreamEvent`] 的映射。
//!
//! 每个 `data: {json}` 对应一个 Gemini `streamGenerateContent` 流式响应对象，
//! 结构为 `{ candidates:[{ content:{ role, parts }, finishReason }], usageMetadata }`。

use agent_domain::{TokenUsage, ToolCallId};
use provider_api::ProviderStreamEvent;
use provider_runtime::usage::{map_stop_reason, normalize_usage};
use serde_json::{json, Value};

/// 解析单条 SSE data 行的 JSON，返回该 chunk 应发射的事件。
///
/// `state` 在 chunk 间保持工具调用顺序及原始名称，用于在
/// `finishReason` 到来时判定 stop reason，并为后续 `functionResponse`
/// 对齐保留稳定元数据。
pub fn chunk_to_events(data: &str, state: &mut ChunkState) -> Vec<ProviderStreamEvent> {
    let value: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut events = Vec::new();

    // usageMetadata（Gemini 字段命名与 OpenAI/Anthropic 不同，需归一后喂 normalize_usage）。
    if let Some(usage) = gemini_usage(&value) {
        if usage != TokenUsage::default() {
            events.push(ProviderStreamEvent::UsageUpdated(usage));
        }
    }

    let candidates = match value.get("candidates").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return events,
    };
    let Some(candidate) = candidates.first() else {
        return events;
    };

    // parts
    if let Some(parts) = candidate
        .get("content")
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
    {
        for part in parts {
            let is_thought = part
                .get("thought")
                .and_then(|t| t.as_bool())
                .unwrap_or(false);

            // functionCall（兼容 snake_case function_call）
            if let Some(fc) = part
                .get("functionCall")
                .or_else(|| part.get("function_call"))
            {
                // thinking 阶段的 function call 不作为工具调用对外暴露。
                if is_thought {
                    continue;
                }
                let name = fc
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = fc
                    .get("args")
                    .or_else(|| fc.get("arguments"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let ordinal = state.tool_calls.len();
                // Gemini 流式响应不提供 call id；以顺序生成可重放的稳定 id，
                // 并把 id/name/ordinal 一起保留在 provider metadata 中。
                let id = format!("gemini-call-{ordinal}");
                state.tool_calls.push(GeminiToolCallRef {
                    id: id.clone(),
                    name: name.clone(),
                    ordinal,
                });

                let args_json = if args.is_null() {
                    "{}".to_string()
                } else {
                    args.to_string()
                };
                events.push(ProviderStreamEvent::ToolCallStarted {
                    id: ToolCallId::new(id.clone()),
                    name,
                });
                events.push(ProviderStreamEvent::ToolCallArgumentsDelta {
                    id: ToolCallId::new(id.clone()),
                    json: args_json,
                });
                events.push(ProviderStreamEvent::ToolCallCompleted {
                    id: ToolCallId::new(id),
                });
                continue;
            }

            // text / thought text
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    if is_thought {
                        events.push(ProviderStreamEvent::ThinkingDelta(text.to_string()));
                    } else {
                        events.push(ProviderStreamEvent::TextDelta(text.to_string()));
                    }
                }
            }
        }
    }

    // finishReason（兼容 finish_reason）
    if let Some(finish) = candidate
        .get("finishReason")
        .or_else(|| candidate.get("finish_reason"))
        .and_then(|f| f.as_str())
    {
        let tool_calls: Vec<Value> = state
            .tool_calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "name": call.name,
                    "ordinal": call.ordinal,
                })
            })
            .collect();
        events.push(ProviderStreamEvent::ProviderMetadata(json!({
            "finishReason": finish,
            "toolCalls": tool_calls,
        })));
        let stop = map_stop_reason(Some(finish), !state.tool_calls.is_empty());
        events.push(ProviderStreamEvent::ResponseCompleted(stop));
    }

    events
}

/// 解析流期间需在 chunk 间保持的状态。
#[derive(Default)]
pub struct ChunkState {
    /// 已发射的工具调用，按 Gemini parts 出现顺序保留。
    tool_calls: Vec<GeminiToolCallRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GeminiToolCallRef {
    id: String,
    name: String,
    ordinal: usize,
}

/// 把 Gemini `usageMetadata` 归一为 [`TokenUsage`]。
///
/// `promptTokenCount`→input、`candidatesTokenCount`→output、
/// `cachedContentTokenCount`→cache_read（P6-7）。其余字段（如 thoughtsTokenCount）
/// 当前不计入 usage。
fn gemini_usage(value: &Value) -> Option<TokenUsage> {
    let um = value.get("usageMetadata").or_else(|| value.get("usage"))?;
    let mut view = serde_json::Map::new();
    if let Some(p) = um
        .get("promptTokenCount")
        .or_else(|| um.get("prompt_token_count"))
    {
        view.insert("input_tokens".into(), p.clone());
    }
    if let Some(c) = um
        .get("candidatesTokenCount")
        .or_else(|| um.get("candidates_token_count"))
        .or_else(|| um.get("output_tokens"))
    {
        view.insert("output_tokens".into(), c.clone());
    }
    if let Some(cc) = um
        .get("cachedContentTokenCount")
        .or_else(|| um.get("cached_content_token_count"))
    {
        view.insert("cache_read_tokens".into(), cc.clone());
    }
    Some(normalize_usage(&Value::Object(view)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::StopReason;

    #[test]
    fn text_delta_maps() {
        let mut state = ChunkState::default();
        let events = chunk_to_events(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hi"}]}}]}"#,
            &mut state,
        );
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::TextDelta(t) if t == "Hi"
        )));
    }

    #[test]
    fn thought_part_maps_to_thinking_delta() {
        let mut state = ChunkState::default();
        let events = chunk_to_events(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"thought":true,"text":"reasoning"}]}}]}"#,
            &mut state,
        );
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ThinkingDelta(t) if t == "reasoning"
        )));
    }

    #[test]
    fn function_call_emits_full_tool_call() {
        let mut state = ChunkState::default();
        let events = chunk_to_events(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read","args":{"path":"a"}}}]}}]}"#,
            &mut state,
        );
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ToolCallStarted { name, .. } if name == "read"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ToolCallArgumentsDelta { json, .. } if json.contains("path")
        )));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::ToolCallCompleted { .. })));
    }

    #[test]
    fn finish_reason_with_tool_call_is_tool_use() {
        let mut state = ChunkState::default();
        chunk_to_events(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"x","args":{}}}]}}]}"#,
            &mut state,
        );
        let events = chunk_to_events(
            r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP"}]}"#,
            &mut state,
        );
        assert!(matches!(
            events.last(),
            Some(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse))
        ));
        let metadata = events
            .iter()
            .find_map(|event| match event {
                ProviderStreamEvent::ProviderMetadata(value) => Some(value),
                _ => None,
            })
            .expect("有 provider metadata");
        assert_eq!(metadata["toolCalls"][0]["id"], "gemini-call-0");
        assert_eq!(metadata["toolCalls"][0]["name"], "x");
        assert_eq!(metadata["toolCalls"][0]["ordinal"], 0);
    }

    #[test]
    fn usage_metadata_normalizes() {
        let mut state = ChunkState::default();
        let events = chunk_to_events(
            r#"{"candidates":[],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"cachedContentTokenCount":3}}"#,
            &mut state,
        );
        let usage = events
            .iter()
            .find_map(|e| match e {
                ProviderStreamEvent::UsageUpdated(u) => Some(u.clone()),
                _ => None,
            })
            .expect("有 UsageUpdated");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_tokens, 3);
    }
}
