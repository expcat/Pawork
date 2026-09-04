//! Settings general 页。

use super::*;

impl AppView {
    pub(super) fn settings_general_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let writes = self.settings_general_writes_enabled();
        let state = &self.projection.settings_general;
        let status_lines = general_status_lines(state);
        let current = match &state.proxy_url {
            Some(url) => url.clone(),
            None => SETTINGS_PROXY_UNSET.to_string(),
        };
        let input_empty = self.settings_proxy_input.read(cx).text().trim().is_empty();
        let save_enabled = writes && !input_empty;
        let clear_enabled = writes && state.proxy_url.is_some();
        let proxy_input = self.settings_proxy_input.clone();
        let save_focus = self.settings_proxy_save_focus.clone();
        let clear_focus = self.settings_proxy_clear_focus.clone();
        let refresh_focus = self.settings_refresh_focus.clone();
        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Refresh")
            .tooltip("Refresh general settings")
            .disabled(!connected)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-refresh", event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-refresh");
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));
        let save = Button::new("settings-proxy-save")
            .track_focus(&save_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Save")
            .tooltip("Save proxy URL")
            .disabled(!save_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-proxy-save", event) {
                    return;
                }
                view.on_settings_proxy_save(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-proxy-save");
                view.on_settings_proxy_save(cx);
                cx.stop_propagation();
            }));
        let clear = Button::new("settings-proxy-clear")
            .track_focus(&clear_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Clear")
            .tooltip("Clear proxy URL")
            .disabled(!clear_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-proxy-clear", event) {
                    return;
                }
                view.on_settings_proxy_clear(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-proxy-clear");
                view.on_settings_proxy_clear(cx);
                cx.stop_propagation();
            }));

        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div().font_weight(FontWeight::MEDIUM).child(
                                    Label::new("General")
                                        .size(font::TITLE)
                                        .color(dark().text.primary),
                                ),
                            )
                            .child(
                                Label::new("Host outbound HTTP proxy")
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().flex_none().pt_1().child(refresh)),
            );
        for (kind, line) in status_lines {
            let color = if kind == "error" {
                dark().semantic.danger_text
            } else {
                dark().text.secondary
            };
            content = content.child(status_line(&line, color));
        }
        content = content
            .child(
                Label::new(format!("Current · {current}"))
                    .size(font::BODY)
                    .color(dark().text.primary),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!writes, |el| el.bg(dark().surface.disabled).opacity(0.55))
                            .child(proxy_input),
                    )
                    .child(save)
                    .child(clear),
            )
            .child(
                Label::new(SETTINGS_PROXY_EFFECT_NOTE)
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
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

    /// proxy Save（SET-6a；三路径同源）。空 trim 禁 Save。
    pub(crate) fn on_settings_proxy_save(&mut self, cx: &mut Context<Self>) {
        if !self.settings_general_writes_enabled() {
            return;
        }
        let value = self.settings_proxy_input.read(cx).text().trim().to_string();
        if value.is_empty() {
            return;
        }
        self.controller.set_proxy_url(Some(value));
        cx.notify();
    }

    /// proxy Clear（SET-6a；三路径同源）。已是 null 时禁用。
    pub(crate) fn on_settings_proxy_clear(&mut self, cx: &mut Context<Self>) {
        if !self.settings_general_writes_enabled()
            || self.projection.settings_general.proxy_url.is_none()
        {
            return;
        }
        self.controller.set_proxy_url(None);
        cx.notify();
    }
}
