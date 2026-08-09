//! Managed MCP client: lifecycle, health, timeouts, cancellation and bounded restart.
//!
//! [`ManagedMcpClient`] wraps an injectable [`McpConnector`](crate::transport::McpConnector)
//! and exposes the transport-independent [`McpPeer`] contract. It owns the live rmcp
//! client, reports health snapshots, applies per-request timeouts, forwards
//! cooperative cancellation as MCP `notifications/cancelled`, shuts down explicitly,
//! and reconnects with bounded exponential back-off after a crash or disconnect.
//!
//! Crash isolation: no method panics. Every failure — spawn failure, handshake failure,
//! mid-flight disconnect, exhausted restart budget — is returned as a typed
//! [`McpError`], so a misbehaving server cannot take down Agent Core.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::CancellationToken;
use async_trait::async_trait;
use rmcp::model::ServerResult;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, GetPromptRequestParams,
    GetPromptResult, PingRequest, Prompt, ReadResourceRequestParams, ReadResourceResult, Resource,
    ResourceTemplate, Tool,
};
use rmcp::service::{Peer, PeerRequestOptions, RoleClient, ServiceError};
use tokio::sync::Mutex;

use crate::error::McpError;
use crate::session::{McpPeer, McpServerCapabilities};
use crate::transport::{McpConnector, RunningClient};

/// Coarse lifecycle state shown in [`HealthSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// No live connection (initial, or after explicit shutdown).
    Disconnected,
    /// A connect/reconnect attempt is in progress.
    Connecting,
    /// A live client is ready to serve requests.
    Connected,
    /// The last connect/reconnect attempt failed; the bounded restart budget may
    /// still allow further attempts after the back-off window elapses.
    Failed,
}

/// Point-in-time view of the managed client's health.
#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    pub state: ConnectionState,
    /// Transport name reported by the connector (e.g. `"stdio"`).
    pub transport: &'static str,
    /// Last error message, secret-safe (configs redact secrets before formatting).
    pub last_error: Option<Arc<str>>,
    pub last_connected_at: Option<Instant>,
    /// Consecutive failed connect/reconnect attempts since the last success.
    pub restart_attempts: u32,
    pub max_restart_attempts: u32,
}

/// Bounded exponential back-off for reconnect attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    /// Maximum consecutive failed attempts before giving up (per back-off window).
    pub max_attempts: u32,
    /// Initial delay; doubled after each failure.
    pub base_delay: Duration,
    /// Cap for the per-attempt delay.
    pub max_delay: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(10),
        }
    }
}

/// Tunables for [`ManagedMcpClient`].
#[derive(Debug, Clone)]
pub struct ManagedMcpClientOptions {
    pub name: Arc<str>,
    /// Per-request deadline applied to every peer operation.
    pub request_timeout: Duration,
    pub restart: RestartPolicy,
}

/// A managed MCP client.
///
/// Holds at most one live rmcp client. Operations clone the rmcp [`Peer`] (cheap,
/// `Send + Sync`) so request execution never holds the internal state lock. A separate
/// lifecycle gate serializes connection replacement while health snapshots remain
/// responsive during back-off, authentication refresh and handshakes.
pub struct ManagedMcpClient {
    connector: Arc<dyn McpConnector>,
    options: ManagedMcpClientOptions,
    lifecycle: Mutex<()>,
    state: Mutex<ClientState>,
    shutdown: CancellationToken,
}

#[derive(Default)]
struct ClientState {
    running: Option<RunningClient>,
    last_error: Option<Arc<str>>,
    last_connected_at: Option<Instant>,
    last_connect_attempt: Option<Instant>,
    /// Consecutive failed attempts since the last successful connect.
    restart_attempts: u32,
    connecting: bool,
    shutdown_requested: bool,
}

impl ManagedMcpClient {
    /// Build a managed client from an explicit connector and options.
    pub fn new(connector: Arc<dyn McpConnector>, options: ManagedMcpClientOptions) -> Self {
        Self {
            connector,
            options,
            lifecycle: Mutex::new(()),
            state: Mutex::new(ClientState::default()),
            shutdown: CancellationToken::new(),
        }
    }

