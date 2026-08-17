//! Encrypted-at-rest storage for opaque reasoning continuation material.
//!
//! Event payloads keep only [`ProtectedBlobRef`]. Plaintext is authenticated with
//! XChaCha20-Poly1305 and scoped to `(ProviderId, SessionId, logical ref, key
//! version)` through AEAD AAD. The logical ref is stable across key rotation while
//! the physical file is addressed by a digest of randomized ciphertext.

use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pawork_domain::{ProtectedBlobRef, ProviderId, SessionId};
use pawork_sqlite::{DatabaseActor, DatabaseError};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, OptionalExtension};
use thiserror::Error;
use zeroize::Zeroizing;

const DATABASE_FILE: &str = "protected.sqlite3";
const BLOBS_DIR: &str = "protected";
const ENVELOPE_MAGIC: &[u8; 4] = b"PWB1";
const ENVELOPE_VERSION: u8 = 1;
const ALGORITHM_XCHACHA20_POLY1305: u8 = 1;
const NONCE_LEN: usize = 24;
const ENVELOPE_HEADER_LEN: usize = 4 + 1 + 1 + 4 + NONCE_LEN;

/// PWB1 磁盘信封：`PWB1`(4) + ver(1) + alg(1) + key_version BE u32(4) + nonce 24B + ciphertext。
pub const PWB1_MAGIC: &[u8; 4] = ENVELOPE_MAGIC;
pub const PWB1_VERSION: u8 = ENVELOPE_VERSION;
pub const PWB1_ALGORITHM: u8 = ALGORITHM_XCHACHA20_POLY1305;
pub const PWB1_NONCE_LEN: usize = NONCE_LEN;
pub const PWB1_HEADER_LEN: usize = ENVELOPE_HEADER_LEN;
const STATE_PENDING: &str = "pending";
const STATE_READY: &str = "ready";
const STATE_DELETING: &str = "deleting";

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS protected_blobs (
    logical_ref TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    physical_digest TEXT NOT NULL UNIQUE CHECK(length(physical_digest) = 64),
    key_version INTEGER NOT NULL CHECK(key_version >= 0),
    plaintext_size INTEGER NOT NULL CHECK(plaintext_size >= 0),
    ciphertext_size INTEGER NOT NULL CHECK(ciphertext_size >= 0),
    ref_count INTEGER NOT NULL CHECK(ref_count >= 0),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
    last_accessed_at_ms INTEGER NOT NULL CHECK(last_accessed_at_ms >= 0),
    retain_until_ms INTEGER,
    state TEXT NOT NULL DEFAULT 'ready'
        CHECK(state IN ('pending', 'ready', 'deleting'))
);
CREATE INDEX IF NOT EXISTS idx_protected_blobs_scope
    ON protected_blobs(provider_id, session_id);
CREATE INDEX IF NOT EXISTS idx_protected_blobs_gc
    ON protected_blobs(ref_count, retain_until_ms);
"#;

const STATE_INDEX_SQL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_protected_blobs_state_gc
    ON protected_blobs(state, ref_count, retain_until_ms);
"#;

pub type KeyVersion = u32;

/// Provider + Session isolation boundary for a protected blob.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlobScope {
    provider_id: ProviderId,
    session_id: SessionId,
}

impl BlobScope {
    pub fn new(provider_id: ProviderId, session_id: SessionId) -> Self {
        Self {
            provider_id,
            session_id,
        }
    }

    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

/// A 256-bit AEAD key whose memory is zeroed on drop.
#[derive(Clone)]
pub struct AeadKey(Zeroizing<[u8; 32]>);

impl AeadKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for AeadKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AeadKey([REDACTED])")
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("protected key unavailable")]
pub struct KeyResolutionError;

/// Dependency-inversion seam for scoped, versioned data keys.
pub trait ProtectedKeyResolver: Send + Sync {
    fn current_version(&self, scope: &BlobScope) -> Result<KeyVersion, KeyResolutionError>;
    fn resolve(
        &self,
        scope: &BlobScope,
        version: KeyVersion,
    ) -> Result<AeadKey, KeyResolutionError>;
}

/// Scope-aware in-memory resolver for tests and composition-layer development.
#[derive(Default)]
pub struct InMemoryKeyResolver {
    keys: RwLock<HashMap<(BlobScope, KeyVersion), AeadKey>>,
    current: RwLock<HashMap<BlobScope, KeyVersion>>,
}

impl InMemoryKeyResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, scope: BlobScope, version: KeyVersion, key: AeadKey) {
        self.keys
            .write()
            .expect("protected key map poisoned")
            .insert((scope, version), key);
    }

    pub fn set_current(&self, scope: BlobScope, version: KeyVersion) {
        self.current
            .write()
            .expect("protected current key map poisoned")
            .insert(scope, version);
    }

    pub fn remove(&self, scope: &BlobScope, version: KeyVersion) {
        self.keys
            .write()
            .expect("protected key map poisoned")
            .remove(&(scope.clone(), version));
    }
}

impl fmt::Debug for InMemoryKeyResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemoryKeyResolver")
            .field("keys", &"[REDACTED]")
            .finish()
    }
}

impl ProtectedKeyResolver for InMemoryKeyResolver {
    fn current_version(&self, scope: &BlobScope) -> Result<KeyVersion, KeyResolutionError> {
        self.current
            .read()
            .expect("protected current key map poisoned")
            .get(scope)
            .copied()
            .ok_or(KeyResolutionError)
    }

    fn resolve(
        &self,
        scope: &BlobScope,
        version: KeyVersion,
    ) -> Result<AeadKey, KeyResolutionError> {
        self.keys
            .read()
            .expect("protected key map poisoned")
            .get(&(scope.clone(), version))
            .cloned()
            .ok_or(KeyResolutionError)
    }
}

