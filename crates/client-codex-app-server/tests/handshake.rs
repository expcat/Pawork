//! 握手：initialize / initialized、握手前拒绝、重复 initialize。

mod common;

use client_codex_app_server::{
    HandshakeState, JsonRpcMessage, ERROR_ALREADY_INITIALIZED, ERROR_NOT_INITIALIZED,
};
use serde_json::json;

use common::{fixture, handshake, initialize_params, new_host};

#[tokio::test]
async fn initialize_then_initialized_reaches_ready() {
    let host = new_host().await;
    assert_eq!(host.handshake_state(), HandshakeState::Uninitialized);
    let result = host
        .handle_request(json!(0), "initialize", Some(initialize_params()))
        .await
        .expect("initialize");
    assert_eq!(
        result,
        fixture("tests/fixtures/2026-08/initialize-response.json")
    );
    assert_eq!(
        host.handshake_state(),
        HandshakeState::WaitingForInitialized
    );
    host.handle_notification("initialized", None)
        .await
        .expect("initialized");
    assert!(host.is_initialized());
    assert_eq!(host.handshake_state(), HandshakeState::Ready);
}

#[tokio::test]
async fn requests_before_handshake_are_not_initialized() {
    let host = new_host().await;
    let error = host
        .handle_request(
            json!(1),
            "thread/start",
            Some(json!({ "cwd": common::TEST_CWD })),
        )
        .await
        .expect_err("must reject");
    assert_eq!(error.message, ERROR_NOT_INITIALIZED);
    assert_eq!(
        json!({ "id": 1, "error": { "code": error.code, "message": error.message } }),
        fixture("tests/fixtures/2026-08/error-not-initialized.json")
    );
}

#[tokio::test]
async fn duplicate_initialize_is_already_initialized() {
    let host = new_host().await;
    handshake(&host).await;
    let error = host
        .handle_request(json!(2), "initialize", Some(initialize_params()))
        .await
        .expect_err("repeat initialize");
    assert_eq!(error.message, ERROR_ALREADY_INITIALIZED);
    assert_eq!(
        json!({ "id": 2, "error": { "code": error.code, "message": error.message } }),
        fixture("tests/fixtures/2026-08/error-already-initialized.json")
    );
}

#[tokio::test]
async fn jsonl_omits_jsonrpc_and_parses_initialize() {
    let host = new_host().await;
    let line = serde_json::to_string(&fixture("tests/fixtures/2026-08/initialize-request.json"))
        .expect("serialize");
    let out = host.handle_line(&line).await;
    assert_eq!(out.len(), 1);
    let value: serde_json::Value = serde_json::from_str(&out[0]).expect("json");
    assert!(value.get("jsonrpc").is_none());
    assert_eq!(
        value["result"],
        fixture("tests/fixtures/2026-08/initialize-response.json")
    );
}

#[tokio::test]
async fn jsonrpc_field_on_wire_is_rejected() {
    let host = new_host().await;
    let out = host
        .handle_line(r#"{"jsonrpc":"2.0","id":0,"method":"initialize"}"#)
        .await;
    let message =
        JsonRpcMessage::parse(serde_json::from_str(&out[0]).expect("json")).expect("parse");
    match message {
        JsonRpcMessage::Error(error) => {
            assert!(error.error.message.contains("jsonrpc"));
        }
        other => panic!("expected error, got {other:?}"),
    }
}
