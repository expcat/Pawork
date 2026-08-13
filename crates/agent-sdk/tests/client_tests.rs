//! agent-sdk 契约测试：mock transport 驱动握手、往返、事件流、背压、
//! 取消与 compat 入口；JSON fixture 覆盖固定协议样例。

use agent_domain::{EventId, QueryId, RunId, SessionId, Timestamp, WorkspaceId};
use agent_sdk::mock::MockTransport;
use agent_sdk::{BackpressurePolicy, PaworkClient, PaworkOptions, SdkError, SdkErrorKind};
use core_api::{AppQuery, AppResponse, EventStream, GlobalSequence, RunState};
use headless_json::{CompatSource, HeadlessResponse, ProtocolErrorKind};
use serde_json::{json, Value};
use std::time::Duration;

fn fixture_text(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read fixture {path}: {error}"))
}

fn hello_ack_line() -> String {
    fixture_text("hello_ack.json").trim().to_string()
}

fn hello_ack() -> MockTransport {
    MockTransport::new().push_response(hello_ack_line())
}

/// 建 client：把 HelloAck 预置进队列后握手。
async fn connect(mock: &MockTransport) -> PaworkClient {
    PaworkClient::from_transport(
        Box::new(mock.clone()),
        PaworkOptions {
            client_name: "agent-sdk-test".into(),
            ..PaworkOptions::default()
        },
    )
    .await
    .expect("handshake succeeds")
}

/// 建 client 并断言握手失败，返回错误。
async fn connect_err(mock: &MockTransport) -> SdkError {
    match PaworkClient::from_transport(Box::new(mock.clone()), PaworkOptions::default()).await {
        Ok(_) => panic!("handshake unexpectedly succeeded"),
        Err(error) => error,
    }
}

fn event_line(stream: EventStream, payload: Value, sequence: u64) -> String {
    let frame = HeadlessResponse::Event {
        envelope: core_api::AppEventEnvelope {
            api_version: core_api::API_VERSION,
            instance_id: "core-fixture-1".into(),
            event_id: format!("evt-{sequence}").into(),
            global_sequence: GlobalSequence(sequence),
            stream,
            stream_sequence: sequence,
            timestamp: Timestamp::from_unix_millis(1700000000000 + sequence),
            source: core_api::EventSource::Core,
            payload: serde_json::from_value(payload).expect("payload is an AppEvent"),
        },
    };
    serde_json::to_string(&frame).expect("encode event")
}

fn run_changed(run: &str, state: &str) -> Value {
    json!({"type": "run_changed", "data": {"run_id": run, "state": state}})
}

#[tokio::test]
async fn handshake_exposes_version_instance_and_capabilities() {
    let mock = hello_ack();
    let client = connect(&mock).await;
    assert_eq!(client.api_version().await, Some(core_api::API_VERSION));
    assert_eq!(
        client.instance_id().await.as_deref(),
        Some("core-fixture-1")
    );
    let capabilities = client.capabilities().await;
    assert!(capabilities.contains(&headless_json::SdkCapability::Sessions));
    assert!(capabilities.contains(&headless_json::SdkCapability::CompatImport));
    let sent = mock.sent_lines();
    assert_eq!(sent.len(), 1, "only hello was sent");
    let hello: Value = serde_json::from_str(&sent[0]).expect("hello frame");
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["client_name"], "agent-sdk-test");
}

#[tokio::test]
async fn handshake_fails_explicitly_on_incompatible_version() {
    let mock = MockTransport::new().push_response(
        r#"{"type":"error","kind":"incompatible_api_version","message":"host only supports major 1"}"#,
    );
    let error = connect_err(&mock).await;
    assert_eq!(
        error.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::IncompatibleApiVersion),
        "{error}"
    );
}

