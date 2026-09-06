//! Settings 壳：导航与共享类型；各页实现见子模块。
//!
//! Settings 壳（SET-3/4/6a/6b/6c/6d/6e/6f/6g）：Settings Rail、「Models &
//! providers」、「Network」、「权限与审批」、「工具与 MCP」、「终端」、「外观」
//!、「高级」与「关于」页。
//!
//! 供应商页只呈现 Host `provider_auth_status` 权威事实：供应商名称、
//! 认证方式、连接状态与目录来源（SET-3）；SET-4 增认证写操作（API key
//! secure 输入验证、OAuth 等待/取消、Replace/Remove），全部由 descriptor
//! （auth_methods + auth.type）驱动，禁止按 provider 名分支。SET-6a 增
//! 「Network」页（`proxy_url` 读/设置/清除；wire 名保持 General）；SET-6b 增「权限与审批」页
//! （五档审批模式 / 会话信任 / Global 默认只读）；SET-6c 增「工具与
//! MCP」页（复用 Resources 的 mcp_list 数据链 + mcp_test /
//! mcp_server_remove 写动作）；SET-6d 增「终端」页（terminal_settings
//! 读取 + set_terminal_settings 全态写）；SET-6e 复用 Desktop 已有的
//! 100% / 125% / 150% 会话级字号能力，不经 Host；SET-6f 只读展示当前
//! 连接的握手摘要、启动 endpoint、恢复游标，并复用既有 Reconnect。Host 查询失败 /
//! 未知则隐藏对应导航项且不渲染写入口。断线保留 stale 只读结果
//! 并禁用 Host 写动作；外观 / 高级页作为本地能力始终可用。SET-6g 仅在
//! 当前认证握手声明非空 Host 数据目录时显示「关于」，且断线时立即隐藏。
//! 可见 / 键盘 / AX 三路径同 gate。

pub(super) use gpui::{div, prelude::*, px, App, Context, FontWeight, Pixels};

pub(super) use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
pub(super) use crate::ui::components::focus_ring::focus_ring;
pub(super) use crate::ui::components::label::Label;
pub(super) use crate::ui::components::list_row::ListRow;
pub(super) use crate::ui::components::panel::Panel;
pub(super) use crate::ui::theme::{dark, font, metrics};

pub(super) use crate::controller::McpServerEntry;
pub(super) use crate::projection::{
    group_models_by_provider, ApprovalModeWire, AuthStartData, ConnectionState, ModelEntry,
    ProviderAuthState, ProviderAuthStatusEntry, ProviderStatusLabels, SettingsPermissionsState,
    SettingsRole, SettingsTerminalState,
};
pub(super) use crate::ui::text_input::TextInput;

pub(super) use super::accessibility::dynamic_identifier;
pub(super) use super::resources::{
    mcp_server_meta_text, mcp_server_name_row, ResourcesFetch, ResourcesPanelState,
};
pub(super) use super::shell_layout;
pub(super) use super::{AppRoute, AppView, SettingsPage};

use crate::ui::i18n::t;

/// Settings 内容区水平 padding（OPT-4c / F2）：Rail 外全宽、两侧 32px，
/// 不保留 820px 内容列上限；AX 经 `settings_content_ax_width` 同源取值。
pub(super) const SETTINGS_CONTENT_PAD: f32 = 32.0;
/// Provider 卡片内边距（8px 节奏）。
pub(super) const PROVIDER_CARD_PAD: f32 = 8.0;
/// Provider 普通概览行高度；详情仅在连接流程、错误或二次确认时展开。
pub(crate) const PROVIDER_OVERVIEW_HEIGHT: f32 = 64.0;
/// 写动作按钮高度（与 Composer 28px 动作槽同节奏）。
pub(super) const SETTINGS_ACTION_HEIGHT: f32 = 28.0;
/// 两行审批说明与内边距随字号一起增长（render / AX 同源）。
pub(crate) const SETTINGS_APPROVAL_ROW_REMS: f32 = 3.5;
/// 「Default models」角色行几何（render 与 AX 同源；OPT-3b / ADR-055 D5）。
pub(crate) const SETTINGS_ROLE_ROW_HEIGHT: f32 = 48.0;
pub(crate) const SETTINGS_ROLE_LABEL_WIDTH: f32 = 172.0;
pub(crate) const SETTINGS_ROLE_MENU_WIDTH: f32 = 260.0;
/// 角色菜单 provider 分组头高度（render 与 AX 几何共用）。
pub(crate) const SETTINGS_ROLE_MENU_GROUP_HEADER_HEIGHT: f32 = 24.0;
/// 角色菜单空态说明块高度（标题 + 指引一行）。
pub(crate) const SETTINGS_ROLE_MENU_EMPTY_HEIGHT: f32 = 56.0;
/// 「Manage models」弹层几何（render 与 AX 同源；OPT-3a / ADR-055 D2）。
pub(crate) const SETTINGS_MODELS_MENU_WIDTH: f32 = 320.0;
pub(crate) const SETTINGS_MODELS_MENU_MAX_HEIGHT: f32 = 320.0;
pub(crate) const SETTINGS_MODELS_MENU_HEADER_HEIGHT: f32 = 28.0;
pub(crate) const SETTINGS_MODELS_MENU_ROW_HEIGHT: f32 = 44.0;
pub(crate) const SETTINGS_MODELS_MENU_EMPTY_HEIGHT: f32 = 104.0;
/// 展开区行高（Proxy / Manage models / Usage 行；render 与 AX 同源；
/// ADR-056 D4）。
pub(crate) const SETTINGS_PROVIDER_ROW_HEIGHT: f32 = 48.0;
/// Credentials 区头部高度（标题 + 副标题两行）。
pub(crate) const SETTINGS_PROVIDER_CREDENTIALS_HEADER_HEIGHT: f32 = 40.0;
/// 单条凭证行高度。
pub(crate) const SETTINGS_PROVIDER_CREDENTIAL_ROW_HEIGHT: f32 = 28.0;
/// Usage 进度条槽位几何（固定槽位，恒无填充；ADR-056 D5）。
pub(crate) const SETTINGS_PROVIDER_USAGE_BAR_WIDTH: f32 = 120.0;
pub(crate) const SETTINGS_PROVIDER_USAGE_BAR_HEIGHT: f32 = 4.0;
/// 「模型与默认项」区失效提示（render 与 AX 同源；只声明事实，不切换）。
pub(crate) fn settings_default_unavailable_note() -> &'static str {
    t("settings.default_unavailable_note")
}
/// null `proxy_url` 展示（ADR-047 D1；render / AX 同源）。
pub(crate) fn settings_proxy_unset() -> &'static str {
    t("settings.network.proxy_unset")
}
/// 生效边界（ADR-047 D2；不得宣称全局即时生效）。
pub(crate) fn settings_proxy_effect_note() -> &'static str {
    t("settings.network.proxy_effect_note")
}
/// 本机代理写入用户配置目录而非 workspace；render / AX 同源。
pub(crate) fn settings_proxy_storage_note() -> &'static str {
    t("settings.network.proxy_storage_note")
}

