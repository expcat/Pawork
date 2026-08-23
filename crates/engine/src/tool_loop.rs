//! 多轮工具循环：在 [`crate::run_turn`] 之上收集 tool call、经 [`LoopContext`]
//! 执行、回填 Tool 消息，直到本轮没有 tool call 或达到轮数上限。

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;

use async_trait::async_trait;
use pawork_domain::{
    CanonicalModelRequest, ModelProvider, ModelResponseSummary, ProviderError, ProviderEventSink,
    ProviderStreamEvent, ToolResult,
};
use pawork_domain::{
    AgentEvent, ApprovalDecision, ArtifactId, CancellationToken, CheckpointId, ContentPart,
    ErrorCategory, ErrorContext, EventSequence, Message, MessageId, MessageMetadata, MessageRole,
    ModelId, RequestId, TextContent, TokenUsage, ToolCallId, ToolResultContent,
};

use crate::appender::{tool_results_message, AssembledTurn, ToolCallResult};
use crate::context::{
    compute_compaction, reply_primer_tokens, AutoCompactionReason, ContextBudgetBreakdown,
    InjectedLayer, ToolSchema, TokenEstimator, TurnContext,
};
use crate::event::{AgentEventSink, EngineError, EventEmitter, LoopEventEmitter, LoopSink};
use crate::session_turn::SessionTurn;
use crate::run_turn;

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
    async fn snapshot_write_tools(
        &self,
        _calls: &[PendingToolInvocation],
        _cancel: CancellationToken,
    ) -> Vec<WriteCheckpoint> {
        Vec::new()
    }
}