#[tokio::test]
async fn handshake_fails_on_unknown_response_type() {
    let mock = MockTransport::new().push_response(r#"{"type":"teleport","params":{}}"#);
    let error = connect_err(&mock).await;
    assert_eq!(error.kind(), SdkErrorKind::UnknownResponseType, "{error}");
}

#[tokio::test]
async fn create_session_roundtrip_uses_command_framing() {
    let mock = hello_ack().push_response(fixture_text("session_response.json"));
    let client = connect(&mock).await;
    let session = client
        .create_session(WorkspaceId::from("ws-1"), Some("demo".into()))
        .await
        .expect("create session");
    assert_eq!(session.session_id, SessionId::from("s-1"));
    assert_eq!(session.title, "demo");
    assert!(session.open);

    // 第二条发送行必须是 session_create 命令信封。
    let sent: Value = serde_json::from_str(&mock.sent_lines()[1]).expect("command frame");
    assert_eq!(sent["type"], "command");
    assert_eq!(sent["envelope"]["command"]["method"], "session_create");
    assert_eq!(
        sent["envelope"]["source"]["type"], "automation",
        "SDK 以 Automation 身份连接，不冒充 GUI"
    );
}

#[tokio::test]
async fn query_run_status_roundtrip() {
    let mock = hello_ack().push_response(
        json!({
            "type": "response",
            "envelope": {
                "api_version": {"major": 1, "minor": 0},
                "request_id": "qry-1",
                "responded_at": 1700000000000u64,
                "response": {
                    "type": "data",
                    "data": {
                        "run_id": "run-9",
                        "session_id": "s-1",
                        "model": "test-model",
                        "provider_id": "test-provider",
                        "state": "completed",
                        "created_at": 1700000000000u64,
                        "message_count": 2,
                        "revision": 3
                    }
                }
            }
        })
        .to_string(),
    );
    let client = connect(&mock).await;
    let run = client
        .run_status(RunId::from("run-9"))
        .await
        .expect("run status");
    assert_eq!(run.run_id, RunId::from("run-9"));
    assert_eq!(run.state, RunState::Completed);
}

#[tokio::test]
async fn cancel_run_roundtrip() {
    let mock = hello_ack().push_response(
        json!({
            "type": "response",
            "envelope": {
                "api_version": {"major": 1, "minor": 0},
                "request_id": "cmd-1",
                "responded_at": 1700000000000u64,
                "response": {
                    "type": "data",
                    "data": {"run_id": "run-9", "cancelled": true, "already_cancelled": false}
                }
            }
        })
        .to_string(),
    );
    let client = connect(&mock).await;
    let outcome = client.cancel(RunId::from("run-9")).await.expect("cancel");
    assert!(outcome.cancelled);
    assert!(!outcome.already_cancelled);
    let sent: Value = serde_json::from_str(&mock.sent_lines()[1]).expect("cancel frame");
    assert_eq!(sent["envelope"]["command"]["method"], "run_cancel");
}

#[tokio::test]
async fn host_business_error_maps_to_request_failed() {
    let mock = hello_ack().push_response(
        json!({
            "type": "response",
            "envelope": {
                "api_version": {"major": 1, "minor": 0},
                "request_id": "cmd-1",
                "responded_at": 1700000000000u64,
                "response": {
                    "type": "error",
                    "data": {
                        "category": "not_found",
                        "message": "session s-missing does not exist",
                        "retryable": false,
                        "diagnostics": {}
                    }
                }
            }
        })
        .to_string(),
    );
    let client = connect(&mock).await;
    let error = client
        .create_session(WorkspaceId::from("ws-1"), None)
        .await
        .expect_err("must fail");
    assert_eq!(error.kind(), SdkErrorKind::RequestFailed, "{error}");
    assert!(error.to_string().contains("session s-missing"));
}

#[tokio::test]
async fn host_error_frame_carries_explicit_kind() {
    let mock = hello_ack().push_response(
        json!({
            "type": "error",
            "request_id": "qry-1",
            "kind": "unsupported_capability",
            "message": "capability not granted"
        })
        .to_string(),
    );
    let client = connect(&mock).await;
    let error = client
        .query(AppQuery::WorkspaceList)
        .await
        .expect_err("must fail");
    assert_eq!(
        error.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::UnsupportedCapability),
        "{error}"
    );
}

