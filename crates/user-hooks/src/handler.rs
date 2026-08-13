//! HookHandler 统一模型与 HookId（P17-1 步骤 2）。

use crate::capability::HookCapability;
use crate::config::{HandlerLifecycle, HookConfig, HookScope};
use crate::error::HookError;
use crate::trigger::TriggerPoint;

/// Hook 的稳定标识。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HookId(pub String);

impl HookId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HookId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 解析后的 HookHandler。每个 handler 声明 trigger、scope、lifecycle、capability
/// 与具体配置；执行经依赖注入执行器完成，自身不含 Provider/平台名称分支。
#[derive(Clone, Debug)]
pub struct HookHandler {
    pub id: HookId,
    pub trigger: TriggerPoint,
    pub scope: HookScope,
    pub lifecycle: HandlerLifecycle,
    pub enabled: bool,
    pub capability: HookCapability,
    pub config: HookConfig,
}

impl HookHandler {
    /// 从用户配置解析。校验 id 非空、secret 引用与 allowlist 对齐、scope 合法。
    pub fn from_config(config: HookConfig) -> Result<Self, HookError> {
        if config.id.trim().is_empty() {
            return Err(HookError::InvalidConfig("hook id must be non-empty".into()));
        }
        validate_handler(&config)?;
        let capability = config.capability();
        Ok(Self {
            id: HookId::new(config.id.clone()),
            trigger: config.trigger,
            scope: config.scope.clone(),
            lifecycle: config.effective_lifecycle(),
            enabled: config.enabled,
            capability,
            config,
        })
    }

    /// 该 handler 是否匹配触发点与 workspace 作用域。
    pub fn matches(
        &self,
        trigger: TriggerPoint,
        workspace: Option<&agent_domain::WorkspaceId>,
    ) -> bool {
        self.enabled && self.trigger == trigger && self.scope.covers(workspace)
    }
}

fn validate_handler(config: &HookConfig) -> Result<(), HookError> {
    use crate::config::HandlerConfig;
    match &config.handler {
        HandlerConfig::Command(c) => {
            if c.program.trim().is_empty() {
                return Err(HookError::InvalidConfig("command.program is empty".into()));
            }
            if c.allowed_env.len() != c.env_secret_refs.len() {
                return Err(HookError::InvalidConfig(
                    "command.allowed_env and env_secret_refs length mismatch".into(),
                ));
            }
        }
        HandlerConfig::Http(h) => {
            if h.url.trim().is_empty() {
                return Err(HookError::InvalidConfig("http.url is empty".into()));
            }
            if h.allowed_headers.len() != h.header_secret_refs.len() {
                return Err(HookError::InvalidConfig(
                    "http.allowed_headers and header_secret_refs length mismatch".into(),
                ));
            }
        }
        HandlerConfig::PromptTransform(p) => {
            if p.template.is_empty() {
                return Err(HookError::InvalidConfig(
                    "prompt_transform.template is empty".into(),
                ));
            }
        }
        HandlerConfig::PromptEval(p) => {
            if p.prompt_template.is_empty() {
                return Err(HookError::InvalidConfig(
                    "prompt_eval.prompt_template is empty".into(),
                ));
            }
        }
        HandlerConfig::AgentEval(a) => {
            if a.restricted_profile.trim().is_empty() {
                return Err(HookError::InvalidConfig(
                    "agent_eval.restricted_profile is empty".into(),
                ));
            }
            if a.prompt_template.is_empty() {
                return Err(HookError::InvalidConfig(
                    "agent_eval.prompt_template is empty".into(),
                ));
            }
            let Some(budget) = a.budget else {
                return Err(HookError::InvalidConfig(
                    "agent_eval requires an explicit token/time budget".into(),
                ));
            };
            if budget.max_tokens.is_none_or(|value| value == 0)
                || budget.timeout_ms.is_none_or(|value| value == 0)
            {
                return Err(HookError::InvalidConfig(
                    "agent_eval budget requires positive max_tokens and timeout_ms".into(),
                ));
            }
        }
        HandlerConfig::McpTool(m) => {
            if m.server_id.trim().is_empty() || m.tool_name.trim().is_empty() {
                return Err(HookError::InvalidConfig(
                    "mcp_tool.server_id and tool_name must be non-empty".into(),
                ));
            }
        }
    }
    Ok(())
}
