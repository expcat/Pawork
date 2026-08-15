//! Secret 存储后端抽象：`SecretBackend` trait 与实现。
//!
//! - [`FileBackend`](crate::FileBackend)：单 JSON 文件（0600 + 原子写，生产用）。
//! - [`MemoryBackend`]：进程内 `HashMap`，仅用于单元测试。
//!
//! 明文 secret 仅在这些后端中流转，永远不会通过错误、日志或返回的元数据泄漏。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::AuthError;

/// 抽象的 Secret 存储后端。
///
/// 所有方法以 `(service, account)` 二元组定位条目。实现必须是 `Send + Sync`，
/// 且不得在任何错误信息中回传明文 secret。
pub trait SecretBackend: Send + Sync {
    /// 写入（或覆盖）一条 secret。
    fn store(&self, service: &str, account: &str, secret: &str) -> Result<(), AuthError>;
    /// 读取一条 secret；不存在时返回 [`AuthError::NotFound`]。
    fn get(&self, service: &str, account: &str) -> Result<String, AuthError>;
    /// 删除一条 secret；不存在时返回 [`AuthError::NotFound`]。
    fn delete(&self, service: &str, account: &str) -> Result<(), AuthError>;
}

// OS Keychain 后端已按用户决策移除：secret 统一走文件后端
// （`~/.pawork/auth.json`，0600，参照 Codex CLI auth.json 形态）。

/// 仅用于测试的内存后端，按 `(service, account)` 存储 secret。
///
/// 故意**不**派生 `Debug`，避免在日志/断言中意外打印明文 secret。
pub struct MemoryBackend {
    entries: Mutex<HashMap<(String, String), String>>,
}

impl MemoryBackend {
    /// 创建一个空的内存后端。
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// 当前存储的条目数量（不含明文，可用于断言）。
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("MemoryBackend mutex poisoned")
            .len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MemoryBackend {
    fn clone(&self) -> Self {
        Self {
            entries: Mutex::new(
                self.entries
                    .lock()
                    .expect("MemoryBackend mutex poisoned")
                    .clone(),
            ),
        }
    }
}

impl SecretBackend for MemoryBackend {
    fn store(&self, service: &str, account: &str, secret: &str) -> Result<(), AuthError> {
        self.entries
            .lock()
            .expect("MemoryBackend mutex poisoned")
            .insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
        Ok(())
    }

    fn get(&self, service: &str, account: &str) -> Result<String, AuthError> {
        self.entries
            .lock()
            .expect("MemoryBackend mutex poisoned")
            .get(&(service.to_string(), account.to_string()))
            .cloned()
            .ok_or(AuthError::NotFound)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), AuthError> {
        self.entries
            .lock()
            .expect("MemoryBackend mutex poisoned")
            .remove(&(service.to_string(), account.to_string()))
            .map(|_| ())
            .ok_or(AuthError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_backend_round_trip_and_delete() {
        let backend = MemoryBackend::new();
        backend.store("svc", "acct", "secret-value").expect("store");
        assert_eq!(backend.len(), 1);

        let got = backend.get("svc", "acct").expect("get");
        assert_eq!(got, "secret-value");

        backend.delete("svc", "acct").expect("delete");
        assert!(backend.is_empty());
    }

    #[test]
    fn memory_backend_missing_returns_not_found() {
        let backend = MemoryBackend::new();
        assert!(matches!(
            backend.get("missing", "missing"),
            Err(AuthError::NotFound)
        ));
        assert!(matches!(
            backend.delete("missing", "missing"),
            Err(AuthError::NotFound)
        ));
    }

    #[test]
    fn memory_backend_store_overwrites() {
        let backend = MemoryBackend::new();
        backend.store("svc", "acct", "first").expect("store first");
        backend
            .store("svc", "acct", "second")
            .expect("store second");
        assert_eq!(backend.get("svc", "acct").expect("get"), "second");
        assert_eq!(backend.len(), 1);
    }

    #[test]
    fn memory_backend_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MemoryBackend>();
        assert_send_sync::<crate::FileBackend>();
    }
}
