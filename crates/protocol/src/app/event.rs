//! 应用层事件信封、流序与 Team 镜像。

use pawork_domain::{
    AgentId, ArtifactId, CheckpointId, CommandId, ConnectionId, CoreInstanceId, DegradeEvent,
    ErrorContext, EventId, GuiClientId, MessageId, PlanId, PlanVersionId, PluginId, ProviderId,
    RunId, SessionId, TenantId, Timestamp, ToolCallId, WorkspaceId,
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
    /// Team 协作 canonical 事件（P17-6）：typed 镜像，与 `teams::TeamEvent`
    /// serde 形态 1:1；`app-service` 在边界做 1:1 转换后经唯一 EventHub 派发，
    /// 可崩溃重放（重放源为 team 事件流，本事件仅为对外镜像）。
    TeamEvent {
        event: Box<TeamEvent>,
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

// =========================================================================
// Team（P17-6）：canonical 镜像 + typed 事件，全部 TS 导出。
// =========================================================================
//
// 这些类型是 `teams` crate canonical 领域类型的协议镜像：core-api 不依赖
// teams / orchestration（协议 crate 保持轻依赖），但 serde 形态与 teams 保持
// 一致（tag `kind` + snake_case），app-service 在边界做 1:1 转换后经唯一
// EventHub 以 `AppEvent::TeamEvent` 派发；GUI / CLI watch 消费同一份 typed
// 事件流，崩溃恢复的重放源仍为 team 事件流。

/// 成员角色镜像（与 `teams::MemberRole` 1:1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum TeamMemberRole {
    /// 团队根（P12 parent）：可审批 plan、增删成员、解散 team。
    Supervisor,
    /// 普通成员（P12 worker）：可认领 task、收发 mailbox、发起受控 peer 消息。
    Worker,
}

/// 任务状态镜像（与 `orchestration::TaskState` 1:1，复用 P12 状态机）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum TeamTaskState {
    Created,
    Ready,
    Assigned,
    Running,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

/// Plan 步骤状态镜像（与 `pawork_domain::PlanStepStatus` 1:1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum TeamPlanStepStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Blocked,
}

/// Plan 步骤快照镜像（与 `pawork_domain::PlanStepSnapshot` 1:1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct TeamPlanStepSnapshot {
    pub step_id: String,
    pub text: String,
    pub status: TeamPlanStepStatus,
}

/// Plan 行锚点镜像（与 `pawork_domain::PlanCommentAnchor` 1:1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct TeamPlanCommentAnchor {
    pub step_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_offset: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_line: Option<u32>,
}

/// 成员 presence 镜像（与 `teams::Presence` 1:1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum TeamPresence {
    #[default]
    Online,
    Busy,
    Idle,
    Offline,
}

/// mailbox 投递范围镜像（与 `teams::Recipients` 1:1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TeamRecipients {
    /// 点对点：精确的成员列表。
    Direct { members: Vec<AgentId> },
    /// 广播：除发送者外的全部成员。
    Broadcast,
}

/// 共享任务板条目镜像（与 `teams::BoardTask` 1:1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct TeamBoardTask {
    pub task_id: String,
    /// 张贴者。
    pub poster: AgentId,
    /// 当前认领者；`None` 表示未认领。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<AgentId>,
    pub description: String,
    /// 依赖的任务 id。
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub state: TeamTaskState,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub max_retries: u32,
}

