//! 内容寻址 Blob Store（P1-6，ADR-004 / ADR-018）。
//!
//! 大型内容（Tool Output、Diff、文件快照等）以 BLAKE3 哈希寻址写入
//! `<root>/blobs/ab/cd/<hash>`，size / 引用计数 / 时间戳经
//! [`crate::sqlite::DatabaseActor`] 持久化到 SQLite。所有会改变状态的操作
//! （`put` / `release` / `gc`）都在 Actor 专用线程上一次性完成文件系统与数据库
//! 两步变更，引用计数与磁盘预算因此在并发下保持一致。
//!
//! 关键不变量：
//!
//! - 相同内容只落盘一份；重复 `put` 仅增加引用计数（去重）。
//! - `release` 防下溢：引用计数已为零或 blob 不存在时返回错误，绝不变负。
//! - 读取时重算 BLAKE3 哈希，检测缺失与损坏。
//! - `gc` 删除引用计数为零的 blob，永不触碰有引用的内容；并在安全延迟 +
//!   哈希校验后回收磁盘有、数据库无记录的 final 孤儿（崩溃窗口残留）。
//! - 磁盘预算不足时安全报错，不删除任何已存储 blob。

use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::sqlite::{DatabaseActor, DatabaseError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

const SCHEMA_VERSION: u32 = 1;
const BLOBS_DIR: &str = "blobs";
const DATABASE_FILE: &str = "artifacts.sqlite3";
/// 崩溃残留的 `.tmp-` 写入临时文件超过该年龄后，由 `gc` 回收。
const TMP_ORPHAN_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// rename 成功但 DB 未落账的 final blob 超过该年龄后，由 `gc` 回收。
const FINAL_ORPHAN_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

const SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS artifact_blobs (
        hash TEXT PRIMARY KEY CHECK (length(hash) = 64),
        size INTEGER NOT NULL CHECK (size >= 0),
        ref_count INTEGER NOT NULL CHECK (ref_count >= 0),
        created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
        last_accessed_at_ms INTEGER NOT NULL CHECK (last_accessed_at_ms >= 0)
    );
    CREATE INDEX IF NOT EXISTS idx_artifact_blobs_gc ON artifact_blobs(ref_count);
";

/// 内容标识：BLAKE3 哈希的 64 字符小写十六进制。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobId(String);

impl BlobId {
    pub fn from_hash(hash: blake3::Hash) -> Self {
        Self(hash.to_hex().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 还原为 BLAKE3 哈希。`BlobId` 构造时已校验，这里不会失败。
    pub fn to_hash(&self) -> blake3::Hash {
        blake3::Hash::from_hex(&self.0).expect("BlobId is always valid hex")
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BlobId {
    type Err = ArtifactStoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        blake3::Hash::from_hex(value)
            .map(|hash| Self(hash.to_hex().to_string()))
            .map_err(|_| ArtifactStoreError::InvalidBlobId(value.to_string()))
    }
}

// 序列化为 64 字符 hex 字符串（与 checkpoint-service 的字符串绕道格式一致），
// 反序列化时复用 `FromStr` 校验，非法 hex 报错而非构造出无效 `BlobId`。
impl Serialize for BlobId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BlobId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(D::Error::custom)
    }
}

/// [`ArtifactStore::open_with_options`] 的打开选项。
#[derive(Clone, Debug)]
pub struct ArtifactStoreOptions {
    /// 存储根目录，`blobs/` 与 `artifacts.sqlite3` 位于其下。
    pub root: PathBuf,
    /// 允许的全部 blob 总字节上限；`None` 表示不限制。
    pub disk_budget: Option<u64>,
}

impl ArtifactStoreOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            disk_budget: None,
        }
    }
}

/// `put` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutOutcome {
    pub id: BlobId,
    /// 本次调用是否新写入了 blob（false 表示命中去重，仅增加引用）。
    pub created: bool,
    /// 变更后的引用计数。
    pub ref_count: u64,
}

/// 单个 blob 的持久化元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobMetadata {
    pub id: BlobId,
    pub size: u64,
    pub ref_count: u64,
    pub created_at_ms: i64,
    pub last_accessed_at_ms: i64,
}

/// `gc` 的回收结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    /// 删除的零引用 blob 数量。
    pub deleted: u64,
    /// 回收的总字节数。
    pub reclaimed_bytes: u64,
    /// 清理的过期 `.tmp-` 孤儿文件数量。
    pub deleted_tmp_orphans: u64,
    /// 清理的过期 final 孤儿（磁盘有、数据库无记录）数量。
    pub deleted_final_orphans: u64,
}

