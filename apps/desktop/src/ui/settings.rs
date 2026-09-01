//! Settings 壳（SET-3）：Settings Rail 与首个只读页「Models & providers」。
//!
//! 只呈现 Host `provider_auth_status` 权威事实：供应商名称、认证方式、
//! 连接状态与目录来源。断线保留 stale 只读结果并可见标注；本片无写
//! capability，不画任何写操作入口（添加 / 验证 / 刷新 / 设默认均属
//! SET-4/SET-5）。

use gpui::{div, prelude::*, px, Context, FontWeight, Pixels};

use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::label::Label;
use crate::ui::components::panel::Panel;
use crate::ui::theme::{dark, font, metrics};

use super::shell_layout;
use super::AppView;

/// Settings 内容可读列（与 Timeline 618px 可读列同节奏；全宽壳层内收敛）。
const SETTINGS_CONTENT_MAX_WIDTH: f32 = 720.0;
/// Provider 卡片内边距（8px 节奏）。
const PROVIDER_CARD_PAD: f32 = 8.0;

/// Settings 供应商页状态行（render 与 AX 同源）。stale / loading / error /
/// 空态独立判定：stale 与 error 可同时出现，空态仅在完全无状态且列表为
/// 空时给出（SET-3 审查修复 2/3）。
pub(super) fn provider_status_lines(
    state: &crate::projection::SettingsProvidersState,
) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(reason) = &state.stale_reason {
        lines.push((
            "stale",
            format!("Offline · showing last known state ({reason})"),
        ));
    } else if state.loading {
        lines.push(("loading", "Loading…".to_string()));
    }
    if let Some(error) = &state.error {
        lines.push(("error", format!("Could not load provider status · {error}")));
    }
    if state.providers.is_empty()
        && !state.loading
        && state.error.is_none()
        && state.stale_reason.is_none()
    {
        lines.push(("empty", "No providers reported by the host.".to_string()));
    }
    lines
}

impl AppView {
    /// Settings 左栏（SET-3）：返回工作台 + 首个导航项「Models & providers」。
    /// 宽度沿用 TaskRail 的响应式 rail（288 / 240 / 320），进入时整体替换
    /// TaskRail；未接通页面不显示（无假导航项）。
    pub(super) fn settings_rail_element(
        &mut self,
        rail_width: Pixels,
        cx: &mut Context<Self>,
    ) -> Panel {
        let back_focus = self.settings_back_focus.clone();
        let back = Button::new("settings-back")
            .track_focus(&back_focus)
            .variant(ButtonVariant::Raised)
            .padding(ButtonPadding::Horizontal(metrics::RAIL_INNER_PAD))
            .height(px(metrics::RAIL_TOP_ROW_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY)
            .label("← Back to workspace")
            .tooltip("Back to workspace")
            .on_click(cx.listener(|view, event, window, cx| {
                if view.consume_button_key_click("settings-back", event) {
                    return;
                }
                view.on_close_settings(window, cx);
            }))
            .on_activate(cx.listener(|view, _event, window, cx| {
                view.note_button_key_activate("settings-back");
                view.on_close_settings(window, cx);
                cx.stop_propagation();
            }));
        // 首页导航项：当前唯一页面，选中态静态行（不画无动作假按钮；
        // 后续页面按真实 capability 逐页加入）。
        let nav_item = div()
            .id("settings-nav-providers")
            .mt_2()
            .w_full()
            .h(px(metrics::RAIL_TOP_ROW_HEIGHT))
            .flex()
            .items_center()
            .px(px(metrics::RAIL_INNER_PAD))
            .rounded(px(4.0))
            .bg(dark().surface.raised)
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("Models & providers")
                        .size(font::BODY_SM)
                        .color(dark().text.primary),
                ),
            );

        Panel::side_right(rail_width)
            .child(shell_layout::rail_safe_area())
            .child(back)
            .child(nav_item)
    }

    /// Settings 全宽内容区（SET-3 只读供应商页）。状态行全部来自
    /// projection（Host 权威 / stale / error），render 与 AX 同源。
    pub(super) fn settings_page_element(&self) -> impl IntoElement {
        let state = &self.projection.settings_providers;
        let page = div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4();

        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2();
        content = content.child(
            div()
                .flex()
                .flex_col()
                .child(
                    div().font_weight(FontWeight::MEDIUM).child(
                        Label::new("Models & providers")
                            .size(font::TITLE)
                            .color(dark().text.primary),
                    ),
                )
                .child(
                    Label::new("Connection status and catalog source for each provider")
                        .size(font::BODY_SM)
                        .color(dark().text.secondary),
                ),
        );

        // 状态行（不只靠颜色区分）：与 AX 共用 provider_status_lines，
        // stale / loading / error / 空态独立发布。
        for (kind, line) in provider_status_lines(state) {
            let color = if kind == "error" {
                dark().semantic.danger_text
            } else {
                dark().text.secondary
            };
            content = content.child(status_line(&line, color));
        }

        if !state.providers.is_empty() {
            let mut cards = div().flex().flex_col().min_w_0().gap_2();
            for (ix, provider) in state.providers.iter().enumerate() {
                cards = cards.child(provider_card(ix, provider));
            }
            content = content.child(cards);
        }

        page.child(
            div()
                .id("settings-page-scroll")
                .flex_1()
                .min_h_0()
                .track_scroll(&self.settings_scroll)
                .child(content),
        )
    }
}

fn status_line(text: &str, color: gpui::Rgba) -> impl IntoElement {
    div().child(
        Label::new(text.to_string())
            .size(font::BODY_SM)
            .color(color),
    )
}

/// 单个 provider 只读卡片：名称 / endpoint、认证方式、连接状态、目录来源。
/// 无写 capability，不画操作按钮。
fn provider_card(ix: usize, provider: &crate::projection::ProviderStatusEntry) -> impl IntoElement {
    div()
        .id(("settings-provider", ix))
        .flex()
        .flex_col()
        .min_w_0()
        .gap_1()
        .p(px(PROVIDER_CARD_PAD))
        .rounded(px(4.0))
        .border_1()
        .border_color(dark().border.subtle)
        .bg(dark().surface.raised)
        .child(
            div()
                .flex()
                .flex_row()
                .items_baseline()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(
                            Label::new(provider.display_name.clone())
                                .size(font::BODY)
                                .color(dark().text.primary),
                        ),
                )
                .child(
                    div().flex_none().child(
                        Label::new(provider.auth_methods_label())
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    ),
                ),
        )
        .child(
            Label::new(provider.auth_label())
                .size(font::BODY_SM)
                .color(dark().text.secondary),
        )
        .child(
            Label::new(provider.endpoint_label.clone())
                .size(font::BODY_SM)
                .color(dark().text.tertiary),
        )
        .child(
            Label::new(provider.catalog_label())
                .size(font::BODY_SM)
                .color(dark().text.tertiary),
        )
}
