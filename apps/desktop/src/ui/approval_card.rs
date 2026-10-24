//! 审批卡（ApprovalCard）：pending approval 的警示卡与 Allow once /
//! Allow for run / Deny 操作（R8 波 C 自 ui/mod.rs 逐样式迁移）。

use gpui::{div, prelude::*, px, Context, SharedString};

use crate::ui::components::button::{Button, ButtonVariant};
use crate::ui::i18n::t;
use crate::ui::theme::{dark, font, metrics};
use crate::ui::timeline_entry::estimated_wrapped_lines;

use super::AppView;

/// 首帧测高使用与 render 相同的 14/22 文本、间距和按钮高度；稳定帧 AX
/// 读取卡片和按钮实测框，不估算翻译后的按钮宽度。
pub(crate) const APPROVAL_CARD_PAD_REMS: f32 = 0.5;
pub(crate) const APPROVAL_BUTTON_HEIGHT: f32 = metrics::BUTTON_MIN_HEIGHT;

/// 审批卡内容高度公式（AX 卡 rect 与 render 布局同源；行数按估算）。
pub(crate) fn approval_card_height(
    tool_name: &str,
    reason: &str,
    detail: Option<&str>,
    card_width: f32,
    rem_px: f32,
) -> f32 {
    let pad = APPROVAL_CARD_PAD_REMS * rem_px;
    let font_px = font::DESCRIPTION.0 * rem_px;
    let inner_width = (card_width - pad * 2.0 - 1.0).max(0.0);
    let line_height = (font::DESCRIPTION_LINE_HEIGHT.0 * rem_px).round();
    let title = t("approval.title").replace("{}", tool_name);
    let title_lines = estimated_wrapped_lines(&title, inner_width, font_px).max(1);
    let reason_lines = estimated_wrapped_lines(reason, inner_width, font_px).max(1);
    let detail_lines = detail
        .filter(|detail| !detail.is_empty())
        .map(|detail| estimated_wrapped_lines(detail, inner_width, font_px).max(1))
        .unwrap_or(0);
    pad * 2.0
        + line_height * (title_lines + reason_lines + detail_lines) as f32
        + pad * if detail_lines > 0 { 3.0 } else { 2.0 }
        + APPROVAL_BUTTON_HEIGHT
}

impl AppView {
    /// 审批卡作为 timeline list 的末项渲染；仅在 pending 存在时挂载。
    /// 按钮 focus handle 为 app 级状态，条目虚拟化卸载不丢失。
    pub(super) fn approval_card_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let pending = self
            .projection
            .pending_approval
            .as_ref()
            .expect("approval card renders only while pending approval exists")
            .clone();
        let can_approve = self.can_approve();
        let mut card = self
            .settings_element("approval-card")
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_md()
            .border_l_1()
            .border_color(dark().semantic.warning_border)
            .bg(dark().semantic.warning_bg)
            .child(
                div()
                    .text_size(font::DESCRIPTION)
                    .line_height(font::DESCRIPTION_LINE_HEIGHT)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(dark().semantic.warning_text)
                    .child(t("approval.title").replace("{}", &pending.tool_name)),
            )
            .child(
                div()
                    .text_size(font::DESCRIPTION)
                    .line_height(font::DESCRIPTION_LINE_HEIGHT)
                    .text_color(dark().text.primary)
                    .child(pending.reason.clone()),
            );
        if let Some(detail) = pending
            .detail
            .as_deref()
            .filter(|detail| !detail.is_empty())
        {
            card = card.child(
                div()
                    .text_size(font::DESCRIPTION)
                    .line_height(font::DESCRIPTION_LINE_HEIGHT)
                    .font_family(font::MONO)
                    .text_color(dark().text.detail)
                    .child(detail.to_string()),
            );
        }
        let buttons = [
            (
                "approve-once",
                t("approval.allow_once"),
                "approve_once",
                ButtonVariant::Primary,
            ),
            (
                "approve-for-run",
                t("approval.allow_for_run"),
                "approve_for_run",
                ButtonVariant::Success,
            ),
            (
                "approve-deny",
                t("approval.deny"),
                "deny",
                ButtonVariant::Danger,
            ),
        ];
        let approve_once_focus = self.approve_once_focus.clone();
        let approve_for_run_focus = self.approve_for_run_focus.clone();
        let deny_focus = self.deny_focus.clone();
        let approve_disabled = SharedString::from(self.approve_disabled_reason());
        let row = div()
            .flex()
            .flex_row()
            .gap_2()
            .children(buttons.into_iter().map(|(id, label, decision, variant)| {
                let focus = match id {
                    "approve-once" => approve_once_focus.clone(),
                    "approve-for-run" => approve_for_run_focus.clone(),
                    _ => deny_focus.clone(),
                };
                let tooltip = if can_approve {
                    SharedString::from(match id {
                        "approve-once" => t("approval.tooltip_allow_once"),
                        "approve-for-run" => t("approval.tooltip_allow_for_run"),
                        _ => t("approval.tooltip_deny"),
                    })
                } else {
                    approve_disabled.clone()
                };
                let mut button = Button::new(id)
                    .variant(variant)
                    .disabled(!can_approve)
                    .height(px(APPROVAL_BUTTON_HEIGHT))
                    .center()
                    .track_focus(&focus)
                    .label(label)
                    .tooltip(tooltip);
                if can_approve {
                    button = button
                        .on_click(cx.listener(move |view, event, window, cx| {
                            if view.consume_button_key_click(id, event) {
                                return;
                            }
                            view.on_approve(decision, window, cx);
                        }))
                        .on_activate(cx.listener(move |view, _event, window, cx| {
                            view.note_button_key_activate(id);
                            view.on_approve(decision, window, cx);
                            cx.stop_propagation();
                        }));
                }
                self.settings_element(id).flex_none().child(button)
            }));
        card.child(row)
    }

    fn approve_disabled_reason(&self) -> String {
        if self.projection.pending_approval.is_none() {
            t("approval.disabled_none_pending").into()
        } else {
            t("approval.disabled_needs_connection").into()
        }
    }
}
