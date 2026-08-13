//! Library-level quota adapter factory / scheduler target registry (P18-14).
//!
//! Registers remote quota adapter factories (the six Provider adapters live in
//! [`crate::providers`]) and reconciles [`crate::refresh::RefreshScheduler`]
//! targets when the account/binding set changes. Production composition root
//! wiring (`app-service::QuotaRuntime`, `apps/pawork`) is orchestrator-owned
//! after P18-13 and is **not** done here.

use std::collections::HashMap;
use std::sync::Arc;

use agent_domain::ProviderId;
use provider_api::ResolvedCredential;
use provider_runtime::http::HttpClient;
use thiserror::Error;

use crate::providers::{anthropic, moonshot, openai, qwen, xai, zhipu};
use crate::refresh::{
    RefreshPolicy, RefreshScheduler, RefreshTarget, RefreshTargetId, TargetReconcileReport,
};
use crate::service::{QuotaService, ScopeMatch};
use crate::{QuotaAdapter, QuotaScope, QuotaUnit, QuotaWindow};

/// Constructs a [`QuotaAdapter`] for one Provider id.
///
/// Core looks factories up by [`ProviderId`]; it never matches on Provider
/// name strings.
pub trait QuotaAdapterFactory: Send + Sync {
    /// Provider this factory serves.
    fn provider_id(&self) -> ProviderId;

    /// Build a fresh adapter instance (no secrets).
    fn build(&self) -> Arc<dyn QuotaAdapter>;
}

/// Account/binding identity that should have a scheduler target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QuotaTargetIdentity {
    pub scope: QuotaScope,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
}

impl QuotaTargetIdentity {
    /// Construct an identity.
    pub fn new(scope: QuotaScope, window: QuotaWindow, unit: QuotaUnit) -> Self {
        Self {
            scope,
            window,
            unit,
        }
    }

    /// Convert to a scheduler target (credential optional; never logged).
    pub fn to_refresh_target(
        &self,
        policy: RefreshPolicy,
        credential: Option<ResolvedCredential>,
    ) -> RefreshTarget {
        RefreshTarget {
            scope: self.scope.clone(),
            window: self.window,
            unit: self.unit.clone(),
            policy,
            credential,
        }
    }

    /// Scheduler identity.
    pub fn refresh_id(&self) -> RefreshTargetId {
        RefreshTargetId {
            scope: self.scope.clone(),
            window: self.window,
            unit: self.unit.clone(),
        }
    }
}

/// Registry error (duplicate factory). Carries no secret.
#[derive(Debug, Error)]
pub enum TargetRegistryError {
    /// A factory for this provider is already registered.
    #[error("quota adapter factory already registered for {0}")]
    Duplicate(ProviderId),
}

/// Non-secret config required by the six remote adapter factories.
#[derive(Clone, Debug)]
pub struct RemoteAdapterConfig {
    pub qwen: qwen::QwenConfig,
    pub xai: xai::XaiConfig,
    pub zhipu: zhipu::ZhipuScrapeConfig,
}

impl Default for RemoteAdapterConfig {
    fn default() -> Self {
        Self {
            qwen: qwen::QwenConfig {
                region: "cn-hangzhou".into(),
            },
            xai: xai::XaiConfig {
                team_id: String::new(),
            },
            zhipu: zhipu::ZhipuScrapeConfig::default(),
        }
    }
}

struct FnFactory {
    provider_id: ProviderId,
    build: Arc<dyn Fn() -> Arc<dyn QuotaAdapter> + Send + Sync>,
}

impl QuotaAdapterFactory for FnFactory {
    fn provider_id(&self) -> ProviderId {
        self.provider_id.clone()
    }

    fn build(&self) -> Arc<dyn QuotaAdapter> {
        (self.build)()
    }
}

/// Registry of remote quota adapter factories + target reconcile helper.
#[derive(Default)]
pub struct QuotaTargetRegistry {
    factories: HashMap<ProviderId, Arc<dyn QuotaAdapterFactory>>,
}

