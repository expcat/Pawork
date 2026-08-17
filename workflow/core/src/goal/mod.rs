//! Pawork P16-3 Goal Mode：可验证成功标准、进度度量、pause/resume 与运行中转向（Phase 16）。
//!
//! # 职责
//! Goal 是 Agent 一次工作的长期锚点：携带可验证 success criteria（`Auto`
//! 可机检、`Human` 需人确认）、基于 criteria 命中率的进度度量、pause/resume
//! 生命周期与运行中 steering。本 crate 提供进程内内存的 Goal 聚合与
//! event-sourcing 的 [`apply`] / [`replay`] 恢复入口。
//!
//! # 设计要点
//! - canonical 领域类型与事件载荷复用 `pawork_domain::workflow`（`GoalEvent` /
//!   `GoalStatus` / `CriterionKind` / `SuccessCriterionSnapshot`）与
//!   `pawork_domain::ids`；事件经 `pawork_domain::AgentEvent::Goal` 持久化
//!   （本 crate 仅产出 `GoalEvent`，封装/落盘由 session-store 负责）。
//! - [`apply`] 是纯函数折叠，为崩溃恢复的唯一入口；命令面（[`GoalService`]）
//!   完成状态机校验后 `apply` 再返回事件给调用方持久化。
//! - 重放完整性（ADR-016）：单项 criterion 满足位由 `CriterionSatisfied` 持久化，
//!   命中率进度由 `ProgressUpdated` 快照；二者同时产出，replay 后 criteria 与
//!   progress 不再自相矛盾。状态机 / 进度 / 转向历史 / 剩余预算均可从事件流恢复。
//! - 权限边界：`satisfy_criterion` 只能满足 `Auto` 项；`Human` 项必须走
//!   [`GoalService::mark_human_satisfied`] 显式人审入口，Agent 不能自行
//!   宣布人审项达成。
//!
//! [`apply`]: state::apply
//! [`replay`]: state::replay

mod error;
mod service;
mod snapshot;
mod state;

pub use error::GoalError;
pub use service::{CriterionDraft, GoalService};
pub use snapshot::GoalSnapshot;
pub use state::{apply, recompute_progress, replay, GoalState};
