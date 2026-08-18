//! 纯 Rust 状态机投影：把 pawork-client 的 Snapshot / TimelinePage / AppEvent
//! 投影为 Desktop UI 可直接渲染的状态。
//!
//! 本模块不依赖 gpui / tokio / OS API（gui-design 四层约束）。
//! 时间线去重键：live 事件的 stream_sequence 与 TimelinePage item 的 sequence
//! 同为 session 事件 sequence（gui_host publish 把 AgentEvent sequence 写入
//! stream_sequence），因此按 sequence 去重即可覆盖「分页期间 live 事件先到」
//! 的重叠（gui-design §4.1 第 3 条）。

use std::collections::BTreeSet;

use pawork_client::{
    AppEvent, AppEventEnvelope, EventStream, ResumeDisposition, ResumeOutcome, Snapshot,
    TimelinePage,
};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected { instance_id: String },
    Disconnected { reason: String },
    Failed { reason: String },
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::Connecting
    }
}

impl ConnectionState {
    /// 侧栏连接状态文本（禁用原因用文字说明，不只靠颜色区分）。
    pub fn label(&self) -> String {
        match self {
            Self::Connecting => "Connecting…".into(),
            Self::Connected { instance_id } => format!("Connected · {instance_id}"),
            Self::Disconnected { reason } => format!("Disconnected · {reason}"),
            Self::Failed { reason } => format!("Connect failed · {reason}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub updated_at_ms: u64,
    pub workspace_id: Option<String>,
    /// 日后 SessionTree 分支节点；扁平 session 数组里为 None。
    pub parent_branch_id: Option<String>,
    pub forked_from_event_id: Option<String>,
    pub active: bool,
}

/// 重连三态（gui-design §4.1 / §5）：必须在 UI 上可区分。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeState {
    Fresh,
    Replay {
        from_sequence: u64,
        through_sequence: u64,
    },
    SnapshotRequired {
        earliest_available_sequence: u64,
    },
    UpToDate {
        current_sequence: u64,
    },
}

impl Default for ResumeState {
    fn default() -> Self {
        Self::Fresh
    }
}

impl ResumeState {
    pub fn from_disposition(disposition: &ResumeDisposition) -> Self {
        match disposition {
            ResumeDisposition::Replay {
                from_sequence,
                through_sequence,
            } => Self::Replay {
                from_sequence: from_sequence.0,
                through_sequence: through_sequence.0,
            },
            ResumeDisposition::SnapshotRequired {
                earliest_available_sequence,
            } => Self::SnapshotRequired {
                earliest_available_sequence: earliest_available_sequence.0,
            },
            ResumeDisposition::UpToDate { current_sequence } => Self::UpToDate {
                current_sequence: current_sequence.0,
            },
        }
    }

    /// 侧栏 / 状态栏可见的三态文案（不只靠颜色）。
    pub fn label(&self) -> Option<String> {
        match self {
            Self::Fresh => None,
            Self::Replay {
                from_sequence,
                through_sequence,
            } => Some(format!("Replay · {from_sequence}–{through_sequence}")),
            Self::SnapshotRequired {
                earliest_available_sequence,
            } => Some(format!(
                "Snapshot required · from {earliest_available_sequence}"
            )),
            Self::UpToDate { current_sequence } => {
                Some(format!("Up to date · {current_sequence}"))
            }
        }
    }

    /// SnapshotRequired 才换基线并重分页；Replay / UpToDate 不闪全量重载。
    pub fn replaces_baseline(&self) -> bool {
        matches!(self, Self::SnapshotRequired { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeApply {
    /// 首连：用握手 Snapshot 建基线。
    Fresh,
    /// Replay：按 sequence 续接，不换 Timeline 基线。
    Continued { timeline_changed: bool },
    /// SnapshotRequired：丢 stale、换 Snapshot，调用方重分页。
    ReplaceBaseline,
    /// UpToDate：不重载 Timeline。
    Unchanged,
}

/// Inspector Terminal 面：滚动文本，不是 VT100 / 本地 PTY。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalState {
    pub session_id: Option<String>,
    pub output: String,
    pub columns: u16,
    pub rows: u16,
    /// 仅 workspace 相对路径。
    pub cwd: String,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            session_id: None,
            output: String::new(),
            columns: 80,
            rows: 24,
            cwd: ".".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSummary {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskRailGrouping {
    Timeline,
    Projects,
}

impl TaskRailGrouping {
    pub fn accessible_name(self) -> &'static str {
        match self {
            Self::Timeline => "Group tasks · Timeline",
            Self::Projects => "Group tasks · Projects",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DateBucket {
    Today,
    Yesterday,
    Previous7Days,
    Earlier,
}

impl DateBucket {
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::Previous7Days => "Previous 7 days",
            Self::Earlier => "Earlier",
        }
    }
}

pub const UNASSIGNED_PROJECT: &str = "Unassigned";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRailProjectGroup {
    pub workspace_id: Option<String>,
    pub name: String,
    pub latest_activity_ms: u64,
    pub tasks: Vec<SessionSummary>,
}

impl TaskRailProjectGroup {
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_unassigned(&self) -> bool {
        self.workspace_id.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRailDateGroup {
    pub bucket: DateBucket,
    pub projects: Vec<TaskRailProjectGroup>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineEntryKind {
    UserMessage { text: String },
    AssistantMessage { text: String },
    ToolCall { name: String, status: String, detail: Option<String> },
    RunState(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEntry {
    pub sequence: u64,
    pub event_id: String,
    pub kind: TimelineEntryKind,
    pub timestamp: String,
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub session_id: Option<String>,
    pub run_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub reason: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelEntry {
    pub provider_id: String,
    pub id: String,
    pub display_name: String,
    pub context_window_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveRun {
    pub run_id: String,
    pub session_id: String,
    pub started_at_ms: u64,
}

/// Assistant 流式合并锚点：同一 run + message 的 delta 追加到同一条目。
/// 用 event_id / sequence 回查，不存会因中间插入而失效的 index。
#[derive(Clone, Debug, PartialEq, Eq)]
struct AssistantAnchor {
    run_id: Option<String>,
    message_id: Option<String>,
    event_id: String,
    sequence: u64,
}

/// Tool 条目锚点：ToolCompleted/ToolOutput 按 run + tool_call_id（live）或
/// run + tool_name（分页历史，TimelineItem 不携带 tool_call_id）回填。
/// 用 event_id / sequence 回查，不存会因中间插入而失效的 index。
#[derive(Clone, Debug, PartialEq, Eq)]
struct ToolAnchor {
    run_id: Option<String>,
    tool_call_id: Option<String>,
    name: Option<String>,
    event_id: String,
    sequence: u64,
}

struct TimelineIdentity {
    event_id: String,
    sequence: u64,
}

/// 从 TimelinePage item 解出的字段值（TimelineItem 类型未从 pawork-client
/// re-export，这里在调用点解构为纯值，保持业务依赖只有 pawork-client）。
struct HistoryItem<'a> {
    sequence: u64,
    event_id: &'a str,
    kind: &'a str,
    run_id: Option<&'a str>,
    text: Option<&'a str>,
    tool_name: Option<&'a str>,
    status: Option<&'a str>,
    detail: Option<&'a str>,
    timestamp: &'a str,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopProjection {
    pub connection: ConnectionState,
    pub sessions: Vec<SessionSummary>,
    pub workspaces: Vec<WorkspaceSummary>,
    pub workspace_id: Option<String>,
    pub active_session_id: Option<String>,
    pub active_run_id: Option<String>,
    pub timeline: Vec<TimelineEntry>,
    pub pending_approval: Option<PendingApproval>,
    pub models: Vec<ModelEntry>,
    pub selected_model: Option<(String, String)>,
    pub pending_model: Option<(String, String)>,
    pub active_runs: Vec<ActiveRun>,
    pub active_run_started_at_ms: Option<u64>,
    pub resume: ResumeState,
    pub terminal: TerminalState,
    snapshot_pendings: Vec<PendingApproval>,
    /// 已消费的 session sequence（live 与分页共用的去重集）。
    seen: BTreeSet<u64>,
    assistant_anchor: Option<AssistantAnchor>,
    tool_anchors: Vec<ToolAnchor>,
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
        for section in &snapshot.sections {
            let kind = enum_name(serde_json::to_value(&section.kind).ok());
            let data = section.data.clone().unwrap_or(Value::Null);
            match kind.as_str() {
                "session_tree" => {
                    self.sessions = parse_sessions(&data);
                }
                "workspaces" => {
                    self.workspaces = parse_workspaces(&data);
                    self.workspace_id = self.workspaces.first().map(|workspace| workspace.id.clone());
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
                    if let Some(id) = parse_terminal_session_id(&data) {
                        self.terminal.session_id = Some(id);
                    }
                }
                _ => {}
            }
        }
    }

    /// 重连三态：Replay 续接事件；SnapshotRequired 丢 stale 换基线；
    /// UpToDate 不碰 Timeline。优先消费 `ResumeOutcome.snapshot`（服务端第二帧）；
    /// `fallback_snapshot` 只在未附带时兜底握手首帧。
    pub fn apply_resume_outcome(
        &mut self,
        outcome: &ResumeOutcome,
        fallback_snapshot: &Snapshot,
    ) -> ResumeApply {
        self.resume = ResumeState::from_disposition(&outcome.disposition);
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
            ResumeDisposition::UpToDate { .. } => ResumeApply::Unchanged,
        }
    }

    /// 首连：握手 Snapshot 建基线，resume 标 Fresh。
    pub fn apply_fresh_snapshot(&mut self, snapshot: &Snapshot) {
        self.resume = ResumeState::Fresh;
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
        self.timeline.clear();
        self.seen.clear();
        self.assistant_anchor = None;
        self.tool_anchors.clear();
    }

    pub fn apply_terminal_output(&mut self, terminal_session_id: &str, delta: &str) -> bool {
        if let Some(current) = self.terminal.session_id.as_deref() {
            if current != terminal_session_id {
                return false;
            }
        } else {
            self.terminal.session_id = Some(terminal_session_id.to_string());
        }
        self.terminal.output.push_str(delta);
        true
    }

    /// 打开（切换）session：清空时间线与去重状态。
    pub fn select_session(&mut self, session_id: &str) {
        self.active_session_id = Some(session_id.to_string());
        self.active_run_id = None;
        self.active_run_started_at_ms = None;
        self.pending_approval = None;
        self.timeline.clear();
        self.seen.clear();
        self.assistant_anchor = None;
        self.tool_anchors.clear();
        self.restore_active_run_from_snapshot();
        self.pending_approval = self.pending_for_active_session();
    }

    fn restore_active_run_from_snapshot(&mut self) {
        let Some(session_id) = self.active_session_id.as_deref() else {
            return;
        };
        if let Some(run) = self
            .active_runs
            .iter()
            .find(|run| run.session_id == session_id)
        {
            self.active_run_id = Some(run.run_id.clone());
            self.active_run_started_at_ms = Some(run.started_at_ms);
        }
    }

    fn pending_for_active_session(&self) -> Option<PendingApproval> {
        self.snapshot_pendings.iter().find(|pending| {
            pending.session_id.as_deref() == self.active_session_id.as_deref()
                || pending.session_id.is_none()
        }).cloned()
    }

    pub fn set_connection(&mut self, state: ConnectionState) {
        self.connection = state;
    }

    /// 合并一页历史时间线（按 sequence 去重，保持 sequence 升序）。
    pub fn apply_timeline_page(&mut self, page: &TimelinePage) {
        for item in &page.items {
            self.merge_history_item(HistoryItem {
                sequence: item.sequence,
                event_id: item.event_id.as_str(),
                kind: &enum_name(serde_json::to_value(&item.kind).ok()),
                run_id: item.run_id.as_deref(),
                text: item.text.as_deref(),
                tool_name: item.tool_name.as_deref(),
                status: item.status.as_deref(),
                detail: item.detail.as_deref(),
                timestamp: item.timestamp.as_str(),
            });
        }
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
        let Some(active) = self.active_session_id.as_deref() else {
            return false;
        };
        match &envelope.stream {
            EventStream::Session(session_id) if session_id.as_str() == active => {}
            _ => return false,
        }
        let sequence = envelope.stream_sequence;
        let event_id = envelope.event_id.as_str().to_string();
        let timestamp = envelope.timestamp.0.to_string();
        match &envelope.payload {
            AppEvent::RunChanged { run_id, state } => {
                let state_name = enum_name(serde_json::to_value(state).ok());
                let run_id = Some(run_id.as_str().to_string());
                if matches!(
                    state_name.as_str(),
                    "completed" | "cancelled" | "failed" | "interrupted"
                ) {
                    if self.active_run_id.as_deref() == run_id.as_deref() {
                        self.active_run_id = None;
                        self.active_run_started_at_ms = None;
                    }
                } else {
                    self.active_run_id = run_id.clone();
                    if self.active_run_started_at_ms.is_none() {
                        self.active_run_started_at_ms = timestamp.parse().ok();
                    }
                }
                if matches!(
                    state_name.as_str(),
                    "completed" | "cancelled" | "failed" | "interrupted"
                ) {
                    self.clear_pending_for_run(run_id.as_deref());
                }
                if self.seen.insert(sequence) {
                    self.push_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::RunState(format!("run {state_name}")),
                        timestamp,
                        run_id,
                    });
                    return true;
                }
            }
            AppEvent::AssistantDelta { run_id, message_id, delta } => {
                if !self.seen.insert(sequence) {
                    return false;
                }
                return self.append_assistant_delta(
                    sequence,
                    event_id,
                    timestamp,
                    run_id.as_str(),
                    Some(message_id.as_str()),
                    delta,
                );
            }
            AppEvent::ToolStarted { run_id, tool_call_id, name } => {
                if !self.seen.insert(sequence) {
                    return false;
                }
                let run = Some(run_id.as_str().to_string());
                self.insert_entry(TimelineEntry {
                    sequence,
                    event_id: event_id.clone(),
                    kind: TimelineEntryKind::ToolCall {
                        name: name.clone(),
                        status: "running".into(),
                        detail: None,
                    },
                    timestamp,
                    run_id: run.clone(),
                });
                if let Some(anchor) = self.anchor_after_insert(&event_id, sequence) {
                    self.tool_anchors.push(ToolAnchor {
                        run_id: run,
                        tool_call_id: Some(tool_call_id.as_str().to_string()),
                        name: Some(name.clone()),
                        event_id: anchor.event_id,
                        sequence: anchor.sequence,
                    });
                }
                return true;
            }
            AppEvent::ToolOutput {
                run_id,
                tool_call_id,
                delta,
                ..
            } => {
                if self.update_tool_entry(
                    Some(run_id.as_str()),
                    Some(tool_call_id.as_str()),
                    None,
                    None,
                    Some(delta),
                ) {
                    self.seen.insert(sequence);
                    return true;
                }
            }
            AppEvent::ToolCompleted { run_id, tool_call_id, success } => {
                let status = if *success { "succeeded" } else { "failed" };
                let run = run_id.as_str();
                self.clear_pending_for_tool(run, tool_call_id.as_str());
                if self.update_tool_entry(
                    Some(run),
                    Some(tool_call_id.as_str()),
                    None,
                    Some(status),
                    None,
                ) {
                    self.seen.insert(sequence);
                    return true;
                }
                if self.seen.insert(sequence) {
                    self.push_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::ToolCall {
                            name: tool_call_id.as_str().to_string(),
                            status: status.into(),
                            detail: None,
                        },
                        timestamp,
                        run_id: Some(run.to_string()),
                    });
                    return true;
                }
            }
            AppEvent::ToolApprovalRequired {
                run_id,
                tool_call_id,
                reason,
            } => {
                let pending = PendingApproval {
                    session_id: self.active_session_id.clone(),
                    run_id: run_id.as_str().to_string(),
                    tool_call_id: tool_call_id.as_str().to_string(),
                    tool_name: extract_tool_name(reason),
                    reason: reason.clone(),
                    detail: None,
                };
                self.snapshot_pendings
                    .retain(|item| item.tool_call_id != pending.tool_call_id);
                self.snapshot_pendings.push(pending.clone());
                self.pending_approval = Some(pending);
                return true;
            }
            AppEvent::Diagnostic { code, message, .. } => {
                if code == "model.switched" {
                    if let Some(confirmed) = parse_model_switch_message(message) {
                        self.selected_model = Some(confirmed);
                        self.pending_model = None;
                        return true;
                    }
                }
                if code == "sandbox.fallback" && self.seen.insert(sequence) {
                    self.push_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::RunState(sandbox_fallback_label(message)),
                        timestamp,
                        run_id: None,
                    });
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    /// 追加 assistant delta：命中锚点则合并，否则新开条目。
    fn append_assistant_delta(
        &mut self,
        sequence: u64,
        event_id: String,
        timestamp: String,
        run_id: &str,
        message_id: Option<&str>,
        delta: &str,
    ) -> bool {
        let run = Some(run_id.to_string());
        let message = message_id.map(str::to_string);
        if let Some(anchor) = &self.assistant_anchor {
            if anchor.run_id == run && anchor.message_id == message {
                if let Some(index) = self.entry_index_by_identity(&anchor.event_id, anchor.sequence)
                {
                    if let Some(TimelineEntryKind::AssistantMessage { text }) =
                        self.timeline.get_mut(index).map(|entry| &mut entry.kind)
                    {
                        text.push_str(delta);
                        return true;
                    }
                }
            }
        }
        self.insert_entry(TimelineEntry {
            sequence,
            event_id: event_id.clone(),
            kind: TimelineEntryKind::AssistantMessage {
                text: delta.to_string(),
            },
            timestamp,
            run_id: run.clone(),
        });
        if let Some(identity) = self.anchor_after_insert(&event_id, sequence) {
            self.assistant_anchor = Some(AssistantAnchor {
                run_id: run,
                message_id: message,
                event_id: identity.event_id,
                sequence: identity.sequence,
            });
        }
        true
    }

    /// 合并单条历史条目。历史中的 assistant 形状是「delta 序列 + 末尾 committed
    /// 消息」：delta 逐段合并，committed 到达时以提交文本替换累积文本，保证
    /// 历史回放不双份渲染。
    fn merge_history_item(&mut self, item: HistoryItem<'_>) {
        match item.kind {
            "user_message" => {
                if self.seen.insert(item.sequence) {
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::UserMessage {
                            text: item.text.unwrap_or_default().to_string(),
                        },
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            "assistant_delta" => {
                if self.seen.insert(item.sequence) {
                    self.append_assistant_delta(
                        item.sequence,
                        item.event_id.to_string(),
                        item.timestamp.to_string(),
                        item.run_id.unwrap_or_default(),
                        None,
                        item.text.unwrap_or_default(),
                    );
                }
            }
            "assistant_message" => {
                if !self.seen.insert(item.sequence) {
                    return;
                }
                let run = item.run_id.map(str::to_string);
                let committed = item.text.unwrap_or_default().to_string();
                if let Some(anchor) = &self.assistant_anchor {
                    if anchor.run_id == run && anchor.message_id.is_none() {
                        if let Some(index) =
                            self.entry_index_by_identity(&anchor.event_id, anchor.sequence)
                        {
                            if matches!(
                                self.timeline.get(index).map(|entry| &entry.kind),
                                Some(TimelineEntryKind::AssistantMessage { .. })
                            ) {
                                if let Some(entry) = self.timeline.get_mut(index) {
                                    entry.sequence = item.sequence;
                                    entry.event_id = item.event_id.to_string();
                                    entry.timestamp = item.timestamp.to_string();
                                    entry.kind =
                                        TimelineEntryKind::AssistantMessage { text: committed };
                                }
                                if let Some(anchor) = &mut self.assistant_anchor {
                                    anchor.event_id = item.event_id.to_string();
                                    anchor.sequence = item.sequence;
                                }
                                return;
                            }
                        }
                    }
                }
                self.insert_entry(TimelineEntry {
                    sequence: item.sequence,
                    event_id: item.event_id.to_string(),
                    kind: TimelineEntryKind::AssistantMessage { text: committed },
                    timestamp: item.timestamp.to_string(),
                    run_id: run.clone(),
                });
                if let Some(identity) =
                    self.anchor_after_insert(item.event_id, item.sequence)
                {
                    self.assistant_anchor = Some(AssistantAnchor {
                        run_id: run,
                        message_id: None,
                        event_id: identity.event_id,
                        sequence: identity.sequence,
                    });
                }
            }
            "tool_started" => {
                if !self.seen.insert(item.sequence) {
                    return;
                }
                let run = item.run_id.map(str::to_string);
                let name = item.tool_name.unwrap_or("tool").to_string();
                self.insert_entry(TimelineEntry {
                    sequence: item.sequence,
                    event_id: item.event_id.to_string(),
                    kind: TimelineEntryKind::ToolCall {
                        name: name.clone(),
                        status: item.status.unwrap_or("running").to_string(),
                        detail: None,
                    },
                    timestamp: item.timestamp.to_string(),
                    run_id: run.clone(),
                });
                if let Some(identity) =
                    self.anchor_after_insert(item.event_id, item.sequence)
                {
                    self.tool_anchors.push(ToolAnchor {
                        run_id: run,
                        tool_call_id: None,
                        name: Some(name),
                        event_id: identity.event_id,
                        sequence: identity.sequence,
                    });
                }
            }
            "tool_output" => {
                if self.seen.insert(item.sequence) {
                    self.update_tool_entry(
                        item.run_id,
                        None,
                        item.tool_name,
                        None,
                        Some(item.text.unwrap_or_default()),
                    );
                }
            }
            "tool_completed" => {
                let status = item.status.unwrap_or("succeeded");
                if matches!(status, "succeeded" | "failed" | "cancelled") {
                    self.pending_approval = None;
                }
                if self.update_tool_entry(item.run_id, None, item.tool_name, Some(status), None) {
                    self.seen.insert(item.sequence);
                } else if self.seen.insert(item.sequence) {
                    let run = item.run_id.map(str::to_string);
                    let name = item.tool_name.unwrap_or("tool").to_string();
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::ToolCall {
                            name: name.clone(),
                            status: status.to_string(),
                            detail: item.detail.map(str::to_string),
                        },
                        timestamp: item.timestamp.to_string(),
                        run_id: run.clone(),
                    });
                    if let Some(identity) =
                        self.anchor_after_insert(item.event_id, item.sequence)
                    {
                        self.tool_anchors.push(ToolAnchor {
                            run_id: run,
                            tool_call_id: None,
                            name: Some(name),
                            event_id: identity.event_id,
                            sequence: identity.sequence,
                        });
                    }
                }
            }
            "run_started" | "run_completed" | "run_cancelled" => {
                if matches!(item.kind, "run_completed" | "run_cancelled") {
                    self.clear_pending_for_run(item.run_id);
                }
                if self.seen.insert(item.sequence) {
                    let state = item.kind.trim_start_matches("run_");
                    self.push_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::RunState(format!("run {state}")),
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            "run_failed" => {
                self.clear_pending_for_run(item.run_id);
                if self.seen.insert(item.sequence) {
                    let reason = item.detail.unwrap_or_default();
                    self.push_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::RunState(format!("run failed · {reason}")),
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            "approval_requested" => {
                if self.seen.insert(item.sequence) {
                    let tool = item.tool_name.unwrap_or("tool");
                    let reason = item.text.or(item.detail).unwrap_or_default();
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::RunState(if reason.is_empty() {
                            format!("approval requested · {tool}")
                        } else {
                            format!("approval requested · {tool} · {reason}")
                        }),
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            "approval_responded" => {
                self.pending_approval = None;
                self.snapshot_pendings.clear();
                if self.seen.insert(item.sequence) {
                    let decision = item.status.or(item.detail).or(item.text).unwrap_or("responded");
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::RunState(format!("approval {decision}")),
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            "diagnostic" => {
                if self.seen.insert(item.sequence) {
                    self.push_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::Error(
                            item.detail.unwrap_or_default().to_string(),
                        ),
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            _ => {}
        }
    }

    fn push_entry(&mut self, entry: TimelineEntry) {
        self.timeline.push(entry);
    }

    /// 按 sequence 有序插入（页数据可能晚于已到达的 live 事件）。
    fn insert_entry(&mut self, entry: TimelineEntry) {
        let position = self
            .timeline
            .partition_point(|existing| existing.sequence < entry.sequence);
        self.timeline.insert(position, entry);
    }

    fn entry_index_by_identity(&self, event_id: &str, sequence: u64) -> Option<usize> {
        self.timeline
            .iter()
            .position(|entry| entry.event_id == event_id)
            .or_else(|| {
                self.timeline
                    .iter()
                    .position(|entry| entry.sequence == sequence)
            })
    }

    /// `insert_entry` 之后按 identity 回查，避免使用插入时的瞬时 index。
    fn anchor_after_insert(&self, event_id: &str, sequence: u64) -> Option<TimelineIdentity> {
        let index = self.entry_index_by_identity(event_id, sequence)?;
        let entry = self.timeline.get(index)?;
        Some(TimelineIdentity {
            event_id: entry.event_id.clone(),
            sequence: entry.sequence,
        })
    }

    /// 按 run + tool_call_id（live）或 run + tool_name（历史）回填 tool 条目。
    fn update_tool_entry(
        &mut self,
        run_id: Option<&str>,
        tool_call_id: Option<&str>,
        name: Option<&str>,
        new_status: Option<&str>,
        detail_delta: Option<&str>,
    ) -> bool {
        let run = run_id.map(str::to_string);
        let found = self.tool_anchors.iter().rev().find_map(|anchor| {
            if anchor.run_id != run {
                return None;
            }
            if let Some(expected) = tool_call_id {
                if anchor.tool_call_id.as_deref() != Some(expected) {
                    return None;
                }
            }
            if let Some(expected) = name {
                if anchor.name.as_deref() != Some(expected) {
                    return None;
                }
            }
            Some((anchor.event_id.clone(), anchor.sequence))
        });
        let Some((event_id, sequence)) = found else {
            return false;
        };
        let Some(index) = self.entry_index_by_identity(&event_id, sequence) else {
            return false;
        };
        if let Some(TimelineEntryKind::ToolCall { status, detail, .. }) =
            self.timeline.get_mut(index).map(|entry| &mut entry.kind)
        {
            if let Some(next) = new_status {
                status.clear();
                status.push_str(next);
            }
            if let Some(delta) = detail_delta {
                if !delta.is_empty() {
                    let text = detail.take().unwrap_or_default();
                    detail.replace(text + delta);
                }
            }
            return true;
        }
        false
    }

    pub fn set_models(&mut self, models: Vec<ModelEntry>) {
        self.models = models;
    }

    pub fn workspace_name(&self, workspace_id: Option<&str>) -> String {
        match workspace_id {
            None => UNASSIGNED_PROJECT.into(),
            Some(id) => self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == id)
                .map(|workspace| workspace.name.clone())
                .unwrap_or_else(|| id.to_string()),
        }
    }

    pub fn scoped_sessions(&self, scope: Option<&str>) -> Vec<&SessionSummary> {
        self.sessions
            .iter()
            .filter(|session| match scope {
                None => true,
                Some(workspace_id) => session.workspace_id.as_deref() == Some(workspace_id),
            })
            .collect()
    }

    /// Timeline：日期 → 项目 → Task。同一 session 只出现一次。
    pub fn timeline_groups(&self, scope: Option<&str>, now_ms: u64) -> Vec<TaskRailDateGroup> {
        let mut by_bucket: Vec<(DateBucket, Vec<SessionSummary>)> = Vec::new();
        for session in self.scoped_sessions(scope) {
            let bucket = date_bucket(session.updated_at_ms, now_ms);
            if let Some((_, sessions)) = by_bucket.iter_mut().find(|(item, _)| *item == bucket) {
                sessions.push(session.clone());
            } else {
                by_bucket.push((bucket, vec![session.clone()]));
            }
        }
        by_bucket.sort_by_key(|(bucket, _)| *bucket);
        by_bucket
            .into_iter()
            .map(|(bucket, sessions)| TaskRailDateGroup {
                bucket,
                projects: group_sessions_by_project(self, sessions),
            })
            .collect()
    }

    /// Projects：按 canonical workspace 分组；缺字段进 Unassigned。
    pub fn project_groups(&self, scope: Option<&str>) -> Vec<TaskRailProjectGroup> {
        group_sessions_by_project(
            self,
            self.scoped_sessions(scope)
                .into_iter()
                .cloned()
                .collect(),
        )
    }

    pub fn project_scope_options(&self) -> Vec<(Option<String>, String)> {
        let mut options = vec![(None, "All projects".into())];
        let mut seen = BTreeSet::new();
        for workspace in &self.workspaces {
            if seen.insert(workspace.id.clone()) {
                options.push((Some(workspace.id.clone()), workspace.name.clone()));
            }
        }
        for session in &self.sessions {
            if let Some(id) = &session.workspace_id {
                if seen.insert(id.clone()) {
                    options.push((Some(id.clone()), self.workspace_name(Some(id))));
                }
            }
        }
        options
    }

    pub fn set_pending_model(&mut self, provider_id: String, id: String) {
        self.pending_model = Some((provider_id, id));
    }

    pub fn effective_model(&self) -> Option<&(String, String)> {
        self.pending_model.as_ref().or(self.selected_model.as_ref())
    }

    /// ContextMeter：当前请求估算未知时显示 unavailable / `—`，只用 catalog window。
    pub fn context_meter_label(&self) -> String {
        match self.selected_context_window() {
            Some(window) => format!("Context · — / {window}"),
            None => "Context · unavailable".into(),
        }
    }

    /// RunStatusBar：缺权威来源的字段显示 `—`，不伪造 token / quota / tok/s。
    /// `now_ms` 由 UI 注入，投影层不读系统时钟。
    pub fn run_status_label(&self, now_ms: u64) -> String {
        let duration = match (self.active_run_id.as_ref(), self.active_run_started_at_ms) {
            (Some(_), Some(started_at_ms)) => format_run_duration(started_at_ms, now_ms),
            (Some(_), None) => "—".into(),
            (None, _) => "idle".into(),
        };
        format!("tokens —  ·  quota —  ·  — tok/s  ·  {duration}")
    }

    fn selected_context_window(&self) -> Option<u64> {
        let (provider, id) = self.effective_model()?;
        self.models.iter().find_map(|entry| {
            if entry.provider_id == *provider && entry.id == *id {
                entry.context_window_tokens
            } else {
                None
            }
        })
    }

    fn clear_pending_for_run(&mut self, run_id: Option<&str>) {
        self.snapshot_pendings.retain(|pending| run_id != Some(pending.run_id.as_str()));
        if self
            .pending_approval
            .as_ref()
            .is_some_and(|pending| run_id == Some(pending.run_id.as_str()))
        {
            self.pending_approval = None;
        }
    }

    fn clear_pending_for_tool(&mut self, run_id: &str, tool_call_id: &str) {
        self.snapshot_pendings.retain(|pending| {
            !(pending.run_id == run_id && pending.tool_call_id == tool_call_id)
        });
        if self.pending_approval.as_ref().is_some_and(|pending| {
            pending.run_id == run_id && pending.tool_call_id == tool_call_id
        }) {
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

fn session_tree_entries(data: &Value) -> Option<&Vec<Value>> {
    if let Some(entries) = data.as_array() {
        return Some(entries);
    }
    data.as_object().and_then(|object| {
        object
            .get("sessions")
            .or_else(|| object.get("nodes"))
            .or_else(|| object.get("branches"))
            .and_then(Value::as_array)
    })
}

fn parse_sessions(data: &Value) -> Vec<SessionSummary> {
    let mut sessions = Vec::new();
    let Some(entries) = session_tree_entries(data) else {
        return sessions;
    };
    for entry in entries {
        // 现行扁平 session 数组用 session_id；日后分支节点用 branch_id。
        let Some(session_id) = entry
            .get("session_id")
            .or_else(|| entry.get("branch_id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let title = entry
            .get("title")
            .or_else(|| entry.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Untitled");
        sessions.push(SessionSummary {
            session_id: session_id.to_string(),
            title: title.to_string(),
            updated_at_ms: entry
                .get("updated_at_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            workspace_id: entry
                .get("workspace_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
            parent_branch_id: entry
                .get("parent_branch_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            forked_from_event_id: entry
                .get("forked_from_event_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            active: entry.get("active").and_then(Value::as_bool).unwrap_or(true),
        });
    }
    sessions.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
    sessions
}

fn parse_terminal_session_id(data: &Value) -> Option<String> {
    if let Some(id) = data.get("terminal_session_id").and_then(Value::as_str) {
        return Some(id.to_string());
    }
    data.as_array().and_then(|entries| {
        entries.iter().find_map(|entry| {
            entry
                .get("terminal_session_id")
                .or_else(|| entry.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    })
}

fn parse_workspaces(data: &Value) -> Vec<WorkspaceSummary> {
    let Some(entries) = data.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str)?;
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(id);
            Some(WorkspaceSummary {
                id: id.to_string(),
                name: name.to_string(),
            })
        })
        .collect()
}

fn date_bucket(updated_at_ms: u64, now_ms: u64) -> DateBucket {
    const DAY_MS: u64 = 86_400_000;
    let now_day = now_ms / DAY_MS;
    let then_day = updated_at_ms / DAY_MS;
    match now_day.saturating_sub(then_day) {
        0 => DateBucket::Today,
        1 => DateBucket::Yesterday,
        2..=7 => DateBucket::Previous7Days,
        _ => DateBucket::Earlier,
    }
}

fn group_sessions_by_project(
    projection: &DesktopProjection,
    mut sessions: Vec<SessionSummary>,
) -> Vec<TaskRailProjectGroup> {
    sessions.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
    let mut groups: Vec<TaskRailProjectGroup> = Vec::new();
    for session in sessions {
        let key = session.workspace_id.clone();
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.workspace_id == key)
        {
            group.tasks.push(session);
        } else {
            let name = projection.workspace_name(key.as_deref());
            let latest_activity_ms = session.updated_at_ms;
            groups.push(TaskRailProjectGroup {
                workspace_id: key,
                name,
                latest_activity_ms,
                tasks: vec![session],
            });
        }
    }
    groups.sort_by(|a, b| b.latest_activity_ms.cmp(&a.latest_activity_ms));
    groups
}

fn parse_provider_status(data: &Value) -> Option<(String, String)> {
    let entry = data.as_array().and_then(|entries| entries.first()).or(Some(data))?;
    let provider = entry.get("provider_id").and_then(Value::as_str)?;
    let model = entry.get("model").and_then(Value::as_str)?;
    Some((provider.to_string(), model.to_string()))
}

fn parse_pending_approvals(data: &Value) -> Vec<PendingApproval> {
    let Some(entries) = data.as_array() else {
        return Vec::new();
    };
    entries.iter().filter_map(parse_pending_approval_entry).collect()
}

fn parse_pending_approval_entry(entry: &Value) -> Option<PendingApproval> {
    let run_id = entry.get("run_id").and_then(Value::as_str)?;
    let tool_call_id = entry.get("tool_call_id").and_then(Value::as_str)?;
    let tool_name = entry
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let message = entry.get("message").and_then(Value::as_str).unwrap_or("");
    let path = entry.get("relative_path").and_then(Value::as_str);
    let preview = entry.get("preview").and_then(Value::as_str);
    let reason = match path {
        Some(path) if !path.is_empty() => format!("{tool_name} · {path} · {message}"),
        _ => format!("{tool_name} · {message}"),
    };
    Some(PendingApproval {
        session_id: entry
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        run_id: run_id.to_string(),
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        reason,
        detail: preview.map(str::to_string),
    })
}

fn parse_active_runs(data: &Value) -> Vec<ActiveRun> {
    let Some(entries) = data.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            Some(ActiveRun {
                run_id: entry.get("run_id").and_then(Value::as_str)?.to_string(),
                session_id: entry.get("session_id").and_then(Value::as_str)?.to_string(),
                started_at_ms: entry.get("started_at_ms").and_then(Value::as_u64)?,
            })
        })
        .collect()
}

fn format_run_duration(started_at_ms: u64, now_ms: u64) -> String {
    let elapsed_s = now_ms.saturating_sub(started_at_ms) / 1000;
    let minutes = elapsed_s / 60;
    let seconds = elapsed_s % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn extract_tool_name(reason: &str) -> String {
    reason
        .split(" · ")
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("tool")
        .to_string()
}

fn sandbox_fallback_label(message: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(message) {
        if let Some(text) = value.get("message").and_then(Value::as_str) {
            return text.to_string();
        }
    }
    if message.is_empty() {
        "沙箱回退：隔离已降级".into()
    } else {
        message.to_string()
    }
}

fn parse_model_switch_message(message: &str) -> Option<(String, String)> {
    let value: Value = serde_json::from_str(message).ok()?;
    let target = value.get("to").cloned().unwrap_or(value);
    let provider = target.get("provider").and_then(Value::as_str)?;
    let model = target.get("model").and_then(Value::as_str)?;
    Some((provider.to_string(), model.to_string()))
}

/// 供 controller / probe 复用的 snapshot 解析。
pub fn sessions_in_snapshot(snapshot: &Snapshot) -> Vec<SessionSummary> {
    let mut projection = DesktopProjection::default();
    projection.merge_snapshot(snapshot);
    projection.sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot_with_sessions(entries: Vec<Value>) -> Snapshot {
        serde_json::from_value(json!({
            "instance_id": "instance-1",
            "snapshot_sequence": 0,
            "generated_at": 1,
            "sections": [
                {
                    "kind": "workspaces",
                    "revision": 1,
                    "data": [{ "id": "ws-default", "name": "default", "trusted": true }]
                },
                { "kind": "session_tree", "revision": 2, "data": entries }
            ]
        }))
        .expect("decode Snapshot")
    }

    fn session_entry(id: &str, title: &str, updated: u64) -> Value {
        session_entry_in(id, title, updated, None)
    }

    fn session_entry_in(id: &str, title: &str, updated: u64, workspace_id: Option<&str>) -> Value {
        let mut entry = json!({
            "session_id": id,
            "title": title,
            "created_at_ms": 1,
            "updated_at_ms": updated,
            "active_branch": "main",
            "archived": false
        });
        if let Some(workspace_id) = workspace_id {
            entry["workspace_id"] = json!(workspace_id);
        }
        entry
    }

    fn event(sequence: u64, payload: Value) -> AppEventEnvelope {
        serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": format!("app-{sequence}"),
            "global_sequence": sequence,
            "stream": { "type": "session", "id": "s-1" },
            "stream_sequence": sequence,
            "timestamp": 1_000 + sequence,
            "source": { "type": "core" },
            "payload": payload
        }))
        .expect("decode AppEventEnvelope")
    }

    fn run_changed(sequence: u64, state: &str) -> AppEventEnvelope {
        event(
            sequence,
            json!({ "type": "run_changed", "data": { "run_id": "r-1", "state": state } }),
        )
    }

    fn assistant_delta(sequence: u64, message_id: &str, delta: &str) -> AppEventEnvelope {
        event(
            sequence,
            json!({
                "type": "assistant_delta",
                "data": { "run_id": "r-1", "message_id": message_id, "delta": delta }
            }),
        )
    }

    fn page(items: Vec<Value>, complete: bool) -> TimelinePage {
        serde_json::from_value(json!({
            "items": items,
            "head_sequence": items.len() as u64,
            "complete": complete
        }))
        .expect("decode TimelinePage")
    }

    fn history_item(sequence: u64, kind: &str, extra: Value) -> Value {
        let mut item = json!({
            "sequence": sequence,
            "event_id": format!("hist-{sequence}"),
            "kind": kind,
            "run_id": "r-1",
            "timestamp": "2000"
        });
        if let Some(fields) = extra.as_object() {
            for (key, value) in fields {
                item[key] = value.clone();
            }
        }
        item
    }

    #[test]
    fn snapshot_rebuilds_sessions_and_events_rebuild_timeline() {
        let snapshot = snapshot_with_sessions(vec![
            session_entry("s-old", "Old", 10),
            session_entry("s-new", "New", 20),
        ]);
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        assert_eq!(projection.workspace_id.as_deref(), Some("ws-default"));
        // 按 updated_at_ms 倒序，最新 session 在最前。
        assert_eq!(projection.sessions[0].session_id, "s-new");
        assert_eq!(projection.sessions.len(), 2);

        projection.set_connection(ConnectionState::Connected {
            instance_id: "instance-1".into(),
        });
        projection.select_session("s-1");

        assert!(projection.apply_event(&run_changed(1, "created")));
        assert!(projection.apply_event(&assistant_delta(2, "m-1", "Hello ")));
        assert!(projection.apply_event(&assistant_delta(3, "m-1", "world")));
        assert!(projection.apply_event(&run_changed(4, "completed")));
        // 终态清空 active_run_id，Composer 恢复可用。
        assert_eq!(projection.active_run_id, None);

        let texts: Vec<String> = projection
            .timeline
            .iter()
            .map(|entry| match &entry.kind {
                TimelineEntryKind::AssistantMessage { text } => format!("assistant:{text}"),
                TimelineEntryKind::RunState(state) => format!("run:{state}"),
                other => format!("other:{other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                "run:run created".to_string(),
                "assistant:Hello world".to_string(),
                "run:run completed".to_string()
            ]
        );
    }

    #[test]
    fn assistant_deltas_merge_until_message_or_run_changes() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");

        projection.apply_event(&assistant_delta(1, "m-1", "a"));
        projection.apply_event(&assistant_delta(2, "m-1", "b"));
        projection.apply_event(&assistant_delta(3, "m-2", "c"));
        assert_eq!(projection.timeline.len(), 2);
        let texts: Vec<&str> = projection
            .timeline
            .iter()
            .map(|entry| match &entry.kind {
                TimelineEntryKind::AssistantMessage { text } => text.as_str(),
                _ => "other",
            })
            .collect();
        assert_eq!(texts, vec!["ab", "c"]);
    }

    #[test]
    fn timeline_pages_dedup_by_sequence_and_merge_committed_text() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");

        let first = page(
            vec![
                history_item(1, "user_message", json!({ "text": "hi" })),
                history_item(2, "assistant_delta", json!({ "text": "He" })),
                history_item(3, "assistant_delta", json!({ "text": "llo" })),
                history_item(4, "assistant_message", json!({ "text": "Hello" })),
            ],
            false,
        );
        projection.apply_timeline_page(&first);
        // 重放同一页：sequence 去重，条目数不变。
        projection.apply_timeline_page(&first);
        assert_eq!(projection.timeline.len(), 2);
        assert!(matches!(
            &projection.timeline[1].kind,
            TimelineEntryKind::AssistantMessage { text } if text == "Hello"
        ));
        // committed 替换后条目携带 committed 的 sequence。
        assert_eq!(projection.timeline[1].sequence, 4);

        let second = page(
            vec![
                history_item(3, "assistant_delta", json!({ "text": "llo" })),
                history_item(
                    5,
                    "tool_started",
                    json!({ "tool_name": "fs_read", "status": "running" }),
                ),
                history_item(6, "tool_output", json!({ "text": "42 bytes" })),
                history_item(
                    7,
                    "tool_completed",
                    json!({ "tool_name": "fs_read", "status": "succeeded" }),
                ),
            ],
            true,
        );
        projection.apply_timeline_page(&second);
        assert_eq!(projection.timeline.len(), 3);
        assert!(matches!(
            &projection.timeline[2].kind,
            TimelineEntryKind::ToolCall { name, status, detail }
                if name == "fs_read" && status == "succeeded" && detail.as_deref() == Some("42 bytes")
        ));

        // 页数据之外先到的 live 事件重放（同 sequence）不再重复。
        assert!(!projection.apply_event(&assistant_delta(2, "m-1", "He")));
    }

    #[test]
    fn approval_card_clears_on_terminal_run() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert!(projection.apply_event(&event(
            1,
            json!({
                "type": "tool_approval_required",
                "data": {
                    "run_id": "r-1",
                    "tool_call_id": "call-1",
                    "reason": "write_file · notes.txt · Approve workspace file write"
                }
            }),
        )));
        assert_eq!(
            projection
                .pending_approval
                .as_ref()
                .map(|item| item.tool_name.as_str()),
            Some("write_file")
        );
        assert!(projection.apply_event(&run_changed(2, "cancelled")));
        assert_eq!(projection.pending_approval, None);
    }

    #[test]
    fn pending_model_is_overwritten_by_diagnostic() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        projection.set_pending_model("mock".into(), "model-2".into());
        assert!(projection.apply_event(&event(
            1,
            json!({
                "type": "diagnostic",
                "data": {
                    "level": "info",
                    "code": "model.switched",
                    "message": "{\"to\":{\"provider\":\"mock\",\"model\":\"model-2\"}}"
                }
            }),
        )));
        assert_eq!(
            projection
                .selected_model
                .as_ref()
                .map(|(provider, model)| (provider.as_str(), model.as_str())),
            Some(("mock", "model-2"))
        );
        assert_eq!(projection.pending_model, None);
    }

    #[test]
    fn sandbox_fallback_diagnostic_appears_on_timeline() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert!(projection.apply_event(&event(
            1,
            json!({
                "type": "diagnostic",
                "data": {
                    "level": "info",
                    "code": "sandbox.fallback",
                    "message": "{\"message\":\"沙箱回退：isolation=soft backend=native_restricted\"}"
                }
            }),
        )));
        assert!(matches!(
            &projection.timeline[0].kind,
            TimelineEntryKind::RunState(text) if text.contains("沙箱回退")
        ));
    }

    fn snapshot_with_runs_and_approvals(runs: Vec<Value>, approvals: Vec<Value>) -> Snapshot {
        serde_json::from_value(json!({
            "instance_id": "instance-1",
            "snapshot_sequence": 0,
            "generated_at": 1,
            "sections": [
                {
                    "kind": "session_tree",
                    "revision": 1,
                    "data": [session_entry("s-1", "One", 20)]
                },
                { "kind": "active_runs", "revision": 2, "data": runs },
                { "kind": "pending_tool_approvals", "revision": 3, "data": approvals }
            ]
        }))
        .expect("decode Snapshot")
    }

    #[test]
    fn snapshot_active_runs_restore_cancel_target_on_select() {
        let snapshot = snapshot_with_runs_and_approvals(
            vec![json!({
                "run_id": "r-live",
                "session_id": "s-1",
                "started_at_ms": 1_700_000_000_000_u64
            })],
            vec![json!({
                "run_id": "r-live",
                "session_id": "s-1",
                "tool_call_id": "call-9",
                "tool_name": "write_file",
                "message": "Approve workspace file write",
                "relative_path": "notes.txt"
            })],
        );
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        assert_eq!(projection.active_run_id, None);
        projection.select_session("s-1");
        assert_eq!(projection.active_run_id.as_deref(), Some("r-live"));
        assert_eq!(projection.active_run_started_at_ms, Some(1_700_000_000_000));
        assert_eq!(
            projection
                .pending_approval
                .as_ref()
                .map(|item| item.tool_call_id.as_str()),
            Some("call-9")
        );
        assert_eq!(
            projection.run_status_label(1_700_000_045_000),
            "tokens —  ·  quota —  ·  — tok/s  ·  00:45"
        );
    }

    #[test]
    fn context_meter_uses_catalog_window_and_stays_honest() {
        let mut projection = DesktopProjection::default();
        assert_eq!(projection.context_meter_label(), "Context · unavailable");
        projection.set_models(vec![ModelEntry {
            provider_id: "glm-coding".into(),
            id: "glm-4.7".into(),
            display_name: "GLM 4.7".into(),
            context_window_tokens: Some(200_000),
        }]);
        projection.set_pending_model("glm-coding".into(), "glm-4.7".into());
        assert_eq!(projection.context_meter_label(), "Context · — / 200000");
    }

    fn day_ms(days: u64) -> u64 {
        days * 86_400_000
    }

    fn snapshot_with_named_workspaces(workspaces: Vec<Value>, sessions: Vec<Value>) -> Snapshot {
        serde_json::from_value(json!({
            "instance_id": "instance-1",
            "snapshot_sequence": 0,
            "generated_at": 1,
            "sections": [
                { "kind": "workspaces", "revision": 1, "data": workspaces },
                { "kind": "session_tree", "revision": 2, "data": sessions }
            ]
        }))
        .expect("decode Snapshot")
    }

    #[test]
    fn task_rail_groups_date_then_project_and_keeps_unassigned() {
        let now = day_ms(20);
        let snapshot = snapshot_with_named_workspaces(
            vec![
                json!({ "id": "ws-alpha", "name": "Alpha" }),
                json!({ "id": "ws-beta", "name": "Beta" }),
            ],
            vec![
                session_entry_in("s-today-beta", "Beta today", now + 2, Some("ws-beta")),
                session_entry_in("s-today-alpha-new", "Alpha new", now + 1, Some("ws-alpha")),
                session_entry_in("s-today-alpha-old", "Alpha old", now, Some("ws-alpha")),
                session_entry_in("s-yesterday", "Y", now - day_ms(1), Some("ws-alpha")),
                session_entry_in("s-week", "W", now - day_ms(3), Some("ws-beta")),
                session_entry_in("s-old", "Old", now - day_ms(20), Some("ws-alpha")),
                session_entry("s-orphan", "Orphan", now - 10),
            ],
        );
        let projection = DesktopProjection::from_snapshot(&snapshot);
        assert_eq!(projection.workspace_name(None), UNASSIGNED_PROJECT);
        assert_eq!(projection.workspace_name(Some("ws-alpha")), "Alpha");

        let timeline = projection.timeline_groups(None, now + 3);
        assert_eq!(
            timeline
                .iter()
                .map(|group| group.bucket)
                .collect::<Vec<_>>(),
            vec![
                DateBucket::Today,
                DateBucket::Yesterday,
                DateBucket::Previous7Days,
                DateBucket::Earlier
            ]
        );
        let today = &timeline[0];
        assert_eq!(
            today
                .projects
                .iter()
                .map(|project| project.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Beta", "Alpha"]
        );
        assert_eq!(
            today.projects[1]
                .tasks
                .iter()
                .map(|task| task.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["s-today-alpha-new", "s-today-alpha-old"]
        );
        assert!(today.projects.iter().all(|project| {
            project
                .tasks
                .iter()
                .all(|task| task.workspace_id.as_deref() != Some("title-guess"))
        }));

        let earlier = timeline.last().expect("earlier");
        assert_eq!(earlier.projects[0].name, "Alpha");
        assert_eq!(earlier.projects[0].tasks[0].session_id, "s-old");

        let projects = projection.project_groups(None);
        assert_eq!(
            projects
                .iter()
                .map(|project| (project.name.as_str(), project.task_count()))
                .collect::<Vec<_>>(),
            vec![("Beta", 2), ("Alpha", 4), (UNASSIGNED_PROJECT, 1)]
        );
        assert!(projects.last().expect("unassigned").is_unassigned());
        assert_eq!(projects.last().expect("unassigned").tasks[0].session_id, "s-orphan");

        let scoped = projection.project_groups(Some("ws-beta"));
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].name, "Beta");
        assert_eq!(scoped[0].task_count(), 2);
    }

    #[test]
    fn task_rail_empty_state_and_scope_options() {
        let empty = DesktopProjection::default();
        assert!(empty.timeline_groups(None, 1).is_empty());
        assert!(empty.project_groups(None).is_empty());
        assert_eq!(
            empty.project_scope_options(),
            vec![(None, "All projects".into())]
        );

        let snapshot = snapshot_with_named_workspaces(
            vec![json!({ "id": "ws-default", "name": "default" })],
            vec![session_entry_in("s-1", "One", 10, Some("ws-default"))],
        );
        let projection = DesktopProjection::from_snapshot(&snapshot);
        assert_eq!(
            projection.project_scope_options(),
            vec![
                (None, "All projects".into()),
                (Some("ws-default".into()), "default".into())
            ]
        );
    }

    #[test]
    fn grouping_switch_does_not_change_active_session() {
        let snapshot = snapshot_with_named_workspaces(
            vec![json!({ "id": "ws-default", "name": "default" })],
            vec![
                session_entry_in("s-1", "One", 20, Some("ws-default")),
                session_entry_in("s-2", "Two", 10, Some("ws-default")),
            ],
        );
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        projection.select_session("s-2");
        let before = projection.active_session_id.clone();
        let _timeline = projection.timeline_groups(None, 20);
        let _projects = projection.project_groups(None);
        assert_eq!(projection.active_session_id, before);
        assert!(projection
            .project_groups(None)
            .iter()
            .flat_map(|project| &project.tasks)
            .any(|task| task.session_id == "s-2"));
    }

    fn resume_outcome(
        disposition: ResumeDisposition,
        replayed: Vec<AppEventEnvelope>,
        snapshot: Option<Snapshot>,
    ) -> ResumeOutcome {
        ResumeOutcome {
            disposition,
            replayed,
            snapshot,
        }
    }

    #[test]
    fn session_tree_accepts_flat_sessions_and_branch_nodes() {
        let flat = snapshot_with_sessions(vec![session_entry("s-1", "One", 20)]);
        let projection = DesktopProjection::from_snapshot(&flat);
        assert_eq!(projection.sessions[0].session_id, "s-1");
        assert!(projection.sessions[0].active);
        assert_eq!(projection.sessions[0].parent_branch_id, None);

        let branched = snapshot_with_sessions(vec![json!({
            "branch_id": "br-2",
            "parent_branch_id": "br-1",
            "forked_from_event_id": "evt-9",
            "active": true,
            "title": "Forked",
            "updated_at_ms": 40,
            "workspace_id": "ws-default"
        })]);
        let projection = DesktopProjection::from_snapshot(&branched);
        assert_eq!(projection.sessions[0].session_id, "br-2");
        assert_eq!(
            projection.sessions[0].parent_branch_id.as_deref(),
            Some("br-1")
        );
        assert_eq!(
            projection.sessions[0].forked_from_event_id.as_deref(),
            Some("evt-9")
        );

        let wrapped = snapshot_with_named_workspaces(
            vec![json!({ "id": "ws-default", "name": "default" })],
            vec![],
        );
        let mut wrapped_json = serde_json::to_value(&wrapped).expect("snapshot json");
        wrapped_json["sections"][1]["data"] = json!({
            "nodes": [{
                "branch_id": "br-wrap",
                "parent_branch_id": null,
                "forked_from_event_id": null,
                "active": false,
                "name": "Wrapped",
                "updated_at_ms": 5
            }]
        });
        let wrapped: Snapshot = serde_json::from_value(wrapped_json).expect("decode wrapped");
        let projection = DesktopProjection::from_snapshot(&wrapped);
        assert_eq!(projection.sessions[0].session_id, "br-wrap");
        assert!(!projection.sessions[0].active);
        assert_eq!(projection.sessions[0].title, "Wrapped");
    }

    #[test]
    fn replay_continues_timeline_without_replacing_baseline() {
        let snapshot = snapshot_with_sessions(vec![session_entry("s-1", "One", 20)]);
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        projection.select_session("s-1");
        assert!(projection.apply_event(&assistant_delta(2, "m-1", "Hello")));
        assert_eq!(projection.timeline.len(), 1);

        let outcome = resume_outcome(
            ResumeDisposition::Replay {
                from_sequence: pawork_client::GlobalSequence(3),
                through_sequence: pawork_client::GlobalSequence(4),
            },
            vec![
                assistant_delta(3, "m-1", " "),
                assistant_delta(4, "m-1", "world"),
            ],
            None,
        );
        let apply = projection.apply_resume_outcome(&outcome, &snapshot);
        assert_eq!(apply, ResumeApply::Continued { timeline_changed: true });
        assert!(!projection.resume.replaces_baseline());
        assert_eq!(
            projection.resume.label().as_deref(),
            Some("Replay · 3–4")
        );
        assert_eq!(projection.timeline.len(), 1);
        assert!(matches!(
            &projection.timeline[0].kind,
            TimelineEntryKind::AssistantMessage { text } if text == "Hello world"
        ));
        // 同 sequence 再来一遍不得双份。
        assert!(!projection.apply_event(&assistant_delta(3, "m-1", " ")));
    }

    #[test]
    fn snapshot_required_discards_stale_and_replaces_baseline() {
        let first = snapshot_with_sessions(vec![session_entry("s-1", "One", 20)]);
        let mut projection = DesktopProjection::from_snapshot(&first);
        projection.select_session("s-1");
        projection.apply_event(&assistant_delta(1, "m-1", "stale"));
        projection.apply_event(&event(
            2,
            json!({
                "type": "tool_approval_required",
                "data": {
                    "run_id": "r-old",
                    "tool_call_id": "call-old",
                    "reason": "write_file · old"
                }
            }),
        ));
        assert_eq!(projection.timeline.len(), 1);
        assert!(projection.pending_approval.is_some());

        let next = snapshot_with_runs_and_approvals(
            vec![json!({
                "run_id": "r-new",
                "session_id": "s-1",
                "started_at_ms": 9
            })],
            vec![],
        );
        let outcome = resume_outcome(
            ResumeDisposition::SnapshotRequired {
                earliest_available_sequence: pawork_client::GlobalSequence(8),
            },
            vec![],
            None,
        );
        assert_eq!(
            projection.apply_resume_outcome(&outcome, &next),
            ResumeApply::ReplaceBaseline
        );
        assert!(projection.resume.replaces_baseline());
        assert_eq!(projection.timeline.len(), 0);
        assert_eq!(projection.pending_approval, None);
        assert_eq!(projection.active_session_id.as_deref(), Some("s-1"));
        assert_eq!(projection.active_run_id.as_deref(), Some("r-new"));
        assert_eq!(
            projection.resume.label().as_deref(),
            Some("Snapshot required · from 8")
        );
    }

    #[test]
    fn up_to_date_does_not_flash_reload() {
        let snapshot = snapshot_with_sessions(vec![session_entry("s-1", "One", 20)]);
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        projection.select_session("s-1");
        projection.apply_event(&assistant_delta(1, "m-1", "keep"));
        let outcome = resume_outcome(
            ResumeDisposition::UpToDate {
                current_sequence: pawork_client::GlobalSequence(1),
            },
            vec![],
            Some(snapshot_with_sessions(vec![session_entry("s-other", "Other", 1)])),
        );
        assert_eq!(
            projection.apply_resume_outcome(&outcome, &snapshot),
            ResumeApply::Unchanged
        );
        assert_eq!(projection.timeline.len(), 1);
        assert_eq!(projection.sessions[0].session_id, "s-1");
        assert_eq!(
            projection.resume.label().as_deref(),
            Some("Up to date · 1")
        );
    }

    fn terminal_output(sequence: u64, terminal: &str, delta: &str) -> AppEventEnvelope {
        serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": format!("term-{sequence}"),
            "global_sequence": sequence,
            "stream": { "type": "terminal", "id": terminal },
            "stream_sequence": sequence,
            "timestamp": 1_000 + sequence,
            "source": { "type": "core" },
            "payload": {
                "type": "terminal_output",
                "data": { "terminal_session_id": terminal, "delta": delta }
            }
        }))
        .expect("decode TerminalOutput")
    }

    #[test]
    fn terminal_output_appends_without_vt100() {
        let mut projection = DesktopProjection::default();
        assert!(projection.apply_event(&terminal_output(1, "term-1", "hello")));
        assert!(projection.apply_event(&terminal_output(2, "term-1", "\nworld")));
        assert_eq!(projection.terminal.session_id.as_deref(), Some("term-1"));
        assert_eq!(projection.terminal.output, "hello\nworld");
        assert!(!projection.apply_event(&terminal_output(3, "term-other", "nope")));
        assert_eq!(projection.terminal.output, "hello\nworld");
    }

    fn tool_started(sequence: u64, tool_call_id: &str, name: &str) -> AppEventEnvelope {
        event(
            sequence,
            json!({
                "type": "tool_started",
                "data": {
                    "run_id": "r-1",
                    "tool_call_id": tool_call_id,
                    "name": name
                }
            }),
        )
    }

    fn tool_output(sequence: u64, tool_call_id: &str, delta: &str) -> AppEventEnvelope {
        event(
            sequence,
            json!({
                "type": "tool_output",
                "data": {
                    "run_id": "r-1",
                    "tool_call_id": tool_call_id,
                    "delta": delta,
                    "truncated": false
                }
            }),
        )
    }

    fn tool_completed(sequence: u64, tool_call_id: &str, success: bool) -> AppEventEnvelope {
        event(
            sequence,
            json!({
                "type": "tool_completed",
                "data": {
                    "run_id": "r-1",
                    "tool_call_id": tool_call_id,
                    "success": success
                }
            }),
        )
    }

    #[test]
    fn live_tool_survives_earlier_page_insert_without_duplicate() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");

        assert!(projection.apply_event(&tool_started(10, "call-1", "fs_read")));
        assert_eq!(projection.timeline.len(), 1);
        assert!(matches!(
            &projection.timeline[0].kind,
            TimelineEntryKind::ToolCall { name, status, .. }
                if name == "fs_read" && status == "running"
        ));

        projection.apply_timeline_page(&page(
            vec![history_item(5, "user_message", json!({ "text": "hi" }))],
            false,
        ));
        assert_eq!(projection.timeline.len(), 2);
        assert!(matches!(
            &projection.timeline[0].kind,
            TimelineEntryKind::UserMessage { text } if text == "hi"
        ));
        assert!(matches!(
            &projection.timeline[1].kind,
            TimelineEntryKind::ToolCall { name, status, .. }
                if name == "fs_read" && status == "running"
        ));

        assert!(projection.apply_event(&tool_completed(11, "call-1", true)));
        let tools: Vec<(&str, &str)> = projection
            .timeline
            .iter()
            .filter_map(|entry| match &entry.kind {
                TimelineEntryKind::ToolCall { name, status, .. } => {
                    Some((name.as_str(), status.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(tools, vec![("fs_read", "succeeded")]);
        assert!(!projection.timeline.iter().any(|entry| matches!(
            &entry.kind,
            TimelineEntryKind::ToolCall { status, .. } if status == "running"
        )));
    }

    #[test]
    fn live_tool_output_fills_running_entry() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert!(projection.apply_event(&tool_started(10, "call-1", "fs_read")));
        assert!(projection.apply_event(&tool_output(11, "call-1", "chunk-a")));
        assert!(matches!(
            &projection.timeline[0].kind,
            TimelineEntryKind::ToolCall { name, status, detail }
                if name == "fs_read" && status == "running" && detail.as_deref() == Some("chunk-a")
        ));
        projection.apply_timeline_page(&page(
            vec![history_item(
                11,
                "tool_output",
                json!({ "tool_name": "fs_read", "text": "chunk-a" }),
            )],
            false,
        ));
        let tools: Vec<_> = projection
            .timeline
            .iter()
            .filter_map(|entry| match &entry.kind {
                TimelineEntryKind::ToolCall { name, status, detail } => {
                    Some((name.as_str(), status.as_str(), detail.as_deref()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(tools, vec![("fs_read", "running", Some("chunk-a"))]);
    }

    #[test]
    fn history_approval_events_leave_traces() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        projection.apply_timeline_page(&page(
            vec![
                history_item(
                    1,
                    "approval_requested",
                    json!({ "tool_name": "write_file", "text": "edit src/lib.rs" }),
                ),
                history_item(
                    2,
                    "approval_responded",
                    json!({ "status": "approve_once" }),
                ),
            ],
            true,
        ));
        let labels: Vec<&str> = projection
            .timeline
            .iter()
            .filter_map(|entry| match &entry.kind {
                TimelineEntryKind::RunState(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            labels.iter().any(|text| text.contains("approval requested") && text.contains("write_file")),
            "history approval_requested should remain, got {labels:?}"
        );
        assert!(
            labels.iter().any(|text| text.contains("approval approve_once")),
            "history approval_responded should remain, got {labels:?}"
        );
        assert!(projection.pending_approval.is_none());
    }

    #[test]
    fn fifty_thousand_timeline_entries_iter_without_clone() {
        let mut projection = DesktopProjection::default();
        projection.timeline.reserve(50_000);
        for sequence in 0..50_000u64 {
            projection.timeline.push(TimelineEntry {
                sequence,
                event_id: format!("e{sequence}"),
                kind: TimelineEntryKind::RunState("x".into()),
                timestamp: "1".into(),
                run_id: None,
            });
        }
        let started = std::time::Instant::now();
        let count = projection.timeline.iter().map(|entry| entry.sequence).count();
        let elapsed = started.elapsed();
        assert_eq!(count, 50_000);
        assert!(
            elapsed.as_millis() < 100,
            "borrowed timeline iter should stay cheap, took {elapsed:?}"
        );
    }

    #[test]
    fn live_assistant_survives_earlier_page_insert_without_split() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");

        assert!(projection.apply_event(&assistant_delta(10, "m-1", "Hello")));
        projection.apply_timeline_page(&page(
            vec![history_item(5, "user_message", json!({ "text": "hi" }))],
            false,
        ));
        assert!(projection.apply_event(&assistant_delta(11, "m-1", " world")));

        let assistants: Vec<&str> = projection
            .timeline
            .iter()
            .filter_map(|entry| match &entry.kind {
                TimelineEntryKind::AssistantMessage { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(assistants, vec!["Hello world"]);
        assert!(matches!(
            &projection.timeline[0].kind,
            TimelineEntryKind::UserMessage { text } if text == "hi"
        ));
    }
}
