//! 可复用的「OAuth 官方额度接口」适配器。
//!
//! 与 [`super::api_key::ApiKeyQuotaAdapter`] 的区别在于凭证来源：本适配器通过
//! 注入的 [`OAuthCredentialSource`] 获取 bearer token，该 source 在内部围绕
//! auth-service 的 refresh + singleflight 编排。命中 401 时适配器**仅重试一次**：
//! 调用 [`OAuthCredentialSource::refresh_after_unauthorized`] 获取当前有效 token 后重发；
//! 若刷新因 refresh token 失效 / 被撤销而失败，则映射为
//! [`QuotaError::ReauthorizationRequired`]，交由上层引导用户重新授权。
//!
//! 硬化约束（P14-3 review）：
//! - access token 只以 [`ResolvedCredential`] 短暂存在于内存调用栈，不持久化、
//!   不进入 provenance / 错误 / 审计文本；明文 token 一律经 [`redact_secrets`] 兜底；
//! - 首次 401 最多刷新一次，退避后仅用当前 token 重试一次；并发请求若发现 token
//!   已被其它请求轮换则直接复用，重试再 401 不再刷新；
//! - 403 / 429 不触发刷新；`invalid_grant` / refresh 失效映射为
//!   [`QuotaError::ReauthorizationRequired`]；
//! - 取消在请求 / 刷新 / 退避 / 锁等待各路径均优先生效；
//! - 并发只使用 tokio 异步锁，不在任何 await 期间持有 std 锁；
//! - 凭据 kind / scope（provider、钉住的 credential_id）不匹配在联网前失败。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::{CancellationToken, Timestamp};
use async_trait::async_trait;
use auth_service::{
    refresh_oauth_credential_if_needed, resolve_oauth_credential, AuthError, OAuthRefreshConfig,
    SecretBackend, StoredCredential,
};
use provider_api::{CredentialKind, ResolvedCredential};
use provider_runtime::http::HttpClient;
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};

use crate::{
    AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaProvenance, QuotaRequest, QuotaReset,
    QuotaSnapshot, QuotaValues,
};

use super::http_util::{
    api_get, bearer_headers, now_millis, redact_endpoint, redact_secrets, sleep_or_cancel,
};

/// 401 → 强制刷新 → 重试前的短暂退避，吸收刷新传播与时钟偏差；取消优先。
const RETRY_BACKOFF: Duration = Duration::from_millis(50);

/// 联网前校验凭证 kind：OAuth 适配器只接受 [`CredentialKind::OAuthBearer`]，
/// 其它 kind（如误配的 API key）一律在发起请求前失败。
fn ensure_bearer(credential: &ResolvedCredential) -> Result<(), QuotaError> {
    if credential.kind() != CredentialKind::OAuthBearer {
        return Err(QuotaError::unauthorized("oauth bearer credential required"));
    }
    Ok(())
}

/// 注入的 OAuth 凭证来源。封装 auth-service 的 refresh/singleflight。
#[async_trait]
pub trait OAuthCredentialSource: Send + Sync {
    /// 解析一个可用于请求的 bearer 凭证（必要时先刷新）。
    async fn resolve(
        &self,
        request: &QuotaRequest,
        cancel: &CancellationToken,
    ) -> Result<ResolvedCredential, QuotaError>;

    /// 在收到 401 后返回可重试凭证。`rejected` 是本次被拒的 bearer；若 source
    /// 内当前 token 已被并发请求轮换则直接复用，否则才强制刷新。refresh token
    /// 失效 / 被撤销时返回 [`QuotaError::ReauthorizationRequired`]。
    async fn refresh_after_unauthorized(
        &self,
        request: &QuotaRequest,
        rejected: &ResolvedCredential,
        cancel: &CancellationToken,
    ) -> Result<ResolvedCredential, QuotaError>;
}

