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

use crate::SessionStoreError;

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

/// Claude 导出 JSON 解析。
pub fn parse_claude(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
    let value: Value = serde_json::from_str(content).map_err(|e| unparseable("claude", e))?;
    let obj = value
        .as_object()
        .ok_or_else(|| unparseable_msg("claude", "expected JSON object"))?;
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

/// Codex rollout JSONL 解析（逐行 typed entry）。
pub fn parse_codex(content: &str) -> Result<ParsedExternalSession, SessionStoreError> {
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
