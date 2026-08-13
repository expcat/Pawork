//! MCP transport configuration and connection establishment.
//!
//! `rmcp` is kept behind this module. Callers build a [`TransportConfig`] and hand it
//! to a [`McpConnector`] (the default implementation is [`DefaultConnector`]). The
//! connector spawns the stdio subprocess or Streamable HTTP transport, drives the rmcp
//! `initialize` / `initialized` handshake, and returns a [`RunningClient`].
//!
//! Secret hygiene: every config type that carries plaintext env vars, bearer tokens or
//! header values implements [`std::fmt::Debug`] manually so secrets never appear in
//! logs, snapshots or panic messages. See [`StdioTransportConfig`] and
//! [`HttpTransportConfig`].

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::service::RunningService;
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};

use crate::config::is_loopback_url;
use crate::sandbox::StdioSpawner;
use crate::McpError;

/// Placeholder written in place of every secret-bearing field when formatting a config
/// for logs. Chosen to be visually unambiguous and free of any caller-supplied bytes.
pub const REDACTED: &str = "[REDACTED]";

/// A running MCP client returned by a connector.
///
/// The service type is the rmcp "do-nothing" client `()`; the rmcp model stays behind
/// this type alias so Agent Core never names an SDK type directly.
pub type RunningClient = RunningService<RoleClient, ()>;

/// Establishes an MCP transport and completes the rmcp handshake.
///
/// This trait exists so the manager (and its tests) can depend on a small, injectable
/// boundary instead of real subprocesses or sockets. [`DefaultConnector`] is the
/// production implementation; tests supply an in-process implementation.
#[async_trait]
pub trait McpConnector: Send + Sync {
    /// Stable transport name used in logs and health snapshots, e.g. `"stdio"` or
    /// `"streamable-http"`.
    fn transport_name(&self) -> &'static str;

    /// Refresh connector-owned authentication state before a request. Returning
    /// `true` asks the manager to replace the live transport before proceeding.
    async fn should_reconnect_before_request(&self) -> Result<bool, McpError> {
        Ok(false)
    }

    /// Build the transport and run the rmcp `initialize` / `initialized` handshake.
    ///
    /// On success the returned [`RunningClient`] is live and ready for requests. On
    /// failure the connector must not leave dangling processes or sessions.
    async fn connect(&self) -> Result<RunningClient, McpError>;
}

/// Stdio transport configuration.
///
/// `env` is injected into the child process environment (overlaying the parent env) and
/// frequently carries secrets such as API keys, so it is redacted in [`Debug`](Self::fmt).
#[derive(Clone)]
pub struct StdioTransportConfig {
    /// Executable to launch.
    pub command: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Extra environment variables for the child (may include secrets).
    pub env: HashMap<String, String>,
    /// Working directory for the child, defaults to the parent's when `None`.
    pub working_dir: Option<PathBuf>,
}

impl StdioTransportConfig {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            working_dir: None,
        }
    }

    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    pub fn with_working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }
}

impl fmt::Debug for StdioTransportConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StdioTransportConfig")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &RedactedMap(&self.env))
            .field("working_dir", &self.working_dir)
            .finish()
    }
}

/// Streamable HTTP transport configuration.
///
/// `auth_token` is sent as `Authorization: Bearer <token>` and `headers` may carry
/// additional secrets (e.g. `X-API-Key`). Both are redacted in [`Debug`](Self::fmt).
#[derive(Clone)]
pub struct HttpTransportConfig {
    /// Endpoint URL, e.g. `https://host/mcp`.
    pub url: String,
    /// Optional bearer token forwarded as `Authorization: Bearer <token>`.
    pub auth_token: Option<String>,
    /// Extra request headers, applied to every HTTP request.
    pub headers: HashMap<String, String>,
}

impl HttpTransportConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_token: None,
            headers: HashMap::new(),
        }
    }

    pub fn with_auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }
}

impl fmt::Debug for HttpTransportConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpTransportConfig")
            .field("url", &safe_url_for_debug(&self.url))
            .field("auth_token", &RedactedOpt(self.auth_token.as_deref()))
            .field("headers", &RedactedMap(&self.headers))
            .finish()
    }
}

