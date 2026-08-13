//! 全帧型 round trip 与帧大小上限（L1 定向测试）。

use agent_domain::{
    ActorId, ArtifactId, CommandId, ConnectionId, CoreInstanceId, EventId, GuiClientId, QueryId,
    RunId, SessionId, Timestamp,
};
use core_api::{
    ActorIdentity, ApiHandle, ApiVersion, AppCommand, AppCommandEnvelope, AppEvent,
    AppEventEnvelope, AppQuery, AppQueryEnvelope, AppResponse, AppResponseEnvelope, CommandSource,
    EventSource, EventStream, GlobalSequence, RunState, API_VERSION,
};
use gui_protocol::{
    encode_client_frame, encode_server_frame, ArtifactChunk, ArtifactReadRequest,
    ClientAuthentication, ClientFrame, GuiCapability, HandshakeRequest, HandshakeResponse,
    ProtocolError, ProtocolErrorCode, ProtocolErrorEnvelope, ResumeDisposition, ResumeRequest,
    ResumeResponse, ServerFrame, Snapshot, SnapshotSection, SnapshotSectionKind, SubscribeRequest,
    MAX_ARTIFACT_CHUNK_BYTES, MAX_PROTOCOL_FRAME_BYTES,
};
use serde_json::json;

fn command_envelope() -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("command-1"),
        source: CommandSource::LocalGui {
            client_id: GuiClientId::from("gui-1"),
        },
        identity: ActorIdentity::LocalUser {
            actor_id: ActorId::from("actor-1"),
            display_name: None,
        },
        expected_revision: Some(3),
        idempotency_key: Some("create-run-once".into()),
        issued_at: Timestamp::from_unix_millis(1),
        command: AppCommand::RunStart {
            session_id: SessionId::from("session-1"),
            user_message: "hello".into(),
            model: None,
            profile: None,
        },
    }
}

fn query_envelope() -> AppQueryEnvelope {
    AppQueryEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from("query-1"),
        source: CommandSource::RemoteGui {
            client_id: GuiClientId::from("gui-2"),
            connection_id: ConnectionId::from("connection-2"),
        },
        identity: ActorIdentity::AuthenticatedClient {
            actor_id: ActorId::from("actor-2"),
            subject: "user@example".into(),
        },
        issued_at: Timestamp::from_unix_millis(2),
        query: AppQuery::WorkspaceList,
    }
}

fn response_envelope() -> AppResponseEnvelope {
    AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from("query-1"),
        responded_at: Timestamp::from_unix_millis(3),
        response: AppResponse::Data(json!({"workspaces": []})),
    }
}

fn event_envelope() -> AppEventEnvelope {
    AppEventEnvelope {
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
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        instance_id: CoreInstanceId::from("instance-1"),
        snapshot_sequence: GlobalSequence(42),
        generated_at: Timestamp::from_unix_millis(5),
        sections: vec![
            SnapshotSection {
                kind: SnapshotSectionKind::ActiveRuns,
                revision: 3,
                data: Some(json!({"run_ids": ["run-1"]})),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::SessionTree,
                revision: 1,
                data: None,
                artifact_id: Some(ArtifactId::from("artifact-1")),
            },
        ],
    }
}

fn handshake_request() -> HandshakeRequest {
    HandshakeRequest {
        request_id: "request-1".into(),
        client_name: "desktop".into(),
        client_version: "0.1.0".into(),
        supported_api_versions: vec![ApiVersion { major: 1, minor: 0 }],
        capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
        authentication: Some(ClientAuthentication {
            scheme: "bearer".into(),
            proof: "secret".into(),
        }),
    }
}

fn sample_client_frames() -> Vec<ClientFrame> {
    vec![
        ClientFrame::Handshake(handshake_request()),
        ClientFrame::Command(command_envelope()),
        ClientFrame::Query(query_envelope()),
        ClientFrame::Subscribe(SubscribeRequest {
            request_id: "request-2".into(),
            subscription_id: "subscription-1".into(),
            streams: vec![EventStream::Global, EventStream::Run(RunId::from("run-1"))],
        }),
        ClientFrame::Unsubscribe {
            request_id: "request-3".into(),
            subscription_id: "subscription-1".into(),
        },
        ClientFrame::Resume(ResumeRequest {
            request_id: "request-4".into(),
            last_global_sequence: GlobalSequence(41),
        }),
        ClientFrame::SnapshotRequest {
            request_id: "request-5".into(),
        },
        ClientFrame::Ack {
            global_sequence: GlobalSequence(42),
        },
        ClientFrame::ArtifactRead(ArtifactReadRequest {
            request_id: "request-6".into(),
            artifact_id: ArtifactId::from("artifact-1"),
            offset: 0,
            limit: 1024,
        }),
        ClientFrame::Heartbeat { nonce: 7 },
        ClientFrame::Pong { nonce: 8 },
    ]
}

