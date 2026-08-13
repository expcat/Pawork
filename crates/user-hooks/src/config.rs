//! User Hook 配置 schema（P17-1 步骤 2、4-7）。
//!
//! [`HookConfig`] 是用户配置驱动的声明式 hook 定义：trigger + scope +
//! lifecycle + 六类 [`HandlerConfig`] 之一。所有外部资源（命令、URL、MCP
//! server、provider profile）经依赖注入执行器消费；本配置不含 Provider 名分支，
//! secret 只存引用。

use crate::capability::HookCapability;
use crate::secret::SecretRef;
use crate::trigger::TriggerPoint;
use agent_domain::WorkspaceId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Handler 生命周期：同步阻断（等待结果回灌）或 async fire-and-forget。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandlerLifecycle {
    /// 同步阻断：dispatcher 等待结果并回灌决策（PromptTransform/PromptEval/
    /// AgentEval/McpTool 默认值）。超时按策略降级。
    Sync,
    /// Async fire-and-forget：dispatcher 投递后立即返回，不阻塞 run loop，
    /// 失败仅记录审计（Command/Http 通知类默认值）。
    Async,
}

/// Hook 作用域：workspace 级或 global。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookScope {
    /// 仅在指定 workspace 触发。
    Workspace { workspace_id: WorkspaceId },
    /// 全局（所有 workspace）。
    #[default]
    Global,
}

impl HookScope {
    /// 判断该 scope 是否覆盖给定 workspace。
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
    /// 显式覆盖；缺省按 capability 取默认 lifecycle。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<HandlerLifecycle>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub handler: HandlerConfig,
}

fn default_enabled() -> bool {
    true
}

impl HookConfig {
    /// 解析后该 hook 请求的能力。
    pub fn capability(&self) -> HookCapability {
        self.handler.capability()
    }

    /// 生效 lifecycle（显式优先，否则按 capability 默认）。
    pub fn effective_lifecycle(&self) -> HandlerLifecycle {
        self.lifecycle
            .unwrap_or_else(|| self.capability().default_lifecycle())
    }
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

impl HandlerConfig {
    pub fn capability(&self) -> HookCapability {
        match self {
            Self::Command(_) => HookCapability::Process,
            Self::Http(_) => HookCapability::Network,
            Self::PromptTransform(_) => HookCapability::PromptTransform,
            Self::PromptEval(_) => HookCapability::PromptEval,
            Self::AgentEval(_) => HookCapability::AgentEval,
            Self::McpTool(_) => HookCapability::McpTool,
        }
    }
}

/// Command handler：经 Sandbox→Process 执行外部命令。
///
/// **执行所有权约束**：本 handler 自身不 spawn 进程，命令统一交由注入的
/// [`crate::CommandExecutor`]（app-service 接 Sandbox Runtime → Process Runtime）
/// 执行；进程生命周期 / policy 判定 / 进程树回收由 Sandbox/Process 承担。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandHandler {
    pub program: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// 注入到子进程的环境变量名 allowlist；仅这些名字会从 secret 解析注入。
    /// 名字本身非 secret，明文值不落配置。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_env: Vec<String>,
    /// 每个 allowed_env 对应的 secret 引用（位置对齐）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_secret_refs: Vec<SecretRef>,
    /// 工作区相对路径（绝对路径由可信 Workspace 服务解析）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    /// 同步模式下生效的超时（毫秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Http handler：经 http-runtime 发 webhook。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HttpHandler {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    /// allowlisted header 名（非 secret）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_headers: Vec<String>,
    /// 与 allowed_headers 对齐的 secret 引用。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub header_secret_refs: Vec<SecretRef>,
    /// body 模板（含 `{trigger}` / `{details}` 占位符，渲染前 redaction）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

fn default_method() -> String {
    "POST".to_string()
}

/// PromptTransform handler：在 PromptAssembled 上改写 Agent 输入。
///
/// 改写以 canonical 审计事件记录（diff + 作用域），且**不允许绕过 system /
/// security policy**——改写结果仍须经 PolicyGate 复核（target=System 的改写
/// 默认被 PolicyGate 拒绝，除非显式 allow）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptTransformHandler {
    /// 改写目标。
    pub target: PromptTarget,
    /// 改写策略：目前支持 prefix/suffix/replace（由注入的执行器解释具体语义）。
    #[serde(default = "default_rewrite_kind")]
    pub rewrite_kind: String,
    /// 改写内容模板（渲染前 redaction）。
    pub template: String,
    /// 是否允许改写 system prompt（默认 false；为 true 时 PolicyGate 仍可拒绝）。
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

/// PromptEval handler：调用模型做 hook 判定（canonical provider，不按名分支）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEvalHandler {
    /// 判定 prompt 模板（渲染前 redaction）。
    pub prompt_template: String,
    /// 期望返回的结构化判定 schema（JSON schema 片段，传给注入 ProviderJudge）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<serde_json::Value>,
    /// 失败/超时时的降级决策。
    #[serde(default)]
    pub on_failure: EvalFallback,
}

/// AgentEval handler：用受限 Agent 执行 hook 判定。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEvalHandler {
    /// 受限 Agent profile 引用（独立 profile、受限 tools、受限预算；由注入
    /// ProviderJudge 解释，本配置不内嵌特权）。
    pub restricted_profile: String,
    /// 工具 allowlist（canonical 工具名）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_allowlist: Vec<String>,
    /// 预算上限（token / 时间，由注入执行器解释）。
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

/// Eval 失败/超时降级策略。缺省必须 fail-closed；`Allow` / `SafeTransform`
/// 只有在宿主明确判定当前 workspace 受信时才可生效。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalFallback {
    /// 失败视为允许继续（不阻断）。
    Allow,
    /// 失败视为阻断。
    #[default]
    Deny,
    /// 失败时改写为安全 prompt（由 PolicyGate 复核）。
    SafeTransform,
}

/// McpTool handler：调用 MCP tool 作为 hook handler。
///
/// 复用 `mcp-client`（P9）；P9-5 每 server 独立审批与输出限制由注入的
/// [`crate::McpToolInvoker`] 承担，handler 不获额外特权。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolHandler {
    /// MCP server 引用（canonical，非特权）。
    pub server_id: String,
    pub tool_name: String,
    /// 参数模板（JSON；渲染前 redaction）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_template: Option<serde_json::Value>,
    /// 调用失败（`success=false` 或 invoke 错误）时的显式降级决策。
    /// 默认 fail-closed（[`McpFallback::Deny`]）。
    #[serde(default)]
    pub on_failure: McpFallback,
}

/// McpTool 调用失败时的降级决策。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpFallback {
    /// 失败视为允许继续。
    Allow,
    /// fail-closed：失败视为阻断（默认）。
    #[default]
    Deny,
}

/// 渲染上下文（占位符替换 + secret redaction 后的结果），传给各 handler 执行器。
#[derive(Clone, Default)]
pub struct RenderContext<'a> {
    /// 已 redaction 的触发负载序列化文本，用于 `{trigger}` 占位。
    pub trigger_json: String,
    /// 已 redaction 的 details 文本。
    pub details_json: String,
    /// 解析出的 secret 明文（短生命周期，仅用于最终注入执行器）。
    pub secrets: Vec<&'a crate::secret::SecretValue>,
    /// 渲染后的额外变量（key=变量名，value=已 redaction 文本）。
    pub vars: BTreeMap<String, String>,
}
