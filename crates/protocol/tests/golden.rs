//! Golden JSON：锁定线上 serde 格式（tag/content/rename_all 冻结契约）。
//!
//! fixture 缺失时设置 `GUI_PROTOCOL_UPDATE_GOLDEN=1` 重新生成；
//! 正常运行下任何线上格式漂移都会在此失败。

use std::{env, fs, path::PathBuf};

use pawork_domain::{
    ArtifactId, CommandId, ConnectionId, CoreInstanceId, EventId, GuiClientId, RunId, Timestamp,
};
use pawork_protocol::{
    encode_client_frame, encode_server_frame, AppQuery, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, ArtifactChunk, ArtifactReadRequest, ClientAuthentication, ClientFrame,
    GuiCapability, HandshakeRequest, HandshakeResponse, ProtocolError, ProtocolErrorCode,
    ProtocolErrorEnvelope, ResumeDisposition, ResumeRequest, ResumeResponse, ServerFrame, Snapshot,
    SnapshotSection, SnapshotSectionKind, SubscribeRequest, TimelineItem, TimelineItemKind,
    TimelinePage, WorkspaceRelativePath,
};
use pawork_protocol::{
    ActorIdentity, ApiHandle, ApiKeySecret, AppCommand, AppCommandEnvelope, AppEvent,
    AppEventEnvelope, AuthChangeState, CommandSource, EventSource, EventStream, GlobalSequence,
    RunState, API_VERSION,
};
use serde_json::Value;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(FIXTURES).join(name)
}

fn assert_golden(name: &str, frame: Value) {
    let path = fixture_path(name);
    if !path.exists() {
        if env::var("GUI_PROTOCOL_UPDATE_GOLDEN").is_ok() {
            fs::create_dir_all(path.parent().expect("fixture parent"))
                .expect("create fixtures dir");
            fs::write(
                &path,
                serde_json::to_string_pretty(&frame).expect("pretty json"),
            )
            .expect("write golden fixture");
            return;
        }
        panic!(
            "golden fixture {} is missing; run with GUI_PROTOCOL_UPDATE_GOLDEN=1 to create it",
            path.display()
        );
    }
    let expected: Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("read golden fixture"))
            .expect("parse golden fixture");
    assert_eq!(
        frame,
        expected,
        "wire format drifted from golden fixture {}",
        path.display()
    );
}

fn client_handshake_frame() -> ClientFrame {
    ClientFrame::Handshake(HandshakeRequest {
        request_id: "request-1".into(),
        client_name: "desktop".into(),
        client_version: "0.1.0".into(),
        supported_api_versions: vec![pawork_protocol::ApiVersion { major: 1, minor: 0 }],
        capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
        authentication: Some(ClientAuthentication {
            scheme: "bearer".into(),
            proof: "secret".into(),
        }),
    })
}

fn client_command_frame() -> ClientFrame {
    ClientFrame::Command(AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("command-1"),
        source: CommandSource::RemoteGui {
            client_id: GuiClientId::from("gui-1"),
            connection_id: ConnectionId::from("connection-1"),
        },
        identity: ActorIdentity::LocalUser {
            actor_id: pawork_domain::ActorId::from("actor-1"),
            display_name: None,
        },
        expected_revision: Some(3),
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command: AppCommand::SessionCreate {
            workspace_id: pawork_domain::WorkspaceId::from("workspace-1"),
            title: None,
        },
    })
}

fn server_handshake_accepted_frame() -> ServerFrame {
    ServerFrame::Handshake(HandshakeResponse::Accepted {
        request_id: "request-1".into(),
        selected_api_version: API_VERSION,
        handle: ApiHandle {
            instance_id: CoreInstanceId::from("instance-1"),
            api_version: API_VERSION,
        },
        client_id: GuiClientId::from("gui-1"),
        connection_id: ConnectionId::from("connection-1"),
        resume: ResumeDisposition::Replay {
            from_sequence: GlobalSequence(16),
            through_sequence: GlobalSequence(20),
        },
        capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
        host_data_dir: Some("/tmp/pawork-data".into()),
    })
}

