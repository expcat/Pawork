//! OAuth 模块（P6-4）：PKCE / Device Flow / auto refresh / callback 接收。
//!
//! 不引入整套 `oauth2` SDK：PKCE 与 Device Flow（RFC 8628）本身很小，直接基于
//! `reqwest` + `serde` 实现，更可控、更易测试。
//!
//! ## 红线
//!
//! - 明文 access / refresh token **绝不**进入 [`StoredCredential`] 的可序列化字段，
//!   只存在于 [`SecretBackend`]（Keychain / 内存）中。
//! - 所有错误（[`AuthError`]）都不得携带明文 token。
//! - `resolve` 返回的 [`ResolvedCredential`](pawork_api::ResolvedCredential) 仅供
//!   Provider adapter 构造认证请求时短暂使用。

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use pawork_domain::{ProviderId, Timestamp};
use base64::Engine;
use pawork_api::{CredentialKind, ResolvedCredential};
use rand::RngCore;
use serde_json::Value;
use tokio::sync::{oneshot, Mutex as AsyncMutex};

use crate::backend::SecretBackend;
use crate::credential::{generate_credential_id, StoredCredential};
use crate::error::AuthError;
use crate::masked::MaskedCredential;

/// OAuth secret 在 SecretBackend 中的 service 命名空间。
const OAUTH_SERVICE_PREFIX: &str = "pawork";

/// 48 个均匀随机字节经无填充 base64url 编码后得到 64 字符 verifier，满足
/// RFC 7636 的 43-128 字符限制且没有取模偏差。
const CODE_VERIFIER_RANDOM_BYTES: usize = 48;
/// Device Flow 默认轮询间隔（秒）。
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
/// OAuth callback 请求头上限，防止本地回调端口被无界输入占满内存。
const MAX_CALLBACK_HEADER_BYTES: usize = 64 * 1024;

/// 当前 Unix 毫秒时间戳（pawork-auth 内部统一口径）。
fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn expires_at_from_now(expires_in: u64) -> Timestamp {
    Timestamp::from_unix_millis(now_unix_millis().saturating_add(expires_in.saturating_mul(1000)))
}

/// 一次 OAuth 交换得到的 token 集合（明文，仅短暂存在）。
#[derive(Clone)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// OIDC id_token（ChatGPT 流从 JWT claim 提取 account id 用）。
    /// 仅在内存短暂存在；Debug 已脱敏，绝不落盘。
    pub id_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: String,
    pub scope: Option<String>,
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenSet")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[REDACTED]"))
            .field("expires_in", &self.expires_in)
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .finish()
    }
}

/// 请求前置自动刷新所需的 OAuth token endpoint 配置。
#[derive(Clone, Debug)]
pub struct OAuthRefreshConfig {
    pub token_url: String,
    pub client_id: String,
    /// 在实际过期前多久主动刷新，吸收网络与时钟偏差。
    pub refresh_skew: Duration,
}

/// PKCE 校验器与挑战。
#[derive(Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    /// 使用的方法（固定 S256）。
    pub method: &'static str,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pkce")
            .field("verifier", &"[REDACTED]")
            .field("challenge", &self.challenge)
            .field("method", &self.method)
            .finish()
    }
}

impl Pkce {
    /// 生成新的 PKCE pair（S256）。
    pub fn generate() -> Self {
        let verifier = random_code_verifier();
        let challenge = pkce_challenge_s256(&verifier);
        Self {
            verifier,
            challenge,
            method: "S256",
        }
    }
}

/// 生成密码学随机的 code_verifier（48B 均匀随机数的 base64url 表达，长度 64）。
fn random_code_verifier() -> String {
    let mut bytes = [0u8; CODE_VERIFIER_RANDOM_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 计算 S256 code_challenge = base64url(sha256(verifier))，不含 `=` 填充。
fn pkce_challenge_s256(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// 高熵随机 state（CSRF 防护）。
pub fn random_state() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE Authorization Code Flow 配置。
#[derive(Clone, Debug)]
pub struct PkceFlowConfig {
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub provider: ProviderId,
    pub extra_auth_params: Vec<(String, String)>,
}

/// 一次待交换的 PKCE 会话（持有 verifier + state，用于在回调后换 token）。
#[derive(Clone)]
pub struct PkceSession {
    pub config: PkceFlowConfig,
    pub pkce: Pkce,
    pub state: String,
    pub auth_url: String,
}

impl fmt::Debug for PkceSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PkceSession")
            .field("config", &self.config)
            .field("pkce", &self.pkce)
            .field("state", &"[REDACTED]")
            .field("auth_url", &"[REDACTED]")
            .finish()
    }
}

/// 构造 PKCE 授权 URL 与待交换会话。
///
/// 调用方应引导用户访问 `auth_url`，收到回调 `?code=&state=` 后用
/// [`exchange_pkce_code`] 交换 token。
pub fn start_pkce_flow(config: PkceFlowConfig) -> Result<PkceSession, AuthError> {
    let pkce = Pkce::generate();
    let state = random_state();
    let auth_url = build_auth_url(
        &config.auth_url,
        &config.client_id,
        &config.redirect_uri,
        &config.scopes,
        &pkce.challenge,
        &state,
        &config.extra_auth_params,
    )?;
    Ok(PkceSession {
        config,
        pkce,
        state,
        auth_url,
    })
}

/// 用回调 code 交换 token。
///
/// `state` 必须与 [`start_pkce_flow`] 返回的 state 一致（CSRF 校验）。返回的
/// [`TokenSet`] 含明文 token，应立即经 [`store_oauth_token`] 写入 SecretBackend。
pub async fn exchange_pkce_code(
    session: &PkceSession,
    code: &str,
    returned_state: &str,
    http: &reqwest::Client,
) -> Result<TokenSet, AuthError> {
    if returned_state != session.state {
        return Err(AuthError::OAuth("state mismatch (possible CSRF)".into()));
    }
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &session.config.redirect_uri),
        ("client_id", &session.config.client_id),
        ("code_verifier", &session.pkce.verifier),
    ];
    exchange_token(http, &session.config.token_url, &params).await
}

