//! Pawork Agent 循环运行时。
//!
//! 本 crate 实现完整 Agent 循环：Run 状态机、Provider Loop、消息队列、预算控制、
//! 重试、取消、事件广播与中断恢复。详见各子模块与 `docs/features/agent-engine.md`。

mod appender;
mod broadcast;
mod budget;
mod cancel;
mod provider_loop;
mod queue;
mod recovery;
mod retry;
mod state;

pub use appender::{tool_results_message, AssembledTurn, PendingToolCall, ToolCallResult};
pub use broadcast::{BroadcastError, EventBroadcaster, Subscriber, DEFAULT_BROADCAST_CAPACITY};
pub use budget::{
    BudgetController, BudgetDimension, BudgetLimits, BudgetReport, BudgetUsage,
    ExternalQuotaSignal, QuotaSignalConfidence, DEFAULT_SOFT_RATIO,
};
pub use cancel::{
    CancelHandle, CancelReason, CancelReceipt, NoopProcessTreeCleaner, ProcessTreeCleaner,
};
pub use provider_loop::{
    ApprovalOutcome, LoopContext, LoopError, LoopEventEmitter, PendingToolInvocation, ProviderLoop,
    ProviderLoopConfig, ProviderTranscriptInvocation, SchedulerLoopContext, TurnOutcome,
};
pub use queue::{MessageQueue, MessageQueueSnapshot, QueuedMessage};
pub use recovery::{
    group_by_run, replay_run, scan_interrupted, RecoveryIssue, RecoveryPlan, RunEventLog,
};
pub use retry::{RetryAttempt, RetryController, RetryDecision, RetryPolicy, RetryReason};
pub use state::{
    event_hint, transition, EventHint, RunState, RunStateMachine, RunTransition, TransitionError,
};
