//! 文档同步：didOpen / didChange / didClose + 推送诊断。

mod common;
use common::{inline, route_handler, test_descriptor, MockAction, MockSpawner};

use lsp_runtime::{
    ClientCapabilities, LanguageClient, LspClient, Position, Range, ServerSpawnConfig,
    MAX_BUFFERED_NOTIFICATIONS,
};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

async fn make_client(handler: common::MockHandler) -> LanguageClient {
    let spawner = MockSpawner::single(handler).into_shared();
    let client = LspClient::start(
        test_descriptor("rust"),
        spawner,
        ServerSpawnConfig::default(),
        ClientCapabilities::pawork_default(),
    )
    .await
    .expect("start");
    LanguageClient::new(client).with_request_timeout(std::time::Duration::from_secs(2))
}

async fn wait_until<F: Fn() -> bool>(cond: F) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition not met within timeout");
}

#[tokio::test]
async fn diagnostics_keep_latest_per_uri_and_notifications_are_bounded_and_drainable() {
    // 推送诊断按 URI 只保留最新一次；服务端通知缓冲有界，超限丢弃最旧并计数，
    // drain 可取回全部剩余且之后为空。
    let seq = Arc::new(AtomicUsize::new(0));
    let seq_clone = seq.clone();
    let handler = Arc::new(
        move |method: &str, _params: &serde_json::Value, _has_id: bool| {
            if method == "textDocument/didOpen" {
                let n = seq_clone.fetch_add(1, Ordering::SeqCst);
                MockAction::Notify(
                    "textDocument/publishDiagnostics".to_string(),
                    json!({
                        "uri": "file:///a.rs",
                        "version": n,
                        "diagnostics": [{
                            "range": {
                                "start": { "line": 0, "character": 0 },
                                "end": { "line": 0, "character": 3 }
                            },
                            "severity": 1,
                            "message": format!("diag-{n}")
                        }]
                    }),
                )
            } else {
                MockAction::Respond(serde_json::Value::Null)
            }
        },
    );
    let lc = make_client(handler).await;
    // 同一 URI 推两次 → 快照只留最新值。
    lc.did_open("file:///a.rs", "rust", "x").await.unwrap();
    lc.did_open("file:///a.rs", "rust", "y").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let snap = lc.lsp().diagnostics_snapshot().await;
    assert_eq!(snap.len(), 1, "每 URI 只保留一条最新诊断");
    assert_eq!(snap[0].diagnostics.len(), 1);
    assert_eq!(snap[0].diagnostics[0].message, "diag-1");
    let diags = inline(lc.diagnostics("file:///a.rs").await.unwrap());
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "diag-1");

    // 灌入超过缓冲上限的通知：最旧被丢弃并计数。
    let pushes = MAX_BUFFERED_NOTIFICATIONS + 64;
    for _ in 0..pushes {
        lc.did_open("file:///a.rs", "rust", "z").await.unwrap();
    }
    // 全部通知落地（dropped 计数达到 2 + pushes - 上限 即证明队列处理完毕）。
    let expected_dropped = (2 + pushes - MAX_BUFFERED_NOTIFICATIONS) as u64;
    for _ in 0..200 {
        if lc.lsp().dropped_notifications().await == expected_dropped {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        lc.lsp().dropped_notifications().await,
        expected_dropped,
        "通知缓冲溢出计数未达到预期"
    );
    let drained = lc.lsp().drain_notifications().await;
    assert_eq!(drained.len(), MAX_BUFFERED_NOTIFICATIONS);
    assert_eq!(lc.lsp().dropped_notifications().await, expected_dropped);
    assert!(lc.lsp().drain_notifications().await.is_empty());
    // 覆盖后仍是每 URI 最新值。
    let snap = lc.lsp().diagnostics_snapshot().await;
    assert_eq!(snap.len(), 1);
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn did_open_emits_notification_and_pushes_diagnostics() {
    let diag_params = json!({
        "uri": "file:///a.rs",
        "version": 0,
        "diagnostics": [{
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } },
            "severity": 1,
            "message": "unused"
        }]
    });
    let diag_params_for_closure = diag_params.clone();
    let handler = Arc::new(
        move |method: &str, _params: &serde_json::Value, _has_id: bool| {
            if method == "textDocument/didOpen" {
                MockAction::Notify(
                    "textDocument/publishDiagnostics".to_string(),
                    diag_params_for_closure.clone(),
                )
            } else {
                MockAction::Respond(serde_json::Value::Null)
            }
        },
    );
    let lc = make_client(handler).await;
    lc.did_open("file:///a.rs", "rust", "fn main() {}\n")
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let diags = inline(lc.diagnostics("file:///a.rs").await.unwrap());
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "unused");
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn did_change_full_then_incremental_then_close() {
    let handler = route_handler(vec![]);
    let lc = make_client(handler).await;
    lc.did_open("file:///a.rs", "rust", "hello world")
        .await
        .unwrap();
    lc.did_change_full("file:///a.rs", "hello rust")
        .await
        .unwrap();
    lc.did_change_incremental(
        "file:///a.rs",
        Range::new(Position::new(0, 6), Position::new(0, 10)),
        "pawork",
    )
    .await
    .unwrap();
    lc.did_close("file:///a.rs").await.unwrap();
    let err = lc.did_change_full("file:///a.rs", "x").await.unwrap_err();
    assert!(matches!(err, lsp_runtime::LspError::InvalidState(_)));
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn did_change_on_unopened_document_errors() {
    let handler = route_handler(vec![]);
    let lc = make_client(handler).await;
    let err = lc
        .did_change_full("file:///missing.rs", "x")
        .await
        .unwrap_err();
    assert!(matches!(err, lsp_runtime::LspError::InvalidState(_)));
    lc.shutdown().await.unwrap();
}

#[tokio::test]
async fn incremental_change_with_utf16_positions_and_emoji() {
    // 捕获 didChange 参数，验证 UTF-16 列范围被正确发送且不 panic。
    let captured: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    let handler = Arc::new(
        move |method: &str, params: &serde_json::Value, _has_id: bool| {
            if method == "textDocument/didChange" {
                captured_clone.lock().unwrap().push(params.clone());
            }
            MockAction::Respond(serde_json::Value::Null)
        },
    );
    let lc = make_client(handler).await;
    lc.did_open("file:///a.rs", "rust", "let s = \"😀中a\";")
        .await
        .unwrap();
    // 😀 占 UTF-16 units 9..11，中 = 11，a = 12；替换 units 9..12（😀中）为 "OK"。
    lc.did_change_incremental(
        "file:///a.rs",
        Range::new(Position::new(0, 9), Position::new(0, 12)),
        "OK",
    )
    .await
    .unwrap();
    wait_until(|| !captured.lock().unwrap().is_empty()).await;
    let params = captured.lock().unwrap().last().cloned().unwrap();
    assert_eq!(params["textDocument"]["version"], 1);
    assert_eq!(
        params["contentChanges"][0]["range"]["start"]["character"],
        9
    );
    assert_eq!(params["contentChanges"][0]["range"]["end"]["character"], 12);
    assert_eq!(params["contentChanges"][0]["text"], "OK");
    // 客户端文档状态更新正确（版本推进、无 panic）——文本内容在 doc.rs 单测断言。
    lc.did_change_incremental(
        "file:///a.rs",
        Range::new(Position::new(0, 12), Position::new(0, 14)),
        "🙂",
    )
    .await
    .unwrap();
    wait_until(|| captured.lock().unwrap().len() >= 2).await;
    assert_eq!(captured.lock().unwrap().len(), 2);
    lc.shutdown().await.unwrap();
}
