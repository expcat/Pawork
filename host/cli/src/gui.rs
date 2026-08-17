//! pawork gui serve：拉起本机 GUI 协议服务器（S7 波 A 最小切片）。
//!
//! 单实例语义：bind 前先向目标 socket 发起一次探测连接；能连上说明已有
//! serve 进程在监听（Unix bind 会清理 stale socket 文件，探测是唯一的
//! 在线判定）。Ctrl-C 关闭监听并退出；关闭不取消已进入 Core 的 Run
//! （进程内 Run 随进程结束，跨进程存活语义归 S10 service）。

use std::sync::Arc;

use pawork_app::{AppCore, GuiApprovalHost, GuiHostAdapter};
use pawork_gui_server::{GuiHost, GuiServer, GuiServerConfig};
use pawork_protocol::{GuiCapability, HandshakeService, SUPPORTED_API_VERSIONS};
use pawork_transport::{
    ConnectOptions, GuiTransportClient, GuiTransportServer, LocalTransport, TransportEndpoint,
};

use crate::{CliError, GuiCommand};

pub async fn run_gui(core: AppCore, command: GuiCommand) -> Result<(), CliError> {
    let GuiCommand::Serve { socket } = command;
    let approvals = Arc::new(GuiApprovalHost::new());
    let mut core = core;
    core.configure_approval(core.approval_mode(), core.workspace_trusted(), approvals.clone());
    let core = Arc::new(tokio::sync::RwLock::new(core));
    let adapter = GuiHostAdapter::from_locked(Arc::clone(&core), approvals);
    let socket_path = match socket.or_else(default_socket_dir) {
        Some(dir) => dir.join("pawork-gui.sock"),
        None => std::env::temp_dir().join("pawork-gui.sock"),
    };
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let address = socket_path.to_string_lossy().to_string();

    ensure_single_instance(&address).await?;

    let transport = Arc::new(LocalTransport::default());
    let handshake = HandshakeService::new(
        adapter.instance_id(),
        SUPPORTED_API_VERSIONS.to_vec(),
        vec![
            GuiCapability::Events,
            GuiCapability::Snapshots,
            GuiCapability::Approvals,
        ],
    );
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
    eprintln!("pawork gui serving on {}", socket_path.display());

    // 保留连接句柄：SessionHandle 被丢弃会关闭 oneshot 并结束该连接任务，
    // 导致客户端握手收到 Broken pipe。断线时由连接任务侧关闭并在此清理。
    let mut connections: Vec<Box<dyn pawork_transport::GuiConnection>> = Vec::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok(handle) => {
                        connections.retain(|connection| connection.info().connection_id != handle.info().connection_id);
                        connections.push(handle);
                    }
                    Err(error) => {
                        eprintln!("gui accept failed: {error}");
                        break;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("shutting down gui server");
                let _ = listener.close().await;
                break;
            }
        }
    }
    drop(connections);
    if let Ok(core) = Arc::try_unwrap(core) {
        core.into_inner().shutdown().await?;
    }
    Ok(())
}

fn default_socket_dir() -> Option<std::path::PathBuf> {
    Some(pawork_app::default_data_dir())
}

async fn ensure_single_instance(address: &str) -> Result<(), CliError> {
    let client = LocalTransport::default();
    let options = ConnectOptions {
        timeout_ms: 300,
        client_label: Some("pawork-gui-serve-probe".into()),
        max_frame_bytes: 1024 * 1024,
    };
    match client
        .connect(
            TransportEndpoint::Local {
                address: address.into(),
            },
            options,
        )
        .await
    {
        Ok(_) => Err(CliError::Usage(format!(
            "another pawork gui serve is already listening on {address}"
        ))),
        Err(_) => Ok(()),
    }
}
