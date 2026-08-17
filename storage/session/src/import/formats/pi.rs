//! Pi JSONL Session 解析器（P5-9）。
//!
//! 最终目的：将 Pi 的历史 JSONL 会话导入新数据库，且**不修改原始 Pi 文件**（ADR-005）。
//!
//! 本模块只含解析纯函数与报告类型；写入在 persist 侧单事务完成。

use pawork_domain::MessageRole;
use serde_json::Value;

/// 单条 Pi JSONL 记录解析结果。
#[derive(Clone, Debug, PartialEq)]
pub struct PiParsedEntry {
    /// 识别出的记录类型（`header` / `message` / `tool_call` / `model_switch` /
    /// `compaction` / `branch` / `custom` / `unknown`）。
    pub kind: PiEntryKind,
    /// 该记录里未被识别的字段（key -> JSON 值字符串）。
    pub unknown_fields: std::collections::BTreeMap<String, String>,
    /// 已解析的载荷（按类型不同）。
    pub payload: PiPayload,
}

/// Pi 记录类型分类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiEntryKind {
    Header,
    Message,
    ToolCall,
    ModelSwitch,
    Compaction,
    Branch,
    /// 自定义 entry（Pi 扩展字段，已知但非核心）。
    Custom,
    /// 完全未识别的行。
    Unknown,
}

/// Pi 记录解析后的载荷。
#[derive(Clone, Debug, PartialEq)]
pub enum PiPayload {
    Header {
        session_id: String,
        title: Option<String>,
    },
    Message {
        sequence: u64,
        role: Option<MessageRole>,
        text: Option<String>,
    },
    ToolCall {
        sequence: u64,
        tool_call_id: Option<String>,
        name: Option<String>,
    },
    ModelSwitch {
        sequence: u64,
        model: Option<String>,
    },
    Compaction {
        sequence: u64,
        summary: Option<String>,
    },
    Branch {
        branch_id: Option<String>,
        parent: Option<String>,
    },
    /// 仅记录未知字段，无结构化载荷。
    Raw,
}

/// Pi 导入报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PiImportReport {
    pub total_lines: usize,
    pub parsed_entries: usize,
    pub header_found: bool,
    pub imported_messages: usize,
    pub imported_tool_calls: usize,
    /// 已作为 `pi.model_switched` Diagnostic 事件持久化的模型切换数。
    pub imported_model_switches: usize,
    pub imported_compactions: usize,
    pub imported_branches: usize,
    /// 整条未识别的行（行号 -> 原始 JSON）。
    pub unknown_entries: std::collections::BTreeMap<usize, String>,
}

impl PiImportReport {
    pub(crate) fn record_unknown_entry(&mut self, line: usize, raw: String) {
        self.unknown_entries.insert(line, raw);
    }
}