/// Device Flow（RFC 8628）配置。
#[derive(Clone, Debug)]
pub struct DeviceFlowConfig {
    pub client_id: String,
    pub device_auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub provider: ProviderId,
}

/// Device Flow 的设备授权响应。
#[derive(Clone)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

impl fmt::Debug for DeviceAuthorization {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceAuthorization")
            .field("device_code", &"[REDACTED]")
            .field("user_code", &"[REDACTED]")
            .field("verification_uri", &self.verification_uri)
            .field(
                "verification_uri_complete",
                &self
                    .verification_uri_complete
                    .as_ref()
                    .map(|_| "[REDACTED]"),
            )
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// Device Flow 的用户引导信息（含 device_code，用于后续轮询）。
#[derive(Clone)]
pub struct DeviceUserPrompt {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub device_code: String,
    pub expires_in: u64,
    pub interval: u64,
}

impl fmt::Debug for DeviceUserPrompt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceUserPrompt")
            .field("user_code", &"[REDACTED]")
            .field("verification_uri", &self.verification_uri)
            .field(
                "verification_uri_complete",
                &self
                    .verification_uri_complete
                    .as_ref()
                    .map(|_| "[REDACTED]"),
            )
            .field("device_code", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// 请求设备授权码。
pub async fn request_device_authorization(
    config: &DeviceFlowConfig,
    http: &reqwest::Client,
) -> Result<DeviceUserPrompt, AuthError> {
    let scope = config.scopes.join(" ");
    let params: Vec<(&str, &str)> = std::iter::once(("client_id", config.client_id.as_str()))
        .chain(std::iter::once(("scope", scope.as_str())))
        .collect();
    let resp = http
        .post(&config.device_auth_url)
        .form(&params)
        .send()
        .await?;
    let status = resp.status();
    let value: Value = resp.json().await?;
    if !status.is_success() {
        return Err(AuthError::OAuth(format!(
            "device authorization failed: {}",
            extract_error(&value)
        )));
    }
    let auth = DeviceAuthorization {
        device_code: value
            .get("device_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthError::OAuth("missing device_code".into()))?
            .to_string(),
        user_code: value
            .get("user_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthError::OAuth("missing user_code".into()))?
            .to_string(),
        verification_uri: value
            .get("verification_uri")
            .or_else(|| value.get("verification_url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        verification_uri_complete: value
            .get("verification_uri_complete")
            .and_then(|v| v.as_str())
            .map(String::from),
        expires_in: value
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(1800),
        interval: value
            .get("interval")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECS),
    };
    Ok(DeviceUserPrompt {
        user_code: auth.user_code,
        verification_uri: auth.verification_uri,
        verification_uri_complete: auth.verification_uri_complete,
        device_code: auth.device_code,
        expires_in: auth.expires_in,
        interval: auth.interval,
    })
}

/// 轮询 device token endpoint 直到拿到 token 或过期。
///
/// - `authorization_pending` → 继续；
/// - `slow_down` → 增大 interval；
/// - `expired_token` → 返回 [`AuthError::ExpiredToken`]。
pub async fn poll_device_token(
    config: &DeviceFlowConfig,
    prompt: &DeviceUserPrompt,
    http: &reqwest::Client,
    max_duration: Duration,
) -> Result<TokenSet, AuthError> {
    let mut interval = prompt.interval.max(1);
    let deadline = tokio::time::Instant::now() + max_duration;
    loop {
        let params = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", prompt.device_code.as_str()),
            ("client_id", config.client_id.as_str()),
        ];
        match exchange_token(http, &config.token_url, &params).await {
            Ok(token) => return Ok(token),
            Err(AuthError::TokenEndpoint { error, .. }) if error == "authorization_pending" => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(AuthError::ExpiredToken);
                }
                tokio::time::sleep(Duration::from_secs(interval)).await;
            }
            Err(AuthError::TokenEndpoint { error, .. }) if error == "slow_down" => {
                interval += 5;
            }
            Err(AuthError::TokenEndpoint { error, .. }) if error == "expired_token" => {
                return Err(AuthError::ExpiredToken);
            }
            Err(other) => return Err(other),
        }
    }
}

/// 用 refresh_token 换新的 access_token。
///
/// 失败（refresh_token 过期等）时返回错误，调用方应重新走授权流程。
pub async fn refresh_access_token(
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
    http: &reqwest::Client,
) -> Result<TokenSet, AuthError> {
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    exchange_token(http, token_url, &params).await
}

/// 与 token endpoint 交互的通用函数。
///
/// 把 OAuth2 标准错误响应归一为 [`AuthError::TokenEndpoint`]，成功响应解析为
/// [`TokenSet`]。绝不把 token 放进错误。
async fn exchange_token(
    http: &reqwest::Client,
    token_url: &str,
    params: &[(&str, &str)],
) -> Result<TokenSet, AuthError> {
    let resp = http.post(token_url).form(params).send().await?;
    let value: Value = resp.json().await?;
    if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
        let description = value
            .get("error_description")
            .and_then(|v| v.as_str())
            .map(String::from);
        return Err(AuthError::TokenEndpoint {
            error: error.to_string(),
            description,
        });
    }
    let access_token = value
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AuthError::OAuth("token response missing access_token".into()))?
        .to_string();
    Ok(TokenSet {
        access_token,
        refresh_token: value
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(String::from),
        id_token: value
            .get("id_token")
            .and_then(|v| v.as_str())
            .map(String::from),
        expires_in: value.get("expires_in").and_then(|v| v.as_u64()),
        token_type: value
            .get("token_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Bearer")
            .to_string(),
        scope: value
            .get("scope")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

/// 把 [`TokenSet`]（明文）写入 SecretBackend，返回**仅含元数据/脱敏状态**的
/// [`StoredCredential`]。
///
/// access_token 与 refresh_token 分别存为两条 secret（service=`pawork.<provider>.oauth`，
/// account=`<cred_id>.access`/`.refresh`）。明文绝不进入返回值。
pub fn store_oauth_token(
    backend: &dyn SecretBackend,
    provider: ProviderId,
    display_name: &str,
    tokens: &TokenSet,
    scopes: Vec<String>,
) -> Result<StoredCredential, AuthError> {
    if tokens.access_token.is_empty() {
        return Err(AuthError::InvalidSecret("access_token is empty".into()));
    }
    let id = generate_credential_id();
    let service = oauth_service(&provider);
    let access_account = format!("{}.access", id.as_str());
    let refresh_account = format!("{}.refresh", id.as_str());

    backend.store(&service, &access_account, &tokens.access_token)?;
    if let Some(refresh) = &tokens.refresh_token {
        backend.store(&service, &refresh_account, refresh)?;
    }

    let stored = StoredCredential {
        masked: MaskedCredential::mask(&tokens.access_token),
        id,
        provider,
        display_name: display_name.to_string(),
        keychain_service: service,
        keychain_account: access_account,
        created_at: Timestamp::from_unix_millis(now_unix_millis()),
        expires_at: tokens.expires_in.map(expires_at_from_now),
        scopes,
    };
    Ok(stored)
}

/// 原地更新一条 OAuth credential 的 access token、可选轮换 refresh token 与
/// 过期元数据。
///
/// token 明文只写回 [`SecretBackend`]；`StoredCredential` 仅更新脱敏展示与
/// `expires_at`。刷新响应未携带 refresh token 时保留后端中的旧值。
pub fn update_oauth_token(
    backend: &dyn SecretBackend,
    stored: &mut StoredCredential,
    tokens: &TokenSet,
) -> Result<(), AuthError> {
    if tokens.access_token.is_empty() {
        return Err(AuthError::InvalidSecret("access_token is empty".into()));
    }
    let expected_service = oauth_service(&stored.provider);
    let expected_access_account = format!("{}.access", stored.id.as_str());
    if stored.keychain_service != expected_service
        || stored.keychain_account != expected_access_account
    {
        return Err(AuthError::MalformedMetadata(
            "credential is not an OAuth token record".into(),
        ));
    }
    if tokens.refresh_token.as_ref().is_some_and(String::is_empty) {
        return Err(AuthError::InvalidSecret("refresh_token is empty".into()));
    }

    // 轮换型 Provider 可能在 token endpoint 响应时立即作废旧
    // refresh token。先持久新 refresh，使后续 access 写入失败时仍可重试刷新。
    if let Some(refresh_token) = &tokens.refresh_token {
        let refresh_account = format!("{}.refresh", stored.id.as_str());
        backend.store(&stored.keychain_service, &refresh_account, refresh_token)?;
    }
    backend.store(
        &stored.keychain_service,
        &stored.keychain_account,
        &tokens.access_token,
    )?;

    stored.masked = MaskedCredential::mask(&tokens.access_token);
    // 部分 Provider 的成功 refresh 响应不返回 expires_in。此时保留原到期时间，
    // 让下一次请求继续尝试刷新，而不是把 None 误解释成“永不过期”。
    if let Some(expires_in) = tokens.expires_in {
        stored.expires_at = Some(expires_at_from_now(expires_in));
    }
    if let Some(scope) = &tokens.scope {
        stored.scopes = scope.split_whitespace().map(str::to_string).collect();
    }
    Ok(())
}

/// 从 SecretBackend 解析出 OAuth bearer credential。
pub fn resolve_oauth_credential(
    stored: &StoredCredential,
    backend: &dyn SecretBackend,
) -> Result<ResolvedCredential, AuthError> {
    let secret = backend.get(&stored.keychain_service, &stored.keychain_account)?;
    Ok(ResolvedCredential::new(CredentialKind::OAuthBearer, secret))
}

/// 读取 refresh_token（供 [`refresh_access_token`] 使用）。
pub fn read_refresh_token(
    stored: &StoredCredential,
    backend: &dyn SecretBackend,
) -> Result<String, AuthError> {
    let refresh_account = format!("{}.refresh", stored.id.as_str());
    backend
        .get(&stored.keychain_service, &refresh_account)
        .or(Err(AuthError::NotFound))
}

/// 判断 credential 是否需要 refresh（临近过期或已过期）。
pub fn needs_refresh(stored: &StoredCredential, skew: Duration) -> bool {
    match stored.expires_at {
        Some(expiry) => {
            let now = now_unix_millis();
            let skew_ms = skew.as_millis() as u64;
            expiry.as_unix_millis() <= now + skew_ms
        }
        None => false,
    }
}

#[derive(Clone)]
struct RefreshedMetadata {
    masked: MaskedCredential,
    expires_at: Option<Timestamp>,
    scopes: Vec<String>,
}

impl From<&StoredCredential> for RefreshedMetadata {
    fn from(stored: &StoredCredential) -> Self {
        Self {
            masked: stored.masked.clone(),
            expires_at: stored.expires_at,
            scopes: stored.scopes.clone(),
        }
    }
}

struct RefreshGate {
    lock: AsyncMutex<()>,
    generation: AtomicU64,
    latest: StdMutex<Option<RefreshedMetadata>>,
}

impl RefreshGate {
    fn new() -> Self {
        Self {
            lock: AsyncMutex::new(()),
            generation: AtomicU64::new(0),
            latest: StdMutex::new(None),
        }
    }

    fn apply_latest(&self, stored: &mut StoredCredential) -> bool {
        let latest = self
            .latest
            .lock()
            .expect("OAuth refresh metadata mutex poisoned")
            .clone();
        let Some(latest) = latest else {
            return false;
        };
        stored.masked = latest.masked;
        stored.expires_at = latest.expires_at;
        stored.scopes = latest.scopes;
        true
    }

    fn publish(&self, stored: &StoredCredential) {
        *self
            .latest
            .lock()
            .expect("OAuth refresh metadata mutex poisoned") =
            Some(RefreshedMetadata::from(stored));
        self.generation.fetch_add(1, Ordering::Release);
    }
}

type RefreshGateKey = (String, String);

static REFRESH_GATES: OnceLock<StdMutex<HashMap<RefreshGateKey, Arc<RefreshGate>>>> =
    OnceLock::new();

fn refresh_gate_for(stored: &StoredCredential) -> Arc<RefreshGate> {
    let gates = REFRESH_GATES.get_or_init(|| StdMutex::new(HashMap::new()));
    let key = (
        stored.keychain_service.clone(),
        stored.keychain_account.clone(),
    );
    let mut gates = gates.lock().expect("OAuth refresh gate mutex poisoned");
    gates
        .entry(key)
        .or_insert_with(|| Arc::new(RefreshGate::new()))
        .clone()
}

/// 请求前置刷新编排：需要刷新时读取旧 refresh token、调用 token endpoint，
/// 再把轮换后的 access/refresh token 与过期元数据原地回写。同一 credential 的
/// 并发请求共用 singleflight gate，避免并行消费同一个一次性 refresh token。
pub async fn refresh_oauth_credential_if_needed(
    stored: &mut StoredCredential,
    backend: &dyn SecretBackend,
    config: &OAuthRefreshConfig,
    http: &reqwest::Client,
) -> Result<bool, AuthError> {
    if !needs_refresh(stored, config.refresh_skew) {
        return Ok(false);
    }

    let gate = refresh_gate_for(stored);
    let observed_generation = gate.generation.load(Ordering::Acquire);
    let _guard = gate.lock.lock().await;

    // 若等待期间已有同 credential 的请求完成刷新，复用其脱敏元数据与后端中
    // 已写回的 token，不再次调用 token endpoint。
    if gate.generation.load(Ordering::Acquire) != observed_generation && gate.apply_latest(stored) {
        return Ok(false);
    }
    if !needs_refresh(stored, config.refresh_skew) {
        return Ok(false);
    }

    let refresh_token = read_refresh_token(stored, backend)?;
    let tokens =
        refresh_access_token(&config.token_url, &config.client_id, &refresh_token, http).await?;
    update_oauth_token(backend, stored, &tokens)?;
    gate.publish(stored);
    Ok(true)
}

/// Provider 构造或每次请求前使用的 OAuth credential 解析入口。
///
/// 与 [`resolve_oauth_credential`] 相比，此入口先执行 auto-refresh，并保证刷新
/// 响应中的轮换 refresh token 已写回 SecretBackend 后才返回 bearer credential。
pub async fn resolve_oauth_credential_for_request(
    stored: &mut StoredCredential,
    backend: &dyn SecretBackend,
    config: &OAuthRefreshConfig,
    http: &reqwest::Client,
) -> Result<ResolvedCredential, AuthError> {
    refresh_oauth_credential_if_needed(stored, backend, config, http).await?;
    resolve_oauth_credential(stored, backend)
}

/// 最小化一次性回调服务器：监听 `port`，接收 `GET /?code=&state=`，通过 channel
/// 返回 `(code, state)`，然后返回固定纯文本提示并关闭。
pub struct CallbackServer {
    addr: SocketAddr,
    rx: Option<oneshot::Receiver<Result<(String, String), AuthError>>>,
}

impl CallbackServer {
    /// 在指定端口启动回调服务器。返回后即开始监听。
    pub fn start(port: u16) -> Result<Self, AuthError> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let (tx, rx) = oneshot::channel();

        // 在后台 tokio 任务中 accept 单个连接
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| AuthError::Callback(e.to_string()))?;
        let _task = runtime.spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener)
                .expect("convert std TcpListener to tokio");
            // 最多等待 5 分钟一个连接
            let accept = tokio::time::timeout(Duration::from_secs(300), listener.accept()).await;
            let result = match accept {
                Ok(Ok((mut stream, _))) => handle_callback_connection(&mut stream).await,
                Ok(Err(e)) => Err(AuthError::Callback(e.to_string())),
                Err(_) => Err(AuthError::Callback("callback timed out".into())),
            };
            let _ = tx.send(result);
        });

        Ok(Self { addr, rx: Some(rx) })
    }

    /// 返回监听地址（用于构造 redirect_uri）。
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// 用实际监听端口回填 redirect URI，并校验只使用本机 HTTP 回调地址。
    pub fn bind_redirect_uri(&self, configured: &str) -> Result<String, AuthError> {
        let mut url = url::Url::parse(configured)?;
        if url.scheme() != "http" {
            return Err(AuthError::Callback(
                "OAuth callback redirect_uri must use http".into(),
            ));
        }
        let host = url.host_str().unwrap_or_default();
        if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
            return Err(AuthError::Callback(
                "OAuth callback redirect_uri must use a loopback host".into(),
            ));
        }
        if let Some(port) = url.port() {
            if port != 0 && port != self.addr.port() {
                return Err(AuthError::Callback(format!(
                    "redirect_uri port {port} does not match callback listener port {}",
                    self.addr.port()
                )));
            }
        }
        url.set_host(Some(&self.addr.ip().to_string()))
            .map_err(|_| AuthError::Callback("invalid callback redirect_uri host".into()))?;
        url.set_port(Some(self.addr.port()))
            .map_err(|_| AuthError::Callback("invalid callback redirect_uri port".into()))?;
        Ok(url.to_string())
    }

    /// 等待授权码（消费 self）。超时返回错误。
    pub async fn wait_for_code(mut self, timeout: Duration) -> Result<(String, String), AuthError> {
        let rx = self.rx.take().expect("channel consumed once");
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(pair))) => Ok(pair),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err(AuthError::Callback("callback sender dropped".into())),
            Err(_) => Err(AuthError::Callback("wait_for_code timed out".into())),
        }
    }
}