/// `integrity_check` 的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// 校验过的数据库记录数。
    pub checked: u64,
    /// 数据库有记录但磁盘缺失的 blob。
    pub missing: Vec<BlobId>,
    /// 磁盘内容与 BLAKE3 哈希不符的 blob。
    pub corrupted: Vec<BlobId>,
    /// 磁盘存在但数据库无记录的游离文件。
    pub orphans: Vec<PathBuf>,
}

impl IntegrityReport {
    pub fn is_ok(&self) -> bool {
        self.missing.is_empty() && self.corrupted.is_empty() && self.orphans.is_empty()
    }
}

/// 内容寻址 Blob Store。
///
/// 克隆只复制句柄；实际状态由 SQLite Actor 与磁盘文件承载。
#[derive(Clone)]
pub struct ArtifactStore {
    database: DatabaseActor,
    root: PathBuf,
    disk_budget: Option<u64>,
}

impl ArtifactStore {
    /// 以默认选项（无磁盘预算）打开。
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self, ArtifactStoreError> {
        Self::open_with_options(ArtifactStoreOptions::new(root)).await
    }

    /// 打开（必要时创建）存储：`<root>/blobs/` 目录与元数据数据库。
    pub async fn open_with_options(
        options: ArtifactStoreOptions,
    ) -> Result<Self, ArtifactStoreError> {
        let blobs_dir = options.root.join(BLOBS_DIR);
        fs::create_dir_all(&blobs_dir).map_err(|source| ArtifactStoreError::Io {
            source,
            path: blobs_dir.clone(),
        })?;
        let database = DatabaseActor::open(options.root.join(DATABASE_FILE)).await?;
        database
            .call(|connection| -> Result<u32, ArtifactStoreError> {
                connection.execute_batch(SCHEMA_SQL)?;
                Ok(SCHEMA_VERSION)
            })
            .await??;
        Ok(Self {
            database,
            root: options.root,
            disk_budget: options.disk_budget,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn disk_budget(&self) -> Option<u64> {
        self.disk_budget
    }

    pub fn database(&self) -> &DatabaseActor {
        &self.database
    }

    /// blob 在磁盘上的路径：`<root>/blobs/ab/cd/<hash>`。
    pub fn blob_path(&self, id: &BlobId) -> PathBuf {
        blob_path(&self.root, id)
    }

    /// 写入内容并增加一个引用；相同内容去重，仅增加引用计数。
    pub async fn put(&self, content: &[u8]) -> Result<PutOutcome, ArtifactStoreError> {
        let content = content.to_vec();
        let root = self.root.clone();
        let budget = self.disk_budget;
        let (id, created, ref_count) = self
            .database
            .call(
                move |connection| -> Result<(BlobId, bool, i64), ArtifactStoreError> {
                    let hash = blake3::hash(&content);
                    let id = BlobId::from_hash(hash);
                    let now = now_ms();
                    if let Some((_, ref_count)) = fetch_row(connection, &id)? {
                        let path = blob_path(&root, &id);
                        let healthy = match fs::read(&path) {
                            Ok(existing) => blake3::hash(&existing) == hash,
                            Err(source) if source.kind() == io::ErrorKind::NotFound => false,
                            Err(source) => {
                                return Err(ArtifactStoreError::Io { source, path });
                            }
                        };
                        if !healthy {
                            atomic_write(&path, &content)?;
                        }
                        connection.execute(
                            "UPDATE artifact_blobs \
                         SET ref_count = ref_count + 1, last_accessed_at_ms = ?2 \
                         WHERE hash = ?1",
                            params![id.as_str(), now],
                        )?;
                        return Ok((id, false, ref_count + 1));
                    }
                    let usage = total_usage(connection)?;
                    if let Some(budget) = budget {
                        let projected = i128::from(usage) + content.len() as i128;
                        if projected > i128::from(budget) {
                            return Err(ArtifactStoreError::DiskBudgetExceeded {
                                usage: usage.max(0) as u64,
                                requested: content.len() as u64,
                                budget,
                            });
                        }
                    }
                    let path = blob_path(&root, &id);
                    if path.exists() {
                        // 磁盘已有同哈希文件（如上次写入后数据库未落账）：校验后直接采纳。
                        let existing =
                            fs::read(&path).map_err(|source| ArtifactStoreError::Io {
                                source,
                                path: path.clone(),
                            })?;
                        if blake3::hash(&existing) != hash {
                            atomic_write(&path, &content)?;
                        }
                    } else {
                        atomic_write(&path, &content)?;
                    }
                    connection.execute(
                        "INSERT INTO artifact_blobs \
                     (hash, size, ref_count, created_at_ms, last_accessed_at_ms) \
                     VALUES (?1, ?2, 1, ?3, ?3)",
                        params![id.as_str(), content.len() as i64, now],
                    )?;
                    Ok((id, true, 1))
                },
            )
            .await??;
        Ok(PutOutcome {
            id,
            created,
            ref_count: ref_count.max(0) as u64,
        })
    }

    /// 读取 blob 内容，读取时重算 BLAKE3 哈希校验完整性。
    pub async fn get(&self, id: &BlobId) -> Result<Vec<u8>, ArtifactStoreError> {
        let id_for_actor = id.clone();
        let row = self
            .database
            .call(
                move |connection| -> Result<Option<(i64, i64)>, ArtifactStoreError> {
                    let row = fetch_row(connection, &id_for_actor)?;
                    if row.is_some() {
                        connection.execute(
                            "UPDATE artifact_blobs SET last_accessed_at_ms = ?2 WHERE hash = ?1",
                            params![id_for_actor.as_str(), now_ms()],
                        )?;
                    }
                    Ok(row)
                },
            )
            .await??;
        if row.is_none() {
            return Err(ArtifactStoreError::UnknownBlob { id: id.clone() });
        }
        let path = self.blob_path(id);
        let data = fs::read(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ArtifactStoreError::BlobMissing {
                    id: id.clone(),
                    path: path.clone(),
                }
            } else {
                ArtifactStoreError::Io {
                    source,
                    path: path.clone(),
                }
            }
        })?;
        let actual = blake3::hash(&data);
        let expected = id.to_hash();
        if actual != expected {
            return Err(ArtifactStoreError::BlobCorrupted {
                id: id.clone(),
                expected: expected.to_hex().to_string(),
                actual: actual.to_hex().to_string(),
            });
        }
        Ok(data)
    }

    /// 按 `[offset, offset + limit)` 读取 blob 的一部分，完整性校验与
    /// [`Self::get`] 相同（读取时重算 BLAKE3 哈希，检测缺失与损坏）。
    ///
    /// 错误语义（结构化可区分）：
    ///
    /// - blob 不存在：[`ArtifactStoreError::UnknownBlob`]；
    /// - `limit == 0`（空范围）：[`ArtifactStoreError::EmptyRange`]；
    /// - `offset > size`（offset 超尾）：[`ArtifactStoreError::RangeOffsetOutOfBounds`]；
    /// - `offset == size && limit > 0`：返回空 `Vec`，作为分片循环的自然终止。
    ///
    /// 分片读取（如 100k 行 diff 按 ≤64KiB chunk 切分）时循环推进
    /// `offset += chunk.len()`，直到返回空切片或错误。
    pub async fn read_range(
        &self,
        id: &BlobId,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<u8>, ArtifactStoreError> {
        let id_for_actor = id.clone();
        let size = self
            .database
            .call(
                move |connection| -> Result<Option<i64>, ArtifactStoreError> {
                    let row = fetch_row(connection, &id_for_actor)?;
                    if row.is_some() {
                        connection.execute(
                            "UPDATE artifact_blobs SET last_accessed_at_ms = ?2 WHERE hash = ?1",
                            params![id_for_actor.as_str(), now_ms()],
                        )?;
                    }
                    Ok(row.map(|(size, _)| size))
                },
            )
            .await??;
        let Some(size) = size else {
            return Err(ArtifactStoreError::UnknownBlob { id: id.clone() });
        };
        let size = to_stored_u64(size, "size")?;
        if limit == 0 {
            return Err(ArtifactStoreError::EmptyRange {
                id: id.clone(),
                offset,
                limit,
            });
        }
        if offset > size {
            return Err(ArtifactStoreError::RangeOffsetOutOfBounds {
                id: id.clone(),
                offset,
                size,
            });
        }
        let path = self.blob_path(id);
        let data = fs::read(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ArtifactStoreError::BlobMissing {
                    id: id.clone(),
                    path: path.clone(),
                }
            } else {
                ArtifactStoreError::Io {
                    source,
                    path: path.clone(),
                }
            }
        })?;
        let actual = blake3::hash(&data);
        let expected = id.to_hash();
        if actual != expected {
            return Err(ArtifactStoreError::BlobCorrupted {
                id: id.clone(),
                expected: expected.to_hex().to_string(),
                actual: actual.to_hex().to_string(),
            });
        }
        let start = offset as usize;
        let end = offset.saturating_add(limit).min(size) as usize;
        Ok(data[start..end].to_vec())
    }

