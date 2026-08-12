//! Pawork P16-4 Background Task Manager：process / agent / monitor / automation
//! 四类长生命周期任务的统一注册、状态与事件模型（Phase 16）。
//!
//! # 职责
//!
//! - 统一抽象：四类任务共用 [`BackgroundTaskId`]、状态机与输出/事件流句柄。
//! - 状态机：`queued → running → suspended → completed | failed | canceled`，
//!   所有转移发出 canonical [`TaskEvent`]，可持久化可重放。
//! - 断连续存：任务运行与连接解耦——in-memory 任务表 + 事件日志，
//!   `snapshot()` / `replay()` 恢复任务视图，`events_since` / `output_since`
//!   续读增量；CLI/GUI 断连不影响任务执行。
//! - 取消传播：取消 parent task 沿 `parent_task_id` 链传播到全部后代，无孤儿。
//! - 执行所有权：process 类任务是本模块完整接线的唯一执行路径，一律经
//!   构造函数注入的 `SandboxBackend` → `ProcessRuntime` 执行；task-manager
//!   只编排，不直连启动子进程、不自造进程树清理、不自定 sandbox policy。
//!   agent / monitor / automation kind 在此只提供注册 + 状态 + 事件抽象，
//!   具体执行由 P16-5 / P16-6 等后续 service 作为 adapter 接入。
//!
//! # 接口
//!
//! [`TaskManager`]：命令面（register / start / suspend / resume / finish /
//! cancel / start_process）与查询面（task / tasks / snapshot / event_log /
//! events_since / output / output_since / replay）+ 实时事件订阅 `subscribe`。
//! [`TaskManagerState`]：纯聚合状态，`apply` 为事件折叠 / 重放唯一入口。
//!
//! # 事件与持久化
//!
//! 命令先校验状态机合法性，再把事件折叠进 state，最后返回事件供调用方经
//! `agent_events::AgentEvent::Task(TaskEvent)` 持久化。Queued 是持久化前
//! 瞬态（注册不发事件，取消 Queued 任务静默移除）；重放以 Started 为任务
//! 创建点，无事件的任务不会在重放后出现。

mod error;
mod manager;
mod state;

pub use error::TaskManagerError;
pub use manager::TaskManager;
pub use state::{
    is_active_status, is_terminal_status, OutputEvent, TaskManagerSnapshot, TaskManagerState,
    TaskSnapshot,
};
