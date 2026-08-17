//! 运维子命令：`status` / `watch` / `shutdown` / `doctor`。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pawork_app::{default_data_dir, instance_dir, session_db_path_for, DEFAULT_INSTANCE};
use pawork_client::{ClientConfig, GuiClient};
use pawork_protocol::headless::translate::encode_protocol_response;
use pawork_protocol::{
    headless::HeadlessResponse, AppEvent, GuiCapability, SUPPORTED_API_VERSIONS,
};
use pawork_transport::{
    ConnectOptions, GuiTransportClient, LocalTransport, TransportEndpoint,
};

use crate::CliError;

pub fn service_name(instance: &str) -> String {
    if instance == DEFAULT_INSTANCE {
        "pawork".into()
    } else {
        format!("pawork.{instance}")
    }
}

pub fn gui_socket_path(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    let data_dir = data_dir.as_ref();
    if instance == DEFAULT_INSTANCE {
        data_dir.join("pawork-gui.sock")
    } else {
        data_dir.join(format!("pawork-gui-{instance}.sock"))
    }
}

pub fn gui_pid_path(data_dir: impl AsRef<Path>, instance: &str) -> PathBuf {
    instance_dir(data_dir, instance).join("gui-serve.pid")
}

pub fn resolved_instance(instance: &str) -> &str {
    if instance.trim().is_empty() {
        DEFAULT_INSTANCE
    } else {
        instance
    }
}

pub async fn run_status(instance: &str, json: bool) -> Result<(), CliError> {
    let report = inspect_instance(instance).await;
    print_report("status", &report, json)
}

pub async fn run_doctor(instance: &str, json: bool) -> Result<(), CliError> {
    let mut report = inspect_instance(instance).await;
    if report.listening {
        report.handshake = Some(probe_handshake(&report.socket).await);
    }
    print_report("doctor", &report, json)
}

