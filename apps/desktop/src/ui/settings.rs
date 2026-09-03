//! Settings 壳（SET-3/4/6a/6b/6c/6d/6e/6f/6g）：Settings Rail、「Models &
//! providers」、「General」、「权限与审批」、「工具与 MCP」、「终端」、「外观」
//!、「高级」与「关于」页。
//!
//! 供应商页只呈现 Host `provider_auth_status` 权威事实：供应商名称、
//! 认证方式、连接状态与目录来源（SET-3）；SET-4 增认证写操作（API key
//! secure 输入验证、OAuth 等待/取消、Replace/Remove），全部由 descriptor
//! （auth_methods + auth.type）驱动，禁止按 provider 名分支。SET-6a 增
//! 「General」页（`proxy_url` 读/设置/清除）；SET-6b 增「权限与审批」页
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

use std::collections::HashSet;

use gpui::{div, prelude::*, px, App, Context, FontWeight, Pixels};

use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::label::Label;
use crate::ui::components::panel::Panel;
use crate::ui::theme::{dark, font, metrics};

use crate::controller::McpServerEntry;
use crate::projection::{
    group_models_by_provider, ApprovalModeSetting, ConnectionState, ModelEntry, OAuthWait,
    ProviderAuthState, ProviderStatusEntry, SettingsPermissionsState, SettingsTerminalState,
};
use crate::ui::text_input::TextInput;

use super::accessibility::dynamic_identifier;
use super::resources::{
    mcp_server_meta_text, mcp_server_name_row, ResourcesFetch, ResourcesPanelState,
};
use super::shell_layout;
use super::{AppRoute, AppView, SettingsPage};

/// Settings 内容可读列（与 Timeline 618px 可读列同节奏；全宽壳层内收敛）。
const SETTINGS_CONTENT_MAX_WIDTH: f32 = 720.0;
/// Provider 卡片内边距（8px 节奏）。
const PROVIDER_CARD_PAD: f32 = 8.0;
/// 写动作按钮高度（与 Composer 28px 动作槽同节奏）。
const SETTINGS_ACTION_HEIGHT: f32 = 28.0;
/// 「模型与默认项」区失效提示（render 与 AX 同源；只声明事实，不切换）。
pub(crate) const SETTINGS_DEFAULT_UNAVAILABLE_NOTE: &str = "Default model unavailable — the default provider is disconnected or the model is not in its current catalog.";
/// null `proxy_url` 展示（ADR-047 D1；render / AX 同源）。
pub(crate) const SETTINGS_PROXY_UNSET: &str = "未设置（跟随系统环境变量）";
/// 生效边界（ADR-047 D2；不得宣称全局即时生效）。
pub(crate) const SETTINGS_PROXY_EFFECT_NOTE: &str =
    "新 OAuth/验证/目录探测同会话生效；当前活跃供应商的模型流量于切换供应商或重启 Host 后生效。";

/// null `trust_workspaces_global` 展示（ADR-048 D1；render / AX 同源）。
pub(crate) const SETTINGS_TRUST_UNSET: &str = "未设置（默认不信任）";
/// 权限页生效边界（ADR-048 D2/D3；不得宣称持久化或影响进行中 Run）。
pub(crate) const SETTINGS_PERMISSIONS_EFFECT_NOTE: &str = "以上变更仅当前会话生效、不持久化：重启 Host 后回到默认；进行中的 Run 不受影响，之后启动的 Run 使用新设置。";

/// 「工具与 MCP」页 Remove 二次确认提示（SET-6c / ADR-049 D2；render 与
/// AX 同源，诚实标注快照语义）。
pub(crate) const SETTINGS_MCP_REMOVE_CONFIRM_NOTE: &str =
    "移除将写回 Global 配置并清理该 server 的凭证；进行中 Run 已快照的工具不会回溯撤销。";
/// 「工具与 MCP」页生效边界（SET-6c / ADR-049 D1/D2；render 与 AX 同源）。
pub(crate) const SETTINGS_MCP_EFFECT_NOTE: &str =
    "Test 现场验证该 server 并回写状态；移除作用于 Global 配置并清理凭证，同会话内其工具不再注册。";

/// null shell 展示（SET-6d / ADR-050 D2；render 与 AX 同源）。
pub(crate) const SETTINGS_TERMINAL_SHELL_UNSET: &str = "未设置（跟随平台默认）";
/// 终端页生效边界（SET-6d / ADR-050 D4；render 与 AX 同源，快照语义）。
pub(crate) const SETTINGS_TERMINAL_EFFECT_NOTE: &str = "只影响之后创建的终端，已存在终端不变。";

/// 外观页主题说明（SET-6e）：只陈述已实现能力，不画未实现的
/// light / system 主题控件。render / AX 同源。
pub(crate) const SETTINGS_APPEARANCE_THEME_NOTE: &str =
    "当前仅提供深色主题；macOS Increase Contrast 由系统控制并自动刷新。";
/// 外观页字号生效边界（SET-6e）：本片不引入第二套配置或假持久化。
pub(crate) const SETTINGS_APPEARANCE_EFFECT_NOTE: &str =
    "字号立即应用于当前 Desktop 会话；重启后恢复 100%。也可用 Cmd+= / Cmd+- / Cmd+0 调整。";

/// 高级页启动目标边界（SET-6f）：runtime ID 不能冒充 CLI 配置实例名，
/// 且任何凭证及其路径都不进入 render / AX。
pub(crate) const SETTINGS_ADVANCED_TARGET_NOTE: &str = "Endpoint 由 Desktop 启动时的 --instance / --socket 决定；切换需重启 Desktop。Host runtime ID 不是配置实例名。本页不显示 GUI token 或 token path。";
/// Host 级自检仍由 pre-Core CLI 命令负责；Desktop 不 shell-out，也不从
/// socket 路径推断 data directory / 配置实例名。
pub(crate) const SETTINGS_ADVANCED_DOCTOR_NOTE: &str = "Host 级 data directory、PID、socket 存活与握手自检请使用 pawork --instance <name> doctor；本页不推断实例名，也不运行该命令。";
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
pub(super) fn provider_status_lines(
    state: &crate::projection::SettingsProvidersState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.error {
        lines.push(("error", format!("Could not load provider status · {error}")));
    }
    if state.providers.is_empty()
        && !state.loading
        && state.error.is_none()
        && state.stale_reason.is_none()
    {
        lines.push(("empty", "No providers reported by the host.".to_string()));
    }
    lines
}

/// Settings 通用页状态行（render 与 AX 同源）。error 文案由事件消费侧
/// 按动作区分（load vs save），此处原样展示。
pub(super) fn general_status_lines(
    state: &crate::projection::SettingsGeneralState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings 权限页状态行（render 与 AX 同源）。error 文案由事件消费侧
/// 按动作区分（load / set mode / set trust），此处原样展示。
pub(super) fn permissions_status_lines(
    state: &SettingsPermissionsState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings 终端页状态行（SET-6d；render 与 AX 同源）。error 文案由事件
/// 消费侧按动作区分（load / set），此处原样展示。
pub(super) fn terminal_status_lines(state: &SettingsTerminalState) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.error {
        lines.push(("error", error.clone()));
    }
    lines
}

/// Settings「工具与 MCP」页状态行（SET-6c；render 与 AX 同源）。复用
/// Resources 面状态：stale / loading / error / 空态独立判定。
pub(super) fn tools_status_lines(state: &ResourcesPanelState) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if matches!(state.fetch, ResourcesFetch::Fetching) {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let ResourcesFetch::Failed(reason) = &state.fetch {
        lines.push(("error", format!("Could not load MCP servers · {reason}")));
    }
    if let Some(error) = &state.action_error {
        lines.push(("action", error.clone()));
    }
    if state.servers.is_empty()
        && !matches!(state.fetch, ResourcesFetch::Fetching)
        && state.stale_reason.is_none()
        && !matches!(state.fetch, ResourcesFetch::Failed(_))
    {
        lines.push(("empty", "No MCP servers configured.".to_string()));
    }
    lines
}

/// Settings 写动作（SET-4）。render / 键盘 / AX 三路径同源：可见按钮、
/// on_activate 与 AX Press 共用同一 identifier 与同一入口 gate。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsAuthAction {
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
    pub(crate) const ALL: [Self; 9] = [
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
            Self::ConnectOauth => "Connect",
            Self::ReplaceOauth => "Replace OAuth",
            Self::CancelOauth => "Cancel",
            Self::ReplaceApiKey => "Replace API key",
            Self::VerifyApiKey => "Verify",
            Self::CancelApiKeyInput => "Cancel",
            Self::Remove => "Remove",
            Self::ConfirmRemove => "Remove connection",
            Self::KeepRemove => "Keep",
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
    /// 设为默认（SET-5）：携带转义后的 "<provider>:<model>"，由 AppView
    /// 对照 projection.models 还原（未知 fail-closed）。
    SetDefaultModel(String),
}

pub(crate) fn settings_api_key_input_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_CONTROL_PREFIX}{}",
        dynamic_identifier("api-key-input", provider_id)
    )
}

