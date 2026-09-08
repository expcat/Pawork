//! Settings Network 页（wire 仍沿用兼容的 general_settings）。

use super::*;

impl AppView {
    pub(super) fn settings_general_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let writes = self.settings_general_writes_enabled();
        let state = &self.projection.settings_general;
        let status_lines = general_status_lines(state);
        let current = match &state.proxy_url {
            Some(url) => url.clone(),
            None => settings_proxy_unset().to_string(),
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
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BASE)
            .label(t("settings.refresh"))
            .tooltip(t("settings.network.refresh_tooltip"))
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
            .variant(ButtonVariant::Primary)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BASE)
            .label(t("settings.save"))
            .tooltip(t("settings.network.save_tooltip"))
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
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BASE)
            .label(t("settings.clear"))
            .tooltip(t("settings.network.clear_tooltip"))
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

        let mut content =
            settings_column().child(
                div()
                    .flex()
                    .items_start()
                    .gap_6()
                    .child(self.settings_heading(
                        t("settings.network.title"),
                        t("settings.network.subtitle"),
                    ))
                    .child(
                        self.settings_element("settings-refresh")
                            .flex_none()
                            .child(refresh),
                    ),
            );
        for (kind, line) in status_lines {
            content = content.child(self.settings_status(kind, line));
        }
        let proxy = settings_section()
            .child(
                self.settings_element("settings-proxy-heading")
                    .child(settings_label(t("settings.network.proxy_title"))),
            )
            .child(self.settings_note(
                "settings-proxy-current",
                t("settings.current").replace("{}", &current),
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        self.settings_element("settings-proxy-input")
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .when(!writes, |el| el.opacity(0.55))
                            .child(proxy_input),
                    )
                    .child(
                        self.settings_element("settings-proxy-save")
                            .flex_none()
                            .child(save),
                    )
                    .child(
                        self.settings_element("settings-proxy-clear")
                            .flex_none()
                            .child(clear),
                    ),
            );
        content.child(proxy).child(
            settings_section()
                .child(self.settings_note("settings-proxy-effect", settings_proxy_effect_note()))
                .child(self.settings_note("settings-proxy-storage", settings_proxy_storage_note())),
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
