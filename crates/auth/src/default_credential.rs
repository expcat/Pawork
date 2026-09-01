//! 每 Provider 唯一的 default OAuth 条目（S6 波 C）。
//!
//! 首发阶段每 provider 只保存一条 OAuth 凭证，使用确定性 SecretBackend 定位：
//! service = pawork.<provider>.oauth，account 为 default.access / default.refresh /
//! default.meta。meta 是仅含掩码与过期时间的 JSON（非 secret），供装配期无网络
//! 重建 StoredCredential 并判断是否需要刷新。多凭证/账号池留 S11。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use pawork_domain::{CredentialId, ProviderId, Timestamp};

use crate::backend::SecretBackend;
use crate::credential::StoredCredential;
use crate::locator::oauth_secret_service;
use crate::masked::MaskedCredential;
use crate::oauth::{
    decode_jwt_payload, needs_refresh, refresh_oauth_credential_with, OAuthRefreshConfig, TokenSet,
};
use crate::AuthError;

/// default 条目的固定 account 前缀。
pub const OAUTH_DEFAULT_ACCOUNT: &str = "default";
const REFRESH_GRACE_MILLIS: u64 = 30_000;
const CHATGPT_ACCOUNT_ID_CLAIM: &str = "chatgpt_account_id";
const CHATGPT_AUTH_CLAIM_PREFIX: &str = "https://api.openai.com/auth";

/// default OAuth 条目的非机密元数据（可安全打印/存 SecretBackend meta 条目）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefaultOAuthMeta {
    pub masked: MaskedCredential,
    pub created_at_ms: u64,
    /// Unix 毫秒；None = 上游未给出到期时间（每次请求前仍会尽量刷新）。
    pub expires_at_ms: Option<u64>,
    pub scopes: Vec<String>,
    /// ChatGPT 专用：JWT claim 中的 account id（路由头，非 secret）。
    pub account_id: Option<String>,
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
    if tokens.refresh_token.as_ref().is_some_and(String::is_empty) {
        return Err(AuthError::InvalidSecret("refresh_token is empty".into()));
    }
    let service = oauth_secret_service(&provider);
    let access_account = access_account();
    let refresh_account = refresh_account();
    let meta_account = meta_account();
    let mut updates = Vec::with_capacity(3);
    if let Some(refresh) = tokens.refresh_token.as_deref() {
        updates.push((service.as_str(), refresh_account.as_str(), refresh));
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
    let meta_json = serialize_meta(&meta)?;
    updates.push((
        service.as_str(),
        access_account.as_str(),
        tokens.access_token.as_str(),
    ));
    updates.push((service.as_str(), meta_account.as_str(), meta_json.as_str()));
    backend.store_batch(&updates)?;
    Ok(stored_from_meta(provider, meta))
}

