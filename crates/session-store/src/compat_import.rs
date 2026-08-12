//! 外部 Agent 会话兼容导入（P16-9）：Claude / Codex / Grok / Cursor。
//!
//! 最终目的：把其他智能体工具的外部会话无损导入为 Pawork 的 canonical event，
//! 使既有对话与产物可被重放、检索与续接，而**绝不破坏 canonical event 模型**——
//! 外部格式始终是输入侧的适配/投影，导入产物是规范事件，不污染、不覆写既有事件。
//!
//! 设计要点（对应 plan/P16-9 细分步骤）：
//! 1. **格式探测与 schema**：[`ExternalSource`] + 四个只读解析器；未识别字段保留为
//!    raw metadata（`Diagnostic` 事件），不猜测填充。
//! 2. **字段映射到 canonical event**：外部消息/工具调用/产物/用量映射到既有 canonical
//!    变体（`RunStarted`/`MessageCommitted`/`ToolCallStarted`/`ToolExecutionCompleted`/
//!    `UsageUpdated`/`RunCompleted`），不可映射字段进 [`AgentEvent::Diagnostic`]，
//!    **不新增非规范事件类型**。
//! 3. **不可破坏 canonical event**：导入只生成新 Session + 新 event id，绝不修改/
//!    覆盖/删除既有 event；`session_events` 表的 append-only 触发器是底层硬保证。
//! 4. **patch / 产物锚点**：外部文件改动中的 unified diff 由内置
//!    [`parse_diff_anchors_owned`] 解析为行锚点，挂到对应 `ToolExecutionCompleted`
//!    的 `metadata`；评审类意见经 canonical `Review(ReviewEvent::FindingOpened)`
//!    锚点化（行锚点为 `agent_domain::ReviewAnchor`）。
//! 5. **重放与校验**：持久化前对内存中的 canonical event 序列做 replay 校验
//!    （sequence 连续、parent 无悬空、tool result 有前置 tool call）；外加对原始输入
//!    做 Secret 扫描。任一失败则**整批不入库**——因为 `session_events` append-only，
//!    回滚等价于「校验门控」：校验不通过时不写入任何事件。
//! 6. **查询面与去重**：导入按 `(source, original_id, content)` 计算 blake3 指纹，
//!    派生确定性 [`SessionId`]；同一外部会话重复导入命中已有 Session，不产生重复 event。
//!
//! # 架构决策（与 plan 的偏离说明）
//!
//! plan 建议「patch 锚点复用 diff-service」。但 diff-service 传递依赖 git-service →
//! process-runtime / policy-engine / workspace-service 等服务层 crate；`session-store`
//! 是底层存储 crate，反向依赖服务层会破坏分层（storage → services）。为守住分层红线，
//! unified-diff 行锚点在本模块内以一个标准文本解析器实现（无 git 依赖），评审锚点使用
//! `session-store` 已依赖的 `agent_domain::ReviewAnchor`。两者均满足 plan 的「带行锚点」
//! 语义，且不引入新的分层依赖。
//!
//! Secret 处理：untrusted 外部导入采用**拒绝**策略（检测到高置信凭证前缀即整批拒绝），
//! 比基线 Event Store 的「redact 后入库」更严格——对外部数据更安全。

use std::collections::BTreeMap;
use std::path::Path;

use agent_domain::{
    ContentPart, EventId, Message, MessageId, MessageMetadata, MessageRole, ReviewAnchor,
    ReviewEvent, ReviewFindingId, ReviewSessionId, ReviewSeverity, RunId, SessionId, StopReason,
    TextContent, Timestamp, TokenUsage, ToolCallId, ToolResultContent,
};
use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};
use serde_json::Value;
use tokio::io::{AsyncReadExt, BufReader};

use crate::{SessionStore, SessionStoreError, DEFAULT_BRANCH_ID};

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
        /// 若该 result 携带 unified diff，保留原文以供行锚点解析。
        file_diff: Option<String>,
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
// Unified diff 行锚点解析（标准格式，无 git 依赖）
// =========================================================================

