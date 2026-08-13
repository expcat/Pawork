//! Host composition layer: bridges the auth-service secret boundary with the
//! provider-control credential resolver contract (ADR-014/033, P18-3).
//!
//! `BackendCredentialResolver` adapts an `auth_service::SecretBackend` (the
//! process-local secret boundary: OS Keychain or `MemoryBackend`) into the
//! `provider_control::CredentialResolver` abstraction.
//!
//! Dependency direction is intentional and never inverted: `provider-control`
//! defines the resolver contract and does NOT depend on `auth-service`. The
//! host crate (this one) sits above both and performs the adaptation, wiring a
//! real `SecretBackend` into the resolver trait that the factory consumes.
//!
//! Security invariants honored here:
//! - Plaintext is read exactly once per `resolve` call and lives only inside
//!   the returned short-lived `ResolvedCredential`.
//! - `ResolvedCredential`'s `Debug` impl is redacted and `Display` is not
//!   implemented at all (provider-api), so plaintext cannot leak via formatting.
//! - Error paths surface neither the plaintext nor the secret reference value.

use std::sync::Arc;

use async_trait::async_trait;
use auth_service::{AuthError, SecretBackend};
use provider_api::{CredentialKind as ProviderCredentialKind, ResolvedCredential};
use provider_control::credential::{BackendErrorCategory, CredentialResolver, ResolveError};
use provider_control::SecretRef;

/// Adapter that resolves an opaque [`SecretRef`] into a short-lived
/// [`ResolvedCredential`] by reading the plaintext exactly once from the
/// underlying [`SecretBackend`].
///
/// Constructed once per process (or per tenant scope) and shared by reference
/// with the provider factory; the factory never receives the plaintext.
pub struct BackendCredentialResolver {
    backend: Arc<dyn SecretBackend>,
}

