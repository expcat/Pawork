//! 可复用的「API Key 官方额度接口」适配器。
//!
//! 把跨 Provider 共有的 mechanics（取凭证、拼认证头、发起带取消竞争的 GET、
//! 把 HTTP 错误归一为 [`QuotaError`]、组装带 provenance 的快照）收口在此；
//! 每个 Provider 只需实现 [`ApiKeyQuotaEndpoint`]：告诉适配器去哪取数、
//! 用什么认证头、如何把 JSON 解析成 `(QuotaValues, QuotaReset)`。
//!
//! 这是「Exact」置信度来源：直接来自 Provider 官方 billing/quota 接口。

use std::sync::Arc;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use provider_api::{CredentialKind, ResolvedCredential};
use provider_runtime::http::HttpClient;

use crate::{
    AdapterKind, Confidence, QuotaAdapter, QuotaError, QuotaProvenance, QuotaRequest, QuotaReset,
    QuotaSnapshot, QuotaValues,
};

use super::http_util::{api_get, now_millis, redact_endpoint};

/// Provider 侧胶水：单端点 API-key 额度接口的取数与解析规则。
pub trait ApiKeyQuotaEndpoint: Send + Sync {
    /// 该端点是否对该请求（scope/window/unit）提供读数。
    fn supports(&self, request: &QuotaRequest) -> bool;
    /// 完整请求 URL（可含 query）。
    fn endpoint(&self, request: &QuotaRequest) -> String;
    /// 由凭证构造认证头；凭证缺失应返回 [`QuotaError::Unauthorized`]。
    fn auth_headers(
        &self,
        credential: &ResolvedCredential,
    ) -> Result<Vec<(String, String)>, QuotaError>;
    /// provenance 中的来源标签（如 `openai.admin`）。
    fn source(&self) -> &'static str;
    /// 把响应 JSON 解析为 `(used/limit/remaining, reset)`。
    fn parse(
        &self,
        request: &QuotaRequest,
        body: serde_json::Value,
    ) -> Result<(QuotaValues, QuotaReset), QuotaError>;
}

/// 通用 API-key 配额适配器。持有一个 [`HttpClient`]（连接池共享）与一个
/// Provider 专属的 [`ApiKeyQuotaEndpoint`]。
pub struct ApiKeyQuotaAdapter {
    http: Arc<HttpClient>,
    endpoint: Box<dyn ApiKeyQuotaEndpoint>,
}

impl ApiKeyQuotaAdapter {
    pub fn new(http: Arc<HttpClient>, endpoint: Box<dyn ApiKeyQuotaEndpoint>) -> Self {
        Self { http, endpoint }
    }
}

#[async_trait]
impl QuotaAdapter for ApiKeyQuotaAdapter {
    fn kind(&self) -> AdapterKind {
        AdapterKind::ApiKeyApi
    }

    fn supports(&self, request: &QuotaRequest) -> bool {
        self.endpoint.supports(request)
    }

    async fn fetch(
        &self,
        request: &QuotaRequest,
        credential: Option<&ResolvedCredential>,
        cancel: &CancellationToken,
    ) -> Result<QuotaSnapshot, QuotaError> {
        let credential = credential.ok_or_else(|| QuotaError::unauthorized("api key required"))?;
        if credential.kind() != CredentialKind::ApiKey {
            return Err(QuotaError::unauthorized("API-key credential required"));
        }
        let url = self.endpoint.endpoint(request);
        let headers = self.endpoint.auth_headers(credential)?;
        let body = api_get(self.http.as_ref(), &url, &headers, cancel).await?;
        let (values, reset) = self.endpoint.parse(request, body)?;

        let now = now_millis();
        let provenance = QuotaProvenance {
            adapter_kind: AdapterKind::ApiKeyApi,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountId, QuotaMeasure, QuotaUnit, QuotaWindow};
    use agent_domain::{ProviderId, TenantId};
    use provider_api::CredentialKind;
    use provider_runtime::http::{HttpClient, HttpClientConfig};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct EchoEndpoint {
        url: String,
    }

    impl ApiKeyQuotaEndpoint for EchoEndpoint {
        fn supports(&self, _request: &QuotaRequest) -> bool {
            true
        }
        fn endpoint(&self, _request: &QuotaRequest) -> String {
            format!("{}/v1/quota?key=SECRET-abcdef0123456789", self.url)
        }
        fn auth_headers(
            &self,
            credential: &ResolvedCredential,
        ) -> Result<Vec<(String, String)>, QuotaError> {
            Ok(vec![(
                "Authorization".to_string(),
                format!("Bearer {}", credential.expose_secret()),
            )])
        }
        fn source(&self) -> &'static str {
            "echo.test"
        }
        fn parse(
            &self,
            request: &QuotaRequest,
            body: serde_json::Value,
        ) -> Result<(QuotaValues, QuotaReset), QuotaError> {
            let used = body
                .get("used")
                .and_then(|v| v.as_u64())
                .map(QuotaMeasure::exact)
                .unwrap_or(QuotaMeasure::Unknown);
            let limit = body
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(QuotaMeasure::exact)
                .unwrap_or(QuotaMeasure::Unknown);
            let _ = request;
            Ok((
                QuotaValues::new(used, limit, QuotaMeasure::Unknown),
                QuotaReset::Unknown,
            ))
        }
    }