/// 从 unified diff 文本解析出每个 hunk 的文件与行范围。
///
/// 识别 `+++ b/<path>`（或 `+++ <path>`）文件头与 `@@ -a,b +c,d @@` hunk 头；
/// `c` 为新文件起始行，`d` 为行数，范围 = `[c, c+d-1]`。无法识别的行被忽略，
/// 不抛错（解析是 best-effort 锚点提取，不是 diff 校验）。
pub fn parse_diff_anchors_owned(diff: &str) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    let mut current_file: Option<String> = None;
    for raw in diff.lines() {
        let line = raw.trim_start();
        if let Some(rest) = line.strip_prefix("+++ ") {
            // 形如 `+++ b/path` 或 `+++ path`
            let path = rest.split_whitespace().next().unwrap_or(rest);
            let stripped = path
                .strip_prefix("b/")
                .or_else(|| path.strip_prefix("a/"))
                .unwrap_or(path);
            if stripped != "/dev/null" {
                current_file = Some(stripped.to_string());
            }
            continue;
        }
        if line.starts_with("@@") {
            if let Some(file) = current_file.clone() {
                if let Some((start, len)) = parse_hunk_new_range(line) {
                    let end = start.saturating_add(len).saturating_sub(1).max(start);
                    out.push((file.clone(), start, end));
                }
            }
        }
    }
    out
}

