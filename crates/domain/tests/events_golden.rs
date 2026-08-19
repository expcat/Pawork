//! `AgentEventEnvelope` serde golden。
//!
//! V1 `agent-events` 没有检入的 JSON 夹具（只有 4 个 in-crate unit test）。
//! 本文件按迁移词典 §6.1「缺失则补」锁定 V1 完整形状：32 个 `AgentEvent`
//! 变体各一条信封，字节级比对 `serde_json::to_string`（字段序 = 结构体声明序）。
//!
//! 重新生成（仅在有意刷新契约时）：
//! `PAWORK_WRITE_EVENT_GOLDEN=1 cargo test -p pawork-domain --test events_golden -- --exact write_event_envelope_golden --ignored`

use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, ArtifactId, AutomationEvent,
    AutomationTriggerKind, BackgroundTaskId, CheckpointId, Citation, CitationSourceKind,
    CriterionKind, CURRENT_SCHEMA_VERSION, ErrorCategory, ErrorContext, EventId, EventSequence,
    GoalEvent, GoalId, MemoryEvent, MemoryId, MemoryPrivacy, Message, MessageId, MessageMetadata,
    MessageRole, MonitorEvent, MonitorId, MonitorSourceKind, PlanEvent, PlanId, PlanStepId,
    PlanStepSnapshot, PlanStepStatus, PlanVersionId, ProgramStream, ProviderId,
    ProviderTranscriptContinuation, ProviderTranscriptEnvelope, RequestId, ReviewEvent,
    ReviewSessionId, RunId, ServerToolEvent, SessionId, StopReason, SuccessCriterionSnapshot,
    TaskEvent, TaskKind, Timestamp, TokenUsage, ToolCallId, ToolKind, ToolOutputStream,
    ToolResultContent, TranscriptItem,
};
use serde_json::Value;

const VARIANTS_GOLDEN: &str = include_str!("fixtures/agent_event_envelope_variants.jsonl");
const PARENT_GOLDEN: &str = include_str!("fixtures/agent_event_envelope_parent.json");

fn envelope(sequence: u64, payload: AgentEvent) -> AgentEventEnvelope {
    AgentEventEnvelope::new(
        EventId::from(format!("event-{sequence}")),
        SessionId::from("session-1"),
        RunId::from("run-1"),
        EventSequence::new(sequence),
        Timestamp::from_unix_millis(1_000_000 + sequence),
        payload,
    )
}

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 10,
        output_tokens: 4,
        cache_read_tokens: 2,
        cache_write_tokens: 1,
    }
}

