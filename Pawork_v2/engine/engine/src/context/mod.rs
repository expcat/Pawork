//! 上下文预算与用量（S5，自 V1 `context-engine` 迁入）。
//!
//! 迁移范围刻意收窄为四个文件：预算（`budget`）、压缩触发判定（`compaction`）、
//! token 估算口径（`token`）、tool result 分级裁剪（`tool_result_trim`）。
//! V1 的 14 源 `ContextBuilder`（`resources.rs` / `source.rs` / `builder.rs`）
//! **不迁移**：V2 的上下文组装由 `run_session` 的消息历史 + 工具 schema 直接构成，
//! 多来源贡献排序与资源挂载属 S8 资源阶段的工作，届时按需引入，避免无消费者库存。

pub mod budget;
pub mod compaction;
pub mod token;
pub mod tool_result_trim;

use std::sync::Arc;

pub use budget::{ContextBudget, ContextBudgetBreakdown};
pub use compaction::{
    compute_compaction, AutoCompactionReason, CompactionReason, CompactionTrigger,
};
pub use token::{HeuristicEstimator, TokenEstimator, ToolSchema};
pub(crate) use token::reply_primer_tokens;
pub use tool_result_trim::{
    byte_len_of_tool_result, trim_tool_result, trim_tool_result_with, ResultSize, TrimStrategy,
    TrimThresholds, TrimmedToolResult,
};

/// 单轮上下文限制：输入硬预算 + 历史软限。
pub struct ContextLimits {
    pub budget: ContextBudget,
    pub history_soft_limit_tokens: Option<u64>,
}

/// `run_session` 的上下文配置。
///
/// [`TurnContext::default`] 全禁用（limits/estimator 为 None、retained 4），
/// 行为与 S5 接线前完全一致：不估算（`estimated_input_tokens = 0`）、不压缩、不截断。
pub struct TurnContext {
    pub limits: Option<ContextLimits>,
    pub estimator: Option<Arc<dyn TokenEstimator>>,
    pub retained_messages: usize,
}

impl Default for TurnContext {
    fn default() -> Self {
        Self {
            limits: None,
            estimator: None,
            retained_messages: 4,
        }
    }
}
