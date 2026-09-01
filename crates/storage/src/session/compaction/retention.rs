//! 压缩保留策略（P5-6）。
//!
//! [`apply`] 在纯数据上决定压缩后保留哪些事件 id，依据 [`RetentionPolicy`]：
//! 最近 N 轮对话、最近 N 个 reasoning item、未解决任务、用户约束、修改文件，
//! 以及待处理 / 失败的 tool call。
//! 本模块不执行 IO，也不依赖 Event Store；调用方（`CompactionEngine` 或上下文重建）
//! 只需装配 [`RetentionInputs`] 并读取 [`RetentionDecision`]。

use std::collections::BTreeSet;

use pawork_domain::{EventId, Message, MessageRole, ReasoningItemId};
use serde::{Deserialize, Serialize};

/// 默认保留的对话轮数（用户发起的一次 turn）。
pub const DEFAULT_RETAINED_TURNS: u32 = 6;

/// 默认保留的最近 reasoning 条目数。
///
/// reasoning 条目按调用方提供的事件顺序（升序）排列，保留末尾 N 条；N = 0 关闭
/// reasoning 保留（与 [`RetentionPolicy::retained_turns`] = 0 同语义）。
pub const DEFAULT_RETAINED_REASONING_ITEMS: u32 = 8;

/// 关联到具体事件的会话消息，供保留策略按 turn 处理。
#[derive(Clone, Debug)]
pub struct RetentionMessage {
    /// 产出该消息的事件 id（通常是 `MessageCommitted`）。
    pub event_id: EventId,
    pub message: Message,
}

/// 一条 reasoning 链条目，供保留策略按「最近 N 条 reasoning」处理。
///
/// `event_id` 通常是携带该 reasoning（`ContentPart::Reasoning` 或引用同一
/// `reasoning_item_id` 的 `ContentPart::Thinking`）的 `MessageCommitted` 事件；
/// 保留这条 reasoning 等价于保留该 `MessageCommitted` 事件。调用方按事件顺序
/// （升序）填入 [`RetentionInputs::reasoning_items`]，`apply` 取末尾 N 条。
///
/// 只携带 [`ReasoningItemId`] 而非 `ProtectedBlobRef`：
/// 保留决策纯基于事件 id，不触碰 protected store、blob refcount，也不按 Provider 分支。
#[derive(Clone, Debug)]
pub struct RetentionReasoning {
    pub event_id: EventId,
    pub reasoning_item_id: ReasoningItemId,
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
    /// reasoning 条目，按事件顺序（升序）排列；末尾 N 条会被保留。
    pub reasoning_items: Vec<RetentionReasoning>,
}

/// 压缩保留策略。
///
/// `#[serde(default)]` 让旧 JSON（缺少 `retained_reasoning_items` 等新字段）按
/// [`RetentionPolicy::default`] 补齐；关键计数字段另以字段级函数明确默认常量。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionPolicy {
    /// 保留最近 N 轮对话（一轮 = 一个用户消息起的所有后续消息）。
    #[serde(default = "default_retained_turns")]
    pub retained_turns: u32,
    /// 保留最近 N 条 reasoning 条目（携带它们的 `MessageCommitted` 事件一并保留）。
    /// 0 关闭 reasoning 保留。
    #[serde(default = "default_retained_reasoning_items")]
    pub retained_reasoning_items: u32,
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
            retained_reasoning_items: DEFAULT_RETAINED_REASONING_ITEMS,
            keep_unresolved_tasks: true,
            keep_user_constraints: true,
            keep_modified_files: true,
            keep_pending_tool_calls: true,
            keep_failed_tool_calls: true,
        }
    }
}

/// serde 默认值函数：返回 [`DEFAULT_RETAINED_TURNS`]，用于缺失字段的向后兼容反序列化。
fn default_retained_turns() -> u32 {
    DEFAULT_RETAINED_TURNS
}

