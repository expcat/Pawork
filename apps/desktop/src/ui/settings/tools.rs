//! Settings tools 页。

use super::*;

impl AppView {
    pub(super) fn settings_tools_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .label(t("settings.refresh"))
            .tooltip(t("settings.tools.refresh_tooltip"))
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
                                    Label::new(t("settings.tools.title"))
                                        .size(font::TITLE)
                                        .color(dark().text.primary),
                                ),
                            )
                            .child(
                                Label::new(
                                    t("settings.tools.subtitle"),
                                )
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
        content = content.child(status_line(settings_mcp_effect_note(), dark().text.secondary));

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
    pub(super) fn settings_mcp_server_card(
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
                settings_mcp_remove_confirm_note(),
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
                SettingsMcpAction::Test => t("settings.tools.tooltip_test"),
                SettingsMcpAction::Remove | SettingsMcpAction::ConfirmRemove => {
                    t("settings.tools.tooltip_remove")
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
    pub(super) fn settings_mcp_action_button(
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

    /// MCP 写动作入口（Test / Remove / 确认 / 取消）：入口级复核 gate
    /// 与权威清单；未知 server fail-closed。
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
}
