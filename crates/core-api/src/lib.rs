//! CLI 与 GUI 共享的应用层协议类型。

use std::{fmt, path::Component, str::FromStr};

use agent_domain::{
    AccountId, ActorId, AgentId, ArtifactId, CheckpointId, CommandId, ConnectionId, CoreInstanceId,
    ErrorContext, EventId, GuiClientId, MessageId, ModelId, PlanId, PlanVersionId, PluginId,
    ProviderId, QueryId, RunId, SessionId, TenantId, Timestamp, ToolCallId, WorkspaceId,
};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use thiserror::Error;
use ts_rs::TS;

pub const API_VERSION: ApiVersion = ApiVersion { major: 1, minor: 0 };

/// 默认 legacy Quota 身份作用域：tenant `local`、account `local/default`。
///
/// 未显式指定作用域的 CLI 查询与 run 归属都落在此默认作用域；非默认作用域
/// 的查询需要显式授权 grant。P14-8 引入 typed Quota 查询/视图/告警事件
/// （`AppQuery::QuotaOverview`、`AppEvent::QuotaChanged`、`AppEvent::QuotaAlert`），
/// 均为 TS 导出的 canonical 镜像，且只暴露脱敏的凭证提示。
/// Control Plane 的 legacy tenant 由 [`DEFAULT_CONTROL_PLANE_TENANT`] 独立冻结为
/// `local/default`，不复用此 Quota 常量。
pub const DEFAULT_QUOTA_TENANT: &str = "local";
pub const DEFAULT_QUOTA_ACCOUNT: &str = "local/default";

/// Canonical 身份 tenant（P18-2）：IdentityContext 归一后的本地用户租户为
/// `local/default`，与 legacy Quota 哨兵 [`DEFAULT_QUOTA_TENANT`]（`local`）
/// 显式映射为同一默认作用域。查询/授权判定同时接受两种写法，避免
/// `pawork usage --tenant local/default` 被误判为非默认作用域而拒绝。
pub const DEFAULT_QUOTA_TENANT_CANONICAL: &str = "local/default";

/// 宿主支持的完整 API 版本表（P13-10 schema 版本化）。
///
/// 同 major 内 minor 只增、已发布 minor 必须继续支持；删除或新增 major 走
/// [ADR-036](../../docs/adr/ADR-036-gui-protocol-versioning.md) 定义的废弃与删除流程。
pub const SUPPORTED_API_VERSIONS: &[ApiVersion] = &[API_VERSION];

/// IDE/Host 上下文快照的资源上限。该数据来自外部客户端，Core 必须在存储和
/// 注入模型请求前 fail-closed，避免诊断风暴或超长 URI/消息放大内存与 prompt。
pub const MAX_CLIENT_CONTEXT_BYTES: usize = 1024 * 1024;
pub const MAX_CLIENT_CONTEXT_DOCUMENTS: usize = 128;
pub const MAX_CLIENT_CONTEXT_DIAGNOSTICS: usize = 1024;
pub const MAX_CLIENT_CONTEXT_URI_BYTES: usize = 4096;
pub const MAX_CLIENT_CONTEXT_MESSAGE_BYTES: usize = 4096;

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

/// Host 观察到的文本位置；采用 LSP 的 zero-based line/character 语义，但不
/// 依赖任何 IDE/LSP crate，保持 Core canonical domain 中立。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClientTextPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClientTextRange {
    pub start: ClientTextPosition,
    pub end: ClientTextPosition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ClientDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// 单个打开文档的有限元数据。刻意不携带正文，只保留上下文定位和字节数提示，
/// 避免 IDE 通道变成绕过 Workspace/Policy 的文件读取入口。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClientDocumentContext {
    pub uri: String,
    pub language_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<ClientTextRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_range: Option<ClientTextRange>,
    pub saved_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_bytes: Option<u64>,
}

/// IDE/LSP 展示的诊断快照。`message` 是不可信观察数据，不具备指令权限。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClientDiagnostic {
    pub document_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    pub range: ClientTextRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<ClientDiagnosticSeverity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
}

/// 外部 Host 对一个 Core session 的全量、单调版本化上下文快照。
///
/// 替换语义使断线重连可直接重放最新状态，不需要累积不可恢复的增量日志。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClientContextSnapshot {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_document: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_documents: Vec<ClientDocumentContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<ClientDiagnostic>,
}

