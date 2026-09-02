//! 版本协商、握手服务端逻辑、认证钩子与信封版本校验（L1 定向测试）。

use pawork_domain::{ConnectionId, CoreInstanceId, GuiClientId};
use pawork_protocol::client_auth::{TokenAuthenticator, TokenStore, TOKEN_SCHEME};
use pawork_protocol::{
    decode_client_frame_checked, decode_server_frame_checked, ensure_compatible_api_version,
    negotiate_api_version, negotiate_api_version_with, validate_client_frame_api_version,
    ClientAuthentication, ClientAuthenticator, ClientFrame, GuiCapability, HandshakeRequest,
    HandshakeResponse, ProtocolError, ProtocolErrorCode, ResumeContext, ResumeDisposition,
    ServerFrame,
};
use pawork_protocol::{ApiVersion, CommandSource, GlobalSequence, API_VERSION};

const INSTANCE_ID: &str = "instance-1";

fn service() -> pawork_protocol::HandshakeService {
    pawork_protocol::HandshakeService::new(
        CoreInstanceId::from(INSTANCE_ID),
        vec![API_VERSION],
        vec![GuiCapability::Events, GuiCapability::Snapshots],
    )
}

fn session() -> pawork_protocol::HandshakeSession {
    pawork_protocol::HandshakeSession::new(
        GuiClientId::from("gui-1"),
        ConnectionId::from("connection-1"),
    )
}

fn request(supported: Vec<ApiVersion>, capabilities: Vec<GuiCapability>) -> HandshakeRequest {
    HandshakeRequest {
        request_id: "request-1".into(),
        client_name: "desktop".into(),
        client_version: "0.1.0".into(),
        supported_api_versions: supported,
        capabilities,
        authentication: None,
    }
}

#[test]
fn negotiate_empty_client_list_returns_none() {
    assert_eq!(negotiate_api_version(&[], API_VERSION), None);
}

#[test]
fn negotiate_all_incompatible_returns_none() {
    assert_eq!(
        negotiate_api_version(&[ApiVersion::new(2, 0), ApiVersion::new(3, 1)], API_VERSION),
        None
    );
}

#[test]
fn negotiate_major_mismatch_returns_none() {
    assert_eq!(
        negotiate_api_version_with(
            &[ApiVersion::new(2, 0)],
            &[API_VERSION, ApiVersion::new(1, 3)]
        ),
        None
    );
}

#[test]
fn negotiate_picks_highest_common_minor() {
    assert_eq!(
        negotiate_api_version(
            &[
                ApiVersion::new(1, 0),
                API_VERSION,
                ApiVersion::new(2, 0)
            ],
            API_VERSION,
        ),
        Some(API_VERSION)
    );
    assert_eq!(
        negotiate_api_version_with(
            &[ApiVersion::new(1, 2), ApiVersion::new(1, 5)],
            &[ApiVersion::new(1, 0), ApiVersion::new(1, 3)],
        ),
        Some(ApiVersion::new(1, 3))
    );
}

#[test]
fn handshake_accepts_and_filters_capabilities() {
    let response = service().accept(
        &request(
            vec![API_VERSION],
            vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::TerminalStreaming,
            ],
        ),
        session(),
    );
    let HandshakeResponse::Accepted {
        request_id,
        selected_api_version,
        handle,
        client_id,
        connection_id,
        resume,
        capabilities,
    } = response
    else {
        panic!("expected accepted handshake, got {response:?}");
    };
    assert_eq!(request_id, "request-1");
    assert_eq!(selected_api_version, API_VERSION);
    assert_eq!(handle.instance_id, CoreInstanceId::from(INSTANCE_ID));
    assert_eq!(client_id, GuiClientId::from("gui-1"));
    assert_eq!(connection_id, ConnectionId::from("connection-1"));
    // TerminalStreaming 不在服务端能力内，被筛选掉。
    assert_eq!(
        capabilities,
        vec![GuiCapability::Events, GuiCapability::Snapshots]
    );
    // 全新客户端（无 last_global_sequence）需要 Snapshot。
    assert!(matches!(resume, ResumeDisposition::SnapshotRequired { .. }));
}

#[test]
fn handshake_rejects_when_no_compatible_version() {
    let response = service().accept(&request(vec![ApiVersion::new(2, 0)], vec![]), session());
    let HandshakeResponse::Rejected { request_id, error } = response else {
        panic!("expected rejected handshake");
    };
    assert_eq!(request_id, "request-1");
    assert_eq!(error.code, ProtocolErrorCode::IncompatibleVersion);
    assert!(!error.retryable);
}

