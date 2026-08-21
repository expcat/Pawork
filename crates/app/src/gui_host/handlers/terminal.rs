use std::path::PathBuf;
use std::sync::Arc;

use pawork_domain::WorkspaceId;
use pawork_exec::{OwnerSessionId, PtyCreateSpec, PtyEvent, PtyWindowSize, TerminalId};
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppResponse, AppEvent, WorkspaceRelativePath,
};
use pawork_workspace::resolve_relative_path;
use serde_json::{json, Value};

use crate::gui_server::GuiHostError;

use super::super::GuiHostAdapter;

impl GuiHostAdapter {
    fn terminal_owner(&self, terminal_session_id: &str) -> Result<OwnerSessionId, GuiHostError> {
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(terminal_session_id)
            .cloned()
            .map(OwnerSessionId::new)
            .ok_or_else(|| {
                Self::host_error(
                    "not_found",
                    format!("terminal {terminal_session_id} is not registered"),
                )
            })
    }

    fn remember_terminal(&self, terminal_id: &TerminalId, owner: &OwnerSessionId) {
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(terminal_id.as_str().to_string(), owner.as_str().to_string());
    }

    fn spawn_terminal_forwarder(&self, terminal_id: TerminalId, owner: OwnerSessionId) {
        let Ok(mut receiver) = self.pty.subscribe(&terminal_id, &owner) else {
            return;
        };
        let bus = Arc::clone(&self.bus);
        let instance = self.instance.clone();
        let terminal_session_id = terminal_id.as_str().to_string();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(PtyEvent::Output { data, .. }) => {
                        bus.publish_terminal(
                            instance.clone(),
                            &terminal_session_id,
                            AppEvent::TerminalOutput {
                                terminal_session_id: terminal_session_id.clone(),
                                delta: String::from_utf8_lossy(&data).into_owned(),
                            },
                        );
                    }
                    Ok(PtyEvent::Exit { .. }) => break,
                    Err(_) => break,
                }
            }
        });
    }

    fn resolve_terminal_cwd(
        core: &crate::AppCore,
        workspace_id: &WorkspaceId,
        working_directory: Option<&WorkspaceRelativePath>,
    ) -> Result<Option<PathBuf>, GuiHostError> {
        let roots = if workspace_id.as_str() == core.workspace_id().as_str() {
            core.extensions.workspace_roots.clone()
        } else {
            core.extensions.workspaces
                .get(workspace_id)
                .map_err(|error| Self::host_error("app_error", error.to_string()))?
                .map(|workspace| workspace.roots)
                .unwrap_or_default()
        };
        match working_directory {
            None => Ok(roots.first().cloned()),
            Some(relative) => {
                if roots.is_empty() {
                    return Err(Self::host_error(
                        "not_found",
                        format!("workspace {} has no roots", workspace_id.as_str()),
                    ));
                }
                resolve_relative_path(&roots, relative.as_str())
                    .map(|resolved| Some(resolved.absolute))
                    .map_err(|error| Self::host_error("invalid_argument", error.to_string()))
            }
        }
    }

    pub(in crate::gui_host) fn terminal_snapshots(&self) -> Vec<Value> {
        let registered: Vec<(String, String)> = self
            .terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, owner)| (id.clone(), owner.clone()))
            .collect();
        registered
            .into_iter()
            .filter_map(|(id, owner)| {
                let terminal_id = TerminalId::new(id);
                let owner = OwnerSessionId::new(owner);
                let snapshot = self.pty.snapshot(&terminal_id, &owner).ok()?;
                Some(json!({
                    "terminal_session_id": snapshot.terminal_id.as_str(),
                    "owner_session": snapshot.owner_session.as_str(),
                    "state": format!("{:?}", snapshot.state).to_ascii_lowercase(),
                    "columns": snapshot.size.cols,
                    "rows": snapshot.size.rows,
                    "dropped_events": snapshot.dropped_events,
                }))
            })
            .collect()
    }
}

pub(crate) async fn terminal_create(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::TerminalCreate {
        workspace_id,
        working_directory,
    } = command
    else {
        unreachable!("terminal_create handler receives TerminalCreate")
    };
    let core = adapter.core.read().await;
    let cwd =
        GuiHostAdapter::resolve_terminal_cwd(&core, workspace_id, working_directory.as_ref())?;
    drop(core);
    let owner = OwnerSessionId::new(workspace_id.as_str());
    let spec = PtyCreateSpec {
        owner_session: owner.clone(),
        cwd,
        size: PtyWindowSize::default(),
        ..PtyCreateSpec::default()
    };
    let terminal_id = adapter
        .pty
        .create(spec)
        .await
        .map_err(GuiHostAdapter::pty_error)?;
    adapter.remember_terminal(&terminal_id, &owner);
    adapter.spawn_terminal_forwarder(terminal_id.clone(), owner);
    Ok(AppResponse::Data(json!({
        "terminal_session_id": terminal_id.as_str(),
        "uncontrolled": true,
        "note": "本机不受控终端：不经沙箱与审批",
    })))
}

pub(crate) async fn terminal_write(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::TerminalWrite {
        terminal_session_id,
        data,
    } = command
    else {
        unreachable!("terminal_write handler receives TerminalWrite")
    };
    let owner = adapter.terminal_owner(terminal_session_id)?;
    adapter
        .pty
        .write(
            &TerminalId::new(terminal_session_id),
            &owner,
            data.as_bytes().to_vec(),
        )
        .await
        .map_err(GuiHostAdapter::pty_error)?;
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}

pub(crate) async fn terminal_resize(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::TerminalResize {
        terminal_session_id,
        columns,
        rows,
    } = command
    else {
        unreachable!("terminal_resize handler receives TerminalResize")
    };
    let owner = adapter.terminal_owner(terminal_session_id)?;
    adapter
        .pty
        .resize(
            &TerminalId::new(terminal_session_id),
            &owner,
            PtyWindowSize {
                rows: *rows,
                cols: *columns,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .await
        .map_err(GuiHostAdapter::pty_error)?;
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}
