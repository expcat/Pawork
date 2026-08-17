//! Goal service 错误类型。

use pawork_domain::{GoalId, GoalStatus};

/// Goal 命令面错误（非法状态机转移、缺失前置条件、权限边界等）。
#[derive(Debug, thiserror::Error)]
pub enum GoalError {
    #[error("goal not found: {0}")]
    GoalNotFound(GoalId),

    #[error("criterion not found: goal={goal_id} criterion={criterion_id}")]
    CriterionNotFound {
        goal_id: GoalId,
        criterion_id: String,
    },

    /// 命令需要 Goal 处于 `Active`（暂停/终态下不可满足标准或转向）。
    #[error("goal is not active: {0}")]
    GoalNotActive(GoalId),

    /// Agent 只能自行满足 `Auto` 项；`Human` 项必须经人审入口。
    #[error("human-review criterion cannot be satisfied by agent: {0}")]
    HumanCriterionNotAutoSatisfiable(String),

    #[error("goal title must not be empty")]
    EmptyTitle,

    #[error("goal must contain at least one success criterion")]
    EmptyCriteria,

    #[error("criterion description must not be empty")]
    EmptyCriterionDescription,

    #[error("steer input must not be empty")]
    EmptySteerInput,

    #[error("illegal goal status transition: {from:?} -> {to:?}")]
    IllegalStatusTransition { from: GoalStatus, to: GoalStatus },
}
