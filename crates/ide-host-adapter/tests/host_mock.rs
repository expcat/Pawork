//! P17-9 定向 mock 测试：IdeHostAdapter 全流程（能力协商 / session/run/event /
//! 取消 / 审批 / diff / 重连 / 边界隔离 / 可选 LSP 输出）。
//!
//! 通道层用 `agent-sdk` 的 `MockTransport`（脚本化响应 + 事件注入），
//! 不 spawn 真实进程；发送帧断言保证 IDE 通道只走 Headless/SDK 帧
//! （不触达 GUI Connection Protocol）。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::{CommandId, EventId, QueryId, RunId, SessionId, Timestamp, WorkspaceId};
use agent_sdk::mock::MockTransport;
use agent_sdk::{PaworkClient, PaworkOptions, Transport};
use async_trait::async_trait;
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppResponse,
    AppResponseEnvelope, ApprovalDecision, CommandSource, EventSource, EventStream, GlobalSequence,
    RunState, API_VERSION,
};
use headless_json::HeadlessResponse;
use ide_host_adapter::{
    IdeAdapterError, IdeCapability, IdeEvent, IdeHostAdapter, IdeHostOptions, IdeRequest,
    LspOutputEncoder, LspQueryKind, LspResultProvider, PaworkSdkChannel,
};
use lsp_runtime::{Diagnostic, DiagnosticSeverity, DocumentDiagnostic, Position, Range};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::timeout;

const ALL_CAPS: [IdeCapability; 4] = [
    IdeCapability::Lifecycle,
    IdeCapability::Diagnostics,
    IdeCapability::Interaction,
    IdeCapability::Reconnect,
];

fn hello_ack(granted: &[&str]) -> String {
    json!({
        "type": "hello_ack",
        "instance_id": "core-fixture-1",
        "negotiated": {"major": 1, "minor": 0},
        "granted": granted,
    })
    .to_string()
}

fn full_hello_ack() -> String {
    hello_ack(&[
        "sessions",
        "runs",
        "streaming",
        "compat_import",
        "compat_history",
    ])
}

fn options(capabilities: &[IdeCapability]) -> IdeHostOptions {
    IdeHostOptions {
        capabilities: capabilities.to_vec(),
        ..IdeHostOptions::default()
    }
}

async fn connect(
    mock: &MockTransport,
    capabilities: &[IdeCapability],
) -> (Arc<IdeHostAdapter>, mpsc::Receiver<IdeEvent>) {
    let client = PaworkClient::from_transport(
        Box::new(mock.clone()),
        PaworkOptions {
            client_name: "ide-host-test".into(),
            ..PaworkOptions::default()
        },
    )
    .await
    .expect("sdk handshake");
    let adapter = IdeHostAdapter::create(
        options(capabilities),
        Box::new(PaworkSdkChannel::new(client)),
    )
    .await
    .expect("adapter create");
    let events = adapter.take_events().expect("event receiver");
    (Arc::new(adapter), events)
}

fn response_line(request_id: &str, response: AppResponse) -> String {
    serde_json::to_string(&HeadlessResponse::Response {
        envelope: AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(request_id),
            responded_at: Timestamp::from_unix_millis(2),
            response,
        },
    })
    .expect("encode response")
}

fn data_line(request_id: &str, data: Value) -> String {
    response_line(request_id, AppResponse::Data(data))
}

/// 解析最近一条已发送帧的 command_id / request_id（用于构造响应帧）。
fn last_sent_request_id(mock: &MockTransport) -> String {
    let sent = mock.sent_lines();
    let line = sent.last().expect("a frame was sent");
    let value: Value = serde_json::from_str(line).expect("sent frame is JSON");
    value["envelope"]["command_id"]
        .as_str()
        .or_else(|| value["envelope"]["request_id"].as_str())
        .expect("frame carries an id")
        .to_string()
}

