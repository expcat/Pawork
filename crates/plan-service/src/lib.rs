//! Pawork P16-1/P16-2 Plan Mode：只读计划状态机、版本替换、评审与审批 gate。
//!
//! # 职责
//! Plan 是 Agent 在动手前产出的「只读建议」——有序、可勾选的步骤序列。本
//! crate 提供进程内内存的 Plan 聚合：步骤状态机（`pending → in_progress →
//! completed | blocked`，`blocked → in_progress`）、版本替换与修订链、评审
//! 状态机（`draft → in_review → changes_requested → approved | rejected`）、
//! 行锚点评审意见与审批 gate，以及 event-sourcing 的 [`apply`] / [`replay`]
//! 恢复入口。
//!
//! # 关键约束（只读）
//! Plan **不携带任何写入 / 工具执行能力**：[`PlanService`] 不暴露 spawn /
//! exec / write API，步骤文本仅作为惰性数据保存，绝不作为命令通道。审批
//! 仅作为执行 gate 放行（[`PlanService::is_approved_for_execution`]），不扩权。
//!
//! # 设计要点
//! - canonical 领域类型与事件载荷复用 `agent_domain::workflow`（`PlanEvent`
//!   等）与 `agent_domain::ids`；事件经 `agent_events::AgentEvent::Plan`
//!   持久化（本 crate 仅产出 `PlanEvent`，封装/落盘由 session-store 负责）。
//! - [`apply`] 是纯函数折叠，为崩溃恢复的唯一入口；命令面（[`PlanService`]）
//!   完成状态机校验后 `apply` 再返回事件给调用方持久化。
//!
//! [`apply`]: state::apply
//! [`replay`]: state::replay

mod error;
mod service;
mod snapshot;
mod state;

pub use error::PlanError;
pub use service::PlanService;
pub use snapshot::{PlanSnapshot, PlanVersionInfo};
pub use state::{apply, is_legal_step_transition, replay, PlanComment, PlanState};
