//! 事件发射：分配 `sequence`，经 [`AgentEventSink`] 双写给调用方。
//!
//! Engine 不依赖 SQLite。落库由 app 的 sink 在 `emit` 里 `append_event`。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use pawork_domain::{
    ProviderError, ProviderEventSink, ProviderStreamEvent, ToolOutputChannel, ToolStreamEvent,
};
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, EventId, EventSequence, MessageId, RunId, SessionId, Timestamp,
    ToolCallId, ToolOutputStream,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("{0}")]
    Sink(String),
    #[error("maximum tool rounds exceeded ({0})")]
    MaxToolRounds(u64),
}

impl EngineError {
    pub fn sink(message: impl Into<String>) -> Self {
        Self::Sink(message.into())
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            Self::Provider(error) if error.kind == pawork_domain::ProviderErrorKind::Cancelled
        )
    }
}

/// 已分配 sequence 的信封出口。调用方负责 persist-first 再渲染。
#[async_trait]
pub trait AgentEventSink: Send + Sync {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError>;
}

#[derive(Clone)]
pub(crate) struct EventEmitter<'a> {
    session_id: SessionId,
    run_id: RunId,
    next_sequence: &'a AtomicU64,
    timestamp: Timestamp,
    sink: &'a dyn AgentEventSink,
}

impl<'a> EventEmitter<'a> {
    pub(crate) fn new(
        session_id: SessionId,
        run_id: RunId,
        next_sequence: &'a AtomicU64,
        timestamp: Timestamp,
        sink: &'a dyn AgentEventSink,
    ) -> Self {
        Self {
            session_id,
            run_id,
            next_sequence,
            timestamp,
            sink,
        }
    }

    pub(crate) async fn emit(&self, payload: AgentEvent) -> Result<EventSequence, EngineError> {
        let sequence = EventSequence::new(self.next_sequence.fetch_add(1, Ordering::SeqCst));
        let envelope = AgentEventEnvelope::new(
            EventId::from(format!("evt-{}-{}", self.run_id, sequence.value())),
            self.session_id.clone(),
            self.run_id.clone(),
            sequence,
            self.timestamp,
            payload,
        );
        self.sink.emit(envelope).await?;
        Ok(sequence)
    }

    /// 当前已发出的最大 sequence（尚未发出任何事件时饱和到 0）。
    pub(crate) fn last_sequence(&self) -> EventSequence {
        EventSequence::new(
            self.next_sequence
                .load(Ordering::SeqCst)
                .saturating_sub(1),
        )
    }
}

/// 可 Clone 的 Loop 事件发射器；复制的是 sequence 与 sink 的引用。
#[derive(Clone)]
pub struct LoopEventEmitter<'a> {
    inner: EventEmitter<'a>,
}

impl<'a> LoopEventEmitter<'a> {
    pub(crate) fn new(inner: EventEmitter<'a>) -> Self {
        Self { inner }
    }

    pub async fn emit(&self, payload: AgentEvent) -> Result<EventSequence, EngineError> {
        self.inner.emit(payload).await
    }

    pub async fn emit_tool_event(
        &self,
        tool_call_id: ToolCallId,
        event: ToolStreamEvent,
    ) -> Result<(), EngineError> {
        match event {
            ToolStreamEvent::OutputDelta { channel, delta } => {
                let stream = match channel {
                    ToolOutputChannel::Stdout => ToolOutputStream::Stdout,
                    ToolOutputChannel::Stderr => ToolOutputStream::Stderr,
                    ToolOutputChannel::Structured => ToolOutputStream::Structured,
                };
                self.inner
                    .emit(AgentEvent::ToolOutputDelta {
                        tool_call_id,
                        stream,
                        delta,
                    })
                    .await?;
                Ok(())
            }
            ToolStreamEvent::Progress { .. } | ToolStreamEvent::ArtifactAvailable(_) => Ok(()),
        }
    }
}