    /// 查询 blob 的字节长度（复用 [`Self::metadata`] 的 `size`）。
    pub async fn byte_length(&self, id: &BlobId) -> Result<u64, ArtifactStoreError> {
        Ok(self.metadata(id).await?.size)
    }

    /// 释放一个引用，返回剩余引用计数。
    ///
    /// blob 不存在返回 [`ArtifactStoreError::UnknownBlob`]；引用计数已为零返回
    /// [`ArtifactStoreError::RefCountUnderflow`]，计数绝不降到负数。
    pub async fn release(&self, id: &BlobId) -> Result<u64, ArtifactStoreError> {
        let id_for_actor = id.clone();
        let remaining = self
            .database
            .call(move |connection| -> Result<u64, ArtifactStoreError> {
                let Some((_, ref_count)) = fetch_row(connection, &id_for_actor)? else {
                    return Err(ArtifactStoreError::UnknownBlob { id: id_for_actor });
                };
                if ref_count <= 0 {
                    return Err(ArtifactStoreError::RefCountUnderflow { id: id_for_actor });
                }
                connection.execute(
                    "UPDATE artifact_blobs SET ref_count = ref_count - 1 WHERE hash = ?1",
                    params![id_for_actor.as_str()],
                )?;
                Ok((ref_count - 1) as u64)
            })
            .await??;
        Ok(remaining)
    }

