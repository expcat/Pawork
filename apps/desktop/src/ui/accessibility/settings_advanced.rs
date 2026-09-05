//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::Window;

use super::app::{CONTROL_HEIGHT, PAD};
use super::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::i18n::t;
use crate::ui::settings::{settings_advanced_doctor_note, settings_advanced_target_note};
use crate::ui::AppView;

impl AppView {
    /// 「高级」页 AX（SET-6f）：与 render 共用诊断行和安全边界；Reconnect
    /// 继续复用全局 identifier、焦点与当前连接态 gate。
    pub(crate) fn settings_advanced_page_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const DIAGNOSTIC_ROW_HEIGHT: f32 = 40.0;
        const NOTE_HEIGHT: f32 = 56.0;
        let width = super::settings::settings_content_ax_width(frame);
        let mut y = frame.y + 16.0 + HEADING_HEIGHT + SUBTITLE_HEIGHT + 8.0;
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
                AxRect::new(
                    frame.x + 16.0,
                    frame.y + 16.0,
                    width,
                    HEADING_HEIGHT + SUBTITLE_HEIGHT,
                ),
            )
            .value(t("settings.advanced.subtitle")),
        );
        for (id, label, value) in self.settings_advanced_diagnostic_rows() {
            page = page.child(
                AxNode::new(
                    id,
                    AxRole::StaticText,
                    label,
                    AxRect::new(frame.x + 16.0, y, width, DIAGNOSTIC_ROW_HEIGHT),
                )
                .value(value),
            );
            y += DIAGNOSTIC_ROW_HEIGHT;
        }
        if self.projection.show_reconnect() {
            page = page.child(
                AxNode::new(
                    "reconnect",
                    AxRole::Button,
                    t("settings.advanced.reconnect"),
                    AxRect::new(frame.x + 16.0, y, 112.0, CONTROL_HEIGHT),
                )
                .focused(self.open_menu.is_none() && self.reconnect_focus.is_focused(window))
                .action(AxAction::Press),
            );
            y += CONTROL_HEIGHT + PAD;
        }
        page = page.child(
            AxNode::new(
                "settings-advanced-target-note",
                AxRole::StaticText,
                t("settings.advanced.ax_target_title"),
                AxRect::new(frame.x + 16.0, y, width, NOTE_HEIGHT),
            )
            .value(settings_advanced_target_note()),
        );
        y += NOTE_HEIGHT + PAD;
        page.child(
            AxNode::new(
                "settings-advanced-doctor-note",
                AxRole::StaticText,
                t("settings.advanced.ax_doctor_title"),
                AxRect::new(frame.x + 16.0, y, width, NOTE_HEIGHT),
            )
            .value(settings_advanced_doctor_note()),
        )
    }
}
