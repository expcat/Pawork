use policy_engine::{ApprovalMode, ExecutionConstraints, PolicyDecision, PolicyInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tool_api::ToolCapability;

use crate::action::BrowserComputerAction;

/// 把 canonical action 映射为 Policy 使用的 `ToolCapability`。
pub fn action_capability(action: &BrowserComputerAction) -> ToolCapability {
    if action.is_read_only() {
        ToolCapability::ReadOnly
    } else {
        ToolCapability::Network
    }
}

/// 构造一次 action 的 `PolicyInput`。
pub fn policy_input_for(
    action: &BrowserComputerAction,
    input: &Value,
    trusted: bool,
    approval_mode: ApprovalMode,
) -> PolicyInput {
    PolicyInput {
        capability: action_capability(action),
        input: input.clone(),
        trusted,
        allowed_in_untrusted_workspace: trusted,
        approval_mode,
    }
}

/// 归一 policy 裁决为约束或错误。
pub fn enforce_decision(
    decision: PolicyDecision,
) -> Result<Option<ExecutionConstraints>, crate::error::BrowserComputerError> {
    match decision {
        PolicyDecision::Allow => Ok(None),
        PolicyDecision::AllowWithConstraints { constraints } => Ok(Some(constraints)),
        PolicyDecision::Deny { reason } => {
            Err(crate::error::BrowserComputerError::PolicyDenied(reason))
        }
        PolicyDecision::AskUser { prompt } => Err(
            crate::error::BrowserComputerError::PolicyAskUser(prompt.message),
        ),
    }
}

/// 一次后端选择 / action 执行的审计记录（可持久化、可重放）。
///
/// 字段用 `String` 以便经 [`crate::audit::AuditSink`] 反序列化 replay。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserComputerAudit {
    pub action: String,
    pub backend: Option<String>,
    pub site: Option<String>,
    pub trust: Option<String>,
    pub cross_trust_fallback: bool,
    pub policy: String,
    pub note: String,
}