impl ClientContextSnapshot {
    /// 在 canonical 边界执行有界校验。错误文本只描述字段与预算，不回显外部
    /// 内容，避免诊断消息或 URI 泄漏到日志/协议错误。
    pub fn validate(&self) -> Result<(), String> {
        if self.revision == 0 {
            return Err("revision must be greater than zero".into());
        }
        if self.open_documents.len() > MAX_CLIENT_CONTEXT_DOCUMENTS {
            return Err(format!(
                "open document count exceeds {MAX_CLIENT_CONTEXT_DOCUMENTS}"
            ));
        }
        if self.diagnostics.len() > MAX_CLIENT_CONTEXT_DIAGNOSTICS {
            return Err(format!(
                "diagnostic count exceeds {MAX_CLIENT_CONTEXT_DIAGNOSTICS}"
            ));
        }
        for document in &self.open_documents {
            validate_client_uri(&document.uri)?;
            if document.language_id.is_empty() || document.language_id.len() > 128 {
                return Err("language_id must contain 1..=128 bytes".into());
            }
            validate_client_range(document.selection)?;
            validate_client_range(document.visible_range)?;
        }
        if let Some(active) = self.active_document.as_deref() {
            validate_client_uri(active)?;
            if !self
                .open_documents
                .iter()
                .any(|document| document.uri == active)
            {
                return Err("active_document must name an open document".into());
            }
        }
        for diagnostic in &self.diagnostics {
            validate_client_uri(&diagnostic.document_uri)?;
            validate_client_range(Some(diagnostic.range))?;
            if diagnostic.message.len() > MAX_CLIENT_CONTEXT_MESSAGE_BYTES {
                return Err(format!(
                    "diagnostic message exceeds {MAX_CLIENT_CONTEXT_MESSAGE_BYTES} bytes"
                ));
            }
            for (name, value) in [
                ("diagnostic code", diagnostic.code.as_deref()),
                ("diagnostic source", diagnostic.source.as_deref()),
            ] {
                if value.is_some_and(|value| value.len() > 256) {
                    return Err(format!("{name} exceeds 256 bytes"));
                }
            }
        }
        let encoded = serde_json::to_vec(self)
            .map_err(|_| "client context could not be encoded".to_string())?;
        if encoded.len() > MAX_CLIENT_CONTEXT_BYTES {
            return Err(format!(
                "client context exceeds {MAX_CLIENT_CONTEXT_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

fn validate_client_uri(uri: &str) -> Result<(), String> {
    if uri.is_empty() || uri.len() > MAX_CLIENT_CONTEXT_URI_BYTES {
        return Err(format!(
            "document URI must contain 1..={MAX_CLIENT_CONTEXT_URI_BYTES} bytes"
        ));
    }
    // P17-9 审查阻塞：低信任 URI 必须携带安全 scheme——禁止无 scheme、
    // 畸形 scheme 或可执行脚本 scheme（javascript/data/vbscript），避免
    // observation 通道里的 URI 被误解为可执行/可加载内容。
    let scheme = uri.split(':').next().unwrap_or("");
    let valid_scheme = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid_scheme {
        return Err("document URI must begin with a valid scheme".into());
    }
    if matches!(
        scheme.to_ascii_lowercase().as_str(),
        "javascript" | "data" | "vbscript"
    ) {
        return Err("document URI scheme is not allowed".into());
    }
    Ok(())
}

fn validate_client_range(range: Option<ClientTextRange>) -> Result<(), String> {
    if let Some(range) = range {
        let start = (range.start.line, range.start.character);
        let end = (range.end.line, range.end.character);
        if start > end {
            return Err("text range start must not follow end".into());
        }
    }
    Ok(())
}

impl ActorIdentity {
    /// 映射为 canonical 主体键（P18-2 身份传播）。
    ///
    /// 供 `app-service` 的身份解析器（`tenant-service::IdentityResolver`）消费：
    /// 本地用户 / 已认证客户端 / 自动化 / 插件 / MCP 服务器均能映射出非空主体键；
    /// `System` 显式映射为 `local/system`。任何携带空白 payload 的身份返回
    /// `None`，解析层据此 fail-closed 拒绝，而不是静默落入默认身份。
    pub fn canonical_principal(&self) -> Option<String> {
        match self {
            ActorIdentity::LocalUser { actor_id, .. } if !actor_id.as_str().trim().is_empty() => {
                Some(DEFAULT_CONTROL_PLANE_PRINCIPAL.to_string())
            }
            ActorIdentity::AuthenticatedClient { subject, .. } if !subject.trim().is_empty() => {
                Some(format!("authenticated_client:{}", subject.trim()))
            }
            ActorIdentity::Automation { name } if !name.trim().is_empty() => {
                Some(format!("automation:{}", name.trim()))
            }
            ActorIdentity::Plugin { plugin_id } if !plugin_id.as_str().trim().is_empty() => {
                Some(format!("plugin:{}", plugin_id.as_str().trim()))
            }
            ActorIdentity::McpServer { server_id } if !server_id.trim().is_empty() => {
                Some(format!("mcp_server:{}", server_id.trim()))
            }
            ActorIdentity::System => Some("local/system".to_string()),
            _ => None,
        }
    }
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
    /// Host（IDE/ACP 等）观察到的 session 上下文全量替换。内容是有界的
    /// 不可信数据；Core 仅将它作为 Agent observation，不授予工具或写权限。
    SessionClientContextReplace {
        session_id: SessionId,
        snapshot: ClientContextSnapshot,
    },
    RunStart {
        session_id: SessionId,
        user_message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<ModelId>,
        /// P17-5：可选 Agent Profile v2 名称。命中生产 `ResourceBundle.profiles_v2`
        /// 时其不可变配置（prompt / canonical effort / tools / max_turns /
        /// background / isolation / memory）成为该 run 的权威来源；未知 /
        /// 跨 workspace / 引用不可用为结构化 fail-closed RunStart 错误。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
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
    /// Team 协作 canonical 事件（P17-6）：typed 镜像，与 `teams::TeamEvent`
    /// serde 形态 1:1；`app-service` 在边界做 1:1 转换后经唯一 EventHub 派发，
    /// 可崩溃重放（重放源为 team 事件流，本事件仅为对外镜像）。
    TeamEvent {
        event: Box<TeamEvent>,
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

    /// 是否落在默认 legacy 作用域：tenant 接受 legacy 哨兵 `local` 或
    /// canonical 身份租户 `local/default`（[`DEFAULT_QUOTA_TENANT_CANONICAL`]，
    /// 显式映射，不静默改写），account 必须为 `local/default`。
    pub fn is_default_scope(&self) -> bool {
        let tenant_is_default = self.tenant_id.as_str() == DEFAULT_QUOTA_TENANT
            || self.tenant_id.as_str() == DEFAULT_QUOTA_TENANT_CANONICAL;
        tenant_is_default && self.account_id == DEFAULT_QUOTA_ACCOUNT
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TeamMemberRole {
    /// 团队根（P12 parent）：可审批 plan、增删成员、解散 team。
    Supervisor,
    /// 普通成员（P12 worker）：可认领 task、收发 mailbox、发起受控 peer 消息。
    Worker,
}

/// 任务状态镜像（与 `orchestration::TaskState` 1:1，复用 P12 状态机）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
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

/// Plan 步骤状态镜像（与 `agent_domain::PlanStepStatus` 1:1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TeamPlanStepStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Blocked,
}

/// Plan 步骤快照镜像（与 `agent_domain::PlanStepSnapshot` 1:1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TeamPlanStepSnapshot {
    pub step_id: String,
    pub text: String,
    pub status: TeamPlanStepStatus,
}

/// Plan 行锚点镜像（与 `agent_domain::PlanCommentAnchor` 1:1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TeamPresence {
    #[default]
    Online,
    Busy,
    Idle,
    Offline,
}

/// mailbox 投递范围镜像（与 `teams::Recipients` 1:1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TeamRecipients {
    /// 点对点：精确的成员列表。
    Direct { members: Vec<AgentId> },
    /// 广播：除发送者外的全部成员。
    Broadcast,
}

/// 共享任务板条目镜像（与 `teams::BoardTask` 1:1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
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

// =========================================================================
// Tenant Policy / RBAC（P18-9）：protocol 镜像 + 脱敏决策事件视图。
// =========================================================================
//
// 这些类型是 tenant-service PolicySet / PrincipalRole / PolicyDecisionEvent
// 的协议镜像：core-api 不依赖 tenant-service，但 serde 形态保持一致，
// app-service 在边界做 1:1 转换。视图永不包含 Secret；决策 reason 在
// tenant-service 构造时已完成脱敏，此处只透传。

/// 主体最小角色（与 tenant-service `PrincipalRole` 对齐）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalRole {
    /// 租户管理员：全部权限，含 audit 导出与策略管理。
    Admin,
    /// 普通用户：操作与读自己的 session / usage / audit。
    #[default]
    User,
    /// 服务账号：执行与 usage 对账，不读内容与 audit。
    Service,
    /// 只读观察者：只读自己的 session / usage。
    Viewer,
}

/// 策略闸口（与 tenant-service `PolicyGate` 对齐）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PolicyGate {
    /// route candidate 过滤。
    RouteCandidate,
    /// credential lease 申请。
    LeaseAcquire,
    /// Agent spawn 准入。
    AgentSpawn,
    /// 请求并发准入。
    RequestAdmission,
    /// Session 查询。
    SessionQuery,
    /// Usage 查询。
    UsageQuery,
    /// Audit 查询。
    AuditQuery,
    /// Audit 导出。
    AuditExport,
    /// Retention（保留期）判定。
    Retention,
}

/// 决策种类：allow / deny / limit / fallback。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecisionKind {
    #[default]
    Allow,
    Deny,
    Limit,
    Fallback,
}

/// Audit 导出策略视图（deny-first：未启用一律拒绝）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AuditExportPolicyView {
    /// 是否启用导出（默认关闭）。
    #[serde(default)]
    pub enabled: bool,
    /// 允许的导出目标（空列表 = 无任何目标可导出）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_destinations: Vec<String>,
}

/// 单条 principal → role 绑定（TS 友好的 Vec 形态，替代 map）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PrincipalRoleBinding {
    pub principal_id: String,
    pub role: PrincipalRole,
}

