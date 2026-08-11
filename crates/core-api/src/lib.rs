//! CLI 与 GUI 共享的应用层协议类型。

use std::{fmt, path::Component, str::FromStr};

use agent_domain::{
    ActorId, ArtifactId, CommandId, ConnectionId, CoreInstanceId, ErrorContext, EventId,
    GuiClientId, MessageId, ModelId, PluginId, ProviderId, QueryId, RunId, SessionId, TenantId,
    Timestamp, ToolCallId, WorkspaceId,
};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;
use ts_rs::TS;

pub const API_VERSION: ApiVersion = ApiVersion { major: 1, minor: 0 };

/// 默认 legacy 身份作用域：tenant `local`、account `local/default`（ADR-033/P18）。
///
/// 未显式指定作用域的 CLI 查询与 run 归属都落在此默认作用域；非默认作用域
/// 的查询需要显式授权 grant。P14-8 引入 typed Quota 查询/视图/告警事件
/// （`AppQuery::QuotaOverview`、`AppEvent::QuotaChanged`、`AppEvent::QuotaAlert`），
/// 均为 TS 导出的 canonical 镜像，且只暴露脱敏的凭证提示。
pub const DEFAULT_QUOTA_TENANT: &str = "local";
pub const DEFAULT_QUOTA_ACCOUNT: &str = "local/default";

/// 宿主支持的完整 API 版本表（P13-10 schema 版本化）。
///
/// 同 major 内 minor 只增、已发布 minor 必须继续支持；删除或新增 major 走
/// [ADR-036](../../docs/adr/ADR-036-gui-protocol-versioning.md) 定义的废弃与删除流程。
pub const SUPPORTED_API_VERSIONS: &[ApiVersion] = &[API_VERSION];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl ApiVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// 返回下一个 minor 版本（minor 只增策略下的常规演进入口）。
    pub const fn bump_minor(self) -> Self {
        Self {
            major: self.major,
            minor: self.minor + 1,
        }
    }

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
    QuotaOverview {
        query: QuotaOverviewQuery,
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
    QuotaChanged {
        view: Box<QuotaOverviewView>,
    },
    QuotaAlert {
        alert: Box<QuotaAlert>,
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

// =========================================================================
// Quota（P14-8）：canonical 镜像 + 查询/视图/告警，全部 TS 导出且脱敏。
// =========================================================================
//
// 这些类型是 quota-service canonical 领域类型的协议镜像：core-api 不依赖
// quota-service（避免把 reqwest/scraper 拖进协议 crate），但 serde 形态保持
// 一致，app-service 在边界做 1:1 转换。视图只暴露脱敏的 `credential_hint`，
// 永不包含 secret/token/cookie。

/// Canonical 配额窗口。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindow {
    #[default]
    Overall,
    Rolling5h,
    Weekly,
    Monthly,
}

/// Canonical 配额单位。`Cost` 携带 ISO-4217 币种。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuotaUnit {
    #[default]
    Count,
    Token,
    Cost {
        currency: String,
    },
}

/// Canonical 非负度量值。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum QuotaMeasure {
    Exact(u64),
    Infinite,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub struct QuotaValues {
    pub used: QuotaMeasure,
    pub limit: QuotaMeasure,
    pub remaining: QuotaMeasure,
}

/// 可信度优先级：exact > derived > scraped；默认最低信任 scraped。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum QuotaConfidence {
    Exact,
    Derived,
    #[default]
    Scraped,
}

/// Canonical 适配器来源种类（脱敏枚举，不含任何凭证字段）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum QuotaAdapterKind {
    ApiKeyApi,
    OAuthApi,
    WebScrape,
    #[default]
    LocalLedger,
}

/// 安全的来源元数据。`endpoint` 已去除 query/fragment，永不泄漏 token。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuotaProvenanceView {
    pub adapter_kind: QuotaAdapterKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub fetched_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<Timestamp>,
    #[serde(default)]
    pub stale: bool,
}

/// 窗口重置语义：绝对 / 相对 / 未知。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuotaReset {
    Absolute {
        at: Timestamp,
        uncertain: bool,
    },
    Relative {
        after_secs: u64,
        observed_at: Timestamp,
        uncertain: bool,
    },
    #[default]
    Unknown,
}