fn sample_server_frames() -> Vec<ServerFrame> {
    vec![
        ServerFrame::Handshake(HandshakeResponse::Accepted {
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
        }),
        ServerFrame::Handshake(HandshakeResponse::Rejected {
            request_id: "request-1".into(),
            error: ProtocolError::incompatible_version("no compatible API version"),
        }),
        ServerFrame::CommandAccepted {
            request_id: "request-2".into(),
            command_id: CommandId::from("command-1"),
        },
        ServerFrame::Response(response_envelope()),
        ServerFrame::Event(event_envelope()),
        ServerFrame::Snapshot(snapshot()),
        ServerFrame::Resume(ResumeResponse {
            request_id: "request-4".into(),
            disposition: ResumeDisposition::Replay {
                from_sequence: GlobalSequence(41),
                through_sequence: GlobalSequence(42),
            },
        }),
        ServerFrame::ArtifactChunk(ArtifactChunk {
            request_id: "request-6".into(),
            artifact_id: ArtifactId::from("artifact-1"),
            offset: 0,
            data: vec![1, 2, 3],
            eof: true,
        }),
        ServerFrame::Error(ProtocolErrorEnvelope {
            request_id: Some("request-7".into()),
            error: ProtocolError {
                code: ProtocolErrorCode::RequestNotFound,
                message: "unknown request".into(),
                retryable: false,
            },
        }),
        ServerFrame::Heartbeat { nonce: 9 },
        ServerFrame::Pong { nonce: 10 },
    ]
}

#[test]
fn all_client_frame_variants_round_trip() {
    for frame in sample_client_frames() {
        let bytes = encode_client_frame(&frame).expect("encode client frame");
        let decoded = gui_protocol::decode_client_frame(&bytes).expect("decode client frame");
        assert_eq!(decoded, frame, "client frame variant did not round trip");
    }
}

#[test]
fn all_server_frame_variants_round_trip() {
    for frame in sample_server_frames() {
        let bytes = encode_server_frame(&frame).expect("encode server frame");
        let decoded = gui_protocol::decode_server_frame(&bytes).expect("decode server frame");
        assert_eq!(decoded, frame, "server frame variant did not round trip");
    }
}

#[test]
fn oversized_payload_is_rejected_on_encode() {
    let frame = ClientFrame::Handshake(HandshakeRequest {
        request_id: "request-1".into(),
        client_name: "x".repeat(MAX_PROTOCOL_FRAME_BYTES),
        client_version: "0.1.0".into(),
        supported_api_versions: vec![API_VERSION],
        capabilities: vec![],
        authentication: None,
    });
    assert!(matches!(
        encode_client_frame(&frame),
        Err(gui_protocol::ProtocolCodecError::FrameTooLarge { .. })
    ));
}

#[test]
fn oversized_artifact_chunk_is_rejected_on_encode_and_decode() {
    let frame = ServerFrame::ArtifactChunk(ArtifactChunk {
        request_id: "request-1".into(),
        artifact_id: ArtifactId::from("artifact-1"),
        offset: 0,
        data: vec![0; MAX_ARTIFACT_CHUNK_BYTES + 1],
        eof: false,
    });
    assert!(matches!(
        encode_server_frame(&frame),
        Err(gui_protocol::ProtocolCodecError::ArtifactChunkTooLarge { .. })
    ));

    let bytes = serde_json::to_vec(&frame).expect("serialize oversized chunk");
    assert!(matches!(
        gui_protocol::decode_server_frame(&bytes),
        Err(gui_protocol::ProtocolCodecError::ArtifactChunkTooLarge { .. })
    ));
}

#[test]
fn snapshot_validation_runs_on_encode_and_decode() {
    let invalid = Snapshot {
        instance_id: CoreInstanceId::from("instance-1"),
        snapshot_sequence: GlobalSequence(42),
        generated_at: Timestamp::from_unix_millis(5),
        sections: vec![SnapshotSection {
            kind: SnapshotSectionKind::ActiveRuns,
            revision: 3,
            data: Some(json!({"run_ids": ["run-1"]})),
            artifact_id: Some(ArtifactId::from("artifact-1")),
        }],
    };
    let frame = ServerFrame::Snapshot(invalid);
    assert!(matches!(
        encode_server_frame(&frame),
        Err(gui_protocol::ProtocolCodecError::AmbiguousSnapshotSection)
    ));

    let bytes = serde_json::to_vec(&frame).expect("serialize invalid snapshot");
    assert!(matches!(
        gui_protocol::decode_server_frame(&bytes),
        Err(gui_protocol::ProtocolCodecError::AmbiguousSnapshotSection)
    ));
}

#[test]
fn oversized_encoded_frame_is_rejected_on_decode() {
    let bytes = vec![b' '; MAX_PROTOCOL_FRAME_BYTES + 1];
    assert!(matches!(
        gui_protocol::decode_client_frame(&bytes),
        Err(gui_protocol::ProtocolCodecError::FrameTooLarge { .. })
    ));
}

#[test]
fn authentication_debug_is_redacted() {
    let auth = ClientAuthentication {
        scheme: "bearer".into(),
        proof: "secret".into(),
    };
    assert!(!format!("{auth:?}").contains("secret"));
}