#[tokio::test]
async fn no_id_error_frame_is_not_misrouted_to_unique_pending() {
    // 短超时：无 id 的 error 帧（如 Host lagged/backpressure）不得按
    // 「唯一 pending」兜底误配到在途请求；请求应超时，error 帧计入 unmatched。
    let mock = hello_ack();
    let client = PaworkClient::from_transport(
        Box::new(mock.clone()),
        PaworkOptions {
            client_name: "agent-sdk-test".into(),
            timeout: Duration::from_millis(300),
            ..PaworkOptions::default()
        },
    )
    .await
    .expect("handshake succeeds");

    mock.clone().push_response(
        r#"{"type":"error","kind":"backpressure","message":"event stream lagged behind by 3 events"}"#,
    );
    let error = client
        .query(AppQuery::WorkspaceList)
        .await
        .expect_err("no-id error frame must not satisfy the pending request");
    assert_eq!(error.kind(), SdkErrorKind::Timeout, "{error}");
    assert!(
        client.unmatched_error_count().await >= 1,
        "no-id error frame is counted as unmatched, not dropped silently"
    );
}

#[tokio::test]
async fn subscription_routes_only_matching_streams() {
    let mock = hello_ack();
    let client = connect(&mock).await;
    let run_stream = EventStream::Run(RunId::from("run-9"));
    let mut sub = client
        .subscribe(run_stream.clone(), BackpressurePolicy::Drop, 16)
        .await
        .expect("subscribe");
    let mut global = client
        .subscribe(EventStream::Global, BackpressurePolicy::Drop, 16)
        .await
        .expect("subscribe global");

    mock.clone().push_event(event_line(
        run_stream.clone(),
        run_changed("run-9", "preparing_context"),
        1,
    ));
    mock.clone().push_event(event_line(
        EventStream::Session(SessionId::from("s-1")),
        run_changed("run-9", "completed"),
        2,
    ));

    let first = sub.next_event().await.expect("first run event");
    assert_eq!(
        serde_json::to_value(&first.payload).unwrap()["type"],
        "run_changed"
    );
    // 非本流事件不投递。
    assert!(sub.try_next().expect("try").is_none());
    // Global 订阅收到全部两条。
    let g1 = global.next_event().await.expect("global 1");
    let g2 = global.next_event().await.expect("global 2");
    assert_ne!(
        serde_json::to_value(&g1.stream).unwrap(),
        serde_json::to_value(&g2.stream).unwrap()
    );
}

#[tokio::test]
async fn backpressure_drop_policy_counts_dropped_events() {
    let mock = hello_ack();
    let client = connect(&mock).await;
    let run_stream = EventStream::Run(RunId::from("run-9"));
    let mut sub = client
        .subscribe(run_stream.clone(), BackpressurePolicy::Drop, 2)
        .await
        .expect("subscribe");
    for sequence in 1..=5 {
        mock.clone().push_event(event_line(
            run_stream.clone(),
            run_changed("run-9", "streaming_response"),
            sequence,
        ));
    }
    // 通道容量 2：等路由完成后读 2 条，剩余 3 条被丢弃并计数。
    wait_until(|| sub.dropped_events() >= 3).await;
    let _ = sub.next_event().await.expect("e1");
    let _ = sub.next_event().await.expect("e2");
    assert_eq!(sub.dropped_events(), 3);
    assert!(sub.try_next().expect("drained").is_none());
}

#[tokio::test]
async fn backpressure_error_policy_surfaces_overflow() {
    let mock = hello_ack();
    let client = connect(&mock).await;
    let run_stream = EventStream::Run(RunId::from("run-9"));
    let mut sub = client
        .subscribe(run_stream.clone(), BackpressurePolicy::Error, 1)
        .await
        .expect("subscribe");
    for sequence in 1..=3 {
        mock.clone().push_event(event_line(
            run_stream.clone(),
            run_changed("run-9", "streaming_response"),
            sequence,
        ));
    }
    wait_until(|| sub.dropped_events() >= 2).await;
    let _ = sub.next_event().await.expect("first fits");
    let error = sub.next_event().await.expect_err("overflow surfaces");
    assert_eq!(error.kind(), SdkErrorKind::Backpressure, "{error}");
    assert_eq!(sub.dropped_events(), 2);
}

