//! Goal 聚合状态与 event-sourcing 折叠（`apply` / `replay`）。
//!
//! [`apply`] 是恢复入口：把一个 canonical [`GoalEvent`] 纯函数式折叠进
//! [`GoalState`]。事件被视为已校验的「事实」，`apply` 不再重复状态机校验
//! （校验由命令面 [`crate::GoalService`] 完成）；崩溃后重放事件序列即可重建
//! Goal、生命周期状态、进度、转向历史与剩余预算。
//!
//! 重放边界：canonical 事件集不包含逐项 criterion 满足事件，`satisfied`
//! 满足位是命令面维护的运行内存事实；进度经 `ProgressUpdated` 以命中率快照
//! 形式持久化，可追溯。

use agent_domain::{GoalEvent, GoalId, GoalStatus, SuccessCriterionSnapshot};

use crate::snapshot::GoalSnapshot;

/// Goal 聚合的当前状态（`apply` 折叠结果）。
///
/// 持有 goal_id / title / criteria（含满足位）、生命周期状态、基于 criteria
/// 命中率的进度与可回溯的转向历史。本结构不执行任何 IO，可被自由克隆/比较。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GoalState {
    goal_id: Option<GoalId>,
    title: Option<String>,
    criteria: Vec<SuccessCriterionSnapshot>,
    status: GoalStatus,
    progress: f64,
    steering_history: Vec<String>,
    remaining_budget_tokens: Option<u64>,
}

impl GoalState {
    pub fn goal_id(&self) -> Option<&GoalId> {
        self.goal_id.as_ref()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn criteria(&self) -> &[SuccessCriterionSnapshot] {
        &self.criteria
    }

    pub fn status(&self) -> GoalStatus {
        self.status
    }

    pub fn progress(&self) -> f64 {
        self.progress
    }

    pub fn steering_history(&self) -> &[String] {
        &self.steering_history
    }

    /// 最近一次 `Resumed` 复算并注入的剩余预算。
    pub fn remaining_budget_tokens(&self) -> Option<u64> {
        self.remaining_budget_tokens
    }

    /// 构造查询面快照；尚未 `Created` 时返回 `None`。
    pub fn snapshot(&self) -> Option<GoalSnapshot> {
        let goal_id = self.goal_id.clone()?;
        Some(GoalSnapshot {
            goal_id,
            title: self.title.clone().unwrap_or_default(),
            criteria: self.criteria.clone(),
            status: self.status,
            progress: self.progress,
            steering_history: self.steering_history.clone(),
            remaining_budget_tokens: self.remaining_budget_tokens,
        })
    }

    pub(crate) fn criterion_mut(
        &mut self,
        criterion_id: &str,
    ) -> Option<&mut SuccessCriterionSnapshot> {
        self.criteria
            .iter_mut()
            .find(|c| c.criterion_id == criterion_id)
    }
}

/// 基于 criteria 命中率计算进度：`satisfied_count / total`（无 criteria 时为 0）。
pub fn recompute_progress(criteria: &[SuccessCriterionSnapshot]) -> f64 {
    if criteria.is_empty() {
        return 0.0;
    }
    let satisfied = criteria.iter().filter(|c| c.satisfied).count();
    satisfied as f64 / criteria.len() as f64
}

/// 把一个 canonical [`GoalEvent`] 纯函数式折叠进 [`GoalState`]（恢复入口）。
pub fn apply(state: &mut GoalState, event: &GoalEvent) {
    match event {
        GoalEvent::Created {
            goal_id,
            title,
            criteria,
        } => {
            state.goal_id = Some(goal_id.clone());
            state.title = Some(title.clone());
            state.criteria = criteria.clone();
            state.status = GoalStatus::Active;
            state.progress = recompute_progress(criteria);
            state.steering_history.clear();
            state.remaining_budget_tokens = None;
        }
        GoalEvent::ProgressUpdated { progress, .. } => {
            // progress 是已校验事实；防御性收敛到 [0,1] 保持不变量。
            state.progress = progress.clamp(0.0, 1.0);
        }
        GoalEvent::Paused { .. } => {
            state.status = GoalStatus::Paused;
        }
        GoalEvent::Resumed {
            remaining_budget_tokens,
            ..
        } => {
            state.status = GoalStatus::Active;
            // resume 时复算的剩余预算，覆盖旧值。
            state.remaining_budget_tokens = Some(*remaining_budget_tokens);
        }
        GoalEvent::Steered { input, .. } => {
            state.steering_history.push(input.clone());
        }
        GoalEvent::Achieved { .. } => {
            state.status = GoalStatus::Achieved;
        }
        GoalEvent::Abandoned { .. } => {
            state.status = GoalStatus::Abandoned;
        }
    }
}

/// 从事件序列重放重建 [`GoalState`]（逐步 [`apply`]）。
pub fn replay<'a>(events: impl IntoIterator<Item = &'a GoalEvent>) -> GoalState {
    let mut state = GoalState::default();
    for event in events {
        apply(&mut state, event);
    }
    state
}
