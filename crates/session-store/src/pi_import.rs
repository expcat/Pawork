//! Pi JSONL Session 导入器（P5-9）。
//!
//! 最终目的：将 Pi 的历史 JSONL 会话导入新数据库，且**不修改原始 Pi 文件**（ADR-005）。
//!
//! 设计要点：
//! - 扫描输入 JSONL（每行一个对象），解析 header / 消息 / tool call / 模型切换 /
//!   compaction / branch / 自定义 entry。
//! - **保存未知字段**：解析器保留所有未识别字段，写入 [`PiImportReport::unknown_entries`]
//!   与每条记录的 `unknown_fields`，保证未来格式向前兼容。
//! - **不修改原文件**：导入器只读取源路径，所有产出写入目标 `SessionStore`；
//!   返回 [`PiImportReport`] 供审计。

use std::path::Path;

use agent_domain::{
    ContentPart, Message, MessageId, MessageMetadata, MessageRole, SessionId, TextContent,
    Timestamp, ToolCallId,
};
use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};

use crate::{SessionStore, SessionStoreError, DEFAULT_BRANCH_ID};

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
    fn record_unknown_entry(&mut self, line: usize, raw: String) {
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

impl SessionStore {
    /// 从 Pi JSONL 文件导入 session，**不修改原文件**。
    ///
    /// 读取 `source_path` 逐行解析，识别 header 后在当前数据库重建 session，再按
    /// sequence 追加 message / tool call / model switch / compaction 事件。未知字段
    /// 与未识别行写入 [`PiImportReport`]。
    pub async fn import_pi_jsonl(
        &self,
        source_path: &Path,
    ) -> Result<PiImportReport, SessionStoreError> {
        let file = tokio::fs::File::open(source_path).await?;
        self.import_pi_jsonl_reader(BufReader::new(file)).await
    }

    /// 从已读取的 Pi JSONL 内容导入（便于测试与流式来源）。
    pub async fn import_pi_jsonl_lines(
        &self,
        content: &str,
    ) -> Result<PiImportReport, SessionStoreError> {
        self.import_pi_jsonl_reader(BufReader::new(content.as_bytes()))
            .await
    }

    /// 从异步缓冲流逐行导入；文件入口不会同步读取或把完整 JSONL 放入内存。
    async fn import_pi_jsonl_reader<R>(
        &self,
        reader: R,
    ) -> Result<PiImportReport, SessionStoreError>
    where
        R: AsyncBufRead + Unpin,
    {
        let mut report = PiImportReport::default();
        let mut session: Option<SessionId> = None;
        let mut pending_entries = Vec::new();
        let mut next_sequence = 1u64;
        let mut lines = reader.lines();

        while let Some(raw) = lines.next_line().await? {
            report.total_lines += 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            match parse_pi_line(&raw) {
                Some(entry) => {
                    report.parsed_entries += 1;
                    if entry.kind == PiEntryKind::Unknown || !entry.unknown_fields.is_empty() {
                        report.record_unknown_entry(report.total_lines, trimmed.to_string());
                    }

                    if let PiPayload::Header { session_id, title } = &entry.payload {
                        if session.is_none() {
                            let imported_session = SessionId::from(session_id.clone());
                            self.create_session(
                                &imported_session,
                                title.clone().unwrap_or_else(|| "imported".into()),
                                Timestamp::from_unix_millis(1),
                            )
                            .await?;
                            report.header_found = true;
                            session = Some(imported_session);

                            for pending in pending_entries.drain(..) {
                                self.import_pi_entry(
                                    session.as_ref().expect("session was just initialized"),
                                    pending,
                                    &mut next_sequence,
                                    &mut report,
                                )
                                .await?;
                            }
                        }
                        continue;
                    }

                    if let Some(session) = session.as_ref() {
                        self.import_pi_entry(session, entry, &mut next_sequence, &mut report)
                            .await?;
                    } else {
                        // 宽松支持 header 前的记录；只缓存 header 前的小段，而非整个文件。
                        pending_entries.push(entry);
                    }
                }
                None => {
                    // 无法解析为 JSON 的行
                    report.record_unknown_entry(report.total_lines, trimmed.to_string());
                }
            }
        }

        if session.is_none() {
            return Err(SessionStoreError::ProjectionInvariant(
                "pi jsonl missing header (session_id)".into(),
            ));
        }

        Ok(report)
    }

    async fn import_pi_entry(
        &self,
        session: &SessionId,
        entry: PiParsedEntry,
        next_sequence: &mut u64,
        report: &mut PiImportReport,
    ) -> Result<(), SessionStoreError> {
        let payload = match entry.payload {
            PiPayload::Message {
                sequence,
                role,
                text,
            } => {
                report.imported_messages += 1;
                AgentEvent::MessageCommitted {
                    message: Message {
                        id: MessageId::from(format!("pi-msg-{sequence}")),
                        role: role.unwrap_or(MessageRole::User),
                        content: vec![ContentPart::Text(TextContent {
                            text: text.unwrap_or_default(),
                        })],
                        metadata: MessageMetadata::default(),
                    },
                }
            }
            PiPayload::ToolCall {
                sequence,
                tool_call_id,
                name,
            } => {
                report.imported_tool_calls += 1;
                AgentEvent::ToolCallStarted {
                    tool_call_id: ToolCallId::from(
                        tool_call_id.unwrap_or_else(|| format!("pi-tool-{sequence}")),
                    ),
                    name: name.unwrap_or_default(),
                }
            }
            PiPayload::ModelSwitch { sequence, model } => {
                // agent-events 当前没有专用 ModelSwitched 变体；用 canonical Diagnostic
                // 如实持久化，而不是只增加报告计数后丢弃原始信息。
                report.imported_model_switches += 1;
                AgentEvent::Diagnostic {
                    code: "pi.model_switched".into(),
                    details: serde_json::json!({
                        "source_sequence": sequence,
                        "model": model,
                    }),
                }
            }
            PiPayload::Compaction {
                sequence,
                summary: _,
            } => {
                report.imported_compactions += 1;
                AgentEvent::CompactionCompleted {
                    summary_message_id: MessageId::from(format!("pi-summary-{sequence}")),
                    compacted_through: EventSequence::new(sequence.max(1)),
                }
            }
            PiPayload::Branch { branch_id, parent } => {
                if let Some(branch_id) = branch_id {
                    if branch_id != DEFAULT_BRANCH_ID {
                        self.create_branch(session, branch_id, parent, None).await?;
                        report.imported_branches += 1;
                    }
                }
                return Ok(());
            }
            PiPayload::Header { .. } | PiPayload::Raw => return Ok(()),
        };

        let envelope = AgentEventEnvelope::new(
            agent_domain::EventId::from(format!("pi-event-{next_sequence}")),
            session.clone(),
            agent_domain::RunId::from("pi-import"),
            EventSequence::new(*next_sequence),
            Timestamp::from_unix_millis(*next_sequence),
            payload,
        );
        self.append_event(DEFAULT_BRANCH_ID, envelope).await?;
        *next_sequence = next_sequence
            .checked_add(1)
            .ok_or(SessionStoreError::SequenceOverflow)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::SessionStore;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path() -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("pawork-pi-{}-{unique}.sqlite3", std::process::id()))
    }

    fn pi_file(content: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pawork-pi-src-{unique}.jsonl"));
        fs::write(&path, content).expect("write pi file");
        path
    }

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

    #[tokio::test]
    async fn import_pi_does_not_modify_original_file() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let pi_content = concat!(
            r#"{"session_id":"pi-session","title":"legacy"}"#,
            "\n",
            r#"{"sequence":1,"role":"user","text":"hello"}"#,
            "\n",
            r#"{"sequence":2,"tool":"run_command","tool_call_id":"t1"}"#,
            "\n",
            r#"{"sequence":3,"model":"gpt-x"}"#,
            "\n",
            r#"{"weird":"value"}"#,
            "\n",
            r#"{"future_entry":{"enabled":true}}"#,
        );
        let src = pi_file(pi_content);
        let before = fs::read_to_string(&src).unwrap();

        let report = store.import_pi_jsonl(&src).await.expect("import");
        assert!(report.header_found);
        assert_eq!(report.imported_messages, 1);
        assert_eq!(report.imported_tool_calls, 1);
        assert_eq!(report.imported_model_switches, 1);
        assert_eq!(report.unknown_entries.len(), 2);
        assert_eq!(
            report.unknown_entries.get(&5).map(String::as_str),
            Some(r#"{"weird":"value"}"#)
        );
        assert_eq!(
            report.unknown_entries.get(&6).map(String::as_str),
            Some(r#"{"future_entry":{"enabled":true}}"#)
        );

        // 原文件未被修改。
        let after = fs::read_to_string(&src).unwrap();
        assert_eq!(before, after);

        // 事件已导入新库。
        let events = store
            .replay_events(&SessionId::from("pi-session"), 1, 100)
            .await
            .expect("replay");
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[2].payload,
            AgentEvent::Diagnostic { code, details }
                if code == "pi.model_switched"
                    && details.get("source_sequence") == Some(&serde_json::json!(3))
                    && details.get("model") == Some(&serde_json::json!("gpt-x"))
        ));

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(src);
    }

    #[tokio::test]
    async fn import_pi_missing_header_errors() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let result = store
            .import_pi_jsonl_lines(r#"{"sequence":1,"role":"user","text":"no header"}"#)
            .await;
        assert!(result.is_err());
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
