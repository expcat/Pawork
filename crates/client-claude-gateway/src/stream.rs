//! Anthropic Messages streaming → canonical Provider/Agent 事件边界（P18-12 §2）。
//!
//! 本层是有状态事件翻译：text / thinking / tool_use / tool_result / usage /
//! error / cancel 逐事件映射为 [`provider_api::ProviderStreamEvent`]（Provider
//! 事件边界）与 SDK 层可观察事件（Agent 事件边界），不携带任何业务决策。
//!
//! signed thinking 材料（`signature` / `data`）在此只被**捕获**为受保护内存
//! 结构（`Debug` 脱敏、不可序列化），随后由宿主驱动的
//! [`protect_pending_signed`] 经 [`SignedThinkingProtector`] 转成 Protected
//! Blob 引用；明文永不进入 canonical 事件 / `Debug` / 日志 / 普通存储。
//! 未协商 `reasoning.signed_continuity` 能力时收到 signed 材料显式失败。

use std::collections::HashMap;
use std::sync::Arc;

use agent_domain::{ReasoningItemId, StopReason, TokenUsage, ToolCallId};
use provider_api::{ProviderError, ProviderErrorKind, ProviderStreamEvent};

use crate::adapter::REASONING_SIGNED_CONTINUITY_CAPABILITY;
use crate::control::{
    map_assistant_snapshot, map_control_request, map_control_response, map_hook_event,
    map_user_message, ControlEvent,
};
use crate::error::ClaudeGatewayError;
use crate::reasoning::SignedThinkingProtector;
use crate::wire::{ClaudeStreamEvent, SignedThinkingBlock};

/// 网关映射层的可观察输出事件。
#[derive(Clone, Debug, PartialEq)]
pub enum GatewayEvent {
    /// canonical Provider 流事件（Provider/Agent 事件边界）。
    Stream(ProviderStreamEvent),
    /// SDK 层 permission / subagent / task / hook / 生命周期可观察事件。
    Control(ControlEvent),
    /// 上游流错误（kind 已翻译；不含 signed thinking 材料）。
    Error(ProviderError),
    /// 未知线协议类型：保留类型名显式上报，不静默丢弃。
    Unmapped { event_type: String },
}

/// 流映射期间需要跨事件保持的状态。
///
/// 安全不变量：
///
/// - `reasoning_supported` 默认 `false`（fail-closed）：未协商时收到 signed
///   thinking 材料显式失败；
/// - `signature_parts` / `pending_signed` 持有明文签名材料，因此本结构不派生
///   `Serialize`，`Debug` 只打印数量不打印内容；
/// - `protector` 由宿主注入（生产为 `ProtectedBlobStoreProtector`，ADR-032），
///   未注入时 [`protect_pending_signed`] 显式失败。
#[derive(Clone)]
pub struct ClaudeStreamState {
    /// content block index → tool call id。
    pub tool_ids: HashMap<usize, String>,
    /// 最近一次 `message_delta` 的 stop_reason。
    pub stop_reason: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub finished: bool,
    /// 是否已协商 `reasoning.signed_continuity` 能力。
    pub reasoning_supported: bool,
    /// signed thinking 保护器（宿主注入；`None` 时保护显式失败）。
    pub protector: Option<Arc<dyn SignedThinkingProtector>>,
    /// `signature_delta` 累积（index → 已收到的签名分片；受保护明文）。
    signature_parts: HashMap<usize, String>,
    /// 已捕获、待保护发射的 signed thinking 块（按到达顺序；受保护明文）。
    pending_signed: Vec<(ReasoningItemId, SignedThinkingBlock)>,
    /// signed thinking 捕获序号（合成 `ReasoningItemId`）。
    reasoning_seq: usize,
}

impl Default for ClaudeStreamState {
    /// fail-closed 默认值：能力未协商、保护器未注入。
    fn default() -> Self {
        Self {
            tool_ids: HashMap::new(),
            stop_reason: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            finished: false,
            reasoning_supported: false,
            protector: None,
            signature_parts: HashMap::new(),
            pending_signed: Vec::new(),
            reasoning_seq: 0,
        }
    }
}

