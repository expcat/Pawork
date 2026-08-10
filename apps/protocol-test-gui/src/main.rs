//! `protocol-test-gui` —— Pawork GUI Connection Protocol 测试客户端。
//!
//! - `--self-test`：进程内装配 server（memory transport + tempdir token，
//!   不依赖 pawork serve）逐项跑契约场景，输出 PASS / FAIL，退出码 0/1；
//! - `--connect <local://地址>`：外部连接模式，仅握手 + status 查询。

mod harness;
mod scenarios;

use std::sync::Arc;

use clap::Parser;
use client_auth::TokenStore;
use core_api::{ActorIdentity, AppQuery, CommandSource};
use gui_client::GuiClient;
use transport_api::{ConnectOptions, GuiTransportClient, TransportEndpoint};
use transport_local::LocalTransport;

#[derive(Parser, Debug)]
#[command(
    name = "protocol-test-gui",
    about = "Pawork GUI Connection Protocol 测试客户端"
)]
struct Cli {
    /// 进程内 self-test：内存 transport + tempdir token 跑全部契约场景。
    #[arg(long)]
    self_test: bool,
    /// 外部连接模式端点：local://<Unix socket 路径或 Windows named pipe 名>。
    #[arg(long, value_name = "ENDPOINT")]
    connect: Option<String>,
    /// 外部连接模式使用的 token 文件路径（--connect 必需）。
    #[arg(long, value_name = "PATH")]
    token: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = if cli.self_test {
        scenarios::run_all().await
    } else if let Some(endpoint) = cli.connect {
        match connect_mode(&endpoint, cli.token.as_deref()).await {
            Ok(()) => 0,
            Err(error) => {
                println!("FAIL connect: {error}");
                1
            }
        }
    } else {
        println!("用法: protocol-test-gui --self-test | --connect <local://地址> [--token <path>]");
        2
    };
    std::process::exit(code);
}

/// 外部连接模式：握手 + status 查询。
async fn connect_mode(endpoint: &str, token_path: Option<&str>) -> Result<(), String> {
    let address = endpoint
        .strip_prefix("local://")
        .ok_or_else(|| format!("仅支持 local:// 端点，got {endpoint}"))?;
    if address.is_empty() {
        return Err("空的 local:// 端点".into());
    }
    let token_path = token_path.ok_or("--connect 需要 --token <path> 提供认证 token")?;
    let token = TokenStore::new(token_path)
        .load()
        .map_err(|error| format!("加载 token 失败: {error}"))?;

    let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
    let client = GuiClient::connect(
        transport,
        TransportEndpoint::Local {
            address: address.into(),
        },
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: Some("protocol-test-gui".into()),
            max_frame_bytes: 1024 * 1024,
        },
        &token,
    )
    .await
    .map_err(|error| format!("连接/握手失败: {error}"))?;
    println!(
        "握手成功: client_id={} connection_id={} api={}.{} capabilities={:?}",
        client.client_id().as_str(),
        client.connection_id().as_str(),
        client.api_version().major,
        client.api_version().minor,
        client.capabilities()
    );

    let status = client
        .query(
            AppQuery::WorkspaceList,
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            ActorIdentity::LocalUser {
                actor_id: agent_domain::ActorId::from("protocol-test-gui"),
                display_name: None,
            },
        )
        .await
        .map_err(|error| format!("status 查询失败: {error}"))?;
    println!(
        "status 查询: {}",
        serde_json::to_string_pretty(&status.response)
            .map_err(|error| format!("序列化响应失败: {error}"))?
    );

    client
        .close()
        .await
        .map_err(|error| format!("断开失败: {error}"))?;
    println!("断开成功");
    Ok(())
}
