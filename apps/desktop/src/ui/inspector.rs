//! Inspector 工作面板（GUI2-05）：面板头为名称选择器（Changes /
//! Terminal / Resources）+ 关闭按钮（`inspector-collapse` 36×36）；
//! Changes 内保留 Files / Summary 二级页签。终端面板滚动维持
//! ScrollHandle（FollowScroll），不随 Timeline 改 list()；各页滚动状态
//! 独立保留。宽窗 440px 侧栏；窄窗显式打开时同一实体占用中央 Workspace。

use gpui::{canvas, div, prelude::*, px, Context, Focusable, MouseDownEvent, Window};

use crate::projection::{ConnectionState, TERMINAL_CWD_UNKNOWN};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::icon::{icon, icon_sized, Icon};
use crate::ui::components::panel::Panel;
use crate::ui::i18n::t;
use crate::ui::shell_layout::InspectorPlacement;
use crate::ui::theme::{dark, font, metrics};

use super::{terminal_can_close, terminal_can_operate, AppView, MenuKind};

pub(crate) use super::terminal_view::{render_terminal_lines, terminal_text_size};

pub(crate) const TERMINAL_TAB_BAR_HEIGHT: f32 = 28.0;
const TERMINAL_HEADER_PAD_X_REMS: f32 = 0.5;

/// Inspector 顶层面板（固定三页；默认 Changes）。名称选择器列出已接通的
/// 三项；identifier `inspector-tab-*` 现为菜单项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum InspectorTab {
    #[default]
    Changes,
    Terminal,
    Resources,
}

