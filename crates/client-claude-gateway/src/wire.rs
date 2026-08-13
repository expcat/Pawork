//! Anthropic Messages / Claude Agent SDK 线协议解析（版本化基线 v1，P18-12 §2）。
//!
//! SSE 帧格式（gateway /v1/messages 流）：
//!
//! ```text
//! event: message_start
//! data: {"type":"message_start","message":{...}}
//!
//! ```
//!
//! SDK 在流内附加 `stream_event` / `hook_event` / `control_request` /
//! `control_response` / `user` / `assistant` / `result` 等消息类型。SDK 类型
//! 是内部线协议，本模块只解析本任务需要的形状，未知事件保留类型名显式上报
//! （不静默丢弃），未知字段不参与 canonical 映射。

use agent_domain::TokenUsage;
use serde_json::Value;
use std::fmt;

use crate::error::ClaudeGatewayError;

/// 一条解析后的 SSE 帧（`event:` 可选，`data:` 至少一行）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

/// 增量 SSE 解析器：按行累积，跨 chunk 断行安全。
///
/// 帧以空行分隔；`event:` / `data:` 字段按 SSE 规范解析（`data` 多行以 `\n`
/// 连接），`id:` / `retry:` / 注释行忽略。非法帧以 `Err` 返回，不静默跳过。
#[derive(Default)]
pub struct SseParser {
    line_buffer: String,
    event: Option<String>,
    data_lines: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一个 chunk，返回其中已完成的帧。
    pub fn push(&mut self, chunk: &str) -> Vec<Result<SseFrame, ClaudeGatewayError>> {
        self.line_buffer.push_str(chunk);
        let mut frames = Vec::new();
        while let Some(newline) = self.line_buffer.find('\n') {
            let mut line = self.line_buffer[..newline].to_string();
            self.line_buffer.drain(..=newline);
            if line.ends_with('\r') {
                line.pop();
            }
            if line.is_empty() {
                // 空行 = 帧结束；无 data 的帧（仅注释）不产生输出。
                let event = self.event.take();
                let data_lines = std::mem::take(&mut self.data_lines);
                if !data_lines.is_empty() {
                    frames.push(Ok(SseFrame {
                        event,
                        data: data_lines.join("\n"),
                    }));
                }
                continue;
            }
            if line.starts_with(':') {
                continue; // 注释 / keep-alive
            }
            let (name, value) = match line.split_once(':') {
                Some((name, value)) => (name.trim(), value.strip_prefix(' ').unwrap_or(value)),
                None => (line.trim(), ""),
            };
            match name {
                "event" => self.event = Some(value.to_string()),
                "data" => self.data_lines.push(value.to_string()),
                // SSE 规范允许的其他字段：保留但不参与映射。
                "id" | "retry" => {}
                _ => {}
            }
        }
        frames
    }
}

/// 受保护字符串：`signature` / `data` 等 signed thinking 材料。
///
/// `Debug` / 序列化均不暴露明文（序列化仅由 [`crate::reasoning::SignedThinkingMaterial`]
/// 以受控方式消费）。
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedString(String);

impl RedactedString {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RedactedString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// thinking block 类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinkingBlockKind {
    /// `{"type":"thinking","signature":...}`：明文推理文本经 ThinkingDelta 流转，
    /// `signature` 是续传凭证（受保护）。
    Thinking,
    /// `{"type":"redacted_thinking","data":...}`：服务端遮蔽的续传凭证（受保护）。
    RedactedThinking,
}

impl ThinkingBlockKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThinkingBlockKind::Thinking => "thinking",
            ThinkingBlockKind::RedactedThinking => "redacted_thinking",
        }
    }
}

/// `content_block_stop` 携带的 signed thinking 材料。
///
/// 红线：`Debug` 脱敏，不实现 `Display` / 不派生 `Serialize`；明文只能被
/// [`crate::reasoning::protect_signed_thinking`] 消费后进入 Protected Blob Store。
#[derive(Clone, PartialEq, Eq)]
pub struct SignedThinkingBlock {
    pub kind: ThinkingBlockKind,
    material: RedactedString,
}

impl SignedThinkingBlock {
    pub fn thinking(signature: String) -> Self {
        Self {
            kind: ThinkingBlockKind::Thinking,
            material: RedactedString::new(signature),
        }
    }

