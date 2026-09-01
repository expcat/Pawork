//! 外部 Agent 会话兼容导入解析（P16-9）：Claude / Codex / Grok / Cursor。
//!
//! 最终目的：把其他智能体工具的外部会话无损导入为 Pawork 的 canonical event，
//! 使既有对话与产物可被重放、检索与续接，而**绝不破坏 canonical event 模型**——
//! 外部格式始终是输入侧的适配/投影，导入产物是规范事件，不污染、不覆写既有事件。
//!
//! 本模块只含解析、指纹、映射与结构校验纯函数；事务写入在 persist 侧。
//! 本包会话 [`ExternalSource`] 只有 Claude / Codex / Grok / Cursor（无配置 Pi）。

use std::collections::BTreeMap;

use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ContentPart, EventId, EventSequence, Message, MessageId,
    MessageMetadata, MessageRole, ReviewAnchor, ReviewEvent, ReviewFindingId, ReviewSessionId,
    ReviewSeverity, RunId, SessionId, StopReason, TextContent, Timestamp, TokenUsage, ToolCallId,
    ToolResultContent,
};
use serde_json::Value;

use crate::session::SessionStoreError;

// =========================================================================
// 来源标识
// =========================================================================

/// 外部会话来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalSource {
    /// Anthropic Claude 导出（JSON，`chat_messages` / `conversation_id`）。
    Claude,
    /// Codex rollout（JSONL，每行一个 typed entry）。
    Codex,
    /// xAI Grok 导出（JSON，`messages`）。
    Grok,
    /// Cursor 导出（JSON，`messages` / bubbles）。
    Cursor,
}

impl ExternalSource {
    /// 稳定的小写字符串标签（用于指纹、event id 与日志，不含平台特例逻辑）。
    pub const fn as_str(self) -> &'static str {
        match self {
            ExternalSource::Claude => "claude",
            ExternalSource::Codex => "codex",
            ExternalSource::Grok => "grok",
            ExternalSource::Cursor => "cursor",
        }
    }
}

impl std::fmt::Display for ExternalSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =========================================================================
// 归一化中间表示（来源无关）
// =========================================================================

/// 来源无关的外部记录。解析器把各家格式归一到这里，再统一映射到 canonical event。
#[derive(Clone, Debug, PartialEq)]
pub enum ExternalRecord {
    UserMessage {
        text: String,
    },
    AssistantMessage {
        text: String,
    },
    ToolCall {
        tool_call_id: String,
        name: String,
        arguments: Option<String>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
        is_error: bool,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// 评审类意见（带 file:line 锚点）。
    ReviewComment {
        file: String,
        line: u32,
        severity: ReviewSeverity,
        body: String,
    },
    /// 无法映射到任何 canonical 结构的记录：进 `Diagnostic` raw metadata。
    Raw {
        kind: String,
        payload: Value,
    },
}

/// 单来源解析结果。
#[derive(Clone, Debug, Default)]
pub struct ParsedExternalSession {
    pub source: Option<ExternalSource>,
    /// 外部会话原始 id（`conversation_id` / `id` / rollout uuid 等）。
    pub original_id: Option<String>,
    pub title: Option<String>,
    pub records: Vec<ExternalRecord>,
    /// 顶层未被识别的字段（key -> JSON 值字符串），保证向前兼容。
    pub unknown_fields: BTreeMap<String, String>,
}

// =========================================================================
// Secret 扫描（untrusted 输入拒绝策略）
// =========================================================================

/// 已知凭证前缀与其后续 token 最小长度。命中即视为 Secret。
const SECRET_SIGNATURES: &[(&str, usize)] = &[
    ("sk-ant-", 20),     // Anthropic
    ("sk-proj-", 30),    // OpenAI project key
    ("sk-", 28),         // OpenAI classic
    ("AKIA", 16),        // AWS access key id
    ("ghp_", 30),        // GitHub PAT
    ("gho_", 30),        // GitHub OAuth
    ("ghs_", 30),        // GitHub server-to-server
    ("github_pat_", 30), // GitHub fine-grained
    ("xoxb-", 20),       // Slack bot token
    ("xoxp-", 20),       // Slack user token
    ("xoxa-", 20),       // Slack app token
    ("xoxe-", 20),       // Slack exchange token
    ("AIza", 30),        // Google API key
];

/// 扫描文本中是否包含高置信凭证；命中返回其前缀标签（用于错误信息）。
pub fn find_secret(text: &str) -> Option<&'static str> {
    for &(prefix, min_tail) in SECRET_SIGNATURES {
        let mut from = 0;
        while let Some(idx) = text[from..].find(prefix) {
            let start = from + idx + prefix.len();
            let tail = text[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .count();
            if tail >= min_tail {
                return Some(prefix);
            }
            from += idx + prefix.len();
        }
    }
    // Bearer <长 token>
    let mut from = 0;
    while let Some(idx) = text[from..].find("Bearer ") {
        let start = from + idx + "Bearer ".len();
        let tail = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            .count();
        if tail >= 20 {
            return Some("Bearer ");
        }
        from += idx + "Bearer ".len();
    }
    None
}

// =========================================================================
// 解析器
// =========================================================================

/// 按来源派发解析。
pub fn parse_external(
    source: ExternalSource,
    content: &str,
) -> Result<ParsedExternalSession, SessionStoreError> {
    match source {
        ExternalSource::Claude => parse_claude(content),
        ExternalSource::Codex => parse_codex(content),
        ExternalSource::Grok => parse_grok(content),
        ExternalSource::Cursor => parse_cursor(content),
    }
}

const CLAUDE_KNOWN: &[&str] = &[
    "conversation_id",
    "id",
    "name",
    "title",
    "chat_messages",
    "messages",
    "sender",
    "role",
    "text",
    "content",
    "created_at",
    "type",
    "uuid",
];
const CONV_KNOWN: &[&str] = &[
    "id",
    "title",
    "name",
    "messages",
    "chat_messages",
    "bubbles",
    "conversation",
    "role",
    "sender",
    "type",
    "text",
    "content",
    "richText",
    "created_at",
    "updated_at",
    "version",
    "model",
];

/// Claude 解析:优先按 claude.ai 导出 JSON(顶层 `chat_messages`/`messages` 数组),
/// 否则回落到 Claude Code 本地 JSONL 逐行解析。
pub fn parse_claude(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
    if let Ok(value) = serde_json::from_str::<Value>(content) {
        if let Some(obj) = value.as_object() {
            let is_export = ["chat_messages", "messages"].iter().any(|key| {
                obj.get(*key)
                    .is_some_and(|messages| messages.as_array().is_some())
            });
            if is_export {
                return parse_claude_export(obj);
            }
        }
    }
    parse_claude_local_jsonl(content)
}

/// claude.ai 导出 JSON(旧路径,行为不变)。
fn parse_claude_export(
    obj: &serde_json::Map<String, Value>,
) -> Result<ParsedExternalSession, SessionStoreError> {
    let original_id = obj
        .get("conversation_id")
        .and_then(Value::as_str)
        .or_else(|| obj.get("uuid").and_then(Value::as_str))
        .map(String::from);
    let title = obj
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| obj.get("title").and_then(Value::as_str))
        .map(String::from);

    let messages = obj
        .get("chat_messages")
        .or_else(|| obj.get("messages"))
        .and_then(Value::as_array);
    let mut records = Vec::new();
    if let Some(msgs) = messages {
        for msg in msgs {
            records.push(record_from_message(msg, "sender"));
        }
    }
    Ok(ParsedExternalSession {
        source: Some(ExternalSource::Claude),
        original_id,
        title,
        records,
        unknown_fields: collect_unknown(obj, CLAUDE_KNOWN),
    })
}

