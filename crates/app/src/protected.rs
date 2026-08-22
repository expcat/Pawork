//! ReasoningProtector 持久化：PWB1 ProtectedBlobStore + 文件主密钥。
//!
//! Provider crate 只看到 [`ReasoningProtector`]；本模块是宿主装配层的生产实现。
//! scope 使用 instance-level `SessionId::from("instance-reasoning")`，因为
//! canonical 请求没有 chat session_id。

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use pawork_domain::{ProtectedBlobRef, ProviderId, SessionId};
use pawork_providers::{
    InMemoryReasoningProtector, ReasoningProtectError, ReasoningProtector,
};
use pawork_storage::blob::{
    AeadKey, BlobScope, ProtectedBlobError, ProtectedBlobStore, ProtectedKeyResolver,
};

use crate::{AppCore, AppError};

const MASTER_KEY_FILE: &str = "master.key";
const INSTANCE_REASONING_SESSION: &str = "instance-reasoning";
const CURRENT_VERSION: u32 = 1;

/// `<protected_root>/master.key`：32 字节主密钥，权限 0600，缺失则原子创建。
pub struct FileKeyResolver {
    path: PathBuf,
    master: [u8; 32],
}

impl FileKeyResolver {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, AppError> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let path = root.join(MASTER_KEY_FILE);
        let master = if path.exists() {
            let bytes = fs::read(&path)?;
            let master: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                AppError::Protected("protected master key must be 32 bytes".into())
            })?;
            master
        } else {
            let mut master = [0u8; 32];
            getrandom::fill(&mut master).map_err(|err| {
                AppError::Protected(format!("failed to generate protected master key: {err}"))
            })?;
            atomic_write_0600(&path, &master)?;
            master
        };
        Ok(Self { path, master })
    }

    fn derive(&self, scope: &BlobScope, version: u32) -> AeadKey {
        let mut material = Vec::with_capacity(
            scope.provider_id().as_str().len() + 1 + scope.session_id().as_str().len() + 4,
        );
        material.extend_from_slice(scope.provider_id().as_str().as_bytes());
        material.push(0);
        material.extend_from_slice(scope.session_id().as_str().as_bytes());
        material.extend_from_slice(&version.to_be_bytes());
        let digest = blake3::keyed_hash(&self.master, &material);
        AeadKey::new(*digest.as_bytes())
    }
}

impl fmt::Debug for FileKeyResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileKeyResolver")
            .field("path", &self.path)
            .field("master", &"[REDACTED]")
            .finish()
    }
}

impl ProtectedKeyResolver for FileKeyResolver {
    fn current_version(
        &self,
        _scope: &BlobScope,
    ) -> Result<u32, pawork_storage::blob::KeyResolutionError> {
        Ok(CURRENT_VERSION)
    }

    fn resolve(
        &self,
        scope: &BlobScope,
        version: u32,
    ) -> Result<AeadKey, pawork_storage::blob::KeyResolutionError> {
        Ok(self.derive(scope, version))
    }
}

/// 把 PWB1 store 接到 ReasoningProtector。scope 是 instance-level，不是 chat session。
pub struct PersistentReasoningProtector {
    store: Arc<ProtectedBlobStore>,
    scope: BlobScope,
}

impl PersistentReasoningProtector {
    pub fn new(store: Arc<ProtectedBlobStore>, provider_id: ProviderId) -> Self {
        Self {
            store,
            scope: BlobScope::new(provider_id, SessionId::from(INSTANCE_REASONING_SESSION)),
        }
    }
}

impl fmt::Debug for PersistentReasoningProtector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PersistentReasoningProtector")
            .field("provider_id", &self.scope.provider_id().as_str())
            .field("session_id", &self.scope.session_id().as_str())
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl ReasoningProtector for PersistentReasoningProtector {
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError> {
        self.store
            .put(&self.scope, payload)
            .await
            .map(|outcome| outcome.blob_ref)
            .map_err(map_blob_error)
    }

    async fn resolve(
        &self,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<Vec<u8>, ReasoningProtectError> {
        self.store
            .get(&self.scope, blob_ref)
            .await
            .map(|blob| blob.expose().to_vec())
            .map_err(map_blob_error)
    }
}

/// 可在 store 打开后把 InMemory 换成 Persistent 的宿主注入点。
pub struct SwappableReasoningProtector {
    inner: RwLock<Arc<dyn ReasoningProtector>>,
}

impl SwappableReasoningProtector {
    pub fn in_memory() -> Self {
        Self {
            inner: RwLock::new(Arc::new(InMemoryReasoningProtector::default())),
        }
    }

    pub fn bind(&self, protector: Arc<dyn ReasoningProtector>) {
        *self.inner.write().expect("reasoning protector lock") = protector;
    }

    pub fn current(&self) -> Arc<dyn ReasoningProtector> {
        self.inner
            .read()
            .expect("reasoning protector lock")
            .clone()
    }
}

impl fmt::Debug for SwappableReasoningProtector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SwappableReasoningProtector")
            .field("inner", &"[REDACTED]")
            .finish()
    }
}