/// Decrypted bytes. Neither Debug nor serialization can expose the plaintext.
pub struct ProtectedBlob(Zeroizing<Vec<u8>>);

impl ProtectedBlob {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl AsRef<[u8]> for ProtectedBlob {
    fn as_ref(&self) -> &[u8] {
        self.expose()
    }
}

impl fmt::Debug for ProtectedBlob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProtectedBlob([REDACTED])")
    }
}

#[derive(Debug, Error)]
pub enum ProtectedBlobError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("protected blob I/O failed at {path}: {kind}")]
    Io { path: PathBuf, kind: io::ErrorKind },
    /// Missing ref, cross-scope access, missing file, or unavailable key all fail
    /// closed with the same public shape.
    #[error("protected blob unavailable")]
    ProtectedBlobUnavailable { blob_ref: ProtectedBlobRef },
    /// Ciphertext digest, envelope, or AEAD authentication failed.
    #[error("protected blob corrupted")]
    ProtectedBlobCorrupted { blob_ref: ProtectedBlobRef },
    #[error("protected blob reference count is already zero")]
    RefCountUnderflow { blob_ref: ProtectedBlobRef },
    #[error("protected blob disk budget exceeded")]
    DiskBudgetExceeded {
        usage: u64,
        requested: u64,
        budget: u64,
    },
}

impl ProtectedBlobError {
    fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            kind: source.kind(),
        }
    }

    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::ProtectedBlobUnavailable { .. })
    }

    pub fn is_corrupted(&self) -> bool {
        matches!(self, Self::ProtectedBlobCorrupted { .. })
    }
}

#[derive(Clone, Debug)]
pub struct ProtectedBlobStoreOptions {
    pub root: PathBuf,
    pub retention: Duration,
    pub disk_budget: Option<u64>,
}

