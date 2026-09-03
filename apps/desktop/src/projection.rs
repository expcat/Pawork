//! Desktop 渲染适配投影：把 pawork-client 的 Snapshot / TimelinePage / AppEvent
//! 投影为 Desktop UI 可直接渲染的状态。
//!
//! 本模块不依赖 gpui / tokio / OS API（gui-design 四层约束）。时间线语义
//! （去重 / 有序插入 / assistant 合并 / tool 双键锚点 / resume 基线）委托
//! pawork-protocol::projection 的单一 reducer（R3 波 C，CR08-08 根治）；
//! 本文件只保留 UI 态（连接 / session 列表 / 审批卡 / 模型 / run 跟踪）与
//! 渲染分组。

use std::collections::{BTreeSet, HashMap, HashSet};

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

/// SET-3 只读供应商页：Host `provider_auth_status` 的单个 provider 认证
/// 状态投影。只承载 Host 权威事实（wire 形状见 gui_host
/// handlers/settings.rs），不含写操作状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderAuthState {
    Connected {
        method: String,
        masked_credential: Option<String>,
    },
    NotConnected,
    Connecting,
    Error {
        message: String,
    },
}

/// 目录三态（与 Host catalog_state 同口径）：remote 探测成功 /
/// 固定回退快照 / 不可用（带原因）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderCatalogState {
    Remote { fetched_at: String },
    FixedFallback { snapshot_label: String },
    Unavailable { error: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderStatusEntry {
    pub provider_id: String,
    pub display_name: String,
    pub endpoint_label: String,
    pub auth_methods: Vec<String>,
    pub auth: ProviderAuthState,
    pub catalog: ProviderCatalogState,
}

/// `provider_auth_status` 的 `AppResponse::Data` 投影：供应商清单 + Host
/// 权威默认模型（顶层 `default`；`None` = 未设置默认）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsProvidersData {
    pub providers: Vec<ProviderStatusEntry>,
    pub default_model: Option<(String, String)>,
}

fn auth_method_display_name(method: &str) -> &str {
    match method {
        "api_key" => "API key",
        "oauth" => "OAuth",
        other => other,
    }
}

impl ProviderStatusEntry {
    /// 认证方式显示名（api_key → API key；oauth → OAuth；未知值原样，
    /// 不臆造能力）。
    pub fn auth_methods_label(&self) -> String {
        self.auth_methods
            .iter()
            .map(|method| auth_method_display_name(method))
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// 连接状态文案（render 与 AX 同源；masked credential 只显示 Host
    /// 已脱敏值）。
    pub fn auth_label(&self) -> String {
        match &self.auth {
            ProviderAuthState::Connected {
                method,
                masked_credential,
            } => {
                let mut label = format!("Connected · {}", auth_method_display_name(method));
                if let Some(masked) = masked_credential {
                    label.push_str(" · ");
                    label.push_str(masked);
                }
                label
            }
            ProviderAuthState::NotConnected => "Not connected".into(),
            ProviderAuthState::Connecting => "Connecting…".into(),
            ProviderAuthState::Error { message } => format!("Error · {message}"),
        }
    }

    /// 目录来源文案（render 与 AX 同源）。
    pub fn catalog_label(&self) -> String {
        match &self.catalog {
            ProviderCatalogState::Remote { fetched_at } => {
                format!("Remote catalog · fetched {fetched_at}")
            }
            ProviderCatalogState::FixedFallback { snapshot_label } => {
                format!("Built-in catalog fallback · {snapshot_label}")
            }
            ProviderCatalogState::Unavailable { error } => {
                format!("Catalog unavailable · {error}")
            }
        }
    }
}

/// Settings「模型与供应商」页整体状态（SET-3 只读）：加载态、断线 stale
/// 标注与最后成功数据；断线保留 stale 只读结果，不伪造刷新。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsProvidersState {
    pub loading: bool,
    pub stale_reason: Option<String>,
    pub error: Option<String>,
    pub providers: Vec<ProviderStatusEntry>,
    /// Host 权威默认 provider/model（provider_auth_status 顶层 default；
    /// 随 apply_loaded 整体替换，无默认即 None）。
    pub default_model: Option<(String, String)>,
    /// 进行中的 OAuth 授权等待（auth_start 响应携带的 URL / user code /
    /// 到期；Desktop 只显示，不接触 token）。终态 AuthChanged 清除。
    pub oauth_waits: HashMap<String, OAuthWait>,
    /// 终态 AuthChanged 的瞬态反馈（取消 / 过期 / 移除）；下次权威状态
    /// 到达即清空。
    pub auth_notes: HashMap<String, String>,
    /// Succeeded 落地后置位：认证成功≠目录成功，UI 需再拉一次
    /// provider_auth_status（两状态分离呈现）。Removed 同样置位：
    /// 目录应变为 unavailable，且 env fallback 仍可能保持连接。
    pub pending_status_refresh: bool,
    /// Replace 写流程基线：流程起点 provider 已 Connected。此后收到非
    /// Succeeded / Removed 终态不清旧凭证（Host 未删除），改为触发
    /// provider_auth_status 重查交权威裁决。
    pub auth_replacing_connected: HashSet<String>,
}

impl SettingsProvidersState {
    pub fn begin_loading(&mut self) {
        self.loading = true;
        // 加载只在已连接时发起（controller 未派出时不进入 loading），
        // 新一轮请求开始即弃置旧 stale 标注；断线由 Disconnected 重新标记。
        self.stale_reason = None;
    }

    /// 新数据到达：清 stale / error / loading，替换列表。
    pub fn apply_loaded(&mut self, data: SettingsProvidersData) {
        self.loading = false;
        self.stale_reason = None;
        self.error = None;
        self.auth_notes.clear();
        self.auth_replacing_connected.clear();
        self.pending_status_refresh = false;
        self.providers = data.providers;
        self.default_model = data.default_model;
    }

    /// 查询失败：保留旧列表供只读（不伪造空态），记录失败原因。
    pub fn apply_failed(&mut self, reason: &str) {
        self.loading = false;
        self.error = Some(reason.to_string());
    }

    /// 断线：终止在途加载，保留最后只读结果并标注 stale。
    pub fn mark_stale(&mut self, reason: &str) {
        self.loading = false;
        self.stale_reason = Some(reason.to_string());
    }

    /// auth_start 响应到达：登记 OAuth 等待信息并置 Connecting（Pending
    /// 事件与响应的先后顺序不影响终态一致性）。
    pub fn apply_auth_started(&mut self, provider_id: &str, wait: OAuthWait) {
        self.oauth_waits.insert(provider_id.to_string(), wait);
        if let Some(entry) = self
            .providers
            .iter_mut()
            .find(|entry| entry.provider_id == provider_id)
        {
            entry.auth = ProviderAuthState::Connecting;
        }
    }

    /// 写流程起点登记：provider 当前已 Connected 时记录 Replace 基线
    ///（UI 乐观置 Connecting 之前调用）。
    pub fn begin_auth_flow(&mut self, provider_id: &str) {
        let connected = self.providers.iter().any(|entry| {
            entry.provider_id == provider_id
                && matches!(entry.auth, ProviderAuthState::Connected { .. })
        });
        if connected {
            self.auth_replacing_connected
                .insert(provider_id.to_string());
        }
    }

