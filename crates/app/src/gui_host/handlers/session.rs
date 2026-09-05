use std::sync::atomic::Ordering;

use crate::app_core::PLACEHOLDER_SESSION_TITLE;
use crate::gui_server::GuiHostError;
use pawork_domain::{SessionId, WorkspaceId};
use pawork_engine::now_timestamp;
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppEvent, AppResponse};
use serde_json::{json, Value};

use super::super::GuiHostAdapter;

fn session_view_json(
    session_id: &SessionId,
    workspace_id: Option<&WorkspaceId>,
    title: &str,
    active_branch: Option<&str>,
) -> Value {
    let mut data = json!({
        "session_id": session_id.as_str(),
        "title": title,
        "revision": 0,
        "open": true,
    });
    if let Some(workspace_id) = workspace_id {
        data["workspace_id"] = json!(workspace_id.as_str());
    }
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
    let title = title
        .clone()
        .unwrap_or_else(|| PLACEHOLDER_SESSION_TITLE.to_string());
    let core = adapter.core.read().await;
    // ADR-054 D1：缺省 / 显式 null → 无归属会话（落盘 NULL）；
    // 显式传值 → 绑定该 workspace（行为不变）。
    let (session_id, workspace_id) = match workspace_id.as_ref() {
        Some(workspace_id) => {
            let session_id = core
                .create_session_with_workspace(title.clone(), workspace_id.clone())
                .await
                .map_err(GuiHostAdapter::app_error)?;
            (session_id, Some(workspace_id.clone()))
        }
        None => {
            let session_id = core
                .create_session_unbound(title.clone())
                .await
                .map_err(GuiHostAdapter::app_error)?;
            (session_id, None)
        }
    };
    Ok(AppResponse::Data(session_view_json(
        &session_id,
        workspace_id.as_ref(),
        &title,
        None,
    )))
}

/// ADR-054 D5：写盘成功后广播写后状态；回执 Data 为写后 session_view。
fn broadcast_session_meta_changed(
    adapter: &GuiHostAdapter,
    session_id: &SessionId,
    title: &str,
    archived: bool,
) {
    adapter.bus.publish_raw(
        adapter.instance.clone(),
        session_id,
        AppEvent::SessionMetaChanged {
            session_id: session_id.clone(),
            title: title.to_string(),
            archived,
        },
    );
}

pub(crate) async fn session_rename(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SessionRename { session_id, title } = command else {
        unreachable!("session_rename handler receives SessionRename")
    };
    // D2：title trim 后为空是结构化错误，不写盘。
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(GuiHostAdapter::host_error(
            "invalid_title",
            "session title must not be blank",
        ));
    }
    let core = adapter.core.read().await;
    core.rename_session(session_id, &title, now_timestamp().as_unix_millis() as i64)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let record = core
        .get_session(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    broadcast_session_meta_changed(adapter, session_id, &record.title, record.archived);
    let workspace_id = core.session_workspace(session_id);
    let mut data = session_view_json(
        session_id,
        workspace_id.as_ref(),
        &record.title,
        Some(&record.active_branch),
    );
    data["archived"] = json!(record.archived);
    Ok(AppResponse::Data(data))
}

pub(crate) async fn session_archive(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SessionArchive {
        session_id,
        archived,
    } = command
    else {
        unreachable!("session_archive handler receives SessionArchive")
    };
    let core = adapter.core.read().await;
    core.archive_session(
        session_id,
        *archived,
        now_timestamp().as_unix_millis() as i64,
    )
    .await
    .map_err(GuiHostAdapter::app_error)?;
    let record = core
        .get_session(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    broadcast_session_meta_changed(adapter, session_id, &record.title, record.archived);
    let workspace_id = core.session_workspace(session_id);
    let mut data = session_view_json(
        session_id,
        workspace_id.as_ref(),
        &record.title,
        Some(&record.active_branch),
    );
    data["archived"] = json!(record.archived);
    Ok(AppResponse::Data(data))
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
    let workspace_id = core.session_workspace(session_id);
    Ok(AppResponse::Data(session_view_json(
        session_id,
        workspace_id.as_ref(),
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
    let workspace_id = core.session_workspace(session_id);
    let mut data = session_view_json(
        session_id,
        workspace_id.as_ref(),
        &record.title,
        Some(&branch_id),
    );
    data["branch_id"] = json!(branch_id);
    data["parent_event_id"] = json!(parent_event_id.as_str());
    Ok(AppResponse::Data(data))
}
