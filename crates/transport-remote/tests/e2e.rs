//! P17-11 定向测试：真实 loopback TCP + TLS 1.3 上的全链路。
//!
//! 覆盖：发布 → 认证（含拒绝与 Secret 不泄漏）→ 加密真实性（抓线上字节
//! 断言无明文）→ 握手 / command / event 往返 → 断线按会话有界续传 /
//! 快照信号 → unpublish / revoke 生命周期 → 帧上限 → 并发双客户端会话隔离
//! → 恶意 ACK（摘要不符 / 从未发送 / 跨会话）→ 端点独立凭证与撤销 → 按
//! 字节 + 帧双重有界的缓冲内存。

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use agent_domain::{
    ActorId, CommandId, CoreInstanceId, EventId, GuiClientId, QueryId, RunId, Timestamp,
};
use app_service::AppService;
use client_auth::{Token, TokenAuthenticator, TokenStore, TOKEN_SCHEME};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery,
    AppQueryEnvelope, AppResponse, CommandSource, EventSource, EventStream, GlobalSequence,
    RunState, API_VERSION, SUPPORTED_API_VERSIONS,
};
use gui_protocol::{
    decode_server_frame, encode_client_frame, encode_server_frame, ClientAuthentication,
    ClientFrame, GuiCapability, HandshakeRequest, HandshakeResponse, HandshakeService, ServerFrame,
};
use gui_server::{GuiServer, GuiServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{version, ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use subscription_hub::EventHub;
use tempfile::TempDir;
use tokio::io::AsyncWrite;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::TlsConnector;

use transport_remote::{
    ConnectOptions, ConnectionLocality, GuiConnection, GuiListener, GuiTransportServer,
    RealRemoteConnector, RealRemoteTransport, RealRemoteTransportConfig,
    RealRemoteTransportProvider, RemoteGuiConnector, RemoteGuiTransportProvider,
    RemotePublishHandle, RemotePublishRequest, ResumeOutcome, TransportEndpoint,
    TransportErrorKind, TransportFrame,
};

fn options(max_frame_bytes: u64) -> ConnectOptions {
    options_with_label(max_frame_bytes, "remote-e2e-gui")
}

fn options_with_label(max_frame_bytes: u64, label: &str) -> ConnectOptions {
    ConnectOptions {
        timeout_ms: 5_000,
        client_label: Some(label.into()),
        max_frame_bytes,
    }
}

/// 默认 e2e 客户端会话 label（与服务端诊断查询一致）。
const E2E_LABEL: &str = "remote-e2e-gui";

fn authentication(token: &Token) -> ClientAuthentication {
    ClientAuthentication {
        scheme: TOKEN_SCHEME.into(),
        proof: token.as_str().into(),
    }
}

/// 等待条件成立（轮询间隔 10ms，上限 3s）。
async fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within 3s"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

struct Harness {
    _temp: TempDir,
    token_store: TokenStore,
    token: Token,
    transport: Arc<RealRemoteTransport>,
}

impl Harness {
    fn new(configure: impl FnOnce(&mut RealRemoteTransportConfig)) -> Self {
        let temp = TempDir::new().expect("tempdir");
        let token_store = TokenStore::new(temp.path().join("remote.token"));
        let token = token_store.generate().expect("generate token");
        let mut config = RealRemoteTransportConfig::new(token_store.clone(), None);
        configure(&mut config);
        let transport = Arc::new(RealRemoteTransport::new(config));
        Self {
            _temp: temp,
            token_store,
            token,
            transport,
        }
    }

    fn connector(&self) -> RealRemoteConnector {
        // `None`：按端点解析其独立凭证（同一进程发布 + 连接）。
        RealRemoteConnector::new(Arc::clone(&self.transport), None)
    }

    fn connector_with(&self, token: client_auth::Token) -> RealRemoteConnector {
        RealRemoteConnector::new(Arc::clone(&self.transport), Some(token))
    }
}

async fn publish_and_bind(
    harness: &Harness,
    name: &str,
) -> (RemotePublishHandle, Box<dyn GuiListener>) {
    let provider = RealRemoteTransportProvider::new(Arc::clone(&harness.transport));
    assert_eq!(provider.describe().adapter, "remote");
    let handle = provider
        .publish(RemotePublishRequest { name: name.into() })
        .await
        .expect("publish");
    let listener = harness
        .transport
        .bind(handle.endpoint.clone())
        .await
        .expect("bind");
    (handle, listener)
}

// ---------- 1. 全链路：publish → connect → 握手 → command → event ----------

#[tokio::test]
async fn publish_connect_handshake_command_and_event_end_to_end() {
    let harness = Harness::new(|_| {});
    let temp = TempDir::new().expect("tempdir");
    let app_service = Arc::new(AppService::new("remote-e2e"));
    let handshake = HandshakeService::new(
        CoreInstanceId::from("remote-instance"),
        SUPPORTED_API_VERSIONS.to_vec(),
        vec![GuiCapability::Events],
    )
    .with_authenticator(Box::new(TokenAuthenticator::new(
        harness.token_store.clone(),
    )));
    let server = GuiServer::new(GuiServerConfig {
        app_service: app_service.clone(),
        handshake,
        transport: Arc::clone(&harness.transport) as Arc<dyn GuiTransportServer>,
        hub: Arc::new(EventHub::new()),
        connections: None,
    });

    let provider = RealRemoteTransportProvider::new(Arc::clone(&harness.transport));
    let handle = provider
        .publish(RemotePublishRequest { name: "e2e".into() })
        .await
        .expect("publish");
    let TransportEndpoint::Remote { address, adapter } = &handle.endpoint else {
        panic!("expected remote endpoint, got {:?}", handle.endpoint);
    };
    assert_eq!(adapter, "remote");
    assert!(address.starts_with("real://e2e-"));

    let listener = server.bind(handle.endpoint.clone()).await.expect("bind");
    let accept = tokio::spawn(async move { listener.accept().await });

    let connector = harness.connector();
    let conn = connector
        .connect(
            &handle.endpoint,
            ConnectOptions {
                timeout_ms: 5_000,
                client_label: Some("remote-e2e-gui".into()),
                max_frame_bytes: 1024 * 1024,
            },
        )
        .await
        .expect("connect");
    assert_eq!(conn.info().locality, ConnectionLocality::Remote);
    assert_eq!(conn.info().peer_label.as_deref(), Some("remote-e2e-gui"));
    assert!(conn.info().encrypted, "connection must be TLS-encrypted");
    let session = accept.await.expect("accept task").expect("accept");
    assert!(session.info().encrypted);
    assert_eq!(session.info().peer_label.as_deref(), Some("remote-e2e-gui"));

    // 握手（与本地 GUI 完全相同的协议帧）。
    conn.send(TransportFrame::new(
        encode_client_frame(&ClientFrame::Handshake(HandshakeRequest {
            request_id: "hs-remote".into(),
            client_name: "e2e-gui".into(),
            client_version: "0.0.1".into(),
            supported_api_versions: vec![API_VERSION],
            capabilities: vec![GuiCapability::Events],
            authentication: Some(authentication(&harness.token)),
        }))
        .expect("encode handshake"),
    ))
    .await
    .expect("send handshake");
    let handshake_response =
        match decode_server_frame(conn.receive().await.expect("receive").as_bytes())
            .expect("decode handshake response")
        {
            ServerFrame::Handshake(response) => response,
            other => panic!("expected handshake response, got {other:?}"),
        };
    let HandshakeResponse::Accepted {
        selected_api_version,
        ..
    } = handshake_response
    else {
        panic!("expected accepted handshake");
    };
    assert_eq!(selected_api_version, API_VERSION);

    // 握手成功后服务端先发 Snapshot。
    match decode_server_frame(conn.receive().await.expect("receive").as_bytes())
        .expect("decode snapshot")
    {
        ServerFrame::Snapshot(snapshot) => {
            assert_eq!(snapshot.instance_id.as_str(), "remote-e2e");
        }
        other => panic!("expected snapshot after handshake, got {other:?}"),
    }

    // Command 往返。
    conn.send(TransportFrame::new(
        encode_client_frame(&ClientFrame::Command(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("remote-cmd-1"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("remote-e2e"),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("remote-e2e-user"),
                display_name: None,
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::WorkspaceAdd {
                root_path: temp.path().to_string_lossy().into_owned(),
            },
        }))
        .expect("encode command"),
    ))
    .await
    .expect("send command");
    let command_response =
        match decode_server_frame(conn.receive().await.expect("receive").as_bytes())
            .expect("decode command response")
        {
            ServerFrame::Response(response) => response,
            other => panic!("expected command response, got {other:?}"),
        };
    assert!(matches!(command_response.response, AppResponse::Data(_)));

    // Event 收到：宿主侧推送事件帧，经真实 TCP + TLS 到达 GUI。
    session
        .send(TransportFrame::new(
            encode_server_frame(&ServerFrame::Event(AppEventEnvelope {
                api_version: API_VERSION,
                instance_id: CoreInstanceId::from("remote-instance"),
                event_id: EventId::from("remote-event-1"),
                global_sequence: GlobalSequence(1),
                stream: EventStream::Run(RunId::from("remote-run-1")),
                stream_sequence: 1,
                timestamp: Timestamp::from_unix_millis(2),
                source: EventSource::Core,
                payload: AppEvent::RunChanged {
                    run_id: RunId::from("remote-run-1"),
                    state: RunState::StreamingResponse,
                },
            }))
            .expect("encode event"),
        ))
        .await
        .expect("host push event");
    let received = decode_server_frame(conn.receive().await.expect("receive").as_bytes())
        .expect("decode event");
    assert!(
        matches!(
            received,
            ServerFrame::Event(ref envelope)
                if matches!(envelope.payload, AppEvent::RunChanged { ref run_id, .. } if run_id.as_str() == "remote-run-1")
        ),
        "expected run event, got {received:?}"
    );

    // Query 仍可往返。
    conn.send(TransportFrame::new(
        encode_client_frame(&ClientFrame::Query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from("remote-q-1"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("remote-e2e"),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("remote-e2e-user"),
                display_name: None,
            },
            issued_at: Timestamp::from_unix_millis(3),
            query: AppQuery::WorkspaceList,
        }))
        .expect("encode query"),
    ))
    .await
    .expect("send query");
    let query_response =
        match decode_server_frame(conn.receive().await.expect("receive").as_bytes())
            .expect("decode query response")
        {
            ServerFrame::Response(response) => response,
            other => panic!("expected query response, got {other:?}"),
        };
    assert!(matches!(query_response.response, AppResponse::Data(_)));

    conn.close().await.expect("client close");
    session.close().await.expect("session close");
}