#[tokio::test]
async fn unsubscribe_removes_slot() {
    let mock = hello_ack();
    let client = connect(&mock).await;
    let stream = EventStream::Run(RunId::from("run-9"));
    let mut sub = client
        .subscribe(stream.clone(), BackpressurePolicy::Drop, 8)
        .await
        .expect("subscribe");
    let removed = client.unsubscribe(stream.clone()).await;
    assert_eq!(removed, 1);
    // 已移除：事件不再投递，订阅读到 Cancelled（sender 已随槽位移除）。
    mock.clone().push_event(event_line(
        stream,
        run_changed("run-9", "streaming_response"),
        1,
    ));
    let error = sub.next_event().await.expect_err("cancelled");
    assert_eq!(error.kind(), SdkErrorKind::Cancelled, "{error}");
}

#[tokio::test]
async fn fork_and_resume_lifecycle() {
    let second_response = {
        let mut value: Value =
            serde_json::from_str(&fixture_text("session_response.json")).expect("session fixture");
        value["envelope"]["request_id"] = json!("cmd-2");
        value.to_string()
    };
    let mock = hello_ack()
        .push_response(fixture_text("session_response.json"))
        .push_response(second_response);
    let client = connect(&mock).await;

    let forked = client
        .fork(SessionId::from("s-1"), EventId::from("evt-7"))
        .await
        .expect("fork");
    assert_eq!(forked.session_id, SessionId::from("s-1"));

    let (session, mut subscription) = client
        .resume(SessionId::from("s-1"), 16)
        .await
        .expect("resume");
    assert_eq!(session.session_id, SessionId::from("s-1"));
    assert_eq!(subscription.stream_label(), "session/s-1");

    mock.clone().push_event(event_line(
        EventStream::Session(SessionId::from("s-1")),
        run_changed("run-9", "completed"),
        9,
    ));
    let event = subscription.next_event().await.expect("resumed event");
    assert_eq!(event.stream_sequence, 9);
}

#[tokio::test]
async fn compat_import_and_history_roundtrip() {
    let mock = hello_ack()
        .push_response(fixture_text("compat_import_response.json"))
        .push_response(
            json!({
                "type": "compat_history_result",
                "request_id": "compat-history-2",
                "entries": [{
                    "session_id": "s-imported-1",
                    "source": "cursor",
                    "original_id": "cur-7",
                    "imported_events": 2,
                    "imported_at_unix_ms": 1700000000000u64
                }],
                "cursor": "next-page"
            })
            .to_string(),
        );
    let client = connect(&mock).await;

    let outcome = client
        .import_compat(CompatSource::Codex, r#"{"rollout": []}"#.into(), false)
        .await
        .expect("import");
    assert_eq!(outcome.report.session_id, "s-imported-1");
    assert_eq!(outcome.report.imported_events, 5);

    let page = client
        .compat_history(Some(10), None)
        .await
        .expect("history");
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].source, CompatSource::Cursor);
    assert_eq!(page.cursor.as_deref(), Some("next-page"));
}

#[tokio::test]
async fn close_cancels_inflight_and_later_requests() {
    let mock = hello_ack();
    let client = connect(&mock).await;
    client.close().await.expect("close");
    let error = client
        .query(AppQuery::WorkspaceList)
        .await
        .expect_err("closed client rejects requests");
    assert_eq!(error.kind(), SdkErrorKind::Io, "{error}");
    assert!(!client.is_open());
}

