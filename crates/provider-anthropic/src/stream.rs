//! Anthropic SSE 事件 → canonical ProviderStreamEvent 的映射。
//!
//! Anthropic 流是「事件类型」驱动的（`type` 字段）：message_start、
//! content_block_start、content_block_delta、content_block_stop、message_delta、
//! message_stop、ping。本模块把它们映射为 canonical 事件。

use agent_domain::{ServerToolEvent, TokenUsage, ToolCallId};
use provider_api::ProviderStreamEvent;
use provider_runtime::usage::map_stop_reason;
use serde_json::Value;
use std::collections::HashMap;

use crate::modern::server_tool_result_block_to_events;
use crate::server_tool::{citation_block_to_citation, server_tool_use_to_started};

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
                    "server_tool_use" => {
                        if !state.server_tool_names.is_empty() {
                            let whitelist: Vec<&str> =
                                state.server_tool_names.iter().map(String::as_str).collect();
                            if let Ok(started) = server_tool_use_to_started(block, &whitelist) {
                                if let ServerToolEvent::Started { tool_call_id, .. } = &started {
                                    state.last_server_tool_id = Some(tool_call_id.to_string());
                                }
                                events.push(ProviderStreamEvent::ServerTool(started));
                            } else {
                                // 声明了 server tools 但收到未声明的 server_tool_use：
                                // fail-closed，显式报错而非静默吞掉。
                                events.push(ProviderStreamEvent::Error(
                                    provider_api::ProviderError::new(
                                        provider_api::ProviderErrorKind::MalformedResponse,
                                        "anthropic server_tool_use block not in declared server tools",
                                    ),
                                ));
                            }
                        }
                    }
                    "text" | "thinking" => {
                        // content_block_start 不带内容，内容在后续 delta 中
                        if block_type == "text" {
                            if let Some(citations) =
                                block.get("citations").and_then(Value::as_array)
                            {
                                append_citations(block, citations, state, &mut events);
                            }
                        }
                    }
                    // P15-3：Provider 端 server tool 结果块（与客户端 tool_result
                    // 严格分离：客户端结果只出现在请求的 user 消息中）。
                    "web_search_tool_result"
                    | "web_fetch_tool_result"
                    | "code_execution_tool_result"
                    | "bash_tool_result"
                    | "bash_code_execution_tool_result"
                    | "text_editor_tool_result"
                    | "computer_tool_result"
                    | "tool_search_tool_result"
                    | "memory_tool_result"
                    | "mcp_connector_tool_result"
                    | "advisor_tool_result" => match server_tool_result_block_to_events(block) {
                        Ok(mapped) => {
                            for server_event in mapped {
                                events.push(ProviderStreamEvent::ServerTool(server_event));
                            }
                        }
                        Err(error) => events.push(ProviderStreamEvent::Error(
                            provider_api::ProviderError::new(
                                provider_api::ProviderErrorKind::MalformedResponse,
                                format!("anthropic server tool result unmapped: {error}"),
                            ),
                        )),
                    },
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
                    "citations_delta" => {
                        if let Some(citations) = delta.get("citations").and_then(Value::as_array) {
                            append_citations(&value, citations, state, &mut events);
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
            // P15-3：extended thinking 的 signature / redacted data 在
            // content_block_stop 的 content_block 中到达；捕获后由驱动方经
            // ReasoningContinuationStore 保护并发射 ReasoningItem。
            if state.capture_signatures {
                if let Some(block) = value.get("content_block") {
                    let block_type = block
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if matches!(block_type, "thinking" | "redacted_thinking") {
                        state.pending_thinking_blocks.push(block.clone());
                    }
                }
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
    /// 是否捕获 thinking signature（现代路径开启；P6-2 基线保持忽略）。
    pub capture_signatures: bool,
    /// 已声明 server tool 的名称白名单（canonical + wire 名）。
    pub server_tool_names: Vec<String>,
    /// 最近一个 server tool 调用 id（citations 归属）。
    pub last_server_tool_id: Option<String>,
    /// 未绑定 server tool 上下文的 citation 序号（合成 tool_call_id）。
    pub citation_seq: usize,
    /// thinking signature 捕获序号（合成 ReasoningItemId）。
    pub reasoning_seq: usize,
    /// 已捕获、待保护发射的 thinking / redacted_thinking 原始块。
    pub pending_thinking_blocks: Vec<Value>,
}

impl AnthropicStreamState {
    /// 取走本轮已捕获的 thinking 续传块（按到达顺序）。
    pub fn drain_pending_thinking(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.pending_thinking_blocks)
    }
}

/// 把 citations 数组归一为 CitationAdded 事件（归属最近 server tool 调用，
/// 无上下文时合成 citation-<n> id，保证不丢字段）。
fn append_citations(
    _block: &Value,
    citations: &[Value],
    state: &mut AnthropicStreamState,
    events: &mut Vec<ProviderStreamEvent>,
) {
    for citation in citations {
        match citation_block_to_citation(citation) {
            Ok(mapped) => {
                let owner = match &state.last_server_tool_id {
                    Some(id) => ToolCallId::from(id.clone()),
                    None => {
                        state.citation_seq += 1;
                        ToolCallId::from(format!("citation-{}", state.citation_seq))
                    }
                };
                events.push(ProviderStreamEvent::ServerTool(
                    ServerToolEvent::CitationAdded {
                        tool_call_id: owner,
                        citation: mapped,
                    },
                ));
            }
            Err(error) => events.push(ProviderStreamEvent::Error(
                provider_api::ProviderError::new(
                    provider_api::ProviderErrorKind::MalformedResponse,
                    format!("anthropic citation unmapped: {error}"),
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modern::WEB_SEARCH_TOOL;
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

    #[test]
    fn server_tool_use_maps_to_server_tool_started_only_for_declared_names() {
        let mut state = AnthropicStreamState {
            server_tool_names: vec!["web_search".into(), WEB_SEARCH_TOOL.into()],
            ..AnthropicStreamState::default()
        };
        let events = event_to_events(
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{"query":"pawork"}}}"#,
            &mut state,
        );
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::ServerTool(agent_domain::ServerToolEvent::Started {
                tool_call_id,
                name,
                arguments: Some(arguments),
            }) if tool_call_id.as_str() == "srvtoolu_1"
                && name == "web_search"
                && arguments["query"] == "pawork"
        ));
        assert_eq!(state.last_server_tool_id.as_deref(), Some("srvtoolu_1"));

        // 未声明的 server_tool_use → fail-closed Error，不静默吞掉。
        let mut state2 = AnthropicStreamState {
            server_tool_names: vec!["web_search".into()],
            ..AnthropicStreamState::default()
        };
        let events = event_to_events(
            r#"{"type":"content_block_start","index":3,"content_block":{"type":"server_tool_use","id":"srvtoolu_2","name":"mystery","input":{}}}"#,
            &mut state2,
        );
        assert!(matches!(&events[0], ProviderStreamEvent::Error(_)));
    }

    #[test]
    fn server_tool_result_block_maps_lifecycle_events() {
        let mut state = AnthropicStreamState {
            server_tool_names: vec!["web_search".into()],
            ..AnthropicStreamState::default()
        };
        let events = event_to_events(
            r#"{"type":"content_block_start","index":4,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","url":"https://pawork.dev","title":"Pawork"}]}}"#,
            &mut state,
        );
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::ServerTool(agent_domain::ServerToolEvent::SourceAdded {
                tool_call_id,
                source,
            }) if tool_call_id.as_str() == "srvtoolu_1"
                && source.url.as_deref() == Some("https://pawork.dev")
        ));
        assert!(matches!(
            &events[1],
            ProviderStreamEvent::ServerTool(agent_domain::ServerToolEvent::Completed {
                tool_call_id,
                ..
            }) if tool_call_id.as_str() == "srvtoolu_1"
        ));
    }

    #[test]
    fn text_citations_normalize_to_citation_added() {
        let mut state = AnthropicStreamState {
            last_server_tool_id: Some("srvtoolu_1".into()),
            ..AnthropicStreamState::default()
        };
        let events = event_to_events(
            r#"{"type":"content_block_start","index":5,"content_block":{"type":"text","text":"see","citations":[{"type":"web_search_result_location","url":"https://pawork.dev","title":"Pawork"}]}}"#,
            &mut state,
        );
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::ServerTool(agent_domain::ServerToolEvent::CitationAdded {
                tool_call_id,
                citation,
            }) if tool_call_id.as_str() == "srvtoolu_1"
                && citation.url.as_deref() == Some("https://pawork.dev")
        ));

        // 无 server tool 上下文 → 合成 citation-<n> id，不丢字段。
        let mut state2 = AnthropicStreamState::default();
        let events = event_to_events(
            r#"{"type":"content_block_delta","index":6,"delta":{"type":"citations_delta","citations":[{"type":"char_location","cited_text":"doc"}]}}"#,
            &mut state2,
        );
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::ServerTool(agent_domain::ServerToolEvent::CitationAdded {
                tool_call_id,
                ..
            }) if tool_call_id.as_str() == "citation-1"
        ));
    }

    #[test]
    fn thinking_signature_captured_only_in_modern_mode() {
        // legacy 模式（capture_signatures=false）：signature 保持忽略。
        let mut legacy = AnthropicStreamState::default();
        let events = event_to_events(
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"hmm","signature":"SIG-SECRET"}}"#,
            &mut legacy,
        );
        assert!(events.is_empty());
        assert!(legacy.drain_pending_thinking().is_empty());

        // modern 模式：块进 pending，等待驱动方保护后发射。
        let mut modern = AnthropicStreamState {
            capture_signatures: true,
            ..AnthropicStreamState::default()
        };
        let events = event_to_events(
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"hmm","signature":"SIG-SECRET"}}"#,
            &mut modern,
        );
        assert!(events.is_empty());
        let pending = modern.drain_pending_thinking();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["signature"], "SIG-SECRET");

        // redacted_thinking 同样进入 pending。
        let mut modern = AnthropicStreamState {
            capture_signatures: true,
            ..AnthropicStreamState::default()
        };
        let _ = event_to_events(
            r#"{"type":"content_block_stop","index":1,"content_block":{"type":"redacted_thinking","data":"REDACTED-B64"}}"#,
            &mut modern,
        );
        assert_eq!(modern.drain_pending_thinking().len(), 1);
    }

    #[test]
    fn interleaved_thinking_and_tools_preserve_wire_order() {
        let mut state = AnthropicStreamState {
            capture_signatures: true,
            server_tool_names: vec!["web_search".into()],
            ..AnthropicStreamState::default()
        };
        let mut all = Vec::new();
        for data in [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step one"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step two"}}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{"query":"x"}}}"#,
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"step one step two","signature":"SIG-1"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"p\":1}"}}"#,
            r#"{"type":"content_block_start","index":3,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","url":"https://pawork.dev"}]}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
            r#"{"type":"message_stop"}"#,
        ] {
            all.extend(event_to_events(data, &mut state));
        }

        use agent_domain::ServerToolEvent;
        let kinds: Vec<&str> = all
            .iter()
            .map(|event| match event {
                ProviderStreamEvent::ThinkingDelta(_) => "thinking",
                ProviderStreamEvent::ToolCallStarted { .. } => "tool_start",
                ProviderStreamEvent::ToolCallArgumentsDelta { .. } => "tool_args",
                ProviderStreamEvent::ServerTool(ServerToolEvent::Started { .. }) => "server_start",
                ProviderStreamEvent::ServerTool(ServerToolEvent::SourceAdded { .. }) => {
                    "server_source"
                }
                ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { .. }) => {
                    "server_completed"
                }
                ProviderStreamEvent::UsageUpdated(_) => "usage",
                ProviderStreamEvent::ResponseCompleted(_) => "completed",
                _ => "other",
            })
            .collect();
        // 顺序与 wire 一致：thinking 交错在 tool_use / server_tool_use 之间。
        assert_eq!(
            kinds,
            vec![
                "thinking",
                "tool_start",
                "thinking",
                "server_start",
                "tool_args",
                "server_source",
                "server_completed",
                "usage",
                "completed",
            ]
        );
        // signature 已捕获（驱动方随后按顺序保护并发射 ReasoningItem）。
        assert_eq!(state.drain_pending_thinking().len(), 1);
    }
}
