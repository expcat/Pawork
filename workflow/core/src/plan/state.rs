//! Plan 聚合状态与 event-sourcing 折叠（`apply` / `replay`）。
//!
//! [`apply`] 是恢复入口：把一个 canonical [`PlanEvent`] 纯函数式折叠进
//! [`PlanState`]。事件被视为已校验的「事实」，`apply` 不再重复状态机校验
//! （校验由命令面 [`crate::plan::PlanService`] 完成）；崩溃后重放事件序列即可重建
//! 当前 Plan 与进度。
//!
//! 评审 / 审批变体（`ReviewRequested` / `Revised` / `Approved` / `Rejected` /
//! `CommentAdded`）在此折叠为评审状态机、评审意见列表与审批 checkpoint；
//! 命令面语义由 [`crate::plan::PlanService`] 校验。

use pawork_domain::{
    CheckpointId, PlanCommentAnchor, PlanEvent, PlanId, PlanReviewStatus, PlanStepId,
    PlanStepSnapshot, PlanStepStatus, PlanVersionId,
};

use super::snapshot::{PlanSnapshot, PlanVersionInfo};

/// 一条评审意见（行锚点 + 正文；serde 可序列化，随快照对外查询）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanComment {
    pub anchor: PlanCommentAnchor,
    pub body: String,
}

/// Plan 聚合的当前状态（`apply` 折叠结果）。
///
/// 持有当前版本的 title / steps / version / parent 指针、全部版本历史
/// （修订链）、评审状态（P16-1 默认 [`PlanReviewStatus::Draft`]）与当前版本下
/// 的评审意见、审批 checkpoint（批准点，可回滚）。本结构不执行任何 IO，可被
/// 自由克隆 / 比较。
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
    approved_checkpoint_id: Option<CheckpointId>,
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

    /// 当前版本审批时关联的 checkpoint（批准点，可回滚）；未审批时 `None`。
    pub fn approved_checkpoint_id(&self) -> Option<&CheckpointId> {
        self.approved_checkpoint_id.as_ref()
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
            comments: self.comments.clone(),
            approved_checkpoint_id: self.approved_checkpoint_id.clone(),
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
            state.approved_checkpoint_id = None;
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
            state.approved_checkpoint_id = None;
        }
        // —— P16-2 评审/审批变体的折叠 ——
        // `ReviewRequested` 推进「评审回合」：Draft → InReview（首次提交评审），
        // InReview → ChangesRequested（评审方请求修改后再提交）。事件是已校验
        // 事实，此处按当前状态推进；重放序列一致即可确定性重建状态。
        PlanEvent::ReviewRequested { version, .. } => {
            if state.current_version.as_ref() == Some(version) {
                state.review_status = match state.review_status {
                    PlanReviewStatus::Draft => PlanReviewStatus::InReview,
                    PlanReviewStatus::InReview => PlanReviewStatus::ChangesRequested,
                    other => other,
                };
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
            state.approved_checkpoint_id = None;
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
        PlanEvent::Approved {
            version,
            checkpoint_id,
            ..
        } => {
            if state.current_version.as_ref() == Some(version) {
                state.review_status = PlanReviewStatus::Approved;
                state.approved_checkpoint_id = checkpoint_id.clone();
            }
        }
        PlanEvent::Rejected { version, .. } => {
            if state.current_version.as_ref() == Some(version) {
                state.review_status = PlanReviewStatus::Rejected;
                state.approved_checkpoint_id = None;
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
