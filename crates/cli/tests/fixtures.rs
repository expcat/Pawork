//! versioned fixture / golden 测试：wire 解析、握手协商 golden、
//! v2 拒绝、未知方法显式拒绝、事件回译 golden。
//!
//! fixture 目录：`fixtures/v1`（稳定 wire protocolVersion=1）、`fixtures/v2`
//! （实验版本，只用于断言显式拒绝）。

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use pawork_cli::channels::acp::wire::{
    ERROR_INVALID_PARAMS, ERROR_INVALID_REQUEST, ERROR_METHOD_NOT_FOUND,
};
use pawork_cli::channels::acp::{
    AcpClientAdapterFactory, CwdResolver, JsonRpcMessage, SessionResolver,
};
use pawork_domain::{CoreInstanceId, EventId, MessageId, RunId, ToolCallId, WorkspaceId};
use pawork_protocol::adapter::{
    AdapterError, CanonicalCoreFrame, CapabilitySnapshot, ClientAdapter, ClientCapability,
    ClientProtocol, ClientSessionId, InMemorySessionRegistryStore, SessionRegistry,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};
use pawork_protocol::{
    AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence, API_VERSION,
};
use serde_json::{json, Value};

use common::{acp_notification, acp_request, parse, MockScript, TestHarness};

fn fixture(path: &str) -> Value {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    serde_json::from_str(&std::fs::read_to_string(&full).expect("read fixture"))
        .expect("fixture is valid JSON")
}

struct FixedCwdResolver(WorkspaceId);

#[async_trait]
impl CwdResolver for FixedCwdResolver {
    async fn resolve(&self, _cwd: &str) -> Result<WorkspaceId, AdapterError> {
        Ok(self.0.clone())
    }
}

struct FixedSessionResolver(ClientSessionId);

#[async_trait]
impl SessionResolver for FixedSessionResolver {
    async fn resolve_client_session(&self, _event: &AppEventEnvelope) -> Option<ClientSessionId> {
        Some(self.0.clone())
    }
}

async fn negotiated_adapter() -> Arc<pawork_cli::channels::acp::AcpClientAdapter> {
    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let workspace = WorkspaceId::from("ws-golden");
    let client_session = ClientSessionId::new("acp-golden-session");
    let factory = AcpClientAdapterFactory::new(
        std::iter::empty::<ClientCapability>(),
        registry,
        Arc::new(FixedCwdResolver(workspace)),
        Arc::new(FixedSessionResolver(client_session)),
        pawork_cli::channels::acp::wire::Implementation {
            name: "test-agent".into(),
            title: None,
            version: "0.0.0".into(),
        },
    );
    let snapshot = CapabilitySnapshot {
        schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
        protocol: ClientProtocol::new("acp"),
        protocol_version: "1".into(),
        client_version: "test-client".into(),
        revision: 1,
        capabilities: BTreeSet::new(),
    };
    factory
        .create_concrete(snapshot)
        .expect("negotiation succeeds")
        .adapter
}

fn event_envelope(stream: EventStream, payload: AppEvent) -> AppEventEnvelope {
    AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: CoreInstanceId::from("instance-golden"),
        event_id: EventId::from("event-golden"),
        global_sequence: GlobalSequence(1),
        stream,
        stream_sequence: 1,
        timestamp: pawork_domain::Timestamp::from_unix_millis(1),
        source: EventSource::Core,
        payload,
    }
}

#[tokio::test]
async fn golden_initialize_handshake_matches_fixture() {
    let harness = TestHarness::new(MockScript::new().complete()).await;
    let result = harness
        .host
        .handle_request(json!(1), "initialize", Some(common::initialize_params()))
        .await
        .expect("initialize 应成功");
    assert_eq!(result, fixture("fixtures/v1/initialize-response.json"));
    assert_eq!(
        harness.degraded_capabilities(),
        vec![
            ClientCapability::new("fs.read_text_file"),
            ClientCapability::new("terminal"),
        ]
    );
    assert!(harness.is_initialized());
}

