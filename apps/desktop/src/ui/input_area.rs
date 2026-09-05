//! Composer 输入区（InputArea）：两行结构（R5 Wave A）。
//! 行 1 输入区；行 2 footer（model / workspace / ContextMeter / 瞬态 status_hint / 32×32 动作槽）。
//! Send 与 Cancel 同槽互换（element id `composer-action`）；placeholder 只走状态机，
//! Forked / 发送失败等瞬态反馈落 footer Label。

use gpui::{
    div, point, prelude::*, px, Context, Corner, Pixels, Point, SharedString, Window,
};

use crate::projection::{group_models_by_provider, ConnectionState, ModelEntry};
use crate::ui::components::button::{Button, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow, ANCHOR_GAP_Y};
use crate::ui::components::label::Label;
use crate::ui::i18n::{t, t2};
use crate::ui::theme::{dark, font, metrics};

use super::{AppView, MenuKind};

/// model menu provider 分组头高度；render 与 AX 几何共用。
pub(super) const MODEL_MENU_GROUP_HEADER_HEIGHT: f32 = 24.0;

/// Composer model menu 的可点击项顺序。provider 保持目录首现顺序，组内
/// 保持原目录顺序；鼠标、键盘与 AX 均使用这份扁平顺序。
pub(super) fn grouped_model_menu_entries(models: &[ModelEntry]) -> Vec<ModelEntry> {
    group_models_by_provider(models)
        .into_iter()
        .flat_map(|(_, models)| models)
        .collect()
}