/// Provider 侧胶水：单端点 OAuth 额度接口的取数与解析规则。
pub trait OAuthQuotaEndpoint: Send + Sync {
    fn supports(&self, request: &QuotaRequest) -> bool;
    fn endpoint(&self, request: &QuotaRequest) -> String;
    fn source(&self) -> &'static str;
    fn parse(
        &self,
        request: &QuotaRequest,
        body: serde_json::Value,
    ) -> Result<(QuotaValues, QuotaReset), QuotaError>;
}

/// 通用 OAuth 配额适配器。
pub struct OAuthQuotaAdapter {
    http: Arc<HttpClient>,
    endpoint: Box<dyn OAuthQuotaEndpoint>,
    credential_source: Arc<dyn OAuthCredentialSource>,
}

impl OAuthQuotaAdapter {
    pub fn new(
        http: Arc<HttpClient>,
        endpoint: Box<dyn OAuthQuotaEndpoint>,
        credential_source: Arc<dyn OAuthCredentialSource>,
    ) -> Self {
        Self {
            http,
            endpoint,
            credential_source,
        }
    }
}

#[async_trait]
impl QuotaAdapter for OAuthQuotaAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::OAuthApi
    }

    fn supports(&self, request: &QuotaRequest) -> bool {
        self.endpoint.supports(request)
    }

    async fn fetch(
        &self,
        request: &QuotaRequest,
        _credential: Option<&ResolvedCredential>,
        cancel: &CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let cred = self.credential_source.resolve(request, cancel).await?;
        ensure_bearer(&cred)?;
        let url = self.endpoint.endpoint(request);
        let first = api_get(self.http.as_ref(), &url, &bearer_headers(&cred), cancel).await;
        let body = match first {
            Ok(body) => body,
            Err(QuotaError::Unauthorized { .. }) => {
                // 401 → 强制刷新恰好一次，退避后用轮换后的 token 重试一次；
                // 重试仍 401 则原样上抛，不再触发第二次刷新（retry 一次上限）。
                let refreshed = self
                    .credential_source
                    .refresh_after_unauthorized(request, &cred, cancel)
                    .await?;
                ensure_bearer(&refreshed)?;
                sleep_or_cancel(RETRY_BACKOFF, cancel).await?;
                api_get(
                    self.http.as_ref(),
                    &url,
                    &bearer_headers(&refreshed),
                    cancel,
                )
                .await?
            }
            Err(other) => return Err(other),
        };

        let (values, reset) = self.endpoint.parse(request, body)?;
        let now = now_millis();
        let provenance = QuotaProvenance {
            adapter_kind: AdapterKind::OAuthApi,
            source: self.endpoint.source().to_string(),
            endpoint: Some(redact_endpoint(&url)),
            fetched_at: now,
            observed_at: None,
            selector_version: None,
            stale: false,
        };
        Ok(QuotaSnapshot {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
            values,
            reset,
            confidence: Confidence::Exact,
            provenance,
        })
    }
}

/// 把 auth-service 的错误归一为配额错误。
///
/// refresh token 失效 / 被撤销 / 凭证缺失统一映射为
/// [`QuotaError::ReauthorizationRequired`]；网络类错误映射为 [`QuotaError::Other`]。
pub fn map_auth_error(error: AuthError) -> QuotaError {
    match error {
        AuthError::TokenEndpoint { ref error, .. } if is_reauth_error(error) => {
            QuotaError::reauthorization_required(redact_secrets(error))
        }
        AuthError::ExpiredToken | AuthError::NotFound => {
            QuotaError::reauthorization_required("oauth credential missing or expired")
        }
        AuthError::InvalidSecret(detail) => {
            QuotaError::reauthorization_required(redact_secrets(&detail))
        }
        AuthError::Http(_) | AuthError::Io(_) | AuthError::Callback(_) => {
            QuotaError::other(redact_secrets(&error.to_string()))
        }
        other => QuotaError::other(redact_secrets(&other.to_string())),
    }
}

