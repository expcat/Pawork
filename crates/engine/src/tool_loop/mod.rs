//! 多轮工具循环：在 [`crate::run_turn`] 之上收集 tool call、经 [`LoopContext`]
//! 执行、回填 Tool 消息，直到本轮没有 tool call 或达到轮数上限。

mod approval;
mod compaction;
mod exec;
mod round;

use std::sync::atomic::AtomicU64;

use async_trait::async_trait;
use pawork_domain::{
    AgentEvent, ApprovalDecision, ArtifactId, CancellationToken, CheckpointId, ErrorCategory,
    ErrorContext, EventSequence, MessageId, RequestId, TokenUsage, ToolCallId,
};
use pawork_domain::{CanonicalModelRequest, ModelProvider, ModelResponseSummary, ProviderError};

use crate::appender::ToolCallResult;
use crate::context::{AutoCompactionReason, TurnContext};
use crate::event::{AgentEventSink, EngineError, EventEmitter, LoopEventEmitter};
use crate::session_turn::SessionTurn;

use approval::{wait_and_apply, ApprovalWait};
pub use compaction::run_manual_compaction;
use compaction::{
    apply_context_limits, apply_injected_layers, estimate_input, injected_layers_details,
};
use exec::{pending_invocations, snapshot_execute_commit, ToolRound};
use round::{assistant_message, collect_stream_round, saturating_add_usage, StreamRound};

#[cfg(test)]
mod tests;

/// 每 run 默认最大工具轮数（防失控）。达到后事件化终止，不再开下一轮 stream。
pub const DEFAULT_MAX_TOOL_ROUNDS: u64 = 20;

/// 待执行的一次工具调用（解析自本轮 tool call）。
#[derive(Clone, Debug)]
pub struct PendingToolInvocation {
    pub tool_call_id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// 宿主对一次工具调用的审批闸门。
///
/// `NotRequired`：策略已放行，不发审批事件。`Asked`：用户可见审批，engine
/// 发 `ToolApprovalRequested/Responded` 事件对。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalGate {
    NotRequired,
    Asked(ApprovalDecision),
}

/// 写工具执行前由宿主拍下的快照标识。engine 只负责发 [`AgentEvent::CheckpointCreated`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteCheckpoint {
    pub checkpoint_id: CheckpointId,
    pub artifacts: Vec<ArtifactId>,
}

/// host 完成 session 侧 fork/snapshot 后回传的元数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactionOutcome {
    /// 被压缩区间覆盖的源事件条数。
    pub source_event_count: u64,
    /// 压缩落点（源分支被压缩到的 sequence）。
    pub compacted_through: EventSequence,
}

/// Agent Loop 执行中需要的回调（由调用方注入）。
///
/// 审批经 [`LoopContext::request_approval`] 注入；engine 不依赖 policy/tools。
#[async_trait]
pub trait LoopContext: Send + Sync {
    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        events: LoopEventEmitter<'_>,
        cancel: CancellationToken,
    ) -> Vec<ToolCallResult>;

    /// 对一批待执行调用给出闸门。`already_approved_for_run` 为 true 时不应再询问。
    ///
    /// 实现必须在每次阻塞等待决策前 emit [`AgentEvent::ToolApprovalRequested`]
    /// （含 batch 已批准的短路路径），reason 格式逐字
    /// tool `{name}` requires approval。
    async fn request_approval(
        &self,
        calls: &[PendingToolInvocation],
        already_approved_for_run: bool,
        events: LoopEventEmitter<'_>,
        cancel: CancellationToken,
    ) -> Result<Vec<ApprovalGate>, EngineError>;

    fn next_message_id(&self) -> MessageId;

    fn next_request_id(&self) -> RequestId;

    /// 压缩回调：host（app）负责 session 侧 fork/snapshot，完成后回传元数据。
    /// 默认实现返回 `Ok(None)`（无持久化宿主时 engine 仍能完成消息层压缩）。
    /// 宿主侧失败必须显式返回 `Err`（映射为 sink 错误），不得静默吞掉。
    async fn compact_history(
        &self,
        _reason: AutoCompactionReason,
        _summary_text: &str,
        _cancel: CancellationToken,
    ) -> Result<Option<CompactionOutcome>, EngineError> {
        Ok(None)
    }

    /// 写工具执行前由宿主拍快照。engine 不依赖 blob/git；默认空。
    /// 快照失败时宿主可经 `events` 发 `AgentEvent::Diagnostic`（写入继续）。
    async fn snapshot_write_tools(
        &self,
        _calls: &[PendingToolInvocation],
        _events: LoopEventEmitter<'_>,
        _cancel: CancellationToken,
    ) -> Vec<WriteCheckpoint> {
        Vec::new()
    }
}

