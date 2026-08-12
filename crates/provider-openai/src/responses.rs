//! OpenAI Responses API（`/v1/responses`）传输路径（P15-2）。
//!
//! 与 P6-1 Chat Completions 路径并存：canonical 请求 → Responses `input` items +
//! hosted tool 声明；Responses SSE output items → canonical `ProviderStreamEvent`；
//! reasoning `encrypted_content` 只经 [`ReasoningProtector`]（Protected Blob Store
//! 边界）往返，绝不进入事件 / 日志 / GUI / Keychain（ADR-032）。
//!
//! 本模块不读 Provider 名称、不触网；所有 Provider 特例只存在于 wire 翻译层。
//! 复用 [`crate::reasoning`]（reasoning item 映射）、[`crate::server_tool`]（web
//! search / citation 映射）、[`provider_runtime::negotiate`]（transport 选择）。

use std::collections::BTreeMap;

use agent_domain::{
    ArtifactId, ContentPart, ImageContent, ImageSource, Message, MessageRole, ProgramStream,
    ServerToolEvent, StopReason, TokenUsage, ToolCallId,
};
use provider_api::{
    CanonicalModelRequest, HostedToolRequest, ProviderError, ProviderErrorKind,
    ProviderStreamEvent, ReasoningConfig, ReasoningEffort, ResponseFormat, ToolChoice,
    ToolDefinition,
};
use provider_runtime::negotiate::clamp_reasoning_to_thinking;
use provider_runtime::usage::{map_stop_reason, normalize_usage};
use serde_json::{json, Map, Value};

use crate::reasoning::canonical_reasoning_to_responses_input;
use crate::server_tool::{
    response_item_to_server_tool_event, url_citation_annotation_to_citation,
    web_search_source_to_source,
};

// ===========================================================================
// Reasoning protector（统一边界：provider_runtime::reasoning）
// ===========================================================================

/// 统一 reasoning 保护边界（共享 API，见 [`provider_runtime::reasoning`]）。
///
/// Provider 受信运行时在拿到 wire `encrypted_content` 后立刻 [`ReasoningProtector::protect`]
/// 存入受保护存储，只把返回的 [`agent_domain::ProtectedBlobRef`] 放进 canonical
/// 事件；回灌下一轮请求时 [`ReasoningProtector::resolve`] 取回
/// `ProtectedBlob` 明文重建 Responses input item。明文绝不进入事件 / 日志 /
/// GUI / OS Keychain（ADR-032）。
///
/// 默认实现 [`InMemoryReasoningProtector`] 仅保证进程内可回放，持久化 /
/// 跨进程保护由 [`provider_runtime::reasoning::ProtectedBlobStoreProtector`]
/// 提供（构造时捕获 store 与 `BlobScope`），host 注入
/// [`crate::OpenAiProvider`]。
///
/// 兼容 re-export：历史路径 `crate::responses::*` /
/// `provider_openai::*` 继续可用，实现由 `provider-runtime` 统一提供。
pub use provider_runtime::reasoning::{
    InMemoryReasoningProtector, ReasoningProtectError, ReasoningProtector,
};

// ===========================================================================
// 请求转换：canonical → Responses 请求体
// ===========================================================================

/// 协商通过、允许进入 Responses 请求体的 hosted tool 类别集合。
///
/// 由 [`crate::OpenAiProvider`] 在 transport 选择后从 `ResolvedCapabilities.supported`
/// 构造，避免把协商 `Reject` 的 hosted tool 仍发给远端。
#[derive(Clone, Debug, Default)]
pub struct AcceptedResponsesTools {
    pub web_search: bool,
    pub file_search: bool,
    pub code_interpreter: bool,
    pub image_generation: bool,
    pub hosted_shell: bool,
    pub apply_patch: bool,
    pub computer_use: bool,
    pub mcp: bool,
    pub tool_search: bool,
}

impl AcceptedResponsesTools {
    /// 从协商 `supported` 标签集构造（稳定 `tool:PascalCase` key，见
    /// `ToolCapabilityTag::capability_key` 与 negotiate 模块）。
    pub fn from_supported(supported: &std::collections::BTreeSet<String>) -> Self {
        use agent_domain::ToolCapabilityTag as T;
        let has = |tag: T| supported.contains(tag.capability_key());
        Self {
            web_search: has(T::WebSearch) || has(T::XSearch),
            file_search: has(T::FileOrCollectionSearch),
            code_interpreter: has(T::CodeExecution),
            image_generation: has(T::ImageGeneration),
            hosted_shell: has(T::HostedShell),
            apply_patch: has(T::ProviderApplyPatch),
            computer_use: has(T::ComputerUse),
            mcp: has(T::ServerSideMcp),
            tool_search: has(T::ToolSearch),
        }
    }
}

