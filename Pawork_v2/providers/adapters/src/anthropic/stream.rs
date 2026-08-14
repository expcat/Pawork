//! Anthropic SSE 事件 → canonical ProviderStreamEvent 的映射（S2 基线）。
//!
//! 只处理 message_start / text / tool_use start-delta-stop / message_delta
//! usage / message_stop / ping / error。thinking / signature / server_tool_use /
//! citations 忽略。

use std::collections::HashMap;

use pawork_api::{ProviderError, ProviderErrorKind, ProviderStreamEvent};
use pawork_domain::{TokenUsage, ToolCallId};
use serde_json::Value;

use crate::usage::{map_stop_reason, normalize_usage};

/// 解析单条 SSE data（一个 Anthropic 事件 JSON），返回应发射的 canonical 事件。
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
                if let Some(usage) = message.get("usage") {
                    let normalized = normalize_usage(usage);
                    if normalized != TokenUsage::default() {
                        state.input_tokens = normalized.input_tokens;
                        state.cache_read_tokens = normalized.cache_read_tokens;
                        state.cache_write_tokens = normalized.cache_write_tokens;
                        if normalized.output_tokens > 0 {
                            state.output_tokens = normalized.output_tokens;
                        }
                        events.push(ProviderStreamEvent::UsageUpdated(normalized));
                    }
                }
            }
        }
        "content_block_start" => {
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            if let Some(block) = value.get("content_block") {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
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
                    events.push(ProviderStreamEvent::ToolCallStarted {
                        id: ToolCallId::new(id),
                        name,
                    });
                }
            }
        }
        "content_block_delta" => {
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            if let Some(delta) = value.get("delta") {
                match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                events.push(ProviderStreamEvent::TextDelta(text.to_string()));
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(|t| t.as_str()) {
                            if let Some(id) = state.tool_ids.get(&index) {
                                events.push(ProviderStreamEvent::ToolCallArgumentsDelta {
                                    id: ToolCallId::new(id.clone()),
                                    json: partial.to_string(),
                                });
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
            if let Some(usage) = value.get("usage") {
                let normalized = normalize_usage(usage);
                if normalized.output_tokens > 0 {
                    state.output_tokens = normalized.output_tokens;
                }
                if normalized.cache_read_tokens > 0 {
                    state.cache_read_tokens = normalized.cache_read_tokens;
                }
                if normalized.cache_write_tokens > 0 {
                    state.cache_write_tokens = normalized.cache_write_tokens;
                }
                events.push(ProviderStreamEvent::UsageUpdated(TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_read_tokens: state.cache_read_tokens,
                    cache_write_tokens: state.cache_write_tokens,
                }));
            }
        }
        "message_stop" => {
            let has_tool_calls = !state.tool_ids.is_empty();
            let stop = map_stop_reason(state.stop_reason.as_deref(), has_tool_calls);
            events.push(ProviderStreamEvent::ResponseCompleted(stop));
            state.finished = true;
        }
        "ping" => {}
        "error" => {
            events.push(ProviderStreamEvent::Error(map_error_event(&value)));
        }
        _ => {}
    }

    events
}

fn map_error_event(value: &Value) -> ProviderError {
    let error = value.get("error");
    let message = error
        .and_then(|err| err.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("anthropic stream error");
    let kind = match error
        .and_then(|err| err.get("type"))
        .and_then(Value::as_str)
    {
        Some("authentication_error") => ProviderErrorKind::Authentication,
        Some("permission_error") => ProviderErrorKind::Authorization,
        Some("rate_limit_error") | Some("overloaded_error") => ProviderErrorKind::RateLimited,
        Some("invalid_request_error") => ProviderErrorKind::InvalidRequest,
        Some("not_found_error") => ProviderErrorKind::ModelNotFound,
        _ => ProviderErrorKind::Unknown,
    };
    ProviderError::new(kind, message)
}

/// 流解析期间需在事件间保持的状态。
#[derive(Default)]
pub struct AnthropicStreamState {
    pub tool_ids: HashMap<usize, String>,
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
    use pawork_domain::StopReason;

    #[test]
    fn message_start_emits_response_started_and_input_usage() {
        let data = r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":12,"output_tokens":1}}}"#;
        let mut state = AnthropicStreamState::default();
        let events = event_to_events(data, &mut state);
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ResponseStarted { response_id } if response_id.as_deref() == Some("msg_1")
        )));
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
    fn tool_use_blocks_emit_full_lifecycle() {
        let mut state = AnthropicStreamState::default();
        let start = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read"}}"#;
        let delta = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"p\":"}}"#;
        let stop = r#"{"type":"content_block_stop","index":0}"#;

        let started = event_to_events(start, &mut state);
        assert!(started.iter().any(|ev| matches!(ev,
            ProviderStreamEvent::ToolCallStarted { name, .. } if name == "read")));
        let args = event_to_events(delta, &mut state);
        assert!(args.iter().any(|ev| matches!(ev,
            ProviderStreamEvent::ToolCallArgumentsDelta { id, json }
                if id.as_str()=="toolu_1" && json == r#"{"p":"#)));
        let completed = event_to_events(stop, &mut state);
        assert!(completed.iter().any(|ev| matches!(ev,
            ProviderStreamEvent::ToolCallCompleted { id } if id.as_str()=="toolu_1")));
    }

    #[test]
    fn input_json_delta_is_not_parsed() {
        let mut state = AnthropicStreamState::default();
        let _ = event_to_events(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read"}}"#,
            &mut state,
        );
        let events = event_to_events(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"p\":"}}"#,
            &mut state,
        );
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::ToolCallArgumentsDelta { json, .. } if json == r#"{"p":"#
        ));
    }

    #[test]
    fn message_stop_completes_with_tool_use_priority() {
        let mut state = AnthropicStreamState::default();
        let _ = event_to_events(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"r"}}"#,
            &mut state,
        );
        let _ = event_to_events(r#"{"type":"content_block_stop","index":0}"#, &mut state);
        let _ = event_to_events(
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            &mut state,
        );
        let events = event_to_events(r#"{"type":"message_stop"}"#, &mut state);
        assert!(matches!(
            events.last(),
            Some(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse))
        ));
    }

    #[test]
    fn message_stop_without_reason_is_completed() {
        let mut state = AnthropicStreamState::default();
        let events = event_to_events(r#"{"type":"message_stop"}"#, &mut state);
        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::ResponseCompleted(StopReason::Completed)]
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
            ProviderStreamEvent::UsageUpdated(u)
                if u.cache_read_tokens == 80 && u.input_tokens == 100 && u.output_tokens == 5)));
    }

    #[test]
    fn ping_and_unknown_types_are_ignored() {
        let mut state = AnthropicStreamState::default();
        assert!(event_to_events(r#"{"type":"ping"}"#, &mut state).is_empty());
        assert!(event_to_events(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
            &mut state
        )
        .is_empty());
        assert!(event_to_events(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            &mut state
        )
        .is_empty());
        assert!(event_to_events(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srv","name":"web_search"}}"#,
            &mut state
        )
        .is_empty());
    }

    #[test]
    fn error_event_maps_to_provider_error() {
        let mut state = AnthropicStreamState::default();
        let events = event_to_events(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
            &mut state,
        );
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::Error(err)
                if err.kind == ProviderErrorKind::RateLimited && err.message == "busy"
        ));
    }
}
