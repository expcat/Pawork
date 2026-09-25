//! pawork gui serve：拉起本机 GUI 协议服务器（S7 波 A 最小切片）。
//!
//! 单实例语义（R-04）：实例级 `gui.lock` 独占锁在装配期（open_store 与
//! bind 之前）由 AppCore 按 InstanceRole::GuiHost 获取并持有整个生命
//! 周期——不存在「探测-绑定」竞态；崩溃遗留的 socket 文件由 bind 侧的
//! stale 清理兜底。PID 文件在 bind 成功后发布。Ctrl-C 关闭监听并退出；
//! 客户端断线不取消 Run；Host 退出则取消并等待 Run 持久收尾后关闭 Core。

use std::path::PathBuf;
use std::sync::Arc;

use pawork_app::gui_server::{GuiHost, GuiServer, GuiServerConfig};
use pawork_app::{AppCore, GuiApprovalHost, GuiHostAdapter};
use pawork_protocol::app::registry::gui_supported_capabilities;
use pawork_protocol::client_auth::{TokenAuthenticator, TokenStore};
use pawork_protocol::{HandshakeService, SUPPORTED_API_VERSIONS};
use pawork_transport::{GuiTransportServer, LocalTransport, TransportEndpoint};

use crate::ops::{gui_pid_path, gui_socket_path, gui_token_path, remove_pid_file, write_pid_file};
use crate::{CliError, GuiCommand};

/// 活跃连接集合（R-11）：按连接 ID 登记，会话完成即移除。
type ConnectionSetInner =
    std::collections::HashMap<String, Arc<dyn pawork_transport::GuiConnection>>;

#[derive(Clone, Default)]
struct ConnectionSet {
    inner: Arc<tokio::sync::Mutex<ConnectionSetInner>>,
}

impl ConnectionSet {
    async fn drain(&self) -> Vec<Arc<dyn pawork_transport::GuiConnection>> {
        self.inner
            .lock()
            .await
            .drain()
            .map(|(_, connection)| connection)
            .collect()
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }
}

/// 登记连接并挂 reaper：会话完成信号（对端断开 / 握手失败 / close）到达
/// 即从集合移除句柄（R-11，集合随短连接探测自动回落）。
async fn track_connection(
    connections: &ConnectionSet,
    handle: Arc<dyn pawork_transport::GuiConnection>,
) {
    let id = handle.info().connection_id.clone();
    connections
        .inner
        .lock()
        .await
        .insert(id.clone(), Arc::clone(&handle));
    tokio::spawn({
        let connections = connections.clone();
        async move {
            handle.wait_done().await;
            connections.inner.lock().await.remove(&id);
        }
    });
}

