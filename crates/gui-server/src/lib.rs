//! GUI 协议服务器（P13-4 / P13-5）。
//!
//! [`GuiServer`] 在 CLI 进程内接受 GUI 连接：`bind` 经由
//! [`transport-api`] 的 [`GuiTransportServer`] 绑定端点并返回
//! [`GuiListener`]；每次 `accept` 派生一个连接任务，完成
//! 握手（[`HandshakeService`] + 注入的 [`ClientAuthenticator`]）后进入帧循环：
//! Command / Query 经 `app-service` 的统一入口派发，ArtifactRead 按 64 KiB
//! 分片回 `ArtifactChunk`。P13-5 起：握手后先发 Snapshot；Subscribe 经
//! [`ConnectionManager`] 登记订阅，事件经每连接有界队列由帧循环任务发送；
//! Resume 按 [`compute_resume_disposition`] 补发 Replay 或降级 Snapshot；
//! Ack 记录 `last_ack`；Heartbeat 刷新活跃并回 Pong；心跳超时断线清理但
//! 不取消 Run。
//!
//! 线上帧编解码只使用 `gui-protocol` 的 encode/decode；传输层分帧（u32 LE
//! 长度前缀）由 transport 实现负责（见 `transport-local`）。

mod session;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_domain::{ConnectionId, GuiClientId};
use app_service::{
    AppService, IdentityContext, IdentityError, IdentityResolver, LocalIdentityResolver,
};
use async_trait::async_trait;
use connection_manager::ConnectionManager;
use gui_protocol::{HandshakeRequest, HandshakeService};
use snapshot_service::SnapshotService;
use subscription_hub::EventHub;
use thiserror::Error;
use transport_api::{
    GuiConnection, GuiListener, GuiTransportServer, TransportEndpoint, TransportError,
};

/// GUI 服务器的共享配置。
pub struct GuiServerConfig {
    pub app_service: Arc<AppService>,
    pub handshake: HandshakeService,
    pub transport: Arc<dyn GuiTransportServer>,
    /// Core 事件 Hub：订阅 / 重放 / `snapshot_sequence` 的来源。
    pub hub: Arc<EventHub>,
    /// 连接管理器覆盖（测试注入慢队列 / 短心跳超时）；缺省按默认配置创建。
    pub connections: Option<Arc<ConnectionManager>>,
}

/// 把已认证握手与连接元数据解析为 canonical GUI 连接身份。
pub trait GuiConnectionIdentityResolver: Send + Sync {
    fn resolve(
        &self,
        request: &HandshakeRequest,
        connection: &transport_api::ConnectionInfo,
    ) -> Result<IdentityContext, IdentityError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalGuiConnectionIdentityResolver;

impl GuiConnectionIdentityResolver for LocalGuiConnectionIdentityResolver {
    fn resolve(
        &self,
        _request: &HandshakeRequest,
        _connection: &transport_api::ConnectionInfo,
    ) -> Result<IdentityContext, IdentityError> {
        let actor = core_api::ActorIdentity::LocalUser {
            actor_id: agent_domain::ActorId::from("gui-connection"),
            display_name: None,
        };
        LocalIdentityResolver.resolve(actor.canonical_principal().as_deref())
    }
}

/// GUI 服务器错误。
#[derive(Debug, Error)]
pub enum GuiServerError {
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("internal error: {0}")]
    Internal(String),
}

pub(crate) struct Inner {
    pub app_service: Arc<AppService>,
    pub handshake: HandshakeService,
    pub hub: Arc<EventHub>,
    pub connections: Arc<ConnectionManager>,
    pub snapshots: SnapshotService,
    pub identity_resolver: Arc<dyn GuiConnectionIdentityResolver>,
}

/// CLI 进程内的 GUI 协议服务器（可廉价克隆，共享同一配置）。
#[derive(Clone)]
pub struct GuiServer {
    inner: Arc<Inner>,
    transport: Arc<dyn GuiTransportServer>,
}

impl GuiServer {
    pub fn new(config: GuiServerConfig) -> Self {
        Self::with_connection_identity_resolver(
            config,
            Arc::new(LocalGuiConnectionIdentityResolver),
        )
    }

    /// 以连接级身份解析器构造；默认 [`Self::new`] 使用本地用户身份。
    pub fn with_connection_identity_resolver(
        config: GuiServerConfig,
        identity_resolver: Arc<dyn GuiConnectionIdentityResolver>,
    ) -> Self {
        let connections = config
            .connections
            .unwrap_or_else(|| Arc::new(ConnectionManager::default()));
        let snapshots = SnapshotService::new(config.app_service.clone(), config.hub.clone());
        Self {
            inner: Arc::new(Inner {
                app_service: config.app_service,
                handshake: config.handshake,
                hub: config.hub,
                connections,
                snapshots,
                identity_resolver,
            }),
            transport: config.transport,
        }
    }

    pub fn app_service(&self) -> &Arc<AppService> {
        &self.inner.app_service
    }

    pub fn handshake(&self) -> &HandshakeService {
        &self.inner.handshake
    }

    pub fn hub(&self) -> &Arc<EventHub> {
        &self.inner.hub
    }

    pub fn connections(&self) -> &Arc<ConnectionManager> {
        &self.inner.connections
    }

    /// 绑定端点并返回 GUI 监听器；每次 `accept` 启动一个连接任务
    /// （握手 → 帧循环），并返回该连接的宿主侧句柄。
    pub async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, GuiServerError> {
        let transport_listener = self.transport.bind(endpoint).await?;
        Ok(Box::new(GuiServerListener {
            inner: Arc::clone(&self.inner),
            transport_listener,
            next_connection: AtomicU64::new(0),
        }))
    }
}

struct GuiServerListener {
    inner: Arc<Inner>,
    transport_listener: Box<dyn GuiListener>,
    next_connection: AtomicU64,
}

