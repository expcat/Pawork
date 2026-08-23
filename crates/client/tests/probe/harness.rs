//! 进程内测试装配：MemoryTransport + GuiServer + GuiHostAdapter + MockProvider。

use std::sync::Arc;

use pawork_app::{AppCore, ApprovalMode, DenyAllApprovals, GuiHostAdapter};
use pawork_client::{ClientConfig, GuiClient};
use pawork_domain::{ActorId, CommandId, ModelId, ProviderId, RunId, SessionId, Timestamp};
use pawork_app::gui_server::{GuiHost, GuiServer, GuiServerConfig};
use pawork_protocol::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppResponse, CommandSource, GuiCapability,
    HandshakeService, API_VERSION, SUPPORTED_API_VERSIONS,
};
use pawork_storage::session::SessionStore;
use pawork_testkit::{MockProvider, MockScript};
use pawork_transport::{
    ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, MemoryTransport,
    TransportEndpoint,
};
use serde_json::Value;
use tempfile::TempDir;

const CHANNEL: &str = "protocol-self-test";

pub struct Harness {
    pub adapter: Arc<GuiHostAdapter>,
    pub transport: Arc<MemoryTransport>,
    pub listener: Arc<dyn GuiListener>,
    sessions: Vec<Box<dyn GuiConnection>>,
    _temp: TempDir,
}

impl Harness {
    pub async fn new(label: &str, script: MockScript) -> Self {
        Self::new_with_approval(label, script, ApprovalMode::AskForDangerous, true).await
    }

    pub async fn new_with_approval(
        label: &str,
        script: MockScript,
        mode: ApprovalMode,
        trusted: bool,
    ) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(temp.path().join("session.db"))
            .await
            .expect("session store");
        let provider = MockProvider::new(script).with_id(ProviderId::from("mock"));
        let mut core = AppCore::from_parts(
            Arc::new(provider),
            None,
            ModelId::from("model-1"),
            ProviderId::from("mock"),
            Some(store),
        );
        // R7 波 B:terminal_create 已入 policy 闸;进程内装配用可创建档位
        // (AskForDangerous + trusted,AskUser 一律 fail-closed 由闸内处理)。
        core.configure_approval(
            mode,
            trusted,
            Arc::new(DenyAllApprovals),
        );
        let core = Arc::new(core);
        let adapter = Arc::new(GuiHostAdapter::new(core));
        let handshake = HandshakeService::new(
            GuiHost::instance_id(adapter.as_ref()),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::TerminalStreaming,
            ],
        );
        let transport = Arc::new(MemoryTransport::new());
        let server = GuiServer::new(GuiServerConfig {
            host: adapter.clone(),
            handshake,
            transport: transport.clone(),
            connections: None,
        });
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: format!("{CHANNEL}-{label}"),
            })
            .await
            .expect("bind");
        Harness {
            adapter,
            transport,
            listener: Arc::from(listener),
            sessions: Vec::new(),
            _temp: temp,
        }
    }

    pub fn endpoint(&self, label: &str) -> TransportEndpoint {
        TransportEndpoint::Memory {
            channel: format!("{CHANNEL}-{label}"),
        }
    }

    pub fn connect_options(label: &str) -> ConnectOptions {
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: Some(label.into()),
            max_frame_bytes: 1024 * 1024,
        }
    }

    pub async fn connect_gui(&mut self, harness_label: &str, client_label: &str) -> Result<GuiClient, String> {
        let listener = Arc::clone(&self.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let transport: Arc<dyn GuiTransportClient> = self.transport.clone();
        let mut config = ClientConfig::default();
        if !config.capabilities.contains(&GuiCapability::TerminalStreaming) {
            config.capabilities.push(GuiCapability::TerminalStreaming);
        }
        let client = GuiClient::connect_with_config(
            transport,
            self.endpoint(harness_label),
            Self::connect_options(client_label),
            None,
            config,
        )
        .await
        .map_err(|error| format!("connect/handshake: {error}"))?;
        let session = accept
            .await
            .map_err(|error| format!("accept task: {error}"))?
            .map_err(|error| format!("accept: {error}"))?;
        self.sessions.push(session);
        Ok(client)
    }

    pub async fn prepare_session(&self, title: &str) -> Result<SessionId, String> {
        let response = self
            .adapter
            .command(&command(
                cli_source(),
                cli_identity(),
                AppCommand::SessionCreate {
                    workspace_id: pawork_domain::WorkspaceId::from("ws-unbound"),
                    title: Some(title.into()),
                },
            ))
            .await
            .map_err(|error| error.to_string())?;
        match response {
            AppResponse::Data(value) => session_id_from_data(&value),
            other => Err(format!("SessionCreate 应返回 Data，got {other:?}")),
        }
    }

    pub async fn start_run_cli(
        &self,
        session_id: &SessionId,
        message: &str,
    ) -> Result<RunId, String> {
        let response = self
            .adapter
            .command(&command(
                cli_source(),
                cli_identity(),
                AppCommand::RunStart {
                    session_id: session_id.clone(),
                    user_message: message.into(),
                    model: None,
                    provider: None,
                    profile: None,
                },
            ))
            .await
            .map_err(|error| error.to_string())?;
        match response {
            AppResponse::Accepted {
                run_id: Some(run_id),
                ..
            } => Ok(run_id),
            other => Err(format!("RunStart 响应缺少 run id，got {other:?}")),
        }
    }

    pub async fn cancel_run_cli(&self, run_id: &RunId) -> Result<(), String> {
        self.adapter
            .command(&command(
                cli_source(),
                cli_identity(),
                AppCommand::RunCancel {
                    run_id: run_id.clone(),
                },
            ))
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }
}

pub fn cli_source() -> CommandSource {
    CommandSource::LocalCli {
        terminal_session_id: Some("terminal-1".into()),
    }
}

pub fn cli_identity() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("self-test"),
        display_name: None,
    }
}

pub fn local_user() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("self-test-gui-user"),
        display_name: None,
    }
}

pub fn gui_source(client: &GuiClient) -> CommandSource {
    CommandSource::LocalGui {
        client_id: client.client_id().clone(),
    }
}

pub fn command(
    source: CommandSource,
    identity: ActorIdentity,
    command: AppCommand,
) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(format!("cmd-{}", next_id())),
        source,
        identity,
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command,
    }
}

pub fn session_id_from_data(value: &Value) -> Result<SessionId, String> {
    Ok(SessionId::from(
        value
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "响应缺少 session_id".to_string())?,
    ))
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}
