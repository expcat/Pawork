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
    Workspace { workspace_id: WorkspaceId },
    #[default]
    Global,
}

impl HookScope {
    pub fn covers(&self, workspace: Option<&WorkspaceId>) -> bool {
        match self {
            Self::Global => true,
            Self::Workspace { workspace_id } => workspace == Some(workspace_id),
        }
    }
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

/// 六类 handler 的统一配置枚举。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HandlerConfig {
    Command(CommandHandler),
    Http(HttpHandler),
    PromptTransform(PromptTransformHandler),
    PromptEval(PromptEvalHandler),
    AgentEval(AgentEvalHandler),
    McpTool(McpToolHandler),
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

/// Http handler：经 http-runtime 发 webhook（导入后不执行）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HttpHandler {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub header_secret_refs: Vec<SecretRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

fn default_method() -> String {
    "POST".to_string()
}

/// PromptTransform handler：在 PromptAssembled 上改写 Agent 输入。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptTransformHandler {
    pub target: PromptTarget,
    #[serde(default = "default_rewrite_kind")]
    pub rewrite_kind: String,
    pub template: String,
    #[serde(default)]
    pub allow_system_override: bool,
}

/// 改写目标。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptTarget {
    System,
    User,
    Injected,
}

fn default_rewrite_kind() -> String {
    "prefix".to_string()
}

/// PromptEval handler：调用模型做 hook 判定。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEvalHandler {
    pub prompt_template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub on_failure: EvalFallback,
}

/// AgentEval handler：用受限 Agent 执行 hook 判定。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEvalHandler {
    pub restricted_profile: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetLimit>,
    pub prompt_template: String,
    #[serde(default)]
    pub on_failure: EvalFallback,
}

/// 受限预算。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetLimit {
    pub max_tokens: Option<u64>,
    pub timeout_ms: Option<u64>,
}

/// Eval 失败/超时降级策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalFallback {
    Allow,
    #[default]
    Deny,
    SafeTransform,
}

/// McpTool handler：调用 MCP tool 作为 hook handler。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolHandler {
    pub server_id: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_template: Option<serde_json::Value>,
    #[serde(default)]
    pub on_failure: McpFallback,
}

/// McpTool 调用失败时的降级决策。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpFallback {
    Allow,
    #[default]
    Deny,
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

impl TriggerPoint {
    pub const ALL: &'static [TriggerPoint] = &[
        TriggerPoint::SessionStart,
        TriggerPoint::SessionEnd,
        TriggerPoint::RunStarted,
        TriggerPoint::RunCompleted,
        TriggerPoint::RunFailed,
        TriggerPoint::PromptAssembled,
        TriggerPoint::PreToolUse,
        TriggerPoint::PostToolUse,
        TriggerPoint::ToolFailed,
        TriggerPoint::PermissionRequest,
        TriggerPoint::SubagentStart,
        TriggerPoint::SubagentStop,
        TriggerPoint::TaskStarted,
        TriggerPoint::TaskCompleted,
        TriggerPoint::PreCompact,
        TriggerPoint::PostCompact,
        TriggerPoint::Notification,
    ];
}
