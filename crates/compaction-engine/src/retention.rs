//! 压缩保留策略（P5-6）。
//!
//! [`apply`] 在纯数据上决定压缩后保留哪些事件 id，依据 [`RetentionPolicy`]：
//! 最近 N 轮对话、未解决任务、用户约束、修改文件，以及待处理 / 失败的 tool call。
//! 本模块不执行 IO，也不依赖 Event Store；调用方（`CompactionEngine` 或上下文重建）
//! 只需装配 [`RetentionInputs`] 并读取 [`RetentionDecision`]。

use std::collections::BTreeSet;

use agent_domain::{EventId, Message, MessageRole};
use serde::{Deserialize, Serialize};

/// 默认保留的对话轮数（用户发起的一次 turn）。
pub const DEFAULT_RETAINED_TURNS: u32 = 6;

/// 关联到具体事件的会话消息，供保留策略按 turn 处理。
#[derive(Clone, Debug)]
pub struct RetentionMessage {
    /// 产出该消息的事件 id（通常是 `MessageCommitted`）。
    pub event_id: EventId,
    pub message: Message,
}

/// 保留策略关心的 tool call 生命周期状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallRetentionState {
    /// 已发起但尚未完成（含等待参数 / 审批 / 执行中）。
    Pending,
    /// 正常完成。
    Completed,
    /// 完成但结果为错误。
    Failed,
}

/// 关联到具体事件的 tool call。
#[derive(Clone, Debug)]
pub struct RetentionToolCall {
    pub event_id: EventId,
    pub state: ToolCallRetentionState,
}

/// 一个跟踪中的任务。
#[derive(Clone, Debug)]
pub struct RetentionTask {
    pub event_id: EventId,
    /// 任务是否已解决；未解决的任务默认保留。
    pub resolved: bool,
}

/// 一条用户约束（如「不要修改 X」「必须遵守 Y」）。
#[derive(Clone, Debug)]
pub struct RetentionConstraint {
    pub event_id: EventId,
}

/// 一次被引用的文件修改记录。
#[derive(Clone, Debug)]
pub struct ModifiedFile {
    pub event_id: EventId,
    pub path: String,
}

/// 保留策略的全部输入；字段对齐压缩时已知的最小投影。
#[derive(Clone, Debug, Default)]
pub struct RetentionInputs {
    pub messages: Vec<RetentionMessage>,
    pub tool_calls: Vec<RetentionToolCall>,
    pub tasks: Vec<RetentionTask>,
    pub constraints: Vec<RetentionConstraint>,
    pub modified_files: Vec<ModifiedFile>,
}

/// 压缩保留策略。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// 保留最近 N 轮对话（一轮 = 一个用户消息起的所有后续消息）。
    pub retained_turns: u32,
    pub keep_unresolved_tasks: bool,
    pub keep_user_constraints: bool,
    pub keep_modified_files: bool,
    pub keep_pending_tool_calls: bool,
    pub keep_failed_tool_calls: bool,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            retained_turns: DEFAULT_RETAINED_TURNS,
            keep_unresolved_tasks: true,
            keep_user_constraints: true,
            keep_modified_files: true,
            keep_pending_tool_calls: true,
            keep_failed_tool_calls: true,
        }
    }
}

/// [`apply`] 的决策结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionDecision {
    /// 压缩后逐字保留的事件 id（已去重、按 `EventId` 字典序排序）。
    pub retained_event_ids: Vec<EventId>,
    /// 将被折叠进摘要的事件数（候选总数 - 保留数）。
    pub dropped_count: usize,
    /// 人可读的保留理由列表。
    pub reasons: Vec<String>,
}

