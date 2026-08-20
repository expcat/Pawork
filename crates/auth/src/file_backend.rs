//! 文件 Secret 后端（参照 Codex CLI auth.json 形态）。
//!
//! 单 JSON 文件（默认 ~/.pawork/auth.json，可用 PAWORK_HOME 覆盖），
//! 0600 权限、跨进程 write/refresh 锁、独立临时文件 + rename 原子写。keyspace
//! 与 OS 后端一致：(service, account) 二元组，上层（default_credential / resolve）
//! 无感知。
//! 损坏文件 fail-closed（报错，不静默清空）。

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backend::SecretBackend;
use crate::error::AuthError;

/// 文件格式版本（将来迁移用；当前只接受 1）。
const FORMAT_VERSION: u64 = 1;
const WRITE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Serialize, Deserialize)]
struct AuthFile {
    version: u64,
    /// service → account → 明文 secret。仅在写盘前的内存中短暂持有明文。
    entries: BTreeMap<String, BTreeMap<String, String>>,
}

/// 文件 Secret 后端。
pub struct FileBackend {
    path: PathBuf,
}

impl FileBackend {
    /// 默认路径：$PAWORK_HOME/auth.json，未设时 ~/.pawork/auth.json。
    pub fn new() -> Self {
        Self {
            path: default_auth_file_path(),
        }
    }

    /// 显式路径（测试用）。
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 存储文件路径（诊断用，不含 secret）。
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_lock_path(&self) -> PathBuf {
        self.path.with_extension("write.lock")
    }

    fn oauth_refresh_lock_path(&self) -> PathBuf {
        self.path.with_extension("refresh.lock")
    }

    fn load(&self) -> Result<BTreeMap<String, BTreeMap<String, String>>, AuthError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Default::default())
            }
            Err(error) => {
                return Err(AuthError::Storage(format!(
                    "read {}: {error}",
                    self.path.display()
                )))
            }
        };
        if bytes.is_empty() {
            return Ok(Default::default());
        }
        let file: AuthFile = serde_json::from_slice(&bytes).map_err(|error| {
            AuthError::Storage(format!(
                "parse {}: {error} (corrupt auth file; fail-closed)",
                self.path.display()
            ))
        })?;
        if file.version != FORMAT_VERSION {
            return Err(AuthError::Storage(format!(
                "unsupported auth file version {} at {} (expected {FORMAT_VERSION})",
                file.version,
                self.path.display()
            )));
        }
        Ok(file.entries)
    }

    fn save(&self, entries: &BTreeMap<String, BTreeMap<String, String>>) -> Result<(), AuthError> {
        let file = AuthFile {
            version: FORMAT_VERSION,
            entries: entries.clone(),
        };
        let json = serde_json::to_vec(&file)
            .map_err(|error| AuthError::Storage(format!("serialize auth file: {error}")))?;

        if let Some(parent) = non_empty_parent(&self.path) {
            std::fs::create_dir_all(parent).map_err(|error| {
                AuthError::Storage(format!("create {}: {error}", parent.display()))
            })?;
        }

        // 每次写入使用独立临时文件，避免多个进程共享固定 auth.json.tmp；最终
        // 仍以同目录 rename 原子替换，中途崩溃不会暴露半写 auth 文件。
        let tmp = loop {
            let candidate = unique_temp_path(&self.path);
            match write_new_file_0600(&candidate, &json) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(AuthError::Storage(format!(
                        "create {}: {error}",
                        candidate.display()
                    )))
                }
            }
        };
        std::fs::rename(&tmp, &self.path).map_err(|error| {
            let _ = std::fs::remove_file(&tmp);
            AuthError::Storage(format!(
                "rename {} -> {}: {error}",
                tmp.display(),
                self.path.display()
            ))
        })
    }
}

impl Default for FileBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretBackend for FileBackend {
    fn store(&self, service: &str, account: &str, secret: &str) -> Result<(), AuthError> {
        self.store_batch(&[(service, account, secret)])
    }

    fn store_batch(&self, updates: &[(&str, &str, &str)]) -> Result<(), AuthError> {
        let _guard = acquire_file_lock(&self.write_lock_path(), WRITE_LOCK_TIMEOUT)?;
        let mut entries = self.load()?;
        for &(service, account, secret) in updates {
            entries
                .entry(service.to_string())
                .or_default()
                .insert(account.to_string(), secret.to_string());
        }
        self.save(&entries)
    }

    fn get(&self, service: &str, account: &str) -> Result<String, AuthError> {
        self.load()?
            .get(service)
            .and_then(|accounts| accounts.get(account))
            .cloned()
            .ok_or(AuthError::NotFound)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), AuthError> {
        let _guard = acquire_file_lock(&self.write_lock_path(), WRITE_LOCK_TIMEOUT)?;
        let mut entries = self.load()?;
        let removed = entries
            .get_mut(service)
            .and_then(|accounts| accounts.remove(account));
        if removed.is_none() {
            return Err(AuthError::NotFound);
        }
        if entries
            .get(service)
            .is_some_and(|accounts| accounts.is_empty())
        {
            entries.remove(service);
        }
        self.save(&entries)
    }