fn variant_payloads() -> Vec<AgentEvent> {
    vec![
        AgentEvent::RunStarted {
            trigger_message_id: MessageId::from("message-1"),
        },
        AgentEvent::ContextPrepared {
            message_count: 3,
            estimated_input_tokens: 128,
        },
        AgentEvent::ProviderRequestStarted {
            request_id: RequestId::from("request-1"),
            provider_id: ProviderId::from("glm-coding"),
            model: "glm-5.2".into(),
        },
        AgentEvent::UsageUpdated { usage: usage() },
        AgentEvent::AssistantTextDelta {
            message_id: MessageId::from("message-2"),
            delta: "hello".into(),
        },
        AgentEvent::AssistantThinkingDelta {
            message_id: MessageId::from("message-2"),
            delta: "think".into(),
        },
        AgentEvent::ToolCallStarted {
            tool_call_id: ToolCallId::from("tool-1"),
            name: "read_file".into(),
        },
        AgentEvent::ToolCallArgumentsDelta {
            tool_call_id: ToolCallId::from("tool-1"),
            json_delta: r#"{"path":"README.md"}"#.into(),
        },
        AgentEvent::ToolApprovalRequested {
            tool_call_id: ToolCallId::from("tool-1"),
            reason: "read workspace file".into(),
        },
        AgentEvent::ToolApprovalResponded {
            tool_call_id: ToolCallId::from("tool-1"),
            decision: ApprovalDecision::ApprovedOnce,
            comment: None,
        },
        AgentEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::from("tool-1"),
        },
        AgentEvent::ToolOutputDelta {
            tool_call_id: ToolCallId::from("tool-1"),
            stream: ToolOutputStream::Stdout,
            delta: "ok".into(),
        },
        AgentEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::from("tool-1"),
            result: ToolResultContent {
                tool_call_id: ToolCallId::from("tool-1"),
                tool_name: Some("read_file".into()),
                content: Vec::new(),
                is_error: false,
                metadata: Value::Null,
                artifacts: Vec::new(),
            },
        },
        AgentEvent::MessageCommitted {
            message: Message {
                id: MessageId::from("message-2"),
                role: MessageRole::Assistant,
                content: Vec::new(),
                metadata: MessageMetadata::default(),
            },
        },
        AgentEvent::ProviderTranscriptContinued {
            calls: vec![ProviderTranscriptContinuation {
                tool_call_id: ToolCallId::from("tool-2"),
                name: "web_search".into(),
                kind: ToolKind::ProviderHosted,
            }],
        },
        AgentEvent::ServerTool(ServerToolEvent::CitationAdded {
            tool_call_id: ToolCallId::from("server-tool-1"),
            citation: Citation {
                url: Some("https://example.com".into()),
                source_kind: CitationSourceKind::Url,
                ..Citation::empty()
            },
        }),
        AgentEvent::TranscriptEnvelope(ProviderTranscriptEnvelope {
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
        }),
        AgentEvent::CompactionStarted {
            source_event_count: 40,
        },
        AgentEvent::CompactionCompleted {
            summary_message_id: MessageId::from("summary-1"),
            compacted_through: EventSequence::new(40),
        },
        AgentEvent::CheckpointCreated {
            checkpoint_id: CheckpointId::from("checkpoint-1"),
            artifacts: Vec::new(),
        },
        AgentEvent::CheckpointRolledBack {
            checkpoint_id: CheckpointId::from("checkpoint-1"),
        },
        AgentEvent::RunCompleted {
            stop_reason: StopReason::Completed,
            usage: usage(),
        },
        AgentEvent::RunCancelled {
            reason: None,
            usage: None,
        },
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
        AgentEvent::Plan(PlanEvent::Created {
            plan_id: PlanId::from("plan-1"),
            version: PlanVersionId::from("plan-ver-1"),
            title: "first plan".into(),
            steps: vec![PlanStepSnapshot {
                step_id: PlanStepId::from("step-1"),
                text: "do work".into(),
                status: PlanStepStatus::Pending,
            }],
        }),
        AgentEvent::Goal(GoalEvent::Created {
            goal_id: GoalId::from("goal-1"),
            title: "ship s1".into(),
            criteria: vec![SuccessCriterionSnapshot {
                criterion_id: "c1".into(),
                description: "envelope golden green".into(),
                kind: CriterionKind::Auto,
                satisfied: false,
            }],
        }),
        AgentEvent::Task(TaskEvent::Started {
            task_id: BackgroundTaskId::from("task-1"),
            task_kind: TaskKind::Process,
            parent_task_id: None,
        }),
        AgentEvent::Automation(AutomationEvent::Registered {
            automation_id: pawork_domain::AutomationId::from("auto-1"),
            trigger: AutomationTriggerKind::Cron,
        }),
        AgentEvent::Monitor(MonitorEvent::Started {
            monitor_id: MonitorId::from("mon-1"),
            source: MonitorSourceKind::FileChange,
            workspace_id: None,
        }),
        AgentEvent::Memory(MemoryEvent::Recorded {
            memory_id: MemoryId::from("mem-1"),
            summary: "note".into(),
            source_event_id: None,
            privacy: MemoryPrivacy::WorkspaceLocal,
            workspace_id: None,
            embedding: Vec::new(),
            confidence: 0.0,
        }),
        AgentEvent::Review(ReviewEvent::SessionCreated {
            session_id: ReviewSessionId::from("review-1"),
            workspace_id: None,
        }),
        AgentEvent::Diagnostic {
            code: "capability_gap".into(),
            details: serde_json::json!({"hint": "none"}),
        },
    ]
}

fn variant_envelopes() -> Vec<AgentEventEnvelope> {
    variant_payloads()
        .into_iter()
        .enumerate()
        .map(|(index, payload)| envelope(index as u64 + 1, payload))
        .collect()
}

