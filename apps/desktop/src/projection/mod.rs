//! Desktop 渲染适配投影：把 pawork-client 的 Snapshot / TimelinePage / AppEvent
//! 投影为 Desktop UI 可直接渲染的状态。
//!
//! 本模块不依赖 gpui / tokio / OS API（gui-design 四层约束）。时间线语义
//! （去重 / 有序插入 / assistant 合并 / tool 双键锚点 / resume 基线）委托
//! pawork-protocol::projection 的单一 reducer（R3 波 C，CR08-08 根治）；
//! 本文件只保留 UI 态装配（连接 / 快照合并 / live 事件）与子模块 re-export。

mod session;
mod settings;
mod terminal;
mod timeline;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use pawork_client::projection::TimelineProjection;
use pawork_client::{
    AppEvent, AppEventEnvelope, EventStream, ResumeDisposition, ResumeOutcome, RunState, Snapshot,
};
use serde_json::Value;

pub use pawork_client::projection::{ForkBoundary, TimelineEntry, TimelineEntryKind};

pub use session::group_models_by_provider;
pub use session::{
    sessions_in_snapshot, ActiveRun, ConnectionState, DateBucket, ModelEntry, PendingApproval,
    ResumeApply, ResumeState, SessionLiveStatus, SessionSummary, TaskRailDateGroup,
    TaskRailGrouping, TaskRailProjectGroup, WorkspaceSummary, UNASSIGNED_PROJECT,
};
pub use settings::{
    parse_auth_change, ApprovalModeWire, AuthChange, AuthStartData, DefaultModelPair,
    GeneralSettingsData, PermissionsSettingsData, ProviderAuthState, ProviderAuthStatusData,
    ProviderAuthStatusEntry, ProviderCatalogState, ProviderStatusLabels, SettingsGeneralState,
    SettingsPermissionsState, SettingsProvidersState, SettingsTerminalState, TerminalSettingsData,
};
pub(crate) use terminal::TERMINAL_CWD_UNKNOWN;
pub use terminal::{TerminalAvailability, TerminalState};
pub use timeline::{run_footer_label, run_summary_texts, TimelineRow};

use session::{
    parse_active_runs, parse_pending_approvals, parse_provider_status, parse_sessions,
    parse_workspaces,
};
use terminal::parse_terminal_sessions;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopProjection {
    pub connection: ConnectionState,
    pub sessions: Vec<SessionSummary>,
    pub workspaces: Vec<WorkspaceSummary>,
    pub workspace_id: Option<String>,
    pub active_session_id: Option<String>,
    pub active_run_id: Option<String>,
    /// 时间线语义委托 protocol reducer；Deref 到 `[TimelineEntry]` 供 UI 直接迭代。
    pub timeline: TimelineProjection,
    pub pending_approval: Option<PendingApproval>,
    pub models: Vec<ModelEntry>,
    pub selected_model: Option<(String, String)>,
    pub pending_model: Option<(String, String)>,
    /// SET-3 Settings 供应商页只读状态（加载 / stale / Host 权威列表）。
    pub settings_providers: SettingsProvidersState,
    /// SET-6a Settings Network 页（Host `general_settings` / `proxy_url`）。
    pub settings_general: SettingsGeneralState,
    /// SET-6b Settings 权限与审批页（Host `permissions_settings`）。
    pub settings_permissions: SettingsPermissionsState,
    /// SET-6d Settings 终端页（Host `terminal_settings`；其生效尺寸
    /// 同时作为新建终端初始尺寸来源，ADR-050 D4）。
    pub settings_terminal: SettingsTerminalState,
    pub active_runs: Vec<ActiveRun>,
    pub active_run_started_at_ms: Option<u64>,
    pub resume: ResumeState,
    pub terminal: TerminalState,
    pub terminals: Vec<TerminalState>,
    pub(super) snapshot_pendings: Vec<PendingApproval>,
    /// R3 Wave B：Blocked 会话（live 派生）。snapshot active_runs 不提供
    /// 终态，快照重建后清空（wire 无此信息，不伪造）；Replay 重放终态
    /// 事件可重新派生。
    pub(super) blocked_sessions: BTreeSet<String>,
    /// R3 Wave B：unread 通道（独立于 SessionLiveStatus）。非 active
    /// session 的 Session-stream 活动事件记 unread；select_session 清除；
    /// 首连 / 快照重建不产生（无 last-seen 基线）。
    pub(super) unread_sessions: BTreeSet<String>,
}

