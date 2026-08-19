//! 超限检测与压缩触发信号（自 V1 `context-engine::compaction` 迁入；
//! `compute_compaction` 由 V1 `builder.rs` 提取为公开纯函数，语义不变：
//! 硬限优先、软限次之）。
//!
//! context 侧只产出触发决策，不执行压缩；session 侧 fork/snapshot 由 host 经
//! `LoopContext::compact_history` 回调完成，engine 只拿回 [`CompactionOutcome`]
//! 元数据。[`AutoCompactionReason`] 是 engine → host 的原因传递（手动入口映射
//! `Manual` 语义），app 侧再映射到 `pawork-storage::session` 的 `CompactionReason`；
//! session 不反向依赖 engine。

use serde::{Deserialize, Serialize};

use crate::context::budget::ContextBudgetBreakdown;

/// 触发压缩的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    /// 历史消息超过软阈值（接近上限，建议提前压缩）。
    HistorySoftLimit,
    /// 输入超过 `max_input_tokens`（硬上限）。
    InputBudgetExceeded,
}

/// 压缩触发信号。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionTrigger {
    pub reason: CompactionReason,
    /// 预计超出 token 数（相对上限的正差额）。
    pub estimated_over: u64,
}

impl CompactionTrigger {
    pub fn new(reason: CompactionReason, estimated_over: u64) -> Self {
        Self {
            reason,
            estimated_over,
        }
    }
}

/// engine → host 的压缩原因（含手动入口）。自动触发携带
/// [`CompactionReason`] 的映射；[`crate::run_manual_compaction`] 使用 `Manual`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoCompactionReason {
    /// 用户显式手动压缩（REPL `/compact` 等）。
    Manual,
    /// 历史消息超过软阈值。
    HistorySoftLimit,
    /// 输入超过 `max_input_tokens` 硬上限。
    InputBudgetExceeded,
}

impl From<CompactionReason> for AutoCompactionReason {
    fn from(reason: CompactionReason) -> Self {
        match reason {
            CompactionReason::HistorySoftLimit => Self::HistorySoftLimit,
            CompactionReason::InputBudgetExceeded => Self::InputBudgetExceeded,
        }
    }
}

/// 触发判定纯函数：硬上限优先于软阈值。
///
/// - `estimated_input_tokens > max_input_tokens` → `InputBudgetExceeded`；
/// - 否则若配置软限且 `history_tokens > soft` → `HistorySoftLimit`；
/// - 否则 `None`。
pub fn compute_compaction(
    breakdown: &ContextBudgetBreakdown,
    history_soft_limit: Option<u64>,
) -> Option<CompactionTrigger> {
    // 硬上限优先于软阈值
    if breakdown.estimated_input_tokens > breakdown.max_input_tokens {
        let over = breakdown.estimated_input_tokens - breakdown.max_input_tokens;
        return Some(CompactionTrigger::new(
            CompactionReason::InputBudgetExceeded,
            over,
        ));
    }
    if let Some(soft) = history_soft_limit {
        if breakdown.history_tokens > soft {
            let over = breakdown.history_tokens - soft;
            return Some(CompactionTrigger::new(
                CompactionReason::HistorySoftLimit,
                over,
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_round_trips_through_serde() {
        let trigger = CompactionTrigger::new(CompactionReason::InputBudgetExceeded, 42);
        let json = serde_json::to_string(&trigger).expect("serialize");
        let back: CompactionTrigger = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(trigger, back);
    }

    #[test]
    fn reason_renames_to_snake_case() {
        let json = serde_json::to_string(&CompactionReason::HistorySoftLimit).expect("serialize");
        assert!(json.contains("history_soft_limit"));
    }

    #[test]
    fn hard_limit_takes_priority_over_soft() {
        let breakdown = ContextBudgetBreakdown {
            history_tokens: 600,
            estimated_input_tokens: 2_000,
            max_input_tokens: 1_000,
            ..ContextBudgetBreakdown::default()
        };
        let trigger = compute_compaction(&breakdown, Some(500)).expect("hard trigger");
        assert_eq!(trigger.reason, CompactionReason::InputBudgetExceeded);
        assert_eq!(trigger.estimated_over, 1_000);
    }

    #[test]
    fn soft_limit_triggers_within_hard_budget() {
        let breakdown = ContextBudgetBreakdown {
            history_tokens: 600,
            estimated_input_tokens: 700,
            max_input_tokens: 1_000,
            ..ContextBudgetBreakdown::default()
        };
        let trigger = compute_compaction(&breakdown, Some(500)).expect("soft trigger");
        assert_eq!(trigger.reason, CompactionReason::HistorySoftLimit);
        assert_eq!(trigger.estimated_over, 100);
        assert!(breakdown.estimated_input_tokens <= breakdown.max_input_tokens);
    }

    #[test]
    fn no_trigger_within_limits_or_without_soft_limit() {
        let within = ContextBudgetBreakdown {
            history_tokens: 100,
            estimated_input_tokens: 200,
            max_input_tokens: 1_000,
            ..ContextBudgetBreakdown::default()
        };
        assert_eq!(compute_compaction(&within, Some(500)), None);
        assert_eq!(compute_compaction(&within, None), None);
    }
}
