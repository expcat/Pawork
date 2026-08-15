//! 每 Provider 唯一的 default OAuth 条目（S6 波 C）。
//!
//! 首发阶段每 provider 只保存一条 OAuth 凭证，使用确定性 Keychain 定位：
//! service = pawork.<provider>.oauth，account 为 default.access / default.refresh /
//! default.meta。meta 是仅含掩码与过期时间的 JSON（非 secret），供装配期无网络
//! 重建 StoredCredential 并判断是否需要刷新。多凭证/账号池留 S11。

use serde::{Deserialize, Serialize};

use pawork_domain::{CredentialId, ProviderId, Timestamp};

use crate::backend::SecretBackend;
use crate::credential::StoredCredential;
use crate::masked::MaskedCredential;
use crate::oauth::{decode_jwt_payload, TokenSet};
use crate::AuthError;

/// default 条目的固定 account 前缀。
pub const OAUTH_DEFAULT_ACCOUNT: &str = "default";
const KEYCHAIN_SERVICE_PREFIX: &str = "pawork";
const SERVICE_SUFFIX: &str = "oauth";
const REFRESH_GRACE_MILLIS: u64 = 30_000;
const CHATGPT_ACCOUNT_ID_CLAIM: &str = "chatgpt_account_id";
const CHATGPT_AUTH_CLAIM_PREFIX: &str = "https://api.openai.com/auth";

/// default OAuth 条目的非机密元数据（可安全打印/存 Keychain meta 账户）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefaultOAuthMeta {
    pub masked: MaskedCredential,
    pub created_at_ms: u64,
    /// Unix 毫秒；None = 永不过期（每次请求前仍会尽量刷新）。
    pub expires_at_ms: Option<u64>,
    pub scopes: Vec<String>,
    /// ChatGPT 专用：JWT claim 中的 account id（路由头，非 secret）。
    pub account_id: Option<String>,
}

/// 该 provider 的 OAuth service 名（与 oauth.rs 私有实现保持同一形状）。
pub fn oauth_service(provider: &ProviderId) -> String {
    format!("{KEYCHAIN_SERVICE_PREFIX}.{provider}.{SERVICE_SUFFIX}")
}

fn access_account() -> String {
    format!("{OAUTH_DEFAULT_ACCOUNT}.access")
}

fn refresh_account() -> String {
    format!("{OAUTH_DEFAULT_ACCOUNT}.refresh")
}

fn meta_account() -> String {
    format!("{OAUTH_DEFAULT_ACCOUNT}.meta")
}

/// 把一次 OAuth 交换结果写入 default 条目（access/refresh/meta 三账户）。
pub fn store_default_oauth_token(
    backend: &dyn SecretBackend,
    provider: ProviderId,
    tokens: &TokenSet,
) -> Result<StoredCredential, AuthError> {
    if tokens.access_token.is_empty() {
        return Err(AuthError::InvalidSecret("access_token is empty".into()));
    }
    let service = oauth_service(&provider);
    backend.store(&service, &access_account(), &tokens.access_token)?;
    if let Some(refresh) = tokens.refresh_token.as_deref() {
        backend.store(&service, &refresh_account(), refresh)?;
    }
    let meta = DefaultOAuthMeta {
        masked: MaskedCredential::mask(&tokens.access_token),
        created_at_ms: now_unix_millis(),
        expires_at_ms: tokens
            .expires_in
            .map(|secs| now_unix_millis().saturating_add(secs.saturating_mul(1000))),
        scopes: token_scopes(tokens),
        account_id: chatgpt_account_id(tokens),
    };
    persist_meta(backend, &provider, &meta)?;
    Ok(stored_from_meta(provider, meta))
}