impl std::fmt::Debug for ClaudeStreamState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaudeStreamState")
            .field("tool_ids", &self.tool_ids)
            .field("stop_reason", &self.stop_reason)
            .field("input_tokens", &self.input_tokens)
            .field("output_tokens", &self.output_tokens)
            .field("cache_read_tokens", &self.cache_read_tokens)
            .field("cache_write_tokens", &self.cache_write_tokens)
            .field("finished", &self.finished)
            .field("reasoning_supported", &self.reasoning_supported)
            .field("protector", &self.protector.as_ref().map(|_| "[PROTECTOR]"))
            .field("signature_parts", &self.signature_parts.len())
            .field("pending_signed", &self.pending_signed.len())
            .finish()
    }
}

impl ClaudeStreamState {
    /// 以显式协商结果构造（默认 [`Default`] 为 fail-closed）。
    pub fn new(reasoning_supported: bool) -> Self {
        Self {
            reasoning_supported,
            ..Self::default()
        }
    }

    /// 注入 signed thinking 保护器。
    pub fn set_protector(&mut self, protector: Arc<dyn SignedThinkingProtector>) {
        self.protector = Some(protector);
    }

    /// 链式注入 signed thinking 保护器。
    pub fn with_protector(mut self, protector: Arc<dyn SignedThinkingProtector>) -> Self {
        self.set_protector(protector);
        self
    }

    /// 待保护 signed thinking 块数量（不暴露材料内容）。
    pub fn pending_signed_count(&self) -> usize {
        self.pending_signed.len()
    }
}