/// Claude Code 本地 JSONL(`~/.claude/projects/**/*.jsonl`)逐行解析。
///
/// 映射裁决(R6 波 C 设计):user/assistant 且非 sidechain 才记录;content parts 中
/// text 拼接、tool_use/tool_result 配对、thinking 跳过并计数;ai-title/custom-title
/// 取标题;queue-operation/last-prompt 跳过并计数;sidechain 行跳过并计数——四类
/// 噪声统一以 `skipped_*` 键写入 unknown_fields(与 codex 侧口径一致);其余未知
/// type 进 Raw(无损哲学不变)。
fn parse_claude_local_jsonl(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
    let mut parsed = ParsedExternalSession {
        source: Some(ExternalSource::Claude),
        ..Default::default()
    };
    let mut skipped: BTreeMap<String, u64> = BTreeMap::new();
    let mut raw_type_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut unparseable_lines = 0u64;
    for (idx, raw) in content.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(e) => {
                unparseable_lines += 1;
                parsed
                    .unknown_fields
                    .insert(format!("line:{}", idx + 1), format!("unparseable: {e}"));
                continue;
            }
        };
        let Some(obj) = value.as_object() else {
            parsed.records.push(ExternalRecord::Raw {
                kind: format!("claude.line:{}", idx + 1),
                payload: value.clone(),
            });
            continue;
        };
        if parsed.original_id.is_none() {
            parsed.original_id = obj
                .get("sessionId")
                .and_then(Value::as_str)
                .map(String::from);
        }
        let line_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
        match line_type {
            "user" | "assistant" => {
                if obj
                    .get("isSidechain")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    *skipped.entry("skipped_sidechain".into()).or_default() += 1;
                    continue;
                }
                claude_local_message_records(
                    obj,
                    line_type,
                    idx,
                    &mut parsed.records,
                    &mut skipped,
                );
            }
            "ai-title" | "custom-title" => {
                // 真实本地格式的标题键是 aiTitle / customTitle(2026-08-23 本机
                // 样本核实);保留 title 兜底以兼容手工构造的导出。
                let title_key = if line_type == "ai-title" {
                    "aiTitle"
                } else {
                    "customTitle"
                };
                let title = obj
                    .get(title_key)
                    .and_then(Value::as_str)
                    .or_else(|| obj.get("title").and_then(Value::as_str));
                if let Some(title) = title {
                    parsed.title = Some(title.to_string());
                }
            }
            "queue-operation" => {
                *skipped.entry("skipped_queue_operation".into()).or_default() += 1;
            }
            "last-prompt" => {
                *skipped.entry("skipped_last_prompt".into()).or_default() += 1;
            }
            other => {
                *raw_type_counts.entry(other.to_string()).or_default() += 1;
                parsed.records.push(ExternalRecord::Raw {
                    kind: format!("claude.type:{other}"),
                    payload: value.clone(),
                });
            }
        }
    }
    for (key, count) in skipped {
        if count > 0 {
            parsed.unknown_fields.insert(key, count.to_string());
        }
    }
    for (kind, count) in raw_type_counts {
        parsed
            .unknown_fields
            .insert(format!("raw_type:{kind}"), count.to_string());
    }
    // 零记录且存在解析失败行:大概率是损坏/错误来源文件,fail-closed 拒绝导入;
    // 全为合法噪声/跳过行(sidechain/title/queue-operation 等)时维持 Ok 空导入。
    if parsed.records.is_empty() && unparseable_lines > 0 {
        return Err(unparseable_msg(
            "claude",
            &format!("no records parsed; {unparseable_lines} unparseable line(s)"),
        ));
    }
    Ok(parsed)
}

/// 把 Claude Code 本地行的 `message.content` 映射为记录序列。
///
/// 文本 parts 先缓冲;遇到 tool_use/tool_result 时先冲刷已缓冲文本,保持
/// [text, tool_use] 的自然顺序。thinking 跳过并计入 `skipped`;未知 part
/// 静默跳过。
fn claude_local_message_records(
    obj: &serde_json::Map<String, Value>,
    line_type: &str,
    idx: usize,
    records: &mut Vec<ExternalRecord>,
    skipped: &mut BTreeMap<String, u64>,
) {
    let Some(message) = obj.get("message").and_then(Value::as_object) else {
        records.push(ExternalRecord::Raw {
            kind: format!("claude.type:{line_type}"),
            payload: Value::Object(obj.clone()),
        });
        return;
    };
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .and_then(parse_role_name)
        .unwrap_or(if line_type == "assistant" {
            MessageRole::Assistant
        } else {
            MessageRole::User
        });
    let mut text = String::new();
    let mut text_pending = false;
    match message.get("content") {
        Some(Value::String(s)) => {
            text.push_str(s);
            text_pending = true;
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                let Some(part_obj) = part.as_object() else {
                    continue;
                };
                match part_obj.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(t) = part_obj.get("text").and_then(Value::as_str) {
                            text.push_str(t);
                            text_pending = true;
                        }
                    }
                    "thinking" => {
                        *skipped.entry("skipped_thinking".into()).or_default() += 1;
                    }
                    "tool_use" => {
                        flush_claude_text(&mut text, &mut text_pending, &role, records);
                        let tool_call_id = part_obj
                            .get("id")
                            .and_then(Value::as_str)
                            .map(String::from)
                            .unwrap_or_else(|| format!("claude-call-{}", idx + 1));
                        let name = part_obj
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let arguments = part_obj.get("input").map(|input| input.to_string());
                        records.push(ExternalRecord::ToolCall {
                            tool_call_id,
                            name,
                            arguments,
                        });
                    }
                    "tool_result" => {
                        flush_claude_text(&mut text, &mut text_pending, &role, records);
                        let tool_call_id = part_obj
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .map(String::from)
                            .unwrap_or_else(|| format!("claude-call-{}", idx + 1));
                        let content = claude_tool_result_text(part_obj);
                        let is_error = part_obj
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        records.push(ExternalRecord::ToolResult {
                            tool_call_id,
                            content,
                            is_error,
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    flush_claude_text(&mut text, &mut text_pending, &role, records);
}

fn flush_claude_text(
    text: &mut String,
    text_pending: &mut bool,
    role: &MessageRole,
    records: &mut Vec<ExternalRecord>,
) {
    if !*text_pending || text.is_empty() {
        text.clear();
        *text_pending = false;
        return;
    }
    let buffer = std::mem::take(text);
    *text_pending = false;
    records.push(match role {
        MessageRole::Assistant => ExternalRecord::AssistantMessage { text: buffer },
        _ => ExternalRecord::UserMessage { text: buffer },
    });
}

/// tool_result 的 content 兼容 string 与 text blocks 数组两种形态。
fn claude_tool_result_text(part_obj: &serde_json::Map<String, Value>) -> String {
    match part_obj.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            let mut buffer = String::new();
            for part in parts {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    buffer.push_str(t);
                }
            }
            buffer
        }
        _ => String::new(),
    }
}

/// Grok 导出 JSON 解析。
pub fn parse_grok(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
    parse_json_conversation(content, ExternalSource::Grok, "grok")
}

/// Cursor 导出 JSON 解析。
pub fn parse_cursor(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
    parse_json_conversation(content, ExternalSource::Cursor, "cursor")
}

fn parse_json_conversation(
    content: &str,
    source: ExternalSource,
    label: &'static str,
) -> Result<ParsedExternalSession, SessionStoreError> {
    let value: Value = serde_json::from_str(content).map_err(|e| unparseable(label, e))?;
    // 顶层可能是数组（纯消息列表）或对象。
    let (original_id, title, messages, unknown) = match &value {
        Value::Array(arr) => (None, None, Some(arr), BTreeMap::new()),
        Value::Object(obj) => {
            let original_id = obj
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| obj.get("conversation_id").and_then(Value::as_str))
                .map(String::from);
            let title = obj
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| obj.get("name").and_then(Value::as_str))
                .map(String::from);
            let messages = obj
                .get("messages")
                .or_else(|| obj.get("chat_messages"))
                .or_else(|| obj.get("bubbles"))
                .and_then(Value::as_array);
            (
                original_id,
                title,
                messages,
                collect_unknown(obj, CONV_KNOWN),
            )
        }
        _ => return Err(unparseable_msg(label, "expected JSON object or array")),
    };

    let mut records = Vec::new();
    if let Some(msgs) = messages {
        for msg in msgs {
            records.push(record_from_message(msg, "role"));
        }
    }
    Ok(ParsedExternalSession {
        source: Some(source),
        original_id,
        title,
        records,
        unknown_fields: unknown,
    })
}

