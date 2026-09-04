//! Settings terminal 页。

use super::*;

impl AppView {
    /// 「终端」页（SET-6d / ADR-050）：Host 权威生效值（shell 持久值 +
    /// columns/rows 生效值）、shell 内联输入 + columns/rows 数值输入 +
    /// Save（全态回传三字段）/ Clear（清除 shell）、生效边界文案；stale
    /// 只读，写入口与 AX 同 gate。
    pub(super) fn settings_terminal_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
