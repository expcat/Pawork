//! 注入式传输契约：lsp-runtime 通过这里定义的 trait 访问语言服务进程，
//! 不自行 `tokio::process::Command` / 直接 spawn / 自建进程树清理。
//!
//! 生产实现（app-service 等宿主）把 [`ServerSpawner`] 桥接到
//! sandbox-runtime → process-runtime 的统一 spawn 路径；崩溃 restart 仍走同一路径，
//! 进程树清理统一由 Sandbox / Process Runtime 承担。

use std::path::PathBuf;
use std::sync::Arc;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use process_runtime::{CommandSpec, ProcessEvent, ProcessHandle, ProcessInput};
use sandbox_runtime::{SandboxBackend, SandboxPolicy, SandboxProcessSpec};

use crate::descriptor::{LanguageServerDescriptor, LspTransport};
use crate::error::LspError;

/// spawn 时由调用方提供的工作区 / 资源约束（不含 sandbox 策略本身）。
#[derive(Debug, Clone, Default)]
pub struct ServerSpawnConfig {
    pub workspace_roots: Vec<PathBuf>,
    pub max_output_bytes: Option<u64>,
}

/// 语言服务的只读半边（stdout → client）。
#[async_trait]
pub trait ServerReader: Send {
    /// 读下一段字节；`Ok(None)` 表示对端干净 EOF（进程结束）。
    async fn read(&mut self) -> Result<Option<Vec<u8>>, LspError>;
}

/// 语言服务的只写半边（client → stdin）。
#[async_trait]
pub trait ServerWriter: Send {
    /// 写一帧（已含 `Content-Length` header）。
    async fn write(&mut self, bytes: &[u8]) -> Result<(), LspError>;
}

/// 生命周期守卫：drop / close 时由生产实现经 Sandbox/Process Runtime 终止整棵进程树。
#[async_trait]
pub trait ServerLifecycle: Send {
    async fn close(&mut self) -> Result<(), LspError>;
}

/// 一次 spawn 产出的拆分传输：reader / writer / lifecycle 三者独立，
/// 因此读循环与写路径不会因单一互斥锁互相阻塞。
pub struct SpawnedServer {
    pub reader: Box<dyn ServerReader>,
    pub writer: Box<dyn ServerWriter>,
    pub lifecycle: Box<dyn ServerLifecycle>,
}

/// 语言服务进程的注入式 spawner：唯一允许启动 / 重启子进程的入口。
///
/// 实现必须经 Sandbox Runtime → Process Runtime 路径 spawn；禁止在 lsp-runtime 内部
/// 绕过。restart 复用同一 spawner，保证 sandbox guarantee 在 restart 阶段不降级。
#[async_trait]
pub trait ServerSpawner: Send + Sync {
    async fn spawn(
        &self,
        descriptor: &LanguageServerDescriptor,
        config: &ServerSpawnConfig,
        cancel: CancellationToken,
    ) -> Result<SpawnedServer, LspError>;
}

/// 可 clone 的共享 spawner 句柄，便于在 restart 任务里持有。
pub type SharedSpawner = std::sync::Arc<dyn ServerSpawner + Send + Sync>;

/// 生产语言服务 spawner：唯一实现路径为 Sandbox Runtime → Process Runtime。
///
/// 调用方必须用可信 Workspace 服务解析出的绝对 roots 构造 [`ServerSpawnConfig`]；
/// descriptor 自身不能借 `file://` URI 绕过 workspace 边界。每次 crash restart 都会
/// 重新调用同一个实例，因此不会降级到 unsandboxed spawn。
#[derive(Clone)]
pub struct SandboxServerSpawner {
    sandbox: Arc<dyn SandboxBackend>,
    policy: SandboxPolicy,
}

impl SandboxServerSpawner {
    pub fn new(sandbox: Arc<dyn SandboxBackend>, policy: SandboxPolicy) -> Self {
        Self { sandbox, policy }
    }
}

struct SandboxReader {
    events: tokio::sync::mpsc::Receiver<ProcessEvent>,
}