impl AppView {
    pub(super) fn composer_element(&self, cx: &mut Context<Self>) -> gpui::Div {
        let can_send = self.can_send(cx);
        let can_cancel = self.can_cancel();
        let can_switch_model = self.can_switch_model();
        let model_menu_open = matches!(self.open_menu, Some(MenuKind::Model)) && can_switch_model;
        let composer_hint = self.composer_placeholder_hint();
        let context_meter = self.projection.context_meter_label();
        let context_available = context_meter != "Context · unavailable";
        self.sync_composer_placeholder(composer_hint.clone(), cx);

        let model_tooltip = if can_switch_model {
            SharedString::from(
                self.projection
                    .effective_model()
                    .map(|(provider, id)| t2("composer.model_tooltip", provider, id))
                    .unwrap_or_else(|| t("composer.model_tooltip_none").to_string()),
            )
        } else {
            SharedString::from(self.model_disabled_reason())
        };
        let model_focus = self.model_focus.clone();
        let mut model_button = Button::new("model-picker")
            .track_focus(&model_focus)
            .variant(ButtonVariant::Raised)
            .disabled(!can_switch_model)
            .label(self.model_label())
            .tooltip(model_tooltip)
            .height(px(metrics::COMPOSER_FOOTER_CONTROL))
            .max_width(px(220.0))
            .vcenter();
        if can_switch_model {
            model_button = model_button
                .on_click(cx.listener(|view, event, window, cx| {
                    if view.consume_button_key_click("model-picker", event) {
                        return;
                    }
                    let down = Self::click_down_position(event);
                    view.on_toggle_model_menu(down, window, cx);
                }))
                .on_activate(cx.listener(|view, _event, window, cx| {
                    if view.open_menu.is_some() {
                        // 菜单已开时让位给 root 的菜单 Enter 处理，并重新
                        // 武装 keyup 合成 click 吞除标记。
                        view.note_button_key_activate("model-picker");
                        return;
                    }
                    view.note_button_key_activate("model-picker");
                    view.on_toggle_model_menu(None, window, cx);
                    cx.stop_propagation();
                }));
        }
        // Composer 紧邻窗口底部，model menu 明确从触发器上方打开；anchored
        // 仍负责贴合窗口边界，MenuPanel 负责长列表内部滚动。
        let mut model_picker = Dropdown::new(model_button).panel_anchor(
            Corner::BottomLeft,
            point(px(metrics::ZERO), px(-ANCHOR_GAP_Y)),
        );
        if model_menu_open {
            model_picker = model_picker.panel(self.model_menu_element(cx));
        }

        let running = self.projection.active_run_id.is_some();
        let action_slot = if running {
            let action_focus = self.composer_action_focus.clone();
            let cancel_tooltip = if can_cancel {
                SharedString::from("Cancel run (Cmd+.)")
            } else {
                SharedString::from(self.cancel_disabled_reason())
            };
            let mut cancel = Button::new("composer-action")
                .variant(ButtonVariant::Danger)
                .icon_circle(metrics::COMPOSER_SEND_SIZE)
                .disabled(!can_cancel)
                .track_focus(&action_focus)
                .label("✕")
                .tooltip(cancel_tooltip);
            if can_cancel {
                cancel = cancel
                    .on_click(cx.listener(|view, event, window, cx| {
                        if view.consume_button_key_click("composer-action", event) {
                            return;
                        }
                        view.on_cancel_clicked(window, cx);
                    }))
                    .on_activate(cx.listener(|view, _event, window, cx| {
                        view.note_button_key_activate("composer-action");
                        view.on_cancel_clicked(window, cx);
                        cx.stop_propagation();
                    }));
            }
            cancel.into_any_element()
        } else {
            let action_focus = self.composer_action_focus.clone();
            let send_tooltip = if can_send {
                SharedString::from("Send message (Enter)")
            } else {
                SharedString::from(self.send_disabled_reason())
            };
            let mut send = Button::new("composer-action")
                .variant(ButtonVariant::Primary)
                .icon_circle(metrics::COMPOSER_SEND_SIZE)
                .disabled(!can_send)
                .track_focus(&action_focus)
                .label("↑")
                .tooltip(send_tooltip);
            if can_send {
                send = send
                    .on_click(cx.listener(|view, event, _window, cx| {
                        if view.consume_button_key_click("composer-action", event) {
                            return;
                        }
                        if view.text_input.read(cx).is_composing() {
                            return;
                        }
                        view.send_current_message(cx);
                    }))
                    .on_activate(cx.listener(|view, _event, _window, cx| {
                        if view.text_input.read(cx).is_composing() {
                            return;
                        }
                        view.note_button_key_activate("composer-action");
                        view.send_current_message(cx);
                        cx.stop_propagation();
                    }));
            }
            send.into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(metrics::COMPOSER_GAP))
            .p(px(metrics::COMPOSER_PAD))
            .min_h(px(metrics::COMPOSER_PANEL_MIN_HEIGHT))
            .max_h(px(metrics::COMPOSER_PANEL_MAX_HEIGHT))
            .border_1()
            .border_color(dark().border.subtle)
            .rounded(px(metrics::SURFACE_RADIUS))
            .bg(dark().surface.raised)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .min_h(px(metrics::COMPOSER_INPUT_MIN_HEIGHT))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(metrics::COMPOSER_INPUT_MIN_HEIGHT))
                            .child(self.text_input.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(metrics::COMPOSER_SEND_SIZE))
                    .child(
                        div()
                            .max_w(px(220.0))
                            .min_w_0()
                            .overflow_hidden()
                            .child(model_picker),
                    )
                    .child(
                        div().max_w(px(180.0)).min_w_0().overflow_hidden().child(
                            // OPT-D / ADR-054 D1：无项目上下文用 chip 呈现
                            //（纯状态展示）；有项目沿用文字 scope 标签。
                            match self.composer_workspace_no_project() {
                                true => div()
                                    .px_2()
                                    .py(px(metrics::SPACE_1))
                                    .border_1()
                                    .border_color(dark().border.subtle)
                                    .rounded(px(metrics::CONTROL_RADIUS))
                                    .truncate()
                                    .child(
                                        Label::new(t("composer.no_project_chip"))
                                            .size(font::XS)
                                            .color(dark().text.secondary),
                                    ),
                                false => div().truncate().child(
                                    Label::new(self.composer_workspace_label())
                                        .size(font::XS)
                                        .color(dark().text.secondary),
                                ),
                            },
                        ),
                    )
                    .when(
                        self.composer_file_tools_unavailable_visible(),
                        |footer| {
                            footer.child(
                                div().max_w(px(320.0)).min_w_0().overflow_hidden().child(
                                    div().truncate().child(
                                        Label::new(t("composer.file_tools_unavailable"))
                                            .size(font::XS)
                                            .color(dark().text.tertiary),
                                    ),
                                ),
                            )
                        },
                    )
                    .child(
                        Label::new(context_meter)
                            .size(font::XS)
                            .color(if context_available {
                                dark().text.secondary
                            } else {
                                dark().text.tertiary
                            }),
                    )
                    .when_some(self.status_hint.as_ref(), |footer, hint| {
                        footer.child(
                            div().max_w(px(360.0)).min_w_0().overflow_hidden().child(
                                div().truncate().child(
                                    Label::new(hint.clone())
                                        .size(font::XS)
                                        .color(dark().semantic.warning_text),
                                ),
                            ),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .w(px(metrics::COMPOSER_SEND_SIZE))
                            .h(px(metrics::COMPOSER_SEND_SIZE))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(action_slot),
                    ),
            )
    }