/// null `trust_workspaces_global` 展示（ADR-048 D1；render / AX 同源）。
pub(crate) fn settings_trust_unset() -> &'static str {
    t("settings.permissions.trust_unset")
}
/// 权限页生效边界（ADR-048 D2/D3；不得宣称持久化或影响进行中 Run）。
pub(crate) fn settings_permissions_effect_note() -> &'static str {
    t("settings.permissions.effect_note")
}

/// 「工具与 MCP」页 Remove 二次确认提示（SET-6c / ADR-049 D2；render 与
/// AX 同源，诚实标注快照语义）。
pub(crate) fn settings_mcp_remove_confirm_note() -> &'static str {
    t("settings.tools.remove_confirm_note")
}
/// 「工具与 MCP」页生效边界（SET-6c / ADR-049 D1/D2；render 与 AX 同源）。
pub(crate) fn settings_mcp_effect_note() -> &'static str {
    t("settings.tools.effect_note")
}

/// null shell 展示（SET-6d / ADR-050 D2；render 与 AX 同源）。
pub(crate) fn settings_terminal_shell_unset() -> &'static str {
    t("settings.terminal.shell_unset")
}
/// 终端页生效边界（SET-6d / ADR-050 D4；render 与 AX 同源，快照语义）。
pub(crate) fn settings_terminal_effect_note() -> &'static str {
    t("settings.terminal.effect_note")
}

/// 高级页启动目标边界（SET-6f）：runtime ID 不能冒充 CLI 配置实例名，
/// 且任何凭证及其路径都不进入 render / AX。
pub(crate) fn settings_advanced_target_note() -> &'static str {
    t("settings.advanced.target_note")
}
/// Host 级自检仍由 pre-Core CLI 命令负责；Desktop 不 shell-out，也不从
/// socket 路径推断 data directory / 配置实例名。
pub(crate) fn settings_advanced_doctor_note() -> &'static str {
    t("settings.advanced.doctor_note")
}

/// Provider 概览中的目录列：只给 availability 与真实模型数，不把错误、
/// endpoint 或 snapshot 标签塞进普通列表与 AX summary。
pub(crate) fn provider_catalog_overview_label(
    provider: &ProviderAuthStatusEntry,
    model_count: usize,
) -> String {
    match &provider.catalog {
        crate::projection::ProviderCatalogState::Remote { .. }
        | crate::projection::ProviderCatalogState::FixedFallback { .. } => {
            crate::ui::i18n::catalog_overview_label(model_count)
        }
        crate::projection::ProviderCatalogState::Unavailable { .. } => {
            t("settings.providers.catalog_unavailable").to_string()
        }
    }
}

/// 凭证类型标签（ADR-056 D4）：已知 kind 映射显示名，未知值原样保留
/// （不臆造能力）；render 与 AX 同源。
pub(crate) fn provider_credential_kind_label(kind: &str) -> String {
    match kind {
        "api_key" => t("settings.providers.credentials_kind_api_key").to_string(),
        "oauth" => t("settings.providers.credentials_kind_oauth").to_string(),
        other => other.to_string(),
    }
}

/// 凭证状态词（ADR-056 D3）：expired 只陈述过期事实；未过期复用
/// provider 连接态词条 Connected。render 与 AX 同源。
pub(crate) fn provider_credential_status_label(expired: bool) -> &'static str {
    if expired {
        t("settings.providers.credentials_expired")
    } else {
        t("provider.auth_connected")
    }
}
/// 外观页字号按钮的固定几何；render 与 AX bounds 共用，避免缩放后命中框漂移。
pub(crate) const SETTINGS_APPEARANCE_CONTROL_HEIGHT: f32 = SETTINGS_ACTION_HEIGHT;
pub(crate) const SETTINGS_APPEARANCE_CONTROL_WIDTH: f32 = 112.0;
pub(crate) const SETTINGS_APPEARANCE_CONTROL_GAP: f32 = 8.0;

/// 外观页唯一可写能力：复用既有三档 `TextScale`。
pub(crate) const SETTINGS_TEXT_SCALES: [font::TextScale; 3] = [
    font::TextScale::Percent100,
    font::TextScale::Percent125,
    font::TextScale::Percent150,
];

/// 字号控件 identifier（render 按钮 / AX 节点 / AX 派发同源）。
pub(crate) const fn settings_text_scale_identifier(scale: font::TextScale) -> &'static str {
    match scale {
        font::TextScale::Percent100 => "settings-text-scale-100",
        font::TextScale::Percent125 => "settings-text-scale-125",
        font::TextScale::Percent150 => "settings-text-scale-150",
    }
}

/// 只接受三个冻结 identifier；未知值 fail-closed。
pub(crate) fn settings_text_scale_from_identifier(identifier: &str) -> Option<font::TextScale> {
    SETTINGS_TEXT_SCALES
        .into_iter()
        .find(|scale| settings_text_scale_identifier(*scale) == identifier)
}

/// 终端页尺寸输入解析（SET-6d）：u16 且 ∈ 2..=1000（与 Host 校验一致，
/// ADR-050 D3）；畸形 / 越界返回 None（Save 禁用，fail-closed）。
/// render 与 AX 同源。
pub(crate) fn parse_terminal_dimension(text: &str) -> Option<u16> {
    let value: u16 = text.trim().parse().ok()?;
    (2..=1000).contains(&value).then_some(value)
}

