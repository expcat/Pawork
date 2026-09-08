//! Settings providers 页。

use std::collections::HashSet;

use gpui::{Point, SharedString, Window};

use super::*;
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::switch::Switch;
use crate::ui::MenuKind;

/// 授权按钮只交给浏览器 HTTP(S) 链接，不启动任意系统协议。
fn oauth_url_can_open(url: &str) -> bool {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .is_some_and(|rest| {
            !rest
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default()
                .is_empty()
                && !url.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        })
}

#[test]
fn oauth_browser_links_reject_non_web_targets() {
    assert!(oauth_url_can_open(
        "https://accounts.x.ai/activate?code=ABCD-1234"
    ));
    assert!(oauth_url_can_open("http://localhost:1455/authorize"));
    for url in [
        "",
        "https://",
        "https:///missing-host",
        "javascript:alert(1)",
        "file:///tmp/auth",
        "pawork://authorize",
        "https://example.com/\nother",
    ] {
        assert!(!oauth_url_can_open(url), "{url:?}");
    }
}

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

        // OPT-3b / ADR-055 D5：「Default models」四角色区（刷新行之下、
        // Providers 列表之上）。
        content = content.child(self.settings_default_roles_section(cx));

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

        content
    }

    /// Provider 卡：两组概览（名称 / 认证方式、连接态 / 目录）随字号自然
    /// 增高；chevron 展开区（Proxy → Manage models → Credentials →
    /// Usage）。默认折叠；流程态（编辑器 / OAuth 等待 / Remove 二次确认 /
    /// 瞬态反馈）保持展开区可见。
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
        // 两组信息随列宽自然收缩，名称与连接状态不再争抢五个固定槽。
        let expand = self.settings_provider_expand_button(&provider_id, expanded, cx);
        let header = div()
            .id(("settings-provider-overview", ix))
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_4()
            .when(expanded, |el| {
                el.border_b_1().border_color(dark().border.subtle)
            })
            .child(
                self.settings_element(dynamic_identifier("settings-provider-name", &provider_id))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_size(font::BODY)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(dark().text.primary)
                            .child(provider.display_name.clone()),
                    )
                    .child(
                        div()
                            .text_size(font::BASE)
                            .text_color(dark().text.secondary)
                            .child(auth_methods),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        self.settings_element(dynamic_identifier(
                            "settings-provider-connection",
                            &provider_id,
                        ))
                        .text_size(font::BASE)
                        .text_color(connection_color)
                        .child(provider.auth_label()),
                    )
                    .child(
                        self.settings_element(dynamic_identifier(
                            "settings-provider-catalog",
                            &provider_id,
                        ))
                        .text_size(font::XS)
                        .text_color(dark().text.secondary)
                        .child(catalog_summary),
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
    /// 同入口。OPT-4a 可见字形合同：36×36 命中区 + font::ICON（20px），
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
            .text_size(font::ICON)
            .label(if expanded { "▾" } else { "▸" })
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
        cx.notify();
    }

    /// 展开区（ADR-056 D4，OPT-D 签字稿）：Proxy 行 → Manage models 行 →
    /// Credentials 区（凭证列表 + 流程详情 + 动作按钮）→ Usage 行。
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
        let mut region = div().flex().flex_col().min_w_0();
        // Proxy 行：迁入展开区，Switch 写回链路与「仅全局 proxy_url 已
        // 配置时渲染」的 gate 均不变（ADR-052 SET-6h）。
        if self.projection.settings_general.proxy_url.is_some() {
            region = region.child(self.settings_provider_proxy_row(provider, writes, cx));
        }
        // Manage models 行：迁入展开区，触发器与弹层逻辑、gate 不变。
        region = region.child(self.settings_provider_manage_row(provider, cx));
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
        // Usage 行：固定槽位 + 诚实空态（ADR-056 D5，恒无数字 / 填充）。
        region = region.child(self.settings_provider_usage_row(&provider.provider_id));
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

    /// Credentials 区（ADR-056 D4）：标题 + 副标题；每条存储凭证一行
    ///（类型标签 + masked + 状态点 Connected/Expired）；空列表诚实空态。
    /// Connect / Replace / Remove 动作按钮与 API key 编辑器、OAuth 等待、
    /// Remove 二次确认等流程详情均在本区内渲染，行为不变。
    fn settings_provider_credentials_block(
        &mut self,
        provider: &ProviderAuthStatusEntry,
        actions: &[SettingsAuthAction],
        row_actions: &[SettingsAuthAction],
        oauth_waits: &std::collections::HashMap<String, AuthStartData>,
        auth_notes: &std::collections::HashMap<String, String>,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let provider_id = provider.provider_id.clone();
        let editor_open = self.settings_api_key_editor_visible(provider);
        let remove_confirm = self.settings_remove_confirm.as_deref() == Some(provider_id.as_str());
        let oauth_waiting = oauth_waits.contains_key(&provider_id);
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
        let mut block = self
            .settings_element(dynamic_identifier(
                "settings-provider-credentials",
                &provider_id,
            ))
            .flex()
            .flex_col()
            .px_4()
            .py_4()
            .gap_3()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(
                self.settings_element(dynamic_identifier(
                    "settings-provider-credentials-header",
                    &provider_id,
                ))
                .flex()
                .flex_col()
                .gap_1()
                .child(settings_label(t("settings.providers.credentials_title")))
                .child(settings_copy(t("settings.providers.credentials_subtitle"))),
            );
        // 凭证行：只列盘上存储条目（kind + masked + 状态点）；空列表给
        // 诚实空态，不渲染假行（env fallback 不入列，Host 口径）。
        if provider.credentials.is_empty() {
            block = block.child(self.settings_note(
                dynamic_identifier("settings-provider-credentials-empty", &provider_id),
                t("settings.providers.credentials_empty"),
            ));
        } else {
            for (ix, credential) in provider.credentials.iter().enumerate() {
                let expired = credential.expired;
                let status_color = if expired {
                    dark().semantic.danger_text
                } else {
                    dark().semantic.success_fg
                };
                block = block.child(
                    self.settings_element(dynamic_identifier(
                        &format!("settings-provider-credential-{ix}"),
                        &provider_id,
                    ))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                Label::new(provider_credential_kind_label(&credential.kind))
                                    .size(font::BODY_SM)
                                    .color(dark().text.primary),
                            )
                            .child(
                                div().min_w_0().truncate().child(
                                    Label::new(credential.masked_credential.clone())
                                        .size(font::BODY_SM)
                                        .color(dark().text.tertiary),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(6.0))
                                    .h(px(6.0))
                                    .rounded_full()
                                    .bg(status_color),
                            )
                            .child(
                                Label::new(provider_credential_status_label(expired))
                                    .size(font::BODY_SM)
                                    .color(status_color),
                            ),
                    ),
                );
            }
        }

        // 登录详情保持原文，长 URL 可横滚、所有可见信息可选中复制。
        let mut details = Vec::new();
        if let (ProviderAuthState::Connecting, Some(wait)) =
            (&provider.auth, oauth_waits.get(&provider_id))
        {
            let mut actions = div().flex().flex_wrap().gap_2();
            for action in [
                SettingsAuthAction::OpenOauth,
                SettingsAuthAction::CopyOauthUrl,
                SettingsAuthAction::CopyOauthCode,
            ] {
                if action == SettingsAuthAction::CopyOauthCode && wait.user_code.is_none() {
                    continue;
                }
                actions = actions.child(self.settings_action_button(
                    action,
                    &provider_id,
                    self.settings_action_enabled(action, &provider_id, writes, cx),
                    "",
                    cx,
                ));
            }
            block = block.child(actions);
            details
                .push(t("settings.providers.authorize_at").replace("{}", &wait.verification_url));
            if let Some(code) = &wait.user_code {
                details.push(t("settings.providers.oauth_code").replace("{}", code));
            }
            if let Some(expires) = &wait.expires_at {
                details.push(t("settings.providers.oauth_expires").replace("{}", expires));
            }
        }
        if let Some(note) = auth_notes.get(&provider_id) {
            details.push(note.clone());
        }
        if let Some(message) = auth_error {
            details.push(t("settings.providers.connection_error").replace("{}", message));
        }
        if catalog_error {
            details.push(provider.catalog_label());
        }
        if endpoint_visible {
            details
                .push(t("settings.providers.endpoint_row").replace("{}", &provider.endpoint_label));
        }
        if !details.is_empty() {
            let details_id = dynamic_identifier("settings-provider-details", &provider_id);
            let value = details.join("\n");
            let input = self
                .settings_auth_details
                .entry(provider_id.clone())
                .or_insert_with(|| {
                    cx.new(|cx| {
                        crate::ui::text_input::TextInput::with_placeholder("", cx)
                            .id(details_id.clone())
                            .read_only()
                            .height_clamp(28., 180.)
                    })
                })
                .clone();
            if input.read(cx).text() != value {
                input.update(cx, |input, cx| input.reset_text(value, cx));
            }
            block = block.child(
                self.settings_element(details_id)
                    .w_full()
                    .min_w_0()
                    .text_color(if auth_error.is_some() || catalog_error {
                        dark().semantic.danger_text
                    } else {
                        dark().text.secondary
                    })
                    .child(input),
            );
        } else {
            self.settings_auth_details.remove(&provider_id);
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
                let mut editor = div().flex().flex_col().gap_3();
                editor = editor.child(
                    self.settings_element(settings_api_key_input_identifier(&provider_id))
                        .child(input),
                );
                let mut editor_actions = div().flex().flex_wrap().gap_2();
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
                                t("settings.providers.api_key_empty")
                            } else {
                                ""
                            },
                        )
                    } else {
                        (writes, "")
                    };
                    editor_actions = editor_actions.child(self.settings_action_button(
                        action,
                        &provider_id,
                        enabled,
                        tooltip,
                        cx,
                    ));
                }
                block = block.child(editor.child(editor_actions));
            }
        }

        // 认证动作按钮（Connect / Replace / Remove / Cancel 等）位于
        // Credentials 区下方；行为不变。
        if !row_actions.is_empty() {
            let mut row = div().flex().flex_row().gap_2().flex_wrap();
            for action in row_actions {
                let tooltip = if *action == SettingsAuthAction::Remove {
                    t("settings.providers.tooltip_remove_credential")
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
            block = block.child(row);
        }
        block
    }

    /// 「Default models」四默认角色区（OPT-3b / ADR-055 D5）：每行角色名 +
    /// 下拉 + 用途说明；候选 = 已连接 provider 的已启用模型（Host 已按
    /// include_disabled=false 过滤启用集，连接态在此过滤）。选择 / 清除走
    /// SetDefaultRoleModel；conversation 默认失效保持既有显式提示行，不做
    /// 任何静默切换；Vision / Search 落地期只保存选择（说明如实标注）。
    pub(super) fn settings_default_roles_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let unavailable = self.projection.default_model_unavailable();
        let mut section = self
            .settings_element("settings-default-roles")
            .flex()
            .flex_col()
            .gap_3()
            .pt_6()
            .border_t_1()
            .border_color(dark().border.subtle)
            .child(
                self.settings_element("settings-default-roles-title")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(settings_label(t("settings.roles.title")))
                    .child(settings_copy(t("settings.roles.subtitle"))),
            );
        if unavailable {
            section = section.child(
                self.settings_element("settings-default-roles-unavailable")
                    .text_size(font::BASE)
                    .text_color(dark().semantic.danger_text)
                    .child(settings_default_unavailable_note()),
            );
        }
        let mut card = self
            .settings_element("settings-default-roles-card")
            .flex()
            .flex_col();
        for (ix, role) in SettingsRole::ALL.into_iter().enumerate() {
            card = card.child(self.settings_role_row(ix, role, cx));
        }
        section.child(card)
    }

    /// 单个角色行：左侧角色名与用途说明，右侧下拉触发器。可见 / 键盘 / AX 三路径同 identifier、同 gate（断线 / stale /
    /// 该角色写中禁用，disabled 不发布 Press）。
    pub(super) fn settings_role_row(
        &mut self,
        ix: usize,
        role: SettingsRole,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = self.settings_role_menu_enabled(role);
        let menu_open =
            matches!(self.open_menu, Some(MenuKind::SettingsRole(open)) if open == role);
        let identifier = settings_role_trigger_identifier(role);
        let focus = self
            .settings_action_focus
            .entry(identifier.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = identifier.clone();
        let activate_id = identifier.clone();
        let trigger = Button::new(identifier.clone())
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .width(px(SETTINGS_ROLE_MENU_WIDTH))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(self.settings_role_value_label(role))
            .tooltip(settings_role_description_label(role))
            .disabled(!enabled);
        let trigger = if enabled {
            trigger
                .on_click(
                    cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                        if view.consume_button_key_click(&click_id, event) {
                            return;
                        }
                        let down = Self::click_down_position(event);
                        view.on_toggle_settings_role_menu(role, down, window, cx);
                    }),
                )
                .on_activate(cx.listener(move |view, _event, window, cx| {
                    if view.open_menu.is_some() {
                        // 菜单已开时让位给 root 的菜单 Enter 处理，并重新
                        // 武装 keyup 合成 click 吞除标记。
                        view.note_button_key_activate(&activate_id);
                        return;
                    }
                    view.note_button_key_activate(&activate_id);
                    view.on_toggle_settings_role_menu(role, None, window, cx);
                    cx.stop_propagation();
                }))
        } else {
            trigger
        };
        let mut picker = Dropdown::new(self.settings_element(identifier).child(trigger));
        if menu_open && enabled {
            picker = picker.panel(self.settings_role_menu_element(role, cx));
        }
        div()
            .id(("settings-role-row", ix))
            .flex()
            .items_center()
            .gap_4()
            .py_4()
            .when(ix + 1 < SettingsRole::ALL.len(), |row| {
                row.border_b_1().border_color(dark().border.subtle)
            })
            .child(
                self.settings_element(format!("settings-role-label-{}", role.wire_name()))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(settings_label(role.label()))
                    .child(settings_copy(settings_role_description_label(role))),
            )
            .child(div().flex_none().child(picker))
    }

    /// 角色下拉当前值文案（render 与 AX 同源）：Provider · Model 显示名
    /// （清单缺失时诚实回落原始 id），未设置为 Not set。
    pub(crate) fn settings_role_value_label(&self, role: SettingsRole) -> String {
        let Some((provider_id, model_id)) =
            self.projection.settings_providers.role_value(role).cloned()
        else {
            return t("settings.roles.not_set").to_string();
        };
        let provider = self
            .projection
            .settings_providers
            .providers
            .iter()
            .find(|entry| entry.provider_id == provider_id)
            .map(|entry| entry.display_name.clone())
            .unwrap_or_else(|| provider_id.clone());
        let model = self
            .projection
            .models
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == model_id)
            .map(|model| model.display_name.clone())
            .unwrap_or_else(|| model_id.clone());
        format!("{provider} · {model}")
    }

    /// 角色下拉 gate（render / 键盘 / AX 三路径同源）：写总闸之上，该
    /// 角色写中（SetDefaultRoleModel 在途）禁用。
    pub(crate) fn settings_role_menu_enabled(&self, role: SettingsRole) -> bool {
        self.settings_writes_enabled() && self.settings_role_pending != Some(role)
    }

    /// 角色下拉浮层面板：清除行 + 已连接 provider 分组的启用模型；空候选
    /// 给诚实空态说明，不编造模型。外点关闭 / 键盘与 MenuKind 语义同源。
    fn settings_role_menu_element(
        &mut self,
        role: SettingsRole,
        cx: &mut Context<Self>,
    ) -> MenuPanel {
        let candidates = settings_role_candidates(
            &self.projection.models,
            &self.projection.settings_providers.providers,
        );
        let current = self.projection.settings_providers.role_value(role).cloned();
        let selected_ix = self.menu_selected_index();
        let highlight = self.menu_highlight_effective(selected_ix);
        let menu_id = format!("settings-role-menu-{}", role.wire_name());
        let menu_scroll = self
            .settings_element_layouts
            .entry(menu_id.clone())
            .or_default()
            .clone();
        if menu_scroll.bounds().size.height == px(0.0) {
            // 首次 prepaint 前 ScrollHandle 尚无 viewport/overflow；等本帧
            // 布局完成后定位当前项，再重绘，避免首帧滚入请求被空几何吞掉。
            let view = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = view.update(cx, |view, cx| {
                    if matches!(view.open_menu, Some(MenuKind::SettingsRole(open)) if open == role)
                    {
                        let highlight = view.menu_highlight_effective(view.menu_selected_index());
                        view.scroll_settings_role_menu_to_item(role, highlight);
                        cx.notify();
                    }
                });
            });
        }
        let mut panel = MenuPanel::new(SharedString::from(menu_id))
            .track_scroll(&menu_scroll)
            .dismiss_on_outside(
                cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                    view.dismiss_menu_on_outside(MenuKind::SettingsRole(role), event.position, cx);
                }),
            );
        panel = panel.child(
            self.settings_element(settings_role_clear_identifier(role))
                .child(
                    MenuRow::new(settings_role_clear_identifier(role))
                        .label(t("settings.roles.clear"))
                        .selected(current.is_none())
                        .highlighted(0 == highlight)
                        .on_click(cx.listener(move |view, _event, _window, cx| {
                            view.on_select_settings_role(role, None, cx);
                        })),
                ),
        );
        if candidates.is_empty() {
            // 无已连接 / 已启用模型：菜单仍可打开，给标题 + 一行指引的
            // 诚实空态；清除行仍可操作，不编造模型。
            return panel.child(
                self.settings_element(format!("settings-role-menu-empty-{}", role.wire_name()))
                    .flex()
                    .flex_col()
                    .gap(px(metrics::SPACE_1))
                    .px_2()
                    .py(px(metrics::SPACE_2))
                    .min_w_0()
                    .child(
                        div()
                            .text_size(font::SM)
                            .text_color(dark().text.primary)
                            .child(t("settings.roles.empty_title")),
                    )
                    .child(
                        div()
                            .text_size(font::XS)
                            .text_color(dark().text.secondary)
                            .child(t("settings.roles.empty_hint")),
                    ),
            );
        }
        let mut item_ix = 1;
        for (provider_id, models) in candidates {
            // 组头显示名取 provider 权威清单；候选已按连接态过滤，此处
            // 回落仅防御清单暂态缺失，不臆造能力。
            let display_name = self
                .projection
                .settings_providers
                .providers
                .iter()
                .find(|entry| entry.provider_id == provider_id)
                .map(|entry| entry.display_name.clone())
                .unwrap_or_else(|| provider_id.to_string());
            panel = panel.child(
                self.settings_element(dynamic_identifier(
                    &format!("settings-role-menu-group-{}", role.wire_name()),
                    &provider_id,
                ))
                .min_h(gpui::rems(1.5))
                .px_2()
                .flex()
                .items_center()
                .min_w_0()
                .truncate()
                .text_size(font::SM)
                .text_color(dark().text.secondary)
                .child(display_name),
            );
            for model in models {
                let selected = current.as_ref().is_some_and(|(provider, id)| {
                    provider == &model.provider_id && id == &model.id
                });
                let item_id = settings_role_item_identifier(role, &model.provider_id, &model.id);
                panel = panel.child(
                    self.settings_element(item_id).child(
                        MenuRow::new(settings_role_item_identifier(
                            role,
                            &model.provider_id,
                            &model.id,
                        ))
                        .label(model.display_name.clone())
                        .selected(selected)
                        .highlighted(item_ix == highlight)
                        .on_click(cx.listener(
                            move |view, _event, _window, cx| {
                                view.on_select_settings_role(
                                    role,
                                    Some((model.provider_id.clone(), model.id.clone())),
                                    cx,
                                );
                            },
                        )),
                    ),
                );
                item_ix += 1;
            }
        }
        panel
    }

    /// 键盘索引只计 Clear 与模型；滚动索引还包含每个供应商组头。
    pub(crate) fn scroll_settings_role_menu_to_item(&self, role: SettingsRole, item: usize) {
        let Some(scroll) = self
            .settings_element_layouts
            .get(&format!("settings-role-menu-{}", role.wire_name()))
        else {
            return;
        };
        if item == 0 {
            scroll.scroll_to_item(0);
            return;
        }
        let mut logical = 1;
        let mut child = 1;
        for (_, models) in settings_role_candidates(
            &self.projection.models,
            &self.projection.settings_providers.providers,
        ) {
            child += 1; // provider header
            if item < logical + models.len() {
                scroll.scroll_to_item(child + item - logical);
                return;
            }
            logical += models.len();
            child += models.len();
        }
    }

    /// 角色菜单触发器 toggle（render 点击 / 键盘激活 / AX Press 三路径
    /// 同源；入口级复核 gate）。
    pub(crate) fn on_toggle_settings_role_menu(
        &mut self,
        role: SettingsRole,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_role_menu_enabled(role) {
            return;
        }
        self.toggle_menu(MenuKind::SettingsRole(role), down_position, cx);
        if matches!(self.open_menu, Some(MenuKind::SettingsRole(open)) if open == role) {
            self.settings_element_layouts
                .remove(&format!("settings-role-menu-{}", role.wire_name()));
        }
    }

    /// 角色选择统一入口（菜单点击 / 键盘 Enter / AX Press 三路径同源；
    /// 入口级复核 gate 与候选目录，未知 pair fail-closed）。确认回执由
    /// DefaultRoleModelConfirmed 收敛，不在此乐观改状态。
    pub(crate) fn on_select_settings_role(
        &mut self,
        role: SettingsRole,
        value: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_role_menu_enabled(role) {
            return;
        }
        if let Some((provider_id, model_id)) = &value {
            let selectable = settings_role_menu_entries(
                &self.projection.models,
                &self.projection.settings_providers.providers,
            )
            .iter()
            .any(|model| model.provider_id == *provider_id && model.id == *model_id);
            if !selectable {
                return;
            }
        }
        self.open_menu = None;
        self.menu_highlight = None;
        self.settings_role_pending = Some(role);
        self.controller.set_default_role_model(role, value);
        cx.notify();
    }

    /// 写操作总 gate：断线 / stale 一律禁写（可见 / 键盘 / AX 三路径共用）。
    pub(crate) fn settings_action_enabled(
        &self,
        action: SettingsAuthAction,
        provider_id: &str,
        writes: bool,
        cx: &App,
    ) -> bool {
        if matches!(
            action,
            SettingsAuthAction::OpenOauth
                | SettingsAuthAction::CopyOauthUrl
                | SettingsAuthAction::CopyOauthCode
        ) {
            let state = &self.projection.settings_providers;
            if !state.providers.iter().any(|entry| {
                entry.provider_id == provider_id
                    && matches!(entry.auth, ProviderAuthState::Connecting)
            }) {
                return false;
            }
            let Some(wait) = state.oauth_waits.get(provider_id) else {
                return false;
            };
            return match action {
                SettingsAuthAction::OpenOauth => {
                    writes && oauth_url_can_open(&wait.verification_url)
                }
                SettingsAuthAction::CopyOauthUrl => !wait.verification_url.is_empty(),
                SettingsAuthAction::CopyOauthCode => {
                    wait.user_code.as_ref().is_some_and(|code| !code.is_empty())
                }
                _ => unreachable!(),
            };
        }
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
    ) -> impl IntoElement {
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
        let button = Button::new(id.clone())
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(
                if self
                    .settings_copied_auth
                    .as_ref()
                    .is_some_and(|(id, copied)| id == provider_id && *copied == action)
                {
                    t("settings.providers.copied")
                } else {
                    action.label()
                },
            )
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
        let button = if tooltip.is_empty() {
            button
        } else {
            button.tooltip(tooltip)
        };
        self.settings_element(id).flex_none().child(button)
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
            SettingsAuthAction::OpenOauth
            | SettingsAuthAction::CopyOauthUrl
            | SettingsAuthAction::CopyOauthCode => {
                let Some(wait) = self
                    .projection
                    .settings_providers
                    .oauth_waits
                    .get(&provider_id)
                else {
                    return;
                };
                if action == SettingsAuthAction::OpenOauth {
                    cx.open_url(&wait.verification_url);
                } else {
                    let value = if action == SettingsAuthAction::CopyOauthCode {
                        wait.user_code.clone().unwrap_or_default()
                    } else {
                        wait.verification_url.clone()
                    };
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
                    self.settings_copied_auth = Some((provider_id, action));
                    cx.notify();
                }
            }
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

    /// 「Manage models」弹层入口 gate（render / 键盘 / AX 三路径同源）：
    /// 写总闸之上要求 provider 已连接且目录可用——未连接先走认证流程，
    /// 目录不可用无全量可谈（ADR-055 D2）。
    pub(crate) fn settings_manage_models_enabled(
        &self,
        provider: &ProviderAuthStatusEntry,
    ) -> bool {
        self.settings_writes_enabled()
            && matches!(provider.auth, ProviderAuthState::Connected { .. })
            && !matches!(
                provider.catalog,
                crate::projection::ProviderCatalogState::Unavailable { .. }
            )
    }

    /// 「Manage models」弹层触发器 toggle（三路径同源；入口级复核 gate）。
    /// 打开时重查全量目录（include_disabled=true）取权威状态。
    pub(crate) fn on_toggle_settings_models_menu(
        &mut self,
        provider_id: String,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let enabled = self
            .projection
            .settings_providers
            .providers
            .iter()
            .any(|entry| {
                entry.provider_id == provider_id && self.settings_manage_models_enabled(entry)
            });
        if !enabled {
            return;
        }
        self.controller.load_model_catalog();
        self.toggle_menu(
            MenuKind::SettingsProviderModels(provider_id),
            down_position,
            cx,
        );
    }

    /// 弹层内单模型 Switch 切换（三路径同源；入口级复核 gate 与全量目录
    /// 条目，未知 pair fail-closed）。不乐观改状态：Switch 以 Host 回执
    /// 为准翻转，随后的权威重查对齐角色默认与 Composer。
    pub(crate) fn on_toggle_provider_model(
        &mut self,
        provider_id: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writes_enabled() {
            return;
        }
        if !matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &provider_id)
        {
            return;
        }
        if self
            .projection
            .settings_providers
            .model_write_pending
            .is_some()
        {
            return;
        }
        let Some(entry) = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .find(|model| model.provider_id == provider_id && model.id == model_id)
        else {
            return;
        };
        let target = !entry.enabled;
        self.projection.settings_providers.model_write_pending =
            Some(crate::projection::ProviderModelWrite::Model {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
            });
        self.controller
            .set_model_enabled(provider_id, model_id, target);
        cx.notify();
    }

    /// 弹层 Enable all / Disable all（三路径同源；空目录禁用——Host 全关
    /// 对空目录 fail-closed catalog_unavailable，UI 不发即防错；写在途
    /// 禁用防重复提交）。
    pub(crate) fn on_toggle_provider_models_all(
        &mut self,
        provider_id: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_writes_enabled() {
            return;
        }
        if !matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &provider_id)
        {
            return;
        }
        if self
            .projection
            .settings_providers
            .model_write_pending
            .is_some()
        {
            return;
        }
        let has_models = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .any(|model| model.provider_id == provider_id);
        if !has_models {
            return;
        }
        self.projection.settings_providers.model_write_pending =
            Some(crate::projection::ProviderModelWrite::All {
                provider_id: provider_id.clone(),
            });
        self.controller
            .set_provider_models_enabled(provider_id, enabled);
        cx.notify();
    }

    /// 「Manage models」弹层面板（OPT-3a / ADR-055 D2/D3）：标题 +
    /// Enable all / Disable all + 每模型一行 Switch；空目录给诚实空态 +
    /// Refresh catalog（复用页级刷新路径）；写在途禁用对应控件。外点
    /// 关闭 / Escape 与 MenuKind 语义同源。
    fn settings_models_menu_element(
        &mut self,
        provider_id: &str,
        cx: &mut Context<Self>,
    ) -> MenuPanel {
        let writes = self.settings_writes_enabled();
        let pending = self
            .projection
            .settings_providers
            .model_write_pending
            .as_ref()
            .is_some_and(|pending| pending.targets(provider_id));
        let models: Vec<ModelEntry> = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .cloned()
            .collect();
        let dismiss_provider = provider_id.to_string();
        let menu_id = settings_models_menu_identifier(provider_id);
        let menu_scroll = self
            .settings_element_layouts
            .entry(menu_id.clone())
            .or_default()
            .clone();
        let mut panel = MenuPanel::new(SharedString::from(menu_id))
            .track_scroll(&menu_scroll)
            .max_height(SETTINGS_MODELS_MENU_MAX_HEIGHT)
            .dismiss_on_outside(
                cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                    view.dismiss_menu_on_outside(
                        MenuKind::SettingsProviderModels(dismiss_provider.clone()),
                        event.position,
                        cx,
                    );
                }),
            );
        // 头行：标题 + Enable all / Disable all（空目录两者禁用）。
        let all_enabled = writes && !pending && !models.is_empty();
        let mut header = div()
            .w(px(SETTINGS_MODELS_MENU_WIDTH))
            .flex()
            .flex_col()
            .gap_3()
            .px_2()
            .py_2()
            .child(
                self.settings_element(format!("settings-models-heading-{provider_id}"))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(settings_label(t("settings.providers.models_title")))
                    .child(settings_copy(crate::ui::i18n::t2(
                        "settings.providers.models_count",
                        &models
                            .iter()
                            .filter(|model| model.enabled)
                            .count()
                            .to_string(),
                        &models.len().to_string(),
                    ))),
            )
            .child(
                self.settings_element(format!("settings-models-scope-{provider_id}"))
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .child(t("settings.providers.catalog_scope")),
            );
        let mut bulk_actions = div().flex().flex_wrap().gap_2();
        for (identifier, label, target) in [
            (
                settings_models_enable_all_identifier(provider_id),
                t("settings.providers.models_enable_all"),
                true,
            ),
            (
                settings_models_disable_all_identifier(provider_id),
                t("settings.providers.models_disable_all"),
                false,
            ),
        ] {
            let focus = self
                .settings_action_focus
                .entry(identifier.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let click_id = identifier.clone();
            let click_provider = provider_id.to_string();
            let activate_id = identifier.clone();
            let activate_provider = provider_id.to_string();
            let button = Button::new(identifier.clone())
                .track_focus(&focus)
                .variant(ButtonVariant::Raised)
                .height(px(SETTINGS_CONTROL_HEIGHT))
                .vcenter()
                .radius(6.0)
                .bordered()
                .text_size(font::SM)
                .label(label)
                .disabled(!all_enabled)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&click_id, event) {
                        return;
                    }
                    view.on_toggle_provider_models_all(click_provider.clone(), target, cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&activate_id);
                    view.on_toggle_provider_models_all(activate_provider.clone(), target, cx);
                    cx.stop_propagation();
                }));
            bulk_actions =
                bulk_actions.child(self.settings_element(identifier).flex_none().child(button));
        }
        header = header.child(bulk_actions);
        panel = panel.child(header);
        if models.is_empty() {
            // 空目录诚实空态：Enable / Disable all 已禁用，Refresh 复用
            // 页级刷新路径（provider 状态 + 两套目录口径）。
            let refresh_id = settings_models_refresh_identifier(provider_id);
            let refresh_focus = self
                .settings_action_focus
                .entry(refresh_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let refresh_click_id = refresh_id.clone();
            let refresh_activate_id = refresh_id.clone();
            let refresh = Button::new(refresh_id.clone())
                .track_focus(&refresh_focus)
                .variant(ButtonVariant::Raised)
                .height(px(SETTINGS_CONTROL_HEIGHT))
                .vcenter()
                .radius(6.0)
                .bordered()
                .text_size(font::SM)
                .label(t("settings.providers.models_refresh"))
                .disabled(!writes)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&refresh_click_id, event) {
                        return;
                    }
                    view.on_refresh_settings(cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&refresh_activate_id);
                    view.on_refresh_settings(cx);
                    cx.stop_propagation();
                }));
            return panel.child(
                self.settings_element(format!("settings-models-empty-{provider_id}"))
                    .flex()
                    .flex_col()
                    .gap(px(metrics::SPACE_1))
                    .px_2()
                    .py(px(metrics::SPACE_2))
                    .min_w_0()
                    .child(
                        div()
                            .text_size(font::SM)
                            .text_color(dark().text.primary)
                            .child(t("settings.providers.models_empty_title")),
                    )
                    .child(
                        div()
                            .text_size(font::XS)
                            .text_color(dark().text.secondary)
                            .child(t("settings.providers.models_empty_hint")),
                    )
                    .child(
                        self.settings_element(refresh_id)
                            .flex_none()
                            .pt_1()
                            .child(refresh),
                    ),
            );
        }
        for model in models {
            let switch_id = settings_model_switch_identifier(provider_id, &model.id);
            let focus = self
                .settings_action_focus
                .entry(switch_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let click_id = switch_id.clone();
            let click_provider = provider_id.to_string();
            let click_model = model.id.clone();
            let activate_id = switch_id.clone();
            let activate_provider = provider_id.to_string();
            let activate_model = model.id.clone();
            let row_switch = Switch::new(switch_id.clone())
                .track_focus(&focus)
                .checked(model.enabled)
                .disabled(!writes || pending)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&click_id, event) {
                        return;
                    }
                    view.on_toggle_provider_model(click_provider.clone(), click_model.clone(), cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&activate_id);
                    view.on_toggle_provider_model(
                        activate_provider.clone(),
                        activate_model.clone(),
                        cx,
                    );
                    cx.stop_propagation();
                }));
            panel = panel.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .min_h(gpui::rems(3.5))
                    .py_2()
                    .border_t_1()
                    .border_color(dark().border.subtle)
                    .px_1()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(font::SM)
                                    .text_color(dark().text.primary)
                                    .child(model.display_name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(font::XS)
                                    .text_color(dark().text.tertiary)
                                    .child(model.id.clone()),
                            ),
                    )
                    .child(
                        self.settings_element(switch_id)
                            .flex_none()
                            .child(row_switch),
                    ),
            );
        }
        panel
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
        self.settings_auth_details.retain(|id, _| {
            self.projection
                .settings_providers
                .providers
                .iter()
                .any(|entry| &entry.provider_id == id)
        });
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
            // 展开 chevron 随 provider 清单常驻（折叠 / 展开均渲染）。
            action_ids.insert(settings_provider_expand_identifier(&entry.provider_id));
            // 展开区控件（代理 Switch / Manage models 触发器 / 弹层内
            // 控件）只在卡片有效展开时渲染；可见性与回收白名单同源
            //（代理开关另受全局代理配置 gate）。
            let editor_open = self.settings_api_key_editor_visible(entry);
            let oauth_waiting = self
                .projection
                .settings_providers
                .oauth_waits
                .contains_key(&entry.provider_id);
            let remove_confirm = self
                .settings_remove_confirm
                .as_deref()
                .is_some_and(|id| id == entry.provider_id);
            if self.settings_provider_card_expanded(
                &entry.provider_id,
                editor_open,
                oauth_waiting,
                remove_confirm,
            ) {
                if self.projection.settings_general.proxy_url.is_some() {
                    action_ids.insert(settings_use_proxy_identifier(&entry.provider_id));
                }
                action_ids.insert(settings_manage_models_identifier(&entry.provider_id));
                // 弹层内控件（Enable / Disable all、Refresh、每模型
                // Switch）只在该 provider 弹层打开时存在；句柄随目录条目
                // 建立、随白名单回收。
                if matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &entry.provider_id)
                {
                    action_ids.insert(settings_models_enable_all_identifier(&entry.provider_id));
                    action_ids.insert(settings_models_disable_all_identifier(&entry.provider_id));
                    action_ids.insert(settings_models_refresh_identifier(&entry.provider_id));
                    for model in self
                        .projection
                        .settings_providers
                        .model_catalog
                        .iter()
                        .filter(|model| model.provider_id == entry.provider_id)
                    {
                        action_ids.insert(settings_model_switch_identifier(
                            &entry.provider_id,
                            &model.id,
                        ));
                    }
                }
            }
        }
        // 四默认角色下拉触发器随页常驻（OPT-3b）。
        for role in SettingsRole::ALL {
            action_ids.insert(settings_role_trigger_identifier(role));
        }
        self.settings_action_focus
            .retain(|id, _| action_ids.contains(id));
        for id in ids {
            self.settings_api_key_inputs
                .entry(id.clone())
                .or_insert_with(|| {
                    let element_id = format!("settings-api-key-input-{id}");
                    cx.new(|cx| {
                        TextInput::with_placeholder(t("settings.providers.api_key_placeholder"), cx)
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

    /// 转义后的 "<provider>:<model>" → 原始 pair（以全量目录为权威，
    /// 未知 fail-closed；不反解转义）。AX 派发用（OPT-3a 弹层 Switch）。
    pub(crate) fn settings_model_target_for_escaped(
        &self,
        escaped: &str,
    ) -> Option<(String, String)> {
        self.projection
            .settings_providers
            .model_catalog
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
        self.settings_remove_confirm = None;
        self.settings_mcp_remove_confirm = None;
        // OPT-3a 弹层本地编辑状态：在途标记与 cleared_roles 说明随离开
        // Settings 清空（Host 权威状态不受影响）。
        self.projection.settings_providers.model_write_pending = None;
        self.projection.settings_providers.model_cleared_note = None;
    }
}
