//! MCP client configuration.
//!
//! Reads the MCP server map from `ResolvedConfig.config.extra["mcp"]`. The
//! config-service already merges global/workspace/session/run tiers
//! recursively at the JSON level, so by the time we read `extra["mcp"]` it is
//! the fully merged, layer-independent result. This module only parses and
//! validates it — it never reads arbitrary filesystem paths itself (all file IO
//! stays in config-service).
//!
//! The parsed [`TransportSpec`] is the transport-facing representation consumed
//! by `transport.rs`; the transport module turns it into a live rmcp transport.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use auth_service::SecretBackend;
use config_service::ResolvedConfig;
use policy_engine::ApprovalMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::manager::{
    ManagedMcpClient, ManagedMcpClientOptions, RestartPolicy as RuntimeRestartPolicy,
};
use crate::security::SecretRef;
use crate::transport::{
    DefaultConnector, HttpTransportConfig, McpConnector, RunningClient, StdioTransportConfig,
    TransportConfig,
};
use crate::McpError;

/// Default hard output cap for MCP tool output (1 MiB).
const DEFAULT_MAX_OUTPUT_BYTES: u64 = 1024 * 1024;
/// Minimum sane output cap.
const MIN_MAX_OUTPUT_BYTES: u64 = 1;
/// Default restart back-off delay.
const DEFAULT_RESTART_DELAY_MS: u64 = 1_000;
/// Default request/handshake timeout when a server does not override it.
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30_000;

/// Top-level MCP configuration: a keyed map of server name → server config.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct McpConfig {
    /// Keyed server map. Keys are stable server identifiers (no `.`).
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerConfig>,
}

