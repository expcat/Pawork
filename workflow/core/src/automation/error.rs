//! automation-service 的错误类型。

use pawork_domain::{AutomationId, BackgroundTaskId};

/// automation-service 命令面与重放路径的错误。
#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    /// 引用了未注册的 automation。
    #[error("automation not registered: {0}")]
    NotRegistered(AutomationId),

    /// automation 已存在（重复注册）。
    #[error("automation already registered: {0}")]
    AlreadyRegistered(AutomationId),

    /// automation 已被挂起（连续失败或手动暂停），不再触发。
    #[error("automation {0} is suspended: {1}")]
    Suspended(AutomationId, String),

    /// once 触发器已触发过一次，不会再次触发。
    #[error("once trigger already fired: {0}")]
    OnceAlreadyFired(AutomationId),

    /// cron 表达式不合法。
    #[error("invalid cron expression `{expr}`: {detail}")]
    InvalidCron { expr: String, detail: String },

    /// event 触发器的匹配模式（正则）不合法。
    #[error("invalid event pattern `{0}`")]
    InvalidEventPattern(String),

    /// 派发失败（注入的 dispatcher / TaskManager 拒绝或不可用）。
    #[error("dispatch failed for automation {automation_id}: {detail}")]
    DispatchFailed {
        automation_id: AutomationId,
        detail: String,
    },

    /// 记录结果时找不到对应的触发记录。
    #[error("no fired task recorded for automation {0}")]
    NoFiredTask(AutomationId),

    /// 记录结果的 task 并非该 automation 的 canonical `Triggered` 事实。
    #[error("task {task_id} was not triggered by automation {automation_id}")]
    TaskNotTriggeredByAutomation {
        automation_id: AutomationId,
        task_id: BackgroundTaskId,
    },
}
