//! Provider-neutral bridge between reasoning mappers and the Protected Blob Store.
//!
//! Provider crates parse and reconstruct their own wire formats. This module only
//! encrypts opaque continuation bytes and resolves the safe reference carried by
//! [`ReasoningItem`]; it never branches on a Provider name or interprets payloads.
//!
//! Reference-count lifecycle: [`ReasoningStateBridge::protect`] stores the blob with
//! `ref_count = 1` owned by the first persisted event, so committing that event needs
//! no extra retain. Only genuinely new owners retain; owners release when their event
//! is physically removed, and a failed commit rolls the reserved reference back.

use agent_domain::{ProtectedBlobRef, ReasoningItem};
use protected_blob_store::{
    BlobScope, GcReport, ProtectedBlob, ProtectedBlobError, ProtectedBlobStore,
};

/// Shared storage boundary for opaque reasoning continuations.
#[derive(Clone)]
pub struct ReasoningStateBridge {
    store: ProtectedBlobStore,
}

impl ReasoningStateBridge {
    pub fn new(store: ProtectedBlobStore) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &ProtectedBlobStore {
        &self.store
    }

    pub async fn shutdown(self) -> Result<(), ProtectedBlobError> {
        self.store.shutdown().await
    }

    /// Encrypt an opaque Provider payload and return the stable logical reference.
    ///
    /// The store assigns the new blob `ref_count = 1`, which belongs to the first
    /// persisted event carrying the returned reference. Committing that event
    /// therefore must not retain again; if the append fails, the reserved ownership
    /// must be handed back via [`Self::rollback_uncommitted`].
    pub async fn protect(
        &self,
        scope: &BlobScope,
        payload: &[u8],
    ) -> Result<ProtectedBlobRef, ProtectedBlobError> {
        Ok(self.store.put(scope, payload).await?.blob_ref)
    }

    /// Resolve the continuation referenced by a canonical reasoning item.
    pub async fn resolve(
        &self,
        scope: &BlobScope,
        item: &ReasoningItem,
    ) -> Result<ProtectedBlob, ProtectedBlobError> {
        self.resolve_ref(scope, &item.protected_blob_ref).await
    }

