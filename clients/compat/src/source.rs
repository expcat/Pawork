//! 外部 Agent 来源枚举与已知配置位置。
//!
//! 五类来源：Claude Code、OpenAI Codex、xAI Grok Build、Cursor 与 Pi。
//! 每种来源只读取其文档化、workspace 内的已知位置（或调用方显式启用的
//! 全局来源根），未知版本只返回诊断，绝不猜测执行。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 外部 Agent 来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalSource {
    /// Anthropic Claude Code。
    Claude,
    /// OpenAI Codex CLI。
    Codex,
    /// xAI Grok Build。
    Grok,
    /// Cursor IDE。
    Cursor,
    /// Pi coding agent。
    Pi,
}

impl ExternalSource {
    pub const ALL: [ExternalSource; 5] = [
        ExternalSource::Claude,
        ExternalSource::Codex,
        ExternalSource::Grok,
        ExternalSource::Cursor,
        ExternalSource::Pi,
    ];

    /// 稳定的小写来源标签（用于 source key、诊断与日志）。
    pub const fn as_str(self) -> &'static str {
        match self {
            ExternalSource::Claude => "claude",
            ExternalSource::Codex => "codex",
            ExternalSource::Grok => "grok",
            ExternalSource::Cursor => "cursor",
            ExternalSource::Pi => "pi",
        }
    }

    /// 确定性来源序：数值越大，在「同 tier、同 canonical id」冲突时优先级越高。
    /// 排序为 Claude < Codex < Grok < Cursor < Pi，与 workspace 内常见
    /// 配置的特异性无关，仅保证同输入同输出的确定性裁决。
    pub const fn rank(self) -> u8 {
        match self {
            ExternalSource::Claude => 1,
            ExternalSource::Codex => 2,
            ExternalSource::Grok => 3,
            ExternalSource::Cursor => 4,
            ExternalSource::Pi => 5,
        }
    }
}

impl std::fmt::Display for ExternalSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 检测到的源文件类别（决定解析器分派）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum SourceFileKind {
    /// 指令文档：CLAUDE.md / AGENTS.md / rules / SYSTEM.md / instructions.md …
    InstructionsDoc,
    /// Claude `settings.json` / `settings.local.json`。
    ClaudeSettings,
    /// Codex / Grok `config.toml`。
    ConfigToml,
    /// 任意来源的 MCP servers JSON（`.mcp.json` / `.cursor/mcp.json` …）。
    McpJson,
    /// Skill 的 `SKILL.md`。
    SkillMarkdown,
    /// Agent / subagent 的 markdown 定义（frontmatter + body）。
    AgentMarkdown,
    /// Codex `agents.json`。
    AgentsJson,
    /// Pi `settings.json`。
    PiSettings,
}

/// 调用方显式启用的全局来源根。全局来源默认不读取（P17-13 步骤 1）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalSource {
    pub source: ExternalSource,
    /// 该来源的用户全局配置根（如 `~/.claude`、`~/.codex`）。
    /// 相对路径只相对此根记录，绝不泄漏宿主绝对路径。
    pub root: PathBuf,
}

impl GlobalSource {
    pub fn new(source: ExternalSource, root: impl Into<PathBuf>) -> Self {
        Self {
            source,
            root: root.into(),
        }
    }
}