fn server_handshake_rejected_frame() -> ServerFrame {
    ServerFrame::Handshake(HandshakeResponse::Rejected {
        request_id: "request-1".into(),
        error: ProtocolError {
            code: ProtocolErrorCode::IncompatibleVersion,
            message: "no compatible API version".into(),
            retryable: false,
        },
    })
}

fn server_event_frame() -> ServerFrame {
    ServerFrame::Event(AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: CoreInstanceId::from("instance-1"),
        event_id: EventId::from("event-1"),
        global_sequence: GlobalSequence(1),
        stream: EventStream::Run(RunId::from("run-1")),
        stream_sequence: 1,
        timestamp: Timestamp::from_unix_millis(4),
        source: EventSource::Core,
        payload: AppEvent::RunChanged {
            run_id: RunId::from("run-1"),
            state: RunState::StreamingResponse,
        },
    })
}

fn server_snapshot_frame() -> ServerFrame {
    ServerFrame::Snapshot(Snapshot {
        instance_id: CoreInstanceId::from("instance-1"),
        snapshot_sequence: GlobalSequence(42),
        generated_at: Timestamp::from_unix_millis(5),
        sections: vec![
            SnapshotSection {
                kind: SnapshotSectionKind::ActiveRuns,
                revision: 3,
                data: Some(serde_json::json!({"run_ids": ["run-1"]})),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::SessionTree,
                revision: 1,
                data: None,
                artifact_id: Some(ArtifactId::from("artifact-1")),
            },
        ],
    })
}

fn server_error_frame() -> ServerFrame {
    ServerFrame::Error(ProtocolErrorEnvelope {
        request_id: Some("request-2".into()),
        error: ProtocolError {
            code: ProtocolErrorCode::FrameTooLarge,
            message: "protocol frame is too large".into(),
            retryable: false,
        },
    })
}

fn client_terminal_command_frame(command: AppCommand) -> ClientFrame {
    ClientFrame::Command(AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("command-terminal-1"),
        source: CommandSource::RemoteGui {
            client_id: GuiClientId::from("gui-1"),
            connection_id: ConnectionId::from("connection-1"),
        },
        identity: ActorIdentity::LocalUser {
            actor_id: pawork_domain::ActorId::from("actor-1"),
            display_name: None,
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(2),
        command,
    })
}

fn client_auth_command_frame(command: AppCommand) -> ClientFrame {
    ClientFrame::Command(AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("command-auth-1"),
        source: CommandSource::RemoteGui {
            client_id: GuiClientId::from("gui-1"),
            connection_id: ConnectionId::from("connection-1"),
        },
        identity: ActorIdentity::LocalUser {
            actor_id: pawork_domain::ActorId::from("actor-1"),
            display_name: None,
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(2),
        command,
    })
}

#[test]
fn golden_client_frames() {
    let handshake: Value = serde_json::from_slice(
        &encode_client_frame(&client_handshake_frame()).expect("encode handshake"),
    )
    .expect("parse handshake");
    assert_golden("client_handshake.json", handshake);

    let command: Value = serde_json::from_slice(
        &encode_client_frame(&client_command_frame()).expect("encode command"),
    )
    .expect("parse command");
    assert_golden("client_command.json", command);
}