/// Transport selection.
#[derive(Clone)]
pub enum TransportConfig {
    Stdio(StdioTransportConfig),
    Http(HttpTransportConfig),
}

impl fmt::Debug for TransportConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportConfig::Stdio(cfg) => {
                f.debug_tuple("TransportConfig::Stdio").field(cfg).finish()
            }
            TransportConfig::Http(cfg) => {
                f.debug_tuple("TransportConfig::Http").field(cfg).finish()
            }
        }
    }
}

/// Production connector backed by a fully authorized transport.
///
/// The representation has no unsandboxed stdio state: HTTP needs no process
/// runtime, while stdio always carries the sandboxed spawner that every initial
/// connection and reconnect must reuse.
#[derive(Clone)]
pub struct DefaultConnector {
    transport: ConnectorTransport,
}

#[derive(Clone)]
enum ConnectorTransport {
    Stdio {
        config: StdioTransportConfig,
        spawner: Arc<dyn StdioSpawner>,
    },
    Http(HttpTransportConfig),
}

impl DefaultConnector {
    /// Construct an HTTP connector. HTTP does not require a process sandbox.
    pub fn http(config: HttpTransportConfig) -> Self {
        Self {
            transport: ConnectorTransport::Http(config),
        }
    }

    /// Construct a stdio connector from its mandatory sandboxed spawner.
    ///
    /// Kept crate-private so production callers must inject a concrete
    /// `SandboxBackend + SandboxPolicy + workspace roots` through
    /// [`crate::config::StdioSandboxRuntime`].
    pub(crate) fn sandboxed_stdio(
        config: StdioTransportConfig,
        spawner: Arc<dyn StdioSpawner>,
    ) -> Self {
        Self {
            transport: ConnectorTransport::Stdio { config, spawner },
        }
    }

    pub fn config(&self) -> TransportConfig {
        match &self.transport {
            ConnectorTransport::Stdio { config, .. } => TransportConfig::Stdio(config.clone()),
            ConnectorTransport::Http(config) => TransportConfig::Http(config.clone()),
        }
    }
}

#[async_trait]
impl McpConnector for DefaultConnector {
    fn transport_name(&self) -> &'static str {
        match self.transport {
            ConnectorTransport::Stdio { .. } => "stdio",
            ConnectorTransport::Http(_) => "streamable-http",
        }
    }

    async fn connect(&self) -> Result<RunningClient, McpError> {
        match &self.transport {
            ConnectorTransport::Stdio { config, spawner } => {
                connect_stdio_sandboxed(spawner, config).await
            }
            ConnectorTransport::Http(config) => connect_http(config).await,
        }
    }
}

/// 经注入的 sandboxed spawner 托管 stdio（Sandbox → Process Runtime），再用 rmcp
/// async_rw transport 完成 initialize/initialized 握手。restart 复用同一 spawner，
/// 因此 sandbox guarantee 在 reconnect 阶段不降级。
async fn connect_stdio_sandboxed(
    spawner: &Arc<dyn StdioSpawner>,
    cfg: &StdioTransportConfig,
) -> Result<RunningClient, McpError> {
    tracing::info!(
        target: "pawork::mcp::transport",
        transport = "stdio",
        command = %cfg.command,
        arg_count = cfg.args.len(),
        sandboxed = true,
        "spawning mcp server process through sandbox runtime"
    );
    let spawned = spawner.spawn(cfg).await?;
    // read 内部持有 ProcessHandle 守卫，随 transport 同生命周期 drop 终止进程树。
    let transport = AsyncRwTransport::new_client(spawned.read, spawned.write);
    ().serve(transport)
        .await
        .map_err(|_| McpError::Transport("stdio handshake failed".into()))
}

