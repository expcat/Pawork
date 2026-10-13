//! Plan service 错误类型。

use pawork_domain::{PlanId, PlanReviewStatus, PlanVersionId};

/// Plan 命令面错误（非法状态机转移、缺失前置条件等）。
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("plan already exists: {0}")]
    AlreadyExists(PlanId),

    #[error("plan has not been created yet")]
    NotCreated,

    #[error("plan must contain at least one step")]
    EmptyPlan,

    #[error("step text must not be empty")]
    EmptyStepText,

    #[error("illegal review transition: {from:?} -> {to:?}")]
    IllegalReviewTransition {
        from: PlanReviewStatus,
        to: PlanReviewStatus,
    },

    #[error("version mismatch: expected {expected}, got {actual}")]
    VersionMismatch {
        expected: PlanVersionId,
        actual: PlanVersionId,
    },

    #[error("plan id mismatch: expected {expected}, got {actual}")]
    PlanIdMismatch { expected: PlanId, actual: PlanId },

    #[error("rejection reason must not be empty")]
    EmptyReason,
}
