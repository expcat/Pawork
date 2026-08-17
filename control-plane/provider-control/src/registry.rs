//! Dynamic ProviderFactory registry + transactional hot reload (P18-14, ADR-032/033).
//!
//! Eliminates hard-coded Provider branches: adapters register via ProviderFactory,
//! the composition layer looks up by ProviderId and composes. Hot reload goes
//! through parse / validate / stage / commit with rollback; the active snapshot
//! stays fully effective if any step fails.
//!
//! Concurrent reloads are serialized via an internal tokio::sync::Mutex so two
//! reloads never interleave into a non-atomic swap; readers take a cheap
//! `Arc<snapshot>` clone decoupled from reload.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock};

use pawork_domain::ProviderId;
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;

use crate::factory::{ProviderDescriptor, ProviderFactory};

/// Registry error (duplicate / unknown / inconsistent descriptors). Carries no secret.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// Provider already registered by another factory (cross-factory duplicate).
    #[error("provider factory already registered for {0}")]
    Duplicate(ProviderId),
    /// A single factory returned duplicate descriptors (intra-factory duplicate id).
    #[error("factory returned duplicate descriptor for {0}")]
    DuplicateDescriptor(ProviderId),
    /// No factory registered for the provider (explicit lookup failure).
    #[error("no provider factory registered for {0}")]
    Unknown(ProviderId),
    /// Factory contributed no descriptors (cannot be indexed).
    #[error("factory returned no descriptors")]
    NoDescriptors,
}

/// Immutable, validated snapshot of the registry contents.
///
/// Produced by [`ProviderRegistryStage::finish`]; the live registry atomically
/// swaps in a new snapshot on commit. Readers only clone the outer `Arc`; the
/// snapshot itself never mutates.
#[derive(Clone)]
pub struct ProviderRegistrySnapshot {
    factories: Vec<Arc<dyn ProviderFactory>>,
    /// `provider_id` -> index into `factories`.
    by_provider: HashMap<ProviderId, usize>,
}

impl ProviderRegistrySnapshot {
    /// Empty snapshot.
    pub fn empty() -> Self {
        Self {
            factories: Vec::new(),
            by_provider: HashMap::new(),
        }
    }

    /// Whether no factory is registered.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }

    /// Number of registered factories (not providers; one factory may serve many).
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    /// All registered factories.
    pub fn factories(&self) -> &[Arc<dyn ProviderFactory>] {
        &self.factories
    }

    /// Whether a provider is registered.
    pub fn contains(&self, provider_id: &ProviderId) -> bool {
        self.by_provider.contains_key(provider_id)
    }

    /// Look up the factory serving this provider.
    pub fn factory_for(&self, provider_id: &ProviderId) -> Option<&Arc<dyn ProviderFactory>> {
        self.by_provider
            .get(provider_id)
            .map(|index| &self.factories[*index])
    }

    /// All registered provider ids (unordered).
    pub fn providers(&self) -> Vec<ProviderId> {
        self.by_provider.keys().cloned().collect()
    }

    /// Flat copy of all descriptors (for model-registry / capability negotiation).
    pub fn descriptors(&self) -> Vec<ProviderDescriptor> {
        let mut out = Vec::new();
        for factory in &self.factories {
            out.extend(factory.descriptors().iter().cloned());
        }
        out
    }
}

impl Default for ProviderRegistrySnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

/// Mutable staging builder: collect factories, validate (no cross-factory duplicate
/// provider, no intra-factory duplicate descriptor), finish into an immutable
/// snapshot for commit.
///
/// `register` / `unregister` validate immediately; any failure returns
/// [`RegistryError`] and leaves the stage at its pre-failure state (it can be
/// fixed and retried, or dropped).
pub struct ProviderRegistryStage {
    factories: Vec<Arc<dyn ProviderFactory>>,
    by_provider: HashMap<ProviderId, usize>,
}

impl Default for ProviderRegistryStage {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistryStage {
    /// Empty stage.
    pub fn new() -> Self {
        Self {
            factories: Vec::new(),
            by_provider: HashMap::new(),
        }
    }

