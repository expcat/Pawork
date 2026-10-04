//! User Hook 数据形状（只读导入用）。
//!
//! 从 V1 `user-hooks` 拷贝配置类型，不含 plugin_api / capability / executor。
//! 导入的 hook 必须 `enabled=false` 且 `requires_review=true`，本 crate 不执行。

use pawork_domain::WorkspaceId;
use serde::{Deserialize, Serialize};

/// Secret 引用：只携带逻辑名，永不包含明文。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(pub String);

impl SecretRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Handler 生命周期：同步阻断或 async fire-and-forget。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandlerLifecycle {
    Sync,
    Async,
}

/// Hook 作用域：workspace 级或 global。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookScope {
    Workspace {
        workspace_id: WorkspaceId,
    },
    #[default]
    Global,
}

/// 完整的 user hook 配置。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HookConfig {
    pub id: String,
    pub trigger: TriggerPoint,
    #[serde(default)]
    pub scope: HookScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<HandlerLifecycle>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub handler: HandlerConfig,
}

fn default_enabled() -> bool {
    true
}

/// handler 配置枚举。解析层只映射 command 型；其余 handler 类型一律
/// 标 Unsupported，不进入计划。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HandlerConfig {
    Command(CommandHandler),
}

/// Command handler：经 Sandbox→Process 执行外部命令（导入后不执行）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandHandler {
    pub program: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_env: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_secret_refs: Vec<SecretRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// User hook 触发点。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPoint {
    SessionStart,
    SessionEnd,
    RunStarted,
    RunCompleted,
    RunFailed,
    PromptAssembled,
    PreToolUse,
    PostToolUse,
    ToolFailed,
    PermissionRequest,
    SubagentStart,
    SubagentStop,
    TaskStarted,
    TaskCompleted,
    PreCompact,
    PostCompact,
    Notification,
}