async fn connect_http(cfg: &HttpTransportConfig) -> Result<RunningClient, McpError> {
    let rmcp_config = build_http_transport_config(cfg)?;
    tracing::info!(
        target: "pawork::mcp::transport",
        transport = "streamable-http",
        has_auth = cfg.auth_token.is_some()
            || cfg.headers.keys().any(|name| name.eq_ignore_ascii_case("authorization")),
        header_count = cfg.headers.len(),
        "connecting mcp http server"
    );

    let transport = StreamableHttpClientTransport::from_config(rmcp_config);
    ().serve(transport)
        .await
        .map_err(|_| McpError::Transport("http handshake failed".into()))
}

/// Translate a Pawork [`HttpTransportConfig`] into the rmcp transport config.
///
/// Exposed (crate-private) so tests can assert redaction-bearing inputs are wired
/// correctly without performing real network IO. Scheme / userinfo / fragment
/// validation already happened at config parse time ([`crate::config`]), so this
/// function only keeps the runtime guards that depend on the resolved config
/// (empty/conflicting auth, loopback+HTTPS for secrets, header validity).
pub(crate) fn build_http_transport_config(
    cfg: &HttpTransportConfig,
) -> Result<StreamableHttpClientTransportConfig, McpError> {
    use reqwest::header::{HeaderName, HeaderValue};

    let url = cfg.url.trim();
    if url.is_empty() {
        return Err(McpError::Config("http url must not be empty".into()));
    }
    let parsed = url::Url::parse(url)
        .map_err(|error| McpError::Config(format!("invalid http url: {error}")))?;
    if cfg.auth_token.as_deref().is_some_and(str::is_empty) {
        return Err(McpError::Config("auth_token must not be empty".into()));
    }
    let has_authorization_header = cfg
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"));
    if cfg.auth_token.is_some() && has_authorization_header {
        return Err(McpError::Config(
            "auth_token conflicts with a custom Authorization header".into(),
        ));
    }
    if parsed.scheme() == "http"
        && (cfg.auth_token.is_some() || !cfg.headers.is_empty())
        && !is_loopback_url(&parsed)
    {
        return Err(McpError::Config(
            "secret-bearing HTTP authentication requires HTTPS for non-loopback endpoints".into(),
        ));
    }

    let mut rmcp_config = StreamableHttpClientTransportConfig::with_uri(url.to_owned());
    if let Some(token) = &cfg.auth_token {
        rmcp_config = rmcp_config.auth_header(token.clone());
    }
    if !cfg.headers.is_empty() {
        let mut headers = HashMap::with_capacity(cfg.headers.len());
        for (name, value) in &cfg.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| McpError::Config(format!("invalid header name {name:?}: {e}")))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|e| McpError::Config(format!("invalid header value for {name:?}: {e}")))?;
            if headers.insert(header_name, header_value).is_some() {
                return Err(McpError::Config(format!("duplicate header {name:?}")));
            }
        }
        rmcp_config = rmcp_config.custom_headers(headers);
    }
    Ok(rmcp_config)
}

fn safe_url_for_debug(value: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(value) else {
        return "[INVALID URL]".into();
    };
    let had_query = parsed.query().is_some();
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    let mut safe = parsed.to_string();
    if had_query {
        safe.push_str("?[REDACTED]");
    }
    safe
}

/// Debug helper that renders a string map with all values replaced by [`REDACTED`].
struct RedactedMap<'a>(&'a HashMap<String, String>);

impl<'a> fmt::Debug for RedactedMap<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut map = f.debug_map();
        for key in self.0.keys() {
            map.entry(&key, &REDACTED);
        }
        map.finish()
    }
}

/// Debug helper that renders an optional secret as `Some("[REDACTED]")` / `None`.
struct RedactedOpt<'a>(Option<&'a str>);