#[test]
fn golden_server_frames() {
    let accepted: Value = serde_json::from_slice(
        &encode_server_frame(&server_handshake_accepted_frame()).expect("encode accepted"),
    )
    .expect("parse accepted");
    assert_golden("server_handshake_accepted.json", accepted);

    let rejected: Value = serde_json::from_slice(
        &encode_server_frame(&server_handshake_rejected_frame()).expect("encode rejected"),
    )
    .expect("parse rejected");
    assert_golden("server_handshake_rejected.json", rejected);

    let event: Value =
        serde_json::from_slice(&encode_server_frame(&server_event_frame()).expect("encode event"))
            .expect("parse event");
    assert_golden("server_event.json", event);

    let snapshot: Value = serde_json::from_slice(
        &encode_server_frame(&server_snapshot_frame()).expect("encode snapshot"),
    )
    .expect("parse snapshot");
    assert_golden("server_snapshot.json", snapshot);

    let error: Value =
        serde_json::from_slice(&encode_server_frame(&server_error_frame()).expect("encode error"))
            .expect("parse error");
    assert_golden("server_error.json", error);
}

fn session_get_timeline_frame() -> ClientFrame {
    ClientFrame::Query(AppQueryEnvelope {
        api_version: API_VERSION,
        request_id: pawork_domain::QueryId::from("query-timeline"),
        source: CommandSource::LocalGui {
            client_id: GuiClientId::from("gui-1"),
        },
        identity: ActorIdentity::LocalUser {
            actor_id: pawork_domain::ActorId::from("actor-1"),
            display_name: None,
        },
        issued_at: Timestamp::from_unix_millis(1),
        query: AppQuery::SessionGet {
            session_id: pawork_domain::SessionId::from("session-1"),
            timeline_after_sequence: Some(10),
            timeline_limit: Some(50),
        },
    })
}

fn timeline_page() -> TimelinePage {
    TimelinePage {
        items: vec![
            TimelineItem {
                sequence: 11,
                event_id: "event-11".into(),
                kind: TimelineItemKind::UserMessage,
                run_id: None,
                text: Some("hello".into()),
                tool_name: None,
                status: None,
                detail: None,
                timestamp: "2026-01-01T00:00:00Z".into(),
            },
            TimelineItem {
                sequence: 12,
                event_id: "event-12".into(),
                kind: TimelineItemKind::AssistantMessage,
                run_id: Some("run-1".into()),
                text: Some("hi".into()),
                tool_name: None,
                status: None,
                detail: None,
                timestamp: "2026-01-01T00:00:01Z".into(),
            },
        ],
        next_sequence: Some(13),
        head_sequence: 20,
        complete: false,
    }
}

#[test]
fn golden_timeline_types() {
    let query: Value = serde_json::from_slice(
        &encode_client_frame(&session_get_timeline_frame()).expect("encode session get"),
    )
    .expect("parse session get");
    assert_golden("session_get_timeline.json", query);

    let page: Value = serde_json::to_value(&timeline_page()).expect("timeline page");
    assert_golden("timeline_page.json", page);
}

fn encode_client(frame: &ClientFrame) -> Value {
    serde_json::from_slice(&encode_client_frame(frame).expect("encode client"))
        .expect("parse client")
}

fn encode_server(frame: &ServerFrame) -> Value {
    serde_json::from_slice(&encode_server_frame(frame).expect("encode server"))
        .expect("parse server")
}

