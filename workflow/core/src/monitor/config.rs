//! Monitor 声明式配置模型与观测样本（P16-6）。
//!
//! 配置由调用方声明，进入 [`crate::monitor::MonitorService`] 后由确定性
//! [`crate::monitor::evaluate::evaluate`] 判定是否命中。配置不携带执行能力：
//! 观测样本由调用方（宿主 / 未来 driver）归一为 [`Observation`] 后喂入判定核心；
//! 需要启动子进程的来源一律经注入的 task-manager（其内部走 SandboxBackend ->
//! ProcessRuntime），本 crate 不直连 spawn。

use pawork_domain::{MonitorId, MonitorSourceKind, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Monitor 配置：按来源携带具体参数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MonitorConfig {
    /// 文件变化：监听一组路径，可选正则过滤。
    FileChange {
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
    },
    /// 进程退出：按 OS pid 或 task_id 监视（至少给一个）。
    ProcessExit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
    /// 正则匹配：在 stream 引用的文本上做模式匹配。
    RegexMatch { stream: String, pattern: String },
    /// 端口状态：监听 host:port 是否处于监听态。
    PortState { host: String, port: u16 },
}

impl MonitorConfig {
    /// 与本配置一致的 canonical 来源种类。
    pub fn source_kind(&self) -> MonitorSourceKind {
        match self {
            Self::FileChange { .. } => MonitorSourceKind::FileChange,
            Self::ProcessExit { .. } => MonitorSourceKind::ProcessExit,
            Self::RegexMatch { .. } => MonitorSourceKind::RegexMatch,
            Self::PortState { .. } => MonitorSourceKind::PortState,
        }
    }

    /// 校验配置自洽（正则可编译 / 必填项存在）。注册时调用。
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::FileChange { pattern, .. } => {
                if let Some(pattern) = pattern {
                    regex::Regex::new(pattern)
                        .map_err(|err| format!("invalid file pattern: {err}"))?;
                }
                Ok(())
            }
            Self::ProcessExit { pid, task_id } => {
                if pid.is_none() && task_id.is_none() {
                    return Err("process_exit requires pid or task_id".into());
                }
                Ok(())
            }
            Self::RegexMatch { stream, pattern } => {
                if stream.is_empty() {
                    return Err("regex_match requires a non-empty stream".into());
                }
                regex::Regex::new(pattern).map_err(|err| format!("invalid regex: {err}"))?;
                Ok(())
            }
            Self::PortState { host, port } => {
                if host.is_empty() {
                    return Err("port_state requires a non-empty host".into());
                }
                if *port == 0 {
                    return Err("port_state requires a non-zero port".into());
                }
                Ok(())
            }
        }
    }
}

/// 声明式 Monitor：ID + canonical 来源 + 配置 + 可选 workspace。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    pub monitor_id: MonitorId,
    pub source: MonitorSourceKind,
    pub config: MonitorConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
}

impl Monitor {
    /// 由配置推导 source，构造 Monitor（不带 workspace）。
    pub fn new(monitor_id: impl Into<MonitorId>, config: MonitorConfig) -> Self {
        let source = config.source_kind();
        Self {
            monitor_id: monitor_id.into(),
            source,
            config,
            workspace_id: None,
        }
    }

    /// 链式附加 workspace。
    pub fn with_workspace(mut self, workspace_id: impl Into<WorkspaceId>) -> Self {
        self.workspace_id = Some(workspace_id.into());
        self
    }
}

/// 注入的观测样本，是确定性 evaluate 的唯一输入。driver 把真实外部事件归一为
/// Observation 后喂入判定核心，保证命中逻辑可独立单测。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    /// 文件变化：发生变更的路径列表。
    FileChange { paths: Vec<String> },
    /// 进程退出：匹配用的 pid / task_id + 退出码。
    ProcessExit {
        pid: Option<u32>,
        task_id: Option<String>,
        code: Option<i32>,
    },
    /// 输出流文本样本：在 stream 的 text 上做正则匹配。
    RegexMatch { stream: String, text: String },
    /// 端口探测结果：open=true 表示监听态。
    PortState { host: String, port: u16, open: bool },
}
