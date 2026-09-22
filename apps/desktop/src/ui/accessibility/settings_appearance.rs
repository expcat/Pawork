//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::Window;

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::i18n::{t, LANGUAGES};
use crate::ui::settings::{settings_text_scale_identifier, SETTINGS_TEXT_SCALES};
use crate::ui::AppView;

impl AppView {
    /// 「外观」页 AX（SET-6e）：三档字号按钮与 render 共用冻结
    /// identifier / 当前选中态，不受 Host 连接状态影响。
    /// 语言切换（i18n）同口径发布：文案与 render 同源。
    pub(crate) fn settings_appearance_page_ax(&self, window: &Window, frame: AxRect) -> AxNode {
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
                self.settings_element_bounds("settings-page-title"),
            )
            .value(t("settings.appearance.subtitle")),
        )
        .child(
            AxNode::new(
                "settings-appearance-theme",
                AxRole::StaticText,
                t("settings.appearance.theme"),
                self.settings_element_bounds("settings-appearance-theme"),
            )
            .value(t("settings.appearance.theme_note")),
        );

        page = page.child(
            AxNode::new(
                "settings-appearance-text-size",
                AxRole::StaticText,
                t("settings.appearance.text_size"),
                self.settings_element_bounds("settings-appearance-text-size"),
            )
            .value(
                t("settings.appearance.current_scale")
                    .replace("{}", &self.text_scale.percent().to_string()),
            ),
        );

        for (index, scale) in SETTINGS_TEXT_SCALES.into_iter().enumerate() {
            let Some(bounds) = self.settings_scale_layout.bounds_for_item(index) else {
                continue;
            };
            let bounds = bounds.intersect(&self.settings_scroll.bounds());
            if bounds.size.width <= gpui::px(0.0) || bounds.size.height <= gpui::px(0.0) {
                continue;
            }
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
                        bounds.origin.x.into(),
                        bounds.origin.y.into(),
                        bounds.size.width.into(),
                        bounds.size.height.into(),
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

        page = page.child(
            AxNode::new(
                "settings-appearance-sample",
                AxRole::StaticText,
                t("settings.appearance.sample_title"),
                self.settings_element_bounds("settings-appearance-sample"),
            )
            .value(format!(
                "{} {}",
                t("settings.appearance.sample_body"),
                t("settings.appearance.sample_sub")
            )),
        );

        page = page.child(
            AxNode::new(
                "settings-appearance-effect",
                AxRole::StaticText,
                t("settings.appearance.scope_title"),
                self.settings_element_bounds("settings-appearance-effect"),
            )
            .value(t("settings.appearance.effect_note")),
        );

        page = page.child(
            AxNode::new(
                "settings-appearance-language",
                AxRole::StaticText,
                t("settings.appearance.language"),
                self.settings_element_bounds("settings-appearance-language"),
            )
            .value(
                t("settings.appearance.language.current")
                    .replace("{}", self.language.display_name()),
            ),
        );

        for (index, language) in LANGUAGES.into_iter().enumerate() {
            let Some(bounds) = self.settings_language_layout.bounds_for_item(index) else {
                continue;
            };
            let bounds = bounds.intersect(&self.settings_scroll.bounds());
            if bounds.size.width <= gpui::px(0.0) || bounds.size.height <= gpui::px(0.0) {
                continue;
            }
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
                        bounds.origin.x.into(),
                        bounds.origin.y.into(),
                        bounds.size.width.into(),
                        bounds.size.height.into(),
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

        page.child(
            AxNode::new(
                "settings-appearance-language-hint",
                AxRole::StaticText,
                t("settings.appearance.scope_title"),
                self.settings_element_bounds("settings-appearance-language-hint"),
            )
            .value(
                self.appearance_error
                    .as_deref()
                    .unwrap_or(t("settings.appearance.language.hint")),
            ),
        )
    }
}