/// Codex rollout 解析:首非空行同时含 `timestamp`+`type`+`payload` 时走信封模式,
/// 否则维持旧的平铺 typed entry JSONL 路径(逐字节不变)。
pub fn parse_codex(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
    if is_codex_envelope(content) {
        return parse_codex_envelope(content);
    }
    parse_codex_flat(content)
}

fn is_codex_envelope(content: &str) -> bool {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .and_then(|value| {
            value.as_object().map(|obj| {
                obj.contains_key("timestamp")
                    && obj.contains_key("type")
                    && obj.contains_key("payload")
            })
        })
        .unwrap_or(false)
}

/// 旧平铺 typed entry JSONL(行为不变)。
fn parse_codex_flat(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
    let mut parsed = ParsedExternalSession {
        source: Some(ExternalSource::Codex),
        ..Default::default()
    };
    let mut original_id: Option<String> = None;
    for (idx, raw) in content.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                parsed
                    .unknown_fields
                    .insert(format!("line:{}", idx + 1), format!("unparseable: {e}"));
                continue;
            }
        };
        let Some(obj) = value.as_object() else {
            parsed.records.push(ExternalRecord::Raw {
                kind: format!("codex.line:{}", idx + 1),
                payload: value.clone(),
            });
            continue;
        };
        // rollout 元信息（session/rollout id）
        if original_id.is_none() {
            original_id = obj
                .get("session_id")
                .and_then(Value::as_str)
                .or_else(|| obj.get("rollout_id").and_then(Value::as_str))
                .map(String::from);
        }
        let entry_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
        match entry_type {
            "message" => parsed.records.push(record_from_message(&value, "role")),
            "function_call" => {
                let call_id = obj
                    .get("call_id")
                    .or_else(|| obj.get("tool_call_id"))
                    .and_then(Value::as_str)
                    .map(String::from);
                let name = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let arguments = obj
                    .get("arguments")
                    .and_then(Value::as_str)
                    .map(String::from);
                parsed.records.push(ExternalRecord::ToolCall {
                    tool_call_id: call_id.unwrap_or_else(|| format!("codex-call-{}", idx + 1)),
                    name,
                    arguments,
                });
            }
            "function_call_output" => {
                let call_id = obj
                    .get("call_id")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .unwrap_or_else(|| format!("codex-call-{}", idx + 1));
                let output = extract_output_text(&value);
                let is_error = obj
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                parsed.records.push(ExternalRecord::ToolResult {
                    tool_call_id: call_id,
                    content: output.unwrap_or_default(),
                    is_error,
                });
            }
            "usage" => {
                let input = obj.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                let output = obj
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                parsed.records.push(ExternalRecord::Usage {
                    input_tokens: input,
                    output_tokens: output,
                });
            }
            other => {
                parsed.records.push(ExternalRecord::Raw {
                    kind: format!("codex.type:{other}"),
                    payload: value.clone(),
                });
            }
        }
    }
    parsed.original_id = original_id;
    Ok(parsed)
}

/// Codex rollout 信封模式(`{timestamp,type,payload}` 逐行)。
///
/// 映射裁决(R6 波 C 设计):`session_meta.payload.id` 取 original_id;
/// `response_item` 按 payload.type 派发(message 只映射 user/assistant,developer/system
/// 跳过;agent_message/user_message → AssistantMessage/UserMessage,文本提取复用
/// codex_message_text,无文本不落记录;function_call/custom_tool_call → ToolCall;两类
/// output → ToolResult;reasoning 跳过;未知 payload.type → Raw);`event_msg` 只取
/// token_count → Usage,其余与
/// response_item 镜像的条目静默跳过防重复;turn_context/world_state/
/// inter_agent_communication_metadata 跳过;跳过项在 unknown_fields 记 `skipped_*` 计数。
fn parse_codex_envelope(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
    let mut parsed = ParsedExternalSession {
        source: Some(ExternalSource::Codex),
        ..Default::default()
    };
    let mut skipped: BTreeMap<String, u64> = BTreeMap::new();
    let mut unparseable_lines = 0u64;
    for (idx, raw) in content.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(e) => {
                unparseable_lines += 1;
                parsed
                    .unknown_fields
                    .insert(format!("line:{}", idx + 1), format!("unparseable: {e}"));
                continue;
            }
        };
        let Some(obj) = value.as_object() else {
            parsed.records.push(ExternalRecord::Raw {
                kind: format!("codex.line:{}", idx + 1),
                payload: value.clone(),
            });
            continue;
        };
        let envelope_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
        let payload = obj.get("payload").cloned().unwrap_or(Value::Null);
        match envelope_type {
            "session_meta" => {
                if parsed.original_id.is_none() {
                    parsed.original_id =
                        payload.get("id").and_then(Value::as_str).map(String::from);
                }
            }
            "response_item" => {
                codex_response_item_records(&payload, idx, &mut parsed.records, &mut skipped);
            }
            "event_msg" => {
                let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
                if payload_type == "token_count" {
                    parsed.records.push(codex_token_count_usage(&payload));
                } else {
                    *skipped.entry("skipped_event_msg".into()).or_default() += 1;
                }
            }
            "turn_context" => {
                *skipped.entry("skipped_turn_context".into()).or_default() += 1;
            }
            "world_state" => {
                *skipped.entry("skipped_world_state".into()).or_default() += 1;
            }
            "inter_agent_communication_metadata" => {
                *skipped
                    .entry("skipped_inter_agent_communication_metadata".into())
                    .or_default() += 1;
            }
            other => {
                parsed.records.push(ExternalRecord::Raw {
                    kind: format!("codex.type:{other}"),
                    payload: value.clone(),
                });
            }
        }
    }
    for (key, count) in skipped {
        parsed.unknown_fields.insert(key, count.to_string());
    }
    // 与 Claude 本地路径同一裁决:零记录 + 解析失败行 → fail-closed;
    // 全为合法跳过行(session_meta/reasoning/event_msg 镜像等)时维持 Ok 空导入。
    if parsed.records.is_empty() && unparseable_lines > 0 {
        return Err(unparseable_msg(
            "codex",
            &format!("no records parsed; {unparseable_lines} unparseable line(s)"),
        ));
    }
    Ok(parsed)
}

