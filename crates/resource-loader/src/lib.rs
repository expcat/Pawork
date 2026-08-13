//! Pawork 的确定性资源加载层。
//!
//! 加载 `AGENTS.md`、Skills、Prompt Templates 与 Agent Profile v1。调用方只能用
//! `workspace_id + root_index + relative_path` 指定工作区目标；单个资源损坏会被隔离为
//! [`ResourceIssue`]，不会让整批加载或 Core 崩溃。

mod agents;
mod diagnostics;
mod error;
mod hooks;
mod io;
mod loader;
mod lsp;
mod profiles;
mod request;
mod skills;
mod source;
mod templates;
mod watch;

pub use config_service::ConfigTier;

pub use agents::{AgentsDocument, AgentsHierarchy};
pub use diagnostics::ResourceDiagnosticView;
pub use error::ResourceLoadError;
pub use hooks::{load_hooks, UserHookConfig, UserHookScope};
pub use loader::{ResourceBundle, ResourceInstruction, ResourceInstructionKind, ResourceLoader};
pub use lsp::LanguageServerResource;
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
pub use templates::{
    PromptDefaults, PromptParameter, PromptTemplate, RenderedPrompt, TemplateResolution,
};
pub use watch::{HotReloadSnapshot, ResourceHotReload, ResourceWatcher};