#[tokio::test]
async fn fixture_run_events_stream_end_to_end() {
    let mock = hello_ack();
    let client = connect(&mock).await;
    let run_stream = EventStream::Run(RunId::from("run-9"));
    let mut sub = client
        .subscribe(run_stream.clone(), BackpressurePolicy::Drop, 16)
        .await
        .expect("subscribe");
    let lines = fixture_text("run_events.jsonl");
    for line in lines.lines() {
        mock.clone().push_event(line.to_string());
    }
    let mut payload_types = Vec::new();
    while let Some(event) = sub.try_next().expect("try") {
        payload_types.push(serde_json::to_value(&event.payload).unwrap()["type"].clone());
    }
    // 事件可能尚未全部路由：用 next_event 收满 3 条。
    for expected in ["run_changed", "assistant_delta", "run_changed"] {
        let event = sub.next_event().await.expect("fixture event");
        assert_eq!(
            serde_json::to_value(&event.payload).unwrap()["type"],
            expected
        );
    }
    let _ = payload_types;
}

#[tokio::test]
async fn fixture_error_frames_are_explicit() {
    let fixtures = fixture_text("error_frames.json");
    let fixtures: Value = serde_json::from_str(&fixtures).expect("error fixtures");

    // unsupported_capability（带 request_id → 关联到在途查询）。
    let mock = hello_ack().push_response(fixtures["unsupported_capability"].to_string());
    let client = connect(&mock).await;
    let error = client
        .query(AppQuery::WorkspaceList)
        .await
        .expect_err("unsupported must fail");
    assert_eq!(
        error.kind(),
        SdkErrorKind::Protocol(ProtocolErrorKind::UnsupportedCapability),
        "{error}"
    );

    // unknown response type。
    let mock = hello_ack().push_response(fixtures["unknown_type"].to_string());
    let client = connect(&mock).await;
    let error = client
        .query(AppQuery::WorkspaceList)
        .await
        .expect_err("unknown must fail");
    assert_eq!(error.kind(), SdkErrorKind::UnknownResponseType, "{error}");
}

#[tokio::test]
async fn fixture_hello_and_session_views() {
    let mock = hello_ack().push_response(fixture_text("session_response.json"));
    let client = connect(&mock).await;
    assert_eq!(
        client.instance_id().await.as_deref(),
        Some("core-fixture-1")
    );
    let session = client
        .create_session(WorkspaceId::from("ws-1"), None)
        .await
        .expect("fixture session");
    assert_eq!(session.session_id, SessionId::from("s-1"));
    assert_eq!(session.workspace_id, WorkspaceId::from("ws-1"));
    assert_eq!(session.revision, 1);
}

#[tokio::test]
async fn raw_query_envelope_returns_app_response() {
    let mock = hello_ack().push_response(
        json!({
            "type": "response",
            "envelope": {
                "api_version": {"major": 1, "minor": 0},
                "request_id": "qry-1",
                "responded_at": 1700000000000u64,
                "response": {"type": "data", "data": {"workspaces": []}}
            }
        })
        .to_string(),
    );
    let client = connect(&mock).await;
    let envelope = client.query(AppQuery::WorkspaceList).await.expect("query");
    assert!(matches!(envelope.response, AppResponse::Data(_)));
    assert_eq!(envelope.request_id, QueryId::from("qry-1"));
}

#[tokio::test]
async fn mock_contract_records_sent_lines_in_order() {
    let mock = hello_ack();
    let _client = connect(&mock).await;
    let sent = mock.sent_lines();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("\"type\":\"hello\""));
    assert!(sent[0].contains("\"client_version\":\""));
}

#[tokio::test]
async fn sdk_error_kind_is_stable_label() {
    assert_eq!(SdkErrorKind::Backpressure.as_str(), "backpressure");
    assert_eq!(
        SdkErrorKind::UnknownResponseType.as_str(),
        "unknown_response_type"
    );
    assert!(SdkErrorKind::Io.is_retryable());
    assert!(!SdkErrorKind::RequestFailed.is_retryable());
}

/// 轮询直到条件成立（读者任务异步路由事件，避免时序断言）。
async fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition not met within 2s");
}
