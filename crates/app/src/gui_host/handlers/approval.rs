use crate::gui_server::GuiHostError;
use crate::ApprovalResolve;
use pawork_engine::AgentEventSink;
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppResponse};

use super::super::{GuiBroadcastSink, GuiHostAdapter};

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
    let domain = protocol_to_domain_decision(decision);
    match adapter
        .approvals
        .resolve(run_id, tool_call_id, domain.clone())
        .map_err(|message| GuiHostAdapter::host_error("conflict", message))?
    {
        ApprovalResolve::Live => {}
        ApprovalResolve::Queued => {
            // live run: keep queued race semantics, never durable-seal a waiting live call.
            if !adapter.runs().contains(run_id) {
                let core = adapter.core.read().await;
                let store = core.store().map_err(GuiHostAdapter::app_error)?;
                if let Some(waiting) = store
                    .waiting_tool_call(tool_call_id.as_str())
                    .await
                    .map_err(GuiHostAdapter::session_error)?
                {
                    if waiting.tool_call.run_id.as_str() != run_id.as_str() {
                        return Err(GuiHostAdapter::host_error(
                            "conflict",
                            format!(
                                "approval {} belongs to a different run",
                                tool_call_id.as_str()
                            ),
                        ));
                    }
                    let mut sequence = core
                        .next_sequence(&waiting.session_id)
                        .await
                        .map_err(GuiHostAdapter::app_error)?;
                    let envelopes = core
                        .resolve_waiting_tool_call(
                            &waiting.session_id,
                            &waiting.tool_call,
                            domain,
                            "approval resolved after restart; tool not executed",
                            &mut sequence,
                        )
                        .await
                        .map_err(GuiHostAdapter::app_error)?;
                    // persist-first 已落库；复用 live 路径的广播 sink 补实时事件。
                    // broadcast_event 过滤后仅 ToolCompleted 上 wire，契约不变。
                    let sink =
                        GuiBroadcastSink::new(adapter.bus.clone(), adapter.instance.clone());
                    for envelope in envelopes {
                        if let Err(error) = sink.emit(envelope).await {
                            tracing::warn!(error = %error, "queued approval closure broadcast failed");
                        }
                    }
                }
            }
        }
    }
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}