/// 构造 Responses 请求体（`stream: true`）。
///
/// - canonical messages → `input[]`（message / function_call / function_call_output）；
/// - 已解密的 reasoning items → `input[]` reasoning item（由调用方经
///   [`ReasoningProtector`] 解密后传入）；
/// - hosted tools / extensions → Responses built-in tool 声明（仅放行协商通过的）；
/// - `previous_response_id` 从 `provider_options` 读取（opaque 续接引用）。
pub fn to_responses_body(
    request: &CanonicalModelRequest,
    reasoning_inputs: Vec<Value>,
    accepted_tools: &AcceptedResponsesTools,
) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));
    body.insert("stream".into(), Value::Bool(true));

    // system 消息抽到顶层 `instructions`（Responses 推荐写法），其余进 input。
    let mut input_items: Vec<Value> = reasoning_inputs;
    let mut instructions: Option<String> = None;
    for message in &request.messages {
        if matches!(message.role, MessageRole::System) {
            if let Some(text) = single_text(message) {
                instructions = Some(match instructions {
                    None => text,
                    Some(mut existing) => {
                        existing.push('\n');
                        existing.push_str(&text);
                        existing
                    }
                });
                continue;
            }
        }
        input_items.extend(message_to_responses_input(message));
    }
    if let Some(instructions) = instructions {
        body.insert("instructions".into(), Value::String(instructions));
    }
    body.insert("input".into(), Value::Array(input_items));

    // hosted tools / extensions → Responses tool 声明（仅放行协商通过的）。
    let mut tools: Vec<Value> = Vec::new();
    let mut include: Vec<String> = Vec::new();
    for hosted in &request.hosted_tools {
        if let Some(tool) = hosted_tool_to_responses_tool(hosted, accepted_tools) {
            if matches!(
                hosted.kind,
                agent_domain::ToolCapabilityTag::WebSearch
                    | agent_domain::ToolCapabilityTag::XSearch
                    | agent_domain::ToolCapabilityTag::FileOrCollectionSearch
            ) {
                include.push(format!("{}.action.sources", responses_tool_type(&tool)));
            }
            tools.push(tool);
        }
    }
    for extension in &request.extensions {
        if let Some(tool) = extension_to_responses_tool(extension, accepted_tools) {
            tools.push(tool);
        }
    }
    for function_tool in &request.tools {
        tools.push(function_tool_to_responses_tool(function_tool));
    }
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools));
        body.insert(
            "tool_choice".into(),
            tool_choice_to_responses(&request.tool_choice),
        );
    }
    if !include.is_empty() {
        body.insert(
            "include".into(),
            Value::Array(include.into_iter().map(Value::String).collect()),
        );
    }

    // reasoning effort（现代 ReasoningConfig 优先，旧 thinking 经 clamp 派生）。
    if let Some(effort) = effective_reasoning_effort(request) {
        if let Some(wire) = reasoning_effort_to_wire(effort) {
            body.insert("reasoning".into(), json!({ "effort": wire }));
        }
    }

    if let Some(temp) = request.temperature {
        body.insert("temperature".into(), json!(temp));
    }
    if let Some(max_tokens) = request.max_output_tokens {
        body.insert("max_output_tokens".into(), json!(max_tokens));
    }

    match &request.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::Json => {
            body.insert("text".into(), json!({"format": {"type": "json_object"}}));
        }
        ResponseFormat::JsonSchema { name, schema } => {
            body.insert(
                "text".into(),
                json!({"format": {"type": "json_schema", "name": name, "schema": schema}}),
            );
        }
    }

    if let Some(Value::String(previous)) = request.provider_options.get("previous_response_id") {
        body.insert(
            "previous_response_id".into(),
            Value::String(previous.clone()),
        );
    }

    for (key, value) in &request.provider_options {
        if is_reserved_responses_option(key) {
            tracing::debug!(option = %key, "ignored reserved Responses provider option");
            continue;
        }
        body.insert(key.clone(), value.clone());
    }

    Value::Object(body)
}

fn responses_tool_type(tool: &Value) -> &str {
    tool.get("type").and_then(Value::as_str).unwrap_or("")
}

/// canonical hosted tool → Responses built-in tool 声明；未协商通过返回 `None`。
fn hosted_tool_to_responses_tool(
    hosted: &HostedToolRequest,
    accepted: &AcceptedResponsesTools,
) -> Option<Value> {
    use agent_domain::ToolCapabilityTag as T;
    match hosted.kind {
        T::WebSearch | T::XSearch if accepted.web_search => {
            Some(json!({"type": "web_search_preview"}))
        }
        T::FileOrCollectionSearch if accepted.file_search => {
            let vector_store_ids = hosted
                .config
                .as_ref()
                .and_then(|config| config.get("vector_store_ids"))
                .cloned()
                .unwrap_or_else(|| json!([]));
            Some(json!({"type": "file_search", "vector_store_ids": vector_store_ids}))
        }
        T::CodeExecution if accepted.code_interpreter => Some(json!({"type": "code_interpreter"})),
        T::ImageGeneration if accepted.image_generation => {
            Some(json!({"type": "image_generation"}))
        }
        T::HostedShell if accepted.hosted_shell => Some(json!({"type": "local_shell"})),
        T::ProviderApplyPatch if accepted.apply_patch => Some(json!({
            "type": "code_interpreter",
            "tools": [{"type": "apply_patch"}]
        })),
        T::ComputerUse if accepted.computer_use => {
            let display_width = hosted
                .config
                .as_ref()
                .and_then(|c| c.get("display_width"))
                .cloned();
            let mut tool = Map::new();
            tool.insert("type".into(), Value::String("computer_use_preview".into()));
            if let Some(width) = display_width {
                tool.insert("display_width".into(), width);
            }
            Some(Value::Object(tool))
        }
        // 未协商通过或未支持的 hosted tool：不发送（由 negotiate 记录 Reject/ClientTool）。
        _ => None,
    }
}

/// canonical extension（MCP / connector）→ Responses MCP tool 声明。
fn extension_to_responses_tool(
    extension: &provider_api::ExtensionToolRequest,
    accepted: &AcceptedResponsesTools,
) -> Option<Value> {
    if !accepted.mcp {
        return None;
    }
    // reference 形如 "connector:remote-mcp" / "https://mcp.example.com/sse"。
    let server_url = extension.reference.as_str();
    let mut tool = Map::new();
    tool.insert("type".into(), Value::String("mcp".into()));
    tool.insert("server_label".into(), Value::String(extension.name.clone()));
    tool.insert("server_url".into(), Value::String(server_url.to_owned()));
    if extension.requires_approval {
        tool.insert("require_approval".into(), Value::String("always".into()));
    }
    Some(Value::Object(tool))
}

fn function_tool_to_responses_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

fn tool_choice_to_responses(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::None => json!("none"),
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Named(name) => json!({"type": "function", "name": name}),
    }
}