    /// Build with sensible defaults: 30s request timeout and the default restart policy.
    pub fn with_defaults(connector: Arc<dyn McpConnector>, name: impl Into<Arc<str>>) -> Self {
        Self::new(
            connector,
            ManagedMcpClientOptions {
                name: name.into(),
                request_timeout: Duration::from_secs(30),
                restart: RestartPolicy::default(),
            },
        )
    }

    pub fn options(&self) -> &ManagedMcpClientOptions {
        &self.options
    }

    /// Acquire a live peer, reconnecting first if necessary.
    async fn peer(&self) -> Result<Peer<RoleClient>, McpError> {
        self.peer_with_cancel(None).await
    }

    /// Acquire a peer while allowing an Agent cancellation token to interrupt
    /// lifecycle-gate waits, OAuth refresh, reconnect back-off and handshake.
    async fn peer_with_cancel(
        &self,
        cancel: Option<&CancellationToken>,
    ) -> Result<Peer<RoleClient>, McpError> {
        let _lifecycle = self.interruptible(self.lifecycle.lock(), cancel).await?;

        let replace = self
            .interruptible(self.connector.should_reconnect_before_request(), cancel)
            .await??;
        let mut state = self.state.lock().await;
        if state.shutdown_requested {
            return Err(self.shutdown_error());
        }
        if replace {
            drop(state.running.take());
        }
        if let Some(running) = state.running.as_ref().filter(|running| !is_dead(running)) {
            return Ok(running.peer().clone());
        }
        drop(state);

        self.reconnect(cancel).await?;
        let state = self.state.lock().await;
        let running = state.running.as_ref().ok_or_else(|| {
            McpError::Disconnected(format!("mcp client {} not connected", self.options.name))
        })?;
        Ok(running.peer().clone())
    }

    /// Drop the current client and run one reconnect cycle (bounded back-off).
    async fn force_reconnect(&self) -> Result<(), McpError> {
        let _lifecycle = self.interruptible(self.lifecycle.lock(), None).await?;
        let mut state = self.state.lock().await;
        if state.shutdown_requested {
            return Err(self.shutdown_error());
        }
        drop(state.running.take());
        drop(state);
        self.reconnect(None).await
    }