    pub fn redacted(data: String) -> Self {
        Self {
            kind: ThinkingBlockKind::RedactedThinking,
            material: RedactedString::new(data),
        }
    }

    /// 受保护材料（仅供 reasoning 模块保护消费，禁止记录 / 持久化 / 日志）。
    pub(crate) fn material(&self) -> &str {
        self.material.as_str()
    }
}

impl fmt::Debug for SignedThinkingBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignedThinkingBlock")
            .field("kind", &self.kind)
            .field("material", &self.material)
            .finish()
    }
}

/// `content_block_start` 的块分类（只解析映射需要的字段）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeContentBlockStart {
    Text,
    Thinking,
    RedactedThinking,
    ToolUse { id: String, name: String },
    ToolResult { tool_use_id: String, is_error: bool },
    Other { block_type: String },
}

/// `content_block_delta` 的 delta 分类。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeContentBlockDelta {
    Text { text: String },
    Thinking { thinking: String },
    Signature { signature: String },
    InputJson { partial_json: String },
    Other { delta_type: String },
}

/// 已解析的 Claude 流事件（Messages API + SDK 扩展）。
#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeStreamEvent {
    MessageStart {
        message_id: Option<String>,
        model: Option<String>,
        usage: TokenUsage,
    },
    ContentBlockStart {
        index: usize,
        block: ClaudeContentBlockStart,
    },
    ContentBlockDelta {
        index: usize,
        delta: ClaudeContentBlockDelta,
    },
    ContentBlockStop {
        index: usize,
        /// thinking / redacted_thinking 且携带受保护材料时为 `Some`。
        thinking: Option<SignedThinkingBlock>,
    },
    MessageDelta {
        stop_reason: Option<String>,
        usage: TokenUsage,
    },
    MessageStop,
    /// SDK 取消 / 中断（abort）。
    Aborted,
    Ping,
    Error {
        error_type: String,
        message: String,
    },
    /// SDK partial assistant 消息快照（`includePartialMessages`）。
    StreamEvent {
        event: Value,
    },
    /// SDK hook 生命周期事件（hook_name / payload）。
    HookEvent {
        event: Value,
    },
    /// SDK 控制请求（如 can_use_tool）。
    ControlRequest {
        request_id: String,
        subtype: String,
        data: Value,
    },
    /// SDK 控制响应（如 permission decision）。
    ControlResponse {
        request_id: String,
        subtype: String,
        data: Value,
    },
    /// SDK 中继的 user 消息（tool_result 提交）。
    UserMessage {
        content: Vec<Value>,
    },
    /// SDK 中继的完整 assistant 消息快照（includePartialMessages 关闭时）。
    AssistantMessage {
        content: Vec<Value>,
    },
    /// SDK 运行结果消息。
    ResultMessage {
        result_type: Option<String>,
    },
    /// 未知事件类型：保留类型名显式上报，不静默丢弃。
    Unknown {
        event_type: String,
    },
}

impl ClaudeStreamEvent {
    /// 稳定事件类型标签（与线协议 `type` 一致）。
    pub fn event_type(&self) -> &str {
        match self {
            ClaudeStreamEvent::MessageStart { .. } => "message_start",
            ClaudeStreamEvent::ContentBlockStart { .. } => "content_block_start",
            ClaudeStreamEvent::ContentBlockDelta { .. } => "content_block_delta",
            ClaudeStreamEvent::ContentBlockStop { .. } => "content_block_stop",
            ClaudeStreamEvent::MessageDelta { .. } => "message_delta",
            ClaudeStreamEvent::MessageStop => "message_stop",
            ClaudeStreamEvent::Aborted => "aborted",
            ClaudeStreamEvent::Ping => "ping",
            ClaudeStreamEvent::Error { .. } => "error",
            ClaudeStreamEvent::StreamEvent { .. } => "stream_event",
            ClaudeStreamEvent::HookEvent { .. } => "hook_event",
            ClaudeStreamEvent::ControlRequest { .. } => "control_request",
            ClaudeStreamEvent::ControlResponse { .. } => "control_response",
            ClaudeStreamEvent::UserMessage { .. } => "user",
            ClaudeStreamEvent::AssistantMessage { .. } => "assistant",
            ClaudeStreamEvent::ResultMessage { .. } => "result",
            ClaudeStreamEvent::Unknown { event_type } => event_type,
        }
    }
}

