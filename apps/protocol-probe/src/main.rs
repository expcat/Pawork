//! `protocol-probe` —— Pawork GUI Connection Protocol 测试客户端。
//!
//! - `--self-test`：进程内装配 server（MemoryTransport + GuiHostAdapter），
//!   不依赖 `pawork gui serve`，逐项跑 9 个契约场景；
//! - `--connect <local://地址>`：外部连接，握手 + WorkspaceList。

mod harness;
mod scenarios;

use std::sync::Arc;

use clap::Parser;
use pawork_client::GuiClient;
use pawork_domain::ActorId;
use pawork_protocol::{
    ActorIdentity, AppQuery, ClientAuthentication, CommandSource,
};
use pawork_transport::{
    ConnectOptions, GuiTransportClient, LocalTransport, TransportEndpoint,
};

#[derive(Parser, Debug)]
#[command(
    name = "protocol-probe",
    about = "Pawork GUI Connection Protocol 测试客户端"
)]
struct Cli {
    /// 进程内 self-test：内存 transport 跑全部契约场景。
    #[arg(long)]
    self_test: bool,
    /// 外部连接模式端点：local://<Unix socket 路径或 Windows named pipe 名>。
    #[arg(long, value_name = "ENDPOINT")]
    connect: Option<String>,
    /// 可选握手 proof（V2 本机 serve 默认不校验）。
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
        println!("用法: protocol-probe --self-test | --connect <local://地址> [--token <path>]");
        2
    };
    std::process::exit(code);
}

async fn connect_mode(endpoint: &str, token_path: Option<&str>) -> Result<(), String> {
    let address = endpoint
        .strip_prefix("local://")
        .ok_or_else(|| format!("仅支持 local:// 端点，got {endpoint}"))?;
    if address.is_empty() {
        return Err("空的 local:// 端点".into());
    }
    let authentication = match token_path {
        Some(path) => {
            let proof = std::fs::read_to_string(path)
                .map_err(|error| format!("读取 token 失败: {error}"))?;
            Some(ClientAuthentication {
                scheme: "token".into(),
                proof: proof.trim().to_string(),
            })
        }
        None => None,
    };

    let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
    let client = GuiClient::connect(
        transport,
        TransportEndpoint::Local {
            address: address.into(),
        },
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: Some("protocol-probe".into()),
            max_frame_bytes: 1024 * 1024,
        },
        authentication,
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
                actor_id: ActorId::from("protocol-probe"),
                display_name: None,
            },
        )
        .await
        .map_err(|error| format!("WorkspaceList 失败: {error}"))?;
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

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn self_test_all_scenarios() {
        assert_eq!(
            crate::scenarios::run_all().await,
            0,
            "protocol-probe --self-test 应全绿"
        );
    }
}
