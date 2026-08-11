//! [`CompactionEngine`]：自动 / 手动压缩的统一入口（P5-5）。
//!
//! 引擎职责：
//! 1. 读取当前事件流，确定压缩区间（`replaced_range`）与 head 事件；
//! 2. 压缩前用 [`SessionStore::create_branch`] Fork 出可恢复的 recovery branch
//!    （`forked_from_event_id` = head 事件 id）；
//! 3. 在 [`RetentionInputs`] 上应用 [`RetentionPolicy`]，产出保留决策；
//! 4. 组装版本化 [`CompactionSnapshot`]（含压缩前后 token 估算）。
//!
//! 引擎只产出快照与决策；真正向事件流追加 `CompactionStarted` /
//! `CompactionCompleted` 事件、以及用摘要消息重建上下文，由调用方
//! （`agent-engine` / `context-engine`）完成。

use std::collections::HashSet;
use std::fmt;

use agent_domain::{EventId, Message, SessionId};
use context_engine::{HeuristicEstimator, TokenEstimator};
use session_store::SessionStore;

use crate::retention::{apply, RetentionDecision, RetentionInputs, RetentionPolicy};
use crate::snapshot::{CompactionSnapshot, SnapshotVersion};
use crate::CompactionError;

/// 压缩触发原因（手动 / 自动）。自动原因对齐 `context-engine` 的触发信号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionReason {
    /// 用户显式手动压缩。
    Manual,
    /// 历史消息超过软阈值。
    HistorySoftLimit,
    /// 输入超过 `max_input_tokens` 硬上限。
    InputBudgetExceeded,
}

/// context-engine 只产生自动触发原因；`Manual` 仅属于本引擎的显式调用入口。
impl From<context_engine::CompactionReason> for CompactionReason {
    fn from(reason: context_engine::CompactionReason) -> Self {
        match reason {
            context_engine::CompactionReason::HistorySoftLimit => Self::HistorySoftLimit,
            context_engine::CompactionReason::InputBudgetExceeded => Self::InputBudgetExceeded,
        }
    }
}

impl fmt::Display for CompactionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Manual => "manual",
            Self::HistorySoftLimit => "history_soft_limit",
            Self::InputBudgetExceeded => "input_budget_exceeded",
        };
        formatter.write_str(value)
    }
}

/// 一次压缩的结果。
#[derive(Clone, Debug)]
pub struct CompactionResult {
    pub reason: CompactionReason,
    pub snapshot: CompactionSnapshot,
    pub decision: RetentionDecision,
    /// 压缩区间内的事件总数（`replaced_range` 覆盖的事件条数）。
    pub total_events: usize,
}

/// 自动 / 手动压缩引擎。持有 [`SessionStore`] 引用与保留策略。
pub struct CompactionEngine<'a> {
    store: &'a SessionStore,
    policy: RetentionPolicy,
}

impl<'a> CompactionEngine<'a> {
    /// 以默认保留策略构造引擎。
    pub fn new(store: &'a SessionStore) -> Self {
        Self {
            store,
            policy: RetentionPolicy::default(),
        }
    }

    /// 以自定义保留策略构造引擎。
    pub fn with_policy(store: &'a SessionStore, policy: RetentionPolicy) -> Self {
        Self { store, policy }
    }

    /// 当前生效的保留策略。
    pub fn policy(&self) -> &RetentionPolicy {
        &self.policy
    }

