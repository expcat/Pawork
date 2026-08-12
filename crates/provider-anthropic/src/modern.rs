//! P15-3 现代 Messages 路径（与 P6-2 基线同 crate 并存）。
//!
//! 本模块实现 Anthropic Modern Messages 的请求转换与 server tool / citation /
//! thinking signature 归一：
//!
//! - [`resolve`]：纯函数。按「请求中的现代字段 × 模型能力声明」选择
//!   [`TransportChoice::Modern`] 或 [`TransportChoice::Legacy`]；不支持项显式
//!   降级并记录 notes，禁止静默丢字段。
//! - [`to_modern_messages_body`]：`output_config.format` 原生 JSON Schema、
//!   request-level effort（`output_config.effort`）、adaptive / interleaved
//!   thinking、modern prompt cache 断点与 server / client tool 声明。
//! - [`server_tool_result_block_to_events`]：Provider 端结果块 → P15-5
//!   [`ServerToolEvent`] 生命周期（复用 [`crate::server_tool`] 的映射）。
//! - [`transcript_to_wire_blocks`]：`ProviderTranscriptEnvelope` → 原生
//!   续接块（P15-5 续传通道）。
//!
//! reasoning 续传经 [`ReasoningProtector`](provider_runtime::reasoning::ReasoningProtector)
//! 统一存取：adapter 只接触待加密字节与 [`ProtectedBlobRef`]；真实加密存储与
//! Provider/Session scope 由接线方（`ProtectedBlobStoreProtector` + `BlobScope`）
//! 注入，缺省进程内实现 `InMemoryReasoningProtector`；本 crate 不依赖
//! protected-blob-store。

use std::collections::BTreeMap;

use agent_domain::{
    ArtifactId, ContentPart, Message, MessageRole, ProviderTranscriptEnvelope, ReasoningItemId,
    ServerToolEvent, ThinkingContent, ToolCallId, ToolCapabilityTag, TranscriptItem,
};
use provider_api::{
    CanonicalModelRequest, ExtensionToolRequest, HostedToolRequest, ModelCapabilities,
    ModelTransport, PromptCachePreference, ReasoningEffort, ResponseFormat, ServerToolMappingError,
    ThinkingConfig,
};
use provider_runtime::negotiate::clamp_reasoning_to_thinking;
use serde_json::{json, Map, Value};

use crate::reasoning::{reconstruct_block, AnthropicThinkingPayload};
use crate::request::{
    is_reserved_provider_option, mark_last_block, message_to_anthropic, thinking_budget,
    tool_choice_to_anthropic, DEFAULT_MAX_TOKENS, DEFAULT_THINKING_OUTPUT_MARGIN,
};
use crate::server_tool::web_search_result_to_source;

// ---------- wire 工具名（Anthropic Modern Messages server tools） ----------

/// `web_search` server tool（2025-03-05 版本）。
pub const WEB_SEARCH_TOOL: &str = "web_search_20250305";
/// `web_fetch` server tool。
pub const WEB_FETCH_TOOL: &str = "web_fetch_20250521";
/// `code_execution` server tool。
pub const CODE_EXECUTION_TOOL: &str = "code_execution_20250522";
/// `computer` server tool（computer use）。
pub const COMPUTER_TOOL: &str = "computer_20250124";
/// `bash` server tool（hosted shell）。
pub const BASH_TOOL: &str = "bash_20250124";
/// `text_editor` server tool。
pub const TEXT_EDITOR_TOOL: &str = "text_editor_20250124";
/// `mcp_connector` server tool（server-side MCP）。
pub const MCP_CONNECTOR_TOOL: &str = "mcp_connector";
/// `tool_search` server tool。
pub const TOOL_SEARCH_TOOL: &str = "tool_search";
/// `memory` server tool（wire 形态按 fixture 冻结）。
pub const MEMORY_TOOL: &str = "memory";
/// `advisor` server tool（canonical tag 尚未冻结，经 canonical 名称映射）。
pub const ADVISOR_TOOL: &str = "advisor";

/// server_tool_use / `<name>_tool_result` 使用的 unversioned stem。
const TOOL_STEMS: &[&str] = &[
    "web_search",
    "web_fetch",
    "code_execution",
    "computer",
    "bash",
    "bash_code_execution",
    "text_editor",
    "mcp_connector",
    "tool_search",
    "memory",
    "advisor",
];

/// 大输出阈值：超过后 ProgramOutput 只留 Artifact 引用（ADR-018）。
const LARGE_OUTPUT_CHARS: usize = 64 * 1024;

// ---------- transport 选择与降级（纯函数） ----------

/// 传输选择结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportChoice {
    /// P6-2 基线 Messages 路径（thinking budget / system-prompt 结构化输出）。
    Legacy,
    /// 现代 Messages 路径（output_config / effort / adaptive / server tools）。
    Modern,
}

/// 现代路径的 thinking 计划。
#[derive(Clone, Debug, PartialEq)]
pub enum ThinkingPlan {
    /// adaptive / interleaved thinking，可带 request-level effort。
    Adaptive { effort: Option<String> },
    /// 旧 budget 模式（模型不支持 effort / adaptive 时显式 clamp 回退）。
    Budget(ThinkingConfig),
    /// 未请求 thinking。
    Off,
}

/// 结构化输出计划。
#[derive(Clone, Debug, PartialEq)]
pub enum FormatPlan {
    Text,
    /// 原生 `output_config.format.json_schema`。
    NativeJsonSchema {
        name: String,
        schema: Value,
    },
    /// 无法原生表达时显式降级到 P6-2 的 system 约束（记录 note，不静默）。
    LegacyJsonInstruction {
        name: Option<String>,
        schema: Option<Value>,
    },
}

/// [`resolve`] 的产物：现代请求体所需的一切决策 + 显式降级记录。
#[derive(Clone, Debug)]
pub struct ModernResolution {
    /// 降级 / 回退原因（逐条可观察，经 ProviderMetadata 透出）。
    pub notes: Vec<String>,
    /// 可表达的 Anthropic server tool 声明。
    pub hosted_tools: Vec<Value>,
    /// 被降级（丢弃）的 hosted/extension 工具 canonical 名。
    pub dropped_tools: Vec<String>,
    pub thinking: ThinkingPlan,
    pub format: FormatPlan,
}

impl Default for ModernResolution {
    fn default() -> Self {
        Self {
            notes: Vec::new(),
            hosted_tools: Vec::new(),
            dropped_tools: Vec::new(),
            thinking: ThinkingPlan::Off,
            format: FormatPlan::Text,
        }
    }
}

