//! Plan 聚合状态与 event-sourcing 折叠（`apply` / `replay`）。
//!
//! [`apply`] 是恢复入口：把一个 canonical [`PlanEvent`] 纯函数式折叠进
//! [`PlanState`]。事件被视为已校验的「事实」，`apply` 不再重复状态机校验
//! （校验由命令面 [`crate::PlanService`] 完成）；崩溃后重放事件序列即可重建
//! 当前 Plan 与进度。
//!
//! 评审 / 审批变体（`ReviewRequested` / `Revised` / `Approved` / `Rejected` /
//! `CommentAdded`）在此仅做最小折叠以保持重放一致性，完整命令面语义由 P16-2 补齐。

use agent_domain::{
    PlanCommentAnchor, PlanEvent, PlanId, PlanReviewStatus, PlanStepId, PlanStepSnapshot,
    PlanStepStatus, PlanVersionId,
};

use crate::snapshot::{PlanSnapshot, PlanVersionInfo};

/// 一条评审意见（P16-2 在命令面正式启用；P16-1 仅在 `apply` 中保留以支持重放）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanComment {
    pub anchor: PlanCommentAnchor,
    pub body: String,
}

/// Plan 聚合的当前状态（`apply` 折叠结果）。
///
/// 持有当前版本的 title / steps / version / parent 指针、全部版本历史
/// （修订链）、评审状态（P16-1 默认 [`PlanReviewStatus::Draft`]）与当前版本下
/// 的评审意见。本结构不执行任何 IO，可被自由克隆 / 比较。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanState {
    plan_id: Option<PlanId>,
    title: Option<String>,
    steps: Vec<PlanStepSnapshot>,
    current_version: Option<PlanVersionId>,
    parent_version: Option<PlanVersionId>,
    history: Vec<PlanVersionInfo>,
    review_status: PlanReviewStatus,
    comments: Vec<PlanComment>,
}

impl PlanState {
    pub fn plan_id(&self) -> Option<&PlanId> {
        self.plan_id.as_ref()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn current_version(&self) -> Option<&PlanVersionId> {
        self.current_version.as_ref()
    }

    pub fn parent_version(&self) -> Option<&PlanVersionId> {
        self.parent_version.as_ref()
    }

    pub fn steps(&self) -> &[PlanStepSnapshot] {
        &self.steps
    }

    pub fn review_status(&self) -> PlanReviewStatus {
        self.review_status
    }

    pub fn history(&self) -> &[PlanVersionInfo] {
        &self.history
    }

    /// 构造查询面快照；尚未 `Created` 时返回 `None`。
    pub fn snapshot(&self) -> Option<PlanSnapshot> {
        let plan_id = self.plan_id.clone()?;
        let version = self.current_version.clone()?;
        let title = self.title.clone().unwrap_or_default();
        Some(PlanSnapshot {
            plan_id,
            version,
            title,
            steps: self.steps.clone(),
            review_status: self.review_status,
        })
    }

    pub(crate) fn step_mut(&mut self, id: &PlanStepId) -> Option<&mut PlanStepSnapshot> {
        self.steps.iter_mut().find(|s| s.step_id == *id)
    }
}

/// 步骤状态机合法转移判定（命令面校验用）。
///
/// 合法：`Pending→InProgress`、`InProgress→Completed|Blocked`、`Blocked→InProgress`。
/// 其余（含同态自环、终态跳出、回退到 `Pending`）均非法。
pub fn is_legal_step_transition(from: PlanStepStatus, to: PlanStepStatus) -> bool {
    matches!(
        (from, to),
        (PlanStepStatus::Pending, PlanStepStatus::InProgress)
            | (PlanStepStatus::InProgress, PlanStepStatus::Completed)
            | (PlanStepStatus::InProgress, PlanStepStatus::Blocked)
            | (PlanStepStatus::Blocked, PlanStepStatus::InProgress)
    )
}

/// 把一个 canonical [`PlanEvent`] 纯函数式折叠进 [`PlanState`]（恢复入口）。
pub fn apply(state: &mut PlanState, event: &PlanEvent) {
    match event {
        PlanEvent::Created {
            plan_id,
            version,
            title,
            steps,
        } => {
            state.plan_id = Some(plan_id.clone());
            state.title = Some(title.clone());
            state.steps = steps.clone();
            state.current_version = Some(version.clone());
            state.parent_version = None;
            state.history.clear();
            state.history.push(PlanVersionInfo {
                version: version.clone(),
                parent_version: None,
                title: title.clone(),
                steps: steps.clone(),
            });
            state.review_status = PlanReviewStatus::Draft;
            state.comments.clear();
        }
        PlanEvent::StepUpdated {
            step_id, status, ..
        } => {
            if let Some(step) = state.step_mut(step_id) {
                step.status = *status;
            }
            // 步骤不存在时静默忽略：事件是已校验事实，此处仅防御性折叠。
        }
        PlanEvent::Replaced {
            version,
            parent_version,
            title,
            steps,
            ..
        } => {
            // plan_id 不变（同一 Plan 的新版本）。
            state.title = Some(title.clone());
            state.steps = steps.clone();
            state.current_version = Some(version.clone());
            state.parent_version = Some(parent_version.clone());
            state.history.push(PlanVersionInfo {
                version: version.clone(),
                parent_version: Some(parent_version.clone()),
                title: title.clone(),
                steps: steps.clone(),
            });
            state.review_status = PlanReviewStatus::Draft;
            state.comments.clear();
        }
        // —— P16-2 评审/审批变体的最小折叠（保持重放一致性；命令面语义待 P16-2）——
        PlanEvent::ReviewRequested { version, .. } => {
            if state.current_version.as_ref() == Some(version) {
                state.review_status = PlanReviewStatus::InReview;
            }
        }
        PlanEvent::Revised {
            version,
            parent_version,
            ..
        } => {
            state.current_version = Some(version.clone());
            state.parent_version = Some(parent_version.clone());
            state.review_status = PlanReviewStatus::Draft;
            if !state.history.iter().any(|h| &h.version == version) {
                let title = state.title.clone().unwrap_or_default();
                let steps = state.steps.clone();
                state.history.push(PlanVersionInfo {
                    version: version.clone(),
                    parent_version: Some(parent_version.clone()),
                    title,
                    steps,
                });
            }
        }
        PlanEvent::Approved { version, .. } => {
            if state.current_version.as_ref() == Some(version) {
                state.review_status = PlanReviewStatus::Approved;
            }
        }
        PlanEvent::Rejected { version, .. } => {
            if state.current_version.as_ref() == Some(version) {
                state.review_status = PlanReviewStatus::Rejected;
            }
        }
        PlanEvent::CommentAdded { anchor, body, .. } => {
            state.comments.push(PlanComment {
                anchor: anchor.clone(),
                body: body.clone(),
            });
        }
    }
}

/// 从事件序列重放重建 [`PlanState`]（逐步 [`apply`]）。
pub fn replay<'a>(events: impl IntoIterator<Item = &'a PlanEvent>) -> PlanState {
    let mut state = PlanState::default();
    for event in events {
        apply(&mut state, event);
    }
    state
}
