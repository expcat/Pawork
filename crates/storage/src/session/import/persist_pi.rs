//! Pi JSONL 导入的 store 写入：Secret 前缀拒绝 + 同一 Immediate 事务。

use std::path::Path;

use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ContentPart, EventId, EventSequence, Message, MessageId,
    MessageMetadata, MessageRole, RunId, SessionId, TextContent, Timestamp, ToolCallId,
};
use rusqlite::{params, Transaction, TransactionBehavior};

use crate::session::event_store::persist_event_in_transaction;
use crate::session::import::formats::compat::find_secret;
use crate::session::import::formats::pi::{
    parse_pi_line, PiEntryKind, PiImportReport, PiParsedEntry, PiPayload,
};
use crate::session::{SessionStore, SessionStoreError, DEFAULT_BRANCH_ID};

const PI_TENANT: &str = "local/default";
const PI_PRINCIPAL: &str = "local/user";

impl SessionStore {
    /// 从 Pi JSONL 文件导入 session，**不修改原文件**。
    pub async fn import_pi_jsonl(
        &self,
        source_path: &Path,
    ) -> Result<PiImportReport, SessionStoreError> {
        let content = tokio::fs::read_to_string(source_path).await?;
        self.import_pi_jsonl_lines(&content).await
    }

    /// 从已读取的 Pi JSONL 内容导入（便于测试与流式来源）。
    pub async fn import_pi_jsonl_lines(
        &self,
        content: &str,
    ) -> Result<PiImportReport, SessionStoreError> {
        if let Some(pattern) = find_secret(content) {
            return Err(SessionStoreError::CompatSecretDetected {
                pattern: pattern.into(),
            });
        }

        let mut report = PiImportReport::default();
        let mut entries = Vec::new();
        for raw in content.lines() {
            report.total_lines += 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            match parse_pi_line(raw) {
                Some(entry) => {
                    report.parsed_entries += 1;
                    if entry.kind == PiEntryKind::Unknown || !entry.unknown_fields.is_empty() {
                        report.record_unknown_entry(report.total_lines, trimmed.to_string());
                    }
                    entries.push(entry);
                }
                None => {
                    report.record_unknown_entry(report.total_lines, trimmed.to_string());
                }
            }
        }

        let header_idx = entries
            .iter()
            .position(|entry| matches!(entry.payload, PiPayload::Header { .. }));
        let Some(header_idx) = header_idx else {
            return Err(SessionStoreError::ProjectionInvariant(
                "pi jsonl missing header (session_id)".into(),
            ));
        };
        let PiPayload::Header { session_id, title } = entries[header_idx].payload.clone() else {
            unreachable!("header_idx points at a Header payload");
        };
        report.header_found = true;

        let mut ordered = Vec::with_capacity(entries.len().saturating_sub(1));
        ordered.extend(entries.iter().take(header_idx).cloned());
        ordered.extend(entries.iter().skip(header_idx + 1).cloned());

        let title = title.unwrap_or_else(|| "imported".into());
        self.database()
            .call(
                move |connection| -> Result<PiImportReport, SessionStoreError> {
                    let transaction =
                        connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    persist_pi_in_transaction(
                        &transaction,
                        &session_id,
                        &title,
                        ordered,
                        &mut report,
                    )?;
                    transaction.commit()?;
                    Ok(report)
                },
            )
            .await?
    }
}

fn persist_pi_in_transaction(
    transaction: &Transaction<'_>,
    session_id: &str,
    title: &str,
    entries: Vec<PiParsedEntry>,
    report: &mut PiImportReport,
) -> Result<(), SessionStoreError> {
    transaction.execute(
        "INSERT INTO sessions(session_id, title, created_at_ms, updated_at_ms, tenant_id, principal_id) \
         VALUES (?1, ?2, 1, 1, ?3, ?4)",
        params![session_id, title, PI_TENANT, PI_PRINCIPAL],
    )?;
    transaction.execute(
        "INSERT INTO session_branches(branch_id, session_id, head_sequence) VALUES (?1, ?2, 0)",
        params![DEFAULT_BRANCH_ID, session_id],
    )?;

    let mut next_sequence = 1u64;
    for entry in entries {
        persist_pi_entry(transaction, session_id, entry, &mut next_sequence, report)?;
    }
    Ok(())
}