/// 从 `@@ -a,b +c,d @@` 提取新文件侧 `(start_line=c, line_count=d)`。
fn parse_hunk_new_range(header: &str) -> Option<(u32, u32)> {
    let plus = header.find(" +")?;
    let after = &header[plus + 2..];
    let body = after.split_whitespace().next()?;
    // body 形如 c,d 或 c
    let mut parts = body.split(',');
    let start: u32 = parts.next()?.parse().ok()?;
    let count: u32 = parts.next().unwrap_or("1").parse().ok().unwrap_or(1);
    Some((start, count))
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
                let file_diff = output.as_ref().and_then(|o| {
                    if o.contains("@@ ") && (o.contains("+++ ") || o.contains("--- ")) {
                        Some(o.clone())
                    } else {
                        None
                    }
                });
                parsed.records.push(ExternalRecord::ToolResult {
                    tool_call_id: call_id,
                    content: output.unwrap_or_default(),
                    is_error,
                    file_diff,
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
        let file_diff = if content.contains("@@ ") && content.contains("+++ ") {
            Some(content.clone())
        } else {
            None
        };
        return ExternalRecord::ToolResult {
            tool_call_id: call_id,
            content,
            is_error,
            file_diff,
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

/// 按 `(source, original_id, content)` 计算 blake3 指纹，派生确定性 SessionId。
///
/// 同一外部会话（同来源 + 同原始 id + 同内容）总是映射到同一个 SessionId，
/// 因此重复导入可在持久化前命中已有 Session 而不产生重复 event。
pub fn fingerprint_session(
    source: ExternalSource,
    original_id: Option<&str>,
    content: &str,
) -> SessionId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(source.as_str().as_bytes());
    hasher.update(&[0]);
    hasher.update(original_id.unwrap_or("").as_bytes());
    hasher.update(&[0]);
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
    SessionId::from(format!(
        "compat-{}-{}",
        source.as_str(),
        hex(&hash.as_bytes()[..16])
    ))
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

/// 把归一记录映射为 canonical event 序列（含 `RunStarted` / `RunCompleted` 边界）。
fn map_to_events(
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
                    message_event(MessageRole::User, text, next_seq),
                ));
                next_seq += 1;
            }
            ExternalRecord::AssistantMessage { text } => {
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    message_event(MessageRole::Assistant, text, next_seq),
                ));
                next_seq += 1;
            }
            ExternalRecord::ToolCall {
                tool_call_id,
                name,
                arguments: _,
            } => {
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    AgentEvent::ToolCallStarted {
                        tool_call_id: ToolCallId::from(tool_call_id),
                        name,
                    },
                ));
                next_seq += 1;
            }
            ExternalRecord::ToolResult {
                tool_call_id,
                content,
                is_error,
                file_diff,
            } => {
                let metadata = file_diff
                    .as_deref()
                    .map(parse_diff_anchors_owned)
                    .filter(|v| !v.is_empty())
                    .map(|v| {
                        serde_json::Value::Array(
                            v.into_iter()
                                .map(|(file, start, end)| {
                                    serde_json::json!({
                                        "file": file,
                                        "start_line": start,
                                        "end_line": end,
                                    })
                                })
                                .collect(),
                        )
                    })
                    .map(|a| serde_json::json!({ "compat": { "patch_anchors": a } }))
                    .unwrap_or_else(|| serde_json::json!({}));
                events.push(envelope(
                    session,
                    run_id,
                    next_seq,
                    AgentEvent::ToolExecutionCompleted {
                        tool_call_id: ToolCallId::from(tool_call_id.clone()),
                        result: ToolResultContent {
                            tool_call_id: ToolCallId::from(tool_call_id),
                            tool_name: None,
                            content: vec![ContentPart::Text(TextContent { text: content })],
                            is_error,
                            metadata,
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

fn message_event(role: MessageRole, text: String, seq: u64) -> AgentEvent {
    AgentEvent::MessageCommitted {
        message: Message {
            id: MessageId::from(format!("compat-msg-{seq}")),
            role,
            content: vec![ContentPart::Text(TextContent { text })],
            metadata: MessageMetadata::default(),
        },
    }
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

/// 校验内存中的 canonical event 序列。
///
/// 检查：非空；sequence 从 1 连续；`RunStarted` 在首、`RunCompleted` 在尾；每个
/// `ToolExecutionCompleted` 的 `tool_call_id` 在其之前出现过 `ToolCallStarted`；
/// 若存在 `parent_event_id`，须在本批次内可解析。
pub fn validate_batch(events: &[AgentEventEnvelope]) -> Result<(), SessionStoreError> {
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

// =========================================================================
// SessionStore 导入入口
// =========================================================================

impl SessionStore {
    /// 从外部会话内容导入（内存字符串）。**不修改任何既有 event**。
    pub async fn import_compat(
        &self,
        source: ExternalSource,
        content: &str,
    ) -> Result<CompatImportReport, SessionStoreError> {
        import_compat_inner(self, source, content).await
    }

    /// 从外部会话文件导入。**只读取源文件，不修改原文件**（ADR-005）。
    pub async fn import_compat_from_file(
        &self,
        source: ExternalSource,
        path: &Path,
    ) -> Result<CompatImportReport, SessionStoreError> {
        let content =
            if path.to_string_lossy().ends_with(".jsonl") || source == ExternalSource::Codex {
                // JSONL：流式读取，避免大文件整体入内存。
                let file = tokio::fs::File::open(path).await?;
                let mut reader = BufReader::new(file);
                let mut buf = String::new();
                reader.read_to_string(&mut buf).await?;
                buf
            } else {
                tokio::fs::read_to_string(path).await?
            };
        import_compat_inner(self, source, &content).await
    }
}

async fn import_compat_inner(
    store: &SessionStore,
    source: ExternalSource,
    content: &str,
) -> Result<CompatImportReport, SessionStoreError> {
    // 1. Secret 扫描（untrusted 拒绝策略）。
    if let Some(pattern) = find_secret(content) {
        return Err(SessionStoreError::CompatSecretDetected {
            pattern: pattern.into(),
        });
    }
    // 2. 解析。
    let parsed = parse_external(source, content)?;
    // 3. 指纹 + 去重。
    let session_id = fingerprint_session(source, parsed.original_id.as_deref(), content);
    let existing = store.replay_events(&session_id, 1, 1).await?;
    let counts = count_records(&parsed.records);
    if !existing.is_empty() {
        return Ok(CompatImportReport {
            source: parsed.source,
            session_id: session_id.to_string(),
            original_id: parsed.original_id.clone(),
            imported_events: 0,
            imported_messages: 0,
            imported_tool_calls: 0,
            imported_tool_results: 0,
            imported_usages: 0,
            imported_reviews: 0,
            raw_records: 0,
            deduplicated: true,
            unknown_fields: parsed.unknown_fields.clone(),
        });
    }
    // 4. 映射为 canonical event 序列。
    let run_id = RunId::from(format!("compat-{}-import", source.as_str()));
    let events = map_to_events(&session_id, &run_id, &parsed);
    // 5. 校验（失败则整批不入库）。
    validate_batch(&events)?;
    // 6. 持久化：新建 Session + 追加事件（绝不触碰既有事件）。
    let title = parsed
        .title
        .clone()
        .unwrap_or_else(|| format!("imported from {source}"));
    store
        .create_session(&session_id, title, Timestamp::from_unix_millis(1))
        .await?;
    for env in &events {
        store.append_event(DEFAULT_BRANCH_ID, env.clone()).await?;
    }
    Ok(CompatImportReport {
        source: parsed.source,
        session_id: session_id.to_string(),
        original_id: parsed.original_id.clone(),
        imported_events: events.len(),
        imported_messages: counts.messages,
        imported_tool_calls: counts.tool_calls,
        imported_tool_results: counts.tool_results,
        imported_usages: counts.usages,
        imported_reviews: counts.reviews,
        raw_records: counts.raw,
        deduplicated: false,
        unknown_fields: parsed.unknown_fields.clone(),
    })
}

#[derive(Default)]
struct Counts {
    messages: usize,
    tool_calls: usize,
    tool_results: usize,
    usages: usize,
    reviews: usize,
    raw: usize,
}

fn count_records(records: &[ExternalRecord]) -> Counts {
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
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path() -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-compat-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    async fn open_store() -> SessionStore {
        let (store, _) = SessionStore::open(&temp_path()).await.expect("open store");
        store
    }

    const CLAUDE_JSON: &str = r#"{
        "conversation_id": "claude-abc",
        "name": "demo chat",
        "chat_messages": [
            {"sender": "human", "text": "hello"},
            {"sender": "assistant", "text": "hi there"},
            {"future_field": {"x": 1}}
        ],
        "custom_export_meta": "v2"
    }"#;

    const CODEX_JSONL: &str = concat!(
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
    fn diff_anchors_parsed_from_unified_diff() {
        let diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -10,3 +10,4 @@\n old\n+new\n@@ -20,2 +20,2 @@\n";
        let anchors = parse_diff_anchors_owned(diff);
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0].0, "src/lib.rs");
        assert_eq!(anchors[0].1, 10);
        assert_eq!(anchors[0].2, 13);
        assert_eq!(anchors[1].1, 20);
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
    fn validate_batch_catches_bad_envelope_and_accepts_good() {
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
        assert!(validate_batch(&bad).is_err());

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
        assert!(validate_batch(&good).is_ok());
    }

    #[tokio::test]
    async fn import_creates_new_session_and_is_append_only() {
        let store = open_store().await;
        let report = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("import");
        assert!(!report.deduplicated);
        assert!(report.imported_events > 0);
        assert_eq!(report.imported_messages, 2);
        assert_eq!(report.raw_records, 1);
        // replay 可消费导入产物（状态机可推进）。
        let sid = SessionId::from(report.session_id.clone());
        let events = store.replay_events(&sid, 1, 100).await.expect("replay");
        assert!(!events.is_empty());
        // 首事件 RunStarted，尾事件 RunCompleted。
        assert!(matches!(
            events.first().map(|e| &e.payload),
            Some(AgentEvent::RunStarted { .. })
        ));
        assert!(matches!(
            events.last().map(|e| &e.payload),
            Some(AgentEvent::RunCompleted { .. })
        ));
    }

    #[tokio::test]
    async fn import_codex_and_verify_counts() {
        let store = open_store().await;
        let report = store
            .import_compat(ExternalSource::Codex, CODEX_JSONL)
            .await
            .expect("import codex");
        assert_eq!(report.imported_tool_calls, 1);
        assert_eq!(report.imported_tool_results, 1);
        assert_eq!(report.imported_usages, 1);
    }

    #[tokio::test]
    async fn import_rejects_secret_and_persists_nothing() {
        let store = open_store().await;
        let malicious = r#"{"id":"x","messages":[{"role":"user","content":"my key is sk-ant-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}]}"#;
        let err = store
            .import_compat(ExternalSource::Grok, malicious)
            .await
            .expect_err("must reject secret");
        assert!(matches!(
            err,
            SessionStoreError::CompatSecretDetected { .. }
        ));
        // 无任何 session 被创建（nothing imported）。
        let sid = fingerprint_session(ExternalSource::Grok, Some("x"), malicious);
        let events = store.replay_events(&sid, 1, 1).await.expect("replay");
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn import_dedup_is_idempotent() {
        let store = open_store().await;
        let first = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("first import");
        let second = store
            .import_compat(ExternalSource::Claude, CLAUDE_JSON)
            .await
            .expect("second import");
        assert_eq!(first.session_id, second.session_id);
        assert!(second.deduplicated);
        assert_eq!(second.imported_events, 0);
        // 重复导入不产生重复 event。
        let sid = SessionId::from(first.session_id.clone());
        let events = store.replay_events(&sid, 1, 1000).await.expect("replay");
        assert_eq!(events.len(), first.imported_events);
    }

    #[tokio::test]
    async fn import_does_not_modify_original_file() {
        let store = open_store().await;
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pawork-compat-src-{unique}.json"));
        fs::write(&path, CLAUDE_JSON).expect("write src");
        let before = fs::read_to_string(&path).unwrap();
        store
            .import_compat_from_file(ExternalSource::Claude, &path)
            .await
            .expect("import file");
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after, "original file must be unchanged");
    }

    #[tokio::test]
    async fn import_review_comment_emits_review_event() {
        let store = open_store().await;
        let json = r#"{"id":"r1","messages":[
            {"role":"user","text":"please review"},
            {"role":"assistant","file":"src/main.rs","line":42,"severity":"major","text":"unwrap here"}
        ]}"#;
        let report = store
            .import_compat(ExternalSource::Cursor, json)
            .await
            .expect("import");
        assert_eq!(report.imported_reviews, 1);
    }
}
