//! GUI 与 CLI/Core 宿主之间的线上协议。
//!
//! 本 crate 只定义帧、版本协商、Snapshot 与重连语义，不实现 Server、连接管理
//! 或 Transport。所有帧都有有界 JSON codec，避免慢客户端或大型 payload 占满内存。

use agent_domain::{ArtifactId, CommandId, ConnectionId, CoreInstanceId, GuiClientId, Timestamp};
use core_api::{
    ApiHandle, ApiVersion, AppCommandEnvelope, AppEventEnvelope, AppQueryEnvelope,
    AppResponseEnvelope, EventStream, GlobalSequence,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use ts_rs::TS;

pub const MAX_PROTOCOL_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_ARTIFACT_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ClientFrame {
    Handshake(HandshakeRequest),
    Command(AppCommandEnvelope),
    Query(AppQueryEnvelope),
    Subscribe(SubscribeRequest),
    Unsubscribe {
        request_id: String,
        subscription_id: String,
    },
    Resume(ResumeRequest),
    SnapshotRequest {
        request_id: String,
    },
    Ack {
        global_sequence: GlobalSequence,
    },
    ArtifactRead(ArtifactReadRequest),
    Heartbeat {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerFrame {
    Handshake(HandshakeResponse),
    CommandAccepted {
        request_id: String,
        command_id: CommandId,
    },
    Response(AppResponseEnvelope),
    Event(AppEventEnvelope),
    Snapshot(Snapshot),
    Resume(ResumeResponse),
    ArtifactChunk(ArtifactChunk),
    Error(ProtocolErrorEnvelope),
    Heartbeat {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct HandshakeRequest {
    pub request_id: String,
    pub client_name: String,
    pub client_version: String,
    pub supported_api_versions: Vec<ApiVersion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<GuiCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<ClientAuthentication>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HandshakeResponse {
    Accepted {
        request_id: String,
        selected_api_version: ApiVersion,
        handle: ApiHandle,
        client_id: GuiClientId,
        connection_id: ConnectionId,
        resume: ResumeDisposition,
    },
    Rejected {
        request_id: String,
        error: ProtocolError,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GuiCapability {
    Events,
    Snapshots,
    ArtifactStreaming,
    TerminalStreaming,
    Approvals,
}

/// 握手凭证只携带 opaque proof；协议日志必须单独执行 redaction。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClientAuthentication {
    pub scheme: String,
    pub proof: String,
}

impl std::fmt::Debug for ClientAuthentication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClientAuthentication")
            .field("scheme", &self.scheme)
            .field("proof", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SubscribeRequest {
    pub request_id: String,
    pub subscription_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<EventStream>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ResumeRequest {
    pub request_id: String,
    pub last_global_sequence: GlobalSequence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ResumeResponse {
    pub request_id: String,
    pub disposition: ResumeDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResumeDisposition {
    Replay {
        from_sequence: GlobalSequence,
        through_sequence: GlobalSequence,
    },
    SnapshotRequired {
        earliest_available_sequence: GlobalSequence,
    },
    UpToDate {
        current_sequence: GlobalSequence,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Snapshot {
    pub instance_id: CoreInstanceId,
    pub snapshot_sequence: GlobalSequence,
    pub generated_at: Timestamp,
    pub sections: Vec<SnapshotSection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct SnapshotSection {
    pub kind: SnapshotSectionKind,
    pub revision: u64,
    /// Snapshot 必须有界；大型内容改用 `artifact_id`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<ArtifactId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotSectionKind {
    Workspaces,
    SessionTree,
    ActiveRuns,
    PendingToolApprovals,
    TerminalSessions,
    ProviderStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ArtifactReadRequest {
    pub request_id: String,
    pub artifact_id: ArtifactId,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ArtifactChunk {
    pub request_id: String,
    pub artifact_id: ArtifactId,
    pub offset: u64,
    pub data: Vec<u8>,
    pub eof: bool,
}

impl ArtifactChunk {
    pub fn validate(&self) -> Result<(), ProtocolCodecError> {
        if self.data.len() > MAX_ARTIFACT_CHUNK_BYTES {
            return Err(ProtocolCodecError::ArtifactChunkTooLarge {
                actual: self.data.len(),
                limit: MAX_ARTIFACT_CHUNK_BYTES,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ProtocolErrorEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub error: ProtocolError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorCode {
    IncompatibleVersion,
    InvalidFrame,
    AuthenticationFailed,
    PermissionDenied,
    RequestNotFound,
    ReplayUnavailable,
    FrameTooLarge,
    Internal,
}

pub fn negotiate_api_version(
    client_supported: &[ApiVersion],
    server: ApiVersion,
) -> Option<ApiVersion> {
    client_supported
        .iter()
        .copied()
        .filter(|candidate| candidate.major == server.major)
        .map(|candidate| ApiVersion {
            major: server.major,
            minor: candidate.minor.min(server.minor),
        })
        .max()
}

pub fn encode_client_frame(frame: &ClientFrame) -> Result<Vec<u8>, ProtocolCodecError> {
    encode_bounded(frame)
}

pub fn decode_client_frame(bytes: &[u8]) -> Result<ClientFrame, ProtocolCodecError> {
    decode_bounded(bytes)
}

pub fn encode_server_frame(frame: &ServerFrame) -> Result<Vec<u8>, ProtocolCodecError> {
    if let ServerFrame::ArtifactChunk(chunk) = frame {
        chunk.validate()?;
    }
    encode_bounded(frame)
}

pub fn decode_server_frame(bytes: &[u8]) -> Result<ServerFrame, ProtocolCodecError> {
    let frame: ServerFrame = decode_bounded(bytes)?;
    if let ServerFrame::ArtifactChunk(chunk) = &frame {
        chunk.validate()?;
    }
    Ok(frame)
}

fn encode_bounded<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolCodecError> {
    let bytes = serde_json::to_vec(value).map_err(ProtocolCodecError::InvalidJson)?;
    ensure_frame_size(bytes.len())?;
    Ok(bytes)
}

fn decode_bounded<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ProtocolCodecError> {
    ensure_frame_size(bytes.len())?;
    serde_json::from_slice(bytes).map_err(ProtocolCodecError::InvalidJson)
}

fn ensure_frame_size(actual: usize) -> Result<(), ProtocolCodecError> {
    if actual > MAX_PROTOCOL_FRAME_BYTES {
        return Err(ProtocolCodecError::FrameTooLarge {
            actual,
            limit: MAX_PROTOCOL_FRAME_BYTES,
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ProtocolCodecError {
    #[error("invalid protocol JSON: {0}")]
    InvalidJson(serde_json::Error),
    #[error("protocol frame is too large: {actual} bytes, limit {limit}")]
    FrameTooLarge { actual: usize, limit: usize },
    #[error("artifact chunk is too large: {actual} bytes, limit {limit}")]
    ArtifactChunkTooLarge { actual: usize, limit: usize },
}

#[cfg(test)]
mod tests {
    use core_api::API_VERSION;

    use super::*;

    #[test]
    fn handshake_round_trip_and_version_negotiation() {
        let frame = ClientFrame::Handshake(HandshakeRequest {
            request_id: "request-1".into(),
            client_name: "desktop".into(),
            client_version: "0.1.0".into(),
            supported_api_versions: vec![
                ApiVersion { major: 1, minor: 0 },
                ApiVersion { major: 1, minor: 2 },
                ApiVersion { major: 2, minor: 0 },
            ],
            capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
            authentication: Some(ClientAuthentication {
                scheme: "bearer".into(),
                proof: "secret".into(),
            }),
        });
        let bytes = encode_client_frame(&frame).expect("encode frame");
        let decoded = decode_client_frame(&bytes).expect("decode frame");
        assert_eq!(decoded, frame);
        assert_eq!(
            negotiate_api_version(
                &[
                    ApiVersion { major: 1, minor: 2 },
                    ApiVersion { major: 2, minor: 0 }
                ],
                API_VERSION,
            ),
            Some(API_VERSION)
        );
    }

    #[test]
    fn resume_explicitly_selects_replay_or_snapshot() {
        let replay = ResumeDisposition::Replay {
            from_sequence: GlobalSequence(11),
            through_sequence: GlobalSequence(20),
        };
        let snapshot = ResumeDisposition::SnapshotRequired {
            earliest_available_sequence: GlobalSequence(15),
        };
        assert_ne!(replay, snapshot);
    }

    #[test]
    fn oversized_frames_and_artifact_chunks_are_rejected() {
        let bytes = vec![b' '; MAX_PROTOCOL_FRAME_BYTES + 1];
        assert!(matches!(
            decode_client_frame(&bytes),
            Err(ProtocolCodecError::FrameTooLarge { .. })
        ));

        let frame = ServerFrame::ArtifactChunk(ArtifactChunk {
            request_id: "request-1".into(),
            artifact_id: ArtifactId::from("artifact-1"),
            offset: 0,
            data: vec![0; MAX_ARTIFACT_CHUNK_BYTES + 1],
            eof: false,
        });
        assert!(matches!(
            encode_server_frame(&frame),
            Err(ProtocolCodecError::ArtifactChunkTooLarge { .. })
        ));
    }

    #[test]
    fn snapshot_is_anchored_to_global_sequence() {
        let snapshot = Snapshot {
            instance_id: CoreInstanceId::from("instance-1"),
            snapshot_sequence: GlobalSequence(42),
            generated_at: Timestamp::from_unix_millis(1),
            sections: vec![SnapshotSection {
                kind: SnapshotSectionKind::ActiveRuns,
                revision: 3,
                data: Some(serde_json::json!({"run_ids": ["run-1"]})),
                artifact_id: None,
            }],
        };
        let frame = ServerFrame::Snapshot(snapshot.clone());
        let decoded = decode_server_frame(&encode_server_frame(&frame).expect("encode snapshot"))
            .expect("decode snapshot");
        assert_eq!(decoded, frame);
        assert_eq!(snapshot.snapshot_sequence, GlobalSequence(42));
    }

    #[test]
    fn authentication_debug_is_redacted() {
        let auth = ClientAuthentication {
            scheme: "bearer".into(),
            proof: "secret".into(),
        };
        assert!(!format!("{auth:?}").contains("secret"));
    }
}
