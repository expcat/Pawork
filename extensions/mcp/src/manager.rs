//! Managed MCP client: lifecycle, health, timeouts, cancellation and bounded restart.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pawork_api::ToolResult;
use pawork_domain::CancellationToken;
use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::codec::{self, ClientPeer, RunningClient};
use crate::config::RestartPolicy;
use crate::transport::McpConnector;
use crate::{McpError, McpPeer, McpServerCapabilities, McpToolCall, McpToolInfo};

/// Coarse lifecycle state shown in [`HealthSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

/// Point-in-time view of the managed client's health.
#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    pub state: ConnectionState,
    pub transport: &'static str,
    pub last_error: Option<Arc<str>>,
    pub last_connected_at: Option<Instant>,
    pub restart_attempts: u32,
    pub max_restart_attempts: u32,
}

/// Tunables for [`ManagedMcpClient`].
#[derive(Debug, Clone)]
pub struct ManagedMcpClientOptions {
    pub name: Arc<str>,
    pub request_timeout: Duration,
    pub restart: RestartPolicy,
}

/// A managed MCP client.
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
    restart_attempts: u32,
    connecting: bool,
    shutdown_requested: bool,
}

impl ManagedMcpClient {
    pub(crate) fn new(connector: Arc<dyn McpConnector>, options: ManagedMcpClientOptions) -> Self {
        Self {
            connector,
            options,
            lifecycle: Mutex::new(()),
            state: Mutex::new(ClientState::default()),
            shutdown: CancellationToken::new(),
        }
    }

