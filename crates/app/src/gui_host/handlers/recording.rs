//! Explicit local skill recording from persisted operations; no automatic execution.
use super::super::GuiHostAdapter;
use crate::gui_server::GuiHostError;
use pawork_domain::{AgentEvent, ContentPart};
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse, CommandSource,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

fn error(message: impl Into<String>) -> GuiHostError {
    GuiHostAdapter::host_error("skill_recording", message)
}
fn authorize(source: &CommandSource, minor: u16) -> Result<(), GuiHostError> {
    if minor < 24 || !matches!(source, CommandSource::LocalGui { .. }) {
        return Err(error("Skill recording requires a local GUI with API 1.24"));
    }
    Ok(())
}
fn safe_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let compact: String = lower
        .chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '\'' | '"'))
        .collect();
    !text.contains('\0')
        && pawork_storage::session::import::formats::find_secret(text).is_none()
        && ![
            "-----begin private key",
            "-----begin rsa private key",
            "[redacted]",
        ]
        .iter()
        .any(|p| lower.contains(p))
        && ![
            "password",
            "api_key",
            "apikey",
            "api-key",
            "access_token",
            "refresh_token",
            "authorization",
            "cookie",
            "secret",
        ]
        .iter()
        .any(|key| {
            [':', '=']
                .iter()
                .any(|separator| compact.contains(&format!("{key}{separator}")))
        })
}
fn safe_arguments(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().all(|(key, value)| {
            let key = key.to_ascii_lowercase().replace(['-', '_'], "");
            ![
                "password",
                "apikey",
                "authorization",
                "accesstoken",
                "refreshtoken",
                "secret",
                "cookie",
            ]
            .iter()
            .any(|p| key.contains(p))
                && safe_arguments(value)
        }),
        Value::Array(values) => values.iter().all(safe_arguments),
        Value::String(text) => safe_text(text),
        _ => true,
    }
}

pub(crate) async fn preview(
    adapter: &GuiHostAdapter,
    envelope: &AppQueryEnvelope,
) -> Result<AppResponse, GuiHostError> {
    authorize(&envelope.source, envelope.api_version.minor)?;
    let AppQuery::SkillRecordPreview {
        session_id,
        event_ids,
    } = &envelope.query
    else {
        unreachable!()
    };
    if event_ids.len() > 64 {
        return Err(error("Select at most 64 operations"));
    }
    let core = adapter.core.read().await;
    core.get_session(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let next = core
        .next_sequence(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let start = next.saturating_sub(4096).max(1);
    let events = core
        .store()
        .map_err(GuiHostAdapter::app_error)?
        .replay_events(session_id, start, 4096)
        .await
        .map_err(GuiHostAdapter::session_error)?;
    let selected: HashSet<_> = event_ids.iter().map(|id| id.as_str()).collect();
    if selected.len() != event_ids.len() {
        return Err(error("Duplicate operation selection"));
    }
    let mut calls = HashMap::new();
    let mut operations = Vec::new();
    let mut recorded = Vec::new();
    for event in &events {
        match &event.payload {
            AgentEvent::MessageCommitted { message } => {
                for part in &message.content {
                    if let ContentPart::ToolCall(call) = part {
                        if call.complete {
                            calls.insert(call.id.clone(), call);
                        }
                    }
                }
            }
            AgentEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
            } => {
                let Some(call) = calls.get(tool_call_id) else {
                    continue;
                };
                let safe = safe_text(&call.name) && safe_arguments(&call.arguments);
                operations.push(json!({"event_id": event.event_id, "name": call.name, "is_error": result.is_error, "recordable": safe}));
                if selected.contains(event.event_id.as_str()) {
                    if !safe {
                        return Err(error(
                            "Selected operation contains credential material; remove it from the recording",
                        ));
                    }
                    let args = serde_json::to_string_pretty(&call.arguments)
                        .map_err(|_| error("Cannot format operation"))?;
                    let indented = args
                        .lines()
                        .map(|l| format!("    {l}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    recorded.push(format!(
                        "{}. Tool {} — {}\n\n{}\n",
                        recorded.len() + 1,
                        json!(call.name),
                        if result.is_error {
                            "failed; revise before reuse"
                        } else {
                            "completed"
                        },
                        indented
                    ));
                }
            }
            _ => {}
        }
    }
    if recorded.len() != selected.len() {
        return Err(error(
            "Selected operations are unavailable; refresh the recent operation list",
        ));
    }
    let content = if recorded.is_empty() {
        String::new()
    } else {
        format!(
            "# Recorded workflow\n\n## When to use\nDescribe when this workflow applies. Review paths and parameters before saving.\n\n## Steps\n\n{}\nRetain normal tool approvals and verify each result before continuing.\n",
            recorded.join("\n")
        )
    };
    if content.len() > 64 * 1024 {
        return Err(error("Recording exceeds 64 KiB; select fewer operations"));
    }
    // Last 128 candidates keep the panel bounded; selected IDs were checked above.
    let omitted = operations.len().saturating_sub(128);
    operations.drain(..omitted);
    Ok(AppResponse::Data(
        json!({"operations":operations,"content":content,"truncated": start > 1 || omitted > 0}),
    ))
}

fn save_files(
    roots: &[PathBuf],
    name: &str,
    description: &str,
    content: &str,
) -> Result<String, GuiHostError> {
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_lowercase()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(error(
            "Use a skill name beginning with a lowercase letter, containing only lowercase letters, digits and hyphens (≤64 characters)",
        ));
    }
    if description.trim().is_empty()
        || description.len() > 512
        || content.trim().is_empty()
        || content.len() > 64 * 1024
        || !safe_text(content)
        || !safe_text(description)
    {
        return Err(error(
            "Provide a description (≤512 bytes) and skill text (≤64 KiB) without credential material",
        ));
    }
    pawork_workspace::resources::write_skill_recording(roots, name, description, content).map_err(
        |e| {
            error(if e.kind() == std::io::ErrorKind::AlreadyExists {
                "A skill with this name already exists"
            } else {
                "Cannot write skill; inspect project permissions and symbolic links"
            })
        },
    )
}

