//! Inspector 侧面板（R6 Wave A）：顶层 Changes / Terminal / Resources 固定三页签
//!（默认 Changes，58px 页签条 + accent 下划线）
//! + 各页内容。终端面板滚动维持 ScrollHandle（FollowScroll）现状，不随
//! Timeline 改 list()；各页签滚动状态独立保留（design/README.md §8.5）。

use gpui::{div, prelude::*, px, Context, Window};

use crate::projection::ConnectionState;
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::panel::Panel;
use crate::ui::theme::{dark, font, metrics};

use super::{terminal_can_operate, terminal_start_enabled, AppView};

/// Terminal 页无输出时的占位文案（R2 Wave B）：视觉与 AX 树共用同源。
pub(super) const TERMINAL_EMPTY_OUTPUT: &str = "Terminal output will appear here.";

/// Inspector 顶层页签（固定三页；R6 Wave A 起默认 Changes）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum InspectorTab {
    #[default]
    Changes,
    Terminal,
    Resources,
}

impl InspectorTab {
    fn label(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::Terminal => "Terminal",
            Self::Resources => "Resources",
        }
    }

    pub(super) fn button_id(self) -> &'static str {
        match self {
            Self::Changes => "inspector-tab-changes",
            Self::Terminal => "inspector-tab-terminal",
            Self::Resources => "inspector-tab-resources",
        }
    }
}

