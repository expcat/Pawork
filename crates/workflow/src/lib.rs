//! `pawork-workflow`：Plan / Task reducer。
//!
//! 各域独立模块与独立状态机。默认构建为纯状态机。
//!
//! canonical 事件类型（`PlanEvent` / `TaskEvent`）定义在 `pawork-domain`，
//! 本 crate 只消费、不重定义。

pub mod plan;
pub mod task;
