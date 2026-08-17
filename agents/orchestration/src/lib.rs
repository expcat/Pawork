//! Pawork 多 Agent 编排：Supervisor / Worker 生命周期、任务图、worktree
//! 隔离、预算闸门，以及 Agent Teams 协作层。
//!
//! 合并自 V1 `orchestration` + `teams`。`OrchestrationEvent` 留在
//! [`lifecycle`]，不并入 `pawork-domain` / `AgentEvent`。

mod budget;
mod identity;
mod lifecycle;
mod merge;
mod supervisor;
mod task_graph;
mod worktree;

pub mod teams;

pub use budget::*;
pub use identity::*;
pub use lifecycle::*;
pub use merge::*;
pub use supervisor::*;
pub use task_graph::*;
pub use worktree::*;
pub use teams::{
    replay, BoardTask, FanOutId, MailboxEntry, MailboxMessageId, MemberRole, MemoryTeamStore,
    NullTeamSink, PeerPolicy, PlanApprovalEntry, Presence, Recipients, RecordingTeamSink,
    TeamAggregate, TeamError, TeamEvent, TeamEventEnvelope, TeamEventSequence, TeamEventSink,
    TeamEventStore, TeamId, TeamService, TeamStoreError,
};