/// 把一条已解析的 Claude 流事件映射为 canonical 事件（有状态）。
///
/// 除 signed thinking 保护（需异步 [`protect_pending_signed`]）外的所有映射
/// 在此完成；signed 材料到达但能力未协商时返回
/// [`ClaudeGatewayError::SignedThinkingNotNegotiated`]（fail-closed）。
pub fn map_sse_event(
    state: &mut ClaudeStreamState,
    event: &ClaudeStreamEvent,
) -> Result<Vec<GatewayEvent>, ClaudeGatewayError> {
    let mut events = Vec::new();
    match event {
        ClaudeStreamEvent::MessageStart {
            message_id, usage, ..
        } => {
            state.input_tokens = usage.input_tokens;
            state.cache_read_tokens = usage.cache_read_tokens;
            state.cache_write_tokens = usage.cache_write_tokens;
            events.push(GatewayEvent::Stream(ProviderStreamEvent::ResponseStarted {
                response_id: message_id.clone(),
            }));
        }
        ClaudeStreamEvent::ContentBlockStart { index, block } => match block {
            crate::wire::ClaudeContentBlockStart::Text
            | crate::wire::ClaudeContentBlockStart::Thinking
            | crate::wire::ClaudeContentBlockStart::RedactedThinking => {}
            crate::wire::ClaudeContentBlockStart::ToolUse { id, name } => {
                state.tool_ids.insert(*index, id.clone());
                events.push(GatewayEvent::Stream(ProviderStreamEvent::ToolCallStarted {
                    id: ToolCallId::from(id.clone()),
                    name: name.clone(),
                }));
            }
            crate::wire::ClaudeContentBlockStart::ToolResult {
                tool_use_id,
                is_error,
            } => {
                events.push(GatewayEvent::Control(ControlEvent::ToolResultSubmitted {
                    tool_use_id: tool_use_id.clone(),
                    is_error: *is_error,
                }));
            }
            crate::wire::ClaudeContentBlockStart::Other { block_type } => {
                events.push(GatewayEvent::Unmapped {
                    event_type: format!("content_block:{block_type}"),
                });
            }
        },
        ClaudeStreamEvent::ContentBlockDelta { index, delta } => match delta {
            crate::wire::ClaudeContentBlockDelta::Text { text } => {
                if !text.is_empty() {
                    events.push(GatewayEvent::Stream(ProviderStreamEvent::TextDelta(
                        text.clone(),
                    )));
                }
            }
            crate::wire::ClaudeContentBlockDelta::Thinking { thinking } => {
                if !thinking.is_empty() {
                    events.push(GatewayEvent::Stream(ProviderStreamEvent::ThinkingDelta(
                        thinking.clone(),
                    )));
                }
            }
            crate::wire::ClaudeContentBlockDelta::Signature { signature } => {
                // 受保护明文只累积进状态（Debug 脱敏），不在 canonical 事件出现。
                state
                    .signature_parts
                    .entry(*index)
                    .or_default()
                    .push_str(signature);
            }
            crate::wire::ClaudeContentBlockDelta::InputJson { partial_json } => {
                if let Some(id) = state.tool_ids.get(index) {
                    if !partial_json.is_empty() {
                        events.push(GatewayEvent::Stream(
                            ProviderStreamEvent::ToolCallArgumentsDelta {
                                id: ToolCallId::from(id.clone()),
                                json: partial_json.clone(),
                            },
                        ));
                    }
                }
            }
            crate::wire::ClaudeContentBlockDelta::Other { delta_type } => {
                events.push(GatewayEvent::Unmapped {
                    event_type: format!("content_block_delta:{delta_type}"),
                });
            }
        },
        ClaudeStreamEvent::ContentBlockStop { index, thinking } => {
            if let Some(id) = state.tool_ids.get(index) {
                events.push(GatewayEvent::Stream(
                    ProviderStreamEvent::ToolCallCompleted {
                        id: ToolCallId::from(id.clone()),
                    },
                ));
            }
            // `signature_delta` 流式到达时，content_block_stop 不重复携带材料：
            // 以累积的签名分片合成 signed 块，与内联 signature 同权处理。
            if let Some(block) = thinking {
                capture_signed(state, block.clone())?;
            } else if let Some(signature) = state.signature_parts.remove(index) {
                if !signature.is_empty() {
                    capture_signed(state, SignedThinkingBlock::thinking(signature))?;
                }
            }
        }
        ClaudeStreamEvent::MessageDelta { stop_reason, usage } => {
            if let Some(reason) = stop_reason {
                state.stop_reason = Some(reason.clone());
            }
            state.output_tokens = usage.output_tokens;
            if usage.cache_read_tokens > 0 {
                state.cache_read_tokens = usage.cache_read_tokens;
            }
            if usage.cache_write_tokens > 0 {
                state.cache_write_tokens = usage.cache_write_tokens;
            }
            if state.output_tokens > 0 {
                events.push(GatewayEvent::Stream(ProviderStreamEvent::UsageUpdated(
                    TokenUsage {
                        input_tokens: state.input_tokens,
                        output_tokens: state.output_tokens,
                        cache_read_tokens: state.cache_read_tokens,
                        cache_write_tokens: state.cache_write_tokens,
                    },
                )));
            }
        }
        ClaudeStreamEvent::MessageStop => {
            let has_tool_calls = !state.tool_ids.is_empty();
            let stop = map_stop_reason(state.stop_reason.as_deref(), has_tool_calls);
            events.push(GatewayEvent::Stream(
                ProviderStreamEvent::ResponseCompleted(stop),
            ));
            state.finished = true;
        }
        ClaudeStreamEvent::Aborted => {
            events.push(GatewayEvent::Control(ControlEvent::Interrupted {
                reason: Some("aborted".into()),
            }));
            events.push(GatewayEvent::Stream(ProviderStreamEvent::Error(
                ProviderError::cancelled("claude stream aborted"),
            )));
            state.finished = true;
        }
        ClaudeStreamEvent::Ping => {}
        ClaudeStreamEvent::Error {
            error_type,
            message,
        } => {
            events.push(GatewayEvent::Error(map_upstream_error(error_type, message)));
            state.finished = true;
        }
        ClaudeStreamEvent::StreamEvent { event } => {
            let content = event
                .get("content")
                .or_else(|| {
                    event
                        .get("message")
                        .and_then(|message| message.get("content"))
                })
                .and_then(serde_json::Value::as_array);
            match content {
                Some(content) => events.extend(map_assistant_snapshot(content)),
                None => events.push(GatewayEvent::Unmapped {
                    event_type: "stream_event".into(),
                }),
            }
        }
        ClaudeStreamEvent::HookEvent { event } => events.extend(map_hook_event(event)),
        ClaudeStreamEvent::ControlRequest { .. } => events.extend(map_control_request(event)?),
        ClaudeStreamEvent::ControlResponse { .. } => events.extend(map_control_response(event)?),
        ClaudeStreamEvent::UserMessage { content } => events.extend(map_user_message(content)),
        ClaudeStreamEvent::AssistantMessage { content } => {
            events.extend(map_assistant_snapshot(content));
        }
        ClaudeStreamEvent::ResultMessage { result_type } => {
            events.push(GatewayEvent::Control(ControlEvent::RunResult {
                result_type: result_type.clone(),
            }));
        }
        ClaudeStreamEvent::Unknown { event_type } => {
            events.push(GatewayEvent::Unmapped {
                event_type: event_type.clone(),
            });
        }
    }
    Ok(events)
}

