//! Timeline 条目（TimelineEntryView）：消息 / 工具调用 / 运行态渲染与
//! 条目「···」fork 菜单（R8 波 C 自 ui/mod.rs 逐样式迁移）。

use gpui::{div, prelude::*, px, Context, SharedString};

use crate::projection::{ConnectionState, TimelineEntry, TimelineEntryKind};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::theme::{dark, font, metrics};

use super::{AppView, MenuKind};

impl AppView {
    pub(super) fn timeline_entry_element(
        &self,
        entry: &TimelineEntry,
        menu_open: bool,
        can_fork: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let body = match &entry.kind {
            TimelineEntryKind::UserMessage { text } => div()
                .py_1()
                .text_color(dark().text.primary)
                .child(format!("You: {text}")),
            TimelineEntryKind::AssistantMessage { text } => div()
                .py_1()
                .text_color(dark().text.assistant)
                .child(format!("Assistant: {text}")),
            TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
            } => {
                let mut element = div()
                    .py_1()
                    .text_color(dark().text.tool)
                    .child(format!("{name} · {status}"));
                if let Some(detail) = detail {
                    if !detail.is_empty() {
                        element = element.child(
                            div()
                                .text_size(px(font::XS))
                                .text_color(dark().text.tertiary)
                                .child(detail.clone()),
                        );
                    }
                }
                element
            }
            TimelineEntryKind::RunState(state) => div()
                .py_1()
                .text_color(dark().text.disabled)
                .child(state.clone()),
            TimelineEntryKind::Error(message) => div()
                .py_1()
                .text_color(dark().semantic.danger_text)
                .child(format!("Error: {message}")),
        };
        let event_id = entry.event_id.clone();
        let actions_button = Button::new(format!("entry-menu-{}", entry.event_id))
            .variant(ButtonVariant::Ghost)
            .text_size(font::XS)
            .text_color(dark().text.secondary)
            .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
            .label("···")
            .on_click(cx.listener({
                let event_id = event_id.clone();
                move |view, event, _window, cx| {
                    let down = Self::click_down_position(event);
                    view.toggle_menu(MenuKind::Entry(event_id.clone()), down, cx);
                }
            }));
        let mut actions = Dropdown::new(actions_button);
        if menu_open {
            let fork_id = event_id.clone();
            actions = actions.panel(
                MenuPanel::new(SharedString::from(format!("fork-menu-{}", entry.event_id)))
                    .dismiss_on_outside(cx.listener({
                        let kind = MenuKind::Entry(event_id.clone());
                        move |view, event: &gpui::MouseDownEvent, _, cx| {
                            view.dismiss_menu_on_outside(kind.clone(), event.position, cx)
                        }
                    }))
                    .child(
                        MenuRow::new(SharedString::from(format!("fork-{}", entry.event_id)))
                            .label("Fork")
                            .disabled(!can_fork)
                            .when(can_fork, |row| {
                                row.on_click(cx.listener(move |view, _event, _window, cx| {
                                    view.close_open_menu(cx);
                                    view.on_fork(&fork_id, cx);
                                }))
                            }),
                    ),
            );
        }
        div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap_2()
            .child(div().flex_1().child(body))
            .child(actions)
    }

    pub(super) fn on_fork(&mut self, event_id: &str, cx: &mut Context<Self>) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            self.status_hint = Some("Open a session before forking.".into());
            cx.notify();
            return;
        };
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.status_hint = Some("Fork needs a live connection.".into());
            cx.notify();
            return;
        }
        // 入口级防线：渲染层已按边界禁用 Fork，这里再按 reducer 的单点判型
        // 复核——connected + active session + run 终止边界缺一不可。
        let forkable = self
            .projection
            .timeline
            .iter()
            .any(|entry| entry.event_id == event_id && entry.is_fork_boundary());
        if !forkable {
            self.status_hint = Some(
                "Fork is only available on a finished run (completed, cancelled, or failed)."
                    .into(),
            );
            cx.notify();
            return;
        }
        self.controller
            .fork_session(session_id, event_id.to_string());
        cx.notify();
    }
}