    /// Reconnect while the caller holds the lifecycle gate. State updates are
    /// deliberately short; no state lock is held across sleep, auth or I/O.
    async fn reconnect(&self, cancel: Option<&CancellationToken>) -> Result<(), McpError> {
        let policy = self.options.restart;
        {
            let mut state = self.state.lock().await;
            // Drop any stale handle; its background task cleans up the transport/child.
            drop(state.running.take());
            if state.shutdown_requested {
                return Err(self.shutdown_error());
            }

            // Allow a fresh retry cycle once a full back-off window has elapsed since
            // the last attempt, so a transient failure does not lock the client out.
            if state.restart_attempts >= policy.max_attempts {
                let cooldown = state
                    .last_connect_attempt
                    .is_none_or(|t| t.elapsed() >= policy.max_delay * 4);
                if !cooldown {
                    return Err(disconnected_error(&state, &self.options.name));
                }
                state.restart_attempts = 0;
            }
            state.connecting = true;
        }

        loop {
            let failed_attempts = {
                let mut state = self.state.lock().await;
                if state.shutdown_requested {
                    state.connecting = false;
                    return Err(self.shutdown_error());
                }
                if state.restart_attempts >= policy.max_attempts {
                    state.connecting = false;
                    return Err(disconnected_error(&state, &self.options.name));
                }
                state.restart_attempts
            };

            if failed_attempts == 0 {
                tracing::info!(name = %self.options.name, transport = self.connector.transport_name(), "connecting mcp client");
            } else {
                let delay = backoff(policy.base_delay, policy.max_delay, failed_attempts - 1);
                tracing::warn!(
                    name = %self.options.name,
                    attempt = failed_attempts,
                    delay = ?delay,
                    "reconnecting mcp client"
                );
                if let Err(error) = self.interruptible(tokio::time::sleep(delay), cancel).await {
                    self.stop_connecting().await;
                    return Err(error);
                }
            }

            self.state.lock().await.last_connect_attempt = Some(Instant::now());
            let connected = match self
                .interruptible(
                    tokio::time::timeout(self.options.request_timeout, self.connector.connect()),
                    cancel,
                )
                .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(McpError::Timeout(self.options.request_timeout)),
                Err(error) => {
                    self.stop_connecting().await;
                    return Err(error);
                }
            };
            match connected {
                Ok(running) => {
                    let mut state = self.state.lock().await;
                    if state.shutdown_requested {
                        state.connecting = false;
                        drop(state);
                        drop(running);
                        return Err(self.shutdown_error());
                    }
                    state.restart_attempts = 0;
                    state.last_error = None;
                    state.last_connected_at = Some(Instant::now());
                    state.running = Some(running);
                    state.connecting = false;
                    tracing::info!(name = %self.options.name, "mcp client connected");
                    return Ok(());
                }
                Err(error) => {
                    let msg: Arc<str> = Arc::from(error.to_string());
                    tracing::warn!(name = %self.options.name, error = %msg, "mcp connect attempt failed");
                    let mut state = self.state.lock().await;
                    state.restart_attempts = state.restart_attempts.saturating_add(1);
                    state.last_error = Some(msg);
                }
            }
        }
    }

    async fn stop_connecting(&self) {
        self.state.lock().await.connecting = false;
    }

    fn shutdown_error(&self) -> McpError {
        McpError::Disconnected(format!(
            "mcp client {} has been shut down",
            self.options.name
        ))
    }

    /// Race lifecycle work against explicit shutdown and, when supplied, the
    /// Agent cancellation token. Cancellation is biased to win ready races.
    async fn interruptible<F, T>(
        &self,
        future: F,
        cancel: Option<&CancellationToken>,
    ) -> Result<T, McpError>
    where
        F: Future<Output = T>,
    {
        tokio::pin!(future);
        if let Some(cancel) = cancel {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(McpError::Cancelled),
                _ = self.shutdown.cancelled() => Err(self.shutdown_error()),
                output = &mut future => Ok(output),
            }
        } else {
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => Err(self.shutdown_error()),
                output = &mut future => Ok(output),
            }
        }
    }

    /// Snapshot of the client's current health.
    pub async fn health(&self) -> HealthSnapshot {
        let state = self.state.lock().await;
        let live = state.running.as_ref().is_some_and(|r| !is_dead(r));
        let state_value = if state.shutdown_requested {
            ConnectionState::Disconnected
        } else if live {
            ConnectionState::Connected
        } else if state.connecting || state.running.is_some() {
            // reconnect in progress, or a dead handle not yet observed by an operation
            ConnectionState::Connecting
        } else if state.last_error.is_some() {
            ConnectionState::Failed
        } else {
            ConnectionState::Disconnected
        };
        HealthSnapshot {
            state: state_value,
            transport: self.connector.transport_name(),
            last_error: state.last_error.clone(),
            last_connected_at: state.last_connected_at,
            restart_attempts: state.restart_attempts,
            max_restart_attempts: self.options.restart.max_attempts,
        }
    }

    /// Send a protocol-level ping with the configured request timeout.
    pub async fn ping(&self) -> Result<(), McpError> {
        let peer = self.peer().await?;
        match self
            .timed(peer.send_request(ClientRequest::PingRequest(PingRequest::default())))
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                if should_retry(&e) {
                    self.force_reconnect().await?;
                    let peer = self.peer().await?;
                    self.timed(
                        peer.send_request(ClientRequest::PingRequest(PingRequest::default())),
                    )
                    .await
                    .map_err(map_service_error)?;
                    Ok(())
                } else {
                    Err(map_service_error(e))
                }
            }
        }
    }

    /// Explicitly close the connection and forbid further use.
    ///
    /// Cancels the rmcp service with a bounded shutdown window so a wedged server
    /// cannot block shutdown indefinitely.
    pub async fn shutdown(&self) -> Result<(), McpError> {
        self.shutdown.cancel();
        let running = {
            let mut state = self.state.lock().await;
            state.shutdown_requested = true;
            state.connecting = false;
            state.last_error = None;
            state.running.take()
        };
        if let Some(mut running) = running {
            let _ = running.close_with_timeout(Duration::from_secs(5)).await;
        }
        tracing::info!(name = %self.options.name, "mcp client shut down");
        Ok(())
    }

    /// Race a future against the configured request timeout.
    async fn timed<F, T>(&self, fut: F) -> Result<T, ServiceError>
    where
        F: Future<Output = Result<T, ServiceError>>,
    {
        match tokio::time::timeout(self.options.request_timeout, fut).await {
            Ok(inner) => inner,
            Err(_) => Err(ServiceError::Timeout {
                timeout: self.options.request_timeout,
            }),
        }
    }
}

