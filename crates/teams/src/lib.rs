//! Pawork P17-6 Agent Teams：多 Agent 协作层。
//!
//! 在 P12 supervisor / worker 之上叠加 team 协作语义：shared task board、
//! 持久 mailbox、presence、受控 peer messaging、plan approval。**复用 P12
//! 编排与 P16 plan**，不重写 run loop、不自建 `tokio::broadcast`。
//!
//! # 事件与统一 Event Hub（ADR-024）
//! 所有协作变化都是 canonical [`event::TeamEvent`]，经注入的
//! [`event::TeamEventSink`] 投递——这是本 crate 的唯一外发出口。
//! `app-service` 实现 sink，把 team 事件适配为 `core_api::AppEvent` 后调用
//! `subscription_hub::EventHub::publish`，由唯一 EventHub 统一全局序列化、
//! ring buffer 化与广播。崩溃恢复重放同一份 team 事件序列（[`service::TeamService::from_envelopes`]）。
//!
//! # 依赖方向
//! 依赖 `agent-domain` / `orchestration`（P12 TaskGraph / WorkerState）/ `plan-service`
//! （P16-2 review 状态机）；被 `app-service` 装配。`task-manager` 仍是唯一的
//! 执行权威——teams 只产出协作事实，不派发后台任务。
//!
//! 模块：
//! - [`service`]：命令面 / 查询面 facade（生命周期 / 任务板 / mailbox / presence / peer / 审批）
//! - [`event`]：canonical 事件、信封、sink 契约
//! - [`store`]：durable 可失败 `TeamEventStore`（append / replay）契约与内存实现
//! - [`state`]：event-sourcing `apply` / `replay`
//! - [`task_board`]、[`mailbox`]、[`peer`]、[`presence`]、[`approval`]：各子系统 pure 校验

#![forbid(unsafe_code)]

pub mod approval;
pub mod error;
pub mod event;
pub mod ids;
pub mod mailbox;
pub mod peer;
pub mod presence;
pub mod service;
pub mod state;
pub mod store;
pub mod task_board;

pub use error::{TeamError, TeamStoreError};
pub use event::{
    BoardTask, NullTeamSink, Recipients, RecordingTeamSink, TeamEvent, TeamEventEnvelope,
    TeamEventSequence, TeamEventSink,
};
pub use ids::{FanOutId, MailboxMessageId, MemberRole, TeamId};
pub use peer::PeerPolicy;
pub use presence::Presence;
pub use service::TeamService;
pub use state::{replay, MailboxEntry, PlanApprovalEntry, TeamAggregate};
pub use store::{MemoryTeamStore, TeamEventStore};
