//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::Window;

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::settings::{
    settings_text_scale_identifier, SETTINGS_APPEARANCE_CONTROL_GAP,
    SETTINGS_APPEARANCE_CONTROL_HEIGHT, SETTINGS_APPEARANCE_CONTROL_WIDTH,
    SETTINGS_APPEARANCE_EFFECT_NOTE, SETTINGS_APPEARANCE_THEME_NOTE, SETTINGS_TEXT_SCALES,
};
use crate::ui::AppView;

impl AppView {
    /// 「外观」页 AX（SET-6e）：三档字号按钮与 render 共用冻结
    /// identifier / 当前选中态，不受 Host 连接状态影响。
    pub(crate) fn settings_appearance_page_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const STATUS_HEIGHT: f32 = 20.0;
        let width = (frame.width - 32.0).max(0.0);
        let mut y = frame.y + 16.0 + HEADING_HEIGHT + SUBTITLE_HEIGHT + 8.0;
        let mut page = AxNode::new("settings-page", AxRole::Group, "外观", frame)
            .child(
                AxNode::new(
                    "settings-page-title",
                    AxRole::StaticText,
                    "外观",
                    AxRect::new(
                        frame.x + 16.0,
                        frame.y + 16.0,
                        width,
                        HEADING_HEIGHT + SUBTITLE_HEIGHT,
                    ),
                )
                .value("Desktop presentation preferences"),
            )
            .child(
                AxNode::new(
                    "settings-appearance-theme",
                    AxRole::StaticText,
                    "主题",
                    AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 3.0),
                )
                .value(format!("深色 · {SETTINGS_APPEARANCE_THEME_NOTE}")),
            );
        y += STATUS_HEIGHT * 3.0 + 8.0;
        page = page.child(
            AxNode::new(
                "settings-appearance-text-size",
                AxRole::StaticText,
                "字号",
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(format!("当前 · {}%", self.text_scale.percent())),
        );
        y += STATUS_HEIGHT * 2.0 + 8.0;
        for (index, scale) in SETTINGS_TEXT_SCALES.into_iter().enumerate() {
            let id = settings_text_scale_identifier(scale);
            let selected = self.text_scale == scale;
            let focused = self
                .settings_appearance_focus
                .get(id)
                .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
            page = page.child(
                AxNode::new(
                    id,
                    AxRole::Button,
                    format!("字号 {}%", scale.percent()),
                    AxRect::new(
                        frame.x
                            + 16.0
                            + index as f32
                                * (SETTINGS_APPEARANCE_CONTROL_WIDTH
                                    + SETTINGS_APPEARANCE_CONTROL_GAP),
                        y,
                        SETTINGS_APPEARANCE_CONTROL_WIDTH,
                        SETTINGS_APPEARANCE_CONTROL_HEIGHT,
                    ),
                )
                .value(if selected { "当前" } else { "可选" })
                .selected(selected)
                .focused(focused)
                .action(AxAction::Press),
            );
        }
        y += SETTINGS_APPEARANCE_CONTROL_HEIGHT + 8.0;
        page.child(
            AxNode::new(
                "settings-appearance-effect",
                AxRole::StaticText,
                "生效范围",
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 3.0),
            )
            .value(SETTINGS_APPEARANCE_EFFECT_NOTE),
        )
    }
}
