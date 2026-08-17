//! 兼容导入的 canonical 输出模型：条目、状态、诊断与来源追踪。
//!
//! 每条导入结果要么落到 Pawork canonical 类型（instructions / skill /
//! MCP server / Agent Profile v2 / User Hook / Permission rule），要么被显式
//! 标为 Disabled / Unsupported / Conflict；不静默丢弃、不静默放宽权限。
//! 任何条目都不携带明文 Secret：敏感值只以 credential reference 形式出现。

use std::collections::BTreeSet;

use pawork_config::ConfigTier;
use pawork_domain::AgentProfileV2;
use pawork_policy::ApprovalMode;
use serde::{Deserialize, Serialize};

use crate::hook::HookConfig;
use crate::mcp::McpServerConfig;
use crate::source::ExternalSource;

/// 导入条目的 canonical 类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportCategory {
    Instructions,
    Skill,
    McpServer,
    AgentProfile,
    UserHook,
    PermissionRule,
}

impl ImportCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            ImportCategory::Instructions => "instructions",
            ImportCategory::Skill => "skill",
            ImportCategory::McpServer => "mcp_server",
            ImportCategory::AgentProfile => "agent_profile",
            ImportCategory::UserHook => "user_hook",
            ImportCategory::PermissionRule => "permission_rule",
        }
    }
}

/// 条目状态：Imported（可用）/ Disabled（默认禁用，需人工审查）
/// / Unsupported（无 canonical 表达）/ Conflict（同 id 竞争失败）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    Imported,
    Disabled,
    Unsupported,
    Conflict,
}

/// 权限规则裁决。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

impl PermissionDecision {
    /// deny-first 优先级：Deny 2 > Ask 1 > Allow 0。
    pub const fn precedence(self) -> u8 {
        match self {
            PermissionDecision::Allow => 0,
            PermissionDecision::Ask => 1,
            PermissionDecision::Deny => 2,
        }
    }
}

/// 条目的来源追踪：外部来源 + 配置 tier + 相对路径（无宿主绝对路径）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ImportSource {
    pub external: ExternalSource,
    pub tier: ConfigTier,
    /// workspace 相对或全局来源根相对的路径（正斜杠分隔）。
    pub relative_path: String,
}

impl ImportSource {
    pub fn new(
        external: ExternalSource,
        tier: ConfigTier,
        relative_path: impl Into<String>,
    ) -> Self {
        Self {
            external,
            tier,
            relative_path: relative_path.into(),
        }
    }
}

/// 明文 Secret 在源配置中的占位记录：只保留名称与位置，绝不复制值。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PendingCredential {
    /// 环境变量名或 header 名（名字本身不是 secret）。
    pub name: String,
    /// 源配置中的位置（如 servers.fs.env.FS_KEY），不含值。
    pub location: String,
}

/// 已映射为 backend locator 的 credential reference。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CredentialReference {
    pub source: ImportSource,
    pub service: String,
    pub account: String,
    /// 源配置中的位置，不含值。
    pub location: String,
}

/// 诊断严重度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Warning,
    Error,
}

/// 单个导入诊断；消息不得包含文件正文、命令参数或 Secret 明文。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatIssue {
    pub severity: IssueSeverity,
    pub code: String,
    pub category: Option<ImportCategory>,
    pub item_id: Option<String>,
    pub source_path: Option<String>,
    pub message: String,
}

impl CompatIssue {
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: IssueSeverity::Warning,
            code: code.into(),
            category: None,
            item_id: None,
            source_path: None,
            message: message.into(),
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: IssueSeverity::Error,
            code: code.into(),
            category: None,
            item_id: None,
            source_path: None,
            message: message.into(),
        }
    }

    pub fn for_item(
        mut self,
        category: ImportCategory,
        item_id: impl Into<String>,
        source_path: impl Into<String>,
    ) -> Self {
        self.category = Some(category);
        self.item_id = Some(item_id.into());
        self.source_path = Some(source_path.into());
        self
    }

    /// 只标注来源路径（文件级诊断，无具体条目）。
    pub fn with_source(mut self, source_path: impl Into<String>) -> Self {
        self.source_path = Some(source_path.into());
        self
    }
}

