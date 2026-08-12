//! 执行派发：抽象 [`AutomationDispatcher`] trait，让 task-manager 作为实现接入，
//! 避免 automation-service 与具体执行后端硬耦合。
//!
//! 触发后调用 `dispatch` 把动作派发为 background task（默认 `TaskKind::Automation`）。
//! service 不自带特权：派发经注入的 TaskManager 注册并 start，受既有 policy / 预算约束，
//! 与任何手动启动的后台任务等价。

use agent_domain::{AutomationId, BackgroundTaskId, TaskKind};
use task_manager::TaskManager;

use crate::automation::AutomationAction;
use crate::error::AutomationError;

/// 派发结果：automation 触发后产生的 background task 与触发时刻。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchOutcome {
    pub automation_id: AutomationId,
    pub task_id: BackgroundTaskId,
    /// 触发时刻（Unix 秒），用于 inbox 与下次调度计算。
    pub fired_at: u64,
}

/// 执行派发抽象。实现方负责把动作转换为 background task，返回其 ID。
///
/// 对象安全：automation-service 内部以 `Box<dyn AutomationDispatcher>` 持有，
/// 便于测试注入 mock 而不构造真实 TaskManager。
pub trait AutomationDispatcher: Send + Sync {
    /// 按 `automation_id` 与动作派发；返回 background task ID。
    fn dispatch(
        &self,
        automation_id: &AutomationId,
        action: &AutomationAction,
    ) -> Result<BackgroundTaskId, AutomationError>;
}

/// 把动作映射为 TaskManager 注册的 task kind。
///
/// 默认 `TaskKind::Automation`；`StartBackgroundTask` 显式指定 kind 时尊重之。
fn kind_for(action: &AutomationAction) -> TaskKind {
    match action {
        AutomationAction::StartBackgroundTask { task_kind } => *task_kind,
        _ => TaskKind::Automation,
    }
}

/// task-manager 适配实现：经注入的 [`TaskManager`] 注册（Queued）并 start。
///
/// 不直接 spawn 子进程：prompt / tool call / automation 类任务的执行语义由后台
/// 任务系统与 agent engine 落地；这里只产出可追踪的 background task 句柄。
#[derive(Clone)]
pub struct TaskManagerDispatcher {
    task_manager: TaskManager,
}

impl TaskManagerDispatcher {
    pub fn new(task_manager: TaskManager) -> Self {
        Self { task_manager }
    }

    /// 注入的 TaskManager 句柄。
    pub fn task_manager(&self) -> &TaskManager {
        &self.task_manager
    }
}

impl AutomationDispatcher for TaskManagerDispatcher {
    fn dispatch(
        &self,
        automation_id: &AutomationId,
        action: &AutomationAction,
    ) -> Result<BackgroundTaskId, AutomationError> {
        let kind = kind_for(action);
        let task_id = self
            .task_manager
            .register(kind, None)
            .map_err(|err| AutomationError::DispatchFailed {
                automation_id: automation_id.clone(),
                detail: err.to_string(),
            })?;
        // start 把 Queued → Running 并发出 TaskEvent::Started；失败时清理 queued
        // 记录以避免幽灵任务（register 的 queued 为持久化前瞬态，安全丢弃）。
        if let Err(err) = self.task_manager.start(&task_id) {
            return Err(AutomationError::DispatchFailed {
                automation_id: automation_id.clone(),
                detail: format!("start failed: {err}"),
            });
        }
        Ok(task_id)
    }
}