pub async fn run_gui(
    core: AppCore,
    command: GuiCommand,
    instance: &str,
    data_dir: PathBuf,
) -> Result<(), CliError> {
    let GuiCommand::Serve { socket } = command;
    let approvals = Arc::new(GuiApprovalHost::new());
    let mut core = core;
    core.set_approval_host(approvals.clone());
    let core = Arc::new(tokio::sync::RwLock::new(core));
    let adapter = GuiHostAdapter::from_locked(Arc::clone(&core), approvals);
    let pty = adapter.pty();
    let runs = adapter.runs();
    let socket_path = socket.unwrap_or_else(|| gui_socket_path(&data_dir, instance));
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        if parent == data_dir.as_path() || parent.starts_with(&data_dir) {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let pid_path = gui_pid_path(&data_dir, instance);
    let address = socket_path.to_string_lossy().to_string();

    let token_path = gui_token_path(&data_dir, instance);
    let store = TokenStore::new(&token_path);
    if token_path.exists() {
        store.load().map_err(|error| {
            CliError::Usage(format!(
                "failed to load gui token {}: {error}",
                token_path.display()
            ))
        })?;
    } else {
        store.generate().map_err(|error| {
            CliError::Usage(format!(
                "failed to generate gui token {}: {error}",
                token_path.display()
            ))
        })?;
    }

    let transport = Arc::new(LocalTransport::default());
    let handshake = HandshakeService::new(
        adapter.instance_id(),
        SUPPORTED_API_VERSIONS.to_vec(),
        gui_supported_capabilities(),
    )
    .with_host_data_dir(data_dir.to_string_lossy().into_owned())
    .with_authenticator(Box::new(TokenAuthenticator::new(store)));
    let server = GuiServer::new(GuiServerConfig {
        host: Arc::new(adapter),
        handshake,
        transport: Arc::clone(&transport) as Arc<dyn GuiTransportServer>,
        connections: None,
    });
    let listener = server
        .bind(TransportEndpoint::Local { address })
        .await
        .map_err(|error| CliError::Usage(error.to_string()))?;
    // R-04：绑定成功后才发布 PID——status/shutdown 读到的 PID 必然对应
    // 已持有端点的进程。
    write_pid_file(&pid_path)?;
    eprintln!(
        "pawork gui serving on {} (instance {instance})",
        socket_path.display()
    );

    // R-11：连接登记为共享集合；每个连接挂 reaper 任务，会话完成信号
    // （对端断开 / 握手失败 / close）到达即移除句柄——状态探测的重复短
    // 连接不再无限累积，集合自动回落。
    let connections = ConnectionSet::default();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok(handle) => {
                        track_connection(&connections, Arc::from(handle)).await;
                    }
                    Err(error) => {
                        eprintln!("gui accept failed: {error}");
                        break;
                    }
                }
            }
            result = &mut shutdown => {
                result?;
                eprintln!("shutting down gui server");
                break;
            }
        }
    }
    // accept 失败与 Ctrl-C 共用关闭路径，持有实例锁直到端点和 PID 清理完成。
    if let Err(error) = listener.close().await {
        tracing::debug!(%error, "gui listener close failed during shutdown");
    }
    runs.begin_shutdown();
    // R-11 有序关闭：listener 已停（或 accept 失败退出循环）→ 关闭并等待
    // 全部会话收口——会话任务持有 Inner/adapter 的 core 引用，不等收口
    // 直接 try_unwrap 必然失败，Core shutdown 会被静默跳过。
    let drained: Vec<Arc<dyn pawork_transport::GuiConnection>> = connections.drain().await;
    for connection in &drained {
        if let Err(error) = connection.close().await {
            tracing::debug!(%error, "gui connection close failed during shutdown");
        }
    }
    for connection in &drained {
        connection.wait_done().await;
    }
    let run_shutdown = runs.shutdown().await;
    // PID 必须在释放锁前删除；实例锁继续随 Core 持有至 shutdown 完成。
    remove_pid_file(&pid_path);
    // 释放 Host 持有者（server 的 Inner 持有 Arc<adapter>），让 core 可独占。
    drop(server);
    drop(listener);
    drop(connections);
    if let Err(error) = pty.shutdown().await {
        tracing::debug!(%error, "pty shutdown failed");
    }
    let core = Arc::try_unwrap(core)
        .map_err(|_| CliError::Usage("GUI shutdown could not release Core holders".into()))?;
    core.into_inner().shutdown().await?;
    run_shutdown.map_err(|error| CliError::Usage(format!("GUI run shutdown failed: {error}")))?;
    Ok(())
}

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_transport::{ConnectionInfo, ConnectionLocality, TransportError, TransportFrame};

    /// 完成信号可控的假连接：test 持有 done_tx，发送后 wait_done 就绪。
    struct FakeConnection {
        info: ConnectionInfo,
        done: tokio::sync::watch::Receiver<bool>,
    }

    fn fake_connection(
        id: &str,
    ) -> (
        Arc<dyn pawork_transport::GuiConnection>,
        tokio::sync::watch::Sender<bool>,
    ) {
        let (done_tx, done) = tokio::sync::watch::channel(false);
        let connection = FakeConnection {
            info: ConnectionInfo {
                connection_id: id.to_string(),
                locality: ConnectionLocality::InProcess,
                peer_label: None,
                encrypted: false,
                max_frame_bytes: 1024,
            },
            done,
        };
        (Arc::new(connection), done_tx)
    }

    #[async_trait::async_trait]
    impl pawork_transport::GuiConnection for FakeConnection {
        async fn send(&self, _frame: TransportFrame) -> Result<(), TransportError> {
            Ok(())
        }

        async fn receive(&self) -> Result<TransportFrame, TransportError> {
            std::future::pending().await
        }

        async fn close(&self) -> Result<(), TransportError> {
            Ok(())
        }

        fn info(&self) -> ConnectionInfo {
            self.info.clone()
        }

        async fn wait_done(&self) {
            let mut done = self.done.clone();
            if !*done.borrow() {
                let _ = done.changed().await;
            }
        }
    }

    #[tokio::test]
    async fn connection_set_shrinks_as_sessions_complete() {
        // R-11：重复短连接（探测）结束后集合自动回落，不无限累积。
        let set = ConnectionSet::default();
        let (a, done_a) = fake_connection("conn-a");
        let (b, done_b) = fake_connection("conn-b");
        let (c, _done_c) = fake_connection("conn-c");
        track_connection(&set, a).await;
        track_connection(&set, b).await;
        track_connection(&set, c).await;
        assert_eq!(set.len().await, 3);

        done_a.send(true).expect("signal a done");
        done_b.send(true).expect("signal b done");
        // reaper 是独立任务：轮询等待移除生效（有界）。
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while set.len().await != 1 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "完成信号到达后集合必须回落"
            );
            tokio::task::yield_now().await;
        }
    }
}