async fn wait_for_sent(mock: &MockTransport, min_sent: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while mock.sent_count() < min_sent {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timeout waiting for {} sent frames (got {})",
            min_sent,
            mock.sent_count()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn event_line(stream: EventStream, payload: AppEvent, sequence: u64) -> String {
    serde_json::to_string(&HeadlessResponse::Event {
        envelope: AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: "core-fixture-1".into(),
            event_id: EventId::from(format!("evt-{sequence}")),
            global_sequence: GlobalSequence(sequence),
            stream,
            stream_sequence: sequence,
            timestamp: Timestamp::from_unix_millis(1700000000000 + sequence),
            source: EventSource::Core,
            payload,
        },
    })
    .expect("encode event")
}

fn session_view(session_id: &str, revision: u64) -> Value {
    json!({
        "session_id": session_id,
        "workspace_id": "ws-1",
        "title": "ide demo",
        "revision": revision,
        "open": true,
    })
}

async fn recv_event(events: &mut mpsc::Receiver<IdeEvent>) -> IdeEvent {
    timeout(Duration::from_secs(3), events.recv())
        .await
        .expect("event within timeout")
        .expect("event bus open")
}

/// session create：spawn 请求 → 等待命令发出 → 注入响应 → 收结果。
async fn create_session(adapter: &Arc<IdeHostAdapter>, mock: &MockTransport) -> SessionId {
    let adapter = adapter.clone();
    let task = tokio::spawn(async move {
        adapter
            .handle_request(IdeRequest::SessionCreate {
                workspace_id: WorkspaceId::from("ws-1"),
                title: Some("ide demo".into()),
            })
            .await
    });
    wait_for_sent(mock, 2).await;
    let request_id = last_sent_request_id(mock);
    mock.clone()
        .push_response(data_line(&request_id, session_view("s-1", 1)));
    let events = task.await.expect("task").expect("session create");
    assert_eq!(events.len(), 1, "one immediate event");
    let IdeEvent::SessionState {
        client_session_id,
        core_session_id,
        state,
        revision,
    } = &events[0]
    else {
        panic!("expected session state, got {:?}", events[0]);
    };
    assert_eq!(client_session_id.0, "ide:s-1");
    assert_eq!(core_session_id.as_str(), "s-1");
    assert_eq!(*state, client_adapter_api::ClientSessionState::Subscribed);
    assert_eq!(*revision, 2, "register + transition bump revision");
    SessionId::from("s-1")
}

/// 等待从 baseline 起的下一帧发出（多 session / 重连场景不依赖绝对计数）。
async fn wait_for_next_frame(mock: &MockTransport, baseline: usize) {
    let target = baseline + 1;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while mock.sent_count() < target {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timeout waiting for frame {} (got {})",
            target,
            mock.sent_count()
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// 以指定 core session id 创建并 attach 一个 session（多 session 场景）。
async fn create_named_session(
    adapter: &Arc<IdeHostAdapter>,
    mock: &MockTransport,
    session_id: &str,
) -> SessionId {
    let baseline = mock.sent_count();
    let adapter = adapter.clone();
    let task = tokio::spawn(async move {
        adapter
            .handle_request(IdeRequest::SessionCreate {
                workspace_id: WorkspaceId::from("ws-1"),
                title: Some("ide demo".into()),
            })
            .await
    });
    wait_for_next_frame(mock, baseline).await;
    let request_id = last_sent_request_id(mock);
    mock.clone()
        .push_response(data_line(&request_id, session_view(session_id, 1)));
    let _events = task.await.expect("task").expect("session create");
    SessionId::from(session_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_negotiates_and_emits_ready() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (_adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let event = recv_event(&mut events).await;
    match event {
        IdeEvent::Ready {
            protocol_version,
            negotiated,
            instance_id,
        } => {
            assert_eq!(protocol_version, "1");
            assert_eq!(negotiated, ALL_CAPS.to_vec());
            assert_eq!(instance_id.as_deref(), Some("core-fixture-1"));
        }
        other => panic!("expected ready, got {other:?}"),
    }
    assert!(mock.sent_lines()[0].contains("\"type\":\"hello\""));
}

#[tokio::test]
async fn host_without_required_sdk_capabilities_is_rejected() {
    let mock = MockTransport::new().push_response(hello_ack(&["compat_import"]));
    let client = PaworkClient::from_transport(Box::new(mock.clone()), PaworkOptions::default())
        .await
        .expect("sdk handshake");
    let error = IdeHostAdapter::create(options(&ALL_CAPS), Box::new(PaworkSdkChannel::new(client)))
        .await
        .err()
        .expect("host grant check is fail-closed");
    assert!(matches!(error, IdeAdapterError::HostUnavailable(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_run_cancel_and_event_forwarding() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;

    let session_id = create_session(&adapter, &mock).await;

    // run start：命令 → 响应 → RunChanged。
    let adapter2 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter2
            .handle_request(IdeRequest::RunStart {
                session_id,
                user_message: "hello".into(),
                model: None,
            })
            .await
    });
    wait_for_sent(&mock, 3).await;
    let request_id = last_sent_request_id(&mock);
    mock.clone().push_response(data_line(
        &request_id,
        json!({
            "run_id": "run-1",
            "session_id": "s-1",
            "model": "mock",
            "state": "streaming_response",
            "message_count": 0,
            "revision": 1,
        }),
    ));
    let run_events = task.await.expect("task").expect("run start");
    assert_eq!(run_events.len(), 1);
    assert!(matches!(
        &run_events[0],
        IdeEvent::RunChanged { run_id, state: RunState::StreamingResponse }
            if run_id.as_str() == "run-1"
    ));

    // Core 事件转发：run/session 流事件 → 契约事件。
    mock.clone().push_event(event_line(
        EventStream::Run(RunId::from("run-1")),
        AppEvent::ToolApprovalRequired {
            run_id: RunId::from("run-1"),
            tool_call_id: "tool-1".into(),
            reason: "apply_patch needs approval".into(),
        },
        10,
    ));
    let forwarded = recv_event(&mut events).await;
    assert!(matches!(
        forwarded,
        IdeEvent::ToolApprovalRequired { tool_call_id, .. }
            if tool_call_id.as_str() == "tool-1"
    ));

    mock.clone().push_event(event_line(
        EventStream::Run(RunId::from("run-1")),
        AppEvent::AssistantDelta {
            run_id: RunId::from("run-1"),
            message_id: "msg-1".into(),
            delta: "hello ".into(),
        },
        11,
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::AssistantDelta { delta, .. } if delta == "hello "
    ));

    // 审批：ToolApprove 落回 AppCommand（断言发送帧内容）。
    let adapter3 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter3
            .handle_request(IdeRequest::ToolApprove {
                run_id: RunId::from("run-1"),
                tool_call_id: "tool-1".into(),
                decision: ApprovalDecision::ApproveOnce,
            })
            .await
    });
    wait_for_sent(&mock, 4).await;
    let approve_line = mock.sent_lines()[3].clone();
    let approve: Value = serde_json::from_str(&approve_line).expect("approve frame");
    assert_eq!(approve["type"], "command");
    assert_eq!(approve["envelope"]["command"]["method"], "tool_approve");
    assert_eq!(
        approve["envelope"]["command"]["params"]["decision"],
        "approve_once"
    );
    let request_id = last_sent_request_id(&mock);
    mock.clone().push_response(response_line(
        &request_id,
        AppResponse::Accepted {
            command_id: CommandId::from(request_id.as_str()),
            run_id: None,
        },
    ));
    let approve_events = task.await.expect("task").expect("approve ok");
    assert!(approve_events.is_empty(), "approve ack has no event");

    // 取消：RunCancel → CancelOutcome → RunChanged(Cancelled)。
    let adapter4 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter4
            .handle_request(IdeRequest::RunCancel {
                run_id: RunId::from("run-1"),
            })
            .await
    });
    wait_for_sent(&mock, 5).await;
    let request_id = last_sent_request_id(&mock);
    mock.clone().push_response(data_line(
        &request_id,
        json!({
            "run_id": "run-1",
            "cancelled": true,
            "already_cancelled": false,
        }),
    ));
    let cancel_events = task.await.expect("task").expect("cancel ok");
    assert!(matches!(
        &cancel_events[0],
        IdeEvent::RunChanged { run_id, state: RunState::Cancelled }
            if run_id.as_str() == "run-1"
    ));

    let streams = adapter.subscribed_streams().await;
    assert!(
        streams.contains(&EventStream::Session(SessionId::from("s-1"))),
        "session stream stays after terminal run"
    );
    assert!(
        !streams.contains(&EventStream::Run(RunId::from("run-1"))),
        "cancelled run must be pruned from subscribed streams"
    );

    // 边界：所有发送帧都是 Headless 协议帧（hello/command/query），无 GUI 帧。
    for line in mock.sent_lines() {
        let frame: Value = serde_json::from_str(&line).expect("frame is JSON");
        let frame_type = frame["type"].as_str().expect("frame type");
        assert!(
            matches!(frame_type, "hello" | "command" | "query"),
            "IDE channel must only use SDK/Headless frames, got {frame_type}: {line}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_diagnostics_diff_and_workspace_flows() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;

    // 生命周期：打开 + 激活 → EditorContextChanged。
    let opened = adapter
        .handle_request(IdeRequest::EditorDidOpen {
            document_uri: "file:///a.rs".into(),
            language_id: "rust".into(),
            text: Some("fn main() {}".into()),
        })
        .await
        .expect("open");
    assert!(matches!(
        &opened[0],
        IdeEvent::EditorContextChanged { open_documents, .. }
            if open_documents == &vec!["file:///a.rs".to_string()]
    ));
    let activated = adapter
        .handle_request(IdeRequest::EditorDidChangeSelection {
            document_uri: "file:///a.rs".into(),
            selection: Range::new(Position::new(0, 0), Position::new(0, 3)),
        })
        .await
        .expect("selection");
    assert!(matches!(
        &activated[0],
        IdeEvent::EditorContextChanged { active_uri, selection, .. }
            if active_uri.as_deref() == Some("file:///a.rs")
                && selection == &Some(Range::new(Position::new(0, 0), Position::new(0, 3)))
    ));

    // 诊断反向回灌：IDE 发布 → canonical 记录 + 确认事件。
    let published = adapter
        .handle_request(IdeRequest::DiagnosticsPublish {
            document_uri: "file:///a.rs".into(),
            version: Some(1),
            diagnostics: vec![ide_host_adapter::IdeDiagnostic {
                range: Range::new(Position::new(1, 0), Position::new(1, 5)),
                severity: Some(lsp_runtime::DiagnosticSeverity::Error),
                code: Some("E0001".into()),
                source: Some("rust-analyzer".into()),
                message: "missing docs".into(),
            }],
        })
        .await
        .expect("diagnostics publish");
    assert!(matches!(
        &published[0],
        IdeEvent::DiagnosticsChanged { document_uri, diagnostics, .. }
            if document_uri == "file:///a.rs" && diagnostics.len() == 1
    ));

    // workspace add：命令 → WorkspaceAdded。
    let adapter2 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter2
            .handle_request(IdeRequest::WorkspaceAdd {
                root_path: "/tmp/demo".into(),
            })
            .await
    });
    wait_for_sent(&mock, 2).await;
    let request_id = last_sent_request_id(&mock);
    mock.clone().push_response(data_line(
        &request_id,
        json!({"id": "ws-1", "root_path": "/tmp/demo"}),
    ));
    let workspace_events = task.await.expect("task").expect("workspace add");
    assert!(matches!(
        &workspace_events[0],
        IdeEvent::WorkspaceAdded { workspace_id }
            if workspace_id.as_str() == "ws-1"
    ));

    // diff 查询 → DiffResult / DiffContent。
    let adapter3 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter3
            .handle_request(IdeRequest::DiffList {
                workspace_id: WorkspaceId::from("ws-1"),
            })
            .await
    });
    wait_for_sent(&mock, 3).await;
    let request_id = last_sent_request_id(&mock);
    mock.clone()
        .push_response(data_line(&request_id, json!({"files": [{"path": "a.rs"}]})));
    let diff_events = task.await.expect("task").expect("diff list");
    assert!(matches!(
        &diff_events[0],
        IdeEvent::DiffResult { workspace_id, payload }
            if workspace_id.as_str() == "ws-1" && payload["files"][0]["path"] == "a.rs"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attached_session_syncs_lifecycle_and_diagnostics_to_core_context() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;
    let _session_id = create_session(&adapter, &mock).await;

    // attach 时上下文为空，不发送；打开文档后必须通过 Sessions capability
    // 发全量、无正文的 session_client_context_replace。
    let adapter2 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter2
            .handle_request(IdeRequest::EditorDidOpen {
                document_uri: "file:///a.rs".into(),
                language_id: "rust".into(),
                text: Some("fn secret() {}".into()),
            })
            .await
    });
    wait_for_sent(&mock, 3).await;
    let line = mock.sent_lines()[2].clone();
    let frame: Value = serde_json::from_str(&line).expect("context frame");
    assert_eq!(
        frame["envelope"]["command"]["method"],
        "session_client_context_replace"
    );
    assert_eq!(frame["envelope"]["command"]["params"]["session_id"], "s-1");
    let snapshot = &frame["envelope"]["command"]["params"]["snapshot"];
    assert_eq!(snapshot["revision"], 1);
    assert_eq!(snapshot["active_document"], "file:///a.rs");
    assert_eq!(snapshot["open_documents"][0]["text_bytes"], 14);
    assert!(
        !line.contains("fn secret"),
        "document text must not cross host boundary"
    );
    let request_id = last_sent_request_id(&mock);
    mock.clone().push_response(data_line(
        &request_id,
        json!({"session_id": "s-1", "revision": 1, "replaced": true}),
    ));
    task.await.expect("task").expect("open + context sync");

    let adapter3 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter3
            .handle_request(IdeRequest::DiagnosticsPublish {
                document_uri: "file:///a.rs".into(),
                version: Some(1),
                diagnostics: vec![ide_host_adapter::IdeDiagnostic {
                    range: Range::new(Position::new(0, 0), Position::new(0, 2)),
                    severity: Some(DiagnosticSeverity::Error),
                    code: Some("E1".into()),
                    source: Some("rust-analyzer".into()),
                    message: "broken syntax".into(),
                }],
            })
            .await
    });
    wait_for_sent(&mock, 4).await;
    let line = mock.sent_lines()[3].clone();
    let frame: Value = serde_json::from_str(&line).expect("diagnostic context frame");
    let snapshot = &frame["envelope"]["command"]["params"]["snapshot"];
    assert_eq!(snapshot["revision"], 2);
    assert_eq!(snapshot["diagnostics"][0]["message"], "broken syntax");
    let request_id = last_sent_request_id(&mock);
    mock.clone().push_response(data_line(
        &request_id,
        json!({"session_id": "s-1", "revision": 2, "replaced": true}),
    ));
    task.await
        .expect("task")
        .expect("diagnostics + context sync");
}

