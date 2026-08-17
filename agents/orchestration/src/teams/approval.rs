//! Plan 审批 gate（pure）：复用 P16-1 plan 与 P16-2 review 状态机。
//!
//! team 在此仅做「提交 → 评审 → approve/reject/comment」的协作语义投影；
//! review 状态机的折叠直接复用 [`pawork_workflow::plan::state::PlanState`] 与
//! [`pawork_domain::PlanEvent`]，不重复实现 P16-2。执行 gate
//! [`is_approved_for_execution`] 是「未批准 plan 不执行」的唯一判定点。

use pawork_domain::{
    CheckpointId, PlanCommentAnchor, PlanEvent, PlanId, PlanReviewStatus, PlanStepSnapshot,
    PlanVersionId,
};
use pawork_workflow::plan::PlanState;

use crate::teams::error::TeamError;

/// 提交 plan：在 fresh [`PlanState`] 上 apply `Created` + `ReviewRequested`，
/// 使其直接进入评审（提交即请求审批）。
pub fn build_submitted_state(
    plan_id: PlanId,
    version: PlanVersionId,
    title: String,
    steps: Vec<PlanStepSnapshot>,
) -> PlanState {
    let mut state = PlanState::default();
    pawork_workflow::plan::apply(
        &mut state,
        &PlanEvent::Created {
            plan_id: plan_id.clone(),
            version: version.clone(),
            title,
            steps,
        },
    );
    pawork_workflow::plan::apply(&mut state, &PlanEvent::ReviewRequested { plan_id, version });
    state
}

/// 当前评审状态；plan 未提交时返回 `None`。
pub fn review_status(state: &PlanState) -> Option<PlanReviewStatus> {
    if state.plan_id().is_some() {
        Some(state.review_status())
    } else {
        None
    }
}

/// 校验 approve：版本匹配、当前可审批（InReview / ChangesRequested）。
pub fn validate_approve(
    state: &PlanState,
    plan_id: &PlanId,
    version: &PlanVersionId,
) -> Result<(), TeamError> {
    check_version(state, plan_id, version)?;
    let from = state.review_status();
    if !matches!(
        from,
        PlanReviewStatus::InReview | PlanReviewStatus::ChangesRequested
    ) {
        return Err(TeamError::PlanNotApproved {
            plan_id: plan_id.clone(),
            version: version.clone(),
        });
    }
    Ok(())
}

/// 校验 reject：版本匹配、当前可审批。`reason` 非空由调用方先校验。
pub fn validate_reject(
    state: &PlanState,
    plan_id: &PlanId,
    version: &PlanVersionId,
) -> Result<(), TeamError> {
    check_version(state, plan_id, version)?;
    let from = state.review_status();
    if !matches!(
        from,
        PlanReviewStatus::InReview | PlanReviewStatus::ChangesRequested
    ) {
        return Err(TeamError::PlanNotApproved {
            plan_id: plan_id.clone(),
            version: version.clone(),
        });
    }
    Ok(())
}

/// 校验 comment：版本匹配（任意评审状态均可追加意见）。
pub fn validate_comment(
    state: &PlanState,
    plan_id: &PlanId,
    version: &PlanVersionId,
    anchor: &PlanCommentAnchor,
) -> Result<(), TeamError> {
    check_version(state, plan_id, version)?;
    if !state.steps().iter().any(|s| s.step_id == anchor.step_id) {
        return Err(TeamError::PlanStepNotFound(anchor.step_id.clone()));
    }
    Ok(())
}

/// 执行 gate：仅当 plan/version 匹配且 review 状态为 `Approved` 才放行。
pub fn is_approved_for_execution(
    state: &PlanState,
    plan_id: &PlanId,
    version: &PlanVersionId,
) -> bool {
    state.plan_id().is_some_and(|p| p == plan_id)
        && state.current_version().is_some_and(|v| v == version)
        && state.review_status() == PlanReviewStatus::Approved
}

/// 把 checkpoint 归一为 `Option<CheckpointId>`，供事件构造复用。
pub fn approved_checkpoint(checkpoint_id: Option<CheckpointId>) -> Option<CheckpointId> {
    checkpoint_id
}

fn check_version(
    state: &PlanState,
    plan_id: &PlanId,
    version: &PlanVersionId,
) -> Result<(), TeamError> {
    if state.plan_id().is_none() {
        return Err(TeamError::PlanNotSubmitted(plan_id.clone()));
    }
    if state.plan_id() != Some(plan_id) || state.current_version() != Some(version) {
        return Err(TeamError::PlanVersionMismatch {
            expected: version.clone(),
            actual: state.current_version().cloned().unwrap_or_default(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::PlanStepId;

    fn submitted() -> PlanState {
        build_submitted_state(
            PlanId::from("p1"),
            PlanVersionId::from("v1"),
            "Plan".into(),
            vec![PlanStepSnapshot {
                step_id: PlanStepId::from("s1"),
                text: "step".into(),
                status: pawork_domain::PlanStepStatus::Pending,
            }],
        )
    }

    #[test]
    fn submitted_plan_is_in_review_and_blocks_execution() {
        let s = submitted();
        assert_eq!(review_status(&s), Some(PlanReviewStatus::InReview));
        assert!(!is_approved_for_execution(
            &s,
            &PlanId::from("p1"),
            &PlanVersionId::from("v1")
        ));
    }

    #[test]
    fn approve_unblocks_execution_gate() {
        let mut s = submitted();
        pawork_workflow::plan::apply(
            &mut s,
            &PlanEvent::Approved {
                plan_id: PlanId::from("p1"),
                version: PlanVersionId::from("v1"),
                checkpoint_id: None,
            },
        );
        assert!(is_approved_for_execution(
            &s,
            &PlanId::from("p1"),
            &PlanVersionId::from("v1")
        ));
    }
}