fn required_string<'a>(
    value: &'a Value,
    field: &'static str,
    event: &str,
) -> Result<&'a str, ClaudeGatewayError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| ClaudeGatewayError::MalformedEvent(event.into(), field))
}

fn parse_usage(value: Option<&Value>) -> TokenUsage {
    value
        .map(|usage| TokenUsage {
            input_tokens: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_write_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        })
        .unwrap_or_default()
}

fn parse_index(value: &Value, event: &str) -> Result<usize, ClaudeGatewayError> {
    let index = value
        .get("index")
        .and_then(Value::as_u64)
        .ok_or_else(|| ClaudeGatewayError::MalformedEvent(event.into(), "index"))?;
    Ok(index as usize)
}

fn parse_content_block_start(value: &Value) -> Result<ClaudeContentBlockStart, ClaudeGatewayError> {
    let block = value.get("content_block").ok_or_else(|| {
        ClaudeGatewayError::MalformedEvent("content_block_start".into(), "content_block")
    })?;
    let block_type = required_string(block, "type", "content_block_start")?;
    match block_type {
        "text" => Ok(ClaudeContentBlockStart::Text),
        "thinking" => Ok(ClaudeContentBlockStart::Thinking),
        "redacted_thinking" => Ok(ClaudeContentBlockStart::RedactedThinking),
        "tool_use" => Ok(ClaudeContentBlockStart::ToolUse {
            id: required_string(block, "id", "content_block_start")?.to_string(),
            name: required_string(block, "name", "content_block_start")?.to_string(),
        }),
        "tool_result" => Ok(ClaudeContentBlockStart::ToolResult {
            tool_use_id: required_string(block, "tool_use_id", "content_block_start")?.to_string(),
            is_error: block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
        other => Ok(ClaudeContentBlockStart::Other {
            block_type: other.to_string(),
        }),
    }
}

fn parse_content_block_delta(value: &Value) -> Result<ClaudeContentBlockDelta, ClaudeGatewayError> {
    let delta = value
        .get("delta")
        .ok_or_else(|| ClaudeGatewayError::MalformedEvent("content_block_delta".into(), "delta"))?;
    let delta_type = required_string(delta, "type", "content_block_delta")?;
    match delta_type {
        "text_delta" => Ok(ClaudeContentBlockDelta::Text {
            text: required_string(delta, "text", "content_block_delta")?.to_string(),
        }),
        "thinking_delta" => Ok(ClaudeContentBlockDelta::Thinking {
            thinking: required_string(delta, "thinking", "content_block_delta")?.to_string(),
        }),
        "signature_delta" => Ok(ClaudeContentBlockDelta::Signature {
            signature: required_string(delta, "signature", "content_block_delta")?.to_string(),
        }),
        "input_json_delta" => Ok(ClaudeContentBlockDelta::InputJson {
            partial_json: required_string(delta, "partial_json", "content_block_delta")?
                .to_string(),
        }),
        other => Ok(ClaudeContentBlockDelta::Other {
            delta_type: other.to_string(),
        }),
    }
}

fn parse_thinking_block(value: &Value) -> Result<Option<SignedThinkingBlock>, ClaudeGatewayError> {
    let Some(block) = value.get("content_block") else {
        return Ok(None);
    };
    let Ok(block_type) = required_string(block, "type", "content_block_stop") else {
        return Ok(None);
    };
    match block_type {
        "thinking" => match block.get("signature").and_then(Value::as_str) {
            Some(signature) => Ok(Some(SignedThinkingBlock::thinking(signature.to_string()))),
            // 无 signature 的 thinking 块：普通推理（文本已流式）→ 不捕获。
            None => Ok(None),
        },
        "redacted_thinking" => {
            let data = required_string(block, "data", "content_block_stop")?;
            Ok(Some(SignedThinkingBlock::redacted(data.to_string())))
        }
        _ => Ok(None),
    }
}

fn parse_sdk_content(value: &Value, event: &str) -> Result<Vec<Value>, ClaudeGatewayError> {
    // Claude Code SDK 中继消息既可能把 content 放顶层（gateway 归一化后），
    // 也可能包在 `message` 字段内（原始 SDK 中继）；两者都读取，不猜测。
    let content = value
        .get("content")
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("content"))
        })
        .and_then(Value::as_array)
        .ok_or_else(|| ClaudeGatewayError::MalformedEvent(event.into(), "content"))?;
    Ok(content.clone())
}