#[async_trait::async_trait]
impl ReasoningProtector for SwappableReasoningProtector {
    async fn protect(&self, payload: &[u8]) -> Result<ProtectedBlobRef, ReasoningProtectError> {
        self.current().protect(payload).await
    }

    async fn resolve(
        &self,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<Vec<u8>, ReasoningProtectError> {
        self.current().resolve(blob_ref).await
    }
}

impl AppCore {
    pub(crate) fn rebind_persistent_protector(&self) {
        let Some(store) = self.protected_store.as_ref() else {
            return;
        };
        let persistent = Arc::new(PersistentReasoningProtector::new(
            store.clone(),
            self.provider_id.clone(),
        ));
        self.reasoning_protector.bind(persistent);
    }

    pub(crate) async fn open_protected(&mut self, root: impl AsRef<Path>) -> Result<(), AppError> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let resolver = Arc::new(FileKeyResolver::open(root)?);
        let store = Arc::new(ProtectedBlobStore::open(root, resolver).await?);
        self.protected_store = Some(store);
        self.rebind_persistent_protector();
        Ok(())
    }
}

fn map_blob_error(error: ProtectedBlobError) -> ReasoningProtectError {
    if error.is_corrupted() {
        ReasoningProtectError::Corrupted
    } else {
        ReasoningProtectError::Unavailable
    }
}

fn atomic_write_0600(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("key.tmp");
    write_new_file_0600(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(unix)]
fn write_new_file_0600(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
}

#[cfg(not(unix))]
fn write_new_file_0600(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_providers::ReasoningProtector;

    #[tokio::test]
    async fn persistent_protector_round_trips_and_redacts_debug() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolver = Arc::new(FileKeyResolver::open(dir.path()).expect("resolver"));
        let store = Arc::new(
            ProtectedBlobStore::open(dir.path(), resolver)
                .await
                .expect("store"),
        );
        let protector = PersistentReasoningProtector::new(store, ProviderId::from("anthropic"));
        let blob_ref = protector.protect(b"signature-secret").await.expect("protect");
        assert_eq!(
            protector.resolve(&blob_ref).await.expect("resolve"),
            b"signature-secret"
        );
        let debug = format!("{protector:?}");
        assert!(!debug.contains("signature-secret"));
        assert!(!debug.contains("[REDACTED]") || debug.contains("PersistentReasoningProtector"));
        let missing = protector
            .resolve(&ProtectedBlobRef::new("missing"))
            .await
            .expect_err("unknown ref");
        assert!(missing.is_unavailable());
    }

    #[tokio::test]
    async fn swappable_starts_in_memory_then_binds_persistent() {
        let swappable = SwappableReasoningProtector::in_memory();
        let first = swappable.protect(b"mem").await.expect("mem protect");
        assert_eq!(swappable.resolve(&first).await.expect("mem resolve"), b"mem");

        let dir = tempfile::tempdir().expect("tempdir");
        let resolver = Arc::new(FileKeyResolver::open(dir.path()).expect("resolver"));
        let store = Arc::new(
            ProtectedBlobStore::open(dir.path(), resolver)
                .await
                .expect("store"),
        );
        let persistent = Arc::new(PersistentReasoningProtector::new(
            store,
            ProviderId::from("test"),
        ));
        swappable.bind(persistent);
        let second = swappable.protect(b"disk").await.expect("disk protect");
        assert_eq!(swappable.resolve(&second).await.expect("disk resolve"), b"disk");
        assert!(swappable.resolve(&first).await.is_err());
        assert!(!format!("{swappable:?}").contains("disk"));
    }

    #[tokio::test]
    async fn load_with_binds_same_swappable_onto_assembled_adapter() {
        use pawork_workspace::config::{PaworkConfig, ProviderConfig};
        use pawork_auth::locator::api_key_env_name;

        let dir = tempfile::tempdir().expect("tempdir");
        let id = "r5c-protector-identity";
        let env_name = api_key_env_name(id);
        crate::testsupport::set_env(&env_name, "not-a-real-key");
        let mut config = PaworkConfig {
            default_provider: Some(id.into()),
            default_model: Some("claude-3-5-sonnet".into()),
            providers: vec![ProviderConfig {
                id: id.into(),
                base_url: Some("https://example.test/v1".into()),
                ..ProviderConfig::default()
            }],
            ..PaworkConfig::default()
        };
        config.extra.insert(
            "provider_protocols".into(),
            serde_json::json!({ id: "messages" }),
        );
        let mut core = crate::AppCore::from_config_inner(
            config,
            None,
            None,
            std::sync::Arc::new(pawork_auth::MemoryBackend::new()),
            false,
        )
        .await
        .expect("assemble");
        core.open_protected(dir.path().join("protected"))
            .await
            .expect("open protected");
        let blob = core
            .reasoning_protector
            .protect(b"signature-secret")
            .await
            .expect("protect via core");
        assert_eq!(
            core.reasoning_protector
                .resolve(&blob)
                .await
                .expect("resolve via core"),
            b"signature-secret"
        );
        crate::testsupport::remove_env(&env_name);
        assert!(!format!("{:?}", core.reasoning_protector).contains("signature-secret"));
        let master = dir.path().join("protected").join("master.key");
        assert_eq!(std::fs::read(&master).expect("master").len(), 32);
    }
}