// ---------- 2. 认证拒绝与 Secret 不泄漏 ----------

#[tokio::test]
async fn bad_token_is_rejected_without_leaking_secret() {
    let harness = Harness::new(|_| {});
    let (handle, listener) = publish_and_bind(&harness, "auth").await;
    let accept = tokio::spawn(async move { listener.accept().await });

    // 伪造凭证：另一个 token 文件生成的 token。
    let temp = TempDir::new().expect("tempdir");
    let wrong_store = TokenStore::new(temp.path().join("wrong.token"));
    let wrong_token = wrong_store.generate().expect("generate wrong token");
    let wrong_connector = harness.connector_with(wrong_token.clone());
    let error = match wrong_connector
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("connect with bad token must fail"),
    };
    assert_eq!(error.kind, TransportErrorKind::AuthenticationFailed);
    assert!(
        !error.message.contains(wrong_token.as_str()),
        "error must not contain the secret: {:?}",
        error.message
    );
    assert!(
        !error.message.contains(harness.token.as_str()),
        "error must not contain the real secret: {:?}",
        error.message
    );
    assert!(
        !format!("{error:?}").contains(wrong_token.as_str()),
        "debug output must not contain the secret"
    );

    // 服务端侧 accept 同步返回认证失败。
    let server_error = match accept.await.expect("accept task") {
        Err(error) => error,
        Ok(_) => panic!("server must reject the bad token"),
    };
    assert_eq!(server_error.kind, TransportErrorKind::AuthenticationFailed);
    assert!(
        !server_error.message.contains(wrong_token.as_str()),
        "server error must not contain the secret"
    );

    // 正确凭证仍可连接（认证失败不破坏端点）。
    let listener = harness
        .transport
        .bind(handle.endpoint.clone())
        .await
        .expect("bind again");
    let accept = tokio::spawn(async move { listener.accept().await });
    let conn = harness
        .connector()
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("connect with good token");
    let server_conn = accept.await.expect("accept task").expect("accept");
    conn.close().await.expect("client close");
    server_conn.close().await.expect("server close");
}

// ---------- 3. 加密真实性：线上字节不含明文 ----------

/// 记录流经字节的 TCP 代理（loopback 中间人视角）。
struct RecordingProxy {
    recorded: Arc<Mutex<Vec<u8>>>,
    port: u16,
}

impl RecordingProxy {
    async fn start(endpoint_addr: (String, u16)) -> Self {
        let proxy = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("proxy bind");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let port = proxy.local_addr().expect("proxy addr").port();
        let (listen, rec) = (proxy, Arc::clone(&recorded));
        let target = endpoint_addr.clone();
        tokio::spawn(async move {
            loop {
                let (client, _) = match listen.accept().await {
                    Ok(accepted) => accepted,
                    Err(_) => break,
                };
                let endpoint = match TcpStream::connect(&target).await {
                    Ok(stream) => stream,
                    Err(_) => continue,
                };
                let rec = Arc::clone(&rec);
                tokio::spawn(async move {
                    let (mut client_read, client_write) = client.into_split();
                    let (mut endpoint_read, endpoint_write) = endpoint.into_split();
                    // 客户端 → 端点（记录上行）
                    let mut up_writer = RecordingWriter::new(endpoint_write, Arc::clone(&rec));
                    let up = tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut client_read, &mut up_writer).await;
                    });
                    // 端点 → 客户端（记录下行）
                    let mut down_writer = RecordingWriter::new(client_write, rec);
                    let down = tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut endpoint_read, &mut down_writer).await;
                    });
                    let _ = tokio::join!(up, down);
                });
            }
        });
        Self { recorded, port }
    }

    fn port(&self) -> u16 {
        self.port
    }
}

/// 转发写字节并记录到共享缓冲。
struct RecordingWriter<W> {
    inner: W,
    recorded: Arc<Mutex<Vec<u8>>>,
}

