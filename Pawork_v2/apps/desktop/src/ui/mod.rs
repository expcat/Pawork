//! UI 层：AppView（Sessions 侧栏 + Timeline + Composer）与事件消费循环。

pub mod text_input;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, Render, ScrollHandle,
    SharedString, Window, div, prelude::*, px, rgb,
};

use crate::controller::{ControllerEvent, DesktopController};
use crate::platform::Platform;
use crate::projection::{
    ConnectionState, DesktopProjection, ModelEntry, TimelineEntry, TimelineEntryKind,
};

pub use text_input::{SendMessage, TextInput};

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

pub fn install_keybindings(cx: &mut App) {
    use text_input::{Backspace, Delete, End, Home, Left, NewLine, Paste, Right};

    // 绑定只对 key_context "TextInput" 聚焦时生效；Enter 冒泡到 AppView，
    // 由 AppView 结合 composing / 发送可用性决定是否发送（gui-design §6）。
    cx.bind_keys([
        KeyBinding::new("enter", SendMessage, Some("TextInput")),
        KeyBinding::new("shift-enter", NewLine, Some("TextInput")),
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("cmd-v", Paste, Some("TextInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
    ]);
}

pub struct AppView {
    /// 持有 tokio Runtime（GUI Connection Protocol 宿主），防止提前 shutdown。
    _platform: Arc<Platform>,
    controller: Arc<DesktopController>,
    socket: PathBuf,
    projection: DesktopProjection,
    text_input: Entity<TextInput>,
    scroll_handle: ScrollHandle,
    status_hint: Option<String>,
    model_menu_open: bool,
}

impl AppView {
    pub fn new(platform: Arc<Platform>, socket: PathBuf, cx: &mut Context<Self>) -> Self {
        let controller = Arc::new(DesktopController::new(platform.handle()));
        let text_input = cx.new(|cx| TextInput::new(cx));
        let mut view = Self {
            _platform: platform,
            controller,
            socket,
            projection: DesktopProjection::default(),
            text_input,
            scroll_handle: ScrollHandle::new(),
            status_hint: None,
            model_menu_open: false,
        };
        view.start_connect(cx);
        view
    }

    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.text_input.read(cx).focus_handle(cx)
    }

    fn focus_composer(&self, window: &mut Window, cx: &App) {
        window.focus(&self.composer_focus_handle(cx));
    }

    fn start_connect(&mut self, cx: &mut Context<Self>) {
        self.projection.set_connection(ConnectionState::Connecting);
        self.status_hint = None;
        let controller = Arc::clone(&self.controller);
        let socket = self.socket.clone();
        cx.spawn(async move |this, cx| {
            match controller.connect(socket).await {
                Ok((snapshot, events)) => {
                    this.update(cx, |view, cx| {
                        view.on_connected(snapshot, events, cx);
                    })
                    .ok();
                }
                Err(reason) => {
                    this.update(cx, |view, cx| {
                        view.projection
                            .set_connection(ConnectionState::Failed { reason });
                        view.status_hint =
                            Some("Connect failed. Click Reconnect to retry.".into());
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    fn on_connected(
        &mut self,
        snapshot: pawork_client::Snapshot,
        events: smol::channel::Receiver<ControllerEvent>,
        cx: &mut Context<Self>,
    ) {
        let instance_id = snapshot.instance_id.as_str().to_string();
        let previous_session = self.projection.active_session_id.clone();
        self.projection.merge_snapshot(&snapshot);
        self.projection
            .set_connection(ConnectionState::Connected { instance_id });
        self.controller.load_models();
        self.consume_events(events, cx);
        if let Some(session_id) = previous_session {
            if self
                .projection
                .sessions
                .iter()
                .any(|session| session.session_id == session_id)
            {
                self.open_session(session_id, cx);
                return;
            }
        }
        cx.notify();
    }

    fn consume_events(
        &mut self,
        events: smol::channel::Receiver<ControllerEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |view, cx| view.handle_controller_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn handle_controller_event(&mut self, event: ControllerEvent, cx: &mut Context<Self>) {
        match event {
            ControllerEvent::Disconnected { reason } => {
                self.projection
                    .set_connection(ConnectionState::Disconnected { reason });
                self.status_hint = Some("Connection lost. Click Reconnect.".into());
            }
            ControllerEvent::Snapshot(snapshot) => {
                self.projection.merge_snapshot(&snapshot);
            }
            ControllerEvent::TimelineLoaded { session_id, page } => {
                if self.projection.active_session_id.as_deref() == Some(&session_id) {
                    self.projection.apply_timeline_page(&page);
                    self.scroll_handle.scroll_to_bottom();
                }
            }
            ControllerEvent::Event(envelope) => {
                if self.projection.apply_event(&envelope) {
                    self.scroll_handle.scroll_to_bottom();
                }
            }
            ControllerEvent::SessionCreated { session_id } => {
                self.open_session(session_id, cx);
            }
            ControllerEvent::MessageSent { session_id, run_id } => {
                if self.projection.active_session_id.as_deref() == Some(&session_id) {
                    self.projection.active_run_id = Some(run_id);
                }
                self.text_input.update(cx, |input, cx| input.clear(cx));
            }
            ControllerEvent::ModelsLoaded(models) => {
                self.projection.set_models(models);
            }
            ControllerEvent::OperationFailed { action, reason } => {
                self.status_hint = Some(format!("{action} failed: {reason}"));
            }
        }
        cx.notify();
    }

    fn open_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        self.projection.select_session(&session_id);
        self.status_hint = None;
        self.scroll_handle = ScrollHandle::new();
        self.controller.open_session(session_id);
        cx.notify();
    }

    fn on_session_clicked(
        &mut self,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.projection.active_session_id.as_deref() == Some(session_id) {
            self.focus_composer(window, cx);
            return;
        }
        self.open_session(session_id.to_string(), cx);
        self.focus_composer(window, cx);
    }

    fn on_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.status_hint = Some("New Session needs a live connection.".into());
            cx.notify();
            return;
        }
        let workspace = self
            .projection
            .workspace_id
            .clone()
            .unwrap_or_else(|| "ws-default".into());
        self.controller.create_session(workspace);
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn on_reconnect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.start_connect(cx);
    }

    fn on_send_clicked(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.send_current_message(cx);
    }

    fn on_send_message(&mut self, _: &SendMessage, _window: &mut Window, cx: &mut Context<Self>) {
        // IME 组合中的 Enter 属于输入法确认（gui-design §6）。
        if self.text_input.read(cx).is_composing() {
            return;
        }
        self.send_current_message(cx);
    }

    fn send_current_message(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            self.status_hint = Some("Open a session first.".into());
            cx.notify();
            return;
        };
        if !self.can_send() {
            return;
        }
        let text = self.text_input.read(cx).text().to_string();
        if text.trim().is_empty() {
            return;
        }
        let model = self
            .projection
            .effective_model()
            .map(|(_, id)| id.clone());
        self.controller.send_message(session_id, text, model);
    }

    fn on_cancel_clicked(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(run_id) = self.projection.active_run_id.clone() else {
            return;
        };
        self.controller.cancel_run(run_id);
        cx.notify();
    }

    fn on_approve(&mut self, decision: &str, cx: &mut Context<Self>) {
        let Some(pending) = self.projection.pending_approval.clone() else {
            return;
        };
        self.controller
            .approve(pending.run_id, pending.tool_call_id, decision);
        cx.notify();
    }

    fn on_toggle_model_menu(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_switch_model() {
            return;
        }
        self.model_menu_open = !self.model_menu_open;
        cx.notify();
    }

    fn on_select_model(&mut self, model: ModelEntry, cx: &mut Context<Self>) {
        self.projection
            .set_pending_model(model.provider_id, model.id);
        self.model_menu_open = false;
        cx.notify();
    }

    fn can_switch_model(&self) -> bool {
        matches!(self.projection.connection, ConnectionState::Connected { .. })
            && self.projection.active_run_id.is_none()
            && !self.projection.models.is_empty()
    }

    fn can_send(&self) -> bool {
        matches!(self.projection.connection, ConnectionState::Connected { .. })
            && self.projection.active_session_id.is_some()
            && self.projection.active_run_id.is_none()
    }

    fn can_approve(&self) -> bool {
        matches!(self.projection.connection, ConnectionState::Connected { .. })
            && self.projection.pending_approval.is_some()
    }

    fn can_cancel(&self) -> bool {
        matches!(self.projection.connection, ConnectionState::Connected { .. })
            && self.projection.active_run_id.is_some()
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

    fn timeline_entry_element(entry: &TimelineEntry) -> gpui::Div {
        match &entry.kind {
            TimelineEntryKind::UserMessage { text } => div()
                .py_1()
                .text_color(rgb(0xe8e8e8))
                .child(format!("You: {text}")),
            TimelineEntryKind::AssistantMessage { text } => div()
                .py_1()
                .text_color(rgb(0xd7d7ff))
                .child(format!("Assistant: {text}")),
            TimelineEntryKind::ToolCall { name, status, detail } => {
                let mut element = div()
                    .py_1()
                    .text_color(rgb(0x9cdcfe))
                    .child(format!("{name} · {status}"));
                if let Some(detail) = detail {
                    if !detail.is_empty() {
                        element = element.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(0x7f7f7f))
                                .child(detail.clone()),
                        );
                    }
                }
                element
            }
            TimelineEntryKind::RunState(state) => {
                div().py_1().text_color(rgb(0x8f8f8f)).child(state.clone())
            }
            TimelineEntryKind::Error(message) => div()
                .py_1()
                .text_color(rgb(0xf48771))
                .child(format!("Error: {message}")),
        }
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(self.projection.connection, ConnectionState::Connected { .. });
        let can_send = self.can_send();
        let can_cancel = self.can_cancel();
        let can_switch_model = self.can_switch_model();
        let can_approve = self.can_approve();
        let model_label = self.model_label();
        let model_menu_open = self.model_menu_open && can_switch_model;
        let connection_label = self.projection.connection.label();
        let composer_hint = self.composer_hint();
        let context_meter = self.projection.context_meter_label();
        let run_status = self.projection.run_status_label(now_unix_ms());

        let sidebar = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .w(px(240.))
            .h_full()
            .bg(rgb(0x161616))
            .border_r_1()
            .border_color(rgb(0x2e2e2e))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x9a9a9a))
                    .child(connection_label),
            )
            .when(!connected, |sidebar| {
                sidebar.child(
                    div()
                        .id("reconnect")
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0x2f6fed))
                        .text_color(rgb(0xffffff))
                        .cursor_pointer()
                        .child("Reconnect")
                        .on_click(cx.listener(|view, _event, window, cx| {
                            view.on_reconnect(window, cx);
                        })),
                )
            })
            .child(
                div()
                    .id("new-session")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x333333))
                    .cursor_pointer()
                        .child("New Session")
                        .on_click(cx.listener(|view, _event, window, cx| {
                            view.on_new_session(window, cx);
                        })),
            )
            .child(div().text_size(px(11.)).text_color(rgb(0xbbbbbb)).child("SESSIONS"))
            .children(self.projection.sessions.iter().map(|session| {
                let session_id = session.session_id.clone();
                let active = self.projection.active_session_id.as_deref()
                    == Some(session.session_id.as_str());
                div()
                    .id(SharedString::from(session_id.clone()))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(if active { rgb(0x2a2a2a) } else { rgb(0x161616) })
                    .cursor_pointer()
                    .child(session.title.clone())
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.on_session_clicked(&session_id, window, cx);
                    }))
            }));

        let timeline = div()
            .id("timeline")
            .flex()
            .flex_col()
            .track_scroll(&self.scroll_handle)
            .flex_1()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .gap_1()
            .children(
                self.projection
                    .timeline
                    .iter()
                    .map(Self::timeline_entry_element),
            )
            .when(self.projection.pending_approval.is_some(), |timeline| {
                let pending = self
                    .projection
                    .pending_approval
                    .as_ref()
                    .expect("pending approval exists");
                let mut card = div()
                    .p_2()
                    .rounded_md()
                    .border_l_1()
                    .border_color(rgb(0x8a6d3b))
                    .bg(rgb(0x2a2418))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0xf0d58c))
                            .child(format!("Approval · {}", pending.tool_name)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0xe8e8e8))
                            .child(pending.reason.clone()),
                    );
                if let Some(detail) = pending.detail.clone() {
                    if !detail.is_empty() {
                        card = card.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(0xb8b8b8))
                                .child(detail),
                        );
                    }
                }
                let buttons = [
                    ("approve-once", "Allow once", "approve_once", 0x2f6fed_u32),
                    ("approve-for-run", "Allow for run", "approve_for_run", 0x3d7a4a_u32),
                    ("approve-deny", "Deny", "deny", 0x8a3b32_u32),
                ];
                let row = div().flex().flex_row().gap_2().children(buttons.into_iter().map(
                    |(id, label, decision, color)| {
                        let decision = decision.to_string();
                        div()
                            .id(SharedString::from(id))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(if can_approve { rgb(color) } else { rgb(0x3a3a3a) })
                            .text_color(rgb(0xffffff))
                            .cursor_pointer()
                            .child(label)
                            .when(can_approve, |button| {
                                button.on_click(cx.listener(move |view, _event, _window, cx| {
                                    view.on_approve(&decision, cx);
                                }))
                            })
                    },
                ));
                timeline.child(card.child(row))
            });

        let mut model_picker = div()
            .id("model-picker")
            .px_2()
            .py_1()
            .rounded_md()
            .bg(if can_switch_model { rgb(0x2a2a2a) } else { rgb(0x242424) })
            .text_color(if can_switch_model { rgb(0xe8e8e8) } else { rgb(0x8f8f8f) })
            .cursor_pointer()
            .child(model_label)
            .when(can_switch_model, |button| {
                button.on_click(cx.listener(|view, _event, window, cx| {
                    view.on_toggle_model_menu(window, cx);
                }))
            });
        if model_menu_open {
            model_picker = model_picker.child(
                div()
                    .mt_1()
                    .p_1()
                    .rounded_md()
                    .bg(rgb(0x1a1a1a))
                    .border_1()
                    .border_color(rgb(0x3a3a3a))
                    .children(self.projection.models.iter().cloned().map(|model| {
                        let selected = self
                            .projection
                            .effective_model()
                            .is_some_and(|(provider, id)| {
                                provider == &model.provider_id && id == &model.id
                            });
                        let label = format!("{} / {}", model.provider_id, model.display_name);
                        div()
                            .id(SharedString::from(format!(
                                "model-{}-{}",
                                model.provider_id, model.id
                            )))
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(if selected { rgb(0x2f6fed) } else { rgb(0x1a1a1a) })
                            .cursor_pointer()
                            .child(label)
                            .on_click(cx.listener(move |view, _event, _window, cx| {
                                view.on_select_model(model.clone(), cx);
                            }))
                    })),
            );
        }

        let composer = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(rgb(0x2e2e2e))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(model_picker)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(rgb(0x9a9a9a))
                            .child(context_meter),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(div().flex_1().child(self.text_input.clone()))
                    .when(can_cancel, |row| {
                        row.child(
                            div()
                                .id("cancel")
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(0x8a3b32))
                                .text_color(rgb(0xffffff))
                                .cursor_pointer()
                                .child("Cancel")
                                .on_click(cx.listener(|view, _event, window, cx| {
                                    view.on_cancel_clicked(window, cx);
                                })),
                        )
                    })
                    .child(
                        div()
                            .id("send")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(if can_send { rgb(0x2f6fed) } else { rgb(0x3a3a3a) })
                            .text_color(rgb(0xffffff))
                            .cursor_pointer()
                            .child("Send")
                            .when(can_send, |button| {
                                button.on_click(cx.listener(|view, _event, window, cx| {
                                    view.on_send_clicked(window, cx);
                                }))
                            }),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(0x9a9a9a))
                    .child(self.status_hint.clone().unwrap_or(composer_hint)),
            );

        div()
            .flex()
            .size_full()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xe8e8e8))
            .on_action(cx.listener(Self::on_send_message))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(timeline)
                    .child(composer)
                    .child(
                        div()
                            .h(px(24.))
                            .px_3()
                            .flex()
                            .items_center()
                            .border_t_1()
                            .border_color(rgb(0x2e2e2e))
                            .bg(rgb(0x161616))
                            .text_size(px(11.))
                            .text_color(rgb(0x9a9a9a))
                            .child(run_status),
                    ),
            )
    }
}
