//! Inspector 侧面板（R6 Wave A）：顶层 Changes / Terminal / Resources 固定三页签
//!（默认 Changes，58px 页签条 + accent 下划线）
//! + 各页内容。终端面板滚动维持 ScrollHandle（FollowScroll）现状，不随
//! Timeline 改 list()；各页签滚动状态独立保留（design/README.md §8.5）。

use gpui::{div, prelude::*, px, Context, FocusHandle, Window};

use crate::projection::{ConnectionState, TerminalState, TERMINAL_CWD_UNKNOWN};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::panel::Panel;
use crate::ui::theme::{dark, font, metrics};

use super::{terminal_can_operate, terminal_known_exited, terminal_start_enabled, AppView};

/// Terminal 页无输出时的占位文案（R2 Wave B）：视觉与 AX 树共用同源。
pub(super) const TERMINAL_EMPTY_OUTPUT: &str = "Terminal output will appear here.";

/// 尺寸 stepper 的步长与安全边界（列 / 行）：步长取常用适配的可感增量，
/// 边界防止把 PTY 缩到不可用或放大到离谱值。
pub(crate) const TERMINAL_COLUMNS_STEP: i32 = 8;
pub(crate) const TERMINAL_ROWS_STEP: i32 = 4;
const TERMINAL_COLUMNS_MIN: u16 = 20;
const TERMINAL_COLUMNS_MAX: u16 = 500;
const TERMINAL_ROWS_MIN: u16 = 6;
const TERMINAL_ROWS_MAX: u16 = 200;

/// 纯函数：按增量调整终端尺寸并钳制在安全边界内（G1，可测）。
pub(crate) fn step_terminal_size(columns: u16, rows: u16, dcolumns: i32, drows: i32) -> (u16, u16) {
    let columns = (i32::from(columns) + dcolumns).clamp(
        i32::from(TERMINAL_COLUMNS_MIN),
        i32::from(TERMINAL_COLUMNS_MAX),
    ) as u16;
    let rows = (i32::from(rows) + drows)
        .clamp(i32::from(TERMINAL_ROWS_MIN), i32::from(TERMINAL_ROWS_MAX)) as u16;
    (columns, rows)
}

/// 可见尺寸与 AX 共用同一来源：有本地 stepper 草稿时展示草稿，否则展示
/// Host 已确认尺寸；草稿只影响展示，Apply 后才下发 Host。
pub(crate) fn terminal_size_for_display(
    terminal: &TerminalState,
    draft: Option<(u16, u16)>,
) -> (u16, u16) {
    draft.unwrap_or((terminal.columns, terminal.rows))
}

/// 尺寸状态只描述当前展示值：草稿与 Host 权威尺寸不同时不得沿用旧回执
/// 宣称 confirmed；一致时才可展示最近一次 resize 确认。
pub(crate) fn terminal_resize_status_label(
    terminal: &TerminalState,
    draft: Option<(u16, u16)>,
) -> Option<&'static str> {
    if draft.is_some_and(|size| size != (terminal.columns, terminal.rows)) {
        Some("size not applied")
    } else if terminal.resize_confirmed {
        Some("resize confirmed")
    } else {
        None
    }
}

/// Terminal 面板当前是纯文本视图，不是 VT emulator。显示前移除 ANSI/VT
/// 控制序列，避免把 bracketed-paste 等终端状态字节直接暴露给用户。
pub(crate) fn plain_terminal_output(raw: &str) -> String {
    let mut plain = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            plain.push(ch);
            continue;
        }

        match chars.next() {
            Some('[') => {
                for sequence_char in chars.by_ref() {
                    if ('@'..='~').contains(&sequence_char) {
                        break;
                    }
                }
            }
            Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
                let mut saw_escape = false;
                for sequence_char in chars.by_ref() {
                    if sequence_char == '\u{7}' || (saw_escape && sequence_char == '\\') {
                        break;
                    }
                    saw_escape = sequence_char == '\u{1b}';
                }
            }
            Some(_) | None => {}
        }
    }

    plain
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

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

