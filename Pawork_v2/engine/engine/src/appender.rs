//! 流式增量组装：把 [`ProviderStreamEvent`] 累积成一条助手消息。
//!
//! 本模块只做组装，不分配 sequence、不落库、不执行工具。
//! `tool_results_message` 依赖 tool-api，留 S2。

use std::collections::BTreeMap;

use pawork_api::{ModelResponseSummary, ProviderStreamEvent};
use pawork_domain::{
    ContentPart, Message, MessageId, MessageMetadata, MessageRole, ReasoningItem, StopReason,
    TextContent, ThinkingContent, TokenUsage, ToolCallContent, ToolCallId,
};
use serde_json::Value;

/// 流式组装过程中累积的一组 tool call。
#[derive(Clone, Debug, Default)]
pub struct PendingToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub raw_arguments: String,
    pub completed: bool,
}

impl PendingToolCall {
    pub fn arguments(&self) -> Value {
        if self.raw_arguments.trim().is_empty() {
            return Value::Null;
        }
        serde_json::from_str(&self.raw_arguments).unwrap_or(Value::Null)
    }

    pub fn into_content(self) -> ToolCallContent {
        let arguments = self.arguments();
        ToolCallContent {
            id: self.id,
            name: self.name,
            arguments,
            raw_arguments: Some(self.raw_arguments),
            complete: self.completed,
        }
    }
}

/// 一轮 Provider 流式调用累积出的结果。
#[derive(Clone, Debug, Default)]
pub struct AssembledTurn {
    pub message_id: MessageId,
    pub text: String,
    pub thinking: String,
    pub reasoning_items: Vec<ReasoningItem>,
    pub tool_calls: BTreeMap<ToolCallId, PendingToolCall>,
    pub tool_call_order: Vec<ToolCallId>,
    pub summary: Option<ModelResponseSummary>,
}