fn is_dead(running: &RunningClient) -> bool {
    running.is_closed() || running.peer().is_transport_closed()
}

fn should_retry(error: &ServiceError) -> bool {
    matches!(
        error,
        ServiceError::TransportClosed | ServiceError::TransportSend(_)
    )
}

fn map_service_error(error: ServiceError) -> McpError {
    match error {
        ServiceError::McpError(data) => McpError::Protocol(data.to_string()),
        ServiceError::TransportSend(inner) => {
            McpError::Disconnected(format!("mcp transport send failed: {inner}"))
        }
        ServiceError::TransportClosed => McpError::Disconnected("mcp transport closed".into()),
        ServiceError::UnexpectedResponse => {
            McpError::Protocol("unexpected response from mcp server".into())
        }
        ServiceError::Cancelled { .. } => McpError::Cancelled,
        ServiceError::Timeout { timeout } => McpError::Timeout(timeout),
        // ServiceError is #[non_exhaustive]; future rmcp variants surface as transport
        // failures so callers still get a typed error instead of a compile break.
        other => McpError::Transport(format!("mcp service error: {other}")),
    }
}

fn disconnected_error(state: &ClientState, name: &str) -> McpError {
    let detail = state.last_error.as_deref().unwrap_or("unknown error");
    McpError::Disconnected(format!(
        "mcp client {name} disconnected after {attempts} attempts: {detail}",
        attempts = state.restart_attempts
    ))
}

fn backoff(base: Duration, max: Duration, attempt_zero_indexed: u32) -> Duration {
    let exp = attempt_zero_indexed.min(30);
    let factor = 1u32 << exp;
    std::cmp::min(base.saturating_mul(factor), max)
}

#[async_trait]
impl McpPeer for ManagedMcpClient {
    async fn server_capabilities(&self) -> Result<McpServerCapabilities, McpError> {
        let peer = self.peer().await?;
        let info = peer.peer_info().ok_or_else(|| {
            McpError::Protocol("MCP server omitted initialize handshake information".into())
        })?;
        Ok(McpServerCapabilities {
            tools: info.capabilities.tools.is_some(),
            resources: info.capabilities.resources.is_some(),
            prompts: info.capabilities.prompts.is_some(),
        })
    }

    async fn list_tools(&self) -> Result<Vec<Tool>, McpError> {
        let peer = self.peer().await?;
        match self.timed(peer.list_all_tools()).await {
            Ok(v) => Ok(v),
            Err(e) if should_retry(&e) => {
                self.force_reconnect().await?;
                let peer = self.peer().await?;
                self.timed(peer.list_all_tools())
                    .await
                    .map_err(map_service_error)
            }
            Err(e) => Err(map_service_error(e)),
        }
    }

    async fn list_resources(&self) -> Result<Vec<Resource>, McpError> {
        let peer = self.peer().await?;
        match self.timed(peer.list_all_resources()).await {
            Ok(v) => Ok(v),
            Err(e) if should_retry(&e) => {
                self.force_reconnect().await?;
                let peer = self.peer().await?;
                self.timed(peer.list_all_resources())
                    .await
                    .map_err(map_service_error)
            }
            Err(e) => Err(map_service_error(e)),
        }
    }

    async fn list_resource_templates(&self) -> Result<Vec<ResourceTemplate>, McpError> {
        let peer = self.peer().await?;
        match self.timed(peer.list_all_resource_templates()).await {
            Ok(v) => Ok(v),
            Err(e) if should_retry(&e) => {
                self.force_reconnect().await?;
                let peer = self.peer().await?;
                self.timed(peer.list_all_resource_templates())
                    .await
                    .map_err(map_service_error)
            }
            Err(e) => Err(map_service_error(e)),
        }
    }