impl InspectorTab {
    pub(super) const ALL: [Self; 3] = [Self::Changes, Self::Terminal, Self::Resources];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Changes => t("inspector.tab_changes"),
            Self::Terminal => t("inspector.tab_terminal"),
            Self::Resources => t("inspector.tab_resources"),
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
    pub(super) fn inspector_element(
        &mut self,
        _connected: bool,
        placement: InspectorPlacement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Panel {
        let current = self.inspector_tab;
        let menu_open = matches!(self.open_menu, Some(MenuKind::InspectorPanel));
        let highlight = self.menu_highlight_effective(self.menu_selected_index());
        let trigger = Button::new("inspector-panel")
            .variant(ButtonVariant::Ghost)
            .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
            .height(px(metrics::ICON_BUTTON_SIZE))
            .vcenter()
            .text_size(font::BASE)
            .text_color(dark().text.primary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(metrics::SPACE_2))
                    .child(current.label())
                    .child(icon_sized(Icon::ChevronDown, px(metrics::ICON_SM))),
            )
            .tooltip(t("inspector.panel"))
            .track_focus(&self.inspector_panel_focus)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("inspector-panel", event) {
                    return;
                }
                let down = Self::click_down_position(event);
                view.toggle_menu(MenuKind::InspectorPanel, down, cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                if view.open_menu.is_some() {
                    view.note_button_key_activate("inspector-panel");
                    return;
                }
                view.note_button_key_activate("inspector-panel");
                view.toggle_menu(MenuKind::InspectorPanel, None, cx);
                cx.stop_propagation();
            }));
        let mut dropdown = Dropdown::new(trigger);
        if menu_open {
            let mut panel = MenuPanel::new("inspector-panel-menu").dismiss_on_outside(cx.listener(
                |view, event: &MouseDownEvent, _window, cx| {
                    view.dismiss_menu_on_outside(MenuKind::InspectorPanel, event.position, cx);
                },
            ));
            for (ix, tab) in InspectorTab::ALL.into_iter().enumerate() {
                panel = panel.child(
                    MenuRow::new(tab.button_id())
                        .label(tab.label())
                        .selected(tab == current)
                        .highlighted(ix == highlight)
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.select_inspector_tab(tab, cx);
                            view.close_menu_and_focus_trigger(MenuKind::InspectorPanel, window, cx);
                        })),
                );
            }
            dropdown = dropdown.panel(panel);
        }
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pl_3()
            .pr_2()
            .h(px(metrics::INSPECTOR_TAB_HEIGHT))
            .flex_none()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(
                self.shell_element("inspector-panel-layout")
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_w_0()
                    .child(dropdown),
            )
            .child(
                self.shell_element("inspector-collapse-layout")
                    .flex_none()
                    .child(
                        Button::new("inspector-collapse")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::None)
                            .width(px(metrics::ICON_BUTTON_SIZE))
                            .height(px(metrics::ICON_BUTTON_SIZE))
                            .center()
                            .radius(4.0)
                            .text_color(dark().text.secondary)
                            .child(icon(Icon::Collapse))
                            .tooltip(t("inspector.hide"))
                            .track_focus(&self.inspector_collapse_focus)
                            .on_click(cx.listener(|view, event, window, cx| {
                                if view.consume_button_key_click("inspector-collapse", event) {
                                    return;
                                }
                                view.on_toggle_inspector(window, cx);
                            })),
                    ),
            );
        let approval =
            (placement.is_center() && self.projection.pending_approval.is_some()).then(|| {
                self.shell_element("inspector-approval-hint-layout")
                    .flex_none()
                    .w_full()
                    .child(
                        Button::new("inspector-approval-hint")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                            .text_size(font::SM)
                            .text_color(dark().semantic.warning_text)
                            .label(t("inspector.return_for_approval"))
                            .track_focus(&self.inspector_approval_focus)
                            .on_click(cx.listener(|view, event, window, cx| {
                                if view.consume_button_key_click("inspector-approval-hint", event) {
                                    return;
                                }
                                view.on_inspector_back(window, cx);
                            })),
                    )
            });
        let body = match current {
            InspectorTab::Changes => self.changes_element(window, cx).into_any_element(),
            InspectorTab::Terminal => self.terminal_page_element(window, cx).into_any_element(),
            InspectorTab::Resources => self.resources_element(cx).into_any_element(),
        };
        let mut panel = if placement.is_center() {
            Panel::fill()
        } else {
            Panel::side_left(px(metrics::INSPECTOR_WIDTH))
        };
        panel = panel.child(header);
        if let Some(approval) = approval {
            panel = panel.child(approval);
        }
        panel.child(body)
    }

    /// 目录与状态供辅助功能描述使用，不占用终端正文。
    pub(super) fn terminal_context_text(&self) -> String {
        let terminal = &self.projection.terminal;
        let owner = terminal
            .workspace_id
            .as_deref()
            .and_then(|id| {
                self.projection
                    .workspaces
                    .iter()
                    .find(|w| w.id == id)
                    .map(|w| w.name.as_str())
            })
            .or(terminal.workspace_id.as_deref())
            .unwrap_or(t("taskrail.unassigned"));
        let mut context = format!(
            "{} · {owner}\n{} · {}",
            t("inspector.project"),
            t("changes.work_dir"),
            terminal.cwd
        );
        if self.terminal_notice_text().is_none() {
            context.push_str(&format!(" · {}", terminal.availability_label()));
        }
        if terminal.dropped_events > 0 {
            context.push_str(&format!(
                " · {}",
                t("inspector.output_dropped").replace("{}", &terminal.dropped_events.to_string())
            ));
        }
        context
    }

    /// Terminal 页：面板头外的内容区；PTY 生命周期与 `terminal_*` 命令流
    /// 不随装配位置（侧栏 / 中央）改变。
    fn terminal_page_element(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let terminal = self.projection.terminal.clone();
        let notice = self.terminal_notice_text();
        let screen = super::terminal_view::terminal_screen(
            &terminal.output,
            terminal.columns,
            terminal.rows,
            window,
        );
        let input = self.terminal_input.clone();
        let focus = input.read(cx).focus_handle(cx);
        let preedit = input.read(cx).preedit().to_owned();
        let caret =
            terminal_can_operate(&self.projection.connection, &terminal) && screen.cursor_visible;
        let lines = render_terminal_lines(&screen, &preedit, caret);
        let (cursor_row, cursor_col) = screen.cursor();
        let scale = f32::from(window.rem_size()) / font::BASE_REM_PIXELS;
        let scroll = self.terminal_scroll.handle().clone();
        let tabs = self.terminal_tabs_element(cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(tabs)
            .child(
                div()
                    .id("terminal-output-area")
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.terminal_action_layouts["terminal-output"])
                    .track_focus(&focus)
                    .key_context("Terminal")
                    .cursor_text()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|view, _, window, cx| {
                            window.focus(&view.terminal_input.read(cx).focus_handle(cx));
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child({
                        let view = cx.entity().downgrade();
                        canvas(
                            move |bounds, _, app| {
                                let width = f32::from(bounds.size.width);
                                let height = f32::from(bounds.size.height);
                                if view.upgrade().is_some_and(|entity| {
                                    entity.read(app).terminal_output_size_changed(width, height)
                                }) {
                                    app.defer(move |app| {
                                        let _ = view.update(app, |view, cx| {
                                            view.observe_terminal_output_size(width, height, cx)
                                        });
                                    });
                                }
                            },
                            move |bounds, _, window, app| {
                                let cursor_bounds = gpui::Bounds::new(
                                    bounds.origin
                                        + scroll.offset()
                                        + gpui::point(
                                            px((8.0
                                                + (cursor_col + preedit.chars().count()) as f32
                                                    * pawork_terminal::TERMINAL_CELL_WIDTH)
                                                * scale),
                                            px((4.0
                                                + cursor_row as f32
                                                    * super::terminal_view::TERMINAL_LINE_HEIGHT)
                                                * scale),
                                        ),
                                    gpui::size(
                                        px(pawork_terminal::TERMINAL_CELL_WIDTH * scale),
                                        px(super::terminal_view::TERMINAL_LINE_HEIGHT * scale),
                                    ),
                                );
                                input.update(app, |input, _| {
                                    input.cursor_bounds = Some(cursor_bounds)
                                });
                                window.handle_input(
                                    &focus,
                                    gpui::ElementInputHandler::new(bounds, input.clone()),
                                    app,
                                );
                            },
                        )
                        .absolute()
                        .size_full()
                    })
                    .child(
                        div()
                            .id("terminal-output")
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .track_scroll(self.terminal_scroll.handle())
                            .overflow_y_scroll()
                            .overflow_x_scroll()
                            .px_2()
                            .py_1()
                            .font_family(font::MONO)
                            .text_size(terminal_text_size())
                            .line_height(gpui::rems(
                                super::terminal_view::TERMINAL_LINE_HEIGHT / font::BASE_REM_PIXELS,
                            ))
                            .text_color(dark().text.emphasis)
                            .on_scroll_wheel(cx.listener(|view, _, _, cx| {
                                view.terminal_scroll.on_scroll_wheel();
                                cx.notify();
                            }))
                            .when(notice.is_some(), |area| {
                                area.child(self.terminal_notice_element(cx))
                            })
                            .children(lines.into_iter().map(|(_, styled)| {
                                div()
                                    .flex()
                                    .flex_row()
                                    .w_full()
                                    .flex_none()
                                    .h(px(super::terminal_view::TERMINAL_LINE_HEIGHT * scale))
                                    .whitespace_nowrap()
                                    .child(styled)
                            })),
                    )
                    .when(!self.terminal_scroll.is_following(), |area| {
                        area.child(BackToBottom::new(
                            Button::new("terminal-back-to-bottom")
                                .variant(ButtonVariant::Raised)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(icon_sized(Icon::ArrowDown, px(metrics::ICON_SM)))
                                        .child(t("timeline.ax_back_to_bottom")),
                                )
                                .track_focus(&self.terminal_back_to_bottom_focus)
                                .on_click(cx.listener(|view, event, _, cx| {
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
    }

    fn terminal_tabs_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self.inspector_workspace_id();
        let current = self.projection.terminal.session_id.clone();
        let tabs: Vec<String> = self
            .projection
            .workspace_terminals(workspace.as_deref())
            .into_iter()
            .filter_map(|terminal| terminal.session_id.clone())
            .collect();
        let can_create = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && !self.terminal_create_blocked()
            && self.terminal_pending_create_workspace.is_none();
        let mut row = div()
            .id("terminal-tabs")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px(gpui::rems(TERMINAL_HEADER_PAD_X_REMS))
            .h(px(TERMINAL_TAB_BAR_HEIGHT))
            .min_w_0();
        for (index, id) in tabs.iter().enumerate() {
            let selected = current.as_deref() == Some(id.as_str());
            let focus = self
                .terminal_tab_focus
                .entry(id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let click_id = format!("terminal-tab-{id}");
            let activate_id = click_id.clone();
            let select_id = id.clone();
            let activate_select = id.clone();
            row = row.child(
                Button::new(click_id.clone())
                    .track_focus(&focus)
                    .variant(if selected {
                        ButtonVariant::Raised
                    } else {
                        ButtonVariant::Ghost
                    })
                    .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                    .height(px(TERMINAL_TAB_BAR_HEIGHT - 4.0))
                    .text_size(font::XS)
                    .label(format!("{} {}", t("inspector.tab_terminal"), index + 1))
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        if view.consume_button_key_click(&click_id, event) {
                            return;
                        }
                        view.on_select_terminal_tab(&select_id, cx);
                    }))
                    .on_activate(cx.listener(move |view, _event, _window, cx| {
                        view.note_button_key_activate(&activate_id);
                        view.on_select_terminal_tab(&activate_select, cx);
                        cx.stop_propagation();
                    })),
            );
        }
        row = row.child(
            Button::new("terminal-new-tab")
                .track_focus(&self.terminal_new_tab_focus)
                .variant(ButtonVariant::Ghost)
                .disabled(!can_create)
                .padding(ButtonPadding::None)
                .width(px(metrics::RAIL_ICON_BUTTON_SIZE))
                .height(px(TERMINAL_TAB_BAR_HEIGHT - 4.0))
                .center()
                .child(icon(Icon::Plus))
                .tooltip(t("inspector.terminal_new_tab"))
                .on_click(cx.listener(|view, event, _window, cx| {
                    if view.consume_button_key_click("terminal-new-tab", event) {
                        return;
                    }
                    view.on_new_terminal_tab(cx);
                }))
                .on_activate(cx.listener(|view, _event, _window, cx| {
                    view.note_button_key_activate("terminal-new-tab");
                    view.on_new_terminal_tab(cx);
                    cx.stop_propagation();
                })),
        );
        if terminal_can_close(&self.projection.connection, &self.projection.terminal) {
            row = row.child(
                div()
                    .id("terminal-close-layout")
                    .track_scroll(&self.terminal_action_layouts["terminal-close"])
                    .child(
                        Button::new("terminal-close")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::None)
                            .width(px(metrics::RAIL_ICON_BUTTON_SIZE))
                            .height(px(TERMINAL_TAB_BAR_HEIGHT - 4.0))
                            .center()
                            .label("×")
                            .tooltip(t("inspector.close"))
                            .disabled(self.terminal_pending_close.is_some())
                            .track_focus(&self.terminal_close_focus)
                            .on_click(cx.listener(|view, event, window, cx| {
                                if view.consume_button_key_click("terminal-close", event) {
                                    return;
                                }
                                view.on_close_terminal(window, cx);
                            })),
                    ),
            );
        }
        row
    }

    pub(super) fn on_select_terminal_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.projection.terminal.session_id.as_deref() == Some(id) {
            return;
        }
        if !self.projection.select_terminal(id) {
            return;
        }
        self.reconcile_terminal_input(cx);
        self.pending_inspector_focus = Some(super::InspectorFocusTarget::SelectedTab);
        self.maybe_apply_fitted_terminal_size(cx);
        self.terminal_scroll.jump_to_bottom();
        cx.notify();
    }

    pub(super) fn on_new_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let workspace = self.inspector_workspace_id();
        let cwd = {
            let terminal = &self.projection.terminal;
            (terminal.cwd != TERMINAL_CWD_UNKNOWN)
                .then(|| terminal.cwd.clone())
                .filter(|cwd| cwd.as_str() != ".")
        };
        self.begin_terminal_create(workspace, cwd);
        cx.notify();
    }

    /// 关闭当前标签：Host 确认终止/清理后才移除本地条目。
    pub(super) fn on_close_terminal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_pending_close.is_some() {
            return;
        }
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            cx.notify();
            return;
        }
        let Some(id) = self.projection.terminal.session_id.clone() else {
            return;
        };
        if !terminal_can_close(&self.projection.connection, &self.projection.terminal) {
            return;
        }
        self.terminal_pending_close = Some(id.clone());
        self.controller.terminal_close(id);
        cx.notify();
    }

    /// 首次打开自动创建；已有终端（包括终态）保留，显式 + 才新建。
    pub(super) fn ensure_terminal(&mut self, _cx: &mut Context<Self>) {
        if self.projection.terminal.session_id.is_none() {
            self.begin_terminal_create(
                self.inspector_workspace_id(),
                Some(self.projection.terminal.cwd.clone()),
            );
        }
    }

    /// terminal_create 的公共发起入口：create-pending 去重、连接与
    /// workspace 守卫、请求 cwd 记账集中在一处（自动启动与 + 共用）。
    fn begin_terminal_create(&mut self, workspace: Option<String>, cwd: Option<String>) {
        if self.terminal_create_blocked() {
            return;
        }
        if self.terminal_pending_create_workspace.is_some() {
            return;
        }
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            return;
        }
        let Some(workspace) = workspace else {
            return;
        };
        self.terminal_details_open = None;
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
            pawork_terminal::plain_output("\u{1b}[?2004hpwd\u{1b}[?2004l\r\n/workspace\r\n"),
            "pwd\n/workspace"
        );
    }
}
