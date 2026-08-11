//! Interrupted Run 恢复（P3-10）。
//!
//! 重启后扫描未完成的 Run（重启前处于活跃态、因进程退出被遗留），从其事件流
//! 重放重建状态机与消息历史，并产出可 resume 的恢复计划。
//!
//! 「可重放」承诺的落地：事件是事实来源（ADR-016），Run 状态可由事件序列
//! 完全重建，因此崩溃后无需额外持久化「当前状态」——只需重放事件即可恢复。

use std::collections::BTreeMap;

use agent_domain::{Message, MessageId, RunId};
use agent_events::{AgentEvent, AgentEventEnvelope};

use crate::state::{RunState, RunStateMachine, RunTransition, TransitionError};

/// 一条 Run 的事件流（按 sequence 升序）。
pub type RunEventLog = Vec<AgentEventEnvelope>;

/// 恢复诊断：一个 Run 重放后的重建结果。
#[derive(Clone, Debug, PartialEq)]
pub struct RecoveryPlan {
    pub run_id: RunId,
    /// 重放重建出的 Run 状态（应等于持久化投影）。
    pub recovered_state: RunState,
    /// 重建出的消息历史。
    pub messages: Vec<Message>,
    /// 重放是否检测到不一致（非法转换 / 缺失事件）。
    pub issues: Vec<RecoveryIssue>,
    /// 该 Run 是否可 resume（活跃态或被标记 Interrupted）。
    pub resumable: bool,
}

/// 重放期间发现的问题。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryIssue {
    IllegalTransition { from: RunState, error: String },
    MissingStartEvent,
    DuplicateMessage { message_id: MessageId },
    UnknownEvent,
}