    /// 消费 Succeeded / Removed 等置位的再查询提示（UI 在事件泵回执后调用）。
    pub fn take_pending_status_refresh(&mut self) -> bool {
        std::mem::take(&mut self.pending_status_refresh)
    }

    /// 应用 AuthChanged 事件（serde Value 形态；畸形载荷 fail-closed：
    /// 只记录错误，不落地任何认证状态变化）。
    pub fn apply_auth_changed_value(&mut self, provider_id: &str, state: &Value) -> bool {
        match parse_auth_change(state) {
            Ok(change) => self.apply_auth_change(provider_id, change),
            Err(reason) => {
                self.error = Some(format!("malformed auth change for {provider_id}: {reason}"));
                false
            }
        }
    }

    fn apply_auth_change(&mut self, provider_id: &str, change: AuthChange) -> bool {
        let terminal = !matches!(change, AuthChange::Pending);
        if terminal {
            self.oauth_waits.remove(provider_id);
        }
        // Replace 基线：非 Succeeded / Removed 终态不清旧凭证（Host 未
        // 删除），改触发权威重查；Succeeded / Removed 落地即清基线。
        let replacing = self.auth_replacing_connected.contains(provider_id);
        if matches!(change, AuthChange::Succeeded { .. } | AuthChange::Removed) {
            self.auth_replacing_connected.remove(provider_id);
        }
        if let Some(entry) = self
            .providers
            .iter_mut()
            .find(|entry| entry.provider_id == provider_id)
        {
            match &change {
                AuthChange::Pending => entry.auth = ProviderAuthState::Connecting,
                AuthChange::Succeeded {
                    method,
                    masked_credential,
                } => {
                    entry.auth = ProviderAuthState::Connected {
                        method: method.clone(),
                        masked_credential: Some(masked_credential.clone()),
                    };
                    // 认证成功≠目录成功：提示 UI 再查一次 provider_auth_status。
                    self.pending_status_refresh = true;
                }
                AuthChange::Failed { error } => {
                    if replacing {
                        // 旧凭证仍在：不断言 Error，交重查裁决；失败原因走
                        // 瞬态 note（权威数据到达即清）。
                        self.pending_status_refresh = true;
                        self.auth_notes.insert(
                            provider_id.to_string(),
                            format!("Replacement failed · {error}"),
                        );
                    } else {
                        entry.auth = ProviderAuthState::Error {
                            message: error.clone(),
                        };
                    }
                }
                AuthChange::Cancelled | AuthChange::Expired | AuthChange::Removed => {
                    if replacing && !matches!(change, AuthChange::Removed) {
                        // Replace 被取消 / 过期：旧凭证仍在，保留现状态并
                        // 重查权威状态。
                        self.pending_status_refresh = true;
                    } else {
                        entry.auth = ProviderAuthState::NotConnected;
                        if matches!(change, AuthChange::Removed) {
                            // 删除后目录与 env 残留都交 Host 权威重查。
                            self.pending_status_refresh = true;
                        }
                    }
                }
            }
        }
        let note = match &change {
            AuthChange::Cancelled => Some("Authorization cancelled"),
            AuthChange::Expired => Some("Authorization expired"),
            AuthChange::Removed => Some("Connection removed"),
            _ => None,
        };
        if let Some(note) = note {
            self.auth_notes
                .insert(provider_id.to_string(), note.to_string());
        }
        false
    }
}

/// AuthChanged 事件的投影视图（wire 六态）。由 serde Value 解析，畸形
/// 形状 fail-closed 报错，不静默丢字段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthChange {
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

/// OAuth 授权等待信息（auth_start 响应；SET-4 只用于显示）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthWait {
    pub verification_url: String,
    pub user_code: Option<String>,
    pub expires_at: Option<String>,
}

/// Settings「通用」页状态（SET-6a / ADR-047）：Host `general_settings`
/// 权威 `proxy_url`。查询失败 / 未知则 `available=false`，导航不显示
/// 该页且不渲染写入口；断线 `mark_stale` 保留最后只读结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsGeneralState {
    pub loading: bool,
    pub stale_reason: Option<String>,
    pub error: Option<String>,
    /// 至少成功解析过一次（capability 到位）。失败 / 未知保持 false。
    pub available: bool,
    /// Host 权威生效值；`None` = 未设置（跟随系统环境变量）。
    pub proxy_url: Option<String>,
}

impl SettingsGeneralState {
    pub fn begin_loading(&mut self) {
        self.loading = true;
        self.stale_reason = None;
    }

    pub fn apply_loaded(&mut self, proxy_url: Option<String>) {
        self.loading = false;
        self.stale_reason = None;
        self.error = None;
        self.available = true;
        self.proxy_url = proxy_url;
    }

    /// 查询或写失败：保留旧值，记录原因。从未成功过则保持 unavailable。
    pub fn apply_failed(&mut self, reason: &str) {
        self.loading = false;
        self.error = Some(reason.to_string());
    }

    pub fn mark_stale(&mut self, reason: &str) {
        self.loading = false;
        self.stale_reason = Some(reason.to_string());
    }

    /// 写动作 gate（render / 键盘 / AX 同源）：须已接通、非 stale、且
    /// 查询已成功（capability 到位）。
    pub fn writes_enabled(&self, connected: bool) -> bool {
        connected && self.available && self.stale_reason.is_none()
    }
}

/// 审批模式五档（SET-6b / ADR-048 D1-D2；wire 串与 Host `ApprovalMode`
/// serde 表示一致）。render 与 AX 共用 label / description；未知 wire 串
/// fail-closed，不臆造档位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalModeSetting {
    AlwaysAsk,
    AskForWrites,
    AskForDangerous,
    NeverAsk,
    ReadOnly,
}

impl ApprovalModeSetting {
    /// 全部档位：页面渲染与解析白名单的单一来源。
    pub const ALL: [Self; 5] = [
        Self::AlwaysAsk,
        Self::AskForWrites,
        Self::AskForDangerous,
        Self::NeverAsk,
        Self::ReadOnly,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            Self::AlwaysAsk => "always_ask",
            Self::AskForWrites => "ask_for_writes",
            Self::AskForDangerous => "ask_for_dangerous",
            Self::NeverAsk => "never_ask",
            Self::ReadOnly => "read_only",
        }
    }

    /// wire 串 → 档位；未知值 Err（fail-closed）。
    pub fn from_wire(value: &str) -> Result<Self, String> {
        Self::ALL
            .iter()
            .copied()
            .find(|mode| mode.wire() == value)
            .ok_or_else(|| format!("unknown approval mode {value}"))
    }

    /// 档位名（render 与 AX 同源）。
    pub fn label(self) -> &'static str {
        match self {
            Self::AlwaysAsk => "每次询问",
            Self::AskForWrites => "写操作询问",
            Self::AskForDangerous => "危险操作询问",
            Self::NeverAsk => "从不询问",
            Self::ReadOnly => "只读",
        }
    }

    /// 档位说明（render 与 AX 同源；从不询问档如实披露灾难命令地板）。
    pub fn description(self) -> &'static str {
        match self {
            Self::AlwaysAsk => "所有工具调用都需要人工批准",
            Self::AskForWrites => "只读放行，写操作需要批准",
            Self::AskForDangerous => "常规操作放行，危险操作需要批准",
            Self::NeverAsk => "全部自动执行；灾难命令仍被 Host 拒绝",
            Self::ReadOnly => "只放行只读操作，不执行任何写操作",
        }
    }
}