    fn model_label(&self) -> String {
        match self.projection.effective_model() {
            Some((provider, id)) => self
                .projection
                .models
                .iter()
                .find(|entry| entry.provider_id == *provider && entry.id == *id)
                .map(|entry| entry.display_name.clone())
                .unwrap_or_else(|| format!("{provider} / {id}")),
            None if self.projection.models.is_empty() => t("composer.model_loading").into(),
            None => t("composer.model_select").into(),
        }
    }

    fn model_disabled_reason(&self) -> String {
        if self.projection.active_run_id.is_some() {
            t("composer.model_disabled_running").into()
        } else if self.projection.models.is_empty() {
            t("composer.model_disabled_loading").into()
        } else {
            t("composer.model_disabled_offline").into()
        }
    }

    /// model 菜单面板（从 composer 内联抽出，与其他组同构的浮层 + MenuRow）。
    fn model_menu_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        let entries = grouped_model_menu_entries(&self.projection.models);
        let selected_ix = self
            .projection
            .effective_model()
            .and_then(|(provider, id)| {
                entries
                    .iter()
                    .position(|entry| entry.provider_id == *provider && entry.id == *id)
            })
            .unwrap_or(0);
        let highlight = self.menu_highlight_effective(selected_ix);
        let mut panel = MenuPanel::new("model-menu").dismiss_on_outside(cx.listener(
            |view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Model, event.position, cx);
            },
        ));
        let mut item_ix = 0;
        for (provider_id, models) in group_models_by_provider(&self.projection.models) {
            panel = panel.child(
                div()
                    .h(px(MODEL_MENU_GROUP_HEADER_HEIGHT))
                    .px_2()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .truncate()
                    .text_size(font::SM)
                    .text_color(dark().text.secondary)
                    .child(provider_id),
            );
            for model in models {
                let selected = self
                    .projection
                    .effective_model()
                    .is_some_and(|(provider, id)| {
                        provider == &model.provider_id && id == &model.id
                    });
                panel = panel.child(
                    MenuRow::new(SharedString::from(format!(
                        "model-{}-{}",
                        model.provider_id, model.id
                    )))
                    .label(model.display_name.clone())
                    .selected(selected)
                    .highlighted(item_ix == highlight)
                    .on_click(cx.listener(
                        move |view, _event, _window, cx| {
                            view.on_select_model(model.clone(), cx);
                        },
                    )),
                );
                item_ix += 1;
            }
        }
        panel
    }

    /// Composer 空输入 placeholder：只走连接/session/run 状态机，不被
    /// status_hint 覆盖（瞬态反馈改落 footer Label）。
    pub(super) fn composer_placeholder_hint(&self) -> String {
        composer_placeholder_hint(
            &self.projection.connection,
            self.projection.active_session_id.is_some(),
            self.projection.active_run_id.is_some(),
        )
    }

    fn send_disabled_reason(&self) -> String {
        if self.projection.active_run_id.is_some() {
            t("composer.placeholder_running").into()
        } else {
            match &self.projection.connection {
                ConnectionState::Connected { .. } => {
                    if self.projection.active_session_id.is_none() {
                        t("composer.placeholder_open_session").into()
                    } else {
                        t("composer.send_disabled_empty").into()
                    }
                }
                ConnectionState::Connecting => t("composer.placeholder_waiting").into(),
                ConnectionState::Disconnected { .. } => {
                    t("composer.placeholder_disconnected").into()
                }
                ConnectionState::Failed { .. } => t("composer.placeholder_connect_failed").into(),
            }
        }
    }

    fn sync_composer_placeholder(&self, hint: String, cx: &mut Context<Self>) {
        self.text_input.update(cx, |input, cx| {
            input.set_placeholder(hint, cx);
        });
    }

    /// 面板常态总高：border + pad×2 + 输入行 + gap + 动作槽。
    pub(super) fn composer_panel_height(input_height: f32) -> f32 {
        (metrics::COMPOSER_BORDER
            + metrics::COMPOSER_PAD * 2.0
            + input_height.max(metrics::COMPOSER_INPUT_MIN_HEIGHT)
            + metrics::COMPOSER_GAP
            + metrics::COMPOSER_SEND_SIZE)
            .clamp(
                metrics::COMPOSER_PANEL_MIN_HEIGHT,
                metrics::COMPOSER_PANEL_MAX_HEIGHT,
            )
    }

    pub(super) fn composer_workspace_label(&self) -> String {
        if let Some(session_id) = &self.projection.active_session_id {
            if let Some(session) = self
                .projection
                .sessions
                .iter()
                .find(|session| &session.session_id == session_id)
            {
                return t("composer.workspace_scope")
                    .replace("{}", &self.projection.workspace_name(session.workspace_id.as_deref()));
            }
        }
        match self.scope_workspace_id.as_deref() {
            Some(id) => t("composer.workspace_scope")
                .replace("{}", &self.projection.workspace_name(Some(id))),
            None => t("composer.no_project_chip").into(),
        }
    }

    /// Composer footer 的项目上下文是否为「无项目」：active session 优先
    ///（ADR-054 D1 无归属会话），否则回落当前 scope；All projects 同样
    /// 视为无项目上下文（新建任务将不绑定 workspace）。
    pub(super) fn composer_workspace_no_project(&self) -> bool {
        if let Some(session_id) = self.projection.active_session_id.as_deref() {
            if let Some(session) = self
                .projection
                .sessions
                .iter()
                .find(|session| session.session_id == session_id)
            {
                return session.workspace_id.is_none();
            }
        }
        self.scope_workspace_id.is_none()
    }

    /// 文件工具不可用提示只在无项目会话激活时出现（无 active session 时
    /// 不提示——还没有任务上下文）。
    pub(super) fn composer_file_tools_unavailable_visible(&self) -> bool {
        self.projection
            .active_session_id
            .as_deref()
            .and_then(|session_id| {
                self.projection
                    .sessions
                    .iter()
                    .find(|session| session.session_id == session_id)
            })
            .is_some_and(|session| session.workspace_id.is_none())
    }

    pub(super) fn on_toggle_model_menu(
        &mut self,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_switch_model() {
            return;
        }
        self.toggle_menu(MenuKind::Model, down_position, cx);
    }

    pub(super) fn on_select_model(&mut self, model: ModelEntry, cx: &mut Context<Self>) {
        if !self.can_switch_model() {
            return;
        }
        self.projection
            .set_pending_model(model.provider_id, model.id);
        self.open_menu = None;
        self.menu_highlight = None;
        cx.notify();
    }

    fn cancel_disabled_reason(&self) -> String {
        if self.projection.active_run_id.is_none() {
            "No active run to cancel.".into()
        } else {
            "Cancel needs a live connection.".into()
        }
    }
}