    fn refresh_lock_path(&self) -> Option<PathBuf> {
        Some(self.oauth_refresh_lock_path())
    }
}

/// 持有平台原生独占锁；`File` 关闭（包括进程退出）即自动释放。
pub(crate) struct FileLockGuard {
    #[allow(dead_code)]
    file: File,
    #[cfg(not(any(unix, windows)))]
    path: PathBuf,
}

#[cfg(not(any(unix, windows)))]
impl Drop for FileLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) fn acquire_file_lock(
    path: &Path,
    timeout: Duration,
) -> Result<FileLockGuard, AuthError> {
    let started = Instant::now();
    loop {
        if let Some(guard) = try_acquire_file_lock(path)? {
            return Ok(guard);
        }
        if started.elapsed() >= timeout {
            return Err(AuthError::Storage(format!(
                "timed out waiting for file lock {}",
                path.display()
            )));
        }
        std::thread::sleep(LOCK_RETRY_DELAY);
    }
}

#[cfg(unix)]
pub(crate) fn try_acquire_file_lock(path: &Path) -> Result<Option<FileLockGuard>, AuthError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::raw::c_int;

    unsafe extern "C" {
        fn flock(fd: c_int, operation: c_int) -> c_int;
    }

    const LOCK_EX: c_int = 2;
    const LOCK_NB: c_int = 4;

    create_parent(path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| AuthError::Storage(format!("open {}: {error}", path.display())))?;

    // SAFETY: `file` owns a valid open fd for the duration of this call and, on success,
    // remains owned by FileLockGuard until the lock should be released.
    if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } == 0 {
        return Ok(Some(FileLockGuard { file }));
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    ) {
        return Ok(None);
    }
    Err(AuthError::Storage(format!(
        "lock {}: {error}",
        path.display()
    )))
}

