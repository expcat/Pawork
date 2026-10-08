//! Pawork 多 Agent 编排：Supervisor / Worker 生命周期、预算闸门，以及尚未
//! 由 Host 装配的任务图、worktree 隔离与 patch merge。
//!
//! `OrchestrationEvent` 留在 [`lifecycle`]，不并入 `pawork-domain` /
//! `AgentEvent`。App 子代理只使用 spawn / start / complete / fail / cancel_tree。
//! 图、分配器、合并器、retry_task 与 recover_report 只在测试构建注入，不是现行调度。

mod budget;
mod identity;
mod lifecycle;
mod merge;
mod supervisor;
mod task_graph;
mod worktree;

pub use budget::*;
pub use identity::*;
pub use lifecycle::*;
pub use merge::*;
pub use supervisor::*;
pub use task_graph::*;
pub use worktree::*;
