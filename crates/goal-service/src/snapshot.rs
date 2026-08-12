//! Goal 查询面 DTO（serde 可序列化，供 CLI/GUI 经 GUI Connection Protocol 消费）。

use agent_domain::{GoalId, GoalStatus, SuccessCriterionSnapshot};
use serde::{Deserialize, Serialize};

/// Goal 的只读快照（查询面）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoalSnapshot {
    pub goal_id: GoalId,
    pub title: String,
    pub criteria: Vec<SuccessCriterionSnapshot>,
    pub status: GoalStatus,
    /// criteria 命中率，恒 ∈ [0,1]。
    pub progress: f64,
    /// 运行中转向输入（按时间顺序，可回溯）。
    pub steering_history: Vec<String>,
    /// 最近一次 `Resumed` 时复算并注入的剩余预算；尚未 resume 过则为 `None`。
    pub remaining_budget_tokens: Option<u64>,
}