    /// Build a stage seeded from an existing snapshot (incremental hot reload).
    pub fn from_snapshot(snapshot: &ProviderRegistrySnapshot) -> Self {
        Self {
            factories: snapshot.factories.clone(),
            by_provider: snapshot.by_provider.clone(),
        }
    }

    /// Number of staged factories.
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    /// Whether the stage is empty.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }

    /// Stage a factory. Validates: at least one descriptor; no intra-factory
    /// duplicate descriptor ids; no provider id already staged. On error the
    /// stage is unchanged.
    pub fn register(&mut self, factory: Arc<dyn ProviderFactory>) -> Result<(), RegistryError> {
        let descriptors = factory.descriptors();
        if descriptors.is_empty() {
            return Err(RegistryError::NoDescriptors);
        }
        // Intra-factory duplicate descriptor ids.
        let mut seen: HashSet<ProviderId> = HashSet::new();
        for descriptor in descriptors {
            if !seen.insert(descriptor.id.clone()) {
                return Err(RegistryError::DuplicateDescriptor(descriptor.id.clone()));
            }
        }
        // Cross-factory duplicate.
        for descriptor in descriptors {
            if self.by_provider.contains_key(&descriptor.id) {
                return Err(RegistryError::Duplicate(descriptor.id.clone()));
            }
        }
        // All checks passed: record the index.
        let index = self.factories.len();
        for descriptor in descriptors {
            self.by_provider.insert(descriptor.id.clone(), index);
        }
        self.factories.push(factory);
        Ok(())
    }

    /// Remove the whole factory that owns `provider_id` (a multi-provider factory
    /// loses all its providers). Idempotent; returns whether a factory was removed.
    pub fn unregister(&mut self, provider_id: &ProviderId) -> bool {
        let Some(&index) = self.by_provider.get(provider_id) else {
            return false;
        };
        self.factories.remove(index);
        self.by_provider.clear();
        for (i, factory) in self.factories.iter().enumerate() {
            for descriptor in factory.descriptors() {
                self.by_provider.insert(descriptor.id.clone(), i);
            }
        }
        true
    }

    /// Finalize into an immutable snapshot (for commit).
    pub fn finish(self) -> ProviderRegistrySnapshot {
        ProviderRegistrySnapshot {
            factories: self.factories,
            by_provider: self.by_provider,
        }
    }
}

/// Live, hot-reloadable provider factory registry.
///
/// Readers take a cheap `Arc<ProviderRegistrySnapshot>` clone; hot reload is
/// serialized and swaps the snapshot atomically. On any parse/validate/stage
/// failure the previous snapshot stays fully effective — no half-applied state,
/// in-flight requests keep using the old factory set.
pub struct ProviderRegistry {
    snapshot: RwLock<Arc<ProviderRegistrySnapshot>>,
    reload_lock: AsyncMutex<()>,
}

impl ProviderRegistry {
    /// Empty registry.
    pub fn empty() -> Self {
        Self::from_snapshot(ProviderRegistrySnapshot::empty())
    }

    /// Build from a given snapshot (initial assembly).
    pub fn from_snapshot(snapshot: ProviderRegistrySnapshot) -> Self {
        Self {
            snapshot: RwLock::new(Arc::new(snapshot)),
            reload_lock: AsyncMutex::new(()),
        }
    }

    /// Build from a stage (equivalent to `from_snapshot(stage.finish())`).
    pub fn from_stage(stage: ProviderRegistryStage) -> Self {
        Self::from_snapshot(stage.finish())
    }

    /// Current active snapshot (cheap `Arc` clone, decoupled from reload).
    pub fn snapshot(&self) -> Arc<ProviderRegistrySnapshot> {
        self.snapshot
            .read()
            .expect("provider registry snapshot lock poisoned")
            .clone()
    }

    /// Whether a provider is registered.
    pub fn contains(&self, provider_id: &ProviderId) -> bool {
        self.snapshot().contains(provider_id)
    }

    /// Look up the factory serving a provider (visible immediately after register).
    pub fn factory_for(&self, provider_id: &ProviderId) -> Option<Arc<dyn ProviderFactory>> {
        self.snapshot().factory_for(provider_id).cloned()
    }