    fn sample_request() -> QuotaRequest {
        QuotaRequest {
            scope: crate::QuotaScope::new(
                TenantId::new("tenant-a"),
                AccountId::new("account-1"),
                ProviderId::new("echo"),
                None,
            ),
            window: QuotaWindow::Monthly,
            unit: QuotaUnit::Count,
        }
    }

    async fn setup(status: u16, body: serde_json::Value) -> (MockServer, ApiKeyQuotaAdapter) {
        let server = MockServer::start().await;
        let mock = Mock::given(method("GET"))
            .and(path("/v1/quota"))
            .and(header("authorization", "Bearer admin-key"));
        let mock = if status == 200 {
            mock.respond_with(ResponseTemplate::new(200).set_body_json(body))
        } else if status == 429 {
            mock.respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
        } else {
            mock.respond_with(ResponseTemplate::new(status).set_body_json(body))
        };
        mock.mount(&server).await;

        let http = Arc::new(
            HttpClient::new(HttpClientConfig::builder().disable_system_proxy().build())
                .expect("client"),
        );
        let endpoint = Box::new(EchoEndpoint { url: server.uri() });
        (server, ApiKeyQuotaAdapter::new(http, endpoint))
    }

    #[tokio::test]
    async fn happy_path_returns_exact_snapshot_with_redacted_endpoint() {
        let (_server, adapter) = setup(200, serde_json::json!({"used": 30, "limit": 100})).await;
        let request = sample_request();
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "admin-key");
        let cancel = CancellationToken::new();
        let snapshot = adapter
            .fetch(&request, Some(&cred), &cancel)
            .await
            .expect("ok");

        assert_eq!(snapshot.confidence, Confidence::Exact);
        assert_eq!(snapshot.values.used, QuotaMeasure::exact(30));
        assert_eq!(snapshot.values.limit, QuotaMeasure::exact(100));
        let endpoint = snapshot.provenance.endpoint.as_deref().unwrap();
        // query string（含伪造的 key）必须被抹掉。
        assert!(!endpoint.contains("SECRET"));
        assert!(!endpoint.contains("key="));
        assert!(endpoint.ends_with("/v1/quota"));
    }

    #[tokio::test]
    async fn unauthorized_maps_distinctly() {
        let (_server, adapter) = setup(401, serde_json::json!({"error": "bad key"})).await;
        let request = sample_request();
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "admin-key");
        let err = adapter
            .fetch(&request, Some(&cred), &CancellationToken::new())
            .await
            .expect_err("401");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));
    }

    #[tokio::test]
    async fn rate_limited_carries_retry_after() {
        let (_server, adapter) = setup(429, serde_json::json!({})).await;
        let request = sample_request();
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "admin-key");
        let err = adapter
            .fetch(&request, Some(&cred), &CancellationToken::new())
            .await
            .expect_err("429");
        assert!(matches!(
            err,
            QuotaError::RateLimited {
                retry_after_ms: Some(_),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn forbidden_maps_distinctly() {
        let (_server, adapter) = setup(403, serde_json::json!({})).await;
        let request = sample_request();
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "admin-key");
        let err = adapter
            .fetch(&request, Some(&cred), &CancellationToken::new())
            .await
            .expect_err("403");
        assert!(matches!(err, QuotaError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn missing_credential_is_unauthorized() {
        let (_server, adapter) = setup(200, serde_json::json!({})).await;
        let request = sample_request();
        let err = adapter
            .fetch(&request, None, &CancellationToken::new())
            .await
            .expect_err("no cred");
        assert!(matches!(err, QuotaError::Unauthorized { .. }));
    }

    #[tokio::test]
    async fn non_api_key_credential_is_rejected_before_network() {
        let (server, adapter) = setup(200, serde_json::json!({})).await;
        let request = sample_request();
        let credential = ResolvedCredential::new(CredentialKind::OAuthBearer, "not-an-api-key");
        let error = adapter
            .fetch(&request, Some(&credential), &CancellationToken::new())
            .await
            .expect_err("wrong credential kind");

        assert!(matches!(error, QuotaError::Unauthorized { .. }));
        assert!(server
            .received_requests()
            .await
            .expect("requests")
            .is_empty());
    }

    #[tokio::test]
    async fn cancellation_aborts_inflight_request() {
        let server = MockServer::start().await;
        // 故意延迟响应，使取消先到。
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(5)))
            .mount(&server)
            .await;
        let http = Arc::new(
            HttpClient::new(HttpClientConfig::builder().disable_system_proxy().build())
                .expect("client"),
        );
        let endpoint = Box::new(EchoEndpoint { url: server.uri() });
        let adapter = ApiKeyQuotaAdapter::new(http, endpoint);
        let request = sample_request();
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "admin-key");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = adapter
            .fetch(&request, Some(&cred), &cancel)
            .await
            .expect_err("cancel");
        assert!(matches!(err, QuotaError::Cancelled));
    }
}
