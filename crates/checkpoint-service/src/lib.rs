//! Pawork Checkpoint 与回滚（P4-11）。
//!
//! 基于 [`artifact_store::ArtifactStore`]（内容寻址 Blob Store）实现写操作的
//! 快照与按 tool call / run 粒度的回滚。本阶段不依赖 git-service（Phase 7 尚未
//! 实现），因此**绝不**执行 `git reset --hard`，仅靠 Blob 还原。
//!
//! ## 设计要点
//!
//! - `snapshot_before_write`：写前把将被改/删文件的当前内容存为 Blob（去重），
//!   记录 pre 内容 `BlobId` + unix 模式 + 是否存在，挂到该 tool_call 的 change 记录。
//! - `rollback_tool_call` / `rollback_run`：从 Blob 取回内容，同目录 tmp+sync+rename
//!   原子写恢复；删除新增文件；按逆序恢复。
//! - `conflict_check`：重读文件重算 BLAKE3，与 pre 哈希比对，不同即用户改过。
//! - 路径解析：`roots` 切片逐个 `join(relative_path)` + `canonicalize`，校验在某
//!   root 内，拒绝 `..` 穿越与绝对路径。
//!
//! ## 关于实现细节的两点偏离
//!
//! 1. `FileSnapshot` 字段类型与 brief 完全一致（`pre_blob: Option<BlobId>`），
//!    但 `artifact_store::BlobId` 未派生 serde（且本任务只能修改本 crate），故
//!    `Serialize`/`Deserialize` 以手写 impl 实现：blob 序列化为 hex 字符串。
//! 2. `state` 的锁用 `std::sync::Mutex` 而非 `tokio::sync::Mutex`：`list_changes`
//!    是同步访问器，tokio 锁无法在同步函数中安全获取；本 crate 从不跨 `.await`
//!    持锁（FS/Blob 操作均在锁外完成），故 `std::sync::Mutex` 语义正确且无死锁风险。

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use artifact_store::{ArtifactStore, BlobId};

use serde::{Deserialize, Serialize};

/// 单个文件在某次写操作前的快照。
///
/// 字段类型与 `CHECKPOINT_API.md` 一字不差。`pre_blob` / `pre_hash` 均为
/// `None` 表示写前该文件不存在（新增文件）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSnapshot {
    pub relative_path: String,
    pub existed: bool,
    pub pre_blob: Option<BlobId>,
    pub pre_hash: Option<String>,
    pub unix_mode: Option<u32>,
}

// 手写 serde：`BlobId` 不派生 Serialize/Deserialize，故按 hex 字符串收发。
impl Serialize for FileSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("FileSnapshot", 5)?;
        state.serialize_field("relative_path", &self.relative_path)?;
        state.serialize_field("existed", &self.existed)?;
        state.serialize_field("pre_blob", &self.pre_blob.as_ref().map(BlobId::as_str))?;
        state.serialize_field("pre_hash", &self.pre_hash)?;
        state.serialize_field("unix_mode", &self.unix_mode)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for FileSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        struct Raw {
            relative_path: String,
            existed: bool,
            pre_blob: Option<String>,
            pre_hash: Option<String>,
            unix_mode: Option<u32>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let pre_blob = match raw.pre_blob {
            Some(value) => {
                Some(BlobId::from_str(&value).map_err(|err| D::Error::custom(err.to_string()))?)
            }
            None => None,
        };
        Ok(FileSnapshot {
            relative_path: raw.relative_path,
            existed: raw.existed,
            pre_blob,
            pre_hash: raw.pre_hash,
            unix_mode: raw.unix_mode,
        })
    }
}

/// 单次 tool call 改动的一组文件快照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub tool_call_id: String,
    pub files: Vec<FileSnapshot>,
}

/// 一个 run 的完整 checkpoint。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCheckpoint {
    pub run_id: String,
    pub created_at_ms: u64,
    pub head: Option<String>,
    pub changes: Vec<ChangeRecord>,
}

/// `conflict_check` 的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictReport {
    pub relative_path: String,
    pub user_modified: bool,
}

/// Checkpoint / 回滚错误。
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("checkpoint not found: {0}")]
    NotFound(String),
    #[error("I/O error{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Store(#[from] artifact_store::ArtifactStoreError),
    #[error("relative path escapes workspace root: {0}")]
    PathEscape(String),
    #[error("relative path resolves outside all provided roots: {0}")]
    UnresolvedPath(String),
    #[error("invalid relative path: {0}")]
    InvalidRelativePath(String),
}

