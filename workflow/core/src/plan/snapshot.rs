//! Plan 查询面 DTO（serde 可序列化，供 CLI/GUI 经 GUI Connection Protocol 消费）。

use pawork_domain::{CheckpointId, PlanId, PlanReviewStatus, PlanStepSnapshot, PlanVersionId};
use serde::{Deserialize, Serialize};

use super::state::PlanComment;

/// 当前 Plan 的只读快照（查询面）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSnapshot {
    pub plan_id: PlanId,
    pub version: PlanVersionId,
    pub title: String,
    pub steps: Vec<PlanStepSnapshot>,
    pub review_status: PlanReviewStatus,
    /// 当前版本下的评审意见（行锚点 + 正文）。
    pub comments: Vec<PlanComment>,
    /// 审批时关联的 checkpoint（批准点，可回滚）；未审批为 `None`。
    pub approved_checkpoint_id: Option<CheckpointId>,
}

/// 版本历史中的一个条目（修订链节点）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanVersionInfo {
    pub version: PlanVersionId,
    pub parent_version: Option<PlanVersionId>,
    pub title: String,
    pub steps: Vec<PlanStepSnapshot>,
}