#[tokio::test]
async fn hello_renegotiation_is_fail_closed() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, _events) = connect(&mock, &ALL_CAPS).await;

    let ready = adapter
        .handle_request(IdeRequest::Hello {
            client_name: "vs-code".into(),
            client_version: "1.0".into(),
            protocol_version: "1".into(),
            capabilities: vec![IdeCapability::Lifecycle, IdeCapability::Interaction],
        })
        .await
        .expect("subset renegotiates");
    assert!(matches!(&ready[0], IdeEvent::Ready { negotiated, .. } if negotiated.len() == 2));

    let error = adapter
        .handle_request(IdeRequest::Hello {
            client_name: "vs-code".into(),
            client_version: "1.0".into(),
            protocol_version: "1".into(),
            capabilities: vec![IdeCapability::LspOutput],
        })
        .await
        .expect_err("unrequested capability is rejected");
    assert!(matches!(error, IdeAdapterError::CapabilityUnsupported(_)));
}

struct FakeLspProvider;

#[async_trait]
impl LspResultProvider for FakeLspProvider {
    async fn resolve(&self, query: &LspQueryKind) -> Result<Value, String> {
        match query {
            LspQueryKind::Hover { uri, position } => {
                Ok(LspOutputEncoder::new().hover_result(&lsp_runtime::Hover {
                    content: format!("hover at {uri}:{}:{}", position.line, position.character),
                    kind: lsp_runtime::MarkupKind::PlainText,
                    range: None,
                }))
            }
            _ => Err("unsupported query".into()),
        }
    }
}