/// 终端页 shell 输入 → wire 载荷（SET-6d / ADR-050 D3）：trim 后空串
/// 映射为 None（null = 跟随平台默认），使尺寸可在未设置 shell 时
/// 单独保存；非空则回传 trimmed。render / 键盘 / AX 同源。
pub(crate) fn parse_terminal_shell(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 终端页 Save 是否可点（SET-6d）：写 gate 开且 columns/rows 合法。
/// 空 shell 是合法全态（null），不参与禁用。render 与 AX 同源。
pub(crate) fn terminal_save_enabled(writes: bool, columns: Option<u16>, rows: Option<u16>) -> bool {
    writes && columns.is_some() && rows.is_some()
}

/// Settings 供应商页状态行（render 与 AX 同源）。stale / loading / error /
/// 空态独立判定：stale 与 error 可同时出现，空态仅在完全无状态且列表为
/// 空时给出（SET-3 审查修复 2/3）。
pub(crate) fn provider_status_lines(
    state: &crate::projection::SettingsProvidersState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.query.stale_reason {
        lines.push((
            "stale",
            t("settings.status.offline_stale").replace("{}", reason),
        ));
    } else if state.query.loading {
        lines.push(("loading", t("settings.status.loading").to_string()));
    }
    if let Some(error) = &state.query.error {
        lines.push((
            "error",
            t("settings.providers.status_error").replace("{}", error),
        ));
    }
    if state.providers.is_empty()
        && !state.query.loading
        && state.query.error.is_none()
        && state.query.stale_reason.is_none()
    {
        lines.push(("empty", t("settings.providers.status_empty").to_string()));
    }
    // OPT-3a / ADR-055 D3：禁用命中角色默认对时的诚实说明（Host
    // cleared_roles 回执）；随权威重查不消失，离开 Settings 清空。
    if let Some(note) = &state.model_cleared_note {
        lines.push(("cleared-roles", note.clone()));
    }
    lines
}

/// Settings Network 页状态行（内部/wire 仍沿用 General；render 与 AX 同源）。error 文案由事件消费侧
/// 按动作区分（load vs save），此处原样展示。
pub(crate) fn general_status_lines(
    state: &crate::projection::SettingsGeneralState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.query.stale_reason {
        lines.push((
            "stale",
            t("settings.status.offline_stale").replace("{}", reason),
        ));
    } else if state.query.loading {
        lines.push(("loading", t("settings.status.loading").to_string()));
    }
    if let Some(error) = &state.query.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings 权限页状态行（render 与 AX 同源）。error 文案由事件消费侧
/// 按动作区分（load / set mode / set trust），此处原样展示。
pub(crate) fn permissions_status_lines(
    state: &SettingsPermissionsState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.query.stale_reason {
        lines.push((
            "stale",
            t("settings.status.offline_stale").replace("{}", reason),
        ));
    } else if state.query.loading {
        lines.push(("loading", t("settings.status.loading").to_string()));
    }
    if let Some(error) = &state.query.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings 终端页状态行（SET-6d；render 与 AX 同源）。error 文案由事件
/// 消费侧按动作区分（load / set），此处原样展示。
pub(crate) fn terminal_status_lines(state: &SettingsTerminalState) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.query.stale_reason {
        lines.push((
            "stale",
            t("settings.status.offline_stale").replace("{}", reason),
        ));
    } else if state.query.loading {
        lines.push(("loading", t("settings.status.loading").to_string()));
    }
    if let Some(error) = &state.query.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings「工具与 MCP」页状态行（SET-6c；render 与 AX 同源）。复用
/// Resources 面状态：stale / loading / error / 空态独立判定。
pub(crate) fn tools_status_lines(state: &ResourcesPanelState) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.stale_reason {
        lines.push((
            "stale",
            t("settings.status.offline_stale").replace("{}", reason),
        ));
    } else if matches!(state.fetch, ResourcesFetch::Fetching) {
        lines.push(("loading", t("settings.status.loading").to_string()));
    }
    if let ResourcesFetch::Failed(reason) = &state.fetch {
        lines.push((
            "error",
            t("settings.tools.status_error").replace("{}", reason),
        ));
    }
    if let Some(error) = &state.action_error {
        lines.push(("action", error.clone()));
    }
    if state.servers.is_empty()
        && !matches!(state.fetch, ResourcesFetch::Fetching)
        && state.stale_reason.is_none()
        && !matches!(state.fetch, ResourcesFetch::Failed(_))
    {
        lines.push(("empty", t("settings.tools.status_empty").to_string()));
    }
    lines
}

/// Settings 写动作（SET-4）。render / 键盘 / AX 三路径同源：可见按钮、
/// on_activate 与 AX Press 共用同一 identifier 与同一入口 gate。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsAuthAction {
    ConnectApiKey,
    ConnectOauth,
    ReplaceOauth,
    CancelOauth,
    ReplaceApiKey,
    VerifyApiKey,
    CancelApiKeyInput,
    Remove,
    ConfirmRemove,
    KeepRemove,
}

/// Settings 控件 identifier 前缀（action key 在 provider id 之前，前缀
/// 锚定解析无歧义）。
pub(crate) const SETTINGS_CONTROL_PREFIX: &str = "settings-action-";

impl SettingsAuthAction {
    /// 全部动作：key 解析与焦点回收白名单的单一来源。
    pub(crate) const ALL: [Self; 10] = [
        Self::ConnectApiKey,
        Self::ConnectOauth,
        Self::ReplaceOauth,
        Self::CancelOauth,
        Self::ReplaceApiKey,
        Self::VerifyApiKey,
        Self::CancelApiKeyInput,
        Self::Remove,
        Self::ConfirmRemove,
        Self::KeepRemove,
    ];

    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::ConnectApiKey => "connect-api-key",
            Self::ConnectOauth => "connect-oauth",
            Self::ReplaceOauth => "replace-oauth",
            Self::CancelOauth => "cancel-oauth",
            Self::ReplaceApiKey => "replace-api-key",
            Self::VerifyApiKey => "verify-api-key",
            Self::CancelApiKeyInput => "cancel-api-key",
            Self::Remove => "remove",
            Self::ConfirmRemove => "confirm-remove",
            Self::KeepRemove => "keep-remove",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|action| action.key() == key)
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::ConnectApiKey => t("settings.providers.action_connect_api_key"),
            Self::ConnectOauth => t("settings.providers.action_connect_oauth"),
            Self::ReplaceOauth => t("settings.providers.action_replace_oauth"),
            Self::CancelOauth => t("settings.providers.action_cancel"),
            Self::ReplaceApiKey => t("settings.providers.action_replace_api_key"),
            Self::VerifyApiKey => t("settings.providers.action_verify"),
            Self::CancelApiKeyInput => t("settings.providers.action_cancel"),
            Self::Remove => t("settings.providers.action_remove"),
            Self::ConfirmRemove => t("settings.providers.action_confirm_remove"),
            Self::KeepRemove => t("settings.providers.action_keep"),
        }
    }

    /// 控件 identifier（render 按钮 id / AX 节点 id / 派发键三用；provider
    /// id 经 dynamic_identifier 转义）。
    pub(crate) fn identifier(&self, provider_id: &str) -> String {
        format!(
            "{SETTINGS_CONTROL_PREFIX}{}",
            dynamic_identifier(self.key(), provider_id)
        )
    }
}

/// settings 页控件（AX 派发用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsControl {
    Action(SettingsAuthAction, String),
    ApiKeyInput(String),
    /// 供应商代理开关（ADR-052 SET-6h）：携带转义后的 provider id，由
    /// AppView 对照 provider 清单还原（未知 fail-closed）。
    UseProxy(String),
    /// 卡片展开 / 折叠 chevron（ADR-056 D4）：本地视图态，携带转义后的
    /// provider id，由 AppView 对照 provider 清单还原（未知 fail-closed）。
    Expand(String),
}

