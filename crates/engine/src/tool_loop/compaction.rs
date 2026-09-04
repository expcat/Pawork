//! 循环内上下文收敛：资源层注入、输入估算、软限压缩链、硬限截断。
//! 自动软限与手动入口共用 [`compact_messages`]。

use std::sync::atomic::AtomicU64;
use std::sync::Mutex;

use async_trait::async_trait;
use pawork_domain::{
    AgentEvent, CancellationToken, ContentPart, EventSequence, Message, MessageId, MessageRole,
    ModelId, TextContent,
};
use pawork_domain::{
    CanonicalModelRequest, ModelProvider, ProviderError, ProviderEventSink, ProviderStreamEvent,
};

use crate::context::{
    compute_compaction, reply_primer_tokens, AutoCompactionReason, ContextBudgetBreakdown,
    InjectedLayer, TokenEstimator, ToolSchema, TurnContext,
};
use crate::event::{AgentEventSink, EngineError, EventEmitter};
use crate::run_turn;
use crate::session_turn::SessionTurn;

use super::LoopContext;

/// 一轮请求的输入估算：system / 工具 schema / 历史与总量的 token 计数。
pub(super) struct InputEstimate {
    system_prompt_tokens: u64,
    tool_schema_tokens: u64,
    history_tokens: u64,
    pub(super) estimated_input_tokens: u64,
}

const INJECTED_SYSTEM_ID: &str = "msg-resources";

/// 把资源层拼成一条 System 消息，插到请求最前。不改冻结请求结构。
pub(super) fn apply_injected_layers(
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

pub(super) fn injected_layers_details(layers: &[InjectedLayer]) -> serde_json::Value {
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
pub(super) fn estimate_input(
    context: &TurnContext,
    request: &CanonicalModelRequest,
) -> InputEstimate {
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
pub(super) async fn apply_context_limits(
    provider: &dyn ModelProvider,
    emitter: &EventEmitter<'_>,
    loop_ctx: &dyn LoopContext,
    model: &ModelId,
    context: &TurnContext,
    current: &mut CanonicalModelRequest,
    estimate: &mut InputEstimate,
    cancel: CancellationToken,
) -> Result<(), EngineError> {
    let (Some(limits), Some(estimator)) = (context.limits.as_ref(), context.estimator.as_deref())
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
        content: vec![ContentPart::Text(TextContent { text: summary_text })],
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