#[async_trait]
impl GuiListener for GuiServerListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        let connection = self.transport_listener.accept().await?;
        let n = self.next_connection.fetch_add(1, Ordering::Relaxed);
        let client_id = GuiClientId::from(format!("client-{n}"));
        let connection_id = ConnectionId::from(format!("connection-{n}"));
        let (handle, task) = session::spawn(
            Arc::clone(&self.inner),
            connection,
            client_id,
            connection_id,
        );
        tokio::spawn(task);
        Ok(Box::new(handle))
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.transport_listener.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{
        ActorId, ArtifactId, CommandId, CoreInstanceId, GuiClientId, QueryId, SessionId, Timestamp,
        WorkspaceId,
    };
    use artifact_store::ArtifactStore;
    use client_auth::{Token, TokenAuthenticator, TokenStore};
    use core_api::{
        ActorIdentity, ApiVersion, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope,
        AppResponse, ClientContextSnapshot, CommandSource, API_VERSION, SUPPORTED_API_VERSIONS,
    };
    use gui_protocol::{
        decode_server_frame, encode_client_frame, ArtifactReadRequest, ClientAuthentication,
        ClientFrame, GuiCapability, HandshakeRequest, HandshakeResponse, ProtocolErrorCode,
        ResumeRequest, ServerFrame, SubscribeRequest,
    };
    use tempfile::TempDir;
    use transport_api::{
        ConnectOptions, GuiConnection, GuiTransportClient, TransportEndpoint, TransportErrorKind,
        TransportFrame,
    };
    use transport_memory::MemoryTransport;

    #[derive(Clone)]
    struct FixedConnectionIdentityResolver(IdentityContext);

    impl GuiConnectionIdentityResolver for FixedConnectionIdentityResolver {
        fn resolve(
            &self,
            _request: &HandshakeRequest,
            _connection: &transport_api::ConnectionInfo,
        ) -> Result<IdentityContext, IdentityError> {
            Ok(self.0.clone())
        }
    }

    struct Harness {
        app_service: Arc<AppService>,
        listener: Arc<dyn GuiListener>,
        transport: Arc<MemoryTransport>,
        token: Token,
        _temp: TempDir,
    }

    fn authentication(token: &Token) -> ClientAuthentication {
        ClientAuthentication {
            scheme: client_auth::TOKEN_SCHEME.into(),
            proof: token.as_str().into(),
        }
    }

    async fn harness(channel: &str) -> Harness {
        harness_with(channel, None, None).await
    }

    async fn harness_with(
        channel: &str,
        connections: Option<Arc<ConnectionManager>>,
        store: Option<Arc<ArtifactStore>>,
    ) -> Harness {
        let app_service = match store {
            Some(store) => Arc::new(AppService::with_artifact_store("gui-server-test", store)),
            None => Arc::new(AppService::new("gui-server-test")),
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let token_path = temp.path().join("gui.token");
        let token = TokenStore::new(&token_path)
            .generate()
            .expect("generate token");
        let handshake = HandshakeService::new(
            CoreInstanceId::from("test-instance"),
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
        let server = GuiServer::new(GuiServerConfig {
            app_service: app_service.clone(),
            handshake,
            transport: transport.clone(),
            hub: Arc::new(EventHub::new()),
            connections,
        });
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: channel.into(),
            })
            .await
            .expect("bind");
        Harness {
            app_service,
            listener: Arc::from(listener),
            transport,
            token,
            _temp: temp,
        }
    }

    async fn tenant_harness(
        channel: &str,
        app_service: Arc<AppService>,
        identity: IdentityContext,
    ) -> Harness {
        tenant_harness_with_hub(channel, app_service, identity, Arc::new(EventHub::new())).await
    }

    /// 多租户测试：两个 GuiServer 共享同一 Hub（事件实时广播与重放共源）。
    async fn tenant_harness_with_hub(
        channel: &str,
        app_service: Arc<AppService>,
        identity: IdentityContext,
        hub: Arc<EventHub>,
    ) -> Harness {
        let temp = tempfile::tempdir().expect("tempdir");
        let token_path = temp.path().join("gui.token");
        let token = TokenStore::new(&token_path)
            .generate()
            .expect("generate token");
        let handshake = HandshakeService::new(
            CoreInstanceId::from("test-instance"),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![GuiCapability::Snapshots, GuiCapability::Events],
        )
        .with_authenticator(Box::new(TokenAuthenticator::new(TokenStore::new(
            &token_path,
        ))));
        let transport = Arc::new(MemoryTransport::new());
        let server = GuiServer::with_connection_identity_resolver(
            GuiServerConfig {
                app_service: app_service.clone(),
                handshake,
                transport: transport.clone(),
                hub,
                connections: None,
            },
            Arc::new(FixedConnectionIdentityResolver(identity)),
        );
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: channel.into(),
            })
            .await
            .expect("bind");
        Harness {
            app_service,
            listener: Arc::from(listener),
            transport,
            token,
            _temp: temp,
        }
    }

    struct TestClient {
        conn: Box<dyn GuiConnection>,
    }

    impl TestClient {
        async fn connect(harness: &Harness, channel: &str) -> Self {
            let conn = harness
                .transport
                .connect(
                    TransportEndpoint::Memory {
                        channel: channel.into(),
                    },
                    ConnectOptions {
                        timeout_ms: 1_000,
                        client_label: Some("test-gui".into()),
                        max_frame_bytes: 1024 * 1024,
                    },
                )
                .await
                .expect("connect");
            Self { conn }
        }

        async fn send(&self, frame: &ClientFrame) {
            let bytes = encode_client_frame(frame).expect("encode client frame");
            self.conn
                .send(TransportFrame::new(bytes))
                .await
                .expect("send frame");
        }

        async fn recv(&self) -> ServerFrame {
            let bytes = self.conn.receive().await.expect("receive frame");
            decode_server_frame(bytes.as_bytes()).expect("decode server frame")
        }
    }

    /// 启动一次 accept 并建立客户端连接，返回 (client, session handle)。
    async fn open_session(
        harness: &Harness,
        channel: &str,
    ) -> (TestClient, Box<dyn GuiConnection>) {
        let listener = Arc::clone(&harness.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = TestClient::connect(harness, channel).await;
        let session = accept.await.expect("accept task").expect("accept");
        (client, session)
    }

    async fn handshake(
        client: &TestClient,
        auth: Option<ClientAuthentication>,
    ) -> HandshakeResponse {
        client
            .send(&ClientFrame::Handshake(HandshakeRequest {
                request_id: "hs-1".into(),
                client_name: "test-gui".into(),
                client_version: "0.0.1".into(),
                supported_api_versions: vec![API_VERSION],
                capabilities: vec![GuiCapability::Events],
                authentication: auth,
            }))
            .await;
        let response = match client.recv().await {
            ServerFrame::Handshake(response) => response,
            other => panic!("expected handshake response, got {other:?}"),
        };
        // P13-5：握手后服务端先发 Snapshot（仅 Accepted 路径）。
        if matches!(response, HandshakeResponse::Accepted { .. }) {
            match client.recv().await {
                ServerFrame::Snapshot(snapshot) => {
                    assert_eq!(
                        snapshot.instance_id.as_str(),
                        "gui-server-test",
                        "首帧快照应属于本实例"
                    );
                }
                other => panic!("expected initial snapshot, got {other:?}"),
            }
        }
        response
    }

    fn local_user() -> ActorIdentity {
        ActorIdentity::LocalUser {
            actor_id: ActorId::from("test-user"),
            display_name: None,
        }
    }

    #[tokio::test]
    async fn handshake_then_command_and_query_round_trip() {
        let harness = harness("gui-1").await;
        let (client, session) = open_session(&harness, "gui-1").await;
        let response = handshake(&client, Some(authentication(&harness.token))).await;
        let HandshakeResponse::Accepted {
            selected_api_version,
            client_id,
            connection_id,
            capabilities,
            ..
        } = response
        else {
            panic!("expected accepted handshake: {response:?}");
        };
        assert_eq!(selected_api_version, API_VERSION);
        assert_eq!(client_id.as_str(), "client-0");
        assert_eq!(connection_id.as_str(), "connection-0");
        assert_eq!(capabilities, vec![GuiCapability::Events]);

        client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("cmd-1"),
                source: CommandSource::LocalGui {
                    client_id: client_id.clone(),
                },
                identity: local_user(),
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(1),
                command: AppCommand::WorkspaceAdd {
                    root_path: std::env::temp_dir().to_string_lossy().into_owned(),
                },
            }))
            .await;
        let ServerFrame::Response(command_response) = client.recv().await else {
            panic!("expected command response");
        };
        assert!(matches!(command_response.response, AppResponse::Data(_)));

        client
            .send(&ClientFrame::Query(AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("q-1"),
                source: CommandSource::LocalGui {
                    client_id: client_id.clone(),
                },
                identity: local_user(),
                issued_at: Timestamp::from_unix_millis(2),
                query: AppQuery::WorkspaceList,
            }))
            .await;
        let ServerFrame::Response(query_response) = client.recv().await else {
            panic!("expected query response");
        };
        assert!(matches!(query_response.response, AppResponse::Data(_)));

        client.conn.close().await.expect("client close");
        session.close().await.expect("session close");
    }

    #[tokio::test]
    async fn server_stamps_host_source_and_identity_over_wire() {
        // P17-5 主审修复：线上伪造的 source/identity 不进入 app-service；
        // 服务端按连接事实（locality + 服务端分配的 client/connection）盖戳。
        let harness = harness("gui-stamp").await;
        let (client, session) = open_session(&harness, "gui-stamp").await;
        let response = handshake(&client, Some(authentication(&harness.token))).await;
        let HandshakeResponse::Accepted { client_id, .. } = response else {
            panic!("expected accepted handshake: {response:?}");
        };
        assert_eq!(client_id.as_str(), "client-0");

        // 命令：wire 伪造 RemoteGui + System，服务端必须重写为 LocalGui +
        // LocalUser（本机 MemoryTransport = InProcess locality）。
        client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("cmd-forged"),
                source: CommandSource::RemoteGui {
                    client_id: GuiClientId::from("forged"),
                    connection_id: agent_domain::ConnectionId::from("forged"),
                },
                identity: ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(1),
                command: AppCommand::WorkspaceAdd {
                    root_path: std::env::temp_dir().to_string_lossy().into_owned(),
                },
            }))
            .await;
        let ServerFrame::Response(command_response) = client.recv().await else {
            panic!("expected command response");
        };
        assert!(matches!(command_response.response, AppResponse::Data(_)));

        // 查询：wire 伪造 Automation 身份，同样必须重写（query 同理）。
        client
            .send(&ClientFrame::Query(AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("q-forged"),
                source: CommandSource::Automation,
                identity: ActorIdentity::Automation {
                    name: "forged".into(),
                },
                issued_at: Timestamp::from_unix_millis(2),
                query: AppQuery::WorkspaceList,
            }))
            .await;
        let ServerFrame::Response(query_response) = client.recv().await else {
            panic!("expected query response");
        };
        assert!(matches!(query_response.response, AppResponse::Data(_)));

        let sources = harness.app_service.router().source_stats();
        assert_eq!(
            sources.get("local_gui"),
            Some(&2),
            "command+query 都必须盖戳为 LocalGui: {sources:?}"
        );
        assert!(
            !sources.contains_key("remote_gui"),
            "forged RemoteGui 不得透传"
        );
        assert!(
            !sources.contains_key("automation"),
            "forged Automation 不得透传"
        );
        let identities = harness.app_service.router().identity_stats();
        assert_eq!(
            identities.get("local_user:client-0"),
            Some(&2),
            "identity 必须为服务端派生的 LocalUser: {identities:?}"
        );
        assert!(!identities.contains_key("system"), "forged System 不得透传");
        assert!(
            !identities.contains_key("automation:forged"),
            "forged Automation 身份不得透传"
        );

        client.conn.close().await.expect("client close");
        session.close().await.expect("session close");
    }

    #[tokio::test]
    async fn gui_wire_cannot_inject_client_context() {
        let harness = harness("gui-context-deny").await;
        let (client, session) = open_session(&harness, "gui-context-deny").await;
        let response = handshake(&client, Some(authentication(&harness.token))).await;
        let HandshakeResponse::Accepted { client_id, .. } = response else {
            panic!("expected accepted handshake: {response:?}");
        };

        client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("cmd-ws"),
                source: CommandSource::LocalGui {
                    client_id: client_id.clone(),
                },
                identity: local_user(),
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(1),
                command: AppCommand::WorkspaceAdd {
                    root_path: std::env::temp_dir().to_string_lossy().into_owned(),
                },
            }))
            .await;
        let ServerFrame::Response(ws_response) = client.recv().await else {
            panic!("expected workspace response");
        };
        let workspace_id = match &ws_response.response {
            AppResponse::Data(value) => WorkspaceId::from(
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .expect("workspace id"),
            ),
            other => panic!("expected workspace data, got {other:?}"),
        };

        client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("cmd-session"),
                source: CommandSource::LocalGui {
                    client_id: client_id.clone(),
                },
                identity: local_user(),
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(2),
                command: AppCommand::SessionCreate {
                    workspace_id,
                    title: Some("gui-context".into()),
                },
            }))
            .await;
        let ServerFrame::Response(session_response) = client.recv().await else {
            panic!("expected session response");
        };
        let session_id = match &session_response.response {
            AppResponse::Data(value) => SessionId::from(
                value
                    .get("session_id")
                    .and_then(serde_json::Value::as_str)
                    .expect("session id"),
            ),
            other => panic!("expected session data, got {other:?}"),
        };

        client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("cmd-context"),
                source: CommandSource::LocalGui {
                    client_id: client_id.clone(),
                },
                identity: local_user(),
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(3),
                command: AppCommand::SessionClientContextReplace {
                    session_id: session_id.clone(),
                    snapshot: ClientContextSnapshot {
                        revision: 1,
                        active_document: None,
                        open_documents: vec![],
                        diagnostics: vec![],
                    },
                },
            }))
            .await;
        match client.recv().await {
            ServerFrame::Error(envelope) => {
                assert_eq!(envelope.request_id.as_deref(), Some("cmd-context"));
                assert_eq!(envelope.error.code, ProtocolErrorCode::PermissionDenied);
                assert!(!envelope.error.retryable);
            }
            other => panic!("expected permission denied error, got {other:?}"),
        }
        assert!(
            harness
                .app_service
                .router()
                .aggregate()
                .client_context(&session_id)
                .is_none(),
            "GUI must not persist client context"
        );

        client.conn.close().await.expect("client close");
        session.close().await.expect("session close");
    }

    #[tokio::test]
    async fn handshake_and_snapshot_request_are_scoped_to_connection_tenant() {
        use agent_domain::{PrincipalId, TenantId, WorkspaceId};

        let app_service = Arc::new(AppService::new("gui-tenant-test"));
        let aggregate = app_service.router().aggregate();
        let workspace_response = app_service.dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("tenant-workspace"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("tenant-seed"),
            },
            identity: local_user(),
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::WorkspaceAdd {
                root_path: std::env::temp_dir().to_string_lossy().into_owned(),
            },
        });
        let AppResponse::Data(workspace) = workspace_response.response else {
            panic!("workspace seed failed");
        };
        let workspace_id = WorkspaceId::from(workspace["id"].as_str().expect("workspace id"));
        let tenant_a =
            IdentityContext::new(TenantId::new("tenant-a"), PrincipalId::new("tenant-a:user"));
        let tenant_b =
            IdentityContext::new(TenantId::new("tenant-b"), PrincipalId::new("tenant-b:user"));
        aggregate
            .create_session_with_identity(
                workspace_id.clone(),
                "tenant-a-secret".into(),
                Timestamp::from_unix_millis(2),
                &tenant_a,
            )
            .expect("tenant-a session");
        aggregate
            .create_session_with_identity(
                workspace_id,
                "tenant-b-secret".into(),
                Timestamp::from_unix_millis(3),
                &tenant_b,
            )
            .expect("tenant-b session");

        let harness = tenant_harness("gui-tenant", app_service, tenant_a).await;
        let (client, session) = open_session(&harness, "gui-tenant").await;
        client
            .send(&ClientFrame::Handshake(HandshakeRequest {
                request_id: "tenant-handshake".into(),
                client_name: "tenant-gui".into(),
                client_version: "0.0.1".into(),
                supported_api_versions: vec![API_VERSION],
                capabilities: vec![GuiCapability::Snapshots],
                authentication: Some(authentication(&harness.token)),
            }))
            .await;
        assert!(matches!(
            client.recv().await,
            ServerFrame::Handshake(HandshakeResponse::Accepted { .. })
        ));
        let ServerFrame::Snapshot(initial) = client.recv().await else {
            panic!("expected initial tenant snapshot");
        };
        let initial = serde_json::to_string(&initial).expect("serialize initial");
        assert!(initial.contains("tenant-a-secret"));
        assert!(!initial.contains("tenant-b-secret"));

        client
            .send(&ClientFrame::SnapshotRequest {
                request_id: "tenant-snapshot".into(),
            })
            .await;
        let ServerFrame::Snapshot(requested) = client.recv().await else {
            panic!("expected requested tenant snapshot");
        };
        let requested = serde_json::to_string(&requested).expect("serialize requested");
        assert!(requested.contains("tenant-a-secret"));
        assert!(!requested.contains("tenant-b-secret"));

        client.conn.close().await.expect("client close");
        session.close().await.expect("session close");
    }

    /// 双租户共享 aggregate 播种：workspace + tenant-a / tenant-b 各一个
    /// session 与 run，返回 (workspace, session_a, session_b, run_a, run_b,
    /// tenant_a, tenant_b)。
    fn seed_dual_tenant(
        app_service: &AppService,
    ) -> (
        WorkspaceId,
        SessionId,
        SessionId,
        agent_domain::RunId,
        agent_domain::RunId,
        IdentityContext,
        IdentityContext,
    ) {
        use agent_domain::{PrincipalId, ProviderId, RunId, TenantId};

        let aggregate = app_service.router().aggregate();
        let workspace_response = app_service.dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("dual-tenant-workspace"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("tenant-seed"),
            },
            identity: local_user(),
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::WorkspaceAdd {
                root_path: std::env::temp_dir().to_string_lossy().into_owned(),
            },
        });
        let AppResponse::Data(workspace) = workspace_response.response else {
            panic!("workspace seed failed");
        };
        let workspace_id = WorkspaceId::from(workspace["id"].as_str().expect("workspace id"));
        let tenant_a =
            IdentityContext::new(TenantId::new("tenant-a"), PrincipalId::new("tenant-a:user"));
        let tenant_b =
            IdentityContext::new(TenantId::new("tenant-b"), PrincipalId::new("tenant-b:user"));
        let session_a = aggregate
            .create_session_with_identity(
                workspace_id.clone(),
                "tenant-a session".into(),
                Timestamp::from_unix_millis(2),
                &tenant_a,
            )
            .expect("tenant-a session");
        let session_b = aggregate
            .create_session_with_identity(
                workspace_id.clone(),
                "tenant-b session".into(),
                Timestamp::from_unix_millis(3),
                &tenant_b,
            )
            .expect("tenant-b session");
        let run_a = RunId::from("run-a");
        let run_b = RunId::from("run-b");
        aggregate
            .record_run_with_identity(
                run_a.clone(),
                session_a.session_id.clone(),
                agent_domain::ModelId::from("model"),
                ProviderId::from("provider"),
                CommandSource::Automation,
                Timestamp::from_unix_millis(4),
                &tenant_a,
            )
            .expect("tenant-a run");
        aggregate
            .record_run_with_identity(
                run_b.clone(),
                session_b.session_id.clone(),
                agent_domain::ModelId::from("model"),
                ProviderId::from("provider"),
                CommandSource::Automation,
                Timestamp::from_unix_millis(5),
                &tenant_b,
            )
            .expect("tenant-b run");
        (
            workspace_id,
            session_a.session_id,
            session_b.session_id.clone(),
            run_a,
            run_b,
            tenant_a,
            tenant_b,
        )
    }

    /// 租户测试连接握手：Accepted + 消费首帧 Snapshot。
    async fn tenant_handshake(client: &TestClient, token: &Token) {
        client
            .send(&ClientFrame::Handshake(HandshakeRequest {
                request_id: "tenant-hs".into(),
                client_name: "tenant-gui".into(),
                client_version: "0.0.1".into(),
                supported_api_versions: vec![API_VERSION],
                capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
                authentication: Some(authentication(token)),
            }))
            .await;
        assert!(matches!(
            client.recv().await,
            ServerFrame::Handshake(HandshakeResponse::Accepted { .. })
        ));
        assert!(
            matches!(client.recv().await, ServerFrame::Snapshot(_)),
            "首帧快照"
        );
    }

    /// 全量订阅（streams 为空）+ Heartbeat/Pong 往返，证明订阅已登记。
    async fn subscribe_all(client: &TestClient) {
        client
            .send(&ClientFrame::Subscribe(SubscribeRequest {
                request_id: "sub-all".into(),
                subscription_id: "sub-1".into(),
                streams: vec![],
            }))
            .await;
        client.send(&ClientFrame::Heartbeat { nonce: 1 }).await;
        assert_eq!(client.recv().await, ServerFrame::Pong { nonce: 1 });
    }

    /// 构造一条 Hub 事件（global_sequence 由 hub.publish 重写）。
    fn tenant_event(instance: &str, payload: core_api::AppEvent) -> core_api::AppEventEnvelope {
        core_api::AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from(instance),
            event_id: agent_domain::EventId::from("tenant-event"),
            global_sequence: core_api::GlobalSequence(0),
            stream: core_api::EventStream::Global,
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(10),
            source: core_api::EventSource::Core,
            payload,
        }
    }

    /// 读一帧（带超时，避免断言失败时挂死测试）。
    async fn recv_event(client: &TestClient) -> core_api::AppEventEnvelope {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), client.recv())
            .await
            .expect("event frame within timeout");
        match frame {
            ServerFrame::Event(envelope) => envelope,
            other => panic!("expected event frame, got {other:?}"),
        }
    }

    async fn assert_no_event(client: &TestClient, context: &str) {
        let missed =
            tokio::time::timeout(std::time::Duration::from_millis(300), client.recv()).await;
        assert!(
            missed.is_err(),
            "{context}: 不应收到事件，实际收到 {missed:?}"
        );
    }

    #[tokio::test]
    async fn command_and_query_stamping_carry_resolved_tenant_principal() {
        // P18-2 审查修复：custom tenant resolver 的 principal 必须经服务端
        // 盖戳到达 app-service（wire 伪造的 System / Automation 一律覆盖），
        // 并由宿主一致注入的 app-service resolver 还原 tenant 完成租户隔离。
        use agent_domain::{PrincipalId, TenantId};

        #[derive(Clone)]
        struct TenantAResolver;

        impl app_service::IdentityResolver for TenantAResolver {
            fn resolve(
                &self,
                principal: Option<&str>,
            ) -> Result<IdentityContext, app_service::IdentityError> {
                match principal {
                    Some("authenticated_client:tenant-a:user") => Ok(IdentityContext::new(
                        TenantId::new("tenant-a"),
                        PrincipalId::new("tenant-a:user"),
                    )),
                    Some(value) if !value.trim().is_empty() => Ok(IdentityContext::local()),
                    _ => Err(app_service::IdentityError::MissingIdentity(
                        "no principal".into(),
                    )),
                }
            }
        }

        let app_service = Arc::new(AppService::with_identity_resolver_and_tenant_policy(
            "gui-tenant-stamp",
            Arc::new(TenantAResolver),
            Arc::new(app_service::InMemoryTenantPolicyEngine::default()),
        ));
        let workspace_response = app_service.dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("stamp-workspace"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("stamp-seed"),
            },
            identity: local_user(),
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::WorkspaceAdd {
                root_path: std::env::temp_dir().to_string_lossy().into_owned(),
            },
        });
        let AppResponse::Data(workspace) = workspace_response.response else {
            panic!("workspace seed failed");
        };
        let workspace_id = WorkspaceId::from(workspace["id"].as_str().expect("workspace id"));
        let tenant_a =
            IdentityContext::new(TenantId::new("tenant-a"), PrincipalId::new("tenant-a:user"));

        let harness = tenant_harness("gui-stamp-tenant", Arc::clone(&app_service), tenant_a).await;
        let (client, session) = open_session(&harness, "gui-stamp-tenant").await;
        tenant_handshake(&client, &harness.token).await;

        // wire 伪造 System：服务端必须覆盖为 resolved principal 后派发。
        client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("stamp-session"),
                source: CommandSource::Automation,
                identity: ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(2),
                command: AppCommand::SessionCreate {
                    workspace_id,
                    title: Some("stamped".into()),
                },
            }))
            .await;
        let ServerFrame::Response(command_response) = client.recv().await else {
            panic!("expected command response");
        };
        let AppResponse::Data(value) = command_response.response else {
            panic!("expected session data: {:?}", command_response.response);
        };
        let session_id = SessionId::from(value["session_id"].as_str().expect("session id"));

        // wire 伪造 Automation：query 同样服务端盖戳。
        client
            .send(&ClientFrame::Query(AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("stamp-query"),
                source: CommandSource::Automation,
                identity: ActorIdentity::Automation {
                    name: "forged".into(),
                },
                issued_at: Timestamp::from_unix_millis(3),
                query: AppQuery::WorkspaceList,
            }))
            .await;
        let ServerFrame::Response(query_response) = client.recv().await else {
            panic!("expected query response");
        };
        assert!(matches!(query_response.response, AppResponse::Data(_)));

        let identities = app_service.router().identity_stats();
        assert_eq!(
            identities.get("authenticated_client:tenant-a:user"),
            Some(&2),
            "command+query 必须盖戳为 resolved principal: {identities:?}"
        );
        assert!(!identities.contains_key("system"), "wire System 不得透传");
        assert!(
            !identities.contains_key("automation:forged"),
            "wire Automation 不得透传"
        );
        let sources = app_service.router().source_stats();
        assert!(
            sources.get("local_gui").is_some_and(|count| *count >= 2),
            "source 仍按 locality 盖戳为 LocalGui: {sources:?}"
        );
        assert!(!sources.contains_key("automation"), "wire source 不得透传");

        // 关键：principal 到达 app-service 后还原为 tenant-a，session 落在
        // tenant-a，而非 local/default。
        assert!(
            app_service
                .router()
                .aggregate()
                .get_session(&session_id, &TenantId::new("tenant-a"))
                .is_some(),
            "session 必须归属 resolved tenant-a"
        );
        assert!(
            app_service
                .router()
                .aggregate()
                .get_session(&session_id, &TenantId::new("local/default"))
                .is_none(),
            "session 不得落入默认本地租户"
        );

        client.conn.close().await.expect("client close");
        session.close().await.expect("session close");
    }

    #[tokio::test]
    async fn dual_tenant_hub_realtime_events_are_tenant_filtered() {
        // P18-2 审查修复：共享 Hub 的实时事件必须按连接 tenant 过滤；
        // 无法从可信 aggregate 判定租户的事件 fail-closed 丢弃。
        use core_api::{AppEvent, RunState};

        let app_service = Arc::new(AppService::new("gui-tenant-realtime"));
        let (workspace_id, _session_a, session_b, run_a, run_b, tenant_a, tenant_b) =
            seed_dual_tenant(&app_service);
        let hub = Arc::new(EventHub::new());
        let publish_hub = Arc::clone(&hub);
        let harness_a = tenant_harness_with_hub(
            "gui-tenant-rt-a",
            Arc::clone(&app_service),
            tenant_a,
            Arc::clone(&hub),
        )
        .await;
        let harness_b =
            tenant_harness_with_hub("gui-tenant-rt-b", Arc::clone(&app_service), tenant_b, hub)
                .await;
        let (client_a, conn_a) = open_session(&harness_a, "gui-tenant-rt-a").await;
        let (client_b, conn_b) = open_session(&harness_b, "gui-tenant-rt-b").await;
        tenant_handshake(&client_a, &harness_a.token).await;
        tenant_handshake(&client_b, &harness_b.token).await;
        subscribe_all(&client_a).await;
        subscribe_all(&client_b).await;

        // tenant-a 的 Run 事件：A 收到，B 不得收到。
        publish_hub.publish(tenant_event(
            "gui-tenant-realtime",
            AppEvent::RunChanged {
                run_id: run_a.clone(),
                state: RunState::StreamingResponse,
            },
        ));
        let envelope = recv_event(&client_a).await;
        assert!(
            matches!(&envelope.payload, AppEvent::RunChanged { run_id, .. } if *run_id == run_a),
            "tenant-a 应收到自己的 Run 事件"
        );
        assert_no_event(&client_b, "tenant-b 不得收到 tenant-a 的 Run 事件").await;

        // tenant-b 的 Session 事件：B 收到，A 不得收到。
        publish_hub.publish(tenant_event(
            "gui-tenant-realtime",
            AppEvent::SessionChanged {
                session_id: session_b.clone(),
                revision: 1,
            },
        ));
        let envelope = recv_event(&client_b).await;
        assert!(
            matches!(&envelope.payload, AppEvent::SessionChanged { session_id, .. } if *session_id == session_b),
            "tenant-b 应收到自己的 Session 事件"
        );
        assert_no_event(&client_a, "tenant-a 不得收到 tenant-b 的 Session 事件").await;

        // 无法从 aggregate 判定租户的 Workspace 事件：双租户都 fail-closed 丢弃。
        publish_hub.publish(tenant_event(
            "gui-tenant-realtime",
            AppEvent::WorkspaceChanged {
                workspace_id: workspace_id.clone(),
                revision: 1,
            },
        ));
        assert_no_event(&client_a, "workspace 事件对 tenant-a fail-closed").await;
        assert_no_event(&client_b, "workspace 事件对 tenant-b fail-closed").await;

        // 收尾：再验证 tenant-b 自己的 Run 事件可见（B 收到、A 不收）。
        publish_hub.publish(tenant_event(
            "gui-tenant-realtime",
            AppEvent::RunChanged {
                run_id: run_b.clone(),
                state: RunState::StreamingResponse,
            },
        ));
        let envelope = recv_event(&client_b).await;
        assert!(
            matches!(&envelope.payload, AppEvent::RunChanged { run_id, .. } if *run_id == run_b),
            "tenant-b 应收到自己的 Run 事件"
        );
        assert_no_event(&client_a, "tenant-a 不得收到 tenant-b 的 Run 事件").await;

        client_a.conn.close().await.expect("close a");
        client_b.conn.close().await.expect("close b");
        conn_a.close().await.expect("session a close");
        conn_b.close().await.expect("session b close");
    }

    #[tokio::test]
    async fn dual_tenant_resume_replays_only_own_tenant_events() {
        // P18-2 审查修复：Resume 重放与实时同源过滤，禁止跨租户泄漏；
        // 无法判定的 Workspace 事件对非默认租户 fail-closed 丢弃。
        use core_api::{AppEvent, RunState};

        let app_service = Arc::new(AppService::new("gui-tenant-replay"));
        let (workspace_id, _session_a, session_b, run_a, run_b, tenant_a, tenant_b) =
            seed_dual_tenant(&app_service);
        let hub = Arc::new(EventHub::new());
        let publish_hub = Arc::clone(&hub);
        let harness_a = tenant_harness_with_hub(
            "gui-tenant-rp-a",
            Arc::clone(&app_service),
            tenant_a,
            Arc::clone(&hub),
        )
        .await;
        let harness_b =
            tenant_harness_with_hub("gui-tenant-rp-b", Arc::clone(&app_service), tenant_b, hub)
                .await;
        let (client_a, conn_a) = open_session(&harness_a, "gui-tenant-rp-a").await;
        let (client_b, conn_b) = open_session(&harness_b, "gui-tenant-rp-b").await;
        tenant_handshake(&client_a, &harness_a.token).await;
        tenant_handshake(&client_b, &harness_b.token).await;

        // 两个客户端都不订阅：重放独立于实时投递。
        publish_hub.publish(tenant_event(
            "gui-tenant-replay",
            AppEvent::RunChanged {
                run_id: run_a.clone(),
                state: RunState::Completed,
            },
        ));
        publish_hub.publish(tenant_event(
            "gui-tenant-replay",
            AppEvent::RunChanged {
                run_id: run_b.clone(),
                state: RunState::Completed,
            },
        ));
        publish_hub.publish(tenant_event(
            "gui-tenant-replay",
            AppEvent::WorkspaceChanged {
                workspace_id: workspace_id.clone(),
                revision: 1,
            },
        ));
        publish_hub.publish(tenant_event(
            "gui-tenant-replay",
            AppEvent::SessionChanged {
                session_id: session_b.clone(),
                revision: 1,
            },
        ));

        // tenant-a：Replay 1..4 → 仅 run-a 一条 Run 事件。
        client_a
            .send(&ClientFrame::Resume(ResumeRequest {
                request_id: "resume-a".into(),
                last_global_sequence: core_api::GlobalSequence(0),
            }))
            .await;
        let ServerFrame::Resume(resume) = client_a.recv().await else {
            panic!("expected resume response for tenant-a");
        };
        assert_eq!(resume.request_id, "resume-a");
        let gui_protocol::ResumeDisposition::Replay {
            from_sequence,
            through_sequence,
        } = resume.disposition
        else {
            panic!("expected replay disposition for tenant-a");
        };
        assert_eq!(from_sequence, core_api::GlobalSequence(1));
        assert_eq!(through_sequence, core_api::GlobalSequence(4));
        let envelope = recv_event(&client_a).await;
        assert!(
            matches!(&envelope.payload, AppEvent::RunChanged { run_id, .. } if *run_id == run_a),
            "tenant-a 重放只应包含自己的 Run 事件"
        );
        assert_no_event(&client_a, "tenant-a 重放不得包含 workspace/他租户事件").await;

        // tenant-b：Replay 1..4 → run-b 与 session-b 两条（workspace 被丢弃）。
        client_b
            .send(&ClientFrame::Resume(ResumeRequest {
                request_id: "resume-b".into(),
                last_global_sequence: core_api::GlobalSequence(0),
            }))
            .await;
        let ServerFrame::Resume(resume) = client_b.recv().await else {
            panic!("expected resume response for tenant-b");
        };
        let gui_protocol::ResumeDisposition::Replay {
            from_sequence,
            through_sequence,
        } = resume.disposition
        else {
            panic!("expected replay disposition for tenant-b");
        };
        assert_eq!(from_sequence, core_api::GlobalSequence(1));
        assert_eq!(through_sequence, core_api::GlobalSequence(4));
        let envelope = recv_event(&client_b).await;
        assert!(
            matches!(&envelope.payload, AppEvent::RunChanged { run_id, .. } if *run_id == run_b),
            "tenant-b 重放的第一条必须是自己的 Run 事件"
        );
        let envelope = recv_event(&client_b).await;
        assert!(
            matches!(&envelope.payload, AppEvent::SessionChanged { session_id, .. } if *session_id == session_b),
            "tenant-b 重放的第二条必须是自己的 Session 事件"
        );
        assert_no_event(&client_b, "tenant-b 重放不得包含 workspace 事件").await;

        client_a.conn.close().await.expect("close a");
        client_b.conn.close().await.expect("close b");
        conn_a.close().await.expect("session a close");
        conn_b.close().await.expect("session b close");
    }

    #[tokio::test]
    async fn handshake_rejected_on_incompatible_version_then_closed() {
        let harness = harness("gui-2").await;
        let (client, _session) = open_session(&harness, "gui-2").await;
        client
            .send(&ClientFrame::Handshake(HandshakeRequest {
                request_id: "hs-v2".into(),
                client_name: "test-gui".into(),
                client_version: "0.0.1".into(),
                supported_api_versions: vec![ApiVersion { major: 2, minor: 0 }],
                capabilities: vec![],
                authentication: Some(authentication(&harness.token)),
            }))
            .await;
        let response = match client.recv().await {
            ServerFrame::Handshake(response) => response,
            other => panic!("expected handshake response, got {other:?}"),
        };
        let HandshakeResponse::Rejected { error, .. } = response else {
            panic!("expected rejection: {response:?}");
        };
        assert_eq!(error.code, ProtocolErrorCode::IncompatibleVersion);
        let error = client
            .conn
            .receive()
            .await
            .expect_err("server should close after rejection");
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    }

    #[tokio::test]
    async fn handshake_rejected_on_auth_failure() {
        let harness = harness("gui-3").await;
        let (client, _session) = open_session(&harness, "gui-3").await;
        let response = handshake(
            &client,
            Some(ClientAuthentication {
                scheme: client_auth::TOKEN_SCHEME.into(),
                proof: "wrong-token".into(),
            }),
        )
        .await;
        let HandshakeResponse::Rejected { error, .. } = response else {
            panic!("expected rejection: {response:?}");
        };
        assert_eq!(error.code, ProtocolErrorCode::AuthenticationFailed);

        // 缺失 authentication 同样拒绝。
        let (client2, _session2) = open_session(&harness, "gui-3").await;
        let response = handshake(&client2, None).await;
        let HandshakeResponse::Rejected { error, .. } = response else {
            panic!("expected rejection: {response:?}");
        };
        assert_eq!(error.code, ProtocolErrorCode::AuthenticationFailed);
    }

    #[tokio::test]
    async fn heartbeat_gets_pong() {
        let harness = harness("gui-4").await;
        let (client, _session) = open_session(&harness, "gui-4").await;
        let _ = handshake(&client, Some(authentication(&harness.token))).await;
        client.send(&ClientFrame::Heartbeat { nonce: 42 }).await;
        assert_eq!(client.recv().await, ServerFrame::Pong { nonce: 42 });
    }

    #[tokio::test]
    async fn artifact_read_is_chunked_at_64kib_with_eof() {
        // P13-8：真实 payload 来自 store.put 的 blob，aggregate 以 BlobId 登记。
        let content = (0..200 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<u8>>();
        let temp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            ArtifactStore::open(temp.path().join("store"))
                .await
                .expect("open store"),
        );
        let outcome = store.put(&content).await.expect("put blob");
        let artifact_id = ArtifactId::from(outcome.id.as_str());
        let harness = harness_with("gui-5", None, Some(Arc::clone(&store))).await;
        harness
            .app_service
            .router()
            .aggregate()
            .put_artifact(
                artifact_id.clone(),
                content.len() as u64,
                "text/plain".into(),
            )
            .expect("put artifact");
        let (client, _session) = open_session(&harness, "gui-5").await;
        let _ = handshake(&client, Some(authentication(&harness.token))).await;

        client
            .send(&ClientFrame::ArtifactRead(ArtifactReadRequest {
                request_id: "ar-1".into(),
                artifact_id: artifact_id.clone(),
                offset: 0,
                limit: 0,
            }))
            .await;
        let mut chunks = Vec::new();
        for _ in 0..4 {
            match client.recv().await {
                ServerFrame::ArtifactChunk(chunk) => chunks.push(chunk),
                other => panic!("expected artifact chunk, got {other:?}"),
            }
        }
        assert_eq!(chunks[0].offset, 0);
        assert_eq!(chunks[1].offset, 64 * 1024);
        assert_eq!(chunks[2].offset, 128 * 1024);
        assert_eq!(chunks[3].offset, 192 * 1024);
        assert!(!chunks[0].eof && !chunks[1].eof && !chunks[2].eof);
        assert!(chunks[3].eof);
        assert_eq!(chunks[3].request_id, "ar-1");
        assert_eq!(chunks[3].artifact_id, artifact_id);
        let assembled = chunks
            .iter()
            .flat_map(|chunk| chunk.data.iter().copied())
            .collect::<Vec<u8>>();
        assert_eq!(assembled, content, "分片重组必须等于 store 中的原始内容");
    }

    #[tokio::test]
    async fn artifact_read_partial_range_and_missing_artifact() {
        let content = (0..200 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<u8>>();
        let temp = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(
            ArtifactStore::open(temp.path().join("store"))
                .await
                .expect("open store"),
        );
        let outcome = store.put(&content).await.expect("put blob");
        let artifact_id = ArtifactId::from(outcome.id.as_str());
        let harness = harness_with("gui-6", None, Some(Arc::clone(&store))).await;
        harness
            .app_service
            .router()
            .aggregate()
            .put_artifact(
                artifact_id.clone(),
                content.len() as u64,
                "text/plain".into(),
            )
            .expect("put artifact");
        let (client, _session) = open_session(&harness, "gui-6").await;
        let _ = handshake(&client, Some(authentication(&harness.token))).await;

        client
            .send(&ClientFrame::ArtifactRead(ArtifactReadRequest {
                request_id: "ar-2".into(),
                artifact_id: artifact_id.clone(),
                offset: 64 * 1024,
                limit: 70 * 1024,
            }))
            .await;
        let first = match client.recv().await {
            ServerFrame::ArtifactChunk(chunk) => chunk,
            other => panic!("expected artifact chunk, got {other:?}"),
        };
        let second = match client.recv().await {
            ServerFrame::ArtifactChunk(chunk) => chunk,
            other => panic!("expected artifact chunk, got {other:?}"),
        };
        assert_eq!(first.offset, 64 * 1024);
        assert!(!first.eof);
        assert_eq!(second.offset, 128 * 1024);
        assert!(second.eof);
        let assembled = first
            .data
            .iter()
            .chain(second.data.iter())
            .copied()
            .collect::<Vec<u8>>();
        assert_eq!(assembled, content[64 * 1024..64 * 1024 + 70 * 1024]);

        client
            .send(&ClientFrame::ArtifactRead(ArtifactReadRequest {
                request_id: "ar-missing".into(),
                artifact_id: ArtifactId::from("art-missing"),
                offset: 0,
                limit: 0,
            }))
            .await;
        let ServerFrame::Error(envelope) = client.recv().await else {
            panic!("expected error for missing artifact");
        };
        assert_eq!(envelope.error.code, ProtocolErrorCode::RequestNotFound);
        assert_eq!(envelope.request_id.as_deref(), Some("ar-missing"));
    }

    #[tokio::test]
    async fn subscribe_resume_snapshot_and_ack_are_wired() {
        let harness = harness("gui-7").await;
        let (client, _session) = open_session(&harness, "gui-7").await;
        let _ = handshake(&client, Some(authentication(&harness.token))).await;

        // SnapshotRequest → 完整 Snapshot。
        client
            .send(&ClientFrame::SnapshotRequest {
                request_id: "snap-req".into(),
            })
            .await;
        let ServerFrame::Snapshot(snapshot) = client.recv().await else {
            panic!("expected snapshot");
        };
        assert_eq!(snapshot.instance_id.as_str(), "gui-server-test");

        // Subscribe 无回复：随后 Heartbeat 的 Pong 顺序到达，证明中间无错误帧。
        client
            .send(&ClientFrame::Subscribe(SubscribeRequest {
                request_id: "sub-req".into(),
                subscription_id: "sub-1".into(),
                streams: vec![],
            }))
            .await;
        client.send(&ClientFrame::Heartbeat { nonce: 1 }).await;
        assert_eq!(client.recv().await, ServerFrame::Pong { nonce: 1 });

        // Ack 无回复：同样以 Pong 顺序证明。
        client
            .send(&ClientFrame::Ack {
                global_sequence: core_api::GlobalSequence(3),
            })
            .await;
        client.send(&ClientFrame::Heartbeat { nonce: 2 }).await;
        assert_eq!(client.recv().await, ServerFrame::Pong { nonce: 2 });

        // Resume：空 Hub（current=0）→ UpToDate，仅回 ResumeResponse。
        client
            .send(&ClientFrame::Resume(ResumeRequest {
                request_id: "resume-req".into(),
                last_global_sequence: core_api::GlobalSequence(0),
            }))
            .await;
        let ServerFrame::Resume(resume) = client.recv().await else {
            panic!("expected resume response");
        };
        assert_eq!(resume.request_id, "resume-req");
        assert!(matches!(
            resume.disposition,
            gui_protocol::ResumeDisposition::UpToDate { .. }
        ));
    }

    #[tokio::test]
    async fn non_handshake_first_frame_is_rejected_and_closed() {
        let harness = harness("gui-8").await;
        let (client, _session) = open_session(&harness, "gui-8").await;
        client.send(&ClientFrame::Heartbeat { nonce: 1 }).await;
        let ServerFrame::Error(envelope) = client.recv().await else {
            panic!("expected error");
        };
        assert_eq!(envelope.error.code, ProtocolErrorCode::InvalidFrame);
        let error = client
            .conn
            .receive()
            .await
            .expect_err("server should close");
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    }

    async fn e2e_exchange(conn: &dyn GuiConnection, token: &Token, query: AppQuery) -> ServerFrame {
        conn.send(TransportFrame::new(
            encode_client_frame(&ClientFrame::Handshake(HandshakeRequest {
                request_id: "hs-e2e".into(),
                client_name: "e2e-gui".into(),
                client_version: "0.0.1".into(),
                supported_api_versions: vec![API_VERSION],
                capabilities: vec![],
                authentication: Some(authentication(token)),
            }))
            .expect("encode handshake"),
        ))
        .await
        .expect("send handshake");
        let frame = decode_server_frame(conn.receive().await.expect("receive").as_bytes())
            .expect("decode handshake response");
        assert!(matches!(
            frame,
            ServerFrame::Handshake(HandshakeResponse::Accepted { .. })
        ));
        // P13-5：握手后服务端先发 Snapshot，query 前先消费掉。
        let snapshot = decode_server_frame(conn.receive().await.expect("receive").as_bytes())
            .expect("decode initial snapshot");
        assert!(matches!(snapshot, ServerFrame::Snapshot(_)));

        conn.send(TransportFrame::new(
            encode_client_frame(&ClientFrame::Query(AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("q-e2e"),
                source: CommandSource::LocalGui {
                    client_id: GuiClientId::from("e2e"),
                },
                identity: local_user(),
                issued_at: Timestamp::from_unix_millis(3),
                query,
            }))
            .expect("encode query"),
        ))
        .await
        .expect("send query");
        decode_server_frame(conn.receive().await.expect("receive").as_bytes())
            .expect("decode query response")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn end_to_end_over_unix_socket() {
        use transport_local::LocalTransport;

        let temp = tempfile::tempdir().expect("tempdir");
        let socket = temp.path().join("gui.sock");
        let app_service = Arc::new(AppService::new("gui-e2e"));
        let token_store = TokenStore::new(temp.path().join("e2e.token"));
        let token = token_store.generate().expect("generate token");
        let handshake = HandshakeService::new(
            CoreInstanceId::from("e2e-instance"),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![GuiCapability::Events],
        )
        .with_authenticator(Box::new(TokenAuthenticator::new(token_store)));
        let transport = Arc::new(LocalTransport::default());
        let server = GuiServer::new(GuiServerConfig {
            app_service,
            handshake,
            transport: transport.clone(),
            hub: Arc::new(EventHub::new()),
            connections: None,
        });
        let listener = server
            .bind(TransportEndpoint::Local {
                address: socket.to_string_lossy().into_owned(),
            })
            .await
            .expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let conn = transport
            .connect(
                TransportEndpoint::Local {
                    address: socket.to_string_lossy().into_owned(),
                },
                ConnectOptions {
                    timeout_ms: 5_000,
                    client_label: None,
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
            .expect("connect");
        let _session = accept.await.expect("accept task").expect("accept");

        let frame = e2e_exchange(conn.as_ref(), &token, AppQuery::WorkspaceList).await;
        assert!(matches!(
            frame,
            ServerFrame::Response(envelope)
                if matches!(envelope.response, AppResponse::Data(_))
        ));
        conn.close().await.expect("close");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn end_to_end_over_named_pipe() {
        use transport_local::LocalTransport;

        let temp = tempfile::tempdir().expect("tempdir");
        let app_service = Arc::new(AppService::new("gui-e2e"));
        let token_store = TokenStore::new(temp.path().join("e2e.token"));
        let token = token_store.generate().expect("generate token");
        let handshake = HandshakeService::new(
            CoreInstanceId::from("e2e-instance"),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![GuiCapability::Events],
        )
        .with_authenticator(Box::new(TokenAuthenticator::new(token_store)));
        let transport = Arc::new(LocalTransport::default());
        let server = GuiServer::new(GuiServerConfig {
            app_service,
            handshake,
            transport: transport.clone(),
            hub: Arc::new(EventHub::new()),
            connections: None,
        });
        let address = format!(
            "pawork-gui-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let listener = server
            .bind(TransportEndpoint::Local {
                address: address.clone(),
            })
            .await
            .expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let conn = transport
            .connect(
                TransportEndpoint::Local {
                    address: address.clone(),
                },
                ConnectOptions {
                    timeout_ms: 5_000,
                    client_label: None,
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
            .expect("connect");
        let _session = accept.await.expect("accept task").expect("accept");

        let frame = e2e_exchange(conn.as_ref(), &token, AppQuery::WorkspaceList).await;
        assert!(matches!(
            frame,
            ServerFrame::Response(envelope)
                if matches!(envelope.response, AppResponse::Data(_))
        ));
        conn.close().await.expect("close");
    }
}
