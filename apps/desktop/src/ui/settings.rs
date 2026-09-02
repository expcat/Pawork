//! Settings 壳（SET-3/4/6a/6b）：Settings Rail、「Models & providers」、
//! 「General」与「权限与审批」页。
//!
//! 供应商页只呈现 Host `provider_auth_status` 权威事实：供应商名称、
//! 认证方式、连接状态与目录来源（SET-3）；SET-4 增认证写操作（API key
//! secure 输入验证、OAuth 等待/取消、Replace/Remove），全部由 descriptor
//! （auth_methods + auth.type）驱动，禁止按 provider 名分支。SET-6a 增
//! 「General」页（`proxy_url` 读/设置/清除）；SET-6b 增「权限与审批」页
//! （五档审批模式 / 会话信任 / Global 默认只读）；查询失败 / 未知则隐藏
//! 该导航项且不渲染写入口。断线保留 stale 只读结果并禁用全部写动作；
//! 可见 / 键盘 / AX 三路径同 gate。

use std::collections::HashSet;

use gpui::{div, prelude::*, px, App, Context, FontWeight, Pixels};

use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::label::Label;
use crate::ui::components::panel::Panel;
use crate::ui::theme::{dark, font, metrics};

use crate::projection::{
    group_models_by_provider, ApprovalModeSetting, ConnectionState, ModelEntry, OAuthWait,
    ProviderAuthState, ProviderStatusEntry, SettingsPermissionsState,
};
use crate::ui::text_input::TextInput;

use super::accessibility::dynamic_identifier;
use super::shell_layout;
use super::AppView;
use super::SettingsPage;

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
        let current_page = match self.settings_page {
            SettingsPage::General if !general_available => SettingsPage::Providers,
            SettingsPage::Permissions if !permissions_available => SettingsPage::Providers,
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
            return self.settings_permissions_page_element(cx).into_any_element();
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

        // ② 会话信任开关：workspace_id 取当前 attached；无 attached 时
        // 禁用（fail-closed，不臆造 id）。
        let trust_enabled = writes && self.projection.workspace_id.is_some();
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
                    Label::new("当前").size(font::XS).color(dark().accent.primary),
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
        self.settings_api_key_editors.clear();
        self.settings_remove_confirm = None;
    }
}

fn status_line(text: &str, color: gpui::Rgba) -> impl IntoElement {
    div().child(
        Label::new(text.to_string())
            .size(font::BODY_SM)
            .color(color),
    )
}