impl<W> RecordingWriter<W> {
    fn new(inner: W, recorded: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { inner, recorded }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for RecordingWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(n)) => {
                self.recorded
                    .lock()
                    .expect("record lock")
                    .extend_from_slice(&buf[..n]);
                Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn wire_bytes_are_tls_encrypted_and_never_plaintext() {
    let harness = Harness::new(|_| {});
    let (handle, listener) = publish_and_bind(&harness, "wire").await;
    let accept = tokio::spawn(async move { listener.accept().await });

    let TransportEndpoint::Remote { address, .. } = &handle.endpoint else {
        panic!("expected remote endpoint");
    };
    let parsed = address
        .strip_prefix("real://")
        .expect("prefix")
        .split_once('?')
        .expect("query");
    let (id, rest) = parsed;
    let (query, _fragment) = rest.split_once('#').expect("fragment");

    // 经代理改端口，抓取真实线上字节。
    let (host, port) = {
        let after = address.rsplit_once('#').expect("tcp fragment").1;
        let tcp = after.strip_prefix("tcp=").expect("tcp=");
        let (host, port) = tcp.rsplit_once(':').expect("host:port");
        (host.to_string(), port.parse::<u16>().expect("port"))
    };
    let proxy = RecordingProxy::start((host, port)).await;
    let proxied_address = format!("real://{id}?{query}#tcp=127.0.0.1:{}", proxy.port());
    let proxied_endpoint = TransportEndpoint::Remote {
        address: proxied_address,
        adapter: "remote".into(),
    };

    // 经代理改端口后，按地址自动解析凭证不再适用（地址键不同），改用端点
    // 独立凭证显式连接；TLS 指纹仍来自地址 query，代理透明转发不影响。
    let token = harness
        .transport
        .endpoint_token(address)
        .expect("endpoint token");
    let conn = harness
        .connector_with(token.clone())
        .connect(&proxied_endpoint, options(1024 * 1024))
        .await
        .expect("connect through proxy");
    assert!(conn.info().encrypted);
    let server_conn = accept.await.expect("accept task").expect("accept");

    // 发送一个可识别的明文哨兵，走完整 TLS。
    let sentinel = b"PLAINTEXT-SENTINEL-7f3a9c";
    conn.send(TransportFrame::new(sentinel.to_vec()))
        .await
        .expect("send sentinel");
    let received = server_conn.receive().await.expect("server receive");
    assert_eq!(received.as_bytes(), sentinel);

    // 让下行也经过代理（会话层推送一帧回来）。
    server_conn
        .send(TransportFrame::new(sentinel.to_vec()))
        .await
        .expect("server send sentinel");
    let echoed = conn.receive().await.expect("client receive");
    assert_eq!(echoed.as_bytes(), sentinel);

    // 线上字节不得包含：明文哨兵、信封魔数（"PW" LE）、token。
    let recorded = proxy.recorded.lock().expect("record lock").clone();
    assert!(
        !recorded
            .windows(sentinel.len())
            .any(|window| window == sentinel),
        "plaintext payload leaked onto the wire"
    );
    let magic = 0x5057u16.to_le_bytes();
    assert!(
        !recorded.windows(2).any(|window| window == magic),
        "envelope magic leaked onto the wire"
    );
    assert!(
        !recorded
            .windows(token.as_str().len())
            .any(|window| window == token.as_str().as_bytes()),
        "authentication secret leaked onto the wire"
    );
    assert!(!recorded.is_empty(), "proxy must have observed traffic");

    conn.close().await.expect("client close");
    server_conn.close().await.expect("server close");
}

// ---------- 4. 断线续传：窗口内有序补发 ----------

#[tokio::test]
async fn reconnect_resumes_unacked_frames_in_order() {
    let harness = Harness::new(|_| {});
    let (handle, listener) = publish_and_bind(&harness, "resume").await;
    let address = match &handle.endpoint {
        TransportEndpoint::Remote { address, .. } => address.clone(),
        _ => panic!("expected remote endpoint"),
    };

    // 第一次连接。
    let accept = tokio::spawn(async move { listener.accept().await });
    let connector = harness.connector();
    let client1 = connector
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("first connect");
    let server1 = accept.await.expect("accept task").expect("accept");

    // 服务端推送 6 帧；客户端只消费 1..3（4..6 已读入队列但未交付）。
    for seq in 1..=6u64 {
        server1
            .send(TransportFrame::new(format!("frame-{seq}").into_bytes()))
            .await
            .expect("server send");
    }
    for expected in 1..=3u64 {
        let frame = client1.receive().await.expect("client receive");
        assert_eq!(frame.as_bytes(), format!("frame-{expected}").as_bytes());
    }
    wait_until(|| harness.transport.acked_sequence(&address, E2E_LABEL) == Some(3)).await;
    assert_eq!(
        harness.transport.buffered_frames(&address, E2E_LABEL),
        Some(3)
    );
    client1.close().await.expect("client close");

    // 重连：续传结论 ResumedFrom(4)，并按序补发 4..6。
    let listener = harness
        .transport
        .bind(handle.endpoint.clone())
        .await
        .expect("bind again");
    let accept = tokio::spawn(async move { listener.accept().await });
    let client2 = connector
        .connect_typed(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("reconnect");
    let server2 = accept.await.expect("accept task").expect("accept");
    assert_eq!(client2.resume_outcome(), ResumeOutcome::ResumedFrom(4));

    for expected in 4..=6u64 {
        let frame = client2.receive().await.expect("replayed frame");
        assert_eq!(frame.as_bytes(), format!("frame-{expected}").as_bytes());
    }
    // 补发后新帧继续有序。
    server2
        .send(TransportFrame::new(b"frame-7".to_vec()))
        .await
        .expect("server send after resume");
    let frame = client2.receive().await.expect("frame after resume");
    assert_eq!(frame.as_bytes(), b"frame-7");

    client2.close().await.expect("client close");
    server2.close().await.expect("server close");
}

// ---------- 5. 发送窗口有界背压 + 有界重连 ----------

#[tokio::test]
async fn resend_window_backpressures_and_reconnects_within_bounds() {
    let harness = Harness::new(|config| {
        config.resend_window_frames = 4;
    });
    let (handle, listener) = publish_and_bind(&harness, "backpressure").await;
    let address = match &handle.endpoint {
        TransportEndpoint::Remote { address, .. } => address.clone(),
        _ => panic!("expected remote endpoint"),
    };

    let accept = tokio::spawn(async move { listener.accept().await });
    let connector = harness.connector();
    let client1 = connector
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("first connect");
    let server1: Arc<dyn GuiConnection> =
        Arc::from(accept.await.expect("accept task").expect("accept"));

    // 先填满 4 帧窗口；第 5 帧必须背压等待，不能淘汰仍在途帧。
    for seq in 1..=4u64 {
        server1
            .send(TransportFrame::new(format!("frame-{seq}").into_bytes()))
            .await
            .expect("server send");
    }
    wait_until(|| harness.transport.buffered_frames(&address, E2E_LABEL) == Some(4)).await;
    let mut blocked = tokio::spawn({
        let server1 = Arc::clone(&server1);
        async move { server1.send(TransportFrame::new(b"frame-5".to_vec())).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut blocked)
            .await
            .is_err(),
        "send beyond the resend window must backpressure"
    );
    assert_eq!(
        harness.transport.buffered_frames(&address, E2E_LABEL),
        Some(4)
    );

    // 交付一帧释放预算后，第 5 帧发送完成；窗口始终不超过 4 帧。
    assert_eq!(
        client1.receive().await.expect("client receive").as_bytes(),
        b"frame-1"
    );
    tokio::time::timeout(Duration::from_secs(3), blocked)
        .await
        .expect("blocked send must resume after ack")
        .expect("send task")
        .expect("send frame 5");
    for expected in 2..=5u64 {
        let frame = client1.receive().await.expect("client receive");
        assert_eq!(frame.as_bytes(), format!("frame-{expected}").as_bytes());
    }
    wait_until(|| harness.transport.acked_sequence(&address, E2E_LABEL) == Some(5)).await;
    assert!(
        harness
            .transport
            .buffered_frames(&address, E2E_LABEL)
            .is_some_and(|frames| frames <= 4),
        "resend frame budget must remain bounded"
    );
    client1.close().await.expect("client close");
    server1.close().await.expect("server close");

    // 同一 connector 的服务端签发 identity 允许安全恢复；全部已 ACK 时无需重放。
    let listener = harness
        .transport
        .bind(handle.endpoint.clone())
        .await
        .expect("bind again");
    let accept = tokio::spawn(async move { listener.accept().await });
    let client2 = connector
        .connect_typed(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("reconnect");
    let server2 = accept.await.expect("accept task").expect("accept");
    assert_eq!(
        client2.resume_outcome(),
        ResumeOutcome::UpToDate,
        "bounded reconnect after all acks must be up to date"
    );

    server2
        .send(TransportFrame::new(b"frame-6".to_vec()))
        .await
        .expect("server send after reconnect");
    let frame = client2.receive().await.expect("new frame");
    assert_eq!(frame.as_bytes(), b"frame-6");

    client2.close().await.expect("client close");
    server2.close().await.expect("server close");
}

// ---------- 6. revoke / unpublish 生命周期 ----------

#[tokio::test]
async fn revoke_closes_established_connections_and_blocks_new_ones() {
    let harness = Harness::new(|_| {});
    let provider = RealRemoteTransportProvider::new(Arc::clone(&harness.transport));
    let handle = provider
        .publish(RemotePublishRequest {
            name: "revoke".into(),
        })
        .await
        .expect("publish");
    let listener = harness
        .transport
        .bind(handle.endpoint.clone())
        .await
        .expect("bind");
    let listener: Arc<dyn GuiListener> = Arc::from(listener);
    let accept = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let conn = harness
        .connector()
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("connect");
    let server_conn = accept.await.expect("accept task").expect("accept");

    // 连接存活时可正常收发。
    server_conn
        .send(TransportFrame::new(b"before-revoke".to_vec()))
        .await
        .expect("send before revoke");
    assert_eq!(
        conn.receive()
            .await
            .expect("receive before revoke")
            .as_bytes(),
        b"before-revoke"
    );

    provider.revoke(&handle.id).await.expect("revoke");

    // 已建立连接在帧循环内断开（轮询间隔内）。
    let error = tokio::time::timeout(Duration::from_secs(3), conn.receive())
        .await
        .expect("revoked connection must close promptly")
        .expect_err("connection must be closed after revoke");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    let error = tokio::time::timeout(Duration::from_secs(3), server_conn.receive())
        .await
        .expect("server side must close promptly")
        .expect_err("server connection must be closed after revoke");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    let error = server_conn
        .send(TransportFrame::new(b"after-revoke".to_vec()))
        .await
        .expect_err("send after revoke must fail");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);

    // 新连接被拒（服务端 accept 返回 revoked）。
    let accept = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let error = match harness
        .connector()
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("connect after revoke must fail"),
    };
    assert!(
        matches!(
            error.kind,
            TransportErrorKind::ConnectionFailed | TransportErrorKind::ConnectionClosed
        ),
        "unexpected connect error after unpublish: {error:?}"
    );
    let server_error = match accept.await.expect("accept task") {
        Err(error) => error,
        Ok(_) => panic!("server must reject post-revoke connection"),
    };
    assert_eq!(
        server_error.kind,
        TransportErrorKind::ConnectionClosed,
        "unexpected server error: {server_error:?}"
    );
}

/// 生产接线全链路：Provider 与 GuiServer 共享同一 transport 实例（与
/// pawork 宿主的装配一致）——publish 后由同一 Core 实际 bind / accept，
/// 客户端经 GUI Connection Protocol 握手拿到 Snapshot，revoke 关闭监听器
/// 与现有连接、凭证立即失效、新连接被拒。
#[tokio::test]
async fn publish_bind_accept_revoke_via_gui_server_wiring() {
    let harness = Harness::new(|_| {});
    let app_service = Arc::new(AppService::new("remote-wiring"));
    let handshake = HandshakeService::new(
        CoreInstanceId::from("remote-wiring"),
        SUPPORTED_API_VERSIONS.to_vec(),
        vec![GuiCapability::Events],
    )
    .with_authenticator(Box::new(TokenAuthenticator::new(
        harness.token_store.clone(),
    )));
    let server = Arc::new(GuiServer::new(GuiServerConfig {
        app_service: app_service.clone(),
        handshake,
        transport: Arc::clone(&harness.transport) as Arc<dyn GuiTransportServer>,
        hub: Arc::new(EventHub::new()),
        connections: None,
    }));

    // 真实 Provider：与 GUI Server 共享同一 transport（唯一 Core 装配形态）。
    let provider = RealRemoteTransportProvider::new(Arc::clone(&harness.transport));
    let handle = provider
        .publish(RemotePublishRequest {
            name: "wiring".into(),
        })
        .await
        .expect("publish");

    // publish 后由同一 Core 实际 bind + accept（ServeGuiHost::bind_remote 路径）。
    let listener = server.bind(handle.endpoint.clone()).await.expect("bind");
    let listener: Arc<dyn GuiListener> = Arc::from(listener);
    let accept_loop = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move {
            // GuiServerListener::accept 内部已 spawn 会话任务；宿主只接受连接
            // 并持有会话句柄，循环在监听器关闭后退出。
            let mut sessions: Vec<Box<dyn GuiConnection>> = Vec::new();
            while let Ok(session) = listener.accept().await {
                sessions.push(session);
            }
        }
    });

    // GUI 客户端：connect → 握手 → Snapshot（与本地 GUI 完全相同的协议帧）。
    let conn = harness
        .connector()
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("connect");
    conn.send(TransportFrame::new(
        encode_client_frame(&ClientFrame::Handshake(HandshakeRequest {
            request_id: "hs-wiring".into(),
            client_name: "wiring-gui".into(),
            client_version: "0.0.1".into(),
            supported_api_versions: vec![API_VERSION],
            capabilities: vec![GuiCapability::Events],
            authentication: Some(authentication(&harness.token)),
        }))
        .expect("encode handshake"),
    ))
    .await
    .expect("send handshake");
    match decode_server_frame(conn.receive().await.expect("receive").as_bytes())
        .expect("decode handshake response")
    {
        ServerFrame::Handshake(HandshakeResponse::Accepted {
            selected_api_version,
            ..
        }) => assert_eq!(selected_api_version, API_VERSION),
        other => panic!("expected accepted handshake, got {other:?}"),
    }
    match decode_server_frame(conn.receive().await.expect("receive").as_bytes())
        .expect("decode snapshot")
    {
        ServerFrame::Snapshot(snapshot) => {
            assert_eq!(snapshot.instance_id.as_str(), "remote-wiring");
        }
        other => panic!("expected snapshot after handshake, got {other:?}"),
    }

    // revoke：监听器关闭 + 现有连接断开 + 新连接被拒（凭证已销毁）。
    provider.revoke(&handle.id).await.expect("revoke");

    let error = tokio::time::timeout(Duration::from_secs(3), conn.receive())
        .await
        .expect("revoked connection must close promptly")
        .expect_err("connection must be closed after revoke");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);

