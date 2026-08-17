//! MCP OAuth: PKCE login flow + an auto-refreshing bearer provider.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use pawork_auth::oauth::{
    exchange_pkce_code, resolve_oauth_credential_for_request, start_pkce_flow, store_oauth_token,
    OAuthRefreshConfig, PkceFlowConfig, PkceSession,
};
use pawork_auth::{SecretBackend, StoredCredential};
use tokio::sync::Mutex;

use crate::codec::RunningClient;
use crate::transport::{DefaultConnector, HttpTransportConfig, McpConnector};
use crate::McpError;

/// Begin a PKCE Authorization Code login for an MCP http server.
pub fn begin_pkce_login(config: PkceFlowConfig) -> Result<PkceSession, McpError> {
    start_pkce_flow(config).map_err(McpError::from_auth)
}

/// Complete a PKCE login: verify `state`, exchange `code`, persist tokens.
pub async fn complete_pkce_login(
    session: &PkceSession,
    code: &str,
    returned_state: &str,
    http: &reqwest::Client,
    backend: &dyn SecretBackend,
    display_name: &str,
) -> Result<StoredCredential, McpError> {
    let tokens = exchange_pkce_code(session, code, returned_state, http)
        .await
        .map_err(McpError::from_auth)?;
    store_oauth_token(
        backend,
        session.config.provider.clone(),
        display_name,
        &tokens,
        session.config.scopes.clone(),
    )
    .map_err(McpError::from_auth)
}

/// Auto-refreshing OAuth bearer provider for an MCP http transport.
pub struct McpBearerProvider {
    stored: Mutex<StoredCredential>,
    backend: Arc<dyn SecretBackend>,
    refresh: OAuthRefreshConfig,
    http: reqwest::Client,
}

impl McpBearerProvider {
    pub fn new(
        stored: StoredCredential,
        backend: Arc<dyn SecretBackend>,
        refresh: OAuthRefreshConfig,
        http: reqwest::Client,
    ) -> Self {
        Self {
            stored: Mutex::new(stored),
            backend,
            refresh,
            http,
        }
    }

    pub async fn bearer(&self) -> Result<String, McpError> {
        let mut stored = self.stored.lock().await;
        let resolved = resolve_oauth_credential_for_request(
            &mut stored,
            self.backend.as_ref(),
            &self.refresh,
            &self.http,
        )
        .await
        .map_err(McpError::from_auth)?;
        Ok(format!("Bearer {}", resolved.expose_secret()))
    }

    pub async fn credential(&self) -> StoredCredential {
        self.stored.lock().await.clone()
    }
}

/// Streamable HTTP connector that injects an auto-refreshing OAuth bearer.
pub struct OAuthHttpConnector {
    base: HttpTransportConfig,
    provider: Arc<McpBearerProvider>,
    last_bearer: Mutex<Option<String>>,
}

impl OAuthHttpConnector {
    pub fn new(base: HttpTransportConfig, provider: Arc<McpBearerProvider>) -> Self {
        Self {
            base,
            provider,
            last_bearer: Mutex::new(None),
        }
    }

    async fn authorized_config(&self) -> Result<HttpTransportConfig, McpError> {
        if self.base.auth_token.is_some()
            || self
                .base
                .headers
                .keys()
                .any(|name| name.eq_ignore_ascii_case("authorization"))
        {
            return Err(McpError::Config(
                "OAuth connector requires an HTTP config without existing Authorization".into(),
            ));
        }

        let bearer = self.provider.bearer().await?;
        *self.last_bearer.lock().await = Some(bearer.clone());
        let mut config = self.base.clone();
        config.headers.insert("Authorization".into(), bearer);
        Ok(config)
    }
}

