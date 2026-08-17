//! 本机回环定向测试：publish → token 认证 → TLS 上收发 TransportFrame → revoke。
//!
//! 只搬字节，不跑 GUI 握手。Secret 不得出现在错误字符串里。

use std::sync::Arc;
use std::time::Duration;

use pawork_protocol::client_auth::TokenStore;

use crate::{
    ConnectOptions, ConnectionLocality, GuiListener, GuiTransportServer, RemoteGuiConnector,
    RemoteGuiTransportProvider, RemotePublishRequest, TransportEndpoint, TransportErrorKind,
    TransportFrame,
};

use super::{
    RealRemoteConnector, RealRemoteTransport, RealRemoteTransportConfig,
    RealRemoteTransportProvider, ADAPTER_NAME,
};

fn options() -> ConnectOptions {
    ConnectOptions {
        timeout_ms: 2_000,
        client_label: Some("loopback".into()),
        max_frame_bytes: crate::DEFAULT_MAX_FRAME_BYTES,
    }
}

fn assert_secret_absent(haystack: &str, secrets: &[&str]) {
    for secret in secrets {
        assert!(
            !secret.is_empty() && !haystack.contains(secret),
            "secret must not appear in error text: {haystack:?}"
        );
    }
}

#[tokio::test]
async fn publish_token_auth_tls_frame_round_trip_then_revoke() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let transport = Arc::new(RealRemoteTransport::new(RealRemoteTransportConfig::new(
        TokenStore::new(temp.path().join("server.token")),
        None,
    )));
    let provider = RealRemoteTransportProvider::new(Arc::clone(&transport));
    let handle = provider
        .publish(RemotePublishRequest {
            name: "loopback".into(),
        })
        .await
        .expect("publish");

    let TransportEndpoint::Remote { address, adapter } = &handle.endpoint else {
        panic!("expected remote endpoint");
    };
    assert_eq!(adapter, ADAPTER_NAME);
    assert!(address.starts_with("real://loopback-0?fp="));

    let good_token = transport
        .endpoint_token(address)
        .expect("published endpoint token");
    let good_secret = good_token.as_str().to_string();
    assert!(
        !format!("{good_token:?}").contains(&good_secret),
        "Token Debug must be redacted"
    );

    let wrong_token = TokenStore::new(temp.path().join("wrong.token"))
        .generate()
        .expect("wrong token");
    let wrong_secret = wrong_token.as_str().to_string();
    assert_ne!(good_secret, wrong_secret);

    let listener: Arc<dyn GuiListener> =
        Arc::from(transport.bind(handle.endpoint.clone()).await.expect("bind"));

    let accept_wrong = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let wrong_error = match RealRemoteConnector::new(Arc::clone(&transport), Some(wrong_token))
        .connect(&handle.endpoint, options())
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("wrong token must be rejected"),
    };
    assert_eq!(wrong_error.kind, TransportErrorKind::AuthenticationFailed);
    assert_secret_absent(&wrong_error.message, &[&good_secret, &wrong_secret]);
    assert_secret_absent(&format!("{wrong_error:?}"), &[&good_secret, &wrong_secret]);

    let accept_wrong = tokio::time::timeout(Duration::from_secs(2), accept_wrong)
        .await
        .expect("accept must finish")
        .expect("accept task");
    let accept_wrong_error = match accept_wrong {
        Err(error) => error,
        Ok(_) => panic!("server must reject wrong token"),
    };
    assert_eq!(
        accept_wrong_error.kind,
        TransportErrorKind::AuthenticationFailed
    );
    assert_secret_absent(
        &accept_wrong_error.message,
        &[&good_secret, &wrong_secret],
    );

    let accept_ok = tokio::spawn({
        let listener = Arc::clone(&listener);
        async move { listener.accept().await }
    });
    let client = RealRemoteConnector::new(Arc::clone(&transport), Some(good_token.clone()))
        .connect(&handle.endpoint, options())
        .await
        .expect("good token must authenticate");
    let server = tokio::time::timeout(Duration::from_secs(2), accept_ok)
        .await
        .expect("accept must finish")
        .expect("accept task")
        .expect("accept");

    assert_eq!(client.info().locality, ConnectionLocality::Remote);
    assert_eq!(server.info().locality, ConnectionLocality::Remote);
    assert!(client.info().encrypted);
    assert!(server.info().encrypted);

    client
        .send(TransportFrame::new(b"ping-bytes".to_vec()))
        .await
        .expect("client send");
    assert_eq!(
        server.receive().await.expect("server receive").as_bytes(),
        b"ping-bytes"
    );
    server
        .send(TransportFrame::new(b"pong-bytes".to_vec()))
        .await
        .expect("server send");
    assert_eq!(
        client.receive().await.expect("client receive").as_bytes(),
        b"pong-bytes"
    );

    provider.revoke(&handle.id).await.expect("revoke");
    assert!(transport.endpoint_token(address).is_none());

    let after_revoke = match RealRemoteConnector::new(Arc::clone(&transport), Some(good_token))
        .connect(&handle.endpoint, options())
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("connect after revoke must fail"),
    };
    assert!(
        matches!(
            after_revoke.kind,
            TransportErrorKind::ConnectionFailed
                | TransportErrorKind::AuthenticationFailed
                | TransportErrorKind::Timeout
                | TransportErrorKind::ConnectionClosed
        ),
        "unexpected revoke connect error: {after_revoke:?}"
    );
    assert_secret_absent(&after_revoke.message, &[&good_secret, &wrong_secret]);

    let _ = client.close().await;
    let _ = server.close().await;
    let _ = listener.close().await;
}
