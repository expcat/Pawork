//! Pawork 确定性上下文构建与 Token 预算分配。
//!
//! 按来源优先级组合上下文，估算并分配 Token 预算，为输出与思考预留空间，并在
//! 超限前触发压缩。详见 `docs/features/context.md`。
//!
//! 关键类型：
//! - [`ContextBuilder`] / [`BuiltContext`]：确定性组装消息并产出预算与超限信号。
//! - [`ContextSource`] / [`ContextContribution`]：14 项上下文来源与稳定排序。
//! - [`TokenEstimator`]（[`TiktokenEstimator`] / [`HeuristicEstimator`]）：Token 估算。
//! - [`ContextBudget`]：输出/思考预留与输入硬上限。
//! - [`CompactionTrigger`]：超限触发信号（不在此 crate 执行压缩）。
//! - [`trim_tool_result`] / [`TrimThresholds`] / [`TrimmedToolResult`]：Tool Result 分级裁剪（P5-7）。

mod budget;
mod builder;
mod compaction;
mod error;
mod source;
mod token;
mod tool_result_trim;

pub use budget::{ContextBudget, ContextBudgetBreakdown};
pub use builder::{BuiltContext, ContextBuilder};
pub use compaction::{CompactionReason, CompactionTrigger};
pub use error::ContextBuildError;
pub use source::{sort_contributions, ContextContribution, ContextSource};
pub use token::{
    default_estimator_for, HeuristicEstimator, TiktokenEstimator, TokenEstimator, ToolSchema,
};
pub use tool_result_trim::{
    byte_len_of_tool_result, trim_tool_result, trim_tool_result_with, ResultSize, TrimStrategy,
    TrimThresholds, TrimmedToolResult,
};