impl McpConfig {
    /// Parse the merged MCP section from a resolved Pawork configuration.
    ///
    /// Reads only the already-merged `extra["mcp"]` value; performs no
    /// filesystem access. Returns an empty config when the section is absent.
    pub fn from_resolved(resolved: &ResolvedConfig) -> Result<Self, McpError> {
        let Some(value) = resolved.config.extra.get("mcp") else {
            return Ok(Self::default());
        };
        if value.is_null() {
            return Ok(Self::default());
        }
        let config: Self = serde_json::from_value(value.clone())
            .map_err(|error| McpError::Config(format!("invalid mcp section: {error}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Parse an MCP config from a raw JSON value (already merged). Useful for
    /// tests and for callers that assemble the merged value themselves.
    pub fn from_value(value: &Value) -> Result<Self, McpError> {
        if value.is_null() {
            return Ok(Self::default());
        }
        let config: Self = serde_json::from_value(value.clone())
            .map_err(|error| McpError::Config(format!("invalid mcp section: {error}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the full server map. Returns the receiver for chaining.
    pub fn validate(&self) -> Result<(), McpError> {
        for (name, server) in &self.servers {
            validate_server_name(name)?;
            server.validate(name)?;
        }
        Ok(())
    }

    /// Merge a higher-priority layer into `self` at whole-server granularity:
    /// a server present in `higher` replaces the one in `self`. Field-level
    /// merging across global/workspace tiers is performed by config-service at
    /// the JSON level before this type is constructed; this method is for
    /// callers that layer explicit [`McpConfig`] values.
    pub fn merge(&mut self, higher: &McpConfig) {
        for (name, server) in &higher.servers {
            self.servers.insert(name.clone(), server.clone());
        }
    }

    /// Look up a server by name.
    pub fn server(&self, name: &str) -> Option<&McpServerConfig> {
        self.servers.get(name)
    }
}

/// One MCP server's configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Transport selection — consumed by `transport.rs`.
    pub transport: TransportSpec,
    /// Start the server automatically when the agent boots. Defaults to false
    /// (explicit opt-in to spawning processes / opening connections).
    #[serde(default)]
    pub auto_start: bool,
    /// Per-call timeout in milliseconds; `None` means the client default.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Crash-restart policy.
    #[serde(default)]
    pub restart: RestartPolicy,
    /// Per-server invocation policy and allowlists.
    #[serde(default)]
    pub permissions: McpPermissions,
    /// Whether this server may run in an untrusted workspace floor.
    #[serde(default)]
    pub trusted: bool,
}

impl McpServerConfig {
    fn validate(&self, name: &str) -> Result<(), McpError> {
        self.transport.validate(name)?;
        if let Some(timeout) = self.timeout_ms {
            if timeout == 0 {
                return Err(McpError::Config(format!(
                    "server '{name}': timeout_ms must be greater than 0"
                )));
            }
        }
        self.restart.validate(name)?;
        self.permissions.validate(name)?;
        Ok(())
    }

    /// Construct a managed client from this server configuration.
    ///
    /// The returned connector retains only [`SecretRef`] values. Plaintext is
    /// resolved immediately before each transport connection/reconnection and
    /// is never written back into the parsed configuration.
    pub fn build_client(
        &self,
        name: impl Into<Arc<str>>,
        backend: Arc<dyn SecretBackend>,
    ) -> Result<ManagedMcpClient, McpError> {
        let name = name.into();
        self.validate(&name)?;
        let connector: Arc<dyn McpConnector> = Arc::new(SecretResolvingConnector::new(
            self.transport.clone(),
            backend,
        ));
        Ok(ManagedMcpClient::new(connector, self.runtime_options(name)))
    }

    /// Map persisted timeout/restart settings into lifecycle-manager options.
    pub fn runtime_options(&self, name: impl Into<Arc<str>>) -> ManagedMcpClientOptions {
        let base_delay = Duration::from_millis(self.restart.delay_ms);
        let max_attempts = if self.restart.enabled {
            // Config expresses retries after the initial attempt; the manager
            // expresses the total bounded attempt count.
            self.restart.max_restarts.saturating_add(1)
        } else {
            1
        };
        ManagedMcpClientOptions {
            name: name.into(),
            request_timeout: Duration::from_millis(
                self.timeout_ms.unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS),
            ),
            restart: RuntimeRestartPolicy {
                max_attempts,
                base_delay,
                max_delay: base_delay.saturating_mul(16),
            },
        }
    }
}

/// Transport configuration consumed by `transport.rs` to build a live rmcp
/// transport. Secret-bearing values are always persisted as [`SecretRef`] and
/// resolved against a `SecretBackend` immediately before transport connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TransportSpec {
    /// Spawn a child process speaking MCP over stdio.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, SecretValue>,
    },
    /// Connect to a streamable-http MCP endpoint.
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, SecretValue>,
    },
}

impl TransportSpec {
    fn validate(&self, name: &str) -> Result<(), McpError> {
        match self {
            TransportSpec::Stdio { command, .. } => {
                if command.trim().is_empty() {
                    return Err(McpError::Config(format!(
                        "server '{name}': stdio transport requires a non-empty command"
                    )));
                }
            }
            TransportSpec::Http { url, headers } => {
                let parsed = Url::parse(url).map_err(|error| {
                    McpError::Config(format!("server '{name}': invalid http url: {error}"))
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(McpError::Config(format!(
                        "server '{name}': http transport requires an http/https url (got '{}')",
                        parsed.scheme()
                    )));
                }
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    return Err(McpError::Config(format!(
                        "server '{name}': credentials must use SecretRef headers, not URL userinfo"
                    )));
                }
                if parsed.fragment().is_some() {
                    return Err(McpError::Config(format!(
                        "server '{name}': http endpoint must not contain a URL fragment"
                    )));
                }
                if parsed.scheme() == "http" && !headers.is_empty() && !is_loopback_url(&parsed) {
                    return Err(McpError::Config(format!(
                        "server '{name}': secret-bearing headers require HTTPS for non-loopback endpoints"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Kind discriminator for diagnostics.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Http { .. } => "http",
        }
    }

    /// Resolve secret references into the transport's short-lived runtime
    /// configuration. The returned value redacts all secret-bearing fields in
    /// `Debug` and must not be persisted.
    pub fn resolve_transport(
        &self,
        backend: &dyn SecretBackend,
    ) -> Result<TransportConfig, McpError> {
        match self {
            Self::Stdio { command, args, env } => {
                let env = resolve_secret_map(env, backend)?;
                Ok(TransportConfig::Stdio(
                    StdioTransportConfig::new(command.clone())
                        .with_args(args.clone())
                        .with_env(env),
                ))
            }
            Self::Http { url, headers } => {
                let headers = resolve_secret_map(headers, backend)?;
                Ok(TransportConfig::Http(
                    HttpTransportConfig::new(url.clone()).with_headers(headers),
                ))
            }
        }
    }
}

/// A secret-bearing configuration value.
///
/// Inline plaintext is intentionally not representable: persisted MCP config
/// contains only the backend locator and resolves it just in time.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SecretValue {
    /// Reference into a `SecretBackend`.
    SecretRef(SecretRef),
}

impl SecretValue {
    /// Resolve to plaintext against `backend`.
    pub fn resolve(&self, backend: &dyn auth_service::SecretBackend) -> Result<String, McpError> {
        match self {
            SecretValue::SecretRef(reference) => {
                Ok(reference.resolve(backend)?.expose_secret().to_string())
            }
        }
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretValue::SecretRef(reference) => {
                formatter.debug_tuple("SecretRef").field(reference).finish()
            }
        }
    }
}

/// Connector that keeps persisted references at rest and resolves them only
/// while constructing a concrete transport.
#[derive(Clone)]
pub struct SecretResolvingConnector {
    spec: TransportSpec,
    backend: Arc<dyn SecretBackend>,
}

impl SecretResolvingConnector {
    pub fn new(spec: TransportSpec, backend: Arc<dyn SecretBackend>) -> Self {
        Self { spec, backend }
    }
}

impl fmt::Debug for SecretResolvingConnector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretResolvingConnector")
            .field("spec", &self.spec)
            .field("backend", &"[SECRET_BACKEND]")
            .finish()
    }
}

#[async_trait]
impl McpConnector for SecretResolvingConnector {
    fn transport_name(&self) -> &'static str {
        self.spec.kind()
    }

    async fn connect(&self) -> Result<RunningClient, McpError> {
        let runtime = self.spec.resolve_transport(self.backend.as_ref())?;
        DefaultConnector::new(runtime).connect().await
    }
}

/// Restart-on-crash policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartPolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub max_restarts: u32,
    #[serde(default = "default_restart_delay_ms")]
    pub delay_ms: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_restarts: 0,
            delay_ms: DEFAULT_RESTART_DELAY_MS,
        }
    }
}

