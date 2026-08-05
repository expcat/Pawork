//! 流式组装（P2-8）。
//!
//! 将 [`ProviderStreamEvent`](provider_api::ProviderStreamEvent) 序列组装为领域
//! [`Message`](agent_domain::Message)，支持流式中途的 partial 消息表达，以及
//! 多个 tool call 并行流的正确还原。
use std::collections::BTreeMap;

use agent_domain::{
    ContentPart, Message, MessageId, MessageMetadata, MessageRole, StopReason, TextContent,
    ThinkingContent, TokenUsage, ToolCallContent, ToolCallId,
};
use provider_api::{ProviderError, ProviderErrorKind, ProviderStreamEvent};

use crate::partial_json::PartialJson;

/// 单个 tool call 的组装状态。
#[derive(Clone, Debug)]
pub struct ToolAssembly {
    pub id: ToolCallId,
    pub name: String,
    pub arguments_json: String,
    pub complete: bool,
}

/// 流式中途可渲染的 partial 消息（`incomplete = true` 表示尚未完成）。
#[derive(Clone, Debug, Default)]
pub struct PartialMessage {
    pub text: String,
    pub thinking: String,
    pub tool_calls: Vec<ToolAssembly>,
    pub usage: TokenUsage,
    pub stop_reason: Option<StopReason>,
    pub complete: bool,
}

/// `ProviderStreamEvent` → 领域消息的增量组装器。
#[derive(Default)]
pub struct StreamAssembler {
    text: String,
    thinking: String,
    tools: BTreeMap<ToolCallId, ToolAssembly>,
    /// 保持 tool call 的到达顺序（BTreeMap 按 id 排序，顺序单独记录）。
    tool_order: Vec<ToolCallId>,
    usage: TokenUsage,
    stop_reason: Option<StopReason>,
    response_id: Option<String>,
    complete: bool,
}

impl StreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// 聚合一个事件。
    pub fn apply(&mut self, event: &ProviderStreamEvent) {
        match event {
            ProviderStreamEvent::ResponseStarted { response_id } => {
                self.response_id.clone_from(response_id);
            }
            ProviderStreamEvent::TextDelta(delta) => {
                self.text.push_str(delta);
            }
            ProviderStreamEvent::ThinkingDelta(delta) => {
                self.thinking.push_str(delta);
            }
            ProviderStreamEvent::ToolCallStarted { id, name } => {
                if !self.tools.contains_key(id) {
                    self.tool_order.push(id.clone());
                }
                self.tools.insert(
                    id.clone(),
                    ToolAssembly {
                        id: id.clone(),
                        name: name.clone(),
                        arguments_json: String::new(),
                        complete: false,
                    },
                );
            }
            ProviderStreamEvent::ToolCallArgumentsDelta { id, json } => {
                if let Some(tool) = self.tools.get_mut(id) {
                    tool.arguments_json.push_str(json);
                }
            }
            ProviderStreamEvent::ToolCallCompleted { id } => {
                if let Some(tool) = self.tools.get_mut(id) {
                    tool.complete = true;
                }
            }
            ProviderStreamEvent::UsageUpdated(usage) => {
                self.usage = usage.clone();
            }
            ProviderStreamEvent::ResponseCompleted(stop) => {
                self.stop_reason = Some(stop.clone());
                self.complete = true;
            }
            ProviderStreamEvent::ProviderMetadata(_) | ProviderStreamEvent::Error(_) => {}
        }
    }

    /// 当前 partial 快照（流式中途可渲染）。
    pub fn partial(&self) -> PartialMessage {
        PartialMessage {
            text: self.text.clone(),
            thinking: self.thinking.clone(),
            tool_calls: self
                .tool_order
                .iter()
                .filter_map(|id| self.tools.get(id).cloned())
                .collect(),
            usage: self.usage.clone(),
            stop_reason: self.stop_reason.clone(),
            complete: self.complete,
        }
    }

    /// 组装为最终的领域消息（assistant 角色）。
    ///
    /// - tool arguments 用 [`PartialJson`] 增量修复解析为 JSON Value；
    /// - 若无任何内容且无 stop reason，返回 [`ProviderErrorKind::StreamInterrupted`]。
    pub fn finalize(self) -> Result<Message, ProviderError> {
        let mut content: Vec<ContentPart> = Vec::new();

        if !self.thinking.is_empty() {
            content.push(ContentPart::Thinking(ThinkingContent {
                text: self.thinking,
                signature: None,
                redacted: false,
            }));
        }
        if !self.text.is_empty() {
            content.push(ContentPart::Text(TextContent { text: self.text }));
        }

        for id in &self.tool_order {
            if let Some(tool) = self.tools.get(id) {
                let arguments = parse_tool_arguments(&tool.arguments_json);
                content.push(ContentPart::ToolCall(ToolCallContent {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    arguments,
                    raw_arguments: Some(tool.arguments_json.clone()),
                    complete: tool.complete,
                }));
            }
        }

        // 空内容且无 stop reason → 流提前断开
        if content.is_empty() && self.stop_reason.is_none() && !self.complete {
            return Err(ProviderError::new(
                ProviderErrorKind::StreamInterrupted,
                "stream ended without any content or completion",
            ));
        }

        Ok(Message {
            id: MessageId::new("assistant"),
            role: MessageRole::Assistant,
            content,
            metadata: MessageMetadata {
                usage: if self.usage == TokenUsage::default() {
                    None
                } else {
                    Some(self.usage)
                },
                stop_reason: self.stop_reason,
                incomplete: !self.complete,
                ..MessageMetadata::default()
            },
        })
    }
}