fn is_reauth_error(code: &str) -> bool {
    matches!(
        code,
        "invalid_grant" | "invalid_request" | "revoked" | "expired_token" | "invalid_client"
    )
}

/// 围绕 auth-service 的具体 OAuth 凭证来源。
///
/// 持有一条可变 [`StoredCredential`]（受 mutex 保护）、一个 [`SecretBackend`]、
/// 刷新配置与 reqwest 客户端。`resolve` 走「按需刷新」，`refresh_after_unauthorized`
/// 在锁内比较当前 bearer 与本次被拒 token：不同则复用，相同才强制刷新。解析出的
/// access token 只以
/// [`ResolvedCredential`] 短暂存在于调用栈，不持久化、不进入任何错误 / 审计文本。
/// 锁仅用 tokio 异步互斥量，任何 await 期间不持有 std 锁；锁等待本身也与取消竞争。
pub struct AuthServiceOAuthSource {
    backend: Arc<dyn SecretBackend>,
    config: OAuthRefreshConfig,
    http: reqwest::Client,
    stored: AsyncMutex<StoredCredential>,
}

impl AuthServiceOAuthSource {
    pub fn new(
        stored: StoredCredential,
        backend: Arc<dyn SecretBackend>,
        config: OAuthRefreshConfig,
        http: reqwest::Client,
    ) -> Self {
        Self {
            backend,
            config,
            http,
            stored: AsyncMutex::new(stored),
        }
    }

    /// 与取消竞争地获取 stored 锁：即使排在另一任务的长刷新之后，取消也能及时返回。
    async fn lock_stored(
        &self,
        cancel: &CancellationToken,
    ) -> Result<MutexGuard<'_, StoredCredential>, QuotaError> {
        let lock = self.stored.lock();
        tokio::pin!(lock);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(QuotaError::Cancelled),
            guard = &mut lock => Ok(guard),
        }
    }

    /// 联网前校验：stored credential 的 provider 必须与请求 scope 一致；请求钉住
    /// credential_id 时还须与 stored.id 一致。不匹配直接失败，绝不发起刷新或
    /// 额度接口请求。
    fn check_scope(request: &QuotaRequest, stored: &StoredCredential) -> Result<(), QuotaError> {
        if stored.provider != request.scope.provider_id {
            return Err(QuotaError::unauthorized(
                "oauth credential provider does not match request scope",
            ));
        }
        if let Some(expected) = &request.scope.credential_id {
            if expected != stored.id.as_str() {
                return Err(QuotaError::unauthorized(
                    "oauth credential id does not match request scope",
                ));
            }
        }
        Ok(())
    }

    async fn do_refresh(
        &self,
        request: &QuotaRequest,
        cancel: &CancellationToken,
    ) -> Result<(), QuotaError> {
        let mut stored = self.lock_stored(cancel).await?;
        Self::check_scope(request, &stored)?;
        let backend = self.backend.clone();
        let config = self.config.clone();
        let http = self.http.clone();
        let refresh =
            refresh_oauth_credential_if_needed(&mut stored, backend.as_ref(), &config, &http);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(QuotaError::Cancelled),
            outcome = refresh => outcome.map_err(map_auth_error).map(|_| ()),
        }
    }

    async fn resolve_locked(
        &self,
        request: &QuotaRequest,
        cancel: &CancellationToken,
    ) -> Result<ResolvedCredential, QuotaError> {
        let stored = self.lock_stored(cancel).await?;
        Self::check_scope(request, &stored)?;
        resolve_oauth_credential(&stored, self.backend.as_ref()).map_err(map_auth_error)
    }

    async fn refresh_rejected(
        &self,
        request: &QuotaRequest,
        rejected: &ResolvedCredential,
        cancel: &CancellationToken,
    ) -> Result<ResolvedCredential, QuotaError> {
        ensure_bearer(rejected)?;
        let mut stored = self.lock_stored(cancel).await?;
        Self::check_scope(request, &stored)?;

        // 明文仅在此临界区的局部 ResolvedCredential 中比较，不记录、不持久化。
        // 若先到的并发请求已经轮换 token，当前 waiter 直接复用新 bearer，避免
        // 再次把 expires_at 置零并消费同一个 refresh-token generation。
        let current =
            resolve_oauth_credential(&stored, self.backend.as_ref()).map_err(map_auth_error)?;
        if current.expose_secret() != rejected.expose_secret() {
            return Ok(current);
        }
        drop(current);

        stored.expires_at = Some(Timestamp::from_unix_millis(0));
        let backend = self.backend.clone();
        let config = self.config.clone();
        let http = self.http.clone();
        let refresh =
            refresh_oauth_credential_if_needed(&mut stored, backend.as_ref(), &config, &http);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(QuotaError::Cancelled),
            outcome = refresh => {
                outcome.map_err(map_auth_error)?;
                resolve_oauth_credential(&stored, backend.as_ref()).map_err(map_auth_error)
            },
        }
    }
}

