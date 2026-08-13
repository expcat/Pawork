//! Pawork 跨 Agent 配置兼容加载（P17-13）。
//!
//! 输入侧 Adapter：只读探测 Claude Code / OpenAI Codex / xAI Grok / Cursor /
//! Pi 的已知项目配置与扩展资源，映射为 Pawork canonical Instructions、Skill、
//! MCP server、Agent Profile v2、User Hook 与 Permission rule。
//!
//! 安全边界：
//! - 加载与预览阶段绝不执行 hook、MCP、script 或任何外部进程 / 网络请求；
//! - 明文 Secret 一律丢弃，只保留 credential reference（名称 / 位置）；
//! - 外部配置永远不是运行时事实源：导入的 hook 默认 disabled，MCP 与权限
//!   条目带 requires_review，无法安全映射的内容标为 Unsupported / Disabled；
//! - 原文件只读、不改写；重复 apply 幂等。
//!
//! 典型流程：`CompatLoader::scan`（只读）→ `CompatPlan::preview`（dry-run
//! 预览）→ 用户确认后 `CompatLoader::apply` 把计划写入指定输出目录。

mod apply;
mod detect;
pub mod error;
mod frontmatter;
mod io;
pub mod limits;
mod map;
pub mod model;
mod parse;
pub mod source;

pub use apply::{ApplyOutcome, ApplyReport, CompatLoader};
pub use limits::CompatLimits;
pub use model::{
    CompatIssue, CompatItem, CompatPayload, CompatPlan, CredentialReference, DetectedSourceSummary,
    ImportCategory, ImportSource, ImportStatus, IssueSeverity, PendingCredential,
    PermissionDecision,
};
pub use source::{ExternalSource, GlobalSource};
