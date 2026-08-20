//! pawork gui serve：拉起本机 GUI 协议服务器（S7 波 A 最小切片）。
//!
//! 单实例语义：bind 前先向目标 socket 发起一次探测连接；能连上说明已有
//! serve 进程在监听（Unix bind 会清理 stale socket 文件，探测是唯一的
//! 在线判定）。Ctrl-C 关闭监听并退出；关闭不取消已进入 Core 的 Run
//! （进程内 Run 随进程结束，跨进程存活语义归 S10 service）。

use std::sync::Arc;

use pawork_app::{AppCore, GuiApprovalHost, GuiHostAdapter};
use pawork_app::gui_server::{GuiHost, GuiServer, GuiServerConfig};
use pawork_protocol::client_auth::{TokenAuthenticator, TokenStore};
use pawork_protocol::app::registry::gui_supported_capabilities;
use pawork_protocol::{HandshakeService, SUPPORTED_API_VERSIONS};
use pawork_transport::{
    ConnectOptions, GuiTransportClient, GuiTransportServer, LocalTransport, TransportEndpoint,
};

use crate::ops::{gui_pid_path, gui_socket_path, gui_token_path, remove_pid_file, write_pid_file};
use crate::{CliError, GuiCommand};

pub async fn run_gui(core: AppCore, command: GuiCommand, instance: &str) -> Result<(), CliError> {
    let GuiCommand::Serve { socket } = command;
    let approvals = Arc::new(GuiApprovalHost::new());
    let mut core = core;
    core.configure_approval(core.approval_mode(), core.workspace_trusted(), approvals.clone());
    let core = Arc::new(tokio::sync::RwLock::new(core));
    let adapter = GuiHostAdapter::from_locked(Arc::clone(&core), approvals);
    let pty = adapter.pty();
    let data_dir = pawork_app::default_data_dir();
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

    ensure_single_instance(&address).await?;

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

    write_pid_file(&pid_path)?;

    let transport = Arc::new(LocalTransport::default());
    let handshake = HandshakeService::new(
        adapter.instance_id(),
        SUPPORTED_API_VERSIONS.to_vec(),
        gui_supported_capabilities(),
    )
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
    eprintln!(
        "pawork gui serving on {} (instance {instance})",
        socket_path.display()
    );

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
    remove_pid_file(&pid_path);
    let _ = pty.shutdown().await;
    if let Ok(core) = Arc::try_unwrap(core) {
        core.into_inner().shutdown().await?;
    }
    Ok(())
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