fn payload_type_name(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::RunStarted { .. } => "run_started",
        AgentEvent::ContextPrepared { .. } => "context_prepared",
        AgentEvent::ProviderRequestStarted { .. } => "provider_request_started",
        AgentEvent::UsageUpdated { .. } => "usage_updated",
        AgentEvent::AssistantTextDelta { .. } => "assistant_text_delta",
        AgentEvent::AssistantThinkingDelta { .. } => "assistant_thinking_delta",
        AgentEvent::ToolCallStarted { .. } => "tool_call_started",
        AgentEvent::ToolCallArgumentsDelta { .. } => "tool_call_arguments_delta",
        AgentEvent::ToolApprovalRequested { .. } => "tool_approval_requested",
        AgentEvent::ToolApprovalResponded { .. } => "tool_approval_responded",
        AgentEvent::ToolExecutionStarted { .. } => "tool_execution_started",
        AgentEvent::ToolOutputDelta { .. } => "tool_output_delta",
        AgentEvent::ToolExecutionCompleted { .. } => "tool_execution_completed",
        AgentEvent::MessageCommitted { .. } => "message_committed",
        AgentEvent::ProviderTranscriptContinued { .. } => "provider_transcript_continued",
        AgentEvent::ServerTool(_) => "server_tool",
        AgentEvent::TranscriptEnvelope(_) => "transcript_envelope",
        AgentEvent::CompactionStarted { .. } => "compaction_started",
        AgentEvent::CompactionCompleted { .. } => "compaction_completed",
        AgentEvent::CheckpointCreated { .. } => "checkpoint_created",
        AgentEvent::CheckpointRolledBack { .. } => "checkpoint_rolled_back",
        AgentEvent::RunCompleted { .. } => "run_completed",
        AgentEvent::RunCancelled { .. } => "run_cancelled",
        AgentEvent::RunFailed { .. } => "run_failed",
        AgentEvent::Plan(_) => "plan",
        AgentEvent::Goal(_) => "goal",
        AgentEvent::Task(_) => "task",
        AgentEvent::Automation(_) => "automation",
        AgentEvent::Monitor(_) => "monitor",
        AgentEvent::Memory(_) => "memory",
        AgentEvent::Review(_) => "review",
        AgentEvent::Diagnostic { .. } => "diagnostic",
    }
}

fn render_variants_jsonl(envelopes: &[AgentEventEnvelope]) -> String {
    let mut out = String::new();
    for envelope in envelopes {
        out.push_str(&serde_json::to_string(envelope).expect("serialize envelope"));
        out.push('\n');
    }
    out
}

fn parent_envelope() -> AgentEventEnvelope {
    envelope(
        2,
        AgentEvent::RunCancelled {
            reason: Some("user".into()),
            usage: None,
        },
    )
    .with_parent(EventId::from("event-1"))
}

#[test]
fn all_thirty_two_agent_event_variants_round_trip() {
    let envelopes = variant_envelopes();
    assert_eq!(envelopes.len(), 32, "AgentEvent must stay at 32 variants");

    let mut seen = std::collections::BTreeSet::new();
    for envelope in &envelopes {
        assert_eq!(envelope.schema_version, CURRENT_SCHEMA_VERSION);
        let name = payload_type_name(&envelope.payload);
        assert!(seen.insert(name), "duplicate variant fixture: {name}");

        let json = serde_json::to_string(envelope).expect("serialize envelope");
        let decoded: AgentEventEnvelope =
            serde_json::from_str(&json).expect("deserialize envelope");
        assert_eq!(decoded, *envelope);
        assert!(
            !json.contains("api_key") && !json.contains("secret"),
            "envelope payload must not carry secret fragments: {json}"
        );
    }
    assert_eq!(seen.len(), 32);
}

#[test]
fn variant_envelopes_match_checked_in_jsonl_bytes() {
    let actual = render_variants_jsonl(&variant_envelopes());
    assert_eq!(
        actual, VARIANTS_GOLDEN,
        "AgentEventEnvelope serde bytes drifted; regenerate only with an explicit contract change"
    );
}

#[test]
fn parent_envelope_matches_checked_in_json_bytes() {
    let actual = serde_json::to_string(&parent_envelope()).expect("serialize parent envelope");
    let expected = PARENT_GOLDEN.trim_end();
    assert_eq!(actual, expected);
}

#[test]
#[ignore = "set PAWORK_WRITE_EVENT_GOLDEN=1 to refresh fixtures"]
fn write_event_envelope_golden() {
    assert_eq!(
        std::env::var("PAWORK_WRITE_EVENT_GOLDEN").ok().as_deref(),
        Some("1"),
        "refusing to overwrite golden without PAWORK_WRITE_EVENT_GOLDEN=1"
    );
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    std::fs::create_dir_all(dir).expect("create fixtures dir");
    std::fs::write(
        format!("{dir}/agent_event_envelope_variants.jsonl"),
        render_variants_jsonl(&variant_envelopes()),
    )
    .expect("write variants golden");
    std::fs::write(
        format!("{dir}/agent_event_envelope_parent.json"),
        format!(
            "{}\n",
            serde_json::to_string(&parent_envelope()).expect("serialize parent")
        ),
    )
    .expect("write parent golden");
}