/// 从事件流重放重建单个 Run 的状态与消息历史。
///
/// 纯函数：不执行 IO，时间复杂度 O(n)。一个 Run 的事件流通常在 KB~MB 级，
/// 重放远低于 1s 目标。
pub fn replay_run(events: &RunEventLog) -> RecoveryPlan {
    let mut sm = RunStateMachine::new();
    let mut messages: Vec<Message> = Vec::new();
    let mut seen_messages: BTreeMap<MessageId, usize> = BTreeMap::new();
    let mut issues = Vec::new();

    let run_id = events
        .first()
        .map(|e| e.run_id.clone())
        .unwrap_or_else(|| RunId::from("unknown"));

    let mut started = false;

    for envelope in events {
        let transitions = match &envelope.payload {
            AgentEvent::RunStarted { .. } => {
                started = true;
                vec![RunTransition::Begin]
            }
            AgentEvent::ContextPrepared { .. } => vec![RunTransition::ContextPrepared],
            AgentEvent::ProviderRequestStarted { .. } => vec![RunTransition::ProviderStarted],
            AgentEvent::ToolApprovalRequested { .. }
                if matches!(sm.state(), RunState::CollectingToolCalls) =>
            {
                vec![RunTransition::ApprovalRequested]
            }
            AgentEvent::ToolExecutionStarted { .. }
                if matches!(sm.state(), RunState::CollectingToolCalls) =>
            {
                vec![RunTransition::ToolsAutoStarted]
            }
            AgentEvent::ToolExecutionStarted { .. }
                if matches!(sm.state(), RunState::WaitingForApproval) =>
            {
                vec![RunTransition::ApprovalGranted]
            }
            AgentEvent::ToolExecutionCompleted { .. }
                if matches!(sm.state(), RunState::ExecutingTools) =>
            {
                vec![RunTransition::ToolsCompleted]
            }
            AgentEvent::MessageCommitted { message } => {
                if seen_messages
                    .insert(message.id.clone(), messages.len())
                    .is_some()
                {
                    issues.push(RecoveryIssue::DuplicateMessage {
                        message_id: message.id.clone(),
                    });
                }
                messages.push(message.clone());
                match sm.state() {
                    RunState::StreamingResponse
                        if message.role == agent_domain::MessageRole::Assistant
                            && message.content.iter().any(|part| {
                                matches!(part, agent_domain::ContentPart::ToolCall(_))
                            }) =>
                    {
                        vec![RunTransition::StreamFinished {
                            has_tool_calls: true,
                        }]
                    }
                    RunState::StreamingResponse
                        if message.role == agent_domain::MessageRole::User =>
                    {
                        vec![RunTransition::QueuedMessageAppended]
                    }
                    RunState::WaitingForApproval
                        if message.role == agent_domain::MessageRole::Tool =>
                    {
                        vec![
                            RunTransition::ApprovalDenied,
                            RunTransition::ResultsAppended,
                        ]
                    }
                    RunState::AppendingToolResults
                        if message.role == agent_domain::MessageRole::Tool =>
                    {
                        vec![RunTransition::ResultsAppended]
                    }
                    _ => Vec::new(),
                }
            }
            AgentEvent::RunCompleted { .. } => vec![RunTransition::Complete],
            AgentEvent::RunCancelled { .. } => vec![RunTransition::Cancel],
            AgentEvent::RunFailed { .. } => vec![RunTransition::Fail],
            // 全部为 Provider-owned 的轮次：单步 CollectingToolCalls → WaitingForProvider。
            AgentEvent::ProviderTranscriptContinued { .. }
                if matches!(sm.state(), RunState::CollectingToolCalls) =>
            {
                vec![RunTransition::ProviderTranscriptContinued]
            }
            // 这些事件不产生独立状态转换，但用于审计。
            AgentEvent::AssistantTextDelta { .. }
            | AgentEvent::AssistantThinkingDelta { .. }
            | AgentEvent::ToolCallStarted { .. }
            | AgentEvent::ToolCallArgumentsDelta { .. }
            | AgentEvent::ToolApprovalResponded { .. }
            | AgentEvent::ToolOutputDelta { .. }
            | AgentEvent::ToolApprovalRequested { .. }
            | AgentEvent::ToolExecutionStarted { .. }
            | AgentEvent::ToolExecutionCompleted { .. }
            | AgentEvent::ProviderTranscriptContinued { .. }
            | AgentEvent::ServerTool(_)
            | AgentEvent::TranscriptEnvelope(_)
            | AgentEvent::CompactionStarted { .. }
            | AgentEvent::CompactionCompleted { .. }
            | AgentEvent::CheckpointCreated { .. }
            | AgentEvent::CheckpointRolledBack { .. }
            | AgentEvent::UsageUpdated { .. }
            | AgentEvent::Diagnostic { .. } => Vec::new(),
        };

        for t in transitions {
            match sm.apply(t) {
                Ok(_) => {}
                Err(TransitionError::Illegal { state, transition }) => {
                    issues.push(RecoveryIssue::IllegalTransition {
                        from: state,
                        error: format!("{transition:?}"),
                    });
                }
                Err(TransitionError::FromTerminal { state, .. }) => {
                    issues.push(RecoveryIssue::IllegalTransition {
                        from: state,
                        error: "transition from terminal".into(),
                    });
                }
            }
        }
    }

    if !started {
        issues.push(RecoveryIssue::MissingStartEvent);
    }

    // 事件流结束时仍未到终态的 Run 视为 Interrupted（进程崩溃遗留）；
    // 存在问题的活跃态也归为 Interrupted，便于人工/自动恢复决策。
    let recovered_state = if sm.state().is_terminal() {
        sm.state()
    } else {
        RunState::Interrupted
    };

    let resumable = matches!(recovered_state, RunState::Interrupted) || recovered_state.is_active();

    RecoveryPlan {
        run_id,
        recovered_state,
        messages,
        issues,
        resumable,
    }
}

/// 从一组 Run 的事件流中，筛出需要恢复的 Run（活跃或 Interrupted）。
pub fn group_by_run(events: &[AgentEventEnvelope]) -> BTreeMap<RunId, RunEventLog> {
    let mut groups: BTreeMap<RunId, RunEventLog> = BTreeMap::new();
    for e in events {
        groups.entry(e.run_id.clone()).or_default().push(e.clone());
    }
    for log in groups.values_mut() {
        log.sort_by_key(|e| e.sequence);
    }
    groups
}

