use crate::gui_server::GuiHostError;
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppResponse};
use serde_json::json;

use super::super::GuiHostAdapter;

pub(crate) async fn workspace_add(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::WorkspaceAdd { root_path } = command else {
        unreachable!("workspace_add handler receives WorkspaceAdd")
    };
    let mut core = adapter.core.write().await;
    let record = core
        .register_workspace(std::path::Path::new(root_path))
        .await
        .map_err(GuiHostAdapter::app_error)?;
    Ok(AppResponse::Data(json!({
        "id": record.workspace_id.as_str(),
        "name": record.name,
    })))
}

pub(crate) async fn run_cancel(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::RunCancel { run_id } = command else {
        unreachable!("run_cancel handler receives RunCancel")
    };
    if adapter.runs.cancel(run_id) {
        Ok(AppResponse::Accepted {
            command_id: envelope.command_id.clone(),
            run_id: None,
        })
    } else {
        Err(GuiHostAdapter::host_error(
            "not_found",
            format!("run {} is not active", run_id.as_str()),
        ))
    }
}