    async fn list_prompts(&self) -> Result<Vec<Prompt>, McpError> {
        let peer = self.peer().await?;
        match self.timed(peer.list_all_prompts()).await {
            Ok(v) => Ok(v),
            Err(e) if should_retry(&e) => {
                self.force_reconnect().await?;
                let peer = self.peer().await?;
                self.timed(peer.list_all_prompts())
                    .await
                    .map_err(map_service_error)
            }
            Err(e) => Err(map_service_error(e)),
        }
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, McpError> {
        let peer = self.peer().await?;
        match self.timed(peer.read_resource(params.clone())).await {
            Ok(v) => Ok(v),
            Err(e) if should_retry(&e) => {
                self.force_reconnect().await?;
                let peer = self.peer().await?;
                self.timed(peer.read_resource(params))
                    .await
                    .map_err(map_service_error)
            }
            Err(e) => Err(map_service_error(e)),
        }
    }

    async fn get_prompt(
        &self,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, McpError> {
        let peer = self.peer().await?;
        match self.timed(peer.get_prompt(params.clone())).await {
            Ok(v) => Ok(v),
            Err(e) if should_retry(&e) => {
                self.force_reconnect().await?;
                let peer = self.peer().await?;
                self.timed(peer.get_prompt(params))
                    .await
                    .map_err(map_service_error)
            }
            Err(e) => Err(map_service_error(e)),
        }
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        cancel: CancellationToken,
    ) -> Result<CallToolResult, McpError> {
        let peer = self.peer_with_cancel(Some(&cancel)).await?;
        // call_tool is never auto-retried: tools may have side effects, so a transport
        // close mid-call must surface to the caller rather than re-execute.
        let handle = self
            .interruptible(
                peer.send_cancellable_request(
                    ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                    PeerRequestOptions::with_timeout(self.options.request_timeout),
                ),
                Some(&cancel),
            )
            .await?
            .map_err(map_service_error)?;

        // `select!` is biased so the cancellation token wins the race when both are
        // ready. `handle` is wrapped in an Option so whichever branch fires first
        // takes ownership; the other becomes a no-op.
        let mut handle = Some(handle);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                if let Some(h) = handle.take() {
                    let _ = h
                        .cancel(Some("client requested cancellation".to_string()))
                        .await;
                }
                Err(McpError::Cancelled)
            }
            result = async {
                let Some(h) = handle.take() else {
                    return Err(McpError::Protocol(
                        "MCP call handle was consumed before response".into(),
                    ));
                };
                h.await_response().await.map_err(map_service_error)
            } => match result? {
                ServerResult::CallToolResult(ct) => Ok(ct),
                other => Err(McpError::Protocol(format!(
                    "unexpected call_tool response: {other:?}"
                ))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{
        CallToolRequestParams, ContentBlock, JsonObject, ListToolsResult, ServerCapabilities,
        ServerInfo,
    };
    use rmcp::service::RequestContext;
    use rmcp::ServerHandler;
    use rmcp::{ErrorData, RoleServer, ServiceExt as _};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Behaviour injected into the in-process test server.
    #[derive(Clone, Copy)]
    enum ServerBehavior {
        Echo,
        /// Sleeps before responding so timeout/cancel can be exercised.
        Slow {
            delay: Duration,
        },
    }

    #[derive(Clone)]
    struct EchoServer {
        behavior: ServerBehavior,
    }

    impl ServerHandler for EchoServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send {
            let tool = Tool::new(
                "echo",
                "echo the tool name back",
                Arc::new(JsonObject::new()),
            );
            std::future::ready(Ok(ListToolsResult::with_all_items(vec![tool])))
        }

        fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<CallToolResult, ErrorData>> + Send {
            let behavior = self.behavior;
            async move {
                match behavior {
                    ServerBehavior::Echo => {
                        let text = format!("echo: {}", request.name);
                        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
                    }
                    ServerBehavior::Slow { delay } => {
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {
                                Ok(CallToolResult::success(vec![ContentBlock::text("slow-done")]))
                            }
                            _ = context.ct.cancelled() => {
                                Ok(CallToolResult::success(vec![ContentBlock::text("cancelled")]))
                            }
                        }
                    }
                }
            }
        }
    }

    /// In-process connector: each `connect` wires the client to a freshly spawned
    /// in-memory server. Optionally fails the first `fail_until` attempts to exercise
    /// bounded restart.
    struct TestConnector {
        server: EchoServer,
        fail_until: u32,
        connect_delay: Duration,
        calls: Arc<AtomicU32>,
    }

    impl TestConnector {
        fn echo() -> Self {
            Self {
                server: EchoServer {
                    behavior: ServerBehavior::Echo,
                },
                fail_until: 0,
                connect_delay: Duration::ZERO,
                calls: Arc::new(AtomicU32::new(0)),
            }
        }

        fn slow(delay: Duration) -> Self {
            Self {
                server: EchoServer {
                    behavior: ServerBehavior::Slow { delay },
                },
                fail_until: 0,
                connect_delay: Duration::ZERO,
                calls: Arc::new(AtomicU32::new(0)),
            }
        }

        fn failing(fail_until: u32, behavior: ServerBehavior) -> Self {
            Self {
                server: EchoServer { behavior },
                fail_until,
                connect_delay: Duration::ZERO,
                calls: Arc::new(AtomicU32::new(0)),
            }
        }

        fn delayed(delay: Duration) -> Self {
            Self {
                server: EchoServer {
                    behavior: ServerBehavior::Echo,
                },
                fail_until: 0,
                connect_delay: delay,
                calls: Arc::new(AtomicU32::new(0)),
            }
        }
    }

    #[async_trait]
    impl McpConnector for TestConnector {
        fn transport_name(&self) -> &'static str {
            "test-in-process"
        }

        async fn connect(&self) -> Result<RunningClient, McpError> {
            if !self.connect_delay.is_zero() {
                tokio::time::sleep(self.connect_delay).await;
            }
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_until {
                return Err(McpError::Transport(format!(
                    "injected connect failure #{n}"
                )));
            }
            let (server_io, client_io) = tokio::io::duplex(8192);
            let server = self.server.clone();
            tokio::spawn(async move {
                let running = match server.serve(server_io).await {
                    Ok(r) => r,
                    Err(error) => {
                        tracing::warn!(%error, "test server failed to initialize");
                        return;
                    }
                };
                let _ = running.waiting().await;
            });
            ().serve(client_io)
                .await
                .map_err(|e| McpError::Transport(format!("test client handshake failed: {e}")))
        }
    }

    fn fast_options(name: &str) -> ManagedMcpClientOptions {
        ManagedMcpClientOptions {
            name: Arc::from(name),
            request_timeout: Duration::from_secs(5),
            restart: RestartPolicy {
                max_attempts: 4,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
            },
        }
    }

    fn client(connector: TestConnector, options: ManagedMcpClientOptions) -> ManagedMcpClient {
        let arc: Arc<dyn McpConnector> = Arc::new(connector);
        ManagedMcpClient::new(arc, options)
    }

    #[tokio::test]
    async fn handshake_list_call_ping_succeed() {
        let connector = TestConnector::echo();
        let client = client(connector, fast_options("echo"));

        let tools = client.list_tools().await.expect("list_tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");

        let result = client
            .call_tool(CallToolRequestParams::new("echo"), CancellationToken::new())
            .await
            .expect("call_tool");
        let echoed = result
            .content
            .iter()
            .any(|c| matches!(c, ContentBlock::Text(t) if t.text.contains("echo: echo")));
        assert!(echoed, "expected echoed content, got {result:?}");

        client.ping().await.expect("ping");

        let health = client.health().await;
        assert_eq!(health.state, ConnectionState::Connected);
        assert_eq!(health.transport, "test-in-process");
        assert!(health.last_error.is_none());
    }

    #[tokio::test]
    async fn call_tool_times_out() {
        let connector = TestConnector::slow(Duration::from_secs(3));
        let mut options = fast_options("slow");
        options.request_timeout = Duration::from_millis(150);
        let client = client(connector, options);

        let error = client
            .call_tool(CallToolRequestParams::new("echo"), CancellationToken::new())
            .await
            .expect_err("expected timeout");
        assert!(matches!(error, McpError::Timeout(_)), "got {error:?}");
    }

    #[tokio::test]
    async fn handshake_is_bounded_by_request_timeout() {
        let connector = TestConnector::delayed(Duration::from_secs(5));
        let mut options = fast_options("slow-handshake");
        options.request_timeout = Duration::from_millis(25);
        options.restart.max_attempts = 1;
        let client = client(connector, options);

        let started = Instant::now();
        let error = client.ping().await.expect_err("handshake must time out");
        assert!(matches!(error, McpError::Disconnected(_)), "got {error:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn call_tool_is_cancellable() {
        let connector = TestConnector::slow(Duration::from_secs(10));
        let mut options = fast_options("cancel");
        options.request_timeout = Duration::from_secs(10);
        let client = client(connector, options);

        let token = CancellationToken::new();
        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel_token.cancel();
        });

        let error = client
            .call_tool(CallToolRequestParams::new("echo"), token)
            .await
            .expect_err("expected cancellation");
        assert!(matches!(error, McpError::Cancelled), "got {error:?}");
    }

