//! 审批卡（ApprovalCard）：pending approval 的警示卡与 Allow once /
//! Allow for run / Deny 操作（R8 波 C 自 ui/mod.rs 逐样式迁移）。

use gpui::{div, prelude::*, px, Context, SharedString};

use crate::ui::components::button::{Button, ButtonVariant};
use crate::ui::theme::{dark, font};
use crate::ui::timeline_entry::{default_text_line_height, estimated_wrapped_lines};

use super::AppView;

/// 审批卡几何（render 与 AX 同源，P4 片 3）：p_2 内边距 + 标题 / reason
/// （SM 默认行高）+ 可选 detail（XS 默认行高）+ 32px 按钮行；文本换行按
/// 公式化估算（gpui 实际按像素 wrap）。
pub(crate) const APPROVAL_CARD_PAD_REMS: f32 = 0.5;
pub(crate) const APPROVAL_BUTTON_ROW_GAP_REMS: f32 = 0.5;
pub(crate) const APPROVAL_BUTTON_HEIGHT: f32 = 32.0;
/// AX 按钮槽宽（顺序 once / for-run / deny）：按钮实际宽随文案与字号档
/// 缩放，此处为冻结估计槽，保证顺序排布与 gap_2 间距可公式化。
pub(crate) const APPROVAL_BUTTON_SLOT_WIDTHS: [f32; 3] = [104.0, 116.0, 72.0];

/// 审批卡内容高度公式（AX 卡 rect 与 render 布局同源；行数按估算）。
pub(crate) fn approval_card_height(
    reason: &str,
    detail: Option<&str>,
    card_width: f32,
    rem_px: f32,
) -> f32 {
    let pad = APPROVAL_CARD_PAD_REMS * rem_px;
    let sm_px = font::SM.0 * rem_px;
    let xs_px = font::XS.0 * rem_px;
    let inner_width = (card_width - pad * 2.0).max(0.0);
    let sm_line = default_text_line_height(sm_px);
    let reason_lines = estimated_wrapped_lines(reason, inner_width, sm_px).max(1);
    let detail_lines = detail
        .filter(|detail| !detail.is_empty())
        .map(|detail| estimated_wrapped_lines(detail, inner_width, xs_px).max(1))
        .unwrap_or(0);
    pad * 2.0
        + sm_line
        + sm_line * reason_lines as f32
        + default_text_line_height(xs_px) * detail_lines as f32
        + APPROVAL_BUTTON_HEIGHT
}

impl AppView {
    /// 审批卡作为 timeline list 的末项渲染；仅在 pending 存在时挂载。
    /// 按钮 focus handle 为 app 级状态，条目虚拟化卸载不丢失。
    pub(super) fn approval_card_element(&self, cx: &mut Context<Self>) -> gpui::Div {
        let pending = self
            .projection
            .pending_approval
            .as_ref()
            .expect("approval card renders only while pending approval exists");
        let can_approve = self.can_approve();
        let mut card = div()
            .p_2()
            .rounded_md()
            .border_l_1()
            .border_color(dark().semantic.warning_border)
            .bg(dark().semantic.warning_bg)
            .child(
                div()
                    .text_size(font::SM)
                    .text_color(dark().semantic.warning_text)
                    .child(format!("Approval · {}", pending.tool_name)),
            )
            .child(
                div()
                    .text_size(font::SM)
                    .text_color(dark().text.primary)
                    .child(pending.reason.clone()),
            );
        if let Some(detail) = pending.detail.clone() {
            if !detail.is_empty() {
                card = card.child(
                    div()
                        .text_size(font::XS)
                        .text_color(dark().text.detail)
                        .child(detail),
                );
            }
        }
        let buttons = [
            (
                "approve-once",
                "Allow once",
                "approve_once",
                ButtonVariant::Primary,
            ),
            (
                "approve-for-run",
                "Allow for run",
                "approve_for_run",
                ButtonVariant::Success,
            ),
            ("approve-deny", "Deny", "deny", ButtonVariant::Danger),
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
                let decision = decision.to_string();
                let focus = match id {
                    "approve-once" => approve_once_focus.clone(),
                    "approve-for-run" => approve_for_run_focus.clone(),
                    _ => deny_focus.clone(),
                };
                let tooltip = if can_approve {
                    SharedString::from(match id {
                        "approve-once" => "Allow once (Cmd+1 / Cmd+Return)",
                        "approve-for-run" => "Allow for run (Cmd+2)",
                        _ => "Deny (Cmd+3)",
                    })
                } else {
                    approve_disabled.clone()
                };
                let click_decision = decision.clone();
                let activate_decision = decision;
                let mut button = Button::new(id)
                    .variant(variant)
                    .disabled(!can_approve)
                    // P4 片 3：按钮行 32px 槽位与 AX rect 同源。
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
                            view.on_approve(&click_decision, window, cx);
                        }))
                        .on_activate(cx.listener(move |view, _event, window, cx| {
                            view.note_button_key_activate(id);
                            view.on_approve(&activate_decision, window, cx);
                            cx.stop_propagation();
                        }));
                }
                button
            }));
        card.child(row)
    }

    fn approve_disabled_reason(&self) -> String {
        if self.projection.pending_approval.is_none() {
            "No pending approval.".into()
        } else {
            "Approval needs a live connection.".into()
        }
    }
}