/// 解析 tool arguments：先尝试完整解析，失败则用 PartialJson 修复。
fn parse_tool_arguments(raw: &str) -> serde_json::Value {
    if raw.is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        return value;
    }
    let mut repair = PartialJson::new();
    repair.push(raw);
    repair.parse_repaired().unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_api::ProviderStreamEvent;

    fn apply_all(events: &[ProviderStreamEvent]) -> StreamAssembler {
        let mut assembler = StreamAssembler::new();
        for event in events {
            assembler.apply(event);
        }
        assembler
    }

    #[test]
    fn text_stream_assembles_to_message() {
        let events = [
            ProviderStreamEvent::ResponseStarted {
                response_id: Some("resp-1".into()),
            },
            ProviderStreamEvent::TextDelta("Hello ".into()),
            ProviderStreamEvent::TextDelta("world".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ];
        let assembler = apply_all(&events);
        let partial = assembler.partial();
        assert_eq!(partial.text, "Hello world");
        assert!(partial.complete);

        let mut assembler = StreamAssembler::new();
        for e in &events {
            assembler.apply(e);
        }
        let message = assembler.finalize().expect("finalize");
        assert_eq!(message.role, MessageRole::Assistant);
        assert_eq!(message.content.len(), 1);
        assert!(matches!(
            &message.content[0],
            ContentPart::Text(TextContent { text }) if text == "Hello world"
        ));
        assert_eq!(message.metadata.stop_reason, Some(StopReason::Completed));
        assert!(!message.metadata.incomplete);
    }

    #[test]
    fn parallel_tool_calls_assemble_in_order() {
        let id_a = ToolCallId::new("call-a");
        let id_b = ToolCallId::new("call-b");
        let events = [
            ProviderStreamEvent::ToolCallStarted {
                id: id_a.clone(),
                name: "read".into(),
            },
            ProviderStreamEvent::ToolCallArgumentsDelta {
                id: id_a.clone(),
                json: "{\"path\":".into(),
            },
            ProviderStreamEvent::ToolCallStarted {
                id: id_b.clone(),
                name: "write".into(),
            },
            ProviderStreamEvent::ToolCallArgumentsDelta {
                id: id_b.clone(),
                json: "{\"line\":1}".into(),
            },
            ProviderStreamEvent::ToolCallArgumentsDelta {
                id: id_a.clone(),
                json: "\"a.txt\"}".into(),
            },
            ProviderStreamEvent::ToolCallCompleted { id: id_a.clone() },
            ProviderStreamEvent::ToolCallCompleted { id: id_b.clone() },
            ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse),
        ];
        let message = apply_all(&events).finalize().expect("finalize");

        let tool_calls: Vec<_> = message
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, id_a);
        assert_eq!(tool_calls[0].name, "read");
        assert_eq!(
            tool_calls[0].arguments,
            serde_json::json!({"path": "a.txt"})
        );
        assert_eq!(tool_calls[1].id, id_b);
        assert_eq!(tool_calls[1].arguments, serde_json::json!({"line": 1}));
        assert!(tool_calls.iter().all(|c| c.complete));
        assert_eq!(message.metadata.stop_reason, Some(StopReason::ToolUse));
    }

    #[test]
    fn thinking_and_text_mixed() {
        let events = [
            ProviderStreamEvent::ThinkingDelta("hmm".into()),
            ProviderStreamEvent::TextDelta("answer".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ];
        let message = apply_all(&events).finalize().expect("finalize");
        assert_eq!(message.content.len(), 2);
        assert!(matches!(message.content[0], ContentPart::Thinking(_)));
        assert!(matches!(message.content[1], ContentPart::Text(_)));
    }

    #[test]
    fn partial_snapshot_available_mid_stream() {
        let mut assembler = StreamAssembler::new();
        assembler.apply(&ProviderStreamEvent::TextDelta("partial".into()));
        assembler.apply(&ProviderStreamEvent::ToolCallStarted {
            id: ToolCallId::new("c1"),
            name: "x".into(),
        });
        let partial = assembler.partial();
        assert_eq!(partial.text, "partial");
        assert_eq!(partial.tool_calls.len(), 1);
        assert!(!partial.complete);
    }

    #[test]
    fn empty_stream_without_completion_is_interrupted() {
        let assembler = StreamAssembler::new();
        let error = assembler.finalize().expect_err("空流应报错");
        assert_eq!(error.kind, ProviderErrorKind::StreamInterrupted);
    }
}
