//! P13-8 GUI ArtifactRead 流式读取集成测试（transport-memory）。
//!
//! 覆盖：store.put + aggregate 登记 → GUI ArtifactRead → 分片重组与原始 payload
//! 一致（约 5MiB、100k 行 diff 文本验证多分片流式读取）；缺失记录 → RequestNotFound；
//! 未配置 store → Internal；客户端 limit 提前耗尽时末片 eof=true。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::{ArtifactId, CoreInstanceId};
use app_service::AppService;
use artifact_store::ArtifactStore;
use client_auth::{Token, TokenAuthenticator, TokenStore};
use core_api::{API_VERSION, SUPPORTED_API_VERSIONS};
use gui_protocol::{
    decode_server_frame, encode_client_frame, ArtifactReadRequest, ClientAuthentication,
    ClientFrame, GuiCapability, HandshakeRequest, HandshakeResponse, ProtocolErrorCode,
    ServerFrame, MAX_ARTIFACT_CHUNK_BYTES,
};
use gui_server::{GuiServer, GuiServerConfig};
use subscription_hub::EventHub;
use tempfile::TempDir;
use transport_api::{
    ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, TransportEndpoint,
    TransportFrame,
};
use transport_memory::MemoryTransport;

const CHANNEL: &str = "artifact-streaming";

struct Harness {
    app_service: Arc<AppService>,
    listener: Arc<dyn GuiListener>,
    transport: Arc<MemoryTransport>,
    token: Token,
    _temp: TempDir,
}

