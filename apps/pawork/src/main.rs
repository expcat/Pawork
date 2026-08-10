//! `pawork` —— Pawork 的唯一正式可执行宿主（CLI 与 Core 同进程同二进制）。
//!
//! 装配流程：解析 CLI → 初始化 tracing → 装配 [`core_runtime::CoreRuntime`]
//! （AppService + EventHub + EventPump）→ 装配 GUI Server（P13-4，serve 模式
//! 打开本地端点）→ 交给 [`cli_host::CliHost`] 按运行模式执行 → 以退出码结束进程。

mod gui_host;

use std::sync::Arc;

use clap::Parser;
use cli_command::Cli;
use cli_host::CliHost;
use client_auth::{TokenAuthenticator, TokenStore};
use core_api::SUPPORTED_API_VERSIONS;
use core_runtime::CoreRuntime;
use gui_host::ServeGuiHost;
use gui_protocol::{GuiCapability, HandshakeService};
use gui_server::{GuiServer, GuiServerConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    init_tracing();
    let cli = Cli::parse();

    // 装配完整 Core：AppService + EventHub + EventPump（10ms 轮询事件队列）。
    let runtime = CoreRuntime::new(cli.instance.clone());
    let mut host = CliHost::with_hub(runtime.service().clone(), runtime.hub().clone());
    // GUI Server 装配（P13-4）：serve 模式打开本地 GUI Endpoint；装配失败
    // （token 目录不可写等）时降级为仅等待信号并告警。
    match build_gui_server(&runtime, &cli.instance) {
        Ok(server) => host.attach_gui_server(Arc::new(server)),
        Err(message) => tracing::warn!("gui server disabled: {message}"),
    }

    let outcome = host.execute(cli).await;
    println!("{}", outcome.output);
    if outcome.exit_code != 0 {
        std::process::exit(outcome.exit_code);
    }
}

/// 装配 serve 模式 GUI Server：LocalTransport 本地端点 + 每实例 token 认证。
fn build_gui_server(runtime: &CoreRuntime, instance: &str) -> Result<ServeGuiHost, String> {
    let token_store = TokenStore::new(gui_host::instance_dir(instance).join("gui.token"));
    if !token_store.path().exists() {
        // 首次运行生成 token；已存在则复用（重启不覆盖）。
        let _ = token_store
            .generate()
            .map_err(|error| format!("cannot create gui token: {error}"))?;
    } else {
        let _ = token_store
            .load()
            .map_err(|error| format!("cannot read gui token: {error}"))?;
    }

    let handshake = HandshakeService::new(
        agent_domain::CoreInstanceId::from(instance),
        SUPPORTED_API_VERSIONS.to_vec(),
        vec![
            GuiCapability::Events,
            GuiCapability::Snapshots,
            GuiCapability::ArtifactStreaming,
        ],
    )
    .with_authenticator(Box::new(TokenAuthenticator::new(token_store)));

    let transport: Arc<dyn transport_api::GuiTransportServer> =
        Arc::new(transport_local::LocalTransport::default());
    let server = Arc::new(GuiServer::new(GuiServerConfig {
        app_service: runtime.service().clone(),
        handshake,
        transport,
        hub: runtime.hub().clone(),
        connections: None,
    }));
    Ok(ServeGuiHost::new(server))
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
