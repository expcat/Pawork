//! Inspector 侧面板（R8 波 D）：顶层 Changes / Terminal / Resources 固定三页签
//! + 各页内容。终端面板滚动维持 ScrollHandle（FollowScroll）现状，不随
//! Timeline 改 list()；各页签滚动状态独立保留（design/README.md §8.5）。

use gpui::{div, prelude::*, px, Context, Window};

use crate::projection::ConnectionState;
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::panel::Panel;
use crate::ui::theme::{dark, font, metrics};

use super::AppView;

/// Terminal 页无输出时的占位文案（R2 Wave B）：视觉与 AX 树共用同源。
pub(super) const TERMINAL_EMPTY_OUTPUT: &str = "Terminal output will appear here.";

/// Inspector 顶层页签（固定三页；默认 Terminal 保持波 C 前的单页行为）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum InspectorTab {
    Changes,
    #[default]
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

    fn button_id(self) -> &'static str {
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
            tabs = tabs.child(
                Button::new(tab.button_id())
                    .variant(if selected {
                        ButtonVariant::Raised
                    } else {
                        ButtonVariant::Ghost
                    })
                    .text_size(font::SM)
                    .text_color(if selected {
                        dark().text.primary
                    } else {
                        dark().text.secondary
                    })
                    .label(tab.label())
                    .on_click(cx.listener(move |view, _event, _window, cx| {
                        view.select_inspector_tab(tab, cx);
                    })),
            );
        }
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
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
                    .on_click(cx.listener(|view, _event, window, cx| {
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
    fn terminal_page_element(&self, connected: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal = &self.projection.terminal;
        let output = if terminal.output.is_empty() {
            TERMINAL_EMPTY_OUTPUT.to_string()
        } else {
            terminal.output.clone()
        };
        let size_label = format!("{}×{}", terminal.columns, terminal.rows);
        let cwd = terminal.cwd.clone();
        let started = terminal.session_id.is_some();
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
                    .child(format!("cwd {cwd}"))
                    .child(
                        Button::new("terminal-resize")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::None)
                            .label(size_label)
                            .on_click(cx.listener(|view, _event, window, cx| {
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
                                .on_click(cx.listener(|view, _event, _window, cx| {
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
                            .disabled(!connected)
                            .text_size(font::XS)
                            .text_color(dark().text.primary)
                            .disabled_text_color(dark().text.primary)
                            .label(if started { "Size" } else { "Start" })
                            .on_click(cx.listener(move |view, _event, window, cx| {
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
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.status_hint = Some("Terminal needs a live connection.".into());
            return;
        }
        let Some(workspace) = self
            .scope_workspace_id
            .clone()
            .or_else(|| self.projection.workspace_id.clone())
        else {
            self.status_hint = Some("Choose a project before opening Terminal.".into());
            return;
        };
        self.controller
            .terminal_create(workspace, Some(self.projection.terminal.cwd.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 波 C 前的唯一页是 Terminal：顶层页签默认仍为 Terminal（cmd-i 展开
    /// 行为与旧单页面板连续）。
    #[test]
    fn inspector_tab_defaults_to_terminal() {
        assert_eq!(InspectorTab::default(), InspectorTab::Terminal);
    }
}