pub(crate) fn settings_api_key_input_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_CONTROL_PREFIX}{}",
        dynamic_identifier("api-key-input", provider_id)
    )
}

/// 「供应商代理开关」控件 identifier（render 按钮 id / AX 节点 id / 派发键
/// 三用；provider id 经 dynamic_identifier 转义）。
pub(crate) fn settings_use_proxy_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_CONTROL_PREFIX}{}",
        dynamic_identifier("use-proxy", provider_id)
    )
}

/// 卡片展开 / 折叠 chevron identifier（render 按钮 id / AX 节点 id /
/// 派发键三用；provider id 经 dynamic_identifier 转义；ADR-056 D4）。
pub(crate) fn settings_provider_expand_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_CONTROL_PREFIX}{}",
        dynamic_identifier("expand", provider_id)
    )
}

/// 「Default models」四角色控件前缀（render / AX / 派发同源；OPT-3b /
/// ADR-055 D5）。
pub(crate) const SETTINGS_ROLE_CONTROL_PREFIX: &str = "settings-role-";

/// 角色下拉触发器 identifier（render 按钮 id / AX 节点 id / 派发键三用）。
pub(crate) fn settings_role_trigger_identifier(role: SettingsRole) -> String {
    format!("{SETTINGS_ROLE_CONTROL_PREFIX}{}", role.wire_name())
}

/// 角色菜单「清除」行 identifier。
pub(crate) fn settings_role_clear_identifier(role: SettingsRole) -> String {
    format!("{SETTINGS_ROLE_PREFIX_CLEAR}{}", role.wire_name())
}

/// 角色菜单候选行 identifier；provider 与 model 以 ':' 拼接后整体转义
///（与 settings_default_target_for_escaped 还原口径一致）。
pub(crate) fn settings_role_item_identifier(
    role: SettingsRole,
    provider_id: &str,
    model_id: &str,
) -> String {
    format!(
        "{SETTINGS_ROLE_PREFIX_ITEM}{}{}",
        role.wire_name(),
        dynamic_identifier("", &format!("{provider_id}:{model_id}"))
    )
}

const SETTINGS_ROLE_PREFIX_CLEAR: &str = "settings-role-clear-";
const SETTINGS_ROLE_PREFIX_ITEM: &str = "settings-role-item-";

/// 「Manage models」弹层控件前缀（render / AX / 派发同源；OPT-3a /
/// ADR-055 D2）。
pub(crate) const SETTINGS_MODELS_CONTROL_PREFIX: &str = "settings-models-";

/// 「Manage models」触发器 identifier（render 按钮 id / AX 节点 id /
/// 派发键三用；provider id 经 dynamic_identifier 转义）。
pub(crate) fn settings_manage_models_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_MODELS_CONTROL_PREFIX}{}",
        dynamic_identifier("manage", provider_id)
    )
}

/// 弹层面板 identifier（render 面板 id / AX 组节点同源）。
pub(crate) fn settings_models_menu_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_MODELS_CONTROL_PREFIX}{}",
        dynamic_identifier("menu", provider_id)
    )
}

/// Enable all / Disable all / Refresh catalog identifier。
pub(crate) fn settings_models_enable_all_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_MODELS_CONTROL_PREFIX}{}",
        dynamic_identifier("enable-all", provider_id)
    )
}

pub(crate) fn settings_models_disable_all_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_MODELS_CONTROL_PREFIX}{}",
        dynamic_identifier("disable-all", provider_id)
    )
}

pub(crate) fn settings_models_refresh_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_MODELS_CONTROL_PREFIX}{}",
        dynamic_identifier("refresh", provider_id)
    )
}

/// 弹层单模型 Switch identifier；provider 与 model 以 ':' 拼接后整体
/// 转义（与 settings_model_target_for_escaped 还原口径一致）。
pub(crate) fn settings_model_switch_identifier(provider_id: &str, model_id: &str) -> String {
    format!(
        "{SETTINGS_MODELS_CONTROL_PREFIX}{}",
        dynamic_identifier("toggle", &format!("{provider_id}:{model_id}"))
    )
}

/// 弹层控件（AX 派发用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsModelsControl {
    /// 触发器（携带转义后的 provider id）。
    Manage(String),
    /// 单模型 Switch（携带转义后的 "<provider>:<model>"）。
    Toggle(String),
    EnableAll(String),
    DisableAll(String),
    /// 空目录态 Refresh catalog（复用页级刷新路径）。
    RefreshCatalog(String),
}

/// 前缀锚定解析弹层控件 identifier；未知形状 fail-closed。
pub(crate) fn parse_settings_models_control(identifier: &str) -> Option<SettingsModelsControl> {
    let rest = identifier.strip_prefix(SETTINGS_MODELS_CONTROL_PREFIX)?;
    if let Some(escaped) = rest.strip_prefix("manage-") {
        return Some(SettingsModelsControl::Manage(escaped.to_string()));
    }
    if let Some(escaped) = rest.strip_prefix("toggle-") {
        return Some(SettingsModelsControl::Toggle(escaped.to_string()));
    }
    if let Some(escaped) = rest.strip_prefix("enable-all-") {
        return Some(SettingsModelsControl::EnableAll(escaped.to_string()));
    }
    if let Some(escaped) = rest.strip_prefix("disable-all-") {
        return Some(SettingsModelsControl::DisableAll(escaped.to_string()));
    }
    if let Some(escaped) = rest.strip_prefix("refresh-") {
        return Some(SettingsModelsControl::RefreshCatalog(escaped.to_string()));
    }
    None
}

/// 角色页控件（render / 键盘 / AX 三路径派发用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsRoleControl {
    Trigger(SettingsRole),
    Clear(SettingsRole),
    /// 候选行：携带转义后的 "<provider>:<model>"，由 AppView 对照
    /// projection.models 还原（未知 fail-closed）。
    Item(SettingsRole, String),
}

fn settings_role_for_key(key: &str) -> Option<SettingsRole> {
    SettingsRole::ALL
        .into_iter()
        .find(|role| role.wire_name() == key)
}

/// 前缀锚定解析角色控件 identifier；未知角色 / 形状 fail-closed。
pub(crate) fn parse_settings_role_control(identifier: &str) -> Option<SettingsRoleControl> {
    if let Some(key) = identifier.strip_prefix(SETTINGS_ROLE_PREFIX_CLEAR) {
        return settings_role_for_key(key).map(SettingsRoleControl::Clear);
    }
    if let Some(item) = identifier.strip_prefix(SETTINGS_ROLE_PREFIX_ITEM) {
        // 构造是 PREFIX + role + dynamic_identifier("", pair)；后者自带
        // 一个 '-'，split_once 已把它吃掉。escaped 即还原口径（查找时
        // 再拼回 "-{escaped}"），不要再剥一层。
        let (key, escaped) = item.split_once('-')?;
        return Some(SettingsRoleControl::Item(
            settings_role_for_key(key)?,
            escaped.to_string(),
        ));
    }
    let key = identifier.strip_prefix(SETTINGS_ROLE_CONTROL_PREFIX)?;
    settings_role_for_key(key).map(SettingsRoleControl::Trigger)
}

