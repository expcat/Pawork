//! Pawork 的确定性资源加载层。
//!
//! 加载 `AGENTS.md`、Skills 与 Agent Profile（v1 视图 + 自动迁 v2）。调用方只能用
//! `workspace_id + root_index + relative_path` 指定工作区目标；单个资源损坏会被隔离为
//! [`ResourceIssue`]，不会让整批加载崩溃。

mod agents;
mod error;
mod io;
mod loader;
mod profiles;
mod request;
mod skills;
mod source;

pub use agents::{AgentsDocument, AgentsHierarchy};
pub use error::ResourceLoadError;
pub use loader::{ResourceBundle, ResourceInstruction, ResourceInstructionKind, ResourceLoader};
pub use profiles::{AgentProfile, LoadedAgentProfileV2, ResolvedInstructions};
pub use request::{
    CurrentPathKind, ResourceLimits, ResourceLoaderOptions, ResourceRequest, ResourceSelection,
    WorkspaceRelativePath,
};
pub use skills::{
    LoadedSkill, SkillDependency, SkillManifest, SkillParameter, SkillResolution, SkillScript,
};
pub use source::{
    ResourceDiagnosticEntry, ResourceDiagnosticStatus, ResourceDiagnostics, ResourceIssue,
    ResourceIssueSeverity, ResourceKind, ResourceOrigin, ResourceProvenance,
};