#[async_trait]
impl OAuthCredentialSource for AuthServiceOAuthSource {
    async fn resolve(
        &self,
        request: &QuotaRequest,
        cancel: &CancellationToken,
    ) -> Result<ResolvedCredential, QuotaError> {
        self.do_refresh(request, cancel).await?;
        self.resolve_locked(request, cancel).await
    }

    async fn refresh_after_unauthorized(
        &self,
        request: &QuotaRequest,
        rejected: &ResolvedCredential,
        cancel: &CancellationToken,
    ) -> Result<ResolvedCredential, QuotaError> {
        self.refresh_rejected(request, rejected, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountId, QuotaMeasure, QuotaUnit, QuotaWindow};
    use agent_domain::{ProviderId, TenantId};
    use auth_service::{CredentialId, MaskedCredential, MemoryBackend};
    use provider_runtime::http::{HttpClient, HttpClientConfig};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 计数型凭证源：可配置轮换 token、刷新失败与「刷新完成」信号，用于断言
    /// 「恰好刷新一次」「重试使用轮换 token」「403/429 不刷新」「退避取消」等行为。
    struct CountingSource {
        cred: ResolvedCredential,
        rotated: ResolvedCredential,
        resolve_calls: AtomicUsize,
        refresh_calls: AtomicUsize,
        fail_refresh: bool,
        refreshed_tx: StdMutex<Option<oneshot::Sender<()>>>,
    }

    impl CountingSource {
        fn new(cred: &str, rotated: &str) -> Self {
            Self {
                cred: ResolvedCredential::new(CredentialKind::OAuthBearer, cred),
                rotated: ResolvedCredential::new(CredentialKind::OAuthBearer, rotated),
                resolve_calls: AtomicUsize::new(0),
                refresh_calls: AtomicUsize::new(0),
                fail_refresh: false,
                refreshed_tx: StdMutex::new(None),
            }
        }
    }

    #[async_trait]
    impl OAuthCredentialSource for CountingSource {
        async fn resolve(
            &self,
            _request: &QuotaRequest,
            cancel: &CancellationToken,
        ) -> Result<ResolvedCredential, QuotaError> {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            if cancel.is_cancelled() {
                return Err(QuotaError::Cancelled);
            }
            Ok(self.cred.clone())
        }
        async fn refresh_after_unauthorized(
            &self,
            _request: &QuotaRequest,
            _rejected: &ResolvedCredential,
            cancel: &CancellationToken,
        ) -> Result<ResolvedCredential, QuotaError> {
            if cancel.is_cancelled() {
                return Err(QuotaError::Cancelled);
            }
            self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_refresh {
                Err(QuotaError::reauthorization_required("invalid_grant"))
            } else {
                if let Some(tx) = self.refreshed_tx.lock().expect("tx lock").take() {
                    let _ = tx.send(());
                }
                Ok(self.rotated.clone())
            }
        }
    }

    struct DynEndpoint(pub String);
    impl OAuthQuotaEndpoint for DynEndpoint {
        fn supports(&self, _r: &QuotaRequest) -> bool {
            true
        }
        fn endpoint(&self, _r: &QuotaRequest) -> String {
            self.0.clone()
        }
        fn source(&self) -> &'static str {
            "echo.oauth"
        }
        fn parse(
            &self,
            _r: &QuotaRequest,
            body: serde_json::Value,
        ) -> Result<(QuotaValues, QuotaReset), QuotaError> {
            let used = body
                .get("used")
                .and_then(|v| v.as_u64())
                .map(QuotaMeasure::exact)
                .unwrap_or(QuotaMeasure::Unknown);
            Ok((
                QuotaValues::new(used, QuotaMeasure::Unknown, QuotaMeasure::Unknown),
                QuotaReset::Unknown,
            ))
        }
    }