/// 扫描所有 Run，返回需要恢复的计划（活跃或 Interrupted）。
pub fn scan_interrupted(events: &[AgentEventEnvelope]) -> Vec<RecoveryPlan> {
    group_by_run(events)
        .values()
        .map(replay_run)
        .filter(|plan| {
            matches!(plan.recovered_state, RunState::Interrupted)
                || plan.recovered_state.is_active()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_events::EventSequence;
    use std::time::Instant;

    use agent_domain::{
        ContentPart, EventId, MessageId, MessageMetadata, MessageRole, RequestId, SessionId,
        StopReason, TextContent, Timestamp, TokenUsage,
    };

    fn env(seq: u64, run: &str, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("e-{seq}")),
            SessionId::from("s-1"),
            RunId::from(run),
            EventSequence::new(seq),
            Timestamp::from_unix_millis(seq),
            payload,
        )
    }

    fn msg(id: &str, text: &str) -> Message {
        Message {
            id: MessageId::from(id),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: text.into() })],
            metadata: MessageMetadata::default(),
        }
    }

    fn assistant_tool_message(id: &str) -> Message {
        Message {
            id: MessageId::from(id),
            role: MessageRole::Assistant,
            content: vec![ContentPart::ToolCall(agent_domain::ToolCallContent {
                id: agent_domain::ToolCallId::from("call-1"),
                name: "read".into(),
                arguments: serde_json::json!({"path": "README.md"}),
                raw_arguments: Some(r#"{"path":"README.md"}"#.into()),
                complete: true,
            })],
            metadata: MessageMetadata::default(),
        }
    }

    fn tool_result_message(id: &str) -> Message {
        Message {
            id: MessageId::from(id),
            role: MessageRole::Tool,
            content: vec![ContentPart::ToolResult(agent_domain::ToolResultContent {
                tool_call_id: agent_domain::ToolCallId::from("call-1"),
                tool_name: Some("read".into()),
                content: vec![ContentPart::Text(TextContent { text: "ok".into() })],
                is_error: false,
                metadata: serde_json::Value::Null,
            })],
            metadata: MessageMetadata::default(),
        }
    }

    #[test]
    fn replays_completed_run_and_recovers_messages() {
        let events = vec![
            env(
                1,
                "run-1",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("m-1"),
                },
            ),
            env(
                2,
                "run-1",
                AgentEvent::ContextPrepared {
                    message_count: 1,
                    estimated_input_tokens: 5,
                },
            ),
            env(
                3,
                "run-1",
                AgentEvent::MessageCommitted {
                    message: msg("m-1", "hi"),
                },
            ),
            env(
                4,
                "run-1",
                AgentEvent::RunCompleted {
                    stop_reason: StopReason::Completed,
                    usage: TokenUsage::default(),
                },
            ),
        ];
        let plan = replay_run(&events);
        assert_eq!(plan.recovered_state, RunState::Completed);
        assert_eq!(plan.messages.len(), 1);
        assert!(plan.issues.is_empty());
        assert!(!plan.resumable);
    }

    #[test]
    fn interrupted_active_run_is_detected_as_interrupted() {
        let events = vec![
            env(
                1,
                "run-2",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("m-1"),
                },
            ),
            env(
                2,
                "run-2",
                AgentEvent::ContextPrepared {
                    message_count: 0,
                    estimated_input_tokens: 0,
                },
            ),
            env(
                3,
                "run-2",
                AgentEvent::ProviderRequestStarted {
                    request_id: RequestId::from("r-1"),
                    provider_id: agent_domain::ProviderId::from("mock"),
                    model: "m".into(),
                },
            ),
        ];
        let plans = scan_interrupted(&events);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].recovered_state, RunState::Interrupted);
        assert!(plans[0].resumable);
    }

    #[test]
    fn interrupted_run_recovers_reasoning_continuation_reference() {
        let reasoning = agent_domain::ReasoningItem {
            id: agent_domain::ReasoningItemId::from("reasoning-1"),
            summary: Some("safe summary".into()),
            protected_blob_ref: agent_domain::ProtectedBlobRef::from("protected-1"),
            opaque_metadata: BTreeMap::new(),
            continuation_metadata: BTreeMap::new(),
        };
        let message = Message {
            id: MessageId::from("assistant-reasoning"),
            role: MessageRole::Assistant,
            content: vec![ContentPart::Reasoning(reasoning.clone())],
            metadata: MessageMetadata::default(),
        };
        let events = vec![
            env(
                1,
                "run-reasoning",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("user-1"),
                },
            ),
            env(
                2,
                "run-reasoning",
                AgentEvent::ContextPrepared {
                    message_count: 1,
                    estimated_input_tokens: 1,
                },
            ),
            env(
                3,
                "run-reasoning",
                AgentEvent::ProviderRequestStarted {
                    request_id: RequestId::from("request-1"),
                    provider_id: agent_domain::ProviderId::from("mock"),
                    model: "mock".into(),
                },
            ),
            env(4, "run-reasoning", AgentEvent::MessageCommitted { message }),
        ];

        let plan = replay_run(&events);
        assert_eq!(plan.recovered_state, RunState::Interrupted);
        assert!(plan.resumable);
        assert_eq!(
            plan.messages[0].content,
            vec![ContentPart::Reasoning(reasoning)]
        );
    }

    #[test]
    fn tool_round_replays_without_illegal_transition() {
        let events = vec![
            env(
                1,
                "run-tools",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("user-1"),
                },
            ),
            env(
                2,
                "run-tools",
                AgentEvent::ContextPrepared {
                    message_count: 1,
                    estimated_input_tokens: 1,
                },
            ),
            env(
                3,
                "run-tools",
                AgentEvent::ProviderRequestStarted {
                    request_id: RequestId::from("request-1"),
                    provider_id: agent_domain::ProviderId::from("mock"),
                    model: "mock".into(),
                },
            ),
            env(
                4,
                "run-tools",
                AgentEvent::MessageCommitted {
                    message: assistant_tool_message("assistant-1"),
                },
            ),
            env(
                5,
                "run-tools",
                AgentEvent::ToolApprovalRequested {
                    tool_call_id: agent_domain::ToolCallId::from("call-1"),
                    reason: "test".into(),
                },
            ),
            env(
                6,
                "run-tools",
                AgentEvent::ToolExecutionStarted {
                    tool_call_id: agent_domain::ToolCallId::from("call-1"),
                },
            ),
            env(
                7,
                "run-tools",
                AgentEvent::ToolExecutionCompleted {
                    tool_call_id: agent_domain::ToolCallId::from("call-1"),
                    result: agent_domain::ToolResultContent {
                        tool_call_id: agent_domain::ToolCallId::from("call-1"),
                        tool_name: Some("read".into()),
                        content: Vec::new(),
                        is_error: false,
                        metadata: serde_json::Value::Null,
                    },
                },
            ),
            env(
                8,
                "run-tools",
                AgentEvent::MessageCommitted {
                    message: tool_result_message("tool-1"),
                },
            ),
            env(
                9,
                "run-tools",
                AgentEvent::ProviderRequestStarted {
                    request_id: RequestId::from("request-2"),
                    provider_id: agent_domain::ProviderId::from("mock"),
                    model: "mock".into(),
                },
            ),
            env(
                10,
                "run-tools",
                AgentEvent::RunCompleted {
                    stop_reason: StopReason::Completed,
                    usage: TokenUsage::default(),
                },
            ),
        ];

        let plan = replay_run(&events);
        assert_eq!(plan.recovered_state, RunState::Completed);
        assert!(
            plan.issues.is_empty(),
            "unexpected issues: {:?}",
            plan.issues
        );
        assert_eq!(plan.messages.len(), 2);
    }

    #[test]
    fn all_hosted_round_replays_to_waiting_for_provider() {
        let events = vec![
            env(
                1,
                "run-hosted",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("user-1"),
                },
            ),
            env(
                2,
                "run-hosted",
                AgentEvent::ContextPrepared {
                    message_count: 1,
                    estimated_input_tokens: 1,
                },
            ),
            env(
                3,
                "run-hosted",
                AgentEvent::ProviderRequestStarted {
                    request_id: RequestId::from("request-1"),
                    provider_id: agent_domain::ProviderId::from("mock"),
                    model: "mock".into(),
                },
            ),
            env(
                4,
                "run-hosted",
                AgentEvent::MessageCommitted {
                    message: assistant_tool_message("assistant-1"),
                },
            ),
            env(
                5,
                "run-hosted",
                AgentEvent::ProviderTranscriptContinued {
                    calls: vec![agent_events::ProviderTranscriptContinuation {
                        tool_call_id: agent_domain::ToolCallId::from("call-1"),
                        name: "web_search".into(),
                        kind: agent_domain::ToolKind::ProviderHosted,
                    }],
                },
            ),
            // 续接轮之后的下一个 Provider 轮次：仅当状态机确实落在
            // WaitingForProvider 时才合法（CollectingToolCalls 会报 Illegal）。
            env(
                6,
                "run-hosted",
                AgentEvent::ProviderRequestStarted {
                    request_id: RequestId::from("request-2"),
                    provider_id: agent_domain::ProviderId::from("mock"),
                    model: "mock".into(),
                },
            ),
        ];
        let plan = replay_run(&events);
        // 事件流结束时仍为活跃态（WaitingForProvider → StreamingResponse），
        // 按恢复语义归一为 Interrupted；无非法转换说明续接转换被合法重放。
        assert_eq!(plan.recovered_state, RunState::Interrupted);
        assert!(plan.resumable);
        assert!(
            plan.issues.is_empty(),
            "unexpected issues: {:?}",
            plan.issues
        );
    }

    #[test]
    fn cancelled_run_is_not_flagged_for_recovery() {
        let events = vec![
            env(
                1,
                "run-3",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("m-1"),
                },
            ),
            env(2, "run-3", AgentEvent::RunCancelled { reason: None }),
        ];
        let plans = scan_interrupted(&events);
        assert!(plans.is_empty(), "已取消的 Run 不需要恢复");
    }

    #[test]
    fn duplicate_message_event_is_reported() {
        let events = vec![
            env(
                1,
                "run-4",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("m-1"),
                },
            ),
            env(
                2,
                "run-4",
                AgentEvent::MessageCommitted {
                    message: msg("m-1", "a"),
                },
            ),
            env(
                3,
                "run-4",
                AgentEvent::MessageCommitted {
                    message: msg("m-1", "a"),
                },
            ),
        ];
        let plan = replay_run(&events);
        assert!(plan
            .issues
            .iter()
            .any(|i| matches!(i, RecoveryIssue::DuplicateMessage { .. })));
    }

    #[test]
    fn multiple_runs_are_grouped_and_scanned() {
        let events = vec![
            env(
                1,
                "run-a",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("m-1"),
                },
            ),
            env(2, "run-a", AgentEvent::RunCancelled { reason: None }),
            env(
                3,
                "run-b",
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("m-2"),
                },
            ),
        ];
        let plans = scan_interrupted(&events);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].run_id, RunId::from("run-b"));
    }

    #[test]
    fn replay_is_fast_under_one_second() {
        let mut events = vec![env(
            1,
            "run-x",
            AgentEvent::RunStarted {
                trigger_message_id: MessageId::from("m-1"),
            },
        )];
        for i in 2..10_002 {
            events.push(env(
                i,
                "run-x",
                AgentEvent::AssistantTextDelta {
                    message_id: MessageId::from("m-1"),
                    delta: "x".into(),
                },
            ));
        }
        events.push(env(
            10_002,
            "run-x",
            AgentEvent::RunCompleted {
                stop_reason: StopReason::Completed,
                usage: TokenUsage::default(),
            },
        ));

        let start = Instant::now();
        let plan = replay_run(&events);
        let elapsed = start.elapsed();
        assert!(elapsed.as_millis() < 1000, "重放耗时 {elapsed:?} 应 < 1s");
        assert_eq!(plan.recovered_state, RunState::Completed);
    }

    #[test]
    fn missing_start_event_reported() {
        let events = vec![env(1, "run-y", AgentEvent::RunCancelled { reason: None })];
        let plan = replay_run(&events);
        // 缺少 RunStarted 起始事件 → 报 MissingStartEvent（Cancel 从 Created 合法）。
        assert!(plan
            .issues
            .iter()
            .any(|i| matches!(i, RecoveryIssue::MissingStartEvent)));
    }
}
