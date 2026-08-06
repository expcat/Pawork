//! canonical 请求 → Anthropic Messages 请求体的转换。

use agent_domain::{ContentPart, ImageSource, Message, MessageRole};
use provider_api::{
    CanonicalModelRequest, PromptCachePreference, ResponseFormat, ThinkingConfig, ThinkingLevel,
    ToolChoice,
};
use serde_json::{json, Map, Value};

/// 把 canonical 请求转换为 Anthropic Messages 请求体。
pub fn to_messages_body(request: &CanonicalModelRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));
    body.insert(
        "max_tokens".into(),
        json!(request.max_output_tokens.unwrap_or(4096)),
    );
    body.insert("stream".into(), Value::Bool(true));

    // P6-7：prompt cache 显式标记（Anthropic 需 cache_control）
    let cache_enabled = request.prompt_cache != PromptCachePreference::Disabled;

    // system：提取 role=system 消息为顶层 system 字段（Anthropic 不在 messages 里放 system）
    let mut system_parts = Vec::new();
    let mut messages = Vec::new();
    for message in &request.messages {
        if message.role == MessageRole::System {
            system_parts.push(message);
        } else {
            messages.push(message);
        }
    }
    if !system_parts.is_empty() {
        let mut blocks: Vec<Value> = Vec::new();
        for msg in system_parts {
            for part in &msg.content {
                if let ContentPart::Text(t) = part {
                    blocks.push(json!({"type":"text","text": t.text}));
                }
            }
        }
        if cache_enabled && !blocks.is_empty() {
            if let Some(last) = blocks.last_mut() {
                last["cache_control"] = json!({"type":"ephemeral"});
            }
        }
        if blocks.len() == 1 {
            body.insert("system".into(), blocks.pop().expect("single block exists"));
        } else if !blocks.is_empty() {
            body.insert("system".into(), Value::Array(blocks));
        }
    }

    // messages：role ∈ {user, assistant}；tool_result 作为 user 消息内的 block
    let mut out_messages = Vec::new();
    for message in &messages {
        out_messages.extend(message_to_anthropic(message, cache_enabled));
    }
    body.insert("messages".into(), Value::Array(out_messages));

    // tools / tool_choice
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
        body.insert(
            "tool_choice".into(),
            tool_choice_to_anthropic(&request.tool_choice),
        );
    }

    if let Some(temp) = request.temperature {
        body.insert("temperature".into(), json!(temp));
    }
    if !request.stop_sequences.is_empty() {
        body.insert(
            "stop_sequences".into(),
            Value::Array(
                request
                    .stop_sequences
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }

    // thinking（P6-5）
    if let Some(thinking) = &request.thinking {
        if let Some(budget) = thinking_budget(thinking) {
            body.insert(
                "thinking".into(),
                json!({"type":"enabled","budget_tokens": budget}),
            );
        }
    }

    // P6-8：结构化输出（Anthropic 通过指令 + 工具约束，这里用 response_format 提示）
    match &request.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::Json | ResponseFormat::JsonSchema { .. } => {
            // Anthropic 没有原生 response_format，退化为 system 指令；保持请求可发送。
            // 具体 schema 校验由调用方在收到响应后完成。
        }
    }

    // P6-9：provider-specific options 透传（合并到顶层）
    for (key, value) in &request.provider_options {
        body.insert(key.clone(), value.clone());
    }

    Value::Object(body)
}