/// Team 协作 canonical 事件镜像（与 `teams::TeamEvent` 1:1，18 变体）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TeamEvent {
    TeamCreated {
        team_id: SessionId,
        tenant_id: TenantId,
        supervisor: AgentId,
        name: String,
    },
    MemberAdded {
        team_id: SessionId,
        agent_id: AgentId,
        role: TeamMemberRole,
    },
    MemberRemoved {
        team_id: SessionId,
        agent_id: AgentId,
    },
    TeamDissolved {
        team_id: SessionId,
    },
    TaskPosted {
        team_id: SessionId,
        task: TeamBoardTask,
    },
    TaskClaimed {
        team_id: SessionId,
        task_id: String,
        claimer: AgentId,
    },
    TaskReleased {
        team_id: SessionId,
        task_id: String,
        by: AgentId,
    },
    TaskAdvanced {
        team_id: SessionId,
        task_id: String,
        state: TeamTaskState,
    },
    MailboxPosted {
        team_id: SessionId,
        message_id: MessageId,
        sender: AgentId,
        recipients: TeamRecipients,
        body: String,
    },
    MailboxDelivered {
        team_id: SessionId,
        message_id: MessageId,
        recipient: AgentId,
    },
    MailboxRead {
        team_id: SessionId,
        message_id: MessageId,
        by: AgentId,
    },
    PresenceChanged {
        team_id: SessionId,
        agent_id: AgentId,
        presence: TeamPresence,
    },
    PeerMessageRouted {
        team_id: SessionId,
        message_id: MessageId,
        fan_out_id: CommandId,
        sender: AgentId,
        recipients: TeamRecipients,
        body: String,
    },
    FanOutDenied {
        team_id: SessionId,
        sender: AgentId,
        recipients: TeamRecipients,
        reason: String,
    },
    PlanSubmitted {
        team_id: SessionId,
        plan_id: PlanId,
        version: PlanVersionId,
        title: String,
        steps: Vec<TeamPlanStepSnapshot>,
    },
    PlanApproved {
        team_id: SessionId,
        plan_id: PlanId,
        version: PlanVersionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<CheckpointId>,
    },
    PlanRejected {
        team_id: SessionId,
        plan_id: PlanId,
        version: PlanVersionId,
        reason: String,
    },
    PlanCommented {
        team_id: SessionId,
        plan_id: PlanId,
        version: PlanVersionId,
        anchor: TeamPlanCommentAnchor,
        body: String,
    },
}

impl TeamEvent {
    /// 事件归属的 team（复用 SessionId 作为 opaque team id）。
    pub fn team_id(&self) -> &SessionId {
        match self {
            Self::TeamCreated { team_id, .. }
            | Self::MemberAdded { team_id, .. }
            | Self::MemberRemoved { team_id, .. }
            | Self::TeamDissolved { team_id }
            | Self::TaskPosted { team_id, .. }
            | Self::TaskClaimed { team_id, .. }
            | Self::TaskReleased { team_id, .. }
            | Self::TaskAdvanced { team_id, .. }
            | Self::MailboxPosted { team_id, .. }
            | Self::MailboxDelivered { team_id, .. }
            | Self::MailboxRead { team_id, .. }
            | Self::PresenceChanged { team_id, .. }
            | Self::PeerMessageRouted { team_id, .. }
            | Self::FanOutDenied { team_id, .. }
            | Self::PlanSubmitted { team_id, .. }
            | Self::PlanApproved { team_id, .. }
            | Self::PlanRejected { team_id, .. }
            | Self::PlanCommented { team_id, .. } => team_id,
        }
    }

    /// 稳定的事件种类标签（snake_case，与 serde tag 一致；供渲染 / 过滤）。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TeamCreated { .. } => "team_created",
            Self::MemberAdded { .. } => "member_added",
            Self::MemberRemoved { .. } => "member_removed",
            Self::TeamDissolved { .. } => "team_dissolved",
            Self::TaskPosted { .. } => "task_posted",
            Self::TaskClaimed { .. } => "task_claimed",
            Self::TaskReleased { .. } => "task_released",
            Self::TaskAdvanced { .. } => "task_advanced",
            Self::MailboxPosted { .. } => "mailbox_posted",
            Self::MailboxDelivered { .. } => "mailbox_delivered",
            Self::MailboxRead { .. } => "mailbox_read",
            Self::PresenceChanged { .. } => "presence_changed",
            Self::PeerMessageRouted { .. } => "peer_message_routed",
            Self::FanOutDenied { .. } => "fan_out_denied",
            Self::PlanSubmitted { .. } => "plan_submitted",
            Self::PlanApproved { .. } => "plan_approved",
            Self::PlanRejected { .. } => "plan_rejected",
            Self::PlanCommented { .. } => "plan_commented",
        }
    }
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
