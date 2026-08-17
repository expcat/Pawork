//! 进程内 reasoning continuation 保护器。
//!
//! 这是 S6 adapter 的临时宿主实现：opaque bytes 只保存在内存表中，canonical
//! 事件仅携带 [`ProtectedBlobRef`]。S7 接入持久化 Protected Blob Store 后由宿主
//! 注入替换；本类型不提供跨进程恢复保证。

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use pawork_domain::ProtectedBlobRef;
use pawork_provider_core::{ReasoningProtectError, ReasoningProtector};

/// 只保证同一实例、同一进程内回放的 reasoning protector。
pub struct InMemoryReasoningProtector {
    namespace: u64,
    next_id: AtomicU64,
    blobs: RwLock<HashMap<ProtectedBlobRef, Vec<u8>>>,
}

static NEXT_PROTECTOR_NAMESPACE: AtomicU64 = AtomicU64::new(0);

impl Default for InMemoryReasoningProtector {
    fn default() -> Self {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(NEXT_PROTECTOR_NAMESPACE.fetch_add(1, Ordering::Relaxed));
        Self {
            namespace: hasher.finish(),
            next_id: AtomicU64::new(0),
            blobs: RwLock::new(HashMap::new()),
        }
    }
}

impl fmt::Debug for InMemoryReasoningProtector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entry_count = self.blobs.read().map(|blobs| blobs.len()).unwrap_or(0);
        formatter
            .debug_struct("InMemoryReasoningProtector")
            .field("entry_count", &entry_count)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl ReasoningProtector for InMemoryReasoningProtector {
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let blob_ref = ProtectedBlobRef::new(format!(
            "memory-reasoning-{:016x}-{id}",
            self.namespace
        ));
        self.blobs
            .write()
            .map_err(|_| ReasoningProtectError::Unavailable)?
            .insert(blob_ref.clone(), payload.to_vec());
        Ok(blob_ref)
    }

    async fn resolve(
        &self,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<Vec<u8>, ReasoningProtectError> {
        self.blobs
            .read()
            .map_err(|_| ReasoningProtectError::Unavailable)?
            .get(blob_ref)
            .cloned()
            .ok_or(ReasoningProtectError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_without_debugging_payload() {
        let protector = InMemoryReasoningProtector::default();
        let secret = b"opaque-encrypted-continuation";
        let blob_ref = protector.protect(secret).await.expect("protect");

        assert_eq!(protector.resolve(&blob_ref).await.unwrap(), secret);
        assert!(!format!("{protector:?}").contains("opaque-encrypted"));
    }

    #[tokio::test]
    async fn unknown_reference_fails_closed() {
        let protector = InMemoryReasoningProtector::default();
        let error = protector
            .resolve(&ProtectedBlobRef::new("missing"))
            .await
            .expect_err("unknown reference");
        assert!(error.is_unavailable());
    }

    #[tokio::test]
    async fn independent_protectors_cannot_alias_blob_references() {
        let first = InMemoryReasoningProtector::default();
        let second = InMemoryReasoningProtector::default();
        let first_ref = first.protect(b"first").await.unwrap();
        let second_ref = second.protect(b"second").await.unwrap();

        assert_ne!(first_ref, second_ref);
        assert!(second.resolve(&first_ref).await.is_err());
    }
}