impl ProtectedBlobStoreOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            retention: Duration::from_secs(7 * 24 * 60 * 60),
            disk_budget: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutOutcome {
    pub blob_ref: ProtectedBlobRef,
    pub key_version: KeyVersion,
    pub ref_count: u64,
    pub plaintext_size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedBlobMetadata {
    pub blob_ref: ProtectedBlobRef,
    pub scope: BlobScope,
    pub physical_digest: String,
    pub key_version: KeyVersion,
    pub plaintext_size: u64,
    pub ciphertext_size: u64,
    pub ref_count: u64,
    pub retain_until_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub deleted: u64,
    pub reclaimed_bytes: u64,
}

#[derive(Clone)]
pub struct ProtectedBlobStore {
    database: DatabaseActor,
    root: PathBuf,
    resolver: Arc<dyn ProtectedKeyResolver>,
    retention_ms: u64,
    disk_budget: Option<u64>,
}

impl ProtectedBlobStore {
    pub async fn open(
        root: impl Into<PathBuf>,
        resolver: Arc<dyn ProtectedKeyResolver>,
    ) -> Result<Self, ProtectedBlobError> {
        Self::open_with_options(ProtectedBlobStoreOptions::new(root), resolver).await
    }

    pub async fn open_with_options(
        options: ProtectedBlobStoreOptions,
        resolver: Arc<dyn ProtectedKeyResolver>,
    ) -> Result<Self, ProtectedBlobError> {
        let blobs = options.root.join(BLOBS_DIR);
        fs::create_dir_all(&blobs).map_err(|source| ProtectedBlobError::io(&blobs, source))?;
        let database = DatabaseActor::open(options.root.join(DATABASE_FILE)).await?;
        database
            .call(|connection| -> Result<(), ProtectedBlobError> {
                connection.execute_batch(SCHEMA_SQL)?;
                ensure_state_column(connection)?;
                connection.execute_batch(STATE_INDEX_SQL)?;
                Ok(())
            })
            .await??;
        let store = Self {
            database,
            root: options.root,
            resolver,
            retention_ms: u64::try_from(options.retention.as_millis()).unwrap_or(u64::MAX),
            disk_budget: options.disk_budget,
        };
        store.reconcile_incomplete_operations().await?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn put(
        &self,
        scope: &BlobScope,
        plaintext: &[u8],
    ) -> Result<PutOutcome, ProtectedBlobError> {
        let version = self
            .resolver
            .current_version(scope)
            .map_err(|_| unavailable(ProtectedBlobRef::from("new")))?;
        let key = self
            .resolver
            .resolve(scope, version)
            .map_err(|_| unavailable(ProtectedBlobRef::from("new")))?;
        let blob_ref = random_blob_ref();
        let envelope = seal(scope, &blob_ref, version, &key, plaintext)
            .map_err(|_| corrupted(blob_ref.clone()))?;
        let digest = blake3::hash(&envelope).to_hex().to_string();
        let path = ciphertext_path(&self.root, &digest)?;
        let scope = scope.clone();
        let logical_ref = blob_ref.clone();
        let plaintext_size = u64::try_from(plaintext.len()).unwrap_or(u64::MAX);
        let ciphertext_size = u64::try_from(envelope.len()).unwrap_or(u64::MAX);
        let budget = self.disk_budget;
        let scope_for_insert = scope.clone();
        let logical_for_insert = logical_ref.clone();
        let digest_for_insert = digest.clone();
        self.database
            .call(move |connection| -> Result<(), ProtectedBlobError> {
                if let Some(budget) = budget {
                    // Pending/deleting rows remain charged until reconciliation completes,
                    // so a crash cannot temporarily bypass the disk budget.
                    let usage: i64 = connection.query_row(
                        "SELECT COALESCE(SUM(ciphertext_size), 0) FROM protected_blobs",
                        [],
                        |row| row.get(0),
                    )?;
                    let usage = stored_u64(usage, &logical_for_insert)?;
                    if usage.saturating_add(ciphertext_size) > budget {
                        return Err(ProtectedBlobError::DiskBudgetExceeded {
                            usage,
                            requested: ciphertext_size,
                            budget,
                        });
                    }
                }
                let now = now_ms();
                connection.execute(
                    "INSERT INTO protected_blobs
                     (logical_ref, provider_id, session_id, physical_digest, key_version,
                      plaintext_size, ciphertext_size, ref_count, created_at_ms,
                      last_accessed_at_ms, retain_until_ms, state)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8, NULL, ?9)",
                    params![
                        logical_for_insert.as_str(),
                        scope_for_insert.provider_id.as_str(),
                        scope_for_insert.session_id.as_str(),
                        digest_for_insert,
                        i64::from(version),
                        stored_i64(plaintext_size),
                        stored_i64(ciphertext_size),
                        stored_i64(now),
                        STATE_PENDING,
                    ],
                )?;
                Ok(())
            })
            .await??;

        if let Err(error) = atomic_write(&path, &envelope) {
            let pending_ref = logical_ref.clone();
            let _ = self
                .database
                .call(move |connection| {
                    connection.execute(
                        "DELETE FROM protected_blobs WHERE logical_ref=?1 AND state=?2",
                        params![pending_ref.as_str(), STATE_PENDING],
                    )
                })
                .await;
            return Err(error);
        }

        let ready_ref = logical_ref.clone();
        let updated = self
            .database
            .call(move |connection| {
                connection.execute(
                    "UPDATE protected_blobs SET state=?2
                     WHERE logical_ref=?1 AND state=?3",
                    params![ready_ref.as_str(), STATE_READY, STATE_PENDING],
                )
            })
            .await??;
        if updated != 1 {
            // A concurrent/open-time reconciliation may have removed the pending row.
            // The caller never receives the ref in that case, so remove the file too.
            let _ = fs::remove_file(&path);
            return Err(unavailable(logical_ref));
        }

        Ok(PutOutcome {
            blob_ref: logical_ref,
            key_version: version,
            ref_count: 1,
            plaintext_size,
        })
    }

    pub async fn get(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<ProtectedBlob, ProtectedBlobError> {
        let row = self.fetch_scoped(scope, blob_ref, true).await?;
        let path = ciphertext_path(&self.root, &row.physical_digest)?;
        let envelope = fs::read(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                unavailable(blob_ref.clone())
            } else {
                ProtectedBlobError::io(&path, source)
            }
        })?;
        if blake3::hash(&envelope).to_hex().as_str() != row.physical_digest {
            return Err(corrupted(blob_ref.clone()));
        }
        let (version, nonce, ciphertext) = parse_envelope(&envelope, blob_ref)?;
        if version != row.key_version {
            return Err(corrupted(blob_ref.clone()));
        }
        let key = self
            .resolver
            .resolve(scope, version)
            .map_err(|_| unavailable(blob_ref.clone()))?;
        open_parsed_envelope(blob_ref, scope, version, nonce, ciphertext, &key)
    }

    pub async fn metadata(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<ProtectedBlobMetadata, ProtectedBlobError> {
        self.fetch_scoped(scope, blob_ref, false).await
    }

    pub async fn retain(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<u64, ProtectedBlobError> {
        let scope = scope.clone();
        let blob_ref = blob_ref.clone();
        self.database
            .call(move |connection| -> Result<u64, ProtectedBlobError> {
                let current = scoped_ref_count(connection, &scope, &blob_ref)?
                    .ok_or_else(|| unavailable(blob_ref.clone()))?;
                let next = current.saturating_add(1);
                connection.execute(
                    "UPDATE protected_blobs SET ref_count=?4, retain_until_ms=NULL
                     WHERE logical_ref=?1 AND provider_id=?2 AND session_id=?3 AND state=?5",
                    params![
                        blob_ref.as_str(),
                        scope.provider_id.as_str(),
                        scope.session_id.as_str(),
                        stored_i64(next),
                        STATE_READY,
                    ],
                )?;
                Ok(next)
            })
            .await?
    }

    pub async fn release(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
    ) -> Result<u64, ProtectedBlobError> {
        let scope = scope.clone();
        let blob_ref = blob_ref.clone();
        let retention_ms = self.retention_ms;
        self.database
            .call(move |connection| -> Result<u64, ProtectedBlobError> {
                let current = scoped_ref_count(connection, &scope, &blob_ref)?
                    .ok_or_else(|| unavailable(blob_ref.clone()))?;
                if current == 0 {
                    return Err(ProtectedBlobError::RefCountUnderflow { blob_ref });
                }
                let next = current - 1;
                let retain_until = (next == 0).then(|| now_ms().saturating_add(retention_ms));
                connection.execute(
                    "UPDATE protected_blobs SET ref_count=?4, retain_until_ms=?5
                     WHERE logical_ref=?1 AND provider_id=?2 AND session_id=?3 AND state=?6",
                    params![
                        blob_ref.as_str(),
                        scope.provider_id.as_str(),
                        scope.session_id.as_str(),
                        stored_i64(next),
                        retain_until.map(stored_i64),
                        STATE_READY,
                    ],
                )?;
                Ok(next)
            })
            .await?
    }

    pub async fn gc(&self) -> Result<GcReport, ProtectedBlobError> {
        let root = self.root.clone();
        self.database
            .call(move |connection| -> Result<GcReport, ProtectedBlobError> {
                let now = stored_i64(now_ms());
                let mut statement = connection.prepare(
                    "SELECT logical_ref, physical_digest, ciphertext_size, state
                     FROM protected_blobs
                     WHERE state=?2
                        OR (state=?3 AND ref_count=0
                            AND retain_until_ms IS NOT NULL AND retain_until_ms<=?1)",
                )?;
                let rows = statement
                    .query_map(params![now, STATE_DELETING, STATE_READY], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(statement);
                let mut report = GcReport::default();
                for (logical_ref, digest, size, state) in rows {
                    let blob_ref = ProtectedBlobRef::from(logical_ref.clone());
                    if state == STATE_READY {
                        let marked = connection.execute(
                            "UPDATE protected_blobs SET state=?2
                             WHERE logical_ref=?1 AND state=?3 AND ref_count=0
                               AND retain_until_ms IS NOT NULL AND retain_until_ms<=?4",
                            params![logical_ref, STATE_DELETING, STATE_READY, now],
                        )?;
                        if marked == 0 {
                            continue;
                        }
                    }
                    let path = ciphertext_path(&root, &digest)?;
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                        Err(source) => return Err(ProtectedBlobError::io(path, source)),
                    }
                    let deleted = connection.execute(
                        "DELETE FROM protected_blobs WHERE logical_ref=?1 AND state=?2",
                        params![logical_ref, STATE_DELETING],
                    )?;
                    if deleted == 1 {
                        report.deleted += 1;
                        report.reclaimed_bytes = report
                            .reclaimed_bytes
                            .saturating_add(stored_u64(size, &blob_ref)?);
                    }
                }
                Ok(report)
            })
            .await?
    }

    pub async fn shutdown(self) -> Result<(), ProtectedBlobError> {
        self.database.shutdown().await?;
        Ok(())
    }

    /// Finish or roll back file/metadata operations interrupted by a process crash.
    ///
    /// `pending` rows are never observable to callers and are removed with any file
    /// they managed to publish. `deleting` rows finish deletion. Finally, ciphertext
    /// files with no metadata owner (including interrupted legacy writes/rotations)
    /// are removed. The store is expected to have one owning host per root.
    async fn reconcile_incomplete_operations(&self) -> Result<(), ProtectedBlobError> {
        let root = self.root.clone();
        self.database
            .call(move |connection| -> Result<(), ProtectedBlobError> {
                let mut statement = connection.prepare(
                    "SELECT logical_ref, physical_digest, state FROM protected_blobs
                     WHERE state<>?1 ORDER BY logical_ref",
                )?;
                let incomplete = statement
                    .query_map(params![STATE_READY], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(statement);

                for (logical_ref, digest, state) in incomplete {
                    let path = ciphertext_path(&root, &digest)?;
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                        Err(source) => return Err(ProtectedBlobError::io(path, source)),
                    }
                    connection.execute(
                        "DELETE FROM protected_blobs WHERE logical_ref=?1 AND state=?2",
                        params![logical_ref, state],
                    )?;
                }

                let mut statement = connection
                    .prepare("SELECT physical_digest FROM protected_blobs WHERE state=?1")?;
                let known = statement
                    .query_map(params![STATE_READY], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<HashSet<_>>>()?;
                drop(statement);
                for path in collect_ciphertext_files(&root.join(BLOBS_DIR))? {
                    let owned = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| is_digest(name) && known.contains(name));
                    if owned {
                        continue;
                    }
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                        Err(source) => return Err(ProtectedBlobError::io(path, source)),
                    }
                }
                Ok(())
            })
            .await?
    }

    async fn fetch_scoped(
        &self,
        scope: &BlobScope,
        blob_ref: &ProtectedBlobRef,
        touch: bool,
    ) -> Result<ProtectedBlobMetadata, ProtectedBlobError> {
        let scope = scope.clone();
        let blob_ref = blob_ref.clone();
        self.database
            .call(
                move |connection| -> Result<ProtectedBlobMetadata, ProtectedBlobError> {
                    let row = connection
                        .query_row(
                            "SELECT physical_digest, key_version, plaintext_size, ciphertext_size,
                                ref_count, retain_until_ms
                         FROM protected_blobs
                         WHERE logical_ref=?1 AND provider_id=?2 AND session_id=?3 AND state=?4",
                            params![
                                blob_ref.as_str(),
                                scope.provider_id.as_str(),
                                scope.session_id.as_str(),
                                STATE_READY,
                            ],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, i64>(1)?,
                                    row.get::<_, i64>(2)?,
                                    row.get::<_, i64>(3)?,
                                    row.get::<_, i64>(4)?,
                                    row.get::<_, Option<i64>>(5)?,
                                ))
                            },
                        )
                        .optional()?
                        .ok_or_else(|| unavailable(blob_ref.clone()))?;
                    if touch {
                        connection.execute(
                            "UPDATE protected_blobs SET last_accessed_at_ms=?4
                         WHERE logical_ref=?1 AND provider_id=?2 AND session_id=?3 AND state=?5",
                            params![
                                blob_ref.as_str(),
                                scope.provider_id.as_str(),
                                scope.session_id.as_str(),
                                stored_i64(now_ms()),
                                STATE_READY,
                            ],
                        )?;
                    }
                    metadata_from_row(blob_ref, scope, row)
                },
            )
            .await?
    }
}