/// 四默认角色下拉候选（OPT-3b / ADR-055 D4/D5）：projection.models 已是
/// Host 启用集（model_list 缺省 include_disabled=false），再按 provider
/// 连接态过滤——只有已连接 provider 的模型可选；未连接（含清单缺失）
/// 整组不出现。组间保持目录首现顺序。
pub(crate) fn settings_role_candidates(
    models: &[ModelEntry],
    providers: &[ProviderAuthStatusEntry],
) -> Vec<(String, Vec<ModelEntry>)> {
    group_models_by_provider(models)
        .into_iter()
        .filter(|(provider_id, _)| {
            providers.iter().any(|entry| {
                entry.provider_id == *provider_id
                    && matches!(entry.auth, ProviderAuthState::Connected { .. })
            })
        })
        .collect()
}

/// 角色菜单可点击项的扁平顺序（清除行在外，由调用方计入）；鼠标、键盘
/// 与 AX 均使用这份扁平顺序。
pub(crate) fn settings_role_menu_entries(
    models: &[ModelEntry],
    providers: &[ProviderAuthStatusEntry],
) -> Vec<ModelEntry> {
    settings_role_candidates(models, providers)
        .into_iter()
        .flat_map(|(_, models)| models)
        .collect()
}

/// 角色行右侧用途说明（render 与 AX 同源）。Vision / Search 落地期只保存
/// 选择、路由未接线（B5 / B1），说明如实标注、不暗示已生效。
pub(crate) fn settings_role_description_label(role: SettingsRole) -> String {
    match role {
        SettingsRole::Conversation => t("settings.roles.conversation_desc").to_string(),
        SettingsRole::Naming => t("settings.roles.naming_desc").to_string(),
        SettingsRole::Vision => format!(
            "{} · {}",
            t("settings.roles.vision_desc"),
            t("settings.roles.save_only_vision")
        ),
        SettingsRole::Search => format!(
            "{} · {}",
            t("settings.roles.search_desc"),
            t("settings.roles.save_only_search")
        ),
    }
}

/// 前缀锚定解析 settings 控件 identifier；provider 部分是转义后的 id，
/// 由 AppView 对照 provider 列表还原（未知 id fail-closed）。
pub(crate) fn parse_settings_control(identifier: &str) -> Option<SettingsControl> {
    let rest = identifier.strip_prefix(SETTINGS_CONTROL_PREFIX)?;
    if let Some(provider) = rest.strip_prefix("api-key-input-") {
        return Some(SettingsControl::ApiKeyInput(provider.to_string()));
    }
    if let Some(provider) = rest.strip_prefix("use-proxy-") {
        return Some(SettingsControl::UseProxy(provider.to_string()));
    }
    if let Some(provider) = rest.strip_prefix("expand-") {
        return Some(SettingsControl::Expand(provider.to_string()));
    }
    // 已知 action key 集合有限且互不为前缀（均以 '-' 收尾成段），
    // 逐个前缀匹配消解复合 key（connect-oauth 等）。
    for key in [
        "connect-api-key",
        "connect-oauth",
        "replace-oauth",
        "cancel-oauth",
        "replace-api-key",
        "verify-api-key",
        "cancel-api-key",
        "confirm-remove",
        "keep-remove",
        "remove",
    ] {
        if let Some(provider) = rest.strip_prefix(&format!("{key}-")) {
            return Some(SettingsControl::Action(
                SettingsAuthAction::from_key(key)?,
                provider.to_string(),
            ));
        }
    }
    None
}

/// 「工具与 MCP」页写动作（SET-6c / ADR-049）。render / 键盘 / AX 三路径
/// 同源：可见按钮、on_activate 与 AX Press 共用同一 identifier 与同一
/// 入口 gate；Remove 走两步确认（先 Remove 再 ConfirmRemove）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsMcpAction {
    Test,
    Remove,
    ConfirmRemove,
    KeepRemove,
}

/// MCP 控件 identifier 前缀（action key 在 server 名之前，前缀锚定解析
/// 无歧义；与 provider 动作的 SETTINGS_CONTROL_PREFIX 区分）。
pub(crate) const SETTINGS_MCP_CONTROL_PREFIX: &str = "settings-mcp-";

impl SettingsMcpAction {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Remove => "remove",
            Self::ConfirmRemove => "confirm-remove",
            Self::KeepRemove => "keep-remove",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "test" => Some(Self::Test),
            "remove" => Some(Self::Remove),
            "confirm-remove" => Some(Self::ConfirmRemove),
            "keep-remove" => Some(Self::KeepRemove),
            _ => None,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Test => t("settings.tools.action_test"),
            Self::Remove => t("settings.tools.action_remove"),
            Self::ConfirmRemove => t("settings.tools.action_confirm_remove"),
            Self::KeepRemove => t("settings.tools.action_keep"),
        }
    }

    /// 控件 identifier（render 按钮 id / AX 节点 id / 派发键三用；server
    /// 名经 dynamic_identifier 转义）。
    pub(crate) fn identifier(&self, server: &str) -> String {
        format!(
            "{SETTINGS_MCP_CONTROL_PREFIX}{}",
            dynamic_identifier(self.key(), server)
        )
    }
}

/// 前缀锚定解析「工具与 MCP」页控件 identifier；server 部分是转义后的
/// 名，由 AppView 对照当前权威清单还原（未知 fail-closed）。
pub(crate) fn parse_settings_mcp_control(identifier: &str) -> Option<(SettingsMcpAction, String)> {
    let rest = identifier.strip_prefix(SETTINGS_MCP_CONTROL_PREFIX)?;
    // key 集合有限；confirm-remove / keep-remove 须先于 remove 匹配。
    for key in ["confirm-remove", "keep-remove", "remove", "test"] {
        if let Some(escaped) = rest.strip_prefix(&format!("{key}-")) {
            return Some((SettingsMcpAction::from_key(key)?, escaped.to_string()));
        }
    }
    None
}

