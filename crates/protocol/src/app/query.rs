//! 应用层查询信封、时间线分页与响应。

use pawork_domain::{
    ArtifactId, CommandId, ErrorContext, ProviderId, QueryId, RunId, SessionId, Timestamp,
    WorkspaceId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(feature = "typegen")]
use ts_rs::TS;

use super::command::{ActorIdentity, CommandSource, WorkspaceRelativePath};
use super::quota::QuotaOverviewQuery;
use super::version::ApiVersion;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct AppQueryEnvelope {
    pub api_version: ApiVersion,
    pub request_id: QueryId,
    pub source: CommandSource,
    pub identity: ActorIdentity,
    pub issued_at: Timestamp,
    pub query: AppQuery,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum AppQuery {
    WorkspaceList,
    SessionGet {
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeline_after_sequence: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeline_limit: Option<u32>,
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
    QuotaOverview {
        query: QuotaOverviewQuery,
    },
    SnapshotFetch,
    PluginList,
    McpList,
    ProviderAuthStatus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_id: Option<ProviderId>,
    },
    /// Global 层通用设置（当前仅 `proxy_url`；ADR-047）。
    GeneralSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct TimelinePage {
    pub items: Vec<TimelineItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_sequence: Option<u64>,
    pub head_sequence: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct TimelineItem {
    pub sequence: u64,
    pub event_id: String,
    pub kind: TimelineItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub timestamp: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum TimelineItemKind {
    UserMessage,
    AssistantDelta,
    AssistantMessage,
    ToolStarted,
    ToolOutput,
    ToolCompleted,
    ApprovalRequested,
    ApprovalResponded,
    RunStarted,
    RunCompleted,
    RunCancelled,
    RunFailed,
    Diagnostic,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
pub struct AppResponseEnvelope {
    pub api_version: ApiVersion,
    pub request_id: QueryId,
    pub responded_at: Timestamp,
    pub response: AppResponse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "typegen", derive(TS))]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AppResponse {
    Accepted {
        command_id: CommandId,
        /// RunStart 专有：该命令确定启动的 run id（并发来源各自携带自己的
        /// run id，不依赖宿主侧全局状态；其余命令为 None）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
    },
    Data(Value),
    Artifact {
        artifact_id: ArtifactId,
        byte_length: u64,
        media_type: String,
    },
    Error(ErrorContext),
}
