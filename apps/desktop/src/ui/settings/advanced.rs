//! Settings advanced 页。

use super::*;

impl AppView {
    /// 「高级」页（SET-6f）：仅呈现 Desktop 已有连接事实，并在断线态
    /// 复用壳层现有 Reconnect。无实例编辑、CLI shell-out 或诊断历史。
    pub(super) fn settings_advanced_page_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new(t("settings.advanced.title"))
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(t("settings.advanced.subtitle"))
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );

        for (id, label, value) in self.settings_advanced_diagnostic_rows() {
            content = content.child(
                div()
                    .id(id)
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .py_1()
                    .border_b_1()
                    .border_color(dark().border.subtle)
                    .child(
                        div().w(px(184.0)).flex_none().child(
                            Label::new(label)
                                .size(font::BODY_SM)
                                .color(dark().text.secondary),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_size(font::BODY)
                            .text_color(dark().text.primary)
                            .child(value),
                    ),
            );
        }

        if self.projection.show_reconnect() {
            let reconnect_focus = self.reconnect_focus.clone();
            content = content.child(
                div().pt_1().child(
                    Button::new("reconnect")
                        .track_focus(&reconnect_focus)
                        .variant(ButtonVariant::Primary)
                        .height(px(SETTINGS_ACTION_HEIGHT))
                        .padding(ButtonPadding::Wide)
                        .center()
                        .radius(4.0)
                        .text_size(font::BODY_SM)
                        .label(t("settings.advanced.reconnect"))
                        .on_click(cx.listener(|view, event, window, cx| {
                            if view.consume_button_key_click("reconnect", event) {
                                return;
                            }
                            view.on_reconnect(window, cx);
                        }))
                        .on_activate(cx.listener(|view, _event, window, cx| {
                            view.note_button_key_activate("reconnect");
                            view.on_reconnect(window, cx);
                            cx.stop_propagation();
                        })),
                ),
            );
        }

        content = content
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(settings_advanced_target_note()),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(settings_advanced_doctor_note()),
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

    /// 「高级」页诊断行（SET-6f）：render / AX 共用；未连接时协商字段
    /// 诚实返回 unavailable，endpoint 与最后 ack 游标仍来自 Desktop 本地事实。
    pub(crate) fn settings_advanced_diagnostic_rows(
        &self,
    ) -> Vec<(&'static str, &'static str, String)> {
        // Connection 只报告相位，不复用 TaskRail「Local · Connected · resume」
        // 合成文案，避免把 resume / runtime id 混进这一行。
        let connection = match &self.projection.connection {
            ConnectionState::Connected { .. } => t("settings.advanced.connected").into(),
            other => other.label(),
        };
        let unavailable = t("settings.advanced.unavailable_connect");
        let (runtime_id, api_version, capabilities, resume) = match &self.handshake_info {
            Some(handshake) => (
                handshake.runtime_id.clone(),
                handshake.api_version.clone(),
                if handshake.capabilities.is_empty() {
                    t("settings.advanced.none_granted").to_string()
                } else {
                    handshake.capabilities.join(", ")
                },
                self.projection
                    .resume
                    .label()
                    .unwrap_or_else(|| t("settings.advanced.fresh_snapshot").into()),
            ),
            None => (
                unavailable.into(),
                unavailable.into(),
                unavailable.into(),
                unavailable.into(),
            ),
        };
        let last_ack = self
            .controller
            .last_acked_sequence()
            .map_or_else(|| t("settings.advanced.unavailable").into(), |sequence| sequence.to_string());
        vec![
            ("settings-advanced-connection", t("settings.advanced.row_connection"), connection),
            ("settings-advanced-runtime", t("settings.advanced.row_runtime"), runtime_id),
            ("settings-advanced-api", t("settings.advanced.row_api"), api_version),
            (
                "settings-advanced-capabilities",
                t("settings.advanced.row_capabilities"),
                capabilities,
            ),
            (
                "settings-advanced-endpoint",
                t("settings.advanced.row_endpoint"),
                self.socket.display().to_string(),
            ),
            ("settings-advanced-resume", t("settings.advanced.row_resume"), resume),
            (
                "settings-advanced-last-ack",
                t("settings.advanced.row_last_ack"),
                last_ack,
            ),
        ]
    }
}
