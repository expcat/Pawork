//! Settings appearance 页。

use super::*;

impl AppView {
    /// 「外观」页（SET-6e）：不经 Host，直接复用 Desktop 已有的
    /// 100% / 125% / 150% `TextScale`。三个按钮始终可达，当前档以
    /// 文字 + 视觉 + AX selected 同时标记；断线不禁用本地能力。
    pub(super) fn settings_appearance_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.text_scale;
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
                format!("Current text size: {}%", scale.percent())
            } else {
                format!("Set text size to {}%", scale.percent())
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

        let content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("Appearance")
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new("Desktop presentation preferences")
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            )
            .child(
                Label::new("Theme · Dark")
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
                    .child(SETTINGS_APPEARANCE_THEME_NOTE),
            )
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("Text size")
                        .size(font::BODY)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(format!("Current · {}%", current.percent()))
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
                        Label::new("The quick brown fox jumps over the lazy dog.")
                            .size(font::BODY)
                            .color(dark().text.primary),
                    )
                    .child(
                        Label::new("Code, tools, and review stay readable at this size.")
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
                    .child(SETTINGS_APPEARANCE_EFFECT_NOTE),
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
}
