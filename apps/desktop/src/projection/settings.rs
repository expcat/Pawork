//! Settings 投影：Host Data 走 protocol 类型；本模块只保留 Desktop UI 态
//!（loading / stale / 写 gate）与 AuthChanged 解析。

use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};

use serde_json::Value;

use super::DesktopProjection;

pub use pawork_client::{
    ApprovalModeWire, AuthStartData, DefaultModelPair, GeneralSettingsData,
    PermissionsSettingsData, ProviderAuthState, ProviderAuthStatusData, ProviderAuthStatusEntry,
    ProviderCatalogState, TerminalSettingsData,
};

/// 通用 / 权限 / 终端 / 供应商页共用的查询门闩（SET-6 / CLN-5）。
///
/// `writes_enabled`：已接通、至少成功解析过一次（`available`）、且非 stale。
/// 供应商页 Default 把 `available` 置 true（页始终是首页，写 gate 不跟
/// 首次查询成功绑定，与既有 `settings_writes_enabled` 同口径）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsQueryGate {
    pub loading: bool,
    pub stale_reason: Option<String>,
    pub error: Option<String>,
    /// 至少成功解析过一次（capability 到位）。失败 / 未知保持 false。
    pub available: bool,
}

impl SettingsQueryGate {
    pub fn begin_loading(&mut self) {
        self.loading = true;
        // 加载只在已连接时发起（controller 未派出时不进入 loading），
        // 新一轮请求开始即弃置旧 stale 标注；断线由 Disconnected 重新标记。
        self.stale_reason = None;
    }

    pub fn mark_stale(&mut self, reason: &str) {
        self.loading = false;
        self.stale_reason = Some(reason.to_string());
    }

    /// 查询或写失败：保留旧值，记录原因。从未成功过则保持 unavailable。
    pub fn apply_failed(&mut self, reason: &str) {
        self.loading = false;
        self.error = Some(reason.to_string());
    }

    pub fn mark_ready(&mut self) {
        self.loading = false;
        self.stale_reason = None;
        self.error = None;
        self.available = true;
    }

    /// 写动作 gate（render / 键盘 / AX 同源）：须已接通、非 stale、且
    /// 查询已成功（capability 到位）。
    pub fn writes_enabled(&self, connected: bool) -> bool {
        connected && self.available && self.stale_reason.is_none()
    }
}

fn auth_method_display_name(method: &str) -> &str {
    match method {
        "api_key" => "API key",
        "oauth" => "OAuth",
        other => other,
    }
}

/// Host `provider_auth_status` 条目的 UI 文案（render 与 AX 同源）。
/// 不能给 protocol 类型加 inherent 方法，故以 Desktop 扩展 trait 承载。
pub trait ProviderStatusLabels {
    fn auth_methods_label(&self) -> String;
    fn auth_label(&self) -> String;
    fn catalog_label(&self) -> String;
}

impl ProviderStatusLabels for ProviderAuthStatusEntry {
    /// 认证方式显示名（api_key → API key；oauth → OAuth；未知值原样，
    /// 不臆造能力）。
    fn auth_methods_label(&self) -> String {
        self.auth_methods
            .iter()
            .map(|method| auth_method_display_name(method))
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// 连接状态文案（render 与 AX 同源；masked credential 只显示 Host
    /// 已脱敏值）。`ProviderAuthState::None` 的用户文案仍是 "Not connected"。
    fn auth_label(&self) -> String {
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
            ProviderAuthState::None => "Not connected".into(),
            ProviderAuthState::Connecting => "Connecting…".into(),
            ProviderAuthState::Error { message } => format!("Error · {message}"),
        }
    }

    /// 目录来源文案（render 与 AX 同源）。
    fn catalog_label(&self) -> String {
        match &self.catalog {
            ProviderCatalogState::Remote { fetched_at } => {
                format!("Remote catalog · fetched {fetched_at}")
            }
            ProviderCatalogState::FixedFallback { snapshot_label, .. } => {
                format!("Built-in catalog fallback · {snapshot_label}")
            }
            ProviderCatalogState::Unavailable { error, .. } => {
                format!("Catalog unavailable · {error}")
            }
        }
    }
}