/// 把已捕获的 signed thinking 块经保护器转成只含 Protected Blob 引用的
/// canonical [`ReasoningItem`] 并发射（异步保护边界）。
///
/// 保护器未注入 / 保护失败时显式失败；无待保护材料时为空操作。明文不进入
/// 任何返回事件或错误信息。
pub async fn protect_pending_signed(
    state: &mut ClaudeStreamState,
) -> Result<Vec<GatewayEvent>, ClaudeGatewayError> {
    if state.pending_signed.is_empty() {
        return Ok(Vec::new());
    }
    let protector =
        state
            .protector
            .clone()
            .ok_or(ClaudeGatewayError::SignedThinkingProtectorUnavailable(
                "no SignedThinkingProtector injected",
            ))?;
    let pending = std::mem::take(&mut state.pending_signed);
    let mut events = Vec::with_capacity(pending.len());
    for (id, block) in pending {
        let item =
            crate::reasoning::protect_signed_thinking(&block, protector.as_ref(), id).await?;
        events.push(GatewayEvent::Stream(ProviderStreamEvent::ReasoningItem(
            item,
        )));
    }
    Ok(events)
}

/// 能力协商检查 + 捕获（fail-closed）：未协商时显式失败，不静默丢弃。
fn capture_signed(
    state: &mut ClaudeStreamState,
    block: SignedThinkingBlock,
) -> Result<(), ClaudeGatewayError> {
    if !state.reasoning_supported {
        return Err(ClaudeGatewayError::SignedThinkingNotNegotiated(
            REASONING_SIGNED_CONTINUITY_CAPABILITY,
        ));
    }
    state.reasoning_seq += 1;
    let id = ReasoningItemId::from(format!("claude-reasoning-{}", state.reasoning_seq));
    state.pending_signed.push((id, block));
    Ok(())
}

/// Anthropic `stop_reason` → canonical [`StopReason`]（与 provider-runtime
/// `map_stop_reason` 的 canonical 口径一致；本 crate 不依赖 provider-runtime）。
fn map_stop_reason(finish: Option<&str>, has_tool_calls: bool) -> StopReason {
    if has_tool_calls {
        return StopReason::ToolUse;
    }
    match finish {
        None => StopReason::Completed,
        Some(reason) => match reason.to_ascii_lowercase().as_str() {
            "stop" | "end_turn" | "ended" => StopReason::Completed,
            "length" | "max_tokens" | "max_output_tokens" => StopReason::MaxTokens,
            "tool_calls" | "function_call" | "tool_use" | "functioncall" | "toolcalls" => {
                StopReason::ToolUse
            }
            "content_filter" | "content_filtered" | "safety" => StopReason::ContentFiltered,
            "cancelled" | "canceled" => StopReason::Cancelled,
            other => StopReason::Other(other.to_string()),
        },
    }
}