/// 把 canonical message → Responses input items（message / function_call /
/// function_call_output）。reasoning 内容部分由调用方经 protector 解密后单独
/// 注入，不在此处处理。
fn message_to_responses_input(message: &Message) -> Vec<Value> {
    let mut out = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();
    let mut image_parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    for part in &message.content {
        match part {
            ContentPart::Text(t) => text_parts.push(t.text.clone()),
            ContentPart::Thinking(_) | ContentPart::Reasoning(_) => {}
            ContentPart::ToolCall(call) => {
                let args = if call.arguments.is_null() {
                    call.raw_arguments.clone().unwrap_or_default()
                } else {
                    call.arguments.to_string()
                };
                tool_calls.push(json!({
                    "type": "function_call",
                    "id": call.id.as_str(),
                    "call_id": call.id.as_str(),
                    "name": call.name,
                    "arguments": args,
                }));
            }
            ContentPart::ToolResult(result) => {
                let output: String = result
                    .content
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push(json!({
                    "type": "function_call_output",
                    "call_id": result.tool_call_id.as_str(),
                    "output": output,
                }));
            }
            ContentPart::Image(image) => {
                if let Some(url) = image_to_responses_input_image(image) {
                    image_parts.push(url);
                }
            }
            ContentPart::ArtifactRef(_) => {}
        }
    }

    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    let mut content: Vec<Value> = Vec::new();
    let text_type = match message.role {
        MessageRole::Assistant => "output_text",
        _ => "input_text",
    };
    if !text_parts.is_empty() {
        let joined = text_parts.join("\n");
        content.push(json!({"type": text_type, "text": joined}));
    }
    content.extend(image_parts);

    if !content.is_empty() {
        out.push(json!({"type": "message", "role": role, "content": content}));
    }
    out.extend(tool_calls);
    out
}

fn image_to_responses_input_image(image: &ImageContent) -> Option<Value> {
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
        Some(json!({"type": "input_image", "image_url": url}))
    }
}

fn single_text(message: &Message) -> Option<String> {
    let mut joined = String::new();
    for part in &message.content {
        if let ContentPart::Text(t) = part {
            if !joined.is_empty() {
                joined.push('\n');
            }
            joined.push_str(&t.text);
        }
    }
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

fn effective_reasoning_effort(request: &CanonicalModelRequest) -> Option<ReasoningEffort> {
    if let Some(reasoning) = &request.reasoning {
        return Some(reasoning.effort);
    }
    let clamped =
        clamp_reasoning_to_thinking(request.reasoning.as_ref(), request.thinking.as_ref());
    use provider_api::ThinkingLevel;
    Some(match clamped.level {
        ThinkingLevel::Off => ReasoningEffort::None,
        ThinkingLevel::Low => ReasoningEffort::Low,
        ThinkingLevel::Medium => ReasoningEffort::Medium,
        ThinkingLevel::High => ReasoningEffort::High,
    })
}

/// Responses `reasoning.effort` 接受 `minimal / low / medium / high`。
fn reasoning_effort_to_wire(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::None => None,
        ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        // Responses 暂无 xhigh / max，clamp 为 high（negotiator 已记录 ClampedEffort）。
        ReasoningEffort::High | ReasoningEffort::XHigh | ReasoningEffort::Max => Some("high"),
    }
}

fn is_reserved_responses_option(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "model"
            | "input"
            | "instructions"
            | "stream"
            | "tools"
            | "tool_choice"
            | "include"
            | "reasoning"
            | "previous_response_id"
            | "text"
            | "authorization"
            | "proxy-authorization"
            | "api_key"
            | "api-key"
            | "x-api-key"
    )
}

/// 解析 canonical 消息中的 reasoning items，经 protector 解密后构造 Responses
/// input reasoning items（按到达顺序）。无法解密 / 重建的条目跳过并记录诊断。
pub async fn resolve_reasoning_inputs(
    request: &CanonicalModelRequest,
    protector: &dyn ReasoningProtector,
) -> (Vec<Value>, Vec<String>) {
    let mut inputs = Vec::new();
    let mut warnings = Vec::new();
    for message in &request.messages {
        for part in &message.content {
            if let ContentPart::Reasoning(item) = part {
                match protector.resolve(&item.protected_blob_ref).await {
                    Ok(payload) => match String::from_utf8(payload.expose().to_vec()) {
                        Ok(decrypted) => {
                            match canonical_reasoning_to_responses_input(item, &decrypted) {
                                Ok(input) => inputs.push(input),
                                Err(error) => warnings.push(format!(
                                    "reasoning item {} rebuild failed: {error}",
                                    item.id.as_str()
                                )),
                            }
                        }
                        Err(_) => warnings.push(format!(
                            "reasoning item {} decrypted payload is not utf-8",
                            item.id.as_str()
                        )),
                    },
                    Err(error) => warnings.push(format!(
                        "reasoning item {} resolve failed: {error}",
                        item.id.as_str()
                    )),
                }
            }
        }
    }
    (inputs, warnings)
}

// ===========================================================================
// 流式组装：Responses SSE → ProviderStreamEvent
// ===========================================================================

/// 流式组装器产出的中间事件。
#[derive(Clone, Debug)]
pub enum ResponsesAssemblyEvent {
    /// 可直接发射的 canonical 事件。
    Canonical(ProviderStreamEvent),
    /// reasoning output item：wire 原文需要先经 [`ReasoningProtector`] 保护，
    /// 由调用方构造 [`agent_domain::ReasoningItem`] 后再发射 `ReasoningItem` 事件。
    ReasoningOutputItem { wire: Value },
}

