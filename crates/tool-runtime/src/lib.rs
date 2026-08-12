//! Pawork Tool 调度器（P3-4）。
//!
//! 基于 `tool-api` 的 capability 分类实现只读并发、写/Shell 串行、同文件串行、
//! Git index 串行与审批暂停，所有调用可取消。调度策略见 `docs/architecture/control-flow.md` §5。

mod scheduler;
#[cfg(feature = "tool-search")]
mod tool_search;

pub use scheduler::{
    extract_file_key, ApprovalMode, ApprovalOutcome, ApprovalResolver, ApprovalState,
    AutoApproveResolver, NoopToolEventSink, ProviderCallDispatch, SchedulingKey, ToolRegistry,
    ToolRegistryError, ToolScheduler, ToolSchedulerConfig, ToolSchedulerError,
};
#[cfg(feature = "tool-search")]
pub use tool_search::{
    ActivationApprovalResolver, ActivationDenied, AutoActivationApproval, LazyToolIndex,
    PolicyActivationGate, ToolActivation, ToolActivationGate, ToolIndexConfig, ToolIndexError,
    ToolManifest, ToolMatch, ToolSource, ToolTokenBudget, HEURISTIC_CHARS_PER_TOKEN,
    TOOL_SCHEMA_FRAMING_TOKENS,
};