impl DesktopProjection {
    /// 从 Snapshot 全量重建（首连 / 重连重取）。
    pub fn from_snapshot(snapshot: &Snapshot) -> Self {
        let mut projection = Self::default();
        projection.merge_snapshot(snapshot);
        projection
    }

    /// 用 Snapshot 的 session_tree / workspaces 段替换列表，保留连接状态、
    /// 打开的 session 与时间线。
    pub fn merge_snapshot(&mut self, snapshot: &Snapshot) {
        let mut terminal_snapshot = None;
        for section in &snapshot.sections {
            let kind = enum_name(serde_json::to_value(&section.kind).ok());
            let data = section.data.clone().unwrap_or(Value::Null);
            match kind.as_str() {
                "session_tree" => {
                    self.sessions = parse_sessions(&data);
                }
                "workspaces" => {
                    self.workspaces = parse_workspaces(&data);
                    self.workspace_id = self
                        .workspaces
                        .first()
                        .map(|workspace| workspace.id.clone());
                }
                "provider_status" => {
                    if let Some((provider, model)) = parse_provider_status(&data) {
                        self.selected_model = Some((provider, model));
                        self.pending_model = None;
                    }
                }
                "pending_tool_approvals" => {
                    self.snapshot_pendings = parse_pending_approvals(&data);
                    self.pending_approval = self.pending_for_active_session();
                }
                "active_runs" => {
                    self.active_runs = parse_active_runs(&data);
                    self.restore_active_run_from_snapshot();
                }
                "terminal_sessions" => {
                    terminal_snapshot = Some(parse_terminal_sessions(&data));
                }
                _ => {}
            }
        }
        if let Some(mut terminals) = terminal_snapshot {
            for terminal in &mut terminals {
                if let Some(old) = self
                    .terminals
                    .iter()
                    .find(|old| old.session_id == terminal.session_id)
                {
                    terminal.output = old.output.clone();
                    // Host 快照是 cwd 的权威来源；只有快照缺键（unknown）时
                    // 才沿用本地已知值，避免跨进程恢复被本地默认 "." 污染。
                    if terminal.cwd == TERMINAL_CWD_UNKNOWN {
                        terminal.cwd = old.cwd.clone();
                    }
                }
            }
            self.terminals = terminals;
            let workspace_id = self.active_workspace_id().map(str::to_string);
            self.select_terminal_for_workspace(workspace_id.as_deref());
        }
        // R3 Wave B：session 列表换新后，消失 session 的 unread 标记
        // 一并清除（仍存 session 保留——用户未看过，不伪造已读）。
        let live: BTreeSet<&str> = self
            .sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        self.unread_sessions
            .retain(|session_id| live.contains(session_id.as_str()));
        self.blocked_sessions
            .retain(|session_id| live.contains(session_id.as_str()));
        // ADR-054 D3：归档后 snapshot 不再携带该会话。merge_snapshot 是
        // rename/archive 回执与 SessionMetaChanged 刷新的共用入口；若仍
        // 保留 active_session_id，Timeline 会变成看不见入口的幽灵会话。
        if self
            .active_session_id
            .as_deref()
            .is_some_and(|session_id| !live.contains(session_id))
        {
            self.active_session_id = None;
            self.timeline.reset_baseline();
            self.active_run_id = None;
            self.active_run_started_at_ms = None;
            self.pending_approval = None;
        }
    }

