//! Settings providers 页。

use super::*;
use crate::ui::components::dropdown::Dropdown;
use crate::ui::components::switch::Switch;
use crate::ui::MenuKind;

impl AppView {
    /// 呈现排序不改 Host 目录；同组保持原顺序，render / AX 共用。
    pub(crate) fn settings_ordered_providers(&self) -> Vec<ProviderAuthStatusEntry> {
        let mut providers = self.projection.settings_providers.providers.clone();
        providers.sort_by_key(|p| !matches!(p.auth, ProviderAuthState::Connected { .. }));
        providers
    }

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
        let providers = self.settings_ordered_providers();
        let oauth_waits = state.oauth_waits.clone();
        let auth_notes = state.auth_notes.clone();
        let mut content = settings_column();
        // 页级刷新（SET-5）：重查 provider_auth_status + model_list；断线
        // 禁用（与 AX / 入口 gate 同源）。
        let refresh_enabled = connected;
        let refresh_focus = self.settings_refresh_focus.clone();
        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(t("settings.refresh"))
            .tooltip(t("settings.providers.refresh_tooltip"))
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
                .items_start()
                .gap_6()
                .child(self.settings_heading(
                    t("settings.providers.title"),
                    t("settings.providers.subtitle"),
                ))
                .child(
                    self.settings_element("settings-refresh")
                        .flex_none()
                        .child(refresh),
                ),
        );
        for (kind, line) in status_lines {
            content = content.child(self.settings_status(kind, line));
        }

        content = content.child(
            self.settings_element("settings-providers-heading")
                .child(settings_label(t("settings.providers.section_providers"))),
        );

        if !providers.is_empty() {
            let mut cards = div().flex().flex_col().gap_3();
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

        content.child(self.settings_default_roles_section(cx))
    }

    /// Provider 卡：两组概览（名称 / 认证方式、连接态 / 目录）随字号自然
    /// 增高；chevron 展开区先账号 / 额度，再代理 / 模型管理 / Usage。默认折叠；流程态（编辑器 / OAuth 等待 / Remove 二次确认 /
    /// 瞬态反馈）保持展开区可见。
    pub(super) fn settings_provider_card(
        &mut self,
        _ix: usize,
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
        let actions = self.settings_provider_auth_actions(
            provider,
            editor_open,
            remove_confirm,
            oauth_waiting,
        );
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
        let expanded = self.settings_provider_card_expanded(
            &provider_id,
            editor_open,
            oauth_waiting,
            remove_confirm,
        );
        let connection_color = match provider.auth {
            ProviderAuthState::Connected { .. } => dark().semantic.success_fg,
            ProviderAuthState::Error { .. } => dark().semantic.danger_text,
            _ => dark().text.secondary,
        };
        let catalog_summary = provider_catalog_overview_label(provider, model_count);
        let auth_methods = provider.auth_methods_label();
        let auth_methods = if auth_methods.is_empty() {
            t("settings.providers.no_auth_method").to_string()
        } else {
            auth_methods
        };
        let expand = self.settings_provider_expand_button(&provider_id, expanded, cx);
        let header = self
            .settings_element(dynamic_identifier("settings-provider-header", &provider_id))
            .h(px(metrics::SETTINGS_PROVIDER_HEADER_HEIGHT))
            .overflow_hidden()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_4()
            .child(
                div()
                    .w(px(metrics::SETTINGS_PROVIDER_STATUS_DOT))
                    .h(px(metrics::SETTINGS_PROVIDER_STATUS_DOT))
                    .flex_none()
                    .rounded_full()
                    .bg(connection_color),
            )
            .child(
                self.settings_element(dynamic_identifier("settings-provider-name", &provider_id))
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(font::BODY)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(dark().text.primary)
                            .child(provider.display_name.clone()),
                    ),
            )
            .child(
                div().flex().flex_row().flex_none().child(
                    div()
                        .truncate()
                        .text_size(font::BASE)
                        .text_color(dark().text.secondary)
                        .child(auth_methods),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_none()
                    .h(px(24.0))
                    .px_2()
                    .rounded(px(metrics::CONTROL_RADIUS))
                    .bg(dark().surface.hover)
                    .child(
                        self.settings_element(dynamic_identifier(
                            "settings-provider-connection",
                            &provider_id,
                        ))
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .truncate()
                                .text_size(font::BASE)
                                .text_color(connection_color)
                                .child(provider.auth_label()),
                        ),
                    )
                    .child(
                        div()
                            .text_size(font::BASE)
                            .text_color(dark().text.secondary)
                            .child(" · "),
                    )
                    .child(
                        self.settings_element(dynamic_identifier(
                            "settings-provider-catalog",
                            &provider_id,
                        ))
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .truncate()
                                .text_size(font::XS)
                                .text_color(dark().text.secondary)
                                .child(catalog_summary),
                        ),
                    ),
            )
            .child(
                self.settings_element(settings_provider_expand_identifier(&provider_id))
                    .flex_none()
                    .child(expand),
            );
        let mut card = self
            .settings_element(dynamic_identifier("settings-provider", &provider_id))
            .flex()
            .flex_col()
            .rounded(px(metrics::INPUT_MENU_RADIUS))
            .border_1()
            .border_color(dark().border.subtle)
            .bg(dark().surface.raised)
            .child(header);
        if expanded {
            card = card.child(self.settings_provider_expanded_region(
                provider,
                &actions,
                &row_actions,
                oauth_waits,
                auth_notes,
                writes,
                cx,
            ));
        }
        card
    }

    /// 卡片有效展开态（render / AX 同源；ADR-056 D4）：显式 chevron 状态
    /// ∨ 流程态。编辑器 / OAuth 等待 / Remove 二次确认与瞬态 auth 反馈
    /// 打开时保持展开区可见，不折叠正在进行的流程。
    pub(crate) fn settings_provider_card_expanded(
        &self,
        provider_id: &str,
        editor_open: bool,
        oauth_waiting: bool,
        remove_confirm: bool,
    ) -> bool {
        self.projection
            .settings_providers
            .provider_expanded(provider_id)
            || editor_open
            || oauth_waiting
            || remove_confirm
            || self
                .projection
                .settings_providers
                .auth_notes
                .contains_key(provider_id)
    }

    /// 展开 / 折叠 chevron：换形指示状态（▸ 折叠 / ▾ 展开）。本地视图
    /// 动作，不受写总闸限制；render / 键盘 / AX 三路径同 identifier、
    /// 同入口。OPT-4a 可见字形合同：36×36 命中区 + metrics::ICON_SIZE（20px），
    /// 与 rail 图标按钮同款几何（12px 文本字号在真窗口 fallback 字体下
    /// 缩成 2-3px 圆点，不可见）。
    fn settings_provider_expand_button(
        &mut self,
        provider_id: &str,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let id = settings_provider_expand_identifier(provider_id);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = id.clone();
        let click_provider = provider_id.to_string();
        let activate_id = id.clone();
        let activate_provider = provider_id.to_string();
        Button::new(id)
            .track_focus(&focus)
            .variant(ButtonVariant::Ghost)
            .padding(ButtonPadding::None)
            .width(px(metrics::ICON_BUTTON_SIZE))
            .height(px(metrics::ICON_BUTTON_SIZE))
            .center()
            .radius(6.0)
            .child(icon(if expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            }))
            .tooltip(if expanded {
                t("settings.providers.collapse_tooltip")
            } else {
                t("settings.providers.expand_tooltip")
            })
            .on_click(
                cx.listener(move |view, event: &gpui::ClickEvent, _window, cx| {
                    if view.consume_button_key_click(&click_id, event) {
                        return;
                    }
                    view.on_toggle_settings_provider_expanded(click_provider.clone(), cx);
                }),
            )
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.on_toggle_settings_provider_expanded(activate_provider.clone(), cx);
                cx.stop_propagation();
            }))
    }

    /// 展开 / 折叠入口（chevron render 点击 / 键盘激活 / AX Press 三路径
    /// 同源；本地视图态，未知 provider fail-closed，不发 Host 命令）。
    pub(crate) fn on_toggle_settings_provider_expanded(
        &mut self,
        provider_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self
            .projection
            .settings_providers
            .providers
            .iter()
            .any(|entry| entry.provider_id == provider_id)
        {
            return;
        }
        self.projection
            .settings_providers
            .toggle_provider_expanded(&provider_id);
        self.projection
            .settings_providers
            .account_quotas
            .retain(|(p, _), _| p != &provider_id);
        if self
            .projection
            .settings_providers
            .provider_expanded(&provider_id)
        {
            self.refresh_expanded_account_quotas(true, cx);
        }
        cx.notify();
    }

    /// UX-06：账号与额度优先，其后保留代理、模型管理与不可用 Usage。
    fn settings_provider_expanded_region(
        &mut self,
        provider: &ProviderAuthStatusEntry,
        actions: &[SettingsAuthAction],
        row_actions: &[SettingsAuthAction],
        oauth_waits: &std::collections::HashMap<String, AuthStartData>,
        auth_notes: &std::collections::HashMap<String, String>,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut region = div()
            .flex()
            .flex_col()
            .min_w_0()
            .border_t_1()
            .border_color(dark().border.subtle);
        // Credentials 区：凭证逐条列出，动作按钮在区下方（ADR-056 D4）。
        region = region.child(self.settings_provider_credentials_block(
            provider,
            actions,
            row_actions,
            oauth_waits,
            auth_notes,
            writes,
            cx,
        ));
        // Proxy 行：迁入展开区，Switch 写回链路与「仅全局 proxy_url 已
        // 配置时渲染」的 gate 均不变（ADR-052 SET-6h）。
        if self.projection.settings_general.proxy_url.is_some() {
            region = region.child(self.settings_provider_proxy_row(provider, writes, cx));
        }
        // Manage models 行：迁入展开区，触发器与弹层逻辑、gate 不变。
        region = region.child(self.settings_provider_manage_row(provider, cx));
        if provider.provider_id != "opencode-go"
            || !self.settings_quota_supported()
            || provider.credentials.is_empty()
        {
            region = region.child(self.settings_provider_usage_row(&provider.provider_id));
        }
        region
    }

    /// 展开区「标题 + 副标题 + 右侧控件」行（Proxy / Manage models /
    /// Usage 共用骨架；行间以 subtle 分隔线收口，对照签字稿）。
    fn settings_provider_row(
        &mut self,
        id: String,
        title: &'static str,
        subtitle: &'static str,
        divider: bool,
        right: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_4()
            .when(divider, |el| {
                el.border_b_1().border_color(dark().border.subtle)
            })
            .child(
                self.settings_element(id)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(settings_label(title))
                    .child(settings_copy(subtitle)),
            )
            .child(div().flex().flex_none().items_center().child(right))
    }

    /// Proxy 行（ADR-052 SET-6h / OPT-3c）：Switch 写回 set_provider_use_proxy
    /// 不变；checked 态以 Host 回执为准收敛，不乐观更新。
    fn settings_provider_proxy_row(
        &mut self,
        provider: &ProviderAuthStatusEntry,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let provider_id = provider.provider_id.clone();
        let id = settings_use_proxy_identifier(&provider_id);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let tooltip = if provider.use_proxy {
            t("settings.providers.proxy_tooltip_on")
        } else {
            t("settings.providers.proxy_tooltip_off")
        };
        let click_id = id.clone();
        let click_provider = provider_id.clone();
        let activate_id = id.clone();
        let activate_provider = provider_id.clone();
        let toggle = Switch::new(id.clone())
            .track_focus(&focus)
            .checked(provider.use_proxy)
            .tooltip(tooltip)
            .disabled(!writes)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&click_id, event) {
                    return;
                }
                view.on_settings_toggle_provider_use_proxy(click_provider.clone(), cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.on_settings_toggle_provider_use_proxy(activate_provider.clone(), cx);
                cx.stop_propagation();
            }));
        let state_label = if provider.use_proxy {
            t("settings.providers.switch_on")
        } else {
            t("settings.providers.switch_off")
        };
        let toggle = self.settings_element(id).flex_none().child(toggle);
        self.settings_provider_row(
            dynamic_identifier("settings-provider-proxy-text", &provider_id),
            t("settings.providers.proxy_title"),
            t("settings.providers.proxy_subtitle"),
            true,
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    Label::new(state_label)
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                )
                .child(toggle),
        )
    }

    /// Manage models 行：既有触发器与弹层（Dropdown + MenuPanel）原样
    /// 迁入展开区；打开弹层取全量目录（include_disabled=true）。
    fn settings_provider_manage_row(
        &mut self,
        provider: &ProviderAuthStatusEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let provider_id = provider.provider_id.clone();
        let manage_enabled = self.settings_manage_models_enabled(provider);
        let manage_id = settings_manage_models_identifier(&provider_id);
        let focus = self
            .settings_action_focus
            .entry(manage_id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let models_menu_open = matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &provider_id);
        let manage_click_id = manage_id.clone();
        let manage_click_provider = provider_id.clone();
        let manage_activate_id = manage_id.clone();
        let manage_activate_provider = provider_id.clone();
        let manage_trigger = Button::new(manage_id.clone())
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(t("settings.providers.manage_models"))
            .tooltip(t("settings.providers.manage_models_tooltip"))
            .disabled(!manage_enabled);
        let manage_trigger = if manage_enabled {
            manage_trigger
                .on_click(
                    cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                        if view.consume_button_key_click(&manage_click_id, event) {
                            return;
                        }
                        let down = Self::click_down_position(event);
                        view.on_toggle_settings_models_menu(
                            manage_click_provider.clone(),
                            down,
                            window,
                            cx,
                        );
                    }),
                )
                .on_activate(cx.listener(move |view, _event, window, cx| {
                    if view.open_menu.is_some() {
                        // 弹层已开时让位给根节点菜单 Enter 处理，并重新
                        // 武装 keyup 合成 click 吞除标记。
                        view.note_button_key_activate(&manage_activate_id);
                        return;
                    }
                    view.note_button_key_activate(&manage_activate_id);
                    view.on_toggle_settings_models_menu(
                        manage_activate_provider.clone(),
                        None,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }))
        } else {
            manage_trigger
        };
        let mut manage_picker =
            Dropdown::new(self.settings_element(manage_id).child(manage_trigger));
        if models_menu_open && manage_enabled {
            manage_picker =
                manage_picker.panel(self.settings_models_menu_element(&provider_id, cx));
        }
        self.settings_provider_row(
            dynamic_identifier("settings-provider-manage-text", &provider_id),
            t("settings.providers.manage_models"),
            t("settings.providers.catalog_scope"),
            true,
            manage_picker,
        )
    }

    /// Usage 行（ADR-056 D5 / OPT-3e）：固定进度条槽位 + 「Usage
    /// unavailable」诚实空态；无权威 QuotaSnapshot 来源前不渲染任何数字、
    /// 百分比或填充。
    fn settings_provider_usage_row(&mut self, provider_id: &str) -> impl IntoElement {
        self.settings_provider_row(
            dynamic_identifier("settings-provider-usage", provider_id),
            t("settings.providers.usage_title"),
            t("settings.providers.usage_unavailable"),
            false,
            div()
                .flex_none()
                .w(px(SETTINGS_PROVIDER_USAGE_BAR_WIDTH))
                .h(px(SETTINGS_PROVIDER_USAGE_BAR_HEIGHT))
                .rounded(px(SETTINGS_PROVIDER_USAGE_BAR_HEIGHT / 2.0))
                .border_1()
                .border_color(dark().border.subtle),
        )
    }

    /// 供应商级代理开关入口（ADR-052 SET-6h；render / 键盘 / AX 三路径
    /// 同源；入口级复核 gate）。未配置全局代理时按钮不渲染，此入口同样
    /// fail-closed。确认回执由 ProviderUseProxyConfirmed / 重查收敛，
    /// 不在此乐观改状态。
    pub(crate) fn on_settings_toggle_provider_use_proxy(
        &mut self,
        provider_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writes_enabled() {
            return;
        }
        if self.projection.settings_general.proxy_url.is_none() {
            return;
        }
        let Some(current) = self
            .projection
            .settings_providers
            .providers
            .iter()
            .find(|entry| entry.provider_id == provider_id)
            .map(|entry| entry.use_proxy)
        else {
            return;
        };
        self.controller
            .set_provider_use_proxy(provider_id, !current);
        cx.notify();
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

    /// 离开 Settings：清空 secure 缓冲（含 undo 栈）与进行中的本地编辑
    /// 状态；不触碰工作台 / 会话 / 草稿 / Run。
    pub(crate) fn clear_settings_buffers(&mut self, cx: &mut Context<Self>) {
        self.projection.settings_providers.account_quotas.clear();
        self.projection
            .settings_providers
            .account_mode_pending
            .clear();
        self.settings_auth_details.clear();
        self.settings_copied_auth = None;
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
        self.settings_account_rename = None;
        self.settings_account_remove_confirm = None;
        self.settings_remove_confirm = None;
        self.settings_mcp_remove_confirm = None;
        // OPT-3a 弹层本地编辑状态：在途标记与 cleared_roles 说明随离开
        // Settings 清空（Host 权威状态不受影响）。
        self.projection.settings_providers.model_write_pending = None;
        self.projection.settings_providers.model_cleared_note = None;
        self.reset_settings_search(cx);
    }
}
