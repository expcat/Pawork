//! 最小「IDE 扩展 ↔ Adapter」契约。
//!
//! 任意 IDE 扩展实现本消息子集即可接入 Adapter，不绑定具体 IDE SDK。
//! 几何类型（`Position` / `Range` / `DiagnosticSeverity`）直接复用
//! P17-4 `lsp-runtime` 的 canonical 形态，IDE 与语言服务共用同一形状。
//!
//! 契约原则：
//! - 版本化（[`IDE_CONTRACT_SCHEMA_VERSION`]），未知能力显式拒绝或降级；
//! - Adapter 只做协议翻译，不做业务/账号决策；
//! - 请求（[`IdeRequest`]，扩展 → Adapter）与事件（[`IdeEvent`]，
//!   Adapter → 扩展）是消息子集；未列出的 Core 事件不属于本契约子集。

use agent_domain::{MessageId, ModelId, RunId, SessionId, ToolCallId, WorkspaceId};
use client_adapter_api::{ClientSessionId, ClientSessionState};
use core_api::{ApprovalDecision, RunState};
use lsp_runtime::{DiagnosticSeverity, Position, Range};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Adapter 声明的协议名（`client-adapter-api` 能力快照的 `protocol` 字段）。
pub const IDE_PROTOCOL: &str = "ide-host";

/// 扩展契约协议版本。
pub const IDE_PROTOCOL_VERSION: &str = "1";

/// 扩展契约 schema 版本。
pub const IDE_CONTRACT_SCHEMA_VERSION: u32 = 1;

/// IDE Adapter 可协商的能力。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdeCapability {
    /// 编辑器生命周期（打开/关闭/激活/选区/可见范围/保存）。
    Lifecycle,
    /// 诊断双向回灌（LSP 聚合 → IDE；IDE 变更 → canonical 记录）。
    Diagnostics,
    /// 交互桥接（run/tool/approval/diff，操作落回 `AppCommand`）。
    Interaction,
    /// 可选 LSP Server 输出（复用 P17-4 聚合结果）。
    LspOutput,
    /// 断连重连（ownership reattach）。
    Reconnect,
}

impl IdeCapability {
    pub const ALL: [IdeCapability; 5] = [
        Self::Lifecycle,
        Self::Diagnostics,
        Self::Interaction,
        Self::LspOutput,
        Self::Reconnect,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lifecycle => "lifecycle",
            Self::Diagnostics => "diagnostics",
            Self::Interaction => "interaction",
            Self::LspOutput => "lsp_output",
            Self::Reconnect => "reconnect",
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|cap| cap.as_str() == value)
    }

    /// 该能力在 SDK/Headless 通道上要求的 Host 能力（空表示纯映射能力）。
    pub fn requires_sdk(self) -> &'static [headless_json::SdkCapability] {
        use headless_json::SdkCapability as Sdk;
        match self {
            Self::Lifecycle => &[Sdk::Sessions],
            Self::Interaction => &[Sdk::Runs, Sdk::Streaming],
            Self::Diagnostics | Self::LspOutput | Self::Reconnect => &[],
        }
    }
}

/// 扩展 → Adapter 请求（最小消息子集）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum IdeRequest {
    /// 能力协商（请求必须 ⊆ 已协商能力，否则显式拒绝）。
    Hello {
        client_name: String,
        client_version: String,
        protocol_version: String,
        capabilities: Vec<IdeCapability>,
    },
    /// IDE 打开文件夹 → Core workspace。
    WorkspaceAdd {
        root_path: String,
    },
    /// 编辑器生命周期：文档打开。
    EditorDidOpen {
        document_uri: String,
        language_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// 编辑器生命周期：文档关闭。
    EditorDidClose {
        document_uri: String,
    },
    /// 编辑器生命周期：文档激活。
    EditorDidActivate {
        document_uri: String,
    },
    /// 编辑器生命周期：活动选区变化。
    EditorDidChangeSelection {
        document_uri: String,
        selection: Range,
    },
    /// 编辑器生命周期：可见范围变化。
    EditorDidChangeVisibleRange {
        document_uri: String,
        range: Range,
    },
    /// 编辑器生命周期：文档保存。
    EditorDidSave {
        document_uri: String,
    },
    SessionCreate {
        workspace_id: WorkspaceId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    SessionOpen {
        session_id: SessionId,
    },
    /// 断线后按 ownership epoch/revision 重挂既有 client session。
    SessionReattach {
        client_session_id: ClientSessionId,
        ownership_epoch: u64,
        revision: u64,
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
    RunStatus {
        run_id: RunId,
    },
    /// IDE 发起的工具调用（如 apply_patch 后的编辑）；落回 `AppCommand::RunTool`。
    RunTool {
        run_id: RunId,
        tool_name: String,
        input: Value,
    },
    ToolApprove {
        run_id: RunId,
        tool_call_id: ToolCallId,
        decision: ApprovalDecision,
    },
    DiffList {
        workspace_id: WorkspaceId,
    },
    DiffGet {
        workspace_id: WorkspaceId,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// 诊断反向回灌：IDE 显示的诊断变化 → canonical 变更记录。
    DiagnosticsPublish {
        document_uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<i64>,
        diagnostics: Vec<IdeDiagnostic>,
    },
    /// 可选 LSP Server 输出查询（经注入的 `LspResultProvider` 消费 P17-4 聚合结果）。
    LspQuery {
        query_id: String,
        query: LspQueryKind,
    },
    /// 显式断开（按 ownership 移除 registry 记录）。
    Disconnect {
        client_session_id: ClientSessionId,
        ownership_epoch: u64,
        revision: u64,
    },
}

/// 可选 LSP Server 输出查询子集（仅消费类，不含写操作）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum LspQueryKind {
    Hover { uri: String, position: Position },
    Definition { uri: String, position: Position },
    References { uri: String, position: Position },
}

/// 契约中的一条 IDE 诊断（与 P17-4 canonical `Diagnostic` 同形）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdeDiagnostic {
    pub range: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<DiagnosticSeverity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
}

/// Adapter → 扩展事件（最小消息子集）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum IdeEvent {
    /// 协商完成（连接成功时立即发出）。
    Ready {
        protocol_version: String,
        negotiated: Vec<IdeCapability>,
        instance_id: Option<String>,
    },
    WorkspaceAdded {
        workspace_id: WorkspaceId,
    },
    SessionState {
        client_session_id: ClientSessionId,
        core_session_id: SessionId,
        state: ClientSessionState,
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
    DiffResult {
        workspace_id: WorkspaceId,
        payload: Value,
    },
    DiffContent {
        workspace_id: WorkspaceId,
        path: String,
        payload: Value,
    },
    DiagnosticsChanged {
        document_uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<i64>,
        diagnostics: Vec<IdeDiagnostic>,
    },
    EditorContextChanged {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection: Option<Range>,
        open_documents: Vec<String>,
    },
    LspResult {
        query_id: String,
        result: Value,
    },
    ConnectionLost {
        reason: String,
    },
    ConnectionRestored {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance_id: Option<String>,
    },
    Error {
        code: String,
        message: String,
    },
}
