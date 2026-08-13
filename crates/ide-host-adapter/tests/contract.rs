//! P17-9 定向 contract 测试：`ClientFrame` ↔ canonical 翻译契约。

use agent_domain::{CommandId, SessionId, Timestamp};
use client_adapter_api::{
    AdapterError, CanonicalClientRequest, CanonicalCoreFrame, CapabilitySnapshot, ClientAdapter,
    ClientAdapterFactory, ClientCapability, ClientFrame, ClientProtocol, ClientSessionId,
    ClientSessionRecord, ClientSessionState, CLIENT_ADAPTER_SCHEMA_VERSION,
};
use core_api::{ActorIdentity, AppCommand, AppCommandEnvelope, CommandSource, API_VERSION};
use ide_host_adapter::{
    IdeCapability, IdeClientAdapter, IdeClientAdapterFactory, IdeRequest,
    IDE_CONTRACT_SCHEMA_VERSION, IDE_PROTOCOL, IDE_PROTOCOL_VERSION,
};
use serde_json::json;

fn snapshot() -> CapabilitySnapshot {
    CapabilitySnapshot {
        schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
        protocol: ClientProtocol::new(IDE_PROTOCOL),
        protocol_version: IDE_PROTOCOL_VERSION.into(),
        client_version: "0.0.0".into(),
        revision: 1,
        capabilities: [ClientCapability::new(IdeCapability::Lifecycle.as_str())]
            .into_iter()
            .collect(),
    }
}

fn command_frame() -> ClientFrame {
    let envelope = AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("cmd-1"),
        source: CommandSource::Automation,
        identity: ActorIdentity::Automation {
            name: "ide-contract".into(),
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command: AppCommand::RunCancel {
            run_id: "run-9".into(),
        },
    };
    ClientFrame {
        schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
        request_id: "cmd-1".into(),
        method: "ide.command".into(),
        payload: serde_json::to_value(CanonicalClientRequest::Command(envelope)).unwrap(),
        extensions: Default::default(),
    }
}

#[tokio::test]
async fn decode_and_encode_round_trip_through_trait_object() {
    let adapter: Box<dyn ClientAdapter> =
        Box::new(IdeClientAdapter::new(snapshot()).expect("valid adapter"));
    assert_eq!(adapter.protocol().0, IDE_PROTOCOL);

    let canonical = adapter
        .decode(command_frame())
        .await
        .expect("command frame decodes");
    match canonical {
        CanonicalClientRequest::Command(envelope) => {
            assert!(matches!(envelope.command, AppCommand::RunCancel { .. }));
        }
        other => panic!("expected command, got {other:?}"),
    }

    let encoded = adapter
        .encode(CanonicalCoreFrame::SessionState(ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(IDE_PROTOCOL),
            client_session_id: ClientSessionId::new("ide:s-1"),
            core_session_id: SessionId::from("s-1"),
            connection_id: "conn-1".into(),
            ownership_epoch: 1,
            revision: 2,
            state: ClientSessionState::Subscribed,
            capabilities: snapshot(),
            updated_at: Timestamp::from_unix_millis(2),
        }))
        .await
        .expect("session state encodes");
    assert_eq!(encoded.method, "ide.session_state");
    assert_eq!(encoded.request_id, "ide:s-1");
    assert!(
        encoded.payload.get("type").is_some(),
        "payload carries canonical frame"
    );
}

#[tokio::test]
async fn negotiation_is_fail_closed() {
    let factory = IdeClientAdapterFactory::new();

    // 允许的能力协商成功。
    let adapter = factory
        .create(snapshot())
        .expect("allowlisted capability negotiates");
    assert!(adapter.require(&ClientCapability::new("lifecycle")).is_ok());

    // 未知能力显式拒绝（不按 client 名猜测）。
    assert!(matches!(
        adapter.require(&ClientCapability::new("account_management")),
        Err(AdapterError::CapabilityUnsupported(_))
    ));

    // 协议不匹配显式拒绝。
    let mut wrong_protocol = snapshot();
    wrong_protocol.protocol = ClientProtocol::new("acp");
    assert!(matches!(
        factory.create(wrong_protocol),
        Err(AdapterError::ProtocolUnsupported(_))
    ));
}

