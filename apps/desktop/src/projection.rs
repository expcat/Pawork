//! Desktop 渲染适配投影：把 pawork-client 的 Snapshot / TimelinePage / AppEvent
//! 投影为 Desktop UI 可直接渲染的状态。
//!
//! 本模块不依赖 gpui / tokio / OS API（gui-design 四层约束）。时间线语义
//! （去重 / 有序插入 / assistant 合并 / tool 双键锚点 / resume 基线）委托
//! pawork-protocol::projection 的单一 reducer（R3 波 C，CR08-08 根治）；
//! 本文件只保留 UI 态（连接 / session 列表 / 审批卡 / 模型 / run 跟踪）与
//! 渲染分组。

use std::collections::BTreeSet;

use pawork_client::projection::TimelineProjection;
use pawork_client::{
    AppEvent, AppEventEnvelope, EventStream, ResumeDisposition, ResumeOutcome, RunState, Snapshot,
    TerminalExitReason, TimelineItemKind, TimelinePage,
};
use serde_json::Value;

pub use pawork_client::projection::{ForkBoundary, TimelineEntry, TimelineEntryKind};

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

/// Inspector Terminal 面：滚动文本，不是 VT100 / 本地 PTY。
///
/// cwd 只承载 Host 可证事实：快照缺 cwd 键（旧 Host / 记账缺失）时用
/// [TERMINAL_CWD_UNKNOWN] 诚实占位，不臆造工作区根 "."。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalState {
    pub session_id: Option<String>,
    /// Host snapshot 的 owner_session；Desktop 将其解释为 terminal 所属
    /// workspace，而不是当前打开的 task/session。
    pub workspace_id: Option<String>,
    pub output: String,
    pub columns: u16,
    pub rows: u16,
    /// 仅 workspace 相对路径。
    pub cwd: String,
    /// Host 快照原样给出的 PTY 状态（running / exited / killed）；不从
    /// output 或本地 UI 动作猜测退出态。
    pub runtime_state: Option<String>,
    /// 实时广播被覆写的权威计数；非零时 UI 可诚实提示输出可能不完整。
    pub dropped_events: u64,
    /// Desktop 本连接已收到 Host resize 回执；snapshot 本身不含该事实，
    /// 重连/快照重建后不宣称已确认。
    pub resize_confirmed: bool,
    pub availability: TerminalAvailability,
}

/// Host 快照未提供 cwd 时的诚实占位（区别于「将创建在工作区根」的 "."）。
pub(crate) const TERMINAL_CWD_UNKNOWN: &str = "unknown";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalAvailability {
    Ready,
    Stale { reason: String },
    Failed { reason: String },
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            session_id: None,
            workspace_id: None,
            output: String::new(),
            columns: 80,
            rows: 24,
            cwd: ".".into(),
            runtime_state: None,
            dropped_events: 0,
            resize_confirmed: false,
            availability: TerminalAvailability::Stale {
                reason: "not started".into(),
            },
        }
    }
}

impl TerminalState {
    fn from_snapshot(entry: &Value) -> Option<Self> {
        let session_id = entry
            .get("terminal_session_id")
            .or_else(|| entry.get("id"))
            .and_then(Value::as_str)?;
        let workspace_id = entry
            .get("owner_session")
            .or_else(|| entry.get("workspace_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let runtime_state = entry
            .get("state")
            .and_then(Value::as_str)
            .map(str::to_string);
        let cwd = entry
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|cwd| !cwd.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| TERMINAL_CWD_UNKNOWN.to_string());
        let availability = match runtime_state.as_deref() {
            Some("running") => TerminalAvailability::Ready,
            Some(state @ ("exited" | "killed")) => TerminalAvailability::Stale {
                reason: format!("terminal {state}"),
            },
            Some(state) => TerminalAvailability::Stale {
                reason: format!("terminal state {state}"),
            },
            None => TerminalAvailability::Stale {
                reason: "terminal state unavailable".into(),
            },
        };
        Some(Self {
            session_id: Some(session_id.to_string()),
            workspace_id,
            columns: entry
                .get("columns")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(80),
            rows: entry
                .get("rows")
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(24),
            runtime_state,
            dropped_events: entry
                .get("dropped_events")
                .or_else(|| entry.get("dropped"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            availability,
            cwd,
            ..Self::default()
        })
    }

    fn mark_stale(&mut self, reason: impl Into<String>) {
        self.availability = TerminalAvailability::Stale {
            reason: reason.into(),
        };
    }

    pub fn mark_failed(&mut self, reason: impl Into<String>) {
        self.availability = TerminalAvailability::Failed {
            reason: reason.into(),
        };
    }

    pub fn availability_label(&self) -> String {
        match &self.availability {
            TerminalAvailability::Ready => {
                self.runtime_state.clone().unwrap_or_else(|| "ready".into())
            }
            TerminalAvailability::Stale { reason } => format!("stale · {reason}"),
            TerminalAvailability::Failed { reason } => format!("failed · {reason}"),
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

/// Timeline 渲染行（R4 Wave A F-08 组装纯数据）：连续 ToolCall（同 run
/// 相邻）合并为 tool activity 组；run 终态条目（fork_boundary 单点判型）
/// 与紧邻其前的 tool 组合成 Run 摘要区域。索引指向 timeline.entries，
/// 行序与条目序一致；approval 卡由 UI 作为 list 末项另行附加，不占行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineRow {
    /// user / assistant 消息条目。
    Message { entry_index: usize },
    /// error 条目（Diagnostic）。
    Error { entry_index: usize },
    /// tool activity 组：同 run 相邻的连续 ToolCall 条目。
    ToolGroup { entry_indices: Vec<usize> },
    /// 非终态 RunState 中间相位行（disabled 单行，不纳入摘要；Interrupted
    /// 无 fork 边界，同按本行处理）。
    RunPhase { entry_index: usize },
    /// run 终态摘要区域：终态条目 + 紧邻前文 tool 组（可无）。
    RunSummary {
        group: Option<Vec<usize>>,
        terminal: usize,
    },
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
    pub active_runs: Vec<ActiveRun>,
    pub active_run_started_at_ms: Option<u64>,
    pub resume: ResumeState,
    pub terminal: TerminalState,
    pub terminals: Vec<TerminalState>,
    snapshot_pendings: Vec<PendingApproval>,
    /// R3 Wave B：Blocked 会话（live 派生）。snapshot active_runs 不提供
    /// 终态，快照重建后清空（wire 无此信息，不伪造）；Replay 重放终态
    /// 事件可重新派生。
    blocked_sessions: BTreeSet<String>,
    /// R3 Wave B：unread 通道（独立于 SessionLiveStatus）。非 active
    /// session 的 Session-stream 活动事件记 unread；select_session 清除；
    /// 首连 / 快照重建不产生（无 last-seen 基线）。
    unread_sessions: BTreeSet<String>,
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

    pub fn apply_terminal_output(&mut self, terminal_session_id: &str, delta: &str) -> bool {
        if let Some(terminal) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id))
        {
            terminal.output.push_str(delta);
            // Replay 可能在当前 snapshot 之后补到 terminal 的历史输出。
            // exited/killed 等 snapshot 终态是更强事实，不能被旧输出复活。
            if terminal
                .runtime_state
                .as_deref()
                .is_none_or(|state| state == "running")
            {
                terminal.runtime_state = Some("running".into());
                terminal.availability = TerminalAvailability::Ready;
            }
            if self.terminal.session_id.as_deref() == Some(terminal_session_id) {
                self.terminal = terminal.clone();
                return true;
            }
            return false;
        }
        // TerminalOutput 可以先于 create 回执抵达。先按 id 缓存；在
        // TerminalCreated 给出权威 workspace 前不展示，避免任务切换期间串屏。
        let terminal = TerminalState {
            session_id: Some(terminal_session_id.to_string()),
            output: delta.to_string(),
            runtime_state: Some("running".into()),
            availability: TerminalAvailability::Ready,
            ..TerminalState::default()
        };
        self.terminals.push(terminal);
        false
    }

    pub fn apply_terminal_created(&mut self, workspace_id: String, terminal_session_id: String) {
        // 同 workspace 的无 id 占位只用于显示 create failure；成功后由真实
        // terminal 取代，避免占位在确定性选择中遮住新会话。
        self.terminals.retain(|terminal| {
            terminal.session_id.is_some()
                || terminal.workspace_id.as_deref() != Some(workspace_id.as_str())
        });
        let mut terminal = self
            .terminals
            .iter()
            .find(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id.as_str()))
            .cloned()
            .or_else(|| {
                (self.terminal.session_id.as_deref() == Some(terminal_session_id.as_str()))
                    .then(|| self.terminal.clone())
            })
            .unwrap_or_else(|| TerminalState {
                session_id: Some(terminal_session_id.clone()),
                ..TerminalState::default()
            });
        // create 回执只补身份与运行态；Host 可能先广播首段 shell prompt，
        // 这里若重置整状态会清掉已经到达的 output。
        terminal.workspace_id = Some(workspace_id.clone());
        terminal.runtime_state = Some("running".into());
        terminal.availability = TerminalAvailability::Ready;
        if let Some(existing) = self
            .terminals
            .iter_mut()
            .find(|existing| existing.session_id.as_deref() == Some(terminal_session_id.as_str()))
        {
            *existing = terminal.clone();
        } else {
            self.terminals.push(terminal.clone());
        }
        if self.active_workspace_id() == Some(workspace_id.as_str())
            || self.terminal.workspace_id.as_deref() == Some(workspace_id.as_str())
        {
            self.terminal = terminal;
        }
    }

    pub fn mark_terminal_ready(&mut self, terminal_session_id: &str) -> bool {
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.runtime_state = Some("running".into());
            terminal.availability = TerminalAvailability::Ready;
        })
    }