#[test]
fn golden_additional_client_frames() {
    assert_golden(
        "client_subscribe.json",
        encode_client(&ClientFrame::Subscribe(SubscribeRequest {
            request_id: "request-2".into(),
            subscription_id: "subscription-1".into(),
            streams: vec![EventStream::Global, EventStream::Run(RunId::from("run-1"))],
        })),
    );
    assert_golden(
        "client_unsubscribe.json",
        encode_client(&ClientFrame::Unsubscribe {
            request_id: "request-3".into(),
            subscription_id: "subscription-1".into(),
        }),
    );
    assert_golden(
        "client_resume.json",
        encode_client(&ClientFrame::Resume(ResumeRequest {
            request_id: "request-4".into(),
            last_global_sequence: GlobalSequence(41),
        })),
    );
    assert_golden(
        "client_snapshot_request.json",
        encode_client(&ClientFrame::SnapshotRequest {
            request_id: "request-5".into(),
        }),
    );
    assert_golden(
        "client_ack.json",
        encode_client(&ClientFrame::Ack {
            global_sequence: GlobalSequence(42),
        }),
    );
    assert_golden(
        "client_artifact_read.json",
        encode_client(&ClientFrame::ArtifactRead(ArtifactReadRequest {
            request_id: "request-6".into(),
            artifact_id: ArtifactId::from("artifact-1"),
            offset: 0,
            limit: 1024,
        })),
    );
    assert_golden(
        "client_heartbeat.json",
        encode_client(&ClientFrame::Heartbeat { nonce: 7 }),
    );
    assert_golden(
        "client_pong.json",
        encode_client(&ClientFrame::Pong { nonce: 8 }),
    );
    assert_golden(
        "client_command_terminal_create.json",
        encode_client(&client_terminal_command_frame(AppCommand::TerminalCreate {
            workspace_id: pawork_domain::WorkspaceId::from("workspace-1"),
            working_directory: None,
        })),
    );
    assert_golden(
        "client_command_terminal_create_working_directory.json",
        encode_client(&client_terminal_command_frame(AppCommand::TerminalCreate {
            workspace_id: pawork_domain::WorkspaceId::from("workspace-1"),
            working_directory: Some(
                WorkspaceRelativePath::new("src").expect("valid relative path"),
            ),
        })),
    );
    assert_golden(
        "client_command_terminal_write.json",
        encode_client(&client_terminal_command_frame(AppCommand::TerminalWrite {
            terminal_session_id: "terminal-1".into(),
            data: "echo hi\n".into(),
        })),
    );
    assert_golden(
        "client_command_terminal_resize.json",
        encode_client(&client_terminal_command_frame(AppCommand::TerminalResize {
            terminal_session_id: "terminal-1".into(),
            columns: 80,
            rows: 24,
        })),
    );
    assert_golden(
        "client_command_terminal_close.json",
        encode_client(&client_terminal_command_frame(AppCommand::TerminalClose {
            terminal_session_id: "terminal-1".into(),
        })),
    );
}

