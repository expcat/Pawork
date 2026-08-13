//! `pawork` GUI Server 宿主集成测试（P13-4 接线）。
//!
//! 与 `pawork serve` 相同的装配：真实 [`gui_server::GuiServer`] +
//! [`transport_local::LocalTransport`] 本地端点 + 每实例 token 认证，
//! 经 [`ServeGuiHost`] 验证 start → 客户端连接/握手/查询 → stop 全流程。

use std::sync::Arc;

use cli_host::GuiServerHost;
use client_auth::{Token, TokenAuthenticator, TokenStore};
use core_api::{ActorIdentity, AppQuery, CommandSource, SUPPORTED_API_VERSIONS};
use core_runtime::CoreRuntime;
use gui_client::GuiClient;
use gui_protocol::{GuiCapability, HandshakeService};
use gui_server::{GuiServer, GuiServerConfig};
use pawork::gui_host::{endpoint_for, ServeGuiHost};
use tempfile::TempDir;
use transport_api::{ConnectOptions, GuiTransportClient, TransportEndpoint};
use transport_local::LocalTransport;

async fn wait_for_session_count(host: &ServeGuiHost, expected: usize) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while host.active_session_count() != expected {
        assert!(
            tokio::time::Instant::now() < deadline,
            "active session count did not reach {expected}; got {}",
            host.active_session_count()
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// 与 `build_gui_server`（main.rs）相同的宿主装配，token 指向测试临时目录。
fn host_with_token(temp: &TempDir, instance: &str) -> (ServeGuiHost, Token, TransportEndpoint) {
    let token_path = temp.path().join("gui.token");
    let token = TokenStore::new(&token_path)
        .generate()
        .expect("generate token");
    let handshake = HandshakeService::new(
        agent_domain::CoreInstanceId::from(instance),
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

    let runtime = CoreRuntime::new(instance);
    let transport: Arc<dyn transport_api::GuiTransportServer> = Arc::new(LocalTransport::default());
    let server = Arc::new(GuiServer::new(GuiServerConfig {
        app_service: runtime.service().clone(),
        handshake,
        transport,
        hub: runtime.hub().clone(),
        connections: None,
    }));
    (ServeGuiHost::new(server), token, endpoint_for(instance))
}

fn unique_instance() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("test-{}-{nanos}", std::process::id())
}

async fn connect_client(
    endpoint: &TransportEndpoint,
    token: &Token,
) -> Result<GuiClient, gui_client::ClientError> {
    let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
    GuiClient::connect(
        transport,
        endpoint.clone(),
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: Some("gui-serve-test".into()),
            max_frame_bytes: 1024 * 1024,
        },
        token,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn start_accepts_authenticated_client_and_stop_releases_endpoint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let instance = unique_instance();
    let (host, token, endpoint) = host_with_token(&temp, &instance);

    host.start(&instance).expect("start");

    // 客户端经真实本地端点完成传输连接 + 握手 + 首帧 Snapshot。
    let client = connect_client(&endpoint, &token)
        .await
        .expect("connect and handshake");
    assert!(!client.client_id().as_str().is_empty());
    assert!(!client.connection_id().as_str().is_empty());

    // 命令面往返：查询经同一端点回到 AppService。
    let response = client
        .query(
            AppQuery::WorkspaceList,
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            ActorIdentity::LocalUser {
                actor_id: agent_domain::ActorId::from("gui-serve-test"),
                display_name: None,
            },
        )
        .await
        .expect("query round trip");
    assert!(matches!(response.response, core_api::AppResponse::Data(_)));

    client.close().await.expect("client close");
    host.stop().expect("stop");

    // stop 已清理监听器（Unix 同时移除 socket 文件）：可重新绑定同一端点。
    host.start(&instance).expect("restart after stop");
    host.stop().expect("stop again");
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_token_is_rejected_at_handshake() {
    let temp = tempfile::tempdir().expect("tempdir");
    let instance = unique_instance();
    let (host, _token, endpoint) = host_with_token(&temp, &instance);
    let wrong_token = TokenStore::new(temp.path().join("wrong.token"))
        .generate()
        .expect("generate wrong token");

    host.start(&instance).expect("start");

    let error = match connect_client(&endpoint, &wrong_token).await {
        Err(error) => error,
        Ok(_) => panic!("wrong token must be rejected"),
    };
    assert!(
        error.is_auth_failure(),
        "expected authentication failure, got {error:?}"
    );

    host.stop().expect("stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_without_start_is_a_noop() {
    let temp = tempfile::tempdir().expect("tempdir");
    let instance = unique_instance();
    let (host, _token, _endpoint) = host_with_token(&temp, &instance);
    host.stop().expect("stop on never-started host");
}

#[tokio::test(flavor = "multi_thread")]
async fn accept_loop_reclaims_closed_sessions_across_bounded_reconnects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let instance = unique_instance();
    let (host, token, endpoint) = host_with_token(&temp, &instance);
    host.start(&instance).expect("start");

    // 重复重连远多于一个会话；accept loop 必须只持有当前活跃句柄，关闭后
    // 及时回收，不能用无界 Vec 累积历史 SessionHandle。
    for _ in 0..32 {
        let client = connect_client(&endpoint, &token)
            .await
            .expect("connect and handshake");
        wait_for_session_count(&host, 1).await;
        client.close().await.expect("client close");
        wait_for_session_count(&host, 0).await;
    }

    assert_eq!(host.active_session_count(), 0);
    host.stop().expect("stop");
}
