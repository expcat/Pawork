use std::str::FromStr;

use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppQuery, AppResponse, ApprovalModeWire,
    PermissionsSettingsData,
};
use serde_json::json;

use crate::gui_host::GuiHostAdapter;
use crate::gui_server::GuiHostError;

use super::{policy_approval_mode, settings_data, wire_approval_mode};

/// ADR-053：当前生效审批与项目信任；Global 全项目信任默认只读。
pub(crate) async fn permissions_settings(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::PermissionsSettings = query else {
        unreachable!("permissions_settings handler receives PermissionsSettings")
    };
    let core = adapter.core.read().await;
    Ok(settings_data(PermissionsSettingsData {
        approval_mode: wire_approval_mode(core.approval_mode()),
        workspace_trusted: core.workspace_trusted(),
        trust_workspaces_global: core.config().trust_workspaces,
        // ADR-048 D1（实现期修订）：透出 Host 权威 attached workspace_id，
        // Desktop 发 workspace_trust 时原样回填，校验方与发送方同源。
        workspace_id: core.workspace_id().to_string(),
    }))
}

/// ADR-053：Global 默认先落盘成功，再替换后续 Run 的审批快照。
pub(crate) async fn set_approval_mode(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetApprovalMode { mode } = command else {
        unreachable!("set_approval_mode handler receives SetApprovalMode")
    };
    let parsed = ApprovalModeWire::from_str(mode).map_err(|reason| {
        GuiHostAdapter::host_error("invalid_approval_mode", reason.to_string())
    })?;
    {
        let mut core = adapter.core.write().await;
        let path = global_config_path()?;
        pawork_workspace::config::write_approval_mode(&path, policy_approval_mode(parsed))
            .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
        core.set_approval_mode(policy_approval_mode(parsed));
    }
    Ok(settings_data(json!({ "approval_mode": parsed })))
}

/// ADR-053：仅当前 attached workspace，校验、写盘与运行态更新共用写锁。
pub(crate) async fn workspace_trust(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::WorkspaceTrust {
        workspace_id,
        trusted,
    } = command
    else {
        unreachable!("workspace_trust handler receives WorkspaceTrust")
    };
    {
        let mut core = adapter.core.write().await;
        if core.workspace_id().as_str() != workspace_id.as_str() {
            return Err(GuiHostAdapter::host_error(
                "unknown_workspace",
                format!(
                    "workspace {} is not the attached workspace {}",
                    workspace_id.as_str(),
                    core.workspace_id().as_str()
                ),
            ));
        }
        let workspace = core
            .workspace_by_id(workspace_id)
            .map_err(GuiHostAdapter::app_error)?;
        let root = workspace
            .roots
            .first()
            .and_then(|root| root.to_str())
            .ok_or_else(|| {
                GuiHostAdapter::host_error("unknown_workspace", "workspace has no persistable root")
            })?
            .to_owned();
        pawork_workspace::config::write_workspace_trust(&global_config_path()?, &root, *trusted)
            .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
        core.set_workspace_trusted(root, *trusted);
    }
    Ok(AppResponse::Data(json!({
        "workspace_trusted": trusted,
    })))
}

fn global_config_path() -> Result<std::path::PathBuf, GuiHostError> {
    pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "global config directory is unavailable",
        )
    })
}