#[test]
fn golden_additional_server_frames() {
    assert_golden(
        "server_command_accepted.json",
        encode_server(&ServerFrame::CommandAccepted {
            request_id: "request-2".into(),
            command_id: CommandId::from("command-1"),
        }),
    );
    assert_golden(
        "server_response.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-1"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({"workspaces": []})),
        })),
    );
    assert_golden(
        "server_response_terminal_create.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-1"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "terminal_session_id": "terminal-1",
                "sandboxed": false,
                "policy": "allow_with_constraints",
                "approval_mode": "ask_for_dangerous",
                "note": "创建已经 policy 闸(ask_for_dangerous 档);PTY 会话内容不经沙箱与逐条审批",
            })),
        })),
    );
    assert_golden(
        "server_event_terminal_output.json",
        encode_server(&ServerFrame::Event(AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: EventId::from("event-1"),
            global_sequence: GlobalSequence(1),
            stream: EventStream::Terminal("terminal-1".into()),
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(4),
            source: EventSource::Core,
            payload: AppEvent::TerminalOutput {
                terminal_session_id: "terminal-1".into(),
                delta: "echo hi\r\n".into(),
            },
        })),
    );
    assert_golden(
        "server_event_terminal_exited.json",
        encode_server(&ServerFrame::Event(AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: EventId::from("event-2"),
            global_sequence: GlobalSequence(2),
            stream: EventStream::Terminal("terminal-1".into()),
            stream_sequence: 2,
            timestamp: Timestamp::from_unix_millis(5),
            source: EventSource::Core,
            payload: AppEvent::TerminalExited {
                terminal_session_id: "terminal-1".into(),
                exit_code: Some(0),
                signal: None,
                reason: pawork_protocol::TerminalExitReason::Exited,
            },
        })),
    );
    assert_golden(
        "server_resume_replay.json",
        encode_server(&ServerFrame::Resume(ResumeResponse {
            request_id: "request-4".into(),
            disposition: ResumeDisposition::Replay {
                from_sequence: GlobalSequence(41),
                through_sequence: GlobalSequence(42),
            },
        })),
    );
    assert_golden(
        "server_resume_snapshot_required.json",
        encode_server(&ServerFrame::Resume(ResumeResponse {
            request_id: "request-4".into(),
            disposition: ResumeDisposition::SnapshotRequired {
                earliest_available_sequence: GlobalSequence(10),
            },
        })),
    );
    assert_golden(
        "server_resume_up_to_date.json",
        encode_server(&ServerFrame::Resume(ResumeResponse {
            request_id: "request-4".into(),
            disposition: ResumeDisposition::UpToDate {
                current_sequence: GlobalSequence(42),
            },
        })),
    );
    assert_golden(
        "server_artifact_chunk.json",
        encode_server(&ServerFrame::ArtifactChunk(ArtifactChunk {
            request_id: "request-6".into(),
            artifact_id: ArtifactId::from("artifact-1"),
            offset: 0,
            data: vec![1, 2, 3],
            eof: true,
        })),
    );
    assert_golden(
        "server_heartbeat.json",
        encode_server(&ServerFrame::Heartbeat { nonce: 9 }),
    );
    assert_golden(
        "server_pong.json",
        encode_server(&ServerFrame::Pong { nonce: 10 }),
    );
    assert_golden(
        "server_handshake_accepted_up_to_date.json",
        encode_server(&ServerFrame::Handshake(HandshakeResponse::Accepted {
            request_id: "request-1".into(),
            selected_api_version: API_VERSION,
            handle: ApiHandle {
                instance_id: CoreInstanceId::from("instance-1"),
                api_version: API_VERSION,
            },
            client_id: GuiClientId::from("gui-1"),
            connection_id: ConnectionId::from("connection-1"),
            resume: ResumeDisposition::UpToDate {
                current_sequence: GlobalSequence(42),
            },
            capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
            host_data_dir: None,
        })),
    );
}

/// SET-1（ADR-046）：Provider 认证与默认模型协议词汇 golden。
#[test]
fn golden_auth_provider_slices() {
    let provider_id = pawork_domain::ProviderId::from("glm-coding");
    assert_golden(
        "client_command_auth_set_api_key.json",
        encode_client(&client_auth_command_frame(AppCommand::AuthSetApiKey {
            provider_id: provider_id.clone(),
            api_key: ApiKeySecret::new("sk-test-fixture-not-a-real-key"),
        })),
    );
    assert_golden(
        "client_command_auth_start.json",
        encode_client(&client_auth_command_frame(AppCommand::AuthStart {
            provider_id: provider_id.clone(),
            flow: "oauth".into(),
        })),
    );
    assert_golden(
        "client_command_auth_remove.json",
        encode_client(&client_auth_command_frame(AppCommand::AuthRemove {
            provider_id: provider_id.clone(),
        })),
    );
    assert_golden(
        "client_command_auth_cancel.json",
        encode_client(&client_auth_command_frame(AppCommand::AuthCancel {
            provider_id: provider_id.clone(),
        })),
    );
    assert_golden(
        "client_command_set_default_model.json",
        encode_client(&client_auth_command_frame(AppCommand::SetDefaultModel {
            provider_id: provider_id.clone(),
            model_id: "glm-4.7".into(),
        })),
    );
    assert_golden(
        "provider_auth_status.json",
        encode_client(&ClientFrame::Query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-auth-status"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("gui-1"),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: pawork_domain::ActorId::from("actor-1"),
                display_name: None,
            },
            issued_at: Timestamp::from_unix_millis(1),
            query: AppQuery::ProviderAuthStatus {
                provider_id: Some(provider_id.clone()),
            },
        })),
    );
    assert_golden(
        "server_event_auth_changed.json",
        encode_server(&ServerFrame::Event(AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: EventId::from("event-auth-1"),
            global_sequence: GlobalSequence(1),
            stream: EventStream::Global,
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(4),
            source: EventSource::Provider {
                provider_id: provider_id.clone(),
            },
            payload: AppEvent::AuthChanged {
                provider_id: provider_id.clone(),
                state: AuthChangeState::Succeeded {
                    method: "api_key".into(),
                    masked_credential: "sk-****abcd".into(),
                },
            },
        })),
    );
    assert_golden(
        "server_response_provider_auth_status.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-auth-status"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "providers": [{
                    "provider_id": "glm-coding",
                    "display_name": "GLM Coding",
                    "endpoint_label": "https://api.z.ai/api/coding/paas/v4",
                    "auth_methods": ["api_key"],
                    "auth": {
                        "type": "connected",
                        "method": "api_key",
                        "masked_credential": "sk-****abcd"
                    },
                    "catalog": {
                        "type": "fixed_fallback",
                        "snapshot_label": "2026-09-01",
                        "fetched_at": null
                    },
                    "use_proxy": true
                }]
            })),
        })),
    );
    assert_golden(
        "server_response_auth_set_api_key.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-auth-status"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "provider_id": "glm-coding",
                "method": "api_key",
                "masked_credential": "sk-****abcd",
                "verified_at": "2026-01-01T00:00:00Z"
            })),
        })),
    );
}