/// Responses SSE 事件 → canonical 事件的增量组装器。
#[derive(Default)]
pub struct ResponsesStreamAssembler {
    function_calls: BTreeMap<String, String>,
    function_started: std::collections::BTreeSet<String>,
    /// 最近一次 web_search / file_search call id，用于把 message url_citation
    /// 归属到产生它的 server tool（可重放，确定性来源于 fixture 顺序）。
    last_search_call_id: Option<String>,
    response_id: Option<String>,
    usage: TokenUsage,
    stop_reason: Option<StopReason>,
    completed: bool,
}

impl ResponsesStreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// 消费一条 SSE `data` JSON，返回本条触发的组装事件。
    pub fn feed(&mut self, data: &str) -> Vec<ResponsesAssemblyEvent> {
        let value: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            "response.created" => {
                let id = response_id_of(&value);
                if id.is_some() {
                    self.response_id = id.clone();
                }
                vec![ResponsesAssemblyEvent::Canonical(
                    ProviderStreamEvent::ResponseStarted { response_id: id },
                )]
            }
            "response.output_text.delta" => {
                let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
                if delta.is_empty() {
                    Vec::new()
                } else {
                    vec![ResponsesAssemblyEvent::Canonical(
                        ProviderStreamEvent::TextDelta(delta.to_owned()),
                    )]
                }
            }
            "response.output_text.done" => self.handle_text_done(&value),
            "response.reasoning_summary_text.delta" => {
                let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
                if delta.is_empty() {
                    Vec::new()
                } else {
                    vec![ResponsesAssemblyEvent::Canonical(
                        ProviderStreamEvent::ThinkingDelta(delta.to_owned()),
                    )]
                }
            }
            "response.output_item.added" => self.handle_item_added(value.get("item")),
            "response.function_call_arguments.delta" => {
                self.handle_function_arguments_delta(&value)
            }
            "response.output_item.done" => self.handle_item_done(value.get("item")),
            "response.completed" => self.handle_completed(&value),
            "response.failed" | "response.incomplete" => self.handle_failed(&value),
            _ => Vec::new(),
        }
    }

    /// 流结束后冲刷残留状态。
    pub fn finish(self) -> ResponsesFinalState {
        ResponsesFinalState {
            response_id: self.response_id,
            usage: self.usage,
            stop_reason: self.stop_reason,
            completed: self.completed,
        }
    }

    fn handle_text_done(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let mut events = Vec::new();
        if let Some(annotations) = value.get("annotations").and_then(Value::as_array) {
            let tool_call_id = self
                .last_search_call_id
                .clone()
                .unwrap_or_else(|| "responses:citation".into());
            for annotation in annotations {
                if let Ok(citation) = url_citation_annotation_to_citation(annotation) {
                    events.push(ResponsesAssemblyEvent::Canonical(
                        ProviderStreamEvent::ServerTool(ServerToolEvent::CitationAdded {
                            tool_call_id: ToolCallId::from(tool_call_id.clone()),
                            citation,
                        }),
                    ));
                }
            }
        }
        events
    }

    fn handle_item_added(&mut self, item: Option<&Value>) -> Vec<ResponsesAssemblyEvent> {
        let Some(item) = item else {
            return Vec::new();
        };
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if item_type == "function_call" {
            let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or(&item_id)
                .to_owned();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            self.function_calls.insert(item_id.clone(), call_id.clone());
            self.function_started.insert(item_id);
            return vec![ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::ToolCallStarted {
                    id: ToolCallId::from(call_id),
                    name,
                },
            )];
        }
        Vec::new()
    }

    fn handle_function_arguments_delta(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let Some(item_id) = value.get("item_id").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Some(call_id) = self.function_calls.get(item_id).cloned() else {
            return Vec::new();
        };
        let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
        if delta.is_empty() {
            return Vec::new();
        }
        vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ToolCallArgumentsDelta {
                id: ToolCallId::from(call_id),
                json: delta.to_owned(),
            },
        )]
    }

    fn handle_item_done(&mut self, item: Option<&Value>) -> Vec<ResponsesAssemblyEvent> {
        let Some(item) = item else {
            return Vec::new();
        };
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "message" => Vec::new(),
            "reasoning" => vec![ResponsesAssemblyEvent::ReasoningOutputItem { wire: item.clone() }],
            "function_call" => self.handle_function_call_done(item),
            "web_search_call" | "file_search_call" => self.handle_search_call_done(item),
            "code_interpreter_call" => self.handle_code_interpreter_done(item),
            "computer_call" => self.handle_computer_call_done(item),
            "image_generation_call" => self.handle_image_generation_done(item),
            "mcp_call" => self.handle_mcp_call_done(item),
            "local_shell_call" => self.handle_local_shell_done(item),
            "custom_tool_call" => self.handle_custom_tool_done(item),
            _ => Vec::new(),
        }
    }

    fn handle_function_call_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or(&item_id)
            .to_owned();
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let arguments = item
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let mut events = Vec::new();
        if !self.function_started.contains(&item_id) {
            self.function_calls.insert(item_id.clone(), call_id.clone());
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::ToolCallStarted {
                    id: ToolCallId::from(call_id.clone()),
                    name: name.clone(),
                },
            ));
            if !arguments.is_empty() {
                events.push(ResponsesAssemblyEvent::Canonical(
                    ProviderStreamEvent::ToolCallArgumentsDelta {
                        id: ToolCallId::from(call_id.clone()),
                        json: arguments.clone(),
                    },
                ));
            }
        }
        events.push(ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ToolCallCompleted {
                id: ToolCallId::from(call_id),
            },
        ));
        events
    }

    fn handle_search_call_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let mut events = Vec::new();
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        match response_item_to_server_tool_event(item) {
            Ok(event) => {
                events.push(ResponsesAssemblyEvent::Canonical(
                    ProviderStreamEvent::ServerTool(event),
                ));
            }
            Err(error) => {
                tracing::debug!(error = %error, "unmapped web_search_call item");
            }
        }
        self.last_search_call_id = Some(id.clone());
        // file_search_call 自带 results[]；web_search_call 需 include 才有 action.sources。
        let sources = item
            .get("action")
            .and_then(|action| action.get("sources"))
            .or_else(|| item.get("results"))
            .or_else(|| item.get("sources"));
        if let Some(sources) = sources.and_then(Value::as_array) {
            for source in sources {
                if let Ok(source) = web_search_source_to_source(source) {
                    events.push(ResponsesAssemblyEvent::Canonical(
                        ProviderStreamEvent::ServerTool(ServerToolEvent::SourceAdded {
                            tool_call_id: ToolCallId::from(id.clone()),
                            source,
                        }),
                    ));
                }
            }
        }
        events
    }

    fn handle_code_interpreter_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let id = required_id(item);
        let tool_call_id = ToolCallId::from(id);
        let mut events = vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramStarted {
                tool_call_id: tool_call_id.clone(),
                command: item.get("code").and_then(Value::as_str).map(str::to_owned),
            }),
        )];
        if let Some(outputs) = item.get("outputs").and_then(Value::as_array) {
            for output in outputs {
                if let Some(artifact) = output
                    .get("file_id")
                    .and_then(Value::as_str)
                    .map(ArtifactId::from)
                {
                    events.push(ResponsesAssemblyEvent::Canonical(
                        ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramOutput {
                            tool_call_id: tool_call_id.clone(),
                            stream: ProgramStream::Stdout,
                            delta: None,
                            artifact: Some(artifact),
                        }),
                    ));
                } else if let Some(text) = output.get("logs").and_then(Value::as_str) {
                    events.push(ResponsesAssemblyEvent::Canonical(
                        ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramOutput {
                            tool_call_id: tool_call_id.clone(),
                            stream: ProgramStream::Stdout,
                            delta: Some(text.to_owned()),
                            artifact: None,
                        }),
                    ));
                }
            }
        }
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        events.push(server_tool_completion(
            &tool_call_id,
            status,
            item.get("error"),
        ));
        events
    }

    fn handle_computer_call_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let id = required_id(item);
        let tool_call_id = ToolCallId::from(id);
        let mut events = Vec::new();
        if let Some(action) = item.get("action") {
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::ServerTool(ServerToolEvent::ComputerActionRequested {
                    tool_call_id: tool_call_id.clone(),
                    action: action.clone(),
                }),
            ));
        }
        if let Some(screenshot) = item
            .get("output")
            .and_then(|o| o.get("image_url"))
            .and_then(Value::as_str)
        {
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::ServerTool(ServerToolEvent::ComputerScreenshot {
                    tool_call_id: tool_call_id.clone(),
                    artifact: ArtifactId::from(screenshot.to_owned()),
                    media_type: Some("image/png".into()),
                }),
            ));
        }
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        events.push(server_tool_completion(
            &tool_call_id,
            status,
            item.get("error"),
        ));
        events
    }

    fn handle_image_generation_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let id = required_id(item);
        let tool_call_id = ToolCallId::from(id);
        let mut events = vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ServerTool(ServerToolEvent::Started {
                tool_call_id: tool_call_id.clone(),
                name: "image_generation".into(),
                arguments: item.get("prompt").cloned(),
            }),
        )];
        if let Some(image) = item
            .get("output")
            .and_then(Value::as_str)
            .or_else(|| item.get("image_url").and_then(Value::as_str))
        {
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::ServerTool(ServerToolEvent::Completed {
                    tool_call_id: tool_call_id.clone(),
                    summary: None,
                    artifacts: vec![ArtifactId::from(image.to_owned())],
                }),
            ));
        } else {
            let status = item.get("status").and_then(Value::as_str).unwrap_or("");
            events.push(server_tool_completion(
                &tool_call_id,
                status,
                item.get("error"),
            ));
        }
        events
    }

    fn handle_mcp_call_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let id = required_id(item);
        let tool_call_id = ToolCallId::from(id);
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("mcp")
            .to_owned();
        let mut events = vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ServerTool(ServerToolEvent::Started {
                tool_call_id: tool_call_id.clone(),
                name: format!("mcp:{name}"),
                arguments: item.get("arguments").cloned(),
            }),
        )];
        if let Some(output) = item.get("output").and_then(Value::as_str) {
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::ServerTool(ServerToolEvent::Completed {
                    tool_call_id: tool_call_id.clone(),
                    summary: Some(output.to_owned()),
                    artifacts: Vec::new(),
                }),
            ));
        } else {
            let status = item.get("status").and_then(Value::as_str).unwrap_or("");
            events.push(server_tool_completion(
                &tool_call_id,
                status,
                item.get("error"),
            ));
        }
        events
    }

    fn handle_local_shell_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let id = required_id(item);
        let tool_call_id = ToolCallId::from(id);
        let command = item
            .get("action")
            .and_then(|a| a.get("command"))
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut events = vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramStarted {
                tool_call_id: tool_call_id.clone(),
                command,
            }),
        )];
        let output = item.get("output");
        if let Some(stdout) = output.and_then(|o| o.get("stdout")).and_then(Value::as_str) {
            if !stdout.is_empty() {
                events.push(ResponsesAssemblyEvent::Canonical(
                    ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramOutput {
                        tool_call_id: tool_call_id.clone(),
                        stream: ProgramStream::Stdout,
                        delta: Some(stdout.to_owned()),
                        artifact: None,
                    }),
                ));
            }
        }
        if let Some(stderr) = output.and_then(|o| o.get("stderr")).and_then(Value::as_str) {
            if !stderr.is_empty() {
                events.push(ResponsesAssemblyEvent::Canonical(
                    ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramOutput {
                        tool_call_id: tool_call_id.clone(),
                        stream: ProgramStream::Stderr,
                        delta: Some(stderr.to_owned()),
                        artifact: None,
                    }),
                ));
            }
        }
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        events.push(server_tool_completion(
            &tool_call_id,
            status,
            item.get("error"),
        ));
        events
    }

    /// custom_tool_call：Responses 把 server-side function（非客户端）也以
    /// function_call 形式返回，但其结果由 Provider 持有，不映射
    /// `function_call_output`（仅客户端 Function Calling 才走该路径）。
    fn handle_custom_tool_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let id = required_id(item);
        let tool_call_id = ToolCallId::from(id);
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("custom")
            .to_owned();
        let mut events = vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ServerTool(ServerToolEvent::Started {
                tool_call_id: tool_call_id.clone(),
                name,
                arguments: item.get("input").cloned(),
            }),
        )];
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        events.push(server_tool_completion(
            &tool_call_id,
            status,
            item.get("error"),
        ));
        events
    }

    fn handle_completed(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let response = value.get("response").unwrap_or(value);
        if let Some(id) = response
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            self.response_id = Some(id);
        }
        let mut events = Vec::new();
        let usage = normalize_usage(response);
        if usage != TokenUsage::default() {
            self.usage = usage.clone();
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::UsageUpdated(usage),
            ));
        }
        let status = response.get("status").and_then(Value::as_str);
        // Responses status 词汇与 Chat Completions finish_reason 不同：先归一到
        // map_stop_reason 能理解的 finish 串（completed → stop；incomplete → length）。
        let mapped_status = match status {
            Some("completed") => Some("stop"),
            Some("incomplete") => Some("length"),
            Some("cancelled") => Some("cancelled"),
            other => other,
        };
        let stop = map_stop_reason(mapped_status, false);
        self.stop_reason = Some(stop.clone());
        self.completed = true;
        events.push(ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ResponseCompleted(stop),
        ));
        events
    }

    fn handle_failed(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let response = value.get("response").unwrap_or(value);
        let status = response.get("status").and_then(Value::as_str);
        let error = response
            .get("error")
            .or_else(|| response.get("status_details"));
        let message = error
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("responses request failed")
            .to_owned();
        let kind = match status {
            Some("incomplete") => ProviderErrorKind::StreamInterrupted,
            _ => ProviderErrorKind::ProviderUnavailable,
        };
        self.completed = true;
        vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::Error(ProviderError::new(kind, message)),
        )]
    }
}

