//! GUI 与 CLI/Core 宿主之间的线上协议。
//!
//! 本 crate 只定义帧、版本协商、Snapshot 与重连语义，不实现 Server、连接管理
//! 或 Transport。所有帧都有有界 JSON codec，避免慢客户端或大型 payload 占满内存。
//!
//! 模块划分：
//! - [`codec`]：有界 JSON 编解码与 u32 LE 长度前缀分帧读写；
//! - [`handshake`]：版本协商、握手服务端逻辑与信封版本校验；
//! - [`resume`]：重连 disposition 计算；
//! - [`snapshot`]：Snapshot 结构校验；
//! - [`error`]：线上结构化错误的构造与 IncompatibleVersion 产生路径。
//!
//! 线上 serde 格式（tag/content/rename_all）是冻结契约，见
//! [ADR-036](../../../../Pawork_v1/docs/adr/ADR-036-gui-protocol-versioning.md)。

use pawork_domain::{ArtifactId, CommandId, ConnectionId, CoreInstanceId, GuiClientId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

pub mod app;
pub mod codec;
pub mod error;
pub mod handshake;
pub mod resume;
pub mod snapshot;

pub use app::*;
pub use app::{
    ActorIdentity, ApiHandle, ApiVersion, AppCommand, AppCommandEnvelope, AppEvent,
    AppEventEnvelope, AppQuery, AppQueryEnvelope, AppResponse, AppResponseEnvelope,
    CommandSource, EventSource, EventStream, GlobalSequence, TimelineItem, TimelineItemKind,
    TimelinePage, API_VERSION, SUPPORTED_API_VERSIONS,
};

pub use codec::{
    decode_client_frame, decode_length_prefixed, decode_server_frame, encode_client_frame,
    encode_length_prefixed, encode_server_frame, read_client_frame, read_frame, read_server_frame,
    read_frame_async, write_client_frame, write_frame, write_frame_async, write_server_frame, ProtocolCodecError,
    FRAME_LENGTH_PREFIX_BYTES,
};
pub use handshake::{
    decode_client_frame_checked, decode_server_frame_checked, ensure_compatible_api_version,
    negotiate_api_version, negotiate_api_version_with, validate_client_frame_api_version,
    validate_server_frame_api_version, ClientAuthenticator, HandshakeService, HandshakeSession,
};
pub use resume::{compute_resume_disposition, ResumeContext};

/// 单帧线上 JSON 上限（含长度前缀）。
pub const MAX_PROTOCOL_FRAME_BYTES: usize = 1024 * 1024;
/// Artifact chunk 数据上限（大 payload 走 Artifact ID，[ADR-018]）。
///
/// [ADR-018]: ../../../../Pawork_v1/docs/adr/ADR-018-large-payload-artifact-id.md
pub const MAX_ARTIFACT_CHUNK_BYTES: usize = 64 * 1024;
/// Snapshot section 内联 data 的编码后上限；超过则必须改用 `artifact_id`。
pub const MAX_SNAPSHOT_SECTION_DATA_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ClientFrame {
    Handshake(HandshakeRequest),
    Command(crate::app::AppCommandEnvelope),
    Query(crate::app::AppQueryEnvelope),
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
        global_sequence: crate::app::GlobalSequence,
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
    Response(crate::app::AppResponseEnvelope),
    Event(crate::app::AppEventEnvelope),
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
    pub supported_api_versions: Vec<crate::app::ApiVersion>,
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
        selected_api_version: crate::app::ApiVersion,
        handle: crate::app::ApiHandle,
        client_id: GuiClientId,
        connection_id: ConnectionId,
        resume: ResumeDisposition,
        /// 服务端按自身能力筛选后授予的能力列表；空列表时省略。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<GuiCapability>,
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
    pub streams: Vec<crate::app::EventStream>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ResumeRequest {
    pub request_id: String,
    pub last_global_sequence: crate::app::GlobalSequence,
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
        from_sequence: crate::app::GlobalSequence,
        through_sequence: crate::app::GlobalSequence,
    },
    SnapshotRequired {
        earliest_available_sequence: crate::app::GlobalSequence,
    },
    UpToDate {
        current_sequence: crate::app::GlobalSequence,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Snapshot {
    pub instance_id: CoreInstanceId,
    pub snapshot_sequence: crate::app::GlobalSequence,
    pub generated_at: Timestamp,
    pub sections: Vec<SnapshotSection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct SnapshotSection {
    pub kind: SnapshotSectionKind,
    pub revision: u64,
    /// Snapshot 必须有界；大型内容改用 `artifact_id`（与 `artifact_id` 互斥）。
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