/// 读取 default 条目元数据；条目不存在返回 None（调用方 fail-closed）。
pub fn load_default_oauth_credential(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<Option<StoredCredential>, AuthError> {
    let service = oauth_service(provider);
    let meta_json = match backend.get(&service, &meta_account()) {
        Ok(value) => value,
        Err(AuthError::NotFound) => return Ok(None),
        Err(error) => return Err(error),
    };
    let meta: DefaultOAuthMeta = serde_json::from_str(&meta_json)
        .map_err(|error| AuthError::MalformedMetadata(format!("default oauth meta: {error}")))?;
    Ok(Some(stored_from_meta(provider.clone(), meta)))
}

/// 读取 meta（auth list 展示用）；条目不存在返回 None。
pub fn load_default_oauth_meta(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<Option<DefaultOAuthMeta>, AuthError> {
    match backend.get(&oauth_service(provider), &meta_account()) {
        Ok(meta_json) => serde_json::from_str(&meta_json)
            .map(Some)
            .map_err(|error| {
                AuthError::MalformedMetadata(format!("default oauth meta: {error}"))
            }),
        Err(AuthError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 删除 default 条目全部账户（幂等：meta 不存在时仍尝试清理 token 账户）。
pub fn delete_default_oauth_token(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<(), AuthError> {
    let service = oauth_service(provider);
    for account in [access_account(), refresh_account(), meta_account()] {
        match backend.delete(&service, &account) {
            Ok(()) | Err(AuthError::NotFound) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// 刷新后回写 access/refresh 与 meta（保持三者一致）。
pub fn update_default_oauth_token(
    backend: &dyn SecretBackend,
    stored: &mut StoredCredential,
    tokens: &TokenSet,
) -> Result<(), AuthError> {
    let service = oauth_service(&stored.provider);
    if stored.keychain_service != service || stored.keychain_account != access_account() {
        return Err(AuthError::MalformedMetadata(
            "credential is not the default oauth entry".into(),
        ));
    }
    if tokens.access_token.is_empty() {
        return Err(AuthError::InvalidSecret("access_token is empty".into()));
    }
    if let Some(refresh) = tokens.refresh_token.as_deref() {
        backend.store(&service, &refresh_account(), refresh)?;
    }
    backend.store(&service, &access_account(), &tokens.access_token)?;
    stored.masked = MaskedCredential::mask(&tokens.access_token);
    if let Some(expires_in) = tokens.expires_in {
        stored.expires_at = Some(Timestamp::from_unix_millis(
            now_unix_millis().saturating_add(expires_in.saturating_mul(1000)),
        ));
    }
    if let Some(scope) = tokens.scope.as_deref() {
        stored.scopes = scope.split_whitespace().map(str::to_string).collect();
    }
    // 刷新响应通常不携带 id_token：保留旧 meta 的 account_id，避免 ChatGPT
    // 路由头信息在自动刷新后丢失。
    let previous_meta = load_default_oauth_meta(backend, &stored.provider).unwrap_or(None);
    let account_id = chatgpt_account_id(tokens)
        .or_else(|| previous_meta.as_ref().and_then(|meta| meta.account_id.clone()));
    let meta = DefaultOAuthMeta {
        masked: stored.masked.clone(),
        created_at_ms: stored.created_at.as_unix_millis(),
        expires_at_ms: stored.expires_at.map(Timestamp::as_unix_millis),
        scopes: stored.scopes.clone(),
        account_id,
    };
    persist_meta(backend, &stored.provider, &meta)
}

/// 到期判断（与 oauth::needs_refresh 同语义：无 expires 视为需要刷新）。
pub fn default_oauth_needs_refresh(stored: &StoredCredential) -> bool {
    match stored.expires_at {
        Some(expires) => {
            now_unix_millis().saturating_add(REFRESH_GRACE_MILLIS) >= expires.as_unix_millis()
        }
        None => true,
    }
}

fn persist_meta(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
    meta: &DefaultOAuthMeta,
) -> Result<(), AuthError> {
    let json = serde_json::to_string(meta)
        .map_err(|error| AuthError::MalformedMetadata(format!("serialize meta: {error}")))?;
    backend.store(&oauth_service(provider), &meta_account(), &json)
}

fn stored_from_meta(provider: ProviderId, meta: DefaultOAuthMeta) -> StoredCredential {
    let service = oauth_service(&provider);
    StoredCredential {
        id: CredentialId::new(OAUTH_DEFAULT_ACCOUNT),
        provider,
        display_name: "default oauth".into(),
        masked: meta.masked,
        keychain_service: service,
        keychain_account: access_account(),
        created_at: Timestamp::from_unix_millis(meta.created_at_ms),
        expires_at: meta.expires_at_ms.map(Timestamp::from_unix_millis),
        scopes: meta.scopes,
    }
}

fn token_scopes(tokens: &TokenSet) -> Vec<String> {
    tokens
        .scope
        .as_deref()
        .map(|scope| scope.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// 从 id_token JWT payload 提取 ChatGPT account id（不验签——该值只作路由头）。
fn chatgpt_account_id(tokens: &TokenSet) -> Option<String> {
    let id_token = tokens.id_token.as_deref()?;
    let payload_b64 = id_token.split('.').nth(1)?;
    let decoded = decode_jwt_payload(payload_b64).ok()?;
    decoded
        .get(CHATGPT_AUTH_CLAIM_PREFIX)?
        .get(CHATGPT_ACCOUNT_ID_CLAIM)?
        .as_str()
        .map(str::to_string)
}

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MemoryBackend;
    use base64::Engine;

    fn token_set(id_token: Option<&str>) -> TokenSet {
        TokenSet {
            access_token: "access-secret-value-123456".into(),
            refresh_token: Some("refresh-secret-value-654321".into()),
            id_token: id_token.map(str::to_string),
            expires_in: Some(3600),
            token_type: "Bearer".into(),
            scope: Some("openid profile".into()),
        }
    }

    #[test]
    fn store_load_roundtrip_without_secret_leak() {
        let backend = MemoryBackend::new();
        let provider = ProviderId::new("chatgpt");
        let stored =
            store_default_oauth_token(&backend, provider.clone(), &token_set(None)).expect("store");
        assert_eq!(stored.id.as_str(), OAUTH_DEFAULT_ACCOUNT);
        assert_eq!(stored.keychain_service, "pawork.chatgpt.oauth");
        assert_eq!(stored.keychain_account, "default.access");
        assert!(stored.expires_at.is_some());

        let loaded = load_default_oauth_credential(&backend, &provider)
            .expect("load")
            .expect("present");
        assert_eq!(loaded, stored);
        let debug = format!("{loaded:?}");
        assert!(!debug.contains("access-secret-value"));
        assert!(!debug.contains("refresh-secret-value"));
    }

    #[test]
    fn missing_entry_returns_none_and_delete_is_idempotent() {
        let backend = MemoryBackend::new();
        let provider = ProviderId::new("xai");
        assert!(load_default_oauth_credential(&backend, &provider)
            .expect("load")
            .is_none());
        store_default_oauth_token(&backend, provider.clone(), &token_set(None)).expect("store");
        delete_default_oauth_token(&backend, &provider).expect("delete");
        delete_default_oauth_token(&backend, &provider).expect("delete again");
        assert!(load_default_oauth_credential(&backend, &provider)
            .expect("load")
            .is_none());
    }

    #[test]
    fn chatgpt_account_id_extracted_from_id_token() {
        let payload = serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acct-abc-123" }
        });
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
        let id_token = format!("eyJhbGciOiJub25lIn0.{payload_b64}.sig");
        let backend = MemoryBackend::new();
        store_default_oauth_token(
            &backend,
            ProviderId::new("chatgpt"),
            &token_set(Some(&id_token)),
        )
        .expect("store");
        let meta = load_default_oauth_meta(&backend, &ProviderId::new("chatgpt"))
            .expect("meta")
            .expect("present");
        assert_eq!(meta.account_id.as_deref(), Some("acct-abc-123"));
    }

    #[test]
    fn update_rotates_refresh_and_meta() {
        let backend = MemoryBackend::new();
        let provider = ProviderId::new("chatgpt");
        let mut stored =
            store_default_oauth_token(&backend, provider.clone(), &token_set(None)).expect("store");
        let rotated = TokenSet {
            access_token: "access-rotated-987654321".into(),
            refresh_token: Some("refresh-rotated-123456789".into()),
            id_token: None,
            expires_in: Some(7200),
            token_type: "Bearer".into(),
            scope: Some("openid".into()),
        };
        update_default_oauth_token(&backend, &mut stored, &rotated).expect("update");
        let loaded = load_default_oauth_credential(&backend, &provider)
            .expect("load")
            .expect("present");
        assert_eq!(loaded, stored);
        assert_eq!(loaded.scopes, vec!["openid".to_string()]);
    }
}
