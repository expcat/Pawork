//! Inspector 工作面板：统一工具 / PTY 标签栏、打开工具菜单与收起动作；
//! 无标签时显示工具入口。关闭 PTY 标签仍等待 Host 确认；
//! Changes 内保留 Files / Summary 二级页签。终端面板滚动维持
//! ScrollHandle（FollowScroll），不随 Timeline 改 list()；各页滚动状态
//! 独立保留。宽窗 440px 侧栏；窄窗显式打开时同一实体占用中央 Workspace。

use gpui::{Context, Focusable, MouseDownEvent, Window, canvas, div, prelude::*, px};

use crate::projection::{ConnectionState, TERMINAL_CWD_UNKNOWN};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::icon::{Icon, icon, icon_sized};
use crate::ui::components::panel::Panel;
use crate::ui::i18n::t;
use crate::ui::shell_layout::InspectorPlacement;
use crate::ui::theme::{dark, font, metrics};

use super::{AppView, MenuKind, terminal_can_close, terminal_can_operate};

pub(crate) use super::terminal_view::{render_terminal_lines, terminal_text_size};

pub(crate) const PANEL_TAB_WIDTH: f32 = 144.0;

/// 默认 Home 工具入口；ALL 仅列已接通的工具。
/// `inspector-tab-*` 用于 Home 工具入口（open 语义），`inspector-menu-*`
/// 用于「+」菜单（launch 语义），已打开页使用独立 identifier。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum InspectorTab {
    Changes,
    Terminal,
    Resources,
    Browser,
    #[default]
    Home,
}

impl InspectorTab {
    pub(super) const ALL: [Self; 4] = [
        Self::Changes,
        Self::Terminal,
        Self::Resources,
        Self::Browser,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Home => t("inspector.panel"),
            Self::Changes => t("inspector.tab_changes"),
            Self::Terminal => t("inspector.tab_terminal"),
            Self::Resources => t("inspector.tab_resources"),
            Self::Browser => t("inspector.tab_browser"),
        }
    }

    pub(super) fn button_id(self) -> &'static str {
        match self {
            Self::Home => "inspector-home",
            Self::Changes => "inspector-tab-changes",
            Self::Terminal => "inspector-tab-terminal",
            Self::Resources => "inspector-tab-resources",
            Self::Browser => "inspector-tab-browser",
        }
    }

    pub(super) fn menu_id(self) -> &'static str {
        match self {
            Self::Home => "inspector-menu-home",
            Self::Changes => "inspector-menu-changes",
            Self::Terminal => "inspector-menu-terminal",
            Self::Resources => "inspector-menu-resources",
            Self::Browser => "inspector-menu-browser",
        }
    }
}

/// 同一标签栏里的工具页或真实 PTY；渲染与 AX 共用这份列表。
#[derive(Clone)]
pub(super) struct PanelTab {
    pub id: String,
    pub label: String,
    pub tool: InspectorTab,
    pub terminal_id: Option<String>,
    pub selected: bool,
    pub close_enabled: bool,
}

impl PanelTab {
    pub fn close_id(&self) -> String {
        if self.selected && self.terminal_id.is_some() {
            "terminal-close".into()
        } else {
            format!("panel-close-{}", self.id)
        }
    }
}

impl InspectorTab {
    pub fn icon(self) -> Icon {
        match self {
            Self::Changes => Icon::Changes,
            Self::Terminal => Icon::Terminal,
            Self::Resources => Icon::Resources,
            Self::Browser => Icon::Network,
            Self::Home => Icon::Inspector,
        }
    }