    /// 重连三态：Replay 续接事件；SnapshotRequired 丢 stale 换基线；
    /// UpToDate 保留 Timeline、但仍合并握手快照里的非事件权威状态（尤其
    /// wire 无 live exit 的 terminal）。`ResumeOutcome.snapshot` 只在
    /// SnapshotRequired 时存在；其余分支使用握手首帧。
    pub fn apply_resume_outcome(
        &mut self,
        outcome: &ResumeOutcome,
        fallback_snapshot: &Snapshot,
    ) -> ResumeApply {
        self.resume = ResumeState::from_disposition(&outcome.disposition);
        // 时间线基线三态语义（Replay 保留 / SnapshotRequired 清 / UpToDate
        // 不动）在 protocol reducer 内单一实现。
        self.timeline.apply_resume_disposition(&outcome.disposition);
        match &outcome.disposition {
            ResumeDisposition::Replay { .. } => {
                self.merge_snapshot(fallback_snapshot);
                let timeline_changed = self.apply_replay(&outcome.replayed);
                ResumeApply::Continued { timeline_changed }
            }
            ResumeDisposition::SnapshotRequired { .. } => {
                let snapshot = outcome.snapshot.as_ref().unwrap_or(fallback_snapshot);
                self.apply_snapshot_required(snapshot);
                ResumeApply::ReplaceBaseline
            }
            ResumeDisposition::UpToDate { .. } => {
                self.merge_snapshot(fallback_snapshot);
                ResumeApply::Unchanged
            }
        }
    }

    /// 首连：握手 Snapshot 建基线，resume 标 Fresh。
    pub fn apply_fresh_snapshot(&mut self, snapshot: &Snapshot) {
        self.resume = ResumeState::Fresh;
        // 首连 / 无 resume 重连同样是快照重建：wire 无终态来源，blocked
        // 清空（诚实）；Replay 路径不经此函数，靠重放重新派生。
        self.blocked_sessions.clear();
        self.merge_snapshot(snapshot);
    }

    /// SnapshotRequired：丢弃 stale 权威标记，换 Snapshot，清空 Timeline。
    /// 保留 active_session_id，由 UI 重分页。
    pub fn apply_snapshot_required(&mut self, snapshot: &Snapshot) {
        self.discard_stale_authority();
        self.merge_snapshot(snapshot);
        if let Some(session_id) = self.active_session_id.clone() {
            if !self
                .sessions
                .iter()
                .any(|session| session.session_id == session_id)
            {
                self.active_session_id = None;
            } else {
                // active 仍存：保留并清其 unread（重分页后用户在看）。
                self.unread_sessions.remove(session_id.as_str());
            }
        }
        self.restore_active_run_from_snapshot();
        self.pending_approval = self.pending_for_active_session();
    }

    /// Replay：按 sequence 去重续接，不换 Timeline 基线。
    pub fn apply_replay(&mut self, events: &[AppEventEnvelope]) -> bool {
        let mut changed = false;
        for event in events {
            if self.apply_event(event) {
                changed = true;
            }
        }
        changed
    }

    fn discard_stale_authority(&mut self) {
        self.pending_approval = None;
        self.snapshot_pendings.clear();
        self.active_runs.clear();
        self.active_run_id = None;
        self.active_run_started_at_ms = None;
        self.blocked_sessions.clear();
        self.timeline.reset_baseline();
    }

