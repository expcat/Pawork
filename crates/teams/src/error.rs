//! Team 协作层错误。

use agent_domain::{AgentId, EventId, PlanId, PlanStepId, PlanVersionId};
use orchestration::{TaskId, TaskState};

use crate::ids::TeamId;

/// Team 协作错误。
#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    /// team 不存在（或已解散）。
    #[error("team not found: {0}")]
    TeamNotFound(TeamId),
    /// team 已解散，拒绝任何写操作。
    #[error("team already dissolved: {0}")]
    TeamDissolved(TeamId),
    /// agent 不是 team 成员。
    #[error("agent {agent_id} is not a member of team {team_id}")]
    NotMember { team_id: TeamId, agent_id: AgentId },
    /// 成员已存在（重复加入）。
    #[error("agent {agent_id} is already a member of team {team_id}")]
    AlreadyMember { team_id: TeamId, agent_id: AgentId },
    /// 仅 supervisor 可执行的命令被普通成员发起。
    #[error("agent {agent_id} is not a supervisor of team {team_id}")]
    NotSupervisor { team_id: TeamId, agent_id: AgentId },
    /// 试图移除最后一个 supervisor（防孤儿：team 必须保留 supervisor 才能
    /// 审批 / 管理 / 解散；先添加第二个 supervisor 再移除）。
    #[error("cannot remove the last supervisor of team {0}")]
    LastSupervisor(TeamId),
    /// 任务不存在于共享任务板。
    #[error("task not on shared board: {0}")]
    TaskNotFound(TaskId),
    /// 任务已被认领，不能被他人认领。
    #[error("task {task_id} already claimed by {owner}")]
    TaskAlreadyClaimed { task_id: TaskId, owner: AgentId },
    /// 任务依赖未满足（前置任务未全部完成）。
    #[error("task {task_id} has unmet dependencies: {missing:?}")]
    UnmetDependencies {
        task_id: TaskId,
        missing: Vec<TaskId>,
    },
    /// 依赖的任务不在任务板上。
    #[error("task {task_id} depends on unknown task {dependency}")]
    UnknownDependency { task_id: TaskId, dependency: TaskId },
    /// 非法的任务状态转换。
    #[error("illegal task transition: {task_id} {from:?} -> {to:?}")]
    IllegalTaskTransition {
        task_id: TaskId,
        from: TaskState,
        to: TaskState,
    },
    /// 只有认领者可推进 / 释放任务。
    #[error("agent {agent_id} is not the owner of task {task_id}")]
    NotTaskOwner { task_id: TaskId, agent_id: AgentId },
    /// mailbox 消息不存在。
    #[error("mailbox message not found")]
    MailboxMessageNotFound,
    /// agent 不是该消息的收件人。
    #[error("agent {agent_id} is not a recipient of message")]
    NotRecipient { agent_id: AgentId },
    /// peer messaging fan-out 被策略拒绝。
    #[error("peer fan-out denied: {reason}")]
    FanOutDenied { reason: String },
    /// plan 不在审批队列中。
    #[error("plan not submitted to team: {0}")]
    PlanNotSubmitted(PlanId),
    /// plan 步骤不存在（评审锚点指向未知 step）。
    #[error("plan step not found: {0}")]
    PlanStepNotFound(PlanStepId),
    /// plan 版本不匹配。
    #[error("plan version mismatch: expected {expected}, got {actual}")]
    PlanVersionMismatch {
        expected: PlanVersionId,
        actual: PlanVersionId,
    },
    /// 未审批的 plan 不允许执行。
    #[error("plan {plan_id} version {version} is not approved for execution")]
    PlanNotApproved {
        plan_id: PlanId,
        version: PlanVersionId,
    },
    /// 事件序列 / 本地 ID 计数已到 u64 上限（checked 溢出，拒绝继续分配）。
    #[error("team {0} id space exhausted (u64 counter overflow)")]
    IdSpaceExhausted(&'static str),
    /// 拒绝 reason / body 等空文本。
    #[error("empty text is not allowed")]
    EmptyText,
    /// 持久化层失败（append / replay 未成功，状态保持不变）。
    #[error(transparent)]
    Store(#[from] TeamStoreError),
}

/// Team 事件持久化错误（durable `TeamEventStore` 的可失败契约）。
///
/// `append` / `replay` 失败时，命令面必须**不改变任何内存状态**（序列不推进、
/// 聚合不折叠、EventHub 镜像不投递）；调用方收到本错误后可安全重试。
#[derive(Debug, thiserror::Error)]
pub enum TeamStoreError {
    /// 后端存储错误（IO / SQLite 等），由实现方映射为可诊断文本。
    #[error("team event store: {0}")]
    Store(String),
    /// 同一事件被重复持久化（幂等冲突）。
    #[error("duplicate team event: {0}")]
    Duplicate(EventId),
    /// 重放的事件序列不连续（store 损坏或非 append-only）。
    #[error("team event sequence not contiguous: team {team_id} expected {expected}, got {found}")]
    NonContiguous {
        team_id: TeamId,
        expected: u64,
        found: u64,
    },
    /// 事件 JSON 无法反序列化（store 内容损坏）。
    #[error("team event json: {0}")]
    Json(#[from] serde_json::Error),
}