impl RestartPolicy {
    fn validate(&self, name: &str) -> Result<(), McpError> {
        if self.enabled && self.max_restarts == 0 {
            return Err(McpError::Config(format!(
                "server '{name}': restart is enabled but max_restarts is 0"
            )));
        }
        if self.enabled && self.delay_ms == 0 {
            return Err(McpError::Config(format!(
                "server '{name}': restart delay_ms must be greater than 0"
            )));
        }
        Ok(())
    }
}

/// Per-server invocation policy and allowlists, surfaced to the capability
/// adapter (see `capabilities::McpInvocationPolicy`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPermissions {
    /// Approval mode used by the per-server [`policy_engine::PolicyEngine`].
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    /// Server-local tool names that may be exposed. Empty means all advertised
    /// tools are eligible (subject to policy/approval).
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,
    /// Workspace ids that may invoke this server. Empty means any workspace
    /// (subject to the trust floor).
    #[serde(default)]
    pub allowed_workspaces: BTreeSet<String>,
    /// Hard byte cap on tool output; oversized output is truncated and flagged.
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: u64,
}

impl Default for McpPermissions {
    fn default() -> Self {
        Self {
            approval_mode: ApprovalMode::ReadOnly,
            allowed_tools: BTreeSet::new(),
            allowed_workspaces: BTreeSet::new(),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl McpPermissions {
    fn validate(&self, name: &str) -> Result<(), McpError> {
        if self.max_output_bytes < MIN_MAX_OUTPUT_BYTES {
            return Err(McpError::Config(format!(
                "server '{name}': max_output_bytes must be at least {MIN_MAX_OUTPUT_BYTES}"
            )));
        }
        Ok(())
    }
}

fn default_max_output_bytes() -> u64 {
    DEFAULT_MAX_OUTPUT_BYTES
}

fn default_restart_delay_ms() -> u64 {
    DEFAULT_RESTART_DELAY_MS
}

fn resolve_secret_map(
    values: &BTreeMap<String, SecretValue>,
    backend: &dyn SecretBackend,
) -> Result<HashMap<String, String>, McpError> {
    values
        .iter()
        .map(|(key, value)| value.resolve(backend).map(|value| (key.clone(), value)))
        .collect()
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// Server names are stable identifiers and must not contain the namespace
/// separator `.` (namespaced tools are `{server}.{tool}`).
fn validate_server_name(name: &str) -> Result<(), McpError> {
    if name.is_empty() {
        return Err(McpError::Config("server name must not be empty".into()));
    }
    if name.contains('.') {
        return Err(McpError::Config(format!(
            "server name '{name}' must not contain '.' (namespace separator)"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use auth_service::MemoryBackend;
    use auth_service::SecretBackend;
    use config_service::{ConfigTier, Loader};
    use serde_json::json;

    fn stdio_server(command: &str) -> Value {
        json!({
            "transport": { "kind": "stdio", "command": command },
            "auto_start": true,
            "timeout_ms": 30_000,
            "restart": { "enabled": true, "max_restarts": 3 },
            "permissions": { "approval_mode": "ask_for_writes", "max_output_bytes": 2048 }
        })
    }

    #[test]
    fn parses_keyed_server_map_and_validates() {
        let config = McpConfig::from_value(&json!({
            "servers": {
                "filesystem": stdio_server("npx"),
                "remote": {
                    "transport": { "kind": "http", "url": "https://example.com/mcp" },
                    "permissions": { "approval_mode": "never_ask" }
                }
            }
        }))
        .expect("valid config");

        let fs = config.server("filesystem").expect("filesystem");
        assert_eq!(fs.transport.kind(), "stdio");
        assert!(fs.auto_start);
        assert_eq!(fs.timeout_ms, Some(30_000));
        assert!(fs.restart.enabled);
        assert_eq!(fs.restart.max_restarts, 3);
        assert_eq!(fs.permissions.approval_mode, ApprovalMode::AskForWrites);
        assert_eq!(fs.permissions.max_output_bytes, 2048);

        let remote = config.server("remote").expect("remote");
        assert_eq!(remote.transport.kind(), "http");
        assert_eq!(remote.permissions.approval_mode, ApprovalMode::NeverAsk);
        // Defaults applied for unspecified fields.
        assert!(!remote.auto_start);
        assert_eq!(
            remote.permissions.max_output_bytes,
            DEFAULT_MAX_OUTPUT_BYTES
        );
    }

    #[test]
    fn rejects_invalid_transport_and_permissions() {
        // empty stdio command
        let err = McpConfig::from_value(&json!({
            "servers": { "fs": { "transport": { "kind": "stdio", "command": " " } } }
        }))
        .expect_err("empty command");
        assert!(matches!(err, McpError::Config(_)));

        // non-http scheme
        let err = McpConfig::from_value(&json!({
            "servers": { "fs": { "transport": { "kind": "http", "url": "file:///etc/passwd" } } }
        }))
        .expect_err("file scheme");
        assert!(err.to_string().contains("http/https"));

        // zero timeout
        let err = McpConfig::from_value(&json!({
            "servers": { "fs": { "transport": { "kind": "stdio", "command": "x" }, "timeout_ms": 0 } }
        }))
        .expect_err("zero timeout");
        assert!(err.to_string().contains("timeout_ms"));

        // restart enabled with zero restarts
        let err = McpConfig::from_value(&json!({
            "servers": { "fs": { "transport": { "kind": "stdio", "command": "x" }, "restart": { "enabled": true, "max_restarts": 0 } } }
        }))
        .expect_err("bad restart");
        assert!(err.to_string().contains("max_restarts"));

        // server name with dot
        let err = McpConfig::from_value(&json!({
            "servers": { "a.b": { "transport": { "kind": "stdio", "command": "x" } } }
        }))
        .expect_err("dotted name");
        assert!(err.to_string().contains("namespace separator"));
    }

    #[test]
    fn global_workspace_merge_through_resolved_config() {
        // Global tier defines the filesystem server (stdio command + timeout);
        // workspace tier overrides auto_start and adds a remote http server.
        let resolved = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "mcp": {
                        "servers": {
                            "filesystem": {
                                "transport": { "kind": "stdio", "command": "npx" },
                                "timeout_ms": 30_000
                            }
                        }
                    }
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({
                    "mcp": {
                        "servers": {
                            "filesystem": { "auto_start": true },
                            "remote": {
                                "transport": { "kind": "http", "url": "https://example.com/mcp" }
                            }
                        }
                    }
                }),
            )
            .resolve()
            .expect("resolve");

        let config = McpConfig::from_resolved(&resolved).expect("from_resolved");
        let fs = config.server("filesystem").expect("filesystem merged");
        // command from global, auto_start from workspace, recursively merged.
        assert_eq!(fs.transport.kind(), "stdio");
        assert!(fs.auto_start);
        assert_eq!(fs.timeout_ms, Some(30_000));
        // remote added at workspace tier.
        assert!(config.server("remote").is_some());
    }

    #[test]
    fn workspace_can_switch_a_server_transport_kind() {
        let resolved = Loader::new()
            .with_value(
                ConfigTier::Global,
                "global",
                json!({
                    "mcp": {
                        "servers": {
                            "shared": {
                                "transport": {
                                    "kind": "stdio",
                                    "command": "local-server",
                                    "args": ["--local"]
                                }
                            }
                        }
                    }
                }),
            )
            .with_value(
                ConfigTier::Workspace,
                "workspace",
                json!({
                    "mcp": {
                        "servers": {
                            "shared": {
                                "transport": {
                                    "kind": "http",
                                    "url": "https://example.com/mcp"
                                }
                            }
                        }
                    }
                }),
            )
            .resolve()
            .expect("resolve");

        let config = McpConfig::from_resolved(&resolved).expect("switch transport");
        assert!(matches!(
            &config.server("shared").expect("shared").transport,
            TransportSpec::Http { url, .. } if url == "https://example.com/mcp"
        ));
    }

    #[test]
    fn from_resolved_handles_absent_and_explicit_layer_merge() {
        // No mcp section at all.
        let empty = Loader::new().resolve().expect("resolve");
        assert!(McpConfig::from_resolved(&empty).unwrap().servers.is_empty());

        // Explicit whole-server merge: workspace fully redefines filesystem.
        let mut lower = McpConfig::from_value(&json!({
            "servers": { "filesystem": stdio_server("old-command") }
        }))
        .unwrap();
        let higher = McpConfig::from_value(&json!({
            "servers": {
                "filesystem": {
                    "transport": { "kind": "http", "url": "https://example.com/mcp" }
                }
            }
        }))
        .unwrap();
        lower.merge(&higher);
        assert_eq!(lower.server("filesystem").unwrap().transport.kind(), "http");
    }

    #[test]
    fn secret_value_resolves_ref_and_inline_config_is_rejected() {
        let backend = MemoryBackend::new();
        backend
            .store("pawork.mcp", "cred-1", "sk-mcp-token")
            .expect("store");

        let reference = SecretValue::SecretRef(SecretRef::new("pawork.mcp", "cred-1"));
        assert_eq!(reference.resolve(&backend).unwrap(), "sk-mcp-token");

        let error = McpConfig::from_value(&json!({
            "servers": {
                "unsafe": {
                    "transport": {
                        "kind": "stdio",
                        "command": "server",
                        "env": {
                            "TOKEN": {
                                "kind": "inline",
                                "value": "plaintext-should-not-be-accepted"
                            }
                        }
                    }
                }
            }
        }))
        .expect_err("inline plaintext must be rejected");
        assert!(!error
            .to_string()
            .contains("plaintext-should-not-be-accepted"));
    }

    #[test]
    fn transport_resolution_injects_refs_without_debug_leakage() {
        let backend = MemoryBackend::new();
        backend
            .store("pawork.mcp", "cred-1", "sk-runtime-secret")
            .expect("store");
        let spec: TransportSpec = serde_json::from_value(json!({
            "kind": "stdio",
            "command": "server",
            "env": {
                "TOKEN": {
                    "kind": "secret_ref",
                    "service": "pawork.mcp",
                    "account": "cred-1"
                }
            }
        }))
        .expect("spec");

        let runtime = spec.resolve_transport(&backend).expect("resolve transport");
        let TransportConfig::Stdio(runtime) = runtime else {
            panic!("expected stdio runtime")
        };
        assert_eq!(
            runtime.env.get("TOKEN").map(String::as_str),
            Some("sk-runtime-secret")
        );
        assert!(!format!("{runtime:?}").contains("sk-runtime-secret"));
    }

    #[test]
    fn runtime_options_preserve_timeout_and_restart_semantics() {
        let config = McpConfig::from_value(&json!({
            "servers": { "fs": stdio_server("server") }
        }))
        .expect("config");
        let options = config.server("fs").unwrap().runtime_options("fs");
        assert_eq!(options.request_timeout, Duration::from_secs(30));
        assert_eq!(options.restart.max_attempts, 4);
        assert_eq!(options.restart.base_delay, Duration::from_secs(1));
        assert_eq!(options.restart.max_delay, Duration::from_secs(16));
    }
}