    /// Explicit lookup; unregistered yields [`RegistryError::Unknown`].
    pub fn require_factory(
        &self,
        provider_id: &ProviderId,
    ) -> Result<Arc<dyn ProviderFactory>, RegistryError> {
        self.factory_for(provider_id)
            .ok_or_else(|| RegistryError::Unknown(provider_id.clone()))
    }

    /// Atomically replace the active snapshot. Not serialized — only for a
    /// snapshot already built via parse/validate/stage; the full hot-reload path
    /// with rollback + concurrency control is [`Self::reload`].
    pub fn commit(&self, snapshot: ProviderRegistrySnapshot) -> Arc<ProviderRegistrySnapshot> {
        let next = Arc::new(snapshot);
        *self
            .snapshot
            .write()
            .expect("provider registry snapshot lock poisoned") = next.clone();
        next
    }

    /// Transactional hot reload: parse -> validate -> stage -> commit, rollback
    /// on failure.
    ///
    /// - Concurrent reloads are serialized (internal `tokio::sync::Mutex`); two
    ///   reloads never interleave.
    /// - `build` receives the current active snapshot (for diff / incremental
    ///   validation) and returns either a fully staged registry (committed
    ///   atomically) or an error (previous snapshot unchanged).
    pub async fn reload<F, Fut, E>(&self, build: F) -> Result<Arc<ProviderRegistrySnapshot>, E>
    where
        F: FnOnce(Arc<ProviderRegistrySnapshot>) -> Fut,
        Fut: Future<Output = Result<ProviderRegistryStage, E>>,
    {
        let _guard = self.reload_lock.lock().await;
        let previous = self.snapshot();
        match build(previous).await {
            Ok(stage) => {
                let next = Arc::new(stage.finish());
                *self
                    .snapshot
                    .write()
                    .expect("provider registry snapshot lock poisoned") = next.clone();
                Ok(next)
            }
            Err(err) => Err(err),
        }
    }

    /// Dynamically register a single factory (transactional, serialized). On
    /// validation error the registry is unchanged.
    pub async fn register_factory(
        &self,
        factory: Arc<dyn ProviderFactory>,
    ) -> Result<Arc<ProviderRegistrySnapshot>, RegistryError> {
        self.reload(|previous| async move {
            let mut stage = ProviderRegistryStage::from_snapshot(&previous);
            stage.register(factory)?;
            Ok(stage)
        })
        .await
    }

