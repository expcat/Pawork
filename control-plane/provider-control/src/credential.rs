//! Secret 边界与短生命周期 [`pawork_api::ResolvedCredential`]（ADR-014/033，P18-3）。
//!
//! 凭据记录（[`crate::account::CredentialRecord`]）只持有 opaque [`SecretRef`]；
//! 明文绝不入库、不进日志、不进事件。运行时由注入的
//! [`CredentialResolver`] 把 `SecretRef` 解析成短生命周期的
//! [`pawork_api::ResolvedCredential`]，仅供 Provider adapter 构造一次认证请求，
//! 用后即弃。
//!
//! 本 crate **不依赖 `auth-service` / OS Keychain**：resolver 是 backend-agnostic
//! trait，宿主组合层注入 keychain 适配实现（`provider-control → provider-api`，
//! 不向 `agent-domain` 或存储底层反转依赖）。

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use pawork_api::{CredentialKind as ProviderCredentialKind, ResolvedCredential};
use thiserror::Error;

use crate::account::SecretRef;

/// Secret 解析错误。**任何变体的 `Display`/`Debug` 都不得包含明文 secret，
/// 也不得携带后端原始错误文本**（review 项：脱敏 unit/typed 变体）。
#[derive(Debug, Error)]
pub enum ResolveError {
    /// `SecretRef` 指向的明文不存在（已删除 / 未回灌 / 合成 sentinel）。
    #[error("credential secret not found for the given reference")]
    NotFound,
    /// 后端（Keychain / 注入实现）返回错误；携带脱敏分类，**绝不传播后端文本**。
    #[error("secret backend error: {0}")]
    Backend(BackendErrorCategory),
}

/// 脱敏的后端错误分类（不携带任何 `AuthError` 文本 / 明文）。
///
/// 宿主组合层把 `AuthError`（或其它后端错误）归因到这几个冻结分类，避免把后端
/// 原始消息（可能含定位 / 部分明文）透传到 resolver 错误路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendErrorCategory {
    /// 后端 I/O / Keychain 故障。
    Backend,
    /// Secret 存在但被后端判定为非法（格式 / 长度等）。
    Invalid,
    /// 其它后端侧故障（脱敏归因）。
    Other,
}

impl std::fmt::Display for BackendErrorCategory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let wire = match self {
            Self::Backend => "backend",
            Self::Invalid => "invalid",
            Self::Other => "other",
        };
        formatter.write_str(wire)
    }
}

/// 把 opaque [`SecretRef`] 解析为短生命周期 [`ResolvedCredential`]。
///
/// 实现约定：
/// - 返回的 [`ResolvedCredential`] 仅供调用方在构造一次认证请求时短暂持有；
///   不得记录到日志、事件、诊断包或 GUI（其 `Debug` 已脱敏）；
/// - 实现自身不得在错误信息或日志中回传明文 secret；
/// - `Send + Sync`，可在异步上下文中跨任务共享。
#[async_trait]
pub trait CredentialResolver: Send + Sync {
    /// 解析 `secret_ref` 对应的明文，包装为短生命周期 [`ResolvedCredential`]。
    async fn resolve(
        &self,
        secret_ref: &SecretRef,
        kind: ProviderCredentialKind,
    ) -> Result<ResolvedCredential, ResolveError>;
}

/// 仅用于测试 / 组合层开发的进程内 resolver：按 `(service, account)` 持有明文。
///
/// 故意**不**派生 `Debug`，避免在日志 / 断言中意外打印明文 secret。
pub struct InMemoryCredentialResolver {
    secrets: Mutex<HashMap<(String, String), String>>,
}

impl InMemoryCredentialResolver {
    /// 创建空 resolver。
    pub fn new() -> Self {
        Self {
            secrets: Mutex::new(HashMap::new()),
        }
    }