    // 新连接被拒。
    let error = match harness
        .connector()
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("connect after revoke must fail"),
    };
    assert!(
        matches!(
            error.kind,
            TransportErrorKind::ConnectionFailed | TransportErrorKind::ConnectionClosed
        ),
        "unexpected connect error after revoke: {error:?}"
    );

    // 宿主侧（ServeGuiHost::close_remote）中止 accept 循环并关闭监听器：
    // 任务终止、监听器关闭后新的 accept 立即失败。
    accept_loop.abort();
    let _ = accept_loop.await;
    listener.close().await.expect("close listener");
    let error = match listener.accept().await {
        Err(error) => error,
        Ok(_) => panic!("accept on closed listener must fail"),
    };
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
}

#[tokio::test]
async fn unpublish_closes_listener_and_established_connections() {
    let harness = Harness::new(|_| {});
    let provider = RealRemoteTransportProvider::new(Arc::clone(&harness.transport));
    let handle = provider
        .publish(RemotePublishRequest {
            name: "unpub".into(),
        })
        .await
        .expect("publish");
    let listener = harness
        .transport
        .bind(handle.endpoint.clone())
        .await
        .expect("bind");
    let listener: Arc<dyn GuiListener> = Arc::from(listener);
    let accept = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let conn = harness
        .connector()
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("connect");
    let server_conn = accept.await.expect("accept task").expect("accept");

    provider.unpublish(&handle.id).await.expect("unpublish");

    // 新连接失败（注册表已移除，listener 关闭 → TCP 拒绝）。
    let error = match harness
        .connector()
        .connect(&handle.endpoint, options(1024 * 1024))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("connect after unpublish must fail"),
    };
    assert!(
        matches!(
            error.kind,
            TransportErrorKind::ConnectionFailed | TransportErrorKind::ConnectionClosed
        ),
        "unexpected connect error after unpublish: {error:?}"
    );

    // 既有连接同样关闭，不能留下仍有权限的 detached session。
    let error = tokio::time::timeout(Duration::from_secs(3), conn.receive())
        .await
        .expect("client must close promptly")
        .expect_err("client connection must close after unpublish");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    let error = tokio::time::timeout(Duration::from_secs(3), server_conn.receive())
        .await
        .expect("server must close promptly")
        .expect_err("server connection must close after unpublish");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
}

