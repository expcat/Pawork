//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::Window;

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::i18n::t;
use crate::ui::settings::{settings_advanced_doctor_note, settings_advanced_target_note};
use crate::ui::AppView;

impl AppView {
    /// 「高级」页 AX（SET-6f）：与 render 共用诊断行和安全边界；Reconnect
    /// 继续复用全局 identifier、焦点与当前连接态 gate。
    pub(crate) fn settings_advanced_page_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.advanced.title"),
            frame,
        )
        .child(
            AxNode::new(
                "settings-page-title",
                AxRole::StaticText,
                t("settings.advanced.title"),
                self.settings_element_bounds("settings-page-title"),
            )
            .value(t("settings.advanced.subtitle")),
        );
        for (id, label, value) in self.settings_advanced_diagnostic_rows() {
            page = page.child(
                AxNode::new(
                    id,
                    AxRole::StaticText,
                    label,
                    self.settings_element_bounds(id),
                )
                .value(value),
            );
        }
        if self.projection.show_reconnect() {
            page = page.child(
                AxNode::new(
                    "reconnect",
                    AxRole::Button,
                    t("settings.advanced.reconnect"),
                    self.settings_element_bounds("reconnect"),
                )
                .focused(self.open_menu.is_none() && self.reconnect_focus.is_focused(window))
                .action(AxAction::Press),
            );
        }
        page = page.child(
            AxNode::new(
                "settings-advanced-target-note",
                AxRole::StaticText,
                t("settings.advanced.ax_target_title"),
                self.settings_element_bounds("settings-advanced-target-note"),
            )
            .value(settings_advanced_target_note()),
        );

        page.child(
            AxNode::new(
                "settings-advanced-doctor-note",
                AxRole::StaticText,
                t("settings.advanced.ax_doctor_title"),
                self.settings_element_bounds("settings-advanced-doctor-note"),
            )
            .value(settings_advanced_doctor_note()),
        )
    }
}