/// Anthropic error `type` → canonical [`ProviderErrorKind`]（脱敏翻译）。
fn map_upstream_error(error_type: &str, message: &str) -> ProviderError {
    let kind = match error_type {
        "overloaded_error" | "rate_limit_error" => ProviderErrorKind::RateLimited,
        "authentication_error" => ProviderErrorKind::Authentication,
        "permission_error" => ProviderErrorKind::Authorization,
        "not_found_error" | "invalid_request_error" => ProviderErrorKind::InvalidRequest,
        "api_error" => ProviderErrorKind::ProviderUnavailable,
        _ => ProviderErrorKind::Unknown,
    };
    ProviderError::new(kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reasoning::InMemorySignedThinkingProtector;
    use crate::wire::parse_event;

    fn drive(state: &mut ClaudeStreamState, events: &[&str]) -> Vec<GatewayEvent> {
        let mut mapped = Vec::new();
        for raw in events {
            let event = parse_event(raw).expect("parse");
            mapped.extend(map_sse_event(state, &event).expect("map"));
        }
        mapped
    }

    #[test]
    fn text_tool_usage_flow_maps_to_canonical_boundary() {
        let mut state = ClaudeStreamState::default();
        let events = drive(
            &mut state,
            &[
                r#"{"type":"message_start","message":{"id":"msg-1","model":"claude-x","usage":{"input_tokens":10,"output_tokens":1}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-1","name":"read","input":{}}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"/a\"}"}}"#,
                r#"{"type":"content_block_stop","index":1}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}"#,
                r#"{"type":"message_stop"}"#,
            ],
        );
        assert_eq!(
            events,
            vec![
                GatewayEvent::Stream(ProviderStreamEvent::ResponseStarted {
                    response_id: Some("msg-1".into()),
                }),
                GatewayEvent::Stream(ProviderStreamEvent::TextDelta("hello".into())),
                GatewayEvent::Stream(ProviderStreamEvent::ToolCallStarted {
                    id: ToolCallId::from("call-1"),
                    name: "read".into(),
                }),
                GatewayEvent::Stream(ProviderStreamEvent::ToolCallArgumentsDelta {
                    id: ToolCallId::from("call-1"),
                    json: "{\"path\":".into(),
                }),
                GatewayEvent::Stream(ProviderStreamEvent::ToolCallArgumentsDelta {
                    id: ToolCallId::from("call-1"),
                    json: "\"/a\"}".into(),
                }),
                GatewayEvent::Stream(ProviderStreamEvent::ToolCallCompleted {
                    id: ToolCallId::from("call-1"),
                }),
                GatewayEvent::Stream(ProviderStreamEvent::UsageUpdated(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 15,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                })),
                GatewayEvent::Stream(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse)),
            ]
        );
        assert!(state.finished);
    }

    #[tokio::test]
    async fn signed_thinking_without_capability_fails_closed() {
        let mut state = ClaudeStreamState::default();
        let event = parse_event(
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"hmm","signature":"SIG-SECRET"}}"#,
        )
        .expect("parse");
        let error = map_sse_event(&mut state, &event).expect_err("must fail closed");
        assert!(matches!(
            error,
            ClaudeGatewayError::SignedThinkingNotNegotiated(REASONING_SIGNED_CONTINUITY_CAPABILITY)
        ));
        assert!(!format!("{error}").contains("SIG-SECRET"));
        assert!(state.pending_signed.is_empty());
    }

    #[tokio::test]
    async fn signature_delta_accumulates_into_signed_block() {
        let mut state = ClaudeStreamState::new(true)
            .with_protector(Arc::new(InMemorySignedThinkingProtector::new()));
        let events = drive(
            &mut state,
            &[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step one"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG-"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SECRET"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
            ],
        );
        assert_eq!(
            events,
            vec![GatewayEvent::Stream(ProviderStreamEvent::ThinkingDelta(
                "step one".into()
            ))]
        );
        let protected = protect_pending_signed(&mut state)
            .await
            .expect("protect pending");
        assert_eq!(protected.len(), 1);
        match &protected[0] {
            GatewayEvent::Stream(ProviderStreamEvent::ReasoningItem(item)) => {
                assert_eq!(item.id.as_str(), "claude-reasoning-1");
                let payload = state
                    .protector
                    .as_ref()
                    .expect("protector")
                    .resolve(&item.protected_blob_ref)
                    .await
                    .expect("resolve");
                let material: crate::reasoning::SignedThinkingMaterial =
                    serde_json::from_slice(&payload).expect("decode");
                assert_eq!(material.kind(), "thinking");
                let encoded = serde_json::to_string(&item).expect("serialize");
                assert!(!encoded.contains("SIG-SECRET"));
                assert!(!encoded.contains("SIG-"));
            }
            other => panic!("expected reasoning item, got {other:?}"),
        }
    }

    #[test]
    fn aborted_and_upstream_error_map_explicitly() {
        let mut state = ClaudeStreamState::default();
        let events = drive(
            &mut state,
            &[
                r#"{"type":"aborted"}"#,
                r#"{"type":"error","error":{"type":"overloaded_error","message":"try later"}}"#,
            ],
        );
        assert!(matches!(
            &events[0],
            GatewayEvent::Control(ControlEvent::Interrupted { reason: Some(reason) })
                if reason == "aborted"
        ));
        assert!(matches!(
            &events[1],
            GatewayEvent::Stream(ProviderStreamEvent::Error(error))
                if error.kind == ProviderErrorKind::Cancelled
        ));
        assert!(matches!(
            &events[2],
            GatewayEvent::Error(error) if error.kind == ProviderErrorKind::RateLimited
        ));
        assert!(state.finished);
    }
}