/// 基于 Blob Store 的 Checkpoint / 回滚服务。
///
/// 克隆只复制句柄；底层状态由 `ArtifactStore` 与内存表承载。
#[derive(Clone)]
pub struct CheckpointService {
    store: ArtifactStore,
    /// run_id -> checkpoint。`std::sync::Mutex`：从不跨 `.await` 持锁。
    state: Arc<Mutex<BTreeMap<String, RunCheckpoint>>>,
    /// tool_call_id -> { relative_path -> 写前解析出的绝对路径 }。
    ///
    /// 回滚 API 不再接收 `roots`，故在快照时记录解析结果，回滚时据此恢复。
    paths: Arc<Mutex<BTreeMap<String, BTreeMap<String, PathBuf>>>>,
}

impl CheckpointService {
    pub fn new(store: ArtifactStore) -> Self {
        Self {
            store,
            state: Arc::new(Mutex::new(BTreeMap::new())),
            paths: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// 幂等：确保 run 条目存在（`head = None`）。
    pub async fn snapshot_run(&self, run_id: &str) -> Result<(), CheckpointError> {
        let mut state = guard(&self.state);
        state
            .entry(run_id.to_string())
            .or_insert_with(|| RunCheckpoint {
                run_id: run_id.to_string(),
                created_at_ms: now_ms(),
                head: None,
                changes: Vec::new(),
            });
        Ok(())
    }

    /// 写前快照：读取当前内容（若存在）存 Blob，挂到该 tool_call 的 change 记录。
    ///
    /// 同一 tool_call 内已记录过的 `relative_path` 不重复记录，返回既有快照。
    pub async fn snapshot_before_write(
        &self,
        run_id: &str,
        tool_call_id: &str,
        roots: &[PathBuf],
        relative_path: &str,
    ) -> Result<FileSnapshot, CheckpointError> {
        let absolute = resolve_within_roots(roots, relative_path)?;

        let (existed, bytes, unix_mode) = match std::fs::read(&absolute) {
            Ok(bytes) => (true, bytes, read_unix_mode(&absolute)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                (false, Vec::new(), None)
            }
            Err(source) => {
                return Err(CheckpointError::Io {
                    context: format!(" while reading {}", absolute.display()),
                    source,
                });
            }
        };

        let (pre_blob, pre_hash) = if existed {
            let outcome = self.store.put(&bytes).await?;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            (Some(outcome.id), Some(hash))
        } else {
            (None, None)
        };

        let snapshot = FileSnapshot {
            relative_path: relative_path.to_string(),
            existed,
            pre_blob,
            pre_hash,
            unix_mode,
        };

        // 挂到 change 记录（去重）。锁内只做 BTreeMap 操作，不跨 await。
        {
            let mut state = guard(&self.state);
            let run = state
                .entry(run_id.to_string())
                .or_insert_with(|| RunCheckpoint {
                    run_id: run_id.to_string(),
                    created_at_ms: now_ms(),
                    head: None,
                    changes: Vec::new(),
                });
            let index = match run
                .changes
                .iter()
                .position(|change| change.tool_call_id == tool_call_id)
            {
                Some(index) => index,
                None => {
                    run.changes.push(ChangeRecord {
                        tool_call_id: tool_call_id.to_string(),
                        files: Vec::new(),
                    });
                    run.changes.len() - 1
                }
            };
            let change = &mut run.changes[index];
            if let Some(existing) = change
                .files
                .iter()
                .find(|file| file.relative_path == relative_path)
                .cloned()
            {
                tracing::debug!(
                    run_id = run_id,
                    tool_call_id = tool_call_id,
                    relative_path = relative_path,
                    "snapshot already recorded; reusing"
                );
                return Ok(existing);
            }
            change.files.push(snapshot.clone());
        }

        {
            let mut paths = guard(&self.paths);
            paths
                .entry(tool_call_id.to_string())
                .or_default()
                .insert(relative_path.to_string(), absolute);
        }

        tracing::debug!(
            run_id = run_id,
            tool_call_id = tool_call_id,
            relative_path = relative_path,
            existed,
            "snapshot recorded"
        );
        Ok(snapshot)
    }

    /// 从 Blob 恢复该 call 改过的所有文件，删除新增文件。返回被恢复的快照。
    pub async fn rollback_tool_call(
        &self,
        tool_call_id: &str,
    ) -> Result<Vec<FileSnapshot>, CheckpointError> {
        let (mut files, abs_map) = self.load_tool_call(tool_call_id)?;
        // 逆序恢复：后改的先还原，避免中间状态污染。
        files.reverse();
        let mut restored = Vec::with_capacity(files.len());
        for snapshot in &files {
            let absolute = abs_map.get(&snapshot.relative_path).map(PathBuf::as_path);
            restore_snapshot(&self.store, snapshot, absolute).await?;
            restored.push(snapshot.clone());
        }
        tracing::debug!(
            tool_call_id = tool_call_id,
            restored = restored.len(),
            "tool call rolled back"
        );
        restored.reverse();
        Ok(restored)
    }

    /// 恢复整个 run（按 tool_call 逆序）。
    pub async fn rollback_run(&self, run_id: &str) -> Result<Vec<FileSnapshot>, CheckpointError> {
        let mut tool_call_ids = {
            let state = guard(&self.state);
            let run = state.get(run_id).cloned();
            match run {
                Some(run) => run
                    .changes
                    .iter()
                    .map(|change| change.tool_call_id.clone())
                    .collect::<Vec<_>>(),
                None => return Err(CheckpointError::NotFound(format!("run {run_id}"))),
            }
        };
        tool_call_ids.reverse();

        let mut all = Vec::new();
        for tool_call_id in &tool_call_ids {
            let mut restored = self.rollback_tool_call(tool_call_id).await?;
            all.append(&mut restored);
        }
        Ok(all)
    }

    /// 冲突检测：重读文件重算 BLAKE3，与 pre 哈希比对。
    pub async fn conflict_check(
        &self,
        tool_call_id: &str,
        relative_path: &str,
    ) -> Result<ConflictReport, CheckpointError> {
        let snapshot = self.find_snapshot(tool_call_id, relative_path)?;
        let absolute = {
            let paths = guard(&self.paths);
            paths
                .get(tool_call_id)
                .and_then(|map| map.get(relative_path))
                .cloned()
        };

        let user_modified = match (snapshot.pre_hash.as_ref(), absolute.as_ref()) {
            (None, _) => false,
            (Some(pre_hash), Some(target)) => match std::fs::read(target) {
                Ok(bytes) => blake3::hash(&bytes).to_hex().to_string() != *pre_hash,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => true,
                Err(source) => {
                    return Err(CheckpointError::Io {
                        context: format!(" while reading {}", target.display()),
                        source,
                    });
                }
            },
            (Some(_), None) => false,
        };

        Ok(ConflictReport {
            relative_path: relative_path.to_string(),
            user_modified,
        })
    }

    /// 同步快照：返回某 run 的 checkpoint 克隆。
    pub fn list_changes(&self, run_id: &str) -> Option<RunCheckpoint> {
        let state = guard(&self.state);
        state.get(run_id).cloned()
    }

    fn load_tool_call(
        &self,
        tool_call_id: &str,
    ) -> Result<(Vec<FileSnapshot>, BTreeMap<String, PathBuf>), CheckpointError> {
        let files = {
            let state = guard(&self.state);
            let mut found = None;
            for run in state.values() {
                if let Some(change) = run
                    .changes
                    .iter()
                    .find(|change| change.tool_call_id == tool_call_id)
                {
                    found = Some(change.files.clone());
                    break;
                }
            }
            found
        };
        let files = match files {
            Some(files) => files,
            None => {
                return Err(CheckpointError::NotFound(format!(
                    "tool_call {tool_call_id}"
                )));
            }
        };
        let abs_map = {
            let paths = guard(&self.paths);
            paths.get(tool_call_id).cloned().unwrap_or_default()
        };
        Ok((files, abs_map))
    }

    fn find_snapshot(
        &self,
        tool_call_id: &str,
        relative_path: &str,
    ) -> Result<FileSnapshot, CheckpointError> {
        let state = guard(&self.state);
        for run in state.values() {
            if let Some(change) = run
                .changes
                .iter()
                .find(|change| change.tool_call_id == tool_call_id)
            {
                if let Some(file) = change
                    .files
                    .iter()
                    .find(|file| file.relative_path == relative_path)
                {
                    return Ok(file.clone());
                }
            }
        }
        Err(CheckpointError::NotFound(format!(
            "tool_call {tool_call_id} / {relative_path}"
        )))
    }
}

/// 在 `roots` 中解析 `relative_path`，返回首个命中的绝对路径。
///
/// 拒绝绝对路径与 `..` 穿越组件；对已存在文件 `canonicalize` 后二次校验仍在
/// root 内，以捕获指向 root 外的 symlink。新文件则取 `canon_root.join(rel)`。
fn resolve_within_roots(
    roots: &[PathBuf],
    relative_path: &str,
) -> Result<PathBuf, CheckpointError> {
    if relative_path.is_empty() {
        return Err(CheckpointError::InvalidRelativePath("empty".to_string()));
    }
    let relative = Path::new(relative_path);
    for component in relative.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(CheckpointError::PathEscape(format!(
                    "absolute component in {relative_path:?}"
                )));
            }
            Component::ParentDir => {
                return Err(CheckpointError::PathEscape(format!(
                    "parent traversal in {relative_path:?}"
                )));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }

    for root in roots {
        let canonical_root = match root.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        let candidate = canonical_root.join(relative_path);
        if !candidate.starts_with(&canonical_root) {
            continue;
        }
        let resolved = match candidate.canonicalize() {
            Ok(path) => path,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => candidate,
            Err(source) => {
                return Err(CheckpointError::Io {
                    context: format!(" while canonicalizing {}", candidate.display()),
                    source,
                });
            }
        };
        if !resolved.starts_with(&canonical_root) {
            continue;
        }
        return Ok(resolved);
    }

    Err(CheckpointError::UnresolvedPath(relative_path.to_string()))
}

/// 从 Blob 恢复单个文件快照。
async fn restore_snapshot(
    store: &ArtifactStore,
    snapshot: &FileSnapshot,
    absolute: Option<&Path>,
) -> Result<(), CheckpointError> {
    let Some(target) = absolute else {
        tracing::warn!(
            relative_path = %snapshot.relative_path,
            "rollback: no resolved path recorded, skipping"
        );
        return Ok(());
    };

    if snapshot.existed {
        if let Some(blob) = &snapshot.pre_blob {
            let content = store.get(blob).await?;
            atomic_write(target, &content)?;
        }
        #[cfg(unix)]
        if let Some(mode) = snapshot.unix_mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode));
        }
    } else {
        // 写前不存在的文件视为新增：回滚时删除。
        match std::fs::remove_file(target) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(CheckpointError::Io {
                    context: format!(" while removing {}", target.display()),
                    source,
                });
            }
        }
    }
    Ok(())
}

