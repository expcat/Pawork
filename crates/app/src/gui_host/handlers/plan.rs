use super::super::GuiHostAdapter;
use crate::gui_server::GuiHostError;
use pawork_domain::SessionId;
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppQuery, AppResponse};

pub(crate) async fn get(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::PlanGet { session_id } = query else {
        unreachable!()
    };
    let core = adapter.core.read().await;
    core.get_session(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let plan = core
        .plan_snapshot(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    Ok(AppResponse::Data(serde_json::to_value(plan).map_err(
        |e| GuiHostAdapter::host_error("internal", e.to_string()),
    )?))
}

pub(crate) async fn change(
    adapter: &GuiHostAdapter,
    _: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let _goal_change = adapter.goal_commands.lock().await;
    let (session_id, expected): (&SessionId, Option<&str>) = match command {
        AppCommand::PlanSave {
            session_id,
            expected_version,
            title,
            steps,
        } => {
            if title.trim().is_empty()
                || title.len() > 256
                || steps.is_empty()
                || steps.len() > 64
                || steps.iter().any(|s| s.trim().is_empty() || s.len() > 2048)
            {
                return Err(GuiHostAdapter::host_error(
                    "invalid_plan",
                    "Plan requires a title (≤256 bytes) and 1–64 nonempty steps (≤2048 bytes each)",
                ));
            }
            (session_id, expected_version.as_deref())
        }
        AppCommand::PlanSubmit {
            session_id,
            expected_version,
        }
        | AppCommand::PlanApprove {
            session_id,
            expected_version,
        } => (session_id, Some(expected_version)),
        AppCommand::PlanReject {
            session_id,
            expected_version,
            reason,
        } => {
            if reason.trim().is_empty() || reason.len() > 2048 {
                return Err(GuiHostAdapter::host_error(
                    "invalid_plan",
                    "Rejection requires a reason (≤2048 bytes)",
                ));
            }
            (session_id, Some(expected_version))
        }
        _ => unreachable!(),
    };
    if adapter
        .goals
        .lock()
        .unwrap()
        .contains_key(session_id.as_str())
    {
        return Err(GuiHostAdapter::host_error(
            "goal_active",
            "Pause the goal before editing its Plan",
        ));
    }
    // Share RunStart's reservation: no Plan event can interleave with run sequence allocation.
    let _slot = super::run_start::SessionSlotGuard::acquire(adapter, session_id)?;
    let core = adapter.core.read().await;
    core.get_session(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let previous = core
        .plan_snapshot(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    if previous.as_ref().map(|p| p.version.as_str()) != expected {
        return Err(GuiHostAdapter::host_error(
            "plan_version_conflict",
            "Plan changed; refresh before editing or approving",
        ));
    }
    let snapshot = match command {
        AppCommand::PlanSave { title, steps, .. } if previous.is_none() => {
            core.plan_create(session_id, title.trim(), steps.clone())
                .await
        }
        AppCommand::PlanSave { title, steps, .. } => {
            core.plan_replace(session_id, title.trim(), steps.clone())
                .await
        }
        AppCommand::PlanSubmit { .. } => core.plan_submit(session_id).await,
        AppCommand::PlanApprove { .. } => core.plan_approve(session_id).await,
        AppCommand::PlanReject { reason, .. } => core.plan_reject(session_id, reason).await,
        _ => unreachable!(),
    }
    .map_err(GuiHostAdapter::app_error)?;
    Ok(AppResponse::Data(serde_json::to_value(snapshot).map_err(
        |e| GuiHostAdapter::host_error("internal", e.to_string()),
    )?))
}