fn composer_placeholder_hint(
    connection: &ConnectionState,
    has_session: bool,
    running: bool,
) -> String {
    if running {
        return t("composer.placeholder_running").into();
    }
    match connection {
        ConnectionState::Connected { .. } => {
            if has_session {
                t("composer.placeholder_message").into()
            } else {
                t("composer.placeholder_open_session").into()
            }
        }
        ConnectionState::Connecting => t("composer.placeholder_waiting").into(),
        ConnectionState::Disconnected { .. } => {
            t("composer.placeholder_disconnected").into()
        }
        ConnectionState::Failed { .. } => t("composer.placeholder_connect_failed").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{composer_placeholder_hint, grouped_model_menu_entries, AppView};
    use crate::projection::{ConnectionState, ModelEntry};
    use crate::ui::theme::metrics;

    #[test]
    fn composer_placeholder_hint_follows_connection_and_run_state() {
        let connected = ConnectionState::Connected {
            instance_id: "dev".into(),
        };
        assert_eq!(
            composer_placeholder_hint(&connected, true, false),
            "Message Pawork… (Enter to send, Shift+Enter for newline)"
        );
        assert_eq!(
            composer_placeholder_hint(&connected, false, false),
            "Open a session to send messages."
        );
        assert_eq!(
            composer_placeholder_hint(&connected, true, true),
            "Run in progress — sending is disabled. Cancel remains available."
        );
        assert_eq!(
            composer_placeholder_hint(&ConnectionState::Connecting, true, false),
            "Waiting for connection…"
        );
        assert_eq!(
            composer_placeholder_hint(
                &ConnectionState::Disconnected {
                    reason: "lost".into(),
                },
                true,
                false,
            ),
            "Disconnected — click Reconnect before sending."
        );
        assert_eq!(
            composer_placeholder_hint(
                &ConnectionState::Failed {
                    reason: "boom".into(),
                },
                true,
                false,
            ),
            "Connect failed — click Reconnect."
        );
        // 瞬态 status_hint 不再覆盖 placeholder 状态机。
        assert_ne!(
            composer_placeholder_hint(&connected, true, false),
            "Forked · s-1"
        );
    }

    #[test]
    fn composer_panel_height_clamps_across_input_sizes() {
        let idle = AppView::composer_panel_height(metrics::COMPOSER_INPUT_MIN_HEIGHT);
        assert!(idle >= 88.0 && idle <= 94.0, "idle panel {idle}");
        assert_eq!(idle, metrics::COMPOSER_PANEL_MIN_HEIGHT);
        let mid = AppView::composer_panel_height(80.0);
        assert!(mid > idle);
        assert!(mid < metrics::COMPOSER_PANEL_MAX_HEIGHT);
        let capped = AppView::composer_panel_height(400.0);
        assert_eq!(capped, metrics::COMPOSER_PANEL_MAX_HEIGHT);
        assert_eq!(metrics::COMPOSER_SEND_SIZE, 32.0);
    }

    #[test]
    fn model_menu_selected_follows_effective_model() {
        let models = [
            ModelEntry {
                provider_id: "openai".into(),
                id: "gpt-4.1".into(),
                display_name: "GPT-4.1".into(),
                context_window_tokens: Some(128_000),
            },
            ModelEntry {
                provider_id: "anthropic".into(),
                id: "opus".into(),
                display_name: "Opus".into(),
                context_window_tokens: Some(200_000),
            },
            ModelEntry {
                provider_id: "openai".into(),
                id: "gpt-4.1-mini".into(),
                display_name: "GPT-4.1 mini".into(),
                context_window_tokens: Some(128_000),
            },
        ];
        let entries = grouped_model_menu_entries(&models);
        assert_eq!(
            entries
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-4.1", "gpt-4.1-mini", "opus"]
        );
        let selected = Some(("anthropic".to_string(), "opus".to_string()));
        let selected_ix = selected
            .as_ref()
            .and_then(|(provider, id)| {
                entries
                    .iter()
                    .position(|entry| entry.provider_id == *provider && entry.id == *id)
            })
            .unwrap_or(0);
        assert_eq!(selected_ix, 2);
        assert_eq!(entries[selected_ix].display_name, "Opus");
        let none_ix = None::<(String, String)>
            .as_ref()
            .and_then(|(provider, id)| {
                entries
                    .iter()
                    .position(|entry| entry.provider_id == *provider && entry.id == *id)
            })
            .unwrap_or(0);
        assert_eq!(none_ix, 0);
    }
}
