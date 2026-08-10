//! 进程内测试装配：MemoryTransport + GuiServer + AppService + EventHub + pump
//! （参照 crates/gui-server/tests/multi_gui_runtime.rs；不依赖 pawork serve 装配）。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::{ActorId, CommandId, CoreInstanceId, SessionId, Timestamp, WorkspaceId};
use app_service::AppService;
use artifact_store::ArtifactStore;
use client_auth::{Token, TokenAuthenticator, TokenStore};
use connection_manager::ConnectionManager;
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppResponse, CommandSource, API_VERSION,
    SUPPORTED_API_VERSIONS,
};
use gui_client::GuiClient;
use gui_protocol::{GuiCapability, HandshakeService};
use gui_server::{GuiServer, GuiServerConfig};
use provider_api::ModelProvider;
use serde_json::Value;
use subscription_hub::EventHub;
use tempfile::TempDir;
use transport_api::{
    ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, TransportEndpoint,
};
use transport_memory::MemoryTransport;

const CHANNEL: &str = "protocol-self-test";

pub struct Harness {
    pub app_service: Arc<AppService>,
    pub hub: Arc<EventHub>,
    pub connections: Arc<ConnectionManager>,
    pub transport: Arc<MemoryTransport>,
    pub listener: Arc<dyn GuiListener>,
    pub token: Token,
    pump: tokio::task::JoinHandle<()>,
    /// 宿主侧连接句柄：drop 会释放 close 通道导致会话断线，须持有到场景结束。
    sessions: Vec<Box<dyn GuiConnection>>,
    _temp: TempDir,
}

impl Harness {
    pub async fn new(instance: &str) -> Self {
        Self::new_with(instance, None, None).await
    }

    pub async fn new_with(
        instance: &str,
        hub: Option<Arc<EventHub>>,
        store: Option<Arc<ArtifactStore>>,
    ) -> Self {
        let app_service = match store {
            Some(store) => Arc::new(AppService::with_artifact_store(instance, store)),
            None => Arc::new(AppService::new(instance)),
        };
        let hub = hub.unwrap_or_else(|| Arc::new(EventHub::new()));
        let pump = spawn_pump(Arc::clone(&app_service), Arc::clone(&hub));
        let temp = tempfile::tempdir().expect("tempdir");
        let token_path = temp.path().join("gui.token");
        let token = TokenStore::new(&token_path)
            .generate()
            .expect("generate token");
        let handshake = HandshakeService::new(
            CoreInstanceId::from(instance),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::ArtifactStreaming,
            ],
        )
        .with_authenticator(Box::new(TokenAuthenticator::new(TokenStore::new(
            &token_path,
        ))));
        let transport = Arc::new(MemoryTransport::new());
        let connections = Arc::new(ConnectionManager::default());
        let server = GuiServer::new(GuiServerConfig {
            app_service: Arc::clone(&app_service),
            handshake,
            transport: Arc::clone(&transport) as Arc<dyn transport_api::GuiTransportServer>,
            hub: Arc::clone(&hub),
            connections: Some(Arc::clone(&connections)),
        });
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: CHANNEL.into(),
            })
            .await
            .expect("bind");
        Harness {
            app_service,
            hub,
            connections,
            transport,
            listener: Arc::from(listener),
            token,
            pump,
            sessions: Vec::new(),
            _temp: temp,
        }
    }

    pub fn register_provider(&self, provider: Arc<dyn ModelProvider>) {
        self.app_service.register_provider(provider);
    }

    pub fn connect_options(label: &str) -> ConnectOptions {
        ConnectOptions {
            timeout_ms: 2_000,
            client_label: Some(label.into()),
            max_frame_bytes: 1024 * 1024,
        }
    }

    /// 经 SDK 连接一个新 GUI：accept 与 connect 并行，持有宿主侧句柄。
    pub async fn connect_gui(&mut self, label: &str) -> Result<GuiClient, String> {
        let listener = Arc::clone(&self.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let transport: Arc<dyn GuiTransportClient> = self.transport.clone();
        let client = GuiClient::connect(
            transport,
            TransportEndpoint::Memory {
                channel: CHANNEL.into(),
            },
            Self::connect_options(label),
            &self.token,
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

    /// 经 SDK 重连（connect_with_resume 辅助），持有宿主侧句柄。
    pub async fn reconnect_gui(
        &mut self,
        label: &str,
        last_global_sequence: Option<core_api::GlobalSequence>,
    ) -> Result<(GuiClient, Option<gui_client::ResumeOutcome>), String> {
        let listener = Arc::clone(&self.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let transport: Arc<dyn GuiTransportClient> = self.transport.clone();
        let (client, outcome) = GuiClient::connect_with_resume(
            transport,
            TransportEndpoint::Memory {
                channel: CHANNEL.into(),
            },
            Self::connect_options(label),
            &self.token,
            last_global_sequence,
        )
        .await
        .map_err(|error| format!("reconnect: {error}"))?;
        let session = accept
            .await
            .map_err(|error| format!("accept task: {error}"))?
            .map_err(|error| format!("accept: {error}"))?;
        self.sessions.push(session);
        Ok((client, outcome))
    }

    /// 建 workspace + session（CLI 来源），返回 session_id。
    pub fn prepare_session(&self) -> Result<SessionId, String> {
        let dir = std::env::temp_dir().join(format!("pawork-self-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|error| format!("create workspace dir: {error}"))?;
        let response = self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::WorkspaceAdd {
                root_path: dir.to_string_lossy().into_owned(),
            },
        ));
        let workspace_id = match &response.response {
            AppResponse::Data(value) => WorkspaceId::from(
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "WorkspaceAdd 响应缺少 id".to_string())?,
            ),
            other => return Err(format!("WorkspaceAdd 应返回 Data，got {other:?}")),
        };
        let response = self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::SessionCreate {
                workspace_id,
                title: Some("self-test".into()),
            },
        ));
        match &response.response {
            AppResponse::Data(value) => Ok(SessionId::from(
                value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "SessionCreate 响应缺少 session_id".to_string())?,
            )),
            other => Err(format!("SessionCreate 应返回 Data，got {other:?}")),
        }
    }

    /// CLI 发起的 RunStart，返回 run_id。
    pub fn start_run_cli(
        &self,
        session_id: &SessionId,
        message: &str,
    ) -> Result<agent_domain::RunId, String> {
        let response = self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: message.into(),
                model: None,
            },
        ));
        if !matches!(response.response, AppResponse::Accepted { .. }) {
            return Err(format!("RunStart 应 Accepted，got {:?}", response.response));
        }
        self.app_service
            .router()
            .last_started_run()
            .ok_or_else(|| "last_started_run 缺失".to_string())
    }

    pub fn cancel_run_cli(&self, run_id: &agent_domain::RunId) {
        self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::RunCancel {
                run_id: run_id.clone(),
            },
        ));
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

fn spawn_pump(app_service: Arc<AppService>, hub: Arc<EventHub>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            for event in app_service.drain_events() {
                hub.publish(event);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
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

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}