/// 纯函数：请求 × 模型能力声明 → 传输选择 + 显式降级决策。
///
/// 不触网、不读 Provider 名、不读 wall-clock；模型能力来自 adapter contract
/// （[`crate::provider::builtin_models`]）。任何不支持项都落到 notes /
/// dropped_tools，禁止静默丢字段。
pub fn resolve(
    request: &CanonicalModelRequest,
    caps: Option<&ModelCapabilities>,
) -> (TransportChoice, ModernResolution) {
    let modern_need = request.reasoning.is_some()
        || !request.hosted_tools.is_empty()
        || !request.extensions.is_empty()
        || matches!(
            request.response_format,
            ResponseFormat::Json | ResponseFormat::JsonSchema { .. }
        );
    if !modern_need {
        return (TransportChoice::Legacy, ModernResolution::default());
    }

    let Some(caps) = caps else {
        return (
            TransportChoice::Legacy,
            ModernResolution {
                notes: vec![
                    "model not in adapter catalog: modern transport not declared, legacy baseline used"
                        .into(),
                ],
                ..ModernResolution::default()
            },
        );
    };

    if caps.transport != ModelTransport::Messages {
        return (
            TransportChoice::Legacy,
            ModernResolution {
                notes: vec![format!(
                    "model declares {:?} transport: modern Messages not declared, legacy baseline used",
                    caps.transport
                )],
                ..ModernResolution::default()
            },
        );
    }

    let mut resolution = ModernResolution {
        thinking: resolve_thinking(request, caps),
        format: resolve_format(request, caps),
        ..ModernResolution::default()
    };

    for tool in &request.hosted_tools {
        // capability 门禁只对可表达 kind 生效；advisor 等经 canonical 名称
        // 回退的工具没有对应 tag，wire 形态由名称映射声明（P15-9 冻结 tag 前
        // 不额外门禁）。
        if let Some(kind_wire) = wire_name_for_kind(tool.kind) {
            if !caps.hosted_tool_tags.contains(&tool.kind) {
                resolution.dropped_tools.push(tool.name.clone());
                resolution.notes.push(format!(
                    "server tool `{}` not declared by model (wire `{kind_wire}`): degraded to client function calling",
                    tool.name
                ));
                continue;
            }
        }
        match hosted_tool_to_anthropic(tool) {
            Ok(declaration) => resolution.hosted_tools.push(declaration),
            Err(error) => {
                resolution.dropped_tools.push(tool.name.clone());
                resolution
                    .notes
                    .push(format!("server tool `{}` degraded: {error}", tool.name));
            }
        }
    }
    for extension in &request.extensions {
        match extension_to_anthropic(extension) {
            Ok(declaration) => resolution.hosted_tools.push(declaration),
            Err(error) => {
                resolution.dropped_tools.push(extension.name.clone());
                resolution
                    .notes
                    .push(format!("extension `{}` degraded: {error}", extension.name));
            }
        }
    }

    (TransportChoice::Modern, resolution)
}

fn resolve_thinking(request: &CanonicalModelRequest, caps: &ModelCapabilities) -> ThinkingPlan {
    if let Some(reasoning) = &request.reasoning {
        let requested = reasoning.requires_reasoning_support()
            || reasoning.state.requires_signature
            || reasoning.state.supports_interleaved;
        if !requested {
            return ThinkingPlan::Off;
        }
        let model_supports_reasoning = caps.thinking
            || caps.reasoning.state.requires_signature
            || caps.reasoning.state.requires_encrypted
            || caps.reasoning.state.supports_interleaved
            || caps.reasoning.supports_granular_effort;
        if !model_supports_reasoning {
            // 显式 clamp 回 P6 budget 模式（XHigh/Max → High 由 clamp 完成）。
            return ThinkingPlan::Budget(clamp_reasoning_to_thinking(
                Some(reasoning),
                request.thinking.as_ref(),
            ));
        }
        let mut effort = effort_to_wire(reasoning.effort);
        if matches!(
            reasoning.effort,
            ReasoningEffort::XHigh | ReasoningEffort::Max
        ) && !caps.reasoning.supports_granular_effort
        {
            effort = Some("high".into());
        }
        ThinkingPlan::Adaptive { effort }
    } else if let Some(thinking) = &request.thinking {
        ThinkingPlan::Budget(thinking.clone())
    } else {
        ThinkingPlan::Off
    }
}

fn resolve_format(request: &CanonicalModelRequest, caps: &ModelCapabilities) -> FormatPlan {
    match &request.response_format {
        ResponseFormat::Text => FormatPlan::Text,
        ResponseFormat::Json => FormatPlan::LegacyJsonInstruction {
            name: None,
            schema: None,
        },
        ResponseFormat::JsonSchema { name, schema } => {
            if caps.structured_output {
                FormatPlan::NativeJsonSchema {
                    name: name.clone(),
                    schema: schema.clone(),
                }
            } else {
                FormatPlan::LegacyJsonInstruction {
                    name: Some(name.clone()),
                    schema: Some(schema.clone()),
                }
            }
        }
    }
}

/// ReasoningEffort → Anthropic `output_config.effort` 字符串。
pub fn effort_to_wire(effort: ReasoningEffort) -> Option<String> {
    match effort {
        ReasoningEffort::None => None,
        ReasoningEffort::Low => Some("low".into()),
        ReasoningEffort::Medium => Some("medium".into()),
        ReasoningEffort::High => Some("high".into()),
        ReasoningEffort::XHigh => Some("xhigh".into()),
        ReasoningEffort::Max => Some("max".into()),
    }
}

// ---------- hosted / extension 工具 → Anthropic server tool 声明 ----------

/// capability kind → Anthropic wire 工具名。
pub fn wire_name_for_kind(kind: ToolCapabilityTag) -> Option<&'static str> {
    match kind {
        ToolCapabilityTag::WebSearch => Some(WEB_SEARCH_TOOL),
        ToolCapabilityTag::WebFetch => Some(WEB_FETCH_TOOL),
        ToolCapabilityTag::CodeExecution => Some(CODE_EXECUTION_TOOL),
        ToolCapabilityTag::HostedShell => Some(BASH_TOOL),
        ToolCapabilityTag::ProviderApplyPatch => Some(TEXT_EDITOR_TOOL),
        ToolCapabilityTag::ComputerUse => Some(COMPUTER_TOOL),
        ToolCapabilityTag::ServerSideMcp => Some(MCP_CONNECTOR_TOOL),
        ToolCapabilityTag::ToolSearch => Some(TOOL_SEARCH_TOOL),
        ToolCapabilityTag::Memory => Some(MEMORY_TOOL),
        // Anthropic 现代 Messages 无法表达的位点：显式 Unsupported（走降级）。
        ToolCapabilityTag::FileOrCollectionSearch
        | ToolCapabilityTag::XSearch
        | ToolCapabilityTag::ImageGeneration
        | ToolCapabilityTag::ProgrammaticToolCalling
        | ToolCapabilityTag::ServerSideMultiAgent => None,
    }
}