// ---------- 7. 帧上限 ----------

#[tokio::test]
async fn frame_size_bound_is_enforced_over_real_transport() {
    let harness = Harness::new(|_| {});
    let (handle, listener) = publish_and_bind(&harness, "bound").await;
    let accept = tokio::spawn(async move { listener.accept().await });
    let conn = harness
        .connector()
        .connect(&handle.endpoint, options(16))
        .await
        .expect("connect");
    let server_conn = accept.await.expect("accept task").expect("accept");

    let error = conn
        .send(TransportFrame::new(vec![0u8; 17]))
        .await
        .expect_err("must reject oversized frame");
    assert_eq!(error.kind, TransportErrorKind::FrameTooLarge);

    // 边界内的小帧正常往返。
    conn.send(TransportFrame::new(vec![7u8; 16]))
        .await
        .expect("in-bounds frame");
    assert_eq!(
        server_conn
            .receive()
            .await
            .expect("server receive")
            .as_bytes(),
        &[7u8; 16]
    );

    conn.close().await.expect("client close");
    server_conn.close().await.expect("server close");
}

// ---------- 8. 并发双客户端：会话隔离与按会话续传 ----------

#[tokio::test]
async fn concurrent_dual_clients_have_isolated_sessions_and_resume() {
    let harness = Harness::new(|_| {});
    let (handle, listener) = publish_and_bind(&harness, "dual").await;
    let address = match &handle.endpoint {
        TransportEndpoint::Remote { address, .. } => address.clone(),
        _ => panic!("expected remote endpoint"),
    };
    let listener: Arc<dyn GuiListener> = Arc::from(listener);
    let connector1 = harness.connector();
    let connector2 = harness.connector();

    // 两个客户端并发建立连接（accept 串行，连接建立本身并发）。
    let accept1 = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let conn1 = connector1
        .connect_typed(
            &handle.endpoint,
            options_with_label(1024 * 1024, "client-1"),
        )
        .await
        .expect("connect client-1");
    let server1 = accept1.await.expect("accept 1 task").expect("accept 1");
    let accept2 = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let conn2 = connector2
        .connect_typed(
            &handle.endpoint,
            options_with_label(1024 * 1024, "client-2"),
        )
        .await
        .expect("connect client-2");
    let server2 = accept2.await.expect("accept 2 task").expect("accept 2");

    // 各自会话序号独立（都从 1 开始），帧互不交叉。
    server1
        .send(TransportFrame::new(b"for-1".to_vec()))
        .await
        .expect("server1 send");
    server2
        .send(TransportFrame::new(b"for-2".to_vec()))
        .await
        .expect("server2 send");
    assert_eq!(
        conn1.receive().await.expect("conn1 receive").as_bytes(),
        b"for-1"
    );
    assert_eq!(
        conn2.receive().await.expect("conn2 receive").as_bytes(),
        b"for-2"
    );
    wait_until(|| {
        harness.transport.acked_sequence(&address, "client-1") == Some(1)
            && harness.transport.acked_sequence(&address, "client-2") == Some(1)
    })
    .await;

    // 各自留下 1 帧未确认（seq 2），然后断开。
    server1
        .send(TransportFrame::new(b"pending-1".to_vec()))
        .await
        .expect("server1 pending");
    server2
        .send(TransportFrame::new(b"pending-2".to_vec()))
        .await
        .expect("server2 pending");
    wait_until(|| {
        harness.transport.buffered_frames(&address, "client-1") == Some(1)
            && harness.transport.buffered_frames(&address, "client-2") == Some(1)
    })
    .await;
    conn1.close().await.expect("conn1 close");
    conn2.close().await.expect("conn2 close");

    // 各自重连：只回放**自己会话**的未确认帧，绝不含另一客户端的帧。
    let accept1 = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let conn1r = connector1
        .connect_typed(
            &handle.endpoint,
            options_with_label(1024 * 1024, "client-1"),
        )
        .await
        .expect("reconnect client-1");
    let server1r = accept1.await.expect("accept 1r task").expect("accept 1r");
    let accept2 = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let conn2r = connector2
        .connect_typed(
            &handle.endpoint,
            options_with_label(1024 * 1024, "client-2"),
        )
        .await
        .expect("reconnect client-2");
    let server2r = accept2.await.expect("accept 2r task").expect("accept 2r");

    assert_eq!(conn1r.resume_outcome(), ResumeOutcome::ResumedFrom(2));
    assert_eq!(conn2r.resume_outcome(), ResumeOutcome::ResumedFrom(2));
    assert_eq!(
        conn1r.receive().await.expect("replay 1").as_bytes(),
        b"pending-1"
    );
    assert_eq!(
        conn2r.receive().await.expect("replay 2").as_bytes(),
        b"pending-2"
    );

    // 补发后新帧继续按各自会话有序。
    server1r
        .send(TransportFrame::new(b"new-1".to_vec()))
        .await
        .expect("server1r send");
    server2r
        .send(TransportFrame::new(b"new-2".to_vec()))
        .await
        .expect("server2r send");
    assert_eq!(conn1r.receive().await.expect("new 1").as_bytes(), b"new-1");
    assert_eq!(conn2r.receive().await.expect("new 2").as_bytes(), b"new-2");

    conn1r.close().await.expect("conn1r close");
    conn2r.close().await.expect("conn2r close");
    server1r.close().await.expect("server1r close");
    server2r.close().await.expect("server2r close");
}

// ---------- 9. 恶意 ACK：摘要不符 / 从未发送 / 跨会话 ----------

/// 裸线上客户端：绕过 transport 库，手工拼写信封，用于注入恶意 Ack。
struct RawWireClient {
    stream: ClientTlsStream<TcpStream>,
    resume_identity: Option<[u8; 32]>,
}

