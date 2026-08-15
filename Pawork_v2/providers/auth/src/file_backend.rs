//! 文件 Secret 后端（参照 Codex CLI auth.json 形态）。
//!
//! 单 JSON 文件（默认 ~/.pawork/auth.json，可用 PAWORK_HOME 覆盖），
//! 0600 权限、临时文件 + rename 原子写。keyspace 与 OS 后端一致：
//! (service, account) 二元组，上层（default_credential / resolve）无感知。
//! 损坏文件 fail-closed（报错，不静默清空）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend::SecretBackend;
use crate::error::AuthError;

/// 文件格式版本（将来迁移用；当前只接受 1）。
const FORMAT_VERSION: u64 = 1;

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

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                AuthError::Storage(format!("create {}: {error}", parent.display()))
            })?;
        }

        // 原子写：同目录临时文件 + rename，中途崩溃不留半写文件。
        let tmp = self.path.with_extension("json.tmp");
        write_file_0600(&tmp, &json)?;
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
        let mut entries = self.load()?;
        entries
            .entry(service.to_string())
            .or_default()
            .insert(account.to_string(), secret.to_string());
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
}

/// 默认 auth 文件路径：$PAWORK_HOME/auth.json 或 ~/.pawork/auth.json。
fn default_auth_file_path() -> PathBuf {
    if let Some(home) = std::env::var_os("PAWORK_HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join("auth.json");
        }
    }
    let home = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".pawork").join("auth.json")
}

#[cfg(unix)]
fn write_file_0600(path: &Path, bytes: &[u8]) -> Result<(), AuthError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| AuthError::Storage(format!("create {}: {error}", path.display())))?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|error| AuthError::Storage(format!("write {}: {error}", path.display())))
}

#[cfg(not(unix))]
fn write_file_0600(path: &Path, bytes: &[u8]) -> Result<(), AuthError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|error| AuthError::Storage(format!("create {}: {error}", path.display())))?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|error| AuthError::Storage(format!("write {}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

