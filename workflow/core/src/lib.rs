//! `pawork-workflow`：Plan / Goal / Task / Automation / Monitor 五合一 reducer。
//!
//! 各域独立模块与独立状态机。默认构建为纯状态机，不依赖 `pawork-exec`。
//! 进程类任务的真实执行经 `process-exec` feature 门控。
//!
//! canonical 事件类型（`PlanEvent` / `GoalEvent` / `TaskEvent` /
//! `AutomationEvent` / `MonitorEvent`）定义在 `pawork-domain`，本 crate 只消费、
//! 不重定义。

pub mod automation;
pub mod goal;
pub mod monitor;
pub mod plan;
pub mod task;