/// 按 Host descriptor（auth_methods + auth.type）推导卡片可见写动作；
/// 未知 method 不臆造入口（fail-closed）。
pub(crate) fn settings_auth_actions(
    provider: &ProviderAuthStatusEntry,
    api_key_editor_open: bool,
    remove_confirm: bool,
    oauth_waiting: bool,
) -> Vec<SettingsAuthAction> {
    let mut actions = Vec::new();
    match provider.auth {
        ProviderAuthState::None | ProviderAuthState::Error { .. } => {
            for method in &provider.auth_methods {
                match method.as_str() {
                    "api_key" => {
                        if api_key_editor_open {
                            actions.push(SettingsAuthAction::VerifyApiKey);
                            actions.push(SettingsAuthAction::CancelApiKeyInput);
                        } else {
                            actions.push(SettingsAuthAction::ConnectApiKey);
                        }
                    }
                    "oauth" => actions.push(SettingsAuthAction::ConnectOauth),
                    _ => {}
                }
            }
        }
        ProviderAuthState::Connecting => {
            // oauth 等待中可取消；api_key 验证是 Host 单次同步请求，
            // 无中途取消（Host auth_cancel 对 api_key 显式拒绝）。
            if oauth_waiting {
                actions.push(SettingsAuthAction::CancelOauth);
            }
        }
        ProviderAuthState::Connected { .. } => {
            for method in &provider.auth_methods {
                match method.as_str() {
                    "api_key" => {
                        if api_key_editor_open {
                            actions.push(SettingsAuthAction::VerifyApiKey);
                            actions.push(SettingsAuthAction::CancelApiKeyInput);
                        } else {
                            actions.push(SettingsAuthAction::ReplaceApiKey);
                        }
                    }
                    "oauth" => actions.push(SettingsAuthAction::ReplaceOauth),
                    _ => {}
                }
            }
            if remove_confirm {
                actions.push(SettingsAuthAction::ConfirmRemove);
                actions.push(SettingsAuthAction::KeepRemove);
            } else {
                actions.push(SettingsAuthAction::Remove);
            }
        }
    }
    actions
}

pub(super) fn status_line(text: &str, color: gpui::Rgba) -> impl IntoElement {
    div().child(
        Label::new(text.to_string())
            .size(font::BODY_SM)
            .color(color),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        parse_settings_control, parse_settings_models_control, parse_settings_role_control,
        parse_terminal_dimension, parse_terminal_shell, settings_manage_models_identifier,
        settings_model_switch_identifier, settings_models_disable_all_identifier,
        settings_models_enable_all_identifier, settings_models_refresh_identifier,
        settings_provider_expand_identifier, settings_role_candidates,
        settings_role_clear_identifier, settings_role_item_identifier, settings_role_menu_entries,
        settings_role_trigger_identifier, terminal_save_enabled, SettingsControl,
        SettingsModelsControl, SettingsRoleControl,
    };
    use crate::projection::{
        ModelEntry, ProviderAuthState, ProviderAuthStatusEntry, ProviderCatalogState, SettingsRole,
    };

    #[test]
    fn terminal_save_allows_empty_shell_when_size_is_valid() {
        assert_eq!(parse_terminal_shell("   "), None);
        assert_eq!(parse_terminal_shell("/bin/zsh"), Some("/bin/zsh".into()));
        assert!(terminal_save_enabled(true, Some(80), Some(24)));
        assert!(!terminal_save_enabled(true, None, Some(24)));
        assert!(!terminal_save_enabled(false, Some(80), Some(24)));
        assert_eq!(parse_terminal_dimension(" 120 "), Some(120));
        assert_eq!(parse_terminal_dimension("1"), None);
        assert_eq!(parse_terminal_dimension("2000"), None);
    }

    /// OPT-3b / ADR-055 D4/D5：角色下拉候选只含已连接 provider 的启用
    /// 模型；未连接（含 provider 清单缺失）整组不出现。
    #[test]
    fn role_candidates_keep_only_connected_providers() {
        let model = |provider_id: &str, id: &str| ModelEntry {
            provider_id: provider_id.to_string(),
            id: id.to_string(),
            display_name: format!("{provider_id}/{id}"),
            context_window_tokens: None,
            enabled: true,
        };
        let provider = |provider_id: &str, auth: ProviderAuthState| ProviderAuthStatusEntry {
            provider_id: provider_id.to_string(),
            display_name: provider_id.to_string(),
            endpoint_label: String::new(),
            auth_methods: vec!["api_key".to_string()],
            credentials: Vec::new(),
            auth,
            catalog: ProviderCatalogState::Unavailable {
                error: "offline".to_string(),
                fetched_at: None,
            },
            use_proxy: true,
        };
        let models = vec![
            model("kimi", "kimi-k2"),
            model("glm", "glm-4.7"),
            model("ghost", "ghost-x"),
        ];
        let providers = vec![
            provider(
                "kimi",
                ProviderAuthState::Connected {
                    method: "api_key".to_string(),
                    masked_credential: None,
                },
            ),
            provider("glm", ProviderAuthState::None),
        ];
        let candidates = settings_role_candidates(&models, &providers);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, "kimi");
        assert_eq!(candidates[0].1.len(), 1);
        assert_eq!(candidates[0].1[0].id, "kimi-k2");
        // 扁平顺序与分组一致：glm（未连接）与 ghost（清单缺失）均不出现。
        let flat = settings_role_menu_entries(&models, &providers);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].provider_id, "kimi");
    }

    /// OPT-3b：角色控件 identifier 三路径同源——构造与解析互逆；
    /// 静态 label 与未知形状 fail-closed。
    #[test]
    fn settings_role_control_identifiers_roundtrip_and_fail_closed() {
        let trigger = settings_role_trigger_identifier(SettingsRole::Naming);
        assert_eq!(trigger, "settings-role-naming");
        assert_eq!(
            parse_settings_role_control(&trigger),
            Some(SettingsRoleControl::Trigger(SettingsRole::Naming))
        );

        let clear = settings_role_clear_identifier(SettingsRole::Vision);
        assert_eq!(
            parse_settings_role_control(&clear),
            Some(SettingsRoleControl::Clear(SettingsRole::Vision))
        );

        let item = settings_role_item_identifier(SettingsRole::Conversation, "kimi", "kimi-k2");
        assert_eq!(item, "settings-role-item-conversation-kimi_3akimi-k2");
        assert_eq!(
            parse_settings_role_control(&item),
            Some(SettingsRoleControl::Item(
                SettingsRole::Conversation,
                "kimi_3akimi-k2".into()
            ))
        );

        assert_eq!(
            parse_settings_role_control("settings-role-label-naming"),
            None
        );
        assert_eq!(
            parse_settings_role_control("settings-models-manage-kimi"),
            None
        );
        assert_eq!(parse_settings_role_control("settings-role-item-"), None);
    }

    /// OPT-3a：Manage models 弹层控件 identifier 三路径（render / 键盘 /
    /// AX）同源——构造与解析互逆，provider:model 对里的非白名单字节靠
    /// 转义消歧；面板等非派发 id 与未知形状 fail-closed。
    #[test]
    fn settings_models_control_identifiers_roundtrip_and_fail_closed() {
        let manage = settings_manage_models_identifier("glm-coding");
        assert_eq!(manage, "settings-models-manage-glm-coding");
        assert_eq!(
            parse_settings_models_control(&manage),
            Some(SettingsModelsControl::Manage("glm-coding".into()))
        );

        // ':'（0x3a）转义为 _3a：provider:model 拼接仍无歧义，还原由
        // AppView 对照 model_catalog 还原。
        let toggle = settings_model_switch_identifier("kimi", "kimi-k2-0905");
        assert_eq!(toggle, "settings-models-toggle-kimi_3akimi-k2-0905");
        assert_eq!(
            parse_settings_models_control(&toggle),
            Some(SettingsModelsControl::Toggle("kimi_3akimi-k2-0905".into()))
        );

        let enable = settings_models_enable_all_identifier("glm-coding");
        assert_eq!(
            parse_settings_models_control(&enable),
            Some(SettingsModelsControl::EnableAll("glm-coding".into()))
        );
        let disable = settings_models_disable_all_identifier("glm-coding");
        assert_eq!(
            parse_settings_models_control(&disable),
            Some(SettingsModelsControl::DisableAll("glm-coding".into()))
        );
        let refresh = settings_models_refresh_identifier("glm-coding");
        assert_eq!(
            parse_settings_models_control(&refresh),
            Some(SettingsModelsControl::RefreshCatalog("glm-coding".into()))
        );

        // 弹层面板 id 不参与派发；无前缀 / 其他页前缀 fail-closed。
        assert_eq!(
            parse_settings_models_control("settings-models-menu-glm-coding"),
            None
        );
        assert_eq!(
            parse_settings_models_control("settings-role-clear-naming"),
            None
        );
        assert_eq!(parse_settings_models_control("use-proxy-glm-coding"), None);
    }

    /// ADR-056 D4：卡片展开 chevron identifier 三路径（render / 键盘 /
    /// AX）同源——构造与解析互逆；与代理开关等其他控件前缀不冲突。
    #[test]
    fn settings_provider_expand_identifier_roundtrips() {
        let id = settings_provider_expand_identifier("glm-coding");
        assert_eq!(id, "settings-action-expand-glm-coding");
        assert_eq!(
            parse_settings_control(&id),
            Some(SettingsControl::Expand("glm-coding".into()))
        );
        // 空转义段与既有 use-proxy 形状一致：派发侧按 provider 清单
        // 还原，未知 fail-closed；非本控件前缀不解析。
        assert_eq!(parse_settings_control("settings-expand-glm-coding"), None);
    }
}

