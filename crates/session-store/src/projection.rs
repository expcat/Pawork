use agent_domain::{
    ArtifactId, Citation, Message, ProgramStream, ProviderTranscriptEnvelope, RunId,
    ServerToolEvent, SessionId, Source, ToolCallId,
};
use agent_events::{AgentEvent, AgentEventEnvelope};
use rusqlite::{params, Connection, OptionalExtension};
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

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedServerToolEvent {
    pub tool_call_id: ToolCallId,
    pub run_id: RunId,
    pub name: String,
    pub state: String,
    pub arguments_json: String,
    pub command: Option<String>,
    pub citations: Vec<Citation>,
    pub sources: Vec<Source>,
    pub screenshots: Vec<ProjectedScreenshot>,
    pub outputs: Vec<ProjectedProgramOutput>,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectedProgramOutput {
    pub stream: ProgramStream,
    pub delta: Option<String>,
    pub artifact: Option<ArtifactId>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectedScreenshot {
    pub artifact: ArtifactId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedTranscriptEnvelope {
    pub run_id: RunId,
    pub sequence: u64,
    pub envelope: ProviderTranscriptEnvelope,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectionSnapshot {
    pub messages: Vec<Message>,
    pub runs: Vec<ProjectedRun>,
    pub tool_calls: Vec<ProjectedToolCall>,
    pub server_tool_events: Vec<ProjectedServerToolEvent>,
    pub transcript_envelopes: Vec<ProjectedTranscriptEnvelope>,
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
        AgentEvent::ServerTool(event) => {
            apply_server_tool_event(connection, event, &session_id, &run_id, sequence)?;
        }
        AgentEvent::TranscriptEnvelope(envelope) => {
            connection.execute(
                "INSERT INTO transcript_envelopes(session_id, run_id, sequence, envelope_json) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    session_id,
                    run_id,
                    sequence,
                    serde_json::to_string(envelope)?
                ],
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

fn apply_server_tool_event(
    connection: &Connection,
    event: &ServerToolEvent,
    session_id: &str,
    run_id: &str,
    sequence: i64,
) -> Result<(), SessionStoreError> {
    match event {
        ServerToolEvent::Started {
            tool_call_id,
            name,
            arguments,
        } => {
            connection.execute(
                "INSERT INTO server_tool_events(\
                 tool_call_id, session_id, run_id, sequence, name, state, arguments_json\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'started', ?6)",
                params![
                    tool_call_id.to_string(),
                    session_id,
                    run_id,
                    sequence,
                    name,
                    arguments
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?
                        .unwrap_or_default(),
                ],
            )?;
        }
        ServerToolEvent::ArgumentsDelta {
            tool_call_id,
            json_delta,
        } => {
            require_one(
                connection.execute(
                    "UPDATE server_tool_events SET arguments_json=arguments_json || ?1 \
                     WHERE tool_call_id=?2 AND session_id=?3",
                    params![json_delta, tool_call_id.to_string(), session_id],
                )?,
                "server tool arguments event references an unknown tool call",
            )?;
        }
        ServerToolEvent::Progress { tool_call_id, .. } => {
            set_server_tool_state(connection, tool_call_id, session_id, "running")?;
        }
        ServerToolEvent::Completed {
            tool_call_id,
            summary,
            artifacts,
        } => {
            require_one(
                connection.execute(
                    "UPDATE server_tool_events SET state='completed', result_json=?1 \
                     WHERE tool_call_id=?2 AND session_id=?3",
                    params![
                        serde_json::to_string(&serde_json::json!({
                            "summary": summary,
                            "artifacts": artifacts,
                        }))?,
                        tool_call_id.to_string(),
                        session_id
                    ],
                )?,
                "server tool completion references an unknown tool call",
            )?;
        }
        ServerToolEvent::Failed {
            tool_call_id,
            message,
            code,
        } => {
            require_one(
                connection.execute(
                    "UPDATE server_tool_events SET state='failed', error_json=?1 \
                     WHERE tool_call_id=?2 AND session_id=?3",
                    params![
                        serde_json::to_string(&serde_json::json!({
                            "message": message,
                            "code": code,
                        }))?,
                        tool_call_id.to_string(),
                        session_id
                    ],
                )?,
                "server tool failure references an unknown tool call",
            )?;
        }
        ServerToolEvent::CitationAdded {
            tool_call_id,
            citation,
        } => {
            append_server_tool_json(
                connection,
                tool_call_id,
                session_id,
                "citations_json",
                &serde_json::to_value(citation)?,
            )?;
        }
        ServerToolEvent::SourceAdded {
            tool_call_id,
            source,
        } => {
            append_server_tool_json(
                connection,
                tool_call_id,
                session_id,
                "sources_json",
                &serde_json::to_value(source)?,
            )?;
        }
        ServerToolEvent::ComputerActionRequested {
            tool_call_id,
            action,
        } => {
            require_one(
                connection.execute(
                    "UPDATE server_tool_events SET state='action_requested', result_json=?1 \
                     WHERE tool_call_id=?2 AND session_id=?3",
                    params![
                        serde_json::to_string(action)?,
                        tool_call_id.to_string(),
                        session_id
                    ],
                )?,
                "computer action references an unknown server tool call",
            )?;
        }
        ServerToolEvent::ComputerScreenshot {
            tool_call_id,
            artifact,
            media_type,
        } => {
            append_server_tool_json(
                connection,
                tool_call_id,
                session_id,
                "screenshots_json",
                &serde_json::to_value(ProjectedScreenshot {
                    artifact: artifact.clone(),
                    media_type: media_type.clone(),
                })?,
            )?;
        }
        ServerToolEvent::ProgramStarted {
            tool_call_id,
            command,
        } => {
            require_one(
                connection.execute(
                    "UPDATE server_tool_events SET state='program_running', command=?1 \
                     WHERE tool_call_id=?2 AND session_id=?3",
                    params![command, tool_call_id.to_string(), session_id],
                )?,
                "program start references an unknown server tool call",
            )?;
        }
        ServerToolEvent::ProgramOutput {
            tool_call_id,
            stream,
            delta,
            artifact,
        } => {
            append_server_tool_json(
                connection,
                tool_call_id,
                session_id,
                "outputs_json",
                &serde_json::json!({
                    "stream": stream,
                    "delta": delta,
                    "artifact": artifact,
                }),
            )?;
        }
    }
    Ok(())
}

fn set_server_tool_state(
    connection: &Connection,
    tool_call_id: &ToolCallId,
    session_id: &str,
    state: &str,
) -> Result<(), SessionStoreError> {
    require_one(
        connection.execute(
            "UPDATE server_tool_events SET state=?1 WHERE tool_call_id=?2 AND session_id=?3",
            params![state, tool_call_id.to_string(), session_id],
        )?,
        "server tool state event references an unknown tool call",
    )
}

/// 以 Rust 读改写方式向 server_tool_events 的 JSON 数组列追加一项（不依赖 JSON1）。
fn append_server_tool_json(
    connection: &Connection,
    tool_call_id: &ToolCallId,
    session_id: &str,
    column: &str,
    item: &Value,
) -> Result<(), SessionStoreError> {
    let current: Option<String> = connection
        .query_row(
            &format!(
                "SELECT {column} FROM server_tool_events \
                 WHERE tool_call_id=?1 AND session_id=?2"
            ),
            params![tool_call_id.to_string(), session_id],
            |row| row.get(0),
        )
        .optional()?;
    let current = current.ok_or_else(|| {
        SessionStoreError::ProjectionInvariant(
            "server tool json append references an unknown tool call".into(),
        )
    })?;
    let mut items: Vec<Value> = serde_json::from_str(&current)?;
    items.push(item.clone());
    require_one(
        connection.execute(
            &format!(
                "UPDATE server_tool_events SET {column}=?1 \
                 WHERE tool_call_id=?2 AND session_id=?3"
            ),
            params![
                serde_json::to_string(&items)?,
                tool_call_id.to_string(),
                session_id
            ],
        )?,
        "server tool json append references an unknown tool call",
    )
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
                transaction.execute(
                    "DELETE FROM server_tool_events WHERE session_id=?1",
                    [&session_id],
                )?;
                transaction.execute(
                    "DELETE FROM transcript_envelopes WHERE session_id=?1",
                    [&session_id],
                )?;
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
    let server_tool_events = {
        let mut statement = connection.prepare(
            "SELECT tool_call_id, run_id, name, state, arguments_json, command, \
             citations_json, sources_json, screenshots_json, outputs_json, result_json, error_json \
             FROM server_tool_events WHERE session_id=?1 ORDER BY sequence, tool_call_id",
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
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(
                |(
                    tool_call_id,
                    run_id,
                    name,
                    state,
                    arguments_json,
                    command,
                    citations_json,
                    sources_json,
                    screenshots_json,
                    outputs_json,
                    result,
                    error,
                )| {
                    Ok(ProjectedServerToolEvent {
                        tool_call_id: ToolCallId::from(tool_call_id),
                        run_id: RunId::from(run_id),
                        name,
                        state,
                        arguments_json,
                        command,
                        citations: serde_json::from_str(&citations_json)?,
                        sources: serde_json::from_str(&sources_json)?,
                        screenshots: serde_json::from_str::<Vec<ProjectedScreenshot>>(
                            &screenshots_json,
                        )?,
                        outputs: serde_json::from_str(&outputs_json)?,
                        result: result.map(|json| serde_json::from_str(&json)).transpose()?,
                        error: error.map(|json| serde_json::from_str(&json)).transpose()?,
                    })
                },
            )
            .collect::<Result<Vec<_>, SessionStoreError>>()?
    };
    let transcript_envelopes = {
        let mut statement = connection.prepare(
            "SELECT run_id, sequence, envelope_json FROM transcript_envelopes \
             WHERE session_id=?1 ORDER BY sequence",
        )?;
        let rows = statement
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(run_id, sequence, envelope_json)| {
                Ok(ProjectedTranscriptEnvelope {
                    run_id: RunId::from(run_id),
                    sequence: u64::try_from(sequence).map_err(|_| {
                        SessionStoreError::ProjectionInvariant(
                            "transcript envelope sequence exceeds u64".into(),
                        )
                    })?,
                    envelope: serde_json::from_str(&envelope_json)?,
                })
            })
            .collect::<Result<Vec<_>, SessionStoreError>>()?
    };
    Ok(ProjectionSnapshot {
        messages,
        runs,
        tool_calls,
        server_tool_events,
        transcript_envelopes,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use agent_domain::{CitationSourceKind, MessageId, Timestamp};
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

    fn server_event(
        session: &SessionId,
        sequence: u64,
        inner: ServerToolEvent,
    ) -> AgentEventEnvelope {
        event(session, sequence, AgentEvent::ServerTool(inner))
    }

    /// 全部 11 个 `ServerToolEvent` 变体按生命周期分组、按 sequence 有序 append，
    /// 随后 projection_snapshot → 删除投影 → rebuild_projection，断言重建等价。
    #[tokio::test]
    async fn all_server_tool_variants_append_snapshot_rebuild_in_order() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-all-variants");
        store
            .create_session(&session, "all-variants", Timestamp::from_unix_millis(1))
            .await
            .expect("session");

        let web = ToolCallId::from("st-web");
        let comp = ToolCallId::from("st-comp");
        let prog = ToolCallId::from("st-prog");
        let fail = ToolCallId::from("st-fail");

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

        // Tool A — web_search：Started(带 arguments) → Progress → Citation → Source → Completed
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    2,
                    ServerToolEvent::Started {
                        tool_call_id: web.clone(),
                        name: "web_search".into(),
                        arguments: Some(serde_json::json!({"query":"pawork"})),
                    },
                ),
            )
            .await
            .expect("web started");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    3,
                    ServerToolEvent::Progress {
                        tool_call_id: web.clone(),
                        message: Some("searching".into()),
                    },
                ),
            )
            .await
            .expect("web progress");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    4,
                    ServerToolEvent::CitationAdded {
                        tool_call_id: web.clone(),
                        citation: Citation {
                            url: Some("https://a.example".into()),
                            title: Some("A".into()),
                            source_kind: CitationSourceKind::WebSearch,
                            ..Citation::empty()
                        },
                    },
                ),
            )
            .await
            .expect("web citation");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    5,
                    ServerToolEvent::SourceAdded {
                        tool_call_id: web.clone(),
                        source: Source {
                            url: Some("https://a.example".into()),
                            title: Some("A".into()),
                            ..Source::default()
                        },
                    },
                ),
            )
            .await
            .expect("web source");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    6,
                    ServerToolEvent::Completed {
                        tool_call_id: web.clone(),
                        summary: Some("3 hits".into()),
                        artifacts: vec![ArtifactId::from("art-web-1")],
                    },
                ),
            )
            .await
            .expect("web completed");

        // Tool B — computer-use：Started(None) → ComputerActionRequested → ComputerScreenshot
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    7,
                    ServerToolEvent::Started {
                        tool_call_id: comp.clone(),
                        name: "computer".into(),
                        arguments: None,
                    },
                ),
            )
            .await
            .expect("comp started");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    8,
                    ServerToolEvent::ComputerActionRequested {
                        tool_call_id: comp.clone(),
                        action: serde_json::json!({"type":"screenshot"}),
                    },
                ),
            )
            .await
            .expect("comp action");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    9,
                    ServerToolEvent::ComputerScreenshot {
                        tool_call_id: comp.clone(),
                        artifact: ArtifactId::from("art-shot-1"),
                        media_type: Some("image/png".into()),
                    },
                ),
            )
            .await
            .expect("comp screenshot");

        // Tool C — program：Started(None) → ArgumentsDelta → ProgramStarted → ProgramOutput×2 → Completed
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    10,
                    ServerToolEvent::Started {
                        tool_call_id: prog.clone(),
                        name: "code_interpreter".into(),
                        arguments: None,
                    },
                ),
            )
            .await
            .expect("prog started");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    11,
                    ServerToolEvent::ArgumentsDelta {
                        tool_call_id: prog.clone(),
                        json_delta: r#"{"code":"x"}"#.into(),
                    },
                ),
            )
            .await
            .expect("prog args delta");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    12,
                    ServerToolEvent::ProgramStarted {
                        tool_call_id: prog.clone(),
                        command: Some("run.sh".into()),
                    },
                ),
            )
            .await
            .expect("prog program_started");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    13,
                    ServerToolEvent::ProgramOutput {
                        tool_call_id: prog.clone(),
                        stream: ProgramStream::Stdout,
                        delta: Some("ok".into()),
                        artifact: None,
                    },
                ),
            )
            .await
            .expect("prog stdout");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    14,
                    ServerToolEvent::ProgramOutput {
                        tool_call_id: prog.clone(),
                        stream: ProgramStream::Stderr,
                        delta: None,
                        artifact: Some(ArtifactId::from("art-log-1")),
                    },
                ),
            )
            .await
            .expect("prog stderr");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    15,
                    ServerToolEvent::Completed {
                        tool_call_id: prog.clone(),
                        summary: Some("done".into()),
                        artifacts: Vec::new(),
                    },
                ),
            )
            .await
            .expect("prog completed");

        // Tool D — failure：Started(None) → Failed
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    16,
                    ServerToolEvent::Started {
                        tool_call_id: fail.clone(),
                        name: "search".into(),
                        arguments: None,
                    },
                ),
            )
            .await
            .expect("fail started");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                server_event(
                    &session,
                    17,
                    ServerToolEvent::Failed {
                        tool_call_id: fail.clone(),
                        message: Some("boom".into()),
                        code: Some("ECONNREFUSED".into()),
                    },
                ),
            )
            .await
            .expect("failed");

        let before = store.projection_snapshot(&session).await.expect("snapshot");
        assert_eq!(
            before.server_tool_events.len(),
            4,
            "four server tools projected"
        );
        fn by_id<'a>(snapshot: &'a ProjectionSnapshot, id: &str) -> &'a ProjectedServerToolEvent {
            snapshot
                .server_tool_events
                .iter()
                .find(|event| event.tool_call_id.to_string() == id)
                .unwrap_or_else(|| panic!("missing server tool {id}"))
        }

        let web_evt = by_id(&before, "st-web");
        assert_eq!(web_evt.name, "web_search");
        assert_eq!(web_evt.state, "completed");
        assert_eq!(web_evt.arguments_json, r#"{"query":"pawork"}"#);
        assert_eq!(web_evt.citations.len(), 1);
        assert_eq!(
            web_evt.citations[0].url.as_deref(),
            Some("https://a.example")
        );
        assert_eq!(web_evt.sources.len(), 1);
        assert_eq!(web_evt.result.as_ref().unwrap()["summary"], "3 hits");

        let comp_evt = by_id(&before, "st-comp");
        assert_eq!(comp_evt.state, "action_requested");
        assert_eq!(comp_evt.result.as_ref().unwrap()["type"], "screenshot");
        assert_eq!(comp_evt.screenshots.len(), 1);
        assert_eq!(
            comp_evt.screenshots[0].artifact,
            ArtifactId::from("art-shot-1")
        );
        assert_eq!(
            comp_evt.screenshots[0].media_type.as_deref(),
            Some("image/png")
        );

        let prog_evt = by_id(&before, "st-prog");
        assert_eq!(prog_evt.state, "completed");
        assert_eq!(prog_evt.arguments_json, r#"{"code":"x"}"#);
        assert_eq!(prog_evt.command.as_deref(), Some("run.sh"));
        assert_eq!(prog_evt.outputs.len(), 2);
        assert_eq!(prog_evt.outputs[0].stream, ProgramStream::Stdout);
        assert_eq!(prog_evt.outputs[0].delta.as_deref(), Some("ok"));
        assert_eq!(prog_evt.outputs[1].stream, ProgramStream::Stderr);
        assert_eq!(
            prog_evt.outputs[1].artifact,
            Some(ArtifactId::from("art-log-1"))
        );

        let fail_evt = by_id(&before, "st-fail");
        assert_eq!(fail_evt.state, "failed");
        assert_eq!(fail_evt.error.as_ref().unwrap()["code"], "ECONNREFUSED");

        // 删除投影后精确重建：rebuild 必须与原 snapshot 逐字段相等。
        store
            .database()
            .call(|connection| {
                connection.execute(
                    "DELETE FROM server_tool_events WHERE session_id='session-all-variants'",
                    [],
                )
            })
            .await
            .expect("actor")
            .expect("delete projection");
        let rebuilt = store.rebuild_projection(&session).await.expect("rebuild");
        assert_eq!(rebuilt, before, "rebuild must equal original snapshot");

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    /// 所有引用未知 server tool call 的变体 append 都必须返回
    /// `ProjectionInvariant`（而非底层 `Sqlite` 错误）。
    #[tokio::test]
    async fn unknown_server_tool_variants_return_projection_invariant() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-unknown-tool");
        store
            .create_session(&session, "unknown-tool", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        // sequence 1 落地，之后所有未知 tool 引用都以 sequence 2 提交；
        // 每次 projection 失败都会回滚事务，因此下一个 expected 始终为 2。
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

        let ghost = ToolCallId::from("ghost");
        // 四个 append 类变体（本次修复的目标）。
        let append_cases = vec![
            server_event(
                &session,
                2,
                ServerToolEvent::CitationAdded {
                    tool_call_id: ghost.clone(),
                    citation: Citation::empty(),
                },
            ),
            server_event(
                &session,
                2,
                ServerToolEvent::SourceAdded {
                    tool_call_id: ghost.clone(),
                    source: Source::default(),
                },
            ),
            server_event(
                &session,
                2,
                ServerToolEvent::ComputerScreenshot {
                    tool_call_id: ghost.clone(),
                    artifact: ArtifactId::from("art-ghost"),
                    media_type: None,
                },
            ),
            server_event(
                &session,
                2,
                ServerToolEvent::ProgramOutput {
                    tool_call_id: ghost.clone(),
                    stream: ProgramStream::Stdout,
                    delta: None,
                    artifact: None,
                },
            ),
        ];
        // 若干 UPDATE 类变体（既有行为回归保护）。
        let update_cases = vec![
            server_event(
                &session,
                2,
                ServerToolEvent::ArgumentsDelta {
                    tool_call_id: ghost.clone(),
                    json_delta: "{}".into(),
                },
            ),
            server_event(
                &session,
                2,
                ServerToolEvent::Progress {
                    tool_call_id: ghost.clone(),
                    message: None,
                },
            ),
            server_event(
                &session,
                2,
                ServerToolEvent::Completed {
                    tool_call_id: ghost.clone(),
                    summary: None,
                    artifacts: Vec::new(),
                },
            ),
            server_event(
                &session,
                2,
                ServerToolEvent::Failed {
                    tool_call_id: ghost.clone(),
                    message: None,
                    code: None,
                },
            ),
            server_event(
                &session,
                2,
                ServerToolEvent::ComputerActionRequested {
                    tool_call_id: ghost.clone(),
                    action: serde_json::json!({}),
                },
            ),
            server_event(
                &session,
                2,
                ServerToolEvent::ProgramStarted {
                    tool_call_id: ghost,
                    command: None,
                },
            ),
        ];

        for case in append_cases.into_iter().chain(update_cases) {
            let result = store.append_event(DEFAULT_BRANCH_ID, case).await;
            assert!(
                matches!(result, Err(SessionStoreError::ProjectionInvariant(_))),
                "expected ProjectionInvariant for unknown server tool, got {result:?}"
            );
        }

        // 没有任何未知 tool 被写入投影。
        let snapshot = store.projection_snapshot(&session).await.expect("snapshot");
        assert!(snapshot.server_tool_events.is_empty());

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }
}