impl<'a> fmt::Debug for RedactedOpt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => write!(f, "None"),
            Some(_) => write!(f, "Some({REDACTED:?})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with_secret() -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        env.insert("API_KEY".to_string(), "sk-super-secret-value".to_string());
        env
    }

    #[test]
    fn stdio_debug_redacts_env_values() {
        let cfg = StdioTransportConfig::new("npx")
            .with_args(["-y", "@modelcontextprotocol/server-everything"])
            .with_env(env_with_secret())
            .with_working_dir("/tmp");

        let rendered = format!("{cfg:?}");
        assert!(rendered.contains("StdioTransportConfig"));
        assert!(rendered.contains(r#"command: "npx""#));
        // keys are visible ...
        assert!(rendered.contains("API_KEY"));
        assert!(rendered.contains("PATH"));
        // ... but no value leaks.
        assert!(!rendered.contains("sk-super-secret-value"));
        assert!(!rendered.contains("/usr/bin"));
        assert!(rendered.contains(REDACTED));
    }

    #[test]
    fn http_debug_redacts_token_and_headers() {
        let mut headers = HashMap::new();
        headers.insert("X-API-Key".to_string(), "header-secret-123".to_string());
        let cfg = HttpTransportConfig::new("https://example.test/mcp")
            .with_auth_token("bearer-token-secret")
            .with_headers(headers);

        let rendered = format!("{cfg:?}");
        assert!(rendered.contains("HttpTransportConfig"));
        assert!(rendered.contains("https://example.test/mcp"));
        assert!(rendered.contains("X-API-Key"));
        assert!(rendered.contains(REDACTED));
        assert!(!rendered.contains("bearer-token-secret"));
        assert!(!rendered.contains("header-secret-123"));
    }

    #[test]
    fn http_debug_redacts_url_credentials_query_and_fragment() {
        let cfg = HttpTransportConfig::new(
            "https://user:password@example.test/mcp?token=query-secret#fragment-secret",
        );
        let rendered = format!("{cfg:?}");
        assert!(rendered.contains("example.test/mcp"));
        assert!(!rendered.contains("user"));
        assert!(!rendered.contains("password"));
        assert!(!rendered.contains("query-secret"));
        assert!(!rendered.contains("fragment-secret"));
        assert!(rendered.contains(REDACTED));
    }

    #[test]
    fn transport_config_enum_debug_is_redacted() {
        let stdio =
            TransportConfig::Stdio(StdioTransportConfig::new("cmd").with_env(env_with_secret()));
        let http = TransportConfig::Http(
            HttpTransportConfig::new("https://h/mcp").with_auth_token("secret"),
        );

        assert!(!format!("{stdio:?}").contains("sk-super-secret-value"));
        assert!(!format!("{http:?}").contains("secret"));
    }

    #[test]
    fn http_config_rejects_invalid_inputs() {
        assert!(matches!(
            build_http_transport_config(&HttpTransportConfig::new("")),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            build_http_transport_config(&HttpTransportConfig::new("not a url")),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            build_http_transport_config(
                &HttpTransportConfig::new("https://h/mcp").with_auth_token("")
            ),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            build_http_transport_config(
                &HttpTransportConfig::new("http://remote.example/mcp").with_auth_token("secret")
            ),
            Err(McpError::Config(_))
        ));
        let mut headers = HashMap::new();
        headers.insert("authorization".into(), "Bearer duplicate".into());
        assert!(matches!(
            build_http_transport_config(
                &HttpTransportConfig::new("https://remote.example/mcp")
                    .with_auth_token("secret")
                    .with_headers(headers)
            ),
            Err(McpError::Config(_))
        ));
    }

    #[test]
    fn http_config_applies_auth_and_headers() {
        use reqwest::header::HeaderName;

        let mut headers = HashMap::new();
        headers.insert("X-Trace-Id".to_string(), "abc".to_string());
        let cfg = HttpTransportConfig::new("https://host/mcp")
            .with_auth_token("tok")
            .with_headers(headers);

        let rmcp_config = build_http_transport_config(&cfg).expect("valid config");
        assert_eq!(rmcp_config.auth_header.as_deref(), Some("tok"));
        assert_eq!(
            rmcp_config
                .custom_headers
                .get(&HeaderName::from_static("x-trace-id"))
                .map(|v| v.to_str().unwrap()),
            Some("abc")
        );
        // the configured endpoint is preserved verbatim
        assert_eq!(rmcp_config.uri.as_ref(), "https://host/mcp");
    }

    #[test]
    fn connector_names_match_variant() {
        let http = DefaultConnector::http(HttpTransportConfig::new("https://host/mcp"));
        assert_eq!(http.transport_name(), "streamable-http");
    }
}