/// 绑定一次性 callback server，并在生成授权 URL 前把实际端口回填到
/// `PkceFlowConfig.redirect_uri`。
pub fn start_pkce_flow_with_callback(
    mut config: PkceFlowConfig,
) -> Result<(PkceSession, CallbackServer), AuthError> {
    let configured = url::Url::parse(&config.redirect_uri)?;
    if configured.scheme() != "http"
        || !matches!(
            configured.host_str().unwrap_or_default(),
            "127.0.0.1" | "localhost" | "::1"
        )
    {
        return Err(AuthError::Callback(
            "OAuth callback redirect_uri must use an HTTP loopback host".into(),
        ));
    }
    let port = configured.port().unwrap_or(0);
    let server = CallbackServer::start(port)?;
    config.redirect_uri = server.bind_redirect_uri(&config.redirect_uri)?;
    let session = start_pkce_flow(config)?;
    Ok((session, server))
}

/// 处理单个回调连接：解析 query，回 200 固定纯文本。
async fn handle_callback_connection(
    stream: &mut tokio::net::TcpStream,
) -> Result<(String, String), AuthError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut request_bytes = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        let remaining = MAX_CALLBACK_HEADER_BYTES.saturating_sub(request_bytes.len());
        if remaining == 0 {
            return Err(AuthError::Callback(
                "callback request headers exceed 64 KiB".into(),
            ));
        }
        let read_len = remaining.min(chunk.len());
        let n = stream.read(&mut chunk[..read_len]).await?;
        if n == 0 {
            return Err(AuthError::Callback(
                "callback connection closed before request headers completed".into(),
            ));
        }
        request_bytes.extend_from_slice(&chunk[..n]);
        if headers_complete(&request_bytes) {
            break;
        }
    }
    let request = std::str::from_utf8(&request_bytes)
        .map_err(|_| AuthError::Callback("callback request headers are not UTF-8".into()))?;

    // 解析请求行 GET /path?query HTTP/1.1
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("");
    let query = path.split('?').nth(1).unwrap_or("");
    let params = parse_query(query);

    let authorization_failed = params.contains_key("error");
    let body = if authorization_failed {
        "Authorization failed. Return to Pawork and retry."
    } else {
        "Authorization complete. You may close this window."
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;

    if authorization_failed {
        return Err(AuthError::Callback(
            "authorization server returned an error".into(),
        ));
    }

    let code = params
        .get("code")
        .cloned()
        .ok_or_else(|| AuthError::Callback("missing code in callback".into()))?;
    let state = params
        .get("state")
        .cloned()
        .ok_or_else(|| AuthError::Callback("missing state in callback".into()))?;
    Ok((code, state))
}