/// Quota 查询：tenant/account 必填，其余为可选过滤维度。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuotaOverviewQuery {
    pub tenant_id: TenantId,
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<ProviderId>,
    /// 凭证元数据 ID（opaque，绝非凭证值）；视图输出时脱敏。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<ModelId>,
    /// 空表 = 默认所有支持的窗口。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<QuotaWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<QuotaUnit>,
}

impl QuotaOverviewQuery {
    /// 默认 legacy 作用域（local / local/default），无任何过滤维度。
    pub fn default_local() -> Self {
        Self {
            tenant_id: TenantId::new(DEFAULT_QUOTA_TENANT),
            account_id: DEFAULT_QUOTA_ACCOUNT.to_string(),
            provider_id: None,
            credential_id: None,
            model_id: None,
            windows: Vec::new(),
            unit: None,
        }
    }

    /// 是否落在默认 legacy 作用域（local / local/default）。
    pub fn is_default_scope(&self) -> bool {
        self.tenant_id.as_str() == DEFAULT_QUOTA_TENANT && self.account_id == DEFAULT_QUOTA_ACCOUNT
    }
}

/// 作用域视图：只暴露脱敏的 `credential_hint`，永不暴露凭证原文。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuotaScopeView {
    pub tenant_id: TenantId,
    pub account_id: String,
    pub provider_id: ProviderId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<ModelId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_hint: Option<String>,
}

/// 单窗口快照（脱敏后的 canonical 镜像）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuotaSnapshotView {
    pub scope: QuotaScopeView,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
    pub values: QuotaValues,
    pub reset: QuotaReset,
    pub confidence: QuotaConfidence,
    pub provenance: QuotaProvenanceView,
    /// 该快照是否来自过期缓存兜底（fresh 抓取失败）。
    #[serde(default)]
    pub served_stale: bool,
}

/// typed 失败：适配器种类（可空）+ 错误码 + 脱敏详情。
///
/// `adapter_kind` 仅当失败确实来自某个 adapter 时为 `Some`；scope 校验、
/// 无候选、取消、内部耗尽等查询级失败为 `None`，不虚构归属。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuotaFailureView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_kind: Option<QuotaAdapterKind>,
    /// 错误短码（如 `forbidden`、`rate_limited`、`timeout`、`unsupported`）。
    pub error_code: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// 单个 (scope, window, unit) 读数结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WindowReadView {
    /// 至少一个适配器产出了可用快照（可能为过期缓存兜底）。
    Ok {
        snapshot: Box<QuotaSnapshotView>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        failures: Vec<QuotaFailureView>,
    },
    /// 所有候选适配器失败且无缓存兜底。
    Failed { failures: Vec<QuotaFailureView> },
    /// 该 (scope, window, unit) 当前无缓存数据（sync 查询只读缓存）。
    NoData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct WindowReadEntry {
    pub window: QuotaWindow,
    pub read: WindowReadView,
}

/// Quota 总览视图：每个窗口一项，附生成时刻与是否命中缓存。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuotaOverviewView {
    pub scope: QuotaScopeView,
    pub windows: Vec<WindowReadEntry>,
    pub generated_at: Timestamp,
    /// 是否来自 quota-service 缓存（false = 当前无缓存，全是 NoData）。
    #[serde(default)]
    pub from_cache: bool,
}

/// 稳定告警种类：与 quota-service `refresh::AlertKind` 1:1 镜像，serde
/// 形态冻结（snake_case）。消费端按 kind 派生可执行动作与文案，不解析
/// 自由文本 `message`；`Threshold` 的 advisory 语义由
/// [`QuotaAlertSeverity`] 区分（Warning = advisory 估算，Critical = 真实触限）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum QuotaAlertKind {
    /// 剩余额度跌破配置阈值（advisory 时为抓取/估算数据，非硬停）。
    Threshold,
    /// 此前触发的 Threshold 已恢复。
    Recovered,
    /// 新鲜抓取失败，读取以过期缓存兜底。
    Stale,
    /// 凭证无效/被吊销，需要用户重新授权。
    ReauthorizationRequired,
    /// 部分适配器失败，但仍有其他适配器产出快照。
    PartialFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum QuotaAlertSeverity {
    Info,
    Warning,
    Critical,
}

