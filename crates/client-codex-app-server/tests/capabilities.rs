//! 未协商能力在使用点显式失败：compaction / tool.namespace / experimentalApi。

mod common;

use std::sync::Arc;

use client_adapter_api::{
    AdapterError, ClientAdapter, ClientCapability, ClientFrame, ClientProtocol, ClientSessionId,
    InMemorySessionRegistryStore, SessionRegistry, CLIENT_ADAPTER_SCHEMA_VERSION,
};
use client_codex_app_server::{
    CodexAppServerAdapterFactory, CodexAppServerHost, CAP_COMPACTION, CAP_EXPERIMENTAL_API,
    CAP_TOOL_NAMESPACE, PROTOCOL_NAME, PROTOCOL_VERSION,
};
use serde_json::json;

use common::{
    handshake, initialize_params_experimental, new_host, test_runtime, FixedCwdResolver,
    FixedSessionResolver, MockCore, TEST_CWD,
};

async fn host_without_optional_caps() -> CodexAppServerHost {
    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let factory = CodexAppServerAdapterFactory::new(
        std::iter::empty::<ClientCapability>(),
        Arc::clone(&registry),
        Arc::new(FixedCwdResolver),
        Arc::new(FixedSessionResolver(ClientSessionId::new("thr_1"))),
    );
    CodexAppServerHost::with_runtime(factory, registry, MockCore::new(), test_runtime())
}

#[tokio::test]
async fn compact_without_capability_is_capability_unsupported() {
    let host = host_without_optional_caps().await;
    host.handle_request(json!(0), "initialize", Some(common::initialize_params()))
        .await
        .expect("initialize");
    host.handle_notification("initialized", None)
        .await
        .expect("initialized");
    assert!(host
        .degraded_capabilities()
        .contains(&ClientCapability::new(CAP_COMPACTION)));
    let error = host
        .handle_request(
            json!(25),
            "thread/compact/start",
            Some(json!({ "threadId": "thr_1" })),
        )
        .await
        .expect_err("compact gated");
    assert!(
        error.message.contains("compaction") || error.message.contains("CapabilityUnsupported"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn tool_namespace_without_capability_is_capability_unsupported() {
    let host = new_host().await;
    handshake(&host).await;
    let error = host
        .handle_request(
            json!(10),
            "thread/start",
            Some(json!({
                "cwd": TEST_CWD,
                "dynamicTools": [{ "type": "namespace", "name": "tickets", "tools": [] }]
            })),
        )
        .await
        .expect_err("tool namespace gated");
    assert!(
        error.message.contains("tool.namespace") || error.message.contains("CapabilityUnsupported"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn experimental_thread_list_filters_require_opt_in() {
    let host = new_host().await;
    handshake(&host).await;
    let error = host
        .handle_request(
            json!(20),
            "thread/list",
            Some(json!({ "parentThreadId": "thr_1" })),
        )
        .await
        .expect_err("experimental filter gated");
    assert!(
        error.message.contains("experimentalApi")
            || error.message.contains("CapabilityUnsupported"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn experimental_opt_in_still_fails_closed_on_thread_list() {
    let host = new_host().await;
    host.handle_request(
        json!(0),
        "initialize",
        Some(initialize_params_experimental()),
    )
    .await
    .expect("initialize");
    host.handle_notification("initialized", None)
        .await
        .expect("initialized");
    let error = host
        .handle_request(
            json!(20),
            "thread/list",
            Some(json!({ "parentThreadId": "thr_1" })),
        )
        .await
        .expect_err("list itself unsupported");
    assert!(error.message.contains("thread/list"), "{}", error.message);
}

#[tokio::test]
async fn compact_with_capability_maps_to_session_compact() {
    let host = new_host().await;
    handshake(&host).await;
    host.handle_request(json!(10), "thread/start", Some(json!({ "cwd": TEST_CWD })))
        .await
        .expect("thread/start");
    let result = host
        .handle_request(
            json!(25),
            "thread/compact/start",
            Some(json!({ "threadId": "thr_1" })),
        )
        .await
        .expect("compact");
    assert_eq!(result, json!({}));
}

#[tokio::test]
async fn deprecated_thread_compacted_is_not_treated_as_context_compaction() {
    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let factory = CodexAppServerAdapterFactory::with_defaults(
        registry,
        Arc::new(FixedCwdResolver),
        Arc::new(FixedSessionResolver(ClientSessionId::new("thr_1"))),
    );
    let adapter = factory
        .create_concrete(client_adapter_api::CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(PROTOCOL_NAME),
            protocol_version: PROTOCOL_VERSION.into(),
            client_version: "0.0.0".into(),
            revision: 1,
            capabilities: Default::default(),
        })
        .expect("negotiate")
        .adapter;
    let error = adapter
        .decode(ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "1".into(),
            method: "thread/compacted".into(),
            payload: json!({ "threadId": "thr_1", "turnId": "turn_1" }),
            extensions: Default::default(),
        })
        .await
        .expect_err("deprecated compacted");
    assert!(
        matches!(error, AdapterError::ProtocolUnsupported(message) if message.contains("deprecated"))
    );
}

#[tokio::test]
async fn unknown_method_is_protocol_unsupported() {
    let host = new_host().await;
    handshake(&host).await;
    let error = host
        .handle_request(json!(9), "session/load", Some(json!({})))
        .await
        .expect_err("unknown method");
    assert_eq!(
        json!({ "id": 9, "error": { "code": error.code, "message": error.message } }),
        common::fixture("tests/fixtures/2026-08/error-unknown-method.json")
    );
}

#[tokio::test]
async fn turn_steer_is_protocol_unsupported() {
    let host = new_host().await;
    handshake(&host).await;
    host.handle_request(json!(10), "thread/start", Some(json!({ "cwd": TEST_CWD })))
        .await
        .expect("thread/start");
    let error = host
        .handle_request(
            json!(21),
            "turn/steer",
            Some(json!({
                "threadId": "thr_1",
                "input": [{ "type": "text", "text": "keep going" }]
            })),
        )
        .await
        .expect_err("steer has no canonical in-flight injection");
    assert!(
        error.message.contains("turn/steer") || error.message.contains("in-flight"),
        "{}",
        error.message
    );
}

#[test]
fn capability_names_are_stable() {
    assert_eq!(CAP_COMPACTION, "compaction");
    assert_eq!(CAP_TOOL_NAMESPACE, "tool.namespace");
    assert_eq!(CAP_EXPERIMENTAL_API, "experimentalApi");
}