/// 多轮事件化：先发 `RunStarted` 与用户 `MessageCommitted`，再循环
/// provider → 组装助手 →（可选）执行工具并回填，直到无 tool call 或超限。
///
/// 是否继续以 [`AssembledTurn::has_tool_calls`] 为准，不看 `StopReason`。
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
        let sink = LoopSink::new(emitter.clone(), assistant_id.clone());
        let result = run_turn(provider, current.clone(), &sink, cancel.clone()).await;
        if let Some(error) = sink.take_persist_error() {
            return Err(error);
        }

        match result {
            Ok(summary) => {
                let mut assembled = AssembledTurn::new(assistant_id);
                for event in sink.drain_events() {
                    assembled.apply(&event);
                }
                assembled.summary = Some(summary.clone());

                let invocations = pending_invocations(&assembled);
                let has_tool_calls = assembled.has_tool_calls();
                let assistant = assembled.into_message(MessageMetadata {
                    usage: Some(summary.usage.clone()),
                    stop_reason: Some(summary.stop_reason.clone()),
                    provider: Some(turn.provider_id.clone()),
                    model: Some(turn.model.clone()),
                    ..MessageMetadata::default()
                });
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

                let gates = loop_ctx
                    .request_approval(
                        &invocations,
                        run_approved,
                        loop_events.clone(),
                        cancel.clone(),
                    )
                    .await?;
                if cancel.is_cancelled() {
                    return emit_cancelled(&emitter, "turn cancelled", &run_usage).await;
                }
                let mismatch = gates.len() != invocations.len();
                let (to_run, mut decided) = if mismatch {
                    let mut decided = BTreeMap::new();
                    for invocation in &invocations {
                        decided.insert(
                            invocation.tool_call_id.clone(),
                            ApprovalDecision::Denied,
                        );
                    }
                    (Vec::new(), decided)
                } else {
                    apply_approval_gates(&invocations, &gates, &mut run_approved)
                };
                for invocation in &invocations {
                    if let Some(decision) = decided.get(&invocation.tool_call_id) {
                        if mismatch {
                            emitter
                                .emit(AgentEvent::ToolApprovalRequested {
                                    tool_call_id: invocation.tool_call_id.clone(),
                                    reason: format!("tool `{}` requires approval", invocation.name),
                                })
                                .await?;
                        }
                        emitter
                            .emit(AgentEvent::ToolApprovalResponded {
                                tool_call_id: invocation.tool_call_id.clone(),
                                decision: decision.clone(),
                                comment: None,
                            })
                            .await?;
                    }
                }

                let checkpoints = loop_ctx
                    .snapshot_write_tools(&to_run, cancel.clone())
                    .await;
                for checkpoint in checkpoints {
                    emitter
                        .emit(AgentEvent::CheckpointCreated {
                            checkpoint_id: checkpoint.checkpoint_id,
                            artifacts: checkpoint.artifacts,
                        })
                        .await?;
                }

                for invocation in &to_run {
                    emitter
                        .emit(AgentEvent::ToolExecutionStarted {
                            tool_call_id: invocation.tool_call_id.clone(),
                        })
                        .await?;
                }

                let raw = if to_run.is_empty() {
                    Vec::new()
                } else {
                    loop_ctx
                        .execute_tools(to_run, loop_events.clone(), cancel.clone())
                        .await
                };
                if cancel.is_cancelled() {
                    return emit_cancelled(&emitter, "turn cancelled", &run_usage).await;
                }
                for result in &raw {
                    decided.remove(&result.tool_call_id);
                }
                let mut merged = raw;
                merged.extend(decided.into_iter().filter_map(|(id, decision)| {
                    if matches!(
                        decision,
                        ApprovalDecision::Denied | ApprovalDecision::Cancelled
                    ) {
                        invocations
                            .iter()
                            .find(|call| call.tool_call_id == id)
                            .map(denied_tool_result)
                    } else {
                        None
                    }
                }));
                let results = align_tool_results(&invocations, merged);

                for result in &results {
                    let content = tool_result_content(result);
                    emitter
                        .emit(AgentEvent::ToolExecutionCompleted {
                            tool_call_id: result.tool_call_id.clone(),
                            result: content.clone(),
                        })
                        .await?;
                    if let Some(details) = sandbox_fallback_details(&content.metadata) {
                        emitter
                            .emit(AgentEvent::Diagnostic {
                                code: "sandbox.fallback".into(),
                                details,
                            })
                            .await?;
                    }
                }

                let tool_message =
                    tool_results_message(loop_ctx.next_message_id(), results);
                emitter
                    .emit(AgentEvent::MessageCommitted {
                        message: tool_message.clone(),
                    })
                    .await?;

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
            Err(error) if error.kind == pawork_domain::ProviderErrorKind::Cancelled => {
                let stream_usage = last_stream_usage(&sink.drain_events());
                run_usage = saturating_add_usage(&run_usage, &stream_usage);
                return emit_cancelled(&emitter, error.message.clone(), &run_usage).await;
            }
            Err(error) => {
                let stream_usage = last_stream_usage(&sink.drain_events());
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

/// 一轮请求的输入估算：system / 工具 schema / 历史与总量的 token 计数。
struct InputEstimate {
    system_prompt_tokens: u64,
    tool_schema_tokens: u64,
    history_tokens: u64,
    estimated_input_tokens: u64,
}

const INJECTED_SYSTEM_ID: &str = "msg-resources";

/// 把资源层拼成一条 System 消息，插到请求最前。不改冻结请求结构。
fn apply_injected_layers(
    mut request: CanonicalModelRequest,
    layers: &[InjectedLayer],
) -> CanonicalModelRequest {
    ensure_injected_prefix(&mut request.messages, layers);
    request
}

fn injected_system_message(layers: &[InjectedLayer]) -> Option<Message> {
    if layers.is_empty() {
        return None;
    }
    let text = layers
        .iter()
        .map(|layer| {
            format!(
                "[{kind}] {id}\n{content}",
                kind = layer.kind,
                id = layer.resource_id,
                content = layer.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(Message {
        id: MessageId::from(INJECTED_SYSTEM_ID),
        role: MessageRole::System,
        content: vec![ContentPart::Text(TextContent { text })],
        metadata: Default::default(),
    })
}

fn ensure_injected_prefix(messages: &mut Vec<Message>, layers: &[InjectedLayer]) {
    let Some(system) = injected_system_message(layers) else {
        return;
    };
    messages.retain(|message| message.id.as_str() != INJECTED_SYSTEM_ID);
    messages.insert(0, system);
}

fn injected_layers_details(layers: &[InjectedLayer]) -> serde_json::Value {
    serde_json::json!({
        "layers": layers.iter().map(|layer| {
            serde_json::json!({
                "kind": layer.kind,
                "resource_id": layer.resource_id,
                "byte_len": layer.content.len(),
            })
        }).collect::<Vec<_>>(),
        "byte_len": layers.iter().map(|layer| layer.content.len()).sum::<usize>(),
    })
}

/// canonical ToolDefinition 转 context 侧可计数的 ToolSchema。
fn tool_schemas(request: &CanonicalModelRequest) -> Vec<ToolSchema> {
    request
        .tools
        .iter()
        .map(|tool| ToolSchema {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        })
        .collect()
}

fn estimate_input_at(
    estimator: &dyn TokenEstimator,
    request: &CanonicalModelRequest,
) -> InputEstimate {
    let tool_schema_tokens = estimator.count_tool_schemas(&tool_schemas(request));
    let mut system_prompt_tokens = 0;
    let mut history_tokens = 0;
    for message in &request.messages {
        let tokens = estimator.count_message(message);
        if message.role == MessageRole::System {
            system_prompt_tokens += tokens;
        } else {
            history_tokens += tokens;
        }
    }
    InputEstimate {
        system_prompt_tokens,
        tool_schema_tokens,
        history_tokens,
        estimated_input_tokens: system_prompt_tokens
            + tool_schema_tokens
            + history_tokens
            + reply_primer_tokens(),
    }
}

/// estimator 未配置时返回全零（保持 S5 前现状：estimated_input_tokens = 0）。
fn estimate_input(context: &TurnContext, request: &CanonicalModelRequest) -> InputEstimate {
    match context.estimator.as_deref() {
        Some(estimator) => estimate_input_at(estimator, request),
        None => InputEstimate {
            system_prompt_tokens: 0,
            tool_schema_tokens: 0,
            history_tokens: 0,
            estimated_input_tokens: 0,
        },
    }
}

/// 触发判定与消息集收敛（compute_compaction 语义：硬限优先、软限次之）：
///
/// - 软限命中先走压缩链（摘要 + 事件三连 + 重建 summary 与 retained tail）；
/// - 压缩后仍超硬限、或软限未配但超硬限时，从最旧截断（永不丢最后
///   retained_messages 条），发 Diagnostic 并重发 ContextPrepared。
async fn apply_context_limits(
    provider: &dyn ModelProvider,
    emitter: &EventEmitter<'_>,
    loop_ctx: &dyn LoopContext,
    model: &ModelId,
    context: &TurnContext,
    current: &mut CanonicalModelRequest,
    estimate: &mut InputEstimate,
    cancel: CancellationToken,
) -> Result<(), EngineError> {
    let (Some(limits), Some(estimator)) =
        (context.limits.as_ref(), context.estimator.as_deref())
    else {
        return Ok(());
    };
    let breakdown = ContextBudgetBreakdown {
        system_prompt_tokens: estimate.system_prompt_tokens,
        tool_schema_tokens: estimate.tool_schema_tokens,
        attachment_tokens: 0,
        history_tokens: estimate.history_tokens,
        estimated_input_tokens: estimate.estimated_input_tokens,
        output_reserve_tokens: limits.budget.output_reserve_tokens,
        thinking_reserve_tokens: limits.budget.thinking_reserve_tokens,
        max_input_tokens: limits.budget.max_input_tokens,
    };
    let soft_hit = matches!(
        limits.history_soft_limit_tokens,
        Some(soft) if estimate.history_tokens > soft
    );
    if let Some(trigger) = compute_compaction(&breakdown, limits.history_soft_limit_tokens) {
        // 软限命中（含硬限同时超限）先压缩；纯硬限且软限未命中时压缩无收益，直接截断。
        if soft_hit {
            if let Some(rebuilt) = compact_messages(
                provider,
                emitter,
                loop_ctx,
                model,
                AutoCompactionReason::from(trigger.reason),
                &current.messages,
                context.retained_messages,
                cancel.clone(),
            )
            .await?
            {
                current.messages = rebuilt;
                ensure_injected_prefix(&mut current.messages, &context.injected_layers);
                *estimate = estimate_input_at(estimator, current);
            }
        }
    }
    if estimate.estimated_input_tokens > limits.budget.max_input_tokens {
        let (dropped, estimated_after) = truncate_for_budget(
            estimator,
            current,
            limits.budget.max_input_tokens,
            context.retained_messages,
        );
        *estimate = estimate_input_at(estimator, current);
        emitter
            .emit(AgentEvent::Diagnostic {
                code: "context_hard_truncated".into(),
                details: serde_json::json!({
                    "dropped_messages": dropped,
                    "estimated_input_tokens": estimated_after,
                }),
            })
            .await?;
        emitter
            .emit(AgentEvent::ContextPrepared {
                message_count: current.messages.len() as u64,
                estimated_input_tokens: estimated_after,
            })
            .await?;
    }
    Ok(())
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

fn last_stream_usage(events: &[ProviderStreamEvent]) -> TokenUsage {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            ProviderStreamEvent::UsageUpdated(usage) => Some(usage.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn sandbox_fallback_details(metadata: &serde_json::Value) -> Option<serde_json::Value> {
    let sandbox = metadata.get("sandbox")?;
    if !sandbox.get("fallback")?.as_bool()? {
        return None;
    }
    let isolation = sandbox
        .get("isolation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let backend = sandbox
        .get("backend")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let note = sandbox
        .get("note")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let message = if note.is_empty() {
        format!("沙箱回退：isolation={isolation} backend={backend}")
    } else {
        format!("沙箱回退：isolation={isolation} backend={backend}（{note}）")
    };
    Some(serde_json::json!({
        "message": message,
        "isolation": isolation,
        "backend": backend,
        "note": note,
        "fallback": true,
    }))
}

/// 摘要请求的 User 指令：要求保留关键事实 / 约束 / 未完成工作。
const COMPACTION_SUMMARY_INSTRUCTION: &str =
    "请把以下对话历史压缩成一段摘要，保留关键事实、约束和未完成的工作，只输出摘要正文：";

/// 只累计 TextDelta 的收集 sink：摘要请求不向 AgentEventSink 转发、
/// 其 usage 也不计入 run_usage（内部请求，非本轮 provider 请求）。
struct SummaryTextSink(Mutex<String>);

#[async_trait]
impl ProviderEventSink for SummaryTextSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        if let ProviderStreamEvent::TextDelta(delta) = event {
            self.0.lock().expect("summary sink mutex").push_str(&delta);
        }
        Ok(())
    }
}

/// 生成被压缩区间的摘要：优先向 provider 发内部摘要请求（assemble_request，
/// 无 tools）；失败或空摘要时降级为结构性摘要。
async fn summarize_history(
    provider: &dyn ModelProvider,
    loop_ctx: &dyn LoopContext,
    model: &ModelId,
    compacted_range: &[Message],
    cancel: CancellationToken,
) -> String {
    let mut transcript = String::new();
    for message in compacted_range {
        let text = message_text(message);
        if text.is_empty() {
            continue;
        }
        if !transcript.is_empty() {
            transcript.push('\n');
        }
        transcript.push_str(&text);
    }
    let request = crate::assemble_request(
        loop_ctx.next_request_id(),
        model.clone(),
        vec![Message {
            id: MessageId::from("engine:compaction-prompt"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: format!("{COMPACTION_SUMMARY_INSTRUCTION}\n\n{transcript}"),
            })],
            metadata: Default::default(),
        }],
    );

    let sink = SummaryTextSink(Mutex::new(String::new()));
    // 注意：摘要请求的 usage 不计入 run_usage，也不进 AgentEventSink。
    if run_turn(provider, request, &sink, cancel).await.is_ok() {
        let text = sink.0.lock().expect("summary sink mutex").clone();
        if !text.trim().is_empty() {
            return text;
        }
    }
    structural_summary(compacted_range)
}

/// 降级摘要：被压缩区间首条用户消息截 2000 chars 加省略号，再接最近一条消息截 500 chars。
fn structural_summary(compacted: &[Message]) -> String {
    const FIRST_USER_MAX_CHARS: usize = 2000;
    const LAST_MESSAGE_MAX_CHARS: usize = 500;

    let mut summary = String::new();
    if let Some(first_user) = compacted
        .iter()
        .find(|message| message.role == MessageRole::User)
    {
        summary.push_str(truncate_chars(
            &message_text(first_user),
            FIRST_USER_MAX_CHARS,
        ));
        summary.push('…');
    }
    if let Some(last) = compacted.last() {
        summary.push_str(truncate_chars(&message_text(last), LAST_MESSAGE_MAX_CHARS));
    }
    summary
}

fn truncate_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 共用压缩链（自动软限 / 手动入口）：摘要、host compact_history 回调、
/// CompactionStarted / MessageCommitted(summary) / CompactionCompleted 事件三连，
/// 返回重建后的消息列表（summary + retained tail）。
///
/// 被压缩区间为空（消息数不超过 retained）时返回 None；source_event_count 取
/// host 回传值，无 outcome 时用被压缩消息数；compacted_through 无 outcome 时
/// 为 0（fail-safe：无持久化水位时不折叠任何已投影消息，摘要与尾部全保留）。
async fn compact_messages(
    provider: &dyn ModelProvider,
    emitter: &EventEmitter<'_>,
    loop_ctx: &dyn LoopContext,
    model: &ModelId,
    reason: AutoCompactionReason,
    messages: &[Message],
    retained_messages: usize,
    cancel: CancellationToken,
) -> Result<Option<Vec<Message>>, EngineError> {
    if messages.len() <= retained_messages {
        return Ok(None);
    }
    let split = messages.len() - retained_messages;
    let (compacted_range, retained) = messages.split_at(split);
    let summary_text =
        summarize_history(provider, loop_ctx, model, compacted_range, cancel.clone()).await;

    let outcome = loop_ctx
        .compact_history(reason, &summary_text, cancel)
        .await?;
    let source_event_count = outcome
        .as_ref()
        .map(|outcome| outcome.source_event_count)
        .unwrap_or_else(|| compacted_range.len() as u64);

    emitter
        .emit(AgentEvent::CompactionStarted { source_event_count })
        .await?;
    let summary = Message {
        id: loop_ctx.next_message_id(),
        role: MessageRole::User,
        content: vec![ContentPart::Text(TextContent {
            text: summary_text,
        })],
        metadata: Default::default(),
    };
    emitter
        .emit(AgentEvent::MessageCommitted {
            message: summary.clone(),
        })
        .await?;
    let compacted_through = outcome
        .map(|outcome| outcome.compacted_through)
        .unwrap_or_else(|| EventSequence::new(0));
    emitter
        .emit(AgentEvent::CompactionCompleted {
            summary_message_id: summary.id.clone(),
            compacted_through,
        })
        .await?;

    let mut rebuilt = Vec::with_capacity(retained_messages + 1);
    rebuilt.push(summary);
    rebuilt.extend(retained.iter().cloned());
    Ok(Some(rebuilt))
}

/// 硬限截断：从最旧开始丢弃消息，永不丢最后 retained_messages 条；
/// 返回（丢弃条数, 截断后估算）。
fn truncate_for_budget(
    estimator: &dyn TokenEstimator,
    request: &mut CanonicalModelRequest,
    max_input_tokens: u64,
    retained_messages: usize,
) -> (u64, u64) {
    let schema_tokens = estimator.count_tool_schemas(&tool_schemas(request));
    let estimate = |messages: &[Message]| -> u64 {
        schema_tokens
            + messages
                .iter()
                .map(|message| estimator.count_message(message))
                .sum::<u64>()
            + reply_primer_tokens()
    };
    let floor = retained_messages.min(request.messages.len());
    let mut dropped: u64 = 0;
    while estimate(&request.messages) > max_input_tokens && request.messages.len() > floor {
        let Some(idx) = request
            .messages
            .iter()
            .position(|message| message.role != MessageRole::System)
        else {
            break;
        };
        if request.messages.len().saturating_sub(idx) <= floor {
            break;
        }
        request.messages.remove(idx);
        dropped += 1;
    }
    (dropped, estimate(&request.messages))
}

/// 手动压缩入口（REPL /compact 等）：不是 run，不发 RunStarted / RunCancelled；
/// 事件序直接 CompactionStarted / MessageCommitted(summary) / CompactionCompleted
/// （reason 映射 Manual 语义，复用自动链同一内部函数），返回重建后的消息列表
/// （summary + retained tail）。
pub async fn run_manual_compaction(
    provider: &dyn ModelProvider,
    request: CanonicalModelRequest,
    turn: SessionTurn,
    events: &dyn AgentEventSink,
    cancel: CancellationToken,
    loop_ctx: &dyn LoopContext,
    context: TurnContext,
) -> Result<Vec<Message>, EngineError> {
    if turn.start_sequence == 0 {
        return Err(EngineError::sink(
            "start_sequence must be >= 1 (session_events CHECK)",
        ));
    }
    if request.messages.len() <= context.retained_messages {
        return Err(EngineError::sink(format!(
            "nothing to compact: {} message(s) <= retained {}",
            request.messages.len(),
            context.retained_messages
        )));
    }
    let next_sequence = AtomicU64::new(turn.start_sequence);
    let emitter = EventEmitter::new(
        turn.session_id.clone(),
        turn.run_id.clone(),
        &next_sequence,
        turn.timestamp,
        events,
    );
    compact_messages(
        provider,
        &emitter,
        loop_ctx,
        &turn.model,
        AutoCompactionReason::Manual,
        &request.messages,
        context.retained_messages,
        cancel,
    )
    .await
    .map(|rebuilt| rebuilt.expect("compaction range checked non-empty above"))
}

fn pending_invocations(assembled: &AssembledTurn) -> Vec<PendingToolInvocation> {
    assembled
        .tool_call_order
        .iter()
        .filter_map(|id| {
            assembled.tool_calls.get(id).map(|call| PendingToolInvocation {
                tool_call_id: id.clone(),
                name: call.name.clone(),
                arguments: call.arguments(),
            })
        })
        .collect()
}

fn apply_approval_gates(
    invocations: &[PendingToolInvocation],
    gates: &[ApprovalGate],
    run_approved: &mut bool,
) -> (
    Vec<PendingToolInvocation>,
    BTreeMap<ToolCallId, ApprovalDecision>,
) {
    let mut to_run = Vec::new();
    let mut decided = BTreeMap::new();
    for (index, invocation) in invocations.iter().enumerate() {
        let gate = gates
            .get(index)
            .cloned()
            .unwrap_or(ApprovalGate::NotRequired);
        match gate {
            ApprovalGate::NotRequired => to_run.push(invocation.clone()),
            ApprovalGate::Asked(decision) => {
                let decision = if *run_approved
                    && !matches!(
                        decision,
                        ApprovalDecision::Denied | ApprovalDecision::Cancelled
                    ) {
                    ApprovalDecision::ApprovedForRun
                } else {
                    decision.clone()
                };
                if matches!(decision, ApprovalDecision::ApprovedForRun) {
                    *run_approved = true;
                }
                decided.insert(invocation.tool_call_id.clone(), decision.clone());
                if matches!(
                    decision,
                    ApprovalDecision::ApprovedOnce | ApprovalDecision::ApprovedForRun
                ) {
                    to_run.push(invocation.clone());
                }
            }
        }
    }
    (to_run, decided)
}

fn denied_tool_result(invocation: &PendingToolInvocation) -> ToolCallResult {
    let mut result = ToolResult::failure(ErrorContext {
        category: ErrorCategory::Authorization,
        message: "tool call denied by user".into(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: Default::default(),
    });
    result.content = vec![ContentPart::Text(TextContent {
        text: "tool call denied by user".into(),
    })];
    ToolCallResult {
        tool_call_id: invocation.tool_call_id.clone(),
        tool_name: invocation.name.clone(),
        arguments: invocation.arguments.clone(),
        result,
    }
}

fn align_tool_results(
    invocations: &[PendingToolInvocation],
    results: Vec<ToolCallResult>,
) -> Vec<ToolCallResult> {
    let mut by_id: BTreeMap<ToolCallId, ToolCallResult> = results
        .into_iter()
        .map(|result| (result.tool_call_id.clone(), result))
        .collect();
    invocations
        .iter()
        .map(|invocation| {
            by_id
                .remove(&invocation.tool_call_id)
                .unwrap_or_else(|| ToolCallResult {
                    tool_call_id: invocation.tool_call_id.clone(),
                    tool_name: invocation.name.clone(),
                    arguments: invocation.arguments.clone(),
                    result: ToolResult::failure(ErrorContext {
                        category: ErrorCategory::NotFound,
                        message: "missing tool result".into(),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    }),
                })
        })
        .collect()
}

fn tool_result_content(result: &ToolCallResult) -> ToolResultContent {
    ToolResultContent {
        tool_call_id: result.tool_call_id.clone(),
        tool_name: Some(result.tool_name.clone()),
        content: result.result.content.clone(),
        is_error: result.result.is_error(),
        metadata: result.result.metadata.clone(),
        artifacts: result.result.artifacts.clone(),
    }
}

fn saturating_add_usage(acc: &TokenUsage, round: &TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: acc.input_tokens.saturating_add(round.input_tokens),
        output_tokens: acc.output_tokens.saturating_add(round.output_tokens),
        cache_read_tokens: acc
            .cache_read_tokens
            .saturating_add(round.cache_read_tokens),
        cache_write_tokens: acc
            .cache_write_tokens
            .saturating_add(round.cache_write_tokens),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use pawork_domain::{
        AgentTool, CanonicalModelRequest, ModelProvider, ModelResponseSummary, ProviderError,
        ProviderEventSink, ToolDefinition, ToolError, ToolErrorKind, ToolEventSink,
        ToolExecutionContext, ToolRequest, ToolResult, ToolStreamEvent,
    };
    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, ApprovalDecision, ArtifactId, ArtifactReference,
        CancellationToken, CheckpointId, ContentPart, ErrorCategory, EventId, EventSequence,
        Message, MessageId, MessageRole, ModelId, ProviderId, RequestId, RunId, SessionId,
        StopReason, TextContent, Timestamp, TokenUsage, ToolCallId, WorkspaceId,
    };
    use pawork_testkit::{MockProvider, MockScript, MockTool};

    use crate::assemble_request_with_tools;
    use crate::context::{ContextBudget, ContextLimits, HeuristicEstimator};
    use crate::event::{AgentEventSink, EngineError, LoopEventEmitter};
    use crate::session_turn::SessionTurn;

    use super::*;

    #[derive(Default)]
    struct RecordingEvents(Mutex<Vec<AgentEventEnvelope>>);

    impl RecordingEvents {
        fn snapshot(&self) -> Vec<AgentEventEnvelope> {
            self.0.lock().expect("events mutex").clone()
        }

        fn types(&self) -> Vec<&'static str> {
            self.snapshot()
                .into_iter()
                .map(|envelope| event_type(&envelope.payload))
                .collect()
        }
    }

    #[async_trait]
    impl AgentEventSink for RecordingEvents {
        async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
            self.0.lock().expect("events mutex").push(envelope);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct RecordingProvider {
        inner: MockProvider,
        requests: Arc<Mutex<Vec<CanonicalModelRequest>>>,
    }

    impl RecordingProvider {
        fn new(inner: MockProvider) -> Self {
            Self {
                inner,
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Vec<CanonicalModelRequest> {
            self.requests.lock().expect("requests mutex").clone()
        }
    }

    #[async_trait]
    impl ModelProvider for RecordingProvider {
        fn id(&self) -> ProviderId {
            self.inner.id()
        }

        async fn list_models(
            &self,
            credential: Option<&pawork_domain::ResolvedCredential>,
        ) -> Result<Vec<pawork_domain::ModelDefinition>, ProviderError> {
            self.inner.list_models(credential).await
        }

        async fn stream(
            &self,
            request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            self.requests
                .lock()
                .expect("requests mutex")
                .push(request.clone());
            self.inner.stream(request, sink, cancel).await
        }
    }

    struct TestContext {
        tools: Vec<MockTool>,
        msg_counter: AtomicU64,
        req_counter: AtomicU64,
    }

    impl TestContext {
        fn new(tools: Vec<MockTool>) -> Self {
            Self {
                tools,
                msg_counter: AtomicU64::new(0),
                req_counter: AtomicU64::new(0),
            }
        }
    }

    struct ForwardingSink<'a> {
        tool_call_id: ToolCallId,
        events: LoopEventEmitter<'a>,
    }

    #[async_trait]
    impl ToolEventSink for ForwardingSink<'_> {
        async fn emit(&self, event: ToolStreamEvent) -> Result<(), ToolError> {
            self.events
                .emit_tool_event(self.tool_call_id.clone(), event)
                .await
                .map_err(|error| ToolError {
                    kind: ToolErrorKind::Internal,
                    message: error.to_string(),
                    retryable: false,
                    retry_after_ms: None,
                })
        }
    }

    async fn execute_one(
        tools: &[MockTool],
        call: PendingToolInvocation,
        events: LoopEventEmitter<'_>,
        cancel: CancellationToken,
    ) -> ToolCallResult {
        let tool = tools
            .iter()
            .find(|tool| tool.descriptor().name == call.name)
            .cloned();
        let result = if let Some(tool) = tool {
            let request = ToolRequest {
                tool_call_id: call.tool_call_id.clone(),
                input: call.arguments.clone(),
            };
            let context = ToolExecutionContext {
                workspace_id: WorkspaceId::from("ws"),
                run_id: RunId::from("run"),
                working_directory: None,
            };
            let sink = ForwardingSink {
                tool_call_id: call.tool_call_id.clone(),
                events,
            };
            tool.execute(request, context, &sink, cancel)
                .await
                .unwrap_or_else(|error| ToolResult::failure(ErrorContext::from(error)))
        } else {
            ToolResult::failure(ErrorContext {
                category: ErrorCategory::NotFound,
                message: format!("unknown tool {}", call.name),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            })
        };
        ToolCallResult {
            tool_call_id: call.tool_call_id,
            tool_name: call.name,
            arguments: call.arguments,
            result,
        }
    }

    #[async_trait]
    impl LoopContext for TestContext {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            events: LoopEventEmitter<'_>,
            cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            let tools = self.tools.clone();
            let jobs = calls.into_iter().map(|call| {
                let events = events.clone();
                let tools = tools.clone();
                let cancel = cancel.clone();
                async move { execute_one(&tools, call, events, cancel).await }
            });
            futures::future::join_all(jobs).await
        }

        async fn request_approval(
            &self,
            calls: &[PendingToolInvocation],
            _already_approved_for_run: bool,
            _events: LoopEventEmitter<'_>,
            _cancel: CancellationToken,
        ) -> Result<Vec<ApprovalGate>, EngineError> {
            Ok(calls.iter().map(|_| ApprovalGate::NotRequired).collect())
        }

        fn next_message_id(&self) -> MessageId {
            let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
            MessageId::from(format!("msg-{n}"))
        }

        fn next_request_id(&self) -> RequestId {
            let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
            RequestId::from(format!("req-{n}"))
        }
    }

    fn user_hello() -> Message {
        Message {
            id: MessageId::from("msg-user"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "hello".into(),
            })],
            metadata: Default::default(),
        }
    }

    fn sample_turn() -> SessionTurn {
        SessionTurn {
            session_id: SessionId::from("ses-1"),
            run_id: RunId::from("run-1"),
            provider_id: ProviderId::from("mock"),
            model: ModelId::from("model-1"),
            start_sequence: 1,
            trigger_message: user_hello(),
            timestamp: Timestamp::from_unix_millis(1),
        }
    }

    fn echo_tool_def() -> ToolDefinition {
        ToolDefinition {
            name: "echo".into(),
            description: "echo".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn sample_request(tools: Vec<ToolDefinition>) -> CanonicalModelRequest {
        assemble_request_with_tools(
            RequestId::from("request-1"),
            ModelId::from("model-1"),
            vec![user_hello()],
            tools,
        )
    }

    fn event_type(event: &AgentEvent) -> &'static str {
        match event {
            AgentEvent::MessageCommitted { message } => match message.role {
                MessageRole::User => "MessageCommitted.user",
                MessageRole::Assistant => "MessageCommitted.assistant",
                MessageRole::Tool => "MessageCommitted.tool",
                MessageRole::System => "MessageCommitted.system",
            },
            AgentEvent::RunStarted { .. } => "RunStarted",
            AgentEvent::ContextPrepared { .. } => "ContextPrepared",
            AgentEvent::ProviderRequestStarted { .. } => "ProviderRequestStarted",
            AgentEvent::AssistantTextDelta { .. } => "AssistantTextDelta",
            AgentEvent::ToolCallStarted { .. } => "ToolCallStarted",
            AgentEvent::ToolCallArgumentsDelta { .. } => "ToolCallArgumentsDelta",
            AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested",
            AgentEvent::ToolApprovalResponded { .. } => "ToolApprovalResponded",
            AgentEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
            AgentEvent::ToolExecutionCompleted { .. } => "ToolExecutionCompleted",
            AgentEvent::ToolOutputDelta { .. } => "ToolOutputDelta",
            AgentEvent::UsageUpdated { .. } => "UsageUpdated",
            AgentEvent::RunCompleted { .. } => "RunCompleted",
            AgentEvent::RunCancelled { .. } => "RunCancelled",
            AgentEvent::RunFailed { .. } => "RunFailed",
            AgentEvent::CompactionStarted { .. } => "CompactionStarted",
            AgentEvent::CompactionCompleted { .. } => "CompactionCompleted",
            AgentEvent::CheckpointCreated { .. } => "CheckpointCreated",
            AgentEvent::CheckpointRolledBack { .. } => "CheckpointRolledBack",
            AgentEvent::Diagnostic { .. } => "Diagnostic",
            _ => "other",
        }
    }

    fn committed_roles(sink: &RecordingEvents) -> Vec<MessageRole> {
        sink.snapshot()
            .into_iter()
            .filter_map(|envelope| match envelope.payload {
                AgentEvent::MessageCommitted { message } => Some(message.role),
                _ => None,
            })
            .collect()
    }

    fn tool_messages(sink: &RecordingEvents) -> Vec<Message> {
        sink.snapshot()
            .into_iter()
            .filter_map(|envelope| match envelope.payload {
                AgentEvent::MessageCommitted { message }
                    if message.role == MessageRole::Tool =>
                {
                    Some(message)
                }
                _ => None,
            })
            .collect()
    }

    fn user_text(id: &str, text: &str) -> Message {
        Message {
            id: MessageId::from(id),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: text.into() })],
            metadata: Default::default(),
        }
    }

    fn numbered_messages(count: usize, body: &str) -> Vec<Message> {
        (0..count)
            .map(|n| user_text(&format!("msg-history-{n}"), &format!("turn {n}: {body}")))
            .collect()
    }

    fn request_with_messages(messages: Vec<Message>) -> CanonicalModelRequest {
        assemble_request_with_tools(
            RequestId::from("request-1"),
            ModelId::from("model-1"),
            messages,
            Vec::new(),
        )
    }

    fn turn_context(
        budget: ContextBudget,
        soft_limit: Option<u64>,
        retained_messages: usize,
    ) -> TurnContext {
        TurnContext {
            limits: Some(ContextLimits {
                budget,
                history_soft_limit_tokens: soft_limit,
            }),
            estimator: Some(Arc::new(HeuristicEstimator::default())),
            retained_messages,
            injected_layers: Vec::new(),
        }
    }

    fn estimate_request_tokens(request: &CanonicalModelRequest) -> u64 {
        let estimator = HeuristicEstimator::default();
        let schemas: Vec<ToolSchema> = request
            .tools
            .iter()
            .map(|tool| ToolSchema {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect();
        estimator.count_tool_schemas(&schemas)
            + request
                .messages
                .iter()
                .map(|message| estimator.count_message(message))
                .sum::<u64>()
            + crate::context::reply_primer_tokens()
    }

    fn context_prepared_events(sink: &RecordingEvents) -> Vec<(u64, u64)> {
        sink.snapshot()
            .into_iter()
            .filter_map(|envelope| match &envelope.payload {
                AgentEvent::ContextPrepared {
                    message_count,
                    estimated_input_tokens,
                } => Some((*message_count, *estimated_input_tokens)),
                _ => None,
            })
            .collect()
    }

    /// 摘要请求（无 tools）返回文本；主请求永远回 tool call，用于长对话仿真。
    struct GrowingProvider {
        requests: Arc<Mutex<Vec<CanonicalModelRequest>>>,
        calls: AtomicU64,
    }

    impl GrowingProvider {
        fn new() -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                calls: AtomicU64::new(0),
            }
        }

        fn requests(&self) -> Vec<CanonicalModelRequest> {
            self.requests.lock().expect("requests mutex").clone()
        }
    }

    #[async_trait]
    impl ModelProvider for GrowingProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        async fn list_models(
            &self,
            _credential: Option<&pawork_domain::ResolvedCredential>,
        ) -> Result<Vec<pawork_domain::ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            self.requests
                .lock()
                .expect("requests mutex")
                .push(request.clone());
            let stop_reason = if request.tools.is_empty() {
                sink.emit(ProviderStreamEvent::TextDelta(
                    "earlier work: fixing the build, constraint: stay pure rust".into(),
                ))
                .await?;
                StopReason::Completed
            } else {
                let n = self.calls.fetch_add(1, Ordering::Relaxed);
                let id = ToolCallId::from(format!("grow-call-{n}"));
                sink.emit(ProviderStreamEvent::ToolCallStarted {
                    id: id.clone(),
                    name: "grow".into(),
                })
                .await?;
                sink.emit(ProviderStreamEvent::ToolCallArgumentsDelta {
                    id: id.clone(),
                    json: "{}".into(),
                })
                .await?;
                sink.emit(ProviderStreamEvent::ToolCallCompleted { id }).await?;
                StopReason::ToolUse
            };
            sink.emit(ProviderStreamEvent::ResponseCompleted(stop_reason.clone()))
                .await?;
            Ok(ModelResponseSummary {
                stop_reason,
                usage: TokenUsage::default(),
                response_id: None,
                provider_metadata: Default::default(),
            })
        }
    }

    #[tokio::test]
    async fn mock_provider_completes_multi_turn_tool_loop() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("echo", serde_json::json!({"text": "hi"}))
                .usage(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 2,
                    cache_read_tokens: 100,
                    cache_write_tokens: 4,
                })
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .text("done")
                .usage(TokenUsage {
                    input_tokens: 20,
                    output_tokens: 1,
                    cache_read_tokens: 5,
                    cache_write_tokens: 8,
                })
                .complete(),
        ]));
        let echo = MockTool::new(
            "echo",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "hi".into(),
            })]),
        );
        let ctx = TestContext::new(vec![echo]);
        let sink = RecordingEvents::default();

        let summary = run_session(
            &provider,
            sample_request(vec![echo_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("multi-turn loop");

        assert_eq!(summary.stop_reason, StopReason::Completed);
        assert_eq!(summary.usage.input_tokens, 30);
        assert_eq!(summary.usage.output_tokens, 3);
        assert_eq!(summary.usage.cache_read_tokens, 105);
        assert_eq!(summary.usage.cache_write_tokens, 12);
        assert_eq!(
            committed_roles(&sink),
            vec![
                MessageRole::User,
                MessageRole::Assistant,
                MessageRole::Tool,
                MessageRole::Assistant,
            ]
        );
        let types = sink.types();
        assert!(types.contains(&"ToolExecutionStarted"));
        assert!(types.contains(&"ToolExecutionCompleted"));
        assert!(types.contains(&"MessageCommitted.tool"));
        assert!(types.contains(&"RunCompleted"));
        assert!(!types.contains(&"RunFailed"));
        assert!(!types.contains(&"ToolApprovalRequested"));

        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| message.role == MessageRole::Tool),
            "second-round request must include a Tool-role message"
        );
        assert_eq!(requests[1].tools, vec![echo_tool_def()]);
    }

    #[tokio::test]
    async fn parallel_readonly_tools_are_dispatched_together() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("read_file", serde_json::json!({"path": "a"}))
                .tool_call("list_directory", serde_json::json!({"path": "."}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let read_file = MockTool::new(
            "read_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "a".into(),
            })]),
        );
        let list_directory = MockTool::new(
            "list_directory",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: ".".into(),
            })]),
        );
        let ctx = TestContext::new(vec![read_file.clone(), list_directory.clone()]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![
                ToolDefinition {
                    name: "read_file".into(),
                    description: "read".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
                ToolDefinition {
                    name: "list_directory".into(),
                    description: "list".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            ]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("parallel tools");

        assert_eq!(read_file.calls().len(), 1);
        assert_eq!(list_directory.calls().len(), 1);
        let second = &provider.requests()[1];
        let tool_parts: Vec<_> = second
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .flat_map(|message| message.content.iter())
            .filter(|part| matches!(part, ContentPart::ToolResult(_)))
            .collect();
        assert_eq!(tool_parts.len(), 2);
        assert_eq!(
            sink.types()
                .into_iter()
                .filter(|name| *name == "ToolExecutionStarted")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn tool_failure_is_filled_back_and_loop_continues() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("broken", serde_json::json!({"x": 1}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("recovered").complete(),
        ]));
        let broken = MockTool::failing(
            "broken",
            ToolError {
                kind: ToolErrorKind::ExecutionFailed,
                message: "boom".into(),
                retryable: false,
                retry_after_ms: None,
            },
        );
        let ctx = TestContext::new(vec![broken]);
        let sink = RecordingEvents::default();

        let summary = run_session(
            &provider,
            sample_request(vec![ToolDefinition {
                name: "broken".into(),
                description: "broken".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("loop continues after tool failure");

        assert_eq!(summary.stop_reason, StopReason::Completed);
        assert_eq!(provider.requests().len(), 2);
        let tool = &tool_messages(&sink)[0];
        assert!(matches!(
            &tool.content[0],
            ContentPart::ToolResult(result) if result.is_error
        ));
        assert!(sink.types().contains(&"RunCompleted"));
    }

    #[tokio::test]
    async fn max_tool_rounds_emits_run_failed_without_extra_stream() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("echo", serde_json::json!({"n": 1}))
                .usage(TokenUsage {
                    input_tokens: 3,
                    output_tokens: 1,
                    ..TokenUsage::default()
                })
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .tool_call("echo", serde_json::json!({"n": 2}))
                .usage(TokenUsage {
                    input_tokens: 4,
                    output_tokens: 2,
                    ..TokenUsage::default()
                })
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .tool_call("echo", serde_json::json!({"n": 3}))
                .complete_with(StopReason::ToolUse),
        ]));
        let echo = MockTool::new("echo", ToolResult::success(Vec::new()));
        let ctx = TestContext::new(vec![echo]);
        let sink = RecordingEvents::default();

        let error = run_session(
            &provider,
            sample_request(vec![echo_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            2,
            TurnContext::default(),
        )
        .await
        .expect_err("max tool rounds");

        assert!(matches!(error, EngineError::MaxToolRounds(2)));
        assert!(!error.is_cancelled());
        assert_eq!(provider.requests().len(), 2);
        assert_eq!(provider.inner.calls().len(), 2);
        let types = sink.types();
        assert!(types.contains(&"RunFailed"));
        assert!(!types.contains(&"RunCompleted"));
        let (failed, usage) = sink
            .snapshot()
            .into_iter()
            .find_map(|envelope| match envelope.payload {
                AgentEvent::RunFailed { error, usage } => Some((error, usage)),
                _ => None,
            })
            .expect("RunFailed");
        assert_eq!(failed.category, ErrorCategory::ResourceExhausted);
        assert!(failed.message.contains("maximum tool rounds exceeded"));
        assert!(failed.message.contains('2'));
        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 7,
                output_tokens: 3,
                ..TokenUsage::default()
            })
        );
    }

    struct ScriptedApprovalCtx {
        inner: TestContext,
        queue: Mutex<Vec<ApprovalDecision>>,
        calls: AtomicU64,
    }

    impl ScriptedApprovalCtx {
        fn new(tools: Vec<MockTool>, queue: Vec<ApprovalDecision>) -> Self {
            Self {
                inner: TestContext::new(tools),
                queue: Mutex::new(queue),
                calls: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl LoopContext for ScriptedApprovalCtx {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            events: LoopEventEmitter<'_>,
            cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            self.inner.execute_tools(calls, events, cancel).await
        }

        async fn request_approval(
            &self,
            calls: &[PendingToolInvocation],
            already_approved_for_run: bool,
            events: LoopEventEmitter<'_>,
            _cancel: CancellationToken,
        ) -> Result<Vec<ApprovalGate>, EngineError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let gates = if already_approved_for_run {
                calls
                    .iter()
                    .map(|_| ApprovalGate::Asked(ApprovalDecision::ApprovedForRun))
                    .collect::<Vec<_>>()
            } else {
                let mut queue = self.queue.lock().expect("approval queue");
                calls
                    .iter()
                    .map(|_| {
                        let decision = if queue.is_empty() {
                            ApprovalDecision::Denied
                        } else {
                            queue.remove(0)
                        };
                        ApprovalGate::Asked(decision)
                    })
                    .collect::<Vec<_>>()
            };
            for call in calls {
                events
                    .emit(AgentEvent::ToolApprovalRequested {
                        tool_call_id: call.tool_call_id.clone(),
                        reason: format!("tool `{}` requires approval", call.name),
                    })
                    .await?;
            }
            Ok(gates)
        }

        fn next_message_id(&self) -> MessageId {
            self.inner.next_message_id()
        }

        fn next_request_id(&self) -> RequestId {
            self.inner.next_request_id()
        }
    }

    fn write_tool_def() -> ToolDefinition {
        ToolDefinition {
            name: "write_file".into(),
            description: "write".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    #[tokio::test]
    async fn approval_event_pair_then_execute_on_approved_once() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("write_file", serde_json::json!({"path": "a.rs"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let write = MockTool::new(
            "write_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "ok".into(),
            })]),
        );
        let ctx = ScriptedApprovalCtx::new(vec![write.clone()], vec![ApprovalDecision::ApprovedOnce]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![write_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("approved once");

        let types = sink.types();
        let requested = types
            .iter()
            .position(|name| *name == "ToolApprovalRequested")
            .expect("requested");
        let responded = types
            .iter()
            .position(|name| *name == "ToolApprovalResponded")
            .expect("responded");
        let started = types
            .iter()
            .position(|name| *name == "ToolExecutionStarted")
            .expect("started");
        assert!(requested < responded);
        assert!(responded < started);
        assert_eq!(write.calls().len(), 1);
        let decision = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::ToolApprovalResponded { decision, .. } => Some(decision),
                _ => None,
            }
        });
        assert_eq!(decision, Some(ApprovalDecision::ApprovedOnce));
    }

    struct EmptyGateCtx {
        inner: TestContext,
        executed: AtomicU64,
    }

    impl EmptyGateCtx {
        fn new(tools: Vec<MockTool>) -> Self {
            Self {
                inner: TestContext::new(tools),
                executed: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl LoopContext for EmptyGateCtx {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            events: LoopEventEmitter<'_>,
            cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            self.executed.fetch_add(1, Ordering::SeqCst);
            self.inner.execute_tools(calls, events, cancel).await
        }

        async fn request_approval(
            &self,
            _calls: &[PendingToolInvocation],
            _already_approved_for_run: bool,
            _events: LoopEventEmitter<'_>,
            _cancel: CancellationToken,
        ) -> Result<Vec<ApprovalGate>, EngineError> {
            Ok(Vec::new())
        }

        fn next_message_id(&self) -> MessageId {
            self.inner.next_message_id()
        }

        fn next_request_id(&self) -> RequestId {
            self.inner.next_request_id()
        }
    }

    #[tokio::test]
    async fn short_approval_gates_fail_closed_without_executing() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("write_file", serde_json::json!({"path": "a.rs"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("stopped").complete(),
        ]));
        let write = MockTool::new(
            "write_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "ok".into(),
            })]),
        );
        let ctx = EmptyGateCtx::new(vec![write.clone()]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![write_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("denied tools still complete the run");

        assert_eq!(ctx.executed.load(Ordering::SeqCst), 0);
        assert_eq!(write.calls().len(), 0);
        assert!(sink.types().contains(&"ToolApprovalResponded"));
        assert!(!sink.types().contains(&"ToolExecutionStarted"));
    }

    #[tokio::test]
    async fn tool_result_artifacts_reach_completed_event_and_tool_message() {
        let artifact = ArtifactReference {
            id: ArtifactId::from("art-1"),
            media_type: "text/plain".into(),
            byte_length: 2,
            content_hash: None,
            label: Some("out".into()),
        };
        let mut result = ToolResult::success(vec![ContentPart::Text(TextContent {
            text: "ok".into(),
        })]);
        result.artifacts = vec![artifact.clone()];
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("echo", serde_json::json!({}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let echo = MockTool::new("echo", result);
        let ctx = TestContext::new(vec![echo]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![echo_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("tool artifacts");

        let completed = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::ToolExecutionCompleted { result, .. } => Some(result),
                _ => None,
            }
        });
        let completed = completed.expect("ToolExecutionCompleted");
        assert_eq!(completed.artifacts, vec![artifact.clone()]);

        let tool_message = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::MessageCommitted { message }
                    if message.role == MessageRole::Tool =>
                {
                    Some(message)
                }
                _ => None,
            }
        });
        let tool_message = tool_message.expect("tool message");
        match &tool_message.content[0] {
            ContentPart::ToolResult(content) => {
                assert_eq!(content.artifacts, vec![artifact]);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn sandbox_fallback_emits_diagnostic() {
        let mut result = ToolResult::success(vec![ContentPart::Text(TextContent {
            text: "ok".into(),
        })]);
        result.metadata = serde_json::json!({
            "sandbox": {
                "backend": "native_restricted",
                "isolation": "soft",
                "fallback": true,
                "note": "seatbelt unavailable"
            }
        });
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("run_command", serde_json::json!({}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let cmd = MockTool::new("run_command", result);
        let ctx = TestContext::new(vec![cmd]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![ToolDefinition {
                name: "run_command".into(),
                description: "run".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("sandbox fallback");

        let diagnostic = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::Diagnostic { code, details } if code == "sandbox.fallback" => {
                    Some(details)
                }
                _ => None,
            }
        });
        let details = diagnostic.expect("sandbox.fallback Diagnostic");
        assert_eq!(details["fallback"], true);
        assert!(details["message"]
            .as_str()
            .is_some_and(|text| text.contains("沙箱回退")));
    }

    #[tokio::test]
    async fn provider_error_after_usage_keeps_run_failed_usage() {
        let usage = TokenUsage {
            input_tokens: 11,
            output_tokens: 5,
            ..TokenUsage::default()
        };
        let provider = RecordingProvider::new(MockProvider::new(
            MockScript::new()
                .usage(usage.clone())
                .fail(ProviderError::new(
                    pawork_domain::ProviderErrorKind::Unknown,
                    "upstream",
                )),
        ));
        let ctx = TestContext::new(Vec::new());
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(Vec::new()),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect_err("provider error");

        let recorded = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::RunFailed { usage, .. } => usage,
                _ => None,
            }
        });
        assert_eq!(recorded, Some(usage));
    }

    struct CheckpointingCtx {
        inner: TestContext,
    }

    impl CheckpointingCtx {
        fn new(tools: Vec<MockTool>) -> Self {
            Self {
                inner: TestContext::new(tools),
            }
        }
    }

    #[async_trait]
    impl LoopContext for CheckpointingCtx {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            events: LoopEventEmitter<'_>,
            cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            self.inner.execute_tools(calls, events, cancel).await
        }

        async fn request_approval(
            &self,
            calls: &[PendingToolInvocation],
            already_approved_for_run: bool,
            events: LoopEventEmitter<'_>,
            cancel: CancellationToken,
        ) -> Result<Vec<ApprovalGate>, EngineError> {
            self.inner
                .request_approval(calls, already_approved_for_run, events, cancel)
                .await
        }

        fn next_message_id(&self) -> MessageId {
            self.inner.next_message_id()
        }

        fn next_request_id(&self) -> RequestId {
            self.inner.next_request_id()
        }

        async fn snapshot_write_tools(
            &self,
            calls: &[PendingToolInvocation],
            _cancel: CancellationToken,
        ) -> Vec<WriteCheckpoint> {
            calls
                .iter()
                .filter(|call| call.name == "write_file")
                .map(|call| WriteCheckpoint {
                    checkpoint_id: CheckpointId::from(format!(
                        "run-1/{}",
                        call.tool_call_id.as_str()
                    )),
                    artifacts: Vec::new(),
                })
                .collect()
        }
    }

    #[tokio::test]
    async fn write_snapshot_emits_checkpoint_created_before_execution() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("write_file", serde_json::json!({"path": "a.rs"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let write = MockTool::new(
            "write_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "ok".into(),
            })]),
        );
        let ctx = CheckpointingCtx::new(vec![write]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![write_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("write loop");

        let types = sink.types();
        let created = types
            .iter()
            .position(|name| *name == "CheckpointCreated")
            .expect("CheckpointCreated");
        let started = types
            .iter()
            .position(|name| *name == "ToolExecutionStarted")
            .expect("started");
        assert!(created < started);
        let checkpoint = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::CheckpointCreated {
                    checkpoint_id,
                    artifacts,
                } => Some((checkpoint_id, artifacts)),
                _ => None,
            }
        });
        assert_eq!(
            checkpoint,
            Some((
                CheckpointId::from("run-1/mock-tool-call-0"),
                Vec::new()
            ))
        );
    }

    #[tokio::test]
    async fn checkpoint_rollback_appends_to_event_stream() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("write_file", serde_json::json!({"path": "a.rs"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let write = MockTool::new(
            "write_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "ok".into(),
            })]),
        );
        let ctx = CheckpointingCtx::new(vec![write]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![write_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("write loop");

        let last = sink.snapshot().last().cloned().expect("events");
        let next = EventSequence::new(last.sequence.value() + 1);
        sink.emit(AgentEventEnvelope::new(
            EventId::from("evt-rollback"),
            last.session_id.clone(),
            last.run_id.clone(),
            next,
            Timestamp::from_unix_millis(2),
            AgentEvent::CheckpointRolledBack {
                checkpoint_id: CheckpointId::from("run-1/mock-tool-call-0"),
            },
        ))
        .await
        .expect("append rollback");

        let snapshot = sink.snapshot();
        let sequences: Vec<u64> = snapshot.iter().map(|e| e.sequence.value()).collect();
        assert_eq!(
            sequences,
            (1..=snapshot.len() as u64).collect::<Vec<_>>()
        );
        assert_eq!(
            event_type(&snapshot.last().expect("last").payload),
            "CheckpointRolledBack"
        );
    }

    #[tokio::test]
    async fn denied_fills_tool_result_and_continues_without_executing() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("write_file", serde_json::json!({"path": "a.rs"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("explained").complete(),
        ]));
        let write = MockTool::new(
            "write_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "should-not-run".into(),
            })]),
        );
        let ctx = ScriptedApprovalCtx::new(vec![write.clone()], vec![ApprovalDecision::Denied]);
        let sink = RecordingEvents::default();

        let summary = run_session(
            &provider,
            sample_request(vec![write_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("denied continues");

        assert_eq!(summary.stop_reason, StopReason::Completed);
        assert_eq!(write.calls().len(), 0);
        assert!(!sink.types().contains(&"ToolExecutionStarted"));
        assert!(sink.types().contains(&"ToolApprovalRequested"));
        assert!(sink.types().contains(&"ToolApprovalResponded"));
        assert_eq!(provider.requests().len(), 2);
        let tool = &tool_messages(&sink)[0];
        match &tool.content[0] {
            ContentPart::ToolResult(result) => {
                assert!(result.is_error);
                assert!(
                    result
                        .content
                        .iter()
                        .any(|part| matches!(part, ContentPart::Text(text) if text.text.contains("denied")))
                );
            }
            other => panic!("expected tool result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn approved_for_run_remembers_across_tool_rounds() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("write_file", serde_json::json!({"path": "a.rs"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .tool_call("write_file", serde_json::json!({"path": "b.rs"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let write = MockTool::new(
            "write_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "ok".into(),
            })]),
        );
        let ctx = ScriptedApprovalCtx::new(
            vec![write.clone()],
            vec![ApprovalDecision::ApprovedForRun, ApprovalDecision::Denied],
        );
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![write_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("approved for run");

        assert_eq!(write.calls().len(), 2);
        assert_eq!(ctx.calls.load(Ordering::SeqCst), 2);
        let decisions: Vec<_> = sink
            .snapshot()
            .into_iter()
            .filter_map(|envelope| match envelope.payload {
                AgentEvent::ToolApprovalResponded { decision, .. } => Some(decision),
                _ => None,
            })
            .collect();
        assert_eq!(
            decisions,
            vec![
                ApprovalDecision::ApprovedForRun,
                ApprovalDecision::ApprovedForRun
            ]
        );
    }

    struct HangUntilCancelCtx {
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        msg_counter: AtomicU64,
        req_counter: AtomicU64,
    }

    impl HangUntilCancelCtx {
        fn new(started: tokio::sync::oneshot::Sender<()>) -> Self {
            Self {
                started: Mutex::new(Some(started)),
                msg_counter: AtomicU64::new(0),
                req_counter: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl LoopContext for HangUntilCancelCtx {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            _events: LoopEventEmitter<'_>,
            cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            if let Some(tx) = self.started.lock().expect("started mutex").take() {
                let _ = tx.send(());
            }
            cancel.cancelled().await;
            calls
                .into_iter()
                .map(|call| ToolCallResult {
                    tool_call_id: call.tool_call_id,
                    tool_name: call.name,
                    arguments: call.arguments,
                    result: ToolResult::failure(ErrorContext {
                        category: ErrorCategory::Cancelled,
                        message: "hang cancelled".into(),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    }),
                })
                .collect()
        }

        async fn request_approval(
            &self,
            calls: &[PendingToolInvocation],
            _already_approved_for_run: bool,
            _events: LoopEventEmitter<'_>,
            _cancel: CancellationToken,
        ) -> Result<Vec<ApprovalGate>, EngineError> {
            Ok(calls.iter().map(|_| ApprovalGate::NotRequired).collect())
        }

        fn next_message_id(&self) -> MessageId {
            let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
            MessageId::from(format!("msg-{n}"))
        }

        fn next_request_id(&self) -> RequestId {
            let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
            RequestId::from(format!("req-{n}"))
        }
    }

    struct CountingCleaner {
        run: RunId,
        count: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crate::ProcessTreeCleaner for CountingCleaner {
        fn cleanup(&self, run_id: &RunId) -> usize {
            assert_eq!(run_id, &self.run);
            self.count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            1
        }
    }

    #[tokio::test]
    async fn cancel_during_long_tool_emits_run_cancelled_without_completing_tools() {
        use crate::{CancelHandle, CancelReason};

        let provider = RecordingProvider::new(MockProvider::sequence(vec![MockScript::new()
            .tool_call("hang", serde_json::json!({}))
            .complete_with(StopReason::ToolUse)]));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let ctx = HangUntilCancelCtx::new(tx);
        let sink = RecordingEvents::default();
        let cleaned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handle = CancelHandle::new(
            RunId::from("run-1"),
            Arc::new(CountingCleaner {
                run: RunId::from("run-1"),
                count: cleaned.clone(),
            }),
        );

        let session = run_session(
            &provider,
            sample_request(vec![ToolDefinition {
                name: "hang".into(),
                description: "hang until cancel".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]),
            sample_turn(),
            &sink,
            handle.token(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        );
        tokio::pin!(session);
        tokio::select! {
            result = &mut session => {
                panic!("session ended before cancel: {result:?}");
            }
            started = rx => {
                started.expect("tool started");
            }
        }
        handle.cancel(CancelReason::User);
        let error = session.await.expect_err("cancelled");
        assert!(matches!(
            error,
            EngineError::Provider(ref err)
                if err.kind == pawork_domain::ProviderErrorKind::Cancelled
        ));

        let types = sink.types();
        assert!(types.contains(&"ToolExecutionStarted"));
        assert!(types.contains(&"RunCancelled"));
        assert!(!types.contains(&"ToolExecutionCompleted"));
        assert!(!types.contains(&"MessageCommitted.tool"));
        assert!(!types.contains(&"RunCompleted"));
        assert!(!types.contains(&"RunFailed"));
        assert_eq!(provider.requests().len(), 1);
        assert_eq!(cleaned.load(std::sync::atomic::Ordering::SeqCst), 1);
        let last = types.last().copied();
        assert_eq!(last, Some("RunCancelled"));
    }

    struct HangUntilApprovalCtx {
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        msg_counter: AtomicU64,
        req_counter: AtomicU64,
    }

    impl HangUntilApprovalCtx {
        fn new(started: tokio::sync::oneshot::Sender<()>) -> Self {
            Self {
                started: Mutex::new(Some(started)),
                msg_counter: AtomicU64::new(0),
                req_counter: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl LoopContext for HangUntilApprovalCtx {
        async fn execute_tools(
            &self,
            _calls: Vec<PendingToolInvocation>,
            _events: LoopEventEmitter<'_>,
            _cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            Vec::new()
        }

        async fn request_approval(
            &self,
            calls: &[PendingToolInvocation],
            _already_approved_for_run: bool,
            events: LoopEventEmitter<'_>,
            cancel: CancellationToken,
        ) -> Result<Vec<ApprovalGate>, EngineError> {
            for call in calls {
                events
                    .emit(AgentEvent::ToolApprovalRequested {
                        tool_call_id: call.tool_call_id.clone(),
                        reason: format!("tool `{}` requires approval", call.name),
                    })
                    .await?;
            }
            if let Some(tx) = self.started.lock().expect("started mutex").take() {
                let _ = tx.send(());
            }
            cancel.cancelled().await;
            Ok(calls
                .iter()
                .map(|_| ApprovalGate::Asked(ApprovalDecision::Cancelled))
                .collect())
        }

        fn next_message_id(&self) -> MessageId {
            let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
            MessageId::from(format!("msg-{n}"))
        }

        fn next_request_id(&self) -> RequestId {
            let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
            RequestId::from(format!("req-{n}"))
        }
    }

    #[tokio::test]
    async fn cancel_while_waiting_for_approval_emits_requested_without_responded() {
        use crate::{CancelHandle, CancelReason};

        let provider = RecordingProvider::new(MockProvider::sequence(vec![MockScript::new()
            .tool_call("write_file", serde_json::json!({"path": "a.rs"}))
            .complete_with(StopReason::ToolUse)]));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let ctx = HangUntilApprovalCtx::new(tx);
        let sink = RecordingEvents::default();
        let handle = CancelHandle::new(
            RunId::from("run-1"),
            Arc::new(CountingCleaner {
                run: RunId::from("run-1"),
                count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }),
        );

        let session = run_session(
            &provider,
            sample_request(vec![write_tool_def()]),
            sample_turn(),
            &sink,
            handle.token(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        );
        tokio::pin!(session);
        tokio::select! {
            result = &mut session => {
                panic!("session ended before approval wait: {result:?}");
            }
            started = rx => {
                started.expect("requested emitted");
            }
        }
        handle.cancel(CancelReason::User);
        let error = session.await.expect_err("cancelled");
        assert!(matches!(
            error,
            EngineError::Provider(ref err)
                if err.kind == pawork_domain::ProviderErrorKind::Cancelled
        ));
        let types = sink.types();
        assert!(types.contains(&"ToolApprovalRequested"));
        assert!(!types.contains(&"ToolApprovalResponded"));
        assert!(!types.contains(&"ToolExecutionStarted"));
        assert_eq!(types.last().copied(), Some("RunCancelled"));
    }

    #[tokio::test]
    async fn default_turn_context_keeps_pre_s5_behavior() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("echo", serde_json::json!({"text": "hi"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]));
        let echo = MockTool::new(
            "echo",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "hi".into(),
            })]),
        );
        let ctx = TestContext::new(vec![echo]);
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            sample_request(vec![echo_tool_def()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            TurnContext::default(),
        )
        .await
        .expect("default context run");

        // 估算为 0（现状）、不压缩、不截断；ContextPrepared 每轮一次。
        assert_eq!(
            context_prepared_events(&sink),
            vec![(1, 0), (3, 0)],
            "per-round ContextPrepared with zero estimate"
        );
        let types = sink.types();
        assert!(!types.contains(&"CompactionStarted"));
        assert!(!types.contains(&"CompactionCompleted"));
        assert!(!types.contains(&"Diagnostic"));
        assert_eq!(provider.requests().len(), 2);
    }

    #[tokio::test]
    async fn injected_layers_prepend_system_and_emit_diagnostic() {
        let provider = RecordingProvider::new(MockProvider::new(
            MockScript::new().text("ok").complete(),
        ));
        let sink = RecordingEvents::default();
        let ctx = TestContext::new(Vec::new());
        let mut context = TurnContext {
            estimator: Some(Arc::new(HeuristicEstimator::default())),
            ..TurnContext::default()
        };
        context.injected_layers = vec![InjectedLayer {
            kind: "root_agents_file".into(),
            resource_id: "AGENTS.md".into(),
            content: "所有回答以『收到』开头".into(),
        }];

        run_session(
            &provider,
            sample_request(Vec::new()),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            context,
        )
        .await
        .expect("injected run");

        let types = sink.types();
        assert!(types.contains(&"Diagnostic"));
        let diagnostic = sink
            .snapshot()
            .into_iter()
            .find_map(|envelope| match envelope.payload {
                AgentEvent::Diagnostic { code, details } => Some((code, details)),
                _ => None,
            })
            .expect("resources diagnostic");
        assert_eq!(diagnostic.0, "resources.injected");
        assert_eq!(diagnostic.1["layers"][0]["resource_id"], "AGENTS.md");

        let request = &provider.requests()[0];
        assert_eq!(request.messages[0].role, MessageRole::System);
        let text = match &request.messages[0].content[0] {
            ContentPart::Text(body) => body.text.as_str(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("收到"), "{text}");
        let prepared = context_prepared_events(&sink);
        assert!(
            prepared[0].1 > 0,
            "injected system must count toward ContextPrepared: {prepared:?}"
        );
    }

    #[tokio::test]
    async fn soft_limit_compaction_summarizes_and_rebuilds_history() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new()
                .text("summary of earlier work")
                .usage(TokenUsage {
                    input_tokens: 111,
                    output_tokens: 222,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                })
                .complete(),
            MockScript::new().text("done").complete(),
        ]));
        let ctx = TestContext::new(Vec::new());
        let sink = RecordingEvents::default();

        let summary = run_session(
            &provider,
            request_with_messages(numbered_messages(6, "with some body")),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            turn_context(
                ContextBudget::from_context_window(1_000_000, 4_096, 0),
                Some(30),
                2,
            ),
        )
        .await
        .expect("soft-limit run");

        // 摘要请求的 usage 不计入 run_usage。
        assert_eq!(summary.usage.input_tokens, 0);
        assert_eq!(summary.usage.output_tokens, 0);

        let types = sink.types();
        let started = types
            .iter()
            .position(|name| *name == "CompactionStarted")
            .expect("CompactionStarted");
        let summary_commit = types
            .iter()
            .enumerate()
            .find(|(index, name)| *index > started && **name == "MessageCommitted.user")
            .expect("summary MessageCommitted after CompactionStarted");
        let completed = types
            .iter()
            .position(|name| *name == "CompactionCompleted")
            .expect("CompactionCompleted");
        assert!(summary_commit.0 < completed);
        assert!(!types.contains(&"Diagnostic"));

        let completed = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::CompactionCompleted {
                    summary_message_id,
                    compacted_through,
 } => Some((summary_message_id, compacted_through)),
                _ => None,
            }
        }).expect("CompactionCompleted payload");
        let started_payload = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::CompactionStarted { source_event_count } => Some(source_event_count),
                _ => None,
            }
        }).expect("CompactionStarted payload");
        // 默认 host 回调返回 None：source_event_count 用被压缩消息数（6 - retained 2）。
        assert_eq!(started_payload, 4);
        // 无 outcome 的 completed 水位 fail-safe 为 0：不得取新摘要自身 sequence。
        assert_eq!(completed.1, EventSequence::new(0));

        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        // 摘要请求：无 tools，单条 User 指令 + 被压缩区间文本。
        assert!(requests[0].tools.is_empty());
        assert_eq!(requests[0].messages.len(), 1);
        assert!(matches!(&requests[0].messages[0].content[0],
            ContentPart::Text(text) if text.text.contains("turn 0: with some body")));
        // 后续主请求以 summary 开头，保留最近 2 条。
        assert_eq!(requests[1].messages.len(), 3);
        assert_eq!(requests[1].messages[0].id, completed.0);
        assert_eq!(requests[1].messages[0].role, MessageRole::User);
        assert!(matches!(&requests[1].messages[0].content[0],
            ContentPart::Text(text) if text.text == "summary of earlier work"));
        assert!(requests[1].messages[1].id.as_str().contains("msg-history-4"));
        assert!(requests[1].messages[2].id.as_str().contains("msg-history-5"));
    }

    #[tokio::test]
    async fn hard_limit_truncates_oldest_with_diagnostic_and_refreshed_estimate() {
        let messages: Vec<Message> = (0..10)
            .map(|n| user_text(&format!("msg-history-{n}"), &"x".repeat(400)))
            .collect();
        let provider =
            RecordingProvider::new(MockProvider::sequence(vec![MockScript::new()
                .text("ok")
                .complete()]));
        let ctx = TestContext::new(Vec::new());
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            request_with_messages(messages.clone()),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            turn_context(ContextBudget::from_context_window(250, 0, 0), None, 2),
        )
        .await
        .expect("hard-limit run");

        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        // 永不丢最后 retained_messages 条：仅保留 msg-8 / msg-9。
        assert_eq!(request.messages, vec![messages[8].clone(), messages[9].clone()]);
        assert!(estimate_request_tokens(request) <= 250);

        let diagnostic = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::Diagnostic { code, details } => Some((code, details)),
                _ => None,
            }
        }).expect("Diagnostic");
        assert_eq!(diagnostic.0, "context_hard_truncated");
        assert_eq!(diagnostic.1["dropped_messages"], serde_json::json!(8));
        // 2 * (framing 4 + role 1 + 100) + primer 3 = 213
        assert_eq!(diagnostic.1["estimated_input_tokens"], serde_json::json!(213));

        // ContextPrepared 重发反映截断后值；首条反映截断前。
        assert_eq!(
            context_prepared_events(&sink),
            vec![(10, 1053), (2, 213)]
        );
        assert!(!sink.types().contains(&"CompactionStarted"));
    }

    #[tokio::test]
    async fn compaction_outcome_metadata_flows_into_events() {
        struct ScriptedCompactionCtx {
            inner: TestContext,
            calls: AtomicU64,
        }

        #[async_trait]
        impl LoopContext for ScriptedCompactionCtx {
            async fn execute_tools(
                &self,
                calls: Vec<PendingToolInvocation>,
                events: LoopEventEmitter<'_>,
                cancel: CancellationToken,
            ) -> Vec<ToolCallResult> {
                self.inner.execute_tools(calls, events, cancel).await
            }

            async fn request_approval(
                &self,
                calls: &[PendingToolInvocation],
                already_approved_for_run: bool,
                events: LoopEventEmitter<'_>,
                cancel: CancellationToken,
            ) -> Result<Vec<ApprovalGate>, EngineError> {
                self.inner
                    .request_approval(calls, already_approved_for_run, events, cancel)
                    .await
            }

            fn next_message_id(&self) -> MessageId {
                self.inner.next_message_id()
            }

            fn next_request_id(&self) -> RequestId {
                self.inner.next_request_id()
            }

            async fn compact_history(
                &self,
                reason: AutoCompactionReason,
                summary_text: &str,
                _cancel: CancellationToken,
            ) -> Result<Option<CompactionOutcome>, EngineError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(reason, AutoCompactionReason::HistorySoftLimit);
                assert!(!summary_text.is_empty());
                Ok(Some(CompactionOutcome {
                    source_event_count: 99,
                    compacted_through: EventSequence::new(42),
                }))
            }
        }

        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new().text("host-backed summary").complete(),
            MockScript::new().text("done").complete(),
        ]));
        let ctx = ScriptedCompactionCtx {
            inner: TestContext::new(Vec::new()),
            calls: AtomicU64::new(0),
        };
        let sink = RecordingEvents::default();

        run_session(
            &provider,
            request_with_messages(numbered_messages(6, "with some body")),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            turn_context(
                ContextBudget::from_context_window(1_000_000, 4_096, 0),
                Some(30),
                2,
            ),
        )
        .await
        .expect("soft-limit with host outcome");

        assert_eq!(ctx.calls.load(Ordering::SeqCst), 1);
        let started = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::CompactionStarted { source_event_count } => Some(source_event_count),
                _ => None,
            }
        }).expect("CompactionStarted");
        assert_eq!(started, 99);
        let completed = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::CompactionCompleted { compacted_through, .. } => Some(compacted_through),
                _ => None,
            }
        }).expect("CompactionCompleted");
        assert_eq!(completed, EventSequence::new(42));
    }

    #[tokio::test]
    async fn compact_history_error_fails_the_run_instead_of_being_swallowed() {
        struct FailingCompactCtx {
            inner: TestContext,
        }

        #[async_trait]
        impl LoopContext for FailingCompactCtx {
            async fn execute_tools(
                &self,
                calls: Vec<PendingToolInvocation>,
                events: LoopEventEmitter<'_>,
                cancel: CancellationToken,
            ) -> Vec<ToolCallResult> {
                self.inner.execute_tools(calls, events, cancel).await
            }

            async fn request_approval(
                &self,
                calls: &[PendingToolInvocation],
                already_approved_for_run: bool,
                events: LoopEventEmitter<'_>,
                cancel: CancellationToken,
            ) -> Result<Vec<ApprovalGate>, EngineError> {
                self.inner
                    .request_approval(calls, already_approved_for_run, events, cancel)
                    .await
            }

            fn next_message_id(&self) -> MessageId {
                self.inner.next_message_id()
            }

            fn next_request_id(&self) -> RequestId {
                self.inner.next_request_id()
            }

            async fn compact_history(
                &self,
                _reason: AutoCompactionReason,
                _summary_text: &str,
                _cancel: CancellationToken,
            ) -> Result<Option<CompactionOutcome>, EngineError> {
                Err(EngineError::sink("session store unavailable"))
            }
        }

        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new().text("summary that will not be committed").complete(),
        ]));
        let ctx = FailingCompactCtx {
            inner: TestContext::new(Vec::new()),
        };
        let sink = RecordingEvents::default();

        let error = run_session(
            &provider,
            request_with_messages(numbered_messages(6, "with some body")),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            turn_context(
                ContextBudget::from_context_window(1_000_000, 4_096, 0),
                Some(30),
                2,
            ),
        )
        .await
        .expect_err("host compact failure must fail the run");

        assert!(matches!(
            error,
            EngineError::Sink(message) if message == "session store unavailable"
        ));
        assert!(
            !sink.types().contains(&"CompactionStarted"),
            "host 失败后不得继续发压缩事件三连"
        );
    }

    #[tokio::test]
    async fn manual_compaction_emits_events_and_returns_rebuilt_messages() {
        let provider = RecordingProvider::new(MockProvider::sequence(vec![
            MockScript::new().text("manual summary").complete(),
        ]));
        let messages = numbered_messages(5, "manual body");
        let ctx = TestContext::new(Vec::new());
        let sink = RecordingEvents::default();

        let rebuilt = run_manual_compaction(
            &provider,
            request_with_messages(messages.clone()),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            turn_context(ContextBudget::default(), None, 2),
        )
        .await
        .expect("manual compaction");

        assert_eq!(rebuilt.len(), 3);
        assert_eq!(rebuilt[0].role, MessageRole::User);
        assert!(matches!(&rebuilt[0].content[0],
            ContentPart::Text(text) if text.text == "manual summary"));
        assert_eq!(rebuilt[1], messages[3]);
        assert_eq!(rebuilt[2], messages[4]);

        // 不是 run：没有 RunStarted / RunCancelled / ContextPrepared。
        assert_eq!(
            sink.types(),
            vec!["CompactionStarted", "MessageCommitted.user", "CompactionCompleted"]
        );
        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].tools.is_empty());
        let completed = sink.snapshot().into_iter().find_map(|envelope| {
            match envelope.payload {
                AgentEvent::CompactionCompleted { summary_message_id, .. } => Some(summary_message_id),
                _ => None,
            }
        }).expect("CompactionCompleted");
        assert_eq!(rebuilt[0].id, completed);
    }

    #[tokio::test]
    async fn manual_compaction_rejects_when_nothing_to_compact() {
        let provider = RecordingProvider::new(MockProvider::sequence(Vec::new()));
        let ctx = TestContext::new(Vec::new());
        let sink = RecordingEvents::default();

        let error = run_manual_compaction(
            &provider,
            request_with_messages(vec![user_hello()]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            TurnContext::default(),
        )
        .await
        .expect_err("nothing to compact");

        assert!(matches!(error, EngineError::Sink(_)));
        assert!(provider.requests().is_empty());
        assert!(sink.snapshot().is_empty());
    }

    #[tokio::test]
    async fn long_conversation_never_exceeds_hard_limit() {
        let provider = GrowingProvider::new();
        let grow = MockTool::new(
            "grow",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "x".repeat(800),
            })]),
        );
        let ctx = TestContext::new(vec![grow]);
        let sink = RecordingEvents::default();
        let hard_limit = 1_200;

        let error = run_session(
            &provider,
            sample_request(vec![ToolDefinition {
                name: "grow".into(),
                description: "grow the context".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }]),
            sample_turn(),
            &sink,
            CancellationToken::new(),
            &ctx,
            15,
            turn_context(
                ContextBudget::from_context_window(hard_limit, 0, 0),
                Some(500),
                4,
            ),
        )
        .await
        .expect_err("max tool rounds expected");
        assert!(matches!(error, EngineError::MaxToolRounds(15)));
        assert!(sink.types().contains(&"CompactionStarted"));

        let requests = provider.requests();
        let main_requests: Vec<&CanonicalModelRequest> = requests
            .iter()
            .filter(|request| !request.tools.is_empty())
            .collect();
        assert_eq!(main_requests.len(), 15);
        for request in &main_requests {
            let estimate = estimate_request_tokens(request);
            assert!(
                estimate <= hard_limit,
                "estimated {estimate} tokens for a main request with {} messages",
                request.messages.len()
            );
        }
    }
}