/// SET-6a（ADR-047）：通用设置查询 / SetProxyUrl 命令 golden。
#[test]
fn golden_general_settings_slices() {
    let provider_id = pawork_domain::ProviderId::from("glm-coding");
    assert_golden(
        "general_settings.json",
        encode_client(&ClientFrame::Query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-general-settings"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("gui-1"),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: pawork_domain::ActorId::from("actor-1"),
                display_name: None,
            },
            issued_at: Timestamp::from_unix_millis(1),
            query: AppQuery::GeneralSettings,
        })),
    );
    assert_golden(
        "client_command_set_proxy_url.json",
        encode_client(&client_auth_command_frame(AppCommand::SetProxyUrl {
            proxy_url: Some("http://127.0.0.1:7890".into()),
        })),
    );
    assert_golden(
        "client_command_set_proxy_url_clear.json",
        encode_client(&client_auth_command_frame(AppCommand::SetProxyUrl {
            proxy_url: None,
        })),
    );
    assert_golden(
        "client_command_set_provider_use_proxy.json",
        encode_client(&client_auth_command_frame(AppCommand::SetProviderUseProxy {
            provider_id: provider_id.clone(),
            use_proxy: false,
        })),
    );
    assert_golden(
        "server_response_general_settings.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-general-settings"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "proxy_url": "http://127.0.0.1:7890"
            })),
        })),
    );
    assert_golden(
        "server_response_set_proxy_url.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-set-proxy-url"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "proxy_url": null
            })),
        })),
    );
    assert_golden(
        "server_response_set_provider_use_proxy.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-set-provider-use-proxy"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "provider_id": "glm-coding",
                "use_proxy": false
            })),
        })),
    );
}

