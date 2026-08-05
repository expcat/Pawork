//! Secret 存储后端抽象：`SecretBackend` trait 与两个实现。
//!
//! - [`KeychainBackend`]：基于 `keyring` 的真实 OS Keychain 访问（生产用）。
//! - [`MemoryBackend`]：进程内 `HashMap`，仅用于单元测试，**不**依赖系统 Keychain。
//!
//! 明文 secret 仅在这些后端中流转，永远不会通过错误、日志或返回的元数据泄漏。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::AuthError;

/// 抽象的 Secret 存储后端。
///
/// 所有方法以 `(service, account)` 二元组定位条目，语义上等价于 keyring 的
/// `Entry`。实现必须是 `Send + Sync`，且不得在任何错误信息中回传明文 secret。
pub trait SecretBackend: Send + Sync {
    /// 写入（或覆盖）一条 secret。
    fn store(&self, service: &str, account: &str, secret: &str) -> Result<(), AuthError>;
    /// 读取一条 secret；不存在时返回 [`AuthError::NotFound`]。
    fn get(&self, service: &str, account: &str) -> Result<String, AuthError>;
    /// 删除一条 secret；不存在时返回 [`AuthError::NotFound`]。
    fn delete(&self, service: &str, account: &str) -> Result<(), AuthError>;
}

/// 基于 `keyring` crate 的 OS Keychain 后端。
///
/// 在没有可用 Keychain（如部分 CI）的环境下，`store`/`get`/`delete` 可能返回
/// [`AuthError::Keychain`]。**自动测试不应依赖此后端**。
#[derive(Clone, Copy, Debug, Default)]
pub struct KeychainBackend;

impl KeychainBackend {
    /// 创建一个新的 Keychain 后端。
    pub const fn new() -> Self {
        Self
    }

    fn entry(&self, service: &str, account: &str) -> Result<keyring::Entry, AuthError> {
        keyring::Entry::new(service, account).map_err(map_keyring_error)
    }
}

impl SecretBackend for KeychainBackend {
    fn store(&self, service: &str, account: &str, secret: &str) -> Result<(), AuthError> {
        self.entry(service, account)?
            .set_password(secret)
            .map_err(map_keyring_error)
    }

    fn get(&self, service: &str, account: &str) -> Result<String, AuthError> {
        match self.entry(service, account)?.get_password() {
            Ok(value) => Ok(value),
            Err(error) => {
                if is_no_entry(&error) {
                    Err(AuthError::NotFound)
                } else {
                    Err(map_keyring_error(error))
                }
            }
        }
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), AuthError> {
        match self.entry(service, account)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(error) => {
                if is_no_entry(&error) {
                    Err(AuthError::NotFound)
                } else {
                    Err(map_keyring_error(error))
                }
            }
        }
    }
}

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

/// 将 keyring 原始错误归一为 [`AuthError::Keychain`]，仅保留可读描述。
///
/// 注意：`keyring::Error` 的 `Display` 不会输出 secret，可安全字符串化。
fn map_keyring_error(error: keyring::Error) -> AuthError {
    AuthError::Keychain(error.to_string())
}

/// 判断 keyring 错误是否表示「条目不存在」。
fn is_no_entry(error: &keyring::Error) -> bool {
    matches!(error, keyring::Error::NoEntry)
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
        assert_send_sync::<KeychainBackend>();
    }
}
