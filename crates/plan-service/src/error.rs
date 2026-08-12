//! Plan service 错误类型。

use agent_domain::{PlanId, PlanStepId, PlanStepStatus};

/// Plan 命令面错误（非法状态机转移、缺失前置条件等）。
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("plan already exists: {0}")]
    AlreadyExists(PlanId),

    #[error("plan has not been created yet")]
    NotCreated,

    #[error("step not found: {0}")]
    StepNotFound(PlanStepId),

    #[error("illegal step transition: {from:?} -> {to:?}")]
    IllegalStepTransition {
        from: PlanStepStatus,
        to: PlanStepStatus,
    },

    #[error("plan must contain at least one step")]
    EmptyPlan,

    #[error("step text must not be empty")]
    EmptyStepText,
}
