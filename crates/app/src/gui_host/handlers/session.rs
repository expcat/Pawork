use std::sync::atomic::Ordering;

use crate::gui_server::GuiHostError;
use pawork_domain::{SessionId, WorkspaceId};
use pawork_engine::now_timestamp;
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppResponse};
use serde_json::{json, Value};

use super::super::GuiHostAdapter;

fn session_view_json(
    session_id: &SessionId,
    workspace_id: &WorkspaceId,
    title: &str,
    active_branch: Option<&str>,
) -> Value {
    let mut data = json!({
        "session_id": session_id.as_str(),
        "workspace_id": workspace_id.as_str(),
        "title": title,
        "revision": 0,
        "open": true,
    });
    if let Some(branch) = active_branch {
        data["active_branch"] = json!(branch);
    }
    data
}

pub(crate) async fn session_create(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SessionCreate {
        title,
        workspace_id,
    } = command
    else {
        unreachable!("session_create handler receives SessionCreate")
    };
    let title = title.clone().unwrap_or_else(|| "New session".into());
    let session_id = adapter
        .core
        .read()
        .await
        .create_session_with_workspace(title.clone(), workspace_id.clone())
        .await
        .map_err(GuiHostAdapter::app_error)?;
    Ok(AppResponse::Data(session_view_json(
        &session_id,
        workspace_id,
        &title,
        None,
    )))
}

pub(crate) async fn session_open(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SessionOpen { session_id } = command else {
        unreachable!("session_open handler receives SessionOpen")
    };
    let core = adapter.core.read().await;
    let record = core
        .get_session(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let workspace_id = core
        .session_workspace(session_id)
        .unwrap_or_else(|| core.workspace_id().clone());
    Ok(AppResponse::Data(session_view_json(
        session_id,
        &workspace_id,
        &record.title,
        Some(&record.active_branch),
    )))
}

pub(crate) async fn session_fork(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SessionFork {
        session_id,
        parent_event_id,
    } = command
    else {
        unreachable!("session_fork handler receives SessionFork")
    };
    let core = adapter.core.read().await;
    let record = core
        .get_session(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let store = core.store().map_err(GuiHostAdapter::app_error)?;
    let n = adapter.next_fork.fetch_add(1, Ordering::Relaxed);
    let branch_id = format!("fork-{}-{n}", now_timestamp().as_unix_millis());
    store
        .fork_from_event(session_id, &branch_id, parent_event_id)
        .await
        .map_err(GuiHostAdapter::session_error)?;
    store
        .switch_branch(session_id, &branch_id)
        .await
        .map_err(GuiHostAdapter::session_error)?;
    let workspace_id = core
        .session_workspace(session_id)
        .unwrap_or_else(|| core.workspace_id().clone());
    let mut data = session_view_json(
        session_id,
        &workspace_id,
        &record.title,
        Some(&branch_id),
    );
    data["branch_id"] = json!(branch_id);
    data["parent_event_id"] = json!(parent_event_id.as_str());
    Ok(AppResponse::Data(data))
}