    fn request() -> QuotaRequest {
        QuotaRequest {
            scope: crate::QuotaScope::new(
                TenantId::new("t"),
                AccountId::new("a"),
                ProviderId::new("echo"),
                None,
            ),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Count,
        }
    }

    fn http() -> Arc<HttpClient> {
        Arc::new(
            HttpClient::new(HttpClientConfig::builder().disable_system_proxy().build())
                .expect("client"),
        )
    }

    fn stored_for(provider: &str, id: &str) -> StoredCredential {
        StoredCredential {
            masked: MaskedCredential::from_masked("x…y"),
            id: CredentialId::new(id),
            provider: ProviderId::new(provider),
            display_name: provider.to_string(),
            keychain_service: format!("pawork.{provider}.oauth"),
            keychain_account: format!("{id}.access"),
            created_at: Timestamp::from_unix_millis(0),
            // 已过期：若 scope 校验缺失，resolve 会立刻发起刷新（联网）。
            expires_at: Some(Timestamp::from_unix_millis(1)),
            scopes: Vec::new(),
        }
    }

    fn source_with(
        stored: StoredCredential,
        backend: Arc<dyn SecretBackend>,
        token_url: String,
    ) -> AuthServiceOAuthSource {
        AuthServiceOAuthSource::new(
            stored,
            backend,
            OAuthRefreshConfig {
                token_url,
                client_id: "client".into(),
                refresh_skew: Duration::ZERO,
            },
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("client"),
        )
    }

    fn live_source(server: &MockServer, credential_id: &str) -> Arc<AuthServiceOAuthSource> {
        let backend: Arc<dyn SecretBackend> = Arc::new(MemoryBackend::new());
        let mut stored = stored_for("echo", credential_id);
        stored.expires_at = None;
        backend
            .store(
                &stored.keychain_service,
                &stored.keychain_account,
                "old-access",
            )
            .expect("store access");
        backend
            .store(
                &stored.keychain_service,
                &format!("{}.refresh", stored.id.as_str()),
                "old-refresh",
            )
            .expect("store refresh");
        Arc::new(source_with(
            stored,
            backend,
            format!("{}/token", server.uri()),
        ))
    }

