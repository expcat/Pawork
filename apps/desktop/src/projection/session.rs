//! Session / workspace / TaskRail 投影类型与 snapshot 解析。

use std::collections::BTreeSet;

use pawork_client::{ResumeDisposition, Snapshot};
use serde_json::Value;

use super::DesktopProjection;

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
            Self::UpToDate { current_sequence } => Some(format!("Up to date · {current_sequence}")),
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

/// Settings「模型与默认项」区分组：按 provider 聚合可运行模型（保持目录
/// 首现顺序），render 与 AX 同源。返回 owned 数据，避免 UI 构建期间的
/// 借用冲突。
pub fn group_models_by_provider(models: &[ModelEntry]) -> Vec<(String, Vec<ModelEntry>)> {
    let mut groups: Vec<(String, Vec<ModelEntry>)> = Vec::new();
    for model in models {
        match groups
            .iter_mut()
            .find(|(provider, _)| *provider == model.provider_id)
        {
            Some((_, entries)) => entries.push(model.clone()),
            None => groups.push((model.provider_id.clone(), vec![model.clone()])),
        }
    }
    groups
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveRun {
    pub run_id: String,
    pub session_id: String,
    pub started_at_ms: u64,
}

/// TaskRail 每会话 live 状态（诚实语义：只消费 wire 已有数据，不伪造终态）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionLiveStatus {
    /// 运行中（snapshot active_runs / RunChanged 非终态）。
    Running,
    /// 需要输入（pending approval，按 session_id 归属）。
    NeedsInput,
    /// 受阻（live 派生：该 session 最近一条 RunChanged 为终态且
    /// state ∈ failed / interrupted；completed / cancelled 不算）。
    Blocked,
}

