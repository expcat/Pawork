//! CLI 与 GUI 共享的应用层协议类型。

use std::{fmt, path::Component, str::FromStr};

use agent_domain::{
    ActorId, ArtifactId, CommandId, ConnectionId, CoreInstanceId, ErrorContext, EventId,
    GuiClientId, MessageId, ModelId, PluginId, ProviderId, QueryId, RunId, SessionId, Timestamp,
    ToolCallId, WorkspaceId,
};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;
use ts_rs::TS;

pub const API_VERSION: ApiVersion = ApiVersion { major: 1, minor: 0 };

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl ApiVersion {
    pub const fn is_compatible_with(self, other: Self) -> bool {
        self.major == other.major
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ApiHandle {
    pub instance_id: CoreInstanceId,
    pub api_version: ApiVersion,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct AppCommandEnvelope {
    pub api_version: ApiVersion,
    pub command_id: CommandId,
    pub source: CommandSource,
    pub identity: ActorIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub issued_at: Timestamp,
    pub command: AppCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct AppQueryEnvelope {
    pub api_version: ApiVersion,
    pub request_id: QueryId,
    pub source: CommandSource,
    pub identity: ActorIdentity,
    pub issued_at: Timestamp,
    pub query: AppQuery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandSource {
    LocalCli {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_session_id: Option<String>,
    },
    LocalGui {
        client_id: GuiClientId,
    },
    RemoteGui {
        client_id: GuiClientId,
        connection_id: ConnectionId,
    },
    Automation,
    Plugin,
    Mcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActorIdentity {
    LocalUser {
        actor_id: ActorId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    AuthenticatedClient {
        actor_id: ActorId,
        subject: String,
    },
    Automation {
        name: String,
    },
    Plugin {
        plugin_id: PluginId,
    },
    McpServer {
        server_id: String,
    },
    System,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AppCommand {
    CoreInitialize,
    WorkspaceAdd {
        root_path: String,
    },
    WorkspaceTrust {
        workspace_id: WorkspaceId,
        trusted: bool,
    },
    SessionCreate {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    SessionOpen {
        session_id: SessionId,
    },
    SessionFork {
        session_id: SessionId,
        parent_event_id: EventId,
    },
    SessionCompact {
        session_id: SessionId,
    },
    RunStart {
        session_id: SessionId,
        user_message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelId>,
    },
    RunCancel {
        run_id: RunId,
    },
    RunRetry {
        run_id: RunId,
    },
    RunTool {
        run_id: RunId,
        tool_name: String,
        input: Value,
    },
    AuthStart {
        provider_id: ProviderId,
        flow: String,
    },
    AuthRemove {
        provider_id: ProviderId,
    },
    ToolApprove {
        run_id: RunId,
        tool_call_id: ToolCallId,
        decision: ApprovalDecision,
    },
    GitStage {
        workspace_id: WorkspaceId,
        paths: Vec<WorkspaceRelativePath>,
    },
    TerminalCreate {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_directory: Option<WorkspaceRelativePath>,
    },
    TerminalWrite {
        terminal_session_id: String,
        data: String,
    },
    TerminalResize {
        terminal_session_id: String,
        columns: u16,
        rows: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    ApproveOnce,
    ApproveForRun,
    Deny,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AppQuery {
    WorkspaceList,
    SessionGet {
        session_id: SessionId,
    },
    RunStatus {
        run_id: RunId,
    },
    ModelList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_id: Option<ProviderId>,
    },
    DiffListFiles {
        workspace_id: WorkspaceId,
    },
    DiffGet {
        workspace_id: WorkspaceId,
        path: WorkspaceRelativePath,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    ArtifactRead {
        artifact_id: ArtifactId,
        offset: u64,
        limit: u64,
    },
    SnapshotFetch,
    PluginList,
    McpList,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct AppResponseEnvelope {
    pub api_version: ApiVersion,
    pub request_id: QueryId,
    pub responded_at: Timestamp,
    pub response: AppResponse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AppResponse {
    Accepted {
        command_id: CommandId,
    },
    Data(Value),
    Artifact {
        artifact_id: ArtifactId,
        byte_length: u64,
        media_type: String,
    },
    Error(ErrorContext),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct AppEventEnvelope {
    pub api_version: ApiVersion,
    pub instance_id: CoreInstanceId,
    pub event_id: EventId,
    pub global_sequence: GlobalSequence,
    pub stream: EventStream,
    pub stream_sequence: u64,
    pub timestamp: Timestamp,
    pub source: EventSource,
    pub payload: AppEvent,
}

impl AppEventEnvelope {
    pub fn validate_after(&self, previous: &Self) -> Result<(), AppEventOrderError> {
        if self.instance_id != previous.instance_id {
            return Err(AppEventOrderError::DifferentInstance);
        }
        if !self
            .global_sequence
            .is_immediately_after(previous.global_sequence)
        {
            return Err(AppEventOrderError::NonContiguousGlobalSequence);
        }
        if self.stream == previous.stream
            && previous.stream_sequence.checked_add(1) != Some(self.stream_sequence)
        {
            return Err(AppEventOrderError::NonContiguousStreamSequence);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub struct GlobalSequence(pub u64);

impl GlobalSequence {
    pub fn is_immediately_after(self, previous: Self) -> bool {
        previous.0.checked_add(1) == Some(self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum EventStream {
    Global,
    Workspace(WorkspaceId),
    Session(SessionId),
    Run(RunId),
    Terminal(String),
    GuiClient(GuiClientId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventSource {
    Core,
    Command {
        command_id: CommandId,
        source: CommandSource,
    },
    Provider {
        provider_id: ProviderId,
    },
    Tool {
        tool_call_id: ToolCallId,
    },
    Plugin {
        plugin_id: PluginId,
    },
    Mcp {
        server_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AppEvent {
    CoreReady {
        handle: ApiHandle,
    },
    WorkspaceChanged {
        workspace_id: WorkspaceId,
        revision: u64,
    },
    SessionChanged {
        session_id: SessionId,
        revision: u64,
    },
    RunChanged {
        run_id: RunId,
        state: RunState,
    },
    AssistantDelta {
        run_id: RunId,
        message_id: MessageId,
        delta: String,
    },
    ThinkingDelta {
        run_id: RunId,
        message_id: MessageId,
        delta: String,
    },
    ToolStarted {
        run_id: RunId,
        tool_call_id: ToolCallId,
        name: String,
    },
    ToolOutput {
        run_id: RunId,
        tool_call_id: ToolCallId,
        delta: String,
        truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact_id: Option<ArtifactId>,
    },
    ToolApprovalRequired {
        run_id: RunId,
        tool_call_id: ToolCallId,
        reason: String,
    },
    ToolCompleted {
        run_id: RunId,
        tool_call_id: ToolCallId,
        success: bool,
    },
    DiffChanged {
        workspace_id: WorkspaceId,
    },
    TerminalOutput {
        terminal_session_id: String,
        delta: String,
    },
    AuthChanged {
        provider_id: ProviderId,
        authenticated: bool,
    },
    ProviderStatus {
        provider_id: ProviderId,
        status: ProviderStatus,
    },
    PluginError {
        plugin_id: PluginId,
        error: ErrorContext,
    },
    Diagnostic {
        level: DiagnosticLevel,
        code: String,
        message: String,
    },
    GuiClientConnected {
        client_id: GuiClientId,
        connection_id: ConnectionId,
    },
    GuiClientDisconnected {
        client_id: GuiClientId,
        connection_id: ConnectionId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Created,
    PreparingContext,
    WaitingForProvider,
    StreamingResponse,
    CollectingToolCalls,
    WaitingForApproval,
    ExecutingTools,
    AppendingToolResults,
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Ready,
    Degraded,
    Unavailable,
    AuthenticationRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AppEventOrderError {
    #[error("events belong to different Core instances")]
    DifferentInstance,
    #[error("global event sequence is not contiguous")]
    NonContiguousGlobalSequence,
    #[error("stream event sequence is not contiguous")]
    NonContiguousStreamSequence,
}

/// 已验证的 Workspace 相对路径。反序列化同样执行校验，不能绕过构造器。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
pub struct WorkspaceRelativePath(String);

impl WorkspaceRelativePath {
    pub fn new(value: impl Into<String>) -> Result<Self, RelativePathError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let has_windows_prefix =
            bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        let has_cross_platform_parent = value.split(['/', '\\']).any(|component| component == "..");
        if value.is_empty()
            || value.contains('\0')
            || value.starts_with(['/', '\\'])
            || has_windows_prefix
            || has_cross_platform_parent
        {
            return Err(RelativePathError);
        }
        let path = std::path::Path::new(&value);
        if path.is_absolute()
            || path.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(RelativePathError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceRelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for WorkspaceRelativePath {
    type Err = RelativePathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for WorkspaceRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WorkspaceRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(|_| de::Error::custom("expected a safe workspace-relative path"))
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("path must be non-empty, workspace-relative, and contain no parent traversal")]
pub struct RelativePathError;

#[cfg(test)]
mod tests {
    use super::*;

    fn command_source() -> CommandSource {
        CommandSource::RemoteGui {
            client_id: GuiClientId::from("gui-1"),
            connection_id: ConnectionId::from("connection-1"),
        }
    }

    #[test]
    fn command_envelope_round_trip_preserves_source_identity_and_idempotency() {
        let envelope = AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("command-1"),
            source: command_source(),
            identity: ActorIdentity::AuthenticatedClient {
                actor_id: ActorId::from("actor-1"),
                subject: "user@example".into(),
            },
            expected_revision: Some(7),
            idempotency_key: Some("create-run-once".into()),
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::GitStage {
                workspace_id: WorkspaceId::from("workspace-1"),
                paths: vec![WorkspaceRelativePath::new("src/lib.rs").expect("relative path")],
            },
        };

        let json = serde_json::to_string(&envelope).expect("serialize command");
        let decoded: AppCommandEnvelope = serde_json::from_str(&json).expect("deserialize command");
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn unsafe_paths_are_rejected_even_during_deserialization() {
        assert!(WorkspaceRelativePath::new("../secret").is_err());
        assert!(WorkspaceRelativePath::new("/absolute").is_err());
        assert!(WorkspaceRelativePath::new(r"..\secret").is_err());
        assert!(WorkspaceRelativePath::new(r"C:\Windows").is_err());
        assert!(WorkspaceRelativePath::new(r"C:drive-relative").is_err());
        assert!(WorkspaceRelativePath::new(r"\\server\share").is_err());
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""../secret""#).is_err());
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""C:\\Windows""#).is_err());
    }

    #[test]
    fn event_sequence_overflow_is_reported_instead_of_panicking() {
        let first = event(u64::MAX, u64::MAX);
        let second = event(0, 0);
        assert_eq!(
            second.validate_after(&first),
            Err(AppEventOrderError::NonContiguousGlobalSequence)
        );
    }

    #[test]
    fn event_sequences_are_global_and_per_stream() {
        let first = event(1, 5);
        let second = event(2, 6);
        assert_eq!(second.validate_after(&first), Ok(()));

        let skipped = event(4, 7);
        assert_eq!(
            skipped.validate_after(&second),
            Err(AppEventOrderError::NonContiguousGlobalSequence)
        );
    }

    fn event(global_sequence: u64, stream_sequence: u64) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: EventId::from(format!("event-{global_sequence}")),
            global_sequence: GlobalSequence(global_sequence),
            stream: EventStream::Run(RunId::from("run-1")),
            stream_sequence,
            timestamp: Timestamp::from_unix_millis(global_sequence),
            source: EventSource::Core,
            payload: AppEvent::RunChanged {
                run_id: RunId::from("run-1"),
                state: RunState::StreamingResponse,
            },
        }
    }

    #[test]
    fn api_major_version_controls_compatibility() {
        assert!(API_VERSION.is_compatible_with(ApiVersion { major: 1, minor: 9 }));
        assert!(!API_VERSION.is_compatible_with(ApiVersion { major: 2, minor: 0 }));
    }
}
