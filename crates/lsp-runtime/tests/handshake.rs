//! 初始化握手 / 能力协商 / 关闭。

mod common;
use common::{route_handler, test_descriptor, MockAction, MockSpawner};

use lsp_runtime::{ClientCapabilities, LanguageClient, LspClient, Phase, ServerSpawnConfig};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

async fn wait_for<F: Fn() -> bool>(cond: F) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within timeout");
}

#[tokio::test]
async fn initialize_negotiates_capabilities_and_shutdown_completes() {
    let handler = route_handler(vec![]);
    let spawner = MockSpawner::single(handler).into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");

    assert_eq!(client.phase().await, Phase::Initialized);
    let caps = client.server_capabilities().await.expect("caps");
    assert!(caps.hover_provider.unwrap_or(false));
    assert!(caps.rename_provider.unwrap_or(false));
    assert!(caps
        .text_document_sync
        .as_ref()
        .map(|s| s.incremental())
        .unwrap_or(false));

    let lc = LanguageClient::new(client);
    lc.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn initialize_reflects_partial_capabilities() {
    use common::{full_capabilities, MockAction, MockServerSpec};
    use lsp_runtime::SharedSpawner;
    use std::sync::Arc;
    let mut caps = full_capabilities();
    // 仅开 hover，其余关闭。
    if let Some(obj) = caps.as_object_mut() {
        obj.insert("definitionProvider".to_string(), serde_json::json!(false));
        obj.insert("renameProvider".to_string(), serde_json::json!(false));
    }
    let handler = Arc::new(|_m: &str, _p: &serde_json::Value, _h: bool| {
        MockAction::Respond(serde_json::Value::Null)
    });
    let spawner: SharedSpawner = MockSpawner::new(vec![MockServerSpec {
        capabilities: caps,
        handler,
        init_delay: None,
    }])
    .into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    let caps = client.server_capabilities().await.unwrap();
    assert!(caps.hover_provider.unwrap_or(false));
    assert!(!caps.definition_provider.unwrap_or(true));
    assert!(!caps.rename_provider.unwrap_or(true));
    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn settings_are_sent_via_did_change_configuration() {
    let received: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();
    let handler = Arc::new(move |method: &str, params: &Value, _has_id: bool| {
        if method == "workspace/didChangeConfiguration" {
            received_clone.lock().unwrap().push(params.clone());
        }
        MockAction::Respond(Value::Null)
    });
    let mut desc = test_descriptor("rust");
    desc.settings = Some(json!({ "rust-analyzer": { "check": { "command": "clippy" } } }));
    let client = LspClient::start(
        desc,
        MockSpawner::single(handler).into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    assert_eq!(client.phase().await, Phase::Initialized);
    wait_for(|| !received.lock().unwrap().is_empty()).await;
    {
        let recv = received.lock().unwrap();
        assert_eq!(recv.len(), 1);
        assert_eq!(
            recv[0]["settings"]["rust-analyzer"]["check"]["command"],
            "clippy"
        );
    }
    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn settings_default_to_empty_object_when_unconfigured() {
    let received: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();
    let handler = Arc::new(move |method: &str, params: &Value, _has_id: bool| {
        if method == "workspace/didChangeConfiguration" {
            received_clone.lock().unwrap().push(params.clone());
        }
        MockAction::Respond(Value::Null)
    });
    let client = LspClient::start(
        test_descriptor("rust"),
        MockSpawner::single(handler).into_shared(),
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    wait_for(|| !received.lock().unwrap().is_empty()).await;
    {
        let recv = received.lock().unwrap();
        assert_eq!(recv[0]["settings"], json!({}));
    }
    client.shutdown().await.expect("shutdown");
}
