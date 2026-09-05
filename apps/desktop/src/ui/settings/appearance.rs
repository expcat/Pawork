//! Settings appearance 页。

use super::*;
use crate::ui::i18n::{t, Language, LANGUAGES};

impl AppView {
    /// 「外观」页（SET-6e）：不经 Host，直接复用 Desktop 已有的
    /// 100% / 125% / 150% `TextScale`。三个按钮始终可达，当前档以
    /// 文字 + 视觉 + AX selected 同时标记；断线不禁用本地能力。
    /// 语言切换（i18n）与字号同口径：本地、即时、不持久化。
    pub(super) fn settings_appearance_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.text_scale;
        let current_language = self.language;
        let mut scale_controls = div()
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
                .radius(4.0)
                .bordered()
                .text_size(font::BODY_SM)
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
                t("settings.appearance.language.tooltip_set")
                    .replace("{}", language.display_name())
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
                .radius(4.0)
                .bordered()
                .text_size(font::BODY_SM)
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

        let content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new(t("settings.appearance.title"))
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(t("settings.appearance.subtitle"))
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            )
            .child(
                Label::new(t("settings.appearance.theme"))
                    .size(font::BODY)
                    .color(dark().text.primary),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(t("settings.appearance.theme_note")),
            )
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new(t("settings.appearance.text_size"))
                        .size(font::BODY)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(
                    t("settings.appearance.current_scale")
                        .replace("{}", &current.percent().to_string()),
                )
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            )
            .child(scale_controls)
            .child(
                div()
                    .id("settings-appearance-sample")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(dark().border.subtle)
                    .bg(dark().surface.raised)
                    .child(
                        Label::new(t("settings.appearance.sample_body"))
                            .size(font::BODY)
                            .color(dark().text.primary),
                    )
                    .child(
                        Label::new(t("settings.appearance.sample_sub"))
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(t("settings.appearance.effect_note")),
            )
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new(t("settings.appearance.language"))
                        .size(font::BODY)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(
                    t("settings.appearance.language.current")
                        .replace("{}", current_language.display_name()),
                )
                .size(font::BODY_SM)
                .color(dark().text.secondary),
            )
            .child(language_controls)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(
                        self.appearance_error
                            .as_deref()
                            .unwrap_or(t("settings.appearance.language.hint")).to_owned(),
                    ),
            );

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
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