const WIRE_MAGIC: u16 = 0x5057;
const WIRE_VERSION: u8 = 3;
const WIRE_HEADER_BYTES: usize = 16;
const KIND_DATA: u8 = 1;
const KIND_ACK: u8 = 2;
const KIND_AUTH: u8 = 3;
const KIND_AUTH_OK: u8 = 4;
const KIND_RESUME_REQUEST: u8 = 6;
const KIND_RESUME_REPLY: u8 = 7;
const KIND_CLOSE: u8 = 8;
const RESUME_SNAPSHOT_REQUIRED: u8 = 2;

#[derive(Debug)]
struct RawEnvelope {
    kind: u8,
    seq: u64,
    payload: Vec<u8>,
}

impl RawWireClient {
    /// 建立 TLS（接受全部证书，测试端点自签名且非本测试关注点），完成 Auth。
    async fn connect(
        endpoint: &TransportEndpoint,
        token: &client_auth::Token,
        label: &str,
    ) -> Self {
        let TransportEndpoint::Remote { address, .. } = endpoint else {
            panic!("expected remote endpoint");
        };
        let rest = address.strip_prefix("real://").expect("real://");
        let (id_and_query, fragment) = rest.split_once('#').expect("#tcp=");
        let (_, query) = id_and_query.split_once('?').expect("?fp=");
        let _fingerprint_hex = query.strip_prefix("fp=").expect("fp=");
        let tcp = fragment.strip_prefix("tcp=").expect("tcp=");
        let (host, port) = tcp.rsplit_once(':').expect("host:port");
        let _ = id_and_query;

        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&version::TLS13])
            .expect("tls13")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAllVerifier { provider }))
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let tcp = TcpStream::connect((host, port.parse::<u16>().expect("port")))
            .await
            .expect("tcp connect");
        let server_name = ServerName::IpAddress(rustls::pki_types::IpAddr::from(
            std::net::Ipv4Addr::LOCALHOST,
        ));
        let stream = connector
            .connect(server_name, tcp)
            .await
            .expect("tls handshake");
        let mut client = Self {
            stream,
            resume_identity: None,
        };
        let auth = format!("pawork-token\0{label}\0{}", token.as_str());
        client.write_envelope(KIND_AUTH, 0, auth.as_bytes()).await;
        let reply = client.read_envelope().await.expect("read auth reply");
        assert_eq!(reply.kind, KIND_AUTH_OK, "auth must succeed");
        client
    }

    /// 发送 ResumeRequest(last_acked)，返回服务端 status 字节。
    async fn resume_with(&mut self, last_acked: u64) -> u8 {
        let mut payload = Vec::with_capacity(41);
        payload.extend_from_slice(&last_acked.to_le_bytes());
        match self.resume_identity {
            Some(identity) => {
                payload.push(1);
                payload.extend_from_slice(&identity);
            }
            None => payload.push(0),
        }
        self.write_envelope(KIND_RESUME_REQUEST, 0, &payload).await;
        let reply = self.read_envelope().await.expect("read resume reply");
        assert_eq!(reply.kind, KIND_RESUME_REPLY);
        assert_eq!(reply.payload.len(), 41);
        self.resume_identity = Some(reply.payload[9..].try_into().expect("resume identity"));
        reply.payload[0]
    }

    /// 读取下一帧 DATA，返回 (seq, payload)。
    async fn read_data(&mut self) -> (u64, Vec<u8>) {
        loop {
            let envelope = self.read_envelope().await.expect("read data envelope");
            if envelope.kind == KIND_DATA {
                return (envelope.seq, envelope.payload);
            }
        }
    }

    /// 发送 Ack（seq + payload sha256）。
    async fn send_ack(&mut self, seq: u64, digest: [u8; 32]) {
        self.send_ack_with_header(seq, seq, digest).await;
    }

    /// 发送可独立控制 header/payload 序号的 Ack，用于验证二者必须一致。
    async fn send_ack_with_header(&mut self, header_seq: u64, payload_seq: u64, digest: [u8; 32]) {
        let mut payload = [0u8; 40];
        payload[..8].copy_from_slice(&payload_seq.to_le_bytes());
        payload[8..].copy_from_slice(&digest);
        self.write_envelope(KIND_ACK, header_seq, &payload).await;
    }

    async fn close(&mut self) {
        self.write_envelope(KIND_CLOSE, 0, &[]).await;
    }

    /// 断言对端关闭：读到 Close 信封或 EOF。
    async fn expect_close(&mut self) -> bool {
        match tokio::time::timeout(Duration::from_secs(3), self.read_envelope()).await {
            Ok(Ok(envelope)) => envelope.kind == KIND_CLOSE,
            Ok(Err(_)) => true,
            Err(_) => false,
        }
    }

    async fn write_envelope(&mut self, kind: u8, seq: u64, payload: &[u8]) {
        use tokio::io::AsyncWriteExt;
        let mut header = [0u8; WIRE_HEADER_BYTES];
        header[..2].copy_from_slice(&WIRE_MAGIC.to_le_bytes());
        header[2] = WIRE_VERSION;
        header[3] = kind;
        header[4..12].copy_from_slice(&seq.to_le_bytes());
        header[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.stream.write_all(&header).await.expect("write header");
        self.stream.write_all(payload).await.expect("write payload");
        self.stream.flush().await.expect("flush");
    }

    async fn read_envelope(&mut self) -> Result<RawEnvelope, std::io::Error> {
        use tokio::io::AsyncReadExt;
        let mut header = [0u8; WIRE_HEADER_BYTES];
        self.stream.read_exact(&mut header).await?;
        assert_eq!(u16::from_le_bytes([header[0], header[1]]), WIRE_MAGIC);
        assert_eq!(header[2], WIRE_VERSION);
        let kind = header[3];
        let seq = u64::from_le_bytes(header[4..12].try_into().expect("8 bytes"));
        let len = u32::from_le_bytes(header[12..16].try_into().expect("4 bytes")) as usize;
        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).await?;
        Ok(RawEnvelope { kind, seq, payload })
    }
}

