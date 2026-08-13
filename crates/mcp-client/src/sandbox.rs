//! Sandbox Runtime → Process Runtime 托管的 MCP stdio spawn（P17-2）。
//!
//! Plugin Package 触达的本地 MCP stdio server 一律经此路径托管：spawn 走
//! [`sandbox_runtime::SandboxBackend::spawn_interactive`]（内部委托
//! [`process_runtime::ProcessRuntime`]），stdin/stdout 经 async_rw 适配器桥接到 rmcp
//! transport。崩溃 restart 由 `ManagedMcpClient` 复用**同一个** connector（从而复用同一
//! 个 [`StdioSpawner`]），保证 restart 阶段不降级为 unsandboxed spawn。
//!
//! 设计与 `lsp-runtime` 的 `ServerSpawner` 平行：本 crate 不复制进程树清理 / sandbox
//! policy，统一由 Sandbox / Process Runtime 承担。

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::task::Poll;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use process_runtime::{ProcessError, ProcessEvent, ProcessHandle, ProcessInput};
use sandbox_runtime::{SandboxBackend, SandboxPolicy, SandboxProcessSpec};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

use crate::McpError;
use crate::transport::StdioTransportConfig;

/// stdio 通道（stdout + stderr 合计）的有界输出预算：单连接硬上限。
///
/// 超出预算的 server 被判定为失控，连接以明确错误失败（不投递截断半帧）；
/// 崩溃 restart 复用同一 spawner 重新 spawn，预算在每次连接上重新生效。
const STDIO_MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// process-runtime 截断点相对读者预算的余量：两个流各至多一个 8 KiB 块在途
/// （process-runtime 每流以 8 KiB 块读取），加上该余量后，process-runtime 的
/// 截断点恒在读者预算之外——读者拒绝越过预算的块时，该块必然已被完整接收，
/// 不会把截断的半帧当作完整数据投递（framing 保持）。
const STDIO_CHUNK_SLACK_BYTES: u64 = 2 * 8192;

/// 一次 sandboxed spawn 的拆分传输：read（stdout）+ write（stdin）。
///
/// 进程树生命周期由 reader 内部持有的 [`ProcessHandle`] 守卫：reader 与 transport 同
/// 生命周期，drop 时取消 kill token 并终止整棵进程树。
pub struct SpawnedStdio {
    pub read: SandboxStdoutReader,
    pub write: SandboxStdinWriter,
}

/// 注入式 stdio spawner：唯一允许启动 / 重启 package MCP stdio server 的入口。
///
/// 实现必须经 Sandbox Runtime → Process Runtime 路径 spawn。restart 复用同一 spawner，
/// 保证 sandbox guarantee 在 restart 阶段不降级（见 acceptance）。
#[async_trait]
pub trait StdioSpawner: Send + Sync {
    /// 按 stdio transport 配置 spawn 一个受控子进程，返回拆分传输。
    async fn spawn(&self, cfg: &StdioTransportConfig) -> Result<SpawnedStdio, McpError>;
}

/// 生产 stdio spawner：唯一实现路径为 Sandbox Runtime → Process Runtime。
///
/// 每次调用都会重新 spawn（包括 crash restart），因此不会降级到 unsandboxed spawn。
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
        let mut command = process_runtime::CommandSpec::new(cfg.command.clone());
        command.args = cfg.args.clone();
        command.cwd = Some(cwd);
        command.env = cfg
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        // MCP stdio 是长连接协议流，整个通道受有界输出预算约束（见
        // STDIO_MAX_OUTPUT_BYTES）；截断点带在途块余量，读者侧据此保持 framing。
        command.max_output_bytes = STDIO_MAX_OUTPUT_BYTES + STDIO_CHUNK_SLACK_BYTES;

        let cancel = CancellationToken::new();
        let process = self
            .sandbox
            .spawn_interactive(
                SandboxProcessSpec {
                    command,
                    workspace_roots: self.workspace_roots.clone(),
                },
                self.policy.clone(),
                cancel,
            )
            .await
            .map_err(|error| {
                McpError::Transport(format!("sandboxed stdio spawn failed: {error}"))
            })?;
        let (events, input, handle) = process.into_parts();
        Ok(SpawnedStdio {
            read: SandboxStdoutReader::new(events, handle, STDIO_MAX_OUTPUT_BYTES),
            write: SandboxStdinWriter::new(input),
        })
    }
}