pub async fn run_watch(instance: &str, json: bool) -> Result<(), CliError> {
    let report = inspect_instance(instance).await;
    if !report.listening {
        return Err(CliError::Usage(format!(
            "no gui serve listening on {} (start with `pawork --instance {} gui serve`)",
            report.socket, report.instance
        )));
    }
    let transport = Arc::new(LocalTransport::default());
    let client = GuiClient::connect(
        transport,
        TransportEndpoint::Local {
            address: report.socket.clone(),
        },
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: Some("pawork-watch".into()),
            max_frame_bytes: 1024 * 1024,
        },
        None,
    )
    .await
    .map_err(|error| CliError::Usage(error.to_string()))?;
    client
        .subscribe_all()
        .await
        .map_err(|error| CliError::Usage(error.to_string()))?;
    if !json {
        eprintln!("watching {} (Ctrl-C to stop)", report.socket);
    }
    loop {
        tokio::select! {
            event = client.next_event() => {
                match event {
                    Ok(envelope) => {
                        if json {
                            let frame = HeadlessResponse::Event { envelope };
                            let line = encode_protocol_response(&frame)
                                .map_err(|error| CliError::Usage(error.to_string()))?;
                            println!("{line}");
                        } else {
                            eprintln!("{}", event_hint(&envelope.payload));
                        }
                    }
                    Err(error) => return Err(CliError::Usage(error.to_string())),
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

pub async fn run_shutdown(instance: &str, json: bool) -> Result<(), CliError> {
    let report = inspect_instance(instance).await;
    let Some(pid) = report.pid else {
        return Err(CliError::Usage(format!(
            "no pid file at {}",
            report.pid_file
        )));
    };
    send_term(pid)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "action": "shutdown",
                "pid": pid,
                "instance": report.instance,
            })
        );
    } else {
        eprintln!("sent SIGTERM to pid {pid}");
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct InstanceReport {
    instance: String,
    data_dir: String,
    socket: String,
    pid_file: String,
    session_db: String,
    listening: bool,
    pid: Option<u32>,
    handshake: Option<String>,
}

async fn inspect_instance(instance: &str) -> InstanceReport {
    let instance = resolved_instance(instance).to_string();
    let data_dir = default_data_dir();
    let socket = gui_socket_path(&data_dir, &instance);
    let pid_file = gui_pid_path(&data_dir, &instance);
    let session_db = session_db_path_for(&data_dir, &instance);
    let pid = std::fs::read_to_string(&pid_file)
        .ok()
        .and_then(|text| text.trim().parse().ok());
    let listening = probe_socket(&socket).await;
    InstanceReport {
        instance,
        data_dir: data_dir.display().to_string(),
        socket: socket.display().to_string(),
        pid_file: pid_file.display().to_string(),
        session_db: session_db.display().to_string(),
        listening,
        pid,
        handshake: None,
    }
}

async fn probe_socket(socket: &Path) -> bool {
    let client = LocalTransport::default();
    client
        .connect(
            TransportEndpoint::Local {
                address: socket.display().to_string(),
            },
            ConnectOptions {
                timeout_ms: 300,
                client_label: Some("pawork-status".into()),
                max_frame_bytes: 1024 * 1024,
            },
        )
        .await
        .is_ok()
}

async fn probe_handshake(socket: &str) -> String {
    let transport = Arc::new(LocalTransport::default());
    match GuiClient::connect_with_config(
        transport,
        TransportEndpoint::Local {
            address: socket.into(),
        },
        ConnectOptions {
            timeout_ms: 3_000,
            client_label: Some("pawork-doctor".into()),
            max_frame_bytes: 1024 * 1024,
        },
        None,
        ClientConfig {
            timeout: Duration::from_secs(3),
            client_name: "pawork-doctor".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
            supported_api_versions: SUPPORTED_API_VERSIONS.to_vec(),
        },
    )
    .await
    {
        Ok(client) => format!(
            "ok client={} caps={}",
            client.client_id().as_str(),
            client.capabilities().len()
        ),
        Err(error) => format!("failed: {error}"),
    }
}

fn print_report(kind: &str, report: &InstanceReport, json: bool) -> Result<(), CliError> {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "kind": kind,
                "instance": report.instance,
                "data_dir": report.data_dir,
                "socket": report.socket,
                "pid_file": report.pid_file,
                "session_db": report.session_db,
                "listening": report.listening,
                "pid": report.pid,
                "handshake": report.handshake,
            })
        );
    } else {
        println!("instance: {}", report.instance);
        println!("data_dir: {}", report.data_dir);
        println!("socket: {} ({})", report.socket, if report.listening { "listening" } else { "down" });
        println!("session_db: {}", report.session_db);
        if let Some(pid) = report.pid {
            println!("pid: {pid}");
        }
        if let Some(handshake) = &report.handshake {
            println!("handshake: {handshake}");
        }
    }
    Ok(())
}

fn send_term(pid: u32) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        let status = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map_err(CliError::Io)?;
        if status.success() {
            Ok(())
        } else {
            Err(CliError::Usage(format!("kill -TERM {pid} failed")))
        }
    }
    #[cfg(windows)]
    {
        let status = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .map_err(CliError::Io)?;
        if status.success() {
            Ok(())
        } else {
            Err(CliError::Usage(format!("taskkill {pid} failed")))
        }
    }
}

pub fn write_pid_file(path: &Path) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{}\n", std::process::id()))?;
    Ok(())
}

pub fn remove_pid_file(path: &Path) {
    let _ = std::fs::remove_file(path);
}

fn event_hint(payload: &AppEvent) -> String {
    match payload {
        AppEvent::RunChanged { run_id, state } => {
            format!("run_changed {} {state:?}", run_id.as_str())
        }
        AppEvent::AssistantDelta { run_id, .. } => {
            format!("assistant_delta {}", run_id.as_str())
        }
        AppEvent::ToolStarted { name, .. } => format!("tool_started {name}"),
        AppEvent::ToolCompleted { success, .. } => format!("tool_completed success={success}"),
        AppEvent::SessionChanged { session_id, .. } => {
            format!("session_changed {}", session_id.as_str())
        }
        other => format!("{other:?}")
            .split('{')
            .next()
            .unwrap_or("event")
            .trim()
            .to_string(),
    }
}
