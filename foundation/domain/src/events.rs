//! Pawork 的持久化事件协议。
//!
//! [`AgentEventEnvelope`] 是 Event Store 的最小写入单元。事件负载与信封均可
//! JSON 往返，`sequence` 在同一 Session 事件流内必须严格递增。
//!
//! 本模块由 V1 `agent-events` 整包并入。信封 [`CURRENT_SCHEMA_VERSION`] 是
//! **磁盘/线上契约版本（值为 1）**，与 session-store 的 DB migration 版本
//! （`CURRENT_SCHEMA_VERSION = 9`）相互独立，不得混用。

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ArtifactId, AutomationEvent, CheckpointId, ErrorContext, EventId, GoalEvent, MemoryEvent,
    Message, MessageId, MonitorEvent, PlanEvent, ProviderId, ProviderTranscriptEnvelope, RequestId,
    ReviewEvent, RunId, ServerToolEvent, SessionId, StopReason, TaskEvent, Timestamp, TokenUsage,
    ToolCallId, ToolKind, ToolResultContent,
};

/// 事件信封契约版本。值必须保持为 `1`；不是 SQLite schema 迁移号。
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
    /// Provider 流式用量快照（canonical）：LoopSink 把
    /// `ProviderStreamEvent::UsageUpdated` 广播到事件流，监督器据此捕获「最近一次
    /// 观测到的用量」，确保失败/取消时已发生用量不丢失。
    UsageUpdated {
        usage: TokenUsage,
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
    /// Provider-owned 调用（Hosted / Extension）已成功 dispatch，经 Provider
    /// transcript 续接；Core 不本地执行、不生成 `ToolResult`。
    ///
    /// 仅在「本轮全部为 Provider-owned 调用」时发出，携带单步
    /// `CollectingToolCalls → WaitingForProvider` 转换，可被崩溃重放无损重建。
    ProviderTranscriptContinued {
        calls: Vec<ProviderTranscriptContinuation>,
    },
    /// Provider 归一后的 server tool 生命周期事件（P15-5）。
    ///
    /// 与本地 `ToolCall*` 事件并列但语义分离：hosted / extension 工具由
    /// Provider 服务端执行，Core 只归一与持久化，不生成本地 `ToolResult`、
    /// 不触发 scheduler。
    ServerTool(ServerToolEvent),
    /// Provider transcript 续传信封（provider-neutral，持久化前脱敏）。
    ///
    /// 携带归一化 output item / cursor / continuation reference；不携带 Provider
    /// 名称，具体协议翻译封装在 provider adapter。
    TranscriptEnvelope(ProviderTranscriptEnvelope),
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
        /// 失败/取消前已观测到的累计用量；缺省兼容旧行。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
    },
    RunFailed {
        error: ErrorContext,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
    },
    /// Phase 16 P16-1/P16-2 Plan Mode 事件（只读计划与评审/审批 gate）。
    Plan(PlanEvent),
    /// Phase 16 P16-3 Goal Mode 事件（目标、成功标准、进度与转向）。
    Goal(GoalEvent),
    /// Phase 16 P16-4 Background Task Manager 事件（统一四 kind 任务）。
    Task(TaskEvent),
    /// Phase 16 P16-5 Scheduled Automation 事件（cron/interval/once/event + inbox）。
    Automation(AutomationEvent),
    /// Phase 16 P16-6 Persistent Process / Monitor 事件（常驻进程与监视循环）。
    Monitor(MonitorEvent),
    /// Phase 16 P16-7 Long-term Memory 事件（只读提炼、嵌入检索、失效）。
    Memory(MemoryEvent),
    /// Phase 16 P16-8 Review Engine 事件（行锚点评审与 resolution）。
    Review(ReviewEvent),
    /// 向前兼容的诊断事件；未知 Provider 元数据不得污染 canonical 分支。
    Diagnostic {
        code: String,
        details: Value,
    },
}