/// SET-6b（ADR-048）：权限与审批查询 / SetApprovalMode / WorkspaceTrust golden。
#[test]
fn golden_permissions_settings_slices() {
    assert_golden(
        "permissions_settings.json",
        encode_client(&ClientFrame::Query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-permissions-settings"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("gui-1"),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: pawork_domain::ActorId::from("actor-1"),
                display_name: None,
            },
            issued_at: Timestamp::from_unix_millis(1),
            query: AppQuery::PermissionsSettings,
        })),
    );
    assert_golden(
        "client_command_set_approval_mode.json",
        encode_client(&client_auth_command_frame(AppCommand::SetApprovalMode {
            mode: "ask_for_writes".into(),
        })),
    );
    assert_golden(
        "client_command_workspace_trust.json",
        encode_client(&client_auth_command_frame(AppCommand::WorkspaceTrust {
            workspace_id: pawork_domain::WorkspaceId::from("workspace-1"),
            trusted: true,
        })),
    );
    assert_golden(
        "server_response_permissions_settings.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-permissions-settings"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "approval_mode": "read_only",
                "workspace_trusted": false,
                "trust_workspaces_global": null,
                "workspace_id": "workspace-1"
            })),
        })),
    );
    assert_golden(
        "server_response_permissions_settings_trust_global.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-permissions-settings"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "approval_mode": "ask_for_writes",
                "workspace_trusted": true,
                "trust_workspaces_global": true,
                "workspace_id": "workspace-1"
            })),
        })),
    );
    assert_golden(
        "server_response_set_approval_mode.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-set-approval-mode"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "approval_mode": "ask_for_writes"
            })),
        })),
    );
    assert_golden(
        "server_response_workspace_trust.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-workspace-trust"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "workspace_trusted": true
            })),
        })),
    );
}

/// SET-6c（ADR-049）：MCP test / server remove 命令与回执 golden。
///
/// 回执 Data 复用 mcp_list 的 servers 数组形状（McpServerStatus：
/// name/transport/state/tools/last_error）；remove 回执为移除后的清单，
/// 不再含该 server。
#[test]
fn golden_mcp_settings_slices() {
    assert_golden(
        "client_command_mcp_test.json",
        encode_client(&client_auth_command_frame(AppCommand::McpTest {
            name: "context7".into(),
        })),
    );
    assert_golden(
        "client_command_mcp_server_remove.json",
        encode_client(&client_auth_command_frame(AppCommand::McpServerRemove {
            name: "context7".into(),
        })),
    );
    assert_golden(
        "server_response_mcp_test.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-mcp-test"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "servers": [{
                    "name": "context7",
                    "transport": "stdio",
                    "state": "connected",
                    "tools": ["search_docs"],
                    "last_error": null
                }]
            })),
        })),
    );
    assert_golden(
        "server_response_mcp_server_remove.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-mcp-server-remove"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "servers": []
            })),
        })),
    );
}

/// SET-6 第四页（ADR-050）：终端设置查询 / SetTerminalSettings 命令 golden。
///
/// 查询响应 Data 形状 `{ shell, columns, rows }`：shell 为 Global 持久值
/// （null = 平台默认），columns/rows 为生效值；set 命令三字段必填，
/// `shell: null` 显式清除，回执回写完整状态（清除帧收据，同
/// set_proxy_url 先例）。
#[test]
fn golden_terminal_settings_slices() {
    assert_golden(
        "terminal_settings.json",
        encode_client(&ClientFrame::Query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-terminal-settings"),
            source: CommandSource::LocalGui {
                client_id: GuiClientId::from("gui-1"),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: pawork_domain::ActorId::from("actor-1"),
                display_name: None,
            },
            issued_at: Timestamp::from_unix_millis(1),
            query: AppQuery::TerminalSettings,
        })),
    );
    assert_golden(
        "client_command_set_terminal_settings.json",
        encode_client(&client_auth_command_frame(AppCommand::SetTerminalSettings {
            shell: Some("/bin/zsh".into()),
            columns: 120,
            rows: 40,
        })),
    );
    assert_golden(
        "client_command_set_terminal_settings_clear.json",
        encode_client(&client_auth_command_frame(AppCommand::SetTerminalSettings {
            shell: None,
            columns: 120,
            rows: 40,
        })),
    );
    assert_golden(
        "server_response_terminal_settings.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-terminal-settings"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "shell": null,
                "columns": 80,
                "rows": 24
            })),
        })),
    );
    assert_golden(
        "server_response_set_terminal_settings.json",
        encode_server(&ServerFrame::Response(AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-set-terminal-settings"),
            responded_at: Timestamp::from_unix_millis(3),
            response: AppResponse::Data(serde_json::json!({
                "shell": null,
                "columns": 120,
                "rows": 40
            })),
        })),
    );
}