    #[tokio::test]
    async fn concurrent_401s_share_one_refresh_and_both_retry_with_rotated_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .and(header("authorization", "Bearer old-access"))
            .respond_with(ResponseTemplate::new(401).set_delay(Duration::from_millis(75)))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(75))
                    .set_body_json(serde_json::json!({
                        "access_token": "rotated-access",
                        "refresh_token": "rotated-refresh",
                        "expires_in": 3600
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .and(header("authorization", "Bearer rotated-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"used": 9})))
            .expect(2)
            .mount(&server)
            .await;

        let source = live_source(&server, "concurrent-refresh");
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source,
        );
        let request_a = request();
        let request_b = request();
        let cancel_a = CancellationToken::new();
        let cancel_b = CancellationToken::new();
        let (result_a, result_b) = tokio::join!(
            adapter.fetch(&request_a, None, &cancel_a),
            adapter.fetch(&request_b, None, &cancel_b),
        );

        assert_eq!(
            result_a.expect("first fetch").values.used,
            QuotaMeasure::exact(9)
        );
        assert_eq!(
            result_b.expect("second fetch").values.used,
            QuotaMeasure::exact(9)
        );
        server.verify().await;
        let token_requests = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.url.path() == "/token")
            .count();
        assert_eq!(token_requests, 1);
    }

    #[tokio::test]
    async fn cancelled_401_waiter_does_not_trigger_second_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/leader"))
            .and(header("authorization", "Bearer old-access"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/waiter"))
            .and(header("authorization", "Bearer old-access"))
            .respond_with(ResponseTemplate::new(401).set_delay(Duration::from_millis(25)))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(500))
                    .set_body_json(serde_json::json!({
                        "access_token": "rotated-access",
                        "refresh_token": "rotated-refresh",
                        "expires_in": 3600
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/leader"))
            .and(header("authorization", "Bearer rotated-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"used": 11})))
            .expect(1)
            .mount(&server)
            .await;

        let source = live_source(&server, "cancelled-waiter");
        let leader = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/leader", server.uri()))),
            source.clone(),
        );
        let waiter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/waiter", server.uri()))),
            source,
        );
        let leader_cancel = CancellationToken::new();
        let waiter_cancel = CancellationToken::new();
        let leader_task =
            tokio::spawn(async move { leader.fetch(&request(), None, &leader_cancel).await });
        let waiter_task = tokio::spawn({
            let waiter_cancel = waiter_cancel.clone();
            async move { waiter.fetch(&request(), None, &waiter_cancel).await }
        });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let requests = server.received_requests().await.unwrap_or_default();
            let refresh_started = requests
                .iter()
                .any(|request| request.url.path() == "/token");
            let waiter_sent_old = requests
                .iter()
                .any(|request| request.url.path() == "/waiter");
            if refresh_started && waiter_sent_old {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "concurrent refresh setup timed out"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // waiter 的 401 已返回且首个 refresh 仍在途，此时应取消其异步锁等待。
        tokio::time::sleep(Duration::from_millis(75)).await;
        waiter_cancel.cancel();
        let waiter_error = tokio::time::timeout(Duration::from_millis(250), waiter_task)
            .await
            .expect("waiter cancellation should be prompt")
            .expect("waiter task")
            .expect_err("waiter cancelled");
        assert!(matches!(waiter_error, QuotaError::Cancelled));

        let leader_snapshot = leader_task
            .await
            .expect("leader task")
            .expect("leader succeeds");
        assert_eq!(leader_snapshot.values.used, QuotaMeasure::exact(11));
        server.verify().await;
        let token_requests = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.url.path() == "/token")
            .count();
        assert_eq!(token_requests, 1);
    }

    #[tokio::test]
    async fn success_uses_resolved_token_no_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"used": 7})))
            .mount(&server)
            .await;
        let source = Arc::new(CountingSource::new("tok-1", "tok-2"));
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let snap = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect("ok");
        assert_eq!(snap.values.used, QuotaMeasure::exact(7));
        assert_eq!(snap.confidence, Confidence::Exact);
        assert_eq!(snap.provenance.adapter_kind, AdapterKind::OAuthApi);
        assert_eq!(source.refresh_calls.load(Ordering::SeqCst), 0);
        // 快照（Debug / 审计面）不得含 access token。
        let rendered = format!("{snap:?}");
        assert!(!rendered.contains("tok-1"));
        assert!(!rendered.contains("tok-2"));
    }

    #[tokio::test]
    async fn retries_once_after_401_with_rotated_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(401))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .and(header("authorization", "Bearer tok-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"used": 9})))
            .mount(&server)
            .await;
        let source = Arc::new(CountingSource::new("tok-1", "tok-2"));
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let snap = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect("ok after retry");
        assert_eq!(snap.values.used, QuotaMeasure::exact(9));
        assert_eq!(source.resolve_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.refresh_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn refresh_failure_maps_to_reauthorization() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let source = Arc::new(CountingSource {
            fail_refresh: true,
            ..CountingSource::new("tok-1", "tok-2")
        });
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let err = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect_err("reauth");
        assert!(matches!(err, QuotaError::ReauthorizationRequired { .. }));
        let rendered = format!("{err:?}");
        assert!(!rendered.contains("tok-1"));
    }

    #[tokio::test]
    async fn repeated_401_after_retry_returns_unauthorized_with_single_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let source = Arc::new(CountingSource::new("tok-1", "tok-2"));
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let err = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect_err("still 401");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));
        // retry 一次上限：第二次 401 不再触发刷新。
        assert_eq!(source.refresh_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn forbidden_does_not_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let source = Arc::new(CountingSource::new("tok-1", "tok-2"));
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let err = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect_err("403");
        assert!(matches!(err, QuotaError::Forbidden { .. }));
        assert_eq!(source.refresh_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rate_limited_does_not_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
            .mount(&server)
            .await;
        let source = Arc::new(CountingSource::new("tok-1", "tok-2"));
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let err = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect_err("429");
        assert!(matches!(
            err,
            QuotaError::RateLimited {
                retry_after_ms: Some(_),
                ..
            }
        ));
        assert_eq!(source.refresh_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancellation_before_request_propagates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let source = Arc::new(CountingSource::new("tok-1", "tok-2"));
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source,
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = adapter
            .fetch(&request(), None, &cancel)
            .await
            .expect_err("cancel");
        assert!(matches!(err, QuotaError::Cancelled));
        assert!(server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    #[tokio::test]
    async fn cancellation_during_backoff_aborts_retry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let (tx, rx) = oneshot::channel();
        let source = Arc::new(CountingSource {
            refreshed_tx: StdMutex::new(Some(tx)),
            ..CountingSource::new("tok-1", "tok-2")
        });
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            async move { adapter.fetch(&request(), None, &cancel).await }
        });
        // 等 refresh 完成、适配器进入退避窗口后再取消。
        rx.await.expect("refresh completed");
        cancel.cancel();
        let err = task
            .await
            .expect("task")
            .expect_err("cancelled during backoff");
        assert!(matches!(err, QuotaError::Cancelled));
        assert_eq!(source.refresh_calls.load(Ordering::SeqCst), 1);
        // 重试请求不得发出：服务器只应看到最初的 401。
        let reqs = server.received_requests().await.unwrap_or_default();
        assert_eq!(reqs.len(), 1);
    }

    #[tokio::test]
    async fn wrong_kind_fails_before_network() {
        let server = MockServer::start().await;
        let source = Arc::new(CountingSource {
            cred: ResolvedCredential::new(CredentialKind::ApiKey, "ak-1"),
            ..CountingSource::new("tok-1", "tok-2")
        });
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let err = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect_err("kind mismatch");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));
        assert_eq!(source.resolve_calls.load(Ordering::SeqCst), 1);
        assert_eq!(source.refresh_calls.load(Ordering::SeqCst), 0);
        // 联网前失败：服务器未收到任何请求。
        assert!(server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    #[tokio::test]
    async fn rotated_kind_mismatch_aborts_retry() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/q"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let source = Arc::new(CountingSource {
            rotated: ResolvedCredential::new(CredentialKind::ApiKey, "ak-2"),
            ..CountingSource::new("tok-1", "tok-2")
        });
        let adapter = OAuthQuotaAdapter::new(
            http(),
            Box::new(DynEndpoint(format!("{}/q", server.uri()))),
            source.clone(),
        );
        let err = adapter
            .fetch(&request(), None, &CancellationToken::new())
            .await
            .expect_err("rotated kind mismatch");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));
        assert_eq!(source.refresh_calls.load(Ordering::SeqCst), 1);
        // 仅最初的 401，重试请求不得发出。
        let reqs = server.received_requests().await.unwrap_or_default();
        assert_eq!(reqs.len(), 1);
    }

    #[tokio::test]
    async fn scope_provider_mismatch_fails_before_network() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "rotated",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;
        let backend: Arc<dyn SecretBackend> = Arc::new(MemoryBackend::new());
        let source = source_with(
            stored_for("other", "c-other"),
            backend,
            format!("{}/token", server.uri()),
        );
        let err = source
            .resolve(&request(), &CancellationToken::new())
            .await
            .expect_err("provider mismatch");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));
        // 凭证已过期且 token endpoint 可达，但 scope 校验必须先于联网失败。
        assert!(server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    #[tokio::test]
    async fn scope_credential_id_mismatch_fails_before_network() {
        let server = MockServer::start().await;
        let backend: Arc<dyn SecretBackend> = Arc::new(MemoryBackend::new());
        let source = source_with(
            stored_for("echo", "c-other"),
            backend,
            format!("{}/token", server.uri()),
        );
        let mut req = request();
        req.scope.credential_id = Some("c-expected".into());
        let err = source
            .resolve(&req, &CancellationToken::new())
            .await
            .expect_err("credential id mismatch");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));
        assert!(server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty());
    }

    #[tokio::test]
    async fn refresh_aborts_promptly_on_cancel_and_does_not_persist_rotated_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(30))
                    .set_body_json(serde_json::json!({
                        "access_token": "rotated-access",
                        "refresh_token": "rotated-refresh",
                        "expires_in": 3600
                    })),
            )
            .mount(&server)
            .await;
        let backend: Arc<dyn SecretBackend> = Arc::new(MemoryBackend::new());
        let service = "pawork.echo.oauth".to_string();
        let access_account = "c1.access".to_string();
        let refresh_account = "c1.refresh".to_string();
        backend
            .store(&service, &access_account, "old-access")
            .expect("store access");
        backend
            .store(&service, &refresh_account, "old-refresh")
            .expect("store refresh");
        let source = Arc::new(source_with(
            stored_for("echo", "c1"),
            backend.clone(),
            format!("{}/token", server.uri()),
        ));
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let source = source.clone();
            let cancel = cancel.clone();
            async move { source.resolve(&request(), &cancel).await }
        });
        // 等 token endpoint 请求在途后再取消，确保取消落在 refresh 路径。
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let arrived = server
                .received_requests()
                .await
                .unwrap_or_default()
                .iter()
                .any(|r| r.url.path() == "/token");
            if arrived {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "token request never arrived"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        cancel.cancel();
        let err = task
            .await
            .expect("task")
            .expect_err("cancelled during refresh");
        assert!(matches!(err, QuotaError::Cancelled));
        // 取消优先于刷新落盘：轮换 token 不得写回 Secret backend。
        assert_eq!(
            backend.get(&service, &access_account).expect("get access"),
            "old-access"
        );
        assert_eq!(
            backend
                .get(&service, &refresh_account)
                .expect("get refresh"),
            "old-refresh"
        );
    }

    #[test]
    fn map_auth_error_classifies_reauth_and_network() {
        let reauth = AuthError::TokenEndpoint {
            error: "invalid_grant".into(),
            description: None,
        };
        assert!(matches!(
            map_auth_error(reauth),
            QuotaError::ReauthorizationRequired { .. }
        ));
        let not_found = AuthError::NotFound;
        assert!(matches!(
            map_auth_error(not_found),
            QuotaError::ReauthorizationRequired { .. }
        ));
        for code in [
            "invalid_request",
            "revoked",
            "expired_token",
            "invalid_client",
        ] {
            let err = AuthError::TokenEndpoint {
                error: code.into(),
                description: None,
            };
            assert!(
                matches!(
                    map_auth_error(err),
                    QuotaError::ReauthorizationRequired { .. }
                ),
                "unexpected mapping for {code}"
            );
        }
    }
}