/// canonical 工具名 → Anthropic wire 工具名（覆盖 kind 无法表达的残余名称，
/// 如 `advisor`）。
pub fn wire_name_for_canonical_name(name: &str) -> Option<&'static str> {
    match name {
        "web_search" => Some(WEB_SEARCH_TOOL),
        "web_fetch" => Some(WEB_FETCH_TOOL),
        "code_execution" => Some(CODE_EXECUTION_TOOL),
        "computer_use" | "computer" => Some(COMPUTER_TOOL),
        "bash" => Some(BASH_TOOL),
        "text_editor" => Some(TEXT_EDITOR_TOOL),
        "mcp_connector" | "mcp" => Some(MCP_CONNECTOR_TOOL),
        "tool_search" => Some(TOOL_SEARCH_TOOL),
        "memory" => Some(MEMORY_TOOL),
        "advisor" => Some(ADVISOR_TOOL),
        _ => None,
    }
}

/// 把 [`HostedToolRequest`] 翻译为 Anthropic server tool 声明。
///
/// 优先按 capability kind 映射；kind 不可表达时按 canonical 名称回退（advisor
/// 等尚未冻结 tag 的工具）。`config` 只透传可无损表达的键，其余显式拒绝。
pub fn hosted_tool_to_anthropic(tool: &HostedToolRequest) -> Result<Value, ServerToolMappingError> {
    let by_kind = wire_name_for_kind(tool.kind);
    let by_name = wire_name_for_canonical_name(&tool.name);
    let wire = match (by_kind, by_name) {
        (Some(kind), Some(name)) if kind != name => {
            return Err(ServerToolMappingError::unsupported(format!(
                "hosted tool `{}` has conflicting kind {:?} and name mappings ({} vs {})",
                tool.name, tool.kind, kind, name
            )))
        }
        (Some(kind), _) => kind,
        (None, Some(name)) => name,
        (None, None) => {
            return Err(ServerToolMappingError::unsupported(format!(
                "hosted tool `{}` kind {:?} not representable on Anthropic modern Messages",
                tool.name, tool.kind
            )))
        }
    };
    server_tool_declaration(wire, tool.config.as_ref())
}

/// 把 [`ExtensionToolRequest`] 翻译为 Anthropic `mcp_connector` 声明。
///
/// 只有可表达的 HTTP(S) connector reference 能映射；其余显式拒绝（走降级）。
pub fn extension_to_anthropic(
    extension: &ExtensionToolRequest,
) -> Result<Value, ServerToolMappingError> {
    if !(extension.reference.starts_with("http://") || extension.reference.starts_with("https://"))
    {
        return Err(ServerToolMappingError::unsupported(format!(
            "extension `{}` reference `{}` not representable as Anthropic mcp_connector",
            extension.name, extension.reference
        )));
    }
    let mut tool = Map::new();
    tool.insert("type".into(), json!(MCP_CONNECTOR_TOOL));
    tool.insert("name".into(), json!(extension.name));
    tool.insert("url".into(), json!(extension.reference));
    Ok(Value::Object(tool))
}

/// server tool 声明：`{"type": <wire name>}` + 可无损表达的 config 键。
fn server_tool_declaration(
    wire: &str,
    config: Option<&Value>,
) -> Result<Value, ServerToolMappingError> {
    let mut tool = Map::new();
    tool.insert("type".into(), json!(wire));
    let Some(config) = config else {
        return Ok(Value::Object(tool));
    };
    let Some(config) = config.as_object() else {
        return Err(ServerToolMappingError::unsupported(
            "server tool config must be an object",
        ));
    };
    let accepted: &[&str] = match wire {
        COMPUTER_TOOL => &["display_width_px", "display_height_px"],
        MCP_CONNECTOR_TOOL => &["url"],
        _ => &[],
    };
    let unknown: Vec<&String> = config
        .keys()
        .filter(|key| !accepted.contains(&key.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(ServerToolMappingError::unsupported(format!(
            "server tool `{wire}` config keys {unknown:?} not representable"
        )));
    }
    for key in accepted {
        if let Some(value) = config.get(*key) {
            tool.insert((*key).into(), value.clone());
        }
    }
    Ok(Value::Object(tool))
}

// ---------- 现代请求体 ----------

/// 现代请求体映射失败（fail-closed，绝不在缺字段时猜值）。
#[derive(Clone, Debug, thiserror::Error)]
pub enum ModernMappingError {
    #[error("reasoning continuation {0} unavailable")]
    MissingContinuation(String),
    #[error("cannot rehydrate reasoning block: {0}")]
    Reasoning(#[from] provider_api::ReasoningMappingError),
}

/// canonical 请求 → Anthropic Modern Messages 请求体。
///
/// `resolution` 来自 [`resolve`]；`continuations` 是已经
/// [`ReasoningProtector`](provider_runtime::reasoning::ReasoningProtector)
/// 解析出的（解密后）Anthropic thinking 载荷，按 ReasoningItemId 索引，用于
/// 重建带 signature 的 thinking / redacted_thinking 块。缺载荷返回
/// [`ModernMappingError::MissingContinuation`]。
pub fn to_modern_messages_body(
    request: &CanonicalModelRequest,
    resolution: &ModernResolution,
    continuations: &BTreeMap<ReasoningItemId, AnthropicThinkingPayload>,
) -> Result<Value, ModernMappingError> {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));

    let cache_enabled = request.prompt_cache != PromptCachePreference::Disabled;

    // 与 P6-2 相同的 thinking/max_tokens 约束：budget 模式下预算必须低于 max。
    let requested_budget = match &resolution.thinking {
        ThinkingPlan::Budget(config) => thinking_budget(config),
        _ => None,
    };
    let mut max_tokens = request
        .max_output_tokens
        .unwrap_or(DEFAULT_MAX_TOKENS)
        .max(2);
    if request.max_output_tokens.is_none() {
        if let Some(budget) = requested_budget {
            max_tokens = max_tokens.max(budget.saturating_add(DEFAULT_THINKING_OUTPUT_MARGIN));
        }
    }
    body.insert("max_tokens".into(), json!(max_tokens));
    body.insert("stream".into(), Value::Bool(true));

    // system 提取 + 显式降级的结构化输出 system 约束 + modern prompt cache 断点。
    let mut system_blocks = Vec::new();
    for message in &request.messages {
        if message.role == MessageRole::System {
            for part in &message.content {
                if let ContentPart::Text(text) = part {
                    system_blocks.push(json!({"type":"text","text": text.text}));
                }
            }
        }
    }
    if let FormatPlan::LegacyJsonInstruction { name, schema } = &resolution.format {
        system_blocks.push(legacy_json_instruction(name.as_deref(), schema.as_ref()));
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

    let mut out_messages = Vec::new();
    for message in &request.messages {
        if message.role == MessageRole::System {
            continue;
        }
        out_messages.extend(modern_message_to_anthropic(message, continuations)?);
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

    // server tool 声明 + 客户端 function 工具（同一 tools 数组，位点不混用）。
    let mut tools: Vec<Value> = resolution.hosted_tools.clone();
    for tool in &request.tools {
        tools.push(json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.input_schema,
        }));
    }
    if !tools.is_empty() {
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

    match &resolution.thinking {
        ThinkingPlan::Adaptive { effort } => {
            body.insert("thinking".into(), json!({"type":"adaptive"}));
            if let Some(effort) = effort {
                body.insert("output_config".into(), json!({"effort": effort}));
            }
        }
        ThinkingPlan::Budget(config) => {
            if let Some(budget) = thinking_budget(config) {
                let budget = budget.min(max_tokens.saturating_sub(1));
                body.insert(
                    "thinking".into(),
                    json!({"type":"enabled","budget_tokens": budget}),
                );
            }
        }
        ThinkingPlan::Off => {}
    }
    if let FormatPlan::NativeJsonSchema { name, schema } = &resolution.format {
        let format = json!({
            "type": "json_schema",
            "name": name,
            "schema": schema,
        });
        match body.get_mut("output_config") {
            Some(Value::Object(existing)) => {
                existing.insert("format".into(), format);
            }
            _ => {
                body.insert("output_config".into(), json!({"format": format}));
            }
        }
    }

    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature));
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
                "ignored reserved Anthropic provider option (modern)"
            );
            continue;
        }
        body.insert(key.clone(), value.clone());
    }

    Ok(Value::Object(body))
}