mod about;
mod advanced;
mod appearance;
mod approval_labels;
mod general;
mod permissions;
mod providers;
mod terminal;
mod tools;

pub(crate) use approval_labels::{
    description as approval_mode_description, label as approval_mode_label,
    ALL as APPROVAL_MODE_ALL,
};

impl AppView {
    /// Settings 左栏（SET-3）：返回工作台 + 首个导航项「Models & providers」。
    /// 宽度沿用 TaskRail 的响应式 rail（288 / 240 / 320），进入时整体替换
    /// TaskRail；未接通页面不显示（无假导航项）。
    pub(super) fn settings_rail_element(
        &mut self,
        rail_width: Pixels,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Panel {
        let back_focus = self.settings_back_focus.clone();
        let back = Button::new("settings-back")
            .track_focus(&back_focus)
            .variant(ButtonVariant::Raised)
            .padding(ButtonPadding::Horizontal(metrics::RAIL_INNER_PAD))
            .height(px(metrics::RAIL_TOP_ROW_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY)
            .label(t("settings.back"))
            .tooltip(t("settings.back_tooltip"))
            .on_click(cx.listener(|view, event, window, cx| {
                if view.consume_button_key_click("settings-back", event) {
                    return;
                }
                view.on_close_settings(window, cx);
            }))
            .on_activate(cx.listener(|view, _event, window, cx| {
                view.note_button_key_activate("settings-back");
                view.on_close_settings(window, cx);
                cx.stop_propagation();
            }));
        let general_available = self.projection.settings_general.query.available;
        let permissions_available = self.projection.settings_permissions.query.available;
        let tools_available = self.resources.available;
        let terminal_available = self.projection.settings_terminal.query.available;
        let about_available = self.settings_about_rows().is_some();
        let current_page = match self.settings_page {
            SettingsPage::General if !general_available => SettingsPage::Providers,
            SettingsPage::Permissions if !permissions_available => SettingsPage::Providers,
            SettingsPage::Tools if !tools_available => SettingsPage::Providers,
            SettingsPage::Terminal if !terminal_available => SettingsPage::Providers,
            SettingsPage::About if !about_available => SettingsPage::Advanced,
            page => page,
        };
        let mut rail = Panel::side_right(rail_width)
            .child(shell_layout::rail_safe_area())
            .child(
                div()
                    .id("settings-rail-title")
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px(px(metrics::RAIL_INNER_PAD))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        Label::new(t("settings.rail_title"))
                            .size(font::TITLE)
                            .color(dark().text.primary),
                    ),
            )
            .child(back)
            .child(self.settings_nav_item(
                "settings-nav-providers",
                t("settings.nav.providers"),
                current_page == SettingsPage::Providers,
                SettingsPage::Providers,
                window,
                cx,
            ));
        if general_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-general",
                t("settings.nav.general"),
                current_page == SettingsPage::General,
                SettingsPage::General,
                window,
                cx,
            ));
        }
        if permissions_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-permissions",
                t("settings.nav.permissions"),
                current_page == SettingsPage::Permissions,
                SettingsPage::Permissions,
                window,
                cx,
            ));
        }
        if tools_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-tools",
                t("settings.nav.tools"),
                current_page == SettingsPage::Tools,
                SettingsPage::Tools,
                window,
                cx,
            ));
        }
        if terminal_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-terminal",
                t("settings.nav.terminal"),
                current_page == SettingsPage::Terminal,
                SettingsPage::Terminal,
                window,
                cx,
            ));
        }
        rail = rail.child(self.settings_nav_item(
            "settings-nav-appearance",
            t("settings.nav.appearance"),
            current_page == SettingsPage::Appearance,
            SettingsPage::Appearance,
            window,
            cx,
        ));
        rail = rail.child(self.settings_nav_item(
            "settings-nav-advanced",
            t("settings.nav.advanced"),
            current_page == SettingsPage::Advanced,
            SettingsPage::Advanced,
            window,
            cx,
        ));
        if about_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-about",
                t("settings.nav.about"),
                current_page == SettingsPage::About,
                SettingsPage::About,
                window,
                cx,
            ));
        }
        rail
    }

    /// Settings 全宽内容区（SET-4 认证写操作）。状态行全部来自
    /// projection（Host 权威 / stale / error）；卡片动作由 descriptor 驱动，
    /// 断线（stale）时可见 / 键盘 / AX 同时禁用。
    pub(super) fn settings_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.settings_page == SettingsPage::General
            && self.projection.settings_general.query.available
        {
            self.settings_general_page_element(cx).into_any_element()
        } else if self.settings_page == SettingsPage::Permissions
            && self.projection.settings_permissions.query.available
        {
            self
                .settings_permissions_page_element(cx)
                .into_any_element()
        } else if self.settings_page == SettingsPage::Tools && self.resources.available {
            self.settings_tools_page_element(cx).into_any_element()
        } else if self.settings_page == SettingsPage::Terminal
            && self.projection.settings_terminal.query.available
        {
            self.settings_terminal_page_element(cx).into_any_element()
        } else if self.settings_page == SettingsPage::Appearance {
            self.settings_appearance_page_element(cx).into_any_element()
        } else if self.settings_page == SettingsPage::Advanced {
            self.settings_advanced_page_element(cx).into_any_element()
        } else if self.settings_page == SettingsPage::About {
            if self.settings_about_rows().is_some() {
                self.settings_about_page_element().into_any_element()
            } else {
                self.settings_advanced_page_element(cx).into_any_element()
            }
        } else {
            self.settings_providers_page_element(cx).into_any_element()
        };
        // OPT-4c（F2）：内容脚手架统一在共享层——Rail 外全宽、两侧 32px
        //（垂直仍 16px）、受限高度纵向滚动；各页只提供内容列，不再各自
        // 复制 p_4 + 820px 钳制。AX 几何经 settings_content_ax_width 同源。
        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .py_4()
            .px(px(SETTINGS_CONTENT_PAD))
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// OPT-4d（F4）：导航选中态零位移。选中与未选中共用同一外壳几何
    ///（同 w_full / 行高 / 水平 padding / 圆角 / 间距 / 字阶），差异只落在
    /// 背景色、字重与不参与布局的左缘指示条。gpui 的 border 参与 Taffy
    /// 布局（按需出现会推移内容），焦点描边改为持焦时挂载 focus_ring
    /// 覆盖层（零布局参与，见 components/focus_ring.rs），文字坐标在选中
    /// 切换与焦点切换下逐像素不变。
    fn settings_nav_item(
        &mut self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        page: SettingsPage,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let focus = match page {
            SettingsPage::General => self.settings_nav_general_focus.clone(),
            SettingsPage::Permissions => self.settings_nav_permissions_focus.clone(),
            SettingsPage::Tools => self.settings_nav_tools_focus.clone(),
            SettingsPage::Terminal => self.settings_nav_terminal_focus.clone(),
            SettingsPage::Appearance => self.settings_nav_appearance_focus.clone(),
            SettingsPage::Advanced => self.settings_nav_advanced_focus.clone(),
            SettingsPage::About => self.settings_nav_about_focus.clone(),
            SettingsPage::Providers => self.settings_nav_providers_focus.clone(),
        };
        let label_element = if selected {
            div()
                .font_weight(FontWeight::MEDIUM)
                .child(
                    Label::new(label)
                        .size(font::BODY_SM)
                        .color(dark().text.primary),
                )
                .into_any_element()
        } else {
            Label::new(label).size(font::BODY_SM).into_any_element()
        };
        let mut item = div()
            .id(id)
            .tab_stop(true)
            .track_focus(&focus)
            .relative()
            .w_full()
            .h(px(metrics::RAIL_TOP_ROW_HEIGHT))
            .flex()
            .items_center()
            .px(px(metrics::RAIL_INNER_PAD))
            .rounded(px(4.0))
            .text_size(font::BODY_SM)
            .when(focus.is_focused(window), |item| {
                item.child(focus_ring(px(4.0)))
            })
            .child(label_element);
        if selected {
            // 选中表达：raised 背景 + 绝对定位左缘指示条（零布局参与）。
            item = item
                .bg(dark().surface.raised)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(3.0))
                        .bg(dark().accent.primary),
                );
        } else {
            // 未选中：Ghost 同款 hover / active 色映射；click / Enter /
            // Space 与 AX Press 同一 handler（consume_button_key_click
            // 过滤 keyup 合成的键盘 click，与 Button 组件口径一致）。
            item = item
                .cursor_pointer()
                .hover(|style| style.bg(dark().surface.raised))
                .active(|style| style.bg(dark().surface.hover))
                .on_click(cx.listener(move |view, event, window, cx| {
                    if view.consume_button_key_click(id, event) {
                        return;
                    }
                    view.on_select_settings_page(page, window, cx);
                }))
                .on_key_down(cx.listener(
                    move |view: &mut Self,
                          event: &gpui::KeyDownEvent,
                          window: &mut gpui::Window,
                          cx: &mut Context<Self>| {
                        // 与 Button::render 的 on_activate 同一激活语义：
                        // 无修饰键的裸 Enter / Space 直接激活。
                        if !event.keystroke.modifiers.modified()
                            && (event.keystroke.key == "enter"
                                || event.keystroke.key == "space")
                        {
                            view.note_button_key_activate(id);
                            view.on_select_settings_page(page, window, cx);
                            cx.stop_propagation();
                        }
                    },
                ));
        }
        item.into_any_element()
    }

    /// 供应商页写操作 gate（SET-4/5）：连接 + 非 stale。页始终可见，
    /// `available` 默认 true，与 `SettingsQueryGate::writes_enabled` 同口径。
    pub(crate) fn settings_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        self.projection.settings_providers.writes_enabled(connected)
    }

    /// Network 页写操作 gate（SET-6a）：连接 + 非 stale + 查询已成功。
    pub(crate) fn settings_general_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        self.projection.settings_general.writes_enabled(connected)
    }

    /// 权限页写操作 gate（SET-6b）：连接 + 非 stale + 查询已成功。
    pub(crate) fn settings_permissions_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        self.projection
            .settings_permissions
            .writes_enabled(connected)
    }

    /// 终端页写操作 gate（SET-6d）：连接 + 非 stale + 查询已成功。
    pub(crate) fn settings_terminal_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        self.projection.settings_terminal.writes_enabled(connected)
    }

    /// 「工具与 MCP」页写操作 gate（SET-6c）：连接 + 非 stale + mcp_list
    /// 已成功（available；语义与通用 / 权限页一致）。
    pub(crate) fn settings_tools_writes_enabled(&self) -> bool {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        connected && self.resources.available && self.resources.stale_reason.is_none()
    }
}
