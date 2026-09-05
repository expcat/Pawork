//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::Window;

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::settings::{
    settings_text_scale_identifier, SETTINGS_APPEARANCE_CONTROL_GAP,
    SETTINGS_APPEARANCE_CONTROL_HEIGHT, SETTINGS_APPEARANCE_CONTROL_WIDTH,
    SETTINGS_TEXT_SCALES,
};
use crate::ui::i18n::{t, LANGUAGES};
use crate::ui::AppView;

impl AppView {
    /// 「外观」页 AX（SET-6e）：三档字号按钮与 render 共用冻结
    /// identifier / 当前选中态，不受 Host 连接状态影响。
    /// 语言切换（i18n）同口径发布：文案与 render 同源。
    pub(crate) fn settings_appearance_page_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const STATUS_HEIGHT: f32 = 20.0;
        let width = super::settings::settings_content_ax_width(frame);
        let mut y = frame.y + 16.0 + HEADING_HEIGHT + SUBTITLE_HEIGHT + 8.0;
        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.appearance.title"),
            frame,
        )
            .child(
                AxNode::new(
                    "settings-page-title",
                    AxRole::StaticText,
                    t("settings.appearance.title"),
                    AxRect::new(
                        frame.x + 16.0,
                        frame.y + 16.0,
                        width,
                        HEADING_HEIGHT + SUBTITLE_HEIGHT,
                    ),
                )
                .value(t("settings.appearance.subtitle")),
            )
            .child(
                AxNode::new(
                    "settings-appearance-theme",
                    AxRole::StaticText,
                    t("settings.appearance.theme"),
                    AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 3.0),
                )
                .value(t("settings.appearance.theme_note")),
            );
        y += STATUS_HEIGHT * 3.0 + 8.0;
        page = page.child(
            AxNode::new(
                "settings-appearance-text-size",
                AxRole::StaticText,
                t("settings.appearance.text_size"),
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(
                t("settings.appearance.current_scale")
                    .replace("{}", &self.text_scale.percent().to_string()),
            ),
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
                    t("settings.appearance.scale_button")
                        .replace("{}", &scale.percent().to_string()),
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
                .value(if selected {
                    t("settings.appearance.state_current")
                } else {
                    t("settings.appearance.state_available")
                })
                .selected(selected)
                .focused(focused)
                .action(AxAction::Press),
            );
        }
        y += SETTINGS_APPEARANCE_CONTROL_HEIGHT + 8.0;
        page = page.child(
            AxNode::new(
                "settings-appearance-sample",
                AxRole::StaticText,
                t("settings.appearance.sample_title"),
                AxRect::new(frame.x + 16.0, y, width, 56.0),
            )
            .value(format!(
                "{} {}",
                t("settings.appearance.sample_body"),
                t("settings.appearance.sample_sub")
            )),
        );
        y += 56.0 + 8.0;
        page = page.child(
            AxNode::new(
                "settings-appearance-effect",
                AxRole::StaticText,
                t("settings.appearance.scope_title"),
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 3.0),
            )
            .value(t("settings.appearance.effect_note")),
        );
        y += STATUS_HEIGHT * 3.0 + 8.0;
        page = page.child(
            AxNode::new(
                "settings-appearance-language",
                AxRole::StaticText,
                t("settings.appearance.language"),
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(
                t("settings.appearance.language.current")
                    .replace("{}", self.language.display_name()),
            ),
        );
        y += STATUS_HEIGHT * 2.0 + 8.0;
        for (index, language) in LANGUAGES.into_iter().enumerate() {
            let id = language.identifier();
            let selected = self.language == language;
            let focused = self
                .settings_appearance_focus
                .get(id)
                .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
            page = page.child(
                AxNode::new(
                    id,
                    AxRole::Button,
                    language.display_name(),
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
                .value(if selected {
                    t("settings.appearance.state_current")
                } else {
                    t("settings.appearance.state_available")
                })
                .selected(selected)
                .focused(focused)
                .action(AxAction::Press),
            );
        }
        y += SETTINGS_APPEARANCE_CONTROL_HEIGHT + 8.0;
        page.child(
            AxNode::new(
                "settings-appearance-language-hint",
                AxRole::StaticText,
                t("settings.appearance.scope_title"),
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(
                self.appearance_error
                    .as_deref()
                    .unwrap_or(t("settings.appearance.language.hint")),
            ),
        )
    }
}
