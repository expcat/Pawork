//! canonical 请求 → OpenAI Chat Completions 请求体的转换。

use provider_api::{
    CanonicalModelRequest, ResponseFormat, ThinkingConfig, ThinkingLevel, ToolChoice,
};
use serde_json::{json, Map, Value};

/// 把 canonical 请求转换为 OpenAI Chat Completions 请求体。
///
/// 一个适配同时覆盖云端 OpenAI 兼容接口与多数本地服务（Ollama / vLLM / LM Studio）。
pub fn to_chat_completions_body(request: &CanonicalModelRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));
    body.insert("stream".into(), Value::Bool(true));
    body.insert("stream_options".into(), json!({ "include_usage": true }));

    // messages
    let mut messages = Vec::new();
    for message in &request.messages {
        messages.extend(message_to_openai(message));
    }
    body.insert("messages".into(), Value::Array(messages));

    // tools / tool_choice
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.input_schema,
                    }
                })
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
        body.insert(
            "tool_choice".into(),
            tool_choice_to_openai(&request.tool_choice),
        );
    }

    if let Some(temp) = request.temperature {
        body.insert("temperature".into(), json!(temp));
    }
    if let Some(max_tokens) = request.max_output_tokens {
        body.insert("max_tokens".into(), json!(max_tokens));
    }
    if !request.stop_sequences.is_empty() {
        body.insert(
            "stop".into(),
            Value::Array(
                request
                    .stop_sequences
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }

    // response_format
    match &request.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::Json => {
            body.insert("response_format".into(), json!({"type": "json_object"}));
        }
        ResponseFormat::JsonSchema { name, schema } => {
            body.insert(
                "response_format".into(),
                json!({
                    "type": "json_schema",
                    "json_schema": { "name": name, "schema": schema }
                }),
            );
        }
    }

    // reasoning_effort（OpenAI o 系 / 本地兼容服务可能忽略）
    if let Some(thinking) = &request.thinking {
        if let Some(effort) = thinking_effort(thinking) {
            body.insert("reasoning_effort".into(), Value::String(effort));
        }
    }

    // provider-specific options 透传（P6-9）：把 provider_options 合并进请求体顶层，
    // 让 provider 专属参数（top_p / seed / service_tier 等）直达远端。
    // canonical 关键字段与认证字段属于保留键，不允许 provider_options 覆盖。
    for (key, value) in &request.provider_options {
        if is_reserved_provider_option(key) {
            tracing::warn!(
                provider_option = %key,
                "ignored reserved OpenAI-compatible provider option"
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
            | "stream"
            | "stream_options"
            | "tools"
            | "tool_choice"
            | "reasoning_effort"
            | "reasoning"
            | "effort"
            | "authorization"
            | "proxy-authorization"
            | "api_key"
            | "api-key"
            | "x-api-key"
    )
}

/// 把 agent-domain Message 转为 OpenAI message(s)。
/// tool_result 会展开为一条 role=tool 的消息；其余聚合成单条消息。
fn message_to_openai(message: &agent_domain::Message) -> Vec<Value> {
    use agent_domain::{ContentPart, MessageRole};

    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };

    // 先把 tool_result 单独抽出（OpenAI 要求 role=tool + tool_call_id）
    let mut out = Vec::new();
    let mut tool_calls = Vec::new();
    // 按 message 内顺序收集 text / image 内容片段；无图片时退化为纯字符串。
    let mut ordered_parts: Vec<Value> = Vec::new();
    let mut has_image = false;

    for part in &message.content {
        match part {
            ContentPart::Text(t) => {
                ordered_parts.push(json!({"type":"text","text": t.text.clone()}))
            }
            ContentPart::Thinking(_) => { /* 推理内容不回传给 provider */ }
            // Chat Completions has no canonical encrypted reasoning item input;
            // Responses adapters resolve this protected ref on the modern path.
            ContentPart::Reasoning(_) => {}
            ContentPart::ToolCall(call) => {
                let args = if call.arguments.is_null() {
                    call.raw_arguments.clone().unwrap_or_default()
                } else {
                    call.arguments.to_string()
                };
                tool_calls.push(json!({
                    "id": call.id,
                    "type": "function",
                    "function": { "name": call.name, "arguments": args }
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
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": result.tool_call_id,
                    "content": content,
                }));
            }
            ContentPart::Image(image) => {
                if let Some(url) = image_to_openai_url(image) {
                    has_image = true;
                    ordered_parts.push(json!({"type":"image_url","image_url":{"url": url}}));
                }
            }
            ContentPart::ArtifactRef(_) => {
                // artifact 由 context-engine 解析为 base64/url 后再进入 provider，此处跳过
            }
        }
    }

    // 主消息（若还有文本或 tool_calls）
    let mut main = Map::new();
    main.insert("role".into(), Value::String(role.into()));
    if has_image {
        main.insert("content".into(), Value::Array(ordered_parts));
    } else if !ordered_parts.is_empty() {
        let text: String = ordered_parts
            .into_iter()
            .filter_map(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))
            .collect::<Vec<_>>()
            .join("\n");
        main.insert("content".into(), Value::String(text));
    }
    if !tool_calls.is_empty() {
        main.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    // 空内容时补一个空 content（OpenAI 要求 assistant 消息有 content 或 tool_calls）
    if !main.contains_key("content") && !main.contains_key("tool_calls") {
        main.insert("content".into(), Value::String(String::new()));
    }
    // tool 角色消息已被单独 push，不重复加入
    if role != "tool" || main.contains_key("tool_calls") {
        out.insert(0, Value::Object(main));
    }

    out
}

/// 把 canonical 图片转换为 OpenAI `image_url` 的 url 字符串。
///
/// - `Url`：直接透传；
/// - `Base64`：拼成 `data:<media_type>;base64,<data>`；
/// - `Artifact`：由 context-engine 解析后再进入 provider，此处返回 `None`。
fn image_to_openai_url(image: &agent_domain::ImageContent) -> Option<String> {
    use agent_domain::ImageSource;
    let url = match &image.source {
        ImageSource::Url(u) => u.clone(),
        ImageSource::Base64(data) => {
            if data.is_empty() {
                return None;
            }
            format!("data:{};base64,{}", image.media_type, data)
        }
        ImageSource::Artifact(_) => return None,
    };
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

fn tool_choice_to_openai(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::None => json!("none"),
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Named(name) => json!({
            "type": "function",
            "function": { "name": name }
        }),
    }
}

fn thinking_effort(config: &ThinkingConfig) -> Option<String> {
    match config.level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Low => Some("low".into()),
        ThinkingLevel::Medium => Some("medium".into()),
        ThinkingLevel::High => Some("high".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{
        ContentPart, Message, MessageId, MessageMetadata, MessageRole, TextContent,
        ToolCallContent, ToolCallId, ToolResultContent,
    };
    use provider_api::{ToolChoice, ToolDefinition};

    fn user(text: &str) -> Message {
        Message {
            id: MessageId::new("m1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: text.into() })],
            metadata: MessageMetadata::default(),
        }
    }

    fn base_request() -> CanonicalModelRequest {
        use std::collections::BTreeMap;
        CanonicalModelRequest {
            request_id: agent_domain::RequestId::from("r1"),
            model: agent_domain::ModelId::from("gpt-4o"),
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
            prompt_cache: provider_api::PromptCachePreference::Automatic,
            budget: provider_api::RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: None,
                    reasoning: None,
        }
    }

    #[test]
    fn basic_request_maps_fields() {
        let body = to_chat_completions_body(&base_request());
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["stop"], serde_json::json!(["END"]));
    }

    #[test]
    fn provider_options_ignore_reserved_keys_and_keep_custom_keys() {
        let mut req = base_request();
        req.provider_options
            .insert("MODEL".into(), json!("attacker-model"));
        req.provider_options
            .insert("stream_options".into(), json!({"include_usage": false}));
        req.provider_options
            .insert("authorization".into(), json!("Bearer secret"));
        req.provider_options.insert("top_p".into(), json!(0.9));

        let body = to_chat_completions_body(&req);

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(body.get("authorization").is_none());
        assert_eq!(body["top_p"], 0.9);
    }

    #[test]
    fn provider_options_cannot_override_reasoning_effort() {
        let mut req = base_request();
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: None,
        });
        req.provider_options
            .insert("reasoning_effort".into(), json!("low"));
        req.provider_options
            .insert("REASONING".into(), json!({"effort": "low"}));
        req.provider_options
            .insert("Effort".into(), json!("minimal"));
        req.provider_options.insert("top_p".into(), json!(0.9));

        let body = to_chat_completions_body(&req);

        // canonical thinking 仍然生效，注入值既不覆盖也不进入 wire body
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body.get("reasoning").is_none());
        assert!(body.get("effort").is_none());
        // 普通自定义 option 仍透传
        assert_eq!(body["top_p"], 0.9);
    }

    #[test]
    fn tools_and_tool_choice_mapped() {
        let mut req = base_request();
        req.tools.push(ToolDefinition {
            name: "read_file".into(),
            description: "read".into(),
            input_schema: json!({"type": "object"}),
        });
        req.tool_choice = ToolChoice::Required;
        let body = to_chat_completions_body(&req);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["tool_choice"], "required");
    }

    #[test]
    fn assistant_with_tool_call_maps_to_tool_calls() {
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
        let body = to_chat_completions_body(&req);
        let assistant = &body["messages"][1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["content"], "calling");
        assert_eq!(assistant["tool_calls"][0]["id"], "call-1");
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn tool_result_maps_to_tool_role_message() {
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
        let body = to_chat_completions_body(&req);
        let tool_msg = &body["messages"][1];
        assert_eq!(tool_msg["role"], "tool");
        assert_eq!(tool_msg["tool_call_id"], "call-1");
        assert_eq!(tool_msg["content"], "body");
    }

    #[test]
    fn image_content_maps_to_image_url_array() {
        use agent_domain::{ImageContent, ImageSource};

        let mut req = base_request();
        req.messages.push(Message {
            id: MessageId::new("u2"),
            role: MessageRole::User,
            content: vec![
                ContentPart::Text(TextContent {
                    text: "what is this".into(),
                }),
                ContentPart::Image(ImageContent {
                    source: ImageSource::Url("https://example.com/a.png".into()),
                    media_type: "image/png".into(),
                    alt_text: None,
                }),
                ContentPart::Image(ImageContent {
                    source: ImageSource::Base64("QkFTRTY0".into()),
                    media_type: "image/png".into(),
                    alt_text: None,
                }),
            ],
            metadata: MessageMetadata::default(),
        });
        let body = to_chat_completions_body(&req);
        let msg = &body["messages"][1];
        assert_eq!(msg["role"], "user");
        // 有图片时 content 为数组：text + image_url(url) + image_url(data:)
        let content = msg["content"].as_array().expect("content 应为数组");
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "https://example.com/a.png");
        assert_eq!(content[2]["type"], "image_url");
        assert_eq!(
            content[2]["image_url"]["url"],
            "data:image/png;base64,QkFTRTY0"
        );
    }
}