#[test]
fn handshake_rejects_empty_client_version_list() {
    let response = service().accept(&request(vec![], vec![]), session());
    let HandshakeResponse::Rejected { error, .. } = response else {
        panic!("expected rejected handshake");
    };
    assert_eq!(error.code, ProtocolErrorCode::IncompatibleVersion);
}

#[test]
fn handshake_computes_resume_from_session_state() {
    let context = ResumeContext::new(GlobalSequence(10), GlobalSequence(20));
    let response = service().accept(
        &request(vec![API_VERSION], vec![]),
        session()
            .with_resume_context(context)
            .with_last_global_sequence(GlobalSequence(15)),
    );
    let HandshakeResponse::Accepted { resume, .. } = response else {
        panic!("expected accepted handshake");
    };
    assert_eq!(
        resume,
        ResumeDisposition::Replay {
            from_sequence: GlobalSequence(16),
            through_sequence: GlobalSequence(20),
        }
    );

    let response = service().accept(
        &request(vec![API_VERSION], vec![]),
        session()
            .with_resume_context(context)
            .with_last_global_sequence(GlobalSequence(20)),
    );
    let HandshakeResponse::Accepted { resume, .. } = response else {
        panic!("expected accepted handshake");
    };
    assert_eq!(
        resume,
        ResumeDisposition::UpToDate {
            current_sequence: GlobalSequence(20),
        }
    );
}

#[test]
fn handshake_rejects_when_authentication_required_but_missing() {
    let service = service().with_authenticator(Box::new(AlwaysReject));
    let response = service.accept(&request(vec![API_VERSION], vec![]), session());
    let HandshakeResponse::Rejected { error, .. } = response else {
        panic!("expected rejected handshake");
    };
    assert_eq!(error.code, ProtocolErrorCode::AuthenticationFailed);
}

#[test]
fn handshake_rejects_when_authentication_fails() {
    let service = service().with_authenticator(Box::new(AlwaysReject));
    let mut request = request(vec![API_VERSION], vec![]);
    request.authentication = Some(ClientAuthentication {
        scheme: "bearer".into(),
        proof: "wrong".into(),
    });
    let response = service.accept(&request, session());
    let HandshakeResponse::Rejected { error, .. } = response else {
        panic!("expected rejected handshake");
    };
    assert_eq!(error.code, ProtocolErrorCode::AuthenticationFailed);
}

#[test]
fn production_token_authenticator_rejects_missing_and_wrong_scheme_and_accepts_valid() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = TokenStore::new(temp.path().join("gui.token"));
    let token = store.generate().expect("generate");
    let service = service().with_authenticator(Box::new(TokenAuthenticator::new(store)));

    let missing = service.accept(&request(vec![API_VERSION], vec![]), session());
    let HandshakeResponse::Rejected { error, .. } = missing else {
        panic!("expected rejected handshake without token");
    };
    assert_eq!(error.code, ProtocolErrorCode::AuthenticationFailed);

    let mut wrong_scheme = request(vec![API_VERSION], vec![]);
    wrong_scheme.authentication = Some(ClientAuthentication {
        scheme: "token".into(),
        proof: token.as_str().into(),
    });
    let rejected = service.accept(&wrong_scheme, session());
    let HandshakeResponse::Rejected { error, .. } = rejected else {
        panic!("expected rejected handshake for wrong scheme");
    };
    assert_eq!(error.code, ProtocolErrorCode::AuthenticationFailed);

    let mut valid = request(vec![API_VERSION], vec![]);
    valid.authentication = Some(ClientAuthentication {
        scheme: TOKEN_SCHEME.into(),
        proof: token.as_str().into(),
    });
    let accepted = service.accept(&valid, session());
    assert!(
        matches!(accepted, HandshakeResponse::Accepted { .. }),
        "expected accepted handshake, got {accepted:?}"
    );
}

#[test]
fn handshake_accepts_when_authentication_passes() {
    let service = service().with_authenticator(Box::new(AlwaysAccept));
    let mut request = request(vec![API_VERSION], vec![]);
    request.authentication = Some(ClientAuthentication {
        scheme: "bearer".into(),
        proof: "valid".into(),
    });
    let response = service.accept(&request, session());
    assert!(matches!(response, HandshakeResponse::Accepted { .. }));
}