/// 解析一行 Pi JSONL。
///
/// 识别规则（宽松、向前兼容）：
/// - 含 `session_id` 的首行视为 header；
/// - 含 `message` / `role` + `content`/`text` 视为 message；
/// - 含 `tool_call` / `tool` 视为 tool call；
/// - 含 `model` 视为 model switch；
/// - 含 `compaction` / `summary` 视为 compaction；
/// - 含 `branch` 视为 branch；
/// - 其余归为 unknown，并保留原始字段。
pub fn parse_pi_line(raw: &str) -> Option<PiParsedEntry> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    let obj = value.as_object()?;
    let known_keys: &[&str] = &[
        "session_id",
        "title",
        "sequence",
        "role",
        "content",
        "text",
        "message",
        "tool_call",
        "tool_call_id",
        "tool",
        "name",
        "model",
        "compaction",
        "summary",
        "branch",
        "branch_id",
        "parent",
        "type",
        "kind",
    ];
    let mut unknown_fields = std::collections::BTreeMap::new();
    for (key, val) in obj {
        if !known_keys.contains(&key.as_str()) {
            unknown_fields.insert(key.clone(), val.to_string());
        }
    }

    let sequence = obj.get("sequence").and_then(Value::as_u64).unwrap_or(0);

    // header
    if let Some(session_id) = obj.get("session_id").and_then(Value::as_str) {
        if !obj.contains_key("sequence") {
            return Some(PiParsedEntry {
                kind: PiEntryKind::Header,
                unknown_fields,
                payload: PiPayload::Header {
                    session_id: session_id.to_string(),
                    title: obj.get("title").and_then(Value::as_str).map(String::from),
                },
            });
        }
    }

    // branch
    if obj.contains_key("branch") || obj.contains_key("branch_id") {
        return Some(PiParsedEntry {
            kind: PiEntryKind::Branch,
            unknown_fields,
            payload: PiPayload::Branch {
                branch_id: obj
                    .get("branch_id")
                    .and_then(Value::as_str)
                    .map(String::from),
                parent: obj.get("parent").and_then(Value::as_str).map(String::from),
            },
        });
    }

    // compaction
    if obj.contains_key("compaction") || obj.contains_key("summary") {
        return Some(PiParsedEntry {
            kind: PiEntryKind::Compaction,
            unknown_fields,
            payload: PiPayload::Compaction {
                sequence,
                summary: obj.get("summary").and_then(Value::as_str).map(String::from),
            },
        });
    }

    // model switch
    if let Some(model) = obj.get("model").and_then(Value::as_str) {
        return Some(PiParsedEntry {
            kind: PiEntryKind::ModelSwitch,
            unknown_fields,
            payload: PiPayload::ModelSwitch {
                sequence,
                model: Some(model.to_string()),
            },
        });
    }

    // tool call
    if obj.contains_key("tool_call") || obj.contains_key("tool") {
        return Some(PiParsedEntry {
            kind: PiEntryKind::ToolCall,
            unknown_fields,
            payload: PiPayload::ToolCall {
                sequence,
                tool_call_id: obj
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(String::from),
                name: obj
                    .get("tool")
                    .and_then(Value::as_str)
                    .or_else(|| obj.get("name").and_then(Value::as_str))
                    .map(String::from),
            },
        });
    }

    // message
    if obj.contains_key("message") || obj.contains_key("role") || obj.contains_key("content") {
        let role = obj.get("role").and_then(Value::as_str).and_then(parse_role);
        let text = obj
            .get("text")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| obj.get("content").and_then(Value::as_str).map(String::from));
        return Some(PiParsedEntry {
            kind: PiEntryKind::Message,
            unknown_fields,
            payload: PiPayload::Message {
                sequence,
                role,
                text,
            },
        });
    }

    // 完全未知
    Some(PiParsedEntry {
        kind: PiEntryKind::Unknown,
        unknown_fields,
        payload: PiPayload::Raw,
    })
}

fn parse_role(s: &str) -> Option<MessageRole> {
    match s.to_ascii_lowercase().as_str() {
        "system" => Some(MessageRole::System),
        "user" => Some(MessageRole::User),
        "assistant" => Some(MessageRole::Assistant),
        "tool" | "function" => Some(MessageRole::Tool),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recognizes_known_kinds_and_preserves_unknown_fields() {
        let header = parse_pi_line(r#"{"session_id":"s1","title":"demo"}"#).unwrap();
        assert_eq!(header.kind, PiEntryKind::Header);
        let msg = parse_pi_line(r#"{"sequence":1,"role":"user","text":"hi"}"#).unwrap();
        assert_eq!(msg.kind, PiEntryKind::Message);
        let tool =
            parse_pi_line(r#"{"sequence":2,"tool":"read_file","tool_call_id":"c1"}"#).unwrap();
        assert_eq!(tool.kind, PiEntryKind::ToolCall);
        let unknown = parse_pi_line(r#"{"weird_field":42,"another":"x"}"#).unwrap();
        assert_eq!(unknown.kind, PiEntryKind::Unknown);
        assert!(unknown.unknown_fields.contains_key("weird_field"));
    }
}
