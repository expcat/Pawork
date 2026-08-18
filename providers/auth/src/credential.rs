//! 凭证元数据与 API Key 认证方式。
//!
//! 关键红线：明文 secret **绝不**进入 [`StoredCredential`] 或 [`ApiKeyCredential`]，
//! 只存在于 `SecretBackend`（auth 文件 / 内存）中。这两个结构只持有可安全序列化、
//! 可记录到数据库与日志的元数据 + 脱敏状态。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pawork_domain::{ProviderId, Timestamp};
use pawork_domain::{CredentialKind, ResolvedCredential};
use serde::{Deserialize, Serialize};

use crate::backend::SecretBackend;
use crate::error::AuthError;
use crate::masked::MaskedCredential;

/// Secret 后端中按 Provider 分组的命名空间前缀。
const KEYCHAIN_SERVICE_PREFIX: &str = "pawork";

/// 统一使用 [`pawork_domain::CredentialId`]（与控制面 CredentialMetadata 对齐）。
pub use pawork_domain::CredentialId;

/// 进程内单调序号，配合纳秒时间戳保证 [`CredentialId`] 唯一。
static CREDENTIAL_SEQ: AtomicU64 = AtomicU64::new(0);

/// 生成全局唯一的 `CredentialId`（`cred_{nanos:x}_{seq:x}` 格式，与历史值兼容）。
///
/// 唯一性来自「纳秒时间戳 + 进程内单调序号」，不依赖外部 UUID 依赖。
pub fn generate_credential_id() -> CredentialId {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let seq = CREDENTIAL_SEQ.fetch_add(1, Ordering::Relaxed);
    CredentialId::new(format!("cred_{nanos:x}_{seq:x}"))
}

/// 当前 Unix 毫秒时间戳（缺失时退化为 0）。
fn now_unix_millis() -> Timestamp {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    Timestamp::from_unix_millis(millis)
}

/// 计算 Provider 在 Secret 后端中的 `service` 名（形如 `pawork.openai`）。
///
/// 同时供 `crate::resolve` 的 Provider 主条目查找复用，保证 service 命名单一来源。
pub(crate) fn keychain_service_for(provider: &ProviderId) -> String {
    format!("{KEYCHAIN_SERVICE_PREFIX}.{provider}")
}

/// 仅含元数据与脱敏状态的凭证记录，可安全持久化到数据库与日志。
///
/// 明文 secret **不**在此结构中——它只存在于 SecretBackend。字段名
/// `keychain_service` / `keychain_account` 为 V1 JSON 兼容名，实际用于定位
/// auth 文件或内存后端中的 service/account。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredential {
    /// 凭证唯一标识，同时作为 SecretBackend 的 `account`。
    pub id: CredentialId,
    /// 凭证所属 Provider。
    pub provider: ProviderId,
    /// 人类可读的显示名（不含明文）。
    pub display_name: String,
    /// 脱敏后的展示状态。
    pub masked: MaskedCredential,
    /// SecretBackend `service`（V1 兼容字段名），用于定位明文 secret。
    pub keychain_service: String,
    /// SecretBackend `account`（V1 兼容字段名），用于定位明文 secret。
    pub keychain_account: String,
    /// 创建时间（Unix 毫秒）。
    pub created_at: Timestamp,
    /// 过期时间（Unix 毫秒），`None` 表示不过期。
    pub expires_at: Option<Timestamp>,
    /// 授权 scope 列表（不含明文）。
    pub scopes: Vec<String>,
}

impl StoredCredential {
    /// 以脱敏状态构造元数据（不接触明文）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: CredentialId,
        provider: ProviderId,
        display_name: impl Into<String>,
        masked: MaskedCredential,
        keychain_service: impl Into<String>,
        keychain_account: impl Into<String>,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            id,
            provider,
            display_name: display_name.into(),
            masked,
            keychain_service: keychain_service.into(),
            keychain_account: keychain_account.into(),
            created_at: now_unix_millis(),
            expires_at: None,
            scopes,
        }
    }

    /// 设置过期时间并返回 `&mut Self`，便于构造器式链式调用。
    pub fn with_expires_at(mut self, expires_at: Timestamp) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// 返回 SecretBackend 定位所需的 `(service, account)`；方法名沿用 V1。
    pub fn keychain_ref(&self) -> (&str, &str) {
        (
            self.keychain_service.as_str(),
            self.keychain_account.as_str(),
        )
    }
}

