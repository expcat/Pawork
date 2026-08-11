//! xAI Responses API（`/v1/responses`）传输路径（P15-4）。
//!
//! 与 P6-10 Chat Completions 路径并存：canonical 请求 → xAI Responses `input`
//! items + hosted tool 声明；xAI Responses SSE output items → canonical
//! [`ProviderStreamEvent`](provider_api::ProviderStreamEvent)；Grok reasoning 的
//! `encrypted_content` 只经 [`ReasoningProtector`]（Protected Blob Store 边界）往返，
//! 绝不进入事件 / 日志 / GUI / Keychain（ADR-032）。
//!
//! 本模块不读 Provider 名称、不触网；所有 xAI 特例只存在于 wire 翻译层。复用
//! [`crate::reasoning`]（Responses reasoning item 映射）、[`crate::server_tool`]
//!（server tool / citation 映射）、[`provider_runtime::negotiate`]（transport 选择）。
//!
//! xAI Responses 的 hosted tools：Web Search / X Search（Live Search）、Collection
//! Search（`file_search`）、Code Execution（`code_interpreter`）、server-side MCP。
//! 这些工具的 `*_call` output items 经 [`crate::server_tool`] 归一为
//! [`ServerToolEvent`](agent_domain::ServerToolEvent)；`sources` / `post` /
//! `document` 结果归一为 [`Source`](agent_domain::Source) / [`Citation`](agent_domain::Citation)。
//! hosted tool 续接走 `ProviderTranscript`，不经过客户端 `function_call_output` 路径。

use std::collections::BTreeMap;

use agent_domain::{
    ArtifactId, Citation, CitationSourceKind, ContentPart, ImageContent, ImageSource, Message,
    MessageRole, ProgramStream, ServerToolEvent, StopReason, TokenUsage, ToolCallId,
};
use provider_api::{
    CanonicalModelRequest, ExtensionToolRequest, HostedToolRequest, ProviderError,
    ProviderErrorKind, ProviderStreamEvent, ReasoningConfig, ReasoningEffort, ResponseFormat,
    ToolChoice, ToolDefinition,
};
use provider_runtime::negotiate::clamp_reasoning_to_thinking;
use provider_runtime::usage::{map_stop_reason, normalize_usage};
use serde_json::{json, Map, Value};

use crate::reasoning::to_responses_input_reasoning;
use crate::server_tool::{response_item_to_server_tool_event, url_citation_annotation_to_citation};

// ===========================================================================
// Reasoning protector（Protected Blob Store 边界抽象）
// ===========================================================================

/// Reasoning continuation 加密凭证的 Protected Blob Store 边界（ADR-032）。
///
/// Provider 受信运行时在拿到 wire `encrypted_content` 后立刻 [`Self::protect`]
/// 存入受保护存储，只把返回的引用放进 canonical 事件；回灌下一轮请求时
/// [`Self::resolve`] 取回明文重建 Responses input item。明文绝不进入事件 / 日志 /
/// GUI / OS Keychain。
///
/// 默认实现 [`InMemoryReasoningProtector`] 仅保证进程内可回放，持久化 / 跨进程
/// 保护由 host 在 P15-7 接入 `ReasoningStateBridge` 实现并注入
/// [`crate::XaiProvider`]。
#[async_trait::async_trait]
pub trait ReasoningProtector: Send + Sync {
    /// 加密保护一段 opaque continuation payload，返回稳定逻辑引用。
    async fn protect(&self, payload: &[u8]) -> Result<String, ReasoningProtectError>;

    /// 按 [`Self::protect`] 返回的引用解析回明文（仅在构造下一轮请求时调用）。
    async fn resolve(&self, blob_ref: &str) -> Result<Vec<u8>, ReasoningProtectError>;
}

/// Reasoning protector 失败：只描述边界错误，绝不携带明文 / 签名。
#[derive(Debug, thiserror::Error)]
pub enum ReasoningProtectError {
    #[error("reasoning blob not found for reference `{0}`")]
    NotFound(String),
    #[error("reasoning protector backend failure: {0}")]
    Backend(String),
}