/// 权限配置视图：默认角色 + 按 principal 覆盖。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PermissionProfileView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_role: Option<PrincipalRole>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub principal_roles: Vec<PrincipalRoleBinding>,
}

/// 租户策略视图（deny-first PolicySet 镜像）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TenantPolicyView {
    pub tenant_id: TenantId,
    /// 策略版本（每次更新递增；未知租户为 0）。
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_agents: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_input_token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_output_token_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_cost_micros_budget: Option<u64>,
    /// Provider 白名单；`None` 表示不限制，`Some([])` 表示拒绝全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_providers: Option<Vec<ProviderId>>,
    /// 模型白名单；`None` 表示不限制，`Some([])` 表示拒绝全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_models: Option<Vec<ModelId>>,
    /// 账号白名单；`None` 表示不限制，`Some([])` 表示拒绝全部。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_accounts: Option<Vec<AccountId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<PermissionProfileView>,
    /// 保留天数；`None` 永久保留。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_export: Option<AuditExportPolicyView>,
}

/// 版本化、脱敏的决策事件视图（审计读取输出）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PolicyDecisionEventView {
    /// 决策发生时生效的策略版本。
    pub policy_version: u64,
    pub tenant_id: TenantId,
    pub principal_id: String,
    pub gate: PolicyGate,
    pub decision: PolicyDecisionKind,
    /// 已脱敏的原因（永不含 Secret / 控制字符）。
    pub reason: String,
    pub at_ms: u64,
}

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

    fn client_snapshot(revision: u64) -> ClientContextSnapshot {
        ClientContextSnapshot {
            revision,
            active_document: Some("file:///workspace/src/lib.rs".into()),
            open_documents: vec![ClientDocumentContext {
                uri: "file:///workspace/src/lib.rs".into(),
                language_id: "rust".into(),
                selection: Some(ClientTextRange {
                    start: ClientTextPosition {
                        line: 1,
                        character: 2,
                    },
                    end: ClientTextPosition {
                        line: 1,
                        character: 4,
                    },
                }),
                visible_range: None,
                saved_version: 3,
                text_bytes: Some(128),
            }],
            diagnostics: vec![ClientDiagnostic {
                document_uri: "file:///workspace/src/lib.rs".into(),
                version: Some(3),
                range: ClientTextRange {
                    start: ClientTextPosition {
                        line: 1,
                        character: 2,
                    },
                    end: ClientTextPosition {
                        line: 1,
                        character: 4,
                    },
                },
                severity: Some(ClientDiagnosticSeverity::Warning),
                code: Some("unused".into()),
                source: Some("rust-analyzer".into()),
                message: "unused variable".into(),
            }],
        }
    }

    #[test]
    fn client_context_round_trips_and_excludes_document_text() {
        let snapshot = client_snapshot(1);
        snapshot.validate().expect("valid bounded snapshot");
        let json = serde_json::to_string(&snapshot).expect("serialize");
        assert!(!json.contains("fn main"));
        assert_eq!(
            serde_json::from_str::<ClientContextSnapshot>(&json).expect("deserialize"),
            snapshot
        );
    }

    #[test]
    fn client_context_rejects_invalid_ranges_and_resource_overflow() {
        let mut snapshot = client_snapshot(1);
        snapshot.diagnostics[0].range.start.line = 2;
        assert!(snapshot.validate().unwrap_err().contains("range start"));

        let mut snapshot = client_snapshot(1);
        snapshot.diagnostics[0].message = "x".repeat(MAX_CLIENT_CONTEXT_MESSAGE_BYTES + 1);
        assert!(snapshot
            .validate()
            .unwrap_err()
            .contains("diagnostic message"));
    }

    #[test]
    fn client_context_rejects_unsafe_uri_schemes() {
        // P17-9：低信任 URI 必须携带安全 scheme；可执行脚本 scheme 与无 scheme 一律拒绝。
        let mut snapshot = client_snapshot(1);
        snapshot.diagnostics[0].document_uri = "javascript:alert(1)".into();
        assert!(snapshot
            .validate()
            .unwrap_err()
            .contains("scheme is not allowed"));

        let mut snapshot = client_snapshot(1);
        snapshot.diagnostics[0].document_uri = "data:text/html,<script>".into();
        assert!(snapshot
            .validate()
            .unwrap_err()
            .contains("scheme is not allowed"));

        let mut snapshot = client_snapshot(1);
        snapshot.open_documents[0].uri = "1noscheme".into();
        assert!(snapshot.validate().unwrap_err().contains("valid scheme"));

        // 安全 scheme（file/http/untitled/vscode-userdata）放行。
        let mut snapshot = client_snapshot(1);
        snapshot.open_documents[0].uri = "untitled:Untitled-1".into();
        snapshot.active_document = Some("untitled:Untitled-1".into());
        snapshot.diagnostics[0].document_uri = "untitled:Untitled-1".into();
        snapshot.validate().expect("untitled scheme is allowed");
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

    // P18-8 租户分歧：canonical 身份租户 `local/default` 必须与 legacy
    // 哨兵 `local` 映射为同一默认作用域（显式映射，不静默改写、不丢历史）。
    let canonical = QuotaOverviewQuery {
        tenant_id: TenantId::new(DEFAULT_QUOTA_TENANT_CANONICAL),
        ..query.clone()
    };
    assert!(canonical.is_default_scope());
    assert_eq!(
        canonical.tenant_id.as_str(),
        DEFAULT_QUOTA_TENANT_CANONICAL,
        "显式查询的 canonical tenant 原样保留，不做静默改写"
    );

    // canonical tenant + 非默认 account：仍不是默认作用域。
    let canonical_wrong_account = QuotaOverviewQuery {
        tenant_id: TenantId::new(DEFAULT_QUOTA_TENANT_CANONICAL),
        account_id: "other/account".into(),
        ..query.clone()
    };
    assert!(!canonical_wrong_account.is_default_scope());

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

// =========================================================================
// Control Plane 作用域（P18-1，ADR-033）：冻结 legacy 作用域与控制面 schema 版本。
// =========================================================================
//
// ADR-033 单独冻结控制面 tenant `local/default`；它与旧 Quota tenant
// `local` 不同，不得复用 `DEFAULT_QUOTA_TENANT`。account 仍与 legacy Quota
// account `local/default` 一致。所有控制面持久化实体与 canonical event 带
// schema_version（ADR-033）。

/// 控制面 schema 版本（与 `provider-control::CONTROL_PLANE_SCHEMA_VERSION` /
/// `app-database::CURRENT_CONTROL_PLANE_SCHEMA_VERSION` 对齐）。
pub const CONTROL_PLANE_SCHEMA_VERSION: u32 = 2;

/// Legacy 控制面租户（ADR-033：`local/default`）。
pub const DEFAULT_CONTROL_PLANE_TENANT: &str = "local/default";
/// Legacy 控制面账号（与 quota 默认账号一致）。
pub const DEFAULT_CONTROL_PLANE_ACCOUNT: &str = DEFAULT_QUOTA_ACCOUNT;
/// Legacy 控制面主体（ADR-033：principal `local/user`）。
pub const DEFAULT_CONTROL_PLANE_PRINCIPAL: &str = "local/user";

/// 控制面作用域：tenant / account / principal 三元组（脱敏，**无 secret 字段**）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ControlPlaneScope {
    pub tenant_id: TenantId,
    pub account_id: String,
    pub principal_id: String,
}