#[tokio::test]
async fn unsupported_methods_and_fields_are_explicit_errors() {
    let adapter: Box<dyn ClientAdapter> =
        Box::new(IdeClientAdapter::new(snapshot()).expect("valid adapter"));

    let unknown = ClientFrame {
        schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
        request_id: "x".into(),
        method: "ide.unknown_method".into(),
        payload: json!({"type": "command", "data": {}}),
        extensions: Default::default(),
    };
    assert!(matches!(
        adapter.decode(unknown).await,
        Err(AdapterError::ProtocolUnsupported(_))
    ));

    let mut with_extension = command_frame();
    with_extension
        .extensions
        .insert("client_field".into(), json!("keep-or-fail"));
    assert!(matches!(
        adapter.decode(with_extension).await,
        Err(AdapterError::InvalidFrame(_))
    ));

    // method 与 payload 变体不一致 → 显式 InvalidFrame，不静默猜类型。
    let mut mismatch = command_frame();
    mismatch.method = "ide.query".into();
    assert!(matches!(
        adapter.decode(mismatch).await,
        Err(AdapterError::InvalidFrame(_))
    ));
}

#[tokio::test]
async fn schema_version_mismatch_is_rejected() {
    let adapter: Box<dyn ClientAdapter> =
        Box::new(IdeClientAdapter::new(snapshot()).expect("valid adapter"));
    let mut frame = command_frame();
    frame.schema_version = 99;
    assert!(matches!(
        adapter.decode(frame).await,
        Err(AdapterError::UnsupportedSchema { .. })
    ));
    assert_eq!(IDE_CONTRACT_SCHEMA_VERSION, 1);
}

/// P18-15 / P17-9：IDE 契约走 `ide-host` + `ide.*` 帧，不经 GUI Connection Protocol，
/// 也不把 Adapter 当作第二 Core（只翻译 ClientAdapter 帧）。
#[test]
fn ide_channel_is_not_gui_and_not_a_second_core() {
    assert_eq!(IDE_PROTOCOL, "ide-host");
    assert_ne!(IDE_PROTOCOL, "gui");
    assert_ne!(IDE_PROTOCOL, "acp");
    let frame = command_frame();
    assert!(
        frame.method.starts_with("ide."),
        "IDE ClientAdapter frames must use ide.* methods, got {}",
        frame.method
    );
    assert!(
        !frame.method.starts_with("gui."),
        "IDE must not speak GUI Connection Protocol frames"
    );

    // 契约消息子集覆盖生命周期 / 诊断 / apply-diff / approval；Adapter 只翻译。
    let _lifecycle = IdeRequest::EditorDidOpen {
        document_uri: "file:///a.rs".into(),
        language_id: "rust".into(),
        text: None,
    };
    let _diagnostics = IdeRequest::DiagnosticsPublish {
        document_uri: "file:///a.rs".into(),
        version: None,
        diagnostics: Vec::new(),
    };
    let _diff = IdeRequest::DiffGet {
        workspace_id: agent_domain::WorkspaceId::from("ws-1"),
        path: "a.rs".into(),
        cursor: None,
    };
    let _approval = IdeRequest::ToolApprove {
        run_id: agent_domain::RunId::from("run-1"),
        tool_call_id: agent_domain::ToolCallId::from("tool-1"),
        decision: core_api::ApprovalDecision::ApproveOnce,
    };
    let adapter = IdeClientAdapter::new(snapshot()).expect("valid adapter");
    assert_eq!(adapter.protocol().0, IDE_PROTOCOL);
    let _ = IdeCapability::Lifecycle;
    let _ = IdeCapability::Diagnostics;
    let _ = IdeCapability::Interaction;
}