/// 「设为默认」控件 identifier（render 按钮 id / AX 节点 id / 派发键三用；
/// provider 与 model 以 ':' 拼接后整体转义）。
pub(crate) fn settings_set_default_identifier(provider_id: &str, model_id: &str) -> String {
    format!(
        "{SETTINGS_CONTROL_PREFIX}{}",
        dynamic_identifier("set-default", &format!("{provider_id}:{model_id}"))
    )
}

/// 前缀锚定解析 settings 控件 identifier；provider 部分是转义后的 id，
/// 由 AppView 对照 provider 列表还原（未知 id fail-closed）。
pub(crate) fn parse_settings_control(identifier: &str) -> Option<SettingsControl> {
    let rest = identifier.strip_prefix(SETTINGS_CONTROL_PREFIX)?;
    if let Some(provider) = rest.strip_prefix("api-key-input-") {
        return Some(SettingsControl::ApiKeyInput(provider.to_string()));
    }
    if let Some(target) = rest.strip_prefix("set-default-") {
        return Some(SettingsControl::SetDefaultModel(target.to_string()));
    }
    // 已知 action key 集合有限且互不为前缀（均以 '-' 收尾成段），
    // 逐个前缀匹配消解复合 key（connect-oauth 等）。
    for key in [
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
            Self::Test => "Test",
            Self::Remove => "移除",
            Self::ConfirmRemove => "确认移除",
            Self::KeepRemove => "保留",
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
    provider: &ProviderStatusEntry,
    api_key_editor_open: bool,
    remove_confirm: bool,
    oauth_waiting: bool,
) -> Vec<SettingsAuthAction> {
    let mut actions = Vec::new();
    match provider.auth {
        ProviderAuthState::NotConnected | ProviderAuthState::Error { .. } => {
            for method in &provider.auth_methods {
                match method.as_str() {
                    "api_key" => {
                        actions.push(SettingsAuthAction::VerifyApiKey);
                        actions.push(SettingsAuthAction::CancelApiKeyInput);
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

impl AppView {
    /// Settings 左栏（SET-3）：返回工作台 + 首个导航项「Models & providers」。
    /// 宽度沿用 TaskRail 的响应式 rail（288 / 240 / 320），进入时整体替换
    /// TaskRail；未接通页面不显示（无假导航项）。
    pub(super) fn settings_rail_element(
        &mut self,
        rail_width: Pixels,
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
            .label("← Back to workspace")
            .tooltip("Back to workspace")
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
        let general_available = self.projection.settings_general.available;
        let permissions_available = self.projection.settings_permissions.available;
        let tools_available = self.resources.available;
        let terminal_available = self.projection.settings_terminal.available;
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
            .child(back)
            .child(self.settings_nav_item(
                "settings-nav-providers",
                "Models & providers",
                current_page == SettingsPage::Providers,
                SettingsPage::Providers,
                cx,
            ));
        if general_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-general",
                "General",
                current_page == SettingsPage::General,
                SettingsPage::General,
                cx,
            ));
        }
        if permissions_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-permissions",
                "权限与审批",
                current_page == SettingsPage::Permissions,
                SettingsPage::Permissions,
                cx,
            ));
        }
        if tools_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-tools",
                "工具与 MCP",
                current_page == SettingsPage::Tools,
                SettingsPage::Tools,
                cx,
            ));
        }
        if terminal_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-terminal",
                "终端",
                current_page == SettingsPage::Terminal,
                SettingsPage::Terminal,
                cx,
            ));
        }
        rail = rail.child(self.settings_nav_item(
            "settings-nav-appearance",
            "外观",
            current_page == SettingsPage::Appearance,
            SettingsPage::Appearance,
            cx,
        ));
        rail = rail.child(self.settings_nav_item(
            "settings-nav-advanced",
            "高级",
            current_page == SettingsPage::Advanced,
            SettingsPage::Advanced,
            cx,
        ));
        if about_available {
            rail = rail.child(self.settings_nav_item(
                "settings-nav-about",
                "关于",
                current_page == SettingsPage::About,
                SettingsPage::About,
                cx,
            ));
        }
        rail
    }

    /// Settings 全宽内容区（SET-4 认证写操作）。状态行全部来自
    /// projection（Host 权威 / stale / error）；卡片动作由 descriptor 驱动，
    /// 断线（stale）时可见 / 键盘 / AX 同时禁用。
    pub(super) fn settings_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.settings_page == SettingsPage::General && self.projection.settings_general.available
        {
            return self.settings_general_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::Permissions
            && self.projection.settings_permissions.available
        {
            return self
                .settings_permissions_page_element(cx)
                .into_any_element();
        }
        if self.settings_page == SettingsPage::Tools && self.resources.available {
            return self.settings_tools_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::Terminal
            && self.projection.settings_terminal.available
        {
            return self.settings_terminal_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::Appearance {
            return self.settings_appearance_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::Advanced {
            return self.settings_advanced_page_element(cx).into_any_element();
        }
        if self.settings_page == SettingsPage::About {
            if self.settings_about_rows().is_some() {
                return self.settings_about_page_element().into_any_element();
            }
            return self.settings_advanced_page_element(cx).into_any_element();
        }
        self.settings_providers_page_element(cx).into_any_element()
    }

    fn settings_nav_item(
        &mut self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        page: SettingsPage,
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
        if selected {
            return div()
                .id(id)
                .tab_stop(true)
                .track_focus(&focus)
                .focus(|style| style.border_1().border_color(dark().accent.primary))
                .mt_2()
                .w_full()
                .h(px(metrics::RAIL_TOP_ROW_HEIGHT))
                .flex()
                .items_center()
                .px(px(metrics::RAIL_INNER_PAD))
                .rounded(px(4.0))
                .bg(dark().surface.raised)
                .child(
                    div().font_weight(FontWeight::MEDIUM).child(
                        Label::new(label)
                            .size(font::BODY_SM)
                            .color(dark().text.primary),
                    ),
                )
                .into_any_element();
        }
        Button::new(id)
            .track_focus(&focus)
            .variant(ButtonVariant::Ghost)
            .padding(ButtonPadding::Horizontal(metrics::RAIL_INNER_PAD))
            .height(px(metrics::RAIL_TOP_ROW_HEIGHT))
            .vcenter()
            .radius(4.0)
            .text_size(font::BODY_SM)
            .label(label)
            .on_click(cx.listener(move |view, event, window, cx| {
                if view.consume_button_key_click(id, event) {
                    return;
                }
                view.on_select_settings_page(page, window, cx);
            }))
            .on_activate(cx.listener(move |view, _event, window, cx| {
                view.note_button_key_activate(id);
                view.on_select_settings_page(page, window, cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    /// 「Models & providers」页（SET-4/5）：状态行全部来自 projection；
    /// 卡片动作由 descriptor 驱动，断线（stale）时可见 / 键盘 / AX 同时禁用。
    fn settings_providers_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let state = &self.projection.settings_providers;
        let writes = connected && state.stale_reason.is_none();
        let status_lines = provider_status_lines(state);
        let providers = state.providers.clone();
        let oauth_waits = state.oauth_waits.clone();
        let auth_notes = state.auth_notes.clone();
        let page = div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4();

        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2();
        // 页级刷新（SET-5）：重查 provider_auth_status + model_list；断线
        // 禁用（与 AX / 入口 gate 同源）。
        let refresh_enabled = connected;
        let refresh_focus = self.settings_refresh_focus.clone();
        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Refresh")
            .tooltip("Refresh provider status and model catalog")
            .disabled(!refresh_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-refresh", event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-refresh");
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));
        content = content.child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div().font_weight(FontWeight::MEDIUM).child(
                                Label::new("Models & providers")
                                    .size(font::TITLE)
                                    .color(dark().text.primary),
                            ),
                        )
                        .child(
                            Label::new("Connection status and catalog source for each provider")
                                .size(font::BODY_SM)
                                .color(dark().text.secondary),
                        ),
                )
                .child(div().flex_1())
                .child(div().flex_none().pt_1().child(refresh)),
        );

        // 状态行（不只靠颜色区分）：与 AX 共用 provider_status_lines，
        // stale / loading / error / 空态独立发布。
        for (kind, line) in status_lines {
            let color = if kind == "error" {
                dark().semantic.danger_text
            } else {
                dark().text.secondary
            };
            content = content.child(status_line(&line, color));
        }

        if !providers.is_empty() {
            let mut cards = div().flex().flex_col().min_w_0().gap_2();
            for (ix, provider) in providers.iter().enumerate() {
                cards = cards.child(self.settings_provider_card(
                    ix,
                    provider,
                    &oauth_waits,
                    &auth_notes,
                    writes,
                    cx,
                ));
            }
            content = content.child(cards);
        }

        // SET-5:「模型与默认项」区（供应商列表下方）。
        content = content.child(self.settings_models_section(cx));

        page.child(
            div()
                .id("settings-page-scroll")
                .flex_1()
                .min_h_0()
                .track_scroll(&self.settings_scroll)
                .child(content),
        )
    }

    /// 「General」页（SET-6a / ADR-047）：Host 权威 proxy_url、内联输入 +
    /// Save/Clear、生效边界文案；stale 只读，写入口与 AX 同 gate。
    fn settings_general_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let writes = self.settings_general_writes_enabled();
        let state = &self.projection.settings_general;
        let status_lines = general_status_lines(state);
        let current = match &state.proxy_url {
            Some(url) => url.clone(),
            None => SETTINGS_PROXY_UNSET.to_string(),
        };
        let input_empty = self.settings_proxy_input.read(cx).text().trim().is_empty();
        let save_enabled = writes && !input_empty;
        let clear_enabled = writes && state.proxy_url.is_some();
        let proxy_input = self.settings_proxy_input.clone();
        let save_focus = self.settings_proxy_save_focus.clone();
        let clear_focus = self.settings_proxy_clear_focus.clone();
        let refresh_focus = self.settings_refresh_focus.clone();
        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Refresh")
            .tooltip("Refresh general settings")
            .disabled(!connected)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-refresh", event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-refresh");
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));
        let save = Button::new("settings-proxy-save")
            .track_focus(&save_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Save")
            .tooltip("Save proxy URL")
            .disabled(!save_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-proxy-save", event) {
                    return;
                }
                view.on_settings_proxy_save(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-proxy-save");
                view.on_settings_proxy_save(cx);
                cx.stop_propagation();
            }));
        let clear = Button::new("settings-proxy-clear")
            .track_focus(&clear_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Clear")
            .tooltip("Clear proxy URL")
            .disabled(!clear_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-proxy-clear", event) {
                    return;
                }
                view.on_settings_proxy_clear(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-proxy-clear");
                view.on_settings_proxy_clear(cx);
                cx.stop_propagation();
            }));

        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div().font_weight(FontWeight::MEDIUM).child(
                                    Label::new("General")
                                        .size(font::TITLE)
                                        .color(dark().text.primary),
                                ),
                            )
                            .child(
                                Label::new("Host outbound HTTP proxy")
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().flex_none().pt_1().child(refresh)),
            );
        for (kind, line) in status_lines {
            let color = if kind == "error" {
                dark().semantic.danger_text
            } else {
                dark().text.secondary
            };
            content = content.child(status_line(&line, color));
        }
        content = content
            .child(
                Label::new(format!("Current · {current}"))
                    .size(font::BODY)
                    .color(dark().text.primary),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!writes, |el| el.bg(dark().surface.disabled).opacity(0.55))
                            .child(proxy_input),
                    )
                    .child(save)
                    .child(clear),
            )
            .child(
                Label::new(SETTINGS_PROXY_EFFECT_NOTE)
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// 「终端」页（SET-6d / ADR-050）：Host 权威生效值（shell 持久值 +
    /// columns/rows 生效值）、shell 内联输入 + columns/rows 数值输入 +
    /// Save（全态回传三字段）/ Clear（清除 shell）、生效边界文案；stale
    /// 只读，写入口与 AX 同 gate。
    fn settings_terminal_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let writes = self.settings_terminal_writes_enabled();
        let state = self.projection.settings_terminal.clone();
        let status_lines = terminal_status_lines(&state);
        let shell_current = state
            .shell
            .clone()
            .unwrap_or_else(|| SETTINGS_TERMINAL_SHELL_UNSET.to_string());
        let size_current = format!("{}×{}", state.columns, state.rows);
        let columns_value =
            parse_terminal_dimension(self.settings_terminal_columns_input.read(cx).text());
        let rows_value =
            parse_terminal_dimension(self.settings_terminal_rows_input.read(cx).text());
        let save_enabled = terminal_save_enabled(writes, columns_value, rows_value);
        let clear_enabled = writes && state.shell.is_some();
        let shell_input = self.settings_terminal_shell_input.clone();
        let columns_input = self.settings_terminal_columns_input.clone();
        let rows_input = self.settings_terminal_rows_input.clone();
        let save_focus = self.settings_terminal_save_focus.clone();
        let clear_focus = self.settings_terminal_clear_focus.clone();
        let refresh_focus = self.settings_refresh_focus.clone();
        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Refresh")
            .tooltip("Refresh terminal settings")
            .disabled(!connected)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-refresh", event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-refresh");
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));
        let save = Button::new("settings-terminal-save")
            .track_focus(&save_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Save")
            .tooltip("Save terminal defaults (shell, columns, rows)")
            .disabled(!save_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-terminal-save", event) {
                    return;
                }
                view.on_settings_terminal_save(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-terminal-save");
                view.on_settings_terminal_save(cx);
                cx.stop_propagation();
            }));
        let clear = Button::new("settings-terminal-clear")
            .track_focus(&clear_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Clear")
            .tooltip("Clear default shell")
            .disabled(!clear_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-terminal-clear", event) {
                    return;
                }
                view.on_settings_terminal_clear(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-terminal-clear");
                view.on_settings_terminal_clear(cx);
                cx.stop_propagation();
            }));

        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div().font_weight(FontWeight::MEDIUM).child(
                                    Label::new("终端")
                                        .size(font::TITLE)
                                        .color(dark().text.primary),
                                ),
                            )
                            .child(
                                Label::new("Default shell and size for new terminals")
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().flex_none().pt_1().child(refresh)),
            );
        for (kind, line) in status_lines {
            let color = if kind == "error" {
                dark().semantic.danger_text
            } else {
                dark().text.secondary
            };
            content = content.child(status_line(&line, color));
        }
        content = content
            .child(
                Label::new(format!("Current shell · {shell_current}"))
                    .size(font::BODY)
                    .color(dark().text.primary),
            )
            .child(
                Label::new(format!("Current size · {size_current}"))
                    .size(font::BODY)
                    .color(dark().text.primary),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        Label::new("Shell")
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!writes, |el| el.bg(dark().surface.disabled).opacity(0.55))
                            .child(shell_input),
                    )
                    .child(clear),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        Label::new("Size")
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    )
                    .child(
                        div()
                            .w(px(96.0))
                            .when(!writes, |el| el.bg(dark().surface.disabled).opacity(0.55))
                            .child(columns_input),
                    )
                    .child(
                        Label::new("×")
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    )
                    .child(
                        div()
                            .w(px(96.0))
                            .when(!writes, |el| el.bg(dark().surface.disabled).opacity(0.55))
                            .child(rows_input),
                    )
                    .child(div().flex_1())
                    .child(save),
            )
            .child(
                Label::new(SETTINGS_TERMINAL_EFFECT_NOTE)
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// 「外观」页（SET-6e）：不经 Host，直接复用 Desktop 已有的
    /// 100% / 125% / 150% `TextScale`。三个按钮始终可达，当前档以
    /// 文字 + 视觉 + AX selected 同时标记；断线不禁用本地能力。
    fn settings_appearance_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.text_scale;
        let mut scale_controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SETTINGS_APPEARANCE_CONTROL_GAP))
            .min_w_0();
        for scale in SETTINGS_TEXT_SCALES {
            let id = settings_text_scale_identifier(scale);
            let selected = scale == current;
            let focus = self
                .settings_appearance_focus
                .entry(id.to_string())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let tooltip = if selected {
                format!("当前字号为 {}%", scale.percent())
            } else {
                format!("将字号设为 {}%", scale.percent())
            };
            let button = Button::new(id)
                .track_focus(&focus)
                .variant(if selected {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Raised
                })
                .height(px(SETTINGS_APPEARANCE_CONTROL_HEIGHT))
                .width(px(SETTINGS_APPEARANCE_CONTROL_WIDTH))
                .padding(ButtonPadding::Wide)
                .center()
                .radius(4.0)
                .bordered()
                .text_size(font::BODY_SM)
                .label(format!("{}%", scale.percent()))
                .tooltip(tooltip)
                .on_click(cx.listener(move |view, event, window, cx| {
                    if view.consume_button_key_click(id, event) {
                        return;
                    }
                    view.on_settings_text_scale(scale, window, cx);
                }))
                .on_activate(cx.listener(move |view, _event, window, cx| {
                    view.note_button_key_activate(id);
                    view.on_settings_text_scale(scale, window, cx);
                    cx.stop_propagation();
                }));
            scale_controls = scale_controls.child(button);
        }

        let content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("外观")
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new("Desktop presentation preferences")
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            )
            .child(
                Label::new("主题 · 深色")
                    .size(font::BODY)
                    .color(dark().text.primary),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(SETTINGS_APPEARANCE_THEME_NOTE),
            )
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("字号")
                        .size(font::BODY)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(format!("当前 · {}%", current.percent()))
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            )
            .child(scale_controls)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(SETTINGS_APPEARANCE_EFFECT_NOTE),
            );

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// 「高级」页诊断行（SET-6f）：render / AX 共用；未连接时协商字段
    /// 诚实返回 unavailable，endpoint 与最后 ack 游标仍来自 Desktop 本地事实。
    pub(super) fn settings_advanced_diagnostic_rows(
        &self,
    ) -> Vec<(&'static str, &'static str, String)> {
        // Connection 只报告相位，不复用 TaskRail「Local · Connected · resume」
        // 合成文案，避免把 resume / runtime id 混进这一行。
        let connection = match &self.projection.connection {
            ConnectionState::Connected { .. } => "Connected".into(),
            other => other.label(),
        };
        let unavailable = "Unavailable · connect to the Host";
        let (runtime_id, api_version, capabilities, resume) = match &self.handshake_info {
            Some(handshake) => (
                handshake.runtime_id.clone(),
                handshake.api_version.clone(),
                if handshake.capabilities.is_empty() {
                    "None granted".to_string()
                } else {
                    handshake.capabilities.join(", ")
                },
                self.projection
                    .resume
                    .label()
                    .unwrap_or_else(|| "Fresh snapshot".into()),
            ),
            None => (
                unavailable.into(),
                unavailable.into(),
                unavailable.into(),
                unavailable.into(),
            ),
        };
        let last_ack = self
            .controller
            .last_acked_sequence()
            .map_or_else(|| "Unavailable".into(), |sequence| sequence.to_string());
        vec![
            ("settings-advanced-connection", "Connection", connection),
            ("settings-advanced-runtime", "Host runtime ID", runtime_id),
            ("settings-advanced-api", "GUI API", api_version),
            (
                "settings-advanced-capabilities",
                "Granted capabilities",
                capabilities,
            ),
            (
                "settings-advanced-endpoint",
                "Endpoint",
                self.socket.display().to_string(),
            ),
            ("settings-advanced-resume", "Resume", resume),
            (
                "settings-advanced-last-ack",
                "Last acknowledged sequence",
                last_ack,
            ),
        ]
    }

    /// 「高级」页（SET-6f）：仅呈现 Desktop 已有连接事实，并在断线态
    /// 复用壳层现有 Reconnect。无实例编辑、CLI shell-out 或诊断历史。
    fn settings_advanced_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("高级")
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new("Connection diagnostics and startup target")
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );

        for (id, label, value) in self.settings_advanced_diagnostic_rows() {
            content = content.child(
                div()
                    .id(id)
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        Label::new(label)
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .whitespace_normal()
                            .text_size(font::BODY)
                            .text_color(dark().text.primary)
                            .child(value),
                    ),
            );
        }

        if self.projection.show_reconnect() {
            let reconnect_focus = self.reconnect_focus.clone();
            content = content.child(
                div().pt_1().child(
                    Button::new("reconnect")
                        .track_focus(&reconnect_focus)
                        .variant(ButtonVariant::Primary)
                        .height(px(SETTINGS_ACTION_HEIGHT))
                        .padding(ButtonPadding::Wide)
                        .center()
                        .radius(4.0)
                        .text_size(font::BODY_SM)
                        .label("Reconnect")
                        .on_click(cx.listener(|view, event, window, cx| {
                            if view.consume_button_key_click("reconnect", event) {
                                return;
                            }
                            view.on_reconnect(window, cx);
                        }))
                        .on_activate(cx.listener(|view, _event, window, cx| {
                            view.note_button_key_activate("reconnect");
                            view.on_reconnect(window, cx);
                            cx.stop_propagation();
                        })),
                ),
            );
        }

        content = content
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(SETTINGS_ADVANCED_TARGET_NOTE),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(SETTINGS_ADVANCED_DOCTOR_NOTE),
            );

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// 「关于」页只读行（SET-6g / ADR-051）：三个值分别来自 Desktop
    /// 构建元数据与当前已认证握手。Host 路径缺失或为空时整页不可用，
    /// render / AX 共用该 fail-closed gate，绝不从 endpoint 推断。
    pub(super) fn settings_about_rows(&self) -> Option<Vec<(&'static str, &'static str, String)>> {
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            return None;
        }
        let handshake = self.handshake_info.as_ref()?;
        let host_data_dir = handshake.host_data_dir.as_deref()?;
        if host_data_dir.trim().is_empty() {
            return None;
        }
        Some(vec![
            (
                "settings-about-desktop-build",
                "Desktop build",
                env!("CARGO_PKG_VERSION").to_string(),
            ),
            (
                "settings-about-api",
                "GUI API",
                handshake.api_version.clone(),
            ),
            (
                "settings-about-data-dir",
                "Host data directory",
                host_data_dir.to_string(),
            ),
        ])
    }

    /// 「关于」页（SET-6g / ADR-051）：仅呈现当前连接的三项权威事实，
    /// 不提供 updater、release、License 或任何写动作。
    fn settings_about_page_element(&mut self) -> impl IntoElement {
        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("关于")
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new("Build and current Host connection information")
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );

        for (id, label, value) in self.settings_about_rows().unwrap_or_default() {
            content = content.child(
                div()
                    .id(id)
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        Label::new(label)
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .whitespace_normal()
                            .text_size(font::BODY)
                            .text_color(dark().text.primary)
                            .child(value),
                    ),
            );
        }

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// 外观页字号选择入口（SET-6e）：只在当前 Settings / 外观
    /// 路由生效，防止迟到的可见 / 键盘 / AX 动作穿透。
    pub(crate) fn on_settings_text_scale(
        &mut self,
        scale: font::TextScale,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.route != AppRoute::Settings || self.settings_page != SettingsPage::Appearance {
            return;
        }
        self.set_text_scale(scale, window, cx);
    }

    /// 「权限与审批」页（SET-6b / ADR-048）：① 五档审批模式显式选择
    /// （当前值高亮；切换发 `set_approval_mode`，等回执才改生效值）；
    /// ② 会话信任开关（发 `workspace_trust`，workspace_id 取当前
    /// attached）；③ `trust_workspaces_global` 只读行；④ 生效边界诚实
    /// 文案。stale 只读，写入口与 AX 同 gate。
    fn settings_permissions_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let writes = self.settings_permissions_writes_enabled();
        let state = self.projection.settings_permissions.clone();
        let status_lines = permissions_status_lines(&state);
        let refresh_focus = self.settings_refresh_focus.clone();
        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Refresh")
            .tooltip("Refresh permissions settings")
            .disabled(!connected)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-refresh", event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-refresh");
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));

        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div().font_weight(FontWeight::MEDIUM).child(
                                    Label::new("权限与审批")
                                        .size(font::TITLE)
                                        .color(dark().text.primary),
                                ),
                            )
                            .child(
                                Label::new("当前会话的审批模式与 workspace 信任")
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().flex_none().pt_1().child(refresh)),
            );
        for (kind, line) in status_lines {
            let color = if kind == "error" {
                dark().semantic.danger_text
            } else {
                dark().text.secondary
            };
            content = content.child(status_line(&line, color));
        }

        // ① 五档审批模式：当前值高亮只读，其余档位显式「选择」。
        let current_mode_label = state
            .approval_mode
            .map(ApprovalModeSetting::label)
            .unwrap_or("未知");
        content = content
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("审批模式")
                        .size(font::BODY)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(format!("当前 · {current_mode_label}"))
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );
        let mut modes = div().flex().flex_col().min_w_0().gap_1();
        for mode in ApprovalModeSetting::ALL {
            modes = modes.child(self.settings_approval_mode_row(mode, &state, writes, cx));
        }
        content = content.child(modes);

        // ② 会话信任开关：workspace_id 取 Host permissions_settings 透出的
        // attached id；缺 id 禁用（fail-closed，不猜注册表首项）。
        let trust_enabled = writes && state.workspace_id.is_some();
        let trust_label = if state.workspace_trusted {
            "取消信任"
        } else {
            "信任 workspace"
        };
        let trust_focus = self
            .settings_permissions_focus
            .entry("settings-workspace-trust".to_string())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let trust_toggle = Button::new("settings-workspace-trust")
            .track_focus(&trust_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(trust_label)
            .tooltip("Toggle session workspace trust")
            .disabled(!trust_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-workspace-trust", event) {
                    return;
                }
                let trusted = view.projection.settings_permissions.workspace_trusted;
                view.on_settings_workspace_trust(!trusted, cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-workspace-trust");
                let trusted = view.projection.settings_permissions.workspace_trusted;
                view.on_settings_workspace_trust(!trusted, cx);
                cx.stop_propagation();
            }));
        let trust_state_label = if state.workspace_trusted {
            "已信任"
        } else {
            "未信任"
        };
        content = content.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w_0()
                .p(px(PROVIDER_CARD_PAD))
                .rounded(px(4.0))
                .border_1()
                .border_color(dark().border.subtle)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            Label::new("会话信任")
                                .size(font::BODY)
                                .color(dark().text.primary),
                        )
                        .child(
                            Label::new("信任当前 workspace（仅本会话，不写盘）")
                                .size(font::BODY_SM)
                                .color(dark().text.secondary),
                        ),
                )
                .child(
                    div().flex_none().child(
                        Label::new(format!("当前 · {trust_state_label}"))
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    ),
                )
                .child(div().flex_none().child(trust_toggle)),
        );

        // ③ Global 默认只读行（本片不写 Global trust）。
        let global_text = match state.trust_workspaces_global {
            None => SETTINGS_TRUST_UNSET,
            Some(true) => "已设置：信任所有 workspace",
            Some(false) => "已设置：不信任所有 workspace",
        };
        content = content.child(
            Label::new(format!("Global 默认（只读）· {global_text}"))
                .size(font::BODY_SM)
                .color(dark().text.secondary),
        );

        // ④ 生效边界诚实文案。
        content = content.child(
            Label::new(SETTINGS_PERMISSIONS_EFFECT_NOTE)
                .size(font::BODY_SM)
                .color(dark().text.secondary),
        );

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// 「工具与 MCP」页（SET-6c / ADR-049）：复用 Resources 的
    /// ResourcesPanelState（mcp_list 数据链 / epoch / stale / 断线
    /// fail-closed）；每行提供 Test / Remove（两步确认）；状态行与写
    /// gate 和 AX 同源。
    fn settings_tools_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let writes = self.settings_tools_writes_enabled();
        let status_lines = tools_status_lines(&self.resources);
        let servers = self.resources.servers.clone();
        let remove_confirm = self.settings_mcp_remove_confirm.clone();
        let refresh_focus = self.settings_refresh_focus.clone();
        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Refresh")
            .tooltip("Refresh MCP servers")
            .disabled(!connected)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-refresh", event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-refresh");
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));

        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div().font_weight(FontWeight::MEDIUM).child(
                                    Label::new("工具与 MCP")
                                        .size(font::TITLE)
                                        .color(dark().text.primary),
                                ),
                            )
                            .child(
                                Label::new("Host 权威 MCP server 清单、状态与配置")
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().flex_none().pt_1().child(refresh)),
            );
        for (kind, line) in status_lines {
            let color = if kind == "error" || kind == "action" {
                dark().semantic.danger_text
            } else {
                dark().text.secondary
            };
            content = content.child(status_line(&line, color));
        }
        for (ix, server) in servers.iter().enumerate() {
            content = content.child(self.settings_mcp_server_card(
                ix,
                server,
                remove_confirm.as_deref(),
                writes,
                cx,
            ));
        }
        // 生效边界诚实文案（ADR-049 D2 快照语义）。
        content = content.child(status_line(SETTINGS_MCP_EFFECT_NOTE, dark().text.secondary));

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// 单个 MCP server 卡片（SET-6c）：清单行复用 Resources 的渲染形状
    /// （name + state / transport · tools · last_error），动作行含 Test /
    /// Remove（Remove 走两步确认）。
    fn settings_mcp_server_card(
        &mut self,
        ix: usize,
        server: &McpServerEntry,
        remove_confirm: Option<&str>,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let confirming = remove_confirm == Some(server.name.as_str());
        let mut card = div()
            .id(("settings-mcp-server", ix))
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .p(px(PROVIDER_CARD_PAD))
            .rounded(px(4.0))
            .border_1()
            .border_color(if confirming {
                dark().semantic.warning_text
            } else {
                dark().border.subtle
            })
            .bg(dark().surface.raised)
            .child(mcp_server_name_row(server))
            .child(
                div()
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(mcp_server_meta_text(server)),
            );
        if confirming {
            card = card.child(status_line(
                SETTINGS_MCP_REMOVE_CONFIRM_NOTE,
                dark().semantic.warning_text,
            ));
        }
        let mut actions = vec![SettingsMcpAction::Test];
        if confirming {
            actions.push(SettingsMcpAction::ConfirmRemove);
            actions.push(SettingsMcpAction::KeepRemove);
        } else {
            actions.push(SettingsMcpAction::Remove);
        }
        let mut row = div().flex().flex_row().gap_1().flex_wrap();
        for action in actions {
            let tooltip = match action {
                SettingsMcpAction::Test => "Ping this server and refresh its state.",
                SettingsMcpAction::Remove | SettingsMcpAction::ConfirmRemove => {
                    "Remove this server from the Global config and clear its credentials."
                }
                SettingsMcpAction::KeepRemove => "",
            };
            row = row.child(self.settings_mcp_action_button(
                action,
                &server.name,
                writes,
                tooltip,
                cx,
            ));
        }
        card.child(row)
    }

    /// MCP 写动作按钮：可见 / 键盘（on_activate）/ AX（同名 identifier
    /// Press）三路径汇入同一 on_settings_mcp_action；disabled 时三者同时
    /// 失效。
    fn settings_mcp_action_button(
        &mut self,
        action: SettingsMcpAction,
        server: &str,
        writes: bool,
        tooltip: &'static str,
        cx: &mut Context<Self>,
    ) -> Button {
        let id = action.identifier(server);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = id.clone();
        let click_server = server.to_string();
        let activate_id = id.clone();
        let activate_server = server.to_string();
        let button = Button::new(id)
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(action.label())
            .disabled(!writes)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&click_id, event) {
                    return;
                }
                view.on_settings_mcp_action(action, click_server.clone(), cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.on_settings_mcp_action(action, activate_server.clone(), cx);
                cx.stop_propagation();
            }));
        if tooltip.is_empty() {
            button
        } else {
            button.tooltip(tooltip)
        }
    }

    /// 单个审批模式行：当前档高亮只读（accent 边框 + 「当前」徽标），
    /// 其余档位「选择」按钮（可见 / 键盘 / AX 三路径同 identifier、同
    /// gate；stale 时禁用）。
    fn settings_approval_mode_row(
        &mut self,
        mode: ApprovalModeSetting,
        state: &SettingsPermissionsState,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let current = state.approval_mode == Some(mode);
        let enabled = writes && !current;
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w_0()
            .p(px(PROVIDER_CARD_PAD))
            .rounded(px(4.0))
            .border_1()
            .border_color(if current {
                dark().accent.primary
            } else {
                dark().border.subtle
            })
            .when(current, |el| el.bg(dark().surface.raised))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        Label::new(mode.label())
                            .size(font::BODY)
                            .color(dark().text.primary),
                    )
                    .child(
                        Label::new(mode.description())
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    ),
            );
        if current {
            row = row.child(
                div().flex_none().child(
                    Label::new("当前")
                        .size(font::XS)
                        .color(dark().accent.primary),
                ),
            );
        } else {
            let id = format!("settings-approval-mode-{}", mode.wire());
            let focus = self
                .settings_permissions_focus
                .entry(id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let click_id = id.clone();
            let activate_id = id.clone();
            let select = Button::new(id)
                .track_focus(&focus)
                .variant(ButtonVariant::Raised)
                .height(px(SETTINGS_ACTION_HEIGHT))
                .vcenter()
                .radius(4.0)
                .bordered()
                .text_size(font::BODY_SM)
                .label("选择")
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&click_id, event) {
                        return;
                    }
                    view.on_settings_approval_mode(mode, cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&activate_id);
                    view.on_settings_approval_mode(mode, cx);
                    cx.stop_propagation();
                }));
            row = row.child(div().flex_none().child(select));
        }
        row.into_any_element()
    }

    /// 单个 provider 卡片（SET-4）：只读事实行 + descriptor 驱动写动作。
    /// secure 输入按 provider 懒建实体；OAuth 等待详情（URL / code /
    /// 到期）只在 Connecting 且有等待信息时呈现。
    fn settings_provider_card(
        &mut self,
        ix: usize,
        provider: &ProviderStatusEntry,
        oauth_waits: &std::collections::HashMap<String, OAuthWait>,
        auth_notes: &std::collections::HashMap<String, String>,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let provider_id = provider.provider_id.clone();
        let editor_open = self.settings_api_key_editor_visible(provider);
        let remove_confirm = self.settings_remove_confirm.as_deref() == Some(provider_id.as_str());
        let oauth_waiting = oauth_waits.contains_key(&provider_id);
        let actions = settings_auth_actions(provider, editor_open, remove_confirm, oauth_waiting);

        let mut card = div()
            .id(("settings-provider", ix))
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .p(px(PROVIDER_CARD_PAD))
            .rounded(px(4.0))
            .border_1()
            .border_color(dark().border.subtle)
            .bg(dark().surface.raised)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(
                                Label::new(provider.display_name.clone())
                                    .size(font::BODY)
                                    .color(dark().text.primary),
                            ),
                    )
                    .child(
                        div().flex_none().child(
                            Label::new(provider.auth_methods_label())
                                .size(font::BODY_SM)
                                .color(dark().text.secondary),
                        ),
                    ),
            )
            .child(
                Label::new(provider.auth_label())
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );

        // OAuth 授权等待详情：Desktop 只显示 URL / user code / 到期，
        // 不接触 token；取消走 auth_cancel。
        if let (ProviderAuthState::Connecting, Some(wait)) =
            (&provider.auth, oauth_waits.get(&provider_id))
        {
            card = card.child(
                Label::new(format!("Authorize at {}", wait.verification_url))
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );
            if let Some(code) = &wait.user_code {
                card = card.child(
                    Label::new(format!("Code {code}"))
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                );
            }
            if let Some(expires) = &wait.expires_at {
                card = card.child(
                    Label::new(format!("Expires {expires}"))
                        .size(font::BODY_SM)
                        .color(dark().text.tertiary),
                );
            }
        }

        // 终态 AuthChanged 的瞬态反馈（取消 / 过期 / 移除）。
        if let Some(note) = auth_notes.get(&provider_id) {
            card = card.child(status_line(note, dark().text.secondary));
        }

        card = card
            .child(
                Label::new(provider.endpoint_label.clone())
                    .size(font::BODY_SM)
                    .color(dark().text.tertiary),
            )
            .child(
                Label::new(provider.catalog_label())
                    .size(font::BODY_SM)
                    .color(dark().text.tertiary),
            );

        // API key secure 输入（内联）：none / error 常驻；connected 由
        // Replace 展开后出现；Verify 空输入禁用，明文不进 projection。
        if editor_open {
            if let Some(input) = self.settings_api_key_inputs.get(&provider_id).cloned() {
                let verify_enabled = self.settings_action_enabled(
                    SettingsAuthAction::VerifyApiKey,
                    &provider_id,
                    writes,
                    cx,
                );
                let mut editor = div().flex().flex_row().items_center().gap_1().min_w_0();
                editor = editor.child(div().flex_1().min_w_0().child(input));
                for action in [
                    SettingsAuthAction::VerifyApiKey,
                    SettingsAuthAction::CancelApiKeyInput,
                ] {
                    if !actions.contains(&action) {
                        continue;
                    }
                    let (enabled, tooltip) = if action == SettingsAuthAction::VerifyApiKey {
                        (
                            verify_enabled,
                            if writes && !verify_enabled {
                                "API key is empty."
                            } else {
                                ""
                            },
                        )
                    } else {
                        (writes, "")
                    };
                    editor = editor.child(self.settings_action_button(
                        action,
                        &provider_id,
                        enabled,
                        tooltip,
                        cx,
                    ));
                }
                card = card.child(editor);
            }
        }

        // 其余动作行（Connect / Replace / Cancel / Remove / 确认组）。
        let row_actions: Vec<SettingsAuthAction> = actions
            .into_iter()
            .filter(|action| {
                !matches!(
                    action,
                    SettingsAuthAction::VerifyApiKey | SettingsAuthAction::CancelApiKeyInput
                )
            })
            .collect();
        if !row_actions.is_empty() {
            let mut row = div().flex().flex_row().gap_1().flex_wrap();
            for action in row_actions {
                let tooltip = if action == SettingsAuthAction::Remove {
                    "Remove the stored credential."
                } else {
                    ""
                };
                row = row.child(self.settings_action_button(
                    action,
                    &provider_id,
                    writes,
                    tooltip,
                    cx,
                ));
            }
            card = card.child(row);
        }
        card
    }

    /// 「模型与默认项」区（SET-5）：按 provider 分组列出 projection.models
    /// 的可运行模型；默认行带徽标，每行提供「设为默认」（gate 与 AX 同
    /// 源）；默认失效时给出显式说明行，不做任何静默切换。
    fn settings_models_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let default = self.projection.settings_providers.default_model.clone();
        let unavailable = self.projection.default_model_unavailable();
        let groups = group_models_by_provider(&self.projection.models);
        let mut section = div()
            .id("settings-models")
            .flex()
            .flex_col()
            .min_w_0()
            .gap_1()
            .mt_2()
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("Models & defaults")
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new("Runnable models per provider; the default applies to new runs")
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );
        if unavailable {
            section = section.child(status_line(
                SETTINGS_DEFAULT_UNAVAILABLE_NOTE,
                dark().semantic.danger_text,
            ));
        }
        if groups.is_empty() {
            section = section.child(status_line(
                "No models reported by the host.",
                dark().text.secondary,
            ));
            return section;
        }
        for (provider_id, models) in groups {
            // 组头显示名取 provider 权威清单；目录里出现而清单缺失的
            // provider 诚实回落原始 id，不臆造。
            let display_name = self
                .projection
                .settings_providers
                .providers
                .iter()
                .find(|entry| entry.provider_id == provider_id)
                .map(|entry| entry.display_name.clone())
                .unwrap_or_else(|| provider_id.to_string());
            let mut group = div().flex().flex_col().min_w_0().gap_1().mt_1().child(
                div().min_w_0().truncate().child(
                    Label::new(display_name)
                        .size(font::BODY)
                        .color(dark().text.primary),
                ),
            );
            for model in models {
                group = group.child(self.settings_model_row(&default, &model, cx));
            }
            section = section.child(group);
        }
        section
    }

    /// 单个模型行：display_name + id + 默认徽标 +「设为默认」按钮
    ///（可见 / 键盘 / AX 三路径同 identifier、同 gate）。
    fn settings_model_row(
        &mut self,
        default: &Option<(String, String)>,
        model: &ModelEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_default = default
            .as_ref()
            .is_some_and(|(provider, id)| provider == &model.provider_id && id == &model.id);
        let enabled = self.settings_set_default_enabled(&model.provider_id, &model.id);
        let id = settings_set_default_identifier(&model.provider_id, &model.id);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = id.clone();
        let click_provider = model.provider_id.clone();
        let click_model = model.id.clone();
        let activate_id = id.clone();
        let activate_provider = model.provider_id.clone();
        let activate_model = model.id.clone();
        let button = Button::new(id)
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Set default")
            .disabled(!enabled)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&click_id, event) {
                    return;
                }
                view.on_settings_set_default(click_provider.clone(), click_model.clone(), cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.on_settings_set_default(activate_provider.clone(), activate_model.clone(), cx);
                cx.stop_propagation();
            }));
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w_0()
            .child(
                div().flex_1().min_w_0().truncate().child(
                    Label::new(format!("{} · {}", model.display_name, model.id))
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                ),
            );
        if is_default {
            row = row.child(
                div().flex_none().child(
                    Label::new("Default")
                        .size(font::XS)
                        .color(dark().accent.primary),
                ),
            );
        }
        row.child(div().flex_none().child(button))
    }

    /// 写操作总 gate：断线 / stale 一律禁写（可见 / 键盘 / AX 三路径共用）。
    pub(crate) fn settings_writes_enabled(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.settings_providers.stale_reason.is_none()
    }

    /// 通用页写操作 gate（SET-6a）：连接 + 非 stale + 查询已成功。
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

    /// 单个写动作的启用谓词（render 与 AX 同源）：writes 总 gate 之上，
    /// Verify 在 secure 输入为空时禁用（进程内读长度，AX value 仍只发布
    /// 掩码）。
    pub(crate) fn settings_action_enabled(
        &self,
        action: SettingsAuthAction,
        provider_id: &str,
        writes: bool,
        cx: &App,
    ) -> bool {
        if !writes {
            return false;
        }
        if action != SettingsAuthAction::VerifyApiKey {
            return true;
        }
        self.settings_api_key_inputs
            .get(provider_id)
            .is_some_and(|input| !input.read(cx).text().trim().is_empty())
    }

    /// 「设为默认」启用谓词（SET-5；render / 键盘 / AX / 入口四路径共用）：
    /// writes 总 gate 之上，要求该 provider 当前已连接、且该行非当前默认。
    pub(crate) fn settings_set_default_enabled(&self, provider_id: &str, model_id: &str) -> bool {
        if !self.settings_writes_enabled() {
            return false;
        }
        let state = &self.projection.settings_providers;
        if state
            .default_model
            .as_ref()
            .is_some_and(|(provider, model)| provider == provider_id && model == model_id)
        {
            return false;
        }
        state.providers.iter().any(|entry| {
            entry.provider_id == provider_id
                && matches!(entry.auth, ProviderAuthState::Connected { .. })
        })
    }

    /// API key 内联编辑器可见性：none / error 常驻；connected 需 Replace
    /// 展开后出现；connecting（验证中）不显示。
    pub(crate) fn settings_api_key_editor_visible(&self, provider: &ProviderStatusEntry) -> bool {
        if !provider
            .auth_methods
            .iter()
            .any(|method| method == "api_key")
        {
            return false;
        }
        match provider.auth {
            ProviderAuthState::NotConnected | ProviderAuthState::Error { .. } => true,
            ProviderAuthState::Connected { .. } => self
                .settings_api_key_editors
                .contains(&provider.provider_id),
            ProviderAuthState::Connecting => false,
        }
    }

    /// 写动作按钮：可见 / 键盘（on_activate）/ AX（同名 identifier Press）
    /// 三路径汇入同一 on_settings_action；disabled 时三者同时失效。
    fn settings_action_button(
        &mut self,
        action: SettingsAuthAction,
        provider_id: &str,
        enabled: bool,
        tooltip: &'static str,
        cx: &mut Context<Self>,
    ) -> Button {
        let id = action.identifier(provider_id);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = id.clone();
        let click_provider = provider_id.to_string();
        let activate_id = id.clone();
        let activate_provider = provider_id.to_string();
        let button = Button::new(id)
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(action.label())
            .disabled(!enabled)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&click_id, event) {
                    return;
                }
                view.on_settings_action(action, click_provider.clone(), cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.on_settings_action(action, activate_provider.clone(), cx);
                cx.stop_propagation();
            }));
        if tooltip.is_empty() {
            button
        } else {
            button.tooltip(tooltip)
        }
    }

    /// settings 写动作统一入口（三路径同源；入口级复核 gate 与 descriptor）。
    pub(crate) fn on_settings_action(
        &mut self,
        action: SettingsAuthAction,
        provider_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_action_enabled(action, &provider_id, self.settings_writes_enabled(), cx) {
            return;
        }
        match action {
            SettingsAuthAction::ConnectOauth | SettingsAuthAction::ReplaceOauth => {
                self.on_settings_connect_oauth(provider_id, cx);
            }
            SettingsAuthAction::CancelOauth => {
                self.controller.auth_cancel(provider_id);
            }
            SettingsAuthAction::ReplaceApiKey => {
                self.settings_api_key_editors.insert(provider_id);
            }
            SettingsAuthAction::VerifyApiKey => {
                self.on_settings_verify_api_key(provider_id, cx);
            }
            SettingsAuthAction::CancelApiKeyInput => {
                self.on_settings_cancel_api_key_input(provider_id, cx);
            }
            SettingsAuthAction::Remove => {
                self.settings_remove_confirm = Some(provider_id);
            }
            SettingsAuthAction::ConfirmRemove => {
                self.settings_remove_confirm = None;
                self.controller.auth_remove(provider_id);
            }
            SettingsAuthAction::KeepRemove => {
                self.settings_remove_confirm = None;
            }
        }
    }

    /// 「设为默认」统一入口（SET-5；三路径同源；入口级复核 gate 与模型
    /// 目录，未知 pair fail-closed）。确认回执由 DefaultModelConfirmed /
    /// ProviderStatusLoaded 收敛，不在此乐观改状态。
    pub(crate) fn on_settings_set_default(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        let in_catalog = self
            .projection
            .models
            .iter()
            .any(|model| model.provider_id == provider_id && model.id == model_id);
        if !in_catalog || !self.settings_set_default_enabled(&provider_id, &model_id) {
            return;
        }
        self.controller.set_default_model(provider_id, model_id);
        cx.notify();
    }

    /// proxy Save（SET-6a；三路径同源）。空输入禁用；提交后等 Host 回执
    /// 才改生效值。
    pub(crate) fn on_settings_proxy_save(&mut self, cx: &mut Context<Self>) {
        if !self.settings_general_writes_enabled() {
            return;
        }
        let value = self.settings_proxy_input.read(cx).text().trim().to_string();
        if value.is_empty() {
            return;
        }
        self.controller.set_proxy_url(Some(value));
        cx.notify();
    }

    /// proxy Clear（SET-6a；三路径同源）。已是 null 时禁用。
    pub(crate) fn on_settings_proxy_clear(&mut self, cx: &mut Context<Self>) {
        if !self.settings_general_writes_enabled()
            || self.projection.settings_general.proxy_url.is_none()
        {
            return;
        }
        self.controller.set_proxy_url(None);
        cx.notify();
    }

    /// 终端页 Save（SET-6d；三路径同源）：shell/columns/rows 三字段全态
    /// 回传（ADR-050 D3）；空 shell 映射为 null（跟随平台默认），畸形 /
    /// 越界尺寸禁用，提交后等 Host 回执才改生效值。
    pub(crate) fn on_settings_terminal_save(&mut self, cx: &mut Context<Self>) {
        if !self.settings_terminal_writes_enabled() {
            return;
        }
        let shell = parse_terminal_shell(self.settings_terminal_shell_input.read(cx).text());
        let Some(columns) =
            parse_terminal_dimension(self.settings_terminal_columns_input.read(cx).text())
        else {
            return;
        };
        let Some(rows) =
            parse_terminal_dimension(self.settings_terminal_rows_input.read(cx).text())
        else {
            return;
        };
        self.controller.set_terminal_settings(shell, columns, rows);
        cx.notify();
    }

    /// 终端页 Clear（SET-6d；三路径同源）：清除只作用于 shell（null 回
    /// 平台默认）；columns/rows 按全态写语义回传 Host 权威生效值。
    pub(crate) fn on_settings_terminal_clear(&mut self, cx: &mut Context<Self>) {
        if !self.settings_terminal_writes_enabled()
            || self.projection.settings_terminal.shell.is_none()
        {
            return;
        }
        let (columns, rows) = self.projection.settings_terminal.effective_size();
        self.controller.set_terminal_settings(None, columns, rows);
        cx.notify();
    }

    /// 切换审批模式（SET-6b；三路径同源）。入口级复核 gate 与当前值；
    /// 确认回执由 ApprovalModeConfirmed 收敛，不在此乐观改状态。
    pub(crate) fn on_settings_approval_mode(
        &mut self,
        mode: ApprovalModeSetting,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_permissions_writes_enabled()
            || self.projection.settings_permissions.approval_mode == Some(mode)
        {
            return;
        }
        self.controller.set_approval_mode(mode.wire());
        cx.notify();
    }

    /// 会话信任切换（SET-6b；三路径同源）。workspace_id 取 Host 查询透出的
    /// attached id（ADR-048 D1 实现期修订；缺 id fail-closed）；确认回执由
    /// WorkspaceTrustConfirmed 收敛。
    pub(crate) fn on_settings_workspace_trust(&mut self, trusted: bool, cx: &mut Context<Self>) {
        if !self.settings_permissions_writes_enabled()
            || self.projection.settings_permissions.workspace_trusted == trusted
        {
            return;
        }
        let Some(workspace_id) = self.projection.settings_permissions.workspace_id.clone() else {
            return;
        };
        self.controller.set_workspace_trust(&workspace_id, trusted);
        cx.notify();
    }

    /// MCP 写动作统一入口（SET-6c；三路径同源；入口级复核 gate 与清单
    /// 成员）。Remove 走两步确认；确认回执（同形状 servers）经
    /// McpServersReceipt 收敛，不在此乐观改清单。
    pub(crate) fn on_settings_mcp_action(
        &mut self,
        action: SettingsMcpAction,
        name: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_mcp_server_action_enabled(&name) {
            return;
        }
        match action {
            SettingsMcpAction::Test => {
                self.controller.mcp_test(name);
            }
            SettingsMcpAction::Remove => {
                self.settings_mcp_remove_confirm = Some(name);
            }
            SettingsMcpAction::ConfirmRemove => {
                self.settings_mcp_remove_confirm = None;
                self.controller.mcp_server_remove(name);
            }
            SettingsMcpAction::KeepRemove => {
                self.settings_mcp_remove_confirm = None;
            }
        }
        cx.notify();
    }

    /// MCP 写动作启用谓词（render 与 AX 同源）：writes 总 gate 之上复核
    /// server 仍在当前权威清单（未知名 fail-closed）。
    pub(crate) fn settings_mcp_server_action_enabled(&self, name: &str) -> bool {
        if !self.settings_tools_writes_enabled() {
            return false;
        }
        self.resources
            .servers
            .iter()
            .any(|server| server.name == name)
    }

    fn on_settings_connect_oauth(&mut self, provider_id: String, cx: &mut Context<Self>) {
        // descriptor 复核：provider 必须存在且声明 oauth（未知 id fail-closed）。
        let declares = self
            .projection
            .settings_providers
            .providers
            .iter()
            .any(|entry| {
                entry.provider_id == provider_id
                    && entry.auth_methods.iter().any(|method| method == "oauth")
            });
        if !declares {
            return;
        }
        // Replace 基线：Connected 起点的写流程终态不清旧凭证（交重查）。
        self.projection
            .settings_providers
            .begin_auth_flow(&provider_id);
        // 乐观置 Connecting；AuthStarted 回执补 URL 详情，失败经
        // OperationFailed 触发状态重查回滚。
        if let Some(entry) = self
            .projection
            .settings_providers
            .providers
            .iter_mut()
            .find(|entry| entry.provider_id == provider_id)
        {
            entry.auth = ProviderAuthState::Connecting;
        }
        self.controller.auth_start(provider_id);
        cx.notify();
    }

    fn on_settings_verify_api_key(&mut self, provider_id: String, cx: &mut Context<Self>) {
        // descriptor 复核：provider 必须声明 api_key。
        let declares = self
            .projection
            .settings_providers
            .providers
            .iter()
            .any(|entry| {
                entry.provider_id == provider_id
                    && entry.auth_methods.iter().any(|method| method == "api_key")
            });
        let Some(input) = self.settings_api_key_inputs.get(&provider_id).cloned() else {
            return;
        };
        let key = input.read(cx).text().trim().to_string();
        if !declares || key.is_empty() {
            return;
        }
        // 清空输入缓冲（含 undo 栈，SET-005「提交后清空 UI 缓冲」）；
        // 明文只进 controller 调用栈。
        input.update(cx, |input, cx| input.reset_text("", cx));
        self.settings_api_key_editors.remove(&provider_id);
        // Replace 基线：Connected 起点的写流程终态不清旧凭证（交重查）。
        self.projection
            .settings_providers
            .begin_auth_flow(&provider_id);
        if let Some(entry) = self
            .projection
            .settings_providers
            .providers
            .iter_mut()
            .find(|entry| entry.provider_id == provider_id)
        {
            entry.auth = ProviderAuthState::Connecting;
        }
        self.controller.auth_set_api_key(provider_id, key);
        cx.notify();
    }

    fn on_settings_cancel_api_key_input(&mut self, provider_id: String, cx: &mut Context<Self>) {
        if let Some(input) = self.settings_api_key_inputs.get(&provider_id).cloned() {
            input.update(cx, |input, cx| input.reset_text("", cx));
        }
        self.settings_api_key_editors.remove(&provider_id);
    }

    /// 按当前 provider 清单懒建 / 回收 secure 输入实体与焦点句柄（含
    /// 「设为默认」按钮随模型目录的回收）。
    pub(crate) fn ensure_settings_api_key_inputs(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .projection
            .settings_providers
            .providers
            .iter()
            .filter(|entry| entry.auth_methods.iter().any(|method| method == "api_key"))
            .map(|entry| entry.provider_id.clone())
            .collect();
        self.settings_api_key_inputs
            .retain(|id, _| ids.iter().any(|current| current == id));
        self.settings_api_key_editors
            .retain(|id| ids.iter().any(|current| current == id));
        // 焦点句柄回收按「当前 provider × 全部动作」的精确 identifier
        // 白名单比对，不用子串匹配（会误伤 id 段重叠的无关条目）。
        let mut action_ids = HashSet::new();
        for entry in &self.projection.settings_providers.providers {
            for action in SettingsAuthAction::ALL {
                action_ids.insert(action.identifier(&entry.provider_id));
            }
        }
        for model in &self.projection.models {
            action_ids.insert(settings_set_default_identifier(
                &model.provider_id,
                &model.id,
            ));
        }
        self.settings_action_focus
            .retain(|id, _| action_ids.contains(id));
        for id in ids {
            self.settings_api_key_inputs
                .entry(id.clone())
                .or_insert_with(|| {
                    let element_id = format!("settings-api-key-input-{id}");
                    cx.new(|cx| {
                        TextInput::with_placeholder("Paste API key", cx)
                            .id(element_id)
                            .secure()
                            .height_clamp(
                                metrics::COMPOSER_INPUT_MIN_HEIGHT,
                                metrics::COMPOSER_INPUT_MIN_HEIGHT,
                            )
                    })
                });
        }
    }

    /// 转义后的 provider id → 原始 id（以 provider 清单为权威，未知
    /// fail-closed；不反解转义）。
    pub(crate) fn settings_provider_id_for_escaped(&self, escaped: &str) -> Option<String> {
        self.projection
            .settings_providers
            .providers
            .iter()
            .find(|entry| dynamic_identifier("", &entry.provider_id) == format!("-{escaped}"))
            .map(|entry| entry.provider_id.clone())
    }

    /// AX 派发用：按转义串对照当前权威 MCP server 清单还原名称（SET-6c；
    /// 未知名 fail-closed）。
    pub(crate) fn settings_mcp_server_for_escaped(&self, escaped: &str) -> Option<String> {
        self.resources
            .servers
            .iter()
            .find(|server| dynamic_identifier("", &server.name) == format!("-{escaped}"))
            .map(|server| server.name.clone())
    }

    /// 转义后的 "<provider>:<model>" → 原始 pair（以 projection.models 为
    /// 权威，未知 fail-closed；不反解转义）。
    pub(crate) fn settings_default_target_for_escaped(
        &self,
        escaped: &str,
    ) -> Option<(String, String)> {
        self.projection
            .models
            .iter()
            .find(|model| {
                dynamic_identifier("", &format!("{}:{}", model.provider_id, model.id))
                    == format!("-{escaped}")
            })
            .map(|model| (model.provider_id.clone(), model.id.clone()))
    }

    /// 离开 Settings：清空 secure 缓冲（含 undo 栈）与进行中的本地编辑
    /// 状态；不触碰工作台 / 会话 / 草稿 / Run。
    pub(crate) fn clear_settings_buffers(&mut self, cx: &mut Context<Self>) {
        for input in self.settings_api_key_inputs.values() {
            input.update(cx, |input, cx| input.reset_text("", cx));
        }
        self.settings_proxy_input
            .update(cx, |input, cx| input.reset_text("", cx));
        self.settings_terminal_shell_input
            .update(cx, |input, cx| input.reset_text("", cx));
        self.settings_terminal_columns_input
            .update(cx, |input, cx| input.reset_text("", cx));
        self.settings_terminal_rows_input
            .update(cx, |input, cx| input.reset_text("", cx));
        self.settings_api_key_editors.clear();
        self.settings_remove_confirm = None;
        self.settings_mcp_remove_confirm = None;
    }
}

fn status_line(text: &str, color: gpui::Rgba) -> impl IntoElement {
    div().child(
        Label::new(text.to_string())
            .size(font::BODY_SM)
            .color(color),
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_terminal_dimension, parse_terminal_shell, terminal_save_enabled};

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
}
