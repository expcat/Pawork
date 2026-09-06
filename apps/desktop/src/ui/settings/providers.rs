//! Settings providers 页。

use std::collections::HashSet;

use gpui::{Point, SharedString, Window};

use super::*;
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::switch::Switch;
use crate::ui::MenuKind;

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
        // OPT-4c（F2）：外层脚手架（全宽 + 两侧 32px + 滚动）统一在
        // settings_page_element；本页只提供内容列。
        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
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
                                Label::new(t("settings.providers.title"))
                                    .size(font::TITLE)
                                    .color(dark().text.primary),
                            ),
                        )
                        .child(
                            Label::new(t("settings.providers.subtitle"))
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

        // OPT-3b / ADR-055 D5：「Default models」四角色区（刷新行之下、
        // Providers 列表之上）。
        content = content.child(self.settings_default_roles_section(cx));

        content = content.child(
            div().font_weight(FontWeight::MEDIUM).child(
                Label::new(t("settings.providers.section_providers"))
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

        content
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
        // 认证动作使用独立详情行，避免窄窗 / 150% 下挤压目录列。
        let actions_in_details = !row_actions.is_empty();
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
            t("settings.providers.no_auth_method").to_string()
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
        // 「Manage models」入口（OPT-3a / ADR-055 D2）：断线 / 未连接 /
        // 目录不可用禁用（disabled 不发布 Press）；打开弹层取全量目录。
        // render 按钮 / 键盘 / AX 三路径同 identifier、同 gate。
        let manage_enabled = self.settings_manage_models_enabled(provider);
        let manage_id = settings_manage_models_identifier(&provider_id);
        let manage_focus = self
            .settings_action_focus
            .entry(manage_id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let models_menu_open = matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &provider_id);
        let manage_click_id = manage_id.clone();
        let manage_click_provider = provider_id.clone();
        let manage_activate_id = manage_id.clone();
        let manage_activate_provider = provider_id.clone();
        let manage_trigger = Button::new(manage_id)
            .track_focus(&manage_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
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
        let mut manage_picker = Dropdown::new(manage_trigger);
        if models_menu_open && manage_enabled {
            manage_picker =
                manage_picker.panel(self.settings_models_menu_element(&provider_id, cx));
        }
        header_actions = header_actions.child(div().flex_none().child(manage_picker));
        // 供应商级代理开关（ADR-052 SET-6h / OPT-3c）：仅在配置了全局代理
        // 时可见；控件形态为 Switch（轨道 + 圆点，开 = 主色）+ 状态文案，
        // 命令仍走 set_provider_use_proxy。与 AX 同 identifier / 同 gate
        //（writes 总闸），checked 态以 Host 回执为准收敛。
        let proxy_visible = self.projection.settings_general.proxy_url.is_some();
        if proxy_visible {
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
            let toggle = Switch::new(id)
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
            header_actions = header_actions.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .child(toggle)
                    .child(
                        Label::new(state_label)
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    ),
            );
        }
        if !actions_in_details {
            for action in &row_actions {
                let tooltip = if *action == SettingsAuthAction::Remove {
                    t("settings.providers.tooltip_remove_credential")
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
                Label::new(
                    t("settings.providers.authorize_at").replace("{}", &wait.verification_url),
                )
                .size(font::BODY_SM)
                .color(dark().text.secondary),
            );
            if let Some(code) = &wait.user_code {
                details = details.child(
                    Label::new(t("settings.providers.oauth_code").replace("{}", code))
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                );
            }
            if let Some(expires) = &wait.expires_at {
                details = details.child(
                    Label::new(t("settings.providers.oauth_expires").replace("{}", expires))
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
                &t("settings.providers.connection_error").replace("{}", message),
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
                Label::new(
                    t("settings.providers.endpoint_row").replace("{}", &provider.endpoint_label),
                )
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
                                t("settings.providers.api_key_empty")
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

        // 认证动作与 destructive 二次确认位于详情，概览保持 64px。
        if actions_in_details && !row_actions.is_empty() {
            let mut row = div().flex().flex_row().gap_1().flex_wrap();
            for action in &row_actions {
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
            details = details.child(row);
        }
        if detail_visible {
            card = card.child(details);
        }
        card
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
        let mut section = div()
            .id("settings-default-roles")
            .flex()
            .flex_col()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div().font_weight(FontWeight::MEDIUM).child(
                            Label::new(t("settings.roles.title"))
                                .size(font::TITLE)
                                .color(dark().text.primary),
                        ),
                    )
                    .child(
                        Label::new(t("settings.roles.subtitle"))
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    ),
            );
        if unavailable {
            section = section.child(status_line(
                settings_default_unavailable_note(),
                dark().semantic.danger_text,
            ));
        }
        let mut card = div()
            .id("settings-default-roles-card")
            .flex()
            .flex_col()
            .min_w_0()
            .rounded(px(6.0))
            .border_1()
            .border_color(dark().border.subtle)
            .bg(dark().surface.raised);
        for (ix, role) in SettingsRole::ALL.into_iter().enumerate() {
            card = card.child(self.settings_role_row(ix, role, cx));
        }
        section.child(card)
    }

    /// 单个角色行：左角色名、中下拉触发器（Dropdown 浮层面板）、右用途
    /// 说明。可见 / 键盘 / AX 三路径同 identifier、同 gate（断线 / stale /
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
        let trigger = Button::new(identifier)
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .width(px(SETTINGS_ROLE_MENU_WIDTH))
            .vcenter()
            .radius(4.0)
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
        let mut picker = Dropdown::new(trigger);
        if menu_open && enabled {
            picker = picker.panel(self.settings_role_menu_element(role, cx));
        }
        div()
            .id(("settings-role-row", ix))
            .h(px(SETTINGS_ROLE_ROW_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w_0()
            .px_2()
            .when(ix + 1 < SettingsRole::ALL.len(), |row| {
                row.border_b_1().border_color(dark().border.subtle)
            })
            .child(
                div()
                    .w(px(SETTINGS_ROLE_LABEL_WIDTH))
                    .min_w_0()
                    .truncate()
                    .child(
                        Label::new(role.label())
                            .size(font::BODY)
                            .color(dark().text.primary),
                    ),
            )
            .child(div().flex_none().child(picker))
            .child(
                div().flex_1().min_w_0().truncate().child(
                    Label::new(settings_role_description_label(role))
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                ),
            )
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
        let mut panel = MenuPanel::new(SharedString::from(format!(
            "settings-role-menu-{}",
            role.wire_name()
        )))
        .dismiss_on_outside(cx.listener(
            move |view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::SettingsRole(role), event.position, cx);
            },
        ));
        if candidates.is_empty() {
            // 无已连接 / 已启用模型：菜单仍可打开，给标题 + 一行指引的
            // 诚实空态；无可选项，不编造模型。
            return panel.child(
                div()
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
        panel = panel.child(
            MenuRow::new(settings_role_clear_identifier(role))
                .label(t("settings.roles.clear"))
                .selected(current.is_none())
                .highlighted(0 == highlight)
                .on_click(cx.listener(move |view, _event, _window, cx| {
                    view.on_select_settings_role(role, None, cx);
                })),
        );
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
                div()
                    .h(px(SETTINGS_ROLE_MENU_GROUP_HEADER_HEIGHT))
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
                panel = panel.child(
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
                );
                item_ix += 1;
            }
        }
        panel
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
        let mut panel = MenuPanel::new(SharedString::from(settings_models_menu_identifier(
            provider_id,
        )))
        .max_height(SETTINGS_MODELS_MENU_MAX_HEIGHT)
        .dismiss_on_outside(cx.listener(
            move |view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(
                    MenuKind::SettingsProviderModels(dismiss_provider.clone()),
                    event.position,
                    cx,
                );
            },
        ));
        // 头行：标题 + Enable all / Disable all（空目录两者禁用）。
        let all_enabled = writes && !pending && !models.is_empty();
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .min_w_0()
            .h(px(SETTINGS_MODELS_MENU_HEADER_HEIGHT))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(font::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(dark().text.primary)
                    .child(t("settings.providers.models_title")),
            );
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
            let button = Button::new(identifier)
                .track_focus(&focus)
                .variant(ButtonVariant::Raised)
                .height(px(SETTINGS_MODELS_MENU_HEADER_HEIGHT))
                .vcenter()
                .radius(4.0)
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
            header = header.child(button);
        }
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
            let refresh = Button::new(refresh_id)
                .track_focus(&refresh_focus)
                .variant(ButtonVariant::Raised)
                .height(px(SETTINGS_ACTION_HEIGHT))
                .vcenter()
                .radius(4.0)
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
                div()
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
                    .child(div().flex_none().pt_1().child(refresh)),
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
            let row_switch = Switch::new(switch_id)
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
                    .h(px(SETTINGS_MODELS_MENU_ROW_HEIGHT))
                    .px_1()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(font::SM)
                                    .text_color(dark().text.primary)
                                    .child(model.display_name.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(font::XS)
                                    .text_color(dark().text.tertiary)
                                    .child(model.id.clone()),
                            ),
                    )
                    .child(div().flex_none().child(row_switch)),
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
            // 代理开关随全局代理配置可见；可见性与回收白名单同源。
            if self.projection.settings_general.proxy_url.is_some() {
                action_ids.insert(settings_use_proxy_identifier(&entry.provider_id));
            }
            // 「Manage models」触发器随 provider 清单常驻（OPT-3a）。
            action_ids.insert(settings_manage_models_identifier(&entry.provider_id));
            // 弹层内控件（Enable / Disable all、Refresh、每模型 Switch）
            // 只在该 provider 弹层打开时存在；句柄随目录条目建立、随
            // 白名单回收。
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
