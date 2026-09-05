//! Settings about 页。

use super::*;

impl AppView {
    /// 「关于」页（SET-6g / ADR-051）：仅呈现当前连接的三项权威事实，
    /// 不提供 updater、release、License 或任何写动作。
    pub(super) fn settings_about_page_element(&mut self) -> impl IntoElement {
        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new(t("settings.about.title"))
                        .size(font::TITLE)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(t("settings.about.subtitle"))
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );

        for (id, label, value) in self.settings_about_rows().unwrap_or_default() {
            content = content.child(
                div()
                    .id(id)
                    .w_full()
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

    /// 「关于」页只读行（SET-6g / ADR-051）：三个值分别来自 Desktop
    /// 构建元数据与当前已认证握手。Host 路径缺失或为空时整页不可用，
    /// render / AX 共用该 fail-closed gate，绝不从 endpoint 推断。
    pub(crate) fn settings_about_rows(&self) -> Option<Vec<(&'static str, &'static str, String)>> {
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            return None;
        }
        let handshake = self.handshake_info.as_ref()?;
        let host_data_dir = handshake.host_data_dir.as_deref()?;
        if host_data_dir.trim().is_empty() {
            return None;
        }
        Some(vec![
            (
                "settings-about-desktop-build",
                t("settings.about.row_desktop_build"),
                env!("CARGO_PKG_VERSION").to_string(),
            ),
            (
                "settings-about-api",
                t("settings.about.row_api"),
                handshake.api_version.clone(),
            ),
            (
                "settings-about-data-dir",
                t("settings.about.row_data_dir"),
                host_data_dir.to_string(),
            ),
        ])
    }
}