    /// Dynamically remove the factory owning `provider_id` (idempotent, serialized).
    pub async fn unregister_factory(
        &self,
        provider_id: ProviderId,
    ) -> Arc<ProviderRegistrySnapshot> {
        let _guard = self.reload_lock.lock().await;
        let previous = self.snapshot();
        let mut stage = ProviderRegistryStage::from_snapshot(&previous);
        stage.unregister(&provider_id);
        let next = Arc::new(stage.finish());
        *self
            .snapshot
            .write()
            .expect("provider registry snapshot lock poisoned") = next.clone();
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn factory(provider_ids: &[&str]) -> Arc<dyn ProviderFactory> {
        let descriptors = provider_ids
            .iter()
            .map(|&id| ProviderDescriptor {
                id: ProviderId::new(id),
                builtin_models: Vec::new(),
            })
            .collect();
        Arc::new(StubFactory { descriptors })
    }

    struct StubFactory {
        descriptors: Vec<ProviderDescriptor>,
    }

    #[async_trait::async_trait]
    impl ProviderFactory for StubFactory {
        fn descriptors(&self) -> &[ProviderDescriptor] {
            &self.descriptors
        }
        async fn compose(
            &self,
            _: &crate::CredentialLease,
            _: &crate::account::CredentialMetadata,
        ) -> Result<crate::factory::ComposedProvider, crate::factory::FactoryError> {
            Err(crate::factory::FactoryError::MissingBuilder(
                self.descriptors
                    .first()
                    .map(|d| d.id.clone())
                    .unwrap_or_else(|| ProviderId::new("stub")),
            ))
        }
    }

    #[test]
    fn stage_rejects_duplicate_provider_and_intra_factory_dup() {
        let mut stage = ProviderRegistryStage::new();
        stage.register(factory(&["openai"])).unwrap();
        assert!(matches!(
            stage.register(factory(&["openai", "anthropic"])),
            Err(RegistryError::Duplicate(id)) if id.as_str() == "openai"
        ));
        assert!(matches!(
            stage.register(factory(&["x", "x"])),
            Err(RegistryError::DuplicateDescriptor(id)) if id.as_str() == "x"
        ));
        assert!(matches!(
            stage.register(factory(&[])),
            Err(RegistryError::NoDescriptors)
        ));
        assert_eq!(stage.len(), 1, "failed register must not mutate stage");
        let snapshot = stage.finish();
        assert!(snapshot.contains(&ProviderId::new("openai")));
        assert!(!snapshot.contains(&ProviderId::new("anthropic")));
    }

    #[test]
    fn unregister_removes_whole_factory_owning_provider() {
        let mut stage = ProviderRegistryStage::new();
        stage.register(factory(&["openai", "anthropic"])).unwrap();
        assert!(stage.unregister(&ProviderId::new("openai")));
        let snapshot = stage.finish();
        assert!(!snapshot.contains(&ProviderId::new("openai")));
        assert!(!snapshot.contains(&ProviderId::new("anthropic")));
        assert!(snapshot.is_empty());
        let mut again = ProviderRegistryStage::from_snapshot(&snapshot);
        assert!(!again.unregister(&ProviderId::new("openai")));
    }

    #[tokio::test]
    async fn unknown_factory_is_explicit_error() {
        let registry = ProviderRegistry::empty();
        assert!(matches!(
            registry.require_factory(&ProviderId::new("nope")),
            Err(RegistryError::Unknown(id)) if id.as_str() == "nope"
        ));
    }

    #[tokio::test]
    async fn duplicate_register_leaves_live_snapshot_unchanged() {
        let mut stage = ProviderRegistryStage::new();
        stage.register(factory(&["keep"])).unwrap();
        let registry = ProviderRegistry::from_stage(stage);
        let err = match registry.register_factory(factory(&["keep"])).await {
            Err(err) => err,
            Ok(_) => panic!("expected duplicate"),
        };
        assert!(matches!(err, RegistryError::Duplicate(_)));
        assert_eq!(registry.snapshot().len(), 1);
        assert!(registry.contains(&ProviderId::new("keep")));
    }

    #[tokio::test]
    async fn failed_reload_keeps_old_snapshot_atomically() {
        let mut stage = ProviderRegistryStage::new();
        stage.register(factory(&["live"])).unwrap();
        let registry = ProviderRegistry::from_stage(stage);
        let err = match registry
            .reload(|previous| async move {
                let mut next = ProviderRegistryStage::from_snapshot(&previous);
                next.register(factory(&[]))?;
                Ok::<_, RegistryError>(next)
            })
            .await
        {
            Err(err) => err,
            Ok(_) => panic!("expected no-descriptors"),
        };
        assert!(matches!(err, RegistryError::NoDescriptors));
        assert!(registry.contains(&ProviderId::new("live")));
        assert!(!registry.contains(&ProviderId::new("ghost")));
    }

    #[tokio::test]
    async fn concurrent_reloads_are_serialized() {
        let mut stage = ProviderRegistryStage::new();
        stage.register(factory(&["a"])).unwrap();
        let registry = Arc::new(ProviderRegistry::from_stage(stage));
        let r1 = registry.clone();
        let r2 = registry.clone();
        let first = tokio::spawn(async move { r1.register_factory(factory(&["b"])).await });
        let second = tokio::spawn(async move { r2.register_factory(factory(&["c"])).await });
        let a = first.await.unwrap();
        let b = second.await.unwrap();
        assert!(a.is_ok());
        assert!(b.is_ok());
        assert!(registry.contains(&ProviderId::new("a")));
        assert!(registry.contains(&ProviderId::new("b")));
        assert!(registry.contains(&ProviderId::new("c")));
    }
}
