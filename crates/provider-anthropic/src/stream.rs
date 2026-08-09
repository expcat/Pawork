//! Anthropic SSE 事件 → canonical ProviderStreamEvent 的映射。
//!
//! Anthropic 流是「事件类型」驱动的（`type` 字段）：message_start、
//! content_block_start、content_block_delta、content_block_stop、message_delta、
//! message_stop、ping。本模块把它们映射为 canonical 事件。

use agent_domain::{TokenUsage, ToolCallId};
use provider_api::ProviderStreamEvent;
use provider_runtime::usage::map_stop_reason;
use serde_json::Value;
use std::collections::HashMap;

/// 解析单条 SSE data（一个 Anthropic 事件 JSON），返回应发射的 canonical 事件。
///
/// `state` 在事件间保持 index→tool_id 映射、是否已结束等状态。
pub fn event_to_events(data: &str, state: &mut AnthropicStreamState) -> Vec<ProviderStreamEvent> {
    let value: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let mut events = Vec::new();

    match event_type {
        "message_start" => {
            if let Some(message) = value.get("message") {
                let response_id = message.get("id").and_then(|i| i.as_str()).map(String::from);
                events.push(ProviderStreamEvent::ResponseStarted { response_id });
                // usage（input tokens 在 message_start，output 在 message_delta）
                if let Some(usage) = message.get("usage") {
                    let normalized = normalize_anthropic_usage(usage);
                    if normalized != TokenUsage::default() {
                        state.input_tokens = normalized.input_tokens;
                        events.push(ProviderStreamEvent::UsageUpdated(normalized));
                    }
                }
            }
        }
        "content_block_start" => {
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            if let Some(block) = value.get("content_block") {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "tool_use" => {
                        let id = block
                            .get("id")
                            .and_then(|i| i.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| format!("call-{index}"));
                        let name = block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(String::from)
                            .unwrap_or_default();
                        state.tool_ids.insert(index, id.clone());
                        state.tool_order.push(index);
                        events.push(ProviderStreamEvent::ToolCallStarted {
                            id: ToolCallId::new(id),
                            name,
                        });
                    }
                    "text" | "thinking" => {
                        // content_block_start 不带内容，内容在后续 delta 中
                    }
                    _ => {}
                }
            }
        }
        "content_block_delta" => {
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            if let Some(delta) = value.get("delta") {
                let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match delta_type {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                events.push(ProviderStreamEvent::TextDelta(text.to_string()));
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(text) = delta.get("thinking").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                events.push(ProviderStreamEvent::ThinkingDelta(text.to_string()));
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(|t| t.as_str()) {
                            if !partial.is_empty() {
                                if let Some(id) = state.tool_ids.get(&index) {
                                    events.push(ProviderStreamEvent::ToolCallArgumentsDelta {
                                        id: ToolCallId::new(id.clone()),
                                        json: partial.to_string(),
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            if let Some(id) = state.tool_ids.get(&index).cloned() {
                events.push(ProviderStreamEvent::ToolCallCompleted {
                    id: ToolCallId::new(id),
                });
            }
        }
        "message_delta" => {
            if let Some(delta) = value.get("delta") {
                if let Some(stop) = delta.get("stop_reason").and_then(|s| s.as_str()) {
                    state.stop_reason = Some(stop.to_string());
                }
            }
            // output tokens 在 message_delta 的 usage 中
            if let Some(usage) = value.get("usage") {
                let normalized = normalize_anthropic_usage(usage);
                if normalized.output_tokens > 0 {
                    state.output_tokens = normalized.output_tokens;
                }
                if normalized.cache_read_tokens > 0 {
                    state.cache_read_tokens = normalized.cache_read_tokens;
                }
                if normalized.cache_write_tokens > 0 {
                    state.cache_write_tokens = normalized.cache_write_tokens;
                }
                // 发射合并后的 usage（input 来自 message_start，output 来自 message_delta）
                let merged = TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_read_tokens: state.cache_read_tokens,
                    cache_write_tokens: state.cache_write_tokens,
                };
                events.push(ProviderStreamEvent::UsageUpdated(merged));
            }
        }
        "message_stop" => {
            let has_tool_calls = !state.tool_ids.is_empty();
            let stop = map_stop_reason(state.stop_reason.as_deref(), has_tool_calls);
            events.push(ProviderStreamEvent::ResponseCompleted(stop));
            state.finished = true;
        }
        "ping" => {}
        _ => {}
    }

    events
}

/// Anthropic usage JSON（input_tokens / output_tokens / cache_read_input_tokens /
/// cache_creation_input_tokens）→ TokenUsage。
fn normalize_anthropic_usage(usage: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read_tokens: usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_write_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    }
}

/// 流解析期间需在事件间保持的状态。
#[derive(Default)]
pub struct AnthropicStreamState {
    pub tool_ids: HashMap<usize, String>,
    pub tool_order: Vec<usize>,
    pub stop_reason: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub finished: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_api::ProviderStreamEvent;

    #[test]
    fn message_start_emits_response_started_and_input_usage() {
        let data = r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":12,"output_tokens":1}}}"#;
        let mut state = AnthropicStreamState::default();
        let events = event_to_events(data, &mut state);
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::ResponseStarted { response_id } if response_id.as_deref() == Some("msg_1"))));
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::UsageUpdated(u) if u.input_tokens == 12)));
    }

    #[test]
    fn text_delta_maps() {
        let data =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#;
        let mut state = AnthropicStreamState::default();
        let events = event_to_events(data, &mut state);
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::TextDelta(t) if t == "Hi"
        )));
    }

    #[test]
    fn thinking_delta_maps() {
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#;
        let mut state = AnthropicStreamState::default();
        let events = event_to_events(data, &mut state);
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ThinkingDelta(t) if t == "hmm"
        )));
    }

    #[test]
    fn tool_use_blocks_emit_full_lifecycle() {
        let mut state = AnthropicStreamState::default();
        let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read"}}"#;
        let delta = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"p\":"}}"#;
        let delta2 = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}"#;
        let stop = r#"{"type":"content_block_stop","index":0}"#;

        let e = event_to_events(start, &mut state);
        assert!(e.iter().any(|ev| matches!(ev,
            ProviderStreamEvent::ToolCallStarted { name, .. } if name == "read")));
        let e1 = event_to_events(delta, &mut state);
        assert!(e1.iter().any(|ev| matches!(ev,
            ProviderStreamEvent::ToolCallArgumentsDelta { id, json } if id.as_str()=="toolu_1" && json == r#"{"p":"#)));
        event_to_events(delta2, &mut state);
        let e3 = event_to_events(stop, &mut state);
        assert!(e3.iter().any(|ev| matches!(ev,
            ProviderStreamEvent::ToolCallCompleted { id } if id.as_str()=="toolu_1")));
    }

    #[test]
    fn message_stop_completes_with_tool_use_priority() {
        let mut state = AnthropicStreamState::default();
        // 先注册一个 tool id，使 has_tool_calls=true
        let _ = event_to_events(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"r"}}"#,
            &mut state,
        );
        let _ = event_to_events(r#"{"type":"content_block_stop","index":0}"#, &mut state);
        let stop_data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#;
        let _ = event_to_events(stop_data, &mut state);
        let events = event_to_events(r#"{"type":"message_stop"}"#, &mut state);
        assert!(matches!(
            events.last(),
            Some(ProviderStreamEvent::ResponseCompleted(
                agent_domain::StopReason::ToolUse
            ))
        ));
    }

    #[test]
    fn message_stop_without_reason_is_completed() {
        let mut state = AnthropicStreamState::default();
        let events = event_to_events(r#"{"type":"message_stop"}"#, &mut state);

        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::ResponseCompleted(
                agent_domain::StopReason::Completed
            )]
        ));
        assert!(state.finished);
    }

    #[test]
    fn cache_tokens_reflected_in_usage() {
        let mut state = AnthropicStreamState::default();
        let _ = event_to_events(
            r#"{"type":"message_start","message":{"id":"m","usage":{"input_tokens":100}}}"#,
            &mut state,
        );
        let events = event_to_events(
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5,"cache_read_input_tokens":80}}"#,
            &mut state,
        );
        assert!(events.iter().any(|e| matches!(e,
            ProviderStreamEvent::UsageUpdated(u) if u.cache_read_tokens == 80 && u.input_tokens == 100 && u.output_tokens == 5)));
    }
}