fn ensure_state_column(connection: &rusqlite::Connection) -> Result<(), ProtectedBlobError> {
    let mut statement = connection.prepare("PRAGMA table_info(protected_blobs)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    if !columns.iter().any(|column| column == "state") {
        connection.execute_batch(
            "ALTER TABLE protected_blobs
             ADD COLUMN state TEXT NOT NULL DEFAULT 'ready'
             CHECK(state IN ('pending', 'ready', 'deleting'));",
        )?;
    }
    Ok(())
}

type MetadataRow = (String, i64, i64, i64, i64, Option<i64>);

fn metadata_from_row(
    blob_ref: ProtectedBlobRef,
    scope: BlobScope,
    row: MetadataRow,
) -> Result<ProtectedBlobMetadata, ProtectedBlobError> {
    Ok(ProtectedBlobMetadata {
        physical_digest: row.0,
        key_version: u32::try_from(stored_u64(row.1, &blob_ref)?)
            .map_err(|_| corrupted(blob_ref.clone()))?,
        plaintext_size: stored_u64(row.2, &blob_ref)?,
        ciphertext_size: stored_u64(row.3, &blob_ref)?,
        ref_count: stored_u64(row.4, &blob_ref)?,
        retain_until_ms: row
            .5
            .map(|value| stored_u64(value, &blob_ref))
            .transpose()?,
        blob_ref,
        scope,
    })
}

