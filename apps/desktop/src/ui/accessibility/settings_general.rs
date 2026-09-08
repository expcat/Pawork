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

        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.network.title"),
            frame,
        )
        .child(
            AxNode::new(
                "settings-page-title",
                AxRole::StaticText,
                t("settings.network.title"),
                self.settings_element_bounds("settings-page-title"),
            )
            .value(t("settings.network.subtitle")),
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

        for (kind, label) in general_status_lines(state) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    t("settings.network.ax_status"),
                    self.settings_element_bounds(&format!("settings-status-{kind}")),
                )
                .value(label),
            );
        }
        page = page.child(AxNode::new(
            "settings-proxy-heading",
            AxRole::StaticText,
            t("settings.network.proxy_title"),
            self.settings_element_bounds("settings-proxy-heading"),
        ));

        page = page.child(
            AxNode::new(
                "settings-proxy-current",
                AxRole::StaticText,
                t("settings.network.ax_current_proxy"),
                self.settings_element_bounds("settings-proxy-current"),
            )
            .value(current),
        );

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
                self.settings_element_bounds("settings-proxy-input"),
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
                self.settings_element_bounds("settings-proxy-save"),
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
                self.settings_element_bounds("settings-proxy-clear"),
            )
            .enabled(clear_enabled)
            .focused(self.open_menu.is_none() && self.settings_proxy_clear_focus.is_focused(window))
            .action(AxAction::Press),
        );

        page = page.child(
            AxNode::new(
                "settings-proxy-effect",
                AxRole::StaticText,
                t("settings.network.ax_effect"),
                self.settings_element_bounds("settings-proxy-effect"),
            )
            .value(settings_proxy_effect_note()),
        );

        page.child(
            AxNode::new(
                "settings-proxy-storage",
                AxRole::StaticText,
                t("settings.network.ax_storage"),
                self.settings_element_bounds("settings-proxy-storage"),
            )
            .value(settings_proxy_storage_note()),
        )
    }
}