fn codex_response_item_records(
    payload: &Value,
    idx: usize,
    records: &mut Vec<ExternalRecord>,
    skipped: &mut BTreeMap<String, u64>,
) {
    let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
    match payload_type {
        "message" => {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            match role {
                "user" | "assistant" => {
                    if let Some(text) = codex_message_text(payload) {
                        records.push(if role == "assistant" {
                            ExternalRecord::AssistantMessage { text }
                        } else {
                            ExternalRecord::UserMessage { text }
                        });
                    }
                }
                "developer" | "system" => {
                    *skipped
                        .entry("skipped_developer_message".into())
                        .or_default() += 1;
                }
                _ => {
                    *skipped.entry("skipped_message".into()).or_default() += 1;
                }
            }
        }
        "agent_message" | "user_message" => {
            if let Some(text) = codex_message_text(payload) {
                records.push(if payload_type == "agent_message" {
                    ExternalRecord::AssistantMessage { text }
                } else {
                    ExternalRecord::UserMessage { text }
                });
            }
        }
        "function_call" | "custom_tool_call" => {
            let tool_call_id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| format!("codex-call-{}", idx + 1));
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let arguments = payload
                .get("arguments")
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| {
                    payload
                        .get("input")
                        .and_then(Value::as_str)
                        .map(String::from)
                });
            records.push(ExternalRecord::ToolCall {
                tool_call_id,
                name,
                arguments,
            });
        }
        "function_call_output" | "custom_tool_call_output" => {
            let tool_call_id = payload
                .get("call_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| format!("codex-call-{}", idx + 1));
            let content = payload
                .get("output")
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| {
                    payload
                        .get("content")
                        .and_then(Value::as_str)
                        .map(String::from)
                })
                .unwrap_or_default();
            let is_error = payload
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            records.push(ExternalRecord::ToolResult {
                tool_call_id,
                content,
                is_error,
            });
        }
        "reasoning" => {
            *skipped.entry("skipped_reasoning".into()).or_default() += 1;
        }
        other => {
            records.push(ExternalRecord::Raw {
                kind: format!("codex.payload:{other}"),
                payload: payload.clone(),
            });
        }
    }
}