/// 读取 default 条目元数据；条目不存在返回 None（调用方 fail-closed）。
pub fn load_default_oauth_credential(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<Option<StoredCredential>, AuthError> {
    let service = oauth_secret_service(provider);
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
    match backend.get(&oauth_secret_service(provider), &meta_account()) {
        Ok(meta_json) => serde_json::from_str(&meta_json)
            .map(Some)
            .map_err(|error| AuthError::MalformedMetadata(format!("default oauth meta: {error}"))),
        Err(AuthError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 删除 default 条目全部账户（幂等：meta 不存在时仍尝试清理 token 账户）。
pub fn delete_default_oauth_token(
    backend: &dyn SecretBackend,
    provider: &ProviderId,
) -> Result<(), AuthError> {
    let service = oauth_secret_service(provider);
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
    let service = oauth_secret_service(&stored.provider);
    if stored.secret_service != service || stored.secret_account != access_account() {
        return Err(AuthError::MalformedMetadata(
            "credential is not the default oauth entry".into(),
        ));
    }
    if tokens.access_token.is_empty() {
        return Err(AuthError::InvalidSecret("access_token is empty".into()));
    }
    if tokens.refresh_token.as_ref().is_some_and(String::is_empty) {
        return Err(AuthError::InvalidSecret("refresh_token is empty".into()));
    }
    let mut updated = stored.clone();
    updated.masked = MaskedCredential::mask(&tokens.access_token);
    if let Some(expires_in) = tokens.expires_in {
        updated.expires_at = Some(Timestamp::from_unix_millis(
            now_unix_millis().saturating_add(expires_in.saturating_mul(1000)),
        ));
    }
    if let Some(scope) = tokens.scope.as_deref() {
        updated.scopes = scope.split_whitespace().map(str::to_string).collect();
    }
    // 刷新响应通常不携带 id_token：保留旧 meta 的 account_id，避免 ChatGPT
    // 路由头信息在自动刷新后丢失。
    let previous_meta = load_default_oauth_meta(backend, &stored.provider)?;
    let account_id = chatgpt_account_id(tokens).or_else(|| {
        previous_meta
            .as_ref()
            .and_then(|meta| meta.account_id.clone())
    });
    let meta = DefaultOAuthMeta {
        masked: updated.masked.clone(),
        created_at_ms: updated.created_at.as_unix_millis(),
        expires_at_ms: updated.expires_at.map(Timestamp::as_unix_millis),
        scopes: updated.scopes.clone(),
        account_id,
    };
    let access_account = access_account();
    let refresh_account = refresh_account();
    let meta_account = meta_account();
    let meta_json = serialize_meta(&meta)?;
    let mut updates = Vec::with_capacity(3);
    if let Some(refresh) = tokens.refresh_token.as_deref() {
        updates.push((service.as_str(), refresh_account.as_str(), refresh));
    }
    updates.push((
        service.as_str(),
        access_account.as_str(),
        tokens.access_token.as_str(),
    ));
    updates.push((service.as_str(), meta_account.as_str(), meta_json.as_str()));
    backend.store_batch(&updates)?;
    *stored = updated;
    Ok(())
}

fn default_oauth_needs_refresh_with_skew(
    stored: &StoredCredential,
    refresh_skew: Duration,
) -> bool {
    stored.expires_at.is_none() || needs_refresh(stored, refresh_skew)
}

/// 到期判断（与 oauth::needs_refresh 同语义：无 expires 视为需要刷新）。
pub fn default_oauth_needs_refresh(stored: &StoredCredential) -> bool {
    default_oauth_needs_refresh_with_skew(stored, Duration::from_millis(REFRESH_GRACE_MILLIS))
}

/// default OAuth 请求前置刷新：复用通用 singleflight gate，并以 default 专用写入
/// 同步 access、轮换 refresh 与 meta。
pub async fn refresh_default_oauth_credential_if_needed(
    stored: &mut StoredCredential,
    backend: &dyn SecretBackend,
    config: &OAuthRefreshConfig,
    http: &reqwest::Client,
) -> Result<bool, AuthError> {
    refresh_oauth_credential_with(
        stored,
        backend,
        config,
        http,
        default_oauth_needs_refresh_with_skew,
        update_default_oauth_token,
        Some(reload_default_oauth_credential),
    )
    .await
}

fn reload_default_oauth_credential(
    backend: &dyn SecretBackend,
    stored: &StoredCredential,
) -> Result<Option<StoredCredential>, AuthError> {
    load_default_oauth_credential(backend, &stored.provider)
}

fn serialize_meta(meta: &DefaultOAuthMeta) -> Result<String, AuthError> {
    serde_json::to_string(meta)
        .map_err(|error| AuthError::MalformedMetadata(format!("serialize meta: {error}")))
}

fn stored_from_meta(provider: ProviderId, meta: DefaultOAuthMeta) -> StoredCredential {
    let service = oauth_secret_service(&provider);
    StoredCredential {
        id: CredentialId::new(OAUTH_DEFAULT_ACCOUNT),
        provider,
        display_name: "default oauth".into(),
        masked: meta.masked,
        secret_service: service,
        secret_account: access_account(),
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
    use crate::oauth::read_refresh_token;
    use crate::FileBackend;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PROCESS_CHILD_AUTH_PATH: &str = "PAWORK_TEST_REFRESH_AUTH_PATH";
    const PROCESS_CHILD_TOKEN_URL: &str = "PAWORK_TEST_REFRESH_TOKEN_URL";
    const PROCESS_CHILD_READY_PATH: &str = "PAWORK_TEST_REFRESH_READY_PATH";
    const PROCESS_CHILD_GO_PATH: &str = "PAWORK_TEST_REFRESH_GO_PATH";

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
        assert_eq!(stored.secret_service, "pawork.chatgpt.oauth");
        assert_eq!(stored.secret_account, "default.access");
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
    fn store_rejects_empty_refresh_without_writes() {
        let backend = MemoryBackend::new();
        let mut tokens = token_set(None);
        tokens.refresh_token = Some(String::new());

        assert!(matches!(
            store_default_oauth_token(&backend, ProviderId::new("xai"), &tokens),
            Err(AuthError::InvalidSecret(message)) if message == "refresh_token is empty"
        ));
        assert!(
            backend.is_empty(),
            "invalid token set must not be partially stored"
        );
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
        let payload_b64 = crate::base64url::encode(payload.to_string().as_bytes());
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

    #[test]
    fn update_rejects_empty_refresh_without_overwriting_valid_token() {
        let backend = MemoryBackend::new();
        let provider = ProviderId::new("chatgpt");
        let mut stored =
            store_default_oauth_token(&backend, provider.clone(), &token_set(None)).expect("store");
        let before_stored = stored.clone();
        let before_access = backend
            .get(&stored.secret_service, &stored.secret_account)
            .expect("old access");
        let before_refresh = read_refresh_token(&stored, &backend).expect("old refresh");
        let before_meta = load_default_oauth_meta(&backend, &provider)
            .expect("old meta")
            .expect("meta present");

        let invalid = TokenSet {
            access_token: "must-not-overwrite-access".into(),
            refresh_token: Some(String::new()),
            id_token: None,
            expires_in: Some(7200),
            token_type: "Bearer".into(),
            scope: Some("changed".into()),
        };
        assert!(matches!(
            update_default_oauth_token(&backend, &mut stored, &invalid),
            Err(AuthError::InvalidSecret(message)) if message == "refresh_token is empty"
        ));

        assert_eq!(stored, before_stored);
        assert_eq!(
            backend
                .get(&stored.secret_service, &stored.secret_account)
                .expect("access unchanged"),
            before_access
        );
        assert_eq!(
            read_refresh_token(&stored, &backend).expect("refresh unchanged"),
            before_refresh
        );
        assert_eq!(
            load_default_oauth_meta(&backend, &provider)
                .expect("meta unchanged")
                .expect("meta present"),
            before_meta
        );
    }

    #[tokio::test]
    async fn delayed_stale_snapshot_reuses_published_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=old-refresh-1111"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-access-token-2222",
                "refresh_token": "new-refresh-token-2222",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "openid profile"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let backend = MemoryBackend::new();
        let provider = ProviderId::new("delayed-stale");
        let stored = store_default_oauth_token(
            &backend,
            provider,
            &TokenSet {
                access_token: "old-access-token-1111".into(),
                refresh_token: Some("old-refresh-1111".into()),
                id_token: None,
                expires_in: Some(0),
                token_type: "Bearer".into(),
                scope: Some("openid".into()),
            },
        )
        .expect("store");
        let mut first = stored.clone();
        let mut delayed = stored;
        let config = OAuthRefreshConfig {
            token_url: format!("{}/token", server.uri()),
            client_id: "client-id".into(),
            refresh_skew: Duration::from_secs(30),
        };
        let http = reqwest::Client::new();

        assert!(
            refresh_default_oauth_credential_if_needed(&mut first, &backend, &config, &http,)
                .await
                .expect("first refresh")
        );
        assert!(!refresh_default_oauth_credential_if_needed(
            &mut delayed,
            &backend,
            &config,
            &http,
        )
        .await
        .expect("reuse published refresh"));
        assert_eq!(delayed.masked, first.masked);
        assert_eq!(delayed.expires_at, first.expires_at);
        assert_eq!(delayed.scopes, first.scopes);
        server.verify().await;
    }

    #[test]
    #[ignore = "helper invoked by file_backend_refresh_is_single_exchange_across_processes"]
    fn cross_process_refresh_child() {
        let Some(auth_path) = std::env::var_os(PROCESS_CHILD_AUTH_PATH) else {
            return;
        };
        let token_url = std::env::var(PROCESS_CHILD_TOKEN_URL).expect("child token URL");
        let ready_path = std::env::var_os(PROCESS_CHILD_READY_PATH).expect("child ready path");
        let go_path = std::env::var_os(PROCESS_CHILD_GO_PATH).expect("child go path");
        let backend = FileBackend::with_path(auth_path);
        let provider = ProviderId::new("cross-process-xai");
        let mut stored = load_default_oauth_credential(&backend, &provider)
            .expect("child load")
            .expect("child credential");

        std::fs::write(&ready_path, b"ready").expect("child ready marker");
        while !std::path::Path::new(&go_path).exists() {
            std::thread::sleep(Duration::from_millis(5));
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("child runtime");
        runtime
            .block_on(refresh_default_oauth_credential_if_needed(
                &mut stored,
                &backend,
                &OAuthRefreshConfig {
                    token_url,
                    client_id: "process-client".into(),
                    refresh_skew: Duration::from_secs(30),
                },
                &reqwest::Client::new(),
            ))
            .expect("child refresh");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn file_backend_refresh_is_single_exchange_across_processes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=process-old-refresh"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(250))
                    .set_body_json(serde_json::json!({
                        "access_token": "process-new-access",
                        "refresh_token": "process-new-refresh",
                        "expires_in": 3600,
                        "token_type": "Bearer",
                        "scope": "openid profile"
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let directory = std::env::temp_dir().join(format!(
            "pawork-cross-process-refresh-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create process test dir");
        let auth_path = directory.join("auth.json");
        let go_path = directory.join("go");
        let ready_paths = [directory.join("ready-1"), directory.join("ready-2")];
        let backend = FileBackend::with_path(&auth_path);
        let provider = ProviderId::new("cross-process-xai");
        store_default_oauth_token(
            &backend,
            provider.clone(),
            &TokenSet {
                access_token: "process-old-access".into(),
                refresh_token: Some("process-old-refresh".into()),
                id_token: None,
                expires_in: Some(0),
                token_type: "Bearer".into(),
                scope: Some("openid".into()),
            },
        )
        .expect("store process credential");

        let executable = std::env::current_exe().expect("current test executable");
        let spawn_child = |ready_path: &std::path::Path| {
            std::process::Command::new(&executable)
                .arg("--exact")
                .arg("default_credential::tests::cross_process_refresh_child")
                .arg("--ignored")
                .arg("--nocapture")
                .env(PROCESS_CHILD_AUTH_PATH, &auth_path)
                .env(PROCESS_CHILD_TOKEN_URL, format!("{}/token", server.uri()))
                .env(PROCESS_CHILD_READY_PATH, ready_path)
                .env(PROCESS_CHILD_GO_PATH, &go_path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn refresh child")
        };
        let mut first = spawn_child(&ready_paths[0]);
        let mut second = spawn_child(&ready_paths[1]);

        let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !ready_paths.iter().all(|path| path.exists()) {
            assert!(
                tokio::time::Instant::now() < ready_deadline,
                "refresh child did not reach the stale-snapshot barrier"
            );
            assert!(first.try_wait().expect("poll first child").is_none());
            assert!(second.try_wait().expect("poll second child").is_none());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        std::fs::write(&go_path, b"go").expect("release process barrier");

        let (first_output, second_output) = tokio::join!(
            tokio::task::spawn_blocking(move || first.wait_with_output()),
            tokio::task::spawn_blocking(move || second.wait_with_output())
        );
        for output in [first_output, second_output] {
            let output = output
                .expect("join child waiter")
                .expect("wait for refresh child");
            assert!(
                output.status.success(),
                "refresh child failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let stored = load_default_oauth_credential(&backend, &provider)
            .expect("load refreshed credential")
            .expect("refreshed credential");
        assert_eq!(
            backend
                .get(&stored.secret_service, &stored.secret_account)
                .expect("rotated access"),
            "process-new-access"
        );
        assert_eq!(
            read_refresh_token(&stored, &backend).expect("rotated refresh"),
            "process-new-refresh"
        );
        server.verify().await;
    }
}
