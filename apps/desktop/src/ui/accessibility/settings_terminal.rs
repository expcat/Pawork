//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Focusable, Window};

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::projection::ConnectionState;
use crate::ui::i18n::t;
use crate::ui::settings::{
    parse_terminal_dimension, settings_terminal_effect_note, settings_terminal_shell_unset,
    terminal_save_enabled, terminal_status_lines,
};
use crate::ui::AppView;

impl AppView {
    /// 「终端」页 AX（SET-6d / ADR-050）：shell / columns / rows 输入
    ///（TextArea，Focus / SetValue）+ Save / Clear（Press）+ 生效边界；
    /// stale 时 enabled=false 且 permits 拒绝写动作，与 render 同 gate
    ///（尺寸解析同源 parse_terminal_dimension）。
    pub(crate) fn settings_terminal_page_ax(
        &self,
        window: &Window,
        cx: &App,
        frame: AxRect,
    ) -> AxNode {
        let state = &self.projection.settings_terminal;
        let writes = self.settings_terminal_writes_enabled();
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let shell_current = state
            .shell
            .clone()
            .unwrap_or_else(|| settings_terminal_shell_unset().to_string());
        let size_current = format!("{}×{}", state.columns, state.rows);
        let shell_text = self
            .settings_terminal_shell_input
            .read(cx)
            .text()
            .trim()
            .to_string();
        let columns_value =
            parse_terminal_dimension(self.settings_terminal_columns_input.read(cx).text());
        let rows_value =
            parse_terminal_dimension(self.settings_terminal_rows_input.read(cx).text());
        let save_enabled = terminal_save_enabled(writes, columns_value, rows_value);
        let clear_enabled = writes && state.shell.is_some();
        let refresh_focused =
            self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window);

        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.terminal.title"),
            frame,
        )
        .child(
            AxNode::new(
                "settings-page-title",
                AxRole::StaticText,
                t("settings.terminal.title"),
                self.settings_element_bounds("settings-page-title"),
            )
            .value(t("settings.terminal.subtitle")),
        )
        .child(
            AxNode::new(
                "settings-refresh",
                AxRole::Button,
                t("settings.refresh"),
                self.settings_element_bounds("settings-refresh"),
            )
            .enabled(connected)
            .focused(refresh_focused)
            .action(AxAction::Press),
        );

        for (kind, label) in terminal_status_lines(state) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    t("settings.terminal.ax_status"),
                    self.settings_element_bounds(&format!("settings-status-{kind}")),
                )
                .value(label),
            );
        }
        page = page
            .child(
                AxNode::new(
                    "settings-terminal-shell-current",
                    AxRole::StaticText,
                    t("settings.terminal.ax_default_shell"),
                    self.settings_element_bounds("settings-terminal-shell-current"),
                )
                .value(shell_current),
            )
            .child(
                AxNode::new(
                    "settings-terminal-size-current",
                    AxRole::StaticText,
                    t("settings.terminal.ax_default_size"),
                    self.settings_element_bounds("settings-terminal-size-current"),
                )
                .value(size_current),
            );

        let shell_input_focused = self.open_menu.is_none()
            && self
                .settings_terminal_shell_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        page = page
            .child(
                AxNode::new(
                    "settings-terminal-shell-input",
                    AxRole::TextArea,
                    t("settings.terminal.shell_label"),
                    self.settings_element_bounds("settings-terminal-shell-input"),
                )
                .value(shell_text)
                .enabled(writes)
                .focused(shell_input_focused)
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "settings-terminal-clear",
                    AxRole::Button,
                    t("settings.clear"),
                    self.settings_element_bounds("settings-terminal-clear"),
                )
                .enabled(clear_enabled)
                .focused(
                    self.open_menu.is_none()
                        && self.settings_terminal_clear_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );

        let columns_input_focused = self.open_menu.is_none()
            && self
                .settings_terminal_columns_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        let rows_input_focused = self.open_menu.is_none()
            && self
                .settings_terminal_rows_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        page = page
            .child(
                AxNode::new(
                    "settings-terminal-columns-input",
                    AxRole::TextArea,
                    t("settings.terminal.ax_columns"),
                    self.settings_element_bounds("settings-terminal-columns-input"),
                )
                .value(
                    self.settings_terminal_columns_input
                        .read(cx)
                        .text()
                        .to_string(),
                )
                .enabled(writes)
                .focused(columns_input_focused)
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "settings-terminal-rows-input",
                    AxRole::TextArea,
                    t("settings.terminal.ax_rows"),
                    self.settings_element_bounds("settings-terminal-rows-input"),
                )
                .value(
                    self.settings_terminal_rows_input
                        .read(cx)
                        .text()
                        .to_string(),
                )
                .enabled(writes)
                .focused(rows_input_focused)
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "settings-terminal-save",
                    AxRole::Button,
                    t("settings.save"),
                    self.settings_element_bounds("settings-terminal-save"),
                )
                .enabled(save_enabled)
                .focused(
                    self.open_menu.is_none()
                        && self.settings_terminal_save_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );

        page.child(
            AxNode::new(
                "settings-terminal-effect",
                AxRole::StaticText,
                t("settings.terminal.ax_effect"),
                self.settings_element_bounds("settings-terminal-effect"),
            )
            .value(settings_terminal_effect_note()),
        )
    }
}