/// API Key 认证方式：持有可持久化的元数据，按需从后端解析出明文 secret。
///
/// `resolve` 产出的 [`ResolvedCredential`] 仅在 Provider adapter 构造认证请求时
/// 短暂使用，不得记录到日志或事件（见 `ResolvedCredential::expose_secret`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeyCredential {
    stored: StoredCredential,
}

impl ApiKeyCredential {
    /// 将明文 secret 写入 Secret 后端，返回**仅含元数据/脱敏状态**的 `StoredCredential`。
    ///
    /// 明文 secret 不会出现在返回值中。
    pub fn store(
        backend: &dyn SecretBackend,
        provider: ProviderId,
        display_name: &str,
        secret: &str,
    ) -> Result<StoredCredential, AuthError> {
        Self::store_with_scopes(backend, provider, display_name, secret, Vec::new())
    }

    /// 与 [`ApiKeyCredential::store`] 相同，但额外写入授权 scope 列表。
    pub fn store_with_scopes(
        backend: &dyn SecretBackend,
        provider: ProviderId,
        display_name: &str,
        secret: &str,
        scopes: Vec<String>,
    ) -> Result<StoredCredential, AuthError> {
        if secret.is_empty() {
            return Err(AuthError::InvalidSecret("secret is empty".into()));
        }
        let id = generate_credential_id();
        let keychain_service = keychain_service_for(&provider);
        let keychain_account = id.as_str().to_string();

        // 明文只在此处短暂出现：写入后端后即从栈上丢弃。
        backend.store(&keychain_service, &keychain_account, secret)?;

        let stored = StoredCredential {
            masked: MaskedCredential::mask(secret),
            id,
            provider,
            display_name: display_name.to_string(),
            keychain_service,
            keychain_account,
            created_at: now_unix_millis(),
            expires_at: None,
            scopes,
        };
        Ok(stored)
    }

    /// 由已持久化的元数据构造 API Key 凭证（不接触明文）。
    pub fn from_stored(stored: StoredCredential) -> Result<Self, AuthError> {
        if stored.keychain_service.is_empty() || stored.keychain_account.is_empty() {
            return Err(AuthError::MalformedMetadata(
                "missing secret backend service/account reference".into(),
            ));
        }
        Ok(Self { stored })
    }

    /// 返回内部元数据引用。
    pub fn stored(&self) -> &StoredCredential {
        &self.stored
    }

    /// 消费并返回内部元数据。
    pub fn into_stored(self) -> StoredCredential {
        self.stored
    }

    /// 从 Secret 后端解析出明文 secret，包装为 [`ResolvedCredential`]。
    ///
    /// 返回值仅供 Provider adapter 构造认证请求使用；调用方不得将其记录到
    /// 日志或事件（`ResolvedCredential` 的 `Debug` 已脱敏）。
    pub fn resolve(&self, backend: &dyn SecretBackend) -> Result<ResolvedCredential, AuthError> {
        let (service, account) = self.stored.keychain_ref();
        let secret = backend.get(service, account)?;
        Ok(ResolvedCredential::new(CredentialKind::ApiKey, secret))
    }

