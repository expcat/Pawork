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
use app_service::AppService;
use async_trait::async_trait;
use connection_manager::ConnectionManager;
use gui_protocol::HandshakeService;
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
}

/// CLI 进程内的 GUI 协议服务器（可廉价克隆，共享同一配置）。
#[derive(Clone)]
pub struct GuiServer {
    inner: Arc<Inner>,
    transport: Arc<dyn GuiTransportServer>,
}

impl GuiServer {
    pub fn new(config: GuiServerConfig) -> Self {
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
        ActorId, ArtifactId, CommandId, CoreInstanceId, GuiClientId, QueryId, Timestamp,
    };
    use artifact_store::ArtifactStore;
    use client_auth::{Token, TokenAuthenticator, TokenStore};
    use core_api::{
        ActorIdentity, ApiVersion, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope,
        AppResponse, CommandSource, API_VERSION, SUPPORTED_API_VERSIONS,
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