/// `permissions_settings` 查询的载荷（SET-6b / ADR-048 D1 含实现期修订）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionsSettingsData {
    pub approval_mode: ApprovalModeSetting,
    pub workspace_trusted: bool,
    pub trust_workspaces_global: Option<bool>,
    /// Host 权威 attached workspace id；发 `workspace_trust` 时原样回填，
    /// 校验方与发送方同源（不猜注册表首项）。
    pub workspace_id: String,
}

/// Settings「权限与审批」页状态（SET-6b / ADR-048）：Host
/// `permissions_settings` 权威三元组（当前 mode / 会话 trusted / Global
/// 持久默认）。查询失败 / 未知则 `available=false`，导航不显示该页且
/// 不渲染写入口；断线 `mark_stale` 保留最后只读结果；写回执按字段
/// 确认（回执即写后状态，不乐观更新）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsPermissionsState {
    pub loading: bool,
    pub stale_reason: Option<String>,
    pub error: Option<String>,
    /// 至少成功解析过一次（capability 到位）。失败 / 未知保持 false。
    pub available: bool,
    pub approval_mode: Option<ApprovalModeSetting>,
    pub workspace_trusted: bool,
    /// Global 层持久值；`None` = 未设置（默认不信任）。本片只读展示。
    pub trust_workspaces_global: Option<bool>,
    /// Host 权威 attached workspace id（ADR-048 D1 实现期修订）。
    pub workspace_id: Option<String>,
}

impl SettingsPermissionsState {
    pub fn begin_loading(&mut self) {
        self.loading = true;
        self.stale_reason = None;
    }

    pub fn apply_loaded(&mut self, data: PermissionsSettingsData) {
        self.loading = false;
        self.stale_reason = None;
        self.error = None;
        self.available = true;
        self.approval_mode = Some(data.approval_mode);
        self.workspace_trusted = data.workspace_trusted;
        self.trust_workspaces_global = data.trust_workspaces_global;
        self.workspace_id = Some(data.workspace_id);
    }

    /// 查询或写失败：保留旧值，记录原因。从未成功过则保持 unavailable。
    pub fn apply_failed(&mut self, reason: &str) {
        self.loading = false;
        self.error = Some(reason.to_string());
    }

    pub fn mark_stale(&mut self, reason: &str) {
        self.loading = false;
        self.stale_reason = Some(reason.to_string());
    }

    /// `set_approval_mode` Data 回执（回执即写后状态，ADR-048 D2）。
    pub fn confirm_approval_mode(&mut self, mode: ApprovalModeSetting) {
        self.approval_mode = Some(mode);
        self.error = None;
    }

    /// `workspace_trust` Data 回执（回执即写后状态，ADR-048 D3）。
    pub fn confirm_workspace_trusted(&mut self, trusted: bool) {
        self.workspace_trusted = trusted;
        self.error = None;
    }

    /// 写动作 gate（render / 键盘 / AX 同源）：须已接通、非 stale、且
    /// 查询已成功（capability 到位）。
    pub fn writes_enabled(&self, connected: bool) -> bool {
        connected && self.available && self.stale_reason.is_none()
    }
}

/// `terminal_settings` 查询 / `set_terminal_settings` 回执的载荷
///（ADR-050 D2/D3）：shell 为 Global 持久值（null = 跟随平台默认），
/// columns/rows 为生效值（未设置 = 80/24）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSettingsData {
    pub shell: Option<String>,
    pub columns: u16,
    pub rows: u16,
}

/// Settings「终端」页状态（SET-6d / ADR-050）：Host `terminal_settings`
/// 权威生效值。查询失败 / 未知则 `available=false`，导航不显示该页且
/// 不渲染写入口；断线 `mark_stale` 保留最后只读结果；写回执即写后
/// 状态（全态写，不乐观更新）。`effective_size` 同时作为新建终端的
/// 初始尺寸来源（未查询到时回落 80×24 现状）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsTerminalState {
    pub loading: bool,
    pub stale_reason: Option<String>,
    pub error: Option<String>,
    /// 至少成功解析过一次（capability 到位）。失败 / 未知保持 false。
    pub available: bool,
    /// Global 持久值；`None` = 未设置（跟随平台默认）。
    pub shell: Option<String>,
    /// Host 生效值（未设置 = 80/24）。
    pub columns: u16,
    pub rows: u16,
}

impl SettingsTerminalState {
    pub fn begin_loading(&mut self) {
        self.loading = true;
        self.stale_reason = None;
    }

    pub fn apply_loaded(&mut self, data: TerminalSettingsData) {
        self.loading = false;
        self.stale_reason = None;
        self.error = None;
        self.available = true;
        self.shell = data.shell;
        self.columns = data.columns;
        self.rows = data.rows;
    }

    /// 查询或写失败：保留旧值，记录原因。从未成功过则保持 unavailable。
    pub fn apply_failed(&mut self, reason: &str) {
        self.loading = false;
        self.error = Some(reason.to_string());
    }

    pub fn mark_stale(&mut self, reason: &str) {
        self.loading = false;
        self.stale_reason = Some(reason.to_string());
    }

    /// `set_terminal_settings` Data 回执（回执即写后状态，ADR-050 D3）。
    pub fn apply_confirmed(&mut self, data: TerminalSettingsData) {
        self.apply_loaded(data);
    }

    /// 写动作 gate（render / 键盘 / AX 同源）：须已接通、非 stale、且
    /// 查询已成功（capability 到位）。
    pub fn writes_enabled(&self, connected: bool) -> bool {
        connected && self.available && self.stale_reason.is_none()
    }

    /// 新建终端的初始尺寸（ADR-050 D4）：查询成功取生效值，未查询到
    /// 回落 80×24（现状行为）。
    pub fn effective_size(&self) -> (u16, u16) {
        if self.available {
            (self.columns, self.rows)
        } else {
            (80, 24)
        }
    }
}

/// 解析 AuthChangeState 的 wire 形态（tag=type / content=data）。
pub fn parse_auth_change(state: &Value) -> Result<AuthChange, String> {
    let kind = state
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "auth change missing type".to_string())?;
    match kind {
        "pending" => Ok(AuthChange::Pending),
        "succeeded" => {
            let data = state
                .get("data")
                .ok_or_else(|| "succeeded auth change missing data".to_string())?;
            Ok(AuthChange::Succeeded {
                method: json_str(data, "method")?,
                masked_credential: json_str(data, "masked_credential")?,
            })
        }
        "failed" => {
            let data = state
                .get("data")
                .ok_or_else(|| "failed auth change missing data".to_string())?;
            Ok(AuthChange::Failed {
                error: json_str(data, "error")?,
            })
        }
        "cancelled" => Ok(AuthChange::Cancelled),
        "expired" => Ok(AuthChange::Expired),
        "removed" => Ok(AuthChange::Removed),
        other => Err(format!("unknown auth change type {other}")),
    }
}