fn scoped_ref_count(
    connection: &rusqlite::Connection,
    scope: &BlobScope,
    blob_ref: &ProtectedBlobRef,
) -> Result<Option<u64>, ProtectedBlobError> {
    let value = connection
        .query_row(
            "SELECT ref_count FROM protected_blobs
             WHERE logical_ref=?1 AND provider_id=?2 AND session_id=?3 AND state=?4",
            params![
                blob_ref.as_str(),
                scope.provider_id.as_str(),
                scope.session_id.as_str(),
                STATE_READY,
            ],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    value.map(|value| stored_u64(value, blob_ref)).transpose()
}

fn seal(
    scope: &BlobScope,
    blob_ref: &ProtectedBlobRef,
    version: KeyVersion,
    key: &AeadKey,
    plaintext: &[u8],
) -> Result<Vec<u8>, ()> {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    seal_with_nonce(scope, blob_ref, version, key, plaintext, nonce)
}

#[cfg(test)]
fn seal_for_test(
    scope: &BlobScope,
    blob_ref: &ProtectedBlobRef,
    version: KeyVersion,
    key: &AeadKey,
    plaintext: &[u8],
    nonce: [u8; NONCE_LEN],
) -> Result<Vec<u8>, ()> {
    seal_with_nonce(scope, blob_ref, version, key, plaintext, nonce)
}

fn seal_with_nonce(
    scope: &BlobScope,
    blob_ref: &ProtectedBlobRef,
    version: KeyVersion,
    key: &AeadKey,
    plaintext: &[u8],
    nonce: [u8; NONCE_LEN],
) -> Result<Vec<u8>, ()> {
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_bytes()).map_err(|_| ())?;
    let aad = aad(scope, blob_ref, version);
    let nonce = XNonce::from(nonce);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| ())?;
    let mut envelope = Vec::with_capacity(ENVELOPE_HEADER_LEN + ciphertext.len());
    envelope.extend_from_slice(ENVELOPE_MAGIC);
    envelope.push(ENVELOPE_VERSION);
    envelope.push(ALGORITHM_XCHACHA20_POLY1305);
    envelope.extend_from_slice(&version.to_be_bytes());
    envelope.extend_from_slice(nonce.as_slice());
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

/// Parse a PWB1 envelope. Bad magic / version / algorithm → [`ProtectedBlobError::ProtectedBlobCorrupted`].
pub fn parse_pwb1_envelope<'a>(
    envelope: &'a [u8],
    blob_ref: &ProtectedBlobRef,
) -> Result<(KeyVersion, &'a [u8], &'a [u8]), ProtectedBlobError> {
    parse_envelope(envelope, blob_ref)
}

/// Authenticated open of a raw PWB1 envelope with an explicit key.
pub fn open_pwb1_envelope(
    envelope: &[u8],
    scope: &BlobScope,
    blob_ref: &ProtectedBlobRef,
    key: &AeadKey,
) -> Result<ProtectedBlob, ProtectedBlobError> {
    let (version, nonce, ciphertext) = parse_envelope(envelope, blob_ref)?;
    open_parsed_envelope(blob_ref, scope, version, nonce, ciphertext, key)
}

/// AAD: `pawork.protected-blob.v1\0` + 长度前缀 provider/session/ref + key_version BE。
pub fn pwb1_aad(scope: &BlobScope, blob_ref: &ProtectedBlobRef, version: KeyVersion) -> Vec<u8> {
    aad(scope, blob_ref, version)
}

fn open_parsed_envelope(
    blob_ref: &ProtectedBlobRef,
    scope: &BlobScope,
    version: KeyVersion,
    nonce: &[u8],
    ciphertext: &[u8],
    key: &AeadKey,
) -> Result<ProtectedBlob, ProtectedBlobError> {
    let aad = aad(scope, blob_ref, version);
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_bytes())
        .map_err(|_| unavailable(blob_ref.clone()))?;
    let nonce = <&XNonce>::try_from(nonce).map_err(|_| corrupted(blob_ref.clone()))?;
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| corrupted(blob_ref.clone()))?;
    Ok(ProtectedBlob(Zeroizing::new(plaintext)))
}

fn parse_envelope<'a>(
    envelope: &'a [u8],
    blob_ref: &ProtectedBlobRef,
) -> Result<(KeyVersion, &'a [u8], &'a [u8]), ProtectedBlobError> {
    if envelope.len() <= ENVELOPE_HEADER_LEN
        || &envelope[..4] != ENVELOPE_MAGIC
        || envelope[4] != ENVELOPE_VERSION
        || envelope[5] != ALGORITHM_XCHACHA20_POLY1305
    {
        return Err(corrupted(blob_ref.clone()));
    }
    let version = u32::from_be_bytes(
        envelope[6..10]
            .try_into()
            .map_err(|_| corrupted(blob_ref.clone()))?,
    );
    Ok((
        version,
        &envelope[10..10 + NONCE_LEN],
        &envelope[ENVELOPE_HEADER_LEN..],
    ))
}

