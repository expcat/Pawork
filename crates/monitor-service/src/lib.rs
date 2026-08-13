//! Pawork P16-6 Persistent Process / Monitor：声明式监视循环（Phase 16）。
//!
//! # 职责
//!
//! - 声明式监视循环：注册 monitors，命中产出 canonical
//!   [`MonitorEvent::Triggered`]，可作为 P16-5 automation 的 `event` 触发器来源；
//!   也是 P17-2 Plugin Package Monitors 声明的唯一运行时执行点。
//! - 确定性判定核心：[`evaluate`] 是纯函数，对注入的 [`Observation`] 做命中
//!   判定，不依赖 tokio 长循环，可独立单测；观测样本由调用方（宿主 / 未来
//!   driver）注入，本 crate 不内置 watcher。
//! - 输出节流：[`Throttle`] 有界缓冲，高吞吐输出经裁剪不堆积。
//! - 断连续存与重放：monitor 注册到注入的 task-manager 为 `TaskKind::Monitor`，
//!   复用其 snapshot+replay；本服务亦提供独立的事件折叠重放入口。
//!
//! # 进程统一所有权（硬约束）
//!
//! monitor 若需启动子进程，禁止直接 `tokio::process::Command` /
//! `std::process::Command`；必须经注入的 [`task_manager::TaskManager`]（其内部
//! 已走 SandboxBackend -> ProcessRuntime）。monitor-service 不自复制进程树
//! 清理、不自定 sandbox policy。常驻进程宿主由 process-runtime 负责，本任务
//! 聚焦监视循环。
//!
//! # 接口
//!
//! [`MonitorService`]：命令面（register / start / evaluate / stop / unregister / replay）与
//! 查询面（snapshot / record / records / event_log）+ 实时事件订阅 `subscribe`。
//! [`state::MonitorServiceState`]：纯聚合状态，`apply` 为事件折叠 / 重放入口。

mod config;
mod error;
mod evaluate;
mod service;
mod state;
mod throttle;

pub use config::{Monitor, MonitorConfig, Observation};
pub use error::MonitorServiceError;
pub use evaluate::evaluate;
pub use service::MonitorService;
pub use state::{MonitorRecord, MonitorServiceSnapshot, MonitorServiceState, MonitorStatus};
pub use throttle::{Throttle, ThrottlePolicy};

// 便捷再导出 canonical 领域类型（调用方通常只需 use monitor_service::*）。
pub use agent_domain::{MonitorEvent, MonitorId, MonitorSourceKind, WorkspaceId};
