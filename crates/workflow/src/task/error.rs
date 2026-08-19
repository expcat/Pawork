//! task-manager 的错误类型。

use pawork_domain::{BackgroundTaskId, TaskStatus};

/// task-manager 命令与重放路径的错误。
#[derive(Debug, thiserror::Error)]
pub enum TaskManagerError {
    /// 引用了不存在的任务。
    #[error("unknown background task `{0}`")]
    UnknownTask(BackgroundTaskId),
    /// 注册子任务时父任务不存在。
    #[error("parent background task `{0}` not found")]
    UnknownParent(BackgroundTaskId),
    /// 状态机拒绝的转移。
    #[error("invalid transition for task `{task_id}`: {from:?} -> {to:?}")]
    InvalidTransition {
        /// 发生非法转移的任务。
        task_id: BackgroundTaskId,
        /// 当前状态。
        from: TaskStatus,
        /// 目标状态。
        to: TaskStatus,
    },
    /// `finish` 只接受终态中的 Completed / Failed；Canceled 必须走 `cancel`。
    #[error("invalid finished status `{0:?}`; use cancel() to cancel a task")]
    InvalidFinishedStatus(TaskStatus),
}