    /// ADR-045：live 终态事件与快照 state 同口径——runtime_state 记录
    /// exited/killed/failed，availability 诚实降级 stale（旧输出不再复活，
    /// 见 apply_terminal_output 的终态闸门）。
    pub fn apply_terminal_exited(
        &mut self,
        terminal_session_id: &str,
        reason: TerminalExitReason,
    ) -> bool {
        let state = match reason {
            TerminalExitReason::Exited => "exited",
            TerminalExitReason::Killed => "killed",
            TerminalExitReason::Failed => "failed",
        };
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.runtime_state = Some(state.into());
            terminal.availability = TerminalAvailability::Stale {
                reason: format!("terminal {state}"),
            };
        })
    }

    /// terminal_close 清理已退出终端的回执：Host 已注销（该路径无 live
    /// 事件），本地同步移除；当前终端被移除时回到 not started 占位。
    pub fn remove_terminal(&mut self, terminal_session_id: &str) -> bool {
        let existed = self
            .terminals
            .iter()
            .any(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id));
        if !existed {
            return false;
        }
        self.terminals
            .retain(|terminal| terminal.session_id.as_deref() != Some(terminal_session_id));
        if self.terminal.session_id.as_deref() == Some(terminal_session_id) {
            self.terminal = TerminalState::default();
        }
        true
    }

    pub fn mark_terminal_failed(
        &mut self,
        terminal_session_id: &str,
        reason: impl Into<String>,
    ) -> bool {
        let reason = reason.into();
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.mark_failed(reason.clone());
        })
    }

    /// write/resize 的瞬态失败归因：终端本体仍 running（Host 事实未变）
    /// 时不降级可用性——wire 无 live exit/failure 事件，一次 IO 失败不能
    /// 把可写终端锁死，报错交给调用方的 status_hint；非 running（含状态
    /// 未知）保持既有 Failed 归因。
    pub fn note_terminal_io_failed(
        &mut self,
        terminal_session_id: &str,
        reason: impl Into<String>,
    ) -> bool {
        let running = self
            .terminals
            .iter()
            .find(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id))
            .or_else(|| {
                (self.terminal.session_id.as_deref() == Some(terminal_session_id))
                    .then(|| &self.terminal)
            })
            .is_some_and(|terminal| terminal.runtime_state.as_deref() == Some("running"));
        if running {
            return false;
        }
        let reason = reason.into();
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.mark_failed(reason.clone());
        })
    }

    /// create 回执不带 cwd（wire 冻结）；成功后由 UI 把请求 cwd 补到新
    /// 终端上，避免本地显示退回默认 "."。
    pub fn apply_terminal_cwd(&mut self, terminal_session_id: &str, cwd: &str) -> bool {
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.cwd = cwd.to_string();
        })
    }

    /// terminal_create 尚无 terminal id，按请求 workspace 保存失败归属；
    /// 用户切回该 workspace 时仍能看到真实原因，且不会污染当前 workspace。
    pub fn mark_terminal_create_failed(&mut self, workspace_id: &str, reason: impl Into<String>) {
        let reason = reason.into();
        let mut failed = self
            .terminals
            .iter()
            .find(|terminal| {
                terminal.workspace_id.as_deref() == Some(workspace_id)
                    && terminal.session_id.is_none()
            })
            .cloned()
            .unwrap_or_else(|| TerminalState {
                workspace_id: Some(workspace_id.to_string()),
                ..TerminalState::default()
            });
        failed.mark_failed(reason);
        if let Some(existing) = self.terminals.iter_mut().find(|terminal| {
            terminal.workspace_id.as_deref() == Some(workspace_id) && terminal.session_id.is_none()
        }) {
            *existing = failed.clone();
        } else {
            self.terminals.push(failed.clone());
        }
        if self.active_workspace_id() == Some(workspace_id)
            || self.terminal.workspace_id.as_deref() == Some(workspace_id)
        {
            self.terminal = failed;
        }
    }

    pub fn apply_terminal_resize(
        &mut self,
        terminal_session_id: &str,
        columns: u16,
        rows: u16,
    ) -> bool {
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.columns = columns;
            terminal.rows = rows;
            terminal.resize_confirmed = true;
            terminal.runtime_state = Some("running".into());
            terminal.availability = TerminalAvailability::Ready;
        })
    }

    fn update_terminal(
        &mut self,
        terminal_session_id: &str,
        update: impl Fn(&mut TerminalState),
    ) -> bool {
        let mut found = false;
        if let Some(terminal) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.session_id.as_deref() == Some(terminal_session_id))
        {
            update(terminal);
            found = true;
            if self.terminal.session_id.as_deref() == Some(terminal_session_id) {
                self.terminal = terminal.clone();
            }
        } else if self.terminal.session_id.as_deref() == Some(terminal_session_id) {
            update(&mut self.terminal);
            self.terminals.push(self.terminal.clone());
            found = true;
        }
        found
    }

    pub fn select_terminal_for_workspace(&mut self, workspace_id: Option<&str>) -> bool {
        let current = self.terminal.session_id.as_deref();
        let selected = current
            .and_then(|id| {
                self.terminals.iter().find(|terminal| {
                    terminal.session_id.as_deref() == Some(id)
                        && terminal.workspace_id.as_deref() == workspace_id
                })
            })
            .cloned()
            .or_else(|| {
                self.terminals
                    .iter()
                    .filter(|terminal| terminal.workspace_id.as_deref() == workspace_id)
                    .min_by_key(|terminal| {
                        (
                            usize::from(terminal.runtime_state.as_deref() != Some("running")),
                            terminal.session_id.clone().unwrap_or_default(),
                        )
                    })
                    .cloned()
            })
            .unwrap_or_else(|| TerminalState {
                workspace_id: workspace_id.map(str::to_string),
                ..TerminalState::default()
            });
        let changed = self.terminal != selected;
        self.terminal = selected;
        changed
    }

    pub fn mark_terminals_stale(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        for terminal in &mut self.terminals {
            terminal.mark_stale(reason.clone());
        }
        self.terminal.mark_stale(reason);
    }

    fn restore_terminal_availability(&mut self) {
        for terminal in &mut self.terminals {
            terminal.availability = match terminal.runtime_state.as_deref() {
                Some("running") => TerminalAvailability::Ready,
                Some(state) => TerminalAvailability::Stale {
                    reason: format!("terminal {state}"),
                },
                None => TerminalAvailability::Stale {
                    reason: "terminal state unavailable".into(),
                },
            };
        }
        if let Some(current) = self
            .terminals
            .iter()
            .find(|terminal| terminal.session_id == self.terminal.session_id)
        {
            self.terminal = current.clone();
        }
    }

    /// Inspector 的 workspace 归属：有 active task 时只认该 task 的
    /// canonical workspace；无 active task 才回落到 snapshot 默认 workspace。
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

    /// 合并一页历史时间线（按 sequence 去重，保持 sequence 升序）。
    pub fn apply_timeline_page(&mut self, page: &TimelinePage) {
        for item in &page.items {
            // 条目语义（去重 / committed 替换 / tool 双键回填）走 protocol
            // reducer；这里只保留历史条目携带的 UI 态副作用。
            self.timeline.apply_item(item);
            match &item.kind {
                TimelineItemKind::RunCompleted
                | TimelineItemKind::RunCancelled
                | TimelineItemKind::RunFailed => {
                    // run 终态可证明该 run 不再有未决议审批；历史中的工具
                    // 完成 / 审批响应则可能属于同 run 的更早工具，不能据此
                    // 清除 snapshot 权威的当前 pending。
                    self.clear_pending_for_run(item.run_id.as_deref());
                }
                _ => {}
            }
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
        if let AppEvent::TerminalExited {
            terminal_session_id,
            reason,
            ..
        } = &envelope.payload
        {
            return self.apply_terminal_exited(terminal_session_id, *reason);
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

    /// timeline 条目 → 渲染行组装（F-08 §4.2）。纯函数，render 与 AX 共用
    /// 同一结果保证同源；每帧调用成本为条目数线性扫描。
    pub fn timeline_rows(&self) -> Vec<TimelineRow> {
        let entries = &self.timeline.entries;
        let mut rows = Vec::new();
        let mut ix = 0;
        while ix < entries.len() {
            match &entries[ix].kind {
                TimelineEntryKind::UserMessage { .. }
                | TimelineEntryKind::AssistantMessage { .. } => {
                    rows.push(TimelineRow::Message { entry_index: ix });
                    ix += 1;
                }
                TimelineEntryKind::Error(_) => {
                    rows.push(TimelineRow::Error { entry_index: ix });
                    ix += 1;
                }
                TimelineEntryKind::ToolCall { .. } => {
                    let run_id = entries[ix].run_id.clone();
                    let mut group = vec![ix];
                    ix += 1;
                    while ix < entries.len() {
                        let next = &entries[ix];
                        if !matches!(next.kind, TimelineEntryKind::ToolCall { .. })
                            || next.run_id != run_id
                        {
                            break;
                        }
                        group.push(ix);
                        ix += 1;
                    }
                    // 紧邻其后的 run 终态条目吸收该组为摘要区域；终态必须
                    // 与本组同 run（含 None==None 的未知 run 近邻），防止
                    // 跨 run 吞并（审查 P2）。
                    if ix < entries.len()
                        && is_run_terminal(&entries[ix])
                        && entries[ix].run_id == run_id
                    {
                        rows.push(TimelineRow::RunSummary {
                            group: Some(group),
                            terminal: ix,
                        });
                        ix += 1;
                    } else {
                        rows.push(TimelineRow::ToolGroup {
                            entry_indices: group,
                        });
                    }
                }
                TimelineEntryKind::RunState(_) => {
                    if is_run_terminal(&entries[ix]) {
                        rows.push(TimelineRow::RunSummary {
                            group: None,
                            terminal: ix,
                        });
                    } else {
                        rows.push(TimelineRow::RunPhase { entry_index: ix });
                    }
                    ix += 1;
                }
            }
        }
        rows
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

    /// MessageSent 本地乐观回显：wire 对 MessageCommitted 返回 None（用户
    /// 消息不进实时流），发送回执即上屏，重选 / 重连后由快照重放的持久化
    /// 行替换。只在 active session 追加；是否追加以返回值告知调用方 bump
    /// 时间线代次。禁止改 protocol 共享 reducer——这里直接 push。
    pub fn note_user_echo(
        &mut self,
        session_id: &str,
        run_id: &str,
        text: &str,
        now_ms: u64,
    ) -> bool {
        if self.active_session_id.as_deref() != Some(session_id) {
            return false;
        }
        // 借用当前最大 wire sequence（entries 升序）：不进 seen、不占号段，
        // 后续 wire 事件严格更大，insert_entry 有序插入自然落在 echo 之后；
        // 重复 wire 事件仍被 seen 去重，不会双插。entries 为空时兜底 0。
        let sequence = self
            .timeline
            .entries
            .last()
            .map(|entry| entry.sequence)
            .unwrap_or(0);
        self.timeline.entries.push(TimelineEntry {
            sequence,
            event_id: format!("local-echo-{run_id}"),
            kind: TimelineEntryKind::UserMessage {
                text: text.to_string(),
            },
            fork_boundary: None,
            timestamp: now_ms.to_string(),
            run_id: Some(run_id.to_string()),
        });
        true
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

fn parse_terminal_sessions(data: &Value) -> Vec<TerminalState> {
    match data {
        Value::Array(entries) => entries
            .iter()
            .filter_map(TerminalState::from_snapshot)
            .collect(),
        Value::Object(_) => TerminalState::from_snapshot(data).into_iter().collect(),
        _ => Vec::new(),
    }
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

fn parse_provider_status(data: &Value) -> Option<(String, String)> {
    let entry = data
        .as_array()
        .and_then(|entries| entries.first())
        .or(Some(data))?;
    let provider = entry.get("provider_id").and_then(Value::as_str)?;
    let model = entry.get("model").and_then(Value::as_str)?;
    Some((provider.to_string(), model.to_string()))
}

fn parse_pending_approvals(data: &Value) -> Vec<PendingApproval> {
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

/// run 终态判定（UI：清 active run 与审批卡）。时间线文案统一在 protocol
/// reducer 内（run_state_label）。
fn run_state_is_terminal(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
    )
}

/// timeline 行组装的 run 终态判定：reducer 的 fork_boundary 是唯一定义源
/// （历史 RunCompleted/Cancelled/Failed 与 live 对应态；Interrupted 无边
/// 界），禁止对 kind 文案做字符串匹配。
fn is_run_terminal(entry: &TimelineEntry) -> bool {
    entry.fork_boundary.is_some()
}

/// Failed 终态摘要原因：protocol reducer 历史臂（RunFailed）把 provider
/// 失败原因写进 RunState 标签 `run failed · {reason}`，live 臂（RunChanged）
/// 只有 `run failed` 无原因。此处仅剥离前缀取原因原文（原因内部再含
/// ` · ` 不受影响）；标签格式变化须与 protocol reducer 同批调整。
fn failed_run_reason(label: &str) -> Option<&str> {
    label
        .strip_prefix("run failed · ")
        .filter(|reason| !reason.is_empty())
}

/// Run 摘要卡内容（F-08 诚实文案）：无权威数据用通用描述，禁止编造
/// 耗时 / 数字；失败原因取 reducer 标签原文；非终态条目返回 None。
pub fn run_summary_texts(entry: &TimelineEntry) -> Option<(&'static str, String)> {
    match entry.fork_boundary {
        Some(ForkBoundary::Completed) => Some((
            "Ready for review",
            "The run finished. Review the changes from this turn.".to_string(),
        )),
        Some(ForkBoundary::Cancelled) => Some((
            "Run cancelled",
            "The run was cancelled. Output from this turn is preserved.".to_string(),
        )),
        // 失败摘要是唯一的失败原因出口（Error 仅来自 Diagnostic，RunFailed
        // 不产生 Error 条目）：有原因用原文，无原因 / 标签剥离失败走通用
        // 兜底，不指向不存在的"上方错误详情"。
        Some(ForkBoundary::Failed) => Some((
            "Run failed",
            match &entry.kind {
                TimelineEntryKind::RunState(label) => failed_run_reason(label)
                    .map(str::to_string)
                    .unwrap_or_else(|| "The run failed.".to_string()),
                _ => "The run failed.".to_string(),
            },
        )),
        None => None,
    }
}

/// Timeline 页脚终态词（§4.4：completed / cancelled / failed；非终态 None）。
pub fn run_footer_label(entry: &TimelineEntry) -> Option<&'static str> {
    match entry.fork_boundary {
        Some(ForkBoundary::Completed) => Some("Run completed"),
        Some(ForkBoundary::Cancelled) => Some("Run cancelled"),
        Some(ForkBoundary::Failed) => Some("Run failed"),
        None => None,
    }
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

    fn raw_entry(sequence: u64, kind: TimelineEntryKind, run_id: Option<&str>) -> TimelineEntry {
        TimelineEntry {
            sequence,
            event_id: format!("raw-{sequence}"),
            kind,
            fork_boundary: None,
            timestamp: "2000".into(),
            run_id: run_id.map(str::to_string),
        }
    }

    fn tool_entry(sequence: u64, run_id: &str, name: &str, status: &str) -> TimelineEntry {
        raw_entry(
            sequence,
            TimelineEntryKind::ToolCall {
                name: name.into(),
                status: status.into(),
                detail: None,
            },
            Some(run_id),
        )
    }

    fn terminal_entry(sequence: u64, boundary: ForkBoundary) -> TimelineEntry {
        let mut entry = raw_entry(
            sequence,
            TimelineEntryKind::RunState("run terminal".into()),
            Some("r-1"),
        );
        entry.fork_boundary = Some(boundary);
        entry
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
                "run:run started".to_string(),
                "assistant:Hello world".to_string(),
                "run:run completed".to_string()
            ]
        );
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
            "Task — tokens | Quota unavailable | — tok/s | Run 00:45"
        );
    }

    #[test]
    fn session_live_status_running_needs_input_priority_and_plain() {
        let snapshot = snapshot_with_runs_and_approvals(
            vec![
                json!({
                    "run_id": "r-run",
                    "session_id": "s-run",
                    "started_at_ms": 10_u64
                }),
                json!({
                    "run_id": "r-both",
                    "session_id": "s-both",
                    "started_at_ms": 11_u64
                }),
            ],
            vec![
                json!({
                    "run_id": "r-both",
                    "session_id": "s-both",
                    "tool_call_id": "c-both",
                    "tool_name": "write_file",
                    "message": "Approve workspace file write"
                }),
                json!({
                    "run_id": "r-wait",
                    "session_id": "s-wait",
                    "tool_call_id": "c-wait",
                    "tool_name": "bash",
                    "message": "Approve command"
                }),
            ],
        );
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        assert_eq!(
            projection.session_live_status("s-run"),
            Some(SessionLiveStatus::Running)
        );
        // 与 Running 并存时 Needs input 优先。
        assert_eq!(
            projection.session_live_status("s-both"),
            Some(SessionLiveStatus::NeedsInput)
        );
        assert_eq!(
            projection.session_live_status("s-wait"),
            Some(SessionLiveStatus::NeedsInput)
        );
        // 无 live 状态：不声明语义（空心灰圆）。
        assert_eq!(projection.session_live_status("s-idle"), None);

        // live ToolApprovalRequired 归属当时的 active session。
        projection.select_session("s-1");
        assert!(projection.apply_event(&event(
            1,
            json!({
                "type": "tool_approval_required",
                "data": {
                    "run_id": "r-live",
                    "tool_call_id": "c-live",
                    "reason": "bash · run.sh · Approve command"
                }
            }),
        )));
        assert_eq!(
            projection.session_live_status("s-1"),
            Some(SessionLiveStatus::NeedsInput)
        );

        // 无 session 归属字段的 snapshot pending 归 active session
        // （与 pending_for_active_session 同规）。
        let orphan: Snapshot = serde_json::from_value(json!({
            "instance_id": "instance-1",
            "snapshot_sequence": 0,
            "generated_at": 1,
            "sections": [
                {
                    "kind": "pending_tool_approvals",
                    "revision": 1,
                    "data": [
                        {
                            "run_id": "r-x",
                            "tool_call_id": "c-x",
                            "tool_name": "bash",
                            "message": "Approve command"
                        }
                    ]
                }
            ]
        }))
        .expect("decode Snapshot");
        let mut orphan_projection = DesktopProjection::from_snapshot(&orphan);
        assert_eq!(orphan_projection.session_live_status("s-any"), None);
        orphan_projection.select_session("s-1");
        assert_eq!(
            orphan_projection.session_live_status("s-1"),
            Some(SessionLiveStatus::NeedsInput)
        );
    }

    #[test]
    fn run_status_label_uses_final_order_and_vertical_separators() {
        let mut projection = DesktopProjection::default();
        assert_eq!(
            projection.run_status_label(0),
            "Task — tokens | Quota unavailable | — tok/s | Run idle"
        );
        // active run 缺权威起始时间：时长诚实显示 —，不编造 mm:ss。
        projection.active_run_id = Some("r-unknown-start".into());
        assert_eq!(
            projection.run_status_label(0),
            "Task — tokens | Quota unavailable | — tok/s | Run —"
        );
    }

    /// R3 Wave A 审查修复（P1）：live RunChanged 非终态登记 run 成员（含
    /// 非 active 的后台会话），终态按 run_id 移除并清 pendings——rail
    /// 状态点不假阴性也不陈旧残留。
    #[test]
    fn session_live_status_tracks_live_run_changed_membership() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        // live 非终态：active 会话登记 Running。
        assert!(projection.apply_event(&run_changed(1, "created")));
        assert_eq!(
            projection.session_live_status("s-1"),
            Some(SessionLiveStatus::Running)
        );
        // 后台会话的 RunChanged 同样登记（不过 active 闸门）。
        let background = serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": "app-2",
            "global_sequence": 2,
            "stream": { "type": "session", "id": "s-2" },
            "stream_sequence": 2,
            "timestamp": 1_002,
            "source": { "type": "core" },
            "payload": { "type": "run_changed", "data": { "run_id": "r-2", "state": "created" } }
        }))
        .expect("decode AppEventEnvelope");
        assert!(projection.apply_event(&background));
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::Running)
        );
        // 终态移除：蓝点不残留；同 run 的 pending 一并清除。
        assert!(projection.apply_event(&event(
            3,
            json!({
                "type": "tool_approval_required",
                "data": {
                    "run_id": "r-1",
                    "tool_call_id": "c-1",
                    "reason": "bash · run.sh · Approve command"
                }
            }),
        )));
        assert_eq!(
            projection.session_live_status("s-1"),
            Some(SessionLiveStatus::NeedsInput)
        );
        assert!(projection.apply_event(&run_changed(4, "completed")));
        assert_eq!(projection.session_live_status("s-1"), None);
        assert!(projection.snapshot_pendings.is_empty());
        // 后台会话终态同样清除（用 completed：failed / interrupted 会按
        // R3 Wave B 语义派生 Blocked，另行专项测试）。
        let background_done = serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": "app-5",
            "global_sequence": 5,
            "stream": { "type": "session", "id": "s-2" },
            "stream_sequence": 5,
            "timestamp": 1_005,
            "source": { "type": "core" },
            "payload": { "type": "run_changed", "data": { "run_id": "r-2", "state": "completed" } }
        }))
        .expect("decode AppEventEnvelope");
        assert!(projection.apply_event(&background_done));
        assert_eq!(projection.session_live_status("s-2"), None);
        assert!(projection.active_runs.is_empty());
    }

    #[test]
    fn note_session_run_marks_running_before_live_run_changed() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        projection.note_session_run("s-1", "r-1", 1_000);
        assert_eq!(
            projection.session_live_status("s-1"),
            Some(SessionLiveStatus::Running)
        );
        assert_eq!(projection.active_run_id.as_deref(), Some("r-1"));
        // 随后的 live RunChanged 不得重复登记。
        assert!(projection.apply_event(&run_changed(1, "created")));
        assert_eq!(
            projection
                .active_runs
                .iter()
                .filter(|run| run.run_id == "r-1")
                .count(),
            1
        );
        assert!(projection.apply_event(&run_changed(2, "completed")));
        assert_eq!(projection.session_live_status("s-1"), None);
    }

    /// R4 Wave B WS-4a：用户消息乐观回显——active session 回执即上屏，
    /// 后续 wire 事件严格落在 echo 之后；非 active 不产生行。
    #[test]
    fn note_user_echo_appends_active_then_wire_events_land_after() {
        // entries 为空的理论分支：sequence 兜底 0。
        let mut fresh = DesktopProjection::default();
        fresh.select_session("s-1");
        assert!(fresh.note_user_echo("s-1", "r-0", "first", 1_000));
        assert_eq!(fresh.timeline.len(), 1);
        assert_eq!(fresh.timeline[0].sequence, 0);

        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert!(projection.apply_event(&assistant_delta(4, "m-1", "before")));
        assert!(projection.note_user_echo("s-1", "r-2", "hello", 5_000));
        let echo = projection.timeline.last().expect("echo appended");
        // 借用最大 wire sequence，不占号段、不进 seen。
        assert_eq!(echo.sequence, 4);
        assert_eq!(echo.event_id, "local-echo-r-2");
        assert_eq!(echo.run_id.as_deref(), Some("r-2"));
        assert_eq!(echo.timestamp, "5000");
        assert!(matches!(
            &echo.kind,
            TimelineEntryKind::UserMessage { text } if text == "hello"
        ));
        // 后续 wire 事件（sequence 严格更大）有序插到 echo 之后。
        assert!(projection.apply_event(&run_changed(5, "created")));
        assert_eq!(projection.timeline.len(), 3);
        assert_eq!(
            projection
                .timeline
                .last()
                .expect("wire after echo")
                .event_id,
            "app-5"
        );
        // 非 active session（发送后已切走）不 echo：重放会补。
        assert!(!projection.note_user_echo("s-2", "r-3", "away", 6_000));
        assert_eq!(projection.timeline.len(), 3);
    }

    /// R4 Wave B 评审 P2 修复：早死路径（engine 未报终态）的合成
    /// RunChanged{Failed} 由宿主 publish_raw 分配 2^60 起的合成序号
    /// （crates/app gui_host SYNTHETIC_SEQUENCE_BASE，不占真实持久化号段），
    /// 有序插入落在用户消息乐观回显之后；seq-0 旧行为会插到时间线顶端。
    #[test]
    fn synthetic_terminal_after_user_echo_lands_at_bottom() {
        const SYNTHETIC_BASE: u64 = 1 << 60;
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert!(projection.apply_event(&assistant_delta(4, "m-1", "before")));
        assert!(projection.note_user_echo("s-1", "r-1", "blocked message", 5_000));
        assert!(projection.apply_event(&run_changed(SYNTHETIC_BASE, "failed")));
        assert_eq!(projection.timeline.len(), 3);
        assert_eq!(projection.timeline[0].event_id, "app-4");
        assert_eq!(projection.timeline[1].event_id, "local-echo-r-1");
        assert_eq!(
            projection.timeline[2].event_id,
            format!("app-{SYNTHETIC_BASE}")
        );
        assert!(matches!(
            &projection.timeline[2].kind,
            TimelineEntryKind::RunState(label) if label == "run failed"
        ));
        // 条目序列保持升序不变量（insert_entry 的 partition_point 前提）。
        assert!(
            projection.timeline[1].sequence <= projection.timeline[2].sequence,
            "entries must stay ascending by sequence: {:?}",
            projection
                .timeline
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn background_tool_approval_marks_needs_input_without_active_session() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        let background = serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": "app-2",
            "global_sequence": 2,
            "stream": { "type": "session", "id": "s-2" },
            "stream_sequence": 2,
            "timestamp": 1_002,
            "source": { "type": "core" },
            "payload": {
                "type": "tool_approval_required",
                "data": {
                    "run_id": "r-2",
                    "tool_call_id": "c-2",
                    "reason": "bash · run.sh · Approve command"
                }
            }
        }))
        .expect("decode AppEventEnvelope");
        assert!(projection.apply_event(&background));
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::NeedsInput)
        );
        assert_eq!(projection.session_live_status("s-1"), None);
        assert!(projection.pending_approval.is_none());
        let background_done = serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": "app-3",
            "global_sequence": 3,
            "stream": { "type": "session", "id": "s-2" },
            "stream_sequence": 3,
            "timestamp": 1_003,
            "source": { "type": "core" },
            "payload": {
                "type": "tool_completed",
                "data": {
                    "run_id": "r-2",
                    "tool_call_id": "c-2",
                    "success": true
                }
            }
        }))
        .expect("decode AppEventEnvelope");
        assert!(projection.apply_event(&background_done));
        assert_eq!(projection.session_live_status("s-2"), None);
    }

    /// R3 Wave B：Blocked live 派生——最近一条 RunChanged 为终态且
    /// failed / interrupted 记 Blocked；非终态与 completed / cancelled
    /// 清除；优先级 NeedsInput > Running > Blocked；快照重建清空、
    /// Replay 重放终态事件重新派生。
    #[test]
    fn session_live_status_blocked_derivation_and_clearing() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert_eq!(SessionLiveStatus::Blocked.label(), "Blocked");

        // 后台会话 failed / interrupted 终态 → Blocked。
        assert!(projection.apply_event(&session_event(
            1,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "failed" } }),
        )));
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::Blocked)
        );
        // 已 Blocked 的重复终态无成员增量，返回 false 是正确语义。
        projection.apply_event(&session_event(
            2,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "interrupted" } }),
        ));
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::Blocked)
        );

        // completed / cancelled 终态不算 Blocked（「最近一条」语义清除）。
        assert!(projection.apply_event(&session_event(
            3,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "completed" } }),
        )));
        assert_eq!(projection.session_live_status("s-2"), None);
        // 已清除后的再次非 Blocked 终态无增量，返回 false 是正确语义。
        projection.apply_event(&session_event(
            4,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-3", "state": "cancelled" } }),
        ));
        assert_eq!(projection.session_live_status("s-2"), None);

        // failed 后同 session 非终态 RunChanged 清除（新一轮 run 开始）。
        assert!(projection.apply_event(&session_event(
            5,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-4", "state": "failed" } }),
        )));
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::Blocked)
        );
        assert!(projection.apply_event(&session_event(
            6,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-5", "state": "created" } }),
        )));
        // 新 run 登记成员：Running（优先级高于 Blocked，且 blocked 已清）。
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::Running)
        );
        assert!(projection.apply_event(&session_event(
            6,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-5", "state": "completed" } }),
        )));
        assert_eq!(projection.session_live_status("s-2"), None);

        // 快照重建清空 blocked（wire 无终态来源，诚实）；Replay 重放终态
        // 事件可重新派生。
        assert!(projection.apply_event(&session_event(
            7,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-6", "state": "interrupted" } }),
        )));
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::Blocked)
        );
        let snapshot = snapshot_with_sessions(vec![session_entry("s-2", "Two", 20)]);
        projection.apply_snapshot_required(&snapshot);
        assert_eq!(projection.session_live_status("s-2"), None);
        assert!(projection.apply_replay(&[session_event(
            8,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-7", "state": "failed" } }),
        )]));
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::Blocked)
        );

        // 优先级：snapshot active run（Running）与 pending（NeedsInput）
        // 均压过 live 派生的 Blocked。
        let snapshot = snapshot_with_runs_and_approvals(
            vec![json!({
                "run_id": "r-run",
                "session_id": "s-run",
                "started_at_ms": 10_u64
            })],
            vec![json!({
                "run_id": "r-wait",
                "session_id": "s-wait",
                "tool_call_id": "c-wait",
                "tool_name": "bash",
                "message": "Approve command"
            })],
        );
        let mut priority = DesktopProjection::from_snapshot(&snapshot);
        assert!(priority.apply_event(&session_event(
            9,
            "s-run",
            json!({ "type": "run_changed", "data": { "run_id": "r-x", "state": "failed" } }),
        )));
        assert_eq!(
            priority.session_live_status("s-run"),
            Some(SessionLiveStatus::Running)
        );
        assert!(priority.apply_event(&session_event(
            10,
            "s-wait",
            json!({ "type": "run_changed", "data": { "run_id": "r-y", "state": "failed" } }),
        )));
        assert_eq!(
            priority.session_live_status("s-wait"),
            Some(SessionLiveStatus::NeedsInput)
        );
    }

    /// R3 Wave B：unread 通道——非 active session 的 Session-stream 活动
    /// 事件记 unread；active 自身活动不记；select_session 清除；首连 /
    /// 快照重建不产生（仍存标记保留、消失清除、新 session 无）；
    /// Replay 重放后台活动同样记 unread。
    #[test]
    fn session_unread_marks_background_activity_and_clears_on_select() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert!(!projection.session_unread("s-2"));

        // 拍板集合逐类事件：RunChanged / AssistantDelta / ToolStarted /
        // ToolOutput / ToolCompleted / Diagnostic。
        let activities = [
            json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "created" } }),
            json!({ "type": "assistant_delta", "data": { "run_id": "r-2", "message_id": "m-1", "delta": "hi" } }),
            json!({ "type": "tool_started", "data": { "run_id": "r-2", "tool_call_id": "c-1", "name": "fs_read" } }),
            json!({ "type": "tool_output", "data": { "run_id": "r-2", "tool_call_id": "c-1", "delta": "chunk", "truncated": false } }),
            json!({ "type": "tool_completed", "data": { "run_id": "r-2", "tool_call_id": "c-1", "success": true } }),
            json!({ "type": "diagnostic", "data": { "level": "info", "code": "sandbox.fallback", "message": "{}" } }),
        ];
        for (index, payload) in activities.into_iter().enumerate() {
            projection.apply_event(&session_event(index as u64 + 1, "s-2", payload));
            assert!(
                projection.session_unread("s-2"),
                "activity #{index} should keep unread"
            );
        }
        // active session 自身的活动不记 unread。
        assert!(projection.apply_event(&assistant_delta(20, "m-9", "active")));
        assert!(!projection.session_unread("s-1"));

        // select_session（打开 / 切换）清除；切走后新活动重新记 unread。
        projection.select_session("s-2");
        assert!(!projection.session_unread("s-2"));
        projection.select_session("s-1");
        assert!(projection.apply_event(&session_event(
            21,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-9", "state": "created" } }),
        )));
        assert!(projection.session_unread("s-2"));

        // 快照重建：仍存 session 的 unread 保留；新增 session（本地新建
        // 同走快照）不产生 unread；全新投影（首连）无 unread。
        let snapshot = snapshot_with_sessions(vec![
            session_entry("s-1", "One", 20),
            session_entry("s-2", "Two", 10),
        ]);
        projection.apply_snapshot_required(&snapshot);
        assert!(projection.session_unread("s-2"));
        assert!(!projection.session_unread("s-new"));
        let fresh = DesktopProjection::from_snapshot(&snapshot);
        assert!(!fresh.session_unread("s-2"));

        // Replay 重放后台活动同样记 unread（断线期间发生的事用户未看过）。
        let mut replayed = DesktopProjection::default();
        replayed.select_session("s-1");
        assert!(replayed.apply_replay(&[session_event(
            1,
            "s-2",
            json!({ "type": "assistant_delta", "data": { "run_id": "r-2", "message_id": "m-1", "delta": "while away" } }),
        )]));
        assert!(replayed.session_unread("s-2"));
    }

    /// R3 Wave B 导航回归：断线（Disconnected）不清 active_session_id /
    /// unread / blocked——连接态与导航态解耦，Reconnect 后可续。
    #[test]
    fn disconnect_preserves_active_unread_and_blocked() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert!(projection.apply_event(&session_event(
            1,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "failed" } }),
        )));
        assert!(projection.apply_event(&session_event(
            2,
            "s-3",
            json!({ "type": "assistant_delta", "data": { "run_id": "r-3", "message_id": "m-1", "delta": "bg" } }),
        )));
        projection.set_connection(ConnectionState::Disconnected {
            reason: "heartbeat timeout".into(),
        });
        assert_eq!(projection.active_session_id.as_deref(), Some("s-1"));
        assert_eq!(
            projection.session_live_status("s-2"),
            Some(SessionLiveStatus::Blocked)
        );
        assert!(projection.session_unread("s-3"));
        assert!(projection.show_reconnect());
    }

    /// R3 Wave B 导航回归：apply_snapshot_required 换基线——active 仍存
    /// 则保留并清其 unread、消失则置 None；消失 session 的 unread 清除、
    /// 仍存保留；blocked 清空（wire 无终态来源）。
    #[test]
    fn snapshot_required_keeps_active_clears_unread_and_prunes_vanished() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");
        assert!(projection.apply_event(&session_event(
            1,
            "s-2",
            json!({ "type": "run_changed", "data": { "run_id": "r-2", "state": "failed" } }),
        )));
        // unread 已记的后续活动事件无增量，返回 false 是正确语义。
        projection.apply_event(&session_event(
            2,
            "s-2",
            json!({ "type": "assistant_delta", "data": { "run_id": "r-2", "message_id": "m-1", "delta": "bg" } }),
        ));
        // 公开路径下 active 不产生 unread（select 即清）；直接置位以钉住
        // 「保留仍存 active 并清其 unread」这条拍板规则。
        projection.unread_sessions.insert("s-1".into());

        let keeps = snapshot_with_sessions(vec![
            session_entry("s-1", "One", 20),
            session_entry("s-new", "New", 10),
        ]);
        projection.apply_snapshot_required(&keeps);
        assert_eq!(projection.active_session_id.as_deref(), Some("s-1"));
        assert!(!projection.session_unread("s-1"));
        assert!(!projection.session_unread("s-2"));
        assert!(!projection.session_unread("s-new"));
        assert_eq!(projection.session_live_status("s-2"), None);

        // active 消失：置 None（UI 侧焦点回退 scope 触发器）。
        assert!(projection.apply_event(&session_event(
            3,
            "s-3",
            json!({ "type": "run_changed", "data": { "run_id": "r-3", "state": "created" } }),
        )));
        assert!(projection.session_unread("s-3"));
        let drops = snapshot_with_sessions(vec![session_entry("s-new", "New", 10)]);
        projection.apply_snapshot_required(&drops);
        assert_eq!(projection.active_session_id, None);
        assert!(!projection.session_unread("s-3"));
    }

    #[test]
    fn reconnect_shows_only_for_disconnected_or_failed() {
        let mut projection = DesktopProjection::default();
        projection.connection = ConnectionState::Connecting;
        assert!(!projection.show_reconnect());
        projection.connection = ConnectionState::Connected {
            instance_id: "i-1".into(),
        };
        assert!(!projection.show_reconnect());
        projection.connection = ConnectionState::Disconnected {
            reason: "heartbeat timeout".into(),
        };
        assert!(projection.show_reconnect());
        projection.connection = ConnectionState::Failed {
            reason: "no token".into(),
        };
        assert!(projection.show_reconnect());
    }

    #[test]
    fn workspace_empty_hint_requires_no_session_and_no_entries() {
        let mut projection = DesktopProjection::default();
        assert!(projection.workspace_empty_hint_visible());
        // 有 active session（即使条目尚未加载）不显示引导。
        projection.active_session_id = Some("s-1".into());
        assert!(!projection.workspace_empty_hint_visible());
        // Disconnected 保留旧条目时不显示引导。
        projection.active_session_id = None;
        projection.connection = ConnectionState::Disconnected {
            reason: "connection lost".into(),
        };
        projection.timeline.entries.push(TimelineEntry {
            sequence: 1,
            event_id: "e-1".into(),
            kind: TimelineEntryKind::UserMessage {
                text: "kept entries".into(),
            },
            fork_boundary: None,
            timestamp: "2026-08-27T00:00:00Z".into(),
            run_id: None,
        });
        assert!(!projection.workspace_empty_hint_visible());
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
        assert_eq!(
            projects.last().expect("unassigned").tasks[0].session_id,
            "s-orphan"
        );

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

    /// 与 `event` 相同，但 stream 指向给定 session/branch（wire 无 branch
    /// 字段，分支事件以分支自身的 stream id 表达）。
    fn session_event(sequence: u64, session: &str, payload: Value) -> AppEventEnvelope {
        serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": format!("app-{sequence}"),
            "global_sequence": sequence,
            "stream": { "type": "session", "id": session },
            "stream_sequence": sequence,
            "timestamp": 1_000 + sequence,
            "source": { "type": "core" },
            "payload": payload
        }))
        .expect("decode AppEventEnvelope")
    }

    #[test]
    fn switching_branch_within_session_resets_timeline_baseline() {
        // R6：切支沿用 select_session -> reset_baseline -> reload，不加 wire
        // 字段；同一 session 换 branch 也无条件清 entries/seen/anchors。
        let snapshot = snapshot_with_sessions(vec![json!({
            "session_id": "s-1",
            "title": "Branching session",
            "updated_at_ms": 20,
            "active_branch": "main",
            "workspace_id": "ws-default"
        })]);
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        projection.select_session("s-1");

        let delta = |sequence: u64, session: &str, message_id: &str, text: &str| {
            session_event(
                sequence,
                session,
                json!({
                    "type": "assistant_delta",
                    "data": { "run_id": "r-1", "message_id": message_id, "delta": text }
                }),
            )
        };
        let tool_started = |sequence: u64, session: &str| {
            session_event(
                sequence,
                session,
                json!({
                    "type": "tool_started",
                    "data": { "run_id": "r-1", "tool_call_id": "call-1", "name": "fs_read" }
                }),
            )
        };
        let tool_output = |sequence: u64, session: &str| {
            session_event(
                sequence,
                session,
                json!({
                    "type": "tool_output",
                    "data": {
                        "run_id": "r-1",
                        "tool_call_id": "call-1",
                        "delta": "chunk",
                        "truncated": false
                    }
                }),
            )
        };
        let run_completed = |sequence: u64, session: &str| {
            session_event(
                sequence,
                session,
                json!({ "type": "run_changed", "data": { "run_id": "r-1", "state": "completed" } }),
            )
        };

        // 基线：assistant committed tombstone、tool 锚点、run 终态边界。
        assert!(projection.apply_event(&delta(2, "s-1", "m-1", "Hello")));
        projection.apply_timeline_page(&page(
            vec![history_item(
                4,
                "assistant_message",
                json!({ "text": "Hello world" }),
            )],
            true,
        ));
        assert!(!projection.apply_event(&delta(3, "s-1", "m-1", " late")));
        assert!(projection.apply_event(&tool_started(10, "s-1")));
        assert!(projection.apply_event(&tool_output(11, "s-1")));
        assert!(projection.apply_event(&run_completed(12, "s-1")));
        assert_eq!(projection.timeline.len(), 3);
        assert!(projection.timeline[2].is_fork_boundary());

        // 同 session 换 branch：entries / seen / assistant / tool anchors 全清。
        // SessionForked 后 controller 以同一个 session_id 重新 open；active branch
        // 只存在 host/storage，不进 wire，因此这里必须用同 id 再次选中。
        projection.select_session("s-1");
        assert!(projection.timeline.is_empty());
        assert_eq!(projection.active_session_id.as_deref(), Some("s-1"));

        // seen 已清：同 sequence 重放不判重；tombstone 已清：同 message delta
        // 不再被吞；tool 锚点已清：重放重建并回填。
        assert!(projection.apply_event(&delta(2, "s-1", "m-1", "Hello")));
        assert!(projection.apply_event(&delta(3, "s-1", "m-1", " again")));
        assert!(projection.apply_event(&tool_started(10, "s-1")));
        assert!(projection.apply_event(&tool_output(11, "s-1")));
        assert!(projection.apply_event(&run_completed(12, "s-1")));
        assert_eq!(projection.timeline.len(), 3);
        let texts: Vec<String> = projection
            .timeline
            .iter()
            .map(|entry| match &entry.kind {
                TimelineEntryKind::AssistantMessage { text } => format!("assistant:{text}"),
                TimelineEntryKind::ToolCall { detail, .. } => format!("tool:{detail:?}"),
                TimelineEntryKind::RunState(state) => format!("run:{state}"),
                other => format!("other:{other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                "assistant:Hello again".to_string(),
                "tool:Some(\"chunk\")".to_string(),
                "run:run completed".to_string(),
            ]
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

    fn terminal_exited(sequence: u64, terminal: &str, reason: &str) -> AppEventEnvelope {
        serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 3 },
            "instance_id": "instance-1",
            "event_id": format!("term-exit-{sequence}"),
            "global_sequence": sequence,
            "stream": { "type": "terminal", "id": terminal },
            "stream_sequence": sequence,
            "timestamp": 1_000 + sequence,
            "source": { "type": "core" },
            "payload": {
                "type": "terminal_exited",
                "data": {
                    "terminal_session_id": terminal,
                    "exit_code": 0,
                    "reason": reason
                }
            }
        }))
        .expect("decode TerminalExited")
    }

    /// ADR-045：live 终态事件即时刷新（不等断连重连快照），且与快照终态
    /// 同口径——旧输出不得复活终态终端。
    #[test]
    fn terminal_exited_event_marks_terminal_stale_and_blocks_resurrection() {
        let mut projection = DesktopProjection::default();
        projection.workspace_id = Some("ws-a".into());
        projection.apply_terminal_created("ws-a".into(), "term-a".into());
        assert_eq!(
            projection.terminal.runtime_state.as_deref(),
            Some("running")
        );

        assert!(projection.apply_event(&terminal_exited(1, "term-a", "killed")));
        assert_eq!(projection.terminal.runtime_state.as_deref(), Some("killed"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Stale { .. }
        ));
        // 迟到输出仍追加（保留现场），但不得复活 running/Ready。
        assert!(projection.apply_event(&terminal_output(2, "term-a", "late")));
        assert_eq!(projection.terminal.runtime_state.as_deref(), Some("killed"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Stale { .. }
        ));
    }

    /// ADR-045：Close 清理回执后本地移除条目；当前终端回到 not started。
    #[test]
    fn remove_terminal_clears_current_terminal_after_close() {
        let mut projection = DesktopProjection::default();
        projection.workspace_id = Some("ws-a".into());
        projection.apply_terminal_created("ws-a".into(), "term-a".into());
        projection.apply_event(&terminal_exited(1, "term-a", "exited"));

        assert!(!projection.remove_terminal("term-unknown"));
        assert!(projection.remove_terminal("term-a"));
        assert!(projection.terminals.is_empty());
        assert_eq!(projection.terminal.session_id, None);
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Stale { .. }
        ));
    }

    #[test]
    fn terminal_output_appends_without_vt100() {
        let mut projection = DesktopProjection::default();
        assert!(!projection.apply_event(&terminal_output(1, "term-1", "hello")));
        assert!(!projection.apply_event(&terminal_output(2, "term-1", "\nworld")));
        assert_eq!(projection.terminal.session_id, None);
        assert_eq!(projection.terminals[0].output, "hello\nworld");
        assert!(!projection.apply_event(&terminal_output(3, "term-other", "nope")));
        assert!(projection.terminal.output.is_empty());
    }

    #[test]
    fn terminal_created_preserves_output_that_arrived_before_receipt() {
        let mut projection = DesktopProjection::default();
        projection.workspace_id = Some("ws-a".into());
        assert!(!projection.apply_event(&terminal_output(1, "term-a", "shell$ ")));
        assert!(projection.terminal.output.is_empty());
        projection.apply_terminal_created("ws-a".into(), "term-a".into());
        assert_eq!(projection.terminal.output, "shell$ ");
        assert_eq!(projection.terminals[0].output, "shell$ ");
        assert_eq!(projection.terminal.workspace_id.as_deref(), Some("ws-a"));
        assert_eq!(
            projection.terminal.availability,
            TerminalAvailability::Ready
        );
    }

    #[test]
    fn terminal_output_waits_for_workspace_receipt_before_becoming_visible() {
        let mut projection = DesktopProjection::default();
        projection.workspace_id = Some("ws-b".into());
        assert!(!projection.apply_event(&terminal_output(1, "term-a", "shell$ ")));
        assert_eq!(projection.terminal.workspace_id.as_deref(), None);

        projection.apply_terminal_created("ws-a".into(), "term-a".into());
        assert_eq!(projection.terminal.session_id, None);
        projection.select_terminal_for_workspace(Some("ws-a"));
        assert_eq!(projection.terminal.session_id.as_deref(), Some("term-a"));
        assert_eq!(projection.terminal.output, "shell$ ");
    }

    #[test]
    fn terminal_selection_prefers_current_then_uses_deterministic_fallback() {
        let mut projection = DesktopProjection::default();
        let terminal = |id: &str| TerminalState {
            session_id: Some(id.into()),
            workspace_id: Some("ws-a".into()),
            runtime_state: Some("running".into()),
            availability: TerminalAvailability::Ready,
            ..TerminalState::default()
        };
        projection.terminals = vec![terminal("term-b"), terminal("term-a")];
        projection.terminal = terminal("term-b");
        assert!(!projection.select_terminal_for_workspace(Some("ws-a")));
        assert_eq!(projection.terminal.session_id.as_deref(), Some("term-b"));

        projection.terminal = terminal("term-other");
        projection.terminal.workspace_id = Some("ws-b".into());
        assert!(projection.select_terminal_for_workspace(Some("ws-a")));
        assert_eq!(projection.terminal.session_id.as_deref(), Some("term-a"));
    }

    #[test]
    fn terminal_snapshot_parses_all_fields_and_selects_active_workspace() {
        let snapshot: Snapshot = serde_json::from_value(json!({
            "instance_id": "instance-1", "snapshot_sequence": 0, "generated_at": 1,
            "sections": [
                { "kind": "workspaces", "revision": 1, "data": [
                    { "id": "ws-a", "name": "A" }, { "id": "ws-b", "name": "B" }
                ]},
                { "kind": "session_tree", "revision": 1, "data": [
                    { "session_id": "s-b", "title": "B task", "updated_at_ms": 1,
                      "workspace_id": "ws-b" }
                ]},
                { "kind": "terminal_sessions", "revision": 2, "data": [
                    { "terminal_session_id": "term-a", "owner_session": "ws-a",
                      "state": "running", "columns": 120, "rows": 40, "dropped_events": 3 },
                    { "terminal_session_id": "term-b", "owner_session": "ws-b",
                      "state": "exited", "columns": 90, "rows": 30, "dropped_events": 0 }
                ]}
            ]
        }))
        .expect("terminal snapshot");
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        assert_eq!(projection.terminals.len(), 2);
        assert_eq!(projection.terminal.session_id.as_deref(), Some("term-a"));
        assert_eq!(
            (projection.terminal.columns, projection.terminal.rows),
            (120, 40)
        );
        assert_eq!(projection.terminal.dropped_events, 3);
        assert_eq!(
            projection.terminal.availability,
            TerminalAvailability::Ready
        );
        projection.select_session("s-b");
        assert_eq!(projection.active_workspace_id(), Some("ws-b"));
        assert_eq!(projection.terminal.session_id.as_deref(), Some("term-b"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Stale { .. }
        ));
    }

    /// G3：快照恢复解析 Host 回报的 workspace 相对 cwd；缺键（旧 Host /
    /// 记账缺失）时诚实显示 unknown，不臆造工作区根 "."。
    #[test]
    fn terminal_snapshot_restores_cwd_or_shows_unknown() {
        let with_cwd = TerminalState::from_snapshot(&json!({
            "terminal_session_id": "term-a",
            "owner_session": "ws-a",
            "state": "running",
            "columns": 80,
            "rows": 24,
            "cwd": "src/app"
        }))
        .expect("terminal with cwd");
        assert_eq!(with_cwd.cwd, "src/app");

        let without_cwd = TerminalState::from_snapshot(&json!({
            "terminal_session_id": "term-b",
            "owner_session": "ws-a",
            "state": "running"
        }))
        .expect("terminal without cwd");
        assert_eq!(without_cwd.cwd, TERMINAL_CWD_UNKNOWN);

        let empty_cwd = TerminalState::from_snapshot(&json!({
            "terminal_session_id": "term-c",
            "owner_session": "ws-a",
            "state": "running",
            "cwd": ""
        }))
        .expect("terminal with empty cwd");
        assert_eq!(empty_cwd.cwd, TERMINAL_CWD_UNKNOWN);
    }

    /// G2：write/resize 瞬态失败不把 running 终端锁死（可用性保持
    /// Ready，报错走 status_hint）；非 running 终端保留 Failed 归因。
    #[test]
    fn terminal_io_failure_keeps_running_terminal_operable() {
        let mut projection = DesktopProjection::default();
        projection.workspace_id = Some("ws-a".into());
        projection.apply_terminal_created("ws-a".into(), "term-a".into());

        assert!(!projection.note_terminal_io_failed("term-a", "transient write error"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Ready
        ));

        projection.terminals[0].runtime_state = Some("exited".into());
        projection.terminal.runtime_state = Some("exited".into());
        assert!(projection.note_terminal_io_failed("term-a", "io error after exit"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Failed { .. }
        ));
    }

    #[test]
    fn up_to_date_snapshot_keeps_timeline_and_terminal_exit_beats_replayed_output() {
        let initial: Snapshot = serde_json::from_value(json!({
            "instance_id": "instance-1", "snapshot_sequence": 1, "generated_at": 1,
            "sections": [
                { "kind": "workspaces", "revision": 1, "data": [
                    { "id": "ws-a", "name": "A" }
                ]},
                { "kind": "session_tree", "revision": 1, "data": [
                    { "session_id": "s-1", "title": "A task", "updated_at_ms": 1,
                      "workspace_id": "ws-a" }
                ]},
                { "kind": "terminal_sessions", "revision": 1, "data": [
                    { "terminal_session_id": "term-a", "owner_session": "ws-a",
                      "state": "running", "columns": 80, "rows": 24 }
                ]}
            ]
        }))
        .expect("initial terminal snapshot");
        let mut projection = DesktopProjection::from_snapshot(&initial);
        projection.select_session("s-1");
        assert!(projection.apply_event(&run_changed(1, "created")));
        let timeline_len = projection.timeline.len();
        projection.set_connection(ConnectionState::Disconnected {
            reason: "socket closed".into(),
        });

        let exited: Snapshot = serde_json::from_value(json!({
            "instance_id": "instance-1", "snapshot_sequence": 1, "generated_at": 2,
            "sections": [
                { "kind": "workspaces", "revision": 2, "data": [
                    { "id": "ws-a", "name": "A" }
                ]},
                { "kind": "session_tree", "revision": 2, "data": [
                    { "session_id": "s-1", "title": "A task", "updated_at_ms": 1,
                      "workspace_id": "ws-a" }
                ]},
                { "kind": "terminal_sessions", "revision": 2, "data": [
                    { "terminal_session_id": "term-a", "owner_session": "ws-a",
                      "state": "exited", "columns": 80, "rows": 24 }
                ]}
            ]
        }))
        .expect("exited terminal snapshot");
        let apply = projection.apply_resume_outcome(
            &resume_outcome(
                ResumeDisposition::UpToDate {
                    current_sequence: pawork_client::GlobalSequence(1),
                },
                Vec::new(),
                None,
            ),
            &exited,
        );
        assert_eq!(apply, ResumeApply::Unchanged);
        assert_eq!(projection.timeline.len(), timeline_len);
        assert_eq!(projection.terminal.runtime_state.as_deref(), Some("exited"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Stale { .. }
        ));

        assert!(projection.apply_event(&terminal_output(2, "term-a", "late output")));
        assert_eq!(projection.terminal.output, "late output");
        assert_eq!(projection.terminal.runtime_state.as_deref(), Some("exited"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Stale { .. }
        ));
    }

    #[test]
    fn terminal_disconnect_and_failure_are_honest_states() {
        let mut projection = DesktopProjection::default();
        projection.workspace_id = Some("ws-a".into());
        projection.apply_terminal_created("ws-a".into(), "term-a".into());
        assert!(!projection.terminal.resize_confirmed);
        assert!(projection.apply_terminal_resize("term-a", 100, 30));
        assert!(projection.terminal.resize_confirmed);
        assert_eq!(
            (projection.terminal.columns, projection.terminal.rows),
            (100, 30)
        );
        assert!(projection.mark_terminal_failed("term-a", "write denied"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Failed { .. }
        ));
        assert!(matches!(
            projection.terminals[0].availability,
            TerminalAvailability::Failed { .. }
        ));
        projection.set_connection(ConnectionState::Disconnected {
            reason: "socket closed".into(),
        });
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Stale { .. }
        ));
        projection.set_connection(ConnectionState::Connected {
            instance_id: "instance-1".into(),
        });
        assert_eq!(
            projection.terminal.availability,
            TerminalAvailability::Ready
        );

        projection.mark_terminal_create_failed("ws-b", "policy denied");
        assert_eq!(projection.terminal.workspace_id.as_deref(), Some("ws-a"));
        projection.select_terminal_for_workspace(Some("ws-b"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Failed { .. }
        ));
        projection.apply_terminal_created("ws-b".into(), "term-b".into());
        assert_eq!(projection.terminal.session_id.as_deref(), Some("term-b"));
        assert!(matches!(
            projection.terminal.availability,
            TerminalAvailability::Ready
        ));
        assert_eq!(
            projection
                .terminals
                .iter()
                .filter(|terminal| terminal.workspace_id.as_deref() == Some("ws-b"))
                .count(),
            1
        );
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
                TimelineEntryKind::ToolCall {
                    name,
                    status,
                    detail,
                } => Some((name.as_str(), status.as_str(), detail.as_deref())),
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
                history_item(2, "approval_responded", json!({ "status": "approve_once" })),
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
            labels
                .iter()
                .any(|text| text.contains("approval requested") && text.contains("write_file")),
            "history approval_requested should remain, got {labels:?}"
        );
        assert!(
            labels
                .iter()
                .any(|text| text.contains("approval approve_once")),
            "history approval_responded should remain, got {labels:?}"
        );
        assert!(projection.pending_approval.is_none());
    }

    #[test]
    fn timeline_repagination_keeps_outstanding_pending_approval() {
        // D3：重开会话重放历史时，其它 run 的 approval_responded /
        // tool_completed 不能改写 snapshot 权威的未决议审批（含
        // tool_call_id，供冻结 tool_approve 使用）。
        let snapshot = snapshot_with_runs_and_approvals(
            vec![json!({
                "run_id": "r-2",
                "session_id": "s-1",
                "started_at_ms": 20_u64
            })],
            vec![json!({
                "run_id": "r-2",
                "session_id": "s-1",
                "tool_call_id": "call-2",
                "tool_name": "run_command",
                "message": "Approve command"
            })],
        );
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        projection.select_session("s-1");
        assert!(projection.pending_approval.is_some());
        projection.apply_timeline_page(&page(
            vec![
                history_item(
                    1,
                    "approval_requested",
                    json!({ "tool_name": "run_command" }),
                ),
                history_item(2, "approval_responded", json!({ "status": "approve_once" })),
                history_item(
                    3,
                    "tool_completed",
                    json!({ "tool_name": "run_command", "status": "succeeded" }),
                ),
                history_item(
                    4,
                    "approval_requested",
                    json!({ "tool_name": "run_command", "run_id": "r-2" }),
                ),
            ],
            true,
        ));
        let pending = projection
            .pending_approval
            .as_ref()
            .expect("outstanding approval must survive timeline repagination");
        assert_eq!(pending.run_id, "r-2");
        assert_eq!(pending.tool_call_id, "call-2");
    }

    #[test]
    fn timeline_earlier_items_in_same_run_keep_later_pending_approval() {
        // 同一 run 可串行执行多个工具：更早工具的 responded/completed
        // 历史条目不能清除 snapshot 中更晚工具的当前审批。历史 wire 的
        // approval_responded 不含 tool_call_id，无法安全做工具级清除。
        let snapshot = snapshot_with_runs_and_approvals(
            vec![],
            vec![json!({
                "run_id": "r-1",
                "session_id": "s-1",
                "tool_call_id": "call-2",
                "tool_name": "run_command",
                "message": "Approve command"
            })],
        );
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        projection.select_session("s-1");
        assert!(projection.pending_approval.is_some());
        projection.apply_timeline_page(&page(
            vec![
                history_item(1, "approval_responded", json!({ "status": "approve_once" })),
                history_item(
                    2,
                    "tool_completed",
                    json!({ "tool_name": "read_file", "status": "succeeded" }),
                ),
                history_item(3, "approval_requested", json!({ "tool_name": "run_command" })),
            ],
            true,
        ));
        let pending = projection
            .pending_approval
            .as_ref()
            .expect("later pending approval in the same run must survive history replay");
        assert_eq!(pending.run_id, "r-1");
        assert_eq!(pending.tool_call_id, "call-2");
    }

    /// R1 Wave B Phase C：读取 `fixtures/ui/expected/snapshot.json`（由
    /// `ui_fixture snapshot-dump` 生成的归一化 golden，再生步骤见
    /// `fixtures/ui/README.md`），断言 DesktopProjection 分组与状态。
    #[test]
    fn ui_fixture_expected_snapshot_rebuilds_groups_and_status() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/ui/expected/snapshot.json"
        );
        let raw = std::fs::read_to_string(path).unwrap_or_else(|error| {
            panic!("读取 {path} 失败（{error}）：golden 由 ui_fixture snapshot-dump 生成，再生步骤见 fixtures/ui/README.md")
        });
        let snapshot: Snapshot = serde_json::from_str(&raw).expect("decode expected snapshot");
        // FIXTURE_NOW_MS 锚点恰为 UTC 午夜；取锚点前 1ms 作参照 now，
        // 使 seed 中 -2h/-2.5h 同日偏移落 Today、四桶齐全（与 app 侧
        // tests/ui_fixture_projection.rs 同一分桶口径）。
        let now_ms = 1_767_225_599_999_u64;

        let mut projection = DesktopProjection::from_snapshot(&snapshot);

        // 会话清单：7 个种子会话全量恢复，最新在前，绑定各自 workspace。
        assert_eq!(projection.sessions.len(), 7);
        assert!(projection.sessions.iter().all(|session| session.active));
        let ids: BTreeSet<&str> = projection
            .sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "fx-ses-alpha-today",
                "fx-ses-alpha-yesterday",
                "fx-ses-beta-pending",
                "fx-ses-beta-toolfailed",
                "fx-ses-beta-cancelled",
                "fx-ses-alpha-longtitle",
                "fx-ses-beta-long",
            ]
            .into_iter()
            .collect::<BTreeSet<_>>()
        );
        assert_eq!(projection.sessions[0].session_id, "fx-ses-alpha-today");
        assert_eq!(projection.sessions[0].title, "Refactor launcher tabs");
        let long_title = projection
            .sessions
            .iter()
            .find(|session| session.session_id == "fx-ses-alpha-longtitle")
            .expect("long title session");
        assert!(long_title.title.chars().count() >= 200);
        // 嵌套 fn 而非闭包：返回引用派生自引用参数时，闭包的 Fn 签名
        // 只能固定单一生命周期（error: lifetime may not live long enough），
        // fn 的省略生命周期天然 higher-ranked。
        fn session_workspace<'a>(projection: &'a DesktopProjection, id: &str) -> Option<&'a str> {
            projection
                .sessions
                .iter()
                .find(|session| session.session_id == id)
                .and_then(|session| session.workspace_id.as_deref())
        }
        assert_eq!(
            session_workspace(&projection, "fx-ses-alpha-today"),
            Some("fx-alpha-app")
        );
        assert_eq!(
            session_workspace(&projection, "fx-ses-beta-pending"),
            Some("fx-beta-lib")
        );

        // TaskRail 分组：日期四桶齐全，桶内会话集合与 seed offsets 一致。
        let timeline = projection.timeline_groups(None, now_ms);
        assert_eq!(
            timeline
                .iter()
                .map(|group| group.bucket)
                .collect::<Vec<_>>(),
            vec![
                DateBucket::Today,
                DateBucket::Yesterday,
                DateBucket::Previous7Days,
                DateBucket::Earlier,
            ]
        );
        fn ids_of(group: &TaskRailDateGroup) -> Vec<&str> {
            let mut ids: Vec<&str> = group
                .projects
                .iter()
                .flat_map(|project| project.tasks.iter().map(|task| task.session_id.as_str()))
                .collect();
            ids.sort_unstable();
            ids
        }
        assert_eq!(
            ids_of(&timeline[0]),
            vec!["fx-ses-alpha-today", "fx-ses-beta-pending"]
        );
        assert_eq!(ids_of(&timeline[1]), vec!["fx-ses-alpha-yesterday"]);
        assert_eq!(
            ids_of(&timeline[2]),
            vec!["fx-ses-beta-long", "fx-ses-beta-toolfailed"]
        );
        assert_eq!(
            ids_of(&timeline[3]),
            vec!["fx-ses-alpha-longtitle", "fx-ses-beta-cancelled"]
        );

        // Today 桶内项目分组：按最新活动排序；wire workspaces 段当前只携带
        // 主 workspace，beta 组名回退 id（诚实回退，不臆造名字）。
        let today = &timeline[0];
        assert_eq!(today.projects.len(), 2);
        assert_eq!(
            today.projects[0].workspace_id.as_deref(),
            Some("fx-alpha-app")
        );
        assert_eq!(today.projects[0].name, "alpha-app");
        assert_eq!(
            today.projects[0]
                .tasks
                .iter()
                .map(|task| task.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["fx-ses-alpha-today"]
        );
        assert_eq!(
            today.projects[1].workspace_id.as_deref(),
            Some("fx-beta-lib")
        );
        assert_eq!(today.projects[1].name, "fx-beta-lib");
        assert_eq!(
            today.projects[1]
                .tasks
                .iter()
                .map(|task| task.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["fx-ses-beta-pending"]
        );

        // Projects 分组：alpha 3 个任务、beta 4 个；gamma 无会话（空项目态），
        // 无 Unassigned。
        let projects = projection.project_groups(None);
        assert_eq!(
            projects
                .iter()
                .map(|project| (project.workspace_id.as_deref(), project.task_count()))
                .collect::<Vec<_>>(),
            vec![(Some("fx-alpha-app"), 3), (Some("fx-beta-lib"), 4)]
        );
        assert!(!projects.iter().any(|project| project.is_unassigned()));

        // 状态：provider 快照恢复；pending 审批卡随会话选择出现/消失；
        // 纯 seed 数据无 live run。
        assert_eq!(
            projection
                .selected_model
                .as_ref()
                .map(|(provider, model)| (provider.as_str(), model.as_str())),
            Some(("mock", "fixture-model"))
        );
        assert!(projection.active_runs.is_empty());
        assert_eq!(projection.active_run_id, None);
        assert_eq!(projection.pending_approval, None);
        projection.select_session("fx-ses-beta-pending");
        let pending = projection
            .pending_approval
            .as_ref()
            .expect("pending approval restored from snapshot");
        assert_eq!(pending.tool_call_id, "call-fx-ses-beta-pending-0-0");
        assert_eq!(pending.tool_name, "write_file");
        assert!(pending.reason.contains("src/lib.ts"));
        projection.select_session("fx-ses-alpha-today");
        assert_eq!(projection.pending_approval, None);
    }

    #[test]
    fn timeline_rows_group_adjacent_tools_and_absorb_into_summary() {
        let mut projection = DesktopProjection::default();
        projection.timeline.entries = vec![
            raw_entry(
                1,
                TimelineEntryKind::UserMessage { text: "go".into() },
                Some("r-1"),
            ),
            tool_entry(2, "r-1", "read_file", "succeeded"),
            tool_entry(3, "r-1", "edit_file", "succeeded"),
            // 同 run 终态紧邻 → 吸收该组为摘要区域。
            terminal_entry(4, ForkBoundary::Completed),
            // 不同 run 的 tool 不被跨 run 终态吞并（审查 P2 防护）。
            tool_entry(5, "r-2", "bash", "succeeded"),
            terminal_entry(6, ForkBoundary::Completed),
        ];
        let rows = projection.timeline_rows();
        assert_eq!(
            rows,
            vec![
                TimelineRow::Message { entry_index: 0 },
                TimelineRow::RunSummary {
                    group: Some(vec![1, 2]),
                    terminal: 3,
                },
                TimelineRow::ToolGroup {
                    entry_indices: vec![4]
                },
                TimelineRow::RunSummary {
                    group: None,
                    terminal: 5,
                },
            ]
        );

        // 不同 run 的相邻 tool 不并组。
        let mut projection = DesktopProjection::default();
        projection.timeline.entries = vec![
            tool_entry(1, "r-1", "read_file", "succeeded"),
            tool_entry(2, "r-2", "bash", "running"),
            tool_entry(3, "r-2", "edit_file", "succeeded"),
        ];
        let rows = projection.timeline_rows();
        assert_eq!(
            rows,
            vec![
                TimelineRow::ToolGroup {
                    entry_indices: vec![0]
                },
                TimelineRow::ToolGroup {
                    entry_indices: vec![1, 2],
                },
            ]
        );
    }

    #[test]
    fn timeline_rows_terminal_without_group_and_phases_stay_single() {
        let mut projection = DesktopProjection::default();
        projection.timeline.entries = vec![
            raw_entry(
                1,
                TimelineEntryKind::AssistantMessage { text: "hi".into() },
                Some("r-1"),
            ),
            raw_entry(
                2,
                TimelineEntryKind::RunState("run streaming_response".into()),
                Some("r-1"),
            ),
            raw_entry(
                3,
                TimelineEntryKind::RunState("approval approved".into()),
                Some("r-1"),
            ),
            terminal_entry(4, ForkBoundary::Failed),
        ];
        let rows = projection.timeline_rows();
        assert_eq!(
            rows,
            vec![
                TimelineRow::Message { entry_index: 0 },
                TimelineRow::RunPhase { entry_index: 1 },
                TimelineRow::RunPhase { entry_index: 2 },
                TimelineRow::RunSummary {
                    group: None,
                    terminal: 3,
                },
            ]
        );
    }

    #[test]
    fn run_summary_and_footer_texts_map_terminal_boundaries_only() {
        let completed = terminal_entry(1, ForkBoundary::Completed);
        assert_eq!(
            run_summary_texts(&completed),
            Some((
                "Ready for review",
                "The run finished. Review the changes from this turn.".to_string()
            ))
        );
        assert_eq!(run_footer_label(&completed), Some("Run completed"));
        assert_eq!(
            run_footer_label(&terminal_entry(2, ForkBoundary::Cancelled)),
            Some("Run cancelled")
        );
        assert_eq!(
            run_footer_label(&terminal_entry(3, ForkBoundary::Failed)),
            Some("Run failed")
        );
        // 非终态（含 Interrupted：无 fork 边界）不产生摘要 / 页脚。
        let phase = raw_entry(
            4,
            TimelineEntryKind::RunState("run interrupted".into()),
            Some("r-1"),
        );
        assert_eq!(run_summary_texts(&phase), None);
        assert_eq!(run_footer_label(&phase), None);
    }

    #[test]
    fn failed_run_summary_description_reports_real_reason() {
        let failed_entry = |sequence: u64, label: &str| {
            let mut entry = raw_entry(
                sequence,
                TimelineEntryKind::RunState(label.into()),
                Some("r-1"),
            );
            entry.fork_boundary = Some(ForkBoundary::Failed);
            entry
        };
        // 有原因：摘要卡显示原因原文；原因内部再含分隔符只剥一次前缀。
        assert_eq!(
            run_summary_texts(&failed_entry(1, "run failed · provider timeout")),
            Some(("Run failed", "provider timeout".to_string()))
        );
        assert_eq!(
            run_summary_texts(&failed_entry(2, "run failed · a · b")),
            Some(("Run failed", "a · b".to_string()))
        );
        // 无原因（live 臂标签）：兜底通用失败文案，不指向不存在的错误详情。
        assert_eq!(
            run_summary_texts(&failed_entry(3, "run failed")),
            Some(("Run failed", "The run failed.".to_string()))
        );
        // 剥离失败（非 reducer 格式标签）/ 剥离后为空：同样兜底。
        assert_eq!(
            run_summary_texts(&failed_entry(4, "run terminal")),
            Some(("Run failed", "The run failed.".to_string()))
        );
        assert_eq!(
            run_summary_texts(&failed_entry(5, "run failed · ")),
            Some(("Run failed", "The run failed.".to_string()))
        );
    }

    #[test]
    fn workspace_header_predicates_follow_active_session_and_live_status() {
        let snapshot = snapshot_with_sessions(vec![session_entry("s-1", "Ship it", 10)]);
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        assert_eq!(projection.workspace_header_title(), None);
        assert_eq!(projection.workspace_header_status(), None);

        projection.select_session("s-1");
        assert_eq!(projection.workspace_header_title(), Some("Ship it"));
        // 空闲会话：无 live 终态可显示（诚实口径，不画 Completed）。
        assert_eq!(projection.workspace_header_status(), None);

        projection.active_runs.push(ActiveRun {
            run_id: "r-1".into(),
            session_id: "s-1".into(),
            started_at_ms: 1,
        });
        assert_eq!(
            projection.workspace_header_status(),
            Some(SessionLiveStatus::Running)
        );
    }
}
