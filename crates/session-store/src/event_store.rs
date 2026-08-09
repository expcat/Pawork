use agent_domain::{SessionId, Timestamp};
use agent_events::{AgentEvent, AgentEventEnvelope};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use crate::{projection::apply_projection, SessionStore, SessionStoreError};

pub const DEFAULT_BRANCH_ID: &str = "main";
const REDACTED_SECRET: &str = "[REDACTED]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendReceipt {
    pub event_id: String,
    pub sequence: u64,
    pub branch_id: String,
}

impl SessionStore {
    pub async fn create_session(
        &self,
        session_id: &SessionId,
        title: impl Into<String>,
        created_at: Timestamp,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let title = title.into();
        let timestamp = i64::try_from(created_at.as_unix_millis()).map_err(|_| {
            SessionStoreError::ProjectionInvariant("timestamp exceeds SQLite INTEGER".into())
        })?;
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let transaction = connection.transaction()?;
                transaction.execute(
                    "INSERT INTO sessions(session_id, title, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?3)",
                    params![session_id, title, timestamp],
                )?;
                transaction.execute(
                    "INSERT INTO session_branches(branch_id, session_id, head_sequence) VALUES (?1, ?2, 0)",
                    params![DEFAULT_BRANCH_ID, session_id],
                )?;
                transaction.commit()?;
                Ok(())
            })
            .await??;
        Ok(())
    }

    pub async fn create_branch(
        &self,
        session_id: &SessionId,
        branch_id: impl Into<String>,
        parent_branch_id: Option<String>,
        forked_from_event_id: Option<String>,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let branch_id = branch_id.into();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let session_exists: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
                    [&session_id],
                    |row| row.get(0),
                )?;
                if !session_exists {
                    return Err(SessionStoreError::SessionNotFound(session_id));
                }
                let head_sequence = if let Some(event_id) = forked_from_event_id.as_deref() {
                    connection
                        .query_row(
                            "SELECT sequence FROM session_events WHERE session_id=?1 AND event_id=?2",
                            params![session_id, event_id],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()?
                        .ok_or_else(|| SessionStoreError::ParentEventNotFound(event_id.into()))?
                } else {
                    0
                };
                connection.execute(
                    "INSERT INTO session_branches(branch_id, session_id, parent_branch_id, forked_from_event_id, head_sequence) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![branch_id, session_id, parent_branch_id, forked_from_event_id, head_sequence],
                )?;
                Ok(())
            })
            .await??;
        Ok(())
    }

    /// 切换 session 的 active branch；后续 [`SessionStore::append_event`] 只允许写入该 branch。
    pub async fn switch_branch(
        &self,
        session_id: &SessionId,
        branch_id: impl Into<String>,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.to_string();
        let branch_id = branch_id.into();
        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let transaction = connection.transaction()?;
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id=?1)",
                    [&session_id],
                    |row| row.get(0),
                )?;
                if !exists {
                    return Err(SessionStoreError::SessionNotFound(session_id));
                }
                let branch_exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_branches \
                     WHERE session_id=?1 AND branch_id=?2)",
                    params![session_id, branch_id.clone()],
                    |row| row.get(0),
                )?;
                if !branch_exists {
                    return Err(SessionStoreError::BranchNotFound {
                        session_id,
                        branch_id,
                    });
                }
                transaction.execute(
                    "UPDATE sessions SET active_branch=?1 WHERE session_id=?2",
                    params![branch_id, session_id],
                )?;
                transaction.commit()?;
                Ok(())
            })
            .await?
    }

    pub async fn append_event(
        &self,
        branch_id: impl Into<String>,
        event: AgentEventEnvelope,
    ) -> Result<AppendReceipt, SessionStoreError> {
        let branch_id = branch_id.into();
        // Event Store 是持久化安全边界：写入事实表和 Projection 的必须是同一份脱敏事件。
        let event = redact_event_for_persistence(&event)?;
        let event_id = event.event_id.to_string();
        let session_id = event.session_id.to_string();
        let sequence = event.sequence.value();
        let run_id = event.run_id.to_string();
        let parent_event_id = event.parent_event_id.as_ref().map(ToString::to_string);
        let schema_version = i64::from(event.schema_version);
        let timestamp = i64::try_from(event.timestamp.as_unix_millis()).map_err(|_| {
            SessionStoreError::ProjectionInvariant("timestamp exceeds SQLite INTEGER".into())
        })?;
        let event_type = event_type(&event.payload);
        let payload_json = serde_json::to_string(&event)?;
        let receipt_branch = branch_id.clone();
        let receipt_event = event_id.clone();

        self.database()
            .call(move |connection| -> Result<(), SessionStoreError> {
                let transaction = connection.transaction()?;
                let branch_exists: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM session_branches WHERE session_id=?1 AND branch_id=?2)",
                        params![session_id, branch_id],
                        |row| row.get(0),
                    )?;
                if !branch_exists {
                    return Err(SessionStoreError::BranchNotFound {
                        session_id,
                        branch_id,
                    });
                }
                // 只允许向 session 当前 active branch 追加事件，保护多分支并发写。
                let active_branch: String = transaction.query_row(
                    "SELECT active_branch FROM sessions WHERE session_id=?1",
                    [&session_id],
                    |row| row.get(0),
                )?;
                if active_branch != branch_id {
                    return Err(SessionStoreError::BranchNotActive {
                        session_id,
                        active_branch,
                        requested_branch: branch_id,
                    });
                }

                let previous: i64 = transaction.query_row(
                    "SELECT COALESCE(MAX(sequence), 0) FROM session_events WHERE session_id=?1",
                    [&session_id],
                    |row| row.get(0),
                )?;
                let expected = u64::try_from(previous)
                    .ok()
                    .and_then(|value| value.checked_add(1))
                    .ok_or(SessionStoreError::SequenceOverflow)?;
                if sequence != expected {
                    return Err(SessionStoreError::NonContiguousSequence {
                        expected,
                        actual: sequence,
                    });
                }
                if let Some(parent) = parent_event_id.as_deref() {
                    let exists: bool = transaction.query_row(
                        "SELECT EXISTS(SELECT 1 FROM session_events WHERE session_id=?1 AND event_id=?2)",
                        params![session_id, parent],
                        |row| row.get(0),
                    )?;
                    if !exists {
                        return Err(SessionStoreError::ParentEventNotFound(parent.into()));
                    }
                }
                let sequence_i64 = i64::try_from(sequence)
                    .map_err(|_| SessionStoreError::SequenceOverflow)?;
                transaction.execute(
                    "INSERT INTO session_events(event_id, session_id, branch_id, run_id, parent_event_id, sequence, event_type, schema_version, timestamp_ms, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![event_id, session_id, branch_id, run_id, parent_event_id, sequence_i64, event_type, schema_version, timestamp, payload_json],
                )?;
                apply_projection(&transaction, &event)?;
                transaction.execute(
                    "UPDATE session_branches SET head_sequence=?1 WHERE session_id=?2 AND branch_id=?3",
                    params![sequence_i64, session_id, branch_id],
                )?;
                transaction.execute(
                    "UPDATE sessions SET updated_at_ms=?1 WHERE session_id=?2",
                    params![timestamp, session_id],
                )?;
                transaction.commit()?;
                Ok(())
            })
            .await??;

        Ok(AppendReceipt {
            event_id: receipt_event,
            sequence,
            branch_id: receipt_branch,
        })
    }

    /// 重放整个 session 的事件流，不区分 branch。
    ///
    /// 返回该 session 中 `sequence >= from_sequence` 的事件，按全局 sequence 升序。
    /// 调用方若只需单一分支，必须使用 [`SessionStore::events_by_branch`]，避免将兄弟
    /// branch 的事件混入上下文或压缩区间。
    pub async fn replay_events(
        &self,
        session_id: &SessionId,
        from_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AgentEventEnvelope>, SessionStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let session_id = session_id.to_string();
        let from_sequence =
            i64::try_from(from_sequence).map_err(|_| SessionStoreError::SequenceOverflow)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let json_rows = self.database().call(move |connection| -> rusqlite::Result<Vec<String>> {
            let mut statement = connection.prepare(
                "SELECT payload_json FROM session_events WHERE session_id=?1 AND sequence>=?2 ORDER BY sequence ASC LIMIT ?3",
            )?;
            let rows = statement
                .query_map(params![session_id, from_sequence, limit], |row| row.get(0))?
                .collect();
            rows
        }).await??;
        json_rows
            .into_iter()
            .map(|json| serde_json::from_str(&json).map_err(SessionStoreError::from))
            .collect()
    }

    /// 读取整个 session 的尾部事件，不区分 branch。
    ///
    /// 结果按全局 sequence 升序返回；`limit` 仅决定从 session 全局事件流尾部取多少条。
    /// 单一分支读取应使用 [`SessionStore::events_by_branch`] 并由调用方选择起始 sequence。
    pub async fn tail_events(
        &self,
        session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<AgentEventEnvelope>, SessionStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let session_id = session_id.to_string();
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut json_rows = self.database().call(move |connection| -> rusqlite::Result<Vec<String>> {
            let mut statement = connection.prepare(
                "SELECT payload_json FROM session_events WHERE session_id=?1 ORDER BY sequence DESC LIMIT ?2",
            )?;
            let rows = statement
                .query_map(params![session_id, limit], |row| row.get(0))?
                .collect();
            rows
        }).await??;
        json_rows.reverse();
        json_rows
            .into_iter()
            .map(|json| serde_json::from_str(&json).map_err(SessionStoreError::from))
            .collect()
    }
}

