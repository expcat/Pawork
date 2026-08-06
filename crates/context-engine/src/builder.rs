//! 确定性上下文构建器与产出。
//!
//! 构建顺序与预算扣减顺序（与 `docs/features/context.md` 一致）：
//! 1. system prompt（由 14 项来源的文本贡献按优先级确定性拼接）
//! 2. 工具 schema（仅计入预算，不进入 messages；由调用方置于请求）
//! 3. 附件（当前轮文件/图片）
//! 4. 历史 + 当前用户消息文本
//! 5. 为 output / thinking 预留空间（已在 [`ContextBudget::max_input_tokens`] 中扣除）
//!
//! 超限时产出 [`CompactionTrigger`]，不在此处压缩。

use agent_domain::{ContentPart, Message, MessageId, MessageMetadata, MessageRole, TextContent};

use crate::budget::{ContextBudget, ContextBudgetBreakdown};
use crate::compaction::{CompactionReason, CompactionTrigger};
use crate::source::{sort_contributions, ContextContribution};
use crate::token::{message_framing_tokens, reply_primer_tokens, TokenEstimator, ToolSchema};

/// 构建结果。
///
/// `messages` 可直接赋值给 `CanonicalModelRequest.messages`。
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltContext {
    /// 按顺序组装的消息：system → 历史 → 当前用户轮。
    pub messages: Vec<Message>,
    /// 估算的输入 token 总量（含 system / 工具 / 附件 / 历史 / reply primer）。
    pub estimated_input_tokens: u64,
    /// 采用的预算。
    pub budget: ContextBudget,
    /// 各项占用明细。
    pub breakdown: ContextBudgetBreakdown,
    /// 超限时的压缩触发信号（None 表示无需压缩）。
    pub compaction: Option<CompactionTrigger>,
}

/// 确定性上下文构建器。
pub struct ContextBuilder<'a> {
    estimator: &'a dyn TokenEstimator,
    budget: ContextBudget,
    contributions: Vec<ContextContribution>,
    history: Vec<Message>,
    attachments: Vec<ContentPart>,
    current_prompt: Option<String>,
    tools: Vec<ToolSchema>,
    history_soft_limit_tokens: Option<u64>,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(estimator: &'a dyn TokenEstimator, budget: ContextBudget) -> Self {
        Self {
            estimator,
            budget,
            contributions: Vec::new(),
            history: Vec::new(),
            attachments: Vec::new(),
            current_prompt: None,
            tools: Vec::new(),
            history_soft_limit_tokens: None,
        }
    }

    /// 追加一条文本贡献（用于 system prompt）。
    pub fn contribution(mut self, contribution: ContextContribution) -> Self {
        self.contributions.push(contribution);
        self
    }

    /// 追加多条文本贡献。
    pub fn contributions(
        mut self,
        contributions: impl IntoIterator<Item = ContextContribution>,
    ) -> Self {
        self.contributions.extend(contributions);
        self
    }

    /// 追加历史消息。
    pub fn history(mut self, history: impl IntoIterator<Item = Message>) -> Self {
        self.history.extend(history);
        self
    }

    /// 设置当前轮附件（文件/图片），成为当前用户消息的一部分。
    pub fn attachments(mut self, attachments: impl IntoIterator<Item = ContentPart>) -> Self {
        self.attachments.extend(attachments);
        self
    }

    /// 设置当前用户 prompt 文本。
    pub fn current_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.current_prompt = Some(prompt.into());
        self
    }

    /// 设置工具定义（仅用于预算估算，不进入 messages）。
    pub fn tools(mut self, tools: impl IntoIterator<Item = ToolSchema>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// 历史软阈值：超过即触发 [`CompactionReason::HistorySoftLimit`]（即使未达硬上限）。
    pub fn history_soft_limit_tokens(mut self, limit: u64) -> Self {
        self.history_soft_limit_tokens = Some(limit);
        self
    }

    /// 组装上下文并估算预算与超限信号。
    pub fn build(mut self) -> BuiltContext {
        let estimator = self.estimator;

        // 1) system prompt：确定性排序后拼接
        sort_contributions(&mut self.contributions);
        let system_text = join_contributions(&self.contributions);

        let mut messages: Vec<Message> = Vec::new();
        let mut system_prompt_tokens = 0u64;
        if !system_text.is_empty() {
            system_prompt_tokens = message_framing_tokens()
                + estimator.count_text("system")
                + estimator.count_text(&system_text);
            messages.push(system_message(system_text));
        }

        // 2) 历史
        let mut history_tokens = 0u64;
        for message in &self.history {
            history_tokens += estimator.count_message(message);
        }
        messages.append(&mut self.history);

        // 3) 当前用户轮（prompt 文本计入 history，附件单独计入 attachment_tokens）
        let mut attachment_tokens = 0u64;
        for part in &self.attachments {
            attachment_tokens += estimator.count_content_part(part);
        }
        let has_current = self.current_prompt.is_some() || !self.attachments.is_empty();
        if has_current {
            let mut content: Vec<ContentPart> = Vec::new();
            if let Some(prompt) = &self.current_prompt {
                content.push(ContentPart::Text(TextContent {
                    text: prompt.clone(),
                }));
            }
            content.extend(self.attachments.iter().cloned());

            history_tokens += message_framing_tokens() + estimator.count_text("user");
            if let Some(prompt) = &self.current_prompt {
                history_tokens += estimator.count_text(prompt);
            }

            messages.push(Message {
                id: MessageId::from("context:user:current"),
                role: MessageRole::User,
                content,
                metadata: MessageMetadata::default(),
            });
        }

        // 4) 工具 schema（仅计入预算）
        let tool_schema_tokens = estimator.count_tool_schemas(&self.tools);

        let estimated_input_tokens = system_prompt_tokens
            + history_tokens
            + attachment_tokens
            + tool_schema_tokens
            + reply_primer_tokens();

        let breakdown = ContextBudgetBreakdown {
            system_prompt_tokens,
            tool_schema_tokens,
            attachment_tokens,
            history_tokens,
            estimated_input_tokens,
            output_reserve_tokens: self.budget.output_reserve_tokens,
            thinking_reserve_tokens: self.budget.thinking_reserve_tokens,
            max_input_tokens: self.budget.max_input_tokens,
        };

        let compaction = compute_compaction(&breakdown, self.history_soft_limit_tokens);

        BuiltContext {
            messages,
            estimated_input_tokens,
            budget: self.budget,
            breakdown,
            compaction,
        }
    }
}