/// 流结束后的最终状态（response_id / usage / stop_reason）。
#[derive(Clone, Debug, Default)]
pub struct ResponsesFinalState {
    pub response_id: Option<String>,
    pub usage: TokenUsage,
    pub stop_reason: Option<StopReason>,
    pub completed: bool,
}

fn server_tool_completion(
    tool_call_id: &ToolCallId,
    status: &str,
    error: Option<&Value>,
) -> ResponsesAssemblyEvent {
    let event = match status {
        "failed" | "incomplete" => {
            let message = error
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let code = error
                .and_then(|e| e.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            ServerToolEvent::Failed {
                tool_call_id: tool_call_id.clone(),
                message,
                code,
            }
        }
        _ => ServerToolEvent::Completed {
            tool_call_id: tool_call_id.clone(),
            summary: None,
            artifacts: Vec::new(),
        },
    };
    ResponsesAssemblyEvent::Canonical(ProviderStreamEvent::ServerTool(event))
}

fn required_id(item: &Value) -> String {
    item.get("id")
        .and_then(Value::as_str)
        .unwrap_or("responses_tool")
        .to_owned()
}

fn response_id_of(value: &Value) -> Option<String> {
    value
        .get("response")
        .and_then(|r| r.get("id"))
        .and_then(Value::as_str)
        .or_else(|| value.get("id").and_then(Value::as_str))
        .map(str::to_owned)
}

// ===========================================================================
// 错误归一
// ===========================================================================

/// 把 HttpClient 已经按状态码归一的 [`ProviderError`] 进一步按 Responses 特有
/// error code 细化（vector store 未就绪 / code_interpreter 超时 / computer_use
/// 需确认 / MCP 不可用），重试建议与 P2-10 一致。
pub fn normalize_responses_error(mut error: ProviderError) -> ProviderError {
    let message = error.message.to_ascii_lowercase();
    let refined = if message.contains("vector_store") && message.contains("not_ready") {
        Some((
            ProviderErrorKind::ProviderUnavailable,
            true,
            "vector store not ready",
        ))
    } else if message.contains("code_interpreter") && message.contains("timeout") {
        Some((ProviderErrorKind::Timeout, true, "code interpreter timeout"))
    } else if message.contains("local_shell") && message.contains("timeout") {
        Some((ProviderErrorKind::Timeout, true, "hosted shell timeout"))
    } else if message.contains("computer_use") && message.contains("confirm") {
        Some((
            ProviderErrorKind::InvalidRequest,
            false,
            "computer use requires explicit confirmation",
        ))
    } else if message.contains("mcp") && message.contains("unavailable") {
        Some((
            ProviderErrorKind::ProviderUnavailable,
            true,
            "server-side mcp unavailable",
        ))
    } else if message.contains("skill") && message.contains("unavailable") {
        Some((
            ProviderErrorKind::ProviderUnavailable,
            true,
            "provider skill unavailable",
        ))
    } else {
        None
    };
    if let Some((kind, retryable, detail)) = refined {
        error.kind = kind;
        error.retryable = retryable;
        error
            .diagnostics
            .insert("responses_error".into(), detail.into());
    }
    error
}

// ===========================================================================
// 协商辅助：从请求构造 CapabilityRequirements
// ===========================================================================

/// 把 canonical 请求折叠为 P15-8 [`provider_api::CapabilityRequirements`]。
///
/// transport 偏好固定优先 Responses（OpenAI 原生偏好现代路径，由 negotiator 按
/// 模型声明收窄 / 降级）；required_tools 取 hosted_tools + extensions 类别；
/// citations 在出现搜索类工具时要求；reasoning 取请求 reasoning / thinking。
pub fn requirements_from_request(
    request: &CanonicalModelRequest,
) -> provider_api::CapabilityRequirements {
    use agent_domain::ToolCapabilityTag as T;
    use std::collections::BTreeSet;

    let mut required_tools: BTreeSet<T> =
        request.hosted_tools.iter().map(|tool| tool.kind).collect();
    for extension in &request.extensions {
        required_tools.insert(T::ServerSideMcp);
        required_tools.extend(extension.capabilities.iter().copied());
    }
    let needs_citations = request.hosted_tools.iter().any(|tool| {
        matches!(
            tool.kind,
            T::WebSearch | T::XSearch | T::FileOrCollectionSearch
        )
    });
    let reasoning = request.reasoning.clone().or_else(|| {
        request.thinking.as_ref().map(|thinking| {
            let clamped = clamp_reasoning_to_thinking(None, Some(thinking));
            ReasoningConfig::new(effort_from_thinking_level(clamped.level))
        })
    });

    provider_api::CapabilityRequirements {
        transport_pref: vec![provider_api::ModelTransport::Responses],
        required_tools,
        reasoning,
        citations: needs_citations,
    }
}

fn effort_from_thinking_level(level: provider_api::ThinkingLevel) -> ReasoningEffort {
    use provider_api::ThinkingLevel;
    match level {
        ThinkingLevel::Off => ReasoningEffort::None,
        ThinkingLevel::Low => ReasoningEffort::Low,
        ThinkingLevel::Medium => ReasoningEffort::Medium,
        ThinkingLevel::High => ReasoningEffort::High,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::ReasoningItemId;
    use std::collections::BTreeMap;

    fn user_request(text: &str) -> CanonicalModelRequest {
        use agent_domain::{MessageId, TextContent};
        CanonicalModelRequest {
            request_id: agent_domain::RequestId::from("r1"),
            model: agent_domain::ModelId::from("o3"),
            messages: vec![Message {
                id: MessageId::new("m1"),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent { text: text.into() })],
                metadata: agent_domain::MessageMetadata::default(),
            }],
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            temperature: Some(0.0),
            max_output_tokens: Some(128),
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: provider_api::PromptCachePreference::Automatic,
            budget: provider_api::RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: Some("trace-1".into()),
            reasoning: None,
        }
    }

    fn canonical_only(events: Vec<ResponsesAssemblyEvent>) -> Vec<ProviderStreamEvent> {
        events
            .into_iter()
            .filter_map(|e| match e {
                ResponsesAssemblyEvent::Canonical(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn body_has_responses_shape_with_input_items_and_stream() {
        let request = user_request("hi");
        let body = to_responses_body(&request, Vec::new(), &AcceptedResponsesTools::default());
        assert_eq!(body["model"], "o3");
        assert_eq!(body["stream"], true);
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][0]["text"], "hi");
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn system_message_lifts_to_instructions() {
        use agent_domain::{MessageId, TextContent};
        let mut request = user_request("hello");
        request.messages.insert(
            0,
            Message {
                id: MessageId::new("sys"),
                role: MessageRole::System,
                content: vec![ContentPart::Text(TextContent {
                    text: "be brief".into(),
                })],
                metadata: agent_domain::MessageMetadata::default(),
            },
        );
        let body = to_responses_body(&request, Vec::new(), &AcceptedResponsesTools::default());
        assert_eq!(body["instructions"], "be brief");
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
        assert_eq!(body["input"][0]["role"], "user");
    }

    #[test]
    fn hosted_tools_only_emitted_when_negotiated_accept() {
        use agent_domain::ToolCapabilityTag as T;
        let mut request = user_request("search");
        request.hosted_tools.push(HostedToolRequest {
            name: "web_search".into(),
            kind: T::WebSearch,
            description: String::new(),
            capabilities: vec![T::WebSearch],
            config: None,
        });
        let body = to_responses_body(&request, Vec::new(), &AcceptedResponsesTools::default());
        assert!(body.get("tools").is_none());
        let accepted = AcceptedResponsesTools {
            web_search: true,
            ..AcceptedResponsesTools::default()
        };
        let body = to_responses_body(&request, Vec::new(), &accepted);
        assert_eq!(body["tools"][0]["type"], "web_search_preview");
        assert_eq!(body["include"][0], "web_search_preview.action.sources");
    }

    #[test]
    fn reasoning_effort_clamps_xhigh_to_high() {
        let mut request = user_request("think");
        request.reasoning = Some(ReasoningConfig::new(ReasoningEffort::XHigh));
        let body = to_responses_body(&request, Vec::new(), &AcceptedResponsesTools::default());
        assert_eq!(body["reasoning"]["effort"], "high");
    }

    #[test]
    fn previous_response_id_passes_through() {
        let mut request = user_request("continue");
        request
            .provider_options
            .insert("previous_response_id".into(), json!("resp_abc"));
        let body = to_responses_body(&request, Vec::new(), &AcceptedResponsesTools::default());
        assert_eq!(body["previous_response_id"], "resp_abc");
    }

    #[test]
    fn reserved_provider_options_cannot_override_canonical() {
        let mut request = user_request("hi");
        request
            .provider_options
            .insert("model".into(), json!("attacker"));
        request.provider_options.insert("input".into(), json!([]));
        request.provider_options.insert("top_p".into(), json!(0.9));
        let body = to_responses_body(&request, Vec::new(), &AcceptedResponsesTools::default());
        assert_eq!(body["model"], "o3");
        assert_eq!(body["input"][0]["content"][0]["text"], "hi");
        assert_eq!(body["top_p"], 0.9);
    }

    #[test]
    fn assembler_streams_text_reasoning_function_and_completes() {
        let mut assembler = ResponsesStreamAssembler::new();
        let created = r#"{"type":"response.created","response":{"id":"resp_1"}}"#;
        let text_delta = r#"{"type":"response.output_text.delta","delta":"Hello"}"#;
        let think_delta = r#"{"type":"response.reasoning_summary_text.delta","delta":"hmm"}"#;
        let fc_added = r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":""}}"#;
        let fc_args = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"p\":1}"}"#;
        let fc_done = r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{\"p\":1}"}}"#;
        let completed = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":3,"output_tokens":2}}}"#;

        assert!(matches!(
            canonical_only(assembler.feed(created)).last(),
            Some(ProviderStreamEvent::ResponseStarted { response_id }) if response_id.as_deref() == Some("resp_1")
        ));
        assert!(matches!(
            canonical_only(assembler.feed(text_delta)).last(),
            Some(ProviderStreamEvent::TextDelta(t)) if t == "Hello"
        ));
        assert!(matches!(
            canonical_only(assembler.feed(think_delta)).last(),
            Some(ProviderStreamEvent::ThinkingDelta(t)) if t == "hmm"
        ));
        assert!(matches!(
            canonical_only(assembler.feed(fc_added)).last(),
            Some(ProviderStreamEvent::ToolCallStarted { name, .. }) if name == "read"
        ));
        assert!(matches!(
            canonical_only(assembler.feed(fc_args)).last(),
            Some(ProviderStreamEvent::ToolCallArgumentsDelta { json, .. }) if json == "{\"p\":1}"
        ));
        let done = canonical_only(assembler.feed(fc_done));
        assert!(done
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::ToolCallCompleted { .. })));
        let final_events = canonical_only(assembler.feed(completed));
        assert!(final_events
            .iter()
            .any(|e| matches!(e, ProviderStreamEvent::UsageUpdated(u) if u.input_tokens == 3)));
        assert!(final_events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed)
        )));
        let state = assembler.finish();
        assert_eq!(state.response_id.as_deref(), Some("resp_1"));
        assert!(state.completed);
    }

    #[test]
    fn reasoning_output_item_surfaces_as_candidate() {
        let mut assembler = ResponsesStreamAssembler::new();
        let item = serde_json::json!({
            "type": "reasoning",
            "id": "rs_abc",
            "summary": [{"type": "summary_text", "text": "step"}],
            "encrypted_content": "opaque-bytes"
        });
        let event = format!(
            "{{\"type\":\"response.output_item.done\",\"item\":{}}}",
            item
        );
        let produced = assembler.feed(&event);
        assert_eq!(produced.len(), 1);
        match &produced[0] {
            ResponsesAssemblyEvent::ReasoningOutputItem { wire } => {
                assert_eq!(wire["id"], "rs_abc")
            }
            other => panic!("expected reasoning candidate, got {other:?}"),
        }
    }

    #[test]
    fn web_search_call_emits_server_tool_and_sources() {
        let mut assembler = ResponsesStreamAssembler::new();
        let event = r#"{"type":"response.output_item.done","item":{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"pawork","sources":[{"type":"url","url":"https://pawork.dev","title":"Pawork"}]}}}"#;
        let events = canonical_only(assembler.feed(event));
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { tool_call_id, .. })
                if tool_call_id.as_str() == "ws_1"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderStreamEvent::ServerTool(ServerToolEvent::SourceAdded { tool_call_id, source })
                if tool_call_id.as_str() == "ws_1" && source.url.as_deref() == Some("https://pawork.dev")
        )));
    }

    #[test]
    fn error_normalization_maps_known_responses_codes() {
        let timeout = ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "code_interpreter timeout while executing",
        );
        let normalized = normalize_responses_error(timeout);
        assert_eq!(normalized.kind, ProviderErrorKind::Timeout);
        assert!(normalized.retryable);

        let confirm = ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "computer_use requires confirm",
        );
        let normalized = normalize_responses_error(confirm);
        assert_eq!(normalized.kind, ProviderErrorKind::InvalidRequest);
        assert!(!normalized.retryable);
    }

    #[tokio::test]
    async fn resolve_reasoning_inputs_round_trips_through_in_memory_protector() {
        use agent_domain::ReasoningItem;
        let protector = InMemoryReasoningProtector::default();
        let reference = protector.protect(b"opaque-bytes").await.unwrap();
        let item = ReasoningItem {
            id: ReasoningItemId::from("rs_1"),
            summary: None,
            protected_blob_ref: reference,
            opaque_metadata: BTreeMap::from([(
                "openai.responses.summary_entries".into(),
                Value::Array(vec![json!({"type":"summary_text","text":"step"})]),
            )]),
            continuation_metadata: BTreeMap::new(),
        };
        let mut request = user_request("continue");
        request.messages[0]
            .content
            .push(ContentPart::Reasoning(item));
        let (inputs, warnings) = resolve_reasoning_inputs(&request, &protector).await;
        assert!(warnings.is_empty());
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0]["type"], "reasoning");
        assert_eq!(inputs[0]["id"], "rs_1");
        assert_eq!(inputs[0]["encrypted_content"], "opaque-bytes");
    }

    #[test]
    fn requirements_from_request_collects_hosted_and_citations() {
        use agent_domain::ToolCapabilityTag as T;
        let mut request = user_request("search");
        request.hosted_tools.push(HostedToolRequest {
            name: "web_search".into(),
            kind: T::WebSearch,
            description: String::new(),
            capabilities: vec![T::WebSearch],
            config: None,
        });
        let requirements = requirements_from_request(&request);
        assert!(requirements.required_tools.contains(&T::WebSearch));
        assert!(requirements.citations);
        assert_eq!(
            requirements.transport_pref,
            vec![provider_api::ModelTransport::Responses]
        );
    }
}