fn parse_control_envelope(value: &Value) -> Result<(String, String, Value), ClaudeGatewayError> {
    // control_request：{"request_id", "request":{"subtype", ...}}
    // control_response：{"response":{"request_id", "response":{"subtype", ...}}}
    let outer = value
        .get("request")
        .or_else(|| value.get("response"))
        .ok_or_else(|| {
            ClaudeGatewayError::MalformedEvent("control_*".into(), "request/response")
        })?;
    // request_id：control_request 在顶层，control_response 在 response 信封内。
    let request_id = value
        .get("request_id")
        .or_else(|| outer.get("request_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| ClaudeGatewayError::MalformedEvent("control_*".into(), "request_id"))?
        .to_string();
    let inner = outer.get("response").unwrap_or(outer);
    let subtype = required_string(inner, "subtype", "control_*")?.to_string();
    Ok((request_id, subtype, inner.clone()))
}

/// 解析单条 SSE data（一个 Claude 事件 JSON）。
pub fn parse_event(data: &str) -> Result<ClaudeStreamEvent, ClaudeGatewayError> {
    let value: Value = serde_json::from_str(data)
        .map_err(|_| ClaudeGatewayError::MalformedSse("event data is not valid JSON".into()))?;
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ClaudeGatewayError::MalformedEvent("<missing>".into(), "type"))?;
    match event_type {
        "message_start" => {
            let message = value.get("message").ok_or_else(|| {
                ClaudeGatewayError::MalformedEvent("message_start".into(), "message")
            })?;
            Ok(ClaudeStreamEvent::MessageStart {
                message_id: message
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                model: message
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                usage: parse_usage(message.get("usage")),
            })
        }
        "content_block_start" => Ok(ClaudeStreamEvent::ContentBlockStart {
            index: parse_index(&value, "content_block_start")?,
            block: parse_content_block_start(&value)?,
        }),
        "content_block_delta" => Ok(ClaudeStreamEvent::ContentBlockDelta {
            index: parse_index(&value, "content_block_delta")?,
            delta: parse_content_block_delta(&value)?,
        }),
        "content_block_stop" => Ok(ClaudeStreamEvent::ContentBlockStop {
            index: parse_index(&value, "content_block_stop")?,
            thinking: parse_thinking_block(&value)?,
        }),
        "message_delta" => Ok(ClaudeStreamEvent::MessageDelta {
            stop_reason: value
                .get("delta")
                .and_then(|delta| delta.get("stop_reason"))
                .and_then(Value::as_str)
                .map(str::to_string),
            usage: parse_usage(value.get("usage")),
        }),
        "message_stop" => Ok(ClaudeStreamEvent::MessageStop),
        "aborted" => Ok(ClaudeStreamEvent::Aborted),
        "ping" => Ok(ClaudeStreamEvent::Ping),
        "error" => {
            let error = value
                .get("error")
                .ok_or_else(|| ClaudeGatewayError::MalformedEvent("error".into(), "error"))?;
            Ok(ClaudeStreamEvent::Error {
                error_type: required_string(error, "type", "error")?.to_string(),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        "stream_event" => Ok(ClaudeStreamEvent::StreamEvent {
            event: value.get("event").cloned().unwrap_or(Value::Null),
        }),
        "hook_event" => Ok(ClaudeStreamEvent::HookEvent {
            event: value.get("event").cloned().unwrap_or(Value::Null),
        }),
        "control_request" => {
            let (request_id, subtype, data) = parse_control_envelope(&value)?;
            Ok(ClaudeStreamEvent::ControlRequest {
                request_id,
                subtype,
                data,
            })
        }
        "control_response" => {
            let (request_id, subtype, data) = parse_control_envelope(&value)?;
            Ok(ClaudeStreamEvent::ControlResponse {
                request_id,
                subtype,
                data,
            })
        }
        "user" => Ok(ClaudeStreamEvent::UserMessage {
            content: parse_sdk_content(&value, "user")?,
        }),
        "assistant" => Ok(ClaudeStreamEvent::AssistantMessage {
            content: parse_sdk_content(&value, "assistant")?,
        }),
        "result" => Ok(ClaudeStreamEvent::ResultMessage {
            result_type: value
                .get("subtype")
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
        "system" => Ok(ClaudeStreamEvent::Unknown {
            event_type: "system".into(),
        }),
        other => Ok(ClaudeStreamEvent::Unknown {
            event_type: other.to_string(),
        }),
    }
}

/// 解析完整 SSE 帧：校验 `event:` 名与 data `type` 一致（fail-closed），再解析事件。
pub fn decode_frame(frame: &SseFrame) -> Result<ClaudeStreamEvent, ClaudeGatewayError> {
    let event = parse_event(&frame.data)?;
    if let Some(name) = &frame.event {
        if name != event.event_type() {
            return Err(ClaudeGatewayError::MalformedSse(format!(
                "SSE `event:` field `{name}` does not match data type `{}`",
                event.event_type()
            )));
        }
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_splits_frames_across_chunks_and_lines() {
        let mut parser = SseParser::new();
        let mut frames = Vec::new();
        frames.extend(parser.push(": keep-alive\nevent: message_start\ndata: {\"type\":\"mess"));
        frames.extend(parser.push("age_start\"}\n\n"));
        frames.extend(parser.push("data: {\"type\":\"ping\"}\n\n"));
        let parsed: Vec<SseFrame> = frames.into_iter().map(|f| f.expect("valid")).collect();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].event.as_deref(), Some("message_start"));
        assert_eq!(parsed[0].data, r#"{"type":"message_start"}"#);
        assert_eq!(parsed[1].event, None);
        assert_eq!(parsed[1].data, r#"{"type":"ping"}"#);
    }

    #[test]
    fn sse_parser_joins_multiline_data_with_newline() {
        let mut parser = SseParser::new();
        let frames = parser.push("data: first\ndata: second\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].as_ref().expect("valid").data, "first\nsecond");
    }

    #[test]
    fn malformed_json_fails_closed() {
        assert!(matches!(
            parse_event("not json"),
            Err(ClaudeGatewayError::MalformedSse(_))
        ));
        assert!(matches!(
            parse_event(r#"{"no_type":true}"#),
            Err(ClaudeGatewayError::MalformedEvent(_, _))
        ));
    }

    #[test]
    fn frame_event_name_mismatch_fails_closed() {
        let frame = SseFrame {
            event: Some("message_start".into()),
            data: r#"{"type":"ping"}"#.into(),
        };
        assert!(matches!(
            decode_frame(&frame),
            Err(ClaudeGatewayError::MalformedSse(_))
        ));
    }

    #[test]
    fn thinking_stop_block_captures_signed_material() {
        let event = parse_event(
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"hmm","signature":"SIG-SECRET"}}"#,
        )
        .expect("parse");
        match event {
            ClaudeStreamEvent::ContentBlockStop { index, thinking } => {
                assert_eq!(index, 0);
                let block = thinking.expect("signed block");
                assert_eq!(block.kind, ThinkingBlockKind::Thinking);
                assert_eq!(block.material(), "SIG-SECRET");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn thinking_stop_block_without_signature_is_not_signed_material() {
        let event = parse_event(
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"hmm"}}"#,
        )
        .expect("parse");
        match event {
            ClaudeStreamEvent::ContentBlockStop { thinking, .. } => {
                assert!(thinking.is_none());
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn signed_material_never_appears_in_debug() {
        let event = parse_event(
            r#"{"type":"content_block_stop","index":0,"content_block":{"type":"redacted_thinking","data":"DATA-SECRET"}}"#,
        )
        .expect("parse");
        let debug = format!("{event:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("DATA-SECRET"));
    }

    #[test]
    fn control_request_envelope_parses() {
        let event = parse_event(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}"#,
        )
        .expect("parse");
        match event {
            ClaudeStreamEvent::ControlRequest {
                request_id,
                subtype,
                data,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(subtype, "can_use_tool");
                assert_eq!(data["tool_name"], "Bash");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn control_response_nested_envelope_parses() {
        let event = parse_event(
            r#"{"type":"control_response","response":{"request_id":"req-1","response":{"subtype":"success","request_id":"req-1","response":{"behavior":"allow"}}}}"#,
        )
        .expect("parse");
        match event {
            ClaudeStreamEvent::ControlResponse {
                request_id,
                subtype,
                data,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(subtype, "success");
                assert_eq!(data["response"]["behavior"], "allow");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn unknown_event_type_is_preserved() {
        let event = parse_event(r#"{"type":"future_event","x":1}"#).expect("parse");
        match event {
            ClaudeStreamEvent::Unknown { event_type } => assert_eq!(event_type, "future_event"),
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
