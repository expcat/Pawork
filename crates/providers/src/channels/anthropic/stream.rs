//! Anthropic SSE 事件 → canonical ProviderStreamEvent 的映射（S2 基线）。
//!
//! 处理 message_start / text / tool_use / thinking_delta / server_tool_use /
//! citations / message_delta usage / message_stop / ping / error。
//! signature 明文不进入 [`ProviderStreamEvent`]；由 adapter 在 process_chunk
//! 中经 ReasoningProtector 换成 ref-only [`ReasoningItem`]。

use std::collections::{HashMap, HashSet};

use pawork_domain::{Citation, ServerToolEvent, ServerToolMappingError, TokenUsage, ToolCallId};
use pawork_domain::{ProviderError, ProviderErrorKind, ProviderStreamEvent};
use serde_json::Value;

use crate::usage::{map_stop_reason, normalize_usage};

/// 解析单条 SSE data 时可能得到的流产物。
#[derive(Debug)]
pub enum StreamOutput {
    Event(ProviderStreamEvent),
    /// thinking / redacted_thinking 完成，待 adapter 经 protector 换成 ReasoningItem。
    PendingSignature {
        id: String,
        summary: Option<String>,
        payload: Vec<u8>,
        redacted: bool,
    },
    /// thinking 块缺少官方 continuation 所需的不透明字段。
    ReasoningError(String),
    MappingError(ServerToolMappingError),
}

/// 解析单条 SSE data（一个 Anthropic 事件 JSON），返回应发射的 canonical 事件。
pub fn event_to_events(data: &str, state: &mut AnthropicStreamState) -> Vec<ProviderStreamEvent> {
    parse_event(data, state)
        .into_iter()
        .filter_map(|output| match output {
            StreamOutput::Event(event) => Some(event),
            StreamOutput::PendingSignature { .. }
            | StreamOutput::ReasoningError(_)
            | StreamOutput::MappingError(_) => None,
        })
        .collect()
}