#[tokio::test]
async fn lsp_query_resolves_through_provider() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, _events) = connect(&mock, &[IdeCapability::LspOutput]).await;
    adapter.set_lsp_provider(Arc::new(FakeLspProvider)).await;

    let events = adapter
        .handle_request(IdeRequest::LspQuery {
            query_id: "q-1".into(),
            query: LspQueryKind::Hover {
                uri: "file:///a.rs".into(),
                position: Position::new(1, 2),
            },
        })
        .await
        .expect("lsp query");
    assert!(matches!(
        &events[0],
        IdeEvent::LspResult { query_id, result }
            if query_id == "q-1" && result["contents"]["value"] == "hover at file:///a.rs:1:2"
    ));

    // 未配置 provider → 显式失败。
    let mock2 = MockTransport::new().push_response(full_hello_ack());
    let (adapter2, _) = connect(&mock2, &[IdeCapability::LspOutput]).await;
    assert!(matches!(
        adapter2
            .handle_request(IdeRequest::LspQuery {
                query_id: "q-2".into(),
                query: LspQueryKind::Hover {
                    uri: "file:///a.rs".into(),
                    position: Position::new(0, 0),
                },
            })
            .await,
        Err(IdeAdapterError::LspProvider(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_reattaches_with_new_ownership_epoch() {
    let mock1 = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock1, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;
    let session_id = create_session(&adapter, &mock1).await;
    let record_before = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-1"))
        .await
        .expect("record before reconnect");
    assert_eq!(record_before.ownership_epoch, 1);
    assert_eq!(session_id.as_str(), "s-1");

    // 模拟连接丢失。
    mock1.close().await.expect("mock close");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while adapter.is_connected().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "adapter should notice the closed channel"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // 新通道重挂：claim（epoch bump）+ open_session + 重订阅 + ConnectionRestored。
    let mock2 = MockTransport::new().push_response(full_hello_ack());
    let client2 = PaworkClient::from_transport(Box::new(mock2.clone()), PaworkOptions::default())
        .await
        .expect("second handshake");
    let adapter2 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter2
            .reattach_with(Box::new(PaworkSdkChannel::new(client2)))
            .await
    });
    wait_for_sent(&mock2, 2).await;
    let request_id = last_sent_request_id(&mock2);
    mock2
        .clone()
        .push_response(data_line(&request_id, session_view("s-1", 1)));
    task.await.expect("task").expect("reattach succeeds");

    let record_after = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-1"))
        .await
        .expect("record after reconnect");
    assert_eq!(
        record_after.ownership_epoch, 2,
        "reconnect bumps ownership epoch (stale owner writes rejected)"
    );
    assert_eq!(
        record_after.state,
        client_adapter_api::ClientSessionState::Loaded
    );

    // 事件序：ConnectionLost（旧连接）→ SessionState → ConnectionRestored。
    let lost = recv_event(&mut events).await;
    assert!(matches!(
        lost,
        IdeEvent::ConnectionLost { reason } if reason == "connection lost"
    ));
    let session_state = recv_event(&mut events).await;
    assert!(matches!(
        session_state,
        IdeEvent::SessionState {
            client_session_id,
            state: client_adapter_api::ClientSessionState::Loaded,
            ..
        } if client_session_id.0 == "ide:s-1"
    ));
    let restored = recv_event(&mut events).await;
    assert!(matches!(
        restored,
        IdeEvent::ConnectionRestored { instance_id }
            if instance_id.as_deref() == Some("core-fixture-1")
    ));

    // 重连后事件流可用：新通道事件 → 契约事件。
    mock2.push_event(event_line(
        EventStream::Session(SessionId::from("s-1")),
        AppEvent::RunChanged {
            run_id: RunId::from("run-2"),
            state: RunState::Completed,
        },
        20,
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::RunChanged { run_id, state: RunState::Completed }
            if run_id.as_str() == "run-2"
    ));

    // 关闭：registry 记录按 ownership 移除。
    adapter.close().await.expect("close");
    assert!(
        adapter
            .session(&client_adapter_api::ClientSessionId::new("ide:s-1"))
            .await
            .is_none(),
        "close removes registry records"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiple_sessions_reattach_on_reconnect() {
    // 多 session 重连：每个 client session 都按 ownership epoch/revision 重挂，
    // 旧 owner 的 stale 写在 canonical 边界被拒，跨 session 写不会污染他人上下文。
    let mock1 = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock1, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;

    create_named_session(&adapter, &mock1, "s-1").await;
    create_named_session(&adapter, &mock1, "s-2").await;

    let record_s1_before = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-1"))
        .await
        .expect("s-1 record before reconnect");
    let record_s2_before = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-2"))
        .await
        .expect("s-2 record before reconnect");
    assert_eq!(record_s1_before.ownership_epoch, 1);
    assert_eq!(record_s2_before.ownership_epoch, 1);

    // 模拟连接丢失。
    mock1.close().await.expect("mock close");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while adapter.is_connected().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "adapter should notice the closed channel"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // 新通道重挂：hello 已由 push_response 消费；每个 session 一个 open_session
    // （claim 是本地 registry 写，不发帧）。
    let mock2 = MockTransport::new().push_response(full_hello_ack());
    let client2 = PaworkClient::from_transport(Box::new(mock2.clone()), PaworkOptions::default())
        .await
        .expect("second handshake");
    let adapter2 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter2
            .reattach_with(Box::new(PaworkSdkChannel::new(client2)))
            .await
    });
    // hello（frame 1）已消费；open_session s-1（frame 2）。
    wait_for_next_frame(&mock2, 1).await;
    mock2.clone().push_response(data_line(
        &last_sent_request_id(&mock2.clone()),
        session_view("s-1", 1),
    ));
    // open_session s-2（frame 3）。
    wait_for_next_frame(&mock2, 2).await;
    mock2.clone().push_response(data_line(
        &last_sent_request_id(&mock2.clone()),
        session_view("s-2", 1),
    ));
    task.await.expect("reattach task").expect("reattach succeeds");

    // 两个 session 的 ownership_epoch 都 bump 到 2：旧 owner 的 stale 写被拒。
    let record_s1_after = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-1"))
        .await
        .expect("s-1 record after reconnect");
    let record_s2_after = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-2"))
        .await
        .expect("s-2 record after reconnect");
    assert_eq!(record_s1_after.ownership_epoch, 2, "s-1 epoch bumps");
    assert_eq!(record_s2_after.ownership_epoch, 2, "s-2 epoch bumps");
    assert_eq!(
        record_s1_after.state,
        client_adapter_api::ClientSessionState::Loaded
    );
    assert_eq!(
        record_s2_after.state,
        client_adapter_api::ClientSessionState::Loaded
    );

    // 事件序：ConnectionLost → SessionState(s-1) → SessionState(s-2) → ConnectionRestored。
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::ConnectionLost { reason } if reason == "connection lost"
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::SessionState { client_session_id, state, .. }
            if client_session_id.0 == "ide:s-1"
                && state == client_adapter_api::ClientSessionState::Loaded
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::SessionState { client_session_id, state, .. }
            if client_session_id.0 == "ide:s-2"
                && state == client_adapter_api::ClientSessionState::Loaded
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::ConnectionRestored { instance_id }
            if instance_id.as_deref() == Some("core-fixture-1")
    ));

    adapter.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_limit_context_sync_does_not_send_frame_or_consume_revision() {
    // validate-before-mutate：诊断 message 超 MAX_CLIENT_CONTEXT_MESSAGE_BYTES 时，
    // snapshot.validate() 在 fetch_add 之前失败——不发帧、不消耗 revision；
    // 后续合法发布仍能同步（失败不污染 canonical 状态）。
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;
    let session_id = create_session(&adapter, &mock).await;
    assert_eq!(session_id.as_str(), "s-1");
    let baseline = mock.sent_count();

    // 超 4096 字节的诊断消息：URI 合法，只触发 message 上限。
    let oversize = "x".repeat(core_api::MAX_CLIENT_CONTEXT_MESSAGE_BYTES + 1);
    let over_limit = adapter
        .handle_request(IdeRequest::DiagnosticsPublish {
            document_uri: "file:///a.rs".into(),
            version: Some(1),
            diagnostics: vec![ide_host_adapter::IdeDiagnostic {
                range: Range::new(Position::new(1, 0), Position::new(1, 5)),
                severity: Some(DiagnosticSeverity::Error),
                code: None,
                source: None,
                message: oversize,
            }],
        })
        .await;
    assert!(
        matches!(&over_limit, Err(IdeAdapterError::InvalidFrame(_))),
        "over-limit diagnostic must fail validation before mutate, got {over_limit:?}"
    );
    assert_eq!(
        mock.sent_count(),
        baseline,
        "validate-before-mutate: over-limit snapshot must not send a frame"
    );

    // 失败不污染：同一文档发布合法诊断集（替换超限项）后仍能同步（发一帧）。
    let adapter_recover = adapter.clone();
    let recover_task = tokio::spawn(async move {
        adapter_recover
            .handle_request(IdeRequest::DiagnosticsPublish {
                document_uri: "file:///a.rs".into(),
                version: Some(1),
                diagnostics: vec![ide_host_adapter::IdeDiagnostic {
                    range: Range::new(Position::new(1, 0), Position::new(1, 5)),
                    severity: Some(DiagnosticSeverity::Warning),
                    code: None,
                    source: None,
                    message: "unused variable".into(),
                }],
            })
            .await
    });
    wait_for_next_frame(&mock, baseline).await;
    mock.clone().push_response(data_line(&last_sent_request_id(&mock.clone()), json!({})));
    let recovered = recover_task
        .await
        .expect("recover task")
        .expect("recover ok");
    assert!(
        matches!(
            &recovered[..],
            [IdeEvent::DiagnosticsChanged { document_uri, diagnostics, .. }]
                if document_uri == "file:///a.rs" && diagnostics.len() == 1
        ),
        "valid publish replaces over-limit set and syncs, got {recovered:?}"
    );
    assert_eq!(
        mock.sent_count(),
        baseline + 1,
        "recovery sends exactly one frame"
    );

    adapter.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consecutive_reconnects_do_not_duplicate_subscriptions_or_events() {
    let mock1 = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock1, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;

    let session_id = create_session(&adapter, &mock1).await;

    // run start：Run 流订阅（订阅记录 = Session + Run）。
    let adapter2 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter2
            .handle_request(IdeRequest::RunStart {
                session_id,
                user_message: "hello".into(),
                model: None,
            })
            .await
    });
    wait_for_sent(&mock1, 3).await;
    let request_id = last_sent_request_id(&mock1);
    mock1.clone().push_response(data_line(
        &request_id,
        json!({
            "run_id": "run-1",
            "session_id": "s-1",
            "model": "mock",
            "state": "streaming_response",
            "message_count": 0,
            "revision": 1,
        }),
    ));
    task.await.expect("task").expect("run start");

    let streams = adapter.subscribed_streams().await;
    assert_eq!(streams.len(), 2, "session + run recorded once");
    assert!(streams.contains(&EventStream::Session(SessionId::from("s-1"))));
    assert!(streams.contains(&EventStream::Run(RunId::from("run-1"))));
    assert_eq!(adapter.connection_generation(), 1);

    // 连续三次重连；旧通道保持打开，验证旧代际转发任务被替换/取消。
    let mut previous: Vec<MockTransport> = vec![mock1.clone()];
    let mut current = mock1;
    for round in 0..3 {
        let next = MockTransport::new().push_response(full_hello_ack());
        let client = PaworkClient::from_transport(Box::new(next.clone()), PaworkOptions::default())
            .await
            .expect("handshake");
        let adapter3 = adapter.clone();
        let task = tokio::spawn(async move {
            adapter3
                .reattach_with(Box::new(PaworkSdkChannel::new(client)))
                .await
        });
        wait_for_sent(&next, 2).await;
        let request_id = last_sent_request_id(&next);
        next.clone()
            .push_response(data_line(&request_id, session_view("s-1", 1)));
        task.await.expect("task").expect("reattach");

        // 事件序：ConnectionLost → SessionState → ConnectionRestored。
        assert!(
            matches!(
                recv_event(&mut events).await,
                IdeEvent::ConnectionLost { .. }
            ),
            "round {round}: connection lost event"
        );
        assert!(
            matches!(recv_event(&mut events).await, IdeEvent::SessionState { .. }),
            "round {round}: session state event"
        );
        assert!(
            matches!(
                recv_event(&mut events).await,
                IdeEvent::ConnectionRestored { .. }
            ),
            "round {round}: connection restored event"
        );

        previous.push(current);
        current = next;
    }

    // 幂等：三次重连后订阅记录仍是 2 条（不指数增长），代际已替换。
    assert_eq!(
        adapter.subscribed_streams().await.len(),
        2,
        "subscription records do not grow across reconnects"
    );
    assert_eq!(
        adapter.connection_generation(),
        4,
        "generation replaced on every reconnect"
    );

    // 旧通道事件不再转发（旧代际任务已取消）。
    for (index, stale) in previous.iter().enumerate() {
        stale.clone().push_event(event_line(
            EventStream::Session(SessionId::from("s-1")),
            AppEvent::RunChanged {
                run_id: RunId::from(format!("stale-{index}")),
                state: RunState::Completed,
            },
            100 + index as u64,
        ));
    }
    assert!(
        timeout(Duration::from_millis(400), events.recv())
            .await
            .is_err(),
        "stale channels must not forward events after reattach"
    );

    // 当前通道事件恰好转发一次，且无重复。
    current.push_event(event_line(
        EventStream::Session(SessionId::from("s-1")),
        AppEvent::RunChanged {
            run_id: RunId::from("run-9"),
            state: RunState::Completed,
        },
        200,
    ));
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::RunChanged { run_id, state: RunState::Completed }
            if run_id.as_str() == "run-9"
    ));
    assert!(
        timeout(Duration::from_millis(400), events.recv())
            .await
            .is_err(),
        "current channel must deliver exactly one event per stream"
    );

    // 关闭：ConnectionLost 可观测 + 通道关闭。
    adapter.close().await.expect("close");
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::ConnectionLost { reason } if reason == "adapter closed"
    ));
    assert!(!adapter.is_connected().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_tool_diff_get_and_session_open_flow() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;

    // RunTool：落回 AppCommand::RunTool 命令帧，ack 无事件。
    let adapter2 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter2
            .handle_request(IdeRequest::RunTool {
                run_id: RunId::from("run-1"),
                tool_name: "apply_patch".into(),
                input: json!({"path": "a.rs"}),
            })
            .await
    });
    wait_for_sent(&mock, 2).await;
    let run_tool_line = mock.sent_lines()[1].clone();
    let frame: Value = serde_json::from_str(&run_tool_line).expect("run tool frame");
    assert_eq!(frame["type"], "command");
    assert_eq!(frame["envelope"]["command"]["method"], "run_tool");
    assert_eq!(
        frame["envelope"]["command"]["params"]["tool_name"],
        "apply_patch"
    );
    assert_eq!(
        frame["envelope"]["command"]["params"]["input"]["path"],
        "a.rs"
    );
    let request_id = last_sent_request_id(&mock);
    mock.clone().push_response(response_line(
        &request_id,
        AppResponse::Accepted {
            command_id: CommandId::from(request_id.as_str()),
            run_id: None,
        },
    ));
    let run_tool_events = task.await.expect("task").expect("run tool");
    assert!(run_tool_events.is_empty(), "run tool ack has no event");

    // DiffGet：查询帧 → DiffContent。
    let adapter3 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter3
            .handle_request(IdeRequest::DiffGet {
                workspace_id: WorkspaceId::from("ws-1"),
                path: "src/main.rs".into(),
                cursor: Some("cursor-1".into()),
            })
            .await
    });
    wait_for_sent(&mock, 3).await;
    let diff_line = mock.sent_lines()[2].clone();
    let frame: Value = serde_json::from_str(&diff_line).expect("diff frame");
    assert_eq!(frame["type"], "query");
    assert_eq!(frame["envelope"]["query"]["method"], "diff_get");
    assert_eq!(frame["envelope"]["query"]["params"]["path"], "src/main.rs");
    assert_eq!(frame["envelope"]["query"]["params"]["cursor"], "cursor-1");
    let request_id = last_sent_request_id(&mock);
    mock.clone().push_response(data_line(
        &request_id,
        json!({"hunks": [{"header": "@@ -1 +1 @@"}]}),
    ));
    let diff_events = task.await.expect("task").expect("diff get");
    assert!(matches!(
        &diff_events[0],
        IdeEvent::DiffContent { workspace_id, path, payload }
            if workspace_id.as_str() == "ws-1"
                && path == "src/main.rs"
                && payload["hunks"][0]["header"] == "@@ -1 +1 @@"
    ));

    // SessionOpen：open_session → attach（registry 记录 + Session 订阅）。
    let adapter4 = adapter.clone();
    let task = tokio::spawn(async move {
        adapter4
            .handle_request(IdeRequest::SessionOpen {
                session_id: SessionId::from("s-9"),
            })
            .await
    });
    wait_for_sent(&mock, 4).await;
    let request_id = last_sent_request_id(&mock);
    mock.clone()
        .push_response(data_line(&request_id, session_view("s-9", 3)));
    let open_events = task.await.expect("task").expect("session open");
    assert!(matches!(
        &open_events[0],
        IdeEvent::SessionState {
            client_session_id,
            core_session_id,
            state: client_adapter_api::ClientSessionState::Subscribed,
            revision: 2,
        } if client_session_id.0 == "ide:s-9" && core_session_id.as_str() == "s-9"
    ));
    let record = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-9"))
        .await
        .expect("session record");
    assert_eq!(
        record.state,
        client_adapter_api::ClientSessionState::Subscribed
    );
    assert!(adapter
        .subscribed_streams()
        .await
        .contains(&EventStream::Session(SessionId::from("s-9"))));

    // 无额外事件（bus 安静）。
    assert!(timeout(Duration::from_millis(200), events.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn session_reattach_bumps_ownership_and_rejects_stale() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, _events) = connect(&mock, &ALL_CAPS).await;
    create_session(&adapter, &mock).await;

    let record = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-1"))
        .await
        .expect("record");
    assert_eq!((record.ownership_epoch, record.revision), (1, 2));

    let events = adapter
        .handle_request(IdeRequest::SessionReattach {
            client_session_id: client_adapter_api::ClientSessionId::new("ide:s-1"),
            ownership_epoch: 1,
            revision: 2,
        })
        .await
        .expect("reattach request");
    assert!(matches!(
        &events[0],
        IdeEvent::SessionState {
            state: client_adapter_api::ClientSessionState::Loaded,
            revision: 3,
            ..
        }
    ));

    let record = adapter
        .session(&client_adapter_api::ClientSessionId::new("ide:s-1"))
        .await
        .expect("record after claim");
    assert_eq!(record.ownership_epoch, 2);

    // 旧 ownership 再重挂 → 显式失败（不静默覆盖新 owner）。
    let error = adapter
        .handle_request(IdeRequest::SessionReattach {
            client_session_id: client_adapter_api::ClientSessionId::new("ide:s-1"),
            ownership_epoch: 1,
            revision: 2,
        })
        .await
        .expect_err("stale ownership is rejected");
    assert!(matches!(error, IdeAdapterError::Adapter(_)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_emits_connection_lost_and_removes_records() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;
    create_session(&adapter, &mock).await;

    adapter.close().await.expect("close");
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::ConnectionLost { reason } if reason == "adapter closed"
    ));
    assert!(!adapter.is_connected().await);
    assert!(
        adapter
            .session(&client_adapter_api::ClientSessionId::new("ide:s-1"))
            .await
            .is_none(),
        "close removes registry records"
    );
    assert_eq!(
        adapter.connection_generation(),
        2,
        "close replaces generation and cancels forward tasks"
    );
}