    /// 查询 blob 元数据。
    pub async fn metadata(&self, id: &BlobId) -> Result<BlobMetadata, ArtifactStoreError> {
        let id_for_actor = id.clone();
        let (size, ref_count, created_at_ms, last_accessed_at_ms) = self
            .database
            .call(
                move |connection| -> Result<(i64, i64, i64, i64), ArtifactStoreError> {
                    let row = connection
                        .query_row(
                            "SELECT size, ref_count, created_at_ms, last_accessed_at_ms \
                         FROM artifact_blobs WHERE hash = ?1",
                            params![id_for_actor.as_str()],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)?,
                                    row.get::<_, i64>(1)?,
                                    row.get::<_, i64>(2)?,
                                    row.get::<_, i64>(3)?,
                                ))
                            },
                        )
                        .optional()?;
                    row.ok_or(ArtifactStoreError::UnknownBlob { id: id_for_actor })
                },
            )
            .await??;
        Ok(BlobMetadata {
            id: id.clone(),
            size: to_stored_u64(size, "size")?,
            ref_count: to_stored_u64(ref_count, "ref_count")?,
            created_at_ms,
            last_accessed_at_ms,
        })
    }

    /// 当前全部 blob 占用的总字节数。
    pub async fn disk_usage(&self) -> Result<u64, ArtifactStoreError> {
        let usage = self.database.call(total_usage).await??;
        Ok(usage.max(0) as u64)
    }

    /// 完整性校验：逐条核对数据库记录与磁盘文件，并扫描游离文件。
    pub async fn integrity_check(&self) -> Result<IntegrityReport, ArtifactStoreError> {
        let rows = self
            .database
            .call(|connection| -> Result<Vec<String>, ArtifactStoreError> {
                let mut statement =
                    connection.prepare("SELECT hash FROM artifact_blobs ORDER BY hash")?;
                let hashes = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(hashes)
            })
            .await??;
        let mut report = IntegrityReport::default();
        let mut known = HashSet::<String>::new();
        for hash in rows {
            let id = BlobId::from_str(&hash)?;
            known.insert(hash);
            report.checked += 1;
            let path = blob_path(&self.root, &id);
            let data = match fs::read(&path) {
                Ok(data) => data,
                Err(source) if source.kind() == io::ErrorKind::NotFound => {
                    report.missing.push(id);
                    continue;
                }
                Err(source) => {
                    return Err(ArtifactStoreError::Io { source, path });
                }
            };
            if blake3::hash(&data) != id.to_hash() {
                report.corrupted.push(id);
            }
        }
        let mut files = Vec::new();
        collect_files(&self.root.join(BLOBS_DIR), &mut files).map_err(|source| {
            ArtifactStoreError::Io {
                source,
                path: self.root.join(BLOBS_DIR),
            }
        })?;
        for file in files {
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if !known.contains(name) {
                report.orphans.push(file);
            }
        }
        Ok(report)
    }

    /// 回收引用计数为零的 blob；有引用 / 有数据库记录的 blob 一律不触碰。
    ///
    /// 同时清理 `blobs/` 下：
    /// - mtime 超过 24h 的 `.tmp-` 崩溃残留；
    /// - mtime 超过 24h、内容哈希与文件名一致、且数据库无记录的 final 孤儿
    ///   （rename 成功后 INSERT 前崩溃留下的内容寻址文件）。
    pub async fn gc(&self) -> Result<GcReport, ArtifactStoreError> {
        let root = self.root.clone();
        let mut report = self
            .database
            .call(move |connection| -> Result<GcReport, ArtifactStoreError> {
                let mut statement = connection
                    .prepare("SELECT hash, size FROM artifact_blobs WHERE ref_count = 0")?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(statement);
                let mut report = GcReport::default();
                for (hash, size) in rows {
                    let id = BlobId::from_str(&hash)?;
                    let path = blob_path(&root, &id);
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                        Err(source) => return Err(ArtifactStoreError::Io { source, path }),
                    }
                    connection
                        .execute("DELETE FROM artifact_blobs WHERE hash = ?1", params![hash])?;
                    report.deleted += 1;
                    report.reclaimed_bytes += size.max(0) as u64;
                }
                let mut known_statement =
                    connection.prepare("SELECT hash FROM artifact_blobs")?;
                let known = known_statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<HashSet<_>>>()?;
                drop(known_statement);
                let (final_orphans, final_bytes) =
                    reclaim_stale_final_orphans(&root.join(BLOBS_DIR), &known, FINAL_ORPHAN_MAX_AGE)?;
                report.deleted_final_orphans = final_orphans;
                report.reclaimed_bytes += final_bytes;
                Ok(report)
            })
            .await??;
        report.deleted_tmp_orphans =
            clean_stale_tmp_orphans(&self.root.join(BLOBS_DIR), TMP_ORPHAN_MAX_AGE)?;
        Ok(report)
    }

    /// 显式关闭数据库 Actor。
    pub async fn shutdown(self) -> Result<(), ArtifactStoreError> {
        self.database.shutdown().await?;
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ArtifactStoreError {
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O error at {path}: {source}")]
    Io {
        #[source]
        source: io::Error,
        path: PathBuf,
    },
    #[error("invalid blob id: {0}")]
    InvalidBlobId(String),
    #[error("unknown blob {id}")]
    UnknownBlob { id: BlobId },
    #[error("blob {id} reference count is already zero")]
    RefCountUnderflow { id: BlobId },
    #[error("blob {id} is missing from disk (expected at {path})")]
    BlobMissing { id: BlobId, path: PathBuf },
    #[error("read range of blob {id} at offset {offset} with limit {limit} is empty (limit must be > 0)")]
    EmptyRange { id: BlobId, offset: u64, limit: u64 },
    #[error("read range offset {offset} is beyond blob {id} size {size}")]
    RangeOffsetOutOfBounds { id: BlobId, offset: u64, size: u64 },
    #[error("blob {id} failed BLAKE3 verification: expected {expected}, actual {actual}")]
    BlobCorrupted {
        id: BlobId,
        expected: String,
        actual: String,
    },
    #[error(
        "disk budget exceeded: current usage {usage} + requested {requested} > budget {budget}"
    )]
    DiskBudgetExceeded {
        usage: u64,
        requested: u64,
        budget: u64,
    },
    #[error("stored {field} value is out of range: {value}")]
    InvalidStoredValue { field: &'static str, value: i64 },
}

