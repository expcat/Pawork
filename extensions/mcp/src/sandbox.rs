//! Sandbox Runtime → Process Runtime hosted MCP stdio spawn.
//!
//! Local MCP stdio servers are always hosted through this path: spawn goes
//! through [`pawork_exec::SandboxBackend::spawn_interactive`]. Crash restart
//! reuses the same [`StdioSpawner`] so the sandbox guarantee does not degrade.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::task::Poll;

use pawork_domain::CancellationToken;
use async_trait::async_trait;
use pawork_exec::{
    CancellationToken as ExecCancellationToken, CommandSpec, ProcessError, ProcessEvent,
    ProcessHandle, ProcessInput, SandboxBackend, SandboxPolicy, SandboxProcessSpec,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

use crate::transport::StdioTransportConfig;
use crate::McpError;

/// Combined stdout + stderr output budget for one stdio connection.
const STDIO_MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// Slack so the process-runtime truncation point stays beyond the reader budget.
const STDIO_CHUNK_SLACK_BYTES: u64 = 2 * 8192;

/// Split transport from one sandboxed spawn: read (stdout) + write (stdin).
pub struct SpawnedStdio {
    pub read: SandboxStdoutReader,
    pub write: SandboxStdinWriter,
}

/// Injected stdio spawner: the only entry that may start or restart a package MCP stdio server.
#[async_trait]
pub trait StdioSpawner: Send + Sync {
    async fn spawn(&self, cfg: &StdioTransportConfig) -> Result<SpawnedStdio, McpError>;
}

/// Production stdio spawner: Sandbox Runtime → Process Runtime only.
#[derive(Clone)]
pub struct SandboxedStdioSpawner {
    sandbox: Arc<dyn SandboxBackend>,
    policy: SandboxPolicy,
    workspace_roots: Vec<PathBuf>,
}

impl SandboxedStdioSpawner {
    pub fn new(
        sandbox: Arc<dyn SandboxBackend>,
        policy: SandboxPolicy,
        workspace_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            sandbox,
            policy,
            workspace_roots,
        }
    }
}

fn bridge_exec_cancel(
    domain: &CancellationToken,
) -> (
    ExecCancellationToken,
    Option<tokio::task::JoinHandle<()>>,
) {
    let exec = ExecCancellationToken::new();
    if domain.is_cancelled() {
        exec.cancel();
        return (exec, None);
    }
    let domain = domain.clone();
    let exec_for_wait = exec.clone();
    let handle = tokio::spawn(async move {
        domain.cancelled().await;
        exec_for_wait.cancel();
    });
    (exec, Some(handle))
}

#[async_trait]
impl StdioSpawner for SandboxedStdioSpawner {
    async fn spawn(&self, cfg: &StdioTransportConfig) -> Result<SpawnedStdio, McpError> {
        if cfg.command.trim().is_empty() {
            return Err(McpError::Config("stdio command must not be empty".into()));
        }
        let cwd = cfg
            .working_dir
            .clone()
            .or_else(|| self.workspace_roots.first().cloned())
            .ok_or_else(|| {
                McpError::Config(
                    "sandboxed stdio spawn requires a trusted workspace root or working dir".into(),
                )
            })?;
        let mut command = CommandSpec::new(cfg.command.clone());
        command.args = cfg.args.clone();
        command.cwd = Some(cwd);
        command.env = cfg
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        command.max_output_bytes = STDIO_MAX_OUTPUT_BYTES + STDIO_CHUNK_SLACK_BYTES;

        let domain_cancel = CancellationToken::new();
        let (exec_cancel, bridge) = bridge_exec_cancel(&domain_cancel);
        let process = self
            .sandbox
            .spawn_interactive(
                SandboxProcessSpec {
                    command,
                    workspace_roots: self.workspace_roots.clone(),
                },
                self.policy.clone(),
                exec_cancel,
            )
            .await
            .map_err(|error| {
                McpError::Transport(format!("sandboxed stdio spawn failed: {error}"))
            })?;
        let (events, input, handle) = process.into_parts();
        Ok(SpawnedStdio {
            read: SandboxStdoutReader::new(
                events,
                handle,
                STDIO_MAX_OUTPUT_BYTES,
                domain_cancel,
                bridge,
            ),
            write: SandboxStdinWriter::new(input),
        })
    }
}