/// 接受全部服务端证书（自签名端点；指纹固定由 transport 库负责，本测试
/// 只关心认证后的信封层）。
#[derive(Debug)]
struct AcceptAllVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for AcceptAllVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[tokio::test]
async fn malicious_ack_is_rejected_and_cross_session_ack_never_leaks() {
    let harness = Harness::new(|_| {});
    let (handle, listener) = publish_and_bind(&harness, "mal-ack").await;
    let address = match &handle.endpoint {
        TransportEndpoint::Remote { address, .. } => address.clone(),
        _ => panic!("expected remote endpoint"),
    };
    let token = harness
        .transport
        .endpoint_token(&address)
        .expect("endpoint token");
    let listener: Arc<dyn GuiListener> = Arc::from(listener);

    // ---- 阶段 1：last_acked=0 显式快照信号；正确摘要的 Ack 保持连接。
    let accept = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let mut raw = RawWireClient::connect(&handle.endpoint, &token, "raw-client").await;
    assert_eq!(
        raw.resume_with(0).await,
        RESUME_SNAPSHOT_REQUIRED,
        "last_acked=0 must explicitly require a snapshot"
    );
    let server = accept.await.expect("accept task").expect("accept");
    server
        .send(TransportFrame::new(b"frame-for-raw".to_vec()))
        .await
        .expect("server send");
    let (seq, payload) = raw.read_data().await;
    assert_eq!(payload, b"frame-for-raw");
    let digest: [u8; 32] = Sha256::digest(&payload).into();
    raw.send_ack(seq, digest).await;
    server
        .send(TransportFrame::new(b"second-for-raw".to_vec()))
        .await
        .expect("server second send");
    let (seq2, payload2) = raw.read_data().await;
    assert_eq!(payload2, b"second-for-raw");

    // ---- 阶段 2：摘要不符的 Ack（对端并未真正收到该帧）→ 断开。
    let wrong_digest: [u8; 32] = Sha256::digest(b"different-bytes").into();
    raw.send_ack(seq2, wrong_digest).await;
    assert!(
        raw.expect_close().await,
        "digest-mismatch ack must close the connection"
    );
    let error = server
        .receive()
        .await
        .expect_err("server must close after malicious ack");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);

    // ---- 阶段 3：从未发送过的序号（凭空确认）→ 断开。
    let accept = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let mut raw2 = RawWireClient::connect(&handle.endpoint, &token, "raw-client").await;
    raw2.resume_with(0).await;
    let server2 = accept.await.expect("accept task").expect("accept");
    server2
        .send(TransportFrame::new(b"one-for-raw2".to_vec()))
        .await
        .expect("server2 send");
    let (seq3, _) = raw2.read_data().await;
    raw2.send_ack(999, [0u8; 32]).await;
    assert!(
        raw2.expect_close().await,
        "never-sent seq ack must close the connection"
    );
    let error = server2
        .receive()
        .await
        .expect_err("server2 must close after never-sent ack");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    let _ = seq3;

    // ---- 阶段 3b：跳过下一帧 / ACK 中间帧 → 断开；即使摘要正确也拒绝。
    let accept = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let mut raw_jump = RawWireClient::connect(&handle.endpoint, &token, "raw-jump").await;
    raw_jump.resume_with(0).await;
    let server_jump = accept.await.expect("accept jump").expect("jump");
    server_jump
        .send(TransportFrame::new(b"jump-first".to_vec()))
        .await
        .expect("send first");
    server_jump
        .send(TransportFrame::new(b"jump-second".to_vec()))
        .await
        .expect("send second");
    let (_first_seq, _first_payload) = raw_jump.read_data().await;
    let (second_seq, second_payload) = raw_jump.read_data().await;
    raw_jump
        .send_ack(second_seq, Sha256::digest(&second_payload).into())
        .await;
    assert!(raw_jump.expect_close().await, "skipped ack must close");
    assert_eq!(
        server_jump
            .receive()
            .await
            .expect_err("server jump must close")
            .kind,
        TransportErrorKind::ConnectionClosed
    );

    // ---- 阶段 3c：ACK header/payload 序号不一致 → 断开；不能只信任其中之一。
    let accept = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let mut raw_header = RawWireClient::connect(&handle.endpoint, &token, "raw-header").await;
    raw_header.resume_with(0).await;
    let server_header = accept.await.expect("accept header").expect("header");
    server_header
        .send(TransportFrame::new(b"header-check".to_vec()))
        .await
        .expect("send header check");
    let (header_seq, header_payload) = raw_header.read_data().await;
    raw_header
        .send_ack_with_header(
            header_seq + 1,
            header_seq,
            Sha256::digest(&header_payload).into(),
        )
        .await;
    assert!(
        raw_header.expect_close().await,
        "ack header/payload mismatch must close"
    );
    assert_eq!(
        server_header
            .receive()
            .await
            .expect_err("server header must close")
            .kind,
        TransportErrorKind::ConnectionClosed
    );

    // ---- 阶段 4：跨会话确认（raw3 确认另一客户端 A 收到的帧）→ raw3
    // 断开，A 及其会话水位不受影响。
    let accept_a = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let conn_a = harness
        .connector()
        .connect_typed(&handle.endpoint, options(1024 * 1024))
        .await
        .expect("connect client a");
    let server_a = accept_a.await.expect("accept a task").expect("accept a");
    server_a
        .send(TransportFrame::new(b"for-client-a".to_vec()))
        .await
        .expect("server_a send");
    assert_eq!(
        conn_a.receive().await.expect("a receive").as_bytes(),
        b"for-client-a"
    );

    let accept_raw3 = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    // 全新 label：raw3 拥有独立会话（seq 1 从未发送），跨会话确认必然被拒。
    let mut raw3 = RawWireClient::connect(&handle.endpoint, &token, "raw-client-3").await;
    raw3.resume_with(0).await;
    let server_raw3 = accept_raw3.await.expect("accept raw3").expect("raw3");
    // raw3 伪造 A 的确认：seq=1 + A 帧的摘要 —— 但 raw3 会话从未发送过
    // seq 1（每会话序号独立）。
    let cross_digest: [u8; 32] = Sha256::digest(b"for-client-a").into();
    raw3.send_ack(1, cross_digest).await;
    assert!(
        raw3.expect_close().await,
        "cross-session ack must close the connection"
    );
    let error = server_raw3
        .receive()
        .await
        .expect_err("server_raw3 must close after cross-session ack");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);

    // A 不受影响：继续收发，会话水位只记录 A 自己的确认。
    server_a
        .send(TransportFrame::new(b"still-for-a".to_vec()))
        .await
        .expect("server_a second send");
    assert_eq!(
        conn_a.receive().await.expect("a second receive").as_bytes(),
        b"still-for-a"
    );
    wait_until(|| harness.transport.acked_sequence(&address, E2E_LABEL) == Some(2)).await;
    assert_eq!(
        harness.transport.acked_sequence(&address, "raw-client-3"),
        Some(0),
        "malicious cross-session ack must not advance the raw session waterline"
    );

    conn_a.close().await.expect("a close");
    server_a.close().await.expect("server_a close");
}

#[tokio::test]
async fn shared_token_and_spoofed_label_cannot_resume_another_session() {
    let harness = Harness::new(|_| {});
    let (handle, listener) = publish_and_bind(&harness, "resume-identity").await;
    let address = match &handle.endpoint {
        TransportEndpoint::Remote { address, .. } => address.clone(),
        _ => panic!("expected remote endpoint"),
    };
    let token = harness
        .transport
        .endpoint_token(&address)
        .expect("endpoint token");
    let listener: Arc<dyn GuiListener> = Arc::from(listener);

    // Owner 获得服务端签发 identity，确认 seq 1，并留下 seq 2 未确认。
    let accept_owner = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let mut owner = RawWireClient::connect(&handle.endpoint, &token, "shared-label").await;
    assert_eq!(owner.resume_with(0).await, RESUME_SNAPSHOT_REQUIRED);
    let server_owner = accept_owner
        .await
        .expect("owner accept task")
        .expect("owner");
    server_owner
        .send(TransportFrame::new(b"owner-acked".to_vec()))
        .await
        .expect("owner acked send");
    let (seq1, payload1) = owner.read_data().await;
    owner.send_ack(seq1, Sha256::digest(&payload1).into()).await;
    wait_until(|| harness.transport.acked_sequence(&address, "shared-label") == Some(1)).await;
    server_owner
        .send(TransportFrame::new(b"owner-secret-pending".to_vec()))
        .await
        .expect("owner pending send");
    let (_, pending) = owner.read_data().await;
    assert_eq!(pending, b"owner-secret-pending");
    owner.close().await;
    assert_eq!(
        server_owner
            .receive()
            .await
            .expect_err("owner server closes")
            .kind,
        TransportErrorKind::ConnectionClosed
    );

    // 攻击者持有同一 endpoint token 并伪报同 label，但没有 owner 的随机
    // resume identity。即使声称 last_acked=1，也只能得到新会话 + Snapshot，
    // 绝不能收到 owner 的 seq 2。
    let accept_attacker = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let mut attacker = RawWireClient::connect(&handle.endpoint, &token, "shared-label").await;
    assert_eq!(
        attacker.resume_with(1).await,
        RESUME_SNAPSHOT_REQUIRED,
        "label + shared token alone must not authorize resume"
    );
    let server_attacker = accept_attacker
        .await
        .expect("attacker accept task")
        .expect("attacker");
    server_attacker
        .send(TransportFrame::new(b"attacker-fresh".to_vec()))
        .await
        .expect("attacker fresh send");
    let (_, received) = attacker.read_data().await;
    assert_eq!(
        received, b"attacker-fresh",
        "owner replay leaked to attacker"
    );

    attacker.close().await;
    server_attacker.close().await.expect("attacker close");
}

