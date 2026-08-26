//! Composer 输入区（InputArea）：模型选择菜单、上下文 / 工作区标签、
//! 工作区确认浮层、输入行与 Cancel / Send（R8 波 C 自 ui/mod.rs 逐样式迁移）。

use gpui::{
    anchored, deferred, div, point, prelude::*, px, Context, Corner, Pixels, Point, SharedString,
    Window,
};

use crate::projection::{ConnectionState, ModelEntry};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow, ANCHOR_GAP_Y};
use crate::ui::components::label::Label;
use crate::ui::theme::{dark, font, metrics};

use super::{AppView, MenuKind};

impl AppView {
    pub(super) fn composer_element(&self, cx: &mut Context<Self>) -> gpui::Div {
        let can_send = self.can_send();
        let can_cancel = self.can_cancel();
        let can_switch_model = self.can_switch_model();
        let model_menu_open = matches!(self.open_menu, Some(MenuKind::Model)) && can_switch_model;
        let composer_hint = self.composer_hint();
        let context_meter = self.projection.context_meter_label();

        let model_tooltip = if can_switch_model {
            SharedString::from("Select model")
        } else {
            SharedString::from(self.model_disabled_reason())
        };
        let model_focus = self.model_focus.clone();
        let mut model_button = Button::new("model-picker")
            .track_focus(&model_focus)
            .variant(ButtonVariant::Raised)
            .disabled(!can_switch_model)
            .label(self.model_label())
            .tooltip(model_tooltip);
        if can_switch_model {
            model_button = model_button.on_click(cx.listener(|view, event, window, cx| {
                let down = Self::click_down_position(event);
                view.on_toggle_model_menu(down, window, cx);
            }));
        }
        let mut model_picker = Dropdown::new(model_button);
        if model_menu_open {
            model_picker = model_picker.panel(self.model_menu_element(cx));
        }

        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(dark().border.subtle)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(model_picker)
                    .child(
                        div().flex_1().child(
                            Label::new(context_meter)
                                .size(font::XS)
                                .color(dark().text.secondary),
                        ),
                    )
                    .child(
                        Label::new(self.composer_workspace_label())
                            .size(font::XS)
                            .color(dark().text.secondary),
                    ),
            )
            .when(
                matches!(self.open_menu, Some(MenuKind::WorkspaceConfirm)),
                |composer| {
                    // 无触发器：锚在 label 行正下方（原 in-flow 位置），浮层化不占布局流。
                    composer.child(
                        deferred(
                            anchored()
                                .anchor(Corner::TopLeft)
                                .offset(point(px(metrics::ZERO), px(ANCHOR_GAP_Y)))
                                .child(self.workspace_confirm_element(cx)),
                        )
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(metrics::COMPOSER_MIN_HEIGHT))
                            .max_h(px(metrics::COMPOSER_MAX_HEIGHT))
                            .child(self.text_input.clone()),
                    )
                    .child({
                        let cancel_focus = self.cancel_focus.clone();
                        let cancel_tooltip = if can_cancel {
                            SharedString::from("Cancel run (⌘.)")
                        } else {
                            SharedString::from(self.cancel_disabled_reason())
                        };
                        let mut cancel = Button::new("cancel")
                            .variant(ButtonVariant::Danger)
                            .disabled(!can_cancel)
                            .track_focus(&cancel_focus)
                            .padding(ButtonPadding::Wide)
                            .label("Cancel")
                            .tooltip(cancel_tooltip);
                        if can_cancel {
                            cancel = cancel.on_click(cx.listener(|view, _event, window, cx| {
                                view.on_cancel_clicked(window, cx);
                            }));
                        }
                        cancel
                    })
                    .child({
                        let send_focus = self.send_focus.clone();
                        let send_tooltip = if can_send {
                            SharedString::from("Send message (Enter)")
                        } else {
                            SharedString::from(composer_hint.clone())
                        };
                        let mut send = Button::new("send")
                            .variant(ButtonVariant::Primary)
                            .disabled(!can_send)
                            .track_focus(&send_focus)
                            .padding(ButtonPadding::Wide)
                            .label("Send")
                            .tooltip(send_tooltip);
                        if can_send {
                            send = send.on_click(cx.listener(|view, _event, _window, cx| {
                                view.send_current_message(cx);
                            }));
                        }
                        send
                    }),
            )
            .child(
                Label::new(self.status_hint.clone().unwrap_or(composer_hint))
                    .size(font::XS)
                    .color(dark().text.secondary),
            )
    }

    fn model_label(&self) -> String {
        match self.projection.effective_model() {
            Some((provider, id)) => self
                .projection
                .models
                .iter()
                .find(|entry| entry.provider_id == *provider && entry.id == *id)
                .map(|entry| format!("{} / {}", entry.provider_id, entry.display_name))
                .unwrap_or_else(|| format!("{provider} / {id}")),
            None if self.projection.models.is_empty() => "Model · loading".into(),
            None => "Model · select".into(),
        }
    }

    fn model_disabled_reason(&self) -> String {
        if self.projection.active_run_id.is_some() {
            "Model switch is disabled while a run is in progress.".into()
        } else if self.projection.models.is_empty() {
            "Model catalog is still loading.".into()
        } else {
            "Model switch needs a live connection.".into()
        }
    }

    /// model 菜单面板（从 composer 内联抽出，与其他组同构的浮层 + MenuRow）。
    fn model_menu_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        MenuPanel::new("model-menu")
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Model, event.position, cx);
            }))
            .children(self.projection.models.iter().cloned().map(|model| {
                let selected = self
                    .projection
                    .effective_model()
                    .is_some_and(|(provider, id)| {
                        provider == &model.provider_id && id == &model.id
                    });
                let label = format!("{} / {}", model.provider_id, model.display_name);
                MenuRow::new(SharedString::from(format!(
                    "model-{}-{}",
                    model.provider_id, model.id
                )))
                .label(label)
                .selected(selected)
                .on_click(cx.listener(move |view, _event, _window, cx| {
                    view.on_select_model(model.clone(), cx);
                }))
            }))
    }

    /// Composer 禁用原因（文本说明，不只靠颜色，gui-design §6）。
    fn composer_hint(&self) -> String {
        if self.projection.active_run_id.is_some() {
            return "Run in progress — sending and model switch are disabled. Cancel remains available.".into();
        }
        match &self.projection.connection {
            ConnectionState::Connected { .. } => {
                if self.projection.active_session_id.is_none() {
                    "Open a session to send messages.".into()
                } else {
                    "Enter to send · Shift+Enter for newline".into()
                }
            }
            ConnectionState::Connecting => "Waiting for connection…".into(),
            ConnectionState::Disconnected { .. } => {
                "Disconnected — click Reconnect before sending.".into()
            }
            ConnectionState::Failed { .. } => "Connect failed — click Reconnect.".into(),
        }
    }

    fn composer_workspace_label(&self) -> String {
        if let Some(session_id) = &self.projection.active_session_id {
            if let Some(session) = self
                .projection
                .sessions
                .iter()
                .find(|session| &session.session_id == session_id)
            {
                return format!(
                    "Workspace · {}",
                    self.projection
                        .workspace_name(session.workspace_id.as_deref())
                );
            }
        }
        match self.scope_workspace_id.as_deref() {
            Some(id) => format!("Workspace · {}", self.projection.workspace_name(Some(id))),
            None => "Workspace · confirm in All projects".into(),
        }
    }

    fn workspace_confirm_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        let choices: Vec<(String, String)> = self
            .projection
            .project_scope_options()
            .into_iter()
            .filter_map(|(id, name)| id.map(|id| (id, name)))
            .collect();
        let mut panel = MenuPanel::new("workspace-confirm")
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::WorkspaceConfirm, event.position, cx);
            }));
        if choices.is_empty() {
            return panel.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(font::SM))
                    .text_color(dark().semantic.warning_text)
                    .child("Add a workspace before creating a task."),
            );
        }
        for (id, name) in choices {
            let pick = id.clone();
            panel = panel.child(
                MenuRow::new(SharedString::from(format!("workspace-confirm-{id}")))
                    .label(name)
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.on_confirm_workspace(pick.clone(), window, cx);
                    })),
            );
        }
        panel
    }

    pub(super) fn on_confirm_workspace(
        &mut self,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu = None;
        self.create_task(Some(workspace_id), window, cx);
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
        self.projection
            .set_pending_model(model.provider_id, model.id);
        self.open_menu = None;
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
