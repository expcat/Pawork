//! 会话轮次标识（[`SessionTurn`]）、墙钟时间戳与工具循环共享的 usage 归并 helper。

use pawork_domain::{
    Message, ModelId, ProviderId, ProviderStreamEvent, RunId, SessionId, Timestamp, TokenUsage,
};

/// 一次会话轮次的标识与起始 sequence。
pub struct SessionTurn {
    pub session_id: SessionId,
    pub run_id: RunId,
    pub provider_id: ProviderId,
    pub model: ModelId,
    pub start_sequence: u64,
    pub trigger_message: Message,
    pub timestamp: Timestamp,
}

impl SessionTurn {
    pub fn new(
        session_id: SessionId,
        run_id: RunId,
        provider_id: ProviderId,
        model: ModelId,
        start_sequence: u64,
        trigger_message: Message,
    ) -> Self {
        Self {
            session_id,
            run_id,
            provider_id,
            model,
            start_sequence,
            trigger_message,
            timestamp: now_timestamp(),
        }
    }
}

pub fn now_timestamp() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    Timestamp::from_unix_millis(millis)
}

pub(crate) fn optional_usage(usage: &TokenUsage) -> Option<TokenUsage> {
    if usage.is_zero() {
        None
    } else {
        Some(usage.clone())
    }
}

pub(crate) fn last_stream_usage(events: &[ProviderStreamEvent]) -> TokenUsage {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            ProviderStreamEvent::UsageUpdated(usage) => Some(usage.clone()),
            _ => None,
        })
        .unwrap_or_default()
}