/// 进程内 in-memory 默认 protector：保证 canonical 事件只携带引用，明文只
/// 存在于受信运行时内存。仅供 adapter 默认行为与测试；生产持久化由 host 注入。
#[derive(Default)]
pub struct InMemoryReasoningProtector {
    inner: tokio::sync::Mutex<BTreeMap<String, Vec<u8>>>,
    seq: tokio::sync::Mutex<u64>,
}

#[async_trait::async_trait]
impl ReasoningProtector for InMemoryReasoningProtector {
    async fn protect(&self, payload: &[u8]) -> Result<String, ReasoningProtectError> {
        let mut seq = self.seq.lock().await;
        *seq += 1;
        let reference = format!("xai-reasoning-{}", *seq);
        self.inner
            .lock()
            .await
            .insert(reference.clone(), payload.to_vec());
        Ok(reference)
    }

    async fn resolve(&self, blob_ref: &str) -> Result<Vec<u8>, ReasoningProtectError> {
        self.inner
            .lock()
            .await
            .get(blob_ref)
            .cloned()
            .ok_or_else(|| ReasoningProtectError::NotFound(blob_ref.to_owned()))
    }
}

// ===========================================================================
// 请求转换：canonical → xAI Responses 请求体
// ===========================================================================

/// 协商通过、允许进入 xAI Responses 请求体的 hosted tool 类别集合。
///
/// 由 [`crate::XaiProvider`] 在 transport 选择后从 `ResolvedCapabilities.supported`
/// 构造，避免把协商 `Reject` 的 hosted tool 仍发给远端。
#[derive(Clone, Debug, Default)]
pub struct AcceptedResponsesTools {
    pub web_search: bool,
    pub x_search: bool,
    pub collection_search: bool,
    pub code_execution: bool,
    pub mcp: bool,
}

impl AcceptedResponsesTools {
    /// 从协商 `supported` 标签集构造（`tool:<Tag>` 形式，见 negotiate 模块）。
    pub fn from_supported(supported: &std::collections::BTreeSet<String>) -> Self {
        use agent_domain::ToolCapabilityTag as T;
        let has = |tag: T| supported.contains(&format!("tool:{tag:?}"));
        Self {
            web_search: has(T::WebSearch),
            x_search: has(T::XSearch),
            collection_search: has(T::FileOrCollectionSearch),
            code_execution: has(T::CodeExecution),
            mcp: has(T::ServerSideMcp),
        }
    }
}