/// 原子写：同目录临时文件写入并 sync 后 rename 到目标（参考 artifact-store）。
fn atomic_write(path: &Path, content: &[u8]) -> Result<(), CheckpointError> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| CheckpointError::Io {
        context: format!(" while creating {}", parent.display()),
        source,
    })?;

    let temp_path = path.with_file_name(format!(
        ".ckpt-tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp_path, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result.map_err(|source| CheckpointError::Io {
        context: format!(" while writing {}", path.display()),
        source,
    })
}

#[cfg(unix)]
fn read_unix_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode())
}

#[cfg(not(unix))]
fn read_unix_mode(_path: &Path) -> Option<u32> {
    None
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 获取 `std::sync::Mutex` 的守卫，从中毒状态恢复（不使用 unwrap/expect）。
fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pawork-checkpoint-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    struct Harness {
        ws: PathBuf,
        store_dir: PathBuf,
        store: ArtifactStore,
    }

    impl Harness {
        async fn new(name: &str) -> Self {
            let ws = temp_root(&format!("ws-{name}"));
            let store_dir = temp_root(&format!("store-{name}"));
            std::fs::create_dir_all(&ws).expect("create ws");
            let store = ArtifactStore::open(&store_dir).await.expect("open store");
            Self {
                ws,
                store_dir,
                store,
            }
        }

        fn roots(&self) -> Vec<PathBuf> {
            vec![self.ws.clone()]
        }

        async fn shutdown(self) {
            let _ = self.store.shutdown().await;
            cleanup(&self.store_dir);
            cleanup(&self.ws);
        }
    }

    #[tokio::test]
    async fn rollback_restores_original_content() {
        let h = Harness::new("restore").await;
        let svc = CheckpointService::new(h.store.clone());
        let target = h.ws.join("file.txt");
        std::fs::write(&target, b"original").expect("write");

        let snap = svc
            .snapshot_before_write("run1", "tc1", &h.roots(), "file.txt")
            .await
            .expect("snapshot");
        assert!(snap.existed);
        assert!(snap.pre_blob.is_some());
        assert!(snap.pre_hash.is_some());

        std::fs::write(&target, b"CHANGED BY TOOL").expect("overwrite");
        let restored = svc.rollback_tool_call("tc1").await.expect("rollback");
        assert_eq!(restored.len(), 1);
        assert_eq!(std::fs::read(&target).expect("read"), b"original");
        h.shutdown().await;
    }

    #[tokio::test]
    async fn rollback_restores_deleted_file() {
        let h = Harness::new("delete").await;
        let svc = CheckpointService::new(h.store.clone());
        let target = h.ws.join("del.txt");
        std::fs::write(&target, b"keep me").expect("write");

        svc.snapshot_before_write("run", "tc", &h.roots(), "del.txt")
            .await
            .expect("snapshot");
        std::fs::remove_file(&target).expect("tool deletes");
        assert!(!target.exists());

        svc.rollback_tool_call("tc").await.expect("rollback");
        assert!(target.exists());
        assert_eq!(std::fs::read(&target).expect("read"), b"keep me");
        h.shutdown().await;
    }

    #[tokio::test]
    async fn rollback_removes_newly_created_file() {
        let h = Harness::new("new").await;
        let svc = CheckpointService::new(h.store.clone());
        let target = h.ws.join("new.txt");
        assert!(!target.exists());

        let snap = svc
            .snapshot_before_write("run", "tc", &h.roots(), "new.txt")
            .await
            .expect("snapshot");
        assert!(!snap.existed);
        assert!(snap.pre_blob.is_none());

        std::fs::write(&target, b"created").expect("tool creates");
        assert!(target.exists());

        svc.rollback_tool_call("tc").await.expect("rollback");
        assert!(!target.exists(), "new file should be removed on rollback");
        h.shutdown().await;
    }

    #[tokio::test]
    async fn rollback_removes_newly_created_nested_file() {
        let h = Harness::new("nested").await;
        let svc = CheckpointService::new(h.store.clone());
        let target = h.ws.join("sub").join("deep").join("new.txt");

        svc.snapshot_before_write("run", "tc", &h.roots(), "sub/deep/new.txt")
            .await
            .expect("snapshot");
        std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        std::fs::write(&target, b"nested").expect("create");
        assert!(target.exists());

        svc.rollback_tool_call("tc").await.expect("rollback");
        assert!(!target.exists());
        h.shutdown().await;
    }

    #[tokio::test]
    async fn conflict_check_detects_user_modification() {
        let h = Harness::new("conflict").await;
        let svc = CheckpointService::new(h.store.clone());
        let target = h.ws.join("f.txt");
        std::fs::write(&target, b"base").expect("write");

        svc.snapshot_before_write("run", "tc", &h.roots(), "f.txt")
            .await
            .expect("snapshot");

        let report = svc
            .conflict_check("tc", "f.txt")
            .await
            .expect("check unchanged");
        assert!(!report.user_modified);

        std::fs::write(&target, b"user edit").expect("user modifies");
        let report = svc
            .conflict_check("tc", "f.txt")
            .await
            .expect("check modified");
        assert!(report.user_modified);

        std::fs::remove_file(&target).expect("user deletes");
        let report = svc
            .conflict_check("tc", "f.txt")
            .await
            .expect("check deleted");
        assert!(report.user_modified, "deletion counts as modification");
        h.shutdown().await;
    }

    #[tokio::test]
    async fn conflict_check_for_new_file_is_not_modified() {
        let h = Harness::new("conflict-new").await;
        let svc = CheckpointService::new(h.store.clone());
        svc.snapshot_before_write("run", "tc", &h.roots(), "fresh.txt")
            .await
            .expect("snapshot");
        let report = svc.conflict_check("tc", "fresh.txt").await.expect("check");
        assert!(!report.user_modified);
        h.shutdown().await;
    }

    #[tokio::test]
    async fn rollback_run_restores_all_tool_calls_in_reverse() {
        let h = Harness::new("run").await;
        let svc = CheckpointService::new(h.store.clone());
        let a = h.ws.join("a.txt");
        let b = h.ws.join("b.txt");
        std::fs::write(&a, b"A0").expect("write a");
        std::fs::write(&b, b"B0").expect("write b");

        svc.snapshot_run("run").await.expect("snapshot_run");
        svc.snapshot_before_write("run", "tc1", &h.roots(), "a.txt")
            .await
            .expect("snap a");
        std::fs::write(&a, b"A1").expect("change a");
        svc.snapshot_before_write("run", "tc2", &h.roots(), "b.txt")
            .await
            .expect("snap b");
        std::fs::write(&b, b"B1").expect("change b");

        let restored = svc.rollback_run("run").await.expect("rollback_run");
        assert_eq!(restored.len(), 2);
        assert_eq!(std::fs::read(&a).expect("read a"), b"A0");
        assert_eq!(std::fs::read(&b).expect("read b"), b"B0");
        h.shutdown().await;
    }

    #[tokio::test]
    async fn rollback_run_missing_run_is_not_found() {
        let h = Harness::new("missing").await;
        let svc = CheckpointService::new(h.store.clone());
        let err = svc.rollback_run("nope").await.unwrap_err();
        assert!(matches!(err, CheckpointError::NotFound(_)));
        h.shutdown().await;
    }

    #[tokio::test]
    async fn snapshot_dedupes_same_path_in_call() {
        let h = Harness::new("dedupe").await;
        let svc = CheckpointService::new(h.store.clone());
        std::fs::write(h.ws.join("f.txt"), b"x").expect("write");

        let first = svc
            .snapshot_before_write("run", "tc", &h.roots(), "f.txt")
            .await
            .expect("first");
        let second = svc
            .snapshot_before_write("run", "tc", &h.roots(), "f.txt")
            .await
            .expect("second");
        assert_eq!(first, second);

        let cp = svc.list_changes("run").expect("list");
        assert_eq!(cp.changes.len(), 1);
        assert_eq!(cp.changes[0].files.len(), 1);
        h.shutdown().await;
    }

    #[tokio::test]
    async fn snapshot_run_is_idempotent_and_keeps_head_none() {
        let h = Harness::new("idem").await;
        let svc = CheckpointService::new(h.store.clone());
        svc.snapshot_run("run").await.expect("first");
        svc.snapshot_run("run").await.expect("second");
        let cp = svc.list_changes("run").expect("list");
        assert_eq!(cp.run_id, "run");
        assert_eq!(cp.head, None);
        assert!(cp.changes.is_empty());
        h.shutdown().await;
    }

    #[tokio::test]
    async fn rejects_parent_traversal_and_absolute_path() {
        let h = Harness::new("escape").await;
        let svc = CheckpointService::new(h.store.clone());

        let err = svc
            .snapshot_before_write("run", "tc", &h.roots(), "../escape.txt")
            .await
            .unwrap_err();
        assert!(matches!(err, CheckpointError::PathEscape(_)));

        let err = svc
            .snapshot_before_write("run", "tc", &h.roots(), "/etc/passwd")
            .await
            .unwrap_err();
        assert!(matches!(err, CheckpointError::PathEscape(_)));
        h.shutdown().await;
    }

    #[tokio::test]
    async fn unresolved_relative_path_outside_roots() {
        let h = Harness::new("unresolved").await;
        let svc = CheckpointService::new(h.store.clone());
        // 唯一 root 不存在时无法 canonicalize -> UnresolvedPath。
        let bogus_roots = vec![temp_root("does-not-exist")];
        let err = svc
            .snapshot_before_write("run", "tc", &bogus_roots, "f.txt")
            .await
            .unwrap_err();
        assert!(matches!(err, CheckpointError::UnresolvedPath(_)));
        h.shutdown().await;
    }

    #[tokio::test]
    async fn snapshots_serialize_round_trip() {
        let h = Harness::new("serde").await;
        let svc = CheckpointService::new(h.store.clone());
        std::fs::write(h.ws.join("f.txt"), b"hello").expect("write");

        let snap = svc
            .snapshot_before_write("run", "tc", &h.roots(), "f.txt")
            .await
            .expect("snapshot");
        let checkpoint = svc.list_changes("run").expect("list");

        let json = serde_json::to_string(&checkpoint).expect("serialize");
        let back: RunCheckpoint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, checkpoint);
        assert_eq!(back.changes[0].files[0].pre_blob, snap.pre_blob);
        assert_eq!(back.changes[0].files[0].pre_hash, snap.pre_hash);
        h.shutdown().await;
    }
}
