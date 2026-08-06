//! 流式增量组装与消息落库（P3-3 组成部分）。
//!
//! 把 Provider 的流式事件增量累积成一条助手消息与若干 tool call，并在
//! `append` 时回调上层持久化。本模块只做「组装」，不直接写 SQLite——持久化
//! 由调用方通过 [`EventSink`] 回调完成，这样状态机、事件广播与 SessionStore
//! 可以解耦组合。

use std::collections::BTreeMap;

use agent_domain::ContentPart;
use agent_domain::Message;
use agent_domain::MessageId;
use agent_domain::MessageMetadata;
use agent_domain::StopReason;
use agent_domain::TextContent;
use agent_domain::ThinkingContent;
use agent_domain::TokenUsage;
use agent_domain::ToolCallContent;
use agent_domain::ToolCallId;
use provider_api::ModelResponseSummary;
use provider_api::ProviderStreamEvent;
use serde_json::Value;

/// 流式组装过程中累积的一组 tool call。
#[derive(Clone, Debug, Default)]
pub struct PendingToolCall {
    pub id: ToolCallId,
    pub name: String,
    /// 已收到的 JSON 增量（未解析）。
    pub raw_arguments: String,
    pub completed: bool,
}

impl PendingToolCall {
    /// 尝试把累积的 JSON 增量解析为结构化参数。
    ///
    /// 解析失败时返回 `Value::Null`（保留 `raw_arguments` 供回填），不抛错，
    /// 由调用方决定是否当作错误工具结果回填。
    pub fn arguments(&self) -> Value {
        if self.raw_arguments.trim().is_empty() {
            return Value::Null;
        }
        serde_json::from_str(&self.raw_arguments).unwrap_or(Value::Null)
    }

    /// 构建 [`ToolCallContent`]（complete 标记按是否 completed）。
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
    /// 助手文本（按到达顺序拼接）。
    pub text: String,
    /// 助手思考（thinking）文本。
    pub thinking: String,
    /// 本轮出现的 tool call（按 started 顺序）。
    pub tool_calls: BTreeMap<ToolCallId, PendingToolCall>,
    /// tool call 的到达顺序（便于保持稳定排序）。
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

    /// 是否收集到 tool call。
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// 应用一条 Provider 流式事件，累积到本轮结果。
    pub fn apply(&mut self, event: &ProviderStreamEvent) {
        match event {
            ProviderStreamEvent::TextDelta(delta) => {
                self.text.push_str(delta);
            }
            ProviderStreamEvent::ThinkingDelta(delta) => {
                self.thinking.push_str(delta);
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
                    // 增量先于 started 到达：创建占位条目。
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
            ProviderStreamEvent::Error(_) => {}
        }
    }

    /// 把累积结果构建为一条 [`Message`]（含文本、思考、tool call 内容块）。
    pub fn into_message(self, metadata: MessageMetadata) -> Message {
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
        for id in &self.tool_call_order {
            if let Some(call) = self.tool_calls.get(id) {
                content.push(ContentPart::ToolCall(call.clone().into_content()));
            }
        }
        Message {
            id: self.message_id,
            role: agent_domain::MessageRole::Assistant,
            content,
            metadata,
        }
    }
}

/// 一条 tool call 的最终结果（回填到消息流）。
#[derive(Clone, Debug)]
pub struct ToolCallResult {
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub arguments: Value,
    pub result: tool_api::ToolResult,
}

/// 把一组 tool call 结果构建为一条 `Tool` 角色消息。
pub fn tool_results_message(message_id: MessageId, results: Vec<ToolCallResult>) -> Message {
    let content = results
        .into_iter()
        .map(|r| {
            let is_error = r.result.is_error();
            ContentPart::ToolResult(agent_domain::ToolResultContent {
                tool_call_id: r.tool_call_id,
                tool_name: Some(r.tool_name),
                content: r.result.content,
                is_error,
                metadata: r.result.metadata,
            })
        })
        .collect();
    Message {
        id: message_id,
        role: agent_domain::MessageRole::Tool,
        content,
        metadata: MessageMetadata::default(),
    }
}

#[cfg(test)]
mod tests {
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
        // 分两个合法增量：`{"path": ` 与 `"README.md"}`
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
    fn tool_results_message_builds_tool_role_parts() {
        let results = vec![ToolCallResult {
            tool_call_id: ToolCallId::from("call-1"),
            tool_name: "read_file".into(),
            arguments: Value::Null,
            result: tool_api::ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "body".into(),
            })]),
        }];
        let message = tool_results_message(MessageId::from("tool-msg"), results);
        assert_eq!(message.role, agent_domain::MessageRole::Tool);
        assert_eq!(message.content.len(), 1);
    }
}