fn persist_pi_entry(
    transaction: &Transaction<'_>,
    session: &str,
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
            // R6 波 B：Pi 导入收编为单分支语义——Branch marker 不再创建
            // 零事件归属的 branch 行，树始终只有 main；marker 折叠为 main 上的
            // Diagnostic（保留 source branch / parent 供追溯）。
            report.imported_branches += 1;
            AgentEvent::Diagnostic {
                code: "pi.branch_collapsed".into(),
                details: serde_json::json!({
                    "source_branch": branch_id,
                    "parent": parent,
                }),
            }
        }
        PiPayload::Header { .. } | PiPayload::Raw => return Ok(()),
    };

    let envelope = AgentEventEnvelope::new(
        EventId::from(format!("pi-event-{next_sequence}")),
        SessionId::from(session.to_string()),
        RunId::from("pi-import"),
        EventSequence::new(*next_sequence),
        Timestamp::from_unix_millis(*next_sequence),
        payload,
    );
    persist_event_in_transaction(transaction, DEFAULT_BRANCH_ID, &envelope)?;
    *next_sequence = next_sequence
        .checked_add(1)
        .ok_or(SessionStoreError::SequenceOverflow)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use pawork_domain::{AgentEvent, SessionId};

    use super::*;
    use crate::session::SessionStore;

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

    #[tokio::test]
    async fn import_pi_collapses_branch_markers_to_main_diagnostics() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let content = concat!(
            r#"{"session_id":"pi-branch","title":"branchy"}"#,
            "\n",
            r#"{"sequence":1,"role":"user","text":"hello"}"#,
            "\n",
            r#"{"branch_id":"feature-x","parent":"main"}"#,
            "\n",
            r#"{"branch_id":"main"}"#,
            "\n",
            r#"{"branch":true}"#,
        );

        let report = store.import_pi_jsonl_lines(content).await.expect("import");
        assert_eq!(report.imported_branches, 3);

        let session = SessionId::from("pi-branch");
        let tree = store.session_tree(&session).await.expect("tree");
        assert_eq!(
            tree.branches
                .iter()
                .map(|node| node.branch_id.as_str())
                .collect::<Vec<_>>(),
            vec!["main"],
            "Pi Branch marker 不创建 branch 行，树始终只有 main"
        );

        let events = store.replay_events(&session, 1, 100).await.expect("replay");
        let collapsed: Vec<&serde_json::Value> = events
            .iter()
            .filter_map(|envelope| match &envelope.payload {
                AgentEvent::Diagnostic { code, details } if code == "pi.branch_collapsed" => {
                    Some(details)
                }
                _ => None,
            })
            .collect();
        assert_eq!(collapsed.len(), 3);
        assert_eq!(
            collapsed[0].get("source_branch"),
            Some(&serde_json::json!("feature-x"))
        );
        assert_eq!(collapsed[0].get("parent"), Some(&serde_json::json!("main")));
        assert_eq!(
            collapsed[1].get("source_branch"),
            Some(&serde_json::json!("main"))
        );
        assert!(
            collapsed[1]
                .get("parent")
                .is_some_and(serde_json::Value::is_null),
            "无 parent 的 marker 以 null 保留字段形状: {:?}",
            collapsed[1]
        );
        assert!(
            collapsed[2]
                .get("source_branch")
                .is_some_and(serde_json::Value::is_null),
            "无 branch_id 的 marker 仍折叠为可追溯 Diagnostic: {:?}",
            collapsed[2]
        );
        assert!(
            collapsed[2]
                .get("parent")
                .is_some_and(serde_json::Value::is_null),
            "无 branch_id/parent 的 marker 保留 null 字段形状: {:?}",
            collapsed[2]
        );

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn import_pi_rejects_secret_and_persists_nothing() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let content = concat!(
            r#"{"session_id":"pi-secret","title":"leaky"}"#,
            "\n",
            r#"{"sequence":1,"role":"user","text":"key sk-ant-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#,
        );
        let error = store
            .import_pi_jsonl_lines(content)
            .await
            .expect_err("must reject secret");
        assert!(matches!(
            error,
            SessionStoreError::CompatSecretDetected { .. }
        ));
        let events = store
            .replay_events(&SessionId::from("pi-secret"), 1, 10)
            .await
            .expect("replay");
        assert!(events.is_empty());
        assert!(matches!(
            store.export_session(&SessionId::from("pi-secret")).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn import_pi_failure_leaves_no_residue() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let content = concat!(
            r#"{"session_id":"pi-dup","title":"dup"}"#,
            "\n",
            r#"{"sequence":1,"tool":"shell","tool_call_id":"same"}"#,
            "\n",
            r#"{"sequence":2,"tool":"shell","tool_call_id":"same"}"#,
        );
        let error = store
            .import_pi_jsonl_lines(content)
            .await
            .expect_err("duplicate tool id must abort");
        assert!(matches!(error, SessionStoreError::Sqlite(..)));
        assert!(matches!(
            store.export_session(&SessionId::from("pi-dup")).await,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
