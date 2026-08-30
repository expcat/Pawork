//! 审批卡（ApprovalCard）：pending approval 的警示卡与 Allow once /
//! Allow for run / Deny 操作（R8 波 C 自 ui/mod.rs 逐样式迁移）。

use gpui::{div, prelude::*, Context, SharedString};

use crate::ui::components::button::{Button, ButtonVariant};
use crate::ui::theme::{dark, font};

use super::AppView;

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
