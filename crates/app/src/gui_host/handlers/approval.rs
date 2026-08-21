use crate::gui_server::GuiHostError;
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppResponse};

use super::super::GuiHostAdapter;

fn protocol_to_domain_decision(
    decision: &pawork_protocol::ApprovalDecision,
) -> pawork_domain::ApprovalDecision {
    match decision {
        pawork_protocol::ApprovalDecision::ApproveOnce => pawork_domain::ApprovalDecision::ApprovedOnce,
        pawork_protocol::ApprovalDecision::ApproveForRun => {
            pawork_domain::ApprovalDecision::ApprovedForRun
        }
        pawork_protocol::ApprovalDecision::Deny => pawork_domain::ApprovalDecision::Denied,
        pawork_protocol::ApprovalDecision::Cancel => pawork_domain::ApprovalDecision::Cancelled,
    }
}

pub(crate) async fn tool_approve(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::ToolApprove {
        run_id,
        tool_call_id,
        decision,
    } = command
    else {
        unreachable!("tool_approve handler receives ToolApprove")
    };
    adapter
        .approvals
        .resolve(run_id, tool_call_id, protocol_to_domain_decision(decision))
        .map_err(|message| GuiHostAdapter::host_error("conflict", message))?;
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}
