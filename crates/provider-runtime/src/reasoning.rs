//! Provider-neutral 的统一 reasoning 保护共享 API。
//!
//! Provider crates 只负责解析与重组各自的 wire 格式；加密 opaque continuation
//! 字节与解析稳定逻辑引用统一走 [`ReasoningProtector`]，不按 Provider 名分支、
//! 不解释明文。持久实现 [`ProtectedBlobStoreProtector`] 构造时捕获
//! `store + BlobScope`，调用方无需逐次传 scope；内存实现
//! [`InMemoryReasoningProtector`] 供测试与组合层开发使用。

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        RwLock,
    },
};

use agent_domain::ProtectedBlobRef;
use protected_blob_store::{BlobScope, ProtectedBlob, ProtectedBlobError, ProtectedBlobStore};
use thiserror::Error;

/// 统一的 reasoning 保护错误，屏蔽底层存储的失败形状。
#[derive(Debug, Error)]
pub enum ReasoningProtectError {
    /// 引用不存在、跨 scope 访问、密钥不可用等一律失败关闭。
    #[error("reasoning continuation unavailable")]
    Unavailable,
    /// 密文摘要、信封或 AEAD 认证失败。
    #[error("reasoning continuation corrupted")]
    Corrupted,
    #[error(transparent)]
    Storage(#[from] ProtectedBlobError),
}

impl ReasoningProtectError {
    pub fn is_unavailable(&self) -> bool {
        match self {
            Self::Unavailable => true,
            Self::Storage(error) => error.is_unavailable(),
            Self::Corrupted => false,
        }
    }

    pub fn is_corrupted(&self) -> bool {
        match self {
            Self::Corrupted => true,
            Self::Storage(error) => error.is_corrupted(),
            Self::Unavailable => false,
        }
    }
}

/// 受保护 reasoning continuation 的统一存取边界。
#[async_trait::async_trait]
pub trait ReasoningProtector: Send + Sync {
    /// 加密 opaque payload，返回稳定逻辑引用。
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError>;

    /// 解析稳定逻辑引用指向的明文，不解释其内容。
    async fn resolve(
        &self,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<ProtectedBlob, ReasoningProtectError>;
}

/// 内存 reasoning protector：测试与组合层开发用，无持久化与密钥管理。
#[derive(Default)]
pub struct InMemoryReasoningProtector {
    blobs: RwLock<HashMap<ProtectedBlobRef, Vec<u8>>>,
    next_ref: AtomicU64,
}

impl InMemoryReasoningProtector {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl ReasoningProtector for InMemoryReasoningProtector {
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError> {
        let blob_ref = ProtectedBlobRef::from(format!(
            "mem_{}",
            self.next_ref.fetch_add(1, Ordering::Relaxed)
        ));
        self.blobs
            .write()
            .expect("in-memory protector poisoned")
            .insert(blob_ref.clone(), payload.to_vec());
        Ok(blob_ref)
    }

    async fn resolve(
        &self,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<ProtectedBlob, ReasoningProtectError> {
        let payload = self
            .blobs
            .read()
            .expect("in-memory protector poisoned")
            .get(blob_ref)
            .cloned()
            .ok_or(ReasoningProtectError::Unavailable)?;
        Ok(ProtectedBlob::new(payload))
    }
}

/// 持久 reasoning protector：构造时捕获 store 与 `BlobScope`。
#[derive(Clone)]
pub struct ProtectedBlobStoreProtector {
    store: ProtectedBlobStore,
    scope: BlobScope,
}

impl ProtectedBlobStoreProtector {
    pub fn new(store: ProtectedBlobStore, scope: BlobScope) -> Self {
        Self { store, scope }
    }
}

#[async_trait::async_trait]
impl ReasoningProtector for ProtectedBlobStoreProtector {
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError> {
        Ok(self.store.put(&self.scope, payload).await?.blob_ref)
    }

    async fn resolve(
        &self,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<ProtectedBlob, ReasoningProtectError> {
        self.store
            .get(&self.scope, blob_ref)
            .await
            .map_err(ReasoningProtectError::from)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, sync::Arc};

    use agent_domain::{ProviderId, SessionId};
    use protected_blob_store::{AeadKey, InMemoryKeyResolver, ProtectedKeyResolver};
    use tempfile::TempDir;

    use super::*;

    fn scope(provider: &str, session: &str) -> BlobScope {
        BlobScope::new(ProviderId::from(provider), SessionId::from(session))
    }

    async fn open_store(root: &Path, scope: &BlobScope) -> ProtectedBlobStore {
        let resolver = Arc::new(InMemoryKeyResolver::new());
        resolver.insert(scope.clone(), 1, AeadKey::new([0x5a; 32]));
        resolver.set_current(scope.clone(), 1);
        let resolver: Arc<dyn ProtectedKeyResolver> = resolver;
        ProtectedBlobStore::open(root, resolver)
            .await
            .expect("open protected store")
    }

    #[tokio::test]
    async fn in_memory_protector_round_trips_and_misses_fail_closed() {
        let protector = InMemoryReasoningProtector::new();
        let secret = b"in-memory-reasoning-continuation";

        let blob_ref = protector.protect(secret).await.expect("protect payload");
        assert_eq!(
            protector
                .resolve(&blob_ref)
                .await
                .expect("resolve payload")
                .expose(),
            secret
        );
        assert_eq!(
            protector
                .resolve(&blob_ref)
                .await
                .expect("resolve payload again")
                .expose(),
            secret
        );

        let missing = ProtectedBlobRef::from("mem_999");
        let error = protector
            .resolve(&missing)
            .await
            .expect_err("unknown ref must fail closed");
        assert!(error.is_unavailable());
        assert!(!error.is_corrupted());
    }

    #[tokio::test]
    async fn persistent_protector_round_trips_across_restart_and_enforces_scope() {
        let root = TempDir::new().expect("temporary store");
        let scope_a = scope("openai", "session-a");
        let scope_b = scope("openai", "session-b");
        let secret = b"persistent-reasoning-continuation";

        let store = open_store(root.path(), &scope_a).await;
        let protector = ProtectedBlobStoreProtector::new(store.clone(), scope_a.clone());
        let blob_ref = protector.protect(secret).await.expect("protect payload");

        // 明文绝不落盘：物理文件只有随机化密文。
        let metadata = store
            .metadata(&scope_a, &blob_ref)
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

        // 同一 store 捕获另一 scope 时解析必须失败关闭。
        let other = ProtectedBlobStoreProtector::new(store.clone(), scope_b);
        let error = other
            .resolve(&blob_ref)
            .await
            .expect_err("cross-scope resolve must fail closed");
        assert!(error.is_unavailable());

        store.shutdown().await.expect("close protected store");
        let store = open_store(root.path(), &scope_a).await;
        let protector = ProtectedBlobStoreProtector::new(store.clone(), scope_a);
        assert_eq!(
            protector
                .resolve(&blob_ref)
                .await
                .expect("resolve payload after restart")
                .expose(),
            secret
        );
        store.shutdown().await.expect("close protected store");
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }
}