/// 现代路径的 assistant 消息转换：在 P6-2 基础上重建 thinking 续传块。
fn modern_message_to_anthropic(
    message: &Message,
    continuations: &BTreeMap<ReasoningItemId, AnthropicThinkingPayload>,
) -> Result<Vec<Value>, ModernMappingError> {
    if message.role != MessageRole::Assistant {
        return Ok(message_to_anthropic(message));
    }

    let mut blocks: Vec<Value> = Vec::new();
    // 最近的 Thinking 文本（其 reasoning_item_id 指向其后出现的 Reasoning item）。
    let mut pending_thinking: Option<&ThinkingContent> = None;
    for part in &message.content {
        match part {
            ContentPart::Thinking(thinking) => pending_thinking = Some(thinking),
            ContentPart::Reasoning(item) => {
                let payload = continuations
                    .get(&item.id)
                    .ok_or_else(|| ModernMappingError::MissingContinuation(item.id.to_string()))?;
                // 文本归属：assembler 把整段 thinking 文本关联到最近一个 item。
                let linked = pending_thinking
                    .filter(|thinking| thinking.reasoning_item_id.as_ref() == Some(&item.id));
                blocks.push(reconstruct_block(item, linked, payload)?);
            }
            ContentPart::Text(text) => {
                blocks.push(json!({"type":"text","text": text.text}));
            }
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
                    .filter_map(|part| match part {
                        ContentPart::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                // 只有客户端 function 的 Core 结果映射 tool_result（P15-5）。
                blocks.push(json!({
                    "type":"tool_result",
                    "tool_use_id": result.tool_call_id,
                    "content": content,
                    "is_error": result.is_error,
                }));
            }
            ContentPart::Image(image) => {
                let block = match &image.source {
                    agent_domain::ImageSource::Base64(data) if !data.is_empty() => Some(json!({
                        "type":"image",
                        "source":{"type":"base64","media_type": image.media_type,"data": data},
                    })),
                    agent_domain::ImageSource::Url(url) if !url.is_empty() => Some(json!({
                        "type":"image",
                        "source":{"type":"url","url": url},
                    })),
                    _ => None,
                };
                if let Some(block) = block {
                    blocks.push(block);
                }
            }
            ContentPart::ArtifactRef(_) => {}
        }
    }
    // 未关联 Thinking 文本（无 signature 可重建）：与 P6-2 口径一致不回传。

    if blocks.is_empty() {
        blocks.push(json!({"type":"text","text":""}));
    }
    Ok(vec![json!({"role": "assistant", "content": blocks})])
}

fn legacy_json_instruction(name: Option<&str>, schema: Option<&Value>) -> Value {
    match (name, schema) {
        (Some(name), Some(schema)) => json!({
            "type":"text",
            "text": format!(
                "Return only one valid JSON value that conforms to the JSON Schema named `{name}`. \
                 Do not include Markdown fences or explanatory text. JSON Schema: {}",
                serde_json::to_string(schema).expect("serde_json::Value always serializes")
            ),
        }),
        _ => json!({
            "type":"text",
            "text": "Return only one valid JSON value. Do not include Markdown fences or explanatory text.",
        }),
    }
}

fn mark_message_last_block(message: &mut Value) {
    if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
        mark_last_block(blocks);
    }
}

// ---------- server tool 结果块 → ServerToolEvent ----------