/// 尺寸 stepper 按钮（G1）：Ghost、无 padding、键盘路径与可见路径同 gate
///（disabled 由调用方传入；adjust_terminal_size 内仍复核最终 gate）。
fn terminal_stepper(
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
    dcolumns: i32,
    drows: i32,
    focus: &FocusHandle,
    enabled: bool,
    cx: &mut Context<AppView>,
) -> Button {
    Button::new(id)
        .variant(ButtonVariant::Ghost)
        .disabled(!enabled)
        .padding(ButtonPadding::None)
        .text_size(font::XS)
        .label(label)
        .tooltip(tooltip)
        .track_focus(focus)
        .on_click(cx.listener(move |view, event, _window, cx| {
            if view.consume_button_key_click(id, event) {
                return;
            }
            view.adjust_terminal_size(dcolumns, drows, cx);
        }))
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
                    .text_size(font::BODY)
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
            plain_terminal_output(&terminal.output)
        };
        let (columns, rows) = terminal_size_for_display(terminal, self.terminal_size_draft);
        let size_label = format!("{columns}×{rows}");
        let cwd = terminal.cwd.clone();
        let started = terminal.session_id.is_some();
        let apply_size = started && !terminal_known_exited(terminal);
        let owner = terminal.workspace_id.as_deref().unwrap_or("unassigned");
        let mut state_label = terminal.availability_label();
        if terminal.dropped_events > 0 {
            state_label.push_str(&format!(
                " · {} output events dropped",
                terminal.dropped_events
            ));
        }
        if let Some(resize_status) =
            terminal_resize_status_label(terminal, self.terminal_size_draft)
        {
            state_label.push_str(&format!(" · {resize_status}"));
        }
        let terminal_operable = terminal_can_operate(&self.projection.connection, terminal);
        let terminal_start_enabled = terminal_start_enabled(
            &self.projection.connection,
            terminal,
            self.terminal_pending_create_workspace.as_ref(),
            self.terminal_pending_resize.is_some(),
        );
        let terminal_resize_enabled = terminal_operable && self.terminal_pending_resize.is_none();
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
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .child(format!("workspace {owner} · cwd {cwd} · {state_label}")),
                    )
                    .child(
                        // G1：尺寸可变参。stepper 维护本地草稿，右侧尺寸按钮
                        // 仍是唯一 apply 入口，走冻结的 terminal_resize。
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .flex_none()
                            .child(terminal_stepper(
                                "terminal-cols-dec",
                                "−W",
                                "Fewer columns",
                                -TERMINAL_COLUMNS_STEP,
                                0,
                                &self.terminal_cols_dec_focus,
                                terminal_operable,
                                cx,
                            ))
                            .child(terminal_stepper(
                                "terminal-cols-inc",
                                "+W",
                                "More columns",
                                TERMINAL_COLUMNS_STEP,
                                0,
                                &self.terminal_cols_inc_focus,
                                terminal_operable,
                                cx,
                            ))
                            .child(
                                Button::new("terminal-resize")
                                    .variant(ButtonVariant::Ghost)
                                    .disabled(!terminal_resize_enabled)
                                    .padding(ButtonPadding::None)
                                    .label(size_label)
                                    .tooltip("Apply terminal size")
                                    .track_focus(&self.terminal_resize_focus)
                                    .on_click(cx.listener(|view, event, window, cx| {
                                        if view.consume_button_key_click("terminal-resize", event) {
                                            return;
                                        }
                                        view.on_apply_terminal_size(window, cx);
                                    })),
                            )
                            .child(terminal_stepper(
                                "terminal-rows-dec",
                                "−H",
                                "Fewer rows",
                                0,
                                -TERMINAL_ROWS_STEP,
                                &self.terminal_rows_dec_focus,
                                terminal_operable,
                                cx,
                            ))
                            .child(terminal_stepper(
                                "terminal-rows-inc",
                                "+H",
                                "More rows",
                                0,
                                TERMINAL_ROWS_STEP,
                                &self.terminal_rows_inc_focus,
                                terminal_operable,
                                cx,
                            )),
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
                            .text_size(font::SM)
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
                            // 已知 exited/killed：单槽变「New」——新建终端入
                            // 口，不伪造旧终端生命周期（G2）。
                            .label(if apply_size {
                                "Size"
                            } else if terminal_known_exited(terminal) {
                                "New"
                            } else {
                                "Start"
                            })
                            .track_focus(&self.terminal_start_focus)
                            .on_click(cx.listener(move |view, event, window, cx| {
                                if view.consume_button_key_click("terminal-start", event) {
                                    return;
                                }
                                if apply_size {
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
        if self.terminal_pending_resize.is_some() {
            self.status_hint = Some("Waiting for the current terminal resize.".into());
            cx.notify();
            return;
        }
        if let Some(id) = self.projection.terminal.session_id.clone() {
            let (columns, rows) = self.terminal_size_draft.unwrap_or((
                self.projection.terminal.columns,
                self.projection.terminal.rows,
            ));
            self.terminal_pending_resize = Some((id.clone(), columns, rows));
            self.controller.terminal_resize(id, columns, rows);
            self.status_hint = Some("Resizing terminal…".into());
        }
        cx.notify();
    }

    /// stepper 调整本地尺寸草稿（G1）：Host 权威 columns/rows 不动，apply
    /// 时才经 terminal_resize 下发；草稿在回执或终端切换后复位。
    pub(super) fn adjust_terminal_size(
        &mut self,
        dcolumns: i32,
        drows: i32,
        cx: &mut Context<Self>,
    ) {
        if !terminal_can_operate(&self.projection.connection, &self.projection.terminal) {
            return;
        }
        let current = (
            self.projection.terminal.columns,
            self.projection.terminal.rows,
        );
        let (columns, rows) = self.terminal_size_draft.unwrap_or(current);
        self.terminal_size_draft = Some(step_terminal_size(columns, rows, dcolumns, drows));
        cx.notify();
    }

    /// 终端会话懒创建（键盘输入路径 ui/mod.rs 亦调用）。
    pub(super) fn ensure_terminal(&mut self, _cx: &mut Context<Self>) {
        let terminal = self.projection.terminal.clone();
        if terminal.session_id.is_some() {
            if !terminal_known_exited(&terminal) {
                return;
            }
            // G2：已知 exited/killed 的终端，Start 恢复为「新建终端」入口——
            // 旧终端只读保留（wire 无 stop/close 与 live exit 事件，不伪造
            // 生命周期），新终端沿用同一 workspace 与 cwd。
            let workspace = terminal
                .workspace_id
                .clone()
                .or_else(|| self.inspector_workspace_id());
            let cwd = (terminal.cwd != TERMINAL_CWD_UNKNOWN)
                .then(|| terminal.cwd.clone())
                .filter(|cwd| cwd.as_str() != ".");
            self.begin_terminal_create(workspace, cwd);
            return;
        }
        self.begin_terminal_create(
            self.inspector_workspace_id(),
            Some(self.projection.terminal.cwd.clone()),
        );
    }

    /// terminal_create 的公共发起入口：create-pending 去重、连接与
    /// workspace 守卫、请求 cwd 记账集中在一处（首次 Start 与 exited 终端
    /// 的新建入口共用）。
    fn begin_terminal_create(&mut self, workspace: Option<String>, cwd: Option<String>) {
        if self.terminal_pending_create_workspace.is_some() {
            self.status_hint = Some("Waiting for the current terminal creation.".into());
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
        // 与 workspace 槽同生命周期：失败/断连清理后不得把上一次请求
        // 的 cwd 误贴到下一次新建（无条件覆盖，None 即清除）。
        self.terminal_pending_create_cwd = cwd.clone();
        self.controller.terminal_create(workspace, cwd);
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

    #[test]
    fn terminal_output_hides_vt_control_sequences() {
        assert_eq!(
            plain_terminal_output("\u{1b}[?2004hpwd\u{1b}[?2004l\r\n/workspace\r\n"),
            "pwd\n/workspace"
        );
    }

    /// G1：尺寸 stepper 只在安全边界内变参（钳制，不产生 0 列 / 离谱值）。
    #[test]
    fn terminal_size_step_clamps_to_sane_bounds() {
        assert_eq!(
            step_terminal_size(80, 24, TERMINAL_COLUMNS_STEP, TERMINAL_ROWS_STEP),
            (88, 28)
        );
        assert_eq!(
            step_terminal_size(24, 10, -1000, -1000),
            (TERMINAL_COLUMNS_MIN, TERMINAL_ROWS_MIN)
        );
        assert_eq!(
            step_terminal_size(496, 198, 1000, 1000),
            (TERMINAL_COLUMNS_MAX, TERMINAL_ROWS_MAX)
        );
        let terminal = crate::projection::TerminalState {
            columns: 80,
            rows: 24,
            ..crate::projection::TerminalState::default()
        };
        assert_eq!(terminal_size_for_display(&terminal, None), (80, 24));
        assert_eq!(
            terminal_size_for_display(&terminal, Some((88, 28))),
            (88, 28)
        );
        assert_eq!(
            terminal_resize_status_label(&terminal, Some((88, 28))),
            Some("size not applied")
        );
        let confirmed = crate::projection::TerminalState {
            resize_confirmed: true,
            ..terminal
        };
        assert_eq!(
            terminal_resize_status_label(&confirmed, None),
            Some("resize confirmed")
        );
    }
}
