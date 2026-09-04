//! 单轮 provider 流收集：经 [`crate::run_turn`] 与 [`LoopSink`] 缓冲，
//! 折叠为 [`AssembledTurn`]。不按 Provider 名称分支。

use pawork_domain::{
    CancellationToken, CanonicalModelRequest, Message, MessageId, MessageMetadata, ModelId,
    ModelProvider, ModelResponseSummary, ProviderError, ProviderErrorKind, ProviderId,
    ProviderStreamEvent, TokenUsage,
};

use crate::appender::AssembledTurn;
use crate::event::{EngineError, EventEmitter, LoopSink};
use crate::run_turn;

pub(super) enum StreamRound {
    Succeeded {
        assembled: AssembledTurn,
        summary: ModelResponseSummary,
    },
    Cancelled {
        message: String,
        stream_usage: TokenUsage,
    },
    Failed {
        error: ProviderError,
        stream_usage: TokenUsage,
    },
}

/// 调用 provider 流、检查 persist 失败、折叠缓冲事件。
/// persist 失败优先返回，不再继续折叠或发射终态。
pub(super) async fn collect_stream_round(
    provider: &dyn ModelProvider,
    request: CanonicalModelRequest,
    emitter: &EventEmitter<'_>,
    assistant_id: MessageId,
    cancel: CancellationToken,
) -> Result<StreamRound, EngineError> {
    let sink = LoopSink::new(emitter.clone(), assistant_id.clone());
    let result = run_turn(provider, request, &sink, cancel).await;
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
            Ok(StreamRound::Succeeded { assembled, summary })
        }
        Err(error) if error.kind == ProviderErrorKind::Cancelled => {
            let stream_usage = last_stream_usage(&sink.drain_events());
            Ok(StreamRound::Cancelled {
                message: error.message.clone(),
                stream_usage,
            })
        }
        Err(error) => {
            let stream_usage = last_stream_usage(&sink.drain_events());
            Ok(StreamRound::Failed {
                error,
                stream_usage,
            })
        }
    }
}

pub(super) fn assistant_message(
    assembled: AssembledTurn,
    summary: &ModelResponseSummary,
    provider_id: &ProviderId,
    model: &ModelId,
) -> Message {
    assembled.into_message(MessageMetadata {
        usage: Some(summary.usage.clone()),
        stop_reason: Some(summary.stop_reason.clone()),
        provider: Some(provider_id.clone()),
        model: Some(model.clone()),
        ..MessageMetadata::default()
    })
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

pub(super) fn saturating_add_usage(acc: &TokenUsage, round: &TokenUsage) -> TokenUsage {
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
