//! P18-12 Claude Gateway production host seam.
//!
//! Registers [`ClaudeGatewayAdapterFactory`] on [`ClientAdapterHost`] (alongside
//! ACP), binds header identity to a **trusted** tenant, projects
//! session/agent/parent-agent into usage/audit dimensions, and injects a
//! per-`(provider_id, session_id)` [`ReasoningProtector`] as
//! [`SignedThinkingProtector`].
//!
//! Adapter still does not choose credentials, route, or override Core policy.
//! Full `pawork` stdio CLI entry is deferred to P18-14 host composition.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_domain::{ProtectedBlobRef, ProviderId, SessionId};
use audit_log::{AuditAction, AuditDecision, AuditDimensions, AuditTargetKind};
use client_adapter_api::{
    AdapterError, CapabilitySnapshot, ClientProtocol, TenantBinding, TrustedTenantContext,
};
use client_claude_gateway::{
    bind_tenant, extract_identity, ClaudeGatewayAdapterFactory, ClaudeGatewayError, HeaderPair,
    NegotiatedClaudeAdapter, SignedThinkingProtector, CLAUDE_GATEWAY_PROTOCOL,
};
use protected_blob_store::{BlobScope, ProtectedBlobStore};
use provider_runtime::reasoning::{
    InMemoryReasoningProtector, ProtectedBlobStoreProtector, ReasoningProtectError,
    ReasoningProtector,
};
use tenant_service::IdentityContext;
use usage_ledger::UsageRecord;

use crate::client_adapter::ClientAdapterHost;
use crate::AppService;

/// Register the Claude Gateway factory on an existing [`ClientAdapterHost`].
///
/// The factory is protocol/capability negotiation only: no protector is shared
/// across sessions. Per-session signed-thinking protectors are injected by
/// [`ClaudeGatewayHost::negotiate_for_session`].
pub fn register_claude_gateway(host: &ClientAdapterHost) {
    host.register_factory(Arc::new(ClaudeGatewayAdapterFactory::with_defaults(None)));
}

/// One protector per `(provider_id, session_id)` / run scope. Never reuse one
/// Session's protector for another Session.
pub trait ClaudeProtectorFactory: Send + Sync {
    fn protector_for(
        &self,
        provider_id: &ProviderId,
        session_id: &SessionId,
    ) -> Arc<dyn SignedThinkingProtector>;
}

/// Bridges `provider-runtime::ReasoningProtector` (production:
/// [`ProtectedBlobStoreProtector`]) into the adapter-local
/// [`SignedThinkingProtector`] seam. Errors are static; plaintext is never
/// copied into the error.
pub struct ReasoningProtectorBridge {
    inner: Arc<dyn ReasoningProtector>,
}

impl ReasoningProtectorBridge {
    pub fn new(inner: Arc<dyn ReasoningProtector>) -> Self {
        Self { inner }
    }
}

fn map_protect_error(error: ReasoningProtectError) -> ClaudeGatewayError {
    let reason = if error.is_corrupted() {
        "reasoning continuation corrupted"
    } else {
        "reasoning continuation unavailable"
    };
    ClaudeGatewayError::SignedThinkingProtectorUnavailable(reason)
}

#[async_trait::async_trait]
impl SignedThinkingProtector for ReasoningProtectorBridge {
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ClaudeGatewayError> {
        self.inner.protect(payload).await.map_err(map_protect_error)
    }

    async fn resolve(&self, blob_ref: &ProtectedBlobRef) -> Result<Vec<u8>, ClaudeGatewayError> {
        self.inner
            .resolve(blob_ref)
            .await
            .map(|blob| blob.expose().to_vec())
            .map_err(map_protect_error)
    }
}

/// Production factory: each `(provider, session)` gets a
/// [`ProtectedBlobStoreProtector`] captured at that [`BlobScope`]. The store
/// may be shared; scopes are not.
#[derive(Clone)]
pub struct ProductionClaudeProtectorFactory {
    store: ProtectedBlobStore,
}

impl ProductionClaudeProtectorFactory {
    pub fn new(store: ProtectedBlobStore) -> Self {
        Self { store }
    }
}

impl ClaudeProtectorFactory for ProductionClaudeProtectorFactory {
    fn protector_for(
        &self,
        provider_id: &ProviderId,
        session_id: &SessionId,
    ) -> Arc<dyn SignedThinkingProtector> {
        let scope = BlobScope::new(provider_id.clone(), session_id.clone());
        let protector = ProtectedBlobStoreProtector::new(self.store.clone(), scope);
        Arc::new(ReasoningProtectorBridge::new(Arc::new(protector)))
    }
}

