//! `probe` —— Pawork GUI Connection Protocol live 测试客户端。
//!
//! `--self-test` 的 9 场景已迁到 `tests/probe.rs`。本 example 只保留 live 模式：
//! - `--connect <local://地址>`：外部连接，握手 + WorkspaceList；
//! - `--live-two-gui <local://地址>`：两个客户端连真实 serve，kill 一个后 Resume；
//! - `--live-pty <local://地址>`：真实 serve 上开 PTY、写一行、断线重连后续接。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use pawork_app::{default_data_dir, DEFAULT_INSTANCE};
use pawork_client::{GuiClient, ResumeDisposition};
use pawork_domain::{ActorId, WorkspaceId};
use pawork_protocol::client_auth::TOKEN_SCHEME;
use pawork_protocol::{
    ActorIdentity, AppCommand, AppEvent, AppQuery, AppResponse, ClientAuthentication, CommandSource,
};
use pawork_transport::{ConnectOptions, GuiTransportClient, LocalTransport, TransportEndpoint};

#[derive(Parser, Debug)]
#[command(
    name = "protocol-probe",
    about = "Pawork GUI Connection Protocol 测试客户端"
)]
struct Cli {
    /// 外部连接模式端点：local://<Unix socket 路径或 Windows named pipe 名>。
    #[arg(long, value_name = "ENDPOINT")]
    connect: Option<String>,
    /// 两个 gui-client 连真实 serve：并发订阅、kill 一个、Resume Replay。
    #[arg(long, value_name = "ENDPOINT")]
    live_two_gui: Option<String>,
    /// 真实 serve 上开 PTY、写一行、断线重连后续接。
    #[arg(long, value_name = "ENDPOINT")]
    live_pty: Option<String>,
    /// 握手 token 文件；省略则读 `{data_dir}/gui.token`（与 `pawork gui serve` 相同）。
    #[arg(long, value_name = "PATH")]
    token: Option<String>,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = if let Some(endpoint) = cli.live_two_gui {
        match live_two_gui(&endpoint, cli.token.as_deref()).await {
            Ok(report) => {
                println!("PASS live-two-gui: {report}");
                0
            }
            Err(error) => {
                println!("FAIL live-two-gui: {error}");
                1
            }
        }
    } else if let Some(endpoint) = cli.live_pty {
        match live_pty(&endpoint, cli.token.as_deref()).await {
            Ok(report) => {
                println!("PASS live-pty: {report}");
                0
            }
            Err(error) => {
                println!("FAIL live-pty: {error}");
                1
            }
        }
    } else if let Some(endpoint) = cli.connect {
        match connect_mode(&endpoint, cli.token.as_deref()).await {
            Ok(()) => 0,
            Err(error) => {
                println!("FAIL connect: {error}");
                1
            }
        }
    } else {
        println!(
            "用法: probe --connect <local://地址> [--token <path>] | --live-two-gui <local://地址> | --live-pty <local://地址>"
        );
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
    let authentication = Some(load_gui_authentication(token_path)?);

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

fn parse_local_address(endpoint: &str) -> Result<String, String> {
    let address = endpoint
        .strip_prefix("local://")
        .ok_or_else(|| format!("仅支持 local:// 端点，got {endpoint}"))?;
    if address.is_empty() {
        return Err("空的 local:// 端点".into());
    }
    Ok(address.into())
}

fn gui_token_path(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    let data_dir = data_dir.as_ref();
    if instance == DEFAULT_INSTANCE {
        data_dir.join("gui.token")
    } else {
        data_dir.join(format!("gui-{instance}.token"))
    }
}

fn load_gui_authentication(token_path: Option<&str>) -> Result<ClientAuthentication, String> {
    let path = match token_path {
        Some(path) => PathBuf::from(path),
        None => gui_token_path(default_data_dir(), DEFAULT_INSTANCE),
    };
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("读取 token 失败 ({}): {error}", path.display()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("token 文件为空或不是有效 UTF-8: {}", path.display()))?;
    let proof = text.trim();
    if proof.is_empty() {
        return Err(format!("token 文件为空或格式错误: {}", path.display()));
    }
    Ok(ClientAuthentication {
        scheme: TOKEN_SCHEME.into(),
        proof: proof.to_string(),
    })
}

async fn connect_named(
    address: &str,
    label: &str,
    authentication: Option<ClientAuthentication>,
) -> Result<GuiClient, String> {
    let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
    GuiClient::connect(
        transport,
        TransportEndpoint::Local {
            address: address.into(),
        },
        ConnectOptions {
            timeout_ms: 8_000,
            client_label: Some(label.into()),
            max_frame_bytes: 1024 * 1024,
        },
        authentication,
    )
    .await
    .map_err(|error| format!("{label} 连接/握手失败: {error}"))
}

fn local_gui_identity(name: &str) -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from(name),
        display_name: None,
    }
}

