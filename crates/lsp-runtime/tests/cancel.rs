//! 请求取消：cancel token 触发后返回 Cancelled 并尽力发送 `$/cancelRequest`。

mod common;
use common::{route_handler, test_descriptor, MockAction, MockSpawner};

use lsp_runtime::{
    CancellationToken, ClientCapabilities, LanguageClient, LspClient, Position, ServerSpawnConfig,
};
use std::time::Duration;

async fn make_lc(handler: common::MockHandler) -> LanguageClient {
    let spawner = MockSpawner::single(handler).into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    LanguageClient::new(client).with_request_timeout(Duration::from_secs(5))
}

#[tokio::test]
async fn cancelling_inflight_request_returns_cancelled() {
    // 服务端对 hover 不响应（Ignore），客户端靠 cancel 取消。
    let handler = route_handler(vec![("textDocument/hover", MockAction::Ignore)]);
    let lc = make_lc(handler).await;
    let token = CancellationToken::new();
    let cancel_clone = token.clone();
    let handle = tokio::spawn(async move {
        lc.hover("file:///a.rs", Position::new(0, 0), Some(&cancel_clone))
            .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    token.cancel();
    let result = handle.await.expect("join").unwrap_err();
    assert!(matches!(result, lsp_runtime::LspError::Cancelled { .. }));
}

#[tokio::test]
async fn request_timeout_returns_timeout_error() {
    let handler = route_handler(vec![("textDocument/hover", MockAction::Ignore)]);
    let spawner = MockSpawner::single(handler).into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let lc = LanguageClient::new(client).with_request_timeout(Duration::from_millis(200));
    let err = lc
        .hover("file:///a.rs", Position::new(0, 0), None)
        .await
        .unwrap_err();
    assert!(matches!(err, lsp_runtime::LspError::Timeout { .. }));
    lc.shutdown().await.unwrap();
}