#[tokio::test]
async fn golden_v2_initialize_is_rejected() {
    let harness = TestHarness::new(MockScript::new().complete()).await;
    let request = fixture("fixtures/v2/initialize-request-v2.json");
    let JsonRpcMessage::Request(message) = parse(request).expect("v2 请求可解析") else {
        panic!("expected request");
    };
    let result = harness
        .host
        .handle_request(message.id, &message.method, message.params)
        .await;
    let error = result.expect_err("v2 握手必须失败");
    assert_eq!(error.code, ERROR_INVALID_PARAMS);
    assert!(
        error.message.contains("protocolVersion 2"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("experimental v2"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn golden_unknown_method_error_matches_fixture() {
    let harness = TestHarness::new(MockScript::new().complete()).await;
    harness.initialize().await.expect("initialize");
    let result = harness
        .host
        .handle_request(json!(9), "session/load", Some(json!({ "sessionId": "s1" })))
        .await;
    let error = result.expect_err("session/load 必须失败");
    let expected = fixture("fixtures/v1/error-unknown-method.json");
    assert_eq!(error.code, ERROR_METHOD_NOT_FOUND);
    assert_eq!(error.code, expected["error"]["code"]);
    assert_eq!(error.message, expected["error"]["message"]);
}

#[tokio::test]
async fn golden_session_update_text_matches_fixture() {
    let adapter = negotiated_adapter().await;
    let envelope = event_envelope(
        EventStream::Run(RunId::from("run-golden")),
        AppEvent::AssistantDelta {
            run_id: RunId::from("run-golden"),
            message_id: MessageId::from("msg-golden-1"),
            delta: "hello golden".into(),
        },
    );
    let frame = adapter
        .encode(CanonicalCoreFrame::Event(envelope))
        .await
        .expect("事件可编码");
    assert_eq!(frame.method, "acp.notification");
    let notification = acp_notification("session/update", frame.payload);
    assert_eq!(
        notification,
        fixture("fixtures/v1/session-update-text.json")
    );
}

#[tokio::test]
async fn golden_session_new_request_matches_fixture() {
    let request = fixture("fixtures/v1/session-new-request.json");
    let JsonRpcMessage::Request(message) = parse(request).expect("fixture 可解析") else {
        panic!("expected request");
    };
    assert_eq!(message.method, "session/new");
    let params = serde_json::from_value::<pawork_cli::channels::acp::wire::SessionNewParams>(
        message.params.clone().unwrap_or(Value::Null),
    )
    .expect("session/new params 可解析");
    assert_eq!(params.cwd, "/tmp");
    assert!(params.mcp_servers.is_empty());
    assert!(params.additional_directories.is_empty());

    let harness = TestHarness::new(MockScript::new().complete()).await;
    let dir = tempfile::TempDir::with_prefix("acp-host-session-new-golden-").expect("temp dir");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    assert!(
        !session_id.is_empty(),
        "session/new must return a sessionId"
    );
}

#[test]
fn golden_session_prompt_request_matches_fixture() {
    let request = fixture("fixtures/v1/session-prompt-request.json");
    let JsonRpcMessage::Request(message) = parse(request).expect("fixture 可解析") else {
        panic!("expected request");
    };
    assert_eq!(message.method, "session/prompt");
    let params = serde_json::from_value::<pawork_cli::channels::acp::wire::SessionPromptParams>(
        message.params.clone().unwrap_or(Value::Null),
    )
    .expect("session/prompt params 可解析");
    assert_eq!(params.session_id, "acp-golden-session");
    assert_eq!(params.prompt.len(), 1);
    assert_eq!(params.prompt[0]["type"], json!("text"));
    assert_eq!(params.prompt[0]["text"], json!("hello"));
}

#[tokio::test]
async fn golden_session_cancel_notification_matches_fixture() {
    let request = fixture("fixtures/v1/session-cancel-notification.json");
    let JsonRpcMessage::Notification(message) = parse(request).expect("fixture 可解析") else {
        panic!("expected notification");
    };
    assert_eq!(message.method, "session/cancel");
    let adapter = negotiated_adapter().await;
    let target = adapter
        .decode_cancel(message.params.clone().unwrap_or(Value::Null))
        .await
        .expect("session/cancel 可解析");
    assert_eq!(target.client_session_id.0, "acp-golden-session");
}

#[tokio::test]
async fn golden_session_update_tool_call_matches_fixture() {
    let adapter = negotiated_adapter().await;
    let envelope = event_envelope(
        EventStream::Run(RunId::from("run-golden")),
        AppEvent::ToolStarted {
            run_id: RunId::from("run-golden"),
            tool_call_id: ToolCallId::from("tool-golden-1"),
            name: "echo".into(),
        },
    );
    let frame = adapter
        .encode(CanonicalCoreFrame::Event(envelope))
        .await
        .expect("事件可编码");
    assert_eq!(frame.method, "acp.notification");
    let notification = acp_notification("session/update", frame.payload);
    assert_eq!(
        notification,
        fixture("fixtures/v1/session-update-tool-call.json")
    );
}

#[tokio::test]
async fn golden_custom_model_method_is_rejected() {
    let harness = TestHarness::new(MockScript::new().complete()).await;
    harness.initialize().await.expect("initialize");
    let request = fixture("fixtures/v1/session-set-model-request.json");
    let JsonRpcMessage::Request(message) = parse(request).expect("fixture 可解析") else {
        panic!("expected request");
    };
    assert_eq!(message.method, "session/set_model");
    let result = harness
        .host
        .handle_request(message.id, &message.method, message.params)
        .await;
    let error = result.expect_err("session/set_model 必须失败");
    let expected = fixture("fixtures/v1/error-unknown-set-model.json");
    assert_eq!(error.code, ERROR_METHOD_NOT_FOUND);
    assert_eq!(error.code, expected["error"]["code"]);
    assert_eq!(error.message, expected["error"]["message"]);
}

#[tokio::test]
async fn golden_permission_response_selected_matches_fixture() {
    let adapter = negotiated_adapter().await;
    let decision = adapter
        .decode_permission_response(fixture("fixtures/v1/permission-response-selected.json"))
        .expect("selected 响应可解析");
    assert_eq!(
        decision,
        pawork_cli::channels::acp::PermissionDecision::Selected {
            option_id: "allow-once".into(),
        }
    );
}

#[tokio::test]
async fn golden_permission_response_cancelled_matches_fixture() {
    let adapter = negotiated_adapter().await;
    let decision = adapter
        .decode_permission_response(fixture("fixtures/v1/permission-response-cancelled.json"))
        .expect("cancelled 响应可解析");
    assert_eq!(
        decision,
        pawork_cli::channels::acp::PermissionDecision::Cancelled
    );
}

#[tokio::test]
async fn flat_permission_response_is_rejected() {
    let adapter = negotiated_adapter().await;
    let error = adapter
        .decode_permission_response(json!({
            "outcome": "selected",
            "optionId": "allow-once",
        }))
        .expect_err("扁平形状必须拒绝");
    assert!(
        error.to_string().contains("optionId"),
        "{}",
        error.to_string()
    );
    let error = adapter
        .decode_permission_response(json!({
            "outcome": { "outcome": "selected", "optionId": "allow-once", "bogus": 1 },
        }))
        .expect_err("未知字段必须拒绝");
    assert!(error.to_string().contains("bogus"), "{}", error.to_string());
}

#[tokio::test]
async fn session_new_requires_mcp_servers_field() {
    let harness = TestHarness::new(MockScript::new().complete()).await;
    harness.initialize().await.expect("initialize");
    let error = harness
        .host
        .handle_request(json!(5), "session/new", Some(json!({ "cwd": "/tmp" })))
        .await
        .expect_err("缺失 mcpServers 必须拒绝");
    assert_eq!(error.code, ERROR_INVALID_PARAMS);
    assert!(error.message.contains("mcpServers"), "{}", error.message);

    let error = harness
        .host
        .handle_request(
            json!(6),
            "session/new",
            Some(json!({ "cwd": "/tmp", "mcpServers": [] })),
        )
        .await
        .expect_err("显式空数组应通过参数解析");
    assert_ne!(error.code, ERROR_INVALID_PARAMS);
}

#[tokio::test]
async fn session_resume_omitted_builder_defaults_are_accepted() {
    let request = fixture("fixtures/v1/session-resume-minimal.json");
    let JsonRpcMessage::Request(message) = parse(request).expect("fixture 可解析") else {
        panic!("expected request");
    };
    let params = serde_json::from_value::<pawork_cli::channels::acp::wire::SessionResumeParams>(
        message.params.clone().unwrap_or(Value::Null),
    )
    .expect("省略 builder 缺省字段必须可解析");
    assert!(
        params.mcp_servers.is_empty(),
        "mcpServers 缺省为空数组，got {:?}",
        params.mcp_servers
    );
    assert!(
        params.additional_directories.is_empty(),
        "additionalDirectories 缺省为空数组，got {:?}",
        params.additional_directories
    );

    let harness = TestHarness::new(MockScript::new().complete()).await;
    let dir = tempfile::TempDir::with_prefix("acp-host-resume-defaults-").expect("temp dir");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    harness
        .host
        .handle_request(
            json!(6),
            "session/close",
            Some(json!({ "sessionId": session_id })),
        )
        .await
        .expect("close 应成功");
    let resume = harness
        .host
        .handle_request(
            json!(7),
            "session/resume",
            Some(json!({
                "sessionId": session_id,
                "cwd": dir.path().to_str().expect("path"),
            })),
        )
        .await
        .expect("省略缺省字段的 resume 应成功");
    assert_eq!(resume, json!({}));
}

#[test]
fn jsonrpc_parse_rejects_non_object_and_bad_version() {
    let error = parse(json!([])).expect_err("数组不是合法消息");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
    let error = parse(json!({ "jsonrpc": "1.0", "id": 1, "method": "initialize" }))
        .expect_err("jsonrpc 1.0 必须拒绝");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
    let error = parse(json!({ "jsonrpc": "2.0", "id": 1 })).expect_err("缺少 method/id");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
    let request = acp_request(3, "session/new", json!({ "cwd": "/tmp" }));
    let parsed = parse(request.clone()).expect("请求可解析");
    assert_eq!(parsed.to_value(), request);
    let notification = acp_notification("session/cancel", json!({ "sessionId": "s1" }));
    let parsed = parse(notification.clone()).expect("通知可解析");
    assert_eq!(parsed.to_value(), notification);
}

#[tokio::test]
async fn unknown_params_fields_are_rejected() {
    let harness = TestHarness::new(MockScript::new().complete()).await;
    harness.initialize().await.expect("initialize");
    let result = harness
        .host
        .handle_request(
            json!(4),
            "session/new",
            Some(json!({
                "cwd": std::env::temp_dir().to_string_lossy(),
                "mcpServers": [],
                "bogusField": 1,
            })),
        )
        .await;
    let error = result.expect_err("未知字段必须拒绝");
    assert_eq!(error.code, ERROR_INVALID_PARAMS);
    assert!(error.message.contains("bogusField"), "{}", error.message);
}