impl ControlPlaneScope {
    /// 默认 legacy 作用域（local/default / local/default / local/user）。
    pub fn legacy_default() -> Self {
        Self {
            tenant_id: TenantId::new(DEFAULT_CONTROL_PLANE_TENANT),
            account_id: DEFAULT_CONTROL_PLANE_ACCOUNT.to_string(),
            principal_id: DEFAULT_CONTROL_PLANE_PRINCIPAL.to_string(),
        }
    }

    /// 是否落在默认 legacy 作用域。
    pub fn is_legacy_default(&self) -> bool {
        self.tenant_id.as_str() == DEFAULT_CONTROL_PLANE_TENANT
            && self.account_id == DEFAULT_CONTROL_PLANE_ACCOUNT
            && self.principal_id == DEFAULT_CONTROL_PLANE_PRINCIPAL
    }
}

#[test]
fn control_plane_legacy_scope_is_default_and_round_trips() {
    let scope = ControlPlaneScope::legacy_default();
    assert!(scope.is_legacy_default());
    assert_eq!(scope.tenant_id.as_str(), "local/default");
    assert_eq!(scope.account_id, "local/default");
    assert_eq!(scope.principal_id, "local/user");
    assert_eq!(CONTROL_PLANE_SCHEMA_VERSION, 2);

    let json = serde_json::to_string(&scope).expect("serialize");
    let decoded: ControlPlaneScope = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded, scope);

    let other = ControlPlaneScope {
        tenant_id: TenantId::new("remote"),
        ..scope.clone()
    };
    assert!(!other.is_legacy_default());
}