impl QuotaTargetRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a factory. Duplicate provider ids fail closed; the registry
    /// is unchanged.
    pub fn register_factory(
        &mut self,
        factory: Arc<dyn QuotaAdapterFactory>,
    ) -> Result<(), TargetRegistryError> {
        let id = factory.provider_id();
        if self.factories.contains_key(&id) {
            return Err(TargetRegistryError::Duplicate(id));
        }
        self.factories.insert(id, factory);
        Ok(())
    }

    /// Remove a factory (idempotent).
    pub fn unregister_factory(&mut self, provider_id: &ProviderId) -> bool {
        self.factories.remove(provider_id).is_some()
    }

    /// Lookup by [`ProviderId`] (no Provider-name match in Core).
    pub fn factory_for(&self, provider_id: &ProviderId) -> Option<Arc<dyn QuotaAdapterFactory>> {
        self.factories.get(provider_id).cloned()
    }

    /// Number of registered factories.
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    /// Whether no factory is registered.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }

    /// Registered provider ids (unordered).
    pub fn provider_ids(&self) -> Vec<ProviderId> {
        self.factories.keys().cloned().collect()
    }

    /// Register the six remote quota adapter factories.
    ///
    /// Provider-specific construction lives in [`crate::providers`]; this method
    /// only indexes the resulting factories by [`ProviderId`].
    pub fn register_remote(
        &mut self,
        http: Arc<HttpClient>,
        config: RemoteAdapterConfig,
    ) -> Result<(), TargetRegistryError> {
        for factory in remote_factories(http, config) {
            self.register_factory(factory)?;
        }
        Ok(())
    }

    /// Install each factory's adapter onto [`QuotaService`] keyed by provider.
    pub fn install_adapters(&self, service: &QuotaService) {
        for factory in self.factories.values() {
            service.register(
                ScopeMatch::for_provider(factory.provider_id()),
                factory.build(),
            );
        }
    }

    /// Reconcile scheduler targets with the current account/binding identity
    /// set. Dangling targets (no longer in `desired`) are removed.
    pub fn reconcile(
        &self,
        scheduler: &RefreshScheduler,
        desired: &[QuotaTargetIdentity],
        policy: RefreshPolicy,
    ) -> TargetReconcileReport {
        self.reconcile_with_credentials(scheduler, desired, policy, |_| None)
    }

    /// Like [`Self::reconcile`], injecting per-scope credentials (never stored
    /// in this registry).
    pub fn reconcile_with_credentials<F>(
        &self,
        scheduler: &RefreshScheduler,
        desired: &[QuotaTargetIdentity],
        policy: RefreshPolicy,
        mut credential: F,
    ) -> TargetReconcileReport
    where
        F: FnMut(&QuotaScope) -> Option<ResolvedCredential>,
    {
        let targets = desired
            .iter()
            .map(|identity| {
                let cred = credential(&identity.scope);
                identity.to_refresh_target(policy.clone(), cred)
            })
            .collect();
        scheduler.reconcile(targets)
    }
}