async fn live_two_gui(endpoint: &str, token_path: Option<&str>) -> Result<String, String> {
    let address = parse_local_address(endpoint)?;
    let authentication = Some(load_gui_authentication(token_path)?);
    let client_a = connect_named(&address, "live-a", authentication.clone()).await?;
    let client_b = connect_named(&address, "live-b", authentication.clone()).await?;
    if client_a.client_id() == client_b.client_id() {
        return Err("两个客户端拿到了同一个 client_id".into());
    }
    client_a
        .subscribe_all()
        .await
        .map_err(|error| format!("A subscribe: {error}"))?;
    client_b
        .subscribe_all()
        .await
        .map_err(|error| format!("B subscribe: {error}"))?;

    let workspace_id = workspace_id_of(&client_a, "live-a").await?;
    let created = client_a
        .command(
            AppCommand::SessionCreate {
                workspace_id: workspace_id.clone(),
                title: Some("s10-live-a".into()),
            },
            CommandSource::LocalGui {
                client_id: client_a.client_id().clone(),
            },
            local_gui_identity("live-a"),
        )
        .await
        .map_err(|error| format!("A SessionCreate: {error}"))?;
    if !matches!(created.response, AppResponse::Data(_)) {
        return Err(format!("A SessionCreate 非 Data: {:?}", created.response));
    }
    let snap_b = client_b
        .snapshot()
        .await
        .map_err(|error| format!("B snapshot: {error}"))?;
    let b_sees_a_session = snap_b.sections.iter().any(|section| {
        section
            .data
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry.get("title").and_then(|title| title.as_str()) == Some("s10-live-a")
                })
            })
    });
    if !b_sees_a_session {
        return Err("B 的 Snapshot 看不到 A 新建的会话（状态串台或未共享）".into());
    }

    let terminal = client_a
        .command(
            AppCommand::TerminalCreate {
                workspace_id: workspace_id.clone(),
                working_directory: None,
            },
            CommandSource::LocalGui {
                client_id: client_a.client_id().clone(),
            },
            local_gui_identity("live-a"),
        )
        .await
        .map_err(|error| format!("A TerminalCreate: {error}"))?;
    let terminal_id = match terminal.response {
        AppResponse::Data(value) => value
            .get("terminal_session_id")
            .and_then(|id| id.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("TerminalCreate 缺少 id: {value}"))?,
        other => return Err(format!("TerminalCreate 非 Data: {other:?}")),
    };
    client_a
        .command(
            AppCommand::TerminalWrite {
                terminal_session_id: terminal_id.clone(),
                data: "echo s10-two-gui\n".into(),
            },
            CommandSource::LocalGui {
                client_id: client_a.client_id().clone(),
            },
            local_gui_identity("live-a"),
        )
        .await
        .map_err(|error| format!("A TerminalWrite: {error}"))?;

    let mut last_a = client_a.last_acked_sequence();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut a_saw_output = false;
    while tokio::time::Instant::now() < deadline {
        match client_a
            .next_event_timeout(Duration::from_millis(400))
            .await
        {
            Ok(event) => {
                last_a = event.global_sequence;
                let _ = client_a.ack(event.global_sequence).await;
                if matches!(
                    &event.payload,
                    AppEvent::TerminalOutput { delta, .. } if delta.contains("s10-two-gui")
                ) {
                    a_saw_output = true;
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    if !a_saw_output {
        return Err("A 未收到自己的 PTY 输出".into());
    }
    let mut b_saw_output = false;
    let wait_b = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < wait_b {
        match client_b
            .next_event_timeout(Duration::from_millis(400))
            .await
        {
            Ok(event) => {
                let _ = client_b.ack(event.global_sequence).await;
                if matches!(
                    &event.payload,
                    AppEvent::TerminalOutput { delta, .. } if delta.contains("s10-two-gui")
                ) {
                    b_saw_output = true;
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    if !b_saw_output {
        return Err("B 未收到 A 的 PTY 事件（Hub 扇出失败）".into());
    }

    client_a
        .close()
        .await
        .map_err(|error| format!("A close: {error}"))?;

    client_b
        .command(
            AppCommand::TerminalWrite {
                terminal_session_id: terminal_id,
                data: "echo s10-after-disconnect\n".into(),
            },
            CommandSource::LocalGui {
                client_id: client_b.client_id().clone(),
            },
            local_gui_identity("live-b"),
        )
        .await
        .map_err(|error| format!("B TerminalWrite: {error}"))?;
    let mut last_b = client_b.last_acked_sequence();
    let drain_b = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < drain_b {
        match client_b
            .next_event_timeout(Duration::from_millis(400))
            .await
        {
            Ok(event) => {
                last_b = event.global_sequence;
                let _ = client_b.ack(event.global_sequence).await;
            }
            Err(_) => break,
        }
    }

    let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
    let (rejoined, resume) = GuiClient::connect_with_resume(
        transport,
        TransportEndpoint::Local {
            address: address.clone(),
        },
        ConnectOptions {
            timeout_ms: 8_000,
            client_label: Some("live-a-resume".into()),
            max_frame_bytes: 1024 * 1024,
        },
        authentication,
        Some(last_a),
    )
    .await
    .map_err(|error| format!("A 重连失败: {error}"))?;
    let outcome = resume.ok_or_else(|| "重连未返回 ResumeOutcome".to_string())?;
    let disposition = match &outcome.disposition {
        ResumeDisposition::Replay {
            from_sequence,
            through_sequence,
        } => {
            if outcome.replayed.is_empty() && through_sequence.0 > from_sequence.0.saturating_sub(1)
            {
                return Err("Replay 声明补发但 replayed 为空".into());
            }
            format!(
                "replay {}-{} n={}",
                from_sequence.0,
                through_sequence.0,
                outcome.replayed.len()
            )
        }
        ResumeDisposition::SnapshotRequired { .. } => "snapshot_required".into(),
        ResumeDisposition::UpToDate { current_sequence } => {
            format!("up_to_date {}", current_sequence.0)
        }
    };
    let _ = rejoined.close().await;
    let _ = client_b.close().await;
    Ok(format!(
        "a={} b={} last_a={} last_b={} resume={disposition}",
        client_a.client_id().as_str(),
        client_b.client_id().as_str(),
        last_a.0,
        last_b.0,
    ))
}

async fn workspace_id_of(client: &GuiClient, name: &str) -> Result<WorkspaceId, String> {
    let listed = client
        .query(
            AppQuery::WorkspaceList,
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            local_gui_identity(name),
        )
        .await
        .map_err(|error| format!("WorkspaceList: {error}"))?;
    match listed.response {
        AppResponse::Data(ref value) => value
            .as_array()
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("id"))
            .and_then(|id| id.as_str())
            .map(WorkspaceId::from)
            .ok_or_else(|| format!("WorkspaceList 缺少 id: {:?}", listed.response)),
        other => Err(format!("WorkspaceList 非 Data: {other:?}")),
    }
}

async fn live_pty(endpoint: &str, token_path: Option<&str>) -> Result<String, String> {
    let address = parse_local_address(endpoint)?;
    let authentication = Some(load_gui_authentication(token_path)?);
    let client = connect_named(&address, "live-pty", authentication.clone()).await?;
    client
        .subscribe_all()
        .await
        .map_err(|error| format!("subscribe: {error}"))?;
    let workspace_id = workspace_id_of(&client, "live-pty").await?;
    let created = client
        .command(
            AppCommand::TerminalCreate {
                workspace_id,
                working_directory: None,
            },
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            local_gui_identity("live-pty"),
        )
        .await
        .map_err(|error| format!("TerminalCreate: {error}"))?;
    let terminal_id = match created.response {
        AppResponse::Data(value) => value
            .get("terminal_session_id")
            .and_then(|id| id.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("TerminalCreate 缺少 terminal_session_id: {value}"))?,
        other => return Err(format!("TerminalCreate 非 Data: {other:?}")),
    };
    client
        .command(
            AppCommand::TerminalWrite {
                terminal_session_id: terminal_id.clone(),
                data: "echo s10-pty\n".into(),
            },
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            local_gui_identity("live-pty"),
        )
        .await
        .map_err(|error| format!("TerminalWrite: {error}"))?;

    let mut output = String::new();
    let mut last = client.last_acked_sequence();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        match client.next_event_timeout(Duration::from_millis(500)).await {
            Ok(event) => {
                last = event.global_sequence;
                let _ = client.ack(event.global_sequence).await;
                if let AppEvent::TerminalOutput {
                    terminal_session_id,
                    delta,
                } = &event.payload
                {
                    if terminal_session_id == &terminal_id {
                        output.push_str(delta);
                    }
                }
                if output.contains("s10-pty") {
                    break;
                }
            }
            Err(_) => continue,
        }
    }
    if !output.contains("s10-pty") {
        return Err(format!("PTY 未回显 s10-pty；已收 {} 字节", output.len()));
    }
    client
        .close()
        .await
        .map_err(|error| format!("close: {error}"))?;

    let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
    let (rejoined, resume) = GuiClient::connect_with_resume(
        transport,
        TransportEndpoint::Local {
            address: address.clone(),
        },
        ConnectOptions {
            timeout_ms: 8_000,
            client_label: Some("live-pty-resume".into()),
            max_frame_bytes: 1024 * 1024,
        },
        authentication,
        Some(last),
    )
    .await
    .map_err(|error| format!("重连失败: {error}"))?;
    let outcome = resume.ok_or_else(|| "重连未返回 ResumeOutcome".to_string())?;
    let replayed_output: String = outcome
        .replayed
        .iter()
        .filter_map(|event| match &event.payload {
            AppEvent::TerminalOutput {
                terminal_session_id,
                delta,
            } if terminal_session_id == &terminal_id => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    let snapshot_has_terminal = rejoined.initial_snapshot().is_some_and(|snapshot| {
        snapshot.sections.iter().any(|section| {
            section
                .data
                .as_ref()
                .and_then(serde_json::Value::as_array)
                .is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry.get("terminal_session_id").and_then(|id| id.as_str())
                            == Some(terminal_id.as_str())
                    })
                })
        })
    });
    let disposition = match &outcome.disposition {
        ResumeDisposition::Replay { .. } => "replay",
        ResumeDisposition::SnapshotRequired { .. } => "snapshot_required",
        ResumeDisposition::UpToDate { .. } => "up_to_date",
    };
    let _ = rejoined.close().await;
    Ok(format!(
        "terminal={terminal_id} first_output_bytes={} resume={disposition} replayed_pty_bytes={} snapshot_has_terminal={snapshot_has_terminal}",
        output.len(),
        replayed_output.len(),
    ))
}