fn aad(scope: &BlobScope, blob_ref: &ProtectedBlobRef, version: KeyVersion) -> Vec<u8> {
    let mut value = b"pawork.protected-blob.v1\0".to_vec();
    push_len_prefixed(&mut value, scope.provider_id.as_str().as_bytes());
    push_len_prefixed(&mut value, scope.session_id.as_str().as_bytes());
    push_len_prefixed(&mut value, blob_ref.as_str().as_bytes());
    value.extend_from_slice(&version.to_be_bytes());
    value
}

fn push_len_prefixed(target: &mut Vec<u8>, value: &[u8]) {
    let length = u32::try_from(value.len()).unwrap_or(u32::MAX);
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(value);
}

fn random_blob_ref() -> ProtectedBlobRef {
    let mut random = [0u8; 32];
    OsRng.fill_bytes(&mut random);
    ProtectedBlobRef::from(format!("pblob_{}", blake3::hash(&random).to_hex()))
}

fn ciphertext_path(root: &Path, digest: &str) -> Result<PathBuf, ProtectedBlobError> {
    if !is_digest(digest) {
        return Err(corrupted(ProtectedBlobRef::from("invalid-digest")));
    }
    Ok(root
        .join(BLOBS_DIR)
        .join(&digest[..2])
        .join(&digest[2..4])
        .join(digest))
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<(), ProtectedBlobError> {
    let parent = path.parent().expect("ciphertext path has parent");
    fs::create_dir_all(parent).map_err(|source| ProtectedBlobError::io(parent, source))?;
    let mut random = [0u8; 8];
    OsRng.fill_bytes(&mut random);
    let temp = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        u64::from_le_bytes(random)
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|source| ProtectedBlobError::io(&temp, source))?;
        file.write_all(content)
            .map_err(|source| ProtectedBlobError::io(&temp, source))?;
        file.sync_all()
            .map_err(|source| ProtectedBlobError::io(&temp, source))?;
        fs::rename(&temp, path).map_err(|source| ProtectedBlobError::io(path, source))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn collect_ciphertext_files(root: &Path) -> Result<Vec<PathBuf>, ProtectedBlobError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|source| ProtectedBlobError::io(&directory, source))?;
        for entry in entries {
            let entry = entry.map_err(|source| ProtectedBlobError::io(&directory, source))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|source| ProtectedBlobError::io(&path, source))?;
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn unavailable(blob_ref: ProtectedBlobRef) -> ProtectedBlobError {
    ProtectedBlobError::ProtectedBlobUnavailable { blob_ref }
}

fn corrupted(blob_ref: ProtectedBlobRef) -> ProtectedBlobError {
    ProtectedBlobError::ProtectedBlobCorrupted { blob_ref }
}

fn stored_u64(value: i64, blob_ref: &ProtectedBlobRef) -> Result<u64, ProtectedBlobError> {
    u64::try_from(value).map_err(|_| corrupted(blob_ref.clone()))
}