#[test]
fn envelope_version_mismatch_produces_incompatible_version() {
    let negotiated = API_VERSION;
    assert!(ensure_compatible_api_version(API_VERSION, negotiated).is_ok());
    let negotiated_minor_one = ApiVersion::new(1, 1);
    assert!(ensure_compatible_api_version(negotiated_minor_one, negotiated_minor_one).is_ok());
    assert!(ensure_compatible_api_version(ApiVersion::new(1, 2), negotiated).is_ok());
    assert!(ensure_compatible_api_version(ApiVersion::new(1, 1), negotiated).is_ok());
    assert!(matches!(
        ensure_compatible_api_version(
            ApiVersion::new(1, negotiated.minor + 1),
            negotiated
        ),
        Err(ProtocolError {
            code: ProtocolErrorCode::IncompatibleVersion,
            ..
        })
    ));
    assert!(matches!(
        ensure_compatible_api_version(ApiVersion::new(2, 0), negotiated),
        Err(ProtocolError {
            code: ProtocolErrorCode::IncompatibleVersion,
            ..
        })
    ));
}

#[test]
fn frame_level_version_validation_covers_command_and_query() {
    let negotiated = API_VERSION;
    let frame = ClientFrame::Heartbeat { nonce: 1 };
    assert!(validate_client_frame_api_version(&frame, negotiated).is_ok());

    // 使用 pawork-protocol 信封构造带错误版本的 Command 帧。
    let envelope = pawork_protocol::AppCommandEnvelope {
        api_version: ApiVersion::new(2, 0),
        command_id: pawork_domain::CommandId::from("command-1"),
        source: CommandSource::LocalCli {
            terminal_session_id: None,
        },
        identity: pawork_protocol::ActorIdentity::System,
        expected_revision: None,
        idempotency_key: None,
        issued_at: pawork_domain::Timestamp::from_unix_millis(1),
        command: pawork_protocol::AppCommand::CoreInitialize,
    };
    assert!(matches!(
        validate_client_frame_api_version(&ClientFrame::Command(envelope), negotiated),
        Err(ProtocolError {
            code: ProtocolErrorCode::IncompatibleVersion,
            ..
        })
    ));
}

#[test]
fn checked_decode_validates_negotiated_version() {
    let frame = ClientFrame::Heartbeat { nonce: 1 };
    let bytes = pawork_protocol::encode_client_frame(&frame).expect("encode");
    assert_eq!(
        decode_client_frame_checked(&bytes, API_VERSION).expect("decode checked"),
        frame
    );

    // 帧字节合法但信封版本与协商结果不符 → IncompatibleVersion。
    let envelope = pawork_protocol::AppCommandEnvelope {
        api_version: ApiVersion::new(1, 99),
        command_id: pawork_domain::CommandId::from("command-1"),
        source: CommandSource::LocalCli {
            terminal_session_id: None,
        },
        identity: pawork_protocol::ActorIdentity::System,
        expected_revision: None,
        idempotency_key: None,
        issued_at: pawork_domain::Timestamp::from_unix_millis(1),
        command: pawork_protocol::AppCommand::CoreInitialize,
    };
    let bytes =
        pawork_protocol::encode_client_frame(&ClientFrame::Command(envelope)).expect("encode");
    assert!(matches!(
        decode_client_frame_checked(&bytes, API_VERSION),
        Err(ProtocolError {
            code: ProtocolErrorCode::IncompatibleVersion,
            ..
        })
    ));

    // 服务器帧的 Event 信封同样被校验。
    let event = pawork_protocol::AppEventEnvelope {
        api_version: ApiVersion::new(2, 0),
        instance_id: CoreInstanceId::from(INSTANCE_ID),
        event_id: pawork_domain::EventId::from("event-1"),
        global_sequence: GlobalSequence(1),
        stream: pawork_protocol::EventStream::Global,
        stream_sequence: 1,
        timestamp: pawork_domain::Timestamp::from_unix_millis(1),
        source: pawork_protocol::EventSource::Core,
        payload: pawork_protocol::AppEvent::CoreReady {
            handle: pawork_protocol::ApiHandle {
                instance_id: CoreInstanceId::from(INSTANCE_ID),
                api_version: API_VERSION,
            },
        },
    };
    let bytes = pawork_protocol::encode_server_frame(&ServerFrame::Event(event)).expect("encode");
    assert!(matches!(
        decode_server_frame_checked(&bytes, API_VERSION),
        Err(ProtocolError {
            code: ProtocolErrorCode::IncompatibleVersion,
            ..
        })
    ));
}

struct AlwaysReject;

impl ClientAuthenticator for AlwaysReject {
    fn authenticate(&self, _authentication: &ClientAuthentication) -> Result<(), ProtocolError> {
        Err(ProtocolError::authentication_failed(
            "rejected by test hook",
        ))
    }
}

struct AlwaysAccept;

impl ClientAuthenticator for AlwaysAccept {
    fn authenticate(&self, _authentication: &ClientAuthentication) -> Result<(), ProtocolError> {
        Ok(())
    }
}
