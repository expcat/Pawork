//! MCP transport configuration and connection establishment.
//!
//! The SDK handshake lives in [`crate::mcp::codec`]. Callers build a [`TransportConfig`]
//! and hand it to a [`McpConnector`] (the default implementation is
//! [`DefaultConnector`]).
//!
//! Secret hygiene: every config type that carries plaintext env vars, bearer tokens or
//! header values implements [`std::fmt::Debug`] manually so secrets never appear in
//! logs, snapshots or panic messages.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::mcp::codec::{self, RunningClient};
use crate::mcp::sandbox::StdioSpawner;
use crate::mcp::McpError;

/// Placeholder written in place of every secret-bearing field when formatting a config
/// for logs. Chosen to be visually unambiguous and free of any caller-supplied bytes.
pub const REDACTED: &str = "[REDACTED]";

/// Establishes an MCP transport and completes the initialize handshake.
#[async_trait]
pub(crate) trait McpConnector: Send + Sync {
    /// Stable transport name used in logs and health snapshots, e.g. `"stdio"` or
    /// `"streamable-http"`.
    fn transport_name(&self) -> &'static str;

    /// Refresh connector-owned authentication state before a request. Returning
    /// `true` asks the manager to replace the live transport before proceeding.
    async fn should_reconnect_before_request(&self) -> Result<bool, McpError> {
        Ok(false)
    }

    /// Build the transport and run the initialize / initialized handshake.
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
    codec::serve_stdio(spawned.read, spawned.write).await
}

async fn connect_http(cfg: &HttpTransportConfig) -> Result<RunningClient, McpError> {
    codec::validate_http_transport_config(cfg)?;
    tracing::info!(
        target: "pawork::mcp::transport",
        transport = "streamable-http",
        has_auth = cfg.auth_token.is_some()
            || cfg.headers.keys().any(|name| name.eq_ignore_ascii_case("authorization")),
        header_count = cfg.headers.len(),
        "connecting mcp http server"
    );
    codec::serve_http(cfg).await
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
        assert!(rendered.contains("API_KEY"));
        assert!(rendered.contains("PATH"));
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
            codec::validate_http_transport_config(&HttpTransportConfig::new("")),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            codec::validate_http_transport_config(&HttpTransportConfig::new("not a url")),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            codec::validate_http_transport_config(
                &HttpTransportConfig::new("https://h/mcp").with_auth_token("")
            ),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            codec::validate_http_transport_config(
                &HttpTransportConfig::new("http://remote.example/mcp").with_auth_token("secret")
            ),
            Err(McpError::Config(_))
        ));
    }

    #[test]
    fn connector_names_match_variant() {
        let http = DefaultConnector::http(HttpTransportConfig::new("https://host/mcp"));
        assert_eq!(http.transport_name(), "streamable-http");
    }
}