/// Test / composition factory: distinct in-memory protector per session scope.
#[derive(Default)]
pub struct InMemoryClaudeProtectorFactory {
    by_scope: Mutex<HashMap<(String, String), Arc<InMemoryReasoningProtector>>>,
}

impl InMemoryClaudeProtectorFactory {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ClaudeProtectorFactory for InMemoryClaudeProtectorFactory {
    fn protector_for(
        &self,
        provider_id: &ProviderId,
        session_id: &SessionId,
    ) -> Arc<dyn SignedThinkingProtector> {
        let key = (
            provider_id.as_str().to_string(),
            session_id.as_str().to_string(),
        );
        let mut map = self.by_scope.lock().expect("claude protector map poisoned");
        let protector = map
            .entry(key)
            .or_insert_with(|| Arc::new(InMemoryReasoningProtector::new()));
        Arc::new(ReasoningProtectorBridge::new(
            Arc::clone(protector) as Arc<dyn ReasoningProtector>
        ))
    }
}

/// Canonical audit dimensions for a bound external identity.
///
/// Only session / agent / parent-agent ids (plus the trusted tenant already
/// recorded on the event). Parent-agent is a correlation `trace_id`, not an
/// affinity key. No secrets.
pub fn audit_dimensions_for_binding(binding: &TenantBinding) -> AuditDimensions {
    AuditDimensions {
        session_id: Some(binding.session_id.clone()),
        agent_id: binding.agent_id.clone(),
        client_id: binding.identity.session_id.clone(),
        trace_id: binding
            .parent_agent_id
            .as_ref()
            .map(|id| format!("parent-agent:{}", id.as_str())),
        ..AuditDimensions::default()
    }
}

/// Copy identity dimensions onto a usage record. Tenant comes from the trusted
/// host context; session/agent/parent-agent come from validated identity.
/// Does not set token/cost fields and does not choose an account/credential.
pub fn apply_external_identity(record: &mut UsageRecord, binding: &TenantBinding) {
    record.tenant_id = binding.tenant.tenant_id.clone();
    record.principal_id = binding.tenant.principal_id.clone();
    record.session_id = binding.session_id.clone();
    record.agent_id = binding
        .agent_id
        .clone()
        .unwrap_or_else(|| crate::supervisor::canonical_root_agent_id(&binding.session_id));
    record.trace_id = binding
        .parent_agent_id
        .as_ref()
        .map(|id| format!("parent-agent:{}", id.as_str()));
}

/// Host-side Claude Gateway registration + identity/protector injection.
pub struct ClaudeGatewayHost {
    adapter_host: ClientAdapterHost,
    service: Arc<AppService>,
    trusted: TrustedTenantContext,
    protector_factory: Arc<dyn ClaudeProtectorFactory>,
}

impl ClaudeGatewayHost {
    /// Register the Claude factory on `adapter_host` and capture trusted tenant
    /// + scoped protector factory. Tenant is never read from client headers.
    pub fn attach(
        service: Arc<AppService>,
        adapter_host: ClientAdapterHost,
        trusted: TrustedTenantContext,
        protector_factory: Arc<dyn ClaudeProtectorFactory>,
    ) -> Result<Self, ClaudeGatewayError> {
        trusted.validate()?;
        register_claude_gateway(&adapter_host);
        Ok(Self {
            adapter_host,
            service,
            trusted,
            protector_factory,
        })
    }

    pub fn adapter_host(&self) -> &ClientAdapterHost {
        &self.adapter_host
    }

    pub fn trusted_tenant(&self) -> &TrustedTenantContext {
        &self.trusted
    }

    pub fn registered_protocol(&self) -> Option<ClientProtocol> {
        self.adapter_host
            .factory(&ClientProtocol::new(CLAUDE_GATEWAY_PROTOCOL))
            .map(|factory| factory.protocol().clone())
    }

    /// Negotiate an adapter with a protector scoped to this `(provider, session)`.
    pub fn negotiate_for_session(
        &self,
        snapshot: CapabilitySnapshot,
        provider_id: &ProviderId,
        session_id: &SessionId,
    ) -> Result<NegotiatedClaudeAdapter, AdapterError> {
        let protector = self
            .protector_factory
            .protector_for(provider_id, session_id);
        ClaudeGatewayAdapterFactory::with_defaults(Some(protector)).create_concrete(snapshot)
    }

