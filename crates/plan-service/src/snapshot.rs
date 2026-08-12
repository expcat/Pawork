//! Plan 查询面 DTO（serde 可序列化，供 CLI/GUI 经 GUI Connection Protocol 消费）。

use agent_domain::{PlanId, PlanReviewStatus, PlanStepSnapshot, PlanVersionId};
use serde::{Deserialize, Serialize};

/// 当前 Plan 的只读快照（查询面）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSnapshot {
    pub plan_id: PlanId,
    pub version: PlanVersionId,
    pub title: String,
    pub steps: Vec<PlanStepSnapshot>,
    pub review_status: PlanReviewStatus,
}

/// 版本历史中的一个条目（修订链节点）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanVersionInfo {
    pub version: PlanVersionId,
    pub parent_version: Option<PlanVersionId>,
    pub title: String,
    pub steps: Vec<PlanStepSnapshot>,
}
