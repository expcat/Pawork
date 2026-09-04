use std::str::FromStr;

use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppQuery, AppResponse, ApprovalModeWire,
    PermissionsSettingsData,
};
use serde_json::json;

use crate::gui_host::GuiHostAdapter;
use crate::gui_server::GuiHostError;

use super::{policy_approval_mode, settings_data, wire_approval_mode};

/// ADR-048 D1：权限与审批三元组。三字段来源语义分列：前两个是当前
/// 会话内存态（对之后启动的 run 生效），trust_workspaces_global 是
/// Global 层持久值（None → null，只读展示，本片不写）。
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

/// ADR-048 D2：会话内切换审批模式。仅收五个 snake_case 规范值
/// （[`ApprovalModeWire::from_str`]），未知值 Error 且旧值保留（fail-closed）；
/// 只影响之后启动的 run，不持久化。
pub(crate) async fn set_approval_mode(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetApprovalMode { mode } = command else {
        unreachable!("set_approval_mode handler receives SetApprovalMode")
    };
    let parsed = ApprovalModeWire::from_str(mode)
        .map_err(|reason| GuiHostAdapter::host_error("invalid_approval_mode", reason.to_string()))?;
    {
        let mut core = adapter.core.write().await;
        core.set_approval_mode(policy_approval_mode(parsed));
    }
    Ok(settings_data(json!({ "approval_mode": parsed })))
}

/// ADR-048 D3：会话内信任切换。workspace_id 必须匹配当前 attached
/// workspace，不匹配 Error 且保旧（fail-closed）；切换只影响之后启动的
/// run，不写盘。校验与写入同一把写锁内完成，避免 check-then-set 竞态。
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
        core.set_workspace_trusted(*trusted);
    }
    Ok(AppResponse::Data(json!({
        "workspace_trusted": trusted,
    })))
}