impl BackendCredentialResolver {
    /// Wrap a concrete secret backend (e.g. `MemoryBackend` for tests,
    /// `KeychainBackend` in production) behind the resolver contract.
    pub fn new(backend: Arc<dyn SecretBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl CredentialResolver for BackendCredentialResolver {
    async fn resolve(
        &self,
        secret_ref: &SecretRef,
        kind: ProviderCredentialKind,
    ) -> Result<ResolvedCredential, ResolveError> {
        let (service, account) = secret_ref.as_pair();
        let secret = self
            .backend
            .get(service, account)
            .map_err(|err| match err {
                // review 项：脱敏分类，绝不传播 AuthError 原始文本（可能含定位 / 部分明文）。
                AuthError::NotFound => ResolveError::NotFound,
                AuthError::InvalidSecret(_) => ResolveError::Backend(BackendErrorCategory::Invalid),
                _ => ResolveError::Backend(BackendErrorCategory::Backend),
            })?;
        Ok(ResolvedCredential::new(kind, secret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auth_service::MemoryBackend;
    use std::sync::Arc;

    /// Plaintext token used across tests; deliberately distinctive so any leak
    /// into `Debug` output is trivially detectable.
    const SECRET: &str = "sk-supersecret-token-9876543210";

    fn backend_with_secret(service: &str, account: &str) -> Arc<MemoryBackend> {
        let backend = Arc::new(MemoryBackend::new());
        backend.store(service, account, SECRET).expect("store");
        backend
    }

    #[tokio::test]
    async fn backend_adapter_resolves_to_short_lived_credential() {
        let backend = backend_with_secret("pawork.openai", "cred_1");
        let resolver = BackendCredentialResolver::new(backend);
        let secret_ref = SecretRef::new("pawork.openai", "cred_1");

        let resolved = resolver
            .resolve(&secret_ref, ProviderCredentialKind::ApiKey)
            .await
            .expect("resolve");

        assert_eq!(resolved.kind(), ProviderCredentialKind::ApiKey);
        assert_eq!(resolved.expose_secret(), SECRET);
        // The credential never outlives this function scope; the resolver holds
        // no plaintext buffer of its own.
    }

    #[tokio::test]
    async fn backend_adapter_fail_closes_on_missing_secret() {
        let backend = Arc::new(MemoryBackend::new());
        let resolver = BackendCredentialResolver::new(backend);
        let secret_ref = SecretRef::new("pawork.openai", "never-stored");

        let err = resolver
            .resolve(&secret_ref, ProviderCredentialKind::ApiKey)
            .await
            .expect_err("missing secret must fail closed");

        assert!(matches!(err, ResolveError::NotFound), "got {err:?}");
        let displayed = format!("{err}");
        assert!(
            !displayed.contains(SECRET),
            "plaintext must not appear in error display"
        );
    }

    #[tokio::test]
    async fn backend_adapter_distinct_refs_isolate_secrets() {
        let backend = Arc::new(MemoryBackend::new());
        backend
            .store("pawork.openai", "cred_a", "sk-aaa-1111111111")
            .expect("store a");
        backend
            .store("pawork.openai", "cred_b", "sk-bbb-2222222222")
            .expect("store b");
        let resolver = BackendCredentialResolver::new(backend);

        let a = SecretRef::new("pawork.openai", "cred_a");
        let b = SecretRef::new("pawork.openai", "cred_b");

        let resolved_a = resolver
            .resolve(&a, ProviderCredentialKind::ApiKey)
            .await
            .expect("resolve a");
        let resolved_b = resolver
            .resolve(&b, ProviderCredentialKind::ApiKey)
            .await
            .expect("resolve b");

        assert_eq!(resolved_a.expose_secret(), "sk-aaa-1111111111");
        assert_eq!(resolved_b.expose_secret(), "sk-bbb-2222222222");
        // A's plaintext must not bleed into B's redacted Debug output.
        let debug_b = format!("{resolved_b:?}");
        assert!(
            !debug_b.contains("sk-aaa-1111111111"),
            "cross-ref plaintext leaked into Debug"
        );
    }

    #[tokio::test]
    async fn backend_adapter_plaintext_never_enters_debug() {
        // `ResolvedCredential` deliberately does not implement `Display`, so
        // the redaction guarantee is verified through `Debug` only.
        let backend = backend_with_secret("pawork.openai", "cred_secret");
        let resolver = BackendCredentialResolver::new(backend);
        let secret_ref = SecretRef::new("pawork.openai", "cred_secret");

        let resolved = resolver
            .resolve(&secret_ref, ProviderCredentialKind::ApiKey)
            .await
            .expect("resolve");

        let debug = format!("{resolved:?}");
        assert!(!debug.contains(SECRET), "plaintext leaked into Debug");
        // Sanity: the redacted marker is present, confirming masking is active.
        assert!(debug.contains("[REDACTED]"), "secret field not redacted");
    }

    /// review 项：Backend 错误不得传播 AuthError 原始文本。用一个返回含敏感字样的
    /// 自定义后端，断言 ResolveError::Backend 只携带脱敏分类。
    #[tokio::test]
    async fn backend_adapter_never_propagates_backend_error_text() {
        /// 故意在错误文本里塞入「敏感」字样，验证它绝不出现在 ResolveError 的任何格式化输出。
        struct PoisonedBackend;
        impl SecretBackend for PoisonedBackend {
            fn store(&self, _: &str, _: &str, _: &str) -> Result<(), AuthError> {
                Ok(())
            }
            fn get(&self, _: &str, _: &str) -> Result<String, AuthError> {
                Err(AuthError::Keychain(
                    "internal-path/with/SECRET-sk-leaked-1234567890".into(),
                ))
            }
            fn delete(&self, _: &str, _: &str) -> Result<(), AuthError> {
                Ok(())
            }
        }

        let resolver = BackendCredentialResolver::new(Arc::new(PoisonedBackend));
        let secret_ref = SecretRef::new("pawork.openai", "cred_1");
        let err = resolver
            .resolve(&secret_ref, ProviderCredentialKind::ApiKey)
            .await
            .expect_err("poisoned backend must error");

        assert!(
            matches!(err, ResolveError::Backend(BackendErrorCategory::Backend)),
            "got {err:?}"
        );
        // 脱敏分类的冻结字符串（backend），不含后端原始文本 / 明文。
        let debug = format!("{err:?}");
        let display = format!("{err}");
        for forbidden in ["SECRET", "sk-leaked", "internal-path", "1234567890"] {
            assert!(
                !debug.contains(forbidden) && !display.contains(forbidden),
                "AuthError 文本 {forbidden} 泄漏到 ResolveError"
            );
        }
        assert!(display.contains("backend"), "脱敏分类应出现在 Display");
    }

    #[test]
    fn backend_adapter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BackendCredentialResolver>();
    }
}