    /// Extract identity from Claude headers and bind to the host trusted tenant.
    pub fn bind_headers<'a>(
        &self,
        headers: impl IntoIterator<Item = HeaderPair<'a>>,
    ) -> Result<TenantBinding, ClaudeGatewayError> {
        let identity = extract_identity(headers)?;
        bind_tenant(&identity, &self.trusted)
    }

    /// Project bound identity into canonical audit (ids only; no secrets).
    pub fn record_audit_identity(&self, binding: &TenantBinding) {
        let identity = IdentityContext::new(
            binding.tenant.tenant_id.clone(),
            binding.tenant.principal_id.clone(),
        );
        self.service.tenant_policy().record_control_event(
            &identity,
            AuditAction::AgentLifecycle,
            AuditTargetKind::Agent,
            AuditDecision::Observe,
            "claude_gateway_identity",
            audit_dimensions_for_binding(binding),
            0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{PrincipalId, TenantId};
    use client_adapter_api::{
        ClientCapability, ClientProtocol, InMemorySessionRegistryStore, SessionRegistry,
        CLIENT_ADAPTER_SCHEMA_VERSION,
    };
    use client_claude_gateway::{
        decode_frame, map_sse_event, ClaudeGatewayError, SseFrame, HEADER_AGENT_ID,
        HEADER_PARENT_AGENT_ID, HEADER_SESSION_ID,
    };
    use subscription_hub::EventHub;
    use tenant_service::IdentityContext;
    use usage_ledger::{InMemoryUsageLedger, UsageLedger, UsageQuery};

    fn snapshot(capabilities: &[&str]) -> CapabilitySnapshot {
        CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(CLAUDE_GATEWAY_PROTOCOL),
            protocol_version: "1".into(),
            client_version: "test".into(),
            revision: 1,
            capabilities: capabilities
                .iter()
                .map(|name| ClientCapability::new(*name))
                .collect(),
        }
    }

    async fn host(tenant: &str, principal: &str) -> ClaudeGatewayHost {
        let service = Arc::new(AppService::new("claude-gateway-host"));
        let hub = Arc::new(EventHub::new());
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let adapter_host = ClientAdapterHost::new(service.clone(), hub, registry);
        let trusted =
            TrustedTenantContext::try_new(TenantId::from(tenant), PrincipalId::from(principal))
                .expect("trusted");
        ClaudeGatewayHost::attach(
            service,
            adapter_host,
            trusted,
            Arc::new(InMemoryClaudeProtectorFactory::new()),
        )
        .expect("attach")
    }

    #[tokio::test]
    async fn registers_claude_factory_alongside_adapter_host() {
        let host = host("local/default", "local/user").await;
        assert_eq!(
            host.registered_protocol().map(|protocol| protocol.0),
            Some(CLAUDE_GATEWAY_PROTOCOL.into())
        );
    }

    #[tokio::test]
    async fn headers_bind_to_trusted_tenant_and_reach_usage_audit_stub() {
        let host = host("local/default", "local/user").await;
        let binding = host
            .bind_headers([
                HeaderPair::new(HEADER_SESSION_ID, "sess-1"),
                HeaderPair::new(HEADER_AGENT_ID, "agent-2"),
                HeaderPair::new(HEADER_PARENT_AGENT_ID, "agent-1"),
                HeaderPair::new("x-claude-code-tenant-id", "tenant-evil"),
            ])
            .expect("bind");
        assert_eq!(binding.tenant.tenant_id.as_str(), "local/default");
        assert_eq!(binding.session_id.as_str(), "sess-1");
        assert_eq!(
            binding.agent_id.as_ref().map(|id| id.as_str()),
            Some("agent-2")
        );
        assert_eq!(
            binding.parent_agent_id.as_ref().map(|id| id.as_str()),
            Some("agent-1")
        );

        host.record_audit_identity(&binding);

        let mut usage = UsageRecord::default();
        apply_external_identity(&mut usage, &binding);
        usage.provider_id = ProviderId::from("anthropic");
        usage.model_id = agent_domain::ModelId::from("claude-test");
        usage.account_id = "unattributed".into();
        usage.currency = "USD".into();
        usage.input_tokens = 1;
        usage.occurred_at_ms = 1;
        let ledger = InMemoryUsageLedger::default();
        ledger.record(usage).await.expect("record usage");

        let recorded = ledger
            .query(&UsageQuery::by_session(binding.session_id.clone()))
            .await
            .expect("query");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].tenant_id.as_str(), "local/default");
        assert_eq!(recorded[0].session_id.as_str(), "sess-1");
        assert_eq!(recorded[0].agent_id.as_str(), "agent-2");
        assert_eq!(
            recorded[0].trace_id.as_deref(),
            Some("parent-agent:agent-1")
        );