#[test]
fn actor_identity_canonical_principal_maps_stable_principals() {
    let cases = [
        (
            ActorIdentity::LocalUser {
                actor_id: ActorId::from("actor-1"),
                display_name: None,
            },
            Some("local/user"),
        ),
        (
            ActorIdentity::AuthenticatedClient {
                actor_id: ActorId::from("actor-2"),
                subject: "subject-1".into(),
            },
            Some("authenticated_client:subject-1"),
        ),
        (
            ActorIdentity::Automation {
                name: "scheduler".into(),
            },
            Some("automation:scheduler"),
        ),
        (
            ActorIdentity::Plugin {
                plugin_id: PluginId::from("plugin-1"),
            },
            Some("plugin:plugin-1"),
        ),
        (
            ActorIdentity::McpServer {
                server_id: "server-1".into(),
            },
            Some("mcp_server:server-1"),
        ),
    ];
    for (identity, expected) in cases {
        assert_eq!(identity.canonical_principal().as_deref(), expected);
    }
    assert_eq!(
        ActorIdentity::System.canonical_principal().as_deref(),
        Some("local/system")
    );
    assert_eq!(
        ActorIdentity::AuthenticatedClient {
            actor_id: ActorId::from("actor"),
            subject: "   ".into(),
        }
        .canonical_principal(),
        None
    );
    assert_eq!(
        ActorIdentity::Automation { name: "\t".into() }.canonical_principal(),
        None
    );
}
