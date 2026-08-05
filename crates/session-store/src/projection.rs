use agent_domain::{Message, RunId, SessionId, ToolCallId};
use agent_events::{AgentEvent, AgentEventEnvelope};
use rusqlite::{params, Connection};
use serde_json::Value;

use crate::{SessionStore, SessionStoreError};

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedRun {
    pub run_id: RunId,
    pub state: String,
    pub data: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedToolCall {
    pub tool_call_id: ToolCallId,
    pub run_id: RunId,
    pub name: String,
    pub state: String,
    pub arguments_json: String,
    pub result: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectionSnapshot {
    pub messages: Vec<Message>,
    pub runs: Vec<ProjectedRun>,
    pub tool_calls: Vec<ProjectedToolCall>,
}

pub(crate) fn apply_projection(
    connection: &Connection,
    event: &AgentEventEnvelope,
) -> Result<(), SessionStoreError> {
    let session_id = event.session_id.to_string();
    let run_id = event.run_id.to_string();
    let sequence =
        i64::try_from(event.sequence.value()).map_err(|_| SessionStoreError::SequenceOverflow)?;
    let timestamp = i64::try_from(event.timestamp.as_unix_millis()).map_err(|_| {
        SessionStoreError::ProjectionInvariant("timestamp exceeds SQLite INTEGER".into())
    })?;
    match &event.payload {
        AgentEvent::RunStarted { .. } => {
            connection.execute(
                "INSERT INTO runs(run_id, session_id, state, started_at_ms, run_json) VALUES (?1, ?2, 'running', ?3, ?4)",
                params![run_id, session_id, timestamp, serde_json::to_string(&event.payload)?],
            )?;
        }
        AgentEvent::MessageCommitted { message } => {
            let role = serde_json::to_value(&message.role)?
                .as_str()
                .unwrap_or("unknown")
                .to_owned();
            connection.execute(
                "INSERT INTO messages(message_id, session_id, run_id, sequence, role, message_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![message.id.to_string(), session_id, run_id, sequence, role, serde_json::to_string(message)?],
            )?;
        }
        AgentEvent::ToolCallStarted { tool_call_id, name } => {
            connection.execute(
                "INSERT INTO tool_calls(tool_call_id, session_id, run_id, name, state) VALUES (?1, ?2, ?3, ?4, 'collecting_arguments')",
                params![tool_call_id.to_string(), session_id, run_id, name],
            )?;
        }
        AgentEvent::ToolCallArgumentsDelta {
            tool_call_id,
            json_delta,
        } => {
            require_one(
                connection.execute(
                    "UPDATE tool_calls SET arguments_json=arguments_json || ?1 WHERE tool_call_id=?2 AND session_id=?3",
                    params![json_delta, tool_call_id.to_string(), session_id],
                )?,
                "tool arguments event references an unknown tool call",
            )?;
        }
        AgentEvent::ToolApprovalRequested { tool_call_id, .. } => {
            set_tool_state(
                connection,
                tool_call_id,
                &session_id,
                "waiting_for_approval",
            )?;
        }
        AgentEvent::ToolApprovalResponded {
            tool_call_id,
            decision,
            ..
        } => {
            set_tool_state(
                connection,
                tool_call_id,
                &session_id,
                &format!("approval_{decision:?}").to_ascii_lowercase(),
            )?;
        }
        AgentEvent::ToolExecutionStarted { tool_call_id } => {
            set_tool_state(connection, tool_call_id, &session_id, "executing")?;
        }
        AgentEvent::ToolExecutionCompleted {
            tool_call_id,
            result,
        } => {
            require_one(
                connection.execute(
                    "UPDATE tool_calls SET state='completed', result_json=?1 WHERE tool_call_id=?2 AND session_id=?3",
                    params![serde_json::to_string(result)?, tool_call_id.to_string(), session_id],
                )?,
                "tool completion references an unknown tool call",
            )?;
        }
        AgentEvent::RunCompleted { .. } => {
            set_run_state(
                connection,
                &run_id,
                &session_id,
                "completed",
                timestamp,
                &event.payload,
            )?;
        }
        AgentEvent::RunCancelled { .. } => {
            set_run_state(
                connection,
                &run_id,
                &session_id,
                "cancelled",
                timestamp,
                &event.payload,
            )?;
        }
        AgentEvent::RunFailed { .. } => {
            set_run_state(
                connection,
                &run_id,
                &session_id,
                "failed",
                timestamp,
                &event.payload,
            )?;
        }
        _ => {}
    }
    Ok(())
}

impl SessionStore {
    pub async fn projection_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<ProjectionSnapshot, SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(move |connection| load_snapshot(connection, &session_id))
            .await?
    }

    pub async fn rebuild_projection(
        &self,
        session_id: &SessionId,
    ) -> Result<ProjectionSnapshot, SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(move |connection| -> Result<ProjectionSnapshot, SessionStoreError> {
                let transaction = connection.transaction()?;
                transaction.execute("DELETE FROM tool_calls WHERE session_id=?1", [&session_id])?;
                transaction.execute("DELETE FROM messages WHERE session_id=?1", [&session_id])?;
                transaction.execute("DELETE FROM runs WHERE session_id=?1", [&session_id])?;
                let rows = {
                    let mut statement = transaction.prepare(
                        "SELECT payload_json FROM session_events WHERE session_id=?1 ORDER BY sequence ASC",
                    )?;
                    let rows = statement
                        .query_map([&session_id], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                for json in rows {
                    let event: AgentEventEnvelope = serde_json::from_str(&json)?;
                    apply_projection(&transaction, &event)?;
                }
                let snapshot = load_snapshot(&transaction, &session_id)?;
                transaction.commit()?;
                Ok(snapshot)
            })
            .await?
    }
}

fn set_tool_state(
    connection: &Connection,
    tool_call_id: &ToolCallId,
    session_id: &str,
    state: &str,
) -> Result<(), SessionStoreError> {
    require_one(
        connection.execute(
            "UPDATE tool_calls SET state=?1 WHERE tool_call_id=?2 AND session_id=?3",
            params![state, tool_call_id.to_string(), session_id],
        )?,
        "tool state event references an unknown tool call",
    )
}

fn set_run_state(
    connection: &Connection,
    run_id: &str,
    session_id: &str,
    state: &str,
    timestamp: i64,
    payload: &AgentEvent,
) -> Result<(), SessionStoreError> {
    require_one(
        connection.execute(
            "UPDATE runs SET state=?1, completed_at_ms=?2, run_json=?3 WHERE run_id=?4 AND session_id=?5",
            params![state, timestamp, serde_json::to_string(payload)?, run_id, session_id],
        )?,
        "run completion event references an unknown run",
    )
}

fn require_one(changed: usize, message: &str) -> Result<(), SessionStoreError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(SessionStoreError::ProjectionInvariant(message.into()))
    }
}