/// 把 `<name>_tool_result` 块归一为按序的 [`ServerToolEvent`] 生命周期。
///
/// 支持 `web_search` / `web_fetch` / `code_execution` / `bash` / `text_editor` /
/// `computer` / `tool_search` / `memory` / `mcp_connector` / `advisor` 的结果
/// 块与 `<name>_tool_result_error` 错误对象。大输出只留 Artifact 引用
/// （ADR-018）；无法无损映射的口径返回 Unsupported，不猜值。
pub fn server_tool_result_block_to_events(
    block: &Value,
) -> Result<Vec<ServerToolEvent>, ServerToolMappingError> {
    let block_type = required_str(block, "type", "result block without `type`")?;
    let Some(stem) = block_type.strip_suffix("_tool_result") else {
        return Err(ServerToolMappingError::unsupported(format!(
            "unmapped result block type `{block_type}`"
        )));
    };
    if !TOOL_STEMS.contains(&stem) {
        return Err(ServerToolMappingError::unsupported(format!(
            "unmapped result block type `{block_type}`"
        )));
    }
    let id = ToolCallId::from(required_str(
        block,
        "tool_use_id",
        "server tool result without `tool_use_id`",
    )?);
    let content = block
        .get("content")
        .ok_or_else(|| ServerToolMappingError::unsupported("result block without `content`"))?;

    if let Some(error) = content.as_object() {
        let error_type = error
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !error_type.ends_with("_tool_result_error") {
            return Err(ServerToolMappingError::unsupported(format!(
                "unmapped result error type `{error_type}`"
            )));
        }
        return Ok(vec![ServerToolEvent::Failed {
            tool_call_id: id,
            message: error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned),
            code: error
                .get("error_code")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }]);
    }

    let items = content.as_array().ok_or_else(|| {
        ServerToolMappingError::unsupported("result content is not an array or error object")
    })?;
    let mut events: Vec<ServerToolEvent> = Vec::new();
    let mut last_text: Option<String> = None;
    let mut screenshot_seq = 0usize;
    let mut output_seq = 0usize;

    match stem {
        "code_execution" | "bash" | "bash_code_execution" => {
            events.push(ServerToolEvent::ProgramStarted {
                tool_call_id: id.clone(),
                command: None,
            });
        }
        _ => {}
    }

    for item in items {
        let item_type = required_str(item, "type", "result content item without `type`")?;
        match item_type {
            "web_search_result" => {
                events.push(ServerToolEvent::SourceAdded {
                    tool_call_id: id.clone(),
                    source: web_search_result_to_source(item)?,
                });
            }
            "text" => {
                let text = item.get("text").and_then(Value::as_str).ok_or_else(|| {
                    ServerToolMappingError::unsupported("text item without `text`")
                })?;
                match stem {
                    "code_execution" | "bash" | "bash_code_execution" => {
                        events.push(program_output(&id, text, &mut output_seq));
                    }
                    "computer" => {
                        events.push(ServerToolEvent::Progress {
                            tool_call_id: id.clone(),
                            message: Some(text.to_string()),
                        });
                    }
                    _ => {
                        last_text = Some(text.to_string());
                    }
                }
            }
            "image" => {
                if stem != "computer" {
                    return Err(ServerToolMappingError::unsupported(format!(
                        "image content in `{stem}` result block"
                    )));
                }
                screenshot_seq += 1;
                events.push(ServerToolEvent::ComputerScreenshot {
                    tool_call_id: id.clone(),
                    artifact: ArtifactId::from(format!("{id}-screenshot-{screenshot_seq}")),
                    media_type: item
                        .get("source")
                        .and_then(|s| s.get("media_type"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
            other => {
                return Err(ServerToolMappingError::unsupported(format!(
                    "unmapped result content type `{other}`"
                )))
            }
        }
    }

    events.push(ServerToolEvent::Completed {
        tool_call_id: id,
        summary: last_text,
        artifacts: Vec::new(),
    });
    Ok(events)
}

/// ProgramOutput：大输出只留 Artifact 引用，与 `delta` 互斥（ADR-018）。
fn program_output(id: &ToolCallId, text: &str, output_seq: &mut usize) -> ServerToolEvent {
    if text.len() > LARGE_OUTPUT_CHARS {
        *output_seq += 1;
        return ServerToolEvent::ProgramOutput {
            tool_call_id: id.clone(),
            stream: agent_domain::ProgramStream::Stdout,
            delta: None,
            artifact: Some(ArtifactId::from(format!("{id}-output-{output_seq}"))),
        };
    }
    ServerToolEvent::ProgramOutput {
        tool_call_id: id.clone(),
        stream: agent_domain::ProgramStream::Stdout,
        delta: Some(text.to_string()),
        artifact: None,
    }
}

// ---------- transcript 信封 → 原生续接块 ----------

/// [`transcript_to_wire_blocks`] 的产物：按角色分组的原生续接块。
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EnvelopeWireBlocks {
    /// assistant 侧 `server_tool_use` 块。
    pub assistant: Vec<Value>,
    /// user 侧 `<name>_tool_result` 块。
    pub user: Vec<Value>,
}

/// 把 [`ProviderTranscriptEnvelope`] 重建为 Anthropic 原生续接块（P15-5）。
///
/// `Started` → assistant `server_tool_use`；`SourceAdded` / `ProgramOutput` /
/// `Completed` / `Failed` → user `<name>_tool_result`。无法重建的载荷
/// （截图 / Artifact 输出）返回 Unsupported，不伪造明文。
pub fn transcript_to_wire_blocks(
    envelope: &ProviderTranscriptEnvelope,
) -> Result<EnvelopeWireBlocks, ServerToolMappingError> {
    let mut assistant: Vec<Value> = Vec::new();
    let mut result_blocks: BTreeMap<String, (String, Vec<Value>)> = BTreeMap::new();
    for item in &envelope.items {
        match item {
            TranscriptItem::ServerTool(ServerToolEvent::Started {
                tool_call_id,
                name,
                arguments,
            }) => {
                assistant.push(json!({
                    "type": "server_tool_use",
                    "id": tool_call_id,
                    "name": name,
                    "input": arguments.clone().unwrap_or_else(|| json!({})),
                }));
            }
            TranscriptItem::ServerTool(event) => {
                let call_id = event.tool_call_id();
                let stem = tool_stem_for_event(event).ok_or_else(|| {
                    ServerToolMappingError::unsupported(
                        "transcript event cannot be reconstructed as an Anthropic result block",
                    )
                })?;
                let entry = result_blocks
                    .entry(call_id.to_string())
                    .or_insert_with(|| (stem.to_string(), Vec::new()));
                if entry.0 != stem {
                    return Err(ServerToolMappingError::unsupported(
                        "transcript mixes result stems for one tool call",
                    ));
                }
                entry.1.extend(transcript_event_content(event)?);
            }
            TranscriptItem::Text(_) => {
                return Err(ServerToolMappingError::unsupported(
                    "transcript text item cannot be reconstructed without wire context",
                ));
            }
        }
    }
    let user: Vec<Value> = result_blocks
        .into_iter()
        .map(|(call_id, (stem, content))| {
            json!({
                "type": format!("{stem}_tool_result"),
                "tool_use_id": call_id,
                "content": content,
            })
        })
        .collect();
    Ok(EnvelopeWireBlocks { assistant, user })
}

fn tool_stem_for_event(event: &ServerToolEvent) -> Option<&'static str> {
    match event {
        ServerToolEvent::SourceAdded { .. } => Some("web_search"),
        ServerToolEvent::ProgramStarted { .. } | ServerToolEvent::ProgramOutput { .. } => {
            Some("code_execution")
        }
        ServerToolEvent::Completed { .. } | ServerToolEvent::Failed { .. } => {
            // stem 由组内首个可重建事件决定；纯 Completed/Failed 组无上下文。
            Some("web_search")
        }
        _ => None,
    }
}

fn transcript_event_content(event: &ServerToolEvent) -> Result<Vec<Value>, ServerToolMappingError> {
    Ok(match event {
        ServerToolEvent::SourceAdded { source, .. } => {
            let mut item = match &source.raw_metadata {
                Some(Value::Object(map)) => Value::Object(map.clone()),
                _ => json!({"type": "web_search_result"}),
            };
            if let Some(object) = item.as_object_mut() {
                object.insert("type".into(), json!("web_search_result"));
                if let Some(url) = &source.url {
                    object.insert("url".into(), json!(url));
                }
                if let Some(title) = &source.title {
                    object.insert("title".into(), json!(title));
                }
            }
            vec![item]
        }
        ServerToolEvent::ProgramOutput {
            delta: Some(delta), ..
        } => vec![json!({"type":"text","text": delta})],
        ServerToolEvent::ProgramOutput {
            artifact: Some(_), ..
        } => {
            return Err(ServerToolMappingError::unsupported(
                "program output artifact payload is not available for wire reconstruction",
            ))
        }
        ServerToolEvent::ProgramOutput {
            delta: None,
            artifact: None,
            ..
        } => Vec::new(),
        ServerToolEvent::ProgramStarted { .. } => Vec::new(),
        ServerToolEvent::Progress { message, .. } => {
            if let Some(message) = message {
                vec![json!({"type":"text","text": message})]
            } else {
                Vec::new()
            }
        }
        ServerToolEvent::Completed { summary, .. } => {
            if let Some(summary) = summary {
                vec![json!({"type":"text","text": summary})]
            } else {
                Vec::new()
            }
        }
        ServerToolEvent::Failed { message, code, .. } => {
            let mut error = json!({"type": "web_search_tool_result_error"});
            if let Some(object) = error.as_object_mut() {
                if let Some(message) = message {
                    object.insert("message".into(), json!(message));
                }
                if let Some(code) = code {
                    object.insert("error_code".into(), json!(code));
                }
            }
            vec![error]
        }
        ServerToolEvent::Started { .. }
        | ServerToolEvent::ArgumentsDelta { .. }
        | ServerToolEvent::CitationAdded { .. }
        | ServerToolEvent::ComputerActionRequested { .. }
        | ServerToolEvent::ComputerScreenshot { .. } => {
            return Err(ServerToolMappingError::unsupported(
                "transcript event cannot be reconstructed as Anthropic result content",
            ))
        }
    })
}

// ---------- 小工具 ----------

/// 为响应流构造 server tool 名称白名单（canonical 名 + wire 名）。
pub fn server_tool_whitelist(request: &CanonicalModelRequest) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for tool in &request.hosted_tools {
        names.push(tool.name.clone());
        if let Some(wire) = wire_name_for_canonical_name(&tool.name) {
            names.push(wire.to_string());
        }
        if let Some(wire) = wire_name_for_kind(tool.kind) {
            names.push(wire.to_string());
        }
    }
    for extension in &request.extensions {
        names.push(extension.name.clone());
        names.push(MCP_CONNECTOR_TOOL.to_string());
    }
    names.sort();
    names.dedup();
    names
}

fn required_str<'a>(
    value: &'a Value,
    key: &str,
    error: &str,
) -> Result<&'a str, ServerToolMappingError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ServerToolMappingError::unsupported(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{
        ImageContent, Message, MessageId, MessageMetadata, ProtectedBlobRef, TextContent,
        ThinkingContent, ToolCallContent, ToolCallId, ToolResultContent,
    };
    use provider_api::{
        PromptCachePreference, ReasoningConfig, ReasoningStateCapability, ReasoningStateDescriptor,
        RequestBudget, ThinkingLevel, ToolChoice, ToolDefinition,
    };

    fn caps(transport: ModelTransport) -> ModelCapabilities {
        ModelCapabilities {
            text: true,
            image_input: true,
            tool_calls: true,
            parallel_tool_calls: true,
            thinking: true,
            structured_output: true,
            prompt_cache: true,
            transport,
            hosted_tool_tags: [
                ToolCapabilityTag::WebSearch,
                ToolCapabilityTag::WebFetch,
                ToolCapabilityTag::CodeExecution,
                ToolCapabilityTag::HostedShell,
                ToolCapabilityTag::ProviderApplyPatch,
                ToolCapabilityTag::ComputerUse,
                ToolCapabilityTag::ToolSearch,
                ToolCapabilityTag::Memory,
                ToolCapabilityTag::ServerSideMcp,
            ]
            .into_iter()
            .collect(),
            citations: true,
            reasoning: ReasoningStateCapability {
                state: ReasoningStateDescriptor {
                    requires_signature: true,
                    requires_encrypted: true,
                    supports_interleaved: true,
                },
                supports_granular_effort: true,
            },
        }
    }

    fn request() -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: agent_domain::RequestId::from("r1"),
            model: agent_domain::ModelId::from("claude-sonnet-4-5"),
            messages: vec![Message {
                id: MessageId::new("m1"),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent { text: "hi".into() })],
                metadata: MessageMetadata::default(),
            }],
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            reasoning: None,
            temperature: Some(0.5),
            max_output_tokens: Some(128),
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: PromptCachePreference::Automatic,
            budget: RequestBudget::default(),
            provider_options: Default::default(),
            trace_id: None,
        }
    }

    #[test]
    fn plain_request_stays_on_legacy_path_without_notes() {
        let (choice, resolution) = resolve(&request(), Some(&caps(ModelTransport::Messages)));
        assert_eq!(choice, TransportChoice::Legacy);
        assert!(resolution.notes.is_empty());
    }

    #[test]
    fn modern_need_requires_declared_messages_transport() {
        let mut req = request();
        req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::High));
        let (choice, resolution) = resolve(&req, Some(&caps(ModelTransport::ChatCompletions)));
        assert_eq!(choice, TransportChoice::Legacy);
        assert_eq!(resolution.notes.len(), 1);
        assert!(resolution.notes[0].contains("legacy baseline"));
    }

    #[test]
    fn modern_need_with_unknown_model_fails_closed_to_legacy() {
        let mut req = request();
        req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::Medium));
        let (choice, resolution) = resolve(&req, None);
        assert_eq!(choice, TransportChoice::Legacy);
        assert!(resolution.notes[0].contains("not in adapter catalog"));
    }

    #[test]
    fn undeclared_server_tools_degrade_to_function_calling() {
        let mut req = request();
        req.hosted_tools.push(HostedToolRequest {
            name: "web_search".into(),
            kind: ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        let mut narrow = caps(ModelTransport::Messages);
        narrow.hosted_tool_tags.clear();
        let (choice, resolution) = resolve(&req, Some(&narrow));
        assert_eq!(choice, TransportChoice::Modern);
        assert_eq!(resolution.dropped_tools, vec!["web_search"]);
        assert!(resolution.hosted_tools.is_empty());
        assert!(resolution.notes[0].contains("degraded to client function calling"));
    }

    #[test]
    fn unrepresentable_kinds_are_explicitly_dropped() {
        let mut req = request();
        req.hosted_tools.push(HostedToolRequest {
            name: "advisor".into(),
            kind: ToolCapabilityTag::FileOrCollectionSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        req.hosted_tools.push(HostedToolRequest {
            name: "x_search".into(),
            kind: ToolCapabilityTag::XSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        let (_, resolution) = resolve(&req, Some(&caps(ModelTransport::Messages)));
        // advisor 经 canonical 名称回退映射；x_search 无法表达 → 显式降级。
        assert_eq!(resolution.hosted_tools.len(), 1);
        assert_eq!(resolution.hosted_tools[0]["type"], ADVISOR_TOOL);
        assert_eq!(resolution.dropped_tools, vec!["x_search"]);
        assert_eq!(resolution.notes.len(), 1);
    }

    #[test]
    fn xhigh_clamps_to_high_when_granular_effort_undeclared() {
        let mut req = request();
        req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::XHigh));
        let mut narrow = caps(ModelTransport::Messages);
        narrow.reasoning.supports_granular_effort = false;
        let (_, resolution) = resolve(&req, Some(&narrow));
        assert_eq!(
            resolution.thinking,
            ThinkingPlan::Adaptive {
                effort: Some("high".into())
            }
        );
    }

    #[test]
    fn unsupported_reasoning_clamps_to_legacy_budget() {
        let mut req = request();
        req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::Max));
        let mut no_reasoning = caps(ModelTransport::Messages);
        no_reasoning.thinking = false;
        no_reasoning.reasoning = ReasoningStateCapability::default();
        let (_, resolution) = resolve(&req, Some(&no_reasoning));
        assert!(matches!(
            resolution.thinking,
            ThinkingPlan::Budget(config) if config.level == ThinkingLevel::High
        ));
    }

    #[test]
    fn body_maps_native_structured_output_effort_and_adaptive_thinking() {
        let mut req = request();
        req.response_format = ResponseFormat::JsonSchema {
            name: "answer".into(),
            schema: json!({"type":"object","required":["ok"]}),
        };
        req.reasoning = Some(ReasoningConfig::new(ReasoningEffort::High));
        let (choice, resolution) = resolve(&req, Some(&caps(ModelTransport::Messages)));
        assert_eq!(choice, TransportChoice::Modern);
        let body =
            to_modern_messages_body(&req, &resolution, &BTreeMap::new()).expect("modern body maps");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["format"]["name"], "answer");
        assert_eq!(
            body["output_config"]["format"]["schema"]["required"],
            json!(["ok"])
        );
        // 原生 output_config 不再注入 system JSON 指令。
        assert!(body.get("system").is_none());
    }

    #[test]
    fn body_maps_legacy_thinking_budget_when_resolution_says_so() {
        let mut req = request();
        req.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: None,
        });
        // 纯 thinking 不触发现代路径（P6-2 已覆盖）；借 hosted tool 强制走现代，
        // 验证现代路径对 legacy budget 模式的 wire 映射。
        req.hosted_tools.push(HostedToolRequest {
            name: "web_search".into(),
            kind: ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        let (_, resolution) = resolve(&req, Some(&caps(ModelTransport::Messages)));
        assert!(matches!(resolution.thinking, ThinkingPlan::Budget(_)));
        let body =
            to_modern_messages_body(&req, &resolution, &BTreeMap::new()).expect("modern body maps");
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 127);
    }

    #[test]
    fn body_declares_server_and_client_tools_without_mixing() {
        let mut req = request();
        req.hosted_tools.push(HostedToolRequest {
            name: "web_search".into(),
            kind: ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        req.hosted_tools.push(HostedToolRequest {
            name: "computer_use".into(),
            kind: ToolCapabilityTag::ComputerUse,
            description: String::new(),
            capabilities: Vec::new(),
            config: Some(json!({"display_width_px": 1280, "display_height_px": 800})),
        });
        req.tools.push(ToolDefinition {
            name: "read_file".into(),
            description: "read".into(),
            input_schema: json!({"type":"object"}),
        });
        let (_, resolution) = resolve(&req, Some(&caps(ModelTransport::Messages)));
        let body =
            to_modern_messages_body(&req, &resolution, &BTreeMap::new()).expect("modern body maps");
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["type"], WEB_SEARCH_TOOL);
        assert_eq!(tools[1]["type"], COMPUTER_TOOL);
        assert_eq!(tools[1]["display_width_px"], 1280);
        assert_eq!(tools[2]["name"], "read_file");
        assert_eq!(tools[2]["input_schema"]["type"], "object");
    }

    #[test]
    fn body_rehydrates_thinking_blocks_with_signature() {
        let mut req = request();
        let item_id = ReasoningItemId::from("reasoning-1");
        let payload = AnthropicThinkingPayload::Thinking {
            signature: "SIG-ROUNDTRIP".into(),
        };
        let mut continuations = BTreeMap::new();
        continuations.insert(item_id.clone(), payload);
        let item = crate::reasoning::build_reasoning_item(
            item_id.clone(),
            ProtectedBlobRef::from("blob-1"),
            continuations.get(&item_id).expect("payload present"),
        );
        req.messages.push(Message {
            id: MessageId::new("a1"),
            role: MessageRole::Assistant,
            content: vec![
                ContentPart::Thinking(ThinkingContent {
                    text: "let me think".into(),
                    reasoning_item_id: Some(item_id.clone()),
                    redacted: false,
                }),
                ContentPart::Reasoning(item),
                ContentPart::Text(TextContent {
                    text: "answer".into(),
                }),
            ],
            metadata: MessageMetadata::default(),
        });
        let (_, resolution) = resolve(&req, Some(&caps(ModelTransport::Messages)));
        let body =
            to_modern_messages_body(&req, &resolution, &continuations).expect("modern body maps");
        let assistant = &body["messages"][1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["content"][0]["type"], "thinking");
        assert_eq!(assistant["content"][0]["thinking"], "let me think");
        assert_eq!(assistant["content"][0]["signature"], "SIG-ROUNDTRIP");
        assert_eq!(assistant["content"][1]["type"], "text");
    }

    #[test]
    fn body_rejects_missing_continuation_payload() {
        let mut req = request();
        let item = crate::reasoning::build_reasoning_item(
            ReasoningItemId::from("reasoning-missing"),
            ProtectedBlobRef::from("blob-missing"),
            &AnthropicThinkingPayload::Thinking {
                signature: "SIG".into(),
            },
        );
        req.messages.push(Message {
            id: MessageId::new("a1"),
            role: MessageRole::Assistant,
            content: vec![ContentPart::Reasoning(item)],
            metadata: MessageMetadata::default(),
        });
        let (_, resolution) = resolve(&req, Some(&caps(ModelTransport::Messages)));
        let error = to_modern_messages_body(&req, &resolution, &BTreeMap::new())
            .expect_err("missing payload fails closed");
        assert!(error.to_string().contains("reasoning-missing"));
    }

    #[test]
    fn web_search_result_block_maps_lifecycle_events() {
        let events = server_tool_result_block_to_events(&json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_1",
            "content": [{
                "type": "web_search_result",
                "title": "Pawork",
                "url": "https://pawork.dev"
            }]
        }))
        .expect("map web search result");
        assert!(matches!(
            &events[0],
            ServerToolEvent::SourceAdded { tool_call_id, source }
                if tool_call_id.as_str() == "srvtoolu_1"
                    && source.url.as_deref() == Some("https://pawork.dev")
        ));
        assert!(matches!(
            &events[1],
            ServerToolEvent::Completed { tool_call_id, .. }
                if tool_call_id.as_str() == "srvtoolu_1"
        ));
    }

    #[test]
    fn code_execution_result_block_maps_program_lifecycle() {
        let events = server_tool_result_block_to_events(&json!({
            "type": "code_execution_tool_result",
            "tool_use_id": "srvtoolu_2",
            "content": [{"type": "text", "text": "ok"}]
        }))
        .expect("map code execution result");
        assert!(matches!(
            events.as_slice(),
            [
                ServerToolEvent::ProgramStarted { tool_call_id, .. },
                ServerToolEvent::ProgramOutput { tool_call_id: _, delta: Some(delta), .. },
                ServerToolEvent::Completed { tool_call_id: _, .. },
            ] if tool_call_id.as_str() == "srvtoolu_2" && delta == "ok"
        ));
    }

    #[test]
    fn error_object_maps_to_failed() {
        let events = server_tool_result_block_to_events(&json!({
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_3",
            "content": {
                "type": "web_search_tool_result_error",
                "error_code": "max_uses_exceeded"
            }
        }))
        .expect("map error object");
        assert!(matches!(
            events.as_slice(),
            [ServerToolEvent::Failed { tool_call_id, code: Some(code), .. }]
                if tool_call_id.as_str() == "srvtoolu_3" && code == "max_uses_exceeded"
        ));
    }

    #[test]
    fn unknown_result_block_type_is_unsupported() {
        let error = server_tool_result_block_to_events(&json!({
            "type": "mystery_tool_result",
            "tool_use_id": "x",
            "content": []
        }))
        .expect_err("unknown result type rejects");
        assert!(error.to_string().contains("mystery_tool_result"));
    }

    #[test]
    fn transcript_envelope_reconstructs_native_blocks() {
        let envelope = ProviderTranscriptEnvelope {
            items: vec![
                TranscriptItem::ServerTool(ServerToolEvent::Started {
                    tool_call_id: ToolCallId::from("srvtoolu_1"),
                    name: "web_search".into(),
                    arguments: Some(json!({"query": "pawork"})),
                }),
                TranscriptItem::ServerTool(ServerToolEvent::SourceAdded {
                    tool_call_id: ToolCallId::from("srvtoolu_1"),
                    source: agent_domain::Source {
                        url: Some("https://pawork.dev".into()),
                        title: Some("Pawork".into()),
                        ..Default::default()
                    },
                }),
                TranscriptItem::ServerTool(ServerToolEvent::Completed {
                    tool_call_id: ToolCallId::from("srvtoolu_1"),
                    summary: Some("found 1".into()),
                    artifacts: Vec::new(),
                }),
            ],
            cursor: None,
            continuation_reference: None,
        };
        let blocks = transcript_to_wire_blocks(&envelope).expect("reconstruct envelope");
        assert_eq!(blocks.assistant.len(), 1);
        assert_eq!(blocks.assistant[0]["type"], "server_tool_use");
        assert_eq!(blocks.assistant[0]["name"], "web_search");
        assert_eq!(blocks.assistant[0]["input"]["query"], "pawork");
        assert_eq!(blocks.user.len(), 1);
        assert_eq!(blocks.user[0]["type"], "web_search_tool_result");
        assert_eq!(blocks.user[0]["tool_use_id"], "srvtoolu_1");
        assert_eq!(blocks.user[0]["content"][0]["type"], "web_search_result");
        assert_eq!(blocks.user[0]["content"][0]["url"], "https://pawork.dev");
        assert_eq!(blocks.user[0]["content"][1]["type"], "text");
    }

    #[test]
    fn transcript_never_produces_client_tool_result() {
        let envelope = ProviderTranscriptEnvelope {
            items: vec![TranscriptItem::ServerTool(ServerToolEvent::Completed {
                tool_call_id: ToolCallId::from("srvtoolu_1"),
                summary: Some("done".into()),
                artifacts: Vec::new(),
            })],
            cursor: None,
            continuation_reference: None,
        };
        let blocks = transcript_to_wire_blocks(&envelope).expect("reconstruct envelope");
        let encoded = serde_json::to_string(&blocks).expect("serialize blocks");
        assert!(
            !encoded.contains("\"type\":\"tool_result\""),
            "server tool continuation must never emit client tool_result: {encoded}"
        );
    }

    #[test]
    fn whitelist_covers_canonical_and_wire_names() {
        let mut req = request();
        req.hosted_tools.push(HostedToolRequest {
            name: "web_search".into(),
            kind: ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        let names = server_tool_whitelist(&req);
        assert!(names.contains(&"web_search".to_string()));
        assert!(names.contains(&WEB_SEARCH_TOOL.to_string()));
    }

    #[test]
    fn assistant_tool_call_and_result_still_use_client_paths() {
        // 客户端 function 位点不受现代路径影响。
        let mut req = request();
        req.messages.push(Message {
            id: MessageId::new("a1"),
            role: MessageRole::Assistant,
            content: vec![ContentPart::ToolCall(ToolCallContent {
                id: ToolCallId::from("call-1"),
                name: "read_file".into(),
                arguments: json!({"path": "a"}),
                raw_arguments: None,
                complete: true,
            })],
            metadata: MessageMetadata::default(),
        });
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
        let (_, resolution) = resolve(&req, Some(&caps(ModelTransport::Messages)));
        let body =
            to_modern_messages_body(&req, &resolution, &BTreeMap::new()).expect("modern body maps");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
    }

    #[test]
    fn image_base64_still_maps_to_image_block() {
        let mut req = request();
        req.messages[0]
            .content
            .push(ContentPart::Image(ImageContent {
                source: agent_domain::ImageSource::Base64("QkFTRQ==".into()),
                media_type: "image/png".into(),
                alt_text: None,
            }));
        let (_, resolution) = resolve(&req, Some(&caps(ModelTransport::Messages)));
        let body =
            to_modern_messages_body(&req, &resolution, &BTreeMap::new()).expect("modern body maps");
        assert_eq!(body["messages"][0]["content"][1]["type"], "image");
    }
}