/// stdout → client 的 AsyncRead 适配器。
///
/// 把 [`ProcessEvent::Stdout`] 字节按到达顺序暴露给 rmcp；`Stderr` 静默丢弃（不混入
/// JSON-RPC framing，也不记录原始内容以防 secret 泄漏）；`Exit` / 通道关闭即 EOF。
///
/// 输出受有界预算约束且保持 framing：累计（stdout + stderr）超过预算时，越过
/// 预算的 stdout 块被整体拒绝并返回明确错误，绝不投递截断的半帧；`Exit` 携带
/// `truncated` 时同样以错误收尾（流可能在任意字节处被截断）。
pub struct SandboxStdoutReader {
    events: mpsc::Receiver<ProcessEvent>,
    buf: Vec<u8>,
    eof: bool,
    failed: bool,
    /// 剩余输出预算；stdout 投递与 stderr 丢弃均消费（与 process-runtime 的
    /// 共享预算镜像，避免 stderr 静默消耗预算后把截断的 stdout 块当完整块）。
    budget_remaining: u64,
    // 进程树守卫：与 transport 同生命周期，drop 终止整棵进程树。
    _handle: ProcessHandle,
}

impl SandboxStdoutReader {
    pub(crate) fn new(
        events: mpsc::Receiver<ProcessEvent>,
        handle: ProcessHandle,
        budget: u64,
    ) -> Self {
        Self {
            events,
            buf: Vec::new(),
            eof: false,
            failed: false,
            budget_remaining: budget,
            _handle: handle,
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
                        // 越过有界预算：该块可能已被 process-runtime 截断（中帧），
                        // 整体拒绝、绝不投递半帧；连接以明确错误失败。
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
                    // 静默丢弃 stderr，但消费共享输出预算，继续等待 stdout / exit。
                    self.budget_remaining =
                        self.budget_remaining.saturating_sub(bytes.len() as u64);
                    continue;
                }
                Poll::Ready(Some(ProcessEvent::Exit {
                    truncated: true, ..
                })) => {
                    // 流在任意字节处被截断：无法保证 framing 完整，以错误收尾。
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

/// client → stdin 的 AsyncWrite 适配器。
///
/// [`ProcessInput::write_all`] 是 async，这里用 owned-bytes future 在 `poll_write` 内驱动，
/// 避免 self-referential 借用。
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
        // 先推进未完成的写入。
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
        // ProcessInput::write_all 内部已 flush；这里无需额外动作。
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        // 进程树生命周期由 ProcessHandle 守卫；shutdown 不强制关 stdin。
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
    use sandbox_runtime::{FilesystemPolicy, NativeRestricted, NetworkMode};

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

    /// 读取直到错误 / EOF / 超时，返回已收字节与终端错误。
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
        // 用 `sh -c cat` 做回环：写入的字节经 sandbox 进程 echo 回 stdout。
        // 这证明 spawn 走 Sandbox → Process Runtime，且 async_rw 适配器双向可用。
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
        // 读取直到拿到写回的字节（cat 回环）。
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
        // 9 MiB 输出超过 8 MiB 预算：读取必须以预算错误收尾，且不投递越过预算
        // 的块（可能为截断半帧）——已收字节严格小于预算。
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
        // 全部输出走 stderr（9 MiB > 预算）：stdout 无内容，通道预算耗尽后以
        // 明确错误收尾（Exit truncated），不产生静默 EOF。
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
