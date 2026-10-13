//! 应用层事件信封与流序。

use pawork_domain::{
    ArtifactId, CommandId, ConnectionId, CoreInstanceId, DegradeEvent, ErrorContext, EventId,
    GuiClientId, MessageId, PluginId, ProviderId, RunId, SessionId, Timestamp, ToolCallId,
    WorkspaceId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
#[cfg(feature = "typegen")]
use ts_rs::TS;

use super::command::CommandSource;
use super::quota::{QuotaAlert, QuotaOverviewView};
use super::version::{ApiHandle, ApiVersion};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct GlobalSequence(pub u64);

impl GlobalSequence {
    pub fn is_immediately_after(self, previous: Self) -> bool {
        previous.0.checked_add(1) == Some(self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum EventStream {
    Global,
    Workspace(WorkspaceId),
    Session(SessionId),
    Run(RunId),
    Terminal(String),
    GuiClient(GuiClientId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
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
    /// ADR-054 D5：改名 / 归档 / 自动标题写回成功后的会话元数据变化；
    /// payload 为写后状态，Desktop 收到后重取 snapshot。
    SessionMetaChanged {
        session_id: SessionId,
        title: String,
        archived: bool,
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
    TerminalExited {
        terminal_session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<String>,
        reason: TerminalExitReason,
    },
    AuthChanged {
        provider_id: ProviderId,
        state: AuthChangeState,
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
    QuotaChanged {
        view: Box<QuotaOverviewView>,
    },
    QuotaAlert {
        alert: Box<QuotaAlert>,
    },
}

/// Provider 认证变更状态机（SET-1，ADR-046）：终态与中间态全部脱敏，
/// `Succeeded` 只携带 method 与 masked_credential，绝无明文凭证。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AuthChangeState {
    Pending,
    Succeeded {
        method: String,
        masked_credential: String,
    },
    Failed {
        error: String,
    },
    Cancelled,
    Expired,
    Removed,
}

/// 终端终态原因（ADR-045）：自然退出 / 经 terminal_close 终止 / 转发链路异常断流。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum TerminalExitReason {
    Exited,
    Killed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Ready,
    Degraded,
    Unavailable,
    AuthenticationRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

impl DiagnosticLevel {
    /// Map a domain `DegradeSeverity` via its frozen `as_str()` wire token.
    /// Unknown tokens fall back to [`DiagnosticLevel::Info`] so a new severity
    /// cannot invent a protocol arm.
    fn from_degrade_severity_str(value: &str) -> Self {
        match value {
            "info" => Self::Info,
            "warning" => Self::Warning,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

/// Protocol-side conversion: `DegradeEvent` reuses the existing
/// [`AppEvent::Diagnostic`] shape (level/code/message). serde is unchanged.
impl From<&DegradeEvent> for AppEvent {
    fn from(event: &DegradeEvent) -> Self {
        AppEvent::Diagnostic {
            level: DiagnosticLevel::from_degrade_severity_str(event.severity.as_str()),
            code: event.code(),
            message: event.message.clone(),
        }
    }
}

// Pin: ACP / desktop consumers must keep treating Diagnostic as the only
// degrade wire arm. Adding a dedicated AppEvent variant would break 26-frame
// golden, events_golden, and typegen schemas.
const _: fn(&DegradeEvent) -> AppEvent = |event| AppEvent::from(event);

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AppEventOrderError {
    #[error("events belong to different Core instances")]
    DifferentInstance,
    #[error("global event sequence is not contiguous")]
    NonContiguousGlobalSequence,
    #[error("stream event sequence is not contiguous")]
    NonContiguousStreamSequence,
}

#[cfg(test)]
mod tests {
    use pawork_domain::{CoreInstanceId, EventId, RunId, Timestamp};

    use super::*;
    use crate::app::API_VERSION;

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

    #[test]
    fn degrade_event_maps_to_diagnostic_via_as_str() {
        use pawork_domain::{DegradeKind, DegradeSeverity};
        use serde_json::json;

        let cases = [
            (DegradeSeverity::Info, DiagnosticLevel::Info),
            (DegradeSeverity::Warning, DiagnosticLevel::Warning),
            (DegradeSeverity::Error, DiagnosticLevel::Error),
        ];
        for (severity, level) in cases {
            let event = DegradeEvent::new(
                DegradeKind::HomeDirFallback,
                severity,
                "home missing; using temp",
                json!({}),
            );
            assert_eq!(
                AppEvent::from(&event),
                AppEvent::Diagnostic {
                    level,
                    code: "degrade.home_dir_fallback".into(),
                    message: "home missing; using temp".into(),
                }
            );
        }
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
}
