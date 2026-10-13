//! Pawork P16-1/P16-2 Plan Mode：只读计划状态机、版本替换、评审与审批 gate。
//!
//! # 职责
//! Plan 是 Agent 在动手前产出的「只读建议」——有序、可勾选的步骤序列。本
//! crate 提供进程内内存的 Plan 聚合：版本替换、评审提交与审批 gate，以及
//! event-sourcing 的 [`apply`] / [`replay`] 恢复入口。历史事件里的步骤推进、
//! 修订与行锚点意见仍由 `apply` 折叠，不经现行命令面发出。
//!
//! # 关键约束（只读）
//! Plan **不携带任何写入 / 工具执行能力**：[`PlanService`] 不暴露 spawn /
//! exec / write API，步骤文本仅作为惰性数据保存，绝不作为命令通道。审批
//! 仅作为执行 gate 放行（[`PlanService::is_approved_for_execution`]），不扩权。
//!
//! # 设计要点
//! - canonical 领域类型与事件载荷复用 `pawork_domain::workflow`（`PlanEvent`
//!   等）与 `pawork_domain::ids`；事件经 `pawork_domain::AgentEvent::Plan`
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
pub use state::{apply, replay, PlanComment, PlanState};