/// 额度告警（安全 typed 视图，仅含脱敏字段）。
///
/// `source` 是已脱敏的来源标签（adapter kind + 短来源名），不携带端点
/// query/fragment 或 secret/token/cookie 原文；`kind` 是稳定种类，动作由
/// 消费端派生。二者均为 `Option`：`kind`/`source` 是后加的持久化字段，
/// 旧事件 JSON 缺省时可解码为 `None`（重放兼容），新事件总是 `Some`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuotaAlert {
    pub tenant_id: TenantId,
    pub account_id: String,
    pub provider_id: ProviderId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<ModelId>,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<QuotaAlertKind>,
    pub severity: QuotaAlertSeverity,
    /// 脱敏来源标签（adapter kind + 短来源名），永不包含 query/fragment
    /// 或 secret/token/cookie 原文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<QuotaSnapshotView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_hint: Option<String>,
}

/// 把凭证元数据 ID 脱敏为安全提示：保留首尾各 2 字符，中间以 `*` 替代；
/// 过短或空值返回 `None`。永不包含 secret/token/cookie 原文。
pub fn mask_credential_hint(id: &str) -> Option<String> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return None;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 4 {
        return Some("*".repeat(chars.len()));
    }
    let head: String = chars.iter().take(2).collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    Some(format!("{head}{}{tail}", "*".repeat(chars.len() - 4)))
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

    #[test]
    fn version_helpers_and_supported_table_are_consistent() {
        assert_eq!(ApiVersion::new(1, 0), API_VERSION);
        assert_eq!(ApiVersion::new(1, 0).bump_minor(), ApiVersion::new(1, 1));
        assert_eq!(
            ApiVersion::new(1, 0).bump_minor().bump_minor(),
            ApiVersion::new(1, 2)
        );
        assert!(SUPPORTED_API_VERSIONS.contains(&API_VERSION));
        assert!(SUPPORTED_API_VERSIONS
            .iter()
            .all(|version| version.major == API_VERSION.major));
    }
}
#[test]
fn mask_credential_hint_never_leaks_full_id() {
    assert_eq!(
        mask_credential_hint("sk-secret-token-1234").as_deref(),
        Some("sk****************34")
    );
    assert_eq!(mask_credential_hint("abcd").as_deref(), Some("****"));
    assert_eq!(mask_credential_hint("ab").as_deref(), Some("**"));
    assert_eq!(mask_credential_hint(""), None);
    assert_eq!(mask_credential_hint("   "), None);
    let masked = mask_credential_hint("credential-abcde-xyz").unwrap();
    assert!(!masked.contains("abcde"));
    assert!(!masked.contains("xyz"));
}

#[test]
fn quota_overview_query_default_local_matches_legacy_scope() {
    let query = QuotaOverviewQuery::default_local();
    assert!(query.is_default_scope());
    assert_eq!(query.tenant_id.as_str(), DEFAULT_QUOTA_TENANT);
    assert_eq!(query.account_id, DEFAULT_QUOTA_ACCOUNT);
    let other = QuotaOverviewQuery {
        tenant_id: TenantId::new("remote"),
        account_id: "remote/acc".into(),
        ..query.clone()
    };
    assert!(!other.is_default_scope());
}

#[test]
fn quota_overview_view_round_trip_carries_no_secret() {
    let view = QuotaOverviewView {
        scope: QuotaScopeView {
            tenant_id: TenantId::new("local"),
            account_id: "local/default".into(),
            provider_id: ProviderId::from("anthropic"),
            model_id: Some(ModelId::from("claude")),
            credential_hint: mask_credential_hint("sk-secret-key-9999"),
        },
        windows: vec![WindowReadEntry {
            window: QuotaWindow::Monthly,
            read: WindowReadView::Ok {
                snapshot: Box::new(QuotaSnapshotView {
                    scope: QuotaScopeView {
                        tenant_id: TenantId::new("local"),
                        account_id: "local/default".into(),
                        provider_id: ProviderId::from("anthropic"),
                        model_id: None,
                        credential_hint: None,
                    },
                    window: QuotaWindow::Monthly,
                    unit: QuotaUnit::Token,
                    values: QuotaValues {
                        used: QuotaMeasure::Exact(25),
                        limit: QuotaMeasure::Exact(100),
                        remaining: QuotaMeasure::Exact(75),
                    },
                    reset: QuotaReset::Unknown,
                    confidence: QuotaConfidence::Exact,
                    provenance: QuotaProvenanceView {
                        adapter_kind: QuotaAdapterKind::ApiKeyApi,
                        source: "anthropic.admin".into(),
                        endpoint: None,
                        fetched_at: Timestamp::from_unix_millis(1),
                        observed_at: None,
                        stale: false,
                    },
                    served_stale: false,
                }),
                failures: Vec::new(),
            },
        }],
        generated_at: Timestamp::from_unix_millis(1),
        from_cache: true,
    };
    let json = serde_json::to_string(&view).expect("serialize view");
    assert!(
        !json.contains("sk-secret-key-9999"),
        "leaked secret: {json}"
    );
    assert!(
        json.contains("sk**************99"),
        "masked hint missing: {json}"
    );
    let decoded: QuotaOverviewView = serde_json::from_str(&json).expect("deserialize view");
    assert_eq!(decoded, view);
}