    /// 执行压缩（手动 / 自动统一入口）。
    ///
    /// `branch_id` 是被压缩的分支（同时作为 recovery branch 的 parent）。
    /// `summary_text` 是替代被压缩区间的摘要；`inputs` 是保留策略输入。
    pub async fn compact(
        &self,
        session_id: &SessionId,
        branch_id: &str,
        reason: CompactionReason,
        summary_text: &str,
        inputs: &RetentionInputs,
    ) -> Result<CompactionResult, CompactionError> {
        let events = self
            .store
            .events_by_branch(session_id, branch_id, 1, usize::MAX)
            .await?;
        let head = events
            .last()
            .ok_or_else(|| CompactionError::NothingToCompact {
                session_id: session_id.to_string(),
                branch_id: branch_id.to_string(),
            })?;
        let head_event_id = head.event_id.clone();
        let first_sequence = events
            .first()
            .expect("a non-empty event stream has a first event")
            .sequence;
        let last_sequence = head.sequence;
        let total_events = events.len();

        // 1. 压缩前 Fork recovery branch（可恢复到压缩前的 head）。
        let recovery_branch_id =
            format!("compaction-recovery-{branch_id}-{}", last_sequence.value());
        self.store
            .create_branch(
                session_id,
                recovery_branch_id.clone(),
                Some(branch_id.to_string()),
                Some(head_event_id.to_string()),
            )
            .await?;

        // 2. 只让目标 branch 的事件进入保留策略，防止调用方投影混入兄弟分支。
        let branch_event_ids: HashSet<EventId> =
            events.iter().map(|event| event.event_id.clone()).collect();
        let branch_inputs = filter_retention_inputs(inputs, &branch_event_ids);
        let decision = apply(&self.policy, &branch_inputs);

        // 3. 估算压缩前后 token：before = 全部消息；after = 保留消息 + 摘要。
        let retained_ids: HashSet<&EventId> = decision.retained_event_ids.iter().collect();
        let estimator = HeuristicEstimator::default();
        let token_usage_before = branch_inputs
            .messages
            .iter()
            .map(|entry| estimate_message_tokens(&entry.message, &estimator))
            .sum::<u64>();
        let retained_message_tokens = branch_inputs
            .messages
            .iter()
            .filter(|entry| retained_ids.contains(&entry.event_id))
            .map(|entry| estimate_message_tokens(&entry.message, &estimator))
            .sum::<u64>();
        let token_usage_after =
            retained_message_tokens.saturating_add(estimator.count_text(summary_text));

        let snapshot = CompactionSnapshot {
            version: SnapshotVersion::current(),
            summary: summary_text.to_string(),
            retained_event_ids: decision.retained_event_ids.clone(),
            replaced_range: (first_sequence, last_sequence),
            token_usage_before,
            token_usage_after,
            recovery_branch_id: Some(recovery_branch_id),
        };
        snapshot.validate()?;

        Ok(CompactionResult {
            reason,
            snapshot,
            decision,
            total_events,
        })
    }
}

fn filter_retention_inputs(
    inputs: &RetentionInputs,
    branch_event_ids: &HashSet<EventId>,
) -> RetentionInputs {
    RetentionInputs {
        messages: inputs
            .messages
            .iter()
            .filter(|entry| branch_event_ids.contains(&entry.event_id))
            .cloned()
            .collect(),
        tool_calls: inputs
            .tool_calls
            .iter()
            .filter(|entry| branch_event_ids.contains(&entry.event_id))
            .cloned()
            .collect(),
        tasks: inputs
            .tasks
            .iter()
            .filter(|entry| branch_event_ids.contains(&entry.event_id))
            .cloned()
            .collect(),
        constraints: inputs
            .constraints
            .iter()
            .filter(|entry| branch_event_ids.contains(&entry.event_id))
            .cloned()
            .collect(),
        modified_files: inputs
            .modified_files
            .iter()
            .filter(|entry| branch_event_ids.contains(&entry.event_id))
            .cloned()
            .collect(),
        reasoning_items: inputs
            .reasoning_items
            .iter()
            .filter(|entry| branch_event_ids.contains(&entry.event_id))
            .cloned()
            .collect(),
    }
}

