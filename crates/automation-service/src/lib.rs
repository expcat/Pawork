//! Pawork P16-5 Scheduled Automation：cron / interval / once / event 四种触发器
//! 与可检索的 result inbox（Phase 16）。
//!
//! # 职责
//! - **触发器配置**：[`Automation`] / [`AutomationTrigger`] / [`AutomationAction`]。
//! - **cron 自实现**：五字段最小子集解析与 [`cron::next_fire`]（不引入调度框架）。
//! - **确定性引擎**：[`AutomationEngine`] 以注入的 `now`（Unix 秒）判定到期、
//!   派发、归档结果；不依赖真实 tokio timer，便于测试。
//! - **派发解耦**：经 [`AutomationDispatcher`] 抽象，[`TaskManagerDispatcher`]
//!   让 task-manager 作为实现接入——service 不自带特权，派发受既有 policy / 预算约束。
//! - **Result Inbox**：[`ResultInbox`] 按 automation / 时间 / 状态检索（内存结构）。
//! - **失败退避**：连续失败达阈值发出 [`AutomationEvent::Suspended`] 暂停并告警，
//!   不静默吞错。
//! - **外部触发器（P2 预留）**：[`ExternalTrigger`] 信封；具体平台 adapter 在 Core
//!   边界完成认证 / 签名 / 限速 / 重放防护后转为 canonical 载荷字符串；engine 只
//!   匹配已认证的 canonical 载荷，**不含任何平台名称分支**。
//!
//! # 事件与持久化
//! 命令面完成校验后构造 canonical [`AutomationEvent`]，`apply` 到 [`AutomationState`]
//! 后返回事件给调用方经 `agent_events::AgentEvent::Automation` 持久化。完整触发器
//! 配置、inbox 状态与失败计数是命令侧视图（不在轻量事件中），重放时按需重新注册。

pub mod automation;
pub mod cron;
pub mod dispatcher;
pub mod engine;
pub mod error;
pub mod external;
pub mod inbox;
pub mod state;

pub use agent_domain::{AutomationEvent, AutomationId, AutomationTriggerKind};

pub use automation::{Automation, AutomationAction, AutomationTrigger};
pub use cron::{next_fire, CronSchedule};
pub use dispatcher::{AutomationDispatcher, DispatchOutcome, TaskManagerDispatcher};
pub use engine::{AutomationEngine, AutomationSnapshot, EngineConfig};
pub use error::AutomationError;
pub use external::{canonical_event_from_external, ExternalTrigger};
pub use inbox::{InboxItem, InboxQuery, InboxStatus, ResultInbox};
pub use state::{replay, ArchivedResult, AutomationState, AutomationView};