fn headers_complete(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|window| window == b"\r\n\r\n")
        || bytes.windows(2).any(|window| window == b"\n\n")
}

/// 解析 URL query string（`code=xxx&state=yyy`）为 map。
fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut split = pair.splitn(2, '=');
        let key = percent_decode(split.next().unwrap_or(""));
        let value = percent_decode(split.next().unwrap_or(""));
        map.insert(key, value);
    }
    map
}

/// 简易 percent-decoding（处理 %XX）。
fn percent_decode(input: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = input.bytes().peekable();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(hi), Some(lo)) = (hi, lo) {
                if let (Some(hv), Some(lv)) = (hex_val(hi), hex_val(lo)) {
                    bytes.push(hv * 16 + lv);
                    continue;
                }
                bytes.push(b'%');
                bytes.push(hi);
                bytes.push(lo);
            } else {
                bytes.push(b'%');
            }
        } else if b == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn oauth_service(provider: &ProviderId) -> String {
    format!("{OAUTH_SERVICE_PREFIX}.{provider}.oauth")
}

/// 解码 JWT payload 段（base64url，不验签）；仅供提取非机密 claim（如
/// ChatGPT account id 路由头）使用，不作为信任边界。
pub(crate) fn decode_jwt_payload(payload_b64: &str) -> Result<Value, AuthError> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|error| AuthError::OAuth(format!("id_token payload is not base64url: {error}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AuthError::OAuth(format!("id_token payload is not JSON: {error}")))
}

/// 构造授权 URL。
fn build_auth_url(
    base: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[String],
    code_challenge: &str,
    state: &str,
    extra: &[(String, String)],
) -> Result<String, AuthError> {
    let mut url = url::Url::parse(base)?;
    let mut query = url.query_pairs_mut();
    query.append_pair("response_type", "code");
    query.append_pair("client_id", client_id);
    query.append_pair("redirect_uri", redirect_uri);
    if !scopes.is_empty() {
        query.append_pair("scope", &scopes.join(" "));
    }
    query.append_pair("code_challenge", code_challenge);
    query.append_pair("code_challenge_method", "S256");
    query.append_pair("state", state);
    for (k, v) in extra {
        query.append_pair(k, v);
    }
    drop(query);
    Ok(url.to_string())
}

/// 从 JSON 错误响应中提取 `error` / `error_description`。
fn extract_error(value: &Value) -> String {
    let error = value
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let desc = value
        .get("error_description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if desc.is_empty() {
        error.to_string()
    } else {
        format!("{error}: {desc}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MemoryBackend;
    use crate::credential::CredentialId;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn pkce_verifier_length_and_charset() {
        let pkce = Pkce::generate();
        assert!((43..=128).contains(&pkce.verifier.len()));
        assert!(pkce.verifier.chars().all(|c| c.is_ascii_alphanumeric()
            || c == '-'
            || c == '.'
            || c == '_'
            || c == '~'));
        assert_eq!(pkce.method, "S256");
        // challenge 为 base64url（无填充），长度可变但非空
        assert!(!pkce.challenge.is_empty());
        assert!(!pkce.challenge.contains('='));
    }

    #[test]
    fn pkce_verifier_is_unbiased_base64url_of_random_bytes() {
        for _ in 0..256 {
            let verifier = random_code_verifier();
            let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&verifier)
                .expect("valid base64url verifier");
            assert_eq!(decoded.len(), CODE_VERIFIER_RANDOM_BYTES);
            assert_eq!(verifier.len(), 64);
        }
    }

    #[test]
    fn pkce_challenge_is_deterministic_for_same_verifier() {
        // S256 challenge = base64url(sha256(verifier))
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = pkce_challenge_s256(verifier);
        // RFC 7636 附录 B 的示例值
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn state_is_high_entropy_and_url_safe() {
        let s = random_state();
        assert!(s.len() >= 32);
        assert!(s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn oauth_debug_output_redacts_ephemeral_secrets() {
        let tokens = TokenSet {
            access_token: "access-secret".into(),
            refresh_token: Some("refresh-secret".into()),
            id_token: None,
            expires_in: Some(3600),
            token_type: "Bearer".into(),
            scope: Some("read".into()),
        };
        let token_debug = format!("{tokens:?}");
        assert!(!token_debug.contains("access-secret"));
        assert!(!token_debug.contains("refresh-secret"));

        let session = PkceSession {
            config: PkceFlowConfig {
                client_id: "client-id".into(),
                auth_url: "https://example.com/authorize".into(),
                token_url: "https://example.com/token".into(),
                redirect_uri: "http://127.0.0.1:0/callback".into(),
                scopes: vec!["read".into()],
                provider: ProviderId::new("xai"),
                extra_auth_params: Vec::new(),
            },
            pkce: Pkce {
                verifier: "verifier-secret".into(),
                challenge: "public-challenge".into(),
                method: "S256",
            },
            state: "state-secret".into(),
            auth_url: "https://example.com/authorize?state=state-secret".into(),
        };
        let session_debug = format!("{session:?}");
        assert!(!session_debug.contains("verifier-secret"));
        assert!(!session_debug.contains("state-secret"));

        let prompt = DeviceUserPrompt {
            user_code: "USER-SECRET".into(),
            verification_uri: "https://example.com/device".into(),
            verification_uri_complete: Some(
                "https://example.com/device?user_code=USER-SECRET".into(),
            ),
            device_code: "DEVICE-SECRET".into(),
            expires_in: 300,
            interval: 5,
        };
        let prompt_debug = format!("{prompt:?}");
        assert!(!prompt_debug.contains("USER-SECRET"));
        assert!(!prompt_debug.contains("DEVICE-SECRET"));
    }

    #[test]
    fn store_oauth_token_keeps_plaintext_out_of_metadata() {
        let backend = MemoryBackend::new();
        let tokens = TokenSet {
            access_token: "ya29.access-secret-token-abcdefgh".into(),
            refresh_token: Some("1//refresh-secret-token-12345".into()),
            id_token: None,
             expires_in: Some(3600),
            token_type: "Bearer".into(),
            scope: Some("read write".into()),
        };
        let stored = store_oauth_token(
            &backend,
            ProviderId::new("google"),
            "Google OAuth",
            &tokens,
            vec!["read".into(), "write".into()],
        )
        .expect("store");

        // 返回值与序列化都不含明文 token
        assert!(!format!("{stored:?}").contains("access-secret-token"));
        assert!(!format!("{stored:?}").contains("refresh-secret-token"));
        assert!(!serde_json::to_string(&stored)
            .unwrap()
            .contains("secret-token"));
        assert_eq!(stored.provider, ProviderId::new("google"));
        assert_eq!(stored.scopes, vec!["read".to_string(), "write".to_string()]);
        assert!(stored.expires_at.is_some());

        // 明文进入了后端
        let access_secret = backend
            .get(&stored.keychain_service, &stored.keychain_account)
            .expect("get access");
        assert_eq!(access_secret, "ya29.access-secret-token-abcdefgh");
        let refresh_secret = backend
            .get(
                &stored.keychain_service,
                &format!("{}.refresh", stored.id.as_str()),
            )
            .expect("get refresh");
        assert_eq!(refresh_secret, "1//refresh-secret-token-12345");
    }

    #[test]
    fn resolve_oauth_credential_returns_bearer() {
        let backend = MemoryBackend::new();
        let tokens = TokenSet {
            access_token: "access-xyz".into(),
            refresh_token: None,
            id_token: None,
            expires_in: Some(100),
            token_type: "Bearer".into(),
            scope: None,
        };
        let stored = store_oauth_token(
            &backend,
            ProviderId::new("github"),
            "GitHub",
            &tokens,
            Vec::new(),
        )
        .expect("store");
        let resolved = resolve_oauth_credential(&stored, &backend).expect("resolve");
        assert_eq!(resolved.kind(), CredentialKind::OAuthBearer);
        assert_eq!(resolved.expose_secret(), "access-xyz");
        // Debug 脱敏
        assert!(!format!("{resolved:?}").contains("access-xyz"));
    }

    #[test]
    fn update_oauth_token_persists_rotated_refresh_and_expiry() {
        let backend = MemoryBackend::new();
        let mut stored = store_oauth_token(
            &backend,
            ProviderId::new("xai"),
            "Grok OAuth",
            &TokenSet {
                access_token: "old-access".into(),
                refresh_token: Some("old-refresh".into()),
                id_token: None,
                 expires_in: Some(1),
                token_type: "Bearer".into(),
                scope: Some("read".into()),
            },
            vec!["read".into()],
        )
        .expect("store");

        update_oauth_token(
            &backend,
            &mut stored,
            &TokenSet {
                access_token: "new-access".into(),
                refresh_token: Some("new-refresh".into()),
                id_token: None,
                 expires_in: Some(3600),
                token_type: "Bearer".into(),
                scope: Some("read write".into()),
            },
        )
        .expect("update");

        assert_eq!(
            backend
                .get(&stored.keychain_service, &stored.keychain_account)
                .expect("access"),
            "new-access"
        );
        assert_eq!(
            read_refresh_token(&stored, &backend).expect("refresh"),
            "new-refresh"
        );
        assert!(stored.expires_at.is_some());
        assert_eq!(stored.scopes, vec!["read", "write"]);
        assert!(!serde_json::to_string(&stored)
            .expect("serialize")
            .contains("new-access"));
    }

    #[test]
    fn update_oauth_token_preserves_expiry_when_refresh_omits_ttl() {
        let backend = MemoryBackend::new();
        let mut stored = store_oauth_token(
            &backend,
            ProviderId::new("xai"),
            "Grok OAuth",
            &TokenSet {
                access_token: "old-access".into(),
                refresh_token: Some("old-refresh".into()),
                id_token: None,
                 expires_in: Some(60),
                token_type: "Bearer".into(),
                scope: None,
            },
            Vec::new(),
        )
        .expect("store");
        let original_expiry = stored.expires_at;

        update_oauth_token(
            &backend,
            &mut stored,
            &TokenSet {
                access_token: "new-access".into(),
                refresh_token: None,
                id_token: None,
                expires_in: None,
                token_type: "Bearer".into(),
                scope: None,
            },
        )
        .expect("update");

        assert_eq!(stored.expires_at, original_expiry);
    }

    #[test]
    fn needs_refresh_respects_expiry_and_skew() {
        let stored = StoredCredential {
            masked: MaskedCredential::from_masked("x…y"),
            id: CredentialId::new("c1"),
            provider: ProviderId::new("p"),
            display_name: "p".into(),
            keychain_service: "svc".into(),
            keychain_account: "acct".into(),
            created_at: Timestamp::from_unix_millis(now_unix_millis()),
            expires_at: Some(Timestamp::from_unix_millis(now_unix_millis() + 60_000)),
            scopes: Vec::new(),
        };
        // 60s 后过期，skew 10s → 还未到 → 不需要
        assert!(!needs_refresh(&stored, Duration::from_secs(10)));
        // skew 120s → 超过 → 需要
        assert!(needs_refresh(&stored, Duration::from_secs(120)));
    }

    #[tokio::test]
    async fn exchange_pkce_code_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "AT-secret-123456",
                "refresh_token": "RT-secret-654321",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "read"
            })))
            .mount(&server)
            .await;

        let config = PkceFlowConfig {
            client_id: "cid".into(),
            auth_url: "https://example.com/auth".into(),
            token_url: format!("{}/token", server.uri()),
            redirect_uri: "http://127.0.0.1:0/cb".into(),
            scopes: vec!["read".into()],
            provider: ProviderId::new("p"),
            extra_auth_params: Vec::new(),
        };
        let session = start_pkce_flow(config).expect("start");
        let http = reqwest::Client::new();
        let token = exchange_pkce_code(&session, "the-code", &session.state, &http)
            .await
            .expect("exchange");
        assert_eq!(token.access_token, "AT-secret-123456");
        assert_eq!(token.refresh_token.as_deref(), Some("RT-secret-654321"));
        assert_eq!(token.expires_in, Some(3600));
        assert_eq!(token.token_type, "Bearer");
    }

    #[tokio::test]
    async fn exchange_pkce_code_rejects_state_mismatch() {
        let config = PkceFlowConfig {
            client_id: "cid".into(),
            auth_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            redirect_uri: "http://127.0.0.1:0/cb".into(),
            scopes: Vec::new(),
            provider: ProviderId::new("p"),
            extra_auth_params: Vec::new(),
        };
        let session = start_pkce_flow(config).expect("start");
        let http = reqwest::Client::new();
        let err = exchange_pkce_code(&session, "code", "wrong-state", &http)
            .await
            .expect_err("state mismatch");
        assert!(matches!(err, AuthError::OAuth(_)));
    }

    #[tokio::test]
    async fn token_endpoint_error_is_normalized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": "bad code"
            })))
            .mount(&server)
            .await;
        let config = PkceFlowConfig {
            client_id: "cid".into(),
            auth_url: "https://example.com/auth".into(),
            token_url: format!("{}/token", server.uri()),
            redirect_uri: "http://127.0.0.1:0/cb".into(),
            scopes: Vec::new(),
            provider: ProviderId::new("p"),
            extra_auth_params: Vec::new(),
        };
        let session = start_pkce_flow(config).expect("start");
        let http = reqwest::Client::new();
        let err = exchange_pkce_code(&session, "code", &session.state, &http)
            .await
            .expect_err("error");
        match err {
            AuthError::TokenEndpoint { error, description } => {
                assert_eq!(error, "invalid_grant");
                assert_eq!(description.as_deref(), Some("bad code"));
            }
            other => panic!("expected TokenEndpoint, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn device_flow_polls_until_success() {
        let server = MockServer::start().await;
        // device authorization
        Mock::given(method("POST"))
            .and(path("/device"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "DC",
                "user_code": "USER-CODE",
                "verification_uri": "https://example.com/device",
                "verification_uri_complete": "https://example.com/device?user_code=USER-CODE",
                "expires_in": 300,
                "interval": 1
            })))
            .mount(&server)
            .await;
        // token: 先 pending 再成功
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "authorization_pending"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "DF-access-token-secret",
                "token_type": "Bearer",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        let config = DeviceFlowConfig {
            client_id: "cid".into(),
            device_auth_url: format!("{}/device", server.uri()),
            token_url: format!("{}/token", server.uri()),
            scopes: vec!["read".into()],
            provider: ProviderId::new("p"),
        };
        let http = reqwest::Client::new();
        let prompt = request_device_authorization(&config, &http)
            .await
            .expect("device auth");
        assert_eq!(prompt.user_code, "USER-CODE");
        assert_eq!(prompt.device_code, "DC");

        let token = poll_device_token(&config, &prompt, &http, Duration::from_secs(60))
            .await
            .expect("poll");
        assert_eq!(token.access_token, "DF-access-token-secret");
    }

    #[tokio::test]
    async fn refresh_access_token_exchanges_new_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "NEW-access-secret",
                "token_type": "Bearer",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let token = refresh_access_token(
            &format!("{}/token", server.uri()),
            "cid",
            "old-refresh",
            &http,
        )
        .await
        .expect("refresh");
        assert_eq!(token.access_token, "NEW-access-secret");
        assert!(token.refresh_token.is_none());
    }

    #[tokio::test]
    async fn request_resolution_auto_refreshes_and_persists_rotation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=old-refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "new-access",
                "refresh_token": "rotated-refresh",
                "token_type": "Bearer",
                "expires_in": 3600,
                "scope": "read write"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let backend = MemoryBackend::new();
        let mut stored = store_oauth_token(
            &backend,
            ProviderId::new("xai"),
            "Grok OAuth",
            &TokenSet {
                access_token: "old-access".into(),
                refresh_token: Some("old-refresh".into()),
                id_token: None,
                 expires_in: Some(0),
                token_type: "Bearer".into(),
                scope: Some("read".into()),
            },
            vec!["read".into()],
        )
        .expect("store");
        let config = OAuthRefreshConfig {
            token_url: format!("{}/token", server.uri()),
            client_id: "client-id".into(),
            refresh_skew: Duration::from_secs(30),
        };

        let resolved = resolve_oauth_credential_for_request(
            &mut stored,
            &backend,
            &config,
            &reqwest::Client::new(),
        )
        .await
        .expect("resolve with refresh");

        assert_eq!(resolved.kind(), CredentialKind::OAuthBearer);
        assert_eq!(resolved.expose_secret(), "new-access");
        assert_eq!(
            read_refresh_token(&stored, &backend).expect("rotated refresh"),
            "rotated-refresh"
        );
        assert!(stored.expires_at.expect("expiry").as_unix_millis() > now_unix_millis());
    }

    #[tokio::test]
    async fn concurrent_refreshes_share_one_singleflight_exchange() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=old-refresh"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(serde_json::json!({
                        "access_token": "singleflight-access",
                        "refresh_token": "singleflight-refresh",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "scope": "read write"
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let backend = MemoryBackend::new();
        let stored = store_oauth_token(
            &backend,
            ProviderId::new("xai"),
            "Grok OAuth",
            &TokenSet {
                access_token: "old-access".into(),
                refresh_token: Some("old-refresh".into()),
                id_token: None,
                 expires_in: Some(0),
                token_type: "Bearer".into(),
                scope: Some("read".into()),
            },
            vec!["read".into()],
        )
        .expect("store");
        let mut first = stored.clone();
        let mut second = stored;
        let config = OAuthRefreshConfig {
            token_url: format!("{}/token", server.uri()),
            client_id: "client-id".into(),
            refresh_skew: Duration::from_secs(30),
        };
        let http = reqwest::Client::new();

        let (first_result, second_result) = tokio::join!(
            refresh_oauth_credential_if_needed(&mut first, &backend, &config, &http),
            refresh_oauth_credential_if_needed(&mut second, &backend, &config, &http)
        );
        let refreshed = [first_result.expect("first"), second_result.expect("second")];

        assert_eq!(
            refreshed
                .into_iter()
                .filter(|did_refresh| *did_refresh)
                .count(),
            1
        );
        assert_eq!(first.masked, second.masked);
        assert_eq!(first.expires_at, second.expires_at);
        assert_eq!(first.scopes, second.scopes);
        assert_eq!(
            backend
                .get(&first.keychain_service, &first.keychain_account)
                .expect("access"),
            "singleflight-access"
        );
        assert_eq!(
            read_refresh_token(&first, &backend).expect("refresh"),
            "singleflight-refresh"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn callback_server_parses_code_and_state() {
        let server = CallbackServer::start(0).expect("start");
        let addr = server.local_addr();
        let handle =
            tokio::spawn(async move { server.wait_for_code(Duration::from_secs(10)).await });
        // 模拟浏览器回调
        tokio::time::sleep(Duration::from_millis(100)).await;
        let resp = reqwest::get(format!("http://{addr}/?code=AUTH_CODE_123&state=STATE_456"))
            .await
            .expect("connect");
        assert!(resp.status().is_success());
        let (code, state) = handle.await.expect("join").expect("code");
        assert_eq!(code, "AUTH_CODE_123");
        assert_eq!(state, "STATE_456");
    }

    #[tokio::test]
    async fn callback_error_response_does_not_reflect_query_input() {
        let server = CallbackServer::start(0).expect("start");
        let addr = server.local_addr();
        let handle =
            tokio::spawn(async move { server.wait_for_code(Duration::from_secs(10)).await });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let response = reqwest::get(format!(
            "http://{addr}/callback?error=%3Cscript%3Ealert(1)%3C%2Fscript%3E"
        ))
        .await
        .expect("connect");
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/plain; charset=utf-8")
        );
        let body = response.text().await.expect("body");
        assert_eq!(body, "Authorization failed. Return to Pawork and retry.");
        assert!(!body.contains("<script>"));

        let error = handle
            .await
            .expect("join")
            .expect_err("callback should surface authorization failure");
        assert!(matches!(error, AuthError::Callback(_)));
        assert!(!error.to_string().contains("script"));
    }

    #[tokio::test]
    async fn callback_server_reads_fragmented_headers_with_large_cookie() {
        use tokio::io::AsyncWriteExt;

        let server = CallbackServer::start(0).expect("start");
        let addr = server.local_addr();
        let handle =
            tokio::spawn(async move { server.wait_for_code(Duration::from_secs(10)).await });
        let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let request = format!(
            "GET /callback?code=SPLIT_CODE&state=SPLIT_STATE HTTP/1.1\r\nHost: {addr}\r\nCookie: session={}\r\n\r\n",
            "x".repeat(8 * 1024)
        );
        let split = 1024;
        stream
            .write_all(&request.as_bytes()[..split])
            .await
            .expect("write first fragment");
        tokio::task::yield_now().await;
        stream
            .write_all(&request.as_bytes()[split..])
            .await
            .expect("write second fragment");

        let (code, state) = handle.await.expect("join").expect("code");
        assert_eq!(code, "SPLIT_CODE");
        assert_eq!(state, "SPLIT_STATE");
    }

    #[tokio::test]
    async fn pkce_callback_flow_uses_actual_listener_port() {
        let config = PkceFlowConfig {
            client_id: "cid".into(),
            auth_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            redirect_uri: "http://127.0.0.1:0/callback".into(),
            scopes: vec!["read".into()],
            provider: ProviderId::new("p"),
            extra_auth_params: Vec::new(),
        };
        let (session, server) = start_pkce_flow_with_callback(config).expect("start flow");
        let redirect = url::Url::parse(&session.config.redirect_uri).expect("redirect URL");
        assert_eq!(redirect.port(), Some(server.local_addr().port()));
        assert_ne!(redirect.port(), Some(0));
        let auth = url::Url::parse(&session.auth_url).expect("auth URL");
        let redirect_param = auth
            .query_pairs()
            .find_map(|(key, value)| (key == "redirect_uri").then(|| value.into_owned()))
            .expect("redirect_uri query");
        assert_eq!(redirect_param, session.config.redirect_uri);
    }

    #[test]
    fn percent_decode_handles_special_chars() {
        assert_eq!(percent_decode("code=abc&state=xyz"), "code=abc&state=xyz");
        let decoded = parse_query("code=hello%20world&state=a+b");
        assert_eq!(decoded.get("code"), Some(&"hello world".to_string()));
        assert_eq!(decoded.get("state"), Some(&"a b".to_string()));
    }
}
