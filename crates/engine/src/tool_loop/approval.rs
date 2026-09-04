//! 审批闸门：经 [`LoopContext::request_approval`] await 决策，engine 补发
//! `ToolApprovalResponded`。`ToolApprovalRequested` 由实现方在阻塞等待前发出。

use std::collections::BTreeMap;

use pawork_domain::{AgentEvent, ApprovalDecision, CancellationToken, ToolCallId};

use crate::event::{EngineError, EventEmitter, LoopEventEmitter};

use super::{ApprovalGate, LoopContext, PendingToolInvocation};

pub(super) struct ApprovalPlan {
    pub to_run: Vec<PendingToolInvocation>,
    pub decided: BTreeMap<ToolCallId, ApprovalDecision>,
}

pub(super) enum ApprovalWait {
    Cancelled,
    Ready(ApprovalPlan),
}

/// 等待宿主审批并应用闸门。取消发生在 `request_approval` 返回之后、
/// 补发 Responded 之前时，返回 [`ApprovalWait::Cancelled`]（有 Requested 无
/// Responded 是合法事件序）。gate 数与调用数不匹配 → fail-closed：全部 Denied、
/// 不执行，并由 engine 补发每个调用的 `ToolApprovalRequested`。
pub(super) async fn wait_and_apply(
    loop_ctx: &dyn LoopContext,
    invocations: &[PendingToolInvocation],
    run_approved: &mut bool,
    events: LoopEventEmitter<'_>,
    emitter: &EventEmitter<'_>,
    cancel: CancellationToken,
) -> Result<ApprovalWait, EngineError> {
    let gates = loop_ctx
        .request_approval(invocations, *run_approved, events, cancel.clone())
        .await?;
    if cancel.is_cancelled() {
        return Ok(ApprovalWait::Cancelled);
    }
    let mismatch = gates.len() != invocations.len();
    let (to_run, decided) = if mismatch {
        let mut decided = BTreeMap::new();
        for invocation in invocations {
            decided.insert(invocation.tool_call_id.clone(), ApprovalDecision::Denied);
        }
        (Vec::new(), decided)
    } else {
        apply_approval_gates(invocations, &gates, run_approved)
    };
    for invocation in invocations {
        if let Some(decision) = decided.get(&invocation.tool_call_id) {
            if mismatch {
                emitter
                    .emit(AgentEvent::ToolApprovalRequested {
                        tool_call_id: invocation.tool_call_id.clone(),
                        reason: format!("tool `{}` requires approval", invocation.name),
                    })
                    .await?;
            }
            emitter
                .emit(AgentEvent::ToolApprovalResponded {
                    tool_call_id: invocation.tool_call_id.clone(),
                    decision: decision.clone(),
                    comment: None,
                })
                .await?;
        }
    }
    Ok(ApprovalWait::Ready(ApprovalPlan { to_run, decided }))
}

fn apply_approval_gates(
    invocations: &[PendingToolInvocation],
    gates: &[ApprovalGate],
    run_approved: &mut bool,
) -> (
    Vec<PendingToolInvocation>,
    BTreeMap<ToolCallId, ApprovalDecision>,
) {
    let mut to_run = Vec::new();
    let mut decided = BTreeMap::new();
    for (index, invocation) in invocations.iter().enumerate() {
        let gate = gates
            .get(index)
            .cloned()
            .unwrap_or(ApprovalGate::NotRequired);
        match gate {
            ApprovalGate::NotRequired => to_run.push(invocation.clone()),
            ApprovalGate::Asked(decision) => {
                let decision = if *run_approved
                    && !matches!(
                        decision,
                        ApprovalDecision::Denied | ApprovalDecision::Cancelled
                    ) {
                    ApprovalDecision::ApprovedForRun
                } else {
                    decision.clone()
                };
                if matches!(decision, ApprovalDecision::ApprovedForRun) {
                    *run_approved = true;
                }
                decided.insert(invocation.tool_call_id.clone(), decision.clone());
                if matches!(
                    decision,
                    ApprovalDecision::ApprovedOnce | ApprovalDecision::ApprovedForRun
                ) {
                    to_run.push(invocation.clone());
                }
            }
        }
    }
    (to_run, decided)
}
