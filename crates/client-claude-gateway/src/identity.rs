//! Claude Code 身份头 → [`ExternalAgentIdentity`] 与 tenant binding（P18-12 §1）。
//!
//! 三个身份头：
//!
//! - `X-Claude-Code-Session-Id`（必需，成本归属锚点）
//! - `X-Claude-Code-Agent-Id`（subagent；root 请求缺省）
//! - `X-Claude-Code-Parent-Agent-Id`（subagent 的父 agent；root 请求缺省）
//!
//! 关键不变量：
//!
//! 1. header 只映射为 session / agent / parent-agent 身份，**绝不**作为跨 tenant
//!    affinity key；tenant / principal 只能来自受信身份上下文
//!    （[`TrustedTenantContext`]），bind 时 tenant 无从 header 推导的路径。
//! 2. 缺失 / 重复 / 畸形（空白、控制字符、超长）/ 伪造（parent 无 agent、
//!    agent 自引用）一律 fail-closed，不静默落入默认身份。

use agent_domain::{AgentId, PrincipalId, SessionId, TenantId};
use serde::{Deserialize, Serialize};

use crate::error::ClaudeGatewayError;

/// Claude Code session 身份头。
pub const HEADER_SESSION_ID: &str = "x-claude-code-session-id";

/// Claude Code agent（subagent）身份头。
pub const HEADER_AGENT_ID: &str = "x-claude-code-agent-id";

/// Claude Code parent agent 身份头。
pub const HEADER_PARENT_AGENT_ID: &str = "x-claude-code-parent-agent-id";

/// 身份头值长度上限（超出视为伪造，fail-closed）。
pub const MAX_ID_LENGTH: usize = 256;

fn validate_id_value(header: &'static str, value: &str) -> Result<String, ClaudeGatewayError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ClaudeGatewayError::MalformedIdentityHeader(header));
    }
    if trimmed.len() > MAX_ID_LENGTH {
        return Err(ClaudeGatewayError::MalformedIdentityHeader(header));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ClaudeGatewayError::MalformedIdentityHeader(header));
    }
    Ok(trimmed.to_string())
}

/// Claude Code 会话标识（客户端命名空间，非 Pawork core session）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClaudeSessionId(String);