/// 多轮事件化：先发 `RunStarted` 与用户 `MessageCommitted`，再循环
/// provider → 组装助手 →（可选）执行工具并回填，直到无 tool call 或超限。
///
/// 是否继续以 [`crate::AssembledTurn::has_tool_calls`] 为准，不看 `StopReason`。
/// persist 失败时不再补终态。AskUser 才发审批事件对。不按 Provider 名称分支。
pub async fn run_session(
    provider: &dyn ModelProvider,
    request: CanonicalModelRequest,
    turn: SessionTurn,
    events: &dyn AgentEventSink,
    cancel: CancellationToken,
    loop_ctx: &dyn LoopContext,
    max_tool_rounds: u64,
    context: TurnContext,
) -> Result<ModelResponseSummary, EngineError> {
    if turn.start_sequence == 0 {
        return Err(EngineError::sink(
            "start_sequence must be >= 1 (session_events CHECK)",
        ));
    }

    let next_sequence = AtomicU64::new(turn.start_sequence);
    let emitter = EventEmitter::new(
        turn.session_id.clone(),
        turn.run_id.clone(),
        &next_sequence,
        turn.timestamp,
        events,
    );
    let loop_events = LoopEventEmitter::new(emitter.clone());
    let trigger_id = turn.trigger_message.id.clone();

    emitter
        .emit(AgentEvent::RunStarted {
            trigger_message_id: trigger_id,
        })
        .await?;
    emitter
        .emit(AgentEvent::MessageCommitted {
            message: turn.trigger_message.clone(),
        })
        .await?;

    if cancel.is_cancelled() {
        return emit_cancelled(&emitter, "turn cancelled", &TokenUsage::default()).await;
    }

    let mut current = apply_injected_layers(request, &context.injected_layers);
    if !context.injected_layers.is_empty() {
        emitter
            .emit(AgentEvent::Diagnostic {
                code: "resources.injected".into(),
                details: injected_layers_details(&context.injected_layers),
            })
            .await?;
    }
    let mut tool_rounds = 0_u64;
    let mut run_usage = TokenUsage::default();
    let mut run_approved = false;

    loop {
        if cancel.is_cancelled() {
            return emit_cancelled(&emitter, "turn cancelled", &run_usage).await;
        }

        // S5：每轮请求前估算输入 token 并发 ContextPrepared（estimator 未配置时
        // 保持 estimated=0 现状），随后按预算收敛消息集（软限压缩 / 硬限截断）。
        let mut estimate = estimate_input(&context, &current);
        emitter
            .emit(AgentEvent::ContextPrepared {
                message_count: current.messages.len() as u64,
                estimated_input_tokens: estimate.estimated_input_tokens,
            })
            .await?;
        apply_context_limits(
            provider,
            &emitter,
            loop_ctx,
            &turn.model,
            &context,
            &mut current,
            &mut estimate,
            cancel.clone(),
        )
        .await?;

        emitter
            .emit(AgentEvent::ProviderRequestStarted {
                request_id: current.request_id.clone(),
                provider_id: turn.provider_id.clone(),
                model: turn.model.as_str().to_string(),
            })
            .await?;

        let assistant_id = loop_ctx.next_message_id();
        match collect_stream_round(
            provider,
            current.clone(),
            &emitter,
            assistant_id,
            cancel.clone(),
        )
        .await?
        {
            StreamRound::Succeeded { assembled, summary } => {
                let invocations = pending_invocations(&assembled);
                let has_tool_calls = assembled.has_tool_calls();
                let assistant =
                    assistant_message(assembled, &summary, &turn.provider_id, &turn.model);
                emitter
                    .emit(AgentEvent::MessageCommitted {
                        message: assistant.clone(),
                    })
                    .await?;

                run_usage = saturating_add_usage(&run_usage, &summary.usage);

                if !has_tool_calls {
                    let mut completed = summary;
                    completed.usage = run_usage.clone();
                    emitter
                        .emit(AgentEvent::RunCompleted {
                            stop_reason: completed.stop_reason.clone(),
                            usage: completed.usage.clone(),
                        })
                        .await?;
                    return Ok(completed);
                }

                match wait_and_apply(
                    loop_ctx,
                    &invocations,
                    &mut run_approved,
                    loop_events.clone(),
                    &emitter,
                    cancel.clone(),
                )
                .await?
                {
                    ApprovalWait::Cancelled => {
                        return emit_cancelled(&emitter, "turn cancelled", &run_usage).await;
                    }
                    ApprovalWait::Ready(plan) => {
                        match snapshot_execute_commit(
                            loop_ctx,
                            &invocations,
                            plan.to_run,
                            plan.decided,
                            loop_events.clone(),
                            &emitter,
                            cancel.clone(),
                        )
                        .await?
                        {
                            ToolRound::Cancelled => {
                                return emit_cancelled(&emitter, "turn cancelled", &run_usage)
                                    .await;
                            }
                            ToolRound::Committed(tool_message) => {
                                current.messages.push(assistant);
                                current.messages.push(tool_message);
                                current.request_id = loop_ctx.next_request_id();

                                tool_rounds += 1;
                                if tool_rounds >= max_tool_rounds {
                                    let message =
                                        format!("maximum tool rounds exceeded ({max_tool_rounds})");
                                    emitter
                                        .emit(AgentEvent::RunFailed {
                                            error: ErrorContext {
                                                category: ErrorCategory::ResourceExhausted,
                                                message,
                                                retryable: false,
                                                retry_after_ms: None,
                                                diagnostics: Default::default(),
                                            },
                                            usage: optional_usage(&run_usage),
                                        })
                                        .await?;
                                    return Err(EngineError::MaxToolRounds(max_tool_rounds));
                                }
                            }
                        }
                    }
                }
            }
            StreamRound::Cancelled {
                message,
                stream_usage,
            } => {
                run_usage = saturating_add_usage(&run_usage, &stream_usage);
                return emit_cancelled(&emitter, message, &run_usage).await;
            }
            StreamRound::Failed {
                error,
                stream_usage,
            } => {
                run_usage = saturating_add_usage(&run_usage, &stream_usage);
                let context = ErrorContext::from(error.clone());
                emitter
                    .emit(AgentEvent::RunFailed {
                        error: context,
                        usage: optional_usage(&run_usage),
                    })
                    .await?;
                return Err(error.into());
            }
        }
    }
}

async fn emit_cancelled(
    emitter: &EventEmitter<'_>,
    reason: impl Into<String>,
    usage: &TokenUsage,
) -> Result<ModelResponseSummary, EngineError> {
    let reason = reason.into();
    emitter
        .emit(AgentEvent::RunCancelled {
            reason: Some(reason.clone()),
            usage: optional_usage(usage),
        })
        .await?;
    Err(ProviderError::cancelled(reason).into())
}

fn optional_usage(usage: &TokenUsage) -> Option<TokenUsage> {
    if usage.is_zero() {
        None
    } else {
        Some(usage.clone())
    }
}