    #[tokio::test]
    async fn call_tool_cancels_during_handshake() {
        let connector = TestConnector::delayed(Duration::from_secs(10));
        let mut options = fast_options("cancel-handshake");
        options.request_timeout = Duration::from_secs(10);
        let client = client(connector, options);

        let token = CancellationToken::new();
        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_token.cancel();
        });

        let started = Instant::now();
        let error = client
            .call_tool(CallToolRequestParams::new("echo"), token)
            .await
            .expect_err("expected handshake cancellation");
        assert!(matches!(error, McpError::Cancelled), "got {error:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn health_remains_responsive_while_connecting() {
        let connector = TestConnector::delayed(Duration::from_secs(10));
        let mut options = fast_options("responsive-health");
        options.request_timeout = Duration::from_secs(10);
        options.restart.max_attempts = 1;
        let client = Arc::new(client(connector, options));

        let ping_client = client.clone();
        let ping = tokio::spawn(async move { ping_client.ping().await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let health = tokio::time::timeout(Duration::from_millis(200), client.health())
            .await
            .expect("health must not wait for handshake");
        assert_eq!(health.state, ConnectionState::Connecting);

        client.shutdown().await.expect("shutdown");
        let error = ping
            .await
            .expect("ping task")
            .expect_err("shutdown must interrupt handshake");
        assert!(matches!(error, McpError::Disconnected(_)), "got {error:?}");
    }

    #[tokio::test]
    async fn bounded_restart_eventually_connects() {
        // fail the first two attempts, then succeed; budget allows four.
        let connector = TestConnector::failing(2, ServerBehavior::Echo);
        let client = client(connector, fast_options("retry"));

        let tools = client.list_tools().await.expect("list_tools after retries");
        assert_eq!(tools.len(), 1);

        let health = client.health().await;
        assert_eq!(health.state, ConnectionState::Connected);
        assert_eq!(health.restart_attempts, 0, "counter resets after success");
    }

    #[tokio::test]
    async fn restart_budget_exhaustion_is_isolated() {
        // always fail: ten injected failures vs a budget of two.
        let connector = TestConnector::failing(10, ServerBehavior::Echo);
        let mut options = fast_options("dead");
        options.restart = RestartPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        };
        let client = client(connector, options);

        // operations return a typed error instead of panicking ...
        let error = client.list_tools().await.expect_err("expected failure");
        assert!(matches!(error, McpError::Disconnected(_)), "got {error:?}");

        // ... and the health snapshot reflects the exhausted budget.
        let health = client.health().await;
        assert_eq!(health.state, ConnectionState::Failed);
        assert_eq!(health.restart_attempts, 2);
        assert_eq!(health.max_restart_attempts, 2);
        assert!(health.last_error.is_some());
    }

    #[tokio::test]
    async fn shutdown_blocks_further_use() {
        let connector = TestConnector::echo();
        let client = client(connector, fast_options("shutdown"));

        // establish a connection first
        client.ping().await.expect("ping before shutdown");
        client.shutdown().await.expect("shutdown");

        let health = client.health().await;
        assert_eq!(health.state, ConnectionState::Disconnected);

        let error = client.ping().await.expect_err("expected disconnected");
        assert!(matches!(error, McpError::Disconnected(_)));
    }

    #[tokio::test]
    async fn backoff_is_exponential_and_capped() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(1);
        assert_eq!(backoff(base, max, 0), Duration::from_millis(100));
        assert_eq!(backoff(base, max, 1), Duration::from_millis(200));
        assert_eq!(backoff(base, max, 2), Duration::from_millis(400));
        assert_eq!(backoff(base, max, 3), Duration::from_millis(800));
        // capped at max despite 2^4 * 100ms = 1600ms
        assert_eq!(backoff(base, max, 4), max);
    }
}