    /// Resolve a stable logical reference without interpreting the plaintext.
    pub async fn resolve_ref(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<ProtectedBlob, ProtectedBlobError> {
        self.store.get(scope, blob_ref).await
    }

    /// Record a new owner of an already-protected blob, returning the new count.
    ///
    /// The initial reference from [`Self::protect`] belongs to the first persisted
    /// event, so replaying or re-persisting that same event must not retain again.
    /// Only a genuinely new owner (for example a compaction re-emission referencing
    /// the same continuation) calls this.
    pub async fn retain(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<u64, ProtectedBlobError> {
        self.store.retain(scope, blob_ref).await
    }

    /// Drop one owner's reference, returning the remaining count.
    ///
    /// Called when an event owning the reference is physically removed (compaction
    /// or session deletion). When the last reference is released the blob enters the
    /// store's retention window and stays resolvable until [`Self::gc`] reclaims it.
    pub async fn release(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<u64, ProtectedBlobError> {
        self.store.release(scope, blob_ref).await
    }

    /// Hand back the initial ownership reserved by [`Self::protect`] after the event
    /// commit failed, returning the remaining count.
    ///
    /// A failed append means the first persisted event never materialized, so its
    /// reserved reference must be released or the blob leaks forever. The blob is
    /// not deleted here; [`Self::gc`] reclaims it once the retention window passes.
    pub async fn rollback_uncommitted(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<u64, ProtectedBlobError> {
        self.store.release(scope, blob_ref).await
    }

    /// Physically delete blobs whose last reference was released before the
    /// retention window, returning what was reclaimed.
    pub async fn gc(&self) -> Result<GcReport, ProtectedBlobError> {
        self.store.gc().await
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, sync::Arc, time::Duration};

    use agent_domain::{ProviderId, ReasoningItemId, SessionId};
    use protected_blob_store::{
        AeadKey, InMemoryKeyResolver, ProtectedBlobStoreOptions, ProtectedKeyResolver,
    };
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn opaque_payload_round_trips_without_entering_canonical_or_plaintext_storage() {
        let root = TempDir::new().expect("temporary store");
        let scope = BlobScope::new(ProviderId::from("openai"), SessionId::from("session-1"));
        let resolver = Arc::new(InMemoryKeyResolver::new());
        resolver.insert(scope.clone(), 1, AeadKey::new([0x5a; 32]));
        resolver.set_current(scope.clone(), 1);
        let resolver: Arc<dyn ProtectedKeyResolver> = resolver;
        let store = ProtectedBlobStore::open(root.path(), resolver.clone())
            .await
            .expect("open protected store");
        let bridge = ReasoningStateBridge::new(store);
        let secret = b"encrypted-reasoning-continuation-do-not-persist";

        let blob_ref = bridge
            .protect(&scope, secret)
            .await
            .expect("protect payload");
        let item = ReasoningItem {
            id: ReasoningItemId::from("reasoning-1"),
            summary: Some("safe summary".into()),
            protected_blob_ref: blob_ref.clone(),
            opaque_metadata: BTreeMap::new(),
            continuation_metadata: BTreeMap::new(),
        };

        let canonical = serde_json::to_vec(&item).expect("serialize canonical item");
        assert!(!contains(&canonical, secret));
        let metadata = bridge
            .store()
            .metadata(&scope, &blob_ref)
            .await
            .expect("blob metadata");
        let digest = metadata.physical_digest;
        let ciphertext = fs::read(
            root.path()
                .join("protected")
                .join(&digest[..2])
                .join(&digest[2..4])
                .join(&digest),
        )
        .expect("ciphertext file");
        assert!(!contains(&ciphertext, secret));

        bridge.shutdown().await.expect("close protected store");
        let store = ProtectedBlobStore::open(root.path(), resolver)
            .await
            .expect("reopen protected store");
        let bridge = ReasoningStateBridge::new(store);
        let replayed: ReasoningItem =
            serde_json::from_slice(&canonical).expect("replay canonical item");
        let recovered = bridge
            .resolve(&scope, &replayed)
            .await
            .expect("resolve payload after restart");
        assert_eq!(recovered.expose(), secret);
        let wrong_scope =
            BlobScope::new(ProviderId::from("openai"), SessionId::from("other-session"));
        let error = bridge
            .resolve(&wrong_scope, &replayed)
            .await
            .expect_err("scope isolation must fail closed");
        assert!(error.is_unavailable());
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[tokio::test]
    async fn failed_commit_rollback_feeds_zero_retention_gc() {
        let root = TempDir::new().expect("temporary store");
        let scope = BlobScope::new(ProviderId::from("openai"), SessionId::from("session-gc"));
        let mut options = ProtectedBlobStoreOptions::new(root.path());
        options.retention = Duration::ZERO;
        let bridge = open_bridge(&scope, options).await;
        let payload = b"uncommitted-continuation";

        let blob_ref = bridge
            .protect(&scope, payload)
            .await
            .expect("protect payload");
        let metadata = bridge
            .store()
            .metadata(&scope, &blob_ref)
            .await
            .expect("blob metadata");
        assert_eq!(metadata.ref_count, 1);

        // First-event ownership keeps the blob alive even with zero retention.
        assert_eq!(bridge.gc().await.expect("gc with owner").deleted, 0);

        // Event append failed: hand back the reserved first-owner reference.
        let remaining = bridge
            .rollback_uncommitted(&scope, &blob_ref)
            .await
            .expect("rollback uncommitted blob");
        assert_eq!(remaining, 0);
        let metadata = bridge
            .store()
            .metadata(&scope, &blob_ref)
            .await
            .expect("blob metadata after rollback");
        assert_eq!(metadata.ref_count, 0);
        assert!(metadata.retain_until_ms.is_some());

        let report = bridge.gc().await.expect("gc after rollback");
        assert_eq!(report.deleted, 1);
        assert!(report.reclaimed_bytes > 0);

        let digest = metadata.physical_digest;
        let ciphertext = root
            .path()
            .join("protected")
            .join(&digest[..2])
            .join(&digest[2..4])
            .join(&digest);
        assert!(!ciphertext.exists());
        let error = bridge
            .resolve_ref(&scope, &blob_ref)
            .await
            .expect_err("rolled-back blob must be gone");
        assert!(error.is_unavailable());
    }

    #[tokio::test]
    async fn additional_owners_retain_and_release_before_gc_collects() {
        let root = TempDir::new().expect("temporary store");
        let scope = BlobScope::new(
            ProviderId::from("anthropic"),
            SessionId::from("session-refcount"),
        );
        let bridge = open_bridge(&scope, ProtectedBlobStoreOptions::new(root.path())).await;
        let payload = b"shared-continuation";

        // protect reserves ref_count = 1 for the first persisted event; committing
        // that event must not retain again.
        let blob_ref = bridge
            .protect(&scope, payload)
            .await
            .expect("protect payload");
        let metadata = bridge
            .store()
            .metadata(&scope, &blob_ref)
            .await
            .expect("blob metadata");
        assert_eq!(metadata.ref_count, 1);

        // A genuinely new owner (e.g. compaction re-emission) takes its own reference.
        assert_eq!(bridge.retain(&scope, &blob_ref).await.expect("retain"), 2);

        // First event physically removed: one reference drops, blob stays live and
        // outside the retention window.
        assert_eq!(bridge.release(&scope, &blob_ref).await.expect("release"), 1);
        let metadata = bridge
            .store()
            .metadata(&scope, &blob_ref)
            .await
            .expect("blob metadata after first release");
        assert!(metadata.retain_until_ms.is_none());
        assert_eq!(
            bridge
                .resolve_ref(&scope, &blob_ref)
                .await
                .expect("resolve while owned")
                .expose(),
            payload
        );
        assert_eq!(bridge.gc().await.expect("gc while owned").deleted, 0);

        // Last owner removed: the blob enters the retention window, stays
        // resolvable, and gc must not collect it before the window passes.
        assert_eq!(bridge.release(&scope, &blob_ref).await.expect("release"), 0);
        let metadata = bridge
            .store()
            .metadata(&scope, &blob_ref)
            .await
            .expect("blob metadata after final release");
        assert!(metadata.retain_until_ms.is_some());
        assert_eq!(
            bridge
                .resolve_ref(&scope, &blob_ref)
                .await
                .expect("resolve during retention window")
                .expose(),
            payload
        );
        assert_eq!(
            bridge
                .gc()
                .await
                .expect("gc inside retention window")
                .deleted,
            0
        );

        let error = bridge
            .release(&scope, &blob_ref)
            .await
            .expect_err("release past zero must fail closed");
        assert!(matches!(
            error,
            ProtectedBlobError::RefCountUnderflow { .. }
        ));
    }

    async fn open_bridge(
        scope: &BlobScope,
        options: ProtectedBlobStoreOptions,
    ) -> ReasoningStateBridge {
        let resolver = Arc::new(InMemoryKeyResolver::new());
        resolver.insert(scope.clone(), 1, AeadKey::new([0x5a; 32]));
        resolver.set_current(scope.clone(), 1);
        let resolver: Arc<dyn ProtectedKeyResolver> = resolver;
        let store = ProtectedBlobStore::open_with_options(options, resolver)
            .await
            .expect("open protected store");
        ReasoningStateBridge::new(store)
    }
}