#[test]
fn quota_alert_round_trip_is_safe() {
    let alert = QuotaAlert {
        tenant_id: TenantId::new("local"),
        account_id: "local/default".into(),
        provider_id: ProviderId::from("openai"),
        model_id: None,
        window: QuotaWindow::Monthly,
        unit: QuotaUnit::Token,
        kind: Some(QuotaAlertKind::ReauthorizationRequired),
        severity: QuotaAlertSeverity::Warning,
        source: Some("ApiKeyApi:api.openai.com/v1/organization/usage".into()),
        message: "low balance".into(),
        snapshot: None,
        credential_hint: mask_credential_hint("sk-leak"),
    };
    let json = serde_json::to_string(&alert).expect("serialize alert");
    assert!(!json.contains("sk-leak"));
    assert!(
        json.contains("\"kind\":\"reauthorization_required\""),
        "kind 必须按冻结的 snake_case 形态序列化: {json}"
    );
    assert!(
        json.contains("\"source\":\"ApiKeyApi:api.openai.com/v1/organization/usage\""),
        "source 原样往返: {json}"
    );
    let decoded: QuotaAlert = serde_json::from_str(&json).expect("deserialize alert");
    assert_eq!(decoded, alert);
}

#[test]
fn quota_alert_legacy_json_without_kind_source_decodes_to_none() {
    // kind/source 是后加的持久化字段：旧事件 JSON 缺少二者时必须可解码
    // （重放兼容），得到 None；其余字段原样保留。
    let alert = QuotaAlert {
        tenant_id: TenantId::new("local"),
        account_id: "local/default".into(),
        provider_id: ProviderId::from("openai"),
        model_id: None,
        window: QuotaWindow::Monthly,
        unit: QuotaUnit::Token,
        kind: Some(QuotaAlertKind::Threshold),
        severity: QuotaAlertSeverity::Warning,
        source: Some("ApiKeyApi:api.openai.com/v1/usage".into()),
        message: "low balance".into(),
        snapshot: None,
        credential_hint: None,
    };
    let mut json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&alert).expect("serialize")).expect("json");
    for key in ["kind", "source"] {
        assert!(
            json.get(key).is_some(),
            "precondition: new events serialize {key}"
        );
        json.as_object_mut().expect("object").remove(key);
    }
    let decoded: QuotaAlert =
        serde_json::from_value(json).expect("legacy JSON without kind/source must decode");
    assert_eq!(decoded.kind, None);
    assert_eq!(decoded.source, None);
    assert_eq!(decoded.severity, QuotaAlertSeverity::Warning);
    assert_eq!(decoded.message, "low balance");
    assert_eq!(decoded.window, QuotaWindow::Monthly);
}

#[test]
fn quota_alert_kind_serde_is_stable_and_exhaustive() {
    // 冻结的线上形态：kind 必须与 quota-service refresh::AlertKind 的
    // snake_case 序列化一致，消费端依赖该字符串做映射，不可漂移。
    let wire = [
        (QuotaAlertKind::Threshold, "threshold"),
        (QuotaAlertKind::Recovered, "recovered"),
        (QuotaAlertKind::Stale, "stale"),
        (
            QuotaAlertKind::ReauthorizationRequired,
            "reauthorization_required",
        ),
        (QuotaAlertKind::PartialFailure, "partial_failure"),
    ];
    for (kind, expected) in wire {
        let json = serde_json::to_string(&kind).expect("serialize kind");
        assert_eq!(json, format!("\"{expected}\""));
        let decoded: QuotaAlertKind = serde_json::from_str(&json).expect("deserialize kind");
        assert_eq!(decoded, kind);
    }
}