impl ClaudeSessionId {
    /// 构造并校验（空白 / 控制字符 / 超长 fail-closed）。
    pub fn new(value: impl Into<String>) -> Result<Self, ClaudeGatewayError> {
        Ok(Self(validate_id_value(HEADER_SESSION_ID, &value.into())?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Claude Code agent 标识（subagent；root 请求不携带）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClaudeAgentId(String);

impl ClaudeAgentId {
    /// 构造并校验（空白 / 控制字符 / 超长 fail-closed）。
    pub fn new(value: impl Into<String>) -> Result<Self, ClaudeGatewayError> {
        Ok(Self(validate_id_value(HEADER_AGENT_ID, &value.into())?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 外部 agent 身份（P18 领域模型 `ExternalAgentIdentity`）。
///
/// `session_id` 必需（归属锚点）；`agent_id` / `parent_agent_id` 仅 subagent
/// 请求携带。类型在 crate 内定义（`client-adapter-api` 尚未承载共享形态），
/// 主代理接线共享 API 时按本形状上移。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalAgentIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<ClaudeSessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<ClaudeAgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<ClaudeAgentId>,
}

impl ExternalAgentIdentity {
    /// 结构校验：session 必需；parent 无 agent、agent 自引用 fail-closed。
    pub fn validate(&self) -> Result<(), ClaudeGatewayError> {
        if self.session_id.is_none() {
            return Err(ClaudeGatewayError::MissingIdentityHeader(HEADER_SESSION_ID));
        }
        if self.agent_id.is_none() && self.parent_agent_id.is_some() {
            return Err(ClaudeGatewayError::InvalidAgentTree(
                "parent_agent_id requires agent_id",
            ));
        }
        if let (Some(agent), Some(parent)) = (&self.agent_id, &self.parent_agent_id) {
            if agent == parent {
                return Err(ClaudeGatewayError::InvalidAgentTree(
                    "agent_id must not equal parent_agent_id",
                ));
            }
        }
        Ok(())
    }

    /// 是否 subagent 请求（agent 与 parent 同时存在）。
    pub fn is_subagent(&self) -> bool {
        self.agent_id.is_some() && self.parent_agent_id.is_some()
    }
}

/// 一个 header 名 / 值对（宿主从真实 HTTP header 提供；名匹配不区分大小写）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderPair<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

impl<'a> HeaderPair<'a> {
    pub fn new(name: &'a str, value: &'a str) -> Self {
        Self { name, value }
    }
}

fn set_once<T>(
    slot: &mut Option<T>,
    header: &'static str,
    value: T,
) -> Result<(), ClaudeGatewayError> {
    if slot.replace(value).is_some() {
        return Err(ClaudeGatewayError::DuplicateIdentityHeader(header));
    }
    Ok(())
}

/// 从 header 集合提取 [`ExternalAgentIdentity`]。
///
/// 只读取三个 `X-Claude-Code-*` 头（名称不区分大小写），其余 header 忽略；
/// 空白值 / 控制字符 / 超长 / 重复 / 树结构非法一律 fail-closed。
pub fn extract_identity<'a>(
    headers: impl IntoIterator<Item = HeaderPair<'a>>,
) -> Result<ExternalAgentIdentity, ClaudeGatewayError> {
    let mut session_id = None;
    let mut agent_id = None;
    let mut parent_agent_id = None;
    for pair in headers {
        let name = pair.name.trim().to_ascii_lowercase();
        match name.as_str() {
            HEADER_SESSION_ID => {
                set_once(
                    &mut session_id,
                    HEADER_SESSION_ID,
                    ClaudeSessionId::new(pair.value)?,
                )?;
            }
            HEADER_AGENT_ID => {
                set_once(
                    &mut agent_id,
                    HEADER_AGENT_ID,
                    ClaudeAgentId::new(pair.value)?,
                )?;
            }
            HEADER_PARENT_AGENT_ID => {
                set_once(
                    &mut parent_agent_id,
                    HEADER_PARENT_AGENT_ID,
                    ClaudeAgentId::new(pair.value)?,
                )?;
            }
            _ => {}
        }
    }
    let identity = ExternalAgentIdentity {
        session_id,
        agent_id,
        parent_agent_id,
    };
    identity.validate()?;
    Ok(identity)
}

/// 受信身份上下文：由宿主从认证层解析注入。
///
/// 客户端身份头永不参与 tenant 推导——本结构只有显式构造路径，且
/// [`bind_tenant`] 只消费本结构与已验证身份。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedTenantContext {
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
}

impl TrustedTenantContext {
    /// 显式构造并校验（空白 tenant / principal fail-closed）。
    pub fn try_new(
        tenant_id: TenantId,
        principal_id: PrincipalId,
    ) -> Result<Self, ClaudeGatewayError> {
        if tenant_id.as_str().trim().is_empty() {
            return Err(ClaudeGatewayError::MissingTenantContext(
                "tenant_id is blank",
            ));
        }
        if principal_id.as_str().trim().is_empty() {
            return Err(ClaudeGatewayError::MissingTenantContext(
                "principal_id is blank",
            ));
        }
        Ok(Self {
            tenant_id,
            principal_id,
        })
    }
}

/// 归属绑定：已验证客户端身份 × 受信租户 → canonical 归属键。
///
/// 供宿主进入 usage / audit 归属：`session_id` 是客户端会话身份（宿主经
/// `SessionRegistry` 解析 core session），`agent_id` / `parent_agent_id` 仅
/// subagent 存在；root 请求的 canonical root agent 由宿主按 run 上下文解析。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantBinding {
    pub identity: ExternalAgentIdentity,
    pub tenant: TrustedTenantContext,
    /// 客户端会话身份（与 `client_session_id` 同源；宿主映射到 core session）。
    pub session_id: SessionId,
    /// subagent 身份；root 请求为 `None`。
    pub agent_id: Option<AgentId>,
    /// 父 agent 身份；root 请求为 `None`。
    pub parent_agent_id: Option<AgentId>,
}

/// 把已验证身份绑定到受信租户（fail-closed）。
///
/// tenant / principal 只来自 [`TrustedTenantContext`]；header 变化不会改变
/// tenant，缺失受信上下文直接失败，禁止从 header 猜测 tenant。
pub fn bind_tenant(
    identity: &ExternalAgentIdentity,
    trusted: &TrustedTenantContext,
) -> Result<TenantBinding, ClaudeGatewayError> {
    identity.validate()?;
    let session_id = SessionId::from(
        identity
            .session_id
            .as_ref()
            .expect("identity validated: session present")
            .as_str(),
    );
    let agent_id = identity
        .agent_id
        .as_ref()
        .map(|agent| AgentId::from(agent.as_str()));
    let parent_agent_id = identity
        .parent_agent_id
        .as_ref()
        .map(|parent| AgentId::from(parent.as_str()));
    Ok(TenantBinding {
        identity: identity.clone(),
        tenant: trusted.clone(),
        session_id,
        agent_id,
        parent_agent_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers<'a>(
        session: Option<&'a str>,
        agent: Option<&'a str>,
        parent: Option<&'a str>,
    ) -> Vec<HeaderPair<'a>> {
        let mut pairs = Vec::new();
        if let Some(value) = session {
            pairs.push(HeaderPair::new(HEADER_SESSION_ID, value));
        }
        if let Some(value) = agent {
            pairs.push(HeaderPair::new(HEADER_AGENT_ID, value));
        }
        if let Some(value) = parent {
            pairs.push(HeaderPair::new(HEADER_PARENT_AGENT_ID, value));
        }
        pairs
    }

    #[test]
    fn three_headers_map_to_identity() {
        let identity =
            extract_identity(headers(Some("sess-abc"), Some("agent-1"), Some("agent-0")))
                .expect("valid headers");
        assert_eq!(
            identity.session_id.as_ref().map(ClaudeSessionId::as_str),
            Some("sess-abc")
        );
        assert_eq!(
            identity.agent_id.as_ref().map(ClaudeAgentId::as_str),
            Some("agent-1")
        );
        assert_eq!(
            identity.parent_agent_id.as_ref().map(ClaudeAgentId::as_str),
            Some("agent-0")
        );
        assert!(identity.is_subagent());
        identity.validate().expect("valid");
    }

    #[test]
    fn root_request_has_session_only() {
        let identity = extract_identity(headers(Some("sess-root"), None, None)).expect("root");
        assert!(!identity.is_subagent());
    }

    #[test]
    fn header_names_are_case_insensitive_and_values_trimmed() {
        let identity = extract_identity([
            HeaderPair::new("X-Claude-Code-Session-Id", "  sess-1  "),
            HeaderPair::new("X-CLAUDE-CODE-AGENT-ID", " agent-1 "),
        ])
        .expect("case-insensitive");
        assert_eq!(
            identity.session_id.as_ref().map(ClaudeSessionId::as_str),
            Some("sess-1")
        );
        assert_eq!(
            identity.agent_id.as_ref().map(ClaudeAgentId::as_str),
            Some("agent-1")
        );
    }

    #[test]
    fn unknown_headers_are_ignored() {
        let identity = extract_identity([
            HeaderPair::new("x-claude-code-session-id", "sess-1"),
            HeaderPair::new("authorization", "Bearer does-not-belong-here"),
            HeaderPair::new("x-forwarded-for", "10.0.0.1"),
        ])
        .expect("unknown headers ignored");
        assert_eq!(
            identity.session_id.as_ref().map(ClaudeSessionId::as_str),
            Some("sess-1")
        );
    }

    #[test]
    fn missing_session_header_fails_closed() {
        assert_eq!(
            extract_identity(headers(None, Some("agent-1"), Some("agent-0"))),
            Err(ClaudeGatewayError::MissingIdentityHeader(HEADER_SESSION_ID))
        );
        assert_eq!(
            extract_identity([]),
            Err(ClaudeGatewayError::MissingIdentityHeader(HEADER_SESSION_ID))
        );
    }

    #[test]
    fn duplicate_header_fails_closed() {
        assert_eq!(
            extract_identity([
                HeaderPair::new("x-claude-code-session-id", "sess-1"),
                HeaderPair::new("X-Claude-Code-Session-Id", "sess-2"),
            ]),
            Err(ClaudeGatewayError::DuplicateIdentityHeader(
                HEADER_SESSION_ID
            ))
        );
    }

    #[test]
    fn forged_header_values_fail_closed() {
        for value in ["", "   ", "\u{7f}ctl", &"x".repeat(MAX_ID_LENGTH + 1)] {
            assert_eq!(
                extract_identity(headers(Some(value), None, None)),
                Err(ClaudeGatewayError::MalformedIdentityHeader(
                    HEADER_SESSION_ID
                )),
                "value `{value:?}` must be rejected"
            );
        }
    }

    #[test]
    fn forged_agent_tree_fails_closed() {
        // parent 无 agent。
        assert_eq!(
            extract_identity(headers(Some("sess-1"), None, Some("agent-0"))),
            Err(ClaudeGatewayError::InvalidAgentTree(
                "parent_agent_id requires agent_id"
            ))
        );
        // agent 自引用。
        assert_eq!(
            extract_identity(headers(Some("sess-1"), Some("agent-1"), Some("agent-1"))),
            Err(ClaudeGatewayError::InvalidAgentTree(
                "agent_id must not equal parent_agent_id"
            ))
        );
    }

    #[test]
    fn identity_round_trips_through_json() {
        let identity =
            extract_identity(headers(Some("sess-9"), Some("a"), Some("p"))).expect("valid");
        let encoded = serde_json::to_string(&identity).expect("serialize");
        let decoded: ExternalAgentIdentity = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, identity);
    }

    #[test]
    fn tenant_binding_never_derives_tenant_from_headers() {
        let trusted =
            TrustedTenantContext::try_new(TenantId::from("tenant-a"), PrincipalId::from("user-1"))
                .expect("trusted");
        let first =
            extract_identity(headers(Some("sess-1"), Some("a-1"), Some("p-1"))).expect("first");
        let second = extract_identity(headers(Some("sess-2"), None, None)).expect("second");
        // 同一受信租户 × 不同 header 集：tenant 不变（header 不是 affinity key）。
        let binding_a = bind_tenant(&first, &trusted).expect("bind a");
        let binding_b = bind_tenant(&second, &trusted).expect("bind b");
        assert_eq!(binding_a.tenant, binding_b.tenant);
        assert_eq!(binding_a.session_id.as_str(), "sess-1");
        assert_eq!(binding_b.session_id.as_str(), "sess-2");
        // subagent 键只随 header 变化，与 tenant 正交。
        assert_eq!(
            binding_a.agent_id.as_ref().map(AgentId::as_str),
            Some("a-1")
        );
        assert_eq!(
            binding_a.parent_agent_id.as_ref().map(AgentId::as_str),
            Some("p-1")
        );
        assert_eq!(binding_b.agent_id, None);
    }

    #[test]
    fn tenant_binding_fails_closed_without_trusted_context() {
        let identity = extract_identity(headers(Some("sess-1"), None, None)).expect("identity");
        for (tenant, principal) in [
            (TenantId::from(""), PrincipalId::from("user-1")),
            (TenantId::from("tenant-a"), PrincipalId::from("   ")),
            (TenantId::from(" \t "), PrincipalId::from("")),
        ] {
            assert!(matches!(
                TrustedTenantContext::try_new(tenant, principal),
                Err(ClaudeGatewayError::MissingTenantContext(_))
            ));
        }
        // 未受信身份无法进入归属路径：bind 前必须通过显式受信上下文。
        let trusted =
            TrustedTenantContext::try_new(TenantId::from("tenant-a"), PrincipalId::from("user-1"))
                .expect("trusted");
        let binding = bind_tenant(&identity, &trusted).expect("bind");
        assert_eq!(binding.tenant.tenant_id.as_str(), "tenant-a");
    }
}