/// 构造 xAI Responses 请求体（`stream: true`）。
///
/// - canonical messages → `input[]`（message / function_call / function_call_output）；
/// - 已解密的 reasoning items → `input[]` reasoning item（由调用方经
///   [`ReasoningProtector`] 解密后传入）；
/// - hosted tools / extensions → xAI Responses built-in tool 声明（仅放行协商通过的）；
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

    // hosted tools / extensions → xAI Responses tool 声明（仅放行协商通过的）。
    let mut tools: Vec<Value> = Vec::new();
    let mut include: Vec<String> = Vec::new();
    for hosted in &request.hosted_tools {
        if let Some(tool) = hosted_tool_to_responses_tool(hosted, accepted_tools) {
            if responses_tool_emits_sources(&tool) {
                include.push(format!("{}.sources", responses_tool_type(&tool)));
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
        body.insert("previous_response_id".into(), Value::String(previous.clone()));
    }

    for (key, value) in &request.provider_options {
        if is_reserved_responses_option(key) {
            tracing::debug!(option = %key, "ignored reserved xAI Responses provider option");
            continue;
        }
        body.insert(key.clone(), value.clone());
    }

    Value::Object(body)
}

fn responses_tool_type(tool: &Value) -> &str {
    tool.get("type").and_then(Value::as_str).unwrap_or("")
}

/// xAI Live Search 类（web_search / x_search / collection search）的 *_call
/// 结果会带 `sources` / `results`；声明 include 让远端回传这些字段。
fn responses_tool_emits_sources(tool: &Value) -> bool {
    matches!(
        responses_tool_type(tool),
        "web_search" | "x_search" | "file_search"
    )
}

/// canonical hosted tool → xAI Responses built-in tool 声明；未协商通过返回 `None`。
fn hosted_tool_to_responses_tool(
    hosted: &HostedToolRequest,
    accepted: &AcceptedResponsesTools,
) -> Option<Value> {
    use agent_domain::ToolCapabilityTag as T;
    let config = hosted.config.as_ref();
    match hosted.kind {
        T::WebSearch if accepted.web_search => {
            let mut tool = Map::new();
            tool.insert("type".into(), Value::String("web_search".into()));
            if let Some(cfg) = config {
                for key in ["mode", "return_citations", "max_search_results"] {
                    if let Some(value) = cfg.get(key) {
                        tool.insert(key.into(), value.clone());
                    }
                }
            }
            Some(Value::Object(tool))
        }
        T::XSearch if accepted.x_search => {
            let mut tool = Map::new();
            tool.insert("type".into(), Value::String("x_search".into()));
            if let Some(cfg) = config {
                for key in ["max_search_results", "return_citations"] {
                    if let Some(value) = cfg.get(key) {
                        tool.insert(key.into(), value.clone());
                    }
                }
            }
            Some(Value::Object(tool))
        }
        T::FileOrCollectionSearch if accepted.collection_search => {
            // xAI Collection Search：集合 id 经 config 透传（不猜值）。
            let collection_ids = config
                .and_then(|c| c.get("collection_ids").cloned())
                .unwrap_or_else(|| json!([]));
            let vector_store_ids = config
                .and_then(|c| c.get("vector_store_ids").cloned())
                .unwrap_or_else(|| json!([]));
            Some(json!({
                "type": "file_search",
                "collection_ids": collection_ids,
                "vector_store_ids": vector_store_ids,
            }))
        }
        T::CodeExecution if accepted.code_execution => Some(json!({"type": "code_interpreter"})),
        T::ServerSideMcp if accepted.mcp => Some(json!({"type": "mcp"})),
        // 未协商通过或未支持的 hosted tool：不发送（由 negotiate 记录 Reject/ClientTool）。
        _ => None,
    }
}

/// canonical extension（server-side MCP）→ xAI Responses MCP tool 声明。
fn extension_to_responses_tool(
    extension: &ExtensionToolRequest,
    accepted: &AcceptedResponsesTools,
) -> Option<Value> {
    if !accepted.mcp {
        return None;
    }
    // reference 形如 "https://mcp.example.com/sse" / "connector:remote-mcp"。
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
    let clamped = clamp_reasoning_to_thinking(request.reasoning.as_ref(), request.thinking.as_ref());
    use provider_api::ThinkingLevel;
    Some(match clamped.level {
        ThinkingLevel::Off => ReasoningEffort::None,
        ThinkingLevel::Low => ReasoningEffort::Low,
        ThinkingLevel::Medium => ReasoningEffort::Medium,
        ThinkingLevel::High => ReasoningEffort::High,
    })
}

/// xAI Responses `reasoning.effort` 接受 `low / medium / high`。
fn reasoning_effort_to_wire(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::None => None,
        ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        // xAI Responses 暂无 xhigh / max，clamp 为 high（negotiator 已记录 ClampedEffort）。
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

/// 解析 canonical 消息中的 reasoning items，经 protector 解密后构造 xAI Responses
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
                match protector.resolve(item.protected_blob_ref.as_str()).await {
                    Ok(payload) => match String::from_utf8(payload) {
                        Ok(decrypted) => {
                            inputs.push(to_responses_input_reasoning(item, &decrypted));
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
// Live Search / Collection 结果 → canonical Source / Citation
// ===========================================================================

/// 把 xAI Live Search 的单条 source 归一为 canonical [`Source`](agent_domain::Source)。
///
/// 支持三类来源（不猜值）：
/// - 纯字符串 URL / `type: "url"` / `"web"` → Web 来源；
/// - `type: "x" / "post"`（X post）→ 保留 `text` / `url` 与原始 `raw_metadata`；
/// - `type: "document"`（Collection 文档）→ 保留 document_index / text。
///
/// 无法识别的结构在有 `url` 时保留原始 metadata，否则返回 `None`（fail-closed）。
pub fn live_search_source_to_source(value: &Value) -> Option<agent_domain::Source> {
    // 纯字符串 URL（xAI top-level citations[] / web source 紧凑形式）。
    if let Some(url) = value.as_str() {
        return Some(agent_domain::Source {
            url: Some(url.to_owned()),
            ..Default::default()
        });
    }
    let source_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let url = value.get("url").and_then(Value::as_str).map(str::to_owned);
    let title = value.get("title").and_then(Value::as_str).map(str::to_owned);
    let snippet = value
        .get("snippet")
        .or_else(|| value.get("text"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    match source_type {
        "url" | "web" => Some(agent_domain::Source {
            url,
            title,
            snippet,
            raw_metadata: Some(value.clone()),
            ..Default::default()
        }),
        "x" | "post" => Some(agent_domain::Source {
            url,
            title,
            text: snippet.clone(),
            snippet,
            raw_metadata: Some(value.clone()),
            ..Default::default()
        }),
        "document" => {
            let document_index = value
                .get("document_index")
                .and_then(Value::as_u64)
                .or_else(|| value.get("index").and_then(Value::as_u64));
            // 标注 Collection 来源种类到 raw_metadata，供下游 Citation 归一复现。
            let mut metadata = value.clone();
            if let Some(meta) = metadata.as_object_mut() {
                meta.insert("xai_source_kind".into(), Value::String("document".into()));
            }
            Some(agent_domain::Source {
                title,
                text: snippet.clone(),
                snippet,
                document_index,
                url,
                raw_metadata: Some(metadata),
                ..Default::default()
            })
        }
        // 未知来源类型：保留原始 metadata，不猜种类（fail-closed）。
        _ => value.get("url").and_then(Value::as_str).map(|url| agent_domain::Source {
            url: Some(url.to_owned()),
            title,
            snippet,
            raw_metadata: Some(value.clone()),
            ..Default::default()
        }),
    }
}

/// 把 [`Source`](agent_domain::Source) 折叠为 [`Citation`]（保留来源种类与可定位字段）。
pub fn source_to_citation(
    source: &agent_domain::Source,
    source_kind: CitationSourceKind,
) -> Citation {
    Citation {
        url: source.url.clone(),
        title: source.title.clone(),
        snippet: source.snippet.clone(),
        text: source.text.clone(),
        document_index: source.document_index,
        source_kind,
        ..Citation::empty()
    }
}

// ===========================================================================
// 流式组装：xAI Responses SSE → ProviderStreamEvent
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

/// xAI Responses SSE 事件 → canonical 事件的增量组装器。
#[derive(Default)]
pub struct ResponsesStreamAssembler {
    function_calls: BTreeMap<String, String>,
    function_started: std::collections::BTreeSet<String>,
    /// 最近一次 web_search / x_search / file_search call id，用于把 message
    /// url_citation 归属到产生它的 server tool（可重放，确定性来源于 fixture 顺序）。
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
                .unwrap_or_else(|| "xai:citation".into());
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
            "web_search_call" | "x_search_call" => self.handle_live_search_done(item),
            "file_search_call" => self.handle_collection_search_done(item),
            "code_interpreter_call" => self.handle_code_interpreter_done(item),
            "mcp_call" => self.handle_mcp_call_done(item),
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

    /// web_search_call / x_search_call：归一生命周期 + Live Search sources。
    fn handle_live_search_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
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
                tracing::debug!(error = %error, "unmapped xAI live_search_call item");
            }
        }
        self.last_search_call_id = Some(id.clone());
        // sources 可能位于 action.sources / sources / results。
        let sources = item
            .get("action")
            .and_then(|action| action.get("sources"))
            .or_else(|| item.get("sources"))
            .or_else(|| item.get("results"));
        if let Some(sources) = sources.and_then(Value::as_array) {
            for source in sources {
                if let Some(source) = live_search_source_to_source(source) {
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

    /// file_search_call（Collection Search）：collection documents → SourceAdded。
    fn handle_collection_search_done(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
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
                tracing::debug!(error = %error, "unmapped xAI file_search_call item");
            }
        }
        self.last_search_call_id = Some(id.clone());
        // Collection documents 可能位于 results / documents / action.sources。
        let docs = item
            .get("results")
            .or_else(|| item.get("documents"))
            .or_else(|| item.get("action").and_then(|a| a.get("sources")));
        if let Some(docs) = docs.and_then(Value::as_array) {
            for doc in docs {
                if let Some(source) = live_search_source_to_source(doc) {
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
        events.push(server_tool_completion(&tool_call_id, status, item.get("error")));
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
        // MCP 大输出走 Artifact（ADR-018）：output 为文件引用时只存 artifact。
        // MCP Secret 只经授权注入，绝不写进事件或日志。
        if let Some(artifact) = item
            .get("output_file_id")
            .and_then(Value::as_str)
            .map(ArtifactId::from)
        {
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::ServerTool(ServerToolEvent::Completed {
                    tool_call_id: tool_call_id.clone(),
                    summary: None,
                    artifacts: vec![artifact],
                }),
            ));
        } else if let Some(output) = item.get("output").and_then(Value::as_str) {
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::ServerTool(ServerToolEvent::Completed {
                    tool_call_id: tool_call_id.clone(),
                    summary: Some(output.to_owned()),
                    artifacts: Vec::new(),
                }),
            ));
        } else {
            let status = item.get("status").and_then(Value::as_str).unwrap_or("");
            events.push(server_tool_completion(&tool_call_id, status, item.get("error")));
        }
        events
    }

    fn handle_completed(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let response = value.get("response").unwrap_or(value);
        if let Some(id) = response.get("id").and_then(Value::as_str).map(str::to_owned) {
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
        let error = response.get("error").or_else(|| response.get("status_details"));
        let message = error
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("xai responses request failed")
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
        .unwrap_or("xai_responses_tool")
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

/// 把 HttpClient 已经按状态码归一的 [`ProviderError`] 进一步按 xAI Responses
/// 特有 error code 细化（Live Search 配额 / Collection 未就绪 / code_interpreter
/// 超时 / MCP 不可用或未授权 / 计费），重试建议与 P2-10 一致。
pub fn normalize_responses_error(mut error: ProviderError) -> ProviderError {
    let message = error.message.to_ascii_lowercase();
    let refined = if message.contains("live_search") && message.contains("quota") {
        Some((
            ProviderErrorKind::RateLimited,
            true,
            "xai live_search quota exceeded",
        ))
    } else if message.contains("web_search") && message.contains("rate") {
        Some((
            ProviderErrorKind::RateLimited,
            true,
            "xai web_search rate limited",
        ))
    } else if message.contains("x_search") && message.contains("quota") {
        Some((
            ProviderErrorKind::RateLimited,
            true,
            "xai x_search quota exceeded",
        ))
    } else if message.contains("collection") && message.contains("not_ready") {
        Some((
            ProviderErrorKind::ProviderUnavailable,
            true,
            "xai collection not ready",
        ))
    } else if message.contains("collection") && message.contains("not_found") {
        Some((
            ProviderErrorKind::InvalidRequest,
            false,
            "xai collection not found",
        ))
    } else if message.contains("code_interpreter") && message.contains("timeout") {
        Some((ProviderErrorKind::Timeout, true, "xai code interpreter timeout"))
    } else if message.contains("mcp") && message.contains("unavailable") {
        Some((
            ProviderErrorKind::ProviderUnavailable,
            true,
            "xai server-side mcp unavailable",
        ))
    } else if message.contains("mcp") && message.contains("unauthorized") {
        Some((
            ProviderErrorKind::Authorization,
            false,
            "xai server-side mcp requires authorization",
        ))
    } else if message.contains("billing") || message.contains("insufficient_quota") {
        Some((
            ProviderErrorKind::Authorization,
            false,
            "xai billing or quota insufficient",
        ))
    } else {
        None
    };
    if let Some((kind, retryable, detail)) = refined {
        error.kind = kind;
        error.retryable = retryable;
        error
            .diagnostics
            .insert("xai_responses_error".into(), detail.into());
    }
    error
}

// ===========================================================================
// 协商辅助：从请求构造 CapabilityRequirements
// ===========================================================================

/// 把 canonical 请求折叠为 P15-8 [`provider_api::CapabilityRequirements`]。
///
/// transport 偏好固定优先 Responses（xAI Grok 现代路径，由 negotiator 按模型声明
/// 收窄 / 降级）；required_tools 取 hosted_tools + extensions 类别；citations 在
/// 出现搜索类工具时要求；reasoning 取请求 reasoning / thinking。
pub fn requirements_from_request(
    request: &CanonicalModelRequest,
) -> provider_api::CapabilityRequirements {
    use agent_domain::ToolCapabilityTag as T;
    use std::collections::BTreeSet;

    let mut required_tools: BTreeSet<T> = request
        .hosted_tools
        .iter()
        .map(|tool| tool.kind)
        .collect();
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