    /// 从 Secret 后端删除对应明文。删除成功后本凭证即不可再 `resolve`。
    pub fn delete(&self, backend: &dyn SecretBackend) -> Result<(), AuthError> {
        let (service, account) = self.stored.keychain_ref();
        backend.delete(service, account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MemoryBackend;

    const SECRET: &str = "sk-supersecret-token-1234567890";

    fn store_api_key(backend: &MemoryBackend) -> StoredCredential {
        ApiKeyCredential::store(
            backend,
            ProviderId::new("openai"),
            "OpenAI Production",
            SECRET,
        )
        .expect("store api key")
    }

    #[test]
    fn store_returns_metadata_without_plaintext() {
        let backend = MemoryBackend::new();
        let stored = store_api_key(&backend);

        // 返回值与序列化结果都不含明文。
        assert_eq!(stored.provider, ProviderId::new("openai"));
        assert_eq!(stored.display_name, "OpenAI Production");
        assert!(!format!("{stored:?}").contains(SECRET));
        assert!(!serde_json::to_string(&stored).unwrap().contains(SECRET));

        // 脱敏展示存在且为已知片段。
        assert!(stored.masked.as_str().starts_with("sk-"));
        assert!(stored.masked.as_str().ends_with("7890"));

        // 明文确实进入了后端。
        let (service, account) = stored.keychain_ref();
        assert_eq!(backend.get(service, account).expect("get"), SECRET);
        assert_eq!(backend.len(), 1);
    }

    #[test]
    fn resolve_produces_api_key_credential() {
        let backend = MemoryBackend::new();
        let stored = store_api_key(&backend);
        let api_key = ApiKeyCredential::from_stored(stored).expect("from_stored");

        let resolved = api_key.resolve(&backend).expect("resolve");
        assert_eq!(resolved.kind(), CredentialKind::ApiKey);
        assert_eq!(resolved.expose_secret(), SECRET);

        // ResolvedCredential 的 Debug 同样脱敏。
        assert!(!format!("{resolved:?}").contains(SECRET));
    }

    #[test]
    fn delete_then_resolve_returns_not_found() {
        let backend = MemoryBackend::new();
        let stored = store_api_key(&backend);
        let api_key = ApiKeyCredential::from_stored(stored).expect("from_stored");

        api_key.delete(&backend).expect("delete");
        assert!(backend.is_empty());

        match api_key.resolve(&backend) {
            Err(AuthError::NotFound) => {}
            other => panic!("expected NotFound after delete, got {other:?}"),
        }
    }

    #[test]
    fn empty_secret_is_rejected() {
        let backend = MemoryBackend::new();
        match ApiKeyCredential::store(&backend, ProviderId::new("openai"), "empty", "") {
            Err(AuthError::InvalidSecret(_)) => {}
            other => panic!("expected InvalidSecret, got {other:?}"),
        }
        assert!(backend.is_empty());
    }

    #[test]
    fn multiple_credentials_for_same_provider_use_distinct_accounts() {
        let backend = MemoryBackend::new();
        let a = ApiKeyCredential::store(
            &backend,
            ProviderId::new("openai"),
            "first",
            "sk-first-secret-aaaaaaaaaaaa",
        )
        .expect("store first");
        let b = ApiKeyCredential::store(
            &backend,
            ProviderId::new("openai"),
            "second",
            "sk-second-secret-bbbbbbbbbb",
        )
        .expect("store second");

        // 同 provider、不同 account，互不覆盖。
        assert_eq!(a.keychain_service, b.keychain_service);
        assert_ne!(a.keychain_account, b.keychain_account);
        assert_eq!(backend.len(), 2);

        let resolved_a = ApiKeyCredential::from_stored(a)
            .expect("from_stored")
            .resolve(&backend)
            .expect("a");
        let resolved_b = ApiKeyCredential::from_stored(b)
            .expect("from_stored")
            .resolve(&backend)
            .expect("b");
        assert_ne!(resolved_a.expose_secret(), resolved_b.expose_secret());
    }

    #[test]
    fn stored_credential_round_trip_preserves_metadata() {
        let backend = MemoryBackend::new();
        let stored = ApiKeyCredential::store_with_scopes(
            &backend,
            ProviderId::new("anthropic"),
            "Claude",
            "sk-ant-secret-cccccccccccc",
            vec!["chat".to_string(), "completions".to_string()],
        )
        .expect("store");

        let json = serde_json::to_string(&stored).expect("serialize");
        assert!(!json.contains("sk-ant-secret-cccccccccccc"));
        let decoded: StoredCredential = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, stored);
    }

    #[test]
    fn from_stored_rejects_missing_backend_ref() {
        let malformed = StoredCredential::new(
            CredentialId::new("cred_x"),
            ProviderId::new("openai"),
            "x",
            MaskedCredential::from_masked("sk-…xxxx"),
            "",
            "acct",
            Vec::new(),
        );
        assert!(matches!(
            ApiKeyCredential::from_stored(malformed),
            Err(AuthError::MalformedMetadata(_))
        ));
    }
}
