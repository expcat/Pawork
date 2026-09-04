//! Settings providers 页。

use std::collections::HashSet;

use super::*;

impl AppView {
    pub(super) fn settings_providers_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let state = &self.projection.settings_providers;
        let writes = self.settings_writes_enabled();
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
        content = content.child(
            div().font_weight(FontWeight::MEDIUM).child(
                Label::new("Providers")
                    .size(font::BODY)
                    .color(dark().text.primary),
            ),
        );

        if !providers.is_empty() {
            let mut cards = div().flex().flex_col().min_w_0().gap_2();
            for (ix, provider) in providers.iter().enumerate() {
                let model_count = self
                    .projection
                    .models
                    .iter()
                    .filter(|model| model.provider_id == provider.provider_id)
                    .count();
                cards = cards.child(self.settings_provider_card(
                    ix,
                    provider,
                    model_count,
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
    pub(super) fn settings_provider_card(
        &mut self,
        ix: usize,
        provider: &ProviderAuthStatusEntry,
        model_count: usize,
        oauth_waits: &std::collections::HashMap<String, AuthStartData>,
        auth_notes: &std::collections::HashMap<String, String>,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let provider_id = provider.provider_id.clone();
        let editor_open = self.settings_api_key_editor_visible(provider);
        let remove_confirm = self.settings_remove_confirm.as_deref() == Some(provider_id.as_str());
        let oauth_waiting = oauth_waits.contains_key(&provider_id);
        let actions = settings_auth_actions(provider, editor_open, remove_confirm, oauth_waiting);
        let row_actions: Vec<SettingsAuthAction> = actions
            .iter()
            .copied()
            .filter(|action| {
                !matches!(
                    action,
                    SettingsAuthAction::VerifyApiKey | SettingsAuthAction::CancelApiKeyInput
                )
            })
            .collect();
        let actions_in_details = remove_confirm || row_actions.len() > 2;
        let endpoint_visible = editor_open || oauth_waiting || remove_confirm;
        let auth_error = match &provider.auth {
            ProviderAuthState::Error { message } => Some(message.as_str()),
            _ => None,
        };
        let catalog_error = matches!(
            (&provider.auth, &provider.catalog),
            (
                ProviderAuthState::Connected { .. },
                crate::projection::ProviderCatalogState::Unavailable { .. },
            )
        );
        let detail_visible = editor_open
            || oauth_waiting
            || remove_confirm
            || actions_in_details
            || auth_error.is_some()
            || catalog_error
            || auth_notes.contains_key(&provider_id);
        let connection_color = match provider.auth {
            ProviderAuthState::Connected { .. } => dark().semantic.success_fg,
            ProviderAuthState::Error { .. } => dark().semantic.danger_text,
            _ => dark().text.secondary,
        };
        let catalog_summary = provider_catalog_overview_label(provider, model_count);
        let auth_methods = provider.auth_methods_label();
        let auth_methods = if auth_methods.is_empty() {
            "No auth method".to_string()
        } else {
            auth_methods
        };
        let mut header_actions = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_end()
            .gap_1();
        if !actions_in_details {
            for action in &row_actions {
                let tooltip = if *action == SettingsAuthAction::Remove {
                    "Remove the stored credential."
                } else {
                    ""
                };
                header_actions = header_actions.child(self.settings_action_button(
                    *action,
                    &provider_id,
                    writes,
                    tooltip,
                    cx,
                ));
            }
        }
        let header = div()
            .id(("settings-provider-overview", ix))
            .h(px(PROVIDER_OVERVIEW_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w_0()
            .px_2()
            .when(detail_visible, |el| {
                el.border_b_1().border_color(dark().border.subtle)
            })
            .child(
                div()
                    .w(px(172.0))
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
                div().w(px(104.0)).min_w_0().truncate().child(
                    Label::new(auth_methods)
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                ),
            )
            .child(
                div().w(px(132.0)).min_w_0().truncate().child(
                    Label::new(provider.auth_label())
                        .size(font::BODY_SM)
                        .color(connection_color),
                ),
            )
            .child(
                div().w(px(132.0)).min_w_0().truncate().child(
                    Label::new(catalog_summary)
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                ),
            )
            .child(header_actions);
        let mut card = div()
            .id(("settings-provider", ix))
            .flex()
            .flex_col()
            .min_w_0()
            .rounded(px(6.0))
            .border_1()
            .border_color(dark().border.subtle)
            .bg(dark().surface.raised)
            .child(header);
        let mut details = div().flex().flex_col().min_w_0().gap_1().p_2();

        // OAuth 授权等待详情：Desktop 只显示 URL / user code / 到期，
        // 不接触 token；取消走 auth_cancel。
        if let (ProviderAuthState::Connecting, Some(wait)) =
            (&provider.auth, oauth_waits.get(&provider_id))
        {
            details = details.child(
                Label::new(format!("Authorize at {}", wait.verification_url))
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );
            if let Some(code) = &wait.user_code {
                details = details.child(
                    Label::new(format!("Code {code}"))
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                );
            }
            if let Some(expires) = &wait.expires_at {
                details = details.child(
                    Label::new(format!("Expires {expires}"))
                        .size(font::BODY_SM)
                        .color(dark().text.tertiary),
                );
            }
        }

        // 终态 AuthChanged 的瞬态反馈（取消 / 过期 / 移除）。
        if let Some(note) = auth_notes.get(&provider_id) {
            details = details.child(status_line(note, dark().text.secondary));
        }
        if let Some(message) = auth_error {
            details = details.child(status_line(
                &format!("Connection error · {message}"),
                dark().semantic.danger_text,
            ));
        }
        if catalog_error {
            details = details.child(status_line(
                &provider.catalog_label(),
                dark().semantic.danger_text,
            ));
        }
        if endpoint_visible {
            details = details.child(
                Label::new(format!("Endpoint · {}", provider.endpoint_label))
                    .size(font::BODY_SM)
                    .color(dark().text.tertiary),
            );
        }

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
                details = details.child(editor);
            }
        }

        // 多动作或 destructive 二次确认移入详情，普通概览保持 64px。
        if actions_in_details && !row_actions.is_empty() {
            let mut row = div().flex().flex_row().gap_1().flex_wrap();
            for action in &row_actions {
                let tooltip = if *action == SettingsAuthAction::Remove {
                    "Remove the stored credential."
                } else {
                    ""
                };
                row = row.child(self.settings_action_button(
                    *action,
                    &provider_id,
                    writes,
                    tooltip,
                    cx,
                ));
            }
            details = details.child(row);
        }
        if detail_visible {
            card = card.child(details);
        }
        card
    }

    /// 「模型与默认项」区（SET-5）：按 provider 分组列出 projection.models
    /// 的可运行模型；默认行带徽标，每行提供「设为默认」（gate 与 AX 同
    /// 源）；默认失效时给出显式说明行，不做任何静默切换。
    pub(super) fn settings_models_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    Label::new("Default model")
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new("Choose the model used when a new task starts")
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

    /// 单个模型行：普通列表只显示 display_name + 默认徽标 +「设为默认」按钮；
    /// raw id 只用于稳定控件 identifier 与 Host 写入。
    ///（可见 / 键盘 / AX 三路径同 identifier、同 gate）。
    pub(super) fn settings_model_row(
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
                    Label::new(model.display_name.clone())
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

    /// API key 内联编辑器只在 Connect / Replace 后展开；普通 provider
    /// 概览始终保持紧凑，connecting（验证中）不显示。
    pub(crate) fn settings_api_key_editor_visible(
        &self,
        provider: &ProviderAuthStatusEntry,
    ) -> bool {
        if !provider
            .auth_methods
            .iter()
            .any(|method| method == "api_key")
        {
            return false;
        }
        match provider.auth {
            ProviderAuthState::None
            | ProviderAuthState::Error { .. }
            | ProviderAuthState::Connected { .. } => self
                .settings_api_key_editors
                .contains(&provider.provider_id),
            ProviderAuthState::Connecting => false,
        }
    }

    /// 写动作按钮：可见 / 键盘（on_activate）/ AX（同名 identifier Press）
    /// 三路径汇入同一 on_settings_action；disabled 时三者同时失效。
    pub(super) fn settings_action_button(
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
            SettingsAuthAction::ConnectApiKey | SettingsAuthAction::ReplaceApiKey => {
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

    /// 启动 OAuth：descriptor 复核后登记 Replace 基线并置 Connecting。
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
