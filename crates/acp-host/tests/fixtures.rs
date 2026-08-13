//! P17-7 versioned fixture / golden 测试：wire 解析、握手协商 golden、
//! v2 拒绝、未知方法显式拒绝、事件回译 golden。
//!
//! fixture 目录：`fixtures/v1`（稳定 wire protocolVersion=1）、`fixtures/v2`
//! （实验版本，只用于断言显式拒绝）。

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use acp_host::wire::{ERROR_INVALID_PARAMS, ERROR_INVALID_REQUEST, ERROR_METHOD_NOT_FOUND};
use acp_host::{AcpClientAdapterFactory, CwdResolver, JsonRpcMessage, SessionResolver};
use agent_domain::{CoreInstanceId, EventId, MessageId, RunId, WorkspaceId};
use async_trait::async_trait;
use client_adapter_api::{
    AdapterError, CanonicalCoreFrame, CapabilitySnapshot, ClientAdapter, ClientCapability,
    ClientProtocol, ClientSessionId, InMemorySessionRegistryStore, SessionRegistry,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};
use core_api::{AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence};
use serde_json::{json, Value};

use common::{acp_notification, acp_request, parse, TestHarness};

fn fixture(path: &str) -> Value {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    serde_json::from_str(&std::fs::read_to_string(&full).expect("read fixture"))
        .expect("fixture is valid JSON")
}

/// 常量 cwd 解析器（测试用）。
struct FixedCwdResolver(WorkspaceId);

#[async_trait]
impl CwdResolver for FixedCwdResolver {
    async fn resolve(&self, _cwd: &str) -> Result<WorkspaceId, AdapterError> {
        Ok(self.0.clone())
    }
}

/// 常量 session 解析器（测试用）。
struct FixedSessionResolver(ClientSessionId);

#[async_trait]
impl SessionResolver for FixedSessionResolver {
    async fn resolve_client_session(&self, _event: &AppEventEnvelope) -> Option<ClientSessionId> {
        Some(self.0.clone())
    }
}