    /// 应用一条 live 事件；返回时间线是否发生变化（用于 UI 自动滚底）。
    pub fn apply_event(&mut self, envelope: &AppEventEnvelope) -> bool {
        if let AppEvent::TerminalOutput {
            terminal_session_id,
            delta,
        } = &envelope.payload
        {
            return self.apply_terminal_output(terminal_session_id, delta);
        }
        if let AppEvent::TerminalExited {
            terminal_session_id,
            reason,
            ..
        } = &envelope.payload
        {
            return self.apply_terminal_exited(terminal_session_id, *reason);
        }
        // SET-4：AuthChanged 是全局供应商事件（不属时间线）。AuthChangeState
        // 未从 pawork-client re-export，经 serde Value 进入纯投影解析，
        // 畸形载荷 fail-closed。返回 false：不改时间线；Succeeded / Removed
        // 的再查询提示经 settings_providers.pending_status_refresh 传递。
        if let AppEvent::AuthChanged { provider_id, state } = &envelope.payload {
            return match serde_json::to_value(state) {
                Ok(value) => self
                    .settings_providers
                    .apply_auth_changed_value(provider_id.as_str(), &value),
                Err(_) => false,
            };
        }
        // R3 Wave A 审查修复（P1）：rail 状态点的 run 成员关系跨会话维护，
        // 必须先于 active-session 闸门——RunChanged 抵达时该会话可能并非
        // active。非终态登记（修假阴性：发消息后 rail 无蓝点）；终态按
        // run_id 移除（修假阳性：快照 active run 结束后蓝点残留）并清该
        // run 的 pendings。成员变化返回 true 触发 rail 重绘。
        let mut membership_changed = false;
        if let AppEvent::RunChanged { run_id, state } = &envelope.payload {
            if let EventStream::Session(session_id) = &envelope.stream {
                // R3 Wave B：Blocked live 派生——session 最近一条 RunChanged
                // 为终态且 state ∈ {failed, interrupted} 记 Blocked；任何
                // 其它 RunChanged（非终态，或 completed / cancelled 终态）
                // 按「最近一条」语义清除。
                let blocked_now = run_state_is_terminal(state)
                    && matches!(state, RunState::Failed | RunState::Interrupted);
                if blocked_now {
                    membership_changed |= self
                        .blocked_sessions
                        .insert(session_id.as_str().to_string());
                } else {
                    membership_changed |= self.blocked_sessions.remove(session_id.as_str());
                }
                if run_state_is_terminal(state) {
                    let before = self.active_runs.len();
                    self.active_runs.retain(|run| run.run_id != run_id.as_str());
                    // |= 而非 =：blocked 清除 / unread 等成员增量不得被
                    // active_runs 的 no-op retain 抹掉（R3 Wave B）。
                    membership_changed |= self.active_runs.len() != before;
                    self.clear_pending_for_run(Some(run_id.as_str()));
                } else if !self
                    .active_runs
                    .iter()
                    .any(|run| run.run_id == run_id.as_str())
                {
                    self.active_runs.push(ActiveRun {
                        run_id: run_id.as_str().to_string(),
                        session_id: session_id.as_str().to_string(),
                        started_at_ms: envelope.timestamp.as_unix_millis(),
                    });
                    membership_changed = true;
                }
            }
        }
        // 后台会话的 ToolApprovalRequired 同样必须过闸门前入账，否则
        // Needs input 点只会出现在当时的 active session 上。
        if let AppEvent::ToolApprovalRequired {
            run_id,
            tool_call_id,
            reason,
        } = &envelope.payload
        {
            if let EventStream::Session(session_id) = &envelope.stream {
                let pending = PendingApproval {
                    session_id: Some(session_id.as_str().to_string()),
                    run_id: run_id.as_str().to_string(),
                    tool_call_id: tool_call_id.as_str().to_string(),
                    tool_name: extract_tool_name(reason),
                    reason: reason.clone(),
                    detail: None,
                };
                self.snapshot_pendings
                    .retain(|item| item.tool_call_id != pending.tool_call_id);
                self.snapshot_pendings.push(pending.clone());
                if self.active_session_id.as_deref() == Some(session_id.as_str()) {
                    self.pending_approval = Some(pending);
                }
                membership_changed = true;
            }
        }
        if let AppEvent::ToolCompleted {
            run_id,
            tool_call_id,
            ..
        } = &envelope.payload
        {
            let before = self.snapshot_pendings.len();
            self.clear_pending_for_tool(run_id.as_str(), tool_call_id.as_str());
            if self.snapshot_pendings.len() != before {
                membership_changed = true;
            }
        }
        // R3 Wave B：unread 通道（独立于 SessionLiveStatus）——非 active 的
        // Session-stream 活动事件记 unread；select_session 清除；快照 /
        // 首连不产生（无 last-seen 基线）。MessageSent 是本地 composer
        // 回执（ControllerEvent），只属于 active session，不经 wire 抵达
        // 此处，故无对应 arm。
        if let EventStream::Session(session_id) = &envelope.stream {
            if self.active_session_id.as_deref() != Some(session_id.as_str())
                && is_session_activity_event(&envelope.payload)
            {
                membership_changed |= self.unread_sessions.insert(session_id.as_str().to_string());
            }
        }
        let Some(active) = self.active_session_id.as_deref() else {
            return membership_changed;
        };
        match &envelope.stream {
            EventStream::Session(session_id) if session_id.as_str() == active => {}
            _ => return membership_changed,
        }
        // 时间线语义（去重 / 条目 / 锚点）委托 protocol reducer。
        let timeline_changed = self.timeline.apply_event(envelope);
        // UI 态：run 跟踪、审批卡、模型切换（与时间线条目无交集）。
        match &envelope.payload {
            AppEvent::RunChanged { run_id, state } => {
                let run_id = Some(run_id.as_str().to_string());
                if run_state_is_terminal(state) {
                    if self.active_run_id.as_deref() == run_id.as_deref() {
                        self.active_run_id = None;
                        self.active_run_started_at_ms = None;
                    }
                    self.clear_pending_for_run(run_id.as_deref());
                } else {
                    self.active_run_id = run_id;
                    if self.active_run_started_at_ms.is_none() {
                        self.active_run_started_at_ms = Some(envelope.timestamp.as_unix_millis());
                    }
                }
            }
            AppEvent::Diagnostic { code, message, .. } => {
                if code == "model.switched" {
                    if let Some(confirmed) = parse_model_switch_message(message) {
                        self.selected_model = Some(confirmed);
                        self.pending_model = None;
                        return true;
                    }
                }
            }
            _ => {}
        }
        timeline_changed || membership_changed
    }