    /// 写入一条明文（仅测试用；明文只存在于此内存 map）。
    pub fn put(&self, secret_ref: &SecretRef, secret: impl Into<String>) {
        let (service, account) = secret_ref.as_pair();
        self.secrets
            .lock()
            .expect("InMemoryCredentialResolver mutex poisoned")
            .insert((service.to_string(), account.to_string()), secret.into());
    }

    /// 当前持有的条目数（不含明文，可用于断言）。
    pub fn len(&self) -> usize {
        self.secrets
            .lock()
            .expect("InMemoryCredentialResolver mutex poisoned")
            .len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for InMemoryCredentialResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CredentialResolver for InMemoryCredentialResolver {
    async fn resolve(
        &self,
        secret_ref: &SecretRef,
        kind: ProviderCredentialKind,
    ) -> Result<ResolvedCredential, ResolveError> {
        let (service, account) = secret_ref.as_pair();
        let secret = self
            .secrets
            .lock()
            .expect("InMemoryCredentialResolver mutex poisoned")
            .get(&(service.to_string(), account.to_string()))
            .cloned()
            .ok_or(ResolveError::NotFound)?;
        Ok(ResolvedCredential::new(kind, secret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "sk-supersecret-token-1234567890";

    #[tokio::test]
    async fn resolve_returns_short_lived_redacted_credential() {
        let resolver = InMemoryCredentialResolver::new();
        let secret_ref = SecretRef::new("pawork.openai", "cred_1");
        resolver.put(&secret_ref, SECRET);

        let resolved = resolver
            .resolve(&secret_ref, ProviderCredentialKind::ApiKey)
            .await
            .expect("resolve");
        assert_eq!(resolved.kind(), ProviderCredentialKind::ApiKey);
        assert_eq!(resolved.expose_secret(), SECRET);
        // ResolvedCredential 的 Debug 已脱敏（provider-api 契约）。
        assert!(!format!("{resolved:?}").contains(SECRET));
    }

    #[tokio::test]
    async fn missing_secret_fails_closed_without_plaintext() {
        let resolver = InMemoryCredentialResolver::new();
        let secret_ref = SecretRef::new("pawork.openai", "missing");
        let error = resolver
            .resolve(&secret_ref, ProviderCredentialKind::ApiKey)
            .await
            .expect_err("missing secret must fail closed");
        assert!(matches!(error, ResolveError::NotFound));
        // 错误信息不含明文。
        assert!(!format!("{error}").contains(SECRET));
        assert!(!format!("{error:?}").contains(SECRET));
    }

    #[tokio::test]
    async fn synthetic_sentinel_secret_ref_resolves_not_found_by_default() {
        // 合成默认凭据的 sentinel ref 在未回灌时 fail-closed。
        let credential = crate::account::CredentialRecord::legacy_synthetic_default();
        let resolver = InMemoryCredentialResolver::new();
        let error = resolver
            .resolve(&credential.secret_ref, ProviderCredentialKind::ApiKey)
            .await
            .expect_err("synthetic sentinel must fail closed until backfilled");
        assert!(matches!(error, ResolveError::NotFound));
    }

    #[tokio::test]
    async fn distinct_refs_isolate_secrets() {
        let resolver = InMemoryCredentialResolver::new();
        let a = SecretRef::new("pawork.openai", "cred_a");
        let b = SecretRef::new("pawork.openai", "cred_b");
        resolver.put(&a, "sk-aaaa-secret-aaaaaaaaaaaa");
        resolver.put(&b, "sk-bbbb-secret-bbbbbbbbbbbb");
        assert_eq!(resolver.len(), 2);

        let ra = resolver
            .resolve(&a, ProviderCredentialKind::ApiKey)
            .await
            .unwrap();
        let rb = resolver
            .resolve(&b, ProviderCredentialKind::ApiKey)
            .await
            .unwrap();
        assert_ne!(ra.expose_secret(), rb.expose_secret());
    }

    #[tokio::test]
    async fn resolver_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<InMemoryCredentialResolver>();
        assert_send_sync::<&dyn CredentialResolver>();
    }
}
