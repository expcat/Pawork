//! 超限检测与压缩触发信号。
//!
//! context-engine **只产出触发决策**，不执行真正的压缩；压缩引擎位于
//! `compaction-engine`。调用方拿到 [`CompactionTrigger`] 后自行决定是否调用压缩引擎
//! 并重建上下文。`compaction-engine::CompactionReason` 另含手动触发的 `Manual`；
//! 自动原因由该 crate 提供显式 `From` 映射。

use serde::{Deserialize, Serialize};

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
}