impl AppView {
    pub(super) fn inspector_element(&self, connected: bool, cx: &mut Context<Self>) -> Panel {
        let current = self.inspector_tab;
        let mut tabs = div().flex().flex_row().items_center().gap_1();
        for tab in [
            InspectorTab::Changes,
            InspectorTab::Terminal,
            InspectorTab::Resources,
        ] {
            let selected = tab == current;
            let hover = dark().surface.raised;
            tabs = tabs.child(
                // R6 Wave A：页签不再是 Raised/Ghost 按钮，改为 58px 条内
                // 文本级页签，选中态 accent 下划线（与二级 56px 层次区分）；
                // hover / active 只改背景，active 复用 hover 色（基准 §8.1）。
                div()
                    .id(tab.button_id())
                    .relative()
                    .w(px(metrics::INSPECTOR_TAB_WIDTH))
                    .h(px(metrics::INSPECTOR_TAB_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tab_stop(true)
                    .track_focus(&self.inspector_tab_focus[tab as usize])
                    .focus(|style| style.border_1().border_color(dark().accent.primary))
                    .text_size(px(font::BODY))
                    .text_color(if selected {
                        dark().text.primary
                    } else {
                        dark().text.secondary
                    })
                    .hover(move |style| style.bg(hover))
                    .active(move |style| style.bg(hover))
                    .child(div().child(tab.label()))
                    .when(selected, |tab| {
                        tab.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .w_full()
                                .h(px(metrics::TAB_UNDERLINE_HEIGHT))
                                .bg(dark().accent.primary),
                        )
                    })
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        if view.consume_button_key_click(tab.button_id(), event) {
                            return;
                        }
                        view.select_inspector_tab(tab, cx);
                    })),
            );
        }
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pl_3()
            .pr_2()
            .h(px(metrics::INSPECTOR_TAB_HEIGHT))
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(tabs)
            .child(
                Button::new("inspector-collapse")
                    .variant(ButtonVariant::Ghost)
                    .text_size(font::SM)
                    .text_color(dark().text.secondary)
                    .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                    .label("⟩")
                    .track_focus(&self.inspector_collapse_focus)
                    .on_click(cx.listener(|view, event, window, cx| {
                        if view.consume_button_key_click("inspector-collapse", event) {
                            return;
                        }
                        view.on_toggle_inspector(window, cx);
                    })),
            );
        Panel::side_left(px(metrics::INSPECTOR_WIDTH))
            .child(header)
            .child(match current {
                InspectorTab::Changes => self.changes_element(cx).into_any_element(),
                InspectorTab::Terminal => {
                    self.terminal_page_element(connected, cx).into_any_element()
                }
                InspectorTab::Resources => self.resources_element(cx).into_any_element(),
            })
    }

    /// Terminal 页（波 C 的面板内容，页签头外移到顶层 strip 后保持原样）。
    fn terminal_page_element(&self, _connected: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal = &self.projection.terminal;
        let output = if terminal.output.is_empty() {
            TERMINAL_EMPTY_OUTPUT.to_string()
        } else {
            terminal.output.clone()
        };
        let size_label = format!("{}×{}", terminal.columns, terminal.rows);
        let cwd = terminal.cwd.clone();
        let started = terminal.session_id.is_some();
        let owner = terminal.workspace_id.as_deref().unwrap_or("unassigned");
        let mut state_label = terminal.availability_label();
        if terminal.dropped_events > 0 {
            state_label.push_str(&format!(
                " · {} output events dropped",
                terminal.dropped_events
            ));
        }
        if terminal.resize_confirmed {
            state_label.push_str(" · resize confirmed");
        }
        let terminal_operable = terminal_can_operate(&self.projection.connection, terminal);
        let terminal_start_enabled = terminal_start_enabled(
            &self.projection.connection,
            terminal,
            self.terminal_pending_create_workspace.as_ref(),
        );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .text_size(px(font::XS))
                    .text_color(dark().text.secondary)
                    .child(format!("workspace {owner} · cwd {cwd} · {state_label}"))
                    .child(
                        Button::new("terminal-resize")
                            .variant(ButtonVariant::Ghost)
                            .disabled(!terminal_operable)
                            .padding(ButtonPadding::None)
                            .label(size_label)
                            .track_focus(&self.terminal_resize_focus)
                            .on_click(cx.listener(|view, event, window, cx| {
                                if view.consume_button_key_click("terminal-resize", event) {
                                    return;
                                }
                                view.on_apply_terminal_size(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("terminal-output-area")
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(
                        div()
                            .id("terminal-output")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .track_scroll(self.terminal_scroll.handle())
                            .overflow_y_scroll()
                            .px_2()
                            .py_1()
                            .text_size(px(font::SM))
                            .text_color(dark().text.emphasis)
                            .on_scroll_wheel(cx.listener(|view, _event, _window, cx| {
                                view.terminal_scroll.on_scroll_wheel();
                                cx.notify();
                            }))
                            .child(output),
                    )
                    .when(!self.terminal_scroll.is_following(), |area| {
                        area.child(BackToBottom::new(
                            Button::new("terminal-back-to-bottom")
                                .variant(ButtonVariant::Raised)
                                .label("↓ 回到底部")
                                .track_focus(&self.terminal_back_to_bottom_focus)
                                .on_click(cx.listener(|view, event, _window, cx| {
                                    if view
                                        .consume_button_key_click("terminal-back-to-bottom", event)
                                    {
                                        return;
                                    }
                                    view.terminal_scroll.jump_to_bottom();
                                    cx.notify();
                                })),
                        ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .p_2()
                    .border_t_1()
                    .border_color(dark().border.subtle)
                    .child(div().flex_1().child(self.terminal_input.clone()))
                    .child(
                        // 迁移前 terminal-start 未设文字色（继承 text.primary），
                        // 禁用态亦保持同色，显式钉住避免 Raised 默认的 disabled 色。
                        Button::new("terminal-start")
                            .variant(ButtonVariant::Raised)
                            .disabled(!terminal_start_enabled)
                            .text_size(font::XS)
                            .text_color(dark().text.primary)
                            .disabled_text_color(dark().text.primary)
                            .label(if started { "Size" } else { "Start" })
                            .track_focus(&self.terminal_start_focus)
                            .on_click(cx.listener(move |view, event, window, cx| {
                                if view.consume_button_key_click("terminal-start", event) {
                                    return;
                                }
                                if started {
                                    view.on_apply_terminal_size(window, cx);
                                } else {
                                    view.on_start_terminal(window, cx);
                                }
                            })),
                    ),
            )
    }

    pub(super) fn on_start_terminal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_terminal(cx);
        cx.notify();
    }

    pub(super) fn on_apply_terminal_size(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !terminal_can_operate(&self.projection.connection, &self.projection.terminal) {
            self.status_hint =
                Some("Terminal is not ready; resize was not sent to the host.".into());
            cx.notify();
            return;
        }
        if let Some(id) = self.projection.terminal.session_id.clone() {
            self.controller.terminal_resize(
                id,
                self.projection.terminal.columns,
                self.projection.terminal.rows,
            );
        }
        cx.notify();
    }

    /// 终端会话懒创建（键盘输入路径 ui/mod.rs 亦调用）。
    pub(super) fn ensure_terminal(&mut self, _cx: &mut Context<Self>) {
        if self.projection.terminal.session_id.is_some() {
            return;
        }
        let workspace = self.inspector_workspace_id();
        if let Some(pending) = self.terminal_pending_create_workspace.as_ref() {
            if workspace.as_ref() == Some(pending) {
                self.status_hint = Some("Starting terminal…".into());
            } else {
                self.status_hint = Some("Waiting for the current terminal creation.".into());
            }
            return;
        }
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.status_hint = Some("Terminal needs a live connection.".into());
            return;
        }
        let Some(workspace) = workspace else {
            self.status_hint = Some("Choose a project before opening Terminal.".into());
            return;
        };
        self.terminal_pending_create_workspace = Some(workspace.clone());
        self.controller
            .terminal_create(workspace, Some(self.projection.terminal.cwd.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R6 Wave A：顶层页签默认 Changes（折叠态 Activity 摘要展开后落点
    /// 即默认页）。
    #[test]
    fn inspector_tab_defaults_to_changes() {
        assert_eq!(InspectorTab::default(), InspectorTab::Changes);
    }
}
