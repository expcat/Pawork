use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ArtifactId, Citation, Message, ProgramStream,
    ProviderTranscriptEnvelope, RunId, ServerToolEvent, SessionId, Source, ToolCallId,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::session_tree::{load_ancestor_lineage, visible_on_lineage};
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
    branch_id: &str,
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
                "INSERT INTO messages(message_id, session_id, run_id, sequence, role, message_json, branch_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    message.id.to_string(),
                    session_id,
                    run_id,
                    sequence,
                    role,
                    serde_json::to_string(message)?,
                    branch_id
                ],
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
        AgentEvent::CompactionCompleted { compacted_through, .. } => {
            // 压缩语义：sequence <= compacted_through 的消息投影被摘要取代。
            // 事件流保持 append-only；摘要消息自身的 sequence 大于该水位，不受影响。
            let through = i64::try_from(compacted_through.value())
                .map_err(|_| SessionStoreError::SequenceOverflow)?;
            connection.execute(
                "DELETE FROM messages WHERE session_id=?1 AND branch_id=?2 AND sequence<=?3",
                params![session_id, branch_id, through],
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
    /// 按 **active branch** 的祖先链过滤 `messages`；runs / tool_calls 仍为全 session。
    pub async fn projection_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<ProjectionSnapshot, SessionStoreError> {
        let session_id = session_id.to_string();
        self.database()
            .call(move |connection| load_snapshot(connection, &session_id, None))
            .await?
    }

    /// 按指定 branch 的祖先链过滤 `messages`；runs / tool_calls 仍为全 session。
    pub async fn projection_snapshot_on_branch(
        &self,
        session_id: &SessionId,
        branch_id: impl Into<String>,
    ) -> Result<ProjectionSnapshot, SessionStoreError> {
        let session_id = session_id.to_string();
        let branch_id = branch_id.into();
        self.database()
            .call(move |connection| load_snapshot(connection, &session_id, Some(branch_id)))
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
                        "SELECT payload_json, branch_id FROM session_events \
                         WHERE session_id=?1 ORDER BY sequence ASC",
                    )?;
                    let rows = statement
                        .query_map([&session_id], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                for (json, branch_id) in rows {
                    let event: AgentEventEnvelope = serde_json::from_str(&json)?;
                    apply_projection(&transaction, &event, &branch_id)?;
                }
                let snapshot = load_snapshot(&transaction, &session_id, None)?;
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
    branch_id: Option<String>,
) -> Result<ProjectionSnapshot, SessionStoreError> {
    let branch_id = match branch_id {
        Some(branch) => branch,
        None => connection
            .query_row(
                "SELECT active_branch FROM sessions WHERE session_id=?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| SessionStoreError::SessionNotFound(session_id.into()))?,
    };
    let lineage = load_ancestor_lineage(connection, session_id, &branch_id)?;
    let messages = {
        let mut statement = connection.prepare(
            "SELECT message_json, branch_id, sequence FROM messages \
             WHERE session_id=?1 ORDER BY sequence, message_id",
        )?;
        let rows = statement
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .filter(|(_, message_branch, sequence)| {
                visible_on_lineage(&lineage, message_branch, *sequence)
            })
            .map(|(json, _, _)| serde_json::from_str(&json).map_err(SessionStoreError::from))
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
    use std::path::PathBuf;

    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, EventSequence, MessageId, RunId, SessionId, Timestamp,
        ToolCallId,
    };

    use super::*;
    use crate::{SessionStore, DEFAULT_BRANCH_ID};

    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("projection.sqlite3");
        (dir, path)
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

    fn text_message(id: &str, text: &str) -> pawork_domain::Message {
        pawork_domain::Message {
            id: MessageId::from(id),
            role: pawork_domain::MessageRole::User,
            content: vec![pawork_domain::ContentPart::Text(
                pawork_domain::TextContent { text: text.into() },
            )],
            metadata: Default::default(),
        }
    }

    #[tokio::test]
    async fn compaction_completed_replaces_messages_projection_but_keeps_event_stream() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-compaction-projection");
        store
            .create_session(&session, "compaction", Timestamp::from_unix_millis(1))
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
            .expect("seq 1");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    2,
                    AgentEvent::MessageCommitted {
                        message: text_message("m-old-1", "old-1"),
                    },
                ),
            )
            .await
            .expect("seq 2");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    3,
                    AgentEvent::MessageCommitted {
                        message: text_message("m-old-2", "old-2"),
                    },
                ),
            )
            .await
            .expect("seq 3");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(&session, 4, AgentEvent::CompactionStarted { source_event_count: 2 }),
            )
            .await
            .expect("seq 4");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    5,
                    AgentEvent::MessageCommitted {
                        message: text_message("m-summary", "summary"),
                    },
                ),
            )
            .await
            .expect("seq 5");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    6,
                    AgentEvent::CompactionCompleted {
                        summary_message_id: MessageId::from("m-summary"),
                        compacted_through: EventSequence::new(3),
                    },
                ),
            )
            .await
            .expect("seq 6");

        let snapshot = store.projection_snapshot(&session).await.expect("snapshot");
        let ids: Vec<&str> = snapshot
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["m-summary"],
            "sequence <= compacted_through 的消息投影被摘要取代"
        );

        let replay = store
            .replay_events(&session, 1, usize::MAX)
            .await
            .expect("replay");
        assert_eq!(replay.len(), 6, "事件流 append-only 不受压缩影响");

        store.rebuild_projection(&session).await.expect("rebuild");
        let rebuilt = store
            .projection_snapshot(&session)
            .await
            .expect("rebuilt snapshot");
        let ids: Vec<&str> = rebuilt
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(ids, vec!["m-summary"], "重放重建与在线投影语义一致");

        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn append_replay_is_contiguous_and_sql_rows_are_immutable() {
        let (_dir, path) = temp_db();
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
                event(
                    &session,
                    3,
                    AgentEvent::RunCancelled {
                        reason: None,
                        usage: None,
                    },
                ),
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
                event(
                    &session,
                    2,
                    AgentEvent::RunCancelled {
                        reason: None,
                        usage: None,
                    },
                ),
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
    }

    #[tokio::test]
    async fn projection_can_be_deleted_and_rebuilt_exactly() {
        let (_dir, path) = temp_db();
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
                        stop_reason: pawork_domain::StopReason::Completed,
                        usage: pawork_domain::TokenUsage::default(),
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
    }

    #[tokio::test]
    async fn fork_lineage_snapshot_excludes_post_fork_main_messages() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-fork-snapshot");
        store
            .create_session(&session, "fork-snapshot", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        for sequence in 1..=3u64 {
            store
                .append_event(
                    DEFAULT_BRANCH_ID,
                    event(
                        &session,
                        sequence,
                        AgentEvent::MessageCommitted {
                            message: text_message(&format!("m-{sequence}"), &format!("t-{sequence}")),
                        },
                    ),
                )
                .await
                .expect("append");
        }
        store
            .fork_from_event(
                &session,
                "experiment",
                &pawork_domain::EventId::from("event-1"),
            )
            .await
            .expect("fork");
        store
            .switch_branch(&session, "experiment")
            .await
            .expect("switch");

        let active = store.projection_snapshot(&session).await.expect("active");
        let ids: Vec<&str> = active
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(ids, vec!["m-1"], "active=experiment 只含祖先前缀");

        let main = store
            .projection_snapshot_on_branch(&session, DEFAULT_BRANCH_ID)
            .await
            .expect("main");
        let main_ids: Vec<&str> = main
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(main_ids, vec!["m-1", "m-2", "m-3"]);

        store.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn fork_compaction_does_not_delete_main_messages() {
        let (_dir, path) = temp_db();
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let session = SessionId::from("session-fork-compact");
        store
            .create_session(&session, "fork-compact", Timestamp::from_unix_millis(1))
            .await
            .expect("session");
        for sequence in 1..=3u64 {
            store
                .append_event(
                    DEFAULT_BRANCH_ID,
                    event(
                        &session,
                        sequence,
                        AgentEvent::MessageCommitted {
                            message: text_message(&format!("m-{sequence}"), &format!("t-{sequence}")),
                        },
                    ),
                )
                .await
                .expect("append");
        }
        store
            .fork_from_event(
                &session,
                "experiment",
                &pawork_domain::EventId::from("event-1"),
            )
            .await
            .expect("fork");
        store
            .switch_branch(&session, "experiment")
            .await
            .expect("switch");
        store
            .append_event(
                "experiment",
                event(
                    &session,
                    4,
                    AgentEvent::MessageCommitted {
                        message: text_message("m-fork", "fork-only"),
                    },
                ),
            )
            .await
            .expect("fork message");
        store
            .append_event(
                "experiment",
                event(
                    &session,
                    5,
                    AgentEvent::CompactionCompleted {
                        summary_message_id: pawork_domain::MessageId::from("m-fork"),
                        compacted_through: EventSequence::new(4),
                    },
                ),
            )
            .await
            .expect("fork compact");

        let remaining: Vec<(String, String, i64)> = store
            .database()
            .call(|connection| {
                let mut statement = connection
                    .prepare(
                        "SELECT message_id, branch_id, sequence FROM messages \
                         WHERE session_id='session-fork-compact' ORDER BY sequence, message_id",
                    )
                    .expect("prepare");
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .expect("query")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("collect")
            })
            .await
            .expect("actor");
        assert_eq!(
            remaining,
            vec![
                ("m-1".into(), DEFAULT_BRANCH_ID.into(), 1),
                ("m-2".into(), DEFAULT_BRANCH_ID.into(), 2),
                ("m-3".into(), DEFAULT_BRANCH_ID.into(), 3),
            ],
            "fork 压缩不得删 main 中低于全局水位的消息"
        );

        store
            .switch_branch(&session, DEFAULT_BRANCH_ID)
            .await
            .expect("switch main");
        let main = store.projection_snapshot(&session).await.expect("main");
        let ids: Vec<&str> = main
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(ids, vec!["m-1", "m-2", "m-3"]);

        store.shutdown().await.expect("shutdown");
    }
}
