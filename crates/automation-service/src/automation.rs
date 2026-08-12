//! Automation 定义：触发器配置与动作配置。
//!
//! 这些是 automation-service 的命令面配置类型（可序列化），用于注册 automation。
//! canonical 事件载荷（`AutomationEvent::Registered` 只携带 [`AutomationTriggerKind`]）
//! 是轻量「事实」；完整配置由 service 在命令侧持有，重放时按需重新注册。

use agent_domain::{AutomationId, AutomationTriggerKind, TaskKind};
use serde::{Deserialize, Serialize};

/// 触发器实际配置。对应四种 [`AutomationTriggerKind`]。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationTrigger {
    /// 五字段 cron 表达式（自实现最小子集，见 [`crate::cron`]）。
    Cron { expr: String },
    /// 固定间隔（秒）。
    Interval { secs: u64 },
    /// 一次性延时（秒）；触发一次后不再触发。
    Once { delay_secs: u64 },
    /// 订阅 canonical event 载荷，按正则 `pattern` 做模式匹配。
    Event { pattern: String },
}

impl AutomationTrigger {
    /// 对应的 canonical 触发器种类。
    pub fn kind(&self) -> AutomationTriggerKind {
        match self {
            AutomationTrigger::Cron { .. } => AutomationTriggerKind::Cron,
            AutomationTrigger::Interval { .. } => AutomationTriggerKind::Interval,
            AutomationTrigger::Once { .. } => AutomationTriggerKind::Once,
            AutomationTrigger::Event { .. } => AutomationTriggerKind::Event,
        }
    }

    /// 是否为时间驱动触发器（参与 `check_due`）。
    pub fn is_time_based(&self) -> bool {
        matches!(
            self,
            AutomationTrigger::Cron { .. }
                | AutomationTrigger::Interval { .. }
                | AutomationTrigger::Once { .. }
        )
    }
}

/// 触发后执行的动作。automation-service 只负责按动作派发为 background task；
/// 具体执行语义（prompt 生成 / 工具调用）由后台任务系统落地，service 不自带特权。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationAction {
    /// 以一段 prompt 启动后台 agent 任务。
    Prompt { prompt: String },
    /// 调用一个工具（`input` 为不透明 JSON 字符串，由执行端解析）。
    ToolCall { name: String, input: String },
    /// 启动指定种类的后台任务（默认派发为 `TaskKind::Automation`）。
    StartBackgroundTask { task_kind: TaskKind },
}

/// 一条 automation：ID + 触发器配置 + 动作。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    pub automation_id: AutomationId,
    pub trigger: AutomationTrigger,
    pub action: AutomationAction,
}