/// 完整解析入口：thinking signature 与 server-tool 映射失败单独返回。
pub fn parse_event(data: &str, state: &mut AnthropicStreamState) -> Vec<StreamOutput> {
    let value: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let mut outputs = Vec::new();

    match event_type {
        "message_start" => {
            if let Some(message) = value.get("message") {
                let response_id = message.get("id").and_then(|i| i.as_str()).map(String::from);
                outputs.push(StreamOutput::Event(ProviderStreamEvent::ResponseStarted {
                    response_id,
                }));
                if let Some(usage) = message.get("usage") {
                    let normalized = normalize_usage(usage);
                    if normalized != TokenUsage::default() {
                        state.input_tokens = normalized.input_tokens;
                        state.cache_read_tokens = normalized.cache_read_tokens;
                        state.cache_write_tokens = normalized.cache_write_tokens;
                        if normalized.output_tokens > 0 {
                            state.output_tokens = normalized.output_tokens;
                        }
                        outputs.push(StreamOutput::Event(ProviderStreamEvent::UsageUpdated(
                            normalized,
                        )));
                    }
                }
            }
        }
        "content_block_start" => {
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            if let Some(block) = value.get("content_block") {
                match block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
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
                        outputs.push(StreamOutput::Event(ProviderStreamEvent::ToolCallStarted {
                            id: ToolCallId::new(id),
                            name,
                        }));
                    }
                    "thinking" | "redacted_thinking" => {
                        let id = block
                            .get("id")
                            .and_then(|i| i.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| format!("thinking-{index}"));
                        let redacted =
                            block.get("type").and_then(|t| t.as_str()) == Some("redacted_thinking");
                        let signature = block
                            .get("signature")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        let data = block
                            .get("data")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        let thinking = block
                            .get("thinking")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string();
                        state.thinking.insert(
                            index,
                            ThinkingBlockState {
                                id,
                                redacted,
                                text: thinking,
                                signature,
                                data,
                            },
                        );
                    }
                    "server_tool_use" => {
                        let id = block
                            .get("id")
                            .and_then(|i| i.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| format!("srv-{index}"));
                        let name = block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(String::from)
                            .unwrap_or_default();
                        if name.is_empty() {
                            outputs.push(StreamOutput::MappingError(
                                ServerToolMappingError::unsupported("server_tool_use missing name"),
                            ));
                        } else {
                            state.server_tool_ids.insert(index, id.clone());
                            state.last_server_tool_id = Some(id.clone());
                            outputs.push(StreamOutput::Event(ProviderStreamEvent::ServerTool(
                                ServerToolEvent::Started {
                                    tool_call_id: ToolCallId::new(id),
                                    name,
                                    arguments: block.get("input").cloned(),
                                },
                            )));
                        }
                    }
                    "web_search_tool_result" | "code_execution_tool_result" | "mcp_tool_result" => {
                        let id = block
                            .get("tool_use_id")
                            .and_then(|i| i.as_str())
                            .or_else(|| block.get("id").and_then(|i| i.as_str()))
                            .map(String::from)
                            .unwrap_or_else(|| format!("srv-{index}"));
                        state.server_tool_ids.insert(index, id.clone());
                        state.last_server_tool_id = Some(id);
                    }
                    _ => {}
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
                                outputs.push(StreamOutput::Event(ProviderStreamEvent::TextDelta(
                                    text.to_string(),
                                )));
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(|t| t.as_str()) {
                            if let Some(id) = state.tool_ids.get(&index) {
                                outputs.push(StreamOutput::Event(
                                    ProviderStreamEvent::ToolCallArgumentsDelta {
                                        id: ToolCallId::new(id.clone()),
                                        json: partial.to_string(),
                                    },
                                ));
                            } else if let Some(id) = state.server_tool_ids.get(&index) {
                                outputs.push(StreamOutput::Event(ProviderStreamEvent::ServerTool(
                                    ServerToolEvent::ArgumentsDelta {
                                        tool_call_id: ToolCallId::new(id.clone()),
                                        json_delta: partial.to_string(),
                                    },
                                )));
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(text) = delta.get("thinking").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                if let Some(block) = state.thinking.get_mut(&index) {
                                    block.text.push_str(text);
                                }
                                outputs.push(StreamOutput::Event(
                                    ProviderStreamEvent::ThinkingDelta(text.to_string()),
                                ));
                            }
                        }
                    }
                    "signature_delta" => {
                        if let Some(signature) = delta.get("signature").and_then(|t| t.as_str()) {
                            if let Some(block) = state.thinking.get_mut(&index) {
                                block.signature.push_str(signature);
                            }
                        }
                    }
                    "citations_delta" | "citation" => {
                        let id = state
                            .server_tool_ids
                            .get(&index)
                            .cloned()
                            .or_else(|| state.last_server_tool_id.clone());
                        if let Some(id) = id {
                            outputs.push(StreamOutput::Event(ProviderStreamEvent::ServerTool(
                                ServerToolEvent::CitationAdded {
                                    tool_call_id: ToolCallId::new(id),
                                    citation: map_citation(
                                        delta.get("citation").or_else(|| delta.get("citations")),
                                    ),
                                },
                            )));
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            if let Some(id) = state.tool_ids.get(&index).cloned() {
                outputs.push(StreamOutput::Event(
                    ProviderStreamEvent::ToolCallCompleted {
                        id: ToolCallId::new(id),
                    },
                ));
            }
            if let Some(id) = state.server_tool_ids.get(&index).cloned() {
                if state.completed_server_tools.insert(id.clone()) {
                    outputs.push(StreamOutput::Event(ProviderStreamEvent::ServerTool(
                        ServerToolEvent::Completed {
                            tool_call_id: ToolCallId::new(id),
                            summary: None,
                            artifacts: Vec::new(),
                        },
                    )));
                }
            }
            if let Some(block) = state.thinking.remove(&index) {
                outputs.push(pending_signature_from(block));
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
                outputs.push(StreamOutput::Event(ProviderStreamEvent::UsageUpdated(
                    TokenUsage {
                        input_tokens: state.input_tokens,
                        output_tokens: state.output_tokens,
                        cache_read_tokens: state.cache_read_tokens,
                        cache_write_tokens: state.cache_write_tokens,
                    },
                )));
            }
        }
        "message_stop" => {
            let has_tool_calls = !state.tool_ids.is_empty();
            let stop = map_stop_reason(state.stop_reason.as_deref(), has_tool_calls);
            outputs.push(StreamOutput::Event(ProviderStreamEvent::ResponseCompleted(
                stop,
            )));
            state.finished = true;
        }
        "ping" => {}
        "error" => {
            outputs.push(StreamOutput::Event(ProviderStreamEvent::Error(
                map_error_event(&value),
            )));
        }
        _ => {}
    }

    outputs
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

fn map_citation(value: Option<&Value>) -> Citation {
    let Some(value) = value else {
        return Citation::empty();
    };
    Citation {
        index: value.get("index").and_then(Value::as_u64),
        url: value.get("url").and_then(Value::as_str).map(str::to_string),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        snippet: value
            .get("snippet")
            .and_then(Value::as_str)
            .map(str::to_string),
        text: value
            .get("text")
            .or_else(|| value.get("cited_text"))
            .and_then(Value::as_str)
            .map(str::to_string),
        document_index: value.get("document_index").and_then(Value::as_u64),
        source_kind: pawork_domain::CitationSourceKind::Unknown,
    }
}

fn pending_signature_from(block: ThinkingBlockState) -> StreamOutput {
    let payload = if block.redacted {
        if block.data.is_empty() {
            return StreamOutput::ReasoningError(
                "Anthropic redacted_thinking block missing data".into(),
            );
        }
        serde_json::to_vec(&serde_json::json!({
            "type": "redacted_thinking",
            "data": block.data,
        }))
        .unwrap_or_default()
    } else {
        if block.signature.is_empty() {
            return StreamOutput::ReasoningError(
                "Anthropic thinking block missing signature".into(),
            );
        }
        serde_json::to_vec(&serde_json::json!({
            "type": "thinking",
            "thinking": block.text,
            "signature": block.signature,
        }))
        .unwrap_or_default()
    };
    StreamOutput::PendingSignature {
        id: block.id,
        summary: if block.text.is_empty() {
            None
        } else {
            Some(block.text)
        },
        payload,
        redacted: block.redacted,
    }
}

#[derive(Clone, Debug, Default)]
struct ThinkingBlockState {
    id: String,
    redacted: bool,
    text: String,
    signature: String,
    data: String,
}

/// 流解析期间需在事件间保持的状态。
#[derive(Default)]
pub struct AnthropicStreamState {
    pub tool_ids: HashMap<usize, String>,
    pub server_tool_ids: HashMap<usize, String>,
    thinking: HashMap<usize, ThinkingBlockState>,
    completed_server_tools: HashSet<String>,
    last_server_tool_id: Option<String>,
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
            [ProviderStreamEvent::ResponseCompleted(
                StopReason::Completed
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
            ProviderStreamEvent::UsageUpdated(u)
                if u.cache_read_tokens == 80 && u.input_tokens == 100 && u.output_tokens == 5)));
    }

    #[test]
    fn ping_is_ignored_and_thinking_server_tool_are_mapped() {
        let mut state = AnthropicStreamState::default();
        assert!(event_to_events(r#"{"type":"ping"}"#, &mut state).is_empty());

        let thinking_start = event_to_events(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
            &mut state,
        );
        assert!(thinking_start.is_empty());
        let thinking = event_to_events(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
            &mut state,
        );
        assert!(matches!(
            &thinking[0],
            ProviderStreamEvent::ThinkingDelta(text) if text == "hmm"
        ));
        let signature_delta = parse_event(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-secret"}}"#,
            &mut state,
        );
        assert!(signature_delta.is_empty());
        let stop = parse_event(r#"{"type":"content_block_stop","index":0}"#, &mut state);
        assert!(matches!(
            &stop[0],
            StreamOutput::PendingSignature { payload, .. }
                if String::from_utf8_lossy(payload).contains("sig-secret")
                    && String::from_utf8_lossy(payload).contains("hmm")
        ));
        let public = event_to_events(
            r#"{"type":"content_block_stop","index":0}"#,
            &mut AnthropicStreamState::default(),
        );
        assert!(public.is_empty());

        let mut state = AnthropicStreamState::default();
        let started = event_to_events(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"server_tool_use","id":"srv","name":"web_search"}}"#,
            &mut state,
        );
        assert!(matches!(
            &started[0],
            ProviderStreamEvent::ServerTool(ServerToolEvent::Started { name, .. }) if name == "web_search"
        ));
        let completed = event_to_events(r#"{"type":"content_block_stop","index":1}"#, &mut state);
        assert!(matches!(
            &completed[0],
            ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { tool_call_id, .. })
                if tool_call_id.as_str() == "srv"
        ));
        let cited = event_to_events(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"citations_delta","citation":{"url":"https://example.com"}}}"#,
            &mut state,
        );
        assert!(matches!(
            &cited[0],
            ProviderStreamEvent::ServerTool(ServerToolEvent::CitationAdded { citation, .. })
                if citation.url.as_deref() == Some("https://example.com")
                    && citation.source_kind == pawork_domain::CitationSourceKind::Unknown
        ));
    }

    #[test]
    fn redacted_thinking_round_trips_exact_wire_shape() {
        let mut state = AnthropicStreamState::default();
        assert!(parse_event(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"opaque-data"}}"#,
            &mut state,
        )
        .is_empty());
        let stopped = parse_event(r#"{"type":"content_block_stop","index":0}"#, &mut state);
        let StreamOutput::PendingSignature {
            payload, redacted, ..
        } = &stopped[0]
        else {
            panic!("expected protected redacted thinking payload");
        };
        assert!(*redacted);
        let value: Value = serde_json::from_slice(payload).expect("payload json");
        assert_eq!(
            value,
            serde_json::json!({"type":"redacted_thinking","data":"opaque-data"})
        );
        assert!(value.get("signature").is_none());
    }

    #[test]
    fn incomplete_thinking_blocks_fail_closed() {
        let mut state = AnthropicStreamState::default();
        let _ = parse_event(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"plan"}}"#,
            &mut state,
        );
        let stopped = parse_event(r#"{"type":"content_block_stop","index":0}"#, &mut state);
        assert!(matches!(
            stopped.as_slice(),
            [StreamOutput::ReasoningError(error)] if error.contains("missing signature")
        ));

        let mut state = AnthropicStreamState::default();
        let _ = parse_event(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking"}}"#,
            &mut state,
        );
        let stopped = parse_event(r#"{"type":"content_block_stop","index":0}"#, &mut state);
        assert!(matches!(
            stopped.as_slice(),
            [StreamOutput::ReasoningError(error)] if error.contains("missing data")
        ));
    }

    #[test]
    fn server_tool_use_and_result_emit_completed_once() {
        let mut state = AnthropicStreamState::default();
        let started = event_to_events(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srv","name":"web_search"}}"#,
            &mut state,
        );
        assert!(matches!(
            &started[0],
            ProviderStreamEvent::ServerTool(ServerToolEvent::Started { name, .. }) if name == "web_search"
        ));
        let first_stop = event_to_events(r#"{"type":"content_block_stop","index":0}"#, &mut state);
        assert_eq!(
            first_stop
                .iter()
                .filter(|event| matches!(
                    event,
                    ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { .. })
                ))
                .count(),
            1
        );
        let _ = event_to_events(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"srv"}}"#,
            &mut state,
        );
        let second_stop = event_to_events(r#"{"type":"content_block_stop","index":1}"#, &mut state);
        assert!(second_stop.iter().all(|event| {
            !matches!(
                event,
                ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { .. })
            )
        }));
        let cited = event_to_events(
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"citations_delta","citation":{"url":"https://example.com"}}}"#,
            &mut state,
        );
        assert!(matches!(
            &cited[0],
            ProviderStreamEvent::ServerTool(ServerToolEvent::CitationAdded { tool_call_id, citation })
                if tool_call_id.as_str() == "srv"
                    && citation.url.as_deref() == Some("https://example.com")
        ));
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
