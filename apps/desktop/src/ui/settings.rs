//! Settings 壳（SET-3/4）：Settings Rail 与「Models & providers」页。
//!
//! 只呈现 Host `provider_auth_status` 权威事实：供应商名称、认证方式、
//! 连接状态与目录来源（SET-3）；SET-4 增认证写操作（API key secure 输入
//! 验证、OAuth 等待/取消、Replace/Remove），全部由 descriptor
//! （auth_methods + auth.type）驱动，禁止按 provider 名分支。断线保留
//! stale 只读结果并禁用全部写动作；可见 / 键盘 / AX 三路径同 gate。

use std::collections::HashSet;

use gpui::{div, prelude::*, px, App, Context, FontWeight, Pixels};

use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::label::Label;
use crate::ui::components::panel::Panel;
use crate::ui::theme::{dark, font, metrics};

use crate::projection::{ConnectionState, OAuthWait, ProviderAuthState, ProviderStatusEntry};
use crate::ui::text_input::TextInput;

use super::accessibility::dynamic_identifier;
use super::shell_layout;
use super::AppView;

/// Settings 内容可读列（与 Timeline 618px 可读列同节奏；全宽壳层内收敛）。
const SETTINGS_CONTENT_MAX_WIDTH: f32 = 720.0;
/// Provider 卡片内边距（8px 节奏）。
const PROVIDER_CARD_PAD: f32 = 8.0;
/// 写动作按钮高度（与 Composer 28px 动作槽同节奏）。
const SETTINGS_ACTION_HEIGHT: f32 = 28.0;

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
}

pub(crate) fn settings_api_key_input_identifier(provider_id: &str) -> String {
    format!(
        "{SETTINGS_CONTROL_PREFIX}{}",
        dynamic_identifier("api-key-input", provider_id)
    )
}

/// 前缀锚定解析 settings 控件 identifier；provider 部分是转义后的 id，
/// 由 AppView 对照 provider 列表还原（未知 id fail-closed）。
pub(crate) fn parse_settings_control(identifier: &str) -> Option<SettingsControl> {
    let rest = identifier.strip_prefix(SETTINGS_CONTROL_PREFIX)?;
    if let Some(provider) = rest.strip_prefix("api-key-input-") {
        return Some(SettingsControl::ApiKeyInput(provider.to_string()));
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
        // 首页导航项：当前唯一页面，选中态静态行（不画无动作假按钮；
        // 后续页面按真实 capability 逐页加入）。
        let nav_item = div()
            .id("settings-nav-providers")
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
                    Label::new("Models & providers")
                        .size(font::BODY_SM)
                        .color(dark().text.primary),
                ),
            );

        Panel::side_right(rail_width)
            .child(shell_layout::rail_safe_area())
            .child(back)
            .child(nav_item)
    }

    /// Settings 全宽内容区（SET-4 认证写操作）。状态行全部来自
    /// projection（Host 权威 / stale / error）；卡片动作由 descriptor 驱动，
    /// 断线（stale）时可见 / 键盘 / AX 同时禁用。
    pub(super) fn settings_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
        content = content.child(
            div()
                .flex()
                .flex_col()
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

        page.child(
            div()
                .id("settings-page-scroll")
                .flex_1()
                .min_h_0()
                .track_scroll(&self.settings_scroll)
                .child(content),
        )
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

    /// 写操作总 gate：断线 / stale 一律禁写（可见 / 键盘 / AX 三路径共用）。
    pub(crate) fn settings_writes_enabled(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.settings_providers.stale_reason.is_none()
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

    /// 按当前 provider 清单懒建 / 回收 secure 输入实体与焦点句柄。
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

    /// 离开 Settings：清空 secure 缓冲（含 undo 栈）与进行中的本地编辑
    /// 状态；不触碰工作台 / 会话 / 草稿 / Run。
    pub(crate) fn clear_settings_buffers(&mut self, cx: &mut Context<Self>) {
        for input in self.settings_api_key_inputs.values() {
            input.update(cx, |input, cx| input.reset_text("", cx));
        }
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
