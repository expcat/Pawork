//! versioned golden fixtures：一条重要线协议消息一个 fixture。

mod common;

use client_codex_app_server::wire::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest};
use serde_json::{json, Value};

use common::fixture;

fn assert_fixture_omits_jsonrpc(value: &Value) {
    assert!(
        value.get("jsonrpc").is_none(),
        "golden must omit jsonrpc: {value}"
    );
}

#[test]
fn golden_initialize_request() {
    let value = fixture("tests/fixtures/2026-08/initialize-request.json");
    assert_fixture_omits_jsonrpc(&value);
    let JsonRpcMessage::Request(JsonRpcRequest { method, .. }) =
        JsonRpcMessage::parse(value).expect("parse")
    else {
        panic!("expected request");
    };
    assert_eq!(method, "initialize");
}

#[test]
fn golden_initialized_notification() {
    let value = fixture("tests/fixtures/2026-08/initialized-notification.json");
    assert_fixture_omits_jsonrpc(&value);
    let JsonRpcMessage::Notification(JsonRpcNotification { method, params }) =
        JsonRpcMessage::parse(value).expect("parse")
    else {
        panic!("expected notification");
    };
    assert_eq!(method, "initialized");
    assert!(params.is_none());
}

#[test]
fn golden_thread_and_turn_requests() {
    for (path, method) in [
        (
            "tests/fixtures/2026-08/thread-start-request.json",
            "thread/start",
        ),
        (
            "tests/fixtures/2026-08/thread-resume-request.json",
            "thread/resume",
        ),
        (
            "tests/fixtures/2026-08/thread-fork-request.json",
            "thread/fork",
        ),
        (
            "tests/fixtures/2026-08/turn-start-request.json",
            "turn/start",
        ),
        (
            "tests/fixtures/2026-08/turn-interrupt-request.json",
            "turn/interrupt",
        ),
        (
            "tests/fixtures/2026-08/compact-start-request.json",
            "thread/compact/start",
        ),
    ] {
        let value = fixture(path);
        assert_fixture_omits_jsonrpc(&value);
        let JsonRpcMessage::Request(request) = JsonRpcMessage::parse(value).expect(path) else {
            panic!("{path} expected request");
        };
        assert_eq!(request.method, method, "{path}");
    }
}

#[test]
fn golden_item_and_turn_notifications() {
    for (path, method) in [
        (
            "tests/fixtures/2026-08/thread-started-notification.json",
            "thread/started",
        ),
        (
            "tests/fixtures/2026-08/turn-started-notification.json",
            "turn/started",
        ),
        (
            "tests/fixtures/2026-08/item-agent-message-delta.json",
            "item/agentMessage/delta",
        ),
        (
            "tests/fixtures/2026-08/item-started-command.json",
            "item/started",
        ),
        (
            "tests/fixtures/2026-08/item-completed-command.json",
            "item/completed",
        ),
        (
            "tests/fixtures/2026-08/turn-completed-notification.json",
            "turn/completed",
        ),
        (
            "tests/fixtures/2026-08/item-context-compaction.json",
            "item/started",
        ),
    ] {
        let value = fixture(path);
        assert_fixture_omits_jsonrpc(&value);
        let JsonRpcMessage::Notification(notification) = JsonRpcMessage::parse(value).expect(path)
        else {
            panic!("{path} expected notification");
        };
        assert_eq!(notification.method, method, "{path}");
    }
}

#[test]
fn golden_approval_is_a_request() {
    let value = fixture("tests/fixtures/2026-08/approval-request.json");
    assert_fixture_omits_jsonrpc(&value);
    let JsonRpcMessage::Request(request) = JsonRpcMessage::parse(value).expect("parse") else {
        panic!("approval must be a request");
    };
    assert_eq!(request.method, "item/commandExecution/requestApproval");
    assert_eq!(request.params.as_ref().unwrap()["threadId"], "thr_1");
    assert_eq!(request.params.as_ref().unwrap()["turnId"], "turn_1");
    assert_eq!(request.params.as_ref().unwrap()["itemId"], "item_cmd");
}

#[test]
fn golden_approval_result_is_decision_only() {
    let value = fixture("tests/fixtures/2026-08/approval-result.json");
    assert_eq!(value, json!({ "decision": "accept" }));
}

#[test]
fn golden_context_compaction_is_not_legacy_compacted() {
    let value = fixture("tests/fixtures/2026-08/item-context-compaction.json");
    assert_eq!(value["params"]["item"]["type"], "contextCompaction");
    assert_ne!(value["method"], "thread/compacted");
}

#[test]
fn golden_subagent_preserves_parent_thread_id() {
    let value = fixture("tests/fixtures/2026-08/subagent-thread.json");
    assert_eq!(value["id"], "thr_child");
    assert_eq!(value["parentThreadId"], "thr_parent");
}
