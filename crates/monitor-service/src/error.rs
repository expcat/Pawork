//! monitor-service 错误类型。

use agent_domain::MonitorId;
use task_manager::TaskManagerError;

/// monitor-service 命令与重放路径的错误。
#[derive(Debug, thiserror::Error)]
pub enum MonitorServiceError {
    /// 引用了未注册的 monitor。
    #[error("unknown monitor `{0}`")]
    UnknownMonitor(MonitorId),
    /// 注册时配置自洽性校验失败。
    #[error("invalid monitor config: {0}")]
    InvalidConfig(String),
    /// 经 task-manager 注册 / 启动 / 结束失败（propagated）。
    #[error(transparent)]
    TaskManager(#[from] TaskManagerError),
    /// 可选 driver 构造失败（watcher 注册等）。
    #[error("file watcher error: {0}")]
    Watcher(String),
}