/// 依据 `policy` 在 `inputs` 上计算保留决策。
///
/// 候选事件集合 = 输入中所有 `event_id` 的并集；保留集合是其子集，
/// `dropped_count` = 候选数 - 保留数。保留的事件 id 去重后按 `EventId` 字典序输出，
/// 保证决策对相同输入确定且稳定。
pub fn apply(policy: &RetentionPolicy, inputs: &RetentionInputs) -> RetentionDecision {
    let mut candidates: BTreeSet<EventId> = BTreeSet::new();
    let mut retained: BTreeSet<EventId> = BTreeSet::new();
    let mut reasons: Vec<String> = Vec::new();

    for message in &inputs.messages {
        candidates.insert(message.event_id.clone());
    }
    for tool_call in &inputs.tool_calls {
        candidates.insert(tool_call.event_id.clone());
    }
    for task in &inputs.tasks {
        candidates.insert(task.event_id.clone());
    }
    for constraint in &inputs.constraints {
        candidates.insert(constraint.event_id.clone());
    }
    for file in &inputs.modified_files {
        candidates.insert(file.event_id.clone());
    }

    // System prompt 永远保留（不属于对话轮）。
    let mut system_kept = 0usize;
    for message in &inputs.messages {
        if message.message.role == MessageRole::System {
            retained.insert(message.event_id.clone());
            system_kept += 1;
        }
    }
    if system_kept > 0 {
        reasons.push(format!("retained {system_kept} system message(s)"));
    }

    // 最近 N 轮：保留从倒数第 N 个用户消息起的所有消息。
    let user_starts: Vec<usize> = inputs
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.message.role == MessageRole::User)
        .map(|(index, _)| index)
        .collect();
    let total_turns = user_starts.len();
    let policy_turns = usize::try_from(policy.retained_turns).unwrap_or(usize::MAX);
    if policy_turns > 0 && total_turns > 0 {
        let retained_turns = policy_turns.min(total_turns);
        let cutoff = total_turns - retained_turns;
        let start = user_starts[cutoff];
        let mut turn_kept = 0usize;
        for message in &inputs.messages[start..] {
            retained.insert(message.event_id.clone());
            turn_kept += 1;
        }
        reasons.push(format!(
            "retained last {retained_turns} turn(s) ({turn_kept} message(s))"
        ));
    }

    if policy.keep_unresolved_tasks {
        let mut kept = 0usize;
        for task in &inputs.tasks {
            if !task.resolved {
                retained.insert(task.event_id.clone());
                kept += 1;
            }
        }
        if kept > 0 {
            reasons.push(format!("retained {kept} unresolved task(s)"));
        }
    }

    if policy.keep_user_constraints {
        let mut kept = 0usize;
        for constraint in &inputs.constraints {
            retained.insert(constraint.event_id.clone());
            kept += 1;
        }
        if kept > 0 {
            reasons.push(format!("retained {kept} user constraint(s)"));
        }
    }

    if policy.keep_modified_files {
        let mut kept = 0usize;
        for file in &inputs.modified_files {
            retained.insert(file.event_id.clone());
            kept += 1;
        }
        if kept > 0 {
            reasons.push(format!("retained {kept} modified file reference(s)"));
        }
    }

    if policy.keep_pending_tool_calls {
        let mut kept = 0usize;
        for tool_call in &inputs.tool_calls {
            if tool_call.state == ToolCallRetentionState::Pending {
                retained.insert(tool_call.event_id.clone());
                kept += 1;
            }
        }
        if kept > 0 {
            reasons.push(format!("retained {kept} pending tool call(s)"));
        }
    }

    if policy.keep_failed_tool_calls {
        let mut kept = 0usize;
        for tool_call in &inputs.tool_calls {
            if tool_call.state == ToolCallRetentionState::Failed {
                retained.insert(tool_call.event_id.clone());
                kept += 1;
            }
        }
        if kept > 0 {
            reasons.push(format!("retained {kept} failed tool call(s)"));
        }
    }

    let candidate_count = candidates.len();
    let retained_event_ids: Vec<EventId> = retained.into_iter().collect();
    let dropped_count = candidate_count.saturating_sub(retained_event_ids.len());
    RetentionDecision {
        retained_event_ids,
        dropped_count,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use agent_domain::{Message, MessageId, MessageMetadata, MessageRole, TokenUsage};

    use super::*;

    fn message(id: &str, role: MessageRole) -> Message {
        Message {
            id: MessageId::from(id),
            role,
            content: Vec::new(),
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

    fn entry(event: &str, role: MessageRole, id: &str) -> RetentionMessage {
        RetentionMessage {
            event_id: EventId::from(event),
            message: message(id, role),
        }
    }

    fn tool_call(event: &str, state: ToolCallRetentionState) -> RetentionToolCall {
        RetentionToolCall {
            event_id: EventId::from(event),
            state,
        }
    }

    /// Golden Session：覆盖 system / 多轮 / 任务 / 约束 / 修改文件 / tool call 各类输入。
    fn golden_inputs() -> RetentionInputs {
        RetentionInputs {
            messages: vec![
                entry("event-sys", MessageRole::System, "sys"),
                entry("event-u1", MessageRole::User, "u1"),
                entry("event-a1", MessageRole::Assistant, "a1"),
                entry("event-u2", MessageRole::User, "u2"),
                entry("event-a2", MessageRole::Assistant, "a2"),
                entry("event-u3", MessageRole::User, "u3"),
                entry("event-a3", MessageRole::Assistant, "a3"),
            ],
            tool_calls: vec![
                tool_call("event-tool-pending", ToolCallRetentionState::Pending),
                tool_call("event-tool-done", ToolCallRetentionState::Completed),
                tool_call("event-tool-failed", ToolCallRetentionState::Failed),
            ],
            tasks: vec![
                RetentionTask {
                    event_id: EventId::from("event-task-open"),
                    resolved: false,
                },
                RetentionTask {
                    event_id: EventId::from("event-task-done"),
                    resolved: true,
                },
            ],
            constraints: vec![RetentionConstraint {
                event_id: EventId::from("event-constraint"),
            }],
            modified_files: vec![ModifiedFile {
                event_id: EventId::from("event-file"),
                path: "src/lib.rs".into(),
            }],
        }
    }

    fn retained_set(decision: &RetentionDecision) -> HashSet<String> {
        decision
            .retained_event_ids
            .iter()
            .map(|id| id.to_string())
            .collect()
    }

    #[test]
    fn golden_session_keeps_critical_state() {
        // 只保留最近 1 轮，但未解决任务 / 用户约束 / 修改文件 / 待处理与失败 tool call 必须存活。
        let policy = RetentionPolicy {
            retained_turns: 1,
            ..RetentionPolicy::default()
        };
        let decision = apply(&policy, &golden_inputs());

        let retained = retained_set(&decision);
        for expected in [
            "event-sys",
            "event-u3",
            "event-a3",
            "event-tool-pending",
            "event-tool-failed",
            "event-task-open",
            "event-constraint",
            "event-file",
        ] {
            assert!(retained.contains(expected), "expected {expected} retained");
        }
        for dropped in [
            "event-u1",
            "event-a1",
            "event-u2",
            "event-a2",
            "event-tool-done",
            "event-task-done",
        ] {
            assert!(!retained.contains(dropped), "expected {dropped} dropped");
        }

        // 候选 = 14，保留 = 8，丢弃 = 6。
        assert_eq!(decision.dropped_count, 6);

        // 输出已去重且按 EventId 字典序稳定排序。
        let mut sorted = decision.retained_event_ids.clone();
        sorted.sort();
        assert_eq!(decision.retained_event_ids, sorted);

        // 每个保留类别都产生了理由。
        let reasons = decision.reasons.join("\n");
        for needle in [
            "system",
            "turn",
            "unresolved task",
            "user constraint",
            "modified file",
            "pending tool call",
            "failed tool call",
        ] {
            assert!(
                reasons.contains(needle),
                "missing reason for {needle}: {reasons}"
            );
        }
    }

    #[test]
    fn disabling_keep_flags_drops_pending_and_failed_tool_calls() {
        let policy = RetentionPolicy {
            retained_turns: 1,
            keep_pending_tool_calls: false,
            keep_failed_tool_calls: false,
            ..RetentionPolicy::default()
        };
        let decision = apply(&policy, &golden_inputs());
        let retained = retained_set(&decision);

        assert!(!retained.contains("event-tool-pending"));
        assert!(!retained.contains("event-tool-failed"));
        assert!(retained.contains("event-constraint"));
        // 候选 14，保留 6，丢弃 8。
        assert_eq!(decision.dropped_count, 8);
    }

    #[test]
    fn large_retained_turns_keeps_all_turns_without_panic() {
        let policy = RetentionPolicy {
            retained_turns: u32::MAX,
            ..RetentionPolicy::default()
        };
        let decision = apply(&policy, &golden_inputs());
        let retained = retained_set(&decision);
        for turn in [
            "event-u1", "event-a1", "event-u2", "event-a2", "event-u3", "event-a3",
        ] {
            assert!(retained.contains(turn));
        }
    }

    #[test]
    fn zero_retained_turns_drops_all_conversation_history() {
        let policy = RetentionPolicy {
            retained_turns: 0,
            ..RetentionPolicy::default()
        };
        let decision = apply(&policy, &golden_inputs());
        let retained = retained_set(&decision);
        assert!(retained.contains("event-sys"));
        assert!(!retained.contains("event-u3"));
        assert!(!retained.contains("event-a3"));
    }

    #[test]
    fn empty_inputs_produce_empty_decision() {
        let decision = apply(&RetentionPolicy::default(), &RetentionInputs::default());
        assert!(decision.retained_event_ids.is_empty());
        assert_eq!(decision.dropped_count, 0);
        assert!(decision.reasons.is_empty());
    }
}