#[tokio::test]
async fn editor_save_emits_context_event() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, _events) = connect(&mock, &ALL_CAPS).await;

    adapter
        .handle_request(IdeRequest::EditorDidOpen {
            document_uri: "file:///a.rs".into(),
            language_id: "rust".into(),
            text: Some("fn main() {}".into()),
        })
        .await
        .expect("open");

    let saved = adapter
        .handle_request(IdeRequest::EditorDidSave {
            document_uri: "file:///a.rs".into(),
        })
        .await
        .expect("save");
    assert!(matches!(
        &saved[0],
        IdeEvent::EditorContextChanged { open_documents, .. }
            if open_documents == &vec!["file:///a.rs".to_string()]
    ));

    // 连续保存不报错、上下文事件持续发出。
    let saved_again = adapter
        .handle_request(IdeRequest::EditorDidSave {
            document_uri: "file:///a.rs".into(),
        })
        .await
        .expect("second save");
    assert!(matches!(
        &saved_again[0],
        IdeEvent::EditorContextChanged { .. }
    ));
}

fn document_diagnostic(uri: &str, version: Option<i64>) -> DocumentDiagnostic {
    DocumentDiagnostic {
        uri: uri.into(),
        version,
        diagnostics: vec![Diagnostic {
            range: Range::new(Position::new(1, 0), Position::new(1, 5)),
            severity: Some(DiagnosticSeverity::Error),
            code: Some(serde_json::json!("E0001")),
            source: Some("rust-analyzer".into()),
            message: "missing docs".into(),
        }],
    }
}