impl AssembledTurn {
    pub fn new(message_id: MessageId) -> Self {
        Self {
            message_id,
            ..Default::default()
        }
    }

    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    pub fn apply(&mut self, event: &ProviderStreamEvent) {
        match event {
            ProviderStreamEvent::TextDelta(delta) => {
                self.text.push_str(delta);
            }
            ProviderStreamEvent::ThinkingDelta(delta) => {
                self.thinking.push_str(delta);
            }
            ProviderStreamEvent::ReasoningItem(item) => {
                self.reasoning_items.push(item.clone());
            }
            ProviderStreamEvent::ToolCallStarted { id, name } => {
                if !self.tool_calls.contains_key(id) {
                    self.tool_call_order.push(id.clone());
                    self.tool_calls.insert(
                        id.clone(),
                        PendingToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            raw_arguments: String::new(),
                            completed: false,
                        },
                    );
                }
            }
            ProviderStreamEvent::ToolCallArgumentsDelta { id, json } => {
                if let Some(call) = self.tool_calls.get_mut(id) {
                    call.raw_arguments.push_str(json);
                } else {
                    self.tool_call_order.push(id.clone());
                    self.tool_calls.insert(
                        id.clone(),
                        PendingToolCall {
                            id: id.clone(),
                            name: String::new(),
                            raw_arguments: json.clone(),
                            completed: false,
                        },
                    );
                }
            }
            ProviderStreamEvent::ToolCallCompleted { id } => {
                if let Some(call) = self.tool_calls.get_mut(id) {
                    call.completed = true;
                }
            }
            ProviderStreamEvent::UsageUpdated(usage) => {
                let summary = self.summary.get_or_insert(ModelResponseSummary {
                    stop_reason: StopReason::Completed,
                    usage: usage.clone(),
                    response_id: None,
                    provider_metadata: Value::Null,
                });
                summary.usage = usage.clone();
            }
            ProviderStreamEvent::ResponseStarted { response_id } => {
                if let Some(response_id) = response_id {
                    let summary = self.summary.get_or_insert(ModelResponseSummary {
                        stop_reason: StopReason::Completed,
                        usage: TokenUsage::default(),
                        response_id: None,
                        provider_metadata: Value::Null,
                    });
                    summary.response_id = Some(response_id.clone());
                }
            }
            ProviderStreamEvent::ResponseCompleted(stop_reason) => {
                let summary = self.summary.get_or_insert(ModelResponseSummary {
                    stop_reason: stop_reason.clone(),
                    usage: TokenUsage::default(),
                    response_id: None,
                    provider_metadata: Value::Null,
                });
                summary.stop_reason = stop_reason.clone();
            }
            ProviderStreamEvent::ProviderMetadata(metadata) => {
                let summary = self.summary.get_or_insert(ModelResponseSummary {
                    stop_reason: StopReason::Completed,
                    usage: TokenUsage::default(),
                    response_id: None,
                    provider_metadata: Value::Null,
                });
                summary.provider_metadata = metadata.clone();
            }
            ProviderStreamEvent::ServerTool(_)
            | ProviderStreamEvent::TranscriptEnvelope(_)
            | ProviderStreamEvent::Error(_) => {}
        }
    }

    pub fn into_message(self, metadata: MessageMetadata) -> Message {
        let mut content: Vec<ContentPart> = Vec::new();
        if !self.thinking.is_empty() {
            content.push(ContentPart::Thinking(ThinkingContent {
                text: self.thinking,
                reasoning_item_id: self.reasoning_items.last().map(|item| item.id.clone()),
                redacted: false,
            }));
        }
        content.extend(self.reasoning_items.into_iter().map(ContentPart::Reasoning));
        if !self.text.is_empty() {
            content.push(ContentPart::Text(TextContent { text: self.text }));
        }
        for id in &self.tool_call_order {
            if let Some(call) = self.tool_calls.get(id) {
                content.push(ContentPart::ToolCall(call.clone().into_content()));
            }
        }
        Message {
            id: self.message_id,
            role: MessageRole::Assistant,
            content,
            metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::{ProtectedBlobRef, ReasoningItemId};

    use super::*;

    #[test]
    fn assembles_text_and_tool_calls_in_order() {
        let mut turn = AssembledTurn::new(MessageId::from("assistant-1"));
        turn.apply(&ProviderStreamEvent::ResponseStarted {
            response_id: Some("resp-1".into()),
        });
        turn.apply(&ProviderStreamEvent::TextDelta("Hello ".into()));
        turn.apply(&ProviderStreamEvent::TextDelta("world".into()));
        turn.apply(&ProviderStreamEvent::ToolCallStarted {
            id: ToolCallId::from("call-1"),
            name: "read_file".into(),
        });
        turn.apply(&ProviderStreamEvent::ToolCallArgumentsDelta {
            id: ToolCallId::from("call-1"),
            json: r#"{"path": "#.to_string(),
        });
        turn.apply(&ProviderStreamEvent::ToolCallArgumentsDelta {
            id: ToolCallId::from("call-1"),
            json: r#""README.md"}"#.to_string(),
        });
        turn.apply(&ProviderStreamEvent::ToolCallCompleted {
            id: ToolCallId::from("call-1"),
        });
        turn.apply(&ProviderStreamEvent::UsageUpdated(TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        }));
        turn.apply(&ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse));

        assert_eq!(turn.text, "Hello world");
        assert!(turn.has_tool_calls());
        let summary = turn.summary.as_ref().unwrap();
        assert_eq!(summary.stop_reason, StopReason::ToolUse);
        assert_eq!(summary.usage.input_tokens, 10);

        let message = turn.into_message(MessageMetadata::default());
        assert_eq!(message.content.len(), 2);
        assert!(matches!(&message.content[0], ContentPart::Text(_)));
        assert!(matches!(&message.content[1], ContentPart::ToolCall(_)));
        if let ContentPart::ToolCall(tc) = &message.content[1] {
            assert_eq!(tc.name, "read_file");
            assert_eq!(tc.arguments, serde_json::json!({"path": "README.md"}));
            assert!(tc.complete);
        }
    }

    #[test]
    fn arguments_parse_returns_null_for_empty_or_invalid() {
        let call = PendingToolCall {
            id: ToolCallId::from("c"),
            name: "x".into(),
            raw_arguments: String::new(),
            completed: true,
        };
        assert_eq!(call.arguments(), Value::Null);
        let call = PendingToolCall {
            id: ToolCallId::from("c"),
            name: "x".into(),
            raw_arguments: "{bad".into(),
            completed: true,
        };
        assert_eq!(call.arguments(), Value::Null);
    }

    #[test]
    fn assembles_reasoning_items_as_safe_message_content() {
        let item = ReasoningItem {
            id: ReasoningItemId::from("reasoning-1"),
            summary: Some("safe summary".into()),
            protected_blob_ref: ProtectedBlobRef::from("protected-1"),
            opaque_metadata: BTreeMap::new(),
            continuation_metadata: BTreeMap::new(),
        };
        let mut turn = AssembledTurn::new(MessageId::from("assistant-reasoning"));
        turn.apply(&ProviderStreamEvent::ThinkingDelta(
            "visible thinking".into(),
        ));
        turn.apply(&ProviderStreamEvent::ReasoningItem(item.clone()));

        let message = turn.into_message(MessageMetadata::default());
        assert!(matches!(
            &message.content[0],
            ContentPart::Thinking(ThinkingContent {
                reasoning_item_id: Some(id),
                ..
            }) if id == &item.id
        ));
        assert_eq!(message.content[1], ContentPart::Reasoning(item));
    }
}
