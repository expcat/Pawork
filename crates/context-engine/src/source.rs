//! 上下文来源优先级与文本贡献。
//!
//! 对应 `docs/features/context.md` 的 14 项上下文来源。优先级由
//! [`ContextSource::priority`] 给出，**不**依赖任何哈希遍历顺序；同一来源的多条
//! 贡献再按 `source_key` 字典序稳定排序，从而保证「同输入同输出」的确定性。

use serde::{Deserialize, Serialize};

/// 上下文来源类别（14 项，见 `docs/features/context.md`）。
///
/// 变体声明顺序即为文档编号顺序；[`ContextSource::priority`] 显式给出该编号，
/// 二者必须保持一致（由单元测试守护）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    /// 1. Core System Prompt
    CoreSystemPrompt,
    /// 2. Agent Profile
    AgentProfile,
    /// 3. 安全和权限策略
    SecurityPolicy,
    /// 4. 用户全局 Instructions
    UserGlobalInstructions,
    /// 5. 工作区 Instructions
    WorkspaceInstructions,
    /// 6. 根目录 `AGENTS.md`
    RootAgentsFile,
    /// 7. 当前文件路径层级的 `AGENTS.md`
    PathAgentsFile,
    /// 8. 激活的 Skills
    ActiveSkills,
    /// 9. 用户选择的 Prompt Template
    PromptTemplate,
    /// 10. Session Summary
    SessionSummary,
    /// 11. 历史消息
    HistoryMessages,
    /// 12. 文件和图片附件
    Attachments,
    /// 13. 工具定义
    ToolDefinitions,
    /// 14. 临时运行指令
    AdHocInstructions,
}

impl ContextSource {
    /// 优先级：数值越小越靠前（越高优先级）。映射由文档编号派生，与变体声明顺序一致。
    pub const fn priority(self) -> u8 {
        match self {
            ContextSource::CoreSystemPrompt => 1,
            ContextSource::AgentProfile => 2,
            ContextSource::SecurityPolicy => 3,
            ContextSource::UserGlobalInstructions => 4,
            ContextSource::WorkspaceInstructions => 5,
            ContextSource::RootAgentsFile => 6,
            ContextSource::PathAgentsFile => 7,
            ContextSource::ActiveSkills => 8,
            ContextSource::PromptTemplate => 9,
            ContextSource::SessionSummary => 10,
            ContextSource::HistoryMessages => 11,
            ContextSource::Attachments => 12,
            ContextSource::ToolDefinitions => 13,
            ContextSource::AdHocInstructions => 14,
        }
    }

    /// 稳定的字符串标识，用于日志与诊断。
    pub const fn as_str(self) -> &'static str {
        match self {
            ContextSource::CoreSystemPrompt => "core_system_prompt",
            ContextSource::AgentProfile => "agent_profile",
            ContextSource::SecurityPolicy => "security_policy",
            ContextSource::UserGlobalInstructions => "user_global_instructions",
            ContextSource::WorkspaceInstructions => "workspace_instructions",
            ContextSource::RootAgentsFile => "root_agents_file",
            ContextSource::PathAgentsFile => "path_agents_file",
            ContextSource::ActiveSkills => "active_skills",
            ContextSource::PromptTemplate => "prompt_template",
            ContextSource::SessionSummary => "session_summary",
            ContextSource::HistoryMessages => "history_messages",
            ContextSource::Attachments => "attachments",
            ContextSource::ToolDefinitions => "tool_definitions",
            ContextSource::AdHocInstructions => "ad_hoc_instructions",
        }
    }

    /// 全部来源，按优先级升序（用于校验与测试）。
    pub const ALL: [ContextSource; 14] = [
        ContextSource::CoreSystemPrompt,
        ContextSource::AgentProfile,
        ContextSource::SecurityPolicy,
        ContextSource::UserGlobalInstructions,
        ContextSource::WorkspaceInstructions,
        ContextSource::RootAgentsFile,
        ContextSource::PathAgentsFile,
        ContextSource::ActiveSkills,
        ContextSource::PromptTemplate,
        ContextSource::SessionSummary,
        ContextSource::HistoryMessages,
        ContextSource::Attachments,
        ContextSource::ToolDefinitions,
        ContextSource::AdHocInstructions,
    ];
}

/// 来自单一上下文来源的文本贡献。多条贡献会被合并进 system prompt。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextContribution {
    pub source: ContextSource,
    /// 同来源内的稳定排序键（字典序），保证确定性。
    pub source_key: String,
    pub content: String,
}

impl ContextContribution {
    pub fn new(
        source: ContextSource,
        source_key: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            source,
            source_key: source_key.into(),
            content: content.into(),
        }
    }

    /// 排序键：(来源优先级, source_key)，保证全序与确定性。
    fn sort_key(&self) -> (u8, &str) {
        (self.source.priority(), self.source_key.as_str())
    }
}

/// 按 (来源优先级, source_key) 对贡献做稳定排序。
///
/// 使用 `sort_by`（稳定排序）配合全序键，与输入顺序无关，结果确定。
pub fn sort_contributions(contributions: &mut [ContextContribution]) {
    contributions.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_match_documented_order() {
        assert_eq!(ContextSource::ALL.len(), 14);
        // 声明顺序即优先级升序
        for window in ContextSource::ALL.windows(2) {
            assert!(
                window[0].priority() < window[1].priority(),
                "{:?} should outrank {:?}",
                window[0],
                window[1]
            );
        }
        assert_eq!(ContextSource::CoreSystemPrompt.priority(), 1);
        assert_eq!(ContextSource::AdHocInstructions.priority(), 14);
    }

    #[test]
    fn sort_is_deterministic_regardless_of_input_order() {
        let mut a = vec![
            ContextContribution::new(ContextSource::AdHocInstructions, "z", "1"),
            ContextContribution::new(ContextSource::CoreSystemPrompt, "a", "2"),
            ContextContribution::new(ContextSource::SecurityPolicy, "m", "3"),
            // 同来源、不同 key，用于验证次级排序
            ContextContribution::new(ContextSource::CoreSystemPrompt, "b", "4"),
        ];
        let mut b = a.clone();
        // 打乱输入顺序
        b.reverse();
        b.swap(0, 1);

        sort_contributions(&mut a);
        sort_contributions(&mut b);

        assert_eq!(a, b, "排序结果必须与输入顺序无关");
        let keys: Vec<(u8, &str)> = a.iter().map(ContextContribution::sort_key).collect();
        assert_eq!(
            keys,
            vec![(1, "a"), (1, "b"), (3, "m"), (14, "z")],
            "应先按优先级、再按 source_key 字典序"
        );
    }

    #[test]
    fn contribution_round_trips_through_serde() {
        let c = ContextContribution::new(ContextSource::RootAgentsFile, "root", "# Agents");
        let json = serde_json::to_string(&c).expect("serialize");
        let back: ContextContribution = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, back);
    }
}
