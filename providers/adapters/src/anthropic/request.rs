//! canonical 请求 → Anthropic Messages 请求体的转换（S2 基线）。
//!
//! 不写 `cache_control` / `thinking`；`prompt_cache` / `thinking` / `hosted_tools` 忽略。
//! 连续 [`MessageRole::Tool`](pawork_domain::MessageRole::Tool) 合并为一条 user，
//! 且同一条 user 里 `tool_result` 块排在最前。

use pawork_api::{CanonicalModelRequest, ResponseFormat, ToolChoice};
use pawork_domain::{ContentPart, ImageSource, Message, MessageRole};
use serde_json::{json, Map, Value};

const DEFAULT_MAX_TOKENS: u64 = 4096;

/// 把 canonical 请求转换为 Anthropic Messages 请求体。
pub fn to_messages_body(request: &CanonicalModelRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));

    let max_tokens = request
        .max_output_tokens
        .unwrap_or(DEFAULT_MAX_TOKENS)
        .max(2);
    body.insert("max_tokens".into(), json!(max_tokens));
    body.insert("stream".into(), Value::Bool(true));

    let mut system_blocks = Vec::new();
    let mut conversation = Vec::new();
    for message in &request.messages {
        if message.role == MessageRole::System {
            for part in &message.content {
                if let ContentPart::Text(text) = part {
                    system_blocks.push(json!({"type":"text","text": text.text}));
                }
            }
        } else {
            conversation.push(message);
        }
    }
    if let Some(instruction) = structured_output_instruction(&request.response_format) {
        system_blocks.push(json!({"type":"text","text": instruction}));
    }
    if system_blocks.len() == 1 {
        body.insert(
            "system".into(),
            system_blocks.pop().expect("single system block exists"),
        );
    } else if !system_blocks.is_empty() {
        body.insert("system".into(), Value::Array(system_blocks));
    }

    let mut out_messages = Vec::new();
    let mut pending_tool_blocks: Vec<Value> = Vec::new();
    for message in conversation {
        if message.role == MessageRole::Tool {
            pending_tool_blocks.extend(content_blocks(message));
            continue;
        }
        flush_pending_tool_user(&mut pending_tool_blocks, &mut out_messages);
        out_messages.extend(message_to_anthropic(message));
    }
    flush_pending_tool_user(&mut pending_tool_blocks, &mut out_messages);
    body.insert("messages".into(), Value::Array(out_messages));

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
                    .map(|sequence| Value::String(sequence.clone()))
                    .collect(),
            ),
        );
    }

    for (key, value) in &request.provider_options {
        if is_reserved_provider_option(key) {
            tracing::warn!(
                provider_option = %key,
                "ignored reserved Anthropic provider option"
            );
            continue;
        }
        body.insert(key.clone(), value.clone());
    }

    Value::Object(body)
}

fn is_reserved_provider_option(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "model"
            | "messages"
            | "max_tokens"
            | "stream"
            | "stream_options"
            | "system"
            | "tools"
            | "tool_choice"
            | "thinking"
            | "output_config"
            | "reasoning_effort"
            | "reasoning"
            | "effort"
            | "temperature"
            | "stop_sequences"
            | "authorization"
            | "proxy-authorization"
            | "api_key"
            | "api-key"
            | "x-api-key"
    )
}

/// 把 pawork-domain Message 转为 Anthropic message(s)。
fn message_to_anthropic(message: &Message) -> Vec<Value> {
    let role = match message.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System | MessageRole::Tool => "user",
    };

    let mut blocks = content_blocks(message);
    if blocks.is_empty() {
        blocks.push(json!({"type":"text","text":""}));
    }

    vec![json!({"role": role, "content": blocks})]
}