/// serde 默认值函数：返回 [`DEFAULT_RETAINED_REASONING_ITEMS`]，用于缺失字段的向后兼容反序列化。
fn default_retained_reasoning_items() -> u32 {
    DEFAULT_RETAINED_REASONING_ITEMS
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
    for reasoning in &inputs.reasoning_items {
        candidates.insert(reasoning.event_id.clone());
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

    // 最近 N 条 reasoning：保留末尾 N 条对应的 `MessageCommitted` 事件。
    // 输入按事件顺序（升序）排列，因此末尾即「最近」。多个 reasoning 条目可能
    // 共享同一 event_id（一条 MessageCommitted 携带多个 reasoning part），
    // 通过 BTreeSet 自然去重。决策只产出 EventId，不触碰 blob refcount。
    let policy_reasoning = usize::try_from(policy.retained_reasoning_items).unwrap_or(usize::MAX);
    if policy_reasoning > 0 && !inputs.reasoning_items.is_empty() {
        let split = inputs
            .reasoning_items
            .len()
            .saturating_sub(policy_reasoning);
        let kept_slice = &inputs.reasoning_items[split..];
        // distinct_events 度量这些 reasoning 跨多少条 MessageCommitted，独立于其他规则
        // 是否已经保留同一事件——保证理由描述稳定可读。
        let mut distinct_events: BTreeSet<EventId> = BTreeSet::new();
        let mut retained_items = 0usize;
        for reasoning in kept_slice {
            distinct_events.insert(reasoning.event_id.clone());
            retained.insert(reasoning.event_id.clone());
            retained_items += 1;
        }
        let scope = if split == 0 { "all" } else { "last" };
        reasons.push(format!(
            "retained {scope} {retained_items} reasoning item(s) across {} message(s)",
            distinct_events.len()
        ));
    }
    // 注：保留集合只新增 EventId。即便 reasoning 的 MessageCommitted 已被其他规则保留，
    // 这里也只是幂等 insert，绝不触碰 ProtectedBlobRef / protected store / blob refcount。

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

    use pawork_domain::{
        Message, MessageId, MessageMetadata, MessageRole, ReasoningItemId, TokenUsage,
    };

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

    fn reasoning(event: &str, item: &str) -> RetentionReasoning {
        RetentionReasoning {
            event_id: EventId::from(event),
            reasoning_item_id: ReasoningItemId::from(item),
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
            reasoning_items: Vec::new(),
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

    #[test]
    fn default_policy_enables_reasoning_retention() {
        let policy = RetentionPolicy::default();
        assert_eq!(
            policy.retained_reasoning_items,
            DEFAULT_RETAINED_REASONING_ITEMS
        );
    }

    #[test]
    fn legacy_policy_json_without_reasoning_field_defaults_to_default() {
        // 本次新增前的旧 JSON：含全部既有字段，唯独缺 retained_reasoning_items。
        let legacy = serde_json::json!({
            "retained_turns": 4,
            "keep_unresolved_tasks": true,
            "keep_user_constraints": true,
            "keep_modified_files": true,
            "keep_pending_tool_calls": true,
            "keep_failed_tool_calls": true,
        });
        let policy: RetentionPolicy =
            serde_json::from_value(legacy).expect("legacy policy deserializes");
        assert_eq!(
            policy.retained_reasoning_items, DEFAULT_RETAINED_REASONING_ITEMS,
            "missing retained_reasoning_items must default to DEFAULT_RETAINED_REASONING_ITEMS"
        );
        // 既有字段保持 JSON 提供的值。
        assert_eq!(policy.retained_turns, 4);
        assert!(policy.keep_unresolved_tasks);
    }

    #[test]
    fn empty_policy_json_uses_full_defaults() {
        // 空 JSON 应整体回退到 Default impl。
        let policy: RetentionPolicy =
            serde_json::from_value(serde_json::json!({})).expect("empty policy deserializes");
        assert_eq!(policy, RetentionPolicy::default());
        assert_eq!(policy.retained_turns, DEFAULT_RETAINED_TURNS);
        assert_eq!(
            policy.retained_reasoning_items,
            DEFAULT_RETAINED_REASONING_ITEMS
        );
        assert!(policy.keep_unresolved_tasks);
    }

    #[test]
    fn policy_round_trip_preserves_reasoning_field() {
        let policy = RetentionPolicy {
            retained_reasoning_items: 3,
            ..RetentionPolicy::default()
        };
        let json = serde_json::to_string(&policy).expect("serialize");
        let back: RetentionPolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, policy);
        assert_eq!(back.retained_reasoning_items, 3);
    }

    fn reasoning_inputs() -> RetentionInputs {
        // 6 条 reasoning，分布在不同事件上；按事件顺序（升序）排列。
        // event-r1..event-r3 属于「旧」一轮（event-u1/a1），event-r4..event-r6 属于「最近」一轮。
        // 注意 event-r2 与 event-r3 共享同一 MessageCommitted（event-a1），用来验证去重。
        RetentionInputs {
            messages: vec![
                entry("event-u1", MessageRole::User, "u1"),
                entry("event-a1", MessageRole::Assistant, "a1"),
                entry("event-u2", MessageRole::User, "u2"),
                entry("event-a2", MessageRole::Assistant, "a2"),
            ],
            reasoning_items: vec![
                reasoning("event-r1", "rsn-1"),
                reasoning("event-a1", "rsn-2"),
                reasoning("event-a1", "rsn-3"),
                reasoning("event-r4", "rsn-4"),
                reasoning("event-a2", "rsn-5"),
                reasoning("event-a2", "rsn-6"),
            ],
            ..RetentionInputs::default()
        }
    }

    #[test]
    fn default_policy_keeps_last_n_reasoning_events() {
        // 默认 N = DEFAULT_RETAINED_REASONING_ITEMS (8) >= 6 → 保留全部 reasoning。
        let decision = apply(&RetentionPolicy::default(), &reasoning_inputs());
        let retained = retained_set(&decision);
        for expected in ["event-r1", "event-a1", "event-r4", "event-a2"] {
            assert!(retained.contains(expected), "expected {expected} retained");
        }
        // 4 个候选事件（event-r2/r3 合并进 event-a1）全保留 → dropped = 0。
        assert_eq!(decision.dropped_count, 0);
        assert!(
            decision.reasons.iter().any(|r| r.contains("reasoning")),
            "expected reasoning reason: {:?}",
            decision.reasons
        );
    }

    #[test]
    fn keeps_last_n_reasoning_items_by_input_order() {
        // N = 3：取末尾 3 条 reasoning（rsn-4 → event-r4, rsn-5 → event-a2, rsn-6 → event-a2）。
        // 保留事件 = {event-r4, event-a2}；event-r1 与 event-a1 被丢弃。
        let policy = RetentionPolicy {
            retained_reasoning_items: 3,
            retained_turns: 0,
            keep_unresolved_tasks: false,
            keep_user_constraints: false,
            keep_modified_files: false,
            keep_pending_tool_calls: false,
            keep_failed_tool_calls: false,
        };
        let decision = apply(&policy, &reasoning_inputs());
        let retained = retained_set(&decision);
        assert!(retained.contains("event-r4"));
        assert!(retained.contains("event-a2"));
        assert!(!retained.contains("event-r1"));
        assert!(!retained.contains("event-a1"));
        // 候选 = 6 messages + 4 reasoning events - 2 共享(event-r2/r3 → event-a1, event-r5/r6 → event-a2)
        // 实际候选事件集合：{event-u1,event-a1,event-u2,event-a2,event-r1,event-r4} = 6
        // 保留 {event-r4,event-a2} = 2 → dropped = 4
        assert_eq!(decision.dropped_count, 4);
        let reason = decision
            .reasons
            .iter()
            .find(|r| r.contains("reasoning"))
            .expect("reasoning reason present");
        assert!(reason.contains("last 3 reasoning item(s) across 2 message"));
    }

    #[test]
    fn zero_retained_reasoning_drops_all_reasoning_events() {
        let policy = RetentionPolicy {
            retained_reasoning_items: 0,
            retained_turns: 0,
            keep_unresolved_tasks: false,
            keep_user_constraints: false,
            keep_modified_files: false,
            keep_pending_tool_calls: false,
            keep_failed_tool_calls: false,
        };
        let decision = apply(&policy, &reasoning_inputs());
        let retained = retained_set(&decision);
        for dropped in ["event-r1", "event-a1", "event-r4", "event-a2"] {
            assert!(!retained.contains(dropped), "expected {dropped} dropped");
        }
        assert!(
            !decision.reasons.iter().any(|r| r.contains("reasoning")),
            "no reasoning reason expected"
        );
    }

    #[test]
    fn reasoning_retention_survives_even_when_turn_is_dropped() {
        // 只保留最近 1 轮（event-u2/event-a2），但 reasoning N=6 会把 event-r1（旧轮的
        // reasoning 事件，不是任何 turn 内的消息）捞回来：确保携带 reasoning 的
        // MessageCommitted 事件被独立保留。
        let policy = RetentionPolicy {
            retained_turns: 1,
            retained_reasoning_items: 6,
            ..RetentionPolicy::default()
        };
        let decision = apply(&policy, &reasoning_inputs());
        let retained = retained_set(&decision);
        // event-r1 是独立 reasoning 事件，被最近 N reasoning 保留；它不在最近一轮消息里。
        assert!(retained.contains("event-r1"));
        // 最近一轮的消息也保留。
        assert!(retained.contains("event-u2"));
        assert!(retained.contains("event-a2"));
        // event-u1 / event-a1 既不在最近一轮，也不被 reasoning（reasoning 取末尾 6 条，
        // 实际上 rsn-2/rsn-3 → event-a1 也被保留，因为末尾 6 条覆盖全部）。
        // 因此 event-a1 通过 reasoning 被保留；event-u1 被丢弃。
        assert!(retained.contains("event-a1"));
        assert!(!retained.contains("event-u1"));
    }

    #[test]
    fn reasoning_decision_never_references_blob_refcount() {
        // 保留决策只产出 EventId；reasons 不应泄漏任何 protected blob 引用语义。
        // 这是一个回归保护：未来若有人误把 ProtectedBlobRef 引入 RetentionReasoning，
        // 该测试会提醒（RetentionReasoning 只有 event_id + reasoning_item_id）。
        let decision = apply(
            &RetentionPolicy::default(),
            &RetentionInputs {
                reasoning_items: vec![reasoning("event-r1", "rsn-1")],
                ..RetentionInputs::default()
            },
        );
        for reason in &decision.reasons {
            assert!(
                !reason.contains("blob") && !reason.contains("refcount"),
                "reason must not mention blob/refcount: {reason}"
            );
        }
        assert_eq!(decision.retained_event_ids, vec![EventId::from("event-r1")]);
    }
}
