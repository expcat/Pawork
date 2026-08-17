//! Pawork P16-5 Scheduled Automation：cron / interval / once / event 四种触发器
//! 与可检索的 result inbox（Phase 16）。
//!
//! # 职责
//! - **触发器配置**：[`Automation`] / [`AutomationTrigger`] / [`AutomationAction`]。
//! - **cron 自实现**：五字段最小子集解析与 [`cron::next_fire`]（不引入调度框架）。
//! - **确定性引擎**：[`AutomationEngine`] 以注入的 `now`（Unix 秒）判定到期、
//!   派发、归档结果；不依赖真实 tokio timer，便于测试。
//! - **派发解耦**：经 [`AutomationDispatcher`] 注入真实 action executor；本 crate
//!   不提供会伪造执行状态的 TaskManager adapter。
//! - **Result Inbox**：[`ResultInbox`] 按 automation / 时间 / 状态检索（内存结构）。
//! - **失败退避**：连续失败达阈值发出 [`AutomationEvent::Suspended`] 暂停并告警，
//!   不静默吞错。
//!
//! # 事件与持久化
//! 命令面完成校验后构造 canonical [`AutomationEvent`]，`apply` 到 [`AutomationState`]
//! 后返回事件给调用方经 `pawork_domain::AgentEvent::Automation` 持久化。完整触发器
//! 配置、inbox 状态与失败计数是命令侧视图（不在轻量事件中），重放时按需重新注册。

pub mod automation;
pub mod cron;
pub mod dispatcher;
pub mod engine;
pub mod error;
pub mod inbox;
pub mod state;

pub use pawork_domain::{AutomationEvent, AutomationId, AutomationTriggerKind};

pub use automation::{Automation, AutomationAction, AutomationTrigger};
pub use cron::{next_fire, CronSchedule};
pub use dispatcher::{AutomationDispatcher, DispatchOutcome};
pub use engine::{AutomationEngine, AutomationSnapshot, EngineConfig};
pub use error::AutomationError;
pub use inbox::{InboxItem, InboxQuery, InboxStatus, ResultInbox};
pub use state::{replay, ArchivedResult, AutomationState, AutomationView};