pub(crate) async fn save(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    authorize(&envelope.source, envelope.api_version.minor)?;
    let AppCommand::SkillRecordSave {
        workspace_id,
        name,
        description,
        content,
    } = command
    else {
        unreachable!()
    };
    let roots = adapter
        .core
        .read()
        .await
        .workspace_by_id(workspace_id)
        .map_err(GuiHostAdapter::app_error)?
        .roots;
    let (name, description, content) = (name.clone(), description.clone(), content.clone());
    let path =
        tokio::task::spawn_blocking(move || save_files(&roots, &name, &description, &content))
            .await
            .map_err(|_| error("Skill write task failed"))??;
    Ok(AppResponse::Data(json!({"path":path})))
}

pub(crate) async fn plugins(_: &GuiHostAdapter, _: &AppQuery) -> Result<AppResponse, GuiHostError> {
    // This build has no plugin installer/runtime or installed plugin records.
    Ok(AppResponse::Data(
        json!({"plugins":[],"runtime_available":false}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_host::tests::{command_envelope, query_envelope};
    use crate::gui_server::GuiHost;
    use pawork_domain::{
        Message, MessageId, MessageMetadata, MessageRole, RunId, ToolCallContent, ToolResultContent,
    };
    use pawork_workspace::resources::{
        CurrentPathKind, ResourceLoader, ResourceLoaderOptions, ResourceRequest, ResourceSelection,
        WorkspaceRelativePath,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn recorded_operations_save_and_load_as_real_skill() {
        let (mut core, _store_dir) = crate::testsupport::mock_core(vec![]).await;
        let workspace = tempfile::tempdir().unwrap();
        let record = core.register_workspace(workspace.path()).await.unwrap();
        let session = core.create_session("Record").await.unwrap();
        let call = ToolCallContent {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: json!({"path":"README.md"}),
            raw_arguments: None,
            complete: true,
        };
        let mut sequence = core.next_sequence(&session).await.unwrap();
        let run = RunId::from("run-record");
        core.append_payload(
            &session,
            &run,
            &mut sequence,
            AgentEvent::MessageCommitted {
                message: Message {
                    id: MessageId::from("message-call"),
                    role: MessageRole::Assistant,
                    content: vec![ContentPart::ToolCall(call.clone())],
                    metadata: MessageMetadata::default(),
                },
            },
        )
        .await
        .unwrap();
        core.append_payload(
            &session,
            &run,
            &mut sequence,
            AgentEvent::ToolCallStarted {
                tool_call_id: call.id.clone(),
                name: call.name.clone(),
            },
        )
        .await
        .unwrap();
        core.append_payload(
            &session,
            &run,
            &mut sequence,
            AgentEvent::ToolExecutionCompleted {
                tool_call_id: call.id.clone(),
                result: ToolResultContent {
                    tool_call_id: call.id,
                    tool_name: Some(call.name),
                    content: Vec::new(),
                    is_error: false,
                    metadata: Value::Null,
                    artifacts: Vec::new(),
                },
            },
        )
        .await
        .unwrap();
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let local = CommandSource::LocalGui {
            client_id: "record-gui".into(),
        };
        let mut query = query_envelope(AppQuery::SkillRecordPreview {
            session_id: session.clone(),
            event_ids: Vec::new(),
        });
        query.source = local.clone();
        let AppResponse::Data(candidates) = adapter.query(&query).await.unwrap() else {
            panic!("data")
        };
        assert_eq!(candidates["operations"].as_array().unwrap().len(), 1);
        let event = candidates["operations"][0]["event_id"].as_str().unwrap();
        query.query = AppQuery::SkillRecordPreview {
            session_id: session,
            event_ids: vec![event.into()],
        };
        let AppResponse::Data(preview) = adapter.query(&query).await.unwrap() else {
            panic!("preview")
        };
        let content = preview["content"].as_str().unwrap();
        assert!(content.contains("read_file") && content.contains("README.md"));
        let mut command = command_envelope(AppCommand::SkillRecordSave {
            workspace_id: record.workspace_id.clone(),
            name: "read-project".into(),
            description: "Read project files".into(),
            content: content.into(),
        });
        command.source = local;
        adapter.command(&command).await.unwrap();
        let workspaces = pawork_workspace::WorkspaceService::new();
        workspaces
            .add(
                record.workspace_id.clone(),
                "record",
                [workspace.path().to_path_buf()],
            )
            .unwrap();
        let loader = ResourceLoader::new(workspaces, ResourceLoaderOptions::default());
        let bundle = loader
            .load(&ResourceRequest {
                workspace_id: record.workspace_id,
                root_index: 0,
                current_path: WorkspaceRelativePath::default(),
                current_path_kind: CurrentPathKind::Directory,
                selection: ResourceSelection {
                    active_skills: ["read-project".to_string()].into_iter().collect(),
                    ..Default::default()
                },
            })
            .unwrap();
        assert_eq!(bundle.skills.skills.len(), 1);
        assert_eq!(bundle.skills.skills[0].skill_markdown, content);
        assert!(save_files(
            &[workspace.path().to_path_buf()],
            "read-project",
            "duplicate",
            "replacement"
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(
                workspace
                    .path()
                    .join(".pawork/skills/read-project/SKILL.md")
            )
            .unwrap(),
            content
        );
    }

    #[test]
    fn recording_rejects_secrets_traversal_and_symbolic_directories() {
        let root = tempfile::tempdir().unwrap();
        let roots = [root.path().to_path_buf()];
        assert!(save_files(&roots, "../outside", "test", "body").is_err());
        assert!(save_files(
            &roots,
            "secret",
            "test",
            "Bearer abcdefghijklmnopqrstuvwxyz123456"
        )
        .is_err());
        assert!(!safe_arguments(&json!({"nested":{"api_key":"hidden"}})));
        for text in [
            r#"{"api_key": "hidden"}"#,
            "password = hidden",
            "Authorization: hidden",
        ] {
            assert!(save_files(&roots, "secret", "test", text).is_err());
        }
        assert!(authorize(&CommandSource::Automation, 24).is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), root.path().join(".pawork")).unwrap();
            assert!(save_files(&roots, "safe", "test", "body").is_err());
            assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        }
    }
}