#[async_trait]
impl ServerReader for SandboxReader {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, LspError> {
        while let Some(event) = self.events.recv().await {
            match event {
                ProcessEvent::Stdout(bytes) => return Ok(Some(bytes)),
                // stderr 是语言服务诊断通道，绝不能混入 JSON-RPC stdout framing；
                // 也不在这里记录原始内容，避免配置/路径/Secret 泄漏日志。
                ProcessEvent::Stderr(_) => {}
                ProcessEvent::Exit {
                    truncated: true, ..
                } => {
                    return Err(LspError::Transport(
                        "language server output exceeded its configured bound".into(),
                    ));
                }
                ProcessEvent::Exit {
                    truncated: false, ..
                } => return Ok(None),
            }
        }
        Ok(None)
    }
}

struct SandboxWriter {
    input: ProcessInput,
}

#[async_trait]
impl ServerWriter for SandboxWriter {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), LspError> {
        self.input
            .write_all(bytes)
            .await
            .map_err(|error| LspError::Transport(error.to_string()))
    }
}

struct SandboxLifecycle {
    handle: ProcessHandle,
}

#[async_trait]
impl ServerLifecycle for SandboxLifecycle {
    async fn close(&mut self) -> Result<(), LspError> {
        self.handle
            .kill()
            .await
            .map_err(|error| LspError::Transport(error.to_string()))
    }
}

#[async_trait]
impl ServerSpawner for SandboxServerSpawner {
    async fn spawn(
        &self,
        descriptor: &LanguageServerDescriptor,
        config: &ServerSpawnConfig,
        cancel: CancellationToken,
    ) -> Result<SpawnedServer, LspError> {
        if descriptor.transport != LspTransport::Stdio {
            return Err(LspError::Spawn(
                "socket LSP transport is reserved but not implemented".into(),
            ));
        }
        let cwd = config.workspace_roots.first().cloned().ok_or_else(|| {
            LspError::Spawn("language server requires a trusted workspace root".into())
        })?;
        let mut command =
            CommandSpec::new(descriptor.command.clone()).args(descriptor.args.clone());
        command.cwd = Some(cwd);
        command.env = descriptor.env.clone();
        // Process Runtime 的上限是进程全生命周期累计值。未显式配置时保持协议流可用，
        // 但读取通道仍为有界 channel；显式配置则在超限时 fail closed 并触发重启。
        command.max_output_bytes = config.max_output_bytes.unwrap_or(u64::MAX);
        let process = self
            .sandbox
            .spawn_interactive(
                SandboxProcessSpec {
                    command,
                    workspace_roots: config.workspace_roots.clone(),
                },
                self.policy.clone(),
                cancel,
            )
            .await
            .map_err(|error| LspError::Spawn(error.to_string()))?;
        let (events, input, handle) = process.into_parts();
        Ok(SpawnedServer {
            reader: Box::new(SandboxReader { events }),
            writer: Box::new(SandboxWriter { input }),
            lifecycle: Box::new(SandboxLifecycle { handle }),
        })
    }
}

#[cfg(test)]
mod production_tests {
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

    #[cfg(unix)]
    #[tokio::test]
    async fn production_spawner_round_trips_stdio_through_sandbox() {
        let root = std::env::temp_dir();
        let spawner = SandboxServerSpawner::new(Arc::new(NativeRestricted::new()), policy(&root));
        let descriptor =
            LanguageServerDescriptor::new("echo-lsp", "sh", "test").with_args(["-c", "cat"]);
        let mut server = spawner
            .spawn(
                &descriptor,
                &ServerSpawnConfig {
                    workspace_roots: vec![root],
                    max_output_bytes: Some(1024),
                },
                CancellationToken::new(),
            )
            .await
            .expect("spawn through sandbox");

        server
            .writer
            .write(b"Content-Length: 2\r\n\r\n{}")
            .await
            .unwrap();
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(2), server.reader.read())
            .await
            .expect("read timeout")
            .expect("read")
            .expect("stdout");
        assert_eq!(bytes, b"Content-Length: 2\r\n\r\n{}");
        server.lifecycle.close().await.unwrap();
    }

    #[tokio::test]
    async fn production_spawner_requires_trusted_workspace_root() {
        let spawner =
            SandboxServerSpawner::new(Arc::new(NativeRestricted::new()), SandboxPolicy::default());
        let error = match spawner
            .spawn(
                &LanguageServerDescriptor::new("missing-root", "unused", "test"),
                &ServerSpawnConfig::default(),
                CancellationToken::new(),
            )
            .await
        {
            Ok(_) => panic!("spawn must fail without a trusted workspace root"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("trusted workspace root"));
    }
}
