//! Hook 能力声明。
//!
//! [`HookCapability`] 描述 handler 请求的外部副作用类别，供注入的
//! [`crate::PolicyGate`] 做 capability 门控。它与 `agent_domain::ToolCapability`
//! 语义对齐但聚焦于 user hook 场景，避免把 hook 强行套进通用工具能力枚举。

use serde::{Deserialize, Serialize};

/// 一条 user hook 在执行时请求的能力。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookCapability {
    /// 经 Sandbox→Process 执行外部命令（Command handler）。
    Process,
    /// 发起网络请求（Http handler）。
    Network,
    /// 改写 Agent 输入 prompt（PromptTransform handler）。
    PromptTransform,
    /// 调用模型做 hook 判定（PromptEval handler）。
    PromptEval,
    /// 用受限 Agent 做 hook 判定（AgentEval handler）。
    AgentEval,
    /// 调用 MCP tool（McpTool handler）。
    McpTool,
}

impl HookCapability {
    /// 默认 lifecycle：通知类 handler（Command/Http）默认 async fire-and-forget；
    /// 需要回灌结果的 handler（PromptTransform/PromptEval/AgentEval/McpTool）默认同步阻断。
    pub const fn default_lifecycle(self) -> crate::config::HandlerLifecycle {
        use crate::config::HandlerLifecycle;
        match self {
            Self::Process | Self::Network => HandlerLifecycle::Async,
            Self::PromptTransform | Self::PromptEval | Self::AgentEval | Self::McpTool => {
                HandlerLifecycle::Sync
            }
        }
    }

    /// 该能力是否允许改写 Agent 输入（仅 PromptTransform）。
    pub const fn can_rewrite_prompt(self) -> bool {
        matches!(self, Self::PromptTransform)
    }
}
