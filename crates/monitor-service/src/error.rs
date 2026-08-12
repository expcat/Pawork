//! monitor-service 错误类型。

use agent_domain::MonitorId;
use task_manager::TaskManagerError;

/// monitor-service 命令与重放路径的错误。
#[derive(Debug, thiserror::Error)]
pub enum MonitorServiceError {
    /// 引用了未注册的 monitor。
    #[error("unknown monitor `{0}`")]
    UnknownMonitor(MonitorId),
    /// monitor ID 已注册；拒绝覆盖配置或创建第二个 task 镜像。
    #[error("monitor already registered: {0}")]
    AlreadyRegistered(MonitorId),
    /// 注册时配置自洽性校验失败。
    #[error("invalid monitor config: {0}")]
    InvalidConfig(String),
    /// 经 task-manager 注册 / 启动 / 结束失败（propagated）。
    #[error(transparent)]
    TaskManager(#[from] TaskManagerError),
}
