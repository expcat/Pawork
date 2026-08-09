//! canonical 请求 → Anthropic Messages 请求体的转换。

use agent_domain::{ContentPart, ImageSource, Message, MessageRole};
use provider_api::{
    CanonicalModelRequest, PromptCachePreference, ResponseFormat, ThinkingConfig, ThinkingLevel,
    ToolChoice,
};
use serde_json::{json, Map, Value};

const DEFAULT_MAX_TOKENS: u64 = 4096;
const DEFAULT_THINKING_OUTPUT_MARGIN: u64 = 1024;

/// 把 canonical 请求转换为 Anthropic Messages 请求体。
pub fn to_messages_body(request: &CanonicalModelRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));

    let requested_thinking_budget = request.thinking.as_ref().and_then(thinking_budget);
    let mut max_tokens = request
        .max_output_tokens
        .unwrap_or(DEFAULT_MAX_TOKENS)
        .max(2);
    if request.max_output_tokens.is_none() {
        if let Some(budget) = requested_thinking_budget {
            max_tokens = max_tokens.max(budget.saturating_add(DEFAULT_THINKING_OUTPUT_MARGIN));
        }
    }
    body.insert("max_tokens".into(), json!(max_tokens));
    body.insert("stream".into(), Value::Bool(true));

    let cache_enabled = request.prompt_cache != PromptCachePreference::Disabled;

    // Anthropic 把 system 放在顶层。结构化输出没有原生 response_format，因此把
    // JSON/schema 约束作为明确的 system block 注入，不能静默丢弃。
    let mut system_blocks = Vec::new();
    let mut messages = Vec::new();
    for message in &request.messages {
        if message.role == MessageRole::System {
            for part in &message.content {
                if let ContentPart::Text(text) = part {
                    system_blocks.push(json!({"type":"text","text": text.text}));
                }
            }
        } else {
            messages.push(message);
        }
    }
    if let Some(instruction) = structured_output_instruction(&request.response_format) {
        system_blocks.push(json!({"type":"text","text": instruction}));
    }
    if cache_enabled {
        mark_last_block(&mut system_blocks);
    }
    if system_blocks.len() == 1 {
        body.insert(
            "system".into(),
            system_blocks.pop().expect("single system block exists"),
        );
    } else if !system_blocks.is_empty() {
        body.insert("system".into(), Value::Array(system_blocks));
    }

    // 只在首个稳定 user turn 上设置一个缓存断点，避免多轮对话每条 user 都累积
    // cache_control 并超过 Anthropic 的断点上限。
    let mut out_messages = Vec::new();
    for message in messages {
        out_messages.extend(message_to_anthropic(message));
    }
    if cache_enabled {
        if let Some(first_user) = out_messages
            .iter_mut()
            .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        {
            mark_message_last_block(first_user);
        }
    }
    body.insert("messages".into(), Value::Array(out_messages));

    if !request.tools.is_empty() {
        let mut tools: Vec<Value> = request
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
        if cache_enabled {
            if let Some(last) = tools.last_mut() {
                last["cache_control"] = json!({"type":"ephemeral"});
            }
        }
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

    if let Some(requested_budget) = requested_thinking_budget {
        let budget = requested_budget.min(max_tokens.saturating_sub(1));
        body.insert(
            "thinking".into(),
            json!({"type":"enabled","budget_tokens": budget}),
        );
    }

    // P6-9：provider-specific options 透传（合并到顶层）。
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
            | "temperature"
            | "stop_sequences"
            | "authorization"
            | "proxy-authorization"
            | "api_key"
            | "api-key"
            | "x-api-key"
    )
}

/// 把 agent-domain Message 转为 Anthropic message(s)。
fn message_to_anthropic(message: &Message) -> Vec<Value> {
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

    if blocks.is_empty() {
        blocks.push(json!({"type":"text","text":""}));
    }

    vec![json!({"role": role, "content": blocks})]
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

fn mark_last_block(blocks: &mut [Value]) {
    if let Some(last) = blocks.last_mut() {
        last["cache_control"] = json!({"type":"ephemeral"});
    }
}

fn mark_message_last_block(message: &mut Value) {
    if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
        mark_last_block(blocks);
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
    fn prompt_cache_marks_system_and_first_user_only() {
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
        req.messages.push(Message {
            id: MessageId::new("m2"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "follow-up".into(),
            })],
            metadata: MessageMetadata::default(),
        });
        let body = to_messages_body(&req);
        assert_eq!(body["system"]["cache_control"]["type"], "ephemeral");
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_none());
        assert_eq!(body.to_string().matches("cache_control").count(), 2);
    }

    #[test]
    fn thinking_budget_is_clamped_below_explicit_max_tokens() {
        let mut req = base_request();
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: None,
        });
        let body = to_messages_body(&req);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 127);
        assert!(
            body["thinking"]["budget_tokens"].as_u64().unwrap()
                < body["max_tokens"].as_u64().unwrap()
        );
    }

    #[test]
    fn thinking_without_explicit_max_lifts_default_output_budget() {
        let mut req = base_request();
        req.max_output_tokens = None;
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: None,
        });
        let body = to_messages_body(&req);
        assert_eq!(body["thinking"]["budget_tokens"], 8192);
        assert_eq!(body["max_tokens"], 9216);
        assert!(
            body["thinking"]["budget_tokens"].as_u64().unwrap()
                < body["max_tokens"].as_u64().unwrap()
        );
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
        assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
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
    fn provider_options_pass_through_but_cannot_override_reserved_fields() {
        let mut req = base_request();
        req.provider_options
            .insert("top_p".into(), serde_json::json!(0.9));
        req.provider_options
            .insert("model".into(), serde_json::json!("attacker-model"));
        req.provider_options
            .insert("messages".into(), serde_json::json!([]));
        req.provider_options
            .insert("max_tokens".into(), serde_json::json!(1));
        req.provider_options.insert(
            "thinking".into(),
            serde_json::json!({"type":"enabled","budget_tokens": 999_999}),
        );
        req.provider_options
            .insert("temperature".into(), serde_json::json!(99));
        req.provider_options
            .insert("stop_sequences".into(), serde_json::json!(["ATTACK"]));
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(64),
        });
        let body = to_messages_body(&req);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["model"], "claude-3-5-sonnet");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["thinking"]["budget_tokens"], 64);
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["stop_sequences"], serde_json::json!(["END"]));
    }
}