fn content_blocks(message: &Message) -> Vec<Value> {
    let mut tool_results = Vec::new();
    let mut others = Vec::new();

    for part in &message.content {
        match part {
            ContentPart::Text(text) => {
                others.push(json!({"type":"text","text": text.text}));
            }
            ContentPart::Thinking(_) | ContentPart::Reasoning(_) | ContentPart::ArtifactRef(_) => {}
            ContentPart::ToolCall(call) => {
                let input = if call.arguments.is_null() {
                    call.raw_arguments
                        .as_deref()
                        .and_then(|raw| serde_json::from_str(raw).ok())
                        .unwrap_or(json!({}))
                } else {
                    call.arguments.clone()
                };
                others.push(json!({
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
                    .filter_map(|part| match part {
                        ContentPart::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                tool_results.push(json!({
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
                if let Some(block) = block {
                    others.push(block);
                }
            }
        }
    }

    tool_results.extend(others);
    tool_results
}

fn flush_pending_tool_user(pending: &mut Vec<Value>, out: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let blocks = std::mem::take(pending);
    let (mut tool_results, others): (Vec<_>, Vec<_>) = blocks.into_iter().partition(|block| {
        block.get("type").and_then(Value::as_str) == Some("tool_result")
    });
    tool_results.extend(others);
    out.push(json!({"role": "user", "content": tool_results}));
}

fn structured_output_instruction(format: &ResponseFormat) -> Option<String> {
    match format {
        ResponseFormat::Text => None,
        ResponseFormat::Json => Some(
            "Return only one valid JSON value. Do not include Markdown fences or explanatory text."
                .to_string(),
        ),
        ResponseFormat::JsonSchema { name, schema } => Some(format!(
            "Return only one valid JSON value that conforms to the JSON Schema named `{name}`. Do not include Markdown fences or explanatory text. JSON Schema: {}",
            serde_json::to_string(schema).expect("serde_json::Value always serializes")
        )),
    }
}

fn tool_choice_to_anthropic(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::None => json!({"type":"none"}),
        ToolChoice::Auto => json!({"type":"auto"}),
        ToolChoice::Required => json!({"type":"any"}),
        ToolChoice::Named(name) => json!({"type":"tool","name": name}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_api::{PromptCachePreference, RequestBudget, ThinkingConfig, ThinkingLevel, ToolDefinition};
    use pawork_domain::{
        ImageContent, MessageId, MessageMetadata, TextContent, ToolCallContent, ToolCallId,
        ToolResultContent,
    };
    use std::collections::BTreeMap;

    fn user(text: &str) -> Message {
        Message {
            id: MessageId::new("m1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: text.into() })],
            metadata: MessageMetadata::default(),
        }
    }

    fn base_request() -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: pawork_domain::RequestId::from("r1"),
            model: pawork_domain::ModelId::from("claude-3-5-sonnet"),
            messages: vec![user("hi")],
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
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
            reasoning: None,
        }
    }

    fn tool_result_message(id: &str, call_id: &str, body: &str) -> Message {
        Message {
            id: MessageId::new(id),
            role: MessageRole::Tool,
            content: vec![ContentPart::ToolResult(ToolResultContent {
                tool_call_id: ToolCallId::from(call_id),
                tool_name: Some("read_file".into()),
                content: vec![ContentPart::Text(TextContent {
                    text: body.into(),
                })],
                is_error: false,
                metadata: Value::Null,
                artifacts: Vec::new(),
            })],
            metadata: MessageMetadata::default(),
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
        assert_eq!(body["stop_sequences"], json!(["END"]));
        assert!(body.get("thinking").is_none());
        assert!(!body.to_string().contains("cache_control"));
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
        assert_eq!(body["system"]["text"], "be helpful");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn thinking_and_prompt_cache_are_not_written() {
        let mut req = base_request();
        req.prompt_cache = PromptCachePreference::Required;
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(64),
        });
        req.messages.insert(
            0,
            Message {
                id: MessageId::new("sys"),
                role: MessageRole::System,
                content: vec![ContentPart::Text(TextContent { text: "sys".into() })],
                metadata: MessageMetadata::default(),
            },
        );
        req.tools.push(ToolDefinition {
            name: "read_file".into(),
            description: "read".into(),
            input_schema: json!({"type":"object"}),
        });
        let body = to_messages_body(&req);
        assert!(body.get("thinking").is_none());
        assert!(!body.to_string().contains("cache_control"));
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
        assert_eq!(body["tools"][0]["description"], "read");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert!(body["tools"][0].get("cache_control").is_none());
        assert_eq!(body["tool_choice"]["type"], "any");
    }

    #[test]
    fn tool_choice_variants_map() {
        let mut req = base_request();
        req.tools.push(ToolDefinition {
            name: "read_file".into(),
            description: "read".into(),
            input_schema: json!({"type":"object"}),
        });

        req.tool_choice = ToolChoice::None;
        assert_eq!(to_messages_body(&req)["tool_choice"]["type"], "none");
        req.tool_choice = ToolChoice::Auto;
        assert_eq!(to_messages_body(&req)["tool_choice"]["type"], "auto");
        req.tool_choice = ToolChoice::Named("read_file".into());
        let named = to_messages_body(&req)["tool_choice"].clone();
        assert_eq!(named["type"], "tool");
        assert_eq!(named["name"], "read_file");
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
        req.messages
            .push(tool_result_message("t1", "call-1", "body"));
        let body = to_messages_body(&req);
        let tool_msg = &body["messages"][1];
        assert_eq!(tool_msg["role"], "user");
        assert_eq!(tool_msg["content"][0]["type"], "tool_result");
        assert_eq!(tool_msg["content"][0]["tool_use_id"], "call-1");
        assert_eq!(tool_msg["content"][0]["content"], "body");
    }

    #[test]
    fn consecutive_tool_messages_merge_into_one_user() {
        let mut req = base_request();
        req.messages
            .push(tool_result_message("t1", "call-1", "first"));
        req.messages
            .push(tool_result_message("t2", "call-2", "second"));
        req.messages.push(user("follow-up"));
        let body = to_messages_body(&req);
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"].as_array().unwrap().len(), 2);
        assert_eq!(messages[1]["content"][0]["tool_use_id"], "call-1");
        assert_eq!(messages[1]["content"][1]["tool_use_id"], "call-2");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["text"], "follow-up");
    }

    #[test]
    fn tool_result_blocks_come_first_in_user_message() {
        let mut req = base_request();
        req.messages.push(Message {
            id: MessageId::new("u2"),
            role: MessageRole::User,
            content: vec![
                ContentPart::Text(TextContent {
                    text: "follow-up".into(),
                }),
                ContentPart::ToolResult(ToolResultContent {
                    tool_call_id: ToolCallId::from("call-1"),
                    tool_name: Some("read_file".into()),
                    content: vec![ContentPart::Text(TextContent {
                        text: "body".into(),
                    })],
                    is_error: false,
                    metadata: Value::Null,
                    artifacts: Vec::new(),
                }),
            ],
            metadata: MessageMetadata::default(),
        });
        let body = to_messages_body(&req);
        let content = &body["messages"][1]["content"];
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "follow-up");
    }

    #[test]
    fn provider_options_cannot_override_reserved() {
        let mut req = base_request();
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(64),
        });
        req.provider_options.insert("top_p".into(), json!(0.9));
        req.provider_options
            .insert("model".into(), json!("attacker-model"));
        req.provider_options.insert("messages".into(), json!([]));
        req.provider_options.insert("max_tokens".into(), json!(1));
        req.provider_options.insert(
            "thinking".into(),
            json!({"type":"enabled","budget_tokens": 999_999}),
        );
        req.provider_options.insert("temperature".into(), json!(99));
        req.provider_options
            .insert("stop_sequences".into(), json!(["ATTACK"]));
        req.provider_options
            .insert("x-api-key".into(), json!("sk-leaked"));
        req.provider_options
            .insert("reasoning_effort".into(), json!("low"));

        let body = to_messages_body(&req);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["model"], "claude-3-5-sonnet");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["max_tokens"], 128);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("x-api-key").is_none());
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["stop_sequences"], json!(["END"]));
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
    fn structured_output_injects_json_schema_system_instruction() {
        let mut req = base_request();
        req.response_format = ResponseFormat::JsonSchema {
            name: "answer".into(),
            schema: json!({"type":"object","required":["ok"]}),
        };
        let body = to_messages_body(&req);
        let system = body["system"]["text"].as_str().expect("system text");
        assert!(system.contains("JSON Schema named `answer`"));
        assert!(system.contains("\"required\":[\"ok\"]"));
    }
}
