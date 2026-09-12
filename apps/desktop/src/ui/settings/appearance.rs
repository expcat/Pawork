//! Settings appearance 页。

use super::*;
use crate::ui::i18n::{t, Language, LANGUAGES};

impl AppView {
    /// 「外观」页（SET-6e）：不经 Host，直接复用 Desktop 已有的
    /// 100% / 125% / 150% `TextScale`。三个按钮始终可达，当前档以
    /// 文字 + 视觉 + AX selected 同时标记；断线不禁用本地能力。
    /// 语言切换（i18n）与字号同口径：本地、即时、保存后重启恢复。
    pub(super) fn settings_appearance_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.text_scale;
        let current_language = self.language;
        let mut scale_controls = div()
            .id("settings-scale-controls")
            .track_scroll(&self.settings_scale_layout)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SETTINGS_APPEARANCE_CONTROL_GAP))
            .min_w_0();
        for scale in SETTINGS_TEXT_SCALES {
            let id = settings_text_scale_identifier(scale);
            let selected = scale == current;
            let focus = self
                .settings_appearance_focus
                .entry(id.to_string())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let tooltip = if selected {
                t("settings.appearance.tooltip_scale_current")
                    .replace("{}", &scale.percent().to_string())
            } else {
                t("settings.appearance.tooltip_scale_set")
                    .replace("{}", &scale.percent().to_string())
            };
            let button = Button::new(id)
                .track_focus(&focus)
                .variant(if selected {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Raised
                })
                .height(px(SETTINGS_APPEARANCE_CONTROL_HEIGHT))
                .width(px(SETTINGS_APPEARANCE_CONTROL_WIDTH))
                .padding(ButtonPadding::Wide)
                .center()
                .radius(6.0)
                .bordered()
                .text_size(font::BASE)
                .label(format!("{}%", scale.percent()))
                .tooltip(tooltip)
                .on_click(cx.listener(move |view, event, window, cx| {
                    if view.consume_button_key_click(id, event) {
                        return;
                    }
                    view.on_settings_text_scale(scale, window, cx);
                }))
                .on_activate(cx.listener(move |view, _event, window, cx| {
                    view.note_button_key_activate(id);
                    view.on_settings_text_scale(scale, window, cx);
                    cx.stop_propagation();
                }));
            scale_controls = scale_controls.child(button);
        }

        let mut language_controls = div()
            .id("settings-language-controls")
            .track_scroll(&self.settings_language_layout)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SETTINGS_APPEARANCE_CONTROL_GAP))
            .min_w_0();
        for language in LANGUAGES {
            let id = language.identifier();
            let selected = language == current_language;
            let focus = self
                .settings_appearance_focus
                .entry(id.to_string())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let tooltip = if selected {
                t("settings.appearance.language.tooltip_current")
                    .replace("{}", language.display_name())
            } else {
                t("settings.appearance.language.tooltip_set").replace("{}", language.display_name())
            };
            let button = Button::new(id)
                .track_focus(&focus)
                .variant(if selected {
                    ButtonVariant::Primary
                } else {
                    ButtonVariant::Raised
                })
                .height(px(SETTINGS_APPEARANCE_CONTROL_HEIGHT))
                .width(px(SETTINGS_APPEARANCE_CONTROL_WIDTH))
                .padding(ButtonPadding::Wide)
                .center()
                .radius(6.0)
                .bordered()
                .text_size(font::BASE)
                .label(language.display_name())
                .tooltip(tooltip)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(id, event) {
                        return;
                    }
                    view.on_settings_language(language, cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(id);
                    view.on_settings_language(language, cx);
                    cx.stop_propagation();
                }));
            language_controls = language_controls.child(button);
        }

        settings_column()
            .child(self.settings_heading(
                t("settings.appearance.title"),
                t("settings.appearance.subtitle"),
            ))
            .child(
                settings_section().child(
                    self.settings_element("settings-appearance-theme")
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(
                            div()
                                .w(px(48.0))
                                .h(px(36.0))
                                .flex_none()
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(dark().border.strong)
                                .bg(dark().bg.panel),
                        )
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(settings_label(t("settings.appearance.theme")))
                                .child(settings_copy(t("settings.appearance.theme_note"))),
                        ),
                ),
            )
            .child(
                settings_section()
                    .child(
                        self.settings_element("settings-appearance-text-size")
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(settings_label(t("settings.appearance.text_size")))
                            .child(settings_copy(
                                t("settings.appearance.current_scale")
                                    .replace("{}", &current.percent().to_string()),
                            )),
                    )
                    .child(scale_controls)
                    .child(
                        self.settings_element("settings-appearance-sample")
                            .flex()
                            .flex_col()
                            .gap_3()
                            .p_4()
                            .rounded(px(8.0))
                            .bg(dark().surface.raised)
                            .child(
                                div()
                                    .text_size(font::BODY)
                                    .text_color(dark().text.primary)
                                    .child(t("settings.appearance.sample_body")),
                            )
                            .child(settings_copy(t("settings.appearance.sample_sub"))),
                    )
                    .child(self.settings_note(
                        "settings-appearance-effect",
                        t("settings.appearance.effect_note"),
                    )),
            )
            .child(
                settings_section()
                    .child(
                        self.settings_element("settings-appearance-language")
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(settings_label(t("settings.appearance.language")))
                            .child(settings_copy(
                                t("settings.appearance.language.current")
                                    .replace("{}", current_language.display_name()),
                            )),
                    )
                    .child(language_controls)
                    .child(
                        self.settings_note(
                            "settings-appearance-language-hint",
                            self.appearance_error.clone().unwrap_or_else(|| {
                                t("settings.appearance.language.hint").to_owned()
                            }),
                        ),
                    ),
            )
    }

    /// 外观页字号选择入口（SET-6e）：只在当前 Settings / 外观页生效。
    pub(crate) fn on_settings_text_scale(
        &mut self,
        scale: font::TextScale,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.route != AppRoute::Settings || self.settings_page != SettingsPage::Appearance {
            return;
        }
        self.set_text_scale(scale, window, cx);
    }

    /// 外观页语言选择入口（i18n）：只在当前 Settings / 外观页生效。
    pub(crate) fn on_settings_language(&mut self, language: Language, cx: &mut Context<Self>) {
        if self.route != AppRoute::Settings || self.settings_page != SettingsPage::Appearance {
            return;
        }
        self.set_language(language, cx);
    }
}