        let other = host
            .bind_headers([HeaderPair::new(HEADER_SESSION_ID, "sess-2")])
            .expect("second session");
        assert_eq!(other.tenant, binding.tenant);
        assert_eq!(other.session_id.as_str(), "sess-2");
        assert_eq!(other.agent_id, None);

        let events = host
            .service
            .canonical_audit_events(&IdentityContext::local())
            .expect("audit query");
        assert!(events.iter().any(|event| {
            event.reason_code == "claude_gateway_identity"
                && event.session_id.as_ref().map(|id| id.as_str()) == Some("sess-1")
                && event.agent_id.as_ref().map(|id| id.as_str()) == Some("agent-2")
                && event.trace_id.as_deref() == Some("parent-agent:agent-1")
        }));
    }

    #[tokio::test]
    async fn forged_or_missing_headers_fail_closed() {
        let host = host("tenant-trusted", "user-1").await;
        assert!(matches!(
            host.bind_headers([]),
            Err(ClaudeGatewayError::MissingIdentityHeader(HEADER_SESSION_ID))
        ));
        assert!(matches!(
            host.bind_headers([HeaderPair::new(HEADER_SESSION_ID, "")]),
            Err(ClaudeGatewayError::MalformedIdentityHeader(
                HEADER_SESSION_ID
            ))
        ));
        assert!(matches!(
            host.bind_headers([
                HeaderPair::new(HEADER_SESSION_ID, "sess-1"),
                HeaderPair::new(HEADER_PARENT_AGENT_ID, "agent-0"),
            ]),
            Err(ClaudeGatewayError::InvalidAgentTree(_))
        ));
        let ok = host
            .bind_headers([
                HeaderPair::new(HEADER_SESSION_ID, "sess-ok"),
                HeaderPair::new("x-pawork-tenant-id", "tenant-evil"),
            ])
            .expect("forged tenant header ignored");
        assert_eq!(ok.tenant.tenant_id.as_str(), "tenant-trusted");
        assert_eq!(ok.session_id.as_str(), "sess-ok");
    }

    #[tokio::test]
    async fn signed_thinking_without_capability_fails_closed() {
        let host = host("local/default", "local/user").await;
        let negotiated = host
            .negotiate_for_session(
                snapshot(&["events"]),
                &ProviderId::from("anthropic"),
                &SessionId::from("sess-1"),
            )
            .expect("negotiate without reasoning");
        assert!(!negotiated.adapter.reasoning_supported());
        let mut state = negotiated.adapter.stream_state();
        let event = decode_frame(&SseFrame {
            event: Some("content_block_stop".into()),
            data: r#"{"type":"content_block_stop","index":0,"content_block":{"type":"thinking","thinking":"hmm","signature":"SIG-SECRET"}}"#.into(),
        })
        .expect("decode");
        let error = map_sse_event(&mut state, &event).expect_err("must fail closed");
        assert!(matches!(
            error,
            ClaudeGatewayError::SignedThinkingNotNegotiated(capability)
                if capability == "reasoning.signed_continuity"
        ));
        assert!(!format!("{error}").contains("SIG-SECRET"));
    }

    #[tokio::test]
    async fn protectors_are_not_shared_across_sessions() {
        let factory = InMemoryClaudeProtectorFactory::new();
        let a = factory.protector_for(&ProviderId::from("anthropic"), &SessionId::from("sess-a"));
        let b = factory.protector_for(&ProviderId::from("anthropic"), &SessionId::from("sess-b"));
        let blob = a.protect(b"session-a-secret").await.expect("protect a");
        let resolved = a.resolve(&blob).await.expect("resolve a");
        assert_eq!(resolved, b"session-a-secret");
        let error = b.resolve(&blob).await.expect_err("cross-session must fail");
        assert!(matches!(
            error,
            ClaudeGatewayError::SignedThinkingProtectorUnavailable(_)
        ));
        assert!(!format!("{error:?}").contains("session-a-secret"));
    }
}
