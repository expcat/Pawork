//! Golden JSON：锁定线上 serde 格式（tag/content/rename_all 冻结契约）。
//!
//! fixture 缺失时设置 `GUI_PROTOCOL_UPDATE_GOLDEN=1` 重新生成；
//! 正常运行下任何线上格式漂移都会在此失败。

use std::{env, fs, path::PathBuf};

use pawork_domain::{
    ArtifactId, CommandId, ConnectionId, CoreInstanceId, EventId, GuiClientId, RunId, Timestamp,
};
use pawork_protocol::{
    ActorIdentity, ApiHandle, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope,
    CommandSource, EventSource, EventStream, GlobalSequence, RunState, API_VERSION,
};
use pawork_protocol::{
    encode_client_frame, encode_server_frame, ArtifactChunk, ArtifactReadRequest,
    ClientAuthentication, ClientFrame, GuiCapability, HandshakeRequest, HandshakeResponse,
    ProtocolError, ProtocolErrorCode, ProtocolErrorEnvelope, AppQuery, AppQueryEnvelope,
    AppResponse, AppResponseEnvelope, ResumeDisposition, ResumeRequest, ResumeResponse,
    ServerFrame, Snapshot, SnapshotSection, SnapshotSectionKind, SubscribeRequest, TimelineItem,
    TimelineItemKind, TimelinePage, WorkspaceRelativePath,
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
        })),
    );
}