/// message payload 的 content 拼接:仅 input_text/output_text(以及 string content),
/// encrypted_content 等其余 part 跳过;无任何文本 part 时不落空消息。
fn codex_message_text(payload: &Value) -> Option<String> {
    match payload.get("content") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => {
            let mut buffer = String::new();
            let mut saw_text = false;
            for part in parts {
                let Some(part_obj) = part.as_object() else {
                    continue;
                };
                match part_obj.get("type").and_then(Value::as_str).unwrap_or("") {
                    "input_text" | "output_text" => {
                        saw_text = true;
                        if let Some(t) = part_obj.get("text").and_then(Value::as_str) {
                            buffer.push_str(t);
                        }
                    }
                    _ => {}
                }
            }
            if saw_text {
                Some(buffer)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// token_count payload:优先平铺 input/output_tokens,其次 info.total_token_usage。
fn codex_token_count_usage(payload: &Value) -> ExternalRecord {
    let totals = payload
        .get("info")
        .and_then(|info| info.get("total_token_usage"));
    let input_tokens = payload
        .get("input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            totals
                .and_then(|t| t.get("input_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let output_tokens = payload
        .get("output_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            totals
                .and_then(|t| t.get("output_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    ExternalRecord::Usage {
        input_tokens,
        output_tokens,
    }
}

/// 把一条消息对象归一为 [`ExternalRecord`]。`role_key` 决定从 `role` 还是 `sender`
/// 字段读取角色（Claude 用 `sender`，其余用 `role`）。
fn record_from_message(msg: &Value, role_key: &str) -> ExternalRecord {
    // 评审类：带 file + line 的评论。
    if let (Some(file), Some(line)) = (
        msg.get("file")
            .or_else(|| msg.get("path"))
            .and_then(Value::as_str),
        msg.get("line").and_then(|v| v.as_u64()),
    ) {
        let body = extract_text_value(msg).unwrap_or_default();
        let severity = parse_severity(
            msg.get("severity")
                .or_else(|| msg.get("level"))
                .and_then(Value::as_str),
        );
        return ExternalRecord::ReviewComment {
            file: file.to_string(),
            line: u32::try_from(line).unwrap_or(0),
            severity,
            body,
        };
    }
    // 内嵌工具调用。
    if let Some(tc) = msg.get("tool_use").or_else(|| msg.get("function_call")) {
        if let Some(tc_obj) = tc.as_object() {
            let name = tc_obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let id = tc_obj
                .get("id")
                .or_else(|| tc_obj.get("call_id"))
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| format!("ext-{name}"));
            let arguments = tc_obj
                .get("arguments")
                .or_else(|| tc_obj.get("input"))
                .map(|v| v.to_string());
            return ExternalRecord::ToolCall {
                tool_call_id: id,
                name,
                arguments,
            };
        }
    }
    if let Some(calls) = msg.get("tool_calls").and_then(Value::as_array) {
        if let Some(first) = calls.first().and_then(|c| c.as_object()) {
            let function = first.get("function").and_then(Value::as_object);
            let name = function
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let id = first
                .get("id")
                .and_then(Value::as_str)
                .map(String::from)
                .unwrap_or_else(|| format!("ext-{name}"));
            let arguments = function
                .and_then(|f| f.get("arguments"))
                .map(|v| v.to_string());
            return ExternalRecord::ToolCall {
                tool_call_id: id,
                name,
                arguments,
            };
        }
    }
    // tool_result / function output。
    if msg.get("tool_result").is_some() || msg.get("output").is_some() {
        let call_id = msg
            .get("tool_use_id")
            .or_else(|| msg.get("call_id"))
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| "ext-tool".to_string());
        let content = extract_text_value(msg).unwrap_or_default();
        let is_error = msg
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        return ExternalRecord::ToolResult {
            tool_call_id: call_id,
            content,
            is_error,
        };
    }
    // 普通文本消息。
    let role = msg
        .get(role_key)
        .and_then(Value::as_str)
        .and_then(parse_role_name);
    let text = extract_text_value(msg).unwrap_or_default();
    // 既无角色又无文本：无法映射，进 raw（不猜测填充）。
    if role.is_none() && text.is_empty() {
        return ExternalRecord::Raw {
            kind: "message".into(),
            payload: msg.clone(),
        };
    }
    match role {
        Some(MessageRole::Assistant) => ExternalRecord::AssistantMessage { text },
        Some(MessageRole::System) => ExternalRecord::Raw {
            kind: "system".into(),
            payload: msg.clone(),
        },
        _ => ExternalRecord::UserMessage { text },
    }
}

fn parse_role_name(s: &str) -> Option<MessageRole> {
    match s.to_ascii_lowercase().as_str() {
        "system" => Some(MessageRole::System),
        "user" | "human" | "customer" => Some(MessageRole::User),
        "assistant" | "ai" | "model" | "bot" => Some(MessageRole::Assistant),
        "tool" | "function" => Some(MessageRole::Tool),
        _ => None,
    }
}

fn parse_severity(s: Option<&str>) -> ReviewSeverity {
    match s.map(|x| x.to_ascii_lowercase()).as_deref() {
        Some("critical") | Some("blocker") => ReviewSeverity::Critical,
        Some("major") | Some("error") => ReviewSeverity::Major,
        Some("minor") | Some("warning") | Some("warn") => ReviewSeverity::Minor,
        _ => ReviewSeverity::Info,
    }
}

/// 从消息对象尽量抽出文本（支持 string content / array of text blocks）。
fn extract_text_value(msg: &Value) -> Option<String> {
    if let Some(s) = msg.get("text").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    match msg.get("content") {
        Some(Value::String(s)) => return Some(s.clone()),
        Some(Value::Array(parts)) => {
            let mut buf = String::new();
            for part in parts {
                if let Some(t) = part.as_str() {
                    buf.push_str(t);
                } else if let Some(t) = part.get("text").and_then(Value::as_str) {
                    buf.push_str(t);
                } else if let Some(t) = part.get("content").and_then(Value::as_str) {
                    buf.push_str(t);
                }
            }
            if !buf.is_empty() {
                return Some(buf);
            }
        }
        _ => {}
    }
    if let Some(s) = msg.get("richText").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    None
}

/// 从 `function_call_output` 抽出 output 文本。
fn extract_output_text(value: &Value) -> Option<String> {
    let obj = value.as_object()?;
    if let Some(s) = obj.get("output").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    if let Some(s) = obj.get("content").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    None
}

fn collect_unknown(
    obj: &serde_json::Map<String, Value>,
    known: &[&str],
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if !known.contains(&k.as_str()) {
            out.insert(k.clone(), v.to_string());
        }
    }
    out
}

fn unparseable<E: std::fmt::Display>(source: &'static str, e: E) -> SessionStoreError {
    SessionStoreError::CompatUnparseable {
        source_label: source.into(),
        detail: e.to_string(),
    }
}

fn unparseable_msg(source: &'static str, detail: &str) -> SessionStoreError {
    SessionStoreError::CompatUnparseable {
        source_label: source.into(),
        detail: detail.into(),
    }
}

// =========================================================================
// 指纹与去重
// =========================================================================

/// Import identity 与 content fingerprint 分离：
/// - **identity** = `(source, effective_identity)`，其中 `effective_identity` 为
///   外部 `original_id`；无 `original_id` 时退化为 content fingerprint。identity 决定
///   目标 SessionId，是去重 / 冲突判定的唯一权威（持久化于 `compat_import_identity`）。
/// - **content fingerprint** = blake3(content)，记录该 identity 当前已导入的内容指纹；
///   同 identity 同指纹 → 幂等；同 identity 不同指纹 → 明确冲突，绝不静默创建第二 Session。
///
/// 这样 identity 不随内容变化漂移（修正了把 content 纳入 SessionId 导致去重失效的问题），
/// 同时仍能识别「无 original_id 的相同内容」为同一会话。
pub fn derive_compat_session_id(
    source: ExternalSource,
    original_id: Option<&str>,
    content: &str,
) -> SessionId {
    let effective = effective_identity(original_id, content);
    let mut hasher = blake3::Hasher::new();
    hasher.update(source.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(effective.as_bytes());
    let hash = hasher.finalize();
    SessionId::from(format!(
        "compat-{}-{}",
        source.as_str(),
        hex(&hash.as_bytes()[..16])
    ))
}

/// 导入 identity 的 effective key：有 `original_id` 用之，否则用 content fingerprint。
pub fn effective_identity(original_id: Option<&str>, content: &str) -> String {
    match original_id {
        Some(id) => id.to_string(),
        None => content_fingerprint(content),
    }
}

/// content 的 blake3 指纹（64 hex 字符）。
pub fn content_fingerprint(content: &str) -> String {
    blake3::hash(content.as_bytes()).to_hex().to_string()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// =========================================================================
// 字段映射：records → canonical AgentEventEnvelope 序列
// =========================================================================

/// 导入报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompatImportReport {
    pub source: Option<ExternalSource>,
    pub session_id: String,
    pub original_id: Option<String>,
    pub imported_events: usize,
    pub imported_messages: usize,
    pub imported_tool_calls: usize,
    pub imported_tool_results: usize,
    pub imported_usages: usize,
    pub imported_reviews: usize,
    pub raw_records: usize,
    /// 命中已有 Session（重复导入）时为 true，且 `imported_events == 0`。
    pub deduplicated: bool,
    pub unknown_fields: BTreeMap<String, String>,
}

/// 导入历史条目（一条 `(source, original_id)` identity 对应一次导入；
/// 重复导入命中 identity 时幂等去重，不新增条目）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatImportHistoryEntry {
    pub session_id: String,
    pub source: ExternalSource,
    pub original_id: Option<String>,
    pub imported_events: usize,
    /// 导入时间（unix ms；旧数据无真实时间戳时为 0，排序靠后）。
    pub imported_at_unix_ms: u64,
}

/// 导入历史分页结果。`cursor` 为不透明续页令牌（无更多页时为 `None`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompatImportHistoryPage {
    pub entries: Vec<CompatImportHistoryEntry>,
    pub cursor: Option<String>,
}

/// 把归一记录映射为 canonical event 序列（含 `RunStarted` / `RunCompleted` 边界）。
pub(crate) fn map_to_events(
    session: &SessionId,
    run_id: &RunId,
    parsed: &ParsedExternalSession,
) -> Vec<AgentEventEnvelope> {
    let review_session = ReviewSessionId::from(format!("{}-review", session.as_str()));
    let mut events: Vec<AgentEventEnvelope> = Vec::new();
    let mut next_seq = 1u64;
    let mut saw_review_session = false;

    // Run 边界：trigger_message_id 用合成 id。
    events.push(envelope(
        session,
        run_id,
        next_seq,
        AgentEvent::RunStarted {
            trigger_message_id: MessageId::from(format!("compat-trigger-{session}")),
        },
    ));
    next_seq += 1;

    for record in &parsed.records {
        match record.clone() {
            ExternalRecord::UserMessage { text } => {
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    message_event(session, MessageRole::User, text, next_seq),
                ));
                next_seq += 1;
            }
            ExternalRecord::AssistantMessage { text } => {
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    message_event(session, MessageRole::Assistant, text, next_seq),
                ));
                next_seq += 1;
            }
            ExternalRecord::ToolCall {
                tool_call_id,
                name,
                arguments,
            } => {
                // tool call id 以目标 session 为 scope，避免跨会话/跨来源撞
                // tool_calls 全局主键；同一外部 id 在 Started / ArgumentsDelta /
                // Completed 间用相同 scope，保证 result 仍能配对。
                let scoped = scope_tool_id(session, &tool_call_id);
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    AgentEvent::ToolCallStarted {
                        tool_call_id: ToolCallId::from(scoped.clone()),
                        name,
                    },
                ));
                next_seq += 1;
                // 保留外部 tool arguments：映射既有 ToolCallArgumentsDelta（projection
                // 累积到 tool_calls.arguments_json）。无/空 arguments 不发空 delta。
                if let Some(arguments) = arguments.filter(|args| !args.is_empty()) {
                    events.push(envelope(
                        session,
                        run_id,
                        next_seq,
                        AgentEvent::ToolCallArgumentsDelta {
                            tool_call_id: ToolCallId::from(scoped),
                            json_delta: arguments,
                        },
                    ));
                    next_seq += 1;
                }
            }
            ExternalRecord::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => {
                // raw 工具输出（含 unified diff）原样保留在 content；锚点化交给将来
                // 真正的 Review consumer，存储层不再实现无消费者的弱化 diff domain。
                let scoped = scope_tool_id(session, &tool_call_id);
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    AgentEvent::ToolExecutionCompleted {
                        tool_call_id: ToolCallId::from(scoped.clone()),
                        result: ToolResultContent {
                            tool_call_id: ToolCallId::from(scoped),
                            tool_name: None,
                            content: vec![ContentPart::Text(TextContent { text: content })],
                            is_error,
                            metadata: serde_json::Value::Null,
                            artifacts: Vec::new(),
                        },
                    },
                ));
                next_seq += 1;
            }
            ExternalRecord::Usage {
                input_tokens,
                output_tokens,
            } => {
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    AgentEvent::UsageUpdated {
                        usage: TokenUsage {
                            input_tokens,
                            output_tokens,
                            cache_read_tokens: 0,
                            cache_write_tokens: 0,
                        },
                    },
                ));
                next_seq += 1;
            }
            ExternalRecord::ReviewComment {
                file,
                line,
                severity,
                body,
            } => {
                if !saw_review_session {
                    events.push(envelope(
                        session,
                        run_id,
                        next_seq,
                        AgentEvent::Review(ReviewEvent::SessionCreated {
                            session_id: review_session.clone(),
                            workspace_id: None,
                        }),
                    ));
                    next_seq += 1;
                    saw_review_session = true;
                }
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    AgentEvent::Review(ReviewEvent::FindingOpened {
                        session_id: review_session.clone(),
                        finding_id: ReviewFindingId::from(format!(
                            "{}-finding-{next_seq}",
                            session.as_str()
                        )),
                        anchor: ReviewAnchor {
                            file,
                            line,
                            end_line: None,
                        },
                        severity,
                        body,
                        evidence: Vec::new(),
                        assignee: None,
                        suggested_patch: None,
                        fingerprint: None,
                    }),
                ));
                next_seq += 1;
            }
            ExternalRecord::Raw { kind, payload } => {
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    AgentEvent::Diagnostic {
                        code: format!("compat.raw.{kind}"),
                        details: payload,
                    },
                ));
                next_seq += 1;
            }
        }
    }

    // Run 边界：completed。
    events.push(envelope(
        session,
        run_id,
        next_seq,
        AgentEvent::RunCompleted {
            stop_reason: StopReason::Completed,
            usage: TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            },
        },
    ));
    events
}