fn blob_path(root: &Path, id: &BlobId) -> PathBuf {
    let hash = id.as_str();
    root.join(BLOBS_DIR)
        .join(&hash[..2])
        .join(&hash[2..4])
        .join(hash)
}

fn fetch_row(
    connection: &mut Connection,
    id: &BlobId,
) -> Result<Option<(i64, i64)>, ArtifactStoreError> {
    let row = connection
        .query_row(
            "SELECT size, ref_count FROM artifact_blobs WHERE hash = ?1",
            params![id.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    Ok(row)
}

fn total_usage(connection: &mut Connection) -> Result<i64, ArtifactStoreError> {
    let usage = connection.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM artifact_blobs",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(usage)
}

fn to_stored_u64(value: i64, field: &'static str) -> Result<u64, ArtifactStoreError> {
    u64::try_from(value).map_err(|_| ArtifactStoreError::InvalidStoredValue { field, value })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 原子写：同目录临时文件写入并同步后 rename 到目标路径。
fn atomic_write(path: &Path, content: &[u8]) -> Result<(), ArtifactStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ArtifactStoreError::Io {
            source,
            path: parent.to_path_buf(),
        })?;
    }
    let temp_path = path.with_file_name(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.map_err(|source| ArtifactStoreError::Io {
        source,
        path: path.to_path_buf(),
    })
}

/// 递归收集目录下的普通文件，跳过 `.tmp-` 前缀的写入中临时文件。
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(&path, out)?;
        } else {
            let is_temp = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".tmp-"));
            if !is_temp {
                out.push(path);
            }
        }
    }
    Ok(())
}