/// 条目负载：已映射到 Pawork canonical 类型的内容。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompatPayload {
    /// 指令层（P8 canonical ResourceInstructionKind）。
    Instructions {
        instruction_kind: pawork_resources::ResourceInstructionKind,
        body: String,
        /// 层级深度（AGENTS.md 场景；其余为 0）。
        depth: u32,
    },
    /// Skill：manifest + SKILL.md 正文。
    Skill {
        manifest: pawork_resources::SkillManifest,
        body: String,
    },
    /// MCP server：canonical McpServerConfig + 未落盘的明文凭据占位。
    McpServer {
        name: String,
        server: McpServerConfig,
        pending_credentials: Vec<PendingCredential>,
    },
    /// Agent Profile v2。
    AgentProfile { profile: AgentProfileV2 },
    /// User Hook（P17-1 canonical）。
    UserHook { hook: HookConfig },
    /// 权限规则 / 全局审批模式。
    PermissionRule {
        /// None 表示全局审批模式规则。
        tool: Option<String>,
        decision: PermissionDecision,
        /// 源规则的约束片段（仅工具参数部分，不回显完整规则）。
        spec: Option<String>,
        /// 全局审批模式（tool 规则为 None）。
        approval_mode: Option<ApprovalMode>,
    },
}

/// 单条导入结果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompatItem {
    pub id: String,
    pub category: ImportCategory,
    pub status: ImportStatus,
    pub source: ImportSource,
    /// 需要人工审查后才可在运行时启用（导入阶段绝不执行 handler / MCP）。
    pub requires_review: bool,
    /// Disabled / Unsupported / Conflict 条目载荷为空；Imported 条目必有载荷。
    pub payload: Option<CompatPayload>,
    pub issues: Vec<CompatIssue>,
}

/// 检测摘要（供诊断与测试；不含宿主绝对路径与正文）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DetectedSourceSummary {
    pub external: ExternalSource,
    pub tier: ConfigTier,
    pub relative_path: String,
    pub kind: String,
}

/// 完整导入计划：条目 + 诊断 + credential references + 幂等指纹。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompatPlan {
    pub manifest_version: u32,
    pub sources: BTreeSet<ExternalSource>,
    pub items: Vec<CompatItem>,
    pub issues: Vec<CompatIssue>,
    pub credential_references: Vec<CredentialReference>,
    /// 输入内容指纹；相同指纹重复 apply 直接 noop（幂等）。
    pub fingerprint: String,
}

impl CompatPlan {
    /// 确定性排序：条目按 (category, id, status, source)，诊断按稳定键。
    /// 保证同输入同输出，扫描顺序与文件系统枚举顺序无关。
    pub fn sort_deterministically(&mut self) {
        self.items.sort_by(|left, right| {
            left.category
                .cmp(&right.category)
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| left.status.cmp(&right.status))
                .then_with(|| left.source.external.cmp(&right.source.external))
                .then_with(|| left.source.tier.cmp(&right.source.tier))
                .then_with(|| left.source.relative_path.cmp(&right.source.relative_path))
        });
        self.issues.sort_by(|left, right| {
            left.severity
                .cmp(&right.severity)
                .then_with(|| left.code.cmp(&right.code))
                .then_with(|| left.category.cmp(&right.category))
                .then_with(|| left.item_id.cmp(&right.item_id))
                .then_with(|| left.source_path.cmp(&right.source_path))
                .then_with(|| left.message.cmp(&right.message))
        });
        self.credential_references.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.service.cmp(&right.service))
                .then_with(|| left.account.cmp(&right.account))
        });
    }
}