/// 一条已交给 Provider transcript 续接的调用（provider-neutral）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderTranscriptContinuation {
    pub tool_call_id: ToolCallId,
    pub name: String,
    pub kind: ToolKind,
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
    use crate::{ErrorCategory, MessageMetadata, MessageRole};

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
        let next = envelope(
            42,
            AgentEvent::RunCancelled {
                reason: None,
                usage: None,
            },
        );
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
                usage: None,
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
    fn different_session_cannot_be_ordered() {
        let first = envelope(
            1,
            AgentEvent::RunStarted {
                trigger_message_id: MessageId::from("message-1"),
            },
        );
        let mut other = envelope(
            2,
            AgentEvent::RunCancelled {
                reason: None,
                usage: None,
            },
        );
        other.session_id = SessionId::from("session-other");

        assert_eq!(
            other.validate_after(&first),
            Err(EventOrderError::DifferentSession)
        );
    }

    #[test]
    fn parent_event_is_serialized_for_causal_replay() {
        let event = envelope(
            2,
            AgentEvent::RunCancelled {
                reason: Some("user".into()),
                usage: None,
            },
        )
        .with_parent(EventId::from("event-1"));
        let value = serde_json::to_value(&event).expect("serialize event");

        assert_eq!(value["parent_event_id"], "event-1");
    }

    #[test]
    fn parent_event_is_omitted_when_absent() {
        let event = envelope(
            1,
            AgentEvent::RunCancelled {
                reason: None,
                usage: None,
            },
        );
        let value = serde_json::to_value(&event).expect("serialize event");
        assert!(value.get("parent_event_id").is_none());
    }

    #[test]
    fn approval_decision_uses_events_snake_case_not_core_api_verbs() {
        let names = [
            (ApprovalDecision::ApprovedOnce, "approved_once"),
            (ApprovalDecision::ApprovedForRun, "approved_for_run"),
            (ApprovalDecision::Denied, "denied"),
            (ApprovalDecision::Cancelled, "cancelled"),
        ];
        for (decision, expected) in names {
            let json = serde_json::to_string(&decision).expect("serialize decision");
            assert_eq!(json, format!("\"{expected}\""));
        }
    }

    #[test]
    fn tool_output_stream_uses_snake_case() {
        let names = [
            (ToolOutputStream::Stdout, "stdout"),
            (ToolOutputStream::Stderr, "stderr"),
            (ToolOutputStream::Structured, "structured"),
        ];
        for (stream, expected) in names {
            let json = serde_json::to_string(&stream).expect("serialize stream");
            assert_eq!(json, format!("\"{expected}\""));
        }
    }

    #[test]
    fn server_tool_and_transcript_envelope_are_persistable_agent_events() {
        use crate::{Citation, CitationSourceKind, ProgramStream, TranscriptItem};

        let server_tool = AgentEvent::ServerTool(ServerToolEvent::CitationAdded {
            tool_call_id: ToolCallId::from("server-tool-1"),
            citation: Citation {
                url: Some("https://example.com".into()),
                source_kind: CitationSourceKind::Url,
                ..Citation::empty()
            },
        });
        let transcript = AgentEvent::TranscriptEnvelope(ProviderTranscriptEnvelope {
            items: vec![
                TranscriptItem::ServerTool(ServerToolEvent::ProgramOutput {
                    tool_call_id: ToolCallId::from("server-tool-1"),
                    stream: ProgramStream::Stderr,
                    delta: None,
                    artifact: Some(ArtifactId::from("artifact-log-1")),
                }),
                TranscriptItem::Text("done".into()),
            ],
            cursor: None,
            continuation_reference: Some("ref-1".into()),
        });

        for payload in [server_tool, transcript] {
            let event = envelope(7, payload);
            let json = serde_json::to_string(&event).expect("serialize event");
            let decoded: AgentEventEnvelope =
                serde_json::from_str(&json).expect("deserialize event");
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn run_cancelled_and_failed_usage_is_additive() {
        let legacy_cancelled: AgentEvent =
            serde_json::from_str(r#"{"type":"run_cancelled","data":{}}"#).expect("legacy");
        assert_eq!(
            legacy_cancelled,
            AgentEvent::RunCancelled {
                reason: None,
                usage: None
            }
        );

        let with_usage = AgentEvent::RunCancelled {
            reason: Some("user".into()),
            usage: Some(TokenUsage {
                input_tokens: 2,
                output_tokens: 1,
                ..TokenUsage::default()
            }),
        };
        let value = serde_json::to_value(&with_usage).expect("serialize");
        assert_eq!(value["data"]["usage"]["input_tokens"], 2);
        let decoded: AgentEvent = serde_json::from_value(value).expect("round-trip");
        assert_eq!(decoded, with_usage);

        let omitted = AgentEvent::RunFailed {
            error: ErrorContext {
                category: ErrorCategory::Internal,
                message: "boom".into(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
            usage: None,
        };
        let value = serde_json::to_value(&omitted).expect("serialize omitted");
        assert!(value["data"].get("usage").is_none());
    }
}