    pub fn set_models(&mut self, models: Vec<ModelEntry>) {
        self.models = models;
    }

    pub(super) fn selected_context_window(&self) -> Option<u64> {
        let (provider, id) = self.effective_model()?;
        self.models.iter().find_map(|entry| {
            if entry.provider_id == *provider && entry.id == *id {
                entry.context_window_tokens
            } else {
                None
            }
        })
    }

    pub(super) fn clear_pending_for_run(&mut self, run_id: Option<&str>) {
        self.snapshot_pendings
            .retain(|pending| run_id != Some(pending.run_id.as_str()));
        if self
            .pending_approval
            .as_ref()
            .is_some_and(|pending| run_id == Some(pending.run_id.as_str()))
        {
            self.pending_approval = None;
        }
    }

    fn clear_pending_for_tool(&mut self, run_id: &str, tool_call_id: &str) {
        self.snapshot_pendings
            .retain(|pending| !(pending.run_id == run_id && pending.tool_call_id == tool_call_id));
        if self
            .pending_approval
            .as_ref()
            .is_some_and(|pending| pending.run_id == run_id && pending.tool_call_id == tool_call_id)
        {
            self.pending_approval = None;
        }
    }
}

/// unit enum（SnapshotSectionKind / TimelineItemKind / RunState）的 serde 名。
/// serde 不是本 crate 依赖：调用点先用 serde_json::to_value 序列化（泛型约束
/// 在调用点解析，无需命名 serde trait），这里只取字符串形态。
fn enum_name(json: Option<Value>) -> String {
    json.and_then(|json| json.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn extract_tool_name(reason: &str) -> String {
    reason
        .split(" · ")
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("tool")
        .to_string()
}

/// run 终态判定（UI：清 active run 与审批卡）。时间线文案统一在 protocol
/// reducer 内（run_state_label）。
fn run_state_is_terminal(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
    )
}

/// unread 通道的 Session-stream 活动事件集合（R3 Wave B 拍板）：RunChanged /
/// AssistantDelta / ToolStarted / ToolOutput / ToolCompleted / MessageSent /
/// Diagnostic。MessageSent 为本地 ControllerEvent（composer 回执），只属于
/// active session，不经 wire 抵达 apply_event，故此处无对应 arm；
/// ToolApprovalRequired 不在拍板集合内（NeedsInput 状态点另行表达）。
fn is_session_activity_event(payload: &AppEvent) -> bool {
    matches!(
        payload,
        AppEvent::RunChanged { .. }
            | AppEvent::AssistantDelta { .. }
            | AppEvent::ToolStarted { .. }
            | AppEvent::ToolOutput { .. }
            | AppEvent::ToolCompleted { .. }
            | AppEvent::Diagnostic { .. }
    )
}

fn parse_model_switch_message(message: &str) -> Option<(String, String)> {
    let value: Value = serde_json::from_str(message).ok()?;
    let target = value.get("to").cloned().unwrap_or(value);
    let provider = target.get("provider").and_then(Value::as_str)?;
    let model = target.get("model").and_then(Value::as_str)?;
    Some((provider.to_string(), model.to_string()))
}
