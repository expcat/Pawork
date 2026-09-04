//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Focusable, Window};

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::projection::ConnectionState;
use crate::ui::settings::{
    parse_terminal_dimension, terminal_save_enabled, terminal_status_lines,
    SETTINGS_TERMINAL_EFFECT_NOTE, SETTINGS_TERMINAL_SHELL_UNSET,
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
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const STATUS_HEIGHT: f32 = 20.0;
        const CONTROL_ROW: f32 = 28.0;
        let state = &self.projection.settings_terminal;
        let writes = self.settings_terminal_writes_enabled();
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let shell_current = state
            .shell
            .clone()
            .unwrap_or_else(|| SETTINGS_TERMINAL_SHELL_UNSET.to_string());
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
        let width = super::settings::settings_content_ax_width(frame);
        let mut page = AxNode::new("settings-page", AxRole::Group, "Terminal", frame)
            .child(
                AxNode::new(
                    "settings-page-title",
                    AxRole::StaticText,
                    "Terminal",
                    AxRect::new(
                        frame.x + 16.0,
                        frame.y + 16.0,
                        (width - 136.0).max(0.0),
                        HEADING_HEIGHT + SUBTITLE_HEIGHT,
                    ),
                )
                .value("Default shell and size for new terminals"),
            )
            .child(
                AxNode::new(
                    "settings-refresh",
                    AxRole::Button,
                    "Refresh",
                    AxRect::new(
                        frame.x + 16.0 + width - 96.0,
                        frame.y + 16.0,
                        96.0,
                        CONTROL_ROW,
                    ),
                )
                .enabled(connected)
                .focused(refresh_focused)
                .action(AxAction::Press),
            );
        let mut y = frame.y + 16.0 + HEADING_HEIGHT + SUBTITLE_HEIGHT + 8.0;
        for (kind, label) in terminal_status_lines(state) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    "Terminal status",
                    AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
                )
                .value(label),
            );
            y += STATUS_HEIGHT + 8.0;
        }
        page = page
            .child(
                AxNode::new(
                    "settings-terminal-shell-current",
                    AxRole::StaticText,
                    "Default shell",
                    AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
                )
                .value(shell_current),
            )
            .child(
                AxNode::new(
                    "settings-terminal-size-current",
                    AxRole::StaticText,
                    "Default size",
                    AxRect::new(
                        frame.x + 16.0,
                        y + STATUS_HEIGHT + 8.0,
                        width,
                        STATUS_HEIGHT,
                    ),
                )
                .value(size_current),
            );
        y += STATUS_HEIGHT * 2.0 + 16.0;
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
                    "Shell",
                    AxRect::new(frame.x + 16.0, y, (width - 88.0).max(120.0), CONTROL_ROW),
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
                    "Clear",
                    AxRect::new(frame.x + 16.0 + width - 80.0, y, 80.0, CONTROL_ROW),
                )
                .enabled(clear_enabled)
                .focused(
                    self.open_menu.is_none()
                        && self.settings_terminal_clear_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );
        y += CONTROL_ROW + 8.0;
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
                    "Columns",
                    AxRect::new(frame.x + 16.0, y, 96.0, CONTROL_ROW),
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
                    "Rows",
                    AxRect::new(frame.x + 16.0 + 96.0 + 16.0, y, 96.0, CONTROL_ROW),
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
                    "Save",
                    AxRect::new(frame.x + 16.0 + width - 80.0, y, 80.0, CONTROL_ROW),
                )
                .enabled(save_enabled)
                .focused(
                    self.open_menu.is_none()
                        && self.settings_terminal_save_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );
        y += CONTROL_ROW + 8.0;
        page.child(
            AxNode::new(
                "settings-terminal-effect",
                AxRole::StaticText,
                "Effect",
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(SETTINGS_TERMINAL_EFFECT_NOTE),
        )
    }
}