fn redact_event_for_persistence(
    event: &AgentEventEnvelope,
) -> Result<AgentEventEnvelope, serde_json::Error> {
    let mut value = serde_json::to_value(event)?;
    redact_sensitive_json(&mut value);
    serde_json::from_value(value)
}

fn redact_sensitive_json(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                if is_sensitive_key(key) || is_sensitive_container(key) {
                    redact_value_preserving_shape(child);
                } else {
                    redact_sensitive_json(child);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_sensitive_json(item);
            }
        }
        _ => {}
    }
}

fn redact_value_preserving_shape(value: &mut Value) {
    match value {
        Value::Null => {}
        Value::Bool(value) => *value = false,
        Value::Number(value) => *value = 0.into(),
        Value::String(value) => {
            value.clear();
            value.push_str(REDACTED_SECRET);
        }
        Value::Array(items) => {
            for item in items {
                redact_value_preserving_shape(item);
            }
        }
        Value::Object(fields) => {
            for child in fields.values_mut() {
                redact_value_preserving_shape(child);
            }
        }
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = normalize_json_key(key);
    if [
        "authorization",
        "apikey",
        "accesskey",
        "privatekey",
        "secret",
        "password",
        "cookie",
        "oauthcode",
    ]
    .iter()
    .any(|fragment| normalized.contains(fragment))
    {
        return true;
    }

    if [
        "accesstoken",
        "refreshtoken",
        "authtoken",
        "bearertoken",
        "idtoken",
    ]
    .iter()
    .any(|fragment| normalized.contains(fragment))
    {
        return true;
    }

    // Token 计数/预算是可重放语义，不是凭证。其余 singular token 形态默认按敏感值处理。
    if normalized.contains("token") {
        let known_count_or_metadata = normalized.ends_with("tokens")
            || matches!(
                normalized.as_str(),
                "tokenusage"
                    | "tokencount"
                    | "tokenbudget"
                    | "tokenlimit"
                    | "tokenestimate"
                    | "tokenizer"
                    | "tokenspersecond"
                    | "tokentype"
                    | "tokenendpoint"
            );
        if !known_count_or_metadata {
            return true;
        }
    }

    matches!(
        normalized.as_str(),
        "credential" | "credentials" | "credentialvalue"
    )
}

fn is_sensitive_container(key: &str) -> bool {
    matches!(
        normalize_json_key(key).as_str(),
        "headers" | "requestheaders" | "responseheaders"
    )
}

fn normalize_json_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn event_type(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::RunStarted { .. } => "run_started",
        AgentEvent::ContextPrepared { .. } => "context_prepared",
        AgentEvent::ProviderRequestStarted { .. } => "provider_request_started",
        AgentEvent::AssistantTextDelta { .. } => "assistant_text_delta",
        AgentEvent::AssistantThinkingDelta { .. } => "assistant_thinking_delta",
        AgentEvent::ToolCallStarted { .. } => "tool_call_started",
        AgentEvent::ToolCallArgumentsDelta { .. } => "tool_call_arguments_delta",
        AgentEvent::ToolApprovalRequested { .. } => "tool_approval_requested",
        AgentEvent::ToolApprovalResponded { .. } => "tool_approval_responded",
        AgentEvent::ToolExecutionStarted { .. } => "tool_execution_started",
        AgentEvent::ToolOutputDelta { .. } => "tool_output_delta",
        AgentEvent::ToolExecutionCompleted { .. } => "tool_execution_completed",
        AgentEvent::MessageCommitted { .. } => "message_committed",
        AgentEvent::CompactionStarted { .. } => "compaction_started",
        AgentEvent::CompactionCompleted { .. } => "compaction_completed",
        AgentEvent::CheckpointCreated { .. } => "checkpoint_created",
        AgentEvent::CheckpointRolledBack { .. } => "checkpoint_rolled_back",
        AgentEvent::RunCompleted { .. } => "run_completed",
        AgentEvent::RunCancelled { .. } => "run_cancelled",
        AgentEvent::RunFailed { .. } => "run_failed",
        AgentEvent::Diagnostic { .. } => "diagnostic",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use agent_domain::{
        EventId, Message, MessageId, MessageMetadata, MessageRole, RunId, SessionId, Timestamp,
        TokenUsage,
    };
    use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path() -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-branch-switch-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn event(session: &SessionId, sequence: u64, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{sequence}")),
            session.clone(),
            RunId::from("run-1"),
            EventSequence::new(sequence),
            Timestamp::from_unix_millis(1_000 + sequence),
            payload,
        )
    }

    #[tokio::test]
    async fn append_redacts_sensitive_values_before_event_projection_and_replay() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-redaction");
        store
            .create_session(&session, "redaction", Timestamp::from_unix_millis(1))
            .await
            .expect("session");

        let api_key = "fake-api-key-that-must-not-reach-sqlite";
        let header_secret = "fake-custom-header-secret";
        let access_token = "fake-access-token";
        let secret_key = "fake-secret-key";
        let secret_access_key = "fake-secret-access-key";
        let aws_secret_access_key = "fake-aws-secret-access-key";
        let password_hash = "fake-password-hash";
        let provider_metadata = [
            (
                "provider_options".into(),
                serde_json::json!({
                    "temperature": 0.2,
                    "api_key": api_key,
                }),
            ),
            (
                "headers".into(),
                serde_json::json!({ "X-Custom-Auth": header_secret }),
            ),
            ("access_token".into(), serde_json::json!(access_token)),
            ("secret_key".into(), serde_json::json!(secret_key)),
            (
                "secret_access_key".into(),
                serde_json::json!(secret_access_key),
            ),
            (
                "AWS_SECRET_ACCESS_KEY".into(),
                serde_json::json!(aws_secret_access_key),
            ),
            ("password_hash".into(), serde_json::json!(password_hash)),
            ("safe".into(), serde_json::json!("preserved")),
        ]
        .into_iter()
        .collect();
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::MessageCommitted {
                        message: Message {
                            id: MessageId::from("message-redacted"),
                            role: MessageRole::Assistant,
                            content: Vec::new(),
                            metadata: MessageMetadata {
                                usage: Some(TokenUsage {
                                    input_tokens: 12,
                                    output_tokens: 3,
                                    ..TokenUsage::default()
                                }),
                                provider_metadata,
                                ..MessageMetadata::default()
                            },
                        },
                    },
                ),
            )
            .await
            .expect("append");

        let (event_json, projection_json): (String, String) = store
            .database()
            .call(|connection| {
                connection.query_row(
                    "SELECT e.payload_json, m.message_json FROM session_events e \
                     JOIN messages m ON m.message_id='message-redacted' \
                     WHERE e.event_id='event-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .await
            .expect("database actor")
            .expect("persistence query");
        for forbidden in [
            api_key,
            header_secret,
            access_token,
            secret_key,
            secret_access_key,
            aws_secret_access_key,
            password_hash,
        ] {
            assert!(!event_json.contains(forbidden), "event leaked: {forbidden}");
            assert!(
                !projection_json.contains(forbidden),
                "projection leaked: {forbidden}"
            );
        }
        assert!(event_json.contains(REDACTED_SECRET));
        assert!(projection_json.contains(REDACTED_SECRET));

        let replayed = store.replay_events(&session, 1, 10).await.expect("replay");
        assert_eq!(replayed.len(), 1);
        let AgentEvent::MessageCommitted { message } = &replayed[0].payload else {
            panic!("redacted event must keep its schema");
        };
        let details = &message.metadata.provider_metadata;
        assert_eq!(details["provider_options"]["temperature"], 0.2);
        assert_eq!(details["provider_options"]["api_key"], REDACTED_SECRET);
        assert_eq!(details["headers"]["X-Custom-Auth"], REDACTED_SECRET);
        assert_eq!(details["access_token"], REDACTED_SECRET);
        assert_eq!(details["secret_key"], REDACTED_SECRET);
        assert_eq!(details["secret_access_key"], REDACTED_SECRET);
        assert_eq!(details["AWS_SECRET_ACCESS_KEY"], REDACTED_SECRET);
        assert_eq!(details["password_hash"], REDACTED_SECRET);
        assert_eq!(details["safe"], "preserved");
        assert_eq!(
            message.metadata.usage.as_ref().expect("usage").input_tokens,
            12
        );

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn switch_branch_routes_appends_to_active_branch_only() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-switch");
        store
            .create_session(&session, "switch", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::RunStarted {
                        trigger_message_id: MessageId::from("t"),
                    },
                ),
            )
            .await
            .expect("append main 1");
        store
            .fork_from_event(&session, "exp", &EventId::from("event-1"))
            .await
            .expect("fork");

        // active 仍是 main：写入 exp 被拒绝。
        let blocked = store
            .append_event(
                "exp",
                event(&session, 2, AgentEvent::RunCancelled { reason: None }),
            )
            .await;
        assert!(matches!(
            blocked,
            Err(SessionStoreError::BranchNotActive {
                ref active_branch,
                ref requested_branch,
                ..
            }) if active_branch == DEFAULT_BRANCH_ID && requested_branch == "exp"
        ));

        // 切换到 exp 后写入成功；写回 main 被拒绝。
        store
            .switch_branch(&session, "exp")
            .await
            .expect("switch to exp");
        store
            .append_event(
                "exp",
                event(&session, 2, AgentEvent::RunCancelled { reason: None }),
            )
            .await
            .expect("append exp 2");
        let blocked_main = store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(&session, 3, AgentEvent::RunCancelled { reason: None }),
            )
            .await;
        assert!(matches!(
            blocked_main,
            Err(SessionStoreError::BranchNotActive {
                ref active_branch,
                ..
            }) if active_branch == "exp"
        ));

        // 切回 main：sequence 3 仍是下一个全局连续值。
        store
            .switch_branch(&session, DEFAULT_BRANCH_ID)
            .await
            .expect("switch back");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(&session, 3, AgentEvent::RunCancelled { reason: None }),
            )
            .await
            .expect("append main 3");

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn switch_branch_rejects_unknown_session_and_branch() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-switch-err");
        store
            .create_session(&session, "switch-err", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        let missing_session = store
            .switch_branch(&SessionId::from("nope"), DEFAULT_BRANCH_ID)
            .await;
        assert!(matches!(
            missing_session,
            Err(SessionStoreError::SessionNotFound(_))
        ));
        let missing_branch = store.switch_branch(&session, "ghost").await;
        assert!(matches!(
            missing_branch,
            Err(SessionStoreError::BranchNotFound { ref branch_id, .. }) if branch_id == "ghost"
        ));
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