/// 回收「磁盘有、数据库无」且已过安全延迟、内容哈希与文件名一致的 final blob。
fn reclaim_stale_final_orphans(
    dir: &Path,
    known: &HashSet<String>,
    max_age: Duration,
) -> Result<(u64, u64), ArtifactStoreError> {
    let mut files = Vec::new();
    collect_files(dir, &mut files).map_err(|source| ArtifactStoreError::Io {
        source,
        path: dir.to_path_buf(),
    })?;
    let now = SystemTime::now();
    let mut deleted = 0u64;
    let mut reclaimed_bytes = 0u64;
    for path in files {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if known.contains(name) {
            continue;
        }
        let Ok(id) = BlobId::from_str(name) else {
            continue;
        };
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(ArtifactStoreError::Io {
                    source,
                    path: path.clone(),
                });
            }
        };
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(source) => {
                return Err(ArtifactStoreError::Io {
                    source,
                    path: path.clone(),
                });
            }
        };
        let age = match now.duration_since(modified) {
            Ok(age) => age,
            Err(_) => continue,
        };
        if age <= max_age {
            continue;
        }
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(ArtifactStoreError::Io {
                    source,
                    path: path.clone(),
                });
            }
        };
        if blake3::hash(&data) != id.to_hash() {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => {
                deleted += 1;
                reclaimed_bytes += data.len() as u64;
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ArtifactStoreError::Io { source, path });
            }
        }
    }
    Ok((deleted, reclaimed_bytes))
}

/// 清理 mtime 超过阈值的 `.tmp-` 崩溃残留文件。
fn clean_stale_tmp_orphans(dir: &Path, max_age: Duration) -> Result<u64, ArtifactStoreError> {
    let mut deleted = 0u64;
    let mut files = Vec::new();
    collect_tmp_files(dir, &mut files).map_err(|source| ArtifactStoreError::Io {
        source,
        path: dir.to_path_buf(),
    })?;
    let now = SystemTime::now();
    for path in files {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(ArtifactStoreError::Io {
                    source,
                    path: path.clone(),
                });
            }
        };
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(source) => {
                return Err(ArtifactStoreError::Io {
                    source,
                    path: path.clone(),
                });
            }
        };
        let age = match now.duration_since(modified) {
            Ok(age) => age,
            Err(_) => continue, // 未来时间戳：跳过，避免误删进行中写入
        };
        if age <= max_age {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => deleted += 1,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ArtifactStoreError::Io { source, path });
            }
        }
    }
    Ok(deleted)
}