pub(crate) struct LoopSink<'a> {
    events: Mutex<Vec<ProviderStreamEvent>>,
    persist_error: Mutex<Option<EngineError>>,
    emitter: EventEmitter<'a>,
    message_id: MessageId,
}

impl<'a> LoopSink<'a> {
    pub(crate) fn new(emitter: EventEmitter<'a>, message_id: MessageId) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            persist_error: Mutex::new(None),
            emitter,
            message_id,
        }
    }

    pub(crate) fn drain_events(&self) -> Vec<ProviderStreamEvent> {
        std::mem::take(&mut *self.events.lock().expect("loop sink mutex"))
    }

    pub(crate) fn take_persist_error(&self) -> Option<EngineError> {
        self.persist_error.lock().expect("persist error mutex").take()
    }
}

#[async_trait]
impl ProviderEventSink for LoopSink<'_> {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        if let Some(payload) = map_provider_event(&event, &self.message_id) {
            if let Err(error) = self.emitter.emit(payload).await {
                *self.persist_error.lock().expect("persist error mutex") = Some(error);
                return Err(ProviderError::new(
                    pawork_domain::ProviderErrorKind::Unknown,
                    "event sink failed",
                ));
            }
        }
        self.events.lock().expect("loop sink mutex").push(event);
        Ok(())
    }
}

/// V1 `LoopSink` 单轮映射。未列出的变体只缓冲给 AssembledTurn。
pub fn map_provider_event(event: &ProviderStreamEvent, message_id: &MessageId) -> Option<AgentEvent> {
    match event {
        ProviderStreamEvent::TextDelta(delta) => Some(AgentEvent::AssistantTextDelta {
            message_id: message_id.clone(),
            delta: delta.clone(),
        }),
        ProviderStreamEvent::ThinkingDelta(delta) => Some(AgentEvent::AssistantThinkingDelta {
            message_id: message_id.clone(),
            delta: delta.clone(),
        }),
        ProviderStreamEvent::ToolCallStarted { id, name } => Some(AgentEvent::ToolCallStarted {
            tool_call_id: id.clone(),
            name: name.clone(),
        }),
        ProviderStreamEvent::ToolCallArgumentsDelta { id, json } => {
            Some(AgentEvent::ToolCallArgumentsDelta {
                tool_call_id: id.clone(),
                json_delta: json.clone(),
            })
        }
        ProviderStreamEvent::UsageUpdated(usage) => Some(AgentEvent::UsageUpdated {
            usage: usage.clone(),
        }),
        ProviderStreamEvent::ServerTool(event) => Some(AgentEvent::ServerTool(event.clone())),
        ProviderStreamEvent::TranscriptEnvelope(envelope) => {
            Some(AgentEvent::TranscriptEnvelope(envelope.clone()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::{TokenUsage, ToolCallId};

    use super::*;

    #[test]
    fn maps_text_thinking_usage_and_tool_started() {
        let id = MessageId::from("asst-1");
        assert!(matches!(
            map_provider_event(&ProviderStreamEvent::TextDelta("hi".into()), &id),
            Some(AgentEvent::AssistantTextDelta { delta, .. }) if delta == "hi"
        ));
        assert!(matches!(
            map_provider_event(&ProviderStreamEvent::ThinkingDelta("t".into()), &id),
            Some(AgentEvent::AssistantThinkingDelta { .. })
        ));
        assert!(matches!(
            map_provider_event(
                &ProviderStreamEvent::UsageUpdated(TokenUsage::default()),
                &id
            ),
            Some(AgentEvent::UsageUpdated { .. })
        ));
        assert!(matches!(
            map_provider_event(
                &ProviderStreamEvent::ToolCallStarted {
                    id: ToolCallId::from("c1"),
                    name: "read_file".into(),
                },
                &id
            ),
            Some(AgentEvent::ToolCallStarted { .. })
        ));
        assert!(map_provider_event(
            &ProviderStreamEvent::ResponseCompleted(pawork_domain::StopReason::Completed),
            &id
        )
        .is_none());
    }
}
