//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Focusable, Window};

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::projection::ConnectionState;
use crate::ui::i18n::t;
use crate::ui::settings::{
    general_status_lines, settings_proxy_effect_note, settings_proxy_storage_note,
    settings_proxy_unset,
};
use crate::ui::AppView;

impl AppView {
    /// 「Network」页 AX（SET-6a）：当前值 / 输入 / Save / Clear / 生效边界；
    /// stale 时 enabled=false，permits 拒绝写动作，与 render 同 gate。
    pub(crate) fn settings_general_page_ax(
        &self,
        window: &Window,
        cx: &App,
        frame: AxRect,
    ) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const STATUS_HEIGHT: f32 = 20.0;
        const CONTROL_ROW: f32 = 28.0;
        let state = &self.projection.settings_general;
        let writes = self.settings_general_writes_enabled();
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let current = match &state.proxy_url {
            Some(url) => url.clone(),
            None => settings_proxy_unset().to_string(),
        };
        let input_empty = self.settings_proxy_input.read(cx).text().trim().is_empty();
        let save_enabled = writes && !input_empty;
        let clear_enabled = writes && state.proxy_url.is_some();
        let refresh_focused =
            self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window);
        let width = super::settings::settings_content_ax_width(frame);
        let mut page = AxNode::new("settings-page", AxRole::Group, t("settings.network.title"), frame)
            .child(
                AxNode::new(
                    "settings-page-title",
                    AxRole::StaticText,
                    t("settings.network.title"),
                    AxRect::new(
                        frame.x + 16.0,
                        frame.y + 16.0,
                        (width - 136.0).max(0.0),
                        HEADING_HEIGHT + SUBTITLE_HEIGHT,
                    ),
                )
                .value(t("settings.network.subtitle")),
            )
            .child(
                AxNode::new(
                    "settings-refresh",
                    AxRole::Button,
                    t("settings.refresh"),
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
        for (kind, label) in general_status_lines(state) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    t("settings.network.ax_status"),
                    AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
                )
                .value(label),
            );
            y += STATUS_HEIGHT + 8.0;
        }
        page = page.child(AxNode::new(
            "settings-proxy-heading",
            AxRole::StaticText,
            t("settings.network.proxy_title"),
            AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
        ));
        y += STATUS_HEIGHT + 8.0;
        page = page.child(
            AxNode::new(
                "settings-proxy-current",
                AxRole::StaticText,
                t("settings.network.ax_current_proxy"),
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
            )
            .value(current),
        );
        y += STATUS_HEIGHT + 8.0;
        let input_value = self.settings_proxy_input.read(cx).text().to_string();
        let input_focused = self.open_menu.is_none()
            && self
                .settings_proxy_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        page = page.child(
            AxNode::new(
                "settings-proxy-input",
                AxRole::TextArea,
                t("settings.network.ax_proxy_input"),
                AxRect::new(frame.x + 16.0, y, (width - 180.0).max(120.0), CONTROL_ROW),
            )
            .value(input_value)
            .enabled(writes)
            .focused(input_focused)
            .action(AxAction::Focus)
            .action(AxAction::SetValue),
        );
        page = page.child(
            AxNode::new(
                "settings-proxy-save",
                AxRole::Button,
                t("settings.save"),
                AxRect::new(frame.x + 16.0 + width - 168.0, y, 80.0, CONTROL_ROW),
            )
            .enabled(save_enabled)
            .focused(self.open_menu.is_none() && self.settings_proxy_save_focus.is_focused(window))
            .action(AxAction::Press),
        );
        page = page.child(
            AxNode::new(
                "settings-proxy-clear",
                AxRole::Button,
                t("settings.clear"),
                AxRect::new(frame.x + 16.0 + width - 80.0, y, 80.0, CONTROL_ROW),
            )
            .enabled(clear_enabled)
            .focused(self.open_menu.is_none() && self.settings_proxy_clear_focus.is_focused(window))
            .action(AxAction::Press),
        );
        y += CONTROL_ROW + 8.0;
        page = page.child(
            AxNode::new(
                "settings-proxy-effect",
                AxRole::StaticText,
                t("settings.network.ax_effect"),
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(settings_proxy_effect_note()),
        );
        y += STATUS_HEIGHT * 2.0 + 8.0;
        page.child(
            AxNode::new(
                "settings-proxy-storage",
                AxRole::StaticText,
                t("settings.network.ax_storage"),
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(settings_proxy_storage_note()),
        )
    }
}
