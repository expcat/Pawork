//! Pawork 的持久化事件协议。
//!
//! [`AgentEventEnvelope`] 是 Event Store 的最小写入单元。事件负载与信封均可
//! JSON 往返，`sequence` 在同一 Session 事件流内必须严格递增。

use std::{error::Error, fmt};

use agent_domain::{
    ArtifactId, CheckpointId, ErrorContext, EventId, Message, MessageId, ProviderId, RequestId,
    RunId, SessionId, StopReason, Timestamp, TokenUsage, ToolCallId, ToolResultContent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSequence(pub u64);

impl EventSequence {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub fn is_immediately_after(self, previous: Self) -> bool {
        previous.0.checked_add(1) == Some(self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEventEnvelope {
    pub schema_version: u32,
    pub event_id: EventId,
    pub session_id: SessionId,
    pub run_id: RunId,
    pub sequence: EventSequence,
    pub timestamp: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<EventId>,
    pub payload: AgentEvent,
}

impl AgentEventEnvelope {
    pub fn new(
        event_id: EventId,
        session_id: SessionId,
        run_id: RunId,
        sequence: EventSequence,
        timestamp: Timestamp,
        payload: AgentEvent,
    ) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            event_id,
            session_id,
            run_id,
            sequence,
            timestamp,
            parent_event_id: None,
            payload,
        }
    }

    pub fn with_parent(mut self, parent_event_id: EventId) -> Self {
        self.parent_event_id = Some(parent_event_id);
        self
    }

    pub fn validate_after(&self, previous: &Self) -> Result<(), EventOrderError> {
        if self.session_id != previous.session_id {
            return Err(EventOrderError::DifferentSession);
        }
        if !self.sequence.is_immediately_after(previous.sequence) {
            return Err(EventOrderError::NonContiguousSequence {
                previous: previous.sequence,
                next: self.sequence,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    RunStarted {
        trigger_message_id: MessageId,
    },
    ContextPrepared {
        message_count: u64,
        estimated_input_tokens: u64,
    },
    ProviderRequestStarted {
        request_id: RequestId,
        provider_id: ProviderId,
        model: String,
    },
    AssistantTextDelta {
        message_id: MessageId,
        delta: String,
    },
    AssistantThinkingDelta {
        message_id: MessageId,
        delta: String,
    },
    ToolCallStarted {
        tool_call_id: ToolCallId,
        name: String,
    },
    ToolCallArgumentsDelta {
        tool_call_id: ToolCallId,
        json_delta: String,
    },
    ToolApprovalRequested {
        tool_call_id: ToolCallId,
        reason: String,
    },
    ToolApprovalResponded {
        tool_call_id: ToolCallId,
        decision: ApprovalDecision,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comment: Option<String>,
    },
    ToolExecutionStarted {
        tool_call_id: ToolCallId,
    },
    ToolOutputDelta {
        tool_call_id: ToolCallId,
        stream: ToolOutputStream,
        delta: String,
    },
    ToolExecutionCompleted {
        tool_call_id: ToolCallId,
        result: ToolResultContent,
    },
    MessageCommitted {
        message: Message,
    },
    CompactionStarted {
        source_event_count: u64,
    },
    CompactionCompleted {
        summary_message_id: MessageId,
        compacted_through: EventSequence,
    },
    CheckpointCreated {
        checkpoint_id: CheckpointId,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        artifacts: Vec<ArtifactId>,
    },
    CheckpointRolledBack {
        checkpoint_id: CheckpointId,
    },
    RunCompleted {
        stop_reason: StopReason,
        usage: TokenUsage,
    },
    RunCancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    RunFailed {
        error: ErrorContext,
    },
    /// 向前兼容的诊断事件；未知 Provider 元数据不得污染 canonical 分支。
    Diagnostic {
        code: String,
        details: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    ApprovedOnce,
    ApprovedForRun,
    Denied,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputStream {
    Stdout,
    Stderr,
    Structured,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventOrderError {
    DifferentSession,
    NonContiguousSequence {
        previous: EventSequence,
        next: EventSequence,
    },
}

impl fmt::Display for EventOrderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DifferentSession => {
                formatter.write_str("cannot compare event order across sessions")
            }
            Self::NonContiguousSequence { previous, next } => write!(
                formatter,
                "event sequence is not contiguous: previous={}, next={}",
                previous.0, next.0
            ),
        }
    }
}

impl Error for EventOrderError {}

#[cfg(test)]
mod tests {
    use agent_domain::{ErrorCategory, MessageMetadata, MessageRole};

    use super::*;

    fn envelope(sequence: u64, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{sequence}")),
            SessionId::from("session-1"),
            RunId::from("run-1"),
            EventSequence::new(sequence),
            Timestamp::from_unix_millis(1_000 + sequence),
            payload,
        )
    }

    #[test]
    fn event_json_round_trip_preserves_version_and_payload() {
        let event = envelope(
            1,
            AgentEvent::MessageCommitted {
                message: Message {
                    id: MessageId::from("message-1"),
                    role: MessageRole::Assistant,
                    content: Vec::new(),
                    metadata: MessageMetadata::default(),
                },
            },
        );

        let json = serde_json::to_string(&event).expect("serialize event");
        let decoded: AgentEventEnvelope = serde_json::from_str(&json).expect("deserialize event");

        assert_eq!(decoded, event);
        assert_eq!(decoded.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn sequence_must_be_strictly_contiguous_within_session() {
        let first = envelope(
            41,
            AgentEvent::RunStarted {
                trigger_message_id: MessageId::from("message-1"),
            },
        );
        let next = envelope(42, AgentEvent::RunCancelled { reason: None });
        let skipped = envelope(
            44,
            AgentEvent::RunFailed {
                error: ErrorContext {
                    category: ErrorCategory::Internal,
                    message: "boom".into(),
                    retryable: false,
                    retry_after_ms: None,
                    diagnostics: Default::default(),
                },
            },
        );

        assert_eq!(next.validate_after(&first), Ok(()));
        assert_eq!(
            skipped.validate_after(&next),
            Err(EventOrderError::NonContiguousSequence {
                previous: EventSequence(42),
                next: EventSequence(44),
            })
        );
    }

    #[test]
    fn parent_event_is_serialized_for_causal_replay() {
        let event = envelope(
            2,
            AgentEvent::RunCancelled {
                reason: Some("user".into()),
            },
        )
        .with_parent(EventId::from("event-1"));
        let value = serde_json::to_value(&event).expect("serialize event");

        assert_eq!(value["parent_event_id"], "event-1");
    }
}