/// 递归收集 `.tmp-` 前缀的临时文件。
fn collect_tmp_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_tmp_files(&path, out)?;
        } else {
            let is_temp = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".tmp-"));
            if is_temp {
                out.push(path);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-artifact-store-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    fn unknown_id(content: &[u8]) -> BlobId {
        BlobId::from_hash(blake3::hash(content))
    }

    #[tokio::test]
    async fn put_deduplicates_identical_content() {
        let root = temp_root("dedup");
        let store = ArtifactStore::open(&root).await.expect("open store");
        let content = b"deduplicate me";
        let first = store.put(content).await.expect("first put");
        let second = store.put(content).await.expect("second put");
        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.id, second.id);
        assert_eq!(second.ref_count, 2);
        let metadata = store.metadata(&first.id).await.expect("metadata");
        assert_eq!(metadata.ref_count, 2);
        assert_eq!(metadata.size, content.len() as u64);
        // 磁盘只有一份内容，且路径符合 blobs/ab/cd/<hash> 布局。
        let path = store.blob_path(&first.id);
        let hash = first.id.as_str();
        assert_eq!(
            path,
            root.join(BLOBS_DIR)
                .join(&hash[..2])
                .join(&hash[2..4])
                .join(hash)
        );
        assert!(path.exists());
        let mut files = Vec::new();
        collect_files(&root.join(BLOBS_DIR), &mut files).expect("walk");
        assert_eq!(files, vec![path]);
        assert_eq!(
            store.disk_usage().await.expect("usage"),
            content.len() as u64
        );
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn ref_counts_survive_restart() {
        let root = temp_root("restart");
        let content = b"persistent reference counting";
        let id = {
            let store = ArtifactStore::open(&root).await.expect("open store");
            let first = store.put(content).await.expect("first put");
            let second = store.put(content).await.expect("second put");
            assert_eq!(second.ref_count, 2);
            store.shutdown().await.expect("shutdown");
            first.id
        };
        let store = ArtifactStore::open(&root).await.expect("reopen store");
        let metadata = store.metadata(&id).await.expect("metadata after reopen");
        assert_eq!(metadata.ref_count, 2);
        assert_eq!(store.get(&id).await.expect("get after reopen"), content);
        assert_eq!(store.release(&id).await.expect("release"), 1);
        store.shutdown().await.expect("shutdown");

        let store = ArtifactStore::open(&root).await.expect("reopen again");
        let metadata = store.metadata(&id).await.expect("metadata");
        assert_eq!(metadata.ref_count, 1);
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn get_detects_corruption_and_integrity_reports_it() {
        let root = temp_root("corruption");
        let store = ArtifactStore::open(&root).await.expect("open store");
        let id = store.put(b"pristine content").await.expect("put").id;
        let path = store.blob_path(&id);
        fs::write(&path, b"tampered").expect("tamper blob");
        let error = store.get(&id).await.expect_err("corrupted read must fail");
        assert!(matches!(error, ArtifactStoreError::BlobCorrupted { .. }));
        let report = store.integrity_check().await.expect("integrity");
        assert_eq!(report.checked, 1);
        assert_eq!(report.corrupted, vec![id.clone()]);
        assert!(report.missing.is_empty());
        assert!(report.orphans.is_empty());
        assert!(!report.is_ok());
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn duplicate_put_repairs_a_corrupted_existing_blob() {
        let root = temp_root("repair-corruption");
        let store = ArtifactStore::open(&root).await.expect("open store");
        let content = b"self-healing content";
        let first = store.put(content).await.expect("put");
        fs::write(store.blob_path(&first.id), b"corrupt").expect("corrupt blob");

        let repaired = store.put(content).await.expect("duplicate put repairs");
        assert!(!repaired.created);
        assert_eq!(repaired.ref_count, 2);
        assert_eq!(store.get(&first.id).await.expect("get repaired"), content);
        assert!(store.integrity_check().await.expect("integrity").is_ok());
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn integrity_reports_missing_and_orphan_files() {
        let root = temp_root("integrity");
        let store = ArtifactStore::open(&root).await.expect("open store");
        let keep = store.put(b"kept").await.expect("put keep").id;
        let gone = store.put(b"gone").await.expect("put gone").id;
        fs::remove_file(store.blob_path(&gone)).expect("remove blob");
        let orphan_path = root
            .join(BLOBS_DIR)
            .join("ff")
            .join("ee")
            .join("f".repeat(64));
        fs::create_dir_all(orphan_path.parent().expect("parent")).expect("mkdir");
        fs::write(&orphan_path, b"stray").expect("write orphan");

        let report = store.integrity_check().await.expect("integrity");
        assert_eq!(report.checked, 2);
        assert_eq!(report.missing, vec![gone.clone()]);
        assert!(report.corrupted.is_empty());
        assert_eq!(report.orphans, vec![orphan_path]);
        assert!(!report.is_ok());
        // 有引用 blob 的读取不受影响。
        assert_eq!(store.get(&keep).await.expect("get keep"), b"kept");
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn gc_only_removes_zero_reference_blobs() {
        let root = temp_root("gc");
        let store = ArtifactStore::open(&root).await.expect("open store");
        let referenced = store.put(b"referenced blob").await.expect("put").id;
        let garbage = store.put(b"garbage blob").await.expect("put").id;
        assert_eq!(store.release(&garbage).await.expect("release"), 0);

        let report = store.gc().await.expect("gc");
        assert_eq!(report.deleted, 1);
        assert_eq!(report.reclaimed_bytes, b"garbage blob".len() as u64);
        assert_eq!(report.deleted_tmp_orphans, 0);
        assert!(!store.blob_path(&garbage).exists());
        assert!(store.blob_path(&referenced).exists());
        assert_eq!(
            store.get(&referenced).await.expect("referenced readable"),
            b"referenced blob"
        );
        // 再次 GC 无可回收内容。
        let second = store.gc().await.expect("second gc");
        assert_eq!(second, GcReport::default());
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn gc_cleans_stale_tmp_orphans_but_keeps_fresh_ones() {
        let root = temp_root("gc-tmp");
        let store = ArtifactStore::open(&root).await.expect("open store");
        let referenced = store.put(b"keep me").await.expect("put").id;
        let blob_dir = store
            .blob_path(&referenced)
            .parent()
            .expect("blob dir")
            .to_path_buf();

        let stale = blob_dir.join(".tmp-stale-orphan");
        let fresh = blob_dir.join(".tmp-fresh-orphan");
        fs::write(&stale, b"stale temp").expect("write stale tmp");
        fs::write(&fresh, b"fresh temp").expect("write fresh tmp");

        let stale_mtime = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
        let file = fs::File::options()
            .write(true)
            .open(&stale)
            .expect("open stale tmp");
        file.set_modified(stale_mtime).expect("set stale mtime");
        drop(file);

        let report = store.gc().await.expect("gc");
        assert_eq!(report.deleted, 0);
        assert_eq!(report.deleted_tmp_orphans, 1);
        assert_eq!(report.deleted_final_orphans, 0);
        assert!(!stale.exists(), "stale .tmp- orphan must be reclaimed");
        assert!(fresh.exists(), "fresh .tmp- must survive 24h threshold");
        assert!(store.blob_path(&referenced).exists());
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn gc_reclaims_stale_final_orphans_after_rename_without_db_row() {
        let root = temp_root("gc-final-orphan");
        let store = ArtifactStore::open(&root).await.expect("open store");
        let kept = store.put(b"has db row").await.expect("put kept").id;

        let orphan_content = b"rename succeeded but db insert never ran";
        let orphan_id = BlobId::from_hash(blake3::hash(orphan_content));
        let orphan_path = store.blob_path(&orphan_id);
        fs::create_dir_all(orphan_path.parent().expect("parent")).expect("mkdir");
        fs::write(&orphan_path, orphan_content).expect("simulate rename");
        let stale_mtime = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(&orphan_path)
            .expect("open orphan")
            .set_modified(stale_mtime)
            .expect("backdate orphan");

        let fresh_content = b"fresh crash-window blob";
        let fresh_id = BlobId::from_hash(blake3::hash(fresh_content));
        let fresh_path = store.blob_path(&fresh_id);
        fs::create_dir_all(fresh_path.parent().expect("parent")).expect("mkdir");
        fs::write(&fresh_path, fresh_content).expect("write fresh orphan");

        fs::File::options()
            .write(true)
            .open(store.blob_path(&kept))
            .expect("open kept")
            .set_modified(stale_mtime)
            .expect("backdate kept");

        let report = store.gc().await.expect("gc");
        assert_eq!(report.deleted, 0);
        assert_eq!(report.deleted_final_orphans, 1);
        assert_eq!(report.reclaimed_bytes, orphan_content.len() as u64);
        assert!(!orphan_path.exists(), "stale final orphan must be reclaimed");
        assert!(
            fresh_path.exists(),
            "fresh final orphan must survive safety delay"
        );
        assert!(store.blob_path(&kept).exists());
        assert_eq!(store.get(&kept).await.expect("kept readable"), b"has db row");

        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn release_never_underflows() {
        let root = temp_root("underflow");
        let store = ArtifactStore::open(&root).await.expect("open store");
        let id = store.put(b"single ref").await.expect("put").id;
        assert_eq!(store.release(&id).await.expect("release"), 0);
        let error = store.release(&id).await.expect_err("underflow must fail");
        assert!(matches!(
            error,
            ArtifactStoreError::RefCountUnderflow { .. }
        ));
        let metadata = store.metadata(&id).await.expect("metadata");
        assert_eq!(metadata.ref_count, 0);
        let unknown = unknown_id(b"never stored");
        let error = store.release(&unknown).await.expect_err("unknown release");
        assert!(matches!(error, ArtifactStoreError::UnknownBlob { .. }));
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }

    #[tokio::test]
    async fn disk_budget_rejects_oversized_put_safely() {
        let root = temp_root("budget");
        let options = ArtifactStoreOptions {
            root: root.clone(),
            disk_budget: Some(32),
        };
        let store = ArtifactStore::open_with_options(options)
            .await
            .expect("open store");
        assert_eq!(store.disk_budget(), Some(32));

        let first = store.put(&[1u8; 16]).await.expect("first put fits");
        assert!(first.created);

        let error = store
            .put(&[2u8; 24])
            .await
            .expect_err("oversized put must fail");
        let exceeded = match error {
            ArtifactStoreError::DiskBudgetExceeded {
                usage,
                requested,
                budget,
            } => (usage, requested, budget),
            other => panic!("unexpected error: {other:?}"),
        };
        assert_eq!(exceeded, (16, 24, 32));
        // 失败的写入不产生残留文件，也不影响已有 blob。
        assert!(!store.blob_path(&unknown_id(&[2u8; 24])).exists());
        assert_eq!(store.disk_usage().await.expect("usage"), 16);
        assert_eq!(store.get(&first.id).await.expect("get"), vec![1u8; 16]);

        // 恰好用满预算允许写入，超出 1 字节即拒绝。
        store.put(&[3u8; 16]).await.expect("fills budget");
        assert_eq!(store.disk_usage().await.expect("usage"), 32);
        let error = store
            .put(&[4u8; 1])
            .await
            .expect_err("over budget by one byte");
        assert!(matches!(
            error,
            ArtifactStoreError::DiskBudgetExceeded { .. }
        ));
        store.shutdown().await.expect("shutdown");
        cleanup(&root);
    }
}