fn stored_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pawork-protected-{}-{}-{name}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn scope(provider: &str, session: &str) -> BlobScope {
        BlobScope::new(ProviderId::from(provider), SessionId::from(session))
    }

    fn resolver_with(scope: &BlobScope, version: KeyVersion, byte: u8) -> Arc<InMemoryKeyResolver> {
        let resolver = Arc::new(InMemoryKeyResolver::new());
        resolver.insert(scope.clone(), version, AeadKey::new([byte; 32]));
        resolver.set_current(scope.clone(), version);
        resolver
    }

    fn options(root: &Path) -> ProtectedBlobStoreOptions {
        let mut options = ProtectedBlobStoreOptions::new(root);
        options.retention = Duration::ZERO;
        options
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn encrypted_round_trip_restart_and_randomized_addressing() {
        let root = temp_root("lifecycle");
        let scope = scope("provider-a", "session-a");
        let resolver = resolver_with(&scope, 1, 7);
        let secret = b"reasoning-secret-that-must-never-appear-on-disk";
        let first;
        let second;
        {
            let store = ProtectedBlobStore::open_with_options(options(&root), resolver.clone())
                .await
                .expect("open");
            first = store.put(&scope, secret).await.expect("put first");
            second = store.put(&scope, secret).await.expect("put second");
            assert_ne!(first.blob_ref, second.blob_ref);
            let first_meta = store
                .metadata(&scope, &first.blob_ref)
                .await
                .expect("metadata first");
            let second_meta = store
                .metadata(&scope, &second.blob_ref)
                .await
                .expect("metadata second");
            assert_ne!(first_meta.physical_digest, second_meta.physical_digest);
            for meta in [&first_meta, &second_meta] {
                let bytes = fs::read(ciphertext_path(&root, &meta.physical_digest).unwrap())
                    .expect("ciphertext");
                assert!(!bytes.windows(secret.len()).any(|window| window == secret));
            }
            store.shutdown().await.expect("shutdown");
        }
        let store = ProtectedBlobStore::open_with_options(options(&root), resolver)
            .await
            .expect("reopen");
        assert_eq!(
            store.get(&scope, &first.blob_ref).await.unwrap().expose(),
            secret
        );
        assert_eq!(
            store
                .metadata(&scope, &first.blob_ref)
                .await
                .unwrap()
                .ref_count,
            1
        );
        store.shutdown().await.unwrap();
        cleanup(&root);
    }

    #[tokio::test]
    async fn scope_isolation_uses_distinct_keys_and_fails_closed() {
        let root = temp_root("scope");
        let a = scope("provider-a", "session-a");
        let b = scope("provider-a", "session-b");
        let resolver = Arc::new(InMemoryKeyResolver::new());
        resolver.insert(a.clone(), 1, AeadKey::new([1; 32]));
        resolver.insert(b.clone(), 1, AeadKey::new([2; 32]));
        resolver.set_current(a.clone(), 1);
        resolver.set_current(b.clone(), 1);
        let store = ProtectedBlobStore::open_with_options(options(&root), resolver)
            .await
            .unwrap();
        let a_ref = store.put(&a, b"a-secret").await.unwrap().blob_ref;
        let b_ref = store.put(&b, b"b-secret").await.unwrap().blob_ref;
        assert_eq!(store.get(&a, &a_ref).await.unwrap().expose(), b"a-secret");
        assert_eq!(store.get(&b, &b_ref).await.unwrap().expose(), b"b-secret");
        let error = store.get(&b, &a_ref).await.expect_err("cross scope");
        assert!(error.is_unavailable());
        assert!(!format!("{error:?}").contains("a-secret"));
        store.shutdown().await.unwrap();
        cleanup(&root);
    }

    #[tokio::test]
    async fn ciphertext_tampering_and_row_swapping_are_corrupted() {
        let root = temp_root("corruption");
        let scope = scope("provider", "session");
        let resolver = resolver_with(&scope, 1, 3);
        let store = ProtectedBlobStore::open_with_options(options(&root), resolver)
            .await
            .unwrap();
        let first = store.put(&scope, b"first-secret").await.unwrap().blob_ref;
        let second = store.put(&scope, b"second-secret").await.unwrap().blob_ref;
        let first_meta = store.metadata(&scope, &first).await.unwrap();
        let second_meta = store.metadata(&scope, &second).await.unwrap();

        store
            .database
            .call({
                let first = first.clone();
                let second = second.clone();
                let first_digest = first_meta.physical_digest.clone();
                let second_digest = second_meta.physical_digest.clone();
                move |connection| {
                    connection.execute(
                        "UPDATE protected_blobs SET physical_digest='ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff' WHERE logical_ref=?1",
                        params![first.as_str()],
                    )?;
                    connection.execute(
                        "UPDATE protected_blobs SET physical_digest=?2 WHERE logical_ref=?1",
                        params![second.as_str(), first_digest],
                    )?;
                    connection.execute(
                        "UPDATE protected_blobs SET physical_digest=?2 WHERE logical_ref=?1",
                        params![first.as_str(), second_digest],
                    )
                }
            })
            .await
            .expect("actor")
            .expect("swap rows");
        assert!(store.get(&scope, &first).await.unwrap_err().is_corrupted());

        let current = store.metadata(&scope, &first).await.unwrap();
        let path = ciphertext_path(&root, &current.physical_digest).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x80;
        fs::write(&path, bytes).unwrap();
        assert!(store.get(&scope, &first).await.unwrap_err().is_corrupted());
        store.shutdown().await.unwrap();
        cleanup(&root);
    }

    #[tokio::test]
    async fn gc_waits_for_zero_ref_and_retention() {
        let root = temp_root("gc");
        let scope = scope("provider", "session");
        let resolver = resolver_with(&scope, 1, 6);
        let store = ProtectedBlobStore::open_with_options(options(&root), resolver)
            .await
            .unwrap();
        let blob_ref = store.put(&scope, b"gc-secret").await.unwrap().blob_ref;
        assert_eq!(store.gc().await.unwrap().deleted, 0);
        assert_eq!(store.release(&scope, &blob_ref).await.unwrap(), 0);
        let report = store.gc().await.unwrap();
        assert_eq!(report.deleted, 1);
        assert!(store
            .get(&scope, &blob_ref)
            .await
            .unwrap_err()
            .is_unavailable());
        store.shutdown().await.unwrap();
        cleanup(&root);
    }

    #[tokio::test]
    async fn open_reconciles_pending_rows_and_orphan_ciphertext() {
        let root = temp_root("pending-reconcile");
        let scope = scope("provider", "session");
        let resolver = resolver_with(&scope, 1, 8);
        let store = ProtectedBlobStore::open_with_options(options(&root), resolver.clone())
            .await
            .unwrap();
        let with_file = store.put(&scope, b"pending-with-file").await.unwrap();
        let without_file = store.put(&scope, b"pending-without-file").await.unwrap();
        let with_file_meta = store.metadata(&scope, &with_file.blob_ref).await.unwrap();
        let without_file_meta = store
            .metadata(&scope, &without_file.blob_ref)
            .await
            .unwrap();
        let with_file_path = ciphertext_path(&root, &with_file_meta.physical_digest).unwrap();
        let without_file_path = ciphertext_path(&root, &without_file_meta.physical_digest).unwrap();
        store
            .database
            .call({
                let first = with_file.blob_ref.clone();
                let second = without_file.blob_ref.clone();
                move |connection| {
                    connection.execute(
                        "UPDATE protected_blobs SET state=?2 WHERE logical_ref=?1",
                        params![first.as_str(), STATE_PENDING],
                    )?;
                    connection.execute(
                        "UPDATE protected_blobs SET state=?2 WHERE logical_ref=?1",
                        params![second.as_str(), STATE_PENDING],
                    )
                }
            })
            .await
            .unwrap()
            .unwrap();
        fs::remove_file(&without_file_path).unwrap();

        let orphan_digest = "a".repeat(64);
        let orphan_path = ciphertext_path(&root, &orphan_digest).unwrap();
        fs::create_dir_all(orphan_path.parent().unwrap()).unwrap();
        fs::write(&orphan_path, b"orphan-ciphertext").unwrap();
        store.shutdown().await.unwrap();

        let store = ProtectedBlobStore::open_with_options(options(&root), resolver)
            .await
            .unwrap();
        for blob_ref in [&with_file.blob_ref, &without_file.blob_ref] {
            assert!(store
                .metadata(&scope, blob_ref)
                .await
                .unwrap_err()
                .is_unavailable());
        }
        assert!(!with_file_path.exists());
        assert!(!without_file_path.exists());
        assert!(!orphan_path.exists());
        store.shutdown().await.unwrap();
        cleanup(&root);
    }

    #[tokio::test]
    async fn open_finishes_interrupted_deletion() {
        let root = temp_root("deleting-reconcile");
        let scope = scope("provider", "session");
        let resolver = resolver_with(&scope, 1, 10);
        let store = ProtectedBlobStore::open_with_options(options(&root), resolver.clone())
            .await
            .unwrap();
        let blob_ref = store
            .put(&scope, b"delete-after-crash")
            .await
            .unwrap()
            .blob_ref;
        let metadata = store.metadata(&scope, &blob_ref).await.unwrap();
        let path = ciphertext_path(&root, &metadata.physical_digest).unwrap();
        assert_eq!(store.release(&scope, &blob_ref).await.unwrap(), 0);
        store
            .database
            .call({
                let blob_ref = blob_ref.clone();
                move |connection| {
                    connection.execute(
                        "UPDATE protected_blobs SET state=?2 WHERE logical_ref=?1",
                        params![blob_ref.as_str(), STATE_DELETING],
                    )
                }
            })
            .await
            .unwrap()
            .unwrap();
        store.shutdown().await.unwrap();

        let store = ProtectedBlobStore::open_with_options(options(&root), resolver)
            .await
            .unwrap();
        assert!(store
            .metadata(&scope, &blob_ref)
            .await
            .unwrap_err()
            .is_unavailable());
        assert!(!path.exists());
        store.shutdown().await.unwrap();
        cleanup(&root);
    }

    #[tokio::test]
    async fn existing_table_without_state_column_is_upgraded() {
        let root = temp_root("state-column-upgrade");
        fs::create_dir_all(&root).unwrap();
        let database_path = root.join(DATABASE_FILE);
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE protected_blobs (
                    logical_ref TEXT PRIMARY KEY,
                    provider_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    physical_digest TEXT NOT NULL UNIQUE CHECK(length(physical_digest) = 64),
                    key_version INTEGER NOT NULL CHECK(key_version >= 0),
                    plaintext_size INTEGER NOT NULL CHECK(plaintext_size >= 0),
                    ciphertext_size INTEGER NOT NULL CHECK(ciphertext_size >= 0),
                    ref_count INTEGER NOT NULL CHECK(ref_count >= 0),
                    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
                    last_accessed_at_ms INTEGER NOT NULL CHECK(last_accessed_at_ms >= 0),
                    retain_until_ms INTEGER
                );",
            )
            .unwrap();
        drop(connection);

        let scope = scope("provider", "session");
        let resolver = resolver_with(&scope, 1, 11);
        let store = ProtectedBlobStore::open_with_options(options(&root), resolver)
            .await
            .unwrap();
        let blob_ref = store.put(&scope, b"post-upgrade").await.unwrap().blob_ref;
        assert_eq!(
            store.get(&scope, &blob_ref).await.unwrap().expose(),
            b"post-upgrade"
        );
        let state = store
            .database
            .call({
                let blob_ref = blob_ref.clone();
                move |connection| {
                    connection.query_row(
                        "SELECT state FROM protected_blobs WHERE logical_ref=?1",
                        params![blob_ref.as_str()],
                        |row| row.get::<_, String>(0),
                    )
                }
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state, STATE_READY);
        store.shutdown().await.unwrap();
        cleanup(&root);
    }

    #[tokio::test]
    async fn missing_key_is_unavailable_and_debug_is_redacted() {
        let root = temp_root("missing-key");
        let scope = scope("provider", "session");
        let resolver = resolver_with(&scope, 1, 9);
        let store = ProtectedBlobStore::open_with_options(options(&root), resolver.clone())
            .await
            .unwrap();
        let blob_ref = store
            .put(&scope, b"missing-key-secret")
            .await
            .unwrap()
            .blob_ref;
        resolver.remove(&scope, 1);
        let error = store.get(&scope, &blob_ref).await.unwrap_err();
        assert!(error.is_unavailable());
        assert_eq!(
            format!("{:?}", AeadKey::new([42; 32])),
            "AeadKey([REDACTED])"
        );
        assert_eq!(
            format!("{:?}", ProtectedBlob(Zeroizing::new(b"secret".to_vec()))),
            "ProtectedBlob([REDACTED])"
        );
        store.shutdown().await.unwrap();
        cleanup(&root);
    }

    #[test]
    fn seal_for_test_matches_pwb1_golden_hex() {
        let hex: String = include_str!("../tests/golden/pwb1_valid.hex")
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        let golden: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex"))
            .collect();
        let scope = scope("provider-golden", "session-golden");
        let blob_ref = ProtectedBlobRef::from("pblob_golden");
        let key = AeadKey::new([0x11; 32]);
        let nonce = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
        ];
        let sealed = seal_for_test(
            &scope,
            &blob_ref,
            1,
            &key,
            b"reasoning-secret-that-must-never-appear-on-disk",
            nonce,
        )
        .expect("seal");
        assert_eq!(sealed, golden);
        let opened = open_pwb1_envelope(&sealed, &scope, &blob_ref, &key).expect("open");
        assert_eq!(
            opened.expose(),
            b"reasoning-secret-that-must-never-appear-on-disk"
        );
    }
}