/// Build the six remote adapter factories. Provider ids are taken from the
/// providers module, not matched in Agent Engine / Core.
pub fn remote_factories(
    http: Arc<HttpClient>,
    config: RemoteAdapterConfig,
) -> Vec<Arc<dyn QuotaAdapterFactory>> {
    fn wrap(
        provider: &'static str,
        build: impl Fn() -> Arc<dyn QuotaAdapter> + Send + Sync + 'static,
    ) -> Arc<dyn QuotaAdapterFactory> {
        Arc::new(FnFactory {
            provider_id: ProviderId::new(provider),
            build: Arc::new(build),
        })
    }

    let openai_http = http.clone();
    let anthropic_http = http.clone();
    let moonshot_http = http.clone();
    let qwen_http = http.clone();
    let xai_http = http.clone();
    let zhipu_http = http;
    let qwen_config = config.qwen;
    let xai_config = config.xai;
    let zhipu_config = config.zhipu;

    vec![
        wrap("openai", move || {
            Arc::from(openai::adapter(openai_http.clone()))
        }),
        wrap("anthropic", move || {
            Arc::from(anthropic::adapter(anthropic_http.clone()))
        }),
        wrap("moonshot", move || {
            Arc::from(moonshot::adapter(moonshot_http.clone()))
        }),
        wrap("qwen", {
            let config = qwen_config;
            move || Arc::from(qwen::adapter(qwen_http.clone(), config.clone()))
        }),
        wrap("xai", {
            let config = xai_config;
            move || Arc::from(xai::adapter(xai_http.clone(), config.clone()))
        }),
        wrap("zhipu", {
            let config = zhipu_config;
            move || Arc::from(zhipu::adapter(zhipu_http.clone(), config.clone()))
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use agent_domain::{CancellationToken, TenantId};
    use async_trait::async_trait;
    use provider_runtime::http::{HttpClient, HttpClientConfig};

    use crate::refresh::{NopAlertSink, NopAuditSink, RefreshScheduler};
    use crate::service::{MutableQuotaClock, QuotaService};
    use crate::{AccountId, QuotaError, QuotaRequest, QuotaSnapshot};

    struct StubAdapter;

    #[async_trait]
    impl QuotaAdapter for StubAdapter {
        fn kind(&self) -> crate::AdapterKind {
            crate::AdapterKind::ApiKeyApi
        }
        fn supports(&self, _: &QuotaRequest) -> bool {
            false
        }
        async fn fetch(
            &self,
            _: &QuotaRequest,
            _: Option<&ResolvedCredential>,
            _: &CancellationToken,
        ) -> Result<QuotaSnapshot, QuotaError> {
            Err(QuotaError::Unsupported {
                detail: "stub".into(),
            })
        }
    }

    struct StubFactory {
        id: ProviderId,
    }

    impl QuotaAdapterFactory for StubFactory {
        fn provider_id(&self) -> ProviderId {
            self.id.clone()
        }
        fn build(&self) -> Arc<dyn QuotaAdapter> {
            Arc::new(StubAdapter)
        }
    }

    fn identity(account: &str, provider: &str) -> QuotaTargetIdentity {
        QuotaTargetIdentity::new(
            QuotaScope::new(
                TenantId::new("t"),
                AccountId::new(account),
                ProviderId::new(provider),
                None,
            ),
            QuotaWindow::Monthly,
            QuotaUnit::Token,
        )
    }

    fn scheduler() -> Arc<RefreshScheduler> {
        let clock = Arc::new(MutableQuotaClock::at(1_000));
        let svc = Arc::new(QuotaService::new(clock.clone()));
        Arc::new(RefreshScheduler::with_seed(
            svc,
            clock,
            Arc::new(NopAlertSink),
            Arc::new(NopAuditSink),
            1,
        ))
    }

    #[test]
    fn duplicate_factory_is_rejected() {
        let mut registry = QuotaTargetRegistry::new();
        registry
            .register_factory(Arc::new(StubFactory {
                id: ProviderId::new("openai"),
            }))
            .unwrap();
        let err = registry
            .register_factory(Arc::new(StubFactory {
                id: ProviderId::new("openai"),
            }))
            .unwrap_err();
        assert!(matches!(err, TargetRegistryError::Duplicate(id) if id.as_str() == "openai"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn lookup_is_by_provider_id_not_name_match() {
        let mut registry = QuotaTargetRegistry::new();
        registry
            .register_factory(Arc::new(StubFactory {
                id: ProviderId::new("openai"),
            }))
            .unwrap();
        assert!(registry.factory_for(&ProviderId::new("openai")).is_some());
        assert!(registry.factory_for(&ProviderId::new("OPENAI")).is_none());
        assert!(registry
            .factory_for(&ProviderId::new("anthropic"))
            .is_none());
    }

    #[test]
    fn reconcile_drops_dangling_targets() {
        let registry = QuotaTargetRegistry::new();
        let sched = scheduler();
        let policy = RefreshPolicy {
            period: Duration::from_secs(30),
            ..RefreshPolicy::default()
        };
        let a = identity("acct-a", "openai");
        let b = identity("acct-b", "anthropic");
        let first = registry.reconcile(&sched, &[a.clone(), b.clone()], policy.clone());
        assert_eq!(first.added, 2);
        assert_eq!(sched.registered_ids().len(), 2);

        let second = registry.reconcile(&sched, std::slice::from_ref(&a), policy.clone());
        assert_eq!(second.removed, 1);
        assert_eq!(second.updated, 1);
        assert_eq!(second.added, 0);
        let ids = sched.registered_ids();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], a.refresh_id());

        let third = registry.reconcile(&sched, &[a], policy);
        assert_eq!(third.removed, 0);
        assert_eq!(third.updated, 1);
        assert_eq!(third.added, 0);
    }

    #[test]
    fn register_remote_indexes_six_factories() {
        let http = Arc::new(
            HttpClient::new(HttpClientConfig::builder().disable_system_proxy().build())
                .expect("http client"),
        );
        let mut registry = QuotaTargetRegistry::new();
        registry
            .register_remote(http, RemoteAdapterConfig::default())
            .unwrap();
        assert_eq!(registry.len(), 6);
        for id in ["openai", "anthropic", "moonshot", "qwen", "xai", "zhipu"] {
            assert!(
                registry.factory_for(&ProviderId::new(id)).is_some(),
                "missing factory for {id}"
            );
        }
    }

    #[tokio::test]
    async fn scheduler_start_cancel_shutdown() {
        let sched = scheduler();
        sched.register(
            identity("acct-a", "openai").to_refresh_target(RefreshPolicy::default(), None),
        );
        let handle = sched.start();
        handle.cancel();
        handle.shutdown().await;
    }
}
