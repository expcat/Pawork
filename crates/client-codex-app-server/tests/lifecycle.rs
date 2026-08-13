//! Thread / Turn / Item / fork 血缘 / 审批 / interrupt / disconnect / 过载。

mod common;

use client_adapter_api::{
    AdapterError, CanonicalCoreFrame, ClientAdapter, ClientSessionId, ClientSessionRecord,
    ClientSessionState, InMemorySessionRegistryStore, SessionRegistry,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};
use client_codex_app_server::wire::{JsonRpcMessage, ERROR_OVERLOADED, ERROR_OVERLOADED_MESSAGE};
use client_codex_app_server::{
    CodexAppServerAdapterFactory, ThreadLineage, PROTOCOL_NAME, PROTOCOL_VERSION,
};
use core_api::{AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence, RunState};
use serde_json::json;
use std::sync::Arc;

use common::{fixture, handshake, new_host, TEST_CWD};

async fn start_thread(host: &client_codex_app_server::CodexAppServerHost) -> serde_json::Value {
    host.handle_request(json!(10), "thread/start", Some(json!({ "cwd": TEST_CWD })))
        .await
        .expect("thread/start")
}

#[tokio::test]
async fn thread_start_resume_and_fork_preserve_lineage() {
    let host = new_host().await;
    handshake(&host).await;

    let started = start_thread(&host).await;
    assert_eq!(started["thread"]["id"], "thr_1");
    assert_eq!(
        started,
        fixture("tests/fixtures/2026-08/thread-start-response.json")
    );

    let resumed = host
        .handle_request(
            json!(11),
            "thread/resume",
            Some(json!({ "threadId": "thr_1" })),
        )
        .await
        .expect("thread/resume");
    assert_eq!(resumed["thread"]["id"], "thr_1");

    let forked = host
        .handle_request(
            json!(12),
            "thread/fork",
            Some(json!({ "threadId": "thr_1" })),
        )
        .await
        .expect("thread/fork");
    assert_eq!(forked["thread"]["id"], "thr_2");
    assert_eq!(forked["thread"]["forkedFromId"], "thr_1");
    assert_eq!(
        forked,
        fixture("tests/fixtures/2026-08/thread-fork-response.json")
    );
    assert_eq!(
        host.lineage_of("thr_2"),
        ThreadLineage {
            parent_thread_id: None,
            forked_from_id: Some("thr_1".into()),
        }
    );
}

#[tokio::test]
async fn subagent_parent_thread_id_is_preserved() {
    let host = new_host().await;
    handshake(&host).await;
    host.record_lineage(
        "thr_child",
        ThreadLineage {
            parent_thread_id: Some("thr_parent".into()),
            forked_from_id: None,
        },
    );
    assert_eq!(
        client_codex_app_server::map::thread_object("thr_child", &host.lineage_of("thr_child")),
        serde_json::from_value(fixture("tests/fixtures/2026-08/subagent-thread.json"))
            .expect("thread object")
    );
}

#[tokio::test]
async fn turn_start_interrupt_and_item_notifications() {
    let host = new_host().await;
    handshake(&host).await;
    start_thread(&host).await;

    let turn = host
        .handle_request(
            json!(30),
            "turn/start",
            Some(json!({
                "threadId": "thr_1",
                "input": [{ "type": "text", "text": "Run tests" }]
            })),
        )
        .await
        .expect("turn/start");
    assert_eq!(turn["turn"]["id"], "turn_1");
    assert_eq!(turn["turn"]["status"], "inProgress");

    let interrupted = host
        .handle_request(
            json!(31),
            "turn/interrupt",
            Some(json!({ "threadId": "thr_1", "turnId": "turn_1" })),
        )
        .await
        .expect("turn/interrupt");
    assert_eq!(interrupted, json!({}));

    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let factory = CodexAppServerAdapterFactory::with_defaults(
        Arc::clone(&registry),
        Arc::new(common::FixedCwdResolver),
        Arc::new(common::FixedSessionResolver(ClientSessionId::new("thr_1"))),
    );
    let adapter = factory
        .create_concrete(client_adapter_api::CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: client_adapter_api::ClientProtocol::new(PROTOCOL_NAME),
            protocol_version: PROTOCOL_VERSION.into(),
            client_version: "0.0.0".into(),
            revision: 1,
            capabilities: Default::default(),
        })
        .expect("negotiate")
        .adapter;

    let delta = adapter
        .encode(CanonicalCoreFrame::Event(event_envelope(
            EventStream::Run(agent_domain::RunId::from("turn_1")),
            AppEvent::AssistantDelta {
                run_id: agent_domain::RunId::from("turn_1"),
                message_id: agent_domain::MessageId::from("item_msg"),
                delta: "hello".into(),
            },
        )))
        .await
        .expect("encode delta");
    assert_eq!(delta.method, "item/agentMessage/delta");
    assert_eq!(
        json!({ "method": delta.method, "params": delta.payload }),
        fixture("tests/fixtures/2026-08/item-agent-message-delta.json")
    );

    let completed = adapter
        .encode(CanonicalCoreFrame::Event(event_envelope(
            EventStream::Run(agent_domain::RunId::from("turn_1")),
            AppEvent::RunChanged {
                run_id: agent_domain::RunId::from("turn_1"),
                state: RunState::Completed,
            },
        )))
        .await
        .expect("encode completed");
    assert_eq!(completed.method, "turn/completed");
    assert_eq!(
        json!({ "method": "turn/completed", "params": completed.payload }),
        fixture("tests/fixtures/2026-08/turn-completed-notification.json")
    );
}