/// Settings「模型与供应商」页整体状态（SET-3 只读）：加载态、断线 stale
/// 标注与最后成功数据；断线保留 stale 只读结果，不伪造刷新。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsProvidersState {
    pub query: SettingsQueryGate,
    pub providers: Vec<ProviderAuthStatusEntry>,
    /// Host 权威默认 provider/model（provider_auth_status 顶层 default；
    /// 随 apply_loaded 整体替换，无默认即 None）。UI 比较仍用 pair 元组。
    pub default_model: Option<(String, String)>,
    /// 进行中的 OAuth 授权等待（auth_start 响应携带的 URL / user code /
    /// 到期；Desktop 只显示，不接触 token）。终态 AuthChanged 清除。
    pub oauth_waits: HashMap<String, AuthStartData>,
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

impl Default for SettingsProvidersState {
    fn default() -> Self {
        Self {
            query: SettingsQueryGate {
                // 供应商页始终是 Settings 首页：写 gate 不要求先查询成功。
                available: true,
                ..SettingsQueryGate::default()
            },
            providers: Vec::new(),
            default_model: None,
            oauth_waits: HashMap::new(),
            auth_notes: HashMap::new(),
            pending_status_refresh: false,
            auth_replacing_connected: HashSet::new(),
        }
    }
}

impl Deref for SettingsProvidersState {
    type Target = SettingsQueryGate;
    fn deref(&self) -> &Self::Target {
        &self.query
    }
}

impl DerefMut for SettingsProvidersState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.query
    }
}

impl SettingsProvidersState {
    /// 新数据到达：清 stale / error / loading，替换列表。
    pub fn apply_loaded(&mut self, data: ProviderAuthStatusData) {
        self.query.mark_ready();
        self.auth_notes.clear();
        self.auth_replacing_connected.clear();
        self.pending_status_refresh = false;
        self.providers = data.providers;
        self.default_model = data
            .default
            .map(|pair| (pair.provider_id, pair.model_id));
    }

    /// auth_start 响应到达：登记 OAuth 等待信息并置 Connecting（Pending
    /// 事件与响应的先后顺序不影响终态一致性）。
    pub fn apply_auth_started(&mut self, provider_id: &str, wait: AuthStartData) {
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
                self.query.error =
                    Some(format!("malformed auth change for {provider_id}: {reason}"));
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
                        entry.auth = ProviderAuthState::None;
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

/// Settings「通用」页状态（SET-6a / ADR-047）：Host `general_settings`
/// 权威 `proxy_url`。查询失败 / 未知则 `available=false`，导航不显示
/// 该页且不渲染写入口；断线 `mark_stale` 保留最后只读结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsGeneralState {
    pub query: SettingsQueryGate,
    /// Host 权威生效值；`None` = 未设置（跟随系统环境变量）。
    pub proxy_url: Option<String>,
}

impl Deref for SettingsGeneralState {
    type Target = SettingsQueryGate;
    fn deref(&self) -> &Self::Target {
        &self.query
    }
}

impl DerefMut for SettingsGeneralState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.query
    }
}

impl SettingsGeneralState {
    pub fn apply_loaded(&mut self, data: GeneralSettingsData) {
        self.query.mark_ready();
        self.proxy_url = data.proxy_url;
    }
}

/// Settings「权限与审批」页状态（SET-6b / ADR-048）：Host
/// `permissions_settings` 权威三元组（当前 mode / 会话 trusted / Global
/// 持久默认）。查询失败 / 未知则 `available=false`，导航不显示该页且
/// 不渲染写入口；断线 `mark_stale` 保留最后只读结果；写回执按字段
/// 确认（回执即写后状态，不乐观更新）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsPermissionsState {
    pub query: SettingsQueryGate,
    pub approval_mode: Option<ApprovalModeWire>,
    pub workspace_trusted: bool,
    /// Global 层持久值；`None` = 未设置（默认不信任）。本片只读展示。
    pub trust_workspaces_global: Option<bool>,
    /// Host 权威 attached workspace id（ADR-048 D1 实现期修订）。
    pub workspace_id: Option<String>,
}