fn message_event(session: &SessionId, role: MessageRole, text: String, seq: u64) -> AgentEvent {
    AgentEvent::MessageCommitted {
        message: Message {
            id: MessageId::from(format!("compat-msg-{}-{seq}", session.as_str())),
            role,
            content: vec![ContentPart::Text(TextContent { text })],
            metadata: MessageMetadata::default(),
        },
    }
}

/// 把外部 tool call id 映射为目标 session scope 的全局唯一 id。
fn scope_tool_id(session: &SessionId, external: &str) -> String {
    format!("compat-tool-{}-{external}", session.as_str())
}

fn envelope(
    session: &SessionId,
    run_id: &RunId,
    seq: u64,
    payload: AgentEvent,
) -> AgentEventEnvelope {
    AgentEventEnvelope::new(
        EventId::from(format!("compat-evt-{session}-{seq}")),
        session.clone(),
        run_id.clone(),
        EventSequence::new(seq),
        Timestamp::from_unix_millis(1_000 + seq),
        payload,
    )
}

// =========================================================================
// Replay 校验（持久化前；失败则整批不入库）
// =========================================================================

/// 对内存中的 canonical event 序列做**结构校验**（structural validation）。
///
/// 这只是结构门控，不是状态机 replay：检查非空、sequence 从 1 连续、
/// `RunStarted` 在首 / `RunCompleted` 在尾、每个 `ToolCallArgumentsDelta` 与
/// `ToolExecutionCompleted` 引用的 `tool_call_id` 在其之前出现过
/// `ToolCallStarted`，以及 `parent_event_id` 在本批次内可解析。它不调用 Run 状态机
/// 或任何 Phase 16 reducer——「状态机可推进」由持久化后 projection 重建承担。
pub fn validate_structure(events: &[AgentEventEnvelope]) -> Result<(), SessionStoreError> {
    if events.is_empty() {
        return Err(SessionStoreError::CompatValidationFailed(
            "empty batch".into(),
        ));
    }
    let ids: std::collections::HashSet<String> =
        events.iter().map(|e| e.event_id.to_string()).collect();
    for (idx, ev) in events.iter().enumerate() {
        let expected = idx as u64 + 1;
        if ev.sequence.value() != expected {
            return Err(SessionStoreError::CompatValidationFailed(format!(
                "non-contiguous sequence at index {idx}: expected {expected}, got {}",
                ev.sequence.value()
            )));
        }
        if let Some(parent) = ev.parent_event_id.as_ref() {
            if !ids.contains(&parent.to_string()) {
                return Err(SessionStoreError::CompatValidationFailed(format!(
                    "dangling parent_event_id {parent} at index {idx}"
                )));
            }
        }
    }
    // 首事件须为 RunStarted。
    if !matches!(
        events.first().map(|e| &e.payload),
        Some(AgentEvent::RunStarted { .. })
    ) {
        return Err(SessionStoreError::CompatValidationFailed(
            "first event must be RunStarted".into(),
        ));
    }
    // 尾事件须为 RunCompleted。
    if !matches!(
        events.last().map(|e| &e.payload),
        Some(AgentEvent::RunCompleted { .. })
    ) {
        return Err(SessionStoreError::CompatValidationFailed(
            "last event must be RunCompleted".into(),
        ));
    }
    // tool result 须有前置 tool call（导入产物里 tool_call_id 同时存在于 envelope 字段）。
    let mut seen_calls: std::collections::HashSet<String> = std::collections::HashSet::new();
    for ev in events {
        match &ev.payload {
            AgentEvent::ToolCallStarted { tool_call_id, .. } => {
                seen_calls.insert(tool_call_id.to_string());
            }
            AgentEvent::ToolCallArgumentsDelta { tool_call_id, .. } => {
                let referenced = tool_call_id.to_string();
                if !referenced.is_empty() && !seen_calls.contains(&referenced) {
                    return Err(SessionStoreError::CompatValidationFailed(format!(
                        "ToolCallArgumentsDelta references unknown tool_call_id '{referenced}'"
                    )));
                }
            }
            AgentEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
                ..
            } => {
                let referenced = tool_call_id.to_string();
                if !referenced.is_empty()
                    && !seen_calls.contains(&referenced)
                    && !result.tool_call_id.as_str().is_empty()
                    && !seen_calls.contains(&result.tool_call_id.to_string())
                {
                    return Err(SessionStoreError::CompatValidationFailed(format!(
                        "ToolExecutionCompleted references unknown tool_call_id '{referenced}'"
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct Counts {
    pub messages: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub usages: usize,
    pub reviews: usize,
    pub raw: usize,
}

pub(crate) fn count_records(records: &[ExternalRecord]) -> Counts {
    let mut c = Counts::default();
    for r in records {
        match r {
            ExternalRecord::UserMessage { .. } | ExternalRecord::AssistantMessage { .. } => {
                c.messages += 1;
            }
            ExternalRecord::ToolCall { .. } => c.tool_calls += 1,
            ExternalRecord::ToolResult { .. } => c.tool_results += 1,
            ExternalRecord::Usage { .. } => c.usages += 1,
            ExternalRecord::ReviewComment { .. } => c.reviews += 1,
            ExternalRecord::Raw { .. } => c.raw += 1,
        }
    }
    c
}

#[cfg(test)]
pub(crate) const CLAUDE_JSON: &str = r#"{
        "conversation_id": "claude-abc",
        "name": "demo chat",
        "chat_messages": [
            {"sender": "human", "text": "hello"},
            {"sender": "assistant", "text": "hi there"},
            {"future_field": {"x": 1}}
        ],
        "custom_export_meta": "v2"
    }"#;

#[cfg(test)]
pub(crate) const CODEX_JSONL: &str = concat!(
    r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"run tests"}]}"#,
    "\n",
    r#"{"type":"message","role":"assistant","content":[{"type":"output_text","text":"on it"}]}"#,
    "\n",
    r#"{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"cmd\":\"cargo test\"}"}"#,
    "\n",
    r#"{"type":"function_call_output","call_id":"c1","output":"test result: ok. 3 passed"}"#,
    "\n",
    r#"{"type":"usage","input_tokens":120,"output_tokens":30}"#,
    "\n",
    r#"{"type":"agent_message_edit","payload":{"rev":2}}"#,
);

/// Claude Code 本地 JSONL 合成样本(结构对齐 2026-08-23 本机真实采样,内容全虚构)。
#[cfg(test)]
pub(crate) const CLAUDE_LOCAL_JSONL: &str = concat!(
    r#"{"type":"ai-title","sessionId":"claude-local-synthetic","aiTitle":"synthetic draft"}"#,
    "\n",
    r#"{"type":"custom-title","sessionId":"claude-local-synthetic","customTitle":"synthetic demo"}"#,
    "\n",
    r#"{"type":"user","sessionId":"claude-local-synthetic","uuid":"u1","parentUuid":null,"isSidechain":false,"timestamp":"2026-08-23T10:00:00.000Z","message":{"role":"user","content":"run the synthetic check"},"cwd":"/tmp/synthetic","version":"1.0.0","gitBranch":"main"}"#,
    "\n",
    r#"{"type":"assistant","sessionId":"claude-local-synthetic","uuid":"a1","parentUuid":"u1","isSidechain":false,"timestamp":"2026-08-23T10:00:01.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"internal reasoning"},{"type":"text","text":"on it"},{"type":"tool_use","id":"toolu_synth_1","name":"shell","input":{"command":"cargo test -p synthetic"}}]}}"#,
    "\n",
    r#"{"type":"user","sessionId":"claude-local-synthetic","uuid":"u2","parentUuid":"a1","isSidechain":false,"timestamp":"2026-08-23T10:00:02.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_synth_1","content":[{"type":"text","text":"test result: ok. 1 passed"}],"is_error":false}]}}"#,
    "\n",
    r#"{"type":"user","sessionId":"claude-local-synthetic","uuid":"u3","parentUuid":"u1","isSidechain":true,"timestamp":"2026-08-23T10:00:03.000Z","message":{"role":"user","content":"sidechain content must be skipped"}}"#,
    "\n",
    r#"{"type":"queue-operation","sessionId":"claude-local-synthetic","operation":"noop"}"#,
    "\n",
    r#"{"type":"last-prompt","sessionId":"claude-local-synthetic","prompt":"skipped noise"}"#,
    "\n",
    r#"{"type":"attachment","sessionId":"claude-local-synthetic","attachment":{"path":"/tmp/synthetic-attachment.png"}}"#,
);

/// Codex rollout 信封 JSONL 合成样本(结构对齐 2026-08-23 本机真实采样,内容全虚构)。
#[cfg(test)]
pub(crate) const CODEX_ENVELOPE_JSONL: &str = concat!(
    r#"{"timestamp":"2026-08-23T10:00:00.000Z","type":"session_meta","payload":{"id":"rollout-synthetic-7","timestamp":"2026-08-23T10:00:00.000Z","cwd":"/tmp/synthetic","originator":"codex_cli_synthetic","cli_version":"1.0.0"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"run the synthetic gate"}]}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"starting"},{"type":"encrypted_content","data":"opaque"}]}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:02.500Z","type":"response_item","payload":{"type":"user_message","author":"user","content":[{"type":"input_text","text":"synthetic user prompt"}],"id":"msg_synth_user_0","recipient":"agent"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:02.600Z","type":"response_item","payload":{"type":"agent_message","author":"agent","content":[{"type":"input_text","text":"synthetic agent reply"},{"type":"encrypted_content","data":"opaque"}],"id":"msg_synth_agent_0","recipient":"user"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:02.700Z","type":"response_item","payload":{"type":"agent_message","author":"agent","content":[{"type":"encrypted_content","data":"opaque only"}],"id":"msg_synth_agent_1","recipient":"user"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:03.000Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"hidden developer note"}]}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:04.000Z","type":"response_item","payload":{"type":"reasoning","summary":[],"content":[]}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:05.000Z","type":"response_item","payload":{"type":"function_call","call_id":"call_synth_0","name":"shell","arguments":"cargo test -p synthetic"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:06.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_synth_0","output":"test result: ok. 1 passed"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:07.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":120,"cached_input_tokens":10,"output_tokens":30},"last_token_usage":{"input_tokens":20,"output_tokens":5}}}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:08.000Z","type":"event_msg","payload":{"type":"agent_message","message":"mirrored entry skipped"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:09.000Z","type":"turn_context","payload":{"cwd":"/tmp/synthetic","model":"synthetic-model"}}"#,
    "\n",
    r#"{"timestamp":"2026-08-23T10:00:10.000Z","type":"response_item","payload":{"type":"future_kind","data":{"x":1}}}"#,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claude_maps_messages_and_preserves_unknown() {
        let parsed = parse_claude(CLAUDE_JSON).unwrap();
        assert_eq!(parsed.original_id.as_deref(), Some("claude-abc"));
        assert_eq!(parsed.title.as_deref(), Some("demo chat"));
        assert_eq!(parsed.records.len(), 3);
        assert!(matches!(
            parsed.records[0],
            ExternalRecord::UserMessage { .. }
        ));
        assert!(matches!(
            parsed.records[1],
            ExternalRecord::AssistantMessage { .. }
        ));
        assert!(matches!(parsed.records[2], ExternalRecord::Raw { .. }));
        assert!(parsed.unknown_fields.contains_key("custom_export_meta"));
    }

    #[test]
    fn parse_codex_maps_typed_entries_and_keeps_unknown_as_raw() {
        let parsed = parse_codex(CODEX_JSONL).unwrap();
        // 2 messages + 1 tool call + 1 tool result + 1 usage + 1 raw(unknown type)
        assert_eq!(parsed.records.len(), 6);
        assert!(matches!(parsed.records[2], ExternalRecord::ToolCall { .. }));
        assert!(matches!(
            parsed.records[3],
            ExternalRecord::ToolResult { .. }
        ));
        assert!(matches!(parsed.records[4], ExternalRecord::Usage { .. }));
        assert!(matches!(parsed.records[5], ExternalRecord::Raw { .. }));
    }

    #[test]
    fn parse_claude_local_jsonl_maps_records_and_counts_skips() {
        let parsed = parse_claude(CLAUDE_LOCAL_JSONL).unwrap();
        assert_eq!(
            parsed.original_id.as_deref(),
            Some("claude-local-synthetic")
        );
        assert_eq!(parsed.title.as_deref(), Some("synthetic demo"));
        // user 文本 + assistant(文本、tool_call) + tool_result + 未知类型 Raw。
        assert_eq!(parsed.records.len(), 5);
        assert!(matches!(
            parsed.records[0],
            ExternalRecord::UserMessage { ref text } if text == "run the synthetic check"
        ));
        assert!(matches!(
            parsed.records[1],
            ExternalRecord::AssistantMessage { ref text } if text == "on it"
        ));
        assert!(matches!(
            parsed.records[2],
            ExternalRecord::ToolCall {
                ref tool_call_id,
                ref name,
                ..
            } if tool_call_id == "toolu_synth_1" && name == "shell"
        ));
        assert!(matches!(
            parsed.records[3],
            ExternalRecord::ToolResult {
                ref tool_call_id,
                ref content,
                is_error: false,
            } if tool_call_id == "toolu_synth_1" && content == "test result: ok. 1 passed"
        ));
        assert!(matches!(
            parsed.records[4],
            ExternalRecord::Raw { ref kind, .. } if kind == "claude.type:attachment"
        ));
        assert_eq!(
            parsed
                .unknown_fields
                .get("skipped_sidechain")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            parsed
                .unknown_fields
                .get("skipped_thinking")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            parsed
                .unknown_fields
                .get("skipped_queue_operation")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            parsed
                .unknown_fields
                .get("skipped_last_prompt")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn claude_local_records_map_to_valid_events() {
        let parsed = parse_claude(CLAUDE_LOCAL_JSONL).unwrap();
        let events = map_to_events(
            &SessionId::from("claude-local"),
            &RunId::from("run-local"),
            &parsed,
        );
        assert!(validate_structure(&events).is_ok());
        // tool_use 与 tool_result 经 scope 后仍按序配对。
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            AgentEvent::ToolCallStarted { name, .. } if name == "shell"
        )));
        assert!(events.iter().any(|event| matches!(
            &event.payload,
            AgentEvent::ToolExecutionCompleted { result, .. }
                if result.content.iter().any(|part| matches!(part, ContentPart::Text(text) if text.text == "test result: ok. 1 passed"))
        )));
    }

    #[test]
    fn claude_local_noise_only_file_stays_empty_ok_but_corrupt_file_fails_closed() {
        // 全为合法噪声行(标题/队列操作):无可导入内容,维持 Ok 空导入。
        let noise_only = concat!(
            r#"{"type":"custom-title","sessionId":"s1","customTitle":"noise only"}"#,
            "\n",
            r#"{"type":"queue-operation","sessionId":"s1","operation":"noop"}"#,
        );
        let parsed = parse_claude(noise_only).unwrap();
        assert!(parsed.records.is_empty());
        assert_eq!(parsed.title.as_deref(), Some("noise only"));

        // 存在 unparseable 行且零记录:fail-closed,避免损坏文件计为空导入。
        let corrupted = concat!(
            r#"{"type":"ai-title","sessionId":"s1","aiTitle":"corrupt"}"#,
            "\n",
            r#"{"type":"user","sessionId":"s1","message":{"broken"#,
        );
        let error = parse_claude(corrupted).unwrap_err();
        assert!(matches!(
            error,
            SessionStoreError::CompatUnparseable { source_label, .. } if source_label == "claude"
        ));
    }

    #[test]
    fn codex_envelope_noise_only_file_stays_empty_ok_but_corrupt_file_fails_closed() {
        // 仅 session_meta + 跳过行:零记录但全部合法,维持 Ok。
        let noise_only = concat!(
            r#"{"timestamp":"2026-08-23T10:00:00.000Z","type":"session_meta","payload":{"id":"s1"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-23T10:00:01.000Z","type":"turn_context","payload":{"cwd":"/tmp"}}"#,
        );
        let parsed = parse_codex(noise_only).unwrap();
        assert!(parsed.records.is_empty());
        assert_eq!(parsed.original_id.as_deref(), Some("s1"));

        // session_meta 合法但后续行损坏且零记录:fail-closed。
        let corrupted = concat!(
            r#"{"timestamp":"2026-08-23T10:00:00.000Z","type":"session_meta","payload":{"id":"s1"}}"#,
            "\n",
            r#"{"timestamp":"2026-08-23T10:00:01.000Z","type":"response_item","pay"#,
        );
        let error = parse_codex(corrupted).unwrap_err();
        assert!(matches!(
            error,
            SessionStoreError::CompatUnparseable { source_label, .. } if source_label == "codex"
        ));
    }

    #[test]
    fn parse_codex_envelope_maps_payloads_and_counts_skips() {
        let parsed = parse_codex(CODEX_ENVELOPE_JSONL).unwrap();
        assert_eq!(parsed.original_id.as_deref(), Some("rollout-synthetic-7"));
        // user + assistant + user_message + agent_message + tool call + tool result
        // + usage + 未知 payload Raw;encrypted-only agent_message 无文本不落记录。
        assert_eq!(parsed.records.len(), 8);
        assert!(matches!(
            parsed.records[0],
            ExternalRecord::UserMessage { ref text } if text == "run the synthetic gate"
        ));
        assert!(matches!(
            parsed.records[1],
            ExternalRecord::AssistantMessage { ref text } if text == "starting"
        ));
        assert!(matches!(
            parsed.records[2],
            ExternalRecord::UserMessage { ref text } if text == "synthetic user prompt"
        ));
        assert!(matches!(
            parsed.records[3],
            ExternalRecord::AssistantMessage { ref text } if text == "synthetic agent reply"
        ));
        assert!(matches!(parsed.records[4], ExternalRecord::ToolCall { .. }));
        assert!(matches!(
            parsed.records[5],
            ExternalRecord::ToolResult { ref tool_call_id, is_error: false, .. }
                if tool_call_id == "call_synth_0"
        ));
        assert!(matches!(
            parsed.records[6],
            ExternalRecord::Usage {
                input_tokens: 120,
                output_tokens: 30
            }
        ));
        assert!(matches!(
            parsed.records[7],
            ExternalRecord::Raw { ref kind, .. } if kind == "codex.payload:future_kind"
        ));
        for key in [
            "skipped_developer_message",
            "skipped_reasoning",
            "skipped_event_msg",
            "skipped_turn_context",
        ] {
            assert_eq!(
                parsed.unknown_fields.get(key).map(String::as_str),
                Some("1"),
                "missing skip counter {key}"
            );
        }
    }

    #[test]
    fn parse_grok_and_cursor_handle_messages_array() {
        let grok = r#"{"id":"grok-1","title":"g","messages":[
            {"role":"user","content":"hi"},
            {"role":"assistant","content":[{"type":"text","text":"yo"}]}]}"#;
        let p = parse_grok(grok).unwrap();
        assert_eq!(p.original_id.as_deref(), Some("grok-1"));
        assert_eq!(p.records.len(), 2);
        assert!(matches!(
            p.records[1],
            ExternalRecord::AssistantMessage { .. }
        ));

        let cursor = r#"{"version":3,"messages":[{"role":"user","text":"ping"}]}"#;
        let p = parse_cursor(cursor).unwrap();
        assert_eq!(p.records.len(), 1);
    }

    #[test]
    fn secret_scan_rejects_known_credentials() {
        assert_eq!(find_secret("hello world"), None);
        assert_eq!(
            find_secret("export OPENAI_API_KEY=sk-proj-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            Some("sk-proj-")
        );
        assert_eq!(
            find_secret("Authorization: Bearer eyJabcdef1234567890xyzXYZ"),
            Some("Bearer ")
        );
        // 不会误报：sk- 后跟短串不算 secret。
        assert_eq!(find_secret("see sk-foo in docs"), None);
    }

    #[test]
    fn validate_structure_catches_bad_envelope_and_accepts_good() {
        // 缺少 RunStarted 边界。
        let bad = vec![envelope(
            &SessionId::from("s"),
            &RunId::from("r"),
            1,
            AgentEvent::RunCompleted {
                stop_reason: StopReason::Completed,
                usage: TokenUsage::default(),
            },
        )];
        assert!(validate_structure(&bad).is_err());

        // 正常序列通过。
        let good = map_to_events(
            &SessionId::from("s"),
            &RunId::from("r"),
            &ParsedExternalSession {
                source: Some(ExternalSource::Codex),
                records: vec![ExternalRecord::UserMessage { text: "hi".into() }],
                ..Default::default()
            },
        );
        assert!(validate_structure(&good).is_ok());
    }
}