/// stdout → client AsyncRead adapter.
pub struct SandboxStdoutReader {
    events: mpsc::Receiver<ProcessEvent>,
    buf: Vec<u8>,
    eof: bool,
    failed: bool,
    budget_remaining: u64,
    _handle: ProcessHandle,
    _domain_cancel: CancellationToken,
    _bridge: Option<tokio::task::JoinHandle<()>>,
}

impl SandboxStdoutReader {
    pub(crate) fn new(
        events: mpsc::Receiver<ProcessEvent>,
        handle: ProcessHandle,
        budget: u64,
        domain_cancel: CancellationToken,
        bridge: Option<tokio::task::JoinHandle<()>>,
    ) -> Self {
        Self {
            events,
            buf: Vec::new(),
            eof: false,
            failed: false,
            budget_remaining: budget,
            _handle: handle,
            _domain_cancel: domain_cancel,
            _bridge: bridge,
        }
    }
}

impl AsyncRead for SandboxStdoutReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.failed {
            return Poll::Ready(Err(budget_exceeded_error()));
        }
        if self.eof && self.buf.is_empty() {
            return Poll::Ready(Ok(()));
        }
        if !self.buf.is_empty() {
            let dst = buf.initialize_unfilled();
            let n = dst.len().min(self.buf.len());
            dst[..n].copy_from_slice(&self.buf[..n]);
            self.buf.drain(..n);
            buf.advance(n);
            return Poll::Ready(Ok(()));
        }
        loop {
            match self.events.poll_recv(cx) {
                Poll::Ready(Some(ProcessEvent::Stdout(bytes))) => {
                    let length = bytes.len() as u64;
                    if length >= self.budget_remaining {
                        self.failed = true;
                        return Poll::Ready(Err(budget_exceeded_error()));
                    }
                    self.budget_remaining -= length;
                    let dst = buf.initialize_unfilled();
                    let n = dst.len().min(bytes.len());
                    dst[..n].copy_from_slice(&bytes[..n]);
                    if n < bytes.len() {
                        self.buf.extend_from_slice(&bytes[n..]);
                    }
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Some(ProcessEvent::Stderr(bytes))) => {
                    self.budget_remaining =
                        self.budget_remaining.saturating_sub(bytes.len() as u64);
                    continue;
                }
                Poll::Ready(Some(ProcessEvent::Exit {
                    truncated: true, ..
                })) => {
                    self.failed = true;
                    return Poll::Ready(Err(budget_exceeded_error()));
                }
                Poll::Ready(Some(ProcessEvent::Exit {
                    truncated: false, ..
                }))
                | Poll::Ready(None) => {
                    self.eof = true;
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn budget_exceeded_error() -> std::io::Error {
    std::io::Error::other("MCP stdio output budget exceeded")
}

/// client → stdin AsyncWrite adapter.
pub struct SandboxStdinWriter {
    input: ProcessInput,
    pending:
        Option<std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>>,
}

impl SandboxStdinWriter {
    pub(crate) fn new(input: ProcessInput) -> Self {
        Self {
            input,
            pending: None,
        }
    }
}

impl AsyncWrite for SandboxStdinWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(mut fut) = self.pending.take() {
            match fut.as_mut().poll(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => {
                    self.pending = Some(fut);
                    return Poll::Pending;
                }
            }
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let bytes = Vec::from(buf);
        let written = bytes.len();
        let input = self.input.clone();
        let mut fut =
            Box::pin(async move { input.write_all(&bytes).await.map_err(process_error_to_io) });
        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(written)),
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => {
                self.pending = Some(fut);
                Poll::Pending
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn process_error_to_io(error: ProcessError) -> std::io::Error {
    match error {
        ProcessError::Io(io) => io,
        other => std::io::Error::other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_exec::{FilesystemPolicy, NativeRestricted, NetworkMode};

    fn policy(root: &std::path::Path) -> SandboxPolicy {
        SandboxPolicy {
            filesystem: FilesystemPolicy {
                read_roots: vec![root.to_path_buf()],
                write_roots: Vec::new(),
                deny: Vec::new(),
            },
            network_mode: NetworkMode::Off,
            allow_spawn: true,
            ..SandboxPolicy::default()
        }
    }

    #[cfg(unix)]
    async fn read_until_terminal(
        reader: &mut SandboxStdoutReader,
    ) -> (Vec<u8>, Option<std::io::Error>) {
        use tokio::io::AsyncReadExt;
        let mut got = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(10), reader.read(&mut chunk))
                .await
            {
                Err(_) => return (got, Some(std::io::Error::other("read timed out"))),
                Ok(Ok(0)) => return (got, None),
                Ok(Ok(n)) => got.extend_from_slice(&chunk[..n]),
                Ok(Err(error)) => return (got, Some(error)),
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_spawner_round_trips_stdio_through_sandbox() {
        let root = std::env::temp_dir();
        let spawner = SandboxedStdioSpawner::new(
            Arc::new(NativeRestricted::new()),
            policy(&root),
            vec![root],
        );
        let cfg = StdioTransportConfig::new("sh")
            .with_args(["-c", "cat"])
            .with_working_dir(std::env::temp_dir());
        let spawned = match spawner.spawn(&cfg).await {
            Ok(value) => value,
            Err(error) => panic!("spawn failed: {error}"),
        };
        let mut writer = spawned.write;
        let mut reader = spawned.read;

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        writer.write_all(b"hello-stdio\n").await.expect("write");
        writer.flush().await.expect("flush");

        let mut got = Vec::new();
        let mut chunk = [0u8; 32];
        for _ in 0..50 {
            let n = tokio::time::timeout(
                std::time::Duration::from_millis(200),
                reader.read(&mut chunk),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(0);
            if n > 0 {
                got.extend_from_slice(&chunk[..n]);
                if got.len() >= b"hello-stdio\n".len() {
                    break;
                }
            }
        }
        assert!(got.starts_with(b"hello-stdio"), "got = {got:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdout_output_budget_is_bounded_without_partial_frames() {
        let root = std::env::temp_dir();
        let spawner = SandboxedStdioSpawner::new(
            Arc::new(NativeRestricted::new()),
            policy(&root),
            vec![root],
        );
        let cfg = StdioTransportConfig::new("sh")
            .with_args([
                "-c",
                "dd if=/dev/zero bs=1m count=9 2>/dev/null | tr '\\0' x",
            ])
            .with_working_dir(std::env::temp_dir());
        let mut spawned = spawner.spawn(&cfg).await.expect("spawn");

        let (got, terminal) = read_until_terminal(&mut spawned.read).await;
        let error = terminal.expect("budget-exceeded output must surface as an I/O error");
        assert!(
            error.to_string().contains("budget exceeded"),
            "unexpected error: {error}"
        );
        assert!(
            (got.len() as u64) < STDIO_MAX_OUTPUT_BYTES,
            "delivered {} bytes, expected strictly below budget",
            got.len()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stderr_only_output_hits_budget_and_fails_cleanly() {
        let root = std::env::temp_dir();
        let spawner = SandboxedStdioSpawner::new(
            Arc::new(NativeRestricted::new()),
            policy(&root),
            vec![root],
        );
        let cfg = StdioTransportConfig::new("sh")
            .with_args(["-c", "dd if=/dev/zero bs=1m count=9 1>&2"])
            .with_working_dir(std::env::temp_dir());
        let mut spawned = spawner.spawn(&cfg).await.expect("spawn");

        let (got, terminal) = read_until_terminal(&mut spawned.read).await;
        let error = terminal.expect("truncated output must surface as an I/O error");
        assert!(
            error.to_string().contains("budget exceeded"),
            "unexpected error: {error}"
        );
        assert!(
            got.is_empty(),
            "stderr must not leak into the framing stream"
        );
    }

    #[tokio::test]
    async fn sandboxed_spawner_requires_trusted_root() {
        let spawner = SandboxedStdioSpawner::new(
            Arc::new(NativeRestricted::new()),
            SandboxPolicy::default(),
            Vec::new(),
        );
        let cfg = StdioTransportConfig::new("unused");
        let err = match spawner.spawn(&cfg).await {
            Ok(_) => panic!("expected spawn to fail without a trusted workspace root"),
            Err(error) => error,
        };
        assert!(err.to_string().contains("trusted workspace root"));
    }

    #[tokio::test]
    async fn sandboxed_spawner_rejects_empty_command() {
        let root = std::env::temp_dir();
        let spawner = SandboxedStdioSpawner::new(
            Arc::new(NativeRestricted::new()),
            policy(&root),
            vec![root],
        );
        let cfg = StdioTransportConfig::new("  ");
        let err = match spawner.spawn(&cfg).await {
            Ok(_) => panic!("expected spawn to fail on an empty command"),
            Err(error) => error,
        };
        assert!(err.to_string().contains("empty"));
    }
}
