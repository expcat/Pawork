//! User Hook trigger point 词汇（P17-1 步骤 1）。
//!
//! [`TriggerPoint`] 与 P10-3 WASM lifecycle hook **共享同一组 canonical trigger
//! point 词汇**（`plugin_api::PluginLifecycleEventKind`）：重叠点经
//! [`TriggerPoint::to_lifecycle_kind`] 一一映射到 canonical kind，P17 专有点
//! （RunFailed/ToolFailed/PermissionRequest/Subagent*/Task*/PostCompact/Notification）
//! 作为扩展。二者共享词汇但走**独立 dispatcher、独立运行时、独立信任边界**，
//! 互不调用、不重复执行（见 [`crate::HookDispatcher`] 与 `hook-runtime`）。
//!
//! 触发点 → canonical `agent_events::AgentEvent` 的映射由消费层（app-service）
//! 完成；本 crate 定义词汇、负载 schema、与 P10-3 的 canonical 映射，并订阅事件。

use agent_domain::{RunId, SessionId, ToolCallId, WorkspaceId};
use plugin_api::PluginLifecycleEventKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// User hook 触发点（覆盖 Session/Run/Prompt/Tool/Permission/Subagent/Task/Compact/Notification）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPoint {
    // —— Session ——
    SessionStart,
    SessionEnd,
    // —— Run ——
    RunStarted,
    RunCompleted,
    RunFailed,
    // —— Prompt ——
    PromptAssembled,
    // —— Tool ——
    PreToolUse,
    PostToolUse,
    ToolFailed,
    // —— Permission ——
    PermissionRequest,
    // —— Subagent ——
    SubagentStart,
    SubagentStop,
    // —— Task ——
    TaskStarted,
    TaskCompleted,
    // —— Compact ——
    PreCompact,
    PostCompact,
    // —— Notification ——
    Notification,
}

impl TriggerPoint {
    /// 文档化的全部触发点（供校验 / 测试枚举完整性）。
    pub const ALL: &'static [TriggerPoint] = &[
        TriggerPoint::SessionStart,
        TriggerPoint::SessionEnd,
        TriggerPoint::RunStarted,
        TriggerPoint::RunCompleted,
        TriggerPoint::RunFailed,
        TriggerPoint::PromptAssembled,
        TriggerPoint::PreToolUse,
        TriggerPoint::PostToolUse,
        TriggerPoint::ToolFailed,
        TriggerPoint::PermissionRequest,
        TriggerPoint::SubagentStart,
        TriggerPoint::SubagentStop,
        TriggerPoint::TaskStarted,
        TriggerPoint::TaskCompleted,
        TriggerPoint::PreCompact,
        TriggerPoint::PostCompact,
        TriggerPoint::Notification,
    ];

    /// 映射到与 P10-3 共享的 canonical lifecycle 词汇
    /// （`plugin_api::PluginLifecycleEventKind`）。
    ///
    /// 重叠的 canonical 点返回 `Some`（且映射是单射，见
    /// `shared_vocabulary_maps_one_to_one_to_plugin_lifecycle_kind`）；
    /// P17 专有扩展点（无 P10-3 对应 canonical 点）返回 `None`。
    pub fn to_lifecycle_kind(&self) -> Option<PluginLifecycleEventKind> {
        match self {
            TriggerPoint::SessionStart => Some(PluginLifecycleEventKind::SessionOpen),
            TriggerPoint::SessionEnd => Some(PluginLifecycleEventKind::SessionClose),
            TriggerPoint::RunStarted => Some(PluginLifecycleEventKind::RunStart),
            TriggerPoint::RunCompleted => Some(PluginLifecycleEventKind::RunEnd),
            TriggerPoint::PromptAssembled => Some(PluginLifecycleEventKind::ContextBuild),
            TriggerPoint::PreToolUse => Some(PluginLifecycleEventKind::ToolCall),
            TriggerPoint::PostToolUse => Some(PluginLifecycleEventKind::ToolResult),
            TriggerPoint::PreCompact => Some(PluginLifecycleEventKind::Compaction),
            // P17 专有扩展（P10-3 无对应 canonical lifecycle 点）。
            TriggerPoint::RunFailed
            | TriggerPoint::ToolFailed
            | TriggerPoint::PermissionRequest
            | TriggerPoint::SubagentStart
            | TriggerPoint::SubagentStop
            | TriggerPoint::TaskStarted
            | TriggerPoint::TaskCompleted
            | TriggerPoint::PostCompact
            | TriggerPoint::Notification => None,
        }
    }
}

/// 一次触发携带的上下文负载。字段按触发点选择性填充；未涉及字段为 `None`。
///
/// 所有字段均为 canonical 引用（不携带 Secret、不携带 Provider 名称）。
/// 额外的事件特定数据放在 `details`（已 redaction）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<ToolCallId>,
    /// 触发时的 prompt 文本（仅 `PromptAssembled` 等含 prompt 的触发点）。
    /// dispatcher 在派发前已对其中 secret 做 redaction。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// PromptTransform 的目标级原文快照。`prompt` 仍是供 Eval 使用的完整
    /// canonical prompt；transform 必须只基于自身 target 的原文计算，禁止把
    /// 完整 prompt 再写入单个 System/User/Injected 目标。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub injected_prompt: Option<String>,
    /// 触发特定的附加数据（命令、URL、tool 名等；已 redaction）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl TriggerPayload {
    pub fn builder() -> TriggerPayloadBuilder {
        TriggerPayloadBuilder::default()
    }
}

/// [`TriggerPayload`] 的构建器。
#[derive(Default, Debug, Clone)]
pub struct TriggerPayloadBuilder {
    inner: TriggerPayload,
}

impl TriggerPayloadBuilder {
    pub fn workspace_id(mut self, id: WorkspaceId) -> Self {
        self.inner.workspace_id = Some(id);
        self
    }
    pub fn session_id(mut self, id: SessionId) -> Self {
        self.inner.session_id = Some(id);
        self
    }
    pub fn run_id(mut self, id: RunId) -> Self {
        self.inner.run_id = Some(id);
        self
    }
    pub fn tool_call_id(mut self, id: ToolCallId) -> Self {
        self.inner.tool_call_id = Some(id);
        self
    }
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.inner.prompt = Some(prompt.into());
        self
    }
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.inner.system_prompt = Some(prompt.into());
        self
    }
    pub fn user_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.inner.user_prompt = Some(prompt.into());
        self
    }
    pub fn injected_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.inner.injected_prompt = Some(prompt.into());
        self
    }
    pub fn details(mut self, details: Value) -> Self {
        self.inner.details = Some(details);
        self
    }
    pub fn build(self) -> TriggerPayload {
        self.inner
    }
}
