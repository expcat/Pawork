//! Token 预算与构建后的占用明细。

use serde::{Deserialize, Serialize};

/// 上下文 Token 预算。
///
/// `max_input_tokens` 由上下文窗口扣除输出与思考预留得到，是输入侧的硬上限；
/// 构建时按 system prompt → 工具 schema → 附件 → 历史 的顺序占用，超出即触发压缩。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudget {
    /// 模型上下文窗口上限。
    pub context_window_tokens: u64,
    /// 为模型输出预留的 token。
    pub output_reserve_tokens: u64,
    /// 为推理/思考预留的 token（0 表示不启用 thinking）。
    pub thinking_reserve_tokens: u64,
    /// 输入侧硬上限 = context_window - output_reserve - thinking_reserve（饱和到 0）。
    pub max_input_tokens: u64,
}

impl ContextBudget {
    /// 从上下文窗口与两项预留推导出 `max_input_tokens`。
    pub fn from_context_window(
        context_window_tokens: u64,
        output_reserve_tokens: u64,
        thinking_reserve_tokens: u64,
    ) -> Self {
        let reserved = output_reserve_tokens.saturating_add(thinking_reserve_tokens);
        let max_input_tokens = context_window_tokens.saturating_sub(reserved);
        Self {
            context_window_tokens,
            output_reserve_tokens,
            thinking_reserve_tokens,
            max_input_tokens,
        }
    }

    /// 已为输出与思考预留的总额。
    pub const fn reserved_tokens(&self) -> u64 {
        self.output_reserve_tokens + self.thinking_reserve_tokens
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        // 保守默认：128k 窗口、4k 输出、不启用 thinking。
        Self::from_context_window(128_000, 4_096, 0)
    }
}

/// 构建后各项占用的明细（用于诊断与压缩决策）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudgetBreakdown {
    pub system_prompt_tokens: u64,
    pub tool_schema_tokens: u64,
    pub attachment_tokens: u64,
    pub history_tokens: u64,
    /// 输入 token 总量（system + history + attachment + tool + reply primer）。
    pub estimated_input_tokens: u64,
    pub output_reserve_tokens: u64,
    pub thinking_reserve_tokens: u64,
    pub max_input_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_output_and_thinking_from_input_ceiling() {
        let budget = ContextBudget::from_context_window(10_000, 2_000, 1_000);
        assert_eq!(budget.max_input_tokens, 7_000);
        assert_eq!(budget.reserved_tokens(), 3_000);
    }

    #[test]
    fn saturates_when_reserves_exceed_window() {
        let budget = ContextBudget::from_context_window(1_000, 2_000, 0);
        assert_eq!(budget.max_input_tokens, 0);
    }

    #[test]
    fn round_trips_through_serde() {
        let budget = ContextBudget::from_context_window(8_000, 1_000, 256);
        let json = serde_json::to_string(&budget).expect("serialize");
        let back: ContextBudget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(budget, back);
    }
}