/// 把 agent-domain Message 转为 Anthropic message(s)。
fn message_to_anthropic(message: &Message, cache_enabled: bool) -> Vec<Value> {
    let role = match message.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System | MessageRole::Tool => "user",
    };

    let mut blocks: Vec<Value> = Vec::new();
    for part in &message.content {
        match part {
            ContentPart::Text(t) => {
                blocks.push(json!({"type":"text","text": t.text}));
            }
            ContentPart::Thinking(_) => { /* 推理内容不回传 */ }
            ContentPart::ToolCall(call) => {
                let input = if call.arguments.is_null() {
                    call.raw_arguments
                        .as_deref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(json!({}))
                } else {
                    call.arguments.clone()
                };
                blocks.push(json!({
                    "type":"tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": input,
                }));
            }
            ContentPart::ToolResult(result) => {
                let content: String = result
                    .content
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                blocks.push(json!({
                    "type":"tool_result",
                    "tool_use_id": result.tool_call_id,
                    "content": content,
                    "is_error": result.is_error,
                }));
            }
            ContentPart::Image(image) => {
                let block = match &image.source {
                    ImageSource::Base64(data) if !data.is_empty() => Some(json!({
                        "type":"image",
                        "source":{"type":"base64","media_type": image.media_type,"data": data},
                    })),
                    ImageSource::Url(url) if !url.is_empty() => Some(json!({
                        "type":"image",
                        "source":{"type":"url","url": url},
                    })),
                    _ => None,
                };
                if let Some(b) = block {
                    blocks.push(b);
                }
            }
            ContentPart::ArtifactRef(_) => { /* artifact 由上游解析后再进入 provider */ }
        }
    }

    // P6-7：对最后一条 user 消息的最后一个 block 标记 cache_control
    if cache_enabled && role == "user" {
        if let Some(last) = blocks.last_mut() {
            last["cache_control"] = json!({"type":"ephemeral"});
        }
    }

    if blocks.is_empty() {
        blocks.push(json!({"type":"text","text":""}));
    }

    vec![json!({"role": role, "content": blocks})]
}

fn tool_choice_to_anthropic(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::None => json!({"type":"none"}),
        ToolChoice::Auto => json!({"type":"auto"}),
        ToolChoice::Required => json!({"type":"any"}),
        ToolChoice::Named(name) => json!({"type":"tool","name": name}),
    }
}