async fn harness(store: Option<Arc<ArtifactStore>>) -> Harness {
    let app_service = match store {
        Some(store) => Arc::new(AppService::with_artifact_store(
            "gui-artifact-stream",
            store,
        )),
        None => Arc::new(AppService::new("gui-artifact-stream")),
    };
    let temp = tempfile::tempdir().expect("tempdir");
    let token_path = temp.path().join("gui.token");
    let token = TokenStore::new(&token_path)
        .generate()
        .expect("generate token");
    let handshake = gui_protocol::HandshakeService::new(
        CoreInstanceId::from("artifact-stream-instance"),
        SUPPORTED_API_VERSIONS.to_vec(),
        vec![GuiCapability::ArtifactStreaming],
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
        connections: None,
    });
    let listener = server
        .bind(TransportEndpoint::Memory {
            channel: CHANNEL.into(),
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

fn authentication(token: &Token) -> ClientAuthentication {
    ClientAuthentication {
        scheme: client_auth::TOKEN_SCHEME.into(),
        proof: token.as_str().into(),
    }
}

struct TestClient {
    conn: Box<dyn GuiConnection>,
    _session: Box<dyn GuiConnection>,
}

impl TestClient {
    async fn connect(harness: &Harness) -> Self {
        let listener = Arc::clone(&harness.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let conn = harness
            .transport
            .connect(
                TransportEndpoint::Memory {
                    channel: CHANNEL.into(),
                },
                ConnectOptions {
                    timeout_ms: 1_000,
                    client_label: Some("artifact-stream-test".into()),
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
            .expect("connect");
        let session = accept.await.expect("accept task").expect("accept");
        let client = Self {
            conn,
            _session: session,
        };
        client
            .send(&ClientFrame::Handshake(HandshakeRequest {
                request_id: "hs-artifact".into(),
                client_name: "artifact-stream-test".into(),
                client_version: "0.0.1".into(),
                supported_api_versions: vec![API_VERSION],
                capabilities: vec![GuiCapability::ArtifactStreaming],
                authentication: Some(authentication(&harness.token)),
            }))
            .await;
        let response = client.recv().await;
        assert!(
            matches!(
                response,
                ServerFrame::Handshake(HandshakeResponse::Accepted { .. })
            ),
            "handshake 应被接受，got {response:?}"
        );
        // P13-5：握手后服务端先发首帧 Snapshot。
        assert!(matches!(client.recv().await, ServerFrame::Snapshot(_)));
        client
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

    async fn recv_timeout(&self) -> ServerFrame {
        tokio::time::timeout(Duration::from_secs(10), self.recv())
            .await
            .expect("recv timed out")
    }
}

/// 约 5MiB、100k 行的 diff 文本（每行 ~58 字节）。
fn diff_payload(lines: usize) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("diff --git a/src/main.rs b/src/main.rs\n");
    out.push_str("--- a/src/main.rs\n");
    out.push_str("+++ b/src/main.rs\n");
    out.push_str(&format!("@@ -1,{lines} +1,{lines} @@\n"));
    for i in 0..lines {
        out.push_str(&format!(
            "+pub fn line_{i}(value: usize) -> usize {{ value + {i} }}\n"
        ));
    }
    out.into_bytes()
}

/// 读取 ArtifactRead 响应直到 eof，返回 (分片数, 重组 payload)。
async fn collect_chunks(
    client: &TestClient,
    request_id: &str,
    artifact_id: &ArtifactId,
    max_chunks: usize,
) -> (usize, Vec<u8>) {
    let mut assembled = Vec::new();
    let mut expected_offset = None;
    let mut last_eof = false;
    let mut chunks = 0usize;
    for _ in 0..max_chunks {
        let frame = client.recv_timeout().await;
        let chunk = match frame {
            ServerFrame::ArtifactChunk(chunk) => chunk,
            other => panic!("expected artifact chunk, got {other:?}"),
        };
        assert_eq!(chunk.request_id, request_id);
        assert_eq!(chunk.artifact_id, *artifact_id);
        assert!(chunk.data.len() <= MAX_ARTIFACT_CHUNK_BYTES);
        if let Some(expected) = expected_offset {
            assert_eq!(chunk.offset, expected, "offset 必须连续");
        }
        chunks += 1;
        assembled.extend_from_slice(&chunk.data);
        expected_offset = Some(chunk.offset + chunk.data.len() as u64);
        last_eof = chunk.eof;
        if chunk.eof {
            break;
        }
    }
    assert!(last_eof, "末片必须 eof=true");
    (chunks, assembled)
}

#[tokio::test]
async fn streams_large_diff_payload_across_many_chunks() {
    let diff = diff_payload(100_000);
    assert!(
        diff.len() > 5 * 1024 * 1024,
        "diff 应约 5MiB，实际 {}",
        diff.len()
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        ArtifactStore::open(temp.path().join("store"))
            .await
            .expect("open store"),
    );
    let outcome = store.put(&diff).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    let harness = harness(Some(Arc::clone(&store))).await;
    harness
        .app_service
        .router()
        .aggregate()
        .put_artifact(artifact_id.clone(), diff.len() as u64, "text/x-diff".into())
        .expect("register artifact");
    let client = TestClient::connect(&harness).await;

    client
        .send(&ClientFrame::ArtifactRead(ArtifactReadRequest {
            request_id: "ar-big".into(),
            artifact_id: artifact_id.clone(),
            offset: 0,
            limit: 0,
        }))
        .await;
    let (chunks, assembled) = collect_chunks(&client, "ar-big", &artifact_id, 200).await;
    assert!(chunks > 80, "约 5MiB 应按 64KiB 切为 80+ 片，实际 {chunks}");
    assert_eq!(
        assembled, diff,
        "分片重组必须与 store.put 的原始 payload 一致"
    );
    assert_eq!(assembled.len() as u64, diff.len() as u64);
}

#[tokio::test]
async fn partial_limit_ends_with_eof_before_file_tail() {
    let diff = diff_payload(2_000);
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        ArtifactStore::open(temp.path().join("store"))
            .await
            .expect("open store"),
    );
    let outcome = store.put(&diff).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());
    let harness = harness(Some(Arc::clone(&store))).await;
    harness
        .app_service
        .router()
        .aggregate()
        .put_artifact(artifact_id.clone(), diff.len() as u64, "text/x-diff".into())
        .expect("register artifact");
    let client = TestClient::connect(&harness).await;

    client
        .send(&ClientFrame::ArtifactRead(ArtifactReadRequest {
            request_id: "ar-partial".into(),
            artifact_id: artifact_id.clone(),
            offset: 0,
            limit: 1_000,
        }))
        .await;
    let (chunks, assembled) = collect_chunks(&client, "ar-partial", &artifact_id, 4).await;
    assert_eq!(chunks, 1, "limit=1000 < 64KiB 应单片返回");
    assert_eq!(assembled, diff[..1_000], "limit 必须精确截断");
    assert!(assembled.len() < diff.len(), "未到文件尾即提前结束");
}

#[tokio::test]
async fn missing_artifact_record_returns_request_not_found() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        ArtifactStore::open(temp.path().join("store"))
            .await
            .expect("open store"),
    );
    let harness = harness(Some(store)).await;
    let client = TestClient::connect(&harness).await;
    let artifact_id = ArtifactId::from("f".repeat(64));

    client
        .send(&ClientFrame::ArtifactRead(ArtifactReadRequest {
            request_id: "ar-missing".into(),
            artifact_id: artifact_id.clone(),
            offset: 0,
            limit: 0,
        }))
        .await;
    let ServerFrame::Error(envelope) = client.recv_timeout().await else {
        panic!("expected error for missing artifact");
    };
    assert_eq!(envelope.error.code, ProtocolErrorCode::RequestNotFound);
    assert_eq!(envelope.request_id.as_deref(), Some("ar-missing"));
}

#[tokio::test]
async fn no_store_maps_to_internal_error() {
    let harness = harness(None).await;
    // 有 aggregate 记录但未配置 store → Unavailable → 协议层 Internal。
    let artifact_id = ArtifactId::from("a".repeat(64));
    harness
        .app_service
        .router()
        .aggregate()
        .put_artifact(artifact_id.clone(), 42, "text/plain".into())
        .expect("register artifact");
    let client = TestClient::connect(&harness).await;

    client
        .send(&ClientFrame::ArtifactRead(ArtifactReadRequest {
            request_id: "ar-no-store".into(),
            artifact_id: artifact_id.clone(),
            offset: 0,
            limit: 0,
        }))
        .await;
    let ServerFrame::Error(envelope) = client.recv_timeout().await else {
        panic!("expected error when store is not configured");
    };
    assert_eq!(envelope.error.code, ProtocolErrorCode::Internal);
    assert_eq!(envelope.request_id.as_deref(), Some("ar-no-store"));
}