/// 解析 Host `provider_auth_status` 的 `AppResponse::Data` 载荷
///（`{"providers":[…], "default": …}`）。缺字段 / 未知状态 fail-closed，
/// 不静默丢条目或默认项。
pub fn parse_provider_status_entries(data: &Value) -> Result<SettingsProvidersData, String> {
    let Some(list) = data.get("providers").and_then(Value::as_array) else {
        return Err("provider status missing providers array".into());
    };
    let providers = list
        .iter()
        .map(parse_provider_status_entry)
        .collect::<Result<Vec<_>, String>>()?;
    Ok(SettingsProvidersData {
        providers,
        default_model: parse_default_model(data)?,
    })
}

/// 解析 Host `general_settings` 的 `AppResponse::Data` 载荷
/// `{ "proxy_url": string | null }`。缺字段 / 非字符串非 null fail-closed，
/// 不把残缺帧静默当成未设置。
pub fn parse_general_settings(data: &Value) -> Result<Option<String>, String> {
    let value = data
        .get("proxy_url")
        .ok_or_else(|| "general settings missing proxy_url".to_string())?;
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .map(str::to_string)
        .map(Some)
        .ok_or_else(|| "proxy_url is not a string or null".to_string())
}

/// 解析 Host `terminal_settings` 的 `AppResponse::Data` 载荷
/// `{ shell: string | null, columns: u16, rows: u16 }`（ADR-050 D2）。
/// 缺字段 / 类型错误 fail-closed，不把残缺帧当成默认值。
pub fn parse_terminal_settings(data: &Value) -> Result<TerminalSettingsData, String> {
    let shell = match data.get("shell") {
        None => return Err("terminal settings missing shell".into()),
        Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| "shell is not a string or null".to_string())?
                .to_string(),
        ),
    };
    let dimension = |field: &str| -> Result<u16, String> {
        let value = data
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("terminal settings missing u16 {field}"))?;
        u16::try_from(value).map_err(|_| format!("{field} exceeds u16 range"))
    };
    Ok(TerminalSettingsData {
        shell,
        columns: dimension("columns")?,
        rows: dimension("rows")?,
    })
}

