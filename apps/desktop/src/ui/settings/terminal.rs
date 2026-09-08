//! Settings terminal 页。

use super::*;

impl AppView {
    /// 「终端」页（SET-6d / ADR-050）：Host 权威生效值（shell 持久值 +
    /// columns/rows 生效值）、shell 内联输入 + columns/rows 数值输入 +
    /// Save（全态回传三字段）/ Clear（清除 shell）、生效边界文案；stale
    /// 只读，写入口与 AX 同 gate。
    pub(super) fn settings_terminal_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
            .unwrap_or_else(|| settings_terminal_shell_unset().to_string());
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
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BASE)
            .label(t("settings.refresh"))
            .tooltip(t("settings.terminal.refresh_tooltip"))
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
            .variant(ButtonVariant::Primary)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BASE)
            .label(t("settings.save"))
            .tooltip(t("settings.terminal.save_tooltip"))
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
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BASE)
            .label(t("settings.clear"))
            .tooltip(t("settings.terminal.clear_tooltip"))
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

        let mut content = settings_column().child(
            div()
                .flex()
                .items_start()
                .gap_6()
                .child(self.settings_heading(
                    t("settings.terminal.title"),
                    t("settings.terminal.subtitle"),
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
        let shell = settings_section()
            .child(settings_label(t("settings.terminal.shell_label")))
            .child(self.settings_note(
                "settings-terminal-shell-current",
                t("settings.terminal.current_shell").replace("{}", &shell_current),
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        self.settings_element("settings-terminal-shell-input")
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .when(!writes, |el| el.opacity(0.55))
                            .child(shell_input),
                    )
                    .child(
                        self.settings_element("settings-terminal-clear")
                            .flex_none()
                            .child(clear),
                    ),
            );
        let size = settings_section()
            .child(settings_label(t("settings.terminal.size_label")))
            .child(self.settings_note(
                "settings-terminal-size-current",
                t("settings.terminal.current_size").replace("{}", &size_current),
            ))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(settings_copy(t("settings.terminal.ax_columns")))
                            .child(
                                self.settings_element("settings-terminal-columns-input")
                                    .flex()
                                    .w(px(112.0))
                                    .when(!writes, |el| el.opacity(0.55))
                                    .child(columns_input),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(settings_copy(t("settings.terminal.ax_rows")))
                            .child(
                                self.settings_element("settings-terminal-rows-input")
                                    .flex()
                                    .w(px(112.0))
                                    .when(!writes, |el| el.opacity(0.55))
                                    .child(rows_input),
                            ),
                    ),
            );
        content.child(shell).child(size).child(
            settings_section().child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_6()
                    .child(
                        self.settings_element("settings-terminal-effect")
                            .flex_1()
                            .min_w_0()
                            .child(settings_copy(settings_terminal_effect_note())),
                    )
                    .child(
                        self.settings_element("settings-terminal-save")
                            .flex_none()
                            .child(save),
                    ),
            ),
        )
    }

    /// 终端页 Save（SET-6d；三路径同源）：shell/columns/rows 三字段全态
    /// 回传（ADR-050 D3）；空 shell 映射为 null（跟随平台默认），畸形 /
    /// 越界尺寸禁 Save。
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
}
