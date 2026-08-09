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

    // Gemini functionResponse 必须携带原 functionCall 名称。优先按
    // canonical call id 对齐；为兼容无 id/历史数据，再按出现顺序回退。
    let prior_tool_calls: Vec<(agent_domain::ToolCallId, String)> = request
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|part| match part {
            agent_domain::ContentPart::ToolCall(call) => Some((call.id.clone(), call.name.clone())),
            _ => None,
        })
        .collect();
    let mut tool_result_ordinal = 0usize;

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
                let mut parts = Vec::new();
                for part in &message.content {
                    let agent_domain::ContentPart::ToolResult(result) = part else {
                        continue;
                    };
                    let name =
                        resolve_tool_result_name(result, &prior_tool_calls, tool_result_ordinal);
                    parts.push(function_response_part(result, &name));
                    tool_result_ordinal += 1;
                }
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
        if is_reserved_provider_option(key) {
            tracing::warn!(
                provider_option = %key,
                "ignored reserved Gemini provider option"
            );
            continue;
        }
        gen.insert(key.clone(), value.clone());
    }

    if !gen.is_empty() {
        body.insert("generationConfig".into(), Value::Object(gen));
    }

    Value::Object(body)
}

fn is_reserved_provider_option(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "model"
            | "messages"
            | "contents"
            | "stream"
            | "stream_options"
            | "system"
            | "systeminstruction"
            | "tools"
            | "tool_choice"
            | "toolconfig"
            | "authorization"
            | "proxy-authorization"
            | "api_key"
            | "api-key"
            | "x-api-key"
            | "x-goog-api-key"
            | "key"
    )
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

/// 解析 ToolResult 对应的原 functionCall 名称。
fn resolve_tool_result_name(
    result: &agent_domain::ToolResultContent,
    prior_tool_calls: &[(agent_domain::ToolCallId, String)],
    ordinal: usize,
) -> String {
    result
        .tool_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            prior_tool_calls
                .iter()
                .find(|(id, _)| id == &result.tool_call_id)
                .map(|(_, name)| name.clone())
        })
        .or_else(|| prior_tool_calls.get(ordinal).map(|(_, name)| name.clone()))
        .unwrap_or_default()
}

/// 把 ToolResult 片段映射为 Gemini `functionResponse` part。
fn function_response_part(result: &agent_domain::ToolResultContent, name: &str) -> Value {
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
    json!({ "functionResponse": { "name": name, "response": response } })
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use agent_domain::{
        ContentPart, Message, MessageId, MessageMetadata, MessageRole, ModelId, RequestId,
        TextContent, ToolCallContent, ToolCallId, ToolResultContent,
    };
    use provider_api::{
        CanonicalModelRequest, PromptCachePreference, RequestBudget, ResponseFormat, ToolChoice,
    };

    use super::*;

    fn message(role: MessageRole, id: &str, content: Vec<ContentPart>) -> Message {
        Message {
            id: MessageId::new(id),
            role,
            content,
            metadata: MessageMetadata::default(),
        }
    }

    #[test]
    fn parallel_tool_results_keep_original_name_order() {
        let request = CanonicalModelRequest {
            request_id: RequestId::from("request-1"),
            model: ModelId::from("gemini-2.5-pro"),
            messages: vec![
                message(
                    MessageRole::Assistant,
                    "assistant-1",
                    vec![
                        ContentPart::ToolCall(ToolCallContent {
                            id: ToolCallId::new("gemini-call-0"),
                            name: "read".into(),
                            arguments: json!({}),
                            raw_arguments: None,
                            complete: true,
                        }),
                        ContentPart::ToolCall(ToolCallContent {
                            id: ToolCallId::new("gemini-call-1"),
                            name: "write".into(),
                            arguments: json!({}),
                            raw_arguments: None,
                            complete: true,
                        }),
                    ],
                ),
                message(
                    MessageRole::Tool,
                    "tool-1",
                    vec![
                        ContentPart::ToolResult(ToolResultContent {
                            tool_call_id: ToolCallId::new("gemini-call-0"),
                            tool_name: None,
                            content: vec![ContentPart::Text(TextContent { text: "one".into() })],
                            is_error: false,
                            metadata: Value::Null,
                        }),
                        // 故意使用旧历史中的不匹配 id，验证 ordinal 回退。
                        ContentPart::ToolResult(ToolResultContent {
                            tool_call_id: ToolCallId::new("legacy-call"),
                            tool_name: None,
                            content: vec![ContentPart::Text(TextContent { text: "two".into() })],
                            is_error: false,
                            metadata: Value::Null,
                        }),
                    ],
                ),
            ],
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            temperature: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: PromptCachePreference::Automatic,
            budget: RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: None,
        };

        let body = to_generate_content_body(&request);
        let names: Vec<&str> = body["contents"][1]["parts"]
            .as_array()
            .expect("function response parts")
            .iter()
            .map(|part| {
                part["functionResponse"]["name"]
                    .as_str()
                    .expect("function name")
            })
            .collect();
        assert_eq!(names, vec!["read", "write"]);
    }

    #[test]
    fn provider_options_ignore_reserved_keys() {
        let mut request = CanonicalModelRequest {
            request_id: RequestId::from("request-2"),
            model: ModelId::from("gemini-2.5-pro"),
            messages: vec![message(
                MessageRole::User,
                "user-1",
                vec![ContentPart::Text(TextContent { text: "hi".into() })],
            )],
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            temperature: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: PromptCachePreference::Automatic,
            budget: RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: None,
        };
        request
            .provider_options
            .insert("model".into(), json!("attacker-model"));
        request
            .provider_options
            .insert("x-goog-api-key".into(), json!("secret"));
        request.provider_options.insert("topP".into(), json!(0.9));

        let body = to_generate_content_body(&request);
        let generation = &body["generationConfig"];
        assert!(generation.get("model").is_none());
        assert!(generation.get("x-goog-api-key").is_none());
        assert_eq!(generation["topP"], 0.9);
    }
}