#[tokio::test]
async fn host_publishes_lsp_snapshot_to_board_and_bus() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;

    let document = document_diagnostic("file:///a.rs", Some(7));
    let changed = adapter
        .publish_lsp_snapshot(std::slice::from_ref(&document))
        .await
        .expect("snapshot publish");
    assert_eq!(changed, 1);
    assert!(matches!(
        recv_event(&mut events).await,
        IdeEvent::DiagnosticsChanged { document_uri, version: Some(7), diagnostics }
            if document_uri == "file:///a.rs" && diagnostics.len() == 1
    ));

    // 幂等：相同快照不重复发事件；看板可观测。
    assert_eq!(
        adapter
            .publish_lsp_snapshot(std::slice::from_ref(&document))
            .await
            .expect("idempotent publish"),
        0
    );
    assert!(
        timeout(Duration::from_millis(200), events.recv())
            .await
            .is_err(),
        "identical snapshot emits no duplicate event"
    );
    let snapshot = adapter.diagnostic_snapshot().await;
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].version, Some(7));
    assert_eq!(snapshot[0].diagnostics[0].code.as_deref(), Some("E0001"));
}

#[tokio::test]
async fn unsupported_command_fails_explicitly_at_protocol_layer() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, _events) = connect(&mock, &ALL_CAPS).await;
    // 契约子集之外的命令（AuthRemove）经 ClientFrame 入口显式拒绝，
    // 不触碰 SDK 通道（无响应注入也不会挂起）。
    let frame_error = adapter
        .handle_client_frame(client_adapter_api::ClientFrame {
            schema_version: client_adapter_api::CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "x".into(),
            method: "ide.command".into(),
            payload: serde_json::to_value(client_adapter_api::CanonicalClientRequest::Command(
                AppCommandEnvelope {
                    api_version: API_VERSION,
                    command_id: CommandId::from("x"),
                    source: CommandSource::Automation,
                    identity: ActorIdentity::Automation {
                        name: "ide-test".into(),
                    },
                    expected_revision: None,
                    idempotency_key: None,
                    issued_at: Timestamp::from_unix_millis(1),
                    command: AppCommand::AuthRemove {
                        provider_id: "openai".into(),
                    },
                },
            ))
            .unwrap(),
            extensions: Default::default(),
        })
        .await;
    assert!(matches!(
        frame_error,
        Err(IdeAdapterError::ProtocolUnsupported(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attach_frame_reuses_bound_session_without_reregister() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;
    let session_id = create_session(&adapter, &mock).await;
    let client_session_id = client_adapter_api::ClientSessionId::new("ide:s-1");
    let existing = adapter
        .session(&client_session_id)
        .await
        .expect("bound session");
    let before_revision = existing.revision;
    let before_streams = adapter.subscribed_streams().await.len();

    let attached = adapter
        .handle_client_frame(client_adapter_api::ClientFrame {
            schema_version: client_adapter_api::CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: "attach-reuse".into(),
            method: "ide.attach".into(),
            payload: serde_json::to_value(client_adapter_api::CanonicalClientRequest::Attach(
                existing,
            ))
            .unwrap(),
            extensions: Default::default(),
        })
        .await
        .expect("attach reuses existing session");

    assert!(matches!(
        &attached[0],
        IdeEvent::SessionState {
            core_session_id,
            state: client_adapter_api::ClientSessionState::Subscribed,
            revision,
            ..
        } if core_session_id == &session_id && *revision == before_revision
    ));
    assert_eq!(
        adapter.subscribed_streams().await.len(),
        before_streams,
        "reattach of a live session must not duplicate subscriptions"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identical_diagnostics_publish_does_not_resync_core() {
    let mock = MockTransport::new().push_response(full_hello_ack());
    let (adapter, mut events) = connect(&mock, &ALL_CAPS).await;
    let _ready = recv_event(&mut events).await;
    let _session_id = create_session(&adapter, &mock).await;

    let diagnostic = ide_host_adapter::IdeDiagnostic {
        range: Range::new(Position::new(0, 0), Position::new(0, 2)),
        severity: Some(DiagnosticSeverity::Error),
        code: Some("E1".into()),
        source: Some("rust-analyzer".into()),
        message: "broken syntax".into(),
    };
    let adapter2 = adapter.clone();
    let first = diagnostic.clone();
    let task = tokio::spawn(async move {
        adapter2
            .handle_request(IdeRequest::DiagnosticsPublish {
                document_uri: "file:///a.rs".into(),
                version: Some(1),
                diagnostics: vec![first],
            })
            .await
    });
    wait_for_sent(&mock, 3).await;
    mock.clone().push_response(data_line(
        &last_sent_request_id(&mock),
        json!({"session_id": "s-1", "revision": 1, "replaced": true}),
    ));
    let first_events = task.await.expect("task").expect("first publish");
    assert_eq!(first_events.len(), 1);
    let sent_after_first = mock.sent_count();

    let repeated = adapter
        .handle_request(IdeRequest::DiagnosticsPublish {
            document_uri: "file:///a.rs".into(),
            version: Some(1),
            diagnostics: vec![diagnostic],
        })
        .await
        .expect("identical publish");
    assert!(
        repeated.is_empty(),
        "identical diagnostics publish must not emit another event"
    );
    assert_eq!(
        mock.sent_count(),
        sent_after_first,
        "unchanged board must not write another Core context snapshot"
    );
}
