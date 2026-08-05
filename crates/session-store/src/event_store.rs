use agent_domain::{SessionId, Timestamp};
use agent_events::{AgentEvent, AgentEventEnvelope};
use rusqlite::{params, OptionalExtension};

use crate::{projection::apply_projection, SessionStore, SessionStoreError};

pub const DEFAULT_BRANCH_ID: &str = "main";

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

    pub async fn append_event(
        &self,
        branch_id: impl Into<String>,
        event: AgentEventEnvelope,
    ) -> Result<AppendReceipt, SessionStoreError> {
        let branch_id = branch_id.into();
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