    pub(crate) fn with_defaults(connector: Arc<dyn McpConnector>, name: impl Into<Arc<str>>) -> Self {
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

    async fn peer(&self) -> Result<ClientPeer, McpError> {
        self.peer_with_cancel(None).await
    }

    async fn peer_with_cancel(
        &self,
        cancel: Option<&CancellationToken>,
    ) -> Result<ClientPeer, McpError> {
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
        if let Some(running) = state.running.as_ref().filter(|running| !running.is_dead()) {
            return Ok(running.peer());
        }
        drop(state);

        self.reconnect(cancel).await?;
        let state = self.state.lock().await;
        let running = state.running.as_ref().ok_or_else(|| {
            McpError::Disconnected(format!("mcp client {} not connected", self.options.name))
        })?;
        Ok(running.peer())
    }

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

    async fn reconnect(&self, cancel: Option<&CancellationToken>) -> Result<(), McpError> {
        let policy = self.options.restart.clone();
        {
            let mut state = self.state.lock().await;
            drop(state.running.take());
            if state.shutdown_requested {
                return Err(self.shutdown_error());
            }

            if state.restart_attempts >= policy.max_attempts {
                let cooldown = state
                    .last_connect_attempt
                    .is_none_or(|t| t.elapsed() >= Duration::from_millis(policy.max_delay_ms) * 4);
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
                let delay = backoff(
                    Duration::from_millis(policy.base_delay_ms),
                    Duration::from_millis(policy.max_delay_ms),
                    failed_attempts - 1,
                );
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

    pub async fn health(&self) -> HealthSnapshot {
        let state = self.state.lock().await;
        let live = state.running.as_ref().is_some_and(|r| !r.is_dead());
        let state_value = if state.shutdown_requested {
            ConnectionState::Disconnected
        } else if live {
            ConnectionState::Connected
        } else if state.connecting || state.running.is_some() {
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

    pub async fn ping(&self) -> Result<(), McpError> {
        let peer = self.peer().await?;
        match codec::timed(self.options.request_timeout, peer.ping()).await {
            Ok(_) => Ok(()),
            Err(e) => {
                if codec::should_retry(&e) {
                    self.force_reconnect().await?;
                    let peer = self.peer().await?;
                    codec::timed(self.options.request_timeout, peer.ping()).await?;
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

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
            running.close_with_timeout(Duration::from_secs(5)).await;
        }
        tracing::info!(name = %self.options.name, "mcp client shut down");
        Ok(())
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
        peer.server_capabilities()
    }

    async fn list_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
        let peer = self.peer().await?;
        match codec::timed(self.options.request_timeout, peer.list_tools()).await {
            Ok(v) => Ok(v),
            Err(e) if codec::should_retry(&e) => {
                self.force_reconnect().await?;
                let peer = self.peer().await?;
                codec::timed(self.options.request_timeout, peer.list_tools()).await
            }
            Err(e) => Err(e),
        }
    }

    async fn call_tool(
        &self,
        call: McpToolCall,
        cancel: CancellationToken,
    ) -> Result<ToolResult, McpError> {
        let peer = self.peer_with_cancel(Some(&cancel)).await?;
        self.interruptible(
            peer.call_tool(call, self.options.request_timeout, cancel.clone()),
            Some(&cancel),
        )
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::test_support::{InProcessConnector, ServerBehavior};
    use pawork_domain::ContentPart;
    use serde_json::Map;

    fn fast_options(name: &str) -> ManagedMcpClientOptions {
        ManagedMcpClientOptions {
            name: Arc::from(name),
            request_timeout: Duration::from_secs(5),
            restart: RestartPolicy {
                max_attempts: 4,
                base_delay_ms: 1,
                max_delay_ms: 10,
            },
        }
    }

    fn client(connector: InProcessConnector, options: ManagedMcpClientOptions) -> ManagedMcpClient {
        let arc: Arc<dyn McpConnector> = Arc::new(connector);
        ManagedMcpClient::new(arc, options)
    }

    fn echo_call() -> McpToolCall {
        McpToolCall {
            name: "echo".into(),
            arguments: Map::new(),
        }
    }

    #[tokio::test]
    async fn handshake_list_call_ping_succeed() {
        let connector = InProcessConnector::echo();
        let client = client(connector, fast_options("echo"));

        let tools = client.list_tools().await.expect("list_tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");

        let result = client
            .call_tool(echo_call(), CancellationToken::new())
            .await
            .expect("call_tool");
        let echoed = result.content.iter().any(|c| {
            matches!(c, ContentPart::Text(t) if t.text.contains("echo: echo"))
        });
        assert!(echoed, "expected echoed content, got {result:?}");

        client.ping().await.expect("ping");

        let health = client.health().await;
        assert_eq!(health.state, ConnectionState::Connected);
        assert_eq!(health.transport, "test-in-process");
        assert!(health.last_error.is_none());
    }

    #[tokio::test]
    async fn call_tool_times_out() {
        let connector = InProcessConnector::slow(Duration::from_secs(3));
        let mut options = fast_options("slow");
        options.request_timeout = Duration::from_millis(150);
        let client = client(connector, options);

        let error = client
            .call_tool(echo_call(), CancellationToken::new())
            .await
            .expect_err("expected timeout");
        assert!(matches!(error, McpError::Timeout(_)), "got {error:?}");
    }

    #[tokio::test]
    async fn handshake_is_bounded_by_request_timeout() {
        let connector = InProcessConnector::delayed(Duration::from_secs(5));
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
        let connector = InProcessConnector::slow(Duration::from_secs(10));
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
            .call_tool(echo_call(), token)
            .await
            .expect_err("expected cancellation");
        assert!(matches!(error, McpError::Cancelled), "got {error:?}");
    }

    #[tokio::test]
    async fn call_tool_cancels_during_handshake() {
        let connector = InProcessConnector::delayed(Duration::from_secs(10));
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
            .call_tool(echo_call(), token)
            .await
            .expect_err("expected handshake cancellation");
        assert!(matches!(error, McpError::Cancelled), "got {error:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn health_remains_responsive_while_connecting() {
        let connector = InProcessConnector::delayed(Duration::from_secs(10));
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
        let connector = InProcessConnector::failing(2, ServerBehavior::Echo);
        let client = client(connector, fast_options("retry"));

        let tools = client.list_tools().await.expect("list_tools after retries");
        assert_eq!(tools.len(), 1);

        let health = client.health().await;
        assert_eq!(health.state, ConnectionState::Connected);
        assert_eq!(health.restart_attempts, 0, "counter resets after success");
    }

    #[tokio::test]
    async fn restart_budget_exhaustion_is_isolated() {
        let connector = InProcessConnector::failing(10, ServerBehavior::Echo);
        let mut options = fast_options("dead");
        options.restart = RestartPolicy {
            max_attempts: 2,
            base_delay_ms: 1,
            max_delay_ms: 5,
        };
        let client = client(connector, options);

        let error = client.list_tools().await.expect_err("expected failure");
        assert!(matches!(error, McpError::Disconnected(_)), "got {error:?}");

        let health = client.health().await;
        assert_eq!(health.state, ConnectionState::Failed);
        assert_eq!(health.restart_attempts, 2);
        assert_eq!(health.max_restart_attempts, 2);
        assert!(health.last_error.is_some());
    }

    #[tokio::test]
    async fn shutdown_blocks_further_use() {
        let connector = InProcessConnector::echo();
        let client = client(connector, fast_options("shutdown"));

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
        assert_eq!(backoff(base, max, 4), max);
    }
}