fn load_snapshot(
    connection: &Connection,
    session_id: &str,
) -> Result<ProjectionSnapshot, SessionStoreError> {
    let messages = {
        let mut statement = connection.prepare(
            "SELECT message_json FROM messages WHERE session_id=?1 ORDER BY sequence, message_id",
        )?;
        let rows = statement
            .query_map([session_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|json| serde_json::from_str(&json).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?
    };
    let runs = {
        let mut statement = connection.prepare(
            "SELECT run_id, state, run_json FROM runs WHERE session_id=?1 ORDER BY started_at_ms, run_id",
        )?;
        let rows = statement
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(run_id, state, data)| {
                Ok(ProjectedRun {
                    run_id: RunId::from(run_id),
                    state,
                    data: serde_json::from_str(&data)?,
                })
            })
            .collect::<Result<Vec<_>, SessionStoreError>>()?
    };
    let tool_calls = {
        let mut statement = connection.prepare(
            "SELECT tool_call_id, run_id, name, state, arguments_json, result_json FROM tool_calls WHERE session_id=?1 ORDER BY tool_call_id",
        )?;
        let rows = statement
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(
                |(tool_call_id, run_id, name, state, arguments_json, result)| {
                    Ok(ProjectedToolCall {
                        tool_call_id: ToolCallId::from(tool_call_id),
                        run_id: RunId::from(run_id),
                        name,
                        state,
                        arguments_json,
                        result: result.map(|json| serde_json::from_str(&json)).transpose()?,
                    })
                },
            )
            .collect::<Result<Vec<_>, SessionStoreError>>()?
    };
    Ok(ProjectionSnapshot {
        messages,
        runs,
        tool_calls,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use agent_domain::{MessageId, Timestamp};
    use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};

    use super::*;
    use crate::{SessionStore, DEFAULT_BRANCH_ID};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path() -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-event-store-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn event(session: &SessionId, sequence: u64, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            format!("event-{sequence}").into(),
            session.clone(),
            RunId::from("run-1"),
            EventSequence::new(sequence),
            Timestamp::from_unix_millis(1_000 + sequence),
            payload,
        )
    }

    #[tokio::test]
    async fn append_replay_is_contiguous_and_sql_rows_are_immutable() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-1");
        store
            .create_session(&session, "test", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .create_session(
                &SessionId::from("session-with-independent-main-branch"),
                "second",
                Timestamp::from_unix_millis(1),
            )
            .await
            .expect("second session can also own a main branch");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::RunStarted {
                        trigger_message_id: MessageId::from("trigger"),
                    },
                ),
            )
            .await
            .expect("append");
        let skipped = store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(&session, 3, AgentEvent::RunCancelled { reason: None }),
            )
            .await;
        assert!(matches!(
            skipped,
            Err(SessionStoreError::NonContiguousSequence {
                expected: 2,
                actual: 3
            })
        ));
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(&session, 2, AgentEvent::RunCancelled { reason: None }),
            )
            .await
            .expect("append 2");
        let replay = store.replay_events(&session, 1, 10).await.expect("replay");
        assert_eq!(
            replay
                .iter()
                .map(|event| event.sequence.value())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let tamper = store
            .database()
            .call(|connection| {
                connection.execute(
                    "UPDATE session_events SET sequence=9 WHERE event_id='event-1'",
                    [],
                )
            })
            .await
            .expect("actor");
        assert!(tamper.is_err());
        let deletion = store
            .database()
            .call(|connection| {
                connection.execute("DELETE FROM session_events WHERE event_id='event-1'", [])
            })
            .await
            .expect("actor");
        assert!(deletion.is_err());
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn projection_can_be_deleted_and_rebuilt_exactly() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-2");
        store
            .create_session(&session, "test", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::RunStarted {
                        trigger_message_id: MessageId::from("trigger"),
                    },
                ),
            )
            .await
            .expect("run");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    2,
                    AgentEvent::ToolCallStarted {
                        tool_call_id: ToolCallId::from("tool-1"),
                        name: "read_file".into(),
                    },
                ),
            )
            .await
            .expect("tool");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    3,
                    AgentEvent::ToolCallArgumentsDelta {
                        tool_call_id: ToolCallId::from("tool-1"),
                        json_delta: "{\"path\":\"a\"}".into(),
                    },
                ),
            )
            .await
            .expect("args");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    4,
                    AgentEvent::RunCompleted {
                        stop_reason: agent_domain::StopReason::Completed,
                        usage: agent_domain::TokenUsage::default(),
                    },
                ),
            )
            .await
            .expect("complete");
        let before = store.projection_snapshot(&session).await.expect("snapshot");
        store
            .database()
            .call(|connection| {
                connection.execute("DELETE FROM tool_calls WHERE session_id='session-2'", [])
            })
            .await
            .expect("actor")
            .expect("delete projection");
        let rebuilt = store.rebuild_projection(&session).await.expect("rebuild");
        assert_eq!(rebuilt, before);
        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