/// 单条消息的 token 估算：优先用 `metadata.usage`，否则按内容启发式估算。
fn estimate_message_tokens(message: &Message, estimator: &dyn TokenEstimator) -> u64 {
    if let Some(usage) = message.metadata.usage.as_ref() {
        return usage.total_tokens();
    }
    estimator.count_message(message)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use agent_domain::{
        ContentPart, EventId, Message, MessageId, MessageMetadata, MessageRole, RunId, SessionId,
        TextContent, Timestamp, TokenUsage, ToolCallId,
    };
    use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};
    use session_store::{SessionStore, DEFAULT_BRANCH_ID};

    use super::*;
    use crate::retention::{
        ModifiedFile, RetentionConstraint, RetentionInputs, RetentionMessage, RetentionTask,
        RetentionToolCall, ToolCallRetentionState,
    };

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path() -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-compaction-engine-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn event(session: &SessionId, sequence: u64, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{sequence}")),
            session.clone(),
            RunId::from("run-1"),
            EventSequence::new(sequence),
            Timestamp::from_unix_millis(1_000 + sequence),
            payload,
        )
    }

    fn committed(id: &str, role: MessageRole) -> Message {
        Message {
            id: MessageId::from(id),
            role,
            content: vec![ContentPart::Text(TextContent {
                text: "hello world from the test harness".into(),
            })],
            metadata: MessageMetadata {
                usage: Some(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    ..TokenUsage::default()
                }),
                ..MessageMetadata::default()
            },
        }
    }

    async fn read_forked_from(
        store: &SessionStore,
        session: &SessionId,
        branch_id: &str,
    ) -> Option<String> {
        let session = session.to_string();
        let branch_id = branch_id.to_string();
        store
            .database()
            .call(move |connection| {
                connection
                    .query_row(
                        "SELECT forked_from_event_id FROM session_branches \
                         WHERE session_id=?1 AND branch_id=?2",
                        [&session, &branch_id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .ok()
                    .flatten()
            })
            .await
            .expect("database actor responds")
    }

    #[tokio::test]
    async fn compact_forks_recovery_branch_and_builds_snapshot() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let session = SessionId::from("session-golden");
        store
            .create_session(&session, "golden", Timestamp::from_unix_millis(1))
            .await
            .expect("create session");

        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::RunStarted {
                        trigger_message_id: MessageId::from("trigger"),
                    },
                ),
            )
            .await
            .expect("run started");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    2,
                    AgentEvent::MessageCommitted {
                        message: committed("user-1", MessageRole::User),
                    },
                ),
            )
            .await
            .expect("user1");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    3,
                    AgentEvent::MessageCommitted {
                        message: committed("assistant-1", MessageRole::Assistant),
                    },
                ),
            )
            .await
            .expect("assistant1");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    4,
                    AgentEvent::ToolCallStarted {
                        tool_call_id: ToolCallId::from("tool-1"),
                        name: "edit_file".into(),
                    },
                ),
            )
            .await
            .expect("tool started");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    5,
                    AgentEvent::MessageCommitted {
                        message: committed("user-2", MessageRole::User),
                    },
                ),
            )
            .await
            .expect("user2");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    6,
                    AgentEvent::MessageCommitted {
                        message: committed("assistant-2", MessageRole::Assistant),
                    },
                ),
            )
            .await
            .expect("assistant2");

        // 只保留最近 1 轮；event-2 通过未解决任务被额外保留，event-3（assistant1）会被丢弃。
        let policy = RetentionPolicy {
            retained_turns: 1,
            ..RetentionPolicy::default()
        };
        let engine = CompactionEngine::with_policy(&store, policy);
        let inputs = RetentionInputs {
            messages: vec![
                RetentionMessage {
                    event_id: EventId::from("event-2"),
                    message: committed("user-1", MessageRole::User),
                },
                RetentionMessage {
                    event_id: EventId::from("event-3"),
                    message: committed("assistant-1", MessageRole::Assistant),
                },
                RetentionMessage {
                    event_id: EventId::from("event-5"),
                    message: committed("user-2", MessageRole::User),
                },
                RetentionMessage {
                    event_id: EventId::from("event-6"),
                    message: committed("assistant-2", MessageRole::Assistant),
                },
            ],
            tool_calls: vec![RetentionToolCall {
                event_id: EventId::from("event-4"),
                state: ToolCallRetentionState::Pending,
            }],
            tasks: vec![RetentionTask {
                event_id: EventId::from("event-2"),
                resolved: false,
            }],
            constraints: vec![RetentionConstraint {
                event_id: EventId::from("event-5"),
            }],
            modified_files: vec![ModifiedFile {
                event_id: EventId::from("event-6"),
                path: "src/lib.rs".into(),
            }],
            reasoning_items: Vec::new(),
        };

        let result = engine
            .compact(
                &session,
                DEFAULT_BRANCH_ID,
                CompactionReason::InputBudgetExceeded,
                "前期讨论已折叠为压缩摘要。",
                &inputs,
            )
            .await
            .expect("compact");

        assert_eq!(result.reason, CompactionReason::InputBudgetExceeded);
        assert_eq!(result.snapshot.version, SnapshotVersion::current());
        assert_eq!(
            result.snapshot.replaced_range,
            (EventSequence::new(1), EventSequence::new(6))
        );
        assert_eq!(result.total_events, 6);
        assert_eq!(
            result.snapshot.recovery_branch_id.as_deref(),
            Some("compaction-recovery-main-6")
        );
        assert!(result.snapshot.token_usage_before >= result.snapshot.token_usage_after);

        let retained: HashSet<String> = result
            .decision
            .retained_event_ids
            .iter()
            .map(|id| id.to_string())
            .collect();
        // event-2 由未解决任务保留；event-4 待处理 tool call；event-5 / event-6 最近一轮。
        for expected in ["event-2", "event-4", "event-5", "event-6"] {
            assert!(retained.contains(expected), "expected {expected} retained");
        }
        assert!(!retained.contains("event-3"));
        assert_eq!(result.decision.dropped_count, 1);

        // recovery branch 真正落库，且 fork 在 head（event-6）。
        let forked = read_forked_from(&store, &session, "compaction-recovery-main-6").await;
        assert_eq!(forked.as_deref(), Some("event-6"));

        // 摘要可 serde 往返。
        let json = serde_json::to_string(&result.snapshot).expect("serialize snapshot");
        let back: CompactionSnapshot = serde_json::from_str(&json).expect("deserialize snapshot");
        assert_eq!(back, result.snapshot);

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn compact_without_events_is_rejected() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let session = SessionId::from("session-empty");
        store
            .create_session(&session, "empty", Timestamp::from_unix_millis(1))
            .await
            .expect("create session");

        let engine = CompactionEngine::new(&store);
        let result = engine
            .compact(
                &session,
                DEFAULT_BRANCH_ID,
                CompactionReason::Manual,
                "noop",
                &RetentionInputs::default(),
            )
            .await;

        assert!(matches!(
            result,
            Err(CompactionError::NothingToCompact { .. })
        ));

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn compact_uses_only_the_requested_branch_event_range() {
        let path = temp_path();
        let (store, _) = SessionStore::open(&path).await.expect("open store");
        let session = SessionId::from("session-branch-compaction");
        store
            .create_session(&session, "branches", Timestamp::from_unix_millis(1))
            .await
            .expect("create session");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    1,
                    AgentEvent::CompactionStarted {
                        source_event_count: 1,
                    },
                ),
            )
            .await
            .expect("main 1");
        store
            .create_branch(
                &session,
                "experiment",
                Some(DEFAULT_BRANCH_ID.into()),
                Some("event-1".into()),
            )
            .await
            .expect("fork experiment");
        store
            .switch_branch(&session, "experiment")
            .await
            .expect("switch experiment");
        store
            .append_event(
                "experiment",
                event(
                    &session,
                    2,
                    AgentEvent::CompactionStarted {
                        source_event_count: 2,
                    },
                ),
            )
            .await
            .expect("experiment 2");
        store
            .switch_branch(&session, DEFAULT_BRANCH_ID)
            .await
            .expect("switch main");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                event(
                    &session,
                    3,
                    AgentEvent::CompactionStarted {
                        source_event_count: 3,
                    },
                ),
            )
            .await
            .expect("main 3");

        let mixed_branch_inputs = RetentionInputs {
            messages: vec![
                RetentionMessage {
                    event_id: EventId::from("event-2"),
                    message: committed("experiment-message", MessageRole::User),
                },
                RetentionMessage {
                    event_id: EventId::from("event-3"),
                    message: committed("main-message", MessageRole::User),
                },
            ],
            ..RetentionInputs::default()
        };
        let result = CompactionEngine::new(&store)
            .compact(
                &session,
                "experiment",
                CompactionReason::Manual,
                "实验分支摘要",
                &mixed_branch_inputs,
            )
            .await
            .expect("compact experiment");

        assert_eq!(result.total_events, 1);
        assert_eq!(
            result.snapshot.replaced_range,
            (EventSequence::new(2), EventSequence::new(2))
        );
        assert_eq!(
            result.snapshot.recovery_branch_id.as_deref(),
            Some("compaction-recovery-experiment-2")
        );
        let forked = read_forked_from(&store, &session, "compaction-recovery-experiment-2").await;
        assert_eq!(forked.as_deref(), Some("event-2"));
        assert_eq!(
            result.decision.retained_event_ids,
            vec![EventId::from("event-2")]
        );

        store.shutdown().await.expect("shutdown");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn automatic_compaction_reasons_map_explicitly() {
        assert_eq!(
            CompactionReason::from(context_engine::CompactionReason::HistorySoftLimit),
            CompactionReason::HistorySoftLimit
        );
        assert_eq!(
            CompactionReason::from(context_engine::CompactionReason::InputBudgetExceeded),
            CompactionReason::InputBudgetExceeded
        );
    }

    #[test]
    fn compaction_message_estimate_uses_cjk_aware_token_estimator() {
        let message = Message {
            id: MessageId::from("cjk"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "你好世界上下文压缩".into(),
            })],
            metadata: MessageMetadata::default(),
        };
        let estimator = HeuristicEstimator::default();
        assert!(estimate_message_tokens(&message, &estimator) >= 9);
    }
}
