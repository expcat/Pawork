//! canonical 请求 → Google Gemini `generateContent` 请求体的转换。
//!
//! 字段命名遵循 Gemini v1beta REST 约定（camelCase）：`contents` / `parts` /
//! `systemInstruction` / `generationConfig` / `toolConfig` / `functionDeclarations`。

use provider_api::{
    CanonicalModelRequest, ResponseFormat, ThinkingConfig, ThinkingLevel, ToolChoice,
};
use serde_json::{json, Map, Value};

/// 把 canonical 请求转换为 Gemini `generateContent` 请求体。
pub fn to_generate_content_body(request: &CanonicalModelRequest) -> Value {
    let mut body = Map::new();

    // system 消息聚合到 systemInstruction；user/assistant/tool 进 contents。
    let mut system_parts: Vec<Value> = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
    for message in &request.messages {
        use agent_domain::MessageRole;
        match message.role {
            MessageRole::System => {
                for part in &message.content {
                    if let agent_domain::ContentPart::Text(t) = part {
                        if !t.text.is_empty() {
                            system_parts.push(json!({ "text": t.text }));
                        }
                    }
                }
            }
            MessageRole::User => {
                let parts = message_to_parts(&message.content);
                if !parts.is_empty() {
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
            MessageRole::Assistant => {
                let parts = message_to_parts(&message.content);
                if !parts.is_empty() {
                    contents.push(json!({ "role": "model", "parts": parts }));
                }
            }
            MessageRole::Tool => {
                // Gemini 要求 functionResponse 放在 role=user 的 content 内。
                let parts: Vec<Value> = message
                    .content
                    .iter()
                    .filter_map(function_response_part)
                    .collect();
                if !parts.is_empty() {
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }
    }

    if !system_parts.is_empty() {
        body.insert("systemInstruction".into(), json!({ "parts": system_parts }));
    }
    body.insert("contents".into(), Value::Array(contents));

    // tools / toolConfig
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                })
            })
            .collect();
        body.insert(
            "tools".into(),
            json!({ "functionDeclarations": declarations }),
        );
        body.insert("toolConfig".into(), tool_config(&request.tool_choice));
    }

    // generationConfig
    let mut gen = Map::new();
    if let Some(temp) = request.temperature {
        gen.insert("temperature".into(), json!(temp));
    }
    if let Some(max) = request.max_output_tokens {
        gen.insert("maxOutputTokens".into(), json!(max));
    }
    if !request.stop_sequences.is_empty() {
        gen.insert("stopSequences".into(), json!(request.stop_sequences));
    }

    // 结构化输出（P6-8）
    match &request.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::Json => {
            gen.insert("responseMimeType".into(), json!("application/json"));
        }
        ResponseFormat::JsonSchema { schema, .. } => {
            gen.insert("responseMimeType".into(), json!("application/json"));
            gen.insert("responseSchema".into(), schema.clone());
        }
    }

    // thinking（P6-5）：thinkingConfig.thinkingBudget
    if let Some(thinking) = &request.thinking {
        if let Some(budget) = thinking_budget(thinking) {
            gen.insert("thinkingConfig".into(), json!({ "thinkingBudget": budget }));
        }
    }

    // provider_options 透传（P6-9）：默认并入 generationConfig。
    for (key, value) in &request.provider_options {
        gen.insert(key.clone(), value.clone());
    }

    if !gen.is_empty() {
        body.insert("generationConfig".into(), Value::Object(gen));
    }

    Value::Object(body)
}

/// 把单条消息的 content 片段转为 Gemini parts（text / inlineData / fileData /
/// functionCall）。ToolResult 与 Thinking 不在此处理。
fn message_to_parts(content: &[agent_domain::ContentPart]) -> Vec<Value> {
    use agent_domain::{ContentPart, ImageSource};

    let mut parts = Vec::new();
    for part in content {
        match part {
            ContentPart::Text(t) => {
                if !t.text.is_empty() {
                    parts.push(json!({ "text": t.text }));
                }
            }
            ContentPart::Image(image) => match &image.source {
                ImageSource::Base64(data) if !data.is_empty() => {
                    parts.push(json!({
                        "inlineData": { "mimeType": image.media_type, "data": data }
                    }));
                }
                ImageSource::Url(url) if !url.is_empty() => {
                    parts.push(json!({
                        "fileData": { "fileUri": url, "mimeType": image.media_type }
                    }));
                }
                // Artifact 由 context-engine 解析后再进入 provider，此处跳过。
                _ => {}
            },
            ContentPart::ToolCall(call) => {
                let args = tool_call_args(call);
                parts.push(json!({ "functionCall": { "name": call.name, "args": args } }));
            }
            // 推理内容不回传给 provider。
            ContentPart::Thinking(_) | ContentPart::ToolResult(_) | ContentPart::ArtifactRef(_) => {
            }
        }
    }
    parts
}

/// 取 tool call 的 args（对象）；优先 arguments，回退 raw_arguments，再回退空对象。
fn tool_call_args(call: &agent_domain::ToolCallContent) -> Value {
    if call.arguments.is_object() {
        call.arguments.clone()
    } else if let Some(raw) = &call.raw_arguments {
        serde_json::from_str(raw).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    }
}

/// 把 ToolResult 片段映射为 Gemini `functionResponse` part。
fn function_response_part(part: &agent_domain::ContentPart) -> Option<Value> {
    let result = match part {
        agent_domain::ContentPart::ToolResult(r) => r,
        _ => return None,
    };
    let text: String = result
        .content
        .iter()
        .filter_map(|p| match p {
            agent_domain::ContentPart::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let response = if text.is_empty() {
        json!({})
    } else {
        // 内容本身是 JSON 对象时原样透传，否则包成 { "content": <text> }。
        match serde_json::from_str::<Value>(&text) {
            Ok(v) if v.is_object() => v,
            _ => json!({ "content": text }),
        }
    };
    let name = result.tool_name.clone().unwrap_or_default();
    Some(json!({ "functionResponse": { "name": name, "response": response } }))
}

/// canonical `ToolChoice` → Gemini `toolConfig.functionCallingConfig`。
fn tool_config(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::None => json!({ "functionCallingConfig": { "mode": "NONE" } }),
        ToolChoice::Auto => json!({ "functionCallingConfig": { "mode": "AUTO" } }),
        ToolChoice::Required => json!({ "functionCallingConfig": { "mode": "ANY" } }),
        ToolChoice::Named(name) => json!({
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": [name]
            }
        }),
    }
}

/// thinking 预算映射：显式 budget_tokens 优先；否则 Off→0、Low/Medium/High 递增。
fn thinking_budget(config: &ThinkingConfig) -> Option<u64> {
    if let Some(budget) = config.budget_tokens {
        return Some(budget);
    }
    match config.level {
        ThinkingLevel::Off => Some(0),
        ThinkingLevel::Low => Some(512),
        ThinkingLevel::Medium => Some(2048),
        ThinkingLevel::High => Some(8192),
    }
}
