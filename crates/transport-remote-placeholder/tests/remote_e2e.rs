//! P13-6 端到端：Mock provider publish → connector connect → gui-server 完整
//! 握手 → command 往返 → event 收到。
//!
//! 证明本地与远程 GUI 复用同一 GUI Connection Protocol（[ADR-027]）：
//! 唯一的差异是传输层（此处为 Mock loopback），GUI Server 与协议编解码
//! 与 `gui-server` 的本地端到端测试完全相同。

use std::sync::Arc;

use agent_domain::{CoreInstanceId, EventId, RunId, Timestamp};
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
use subscription_hub::EventHub;
use tempfile::TempDir;
use transport_api::{ConnectOptions, ConnectionLocality, TransportFrame};
use transport_remote_placeholder::{
    MockRemoteConnector, MockRemoteTransport, MockRemoteTransportProvider, RemoteGuiConnector,
    RemoteGuiTransportProvider, RemotePublishRequest,
};

fn authentication(token: &Token) -> ClientAuthentication {
    ClientAuthentication {
        scheme: TOKEN_SCHEME.into(),
        proof: token.as_str().into(),
    }
}

#[tokio::test]
async fn mock_publish_connect_handshake_command_and_event_end_to_end() {
    let temp = TempDir::new().expect("tempdir");
    let token_store = TokenStore::new(temp.path().join("remote.token"));
    let token = token_store.generate().expect("generate token");
    let app_service = Arc::new(AppService::new("remote-e2e"));
    let handshake = HandshakeService::new(
        CoreInstanceId::from("remote-instance"),
        SUPPORTED_API_VERSIONS.to_vec(),
        vec![GuiCapability::Events],
    )
    .with_authenticator(Box::new(TokenAuthenticator::new(token_store)));

    // 同一 MockRemoteTransport 同时充当 Server（CLI 侧）与 Client（GUI 侧）。
    let transport = Arc::new(MockRemoteTransport::default());
    let server = GuiServer::new(GuiServerConfig {
        app_service: app_service.clone(),
        handshake,
        transport: transport.clone(),
        hub: Arc::new(EventHub::new()),
        connections: None,
    });

    // 1) CLI 侧经 Provider 发布 mock endpoint。
    let provider = MockRemoteTransportProvider::new(Arc::clone(&transport));
    assert_eq!(provider.describe().adapter, "mock");
    let handle = provider
        .publish(RemotePublishRequest { name: "e2e".into() })
        .await
        .expect("publish");
    let transport_api::TransportEndpoint::Remote { address, adapter } = &handle.endpoint else {
        panic!("expected remote endpoint, got {:?}", handle.endpoint);
    };
    assert_eq!(adapter, "mock");
    assert!(address.starts_with("mock://e2e-"));

    // 2) GUI Server 绑定已发布端点（与本地端到端完全相同的 bind→accept 流程）。
    let listener = server.bind(handle.endpoint.clone()).await.expect("bind");
    let accept = tokio::spawn(async move { listener.accept().await });

    // 3) GUI 侧经 Connector 连接，locality 为 Remote。
    let connector = MockRemoteConnector::new(Arc::clone(&transport));
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
    let session = accept.await.expect("accept task").expect("accept");

    // 4) 握手：与本地 GUI 完全相同的协议帧。
    conn.send(TransportFrame::new(
        encode_client_frame(&ClientFrame::Handshake(HandshakeRequest {
            request_id: "hs-remote".into(),
            client_name: "e2e-gui".into(),
            client_version: "0.0.1".into(),
            supported_api_versions: vec![API_VERSION],
            capabilities: vec![GuiCapability::Events],
            authentication: Some(authentication(&token)),
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

    // P13-5 起：握手成功后服务端先发 Snapshot，再进入帧循环。
    match decode_server_frame(conn.receive().await.expect("receive").as_bytes())
        .expect("decode snapshot")
    {
        ServerFrame::Snapshot(snapshot) => {
            assert_eq!(snapshot.instance_id.as_str(), "remote-e2e");
        }
        other => panic!("expected snapshot after handshake, got {other:?}"),
    }

    // 5) Command 往返：命令经 app-service 派发并返回响应。
    conn.send(TransportFrame::new(
        encode_client_frame(&ClientFrame::Command(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: agent_domain::CommandId::from("remote-cmd-1"),
            source: CommandSource::LocalGui {
                client_id: agent_domain::GuiClientId::from("remote-e2e"),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: agent_domain::ActorId::from("remote-e2e-user"),
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

    // 6) Event 收到：宿主侧 SessionHandle 推送事件帧，经 mock 传输到达 GUI。
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

    // 7) Query 仍可往返（连接在事件后保持可用）。
    conn.send(TransportFrame::new(
        encode_client_frame(&ClientFrame::Query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: agent_domain::QueryId::from("remote-q-1"),
            source: CommandSource::LocalGui {
                client_id: agent_domain::GuiClientId::from("remote-e2e"),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: agent_domain::ActorId::from("remote-e2e-user"),
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