fn join_contributions(contributions: &[ContextContribution]) -> String {
    let mut blocks: Vec<&str> = Vec::new();
    for contribution in contributions {
        if !contribution.content.is_empty() {
            blocks.push(contribution.content.as_str());
        }
    }
    blocks.join("\n\n")
}

fn system_message(text: String) -> Message {
    Message {
        id: MessageId::from("context:system"),
        role: MessageRole::System,
        content: vec![ContentPart::Text(TextContent { text })],
        metadata: MessageMetadata::default(),
    }
}

fn compute_compaction(
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
    use crate::source::ContextSource;
    use crate::token::HeuristicEstimator;
    use agent_domain::{ImageContent, ImageSource};

    fn estimator() -> HeuristicEstimator {
        HeuristicEstimator::default()
    }

    fn user_message(text: &str) -> Message {
        Message {
            id: MessageId::from("m"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: text.into() })],
            metadata: MessageMetadata::default(),
        }
    }

    fn system_text(ctx: &BuiltContext) -> String {
        match ctx.messages.first() {
            Some(Message {
                role: MessageRole::System,
                content,
                ..
            }) => match content.first() {
                Some(ContentPart::Text(text)) => text.text.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    }

    #[test]
    fn build_is_deterministic_regardless_of_insertion_order() {
        let est = estimator();
        let budget = ContextBudget::from_context_window(100_000, 1_000, 500);

        let build = |contribs: Vec<ContextContribution>| {
            ContextBuilder::new(&est, budget.clone())
                .contributions(contribs)
                .history(vec![user_message("hello")])
                .current_prompt("do it")
                .build()
        };

        let a = build(vec![
            ContextContribution::new(ContextSource::AdHocInstructions, "z", "late"),
            ContextContribution::new(ContextSource::CoreSystemPrompt, "a", "core"),
            ContextContribution::new(ContextSource::SecurityPolicy, "m", "policy"),
        ]);
        let b = build(vec![
            ContextContribution::new(ContextSource::SecurityPolicy, "m", "policy"),
            ContextContribution::new(ContextSource::AdHocInstructions, "z", "late"),
            ContextContribution::new(ContextSource::CoreSystemPrompt, "a", "core"),
        ]);

        assert_eq!(a.messages, b.messages);
        assert_eq!(a.estimated_input_tokens, b.estimated_input_tokens);
        assert_eq!(a.compaction, b.compaction);

        // system 内容按优先级顺序拼接：core -> policy -> late
        let text = system_text(&a);
        let core = text.find("core").unwrap();
        let policy = text.find("policy").unwrap();
        let late = text.find("late").unwrap();
        assert!(core < policy && policy < late);
    }

    #[test]
    fn compaction_triggers_when_input_exceeds_budget() {
        let est = estimator();
        let budget = ContextBudget::from_context_window(1_000, 100, 0);
        assert_eq!(budget.max_input_tokens, 900);

        let big = user_message(&"x".repeat(5_000)); // 远超 900
        let ctx = ContextBuilder::new(&est, budget).history(vec![big]).build();

        let trigger = ctx.compaction.expect("should trigger compaction");
        assert_eq!(trigger.reason, CompactionReason::InputBudgetExceeded);
        assert!(ctx.estimated_input_tokens > 900);
        assert_eq!(trigger.estimated_over, ctx.estimated_input_tokens - 900);
    }

    #[test]
    fn no_compaction_when_within_budget() {
        let est = estimator();
        let budget = ContextBudget::from_context_window(100_000, 1_000, 0);
        let max_input = budget.max_input_tokens;
        let ctx = ContextBuilder::new(&est, budget)
            .history(vec![user_message("hi")])
            .build();
        assert!(ctx.compaction.is_none());
        assert!(ctx.estimated_input_tokens <= max_input);
    }

    #[test]
    fn soft_limit_triggers_before_hard_limit() {
        let est = estimator();
        let budget = ContextBudget::from_context_window(100_000, 1_000, 0);
        let max_input = budget.max_input_tokens;
        let msg = user_message(&"y".repeat(400)); // ~100 tokens + framing
        let ctx = ContextBuilder::new(&est, budget)
            .history(vec![msg])
            .history_soft_limit_tokens(50)
            .build();

        let trigger = ctx.compaction.expect("soft trigger");
        assert_eq!(trigger.reason, CompactionReason::HistorySoftLimit);
        // 仍在硬上限内
        assert!(ctx.estimated_input_tokens <= max_input);
        assert!(ctx.breakdown.history_tokens > 50);
    }

    #[test]
    fn attachments_counted_and_emitted_in_current_user_message() {
        let est = estimator();
        let budget = ContextBudget::from_context_window(100_000, 1_000, 0);
        let image = ContentPart::Image(ImageContent {
            source: ImageSource::Base64("AAAA".into()),
            media_type: "image/png".into(),
            alt_text: Some("a diagram".into()),
        });
        let ctx = ContextBuilder::new(&est, budget)
            .current_prompt("look at this")
            .attachments(vec![image])
            .build();

        let last = ctx.messages.last().expect("current user message");
        assert_eq!(last.role, MessageRole::User);
        assert_eq!(last.content.len(), 2); // text + image
        assert!(ctx.breakdown.attachment_tokens >= 85);
        assert!(ctx.compaction.is_none());
    }

    #[test]
    fn tool_schemas_reduce_available_budget_without_entering_messages() {
        let est = estimator();
        let budget = ContextBudget::from_context_window(100_000, 1_000, 0);
        let tools = vec![ToolSchema {
            name: "read_file".into(),
            description: "read a file from disk".into(),
            input_schema: serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}}),
        }];
        let ctx = ContextBuilder::new(&est, budget)
            .tools(tools)
            .current_prompt("hi")
            .build();

        assert!(ctx.breakdown.tool_schema_tokens > 0);
        assert!(ctx
            .messages
            .iter()
            .all(|m| !matches!(m.role, MessageRole::Tool)));
    }
}