#[tokio::test]
async fn approval_is_server_to_client_request_not_notification() {
    let host = new_host().await;
    handshake(&host).await;
    start_thread(&host).await;

    let message = host
        .encode_event(event_envelope(
            EventStream::Run(agent_domain::RunId::from("turn_1")),
            AppEvent::ToolApprovalRequired {
                run_id: agent_domain::RunId::from("turn_1"),
                tool_call_id: agent_domain::ToolCallId::from("item_cmd"),
                reason: "run ls".into(),
            },
        ))
        .await
        .expect("approval request");
    let JsonRpcMessage::Request(request) = message else {
        panic!("approval must be a JSON-RPC request, got {message:?}");
    };
    assert_eq!(request.method, "item/commandExecution/requestApproval");
    assert_eq!(
        json!({
            "method": request.method,
            "id": request.id,
            "params": request.params
        }),
        fixture("tests/fixtures/2026-08/approval-request.json")
    );

    let replies = host
        .handle_message(JsonRpcMessage::Response(
            client_codex_app_server::wire::JsonRpcResponse {
                id: request.id,
                result: fixture("tests/fixtures/2026-08/approval-result.json"),
            },
        ))
        .await;
    assert!(
        replies.is_empty(),
        "approval result has no wire reply: {replies:?}"
    );
}

#[tokio::test]
async fn unknown_semantic_field_fails_closed() {
    let host = new_host().await;
    handshake(&host).await;
    let error = host
        .handle_request(
            json!(99),
            "thread/start",
            Some(json!({
                "cwd": TEST_CWD,
                "sandbox": "dangerFullAccess"
            })),
        )
        .await
        .expect_err("unknown semantic field");
    assert!(error.message.contains("sandbox"), "{}", error.message);
}

#[tokio::test]
async fn overload_maps_to_minus_32001() {
    let host = new_host().await;
    handshake(&host).await;
    host.set_ingress_saturated(true);
    let error = host
        .handle_request(json!(7), "thread/start", Some(json!({ "cwd": TEST_CWD })))
        .await
        .expect_err("overloaded");
    assert_eq!(error.code, ERROR_OVERLOADED);
    assert_eq!(error.message, ERROR_OVERLOADED_MESSAGE);
    assert_eq!(
        json!({ "id": 7, "error": { "code": error.code, "message": error.message } }),
        fixture("tests/fixtures/2026-08/error-overloaded.json")
    );
}

#[tokio::test]
async fn disconnect_and_stale_owner_use_session_registry() {
    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let snapshot = client_adapter_api::CapabilitySnapshot {
        schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
        protocol: client_adapter_api::ClientProtocol::new(PROTOCOL_NAME),
        protocol_version: PROTOCOL_VERSION.into(),
        client_version: "0.0.0".into(),
        revision: 1,
        capabilities: Default::default(),
    };
    let record = ClientSessionRecord {
        schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
        protocol: snapshot.protocol.clone(),
        client_session_id: ClientSessionId::new("thr_own"),
        core_session_id: agent_domain::SessionId::from("thr_own"),
        connection_id: agent_domain::ConnectionId::from("conn-1"),
        ownership_epoch: 1,
        revision: 1,
        state: ClientSessionState::Subscribed,
        capabilities: snapshot,
        updated_at: agent_domain::Timestamp::from_unix_millis(1),
    };
    registry.register(record).await.expect("register");
    let disconnected = registry
        .transition(
            &ClientSessionId::new("thr_own"),
            1,
            1,
            ClientSessionState::Disconnected,
            agent_domain::Timestamp::from_unix_millis(2),
        )
        .await
        .expect("disconnect");
    assert_eq!(disconnected.state, ClientSessionState::Disconnected);
    assert_eq!(disconnected.revision, 2);

    let stale = registry
        .claim(
            &ClientSessionId::new("thr_own"),
            1,
            1,
            agent_domain::ConnectionId::from("conn-2"),
            ClientSessionState::Subscribed,
            agent_domain::Timestamp::from_unix_millis(3),
        )
        .await
        .expect_err("stale owner");
    assert!(matches!(stale, AdapterError::StaleOwner { .. }));

    let claimed = registry
        .claim(
            &ClientSessionId::new("thr_own"),
            disconnected.ownership_epoch,
            disconnected.revision,
            agent_domain::ConnectionId::from("conn-2"),
            ClientSessionState::Subscribed,
            agent_domain::Timestamp::from_unix_millis(3),
        )
        .await
        .expect("reattach");
    assert_eq!(claimed.ownership_epoch, 2);
    assert_eq!(claimed.revision, 3);
}

fn event_envelope(stream: EventStream, payload: AppEvent) -> AppEventEnvelope {
    AppEventEnvelope {
        api_version: core_api::API_VERSION,
        instance_id: agent_domain::CoreInstanceId::from("instance-1"),
        event_id: agent_domain::EventId::from("event-1"),
        global_sequence: GlobalSequence(1),
        stream,
        stream_sequence: 1,
        timestamp: agent_domain::Timestamp::from_unix_millis(1),
        source: EventSource::Core,
        payload,
    }
}
