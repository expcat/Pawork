//! Protocol-neutral external agent identity and trusted tenant binding (P18-12).
//!
//! [`ExternalAgentIdentity`] carries session / agent / parent-agent attribution
//! dimensions only. These ids **must not** be used as cross-tenant affinity
//! keys: tenant and principal come exclusively from [`TrustedTenantContext`]
//! supplied by the host identity layer, never from client headers or identity
//! fields.

use agent_domain::{AgentId, PrincipalId, SessionId, TenantId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::AdapterError;

/// Fail-closed identity / tenant-binding errors.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum IdentityError {
    /// Session id is required as the attribution anchor.
    #[error("external agent identity is missing session_id")]
    MissingSession,
    /// Agent tree is structurally invalid.
    #[error("invalid agent identity tree: {0}")]
    InvalidAgentTree(&'static str),
    /// Host did not supply a trusted tenant / principal.
    #[error("tenant binding requires a trusted tenant context: {0}")]
    MissingTenantContext(&'static str),
}

impl From<IdentityError> for AdapterError {
    fn from(error: IdentityError) -> Self {
        AdapterError::InvalidFrame(error.to_string())
    }
}

/// Protocol-neutral external agent identity.
///
/// `session_id` is required after validation (cost / audit anchor).
/// `agent_id` / `parent_agent_id` are present only on subagent requests.
/// Values are opaque attribution ids, not routing or affinity keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalAgentIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
}

impl ExternalAgentIdentity {
    /// Structure checks: session required; parent without agent, or agent
    /// self-parent, fail closed.
    pub fn validate(&self) -> Result<(), IdentityError> {
        if self
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            return Err(IdentityError::MissingSession);
        }
        if self.agent_id.is_none() && self.parent_agent_id.is_some() {
            return Err(IdentityError::InvalidAgentTree(
                "parent_agent_id requires agent_id",
            ));
        }
        if let (Some(agent), Some(parent)) = (&self.agent_id, &self.parent_agent_id) {
            if agent == parent {
                return Err(IdentityError::InvalidAgentTree(
                    "agent_id must not equal parent_agent_id",
                ));
            }
        }
        Ok(())
    }

    /// Whether this is a subagent request (agent and parent both present).
    pub fn is_subagent(&self) -> bool {
        self.agent_id.is_some() && self.parent_agent_id.is_some()
    }
}

/// Host-injected tenant / principal. Client identity fields never participate
/// in tenant derivation — the only construction path is explicit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedTenantContext {
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
}

impl TrustedTenantContext {
    /// Explicit constructor; blank tenant / principal fail closed.
    pub fn try_new(tenant_id: TenantId, principal_id: PrincipalId) -> Result<Self, IdentityError> {
        let trusted = Self {
            tenant_id,
            principal_id,
        };
        trusted.validate()?;
        Ok(trusted)
    }

    /// Blank tenant / principal fail closed.
    pub fn validate(&self) -> Result<(), IdentityError> {
        if self.tenant_id.as_str().trim().is_empty() {
            return Err(IdentityError::MissingTenantContext("tenant_id is blank"));
        }
        if self.principal_id.as_str().trim().is_empty() {
            return Err(IdentityError::MissingTenantContext("principal_id is blank"));
        }
        Ok(())
    }
}

/// Validated client identity bound to a trusted tenant.
///
/// `session_id` / `agent_id` / `parent_agent_id` are canonical attribution
/// dimensions. Tenant is copied from [`TrustedTenantContext`] only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantBinding {
    pub identity: ExternalAgentIdentity,
    pub tenant: TrustedTenantContext,
    /// Client session identity (host maps to a Core session via SessionRegistry).
    pub session_id: SessionId,
    /// Subagent identity; `None` on root requests.
    pub agent_id: Option<AgentId>,
    /// Parent agent identity; `None` on root requests.
    pub parent_agent_id: Option<AgentId>,
}

/// Bind a validated identity to a trusted tenant (fail-closed).
///
/// Header / identity field changes never change tenant. Missing trusted
/// context fails; there is no path that guesses tenant from identity ids.
pub fn bind_tenant(
    identity: &ExternalAgentIdentity,
    trusted: &TrustedTenantContext,
) -> Result<TenantBinding, IdentityError> {
    identity.validate()?;
    trusted.validate()?;
    let session_id = SessionId::from(
        identity
            .session_id
            .as_deref()
            .expect("identity validated: session present"),
    );
    let agent_id = identity.agent_id.as_deref().map(AgentId::from);
    let parent_agent_id = identity.parent_agent_id.as_deref().map(AgentId::from);
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

    fn identity(session: &str, agent: Option<&str>, parent: Option<&str>) -> ExternalAgentIdentity {
        ExternalAgentIdentity {
            session_id: Some(session.into()),
            agent_id: agent.map(str::to_string),
            parent_agent_id: parent.map(str::to_string),
        }
    }

    #[test]
    fn missing_session_fails_closed() {
        let empty = ExternalAgentIdentity {
            session_id: None,
            agent_id: Some("agent-1".into()),
            parent_agent_id: Some("agent-0".into()),
        };
        assert_eq!(empty.validate(), Err(IdentityError::MissingSession));
        assert_eq!(
            ExternalAgentIdentity {
                session_id: Some("   ".into()),
                agent_id: None,
                parent_agent_id: None,
            }
            .validate(),
            Err(IdentityError::MissingSession)
        );
    }

    #[test]
    fn forged_agent_tree_fails_closed() {
        assert_eq!(
            identity("sess-1", None, Some("agent-0")).validate(),
            Err(IdentityError::InvalidAgentTree(
                "parent_agent_id requires agent_id"
            ))
        );
        assert_eq!(
            identity("sess-1", Some("agent-1"), Some("agent-1")).validate(),
            Err(IdentityError::InvalidAgentTree(
                "agent_id must not equal parent_agent_id"
            ))
        );
    }

    #[test]
    fn tenant_binding_never_uses_identity_as_affinity_key() {
        let trusted = TrustedTenantContext::try_new(
            TenantId::from("tenant-trusted"),
            PrincipalId::from("user-1"),
        )
        .expect("trusted");
        let first = bind_tenant(&identity("sess-1", Some("a-1"), Some("p-1")), &trusted)
            .expect("bind first");
        let second = bind_tenant(&identity("sess-2", None, None), &trusted).expect("bind second");
        assert_eq!(first.tenant, second.tenant);
        assert_eq!(first.tenant.tenant_id.as_str(), "tenant-trusted");
        assert_eq!(first.session_id.as_str(), "sess-1");
        assert_eq!(second.session_id.as_str(), "sess-2");
        assert_eq!(first.agent_id.as_ref().map(AgentId::as_str), Some("a-1"));
        assert_eq!(
            first.parent_agent_id.as_ref().map(AgentId::as_str),
            Some("p-1")
        );
        assert_eq!(second.agent_id, None);
        assert!(first.identity.is_subagent());
        assert!(!second.identity.is_subagent());
    }

    #[test]
    fn trusted_tenant_fails_closed_when_blank() {
        for (tenant, principal) in [
            (TenantId::from(""), PrincipalId::from("user-1")),
            (TenantId::from("tenant-a"), PrincipalId::from("   ")),
        ] {
            assert!(matches!(
                TrustedTenantContext::try_new(tenant, principal),
                Err(IdentityError::MissingTenantContext(_))
            ));
        }
    }

    #[test]
    fn identity_round_trips_through_json() {
        let value = identity("sess-9", Some("a"), Some("p"));
        let encoded = serde_json::to_string(&value).expect("serialize");
        let decoded: ExternalAgentIdentity = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, value);
    }
}