impl SessionLiveStatus {
    /// AX description / 状态点语义共用状态词。
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::NeedsInput => "Needs input",
            Self::Blocked => "Blocked",
        }
    }
}
pub(super) fn session_tree_entries(data: &Value) -> Option<&Vec<Value>> {
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

pub(super) fn parse_sessions(data: &Value) -> Vec<SessionSummary> {
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
pub(super) fn parse_workspaces(data: &Value) -> Vec<WorkspaceSummary> {
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
        if let Some(group) = groups.iter_mut().find(|group| group.workspace_id == key) {
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
pub(super) fn parse_provider_status(data: &Value) -> Option<(String, String)> {
    let entry = data
        .as_array()
        .and_then(|entries| entries.first())
        .or(Some(data))?;
    let provider = entry.get("provider_id").and_then(Value::as_str)?;
    let model = entry.get("model").and_then(Value::as_str)?;
    Some((provider.to_string(), model.to_string()))
}

pub(super) fn parse_pending_approvals(data: &Value) -> Vec<PendingApproval> {
    let Some(entries) = data.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(parse_pending_approval_entry)
        .collect()
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

pub(super) fn parse_active_runs(data: &Value) -> Vec<ActiveRun> {
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
/// 供 controller / probe 复用的 snapshot 解析。
pub fn sessions_in_snapshot(snapshot: &Snapshot) -> Vec<SessionSummary> {
    let mut projection = DesktopProjection::default();
    projection.merge_snapshot(snapshot);
    projection.sessions
}

impl DesktopProjection {
    pub fn active_workspace_id(&self) -> Option<&str> {
        match self.active_session_id.as_deref() {
            Some(session_id) => self
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
                .and_then(|session| session.workspace_id.as_deref()),
            None => self.workspace_id.as_deref(),
        }
    }

    /// 打开（切换）session：清空时间线与去重状态。
    pub fn select_session(&mut self, session_id: &str) {
        self.active_session_id = Some(session_id.to_string());
        // R3 Wave B：打开 / 切换即视为已读。
        self.unread_sessions.remove(session_id);
        self.active_run_id = None;
        self.active_run_started_at_ms = None;
        self.pending_approval = None;
        self.timeline.reset_baseline();
        self.restore_active_run_from_snapshot();
        self.pending_approval = self.pending_for_active_session();
        let workspace_id = self.active_workspace_id().map(str::to_string);
        self.select_terminal_for_workspace(workspace_id.as_deref());
    }

    pub(super) fn restore_active_run_from_snapshot(&mut self) {
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

    pub(super) fn pending_for_active_session(&self) -> Option<PendingApproval> {
        self.snapshot_pendings
            .iter()
            .find(|pending| {
                pending.session_id.as_deref() == self.active_session_id.as_deref()
                    || pending.session_id.is_none()
            })
            .cloned()
    }

    pub fn set_connection(&mut self, state: ConnectionState) {
        if matches!(state, ConnectionState::Connected { .. }) {
            self.restore_terminal_availability();
        } else {
            self.mark_terminals_stale(state.label());
        }
        self.connection = state;
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
            self.scoped_sessions(scope).into_iter().cloned().collect(),
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
    /// F-13 定稿语序与竖线分隔：Task tokens | quota | tok/s | Run 时长。
    /// `now_ms` 由 UI 注入，投影层不读系统时钟。
    pub fn run_status_label(&self, now_ms: u64) -> String {
        let duration = match (self.active_run_id.as_ref(), self.active_run_started_at_ms) {
            (Some(_), Some(started_at_ms)) => format_run_duration(started_at_ms, now_ms),
            (Some(_), None) => "—".into(),
            (None, _) => "idle".into(),
        };
        format!("Task — tokens | Quota unavailable | — tok/s | Run {duration}")
    }

    /// Reconnect 相位（F-02 壳层校准）：仅 Disconnected / ConnectFailed 提供
    /// 手动重连；Connecting 属进行中、Connected 无需重连，均不显示按钮。
    pub fn show_reconnect(&self) -> bool {
        matches!(
            self.connection,
            ConnectionState::Disconnected { .. } | ConnectionState::Failed { .. }
        )
    }

    /// TaskRail 每会话 live 状态（R3 Wave A 状态点语义）：
    /// - Needs input：pending approval 按 session_id 归属（无归属字段的
    ///   pending 沿用 pending_for_active_session 同规，归 active session）；
    ///   与 Running 并存时优先。
    /// - Running：snapshot active_runs / RunChanged 非终态。
    /// - Blocked（R3 Wave B）：最近一条 RunChanged 为 failed / interrupted
    ///   终态（live 派生；快照重建清空），优先级最低。
    /// - None：无 live 状态，rail 画空心灰圆（不声明语义）。wire 无每会话
    ///   终态字段，终态绿点不画（伪造即红线）。
    pub fn session_live_status(&self, session_id: &str) -> Option<SessionLiveStatus> {
        let needs_input = self.snapshot_pendings.iter().any(|pending| {
            pending.session_id.as_deref() == Some(session_id)
                || (pending.session_id.is_none()
                    && self.active_session_id.as_deref() == Some(session_id))
        });
        if needs_input {
            return Some(SessionLiveStatus::NeedsInput);
        }
        if self
            .active_runs
            .iter()
            .any(|run| run.session_id == session_id)
        {
            return Some(SessionLiveStatus::Running);
        }
        if self.blocked_sessions.contains(session_id) {
            return Some(SessionLiveStatus::Blocked);
        }
        None
    }

    /// R3 Wave B：unread 通道——非 active session 收到 Session-stream 活动
    /// 事件后为 true，select_session 清除；快照 / 首连不产生 unread。
    pub fn session_unread(&self, session_id: &str) -> bool {
        self.unread_sessions.contains(session_id)
    }

    /// Timeline 空态引导可见条件：无 active session 且无任何条目（含审批卡）。
    /// Disconnected 保留旧条目时条目数非零，不显示引导（gui-design 空态原则）。
    pub fn workspace_empty_hint_visible(&self) -> bool {
        self.active_session_id.is_none()
            && self.timeline.is_empty()
            && self.pending_approval.is_none()
    }

    /// Workspace Header 任务标题（F-05）：active session 的权威标题；
    /// 无 active session 返回 None（UI 隐藏标题项，骨架常存）。
    pub fn workspace_header_title(&self) -> Option<&str> {
        let session_id = self.active_session_id.as_deref()?;
        self.sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .map(|session| session.title.as_str())
    }

    /// Workspace Header 终态（F-05 诚实口径）：active session 的 live 派生
    /// 态，与 TaskRail 状态点同源（NeedsInput > Running > Blocked）；wire
    /// 无每会话终态字段，空闲会话返回 None（不画 Completed 绿点）。
    pub fn workspace_header_status(&self) -> Option<SessionLiveStatus> {
        self.active_session_id
            .as_deref()
            .and_then(|session_id| self.session_live_status(session_id))
    }

    /// MessageSent 乐观登记：composer 回执先于 live RunChanged 到达时，
    /// rail 状态点也必须立刻变成 Running，不能等下一帧事件。
    pub fn note_session_run(&mut self, session_id: &str, run_id: &str, started_at_ms: u64) {
        if self.active_session_id.as_deref() == Some(session_id) {
            self.active_run_id = Some(run_id.to_string());
            if self.active_run_started_at_ms.is_none() {
                self.active_run_started_at_ms = Some(started_at_ms);
            }
        }
        if self.active_runs.iter().any(|run| run.run_id == run_id) {
            return;
        }
        self.active_runs.push(ActiveRun {
            run_id: run_id.to_string(),
            session_id: session_id.to_string(),
            started_at_ms,
        });
    }

}
