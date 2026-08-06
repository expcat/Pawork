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
//! - `resolve` 返回的 [`ResolvedCredential`](provider_api::ResolvedCredential) 仅供
//!   Provider adapter 构造认证请求时短暂使用。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use agent_domain::{ProviderId, Timestamp};
use base64::Engine;
use provider_api::{CredentialKind, ResolvedCredential};
use rand::RngCore;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::backend::SecretBackend;
use crate::credential::{CredentialId, StoredCredential};
use crate::error::AuthError;
use crate::masked::MaskedCredential;

/// OAuth secret 在 SecretBackend 中的 service 命名空间。
const OAUTH_SERVICE_PREFIX: &str = "pawork";

/// PKCE code_verifier 长度（RFC 7636：43-128 字符）。
const CODE_VERIFIER_LEN: usize = 64;
/// code_verifier 字符集（unreserved，RFC 7636）。
const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
/// Device Flow 默认轮询间隔（秒）。
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;

/// 当前 Unix 毫秒时间戳（auth-service 内部统一口径）。
fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// 一次 OAuth 交换得到的 token 集合（明文，仅短暂存在）。
#[derive(Clone, Debug)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_type: String,
    pub scope: Option<String>,
}

/// PKCE 校验器与挑战。
#[derive(Clone, Debug)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    /// 使用的方法（固定 S256）。
    pub method: &'static str,
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

/// 生成密码学随机的 code_verifier（unreserved 字符集，长度 64）。
fn random_code_verifier() -> String {
    let mut bytes = [0u8; CODE_VERIFIER_LEN];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| UNRESERVED[(*b % UNRESERVED.len() as u8) as usize] as char)
        .collect()
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
#[derive(Clone, Debug)]
pub struct PkceSession {
    pub config: PkceFlowConfig,
    pub pkce: Pkce,
    pub state: String,
    pub auth_url: String,
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
#[derive(Clone, Debug)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

/// Device Flow 的用户引导信息（含 device_code，用于后续轮询）。
#[derive(Clone, Debug)]
pub struct DeviceUserPrompt {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub device_code: String,
    pub expires_in: u64,
    pub interval: u64,
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
    let id = CredentialId::generate();
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
        expires_at: tokens
            .expires_in
            .map(|secs| Timestamp::from_unix_millis(now_unix_millis() + secs * 1000)),
        scopes,
    };
    Ok(stored)
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

/// 最小化一次性回调服务器：监听 `port`，接收 `GET /?code=&state=`，通过 channel
/// 返回 `(code, state)`，然后返回成功 HTML 并关闭。
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

/// 处理单个回调连接：解析 query，回 200 HTML。
async fn handle_callback_connection(
    stream: &mut tokio::net::TcpStream,
) -> Result<(String, String), AuthError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = std::str::from_utf8(&buf[..n]).unwrap_or("");

    // 解析请求行 GET /path?query HTTP/1.1
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("");
    let query = path.split('?').nth(1).unwrap_or("");
    let params = parse_query(query);

    let body = if params.contains_key("error") {
        format!(
            "<h1>Authorization failed: {}</h1>",
            params.get("error").cloned().unwrap_or_default()
        )
    } else {
        "<h1>Authorization complete. You may close this window.</h1>".to_string()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;

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

fn oauth_service(provider: &ProviderId) -> String {
    format!("{OAUTH_SERVICE_PREFIX}.{provider}.oauth")
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
    use wiremock::matchers::{method, path};
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
    fn store_oauth_token_keeps_plaintext_out_of_metadata() {
        let backend = MemoryBackend::new();
        let tokens = TokenSet {
            access_token: "ya29.access-secret-token-abcdefgh".into(),
            refresh_token: Some("1//refresh-secret-token-12345".into()),
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

    #[test]
    fn percent_decode_handles_special_chars() {
        assert_eq!(percent_decode("code=abc&state=xyz"), "code=abc&state=xyz");
        let decoded = parse_query("code=hello%20world&state=a+b");
        assert_eq!(decoded.get("code"), Some(&"hello world".to_string()));
        assert_eq!(decoded.get("state"), Some(&"a b".to_string()));
    }
}