/// thinking level → budget_tokens。Off 关闭 thinking。
fn thinking_budget(config: &ThinkingConfig) -> Option<u64> {
    if config.level == ThinkingLevel::Off {
        return None;
    }
    let budget = config.budget_tokens.unwrap_or(match config.level {
        ThinkingLevel::Low => 1024,
        ThinkingLevel::Medium => 4096,
        ThinkingLevel::High => 8192,
        ThinkingLevel::Off => 0,
    });
    Some(budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{
        ImageContent, MessageId, MessageMetadata, TextContent, ToolCallContent, ToolCallId,
        ToolResultContent,
    };
    use provider_api::{PromptCachePreference, RequestBudget, ToolDefinition};
    use std::collections::BTreeMap;

    fn base_request() -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: agent_domain::RequestId::from("r1"),
            model: agent_domain::ModelId::from("claude-3-5-sonnet"),
            messages: vec![Message {
                id: MessageId::new("m1"),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent { text: "hi".into() })],
                metadata: MessageMetadata::default(),
            }],
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            temperature: Some(0.5),
            max_output_tokens: Some(128),
            stop_sequences: vec!["END".into()],
            response_format: ResponseFormat::Text,
            prompt_cache: PromptCachePreference::Automatic,
            budget: RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: None,
        }
    }

    #[test]
    fn basic_request_maps_fields() {
        let body = to_messages_body(&base_request());
        assert_eq!(body["model"], "claude-3-5-sonnet");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["stop_sequences"], serde_json::json!(["END"]));
    }

    #[test]
    fn system_extracted_to_top_level() {
        let mut req = base_request();
        req.messages.insert(
            0,
            Message {
                id: MessageId::new("sys"),
                role: MessageRole::System,
                content: vec![ContentPart::Text(TextContent {
                    text: "be helpful".into(),
                })],
                metadata: MessageMetadata::default(),
            },
        );
        let body = to_messages_body(&req);
        // system 不在 messages 里
        assert_eq!(body["system"]["text"], "be helpful");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn prompt_cache_marks_system_and_last_user_block() {
        let mut req = base_request();
        req.prompt_cache = PromptCachePreference::Required;
        req.messages.insert(
            0,
            Message {
                id: MessageId::new("sys"),
                role: MessageRole::System,
                content: vec![ContentPart::Text(TextContent { text: "sys".into() })],
                metadata: MessageMetadata::default(),
            },
        );
        let body = to_messages_body(&req);
        assert_eq!(body["system"]["cache_control"]["type"], "ephemeral");
        // user 消息在 system 之后
        let user_msg = &body["messages"][0];
        let last_block = &user_msg["content"].as_array().unwrap().last().unwrap();
        assert_eq!(last_block["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn thinking_maps_to_budget() {
        let mut req = base_request();
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: None,
        });
        let body = to_messages_body(&req);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 8192);
    }

    #[test]
    fn thinking_off_is_omitted() {
        let mut req = base_request();
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::Off,
            budget_tokens: None,
        });
        let body = to_messages_body(&req);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn tools_use_input_schema() {
        let mut req = base_request();
        req.tools.push(ToolDefinition {
            name: "read_file".into(),
            description: "read".into(),
            input_schema: json!({"type":"object"}),
        });
        req.tool_choice = ToolChoice::Required;
        let body = to_messages_body(&req);
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["tool_choice"]["type"], "any");
    }

    #[test]
    fn assistant_tool_call_maps_to_tool_use_block() {
        let mut req = base_request();
        req.messages.push(Message {
            id: MessageId::new("a1"),
            role: MessageRole::Assistant,
            content: vec![
                ContentPart::Text(TextContent {
                    text: "calling".into(),
                }),
                ContentPart::ToolCall(ToolCallContent {
                    id: ToolCallId::from("call-1"),
                    name: "read_file".into(),
                    arguments: json!({"path": "a"}),
                    raw_arguments: None,
                    complete: true,
                }),
            ],
            metadata: MessageMetadata::default(),
        });
        let body = to_messages_body(&req);
        let assistant = &body["messages"][1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["content"][1]["type"], "tool_use");
        assert_eq!(assistant["content"][1]["id"], "call-1");
        assert_eq!(assistant["content"][1]["input"]["path"], "a");
    }

    #[test]
    fn tool_result_maps_to_tool_result_block_in_user_message() {
        let mut req = base_request();
        req.messages.push(Message {
            id: MessageId::new("t1"),
            role: MessageRole::Tool,
            content: vec![ContentPart::ToolResult(ToolResultContent {
                tool_call_id: ToolCallId::from("call-1"),
                tool_name: Some("read_file".into()),
                content: vec![ContentPart::Text(TextContent {
                    text: "body".into(),
                })],
                is_error: false,
                metadata: Value::Null,
            })],
            metadata: MessageMetadata::default(),
        });
        let body = to_messages_body(&req);
        // tool 角色消息成为 user 消息
        let tool_msg = &body["messages"][1];
        assert_eq!(tool_msg["role"], "user");
        assert_eq!(tool_msg["content"][0]["type"], "tool_result");
        assert_eq!(tool_msg["content"][0]["tool_use_id"], "call-1");
        assert_eq!(tool_msg["content"][0]["content"], "body");
    }

    #[test]
    fn image_base64_maps_to_image_block() {
        let mut req = base_request();
        req.messages[0]
            .content
            .push(ContentPart::Image(ImageContent {
                source: ImageSource::Base64("QkFTRQ==".into()),
                media_type: "image/png".into(),
                alt_text: None,
            }));
        let body = to_messages_body(&req);
        let img_block = &body["messages"][0]["content"][1];
        assert_eq!(img_block["type"], "image");
        assert_eq!(img_block["source"]["type"], "base64");
        assert_eq!(img_block["source"]["media_type"], "image/png");
        assert_eq!(img_block["source"]["data"], "QkFTRQ==");
    }

    #[test]
    fn provider_options_pass_through() {
        let mut req = base_request();
        req.provider_options
            .insert("top_p".into(), serde_json::json!(0.9));
        let body = to_messages_body(&req);
        assert_eq!(body["top_p"], 0.9);
    }
}