impl fmt::Debug for OAuthHttpConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthHttpConnector")
            .field("base", &self.base)
            .field("provider", &"[configured]")
            .field("last_bearer", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
impl McpConnector for OAuthHttpConnector {
    fn transport_name(&self) -> &'static str {
        "streamable-http+oauth"
    }

    async fn should_reconnect_before_request(&self) -> Result<bool, McpError> {
        let bearer = self.provider.bearer().await?;
        let mut previous = self.last_bearer.lock().await;
        let changed = previous.as_ref().is_some_and(|current| current != &bearer);
        *previous = Some(bearer);
        Ok(changed)
    }

    async fn connect(&self) -> Result<RunningClient, McpError> {
        let config = self.authorized_config().await?;
        DefaultConnector::http(config).connect().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_auth::oauth::{read_refresh_token, store_oauth_token, TokenSet};
    use pawork_auth::MemoryBackend;
    use pawork_domain::ProviderId;
    use std::time::Duration;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn refresh_config(server_uri: String) -> OAuthRefreshConfig {
        OAuthRefreshConfig {
            token_url: format!("{server_uri}/token"),
            client_id: "mcp-client".into(),
            refresh_skew: Duration::from_secs(30),
        }
    }

    fn token_set(
        access: &str,
        refresh: Option<&str>,
        expires_in: Option<u64>,
        scope: Option<&str>,
    ) -> TokenSet {
        TokenSet {
            access_token: access.into(),
            refresh_token: refresh.map(str::to_string),
            id_token: None,
            expires_in,
            token_type: "Bearer".into(),
            scope: scope.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn bearer_returns_existing_token_without_refresh_when_not_expired() {
        let backend = Arc::new(MemoryBackend::new());
        let stored = store_oauth_token(
            backend.as_ref(),
            ProviderId::new("mcp-fresh"),
            "fresh",
            &token_set("fresh-access", Some("fresh-refresh"), Some(3600), None),
            Vec::new(),
        )
        .expect("store");

        let provider = McpBearerProvider::new(
            stored,
            backend.clone(),
            refresh_config("http://must-not-be-called.invalid".into()),
            reqwest::Client::new(),
        );

        let bearer = provider.bearer().await.expect("bearer");
        assert_eq!(bearer, "Bearer fresh-access");
        let cred = provider.credential().await;
        assert_eq!(
            backend
                .get(&cred.keychain_service, &cred.keychain_account)
                .unwrap(),
            "fresh-access"
        );
    }

    #[tokio::test]
    async fn bearer_auto_refreshes_and_writes_back_rotated_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=expired-refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "rotated-access",
                "refresh_token": "rotated-refresh",
                "token_type": "Bearer",
                "expires_in": 3600,
                "scope": "read write"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let backend = Arc::new(MemoryBackend::new());
        let stored = store_oauth_token(
            backend.as_ref(),
            ProviderId::new("mcp-refresh"),
            "refresh",
            &token_set("expired-access", Some("expired-refresh"), Some(0), Some("read")),
            vec!["read".into()],
        )
        .expect("store");

        let provider = McpBearerProvider::new(
            stored,
            backend.clone(),
            refresh_config(server.uri()),
            reqwest::Client::new(),
        );

        let bearer = provider.bearer().await.expect("bearer with refresh");
        assert_eq!(bearer, "Bearer rotated-access");

        let cred = provider.credential().await;
        assert_eq!(
            backend
                .get(&cred.keychain_service, &cred.keychain_account)
                .unwrap(),
            "rotated-access"
        );
        assert_eq!(
            read_refresh_token(&cred, backend.as_ref()).unwrap(),
            "rotated-refresh"
        );
        assert!(!serde_json::to_string(&cred)
            .unwrap()
            .contains("rotated-access"));
        assert!(cred.expires_at.is_some());
        server.verify().await;
    }

    #[tokio::test]
    async fn pkce_login_stores_tokens_without_plaintext_in_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "AT-mcp-pkce-secret",
                "refresh_token": "RT-mcp-pkce-secret",
                "token_type": "Bearer",
                "expires_in": 3600
            })))
            .mount(&server)
            .await;

        let config = PkceFlowConfig {
            client_id: "mcp-client".into(),
            auth_url: "https://example.com/auth".into(),
            token_url: format!("{}/token", server.uri()),
            redirect_uri: "http://127.0.0.1:0/cb".into(),
            scopes: vec!["read".into()],
            provider: ProviderId::new("mcp-pkce"),
            extra_auth_params: Vec::new(),
        };
        let session = begin_pkce_login(config).expect("begin pkce");
        let backend = MemoryBackend::new();
        let stored = complete_pkce_login(
            &session,
            "the-code",
            &session.state,
            &reqwest::Client::new(),
            &backend,
            "MCP PKCE",
        )
        .await
        .expect("complete pkce");

        let serialized = serde_json::to_string(&stored).unwrap();
        assert!(!serialized.contains("AT-mcp-pkce-secret"));
        assert_eq!(stored.scopes, vec!["read".to_string()]);
    }

    #[tokio::test]
    async fn oauth_http_connector_injects_and_rotates_bearer_without_debug_leakage() {
        let backend = Arc::new(MemoryBackend::new());
        let stored = store_oauth_token(
            backend.as_ref(),
            ProviderId::new("mcp-connector"),
            "connector",
            &token_set(
                "connector-access-secret",
                Some("connector-refresh-secret"),
                Some(3600),
                None,
            ),
            Vec::new(),
        )
        .expect("store");
        let service = stored.keychain_service.clone();
        let account = stored.keychain_account.clone();
        let provider = Arc::new(McpBearerProvider::new(
            stored,
            backend.clone(),
            refresh_config("http://must-not-be-called.invalid".into()),
            reqwest::Client::new(),
        ));
        let connector = OAuthHttpConnector::new(
            HttpTransportConfig::new("http://127.0.0.1:9000/mcp"),
            provider,
        );

        let config = connector
            .authorized_config()
            .await
            .expect("authorized config");
        assert_eq!(
            config.headers.get("Authorization").map(String::as_str),
            Some("Bearer connector-access-secret")
        );
        let rendered = format!("{connector:?}");
        assert!(!rendered.contains("connector-access-secret"));
        assert!(!rendered.contains("connector-refresh-secret"));

        backend
            .store(&service, &account, "rotated-connector-access")
            .expect("rotate access token");
        assert!(connector
            .should_reconnect_before_request()
            .await
            .expect("detect rotation"));
        assert!(!connector
            .should_reconnect_before_request()
            .await
            .expect("stable bearer"));
    }
}