    pub fn hint(self) -> &'static str {
        t(match self {
            Self::Changes => "inspector.changes_hint",
            Self::Terminal => "inspector.terminal_hint",
            Self::Resources => "inspector.resources_hint",
            Self::Browser => "inspector.browser_hint",
            Self::Home => "inspector.home_hint",
        })
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
            .padding(ButtonPadding::None)
            .width(px(metrics::ICON_BUTTON_SIZE))
            .height(px(metrics::ICON_BUTTON_SIZE))
            .center()
            .child(icon(Icon::Plus))
            .tooltip(t("inspector.add_tool"))
            .track_focus(&self.inspector_panel_focus)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("inspector-panel", event) {
                    return;
                }
                view.toggle_menu(
                    MenuKind::InspectorPanel,
                    Self::click_down_position(event),
                    cx,
                );
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("inspector-panel");
                if view.open_menu.is_some() {
                    return;
                }
                view.toggle_menu(MenuKind::InspectorPanel, None, cx);
                cx.stop_propagation();
            }));
        let mut dropdown = Dropdown::new(trigger);
        if menu_open {
            let mut panel = MenuPanel::new("inspector-panel-menu")
                .track_scroll(&self.shell_layouts["inspector-menu-layout"])
                .dismiss_on_outside(cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                    view.dismiss_menu_on_outside(MenuKind::InspectorPanel, event.position, cx);
                }));
            for (ix, tab) in InspectorTab::ALL.into_iter().enumerate() {
                panel = panel.child(
                    MenuRow::new(tab.menu_id())
                        .label(if tab == InspectorTab::Terminal {
                            t("inspector.terminal_new_tab")
                        } else {
                            tab.label()
                        })
                        .selected(tab == current && tab != InspectorTab::Terminal)
                        .highlighted(ix == highlight)
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.launch_inspector_tool(tab, cx);
                            view.close_menu_and_focus_trigger(MenuKind::InspectorPanel, window, cx);
                        })),
                );
            }
            dropdown = dropdown.panel(panel);
        }
        let tabs = self.panel_tabs();
        if std::mem::take(&mut self.inspector_reveal_selected) {
            if let Some(index) = tabs.iter().position(|tab| tab.selected) {
                self.inspector_tabs_scroll.scroll_to_item(index);
            }
        }
        let mut strip = div()
            .id("panel-tabs")
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_1()
            .track_scroll(&self.inspector_tabs_scroll)
            .overflow_x_scroll();
        if tabs.is_empty() {
            strip = strip.child(
                div()
                    .px_2()
                    .text_size(font::SM)
                    .text_color(dark().text.secondary)
                    .child(t("inspector.panel")),
            );
        }
        for tab in tabs {
            strip = strip.child(self.panel_tab_element(tab, cx));
        }
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(metrics::INSPECTOR_TAB_HEIGHT))
            .flex_none()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(strip)
            .child(
                self.shell_element("inspector-panel-layout")
                    .flex_none()
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
                            .child(icon(Icon::Inspector))
                            .tooltip(t("inspector.hide"))
                            .track_focus(&self.inspector_collapse_focus)
                            .on_click(cx.listener(|view, event, window, cx| {
                                if !view.consume_button_key_click("inspector-collapse", event) {
                                    view.on_toggle_inspector(window, cx);
                                }
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
            InspectorTab::Home => self.inspector_home_element(cx).into_any_element(),
            InspectorTab::Changes => self.changes_element(window, cx).into_any_element(),
            InspectorTab::Terminal => self.terminal_page_element(window, cx).into_any_element(),
            InspectorTab::Resources => self.resources_element(cx).into_any_element(),
            InspectorTab::Browser => self.browser_element(cx).into_any_element(),
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

    pub(super) fn remember_inspector_tab(&mut self, tab: InspectorTab) {
        if tab != InspectorTab::Home && !self.inspector_open_tabs.contains(&tab) {
            self.inspector_open_tabs.push(tab);
        }
    }

    pub(super) fn panel_tabs(&self) -> Vec<PanelTab> {
        let workspace = self.inspector_workspace_id();
        let mut tools = self.inspector_open_tabs.clone();
        if self.inspector_tab != InspectorTab::Home && !tools.contains(&self.inspector_tab) {
            tools.push(self.inspector_tab);
        }
        if !tools.contains(&InspectorTab::Terminal)
            && !self
                .projection
                .workspace_terminals(workspace.as_deref())
                .is_empty()
        {
            tools.push(InspectorTab::Terminal);
        }
        let mut tabs = Vec::new();
        for tool in tools {
            if tool == InspectorTab::Terminal {
                let terminals = self.projection.workspace_terminals(workspace.as_deref());
                if !terminals.is_empty() {
                    for (index, terminal) in terminals.into_iter().enumerate() {
                        let Some(id) = terminal.session_id.as_ref() else {
                            continue;
                        };
                        tabs.push(PanelTab {
                            id: format!("terminal-tab-{id}"),
                            label: format!("{} {}", tool.label(), index + 1),
                            tool,
                            terminal_id: Some(id.clone()),
                            selected: self.inspector_tab == tool
                                && self.projection.terminal.session_id.as_ref() == Some(id),
                            close_enabled: self.terminal_pending_close.is_none()
                                && terminal_can_close(&self.projection.connection, terminal),
                        });
                    }
                    continue;
                }
            }
            tabs.push(PanelTab {
                id: tool
                    .button_id()
                    .replace("inspector-tab-", "inspector-open-"),
                label: if tool == InspectorTab::Browser && !self.browser.state.title.is_empty() {
                    self.browser.state.title.clone()
                } else {
                    tool.label().into()
                },
                tool,
                terminal_id: None,
                selected: tool == self.inspector_tab,
                close_enabled: tool != InspectorTab::Terminal
                    || self.terminal_pending_create_workspace.is_none(),
            });
        }
        tabs
    }

    pub(super) fn open_inspector_tool(&mut self, tool: InspectorTab, cx: &mut Context<Self>) {
        self.select_inspector_tab(tool, cx);
    }

    pub(super) fn launch_inspector_tool(&mut self, tool: InspectorTab, cx: &mut Context<Self>) {
        // 菜单语义始终是新建一枚 PTY：无终端或已停在未开始页时
        // select 不会触发 ensure_terminal，必须显式发起；create-pending
        // 去重保证切页自动创建与这里不重复。
        if tool == InspectorTab::Terminal {
            self.on_new_terminal_tab(cx);
        }
        self.select_inspector_tab(tool, cx);
    }

    pub(super) fn close_inspector_tool(&mut self, tool: InspectorTab, cx: &mut Context<Self>) {
        if tool == InspectorTab::Browser {
            self.browser.close(cx);
        }
        self.inspector_open_tabs.retain(|open| *open != tool);
        let workspace = self.inspector_workspace_id();
        let terminals = self.projection.workspace_terminals(workspace.as_deref());
        let has_terminal = terminals
            .iter()
            .any(|terminal| terminal.session_id.is_some());
        if self.projection.terminal.session_id.is_none() && has_terminal {
            self.projection
                .select_terminal_for_workspace(workspace.as_deref());
        }
        if self.inspector_tab == tool {
            // 回落取 retain 后最近的非 Terminal 工具；只剩 Terminal 且项目
            // 还有 PTY 才落到 Terminal，否则回 Home。
            self.inspector_tab = self
                .inspector_open_tabs
                .iter()
                .rev()
                .find(|open| **open != InspectorTab::Terminal)
                .copied()
                .or_else(|| has_terminal.then_some(InspectorTab::Terminal))
                .unwrap_or(InspectorTab::Home);
            self.pending_inspector_focus = Some(super::InspectorFocusTarget::SelectedTab);
            self.refresh_open_inspector_tab(cx);
        }
        if has_terminal && !self.inspector_open_tabs.contains(&InspectorTab::Terminal) {
            self.inspector_open_tabs.push(InspectorTab::Terminal);
        }
        cx.notify();
    }

    pub(super) fn close_panel_tab(&mut self, tab: &PanelTab, cx: &mut Context<Self>) {
        if !tab.close_enabled {
            return;
        }
        if let Some(id) = &tab.terminal_id {
            self.terminal_pending_close = Some(id.clone());
            self.controller.terminal_close(id.clone());
        } else {
            self.close_inspector_tool(tab.tool, cx);
        }
        cx.notify();
    }

    fn panel_tab_element(&mut self, tab: PanelTab, cx: &mut Context<Self>) -> impl IntoElement {
        let id = tab.id.clone();
        let close_id = tab.close_id();
        let selected = tab.selected;
        let focus = self
            .inspector_item_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let close_focus = if close_id == "terminal-close" {
            self.terminal_close_focus.clone()
        } else {
            self.inspector_item_focus
                .entry(close_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone()
        };
        let layout = self
            .inspector_item_layouts
            .entry(id.clone())
            .or_default()
            .clone();
        let close_layout = self
            .inspector_item_layouts
            .entry(close_id.clone())
            .or_default()
            .clone();
        let click_tab = tab.clone();
        let key_tab = tab.clone();
        let click_id = id.clone();
        let key_id = id.clone();
        let key_close_id = close_id.clone();
        let click_close_id = close_id.clone();
        let click_close_tab = tab.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .w(px(PANEL_TAB_WIDTH))
            .h(px(metrics::ICON_BUTTON_SIZE))
            .rounded(px(6.0))
            .when(selected, |row| row.bg(dark().surface.raised))
            .child(
                div()
                    .id(gpui::SharedString::from(format!("{id}-layout")))
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_w_0()
                    .track_scroll(&layout)
                    .child(
                        Button::new(id)
                            .variant(ButtonVariant::Ghost)
                            .track_focus(&focus)
                            .padding(ButtonPadding::Horizontal(8.0))
                            .width(px(PANEL_TAB_WIDTH - 28.0))
                            .height(px(metrics::ICON_BUTTON_SIZE))
                            .vcenter()
                            .text_size(font::SM)
                            .tooltip(tab.label.clone())
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .flex_1()
                                    .min_w_0()
                                    .child(icon_sized(tab.tool.icon(), px(metrics::ICON_SM)))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_color(if selected {
                                                dark().text.primary
                                            } else {
                                                dark().text.secondary
                                            })
                                            .child(tab.label.clone()),
                                    ),
                            )
                            .on_click(cx.listener(move |view, event, _, cx| {
                                if view.consume_button_key_click(&click_id, event) {
                                    return;
                                }
                                view.activate_panel_tab(&click_tab, cx);
                            }))
                            .on_activate(cx.listener(move |view, _, _, cx| {
                                if view.open_menu.is_some() {
                                    return;
                                }
                                view.note_button_key_activate(&key_id);
                                view.activate_panel_tab(&key_tab, cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
            .child(
                div()
                    .id(gpui::SharedString::from(format!("{close_id}-layout")))
                    .flex_none()
                    .track_scroll(&close_layout)
                    .child(
                        Button::new(close_id)
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::None)
                            .width(px(28.0))
                            .height(px(metrics::ICON_BUTTON_SIZE))
                            .center()
                            .child(icon_sized(Icon::Cancel, px(12.0)))
                            .tooltip(format!(
                                "{} · {}",
                                t("inspector.close_tab"),
                                click_close_tab.label
                            ))
                            .track_focus(&close_focus)
                            .disabled(!tab.close_enabled)
                            .on_click(cx.listener(move |view, event, _, cx| {
                                if view.consume_button_key_click(&click_close_id, event) {
                                    return;
                                }
                                view.close_panel_tab(&click_close_tab, cx);
                            }))
                            .on_activate(cx.listener(move |view, _, _, cx| {
                                if view.open_menu.is_some() {
                                    return;
                                }
                                view.note_button_key_activate(&key_close_id);
                                view.close_panel_tab(&tab, cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
    }

    pub(super) fn activate_panel_tab(&mut self, tab: &PanelTab, cx: &mut Context<Self>) {
        if let Some(id) = &tab.terminal_id {
            self.on_select_terminal_tab(id, cx);
        } else {
            self.select_inspector_tab(tab.tool, cx);
        }
    }

    fn inspector_home_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut actions = div().flex().flex_col().w_full().max_w(px(360.0)).gap_2();
        for tool in InspectorTab::ALL {
            let id = tool.button_id();
            let focus = self
                .inspector_item_focus
                .entry(id.into())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let layout = self
                .inspector_item_layouts
                .entry(id.into())
                .or_default()
                .clone();
            actions = actions.child(
                div()
                    .id(gpui::SharedString::from(format!("{id}-layout")))
                    .flex()
                    .flex_row()
                    .track_scroll(&layout)
                    .child(
                        Button::new(id)
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::Horizontal(12.0))
                            .height(px(
                                52.0 * self.text_scale.rem_pixels() / font::BASE_REM_PIXELS
                            ))
                            .width(px(336.0))
                            .vcenter()
                            .track_focus(&focus)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_3()
                                    .child(icon(tool.icon()))
                                    .child(
                                        div().flex().flex_row().flex_1().min_w_0().child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_1()
                                                .child(
                                                    div().text_size(font::BASE).child(tool.label()),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(font::XS)
                                                        .text_color(dark().text.secondary)
                                                        .child(tool.hint()),
                                                ),
                                        ),
                                    ),
                            )
                            .on_click(cx.listener(move |view, event, _, cx| {
                                if view.consume_button_key_click(id, event) {
                                    return;
                                }
                                view.open_inspector_tool(tool, cx);
                            }))
                            .on_activate(cx.listener(move |view, _, _, cx| {
                                if view.open_menu.is_some() {
                                    return;
                                }
                                view.note_button_key_activate(id);
                                view.open_inspector_tool(tool, cx);
                                cx.stop_propagation();
                            })),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .justify_center()
            .items_center()
            .px_4()
            .gap_4()
            .child(
                div()
                    .text_size(font::SM)
                    .text_color(dark().text.secondary)
                    .child(t("inspector.home_hint")),
            )
            .child(actions)
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
        div().flex().flex_col().flex_1().min_h_0().child(
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
                            input.update(app, |input, _| input.cursor_bounds = Some(cursor_bounds));
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
                                if view.consume_button_key_click("terminal-back-to-bottom", event) {
                                    return;
                                }
                                view.terminal_scroll.jump_to_bottom();
                                cx.notify();
                            })),
                    ))
                }),
        )
    }

    pub(super) fn on_select_terminal_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.projection.terminal.session_id.as_deref() != Some(id)
            && !self.projection.select_terminal(id)
        {
            return;
        }
        self.inspector_tab = InspectorTab::Terminal;
        self.remember_inspector_tab(InspectorTab::Terminal);
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

    /// 初次打开为工具入口；Activity / Review 仍显式进入 Changes。
    #[test]
    fn inspector_tab_defaults_to_home() {
        assert_eq!(InspectorTab::default(), InspectorTab::Home);
    }

    #[test]
    fn terminal_output_hides_vt_control_sequences() {
        assert_eq!(
            pawork_terminal::plain_output("\u{1b}[?2004hpwd\u{1b}[?2004l\r\n/workspace\r\n"),
            "pwd\n/workspace"
        );
    }
}