// ---------- 10. 端点独立凭证与撤销真正失效 ----------

#[tokio::test]
async fn endpoint_credentials_are_isolated_and_revoke_truly_invalidates() {
    let harness = Harness::new(|_| {});
    let provider = RealRemoteTransportProvider::new(Arc::clone(&harness.transport));
    let handle_a = provider
        .publish(RemotePublishRequest {
            name: "cred-a".into(),
        })
        .await
        .expect("publish a");
    let handle_b = provider
        .publish(RemotePublishRequest {
            name: "cred-b".into(),
        })
        .await
        .expect("publish b");
    let listener_a = harness
        .transport
        .bind(handle_a.endpoint.clone())
        .await
        .expect("bind a");
    let listener_b = harness
        .transport
        .bind(handle_b.endpoint.clone())
        .await
        .expect("bind b");
    let TransportEndpoint::Remote {
        address: addr_a, ..
    } = &handle_a.endpoint
    else {
        panic!("expected remote endpoint");
    };
    let TransportEndpoint::Remote {
        address: addr_b, ..
    } = &handle_b.endpoint
    else {
        panic!("expected remote endpoint");
    };
    let token_a = harness.transport.endpoint_token(addr_a).expect("token a");
    let token_b = harness.transport.endpoint_token(addr_b).expect("token b");
    let listener_a: Arc<dyn GuiListener> = Arc::from(listener_a);
    let listener_b: Arc<dyn GuiListener> = Arc::from(listener_b);
    assert_ne!(
        token_a.as_str(),
        token_b.as_str(),
        "each endpoint must bind its own credential"
    );

    // 交叉凭证被服务端拒绝：A 的凭证无法打开 B。
    let accept_reject = tokio::spawn({
        let listener = Arc::clone(&listener_b);
        async move { listener.accept().await }
    });
    let error = match harness
        .connector_with(token_a.clone())
        .connect(&handle_b.endpoint, options(1024 * 1024))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("token a must not open endpoint b"),
    };
    assert_eq!(error.kind, TransportErrorKind::AuthenticationFailed);
    let server_error = match accept_reject.await.expect("accept reject task") {
        Err(error) => error,
        Ok(_) => panic!("server must reject cross credential"),
    };
    assert_eq!(server_error.kind, TransportErrorKind::AuthenticationFailed);

    // A 的凭证打开 A 成功。
    let accept_a = tokio::spawn({
        let listener = Arc::clone(&listener_a);
        async move { listener.accept().await }
    });
    let conn_a = harness
        .connector_with(token_a)
        .connect_typed(&handle_a.endpoint, options(1024 * 1024))
        .await
        .expect("connect a");
    let _server_a = accept_a.await.expect("accept a task").expect("accept a");

    // revoke A：凭证文件删除、已建立连接断开、新连接失败；B 完全不受影响。
    let base = harness.token_store.path();
    let credential_file_a = base.parent().expect("parent").join(format!(
        "{}.d/{}/token",
        base.file_name().expect("name").to_string_lossy(),
        handle_a.id
    ));
    assert!(credential_file_a.exists(), "credential file must exist");
    provider.revoke(&handle_a.id).await.expect("revoke a");
    assert!(
        !credential_file_a.exists(),
        "revoke must delete the endpoint credential file"
    );
    assert_eq!(harness.transport.endpoint_token(addr_a), None);

    let error = tokio::time::timeout(Duration::from_secs(3), conn_a.receive())
        .await
        .expect("revoked connection must close promptly")
        .expect_err("established connection must be closed after revoke");
    assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    let error = match harness
        .connector()
        .connect(&handle_a.endpoint, options(1024 * 1024))
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("post-revoke connect must fail"),
    };
    assert!(
        matches!(
            error.kind,
            TransportErrorKind::ConnectionFailed
                | TransportErrorKind::ConnectionClosed
                | TransportErrorKind::AuthenticationFailed
        ),
        "unexpected post-revoke error: {error:?}"
    );

    // B 正常：凭证仍有效，连接可建立并收发。
    let accept_b = tokio::spawn({
        let listener = Arc::clone(&listener_b);
        async move { listener.accept().await }
    });
    let conn_b = harness
        .connector_with(token_b)
        .connect_typed(&handle_b.endpoint, options(1024 * 1024))
        .await
        .expect("connect b after revoke of a");
    let server_b = accept_b.await.expect("accept b task").expect("accept b");
    server_b
        .send(TransportFrame::new(b"b-alive".to_vec()))
        .await
        .expect("server_b send");
    assert_eq!(
        conn_b.receive().await.expect("b receive").as_bytes(),
        b"b-alive"
    );
    conn_b.close().await.expect("b close");
    server_b.close().await.expect("server_b close");
}

// ---------- 11. 缓冲内存按字节 + 帧双重有界 ----------

#[tokio::test]
async fn buffered_memory_is_bounded_by_bytes_and_frames() {
    let harness = Harness::new(|config| {
        config.resend_window_frames = 4;
        config.max_buffered_bytes = 1_200_000; // ≈ 4.5 × 256 KiB 帧
    });
    let (handle, listener) = publish_and_bind(&harness, "mem").await;
    let address = match &handle.endpoint {
        TransportEndpoint::Remote { address, .. } => address.clone(),
        _ => panic!("expected remote endpoint"),
    };
    let accept = tokio::spawn(async move { listener.accept().await });
    let conn = harness
        .connector()
        .connect_typed(&handle.endpoint, options(256 * 1024))
        .await
        .expect("connect");
    let server = accept.await.expect("accept task").expect("accept");

    // 服务端连推 8 帧 × 256 KiB，客户端不消费：窗口按帧数 + 字节数双重
    // 有界，不随推送量增长。
    let frame_bytes = vec![0xABu8; 256 * 1024];
    let send_task = tokio::spawn(async move {
        for _ in 0..8 {
            server
                .send(TransportFrame::new(frame_bytes.clone()))
                .await
                .expect("server send");
        }
    });
    wait_until(|| harness.transport.buffered_frames(&address, E2E_LABEL) == Some(4)).await;
    let frames = harness
        .transport
        .buffered_frames(&address, E2E_LABEL)
        .expect("buffered frames");
    let bytes = harness
        .transport
        .buffered_bytes(&address, E2E_LABEL)
        .expect("buffered bytes");
    assert!(frames <= 4, "frame bound violated: {frames}");
    assert!(bytes <= 1_200_000, "byte bound violated: {bytes} buffered");
    assert_eq!(bytes, frames as u64 * 256 * 1024, "steady state 4 frames");

    // 全部消费：8 帧按序完整到达，发送任务全部完成。
    for _ in 0..8u64 {
        let received = conn.receive().await.expect("receive");
        assert_eq!(received.as_bytes().len(), 256 * 1024);
        assert!(received.as_bytes().iter().all(|byte| *byte == 0xAB));
    }
    send_task.await.expect("send task");
    wait_until(|| harness.transport.acked_sequence(&address, E2E_LABEL) == Some(8)).await;

    conn.close().await.expect("client close");
}