impl Deref for SettingsPermissionsState {
    type Target = SettingsQueryGate;
    fn deref(&self) -> &Self::Target {
        &self.query
    }
}

impl DerefMut for SettingsPermissionsState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.query
    }
}

impl SettingsPermissionsState {
    pub fn apply_loaded(&mut self, data: PermissionsSettingsData) {
        self.query.mark_ready();
        self.approval_mode = Some(data.approval_mode);
        self.workspace_trusted = data.workspace_trusted;
        self.trust_workspaces_global = data.trust_workspaces_global;
        self.workspace_id = Some(data.workspace_id);
    }

    /// `set_approval_mode` Data 回执（回执即写后状态，ADR-048 D2）。
    pub fn confirm_approval_mode(&mut self, mode: ApprovalModeWire) {
        self.approval_mode = Some(mode);
        self.query.error = None;
    }

    /// `workspace_trust` Data 回执（回执即写后状态，ADR-048 D3）。
    pub fn confirm_workspace_trusted(&mut self, trusted: bool) {
        self.workspace_trusted = trusted;
        self.query.error = None;
    }
}

/// Settings「终端」页状态（SET-6d / ADR-050）：Host `terminal_settings`
/// 权威生效值。查询失败 / 未知则 `available=false`，导航不显示该页且
/// 不渲染写入口；断线 `mark_stale` 保留最后只读结果；写回执即写后
/// 状态（全态写，不乐观更新）。`effective_size` 同时作为新建终端的
/// 初始尺寸来源（未查询到时回落 80×24 现状）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsTerminalState {
    pub query: SettingsQueryGate,
    /// Global 持久值；`None` = 未设置（跟随平台默认）。
    pub shell: Option<String>,
    /// Host 生效值（未设置 = 80/24）。
    pub columns: u16,
    pub rows: u16,
}

impl Deref for SettingsTerminalState {
    type Target = SettingsQueryGate;
    fn deref(&self) -> &Self::Target {
        &self.query
    }
}

impl DerefMut for SettingsTerminalState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.query
    }
}

impl SettingsTerminalState {
    pub fn apply_loaded(&mut self, data: TerminalSettingsData) {
        self.query.mark_ready();
        self.shell = data.shell;
        self.columns = data.columns;
        self.rows = data.rows;
    }

    /// `set_terminal_settings` Data 回执（回执即写后状态，ADR-050 D3）。
    pub fn apply_confirmed(&mut self, data: TerminalSettingsData) {
        self.apply_loaded(data);
    }

    /// 新建终端的初始尺寸（ADR-050 D4）：查询成功取生效值，未查询到
    /// 回落 80×24（现状行为）。
    pub fn effective_size(&self) -> (u16, u16) {
        if self.query.available {
            (self.columns, self.rows)
        } else {
            (80, 24)
        }
    }
}

/// 解析 AuthChangeState 的 wire 形态（tag=type / content=data）。
/// AuthChanged 事件不是 CLN-4 Data 载荷，仍走手写解析。
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

fn json_str(value: &Value, field: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string field {field}"))
}

impl DesktopProjection {
    /// 断线：四个 Settings 切片同口径标 stale，保留最后只读结果。
    pub fn mark_settings_stale(&mut self, reason: &str) {
        self.settings_providers.mark_stale(reason);
        self.settings_general.mark_stale(reason);
        self.settings_permissions.mark_stale(reason);
        self.settings_terminal.mark_stale(reason);
    }

    /// set_default_model 获 Host Data 确认：Composer 同步到已确认默认
    ///（清 pending 切换；不改当前会话 / 草稿 / Run）。
    pub fn confirm_default_model(&mut self, provider_id: String, model_id: String) {
        self.selected_model = Some((provider_id, model_id));
        self.pending_model = None;
    }

    pub fn confirm_default_model_pair(&mut self, pair: DefaultModelPair) {
        self.confirm_default_model(pair.provider_id, pair.model_id);
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
}