/// 构造一个已协商的 ACP adapter（不经 host，供回译 golden 测试）。
async fn negotiated_adapter() -> Arc<acp_host::AcpClientAdapter> {
    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let workspace = WorkspaceId::from("ws-golden");
    let client_session = ClientSessionId::new("acp-golden-session");
    let factory = AcpClientAdapterFactory::new(
        std::iter::empty::<ClientCapability>(),
        registry,
        Arc::new(FixedCwdResolver(workspace)),
        Arc::new(FixedSessionResolver(client_session)),
        acp_host::wire::Implementation {
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
        api_version: core_api::API_VERSION,
        instance_id: CoreInstanceId::from("instance-golden"),
        event_id: EventId::from("event-golden"),
        global_sequence: GlobalSequence(1),
        stream,
        stream_sequence: 1,
        timestamp: agent_domain::Timestamp::from_unix_millis(1),
        source: EventSource::Core,
        payload,
    }
}

/// golden：握手 result 与 v1 fixture 完全一致（协议版本、能力声明、agent 身份）。
#[tokio::test]
async fn golden_initialize_handshake_matches_fixture() {
    let harness = TestHarness::new(test_support::MockScript::new().complete()).await;
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

/// golden：实验 protocolVersion=2 的握手被显式拒绝（不混入实验 v2）。
#[tokio::test]
async fn golden_v2_initialize_is_rejected() {
    let harness = TestHarness::new(test_support::MockScript::new().complete()).await;
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

/// golden：未知方法（session/load 能力未声明）显式拒绝，错误对象与 fixture 一致。
#[tokio::test]
async fn golden_unknown_method_error_matches_fixture() {
    let harness = TestHarness::new(test_support::MockScript::new().complete()).await;
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

/// golden：AssistantDelta 回译为 session/update 通知，负载与 fixture 一致。
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

/// golden：`session/request_permission` 响应按官方嵌套形状解析为 Selected，
/// 与 fixture 完全一致。
#[tokio::test]
async fn golden_permission_response_selected_matches_fixture() {
    let adapter = negotiated_adapter().await;
    let decision = adapter
        .decode_permission_response(fixture("fixtures/v1/permission-response-selected.json"))
        .expect("selected 响应可解析");
    assert_eq!(
        decision,
        acp_host::PermissionDecision::Selected {
            option_id: "allow-once".into(),
        }
    );
}

/// golden：官方嵌套 `{"outcome":{"outcome":"cancelled"}}` 解析为 Cancelled。
#[tokio::test]
async fn golden_permission_response_cancelled_matches_fixture() {
    let adapter = negotiated_adapter().await;
    let decision = adapter
        .decode_permission_response(fixture("fixtures/v1/permission-response-cancelled.json"))
        .expect("cancelled 响应可解析");
    assert_eq!(decision, acp_host::PermissionDecision::Cancelled);
}

/// 评审前扁平旧形状（`{"outcome":"selected",...}`）必须显式拒绝（-32602 语义），
/// 不静默兼容旧格式。
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
    // 嵌套形状里混入未知字段同样拒绝。
    let error = adapter
        .decode_permission_response(json!({
            "outcome": { "outcome": "selected", "optionId": "allow-once", "bogus": 1 },
        }))
        .expect_err("未知字段必须拒绝");
    assert!(error.to_string().contains("bogus"), "{}", error.to_string());
}

/// wire：`session/new` 的 `mcpServers` 按官方 schema 必填——缺失 → -32602，
/// 显式空数组放行到后续校验（cwd 未登记 → -32603，证明已通过参数解析）。
#[tokio::test]
async fn session_new_requires_mcp_servers_field() {
    let harness = TestHarness::new(test_support::MockScript::new().complete()).await;
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

/// golden fixture + 全链路：`session/resume` 的 `mcpServers` /
/// `additionalDirectories` 按官方 builder 缺省（省略即空数组）——wire 解析、
/// serde 默认值与真实 host 路径（new → close → resume 不带这两个字段）全部
/// 通过；`session/new` 的 mcpServers 必填语义由
/// `session_new_requires_mcp_servers_field` 单独覆盖。
#[tokio::test]
async fn session_resume_omitted_builder_defaults_are_accepted() {
    let request = fixture("fixtures/v1/session-resume-minimal.json");
    let JsonRpcMessage::Request(message) = parse(request).expect("fixture 可解析") else {
        panic!("expected request");
    };
    let params = serde_json::from_value::<acp_host::wire::SessionResumeParams>(
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

    // 真实 host 路径：session/new（必填保持）→ close → resume 省略
    // mcpServers / additionalDirectories。
    let harness = TestHarness::new(test_support::MockScript::new().complete()).await;
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

/// wire：非对象消息与错误 jsonrpc 版本按规范拒绝。
#[test]
fn jsonrpc_parse_rejects_non_object_and_bad_version() {
    let error = parse(json!([])).expect_err("数组不是合法消息");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
    let error = parse(json!({ "jsonrpc": "1.0", "id": 1, "method": "initialize" }))
        .expect_err("jsonrpc 1.0 必须拒绝");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
    let error = parse(json!({ "jsonrpc": "2.0", "id": 1 })).expect_err("缺少 method/id");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
    // 合法请求与通知往返一致。
    let request = acp_request(3, "session/new", json!({ "cwd": "/tmp" }));
    let parsed = parse(request.clone()).expect("请求可解析");
    assert_eq!(parsed.to_value(), request);
    let notification = acp_notification("session/cancel", json!({ "sessionId": "s1" }));
    let parsed = parse(notification.clone()).expect("通知可解析");
    assert_eq!(parsed.to_value(), notification);
}

/// wire：未知 params 字段显式拒绝（-32602）。
#[tokio::test]
async fn unknown_params_fields_are_rejected() {
    let harness = TestHarness::new(test_support::MockScript::new().complete()).await;
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