/// 解析 Host `permissions_settings` 的 `AppResponse::Data` 载荷
/// `{ approval_mode, workspace_trusted, trust_workspaces_global }`。
/// 缺字段 / 未知 mode / 类型错误 fail-closed，不把残缺帧当成默认值。
pub fn parse_permissions_settings(data: &Value) -> Result<PermissionsSettingsData, String> {
    let mode = data
        .get("approval_mode")
        .and_then(Value::as_str)
        .ok_or_else(|| "permissions settings missing string approval_mode".to_string())?;
    let approval_mode = ApprovalModeSetting::from_wire(mode)
        .map_err(|_| format!("unknown approval mode {mode}"))?;
    let workspace_trusted = data
        .get("workspace_trusted")
        .and_then(Value::as_bool)
        .ok_or_else(|| "permissions settings missing boolean workspace_trusted".to_string())?;
    let global = data
        .get("trust_workspaces_global")
        .ok_or_else(|| "permissions settings missing trust_workspaces_global".to_string())?;
    let trust_workspaces_global = if global.is_null() {
        None
    } else {
        global
            .as_bool()
            .map(Some)
            .ok_or_else(|| "trust_workspaces_global is not a boolean or null".to_string())?
    };
    let workspace_id = data
        .get("workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "permissions settings missing string workspace_id".to_string())?
        .to_string();
    Ok(PermissionsSettingsData {
        approval_mode,
        workspace_trusted,
        trust_workspaces_global,
        workspace_id,
    })
}

/// 顶层 `default`：null = 未设置默认；对象须同时携带字符串
/// `provider_id` / `model_id`。缺字段 / 非法形状 fail-closed（整载荷
/// 报错，不静默丢默认项）。
fn parse_default_model(data: &Value) -> Result<Option<(String, String)>, String> {
    let value = data
        .get("default")
        .ok_or_else(|| "provider status missing default".to_string())?;
    if value.is_null() {
        return Ok(None);
    }
    let provider_id = json_str(value, "provider_id")
        .map_err(|_| "default missing string field provider_id".to_string())?;
    let model_id = json_str(value, "model_id")
        .map_err(|_| "default missing string field model_id".to_string())?;
    Ok(Some((provider_id, model_id)))
}

fn parse_provider_status_entry(entry: &Value) -> Result<ProviderStatusEntry, String> {
    let provider_id = json_str(entry, "provider_id")?;
    let auth = entry
        .get("auth")
        .ok_or_else(|| format!("provider {provider_id} missing auth"))?;
    let auth_kind = auth
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("provider {provider_id} auth missing type"))?;
    let auth_state = match auth_kind {
        "connected" => ProviderAuthState::Connected {
            method: json_str(auth, "method")?,
            masked_credential: auth
                .get("masked_credential")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        "none" => ProviderAuthState::NotConnected,
        "connecting" => ProviderAuthState::Connecting,
        "error" => ProviderAuthState::Error {
            message: json_str(auth, "message")?,
        },
        other => return Err(format!("provider {provider_id} unknown auth type {other}")),
    };
    let catalog = entry
        .get("catalog")
        .ok_or_else(|| format!("provider {provider_id} missing catalog"))?;
    let catalog_kind = catalog
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("provider {provider_id} catalog missing type"))?;
    let catalog_state = match catalog_kind {
        "remote" => ProviderCatalogState::Remote {
            fetched_at: json_str(catalog, "fetched_at")?,
        },
        "fixed_fallback" => ProviderCatalogState::FixedFallback {
            snapshot_label: json_str(catalog, "snapshot_label")?,
        },
        "unavailable" => ProviderCatalogState::Unavailable {
            error: json_str(catalog, "error")?,
        },
        other => {
            return Err(format!(
                "provider {provider_id} unknown catalog type {other}"
            ));
        }
    };
    // 认证方式 fail-closed：缺失 / 非数组 / 含非字符串项即报错，不静默
    //默认空列表（空列表会被当成「无认证方式」渲染，属伪造能力）。
    let auth_methods = entry
        .get("auth_methods")
        .ok_or_else(|| format!("provider {provider_id} missing auth_methods"))?
        .as_array()
        .ok_or_else(|| format!("provider {provider_id} auth_methods is not an array"))?
        .iter()
        .map(|method| {
            method
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("provider {provider_id} auth_methods entry is not a string"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ProviderStatusEntry {
        provider_id: provider_id.clone(),
        display_name: json_str(entry, "display_name")?,
        endpoint_label: json_str(entry, "endpoint_label")?,
        auth_methods,
        auth: auth_state,
        catalog: catalog_state,
    })
}

fn json_str(value: &Value, field: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string field {field}"))
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
    /// SET-3 Settings 供应商页只读状态（加载 / stale / Host 权威列表）。
    pub settings_providers: SettingsProvidersState,
    /// SET-6a Settings 通用页（Host `general_settings` / `proxy_url`）。
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

    /// 新建终端初始尺寸（ADR-050 D4）：create 回执后按 terminal_settings
    /// 生效值覆盖投影默认 80×24（尺寸仍在途——只写 columns/rows，不置
    /// resize_confirmed；随后那次 terminal_resize 的回执才确认）。
    pub fn apply_terminal_initial_size(
        &mut self,
        terminal_session_id: &str,
        columns: u16,
        rows: u16,
    ) -> bool {
        self.update_terminal(terminal_session_id, |terminal| {
            terminal.columns = columns;
            terminal.rows = rows;
        })
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

    /// set_default_model 获 Host Data 确认：Composer 同步到已确认默认
    ///（清 pending 切换；不改当前会话 / 草稿 / Run）。
    pub fn confirm_default_model(&mut self, provider_id: String, model_id: String) {
        self.selected_model = Some((provider_id, model_id));
        self.pending_model = None;
    }

    /// Settings「模型与默认项」失效判定：默认 provider 未连接，或默认
    /// model 不在该 provider 当前可运行目录（projection.models）。无默认
    /// 返回 false；目录为空（尚未成功加载或 model_list 失败）时无法判定，
    /// 抑制提示不误报；只判定显式提示，不做任何静默切换。
    pub fn default_model_unavailable(&self) -> bool {
        let Some((provider_id, model_id)) = &self.settings_providers.default_model else {
            return false;
        };
        let connected = self.settings_providers.providers.iter().any(|entry| {
            entry.provider_id == *provider_id
                && matches!(entry.auth, ProviderAuthState::Connected { .. })
        });
        if !connected {
            return true;
        }
        // 目录为空 = 无成功目录数据：区分「无目录数据」与「目录明确
        // 不含」，不误报失效。
        if self.models.is_empty() {
            return false;
        }
        !self
            .models
            .iter()
            .any(|model| model.provider_id == *provider_id && model.id == *model_id)
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

    #[test]
    fn provider_status_entries_map_host_wire_to_readonly_labels() {
        let data = json!({
            "providers": [
                {
                    "provider_id": "glm-coding",
                    "display_name": "Z.AI GLM Coding Plan",
                    "endpoint_label": "https://api.z.ai",
                    "auth_methods": ["api_key"],
                    "auth": {
                        "type": "connected",
                        "method": "api_key",
                        "masked_credential": "sk-…ab12"
                    },
                    "catalog": { "type": "remote", "fetched_at": "2026-09-02T08:00:00Z" }
                },
                {
                    "provider_id": "kimi",
                    "display_name": "Kimi",
                    "endpoint_label": "https://api.moonshot.cn",
                    "auth_methods": ["api_key", "oauth"],
                    "auth": { "type": "none" },
                    "catalog": {
                        "type": "fixed_fallback",
                        "snapshot_label": "models.dev@v1",
                        "fetched_at": null
                    }
                }
            ],
            "default": null
        });
        let loaded = parse_provider_status_entries(&data).expect("parse provider status");
        let entries = &loaded.providers;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].auth_methods_label(), "API key");
        assert_eq!(entries[0].auth_label(), "Connected · API key · sk-…ab12");
        assert_eq!(
            entries[0].catalog_label(),
            "Remote catalog · fetched 2026-09-02T08:00:00Z"
        );
        assert_eq!(entries[1].auth_methods_label(), "API key / OAuth");
        assert_eq!(entries[1].auth_label(), "Not connected");
        assert_eq!(
            entries[1].catalog_label(),
            "Built-in catalog fallback · models.dev@v1"
        );
        assert_eq!(loaded.default_model, None);
    }

    #[test]
    fn provider_status_entries_fail_closed_on_malformed_payload() {
        // default 合法（null），钉住错误只来自 providers 侧。
        let payload = |providers: Value| json!({ "providers": providers, "default": null });
        // 缺 providers 数组：整体 fail-closed。
        assert!(parse_provider_status_entries(&payload(json!("nope"))).is_err());
        // 单条缺 auth / 未知 auth 状态：不静默丢条目。
        assert!(parse_provider_status_entries(&payload(json!([
            { "provider_id": "glm-coding", "display_name": "Z.AI", "endpoint_label": "e" }
        ])))
        .is_err());
        // auth_methods 缺失 / 非数组 / 含非字符串项：fail-closed，不默认空表。
        assert!(parse_provider_status_entries(&payload(json!([
            {
                "provider_id": "glm-coding",
                "display_name": "Z.AI",
                "endpoint_label": "e",
                "auth": { "type": "none" },
                "catalog": { "type": "remote", "fetched_at": "t" }
            }
        ])))
        .is_err());
        assert!(parse_provider_status_entries(&payload(json!([
            {
                "provider_id": "glm-coding",
                "display_name": "Z.AI",
                "endpoint_label": "e",
                "auth_methods": "api_key",
                "auth": { "type": "none" },
                "catalog": { "type": "remote", "fetched_at": "t" }
            }
        ])))
        .is_err());
        assert!(parse_provider_status_entries(&payload(json!([
            {
                "provider_id": "glm-coding",
                "display_name": "Z.AI",
                "endpoint_label": "e",
                "auth": { "type": "mystery" },
                "catalog": { "type": "remote", "fetched_at": "t" }
            }
        ])))
        .is_err());
    }

    #[test]
    fn provider_status_default_parses_host_default() {
        // 主路径：default 对象 → Some(pair)；null → None（Host 权威语义）。
        let mut data = json!({
            "default": { "provider_id": "kimi", "model_id": "kimi-k2-0905-preview" }
        });
        data["providers"] = json!([]);
        let loaded = parse_provider_status_entries(&data).expect("parse default");
        assert_eq!(
            loaded.default_model,
            Some(("kimi".to_string(), "kimi-k2-0905-preview".to_string()))
        );
        let mut none = json!({ "default": null });
        none["providers"] = json!([]);
        assert_eq!(
            parse_provider_status_entries(&none)
                .expect("parse null default")
                .default_model,
            None
        );
    }

    #[test]
    fn provider_status_default_fails_closed_on_malformed_payload() {
        let payload = |default: Value| json!({ "providers": [], "default": default });
        // 缺顶层 default：整体 fail-closed，不静默当 null。
        assert!(parse_provider_status_entries(&json!({ "providers": [] })).is_err());
        // 非对象非 null / 缺 model_id / 字段非字符串：同样 fail-closed。
        assert!(parse_provider_status_entries(&payload(json!("kimi"))).is_err());
        assert!(parse_provider_status_entries(&payload(json!({ "provider_id": "kimi" }))).is_err());
        assert!(parse_provider_status_entries(&payload(json!({
            "provider_id": "kimi",
            "model_id": 7
        })))
        .is_err());
    }

    #[test]
    fn set_default_confirmation_syncs_composer_projection() {
        let mut projection = DesktopProjection::default();
        // 确认后重查 provider_auth_status：权威 default 先落地 Settings 状态。
        projection
            .settings_providers
            .apply_loaded(SettingsProvidersData {
                providers: Vec::new(),
                default_model: Some(("kimi".to_string(), "kimi-k2-0905-preview".to_string())),
            });
        projection.set_pending_model("glm-coding".into(), "glm-4.7".into());
        // Host Data 确认到达：selected_model 同步为已确认默认，pending 清空
        //（Composer 同步；不改会话 / 草稿 / Run）。
        projection.confirm_default_model("kimi".into(), "kimi-k2-0905-preview".into());
        assert_eq!(
            projection.selected_model,
            Some(("kimi".to_string(), "kimi-k2-0905-preview".to_string()))
        );
        assert_eq!(projection.pending_model, None);
        assert_eq!(
            projection.effective_model(),
            Some(&("kimi".to_string(), "kimi-k2-0905-preview".to_string()))
        );
        assert_eq!(
            projection.settings_providers.default_model,
            projection.selected_model
        );
    }

    #[test]
    fn default_model_unavailable_flag_tracks_connection_and_catalog() {
        let mut projection = DesktopProjection::default();
        let entry = |auth: ProviderAuthState| ProviderStatusEntry {
            provider_id: "kimi".into(),
            display_name: "Kimi".into(),
            endpoint_label: "https://api.kimi.com".into(),
            auth_methods: vec!["oauth".into()],
            auth,
            catalog: ProviderCatalogState::Unavailable {
                error: "offline".into(),
            },
        };
        // 目录为空（尚未加载 / model_list 失败）：即使已连接且有默认，
        // 也区分「无目录数据」与「目录明确不含」，不误报失效。
        projection.settings_providers.providers = vec![entry(ProviderAuthState::Connected {
            method: "oauth".into(),
            masked_credential: None,
        })];
        projection.settings_providers.default_model =
            Some(("kimi".into(), "kimi-k2-0905-preview".into()));
        assert!(!projection.default_model_unavailable());
        projection.set_models(vec![ModelEntry {
            provider_id: "kimi".into(),
            id: "kimi-k2-0905-preview".into(),
            display_name: "Kimi K2".into(),
            context_window_tokens: None,
        }]);
        // 无默认：不误报失效。
        projection.settings_providers.default_model = None;
        assert!(!projection.default_model_unavailable());
        // 默认 provider 未连接：显式失效。
        projection
            .settings_providers
            .apply_loaded(SettingsProvidersData {
                providers: vec![entry(ProviderAuthState::NotConnected)],
                default_model: Some(("kimi".into(), "kimi-k2-0905-preview".into())),
            });
        assert!(projection.default_model_unavailable());
        // 已连接但默认 model 不在该 provider 当前目录：显式失效。
        projection.settings_providers.providers[0].auth = ProviderAuthState::Connected {
            method: "oauth".into(),
            masked_credential: None,
        };
        projection.settings_providers.default_model = Some(("kimi".into(), "kimi-latest".into()));
        assert!(projection.default_model_unavailable());
        // 已连接且在当前目录：可用。
        projection.settings_providers.default_model =
            Some(("kimi".into(), "kimi-k2-0905-preview".into()));
        assert!(!projection.default_model_unavailable());
    }

    #[test]
    fn provider_status_refresh_failure_keeps_last_list_and_default() {
        // 页级刷新失败（OperationFailed → apply_failed）：保留旧列表与
        // 默认项，只记录错误，不伪造空态。
        let mut state = SettingsProvidersState::default();
        state.apply_loaded(SettingsProvidersData {
            providers: vec![ProviderStatusEntry {
                provider_id: "kimi".into(),
                display_name: "Kimi".into(),
                endpoint_label: "https://api.kimi.com".into(),
                auth_methods: vec!["oauth".into()],
                auth: ProviderAuthState::NotConnected,
                catalog: ProviderCatalogState::Unavailable {
                    error: "offline".into(),
                },
            }],
            default_model: Some(("kimi".to_string(), "kimi-k2-0905-preview".to_string())),
        });
        state.apply_failed("query failed");
        assert_eq!(state.providers.len(), 1);
        assert_eq!(
            state.default_model,
            Some(("kimi".to_string(), "kimi-k2-0905-preview".to_string()))
        );
        assert_eq!(state.error.as_deref(), Some("query failed"));
        assert!(!state.loading);
    }

    #[test]
    fn general_settings_parses_host_proxy_url() {
        assert_eq!(
            parse_general_settings(&json!({ "proxy_url": "http://127.0.0.1:7890" }))
                .expect("parse proxy_url string"),
            Some("http://127.0.0.1:7890".into())
        );
        assert_eq!(
            parse_general_settings(&json!({ "proxy_url": null })).expect("parse null proxy_url"),
            None
        );
        let mut state = SettingsGeneralState::default();
        state.apply_loaded(Some("http://127.0.0.1:7890".into()));
        assert!(state.available);
        assert_eq!(state.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
        state.apply_loaded(None);
        assert_eq!(state.proxy_url, None);
        assert!(state.available);
    }

    #[test]
    fn general_settings_fails_closed_on_malformed_payload() {
        assert!(parse_general_settings(&json!({})).is_err());
        assert!(parse_general_settings(&json!({ "proxy_url": 7 })).is_err());
        assert!(parse_general_settings(&json!({ "proxy_url": { "url": "x" } })).is_err());
        let mut state = SettingsGeneralState::default();
        state.apply_failed("malformed payload");
        assert!(!state.available);
        assert_eq!(state.proxy_url, None);
        assert_eq!(state.error.as_deref(), Some("malformed payload"));
    }

    #[test]
    fn general_settings_stale_keeps_last_value_and_disables_writes() {
        let mut state = SettingsGeneralState::default();
        state.apply_loaded(Some("http://127.0.0.1:7890".into()));
        assert!(state.writes_enabled(true));
        state.mark_stale("socket closed");
        assert_eq!(state.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
        assert!(state.available);
        assert_eq!(state.stale_reason.as_deref(), Some("socket closed"));
        assert!(!state.writes_enabled(true));
        assert!(!state.writes_enabled(false));
        state.apply_failed("query failed");
        assert_eq!(state.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
        assert!(state.available);
    }

    #[test]
    fn permissions_settings_parses_host_triple() {
        // 主路径：四元组解析（null global = 未设置）+ 全五档 wire 串往返。
        let data = parse_permissions_settings(&json!({
            "approval_mode": "ask_for_writes",
            "workspace_trusted": true,
            "trust_workspaces_global": null,
            "workspace_id": "workspace-1"
        }))
        .expect("parse permissions settings");
        assert_eq!(data.approval_mode, ApprovalModeSetting::AskForWrites);
        assert!(data.workspace_trusted);
        assert_eq!(data.trust_workspaces_global, None);
        assert_eq!(data.workspace_id, "workspace-1");
        for mode in ApprovalModeSetting::ALL {
            let parsed = parse_permissions_settings(&json!({
                "approval_mode": mode.wire(),
                "workspace_trusted": false,
                "trust_workspaces_global": true,
                "workspace_id": "workspace-1"
            }))
            .expect("known mode parses");
            assert_eq!(parsed.approval_mode, mode);
        }
        let mut state = SettingsPermissionsState::default();
        state.apply_loaded(data);
        assert!(state.available);
        assert_eq!(state.approval_mode, Some(ApprovalModeSetting::AskForWrites));
        assert!(state.writes_enabled(true));
        // 写回执按字段确认（回执即写后状态）。
        state.confirm_approval_mode(ApprovalModeSetting::NeverAsk);
        assert_eq!(state.approval_mode, Some(ApprovalModeSetting::NeverAsk));
        state.confirm_workspace_trusted(false);
        assert!(!state.workspace_trusted);
    }

    #[test]
    fn permissions_settings_fails_closed_on_malformed_payload() {
        assert!(parse_permissions_settings(&json!({})).is_err());
        assert!(parse_permissions_settings(&json!({ "approval_mode": "always_ask" })).is_err());
        assert!(parse_permissions_settings(&json!({
            "approval_mode": "yolo",
            "workspace_trusted": false,
            "trust_workspaces_global": null,
            "workspace_id": "workspace-1"
        }))
        .is_err());
        assert!(parse_permissions_settings(&json!({
            "approval_mode": 7,
            "workspace_trusted": false,
            "trust_workspaces_global": null,
            "workspace_id": "workspace-1"
        }))
        .is_err());
        assert!(parse_permissions_settings(&json!({
            "approval_mode": "read_only",
            "workspace_trusted": "yes",
            "trust_workspaces_global": null,
            "workspace_id": "workspace-1"
        }))
        .is_err());
        assert!(parse_permissions_settings(&json!({
            "approval_mode": "read_only",
            "workspace_trusted": false,
            "trust_workspaces_global": "true",
            "workspace_id": "workspace-1"
        }))
        .is_err());
        // 缺 workspace_id 同样 fail-closed（ADR-048 D1 实现期修订字段）。
        assert!(parse_permissions_settings(&json!({
            "approval_mode": "read_only",
            "workspace_trusted": false,
            "trust_workspaces_global": null
        }))
        .is_err());
        let mut state = SettingsPermissionsState::default();
        state.apply_failed("malformed payload");
        assert!(!state.available);
        assert_eq!(state.approval_mode, None);
        assert!(!state.workspace_trusted);
        assert_eq!(state.error.as_deref(), Some("malformed payload"));
    }

    #[test]
    fn permissions_settings_stale_keeps_last_values_and_disables_writes() {
        let mut state = SettingsPermissionsState::default();
        state.apply_loaded(
            parse_permissions_settings(&json!({
                "approval_mode": "ask_for_dangerous",
                "workspace_trusted": true,
                "trust_workspaces_global": null,
                "workspace_id": "workspace-1"
            }))
            .expect("parse"),
        );
        assert!(state.writes_enabled(true));
        state.mark_stale("socket closed");
        assert_eq!(
            state.approval_mode,
            Some(ApprovalModeSetting::AskForDangerous)
        );
        assert!(state.workspace_trusted);
        assert_eq!(state.trust_workspaces_global, None);
        assert!(state.available);
        assert_eq!(state.stale_reason.as_deref(), Some("socket closed"));
        assert!(!state.writes_enabled(true));
        assert!(!state.writes_enabled(false));
        // 写失败保旧（fail-closed）：值不动，只记录错误。
        state.apply_failed("set approval mode failed");
        assert_eq!(
            state.approval_mode,
            Some(ApprovalModeSetting::AskForDangerous)
        );
        assert!(state.workspace_trusted);
        assert!(state.available);
    }

    #[test]
    fn terminal_settings_main_path_confirms_full_state_and_sizes_new_terminal() {
        // 主路径：解析应用 + 全态写串联 + 初始尺寸取生效值（ADR-050 D2-D4）。
        let mut state = SettingsTerminalState::default();
        assert_eq!(state.effective_size(), (80, 24), "unqueried falls back");
        state.apply_loaded(
            parse_terminal_settings(&json!({
                "shell": "/bin/zsh", "columns": 120, "rows": 40
            }))
            .expect("parse terminal settings"),
        );
        assert!(state.available);
        assert!(state.writes_enabled(true));
        assert_eq!(state.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(state.effective_size(), (120, 40));
        // 全态写回执（shell=null 清除 + 新尺寸）即写后状态。
        state.apply_confirmed(
            parse_terminal_settings(&json!({
                "shell": null, "columns": 100, "rows": 30
            }))
            .expect("parse clear receipt"),
        );
        assert_eq!(state.shell, None);
        assert_eq!(state.effective_size(), (100, 30));
        // 新建终端投影初始尺寸取生效值（不置 resize_confirmed，回执才确认）。
        let mut projection = DesktopProjection::default();
        projection.workspace_id = Some("ws-1".into());
        projection.settings_terminal = state;
        projection.apply_terminal_created("ws-1".into(), "term-1".into());
        let (columns, rows) = projection.settings_terminal.effective_size();
        assert!(projection.apply_terminal_initial_size("term-1", columns, rows));
        assert_eq!(
            (projection.terminal.columns, projection.terminal.rows),
            (100, 30)
        );
        assert!(!projection.terminal.resize_confirmed);
    }

    #[test]
    fn terminal_settings_fails_closed_on_malformed_payload() {
        assert!(parse_terminal_settings(&json!({})).is_err());
        assert!(parse_terminal_settings(&json!({ "columns": 80, "rows": 24 })).is_err());
        assert!(parse_terminal_settings(&json!({
            "shell": null, "rows": 24
        }))
        .is_err());
        assert!(parse_terminal_settings(&json!({
            "shell": 7, "columns": 80, "rows": 24
        }))
        .is_err());
        assert!(parse_terminal_settings(&json!({
            "shell": null, "columns": "80", "rows": 24
        }))
        .is_err());
        assert!(parse_terminal_settings(&json!({
            "shell": null, "columns": 80, "rows": 70000
        }))
        .is_err());
        let mut state = SettingsTerminalState::default();
        state.apply_failed("malformed payload");
        assert!(!state.available);
        assert_eq!(state.shell, None);
        assert_eq!((state.columns, state.rows), (0, 0));
        assert_eq!(state.error.as_deref(), Some("malformed payload"));
    }

    fn settings_state_with_provider(auth_methods: &[&str]) -> SettingsProvidersState {
        let mut state = SettingsProvidersState::default();
        state.apply_loaded(SettingsProvidersData {
            providers: vec![ProviderStatusEntry {
                provider_id: "kimi".into(),
                display_name: "Kimi".into(),
                endpoint_label: "https://api.moonshot.cn".into(),
                auth_methods: auth_methods
                    .iter()
                    .map(|method| method.to_string())
                    .collect(),
                auth: ProviderAuthState::NotConnected,
                catalog: ProviderCatalogState::Unavailable {
                    error: "offline".into(),
                },
            }],
            default_model: None,
        });
        state
    }

    fn provider_auth(state: &SettingsProvidersState) -> &ProviderAuthState {
        &state.providers[0].auth
    }

    #[test]
    fn auth_changed_states_parse_and_apply_to_provider_auth() {
        // wire 形态（tag=type / content=data）六态解析。
        assert_eq!(
            parse_auth_change(&json!({ "type": "pending" })),
            Ok(AuthChange::Pending)
        );
        assert_eq!(
            parse_auth_change(&json!({
                "type": "succeeded",
                "data": { "method": "api_key", "masked_credential": "sk-…ab12" }
            })),
            Ok(AuthChange::Succeeded {
                method: "api_key".into(),
                masked_credential: "sk-…ab12".into()
            })
        );
        assert_eq!(
            parse_auth_change(&json!({
                "type": "failed",
                "data": { "error": "invalid key" }
            })),
            Ok(AuthChange::Failed {
                error: "invalid key".into()
            })
        );
        assert_eq!(
            parse_auth_change(&json!({ "type": "cancelled" })),
            Ok(AuthChange::Cancelled)
        );
        assert_eq!(
            parse_auth_change(&json!({ "type": "expired" })),
            Ok(AuthChange::Expired)
        );
        assert_eq!(
            parse_auth_change(&json!({ "type": "removed" })),
            Ok(AuthChange::Removed)
        );

        // Pending → Connecting。
        let mut state = settings_state_with_provider(&["oauth"]);
        state.apply_auth_changed_value("kimi", &json!({ "type": "pending" }));
        assert_eq!(*provider_auth(&state), ProviderAuthState::Connecting);

        // auth_start 回执登记等待详情；Succeeded 清等待、置 Connected，
        // 并置再查询提示（认证成功≠目录成功）。
        state.apply_auth_started(
            "kimi",
            OAuthWait {
                verification_url: "https://example/verify".into(),
                user_code: Some("ABCD".into()),
                expires_at: Some("2026-09-02T09:00:00Z".into()),
            },
        );
        assert_eq!(state.oauth_waits["kimi"].user_code.as_deref(), Some("ABCD"));
        state.apply_auth_changed_value(
            "kimi",
            &json!({
                "type": "succeeded",
                "data": { "method": "oauth", "masked_credential": "mo…cd" }
            }),
        );
        assert_eq!(
            *provider_auth(&state),
            ProviderAuthState::Connected {
                method: "oauth".into(),
                masked_credential: Some("mo…cd".into())
            }
        );
        assert!(!state.oauth_waits.contains_key("kimi"));
        assert!(state.take_pending_status_refresh());
        assert!(!state.pending_status_refresh);

        // Failed → Error（只承载 Host 已脱敏 message）。
        state.apply_auth_changed_value(
            "kimi",
            &json!({ "type": "failed", "data": { "error": "denied" } }),
        );
        assert_eq!(
            *provider_auth(&state),
            ProviderAuthState::Error {
                message: "denied".into()
            }
        );

        // Cancelled / Expired / Removed → NotConnected + 瞬态 note + 清等待。
        for (kind, note) in [
            ("cancelled", "Authorization cancelled"),
            ("expired", "Authorization expired"),
            ("removed", "Connection removed"),
        ] {
            state.apply_auth_started(
                "kimi",
                OAuthWait {
                    verification_url: "u".into(),
                    user_code: None,
                    expires_at: None,
                },
            );
            state.apply_auth_changed_value("kimi", &json!({ "type": kind }));
            assert_eq!(
                *provider_auth(&state),
                ProviderAuthState::NotConnected,
                "{kind}"
            );
            assert_eq!(state.auth_notes.get("kimi").map(String::as_str), Some(note));
            assert!(!state.oauth_waits.contains_key("kimi"), "{kind}");
            if kind == "removed" {
                assert!(state.take_pending_status_refresh(), "{kind}");
            } else {
                assert!(!state.pending_status_refresh, "{kind}");
            }
        }
        // 下一次权威状态到达即清空瞬态反馈。
        let providers = state.providers.clone();
        state.apply_loaded(SettingsProvidersData {
            providers,
            default_model: None,
        });
        assert!(state.auth_notes.is_empty());
    }

    #[test]
    fn malformed_auth_change_fails_closed_without_state_landing() {
        let mut state = settings_state_with_provider(&["api_key"]);
        for payload in [
            json!({ "type": "mystery" }),
            json!({ "type": "succeeded", "data": { "method": "api_key" } }),
            json!({ "data": { "error": "x" } }),
        ] {
            assert!(!state.apply_auth_changed_value("kimi", &payload));
            assert_eq!(*provider_auth(&state), ProviderAuthState::NotConnected);
        }
        assert!(state
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("malformed auth change"));
        assert!(state.oauth_waits.is_empty());
        assert!(state.auth_notes.is_empty());
        assert!(!state.pending_status_refresh);
    }

    #[test]
    fn replace_flow_terminal_keeps_old_credential_and_triggers_requery() {
        // Connected 起点的 Replace 流程：Cancelled / Expired / Failed 不清
        // 旧凭证（Host 未删除），保留现状态并触发权威重查。
        let mut state = settings_state_with_provider(&["oauth"]);
        state.providers[0].auth = ProviderAuthState::Connected {
            method: "oauth".into(),
            masked_credential: Some("mo…cd".into()),
        };
        state.begin_auth_flow("kimi");
        // 乐观 / Pending 先置 Connecting（终态到达前的 UI 状态）。
        state.apply_auth_changed_value("kimi", &json!({ "type": "pending" }));
        assert_eq!(*provider_auth(&state), ProviderAuthState::Connecting);

        for kind in ["cancelled", "expired"] {
            state.apply_auth_changed_value("kimi", &json!({ "type": kind }));
            assert_eq!(
                *provider_auth(&state),
                ProviderAuthState::Connecting,
                "{kind}: replace keeps the old credential pending requery"
            );
            assert!(state.pending_status_refresh, "{kind}");
            assert!(state.take_pending_status_refresh());
        }

        // Failed：不降级 Error，失败原因走瞬态 note，重查置位。
        state.apply_auth_changed_value(
            "kimi",
            &json!({ "type": "failed", "data": { "error": "invalid key" } }),
        );
        assert_eq!(*provider_auth(&state), ProviderAuthState::Connecting);
        assert!(state.pending_status_refresh);
        assert_eq!(
            state.auth_notes.get("kimi").map(String::as_str),
            Some("Replacement failed · invalid key")
        );
        assert!(state.take_pending_status_refresh());

        // Removed：凭证确实删除，仍复位 NotConnected。
        state.providers[0].auth = ProviderAuthState::Connected {
            method: "oauth".into(),
            masked_credential: Some("mo…cd".into()),
        };
        state.begin_auth_flow("kimi");
        state.apply_auth_changed_value("kimi", &json!({ "type": "removed" }));
        assert_eq!(*provider_auth(&state), ProviderAuthState::NotConnected);
        assert!(state.take_pending_status_refresh());

        // 权威数据到达即清基线（后续事件回到首连语义）。
        state.providers[0].auth = ProviderAuthState::Connected {
            method: "oauth".into(),
            masked_credential: Some("mo…cd".into()),
        };
        state.begin_auth_flow("kimi");
        let providers = state.providers.clone();
        let default_model = state.default_model.clone();
        state.apply_loaded(SettingsProvidersData {
            providers,
            default_model,
        });
        assert!(state.auth_replacing_connected.is_empty());
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
                history_item(
                    3,
                    "approval_requested",
                    json!({ "tool_name": "run_command" }),
                ),
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