#[cfg(windows)]
pub(crate) fn try_acquire_file_lock(path: &Path) -> Result<Option<FileLockGuard>, AuthError> {
    use std::os::windows::fs::OpenOptionsExt;

    create_parent(path)?;
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .share_mode(0)
        .open(path)
    {
        Ok(file) => Ok(Some(FileLockGuard { file })),
        Err(error) if matches!(error.raw_os_error(), Some(32) | Some(33)) => Ok(None),
        Err(error) => Err(AuthError::Storage(format!(
            "lock {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn try_acquire_file_lock(path: &Path) -> Result<Option<FileLockGuard>, AuthError> {
    create_parent(path)?;
    match OpenOptions::new().read(true).write(true).create_new(true).open(path) {
        Ok(file) => Ok(Some(FileLockGuard {
            file,
            path: path.to_path_buf(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(AuthError::Storage(format!(
            "lock {}: {error}",
            path.display()
        ))),
    }
}

fn create_parent(path: &Path) -> Result<(), AuthError> {
    if let Some(parent) = non_empty_parent(path) {
        std::fs::create_dir_all(parent)
            .map_err(|error| AuthError::Storage(format!("create {}: {error}", parent.display())))?;
    }
    Ok(())
}

fn non_empty_parent(path: &Path) -> Option<&Path> {
    path.parent().filter(|parent| !parent.as_os_str().is_empty())
}

fn unique_temp_path(path: &Path) -> PathBuf {
    let directory = non_empty_parent(path).unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("auth.json");
    let sequence = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ))
}

/// 默认 auth 文件路径：$PAWORK_HOME/auth.json 或 ~/.pawork/auth.json。
fn default_auth_file_path() -> PathBuf {
    resolve_auth_file_path(
        std::env::var_os("PAWORK_HOME").map(PathBuf::from),
        directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
    )
}

fn resolve_auth_file_path(pawork_home: Option<PathBuf>, base_home: Option<PathBuf>) -> PathBuf {
    if let Some(home) = pawork_home.filter(|home| !home.as_os_str().is_empty()) {
        return home.join("auth.json");
    }
    let home = base_home.unwrap_or_else(|| PathBuf::from("."));
    home.join(".pawork").join("auth.json")
}

#[cfg(unix)]
fn write_new_file_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
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
fn write_new_file_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
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

    #[test]
    #[cfg(target_os = "macos")]
    fn default_auth_file_path_macos_snapshot_uses_home_pawork_fallback() {
        // 快照 golden：directories 主版本升级不得改变 macOS home 兜底语义。
        let home = directories::BaseDirs::new()
            .expect("macOS home directory is available")
            .home_dir()
            .to_path_buf();
        let expected = home.join(".pawork").join("auth.json");
        assert_eq!(
            resolve_auth_file_path(None, Some(home)),
            expected,
            "macOS auth file home fallback snapshot changed"
        );
    }

    #[test]
    fn auth_file_path_prefers_non_empty_pawork_home() {
        assert_eq!(
            resolve_auth_file_path(
                Some(PathBuf::from("custom-home")),
                Some(PathBuf::from("base-home")),
            ),
            PathBuf::from("custom-home").join("auth.json")
        );
    }

    #[test]
    fn auth_file_path_missing_or_empty_override_uses_base_home() {
        let expected = PathBuf::from("base-home")
            .join(".pawork")
            .join("auth.json");
        assert_eq!(
            resolve_auth_file_path(None, Some(PathBuf::from("base-home"))),
            expected
        );
        assert_eq!(
            resolve_auth_file_path(Some(PathBuf::new()), Some(PathBuf::from("base-home"))),
            expected
        );
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pawork-file-backend-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("auth.json")
    }

    #[test]
    fn store_get_delete_roundtrip() {
        let path = temp_path("roundtrip");
        let backend = FileBackend::with_path(&path);
        backend
            .store("pawork.glm-coding", "default", "sk-test")
            .unwrap();
        assert_eq!(
            backend.get("pawork.glm-coding", "default").unwrap(),
            "sk-test"
        );
        backend.delete("pawork.glm-coding", "default").unwrap();
        assert!(matches!(
            backend.get("pawork.glm-coding", "default"),
            Err(AuthError::NotFound)
        ));
    }

    #[test]
    fn persists_across_instances() {
        let path = temp_path("persist");
        FileBackend::with_path(&path)
            .store("pawork.chatgpt.oauth", "default.access", "eyJ")
            .unwrap();
        let second = FileBackend::with_path(&path);
        assert_eq!(
            second
                .get("pawork.chatgpt.oauth", "default.access")
                .unwrap(),
            "eyJ"
        );
    }

    #[test]
    fn corrupt_file_fails_closed() {
        let path = temp_path("corrupt");
        std::fs::write(&path, b"{ not json").unwrap();
        let backend = FileBackend::with_path(&path);
        assert!(matches!(backend.get("any", "any"), Err(AuthError::Storage(_))));
    }

    #[cfg(unix)]
    #[test]
    fn file_permissions_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("perms");
        let backend = FileBackend::with_path(&path);
        backend.store("svc", "acct", "v").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn delete_last_account_removes_service() {
        let path = temp_path("cleanup");
        let backend = FileBackend::with_path(&path);
        backend.store("svc", "a", "1").unwrap();
        backend.delete("svc", "a").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("svc"),
            "empty service map should be dropped: {raw}"
        );
    }

    #[test]
    fn batch_store_persists_all_entries_in_one_file() {
        let path = temp_path("batch");
        let backend = FileBackend::with_path(&path);
        backend
            .store_batch(&[
                ("pawork.xai.oauth", "default.refresh", "refresh"),
                ("pawork.xai.oauth", "default.access", "access"),
                ("pawork.xai.oauth", "default.meta", "meta"),
            ])
            .expect("store batch");
        assert_eq!(
            backend
                .get("pawork.xai.oauth", "default.refresh")
                .expect("refresh"),
            "refresh"
        );
        assert_eq!(
            backend
                .get("pawork.xai.oauth", "default.access")
                .expect("access"),
            "access"
        );
        assert_eq!(
            backend
                .get("pawork.xai.oauth", "default.meta")
                .expect("meta"),
            "meta"
        );
    }

    #[test]
    fn native_file_lock_excludes_another_backend_instance() {
        let path = temp_path("native-lock").with_extension("write.lock");
        let first = try_acquire_file_lock(&path)
            .expect("first lock")
            .expect("first lock acquired");
        assert!(
            try_acquire_file_lock(&path)
                .expect("second lock attempt")
                .is_none(),
            "second instance must observe lock contention"
        );
        drop(first);
        assert!(
            try_acquire_file_lock(&path)
                .expect("lock after release")
                .is_some(),
            "closing the owner file must release the lock"
        );
    }

    #[test]
    fn concurrent_file_writers_preserve_every_entry() {
        const WRITERS: usize = 12;

        let path = temp_path("concurrent-writers");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(WRITERS));
        let mut threads = Vec::with_capacity(WRITERS);
        for index in 0..WRITERS {
            let path = path.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                let backend = FileBackend::with_path(path);
                barrier.wait();
                backend
                    .store("svc", &format!("account-{index}"), &format!("value-{index}"))
                    .expect("concurrent store");
            }));
        }
        for thread in threads {
            thread.join().expect("writer thread");
        }

        let backend = FileBackend::with_path(&path);
        for index in 0..WRITERS {
            assert_eq!(
                backend
                    .get("svc", &format!("account-{index}"))
                    .expect("preserved entry"),
                format!("value-{index}")
            );
        }
        let parent = path.parent().expect("temp parent");
        assert!(
            std::fs::read_dir(parent)
                .expect("read temp dir")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
            "successful atomic writes must not leave temp files"
        );
    }
}
