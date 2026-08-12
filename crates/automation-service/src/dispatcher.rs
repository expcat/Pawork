//! 执行派发抽象。automation-service 只负责调度；真实 action executor 由调用方
//! 注入。当前 crate 不提供 TaskManager adapter，避免无执行器的实现创建或终结
//! 不属于自己的后台任务。

use agent_domain::{AutomationId, BackgroundTaskId};

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
