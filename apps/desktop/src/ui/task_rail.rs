//! Sessions 侧栏（TaskRail）：分组 / 范围菜单、项目块与任务列表
//! （R8 波 C 自 ui/mod.rs 逐样式迁移）。长任务标题与项目头标题 truncate，
//! 相对时间 Label 保留。

use gpui::{div, prelude::*, px, Context, Pixels, Point, SharedString, Window};

use crate::projection::{
    ConnectionState, TaskRailDateGroup, TaskRailGrouping, TaskRailProjectGroup,
    UNASSIGNED_PROJECT,
};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::label::{Badge, Label};
use crate::ui::components::list_row::ListRow;
use crate::ui::components::panel::Panel;
use crate::ui::theme::{dark, font, metrics};

use super::{now_unix_ms, shell_layout, AppView, MenuKind};

enum RailView {
    Timeline(Vec<TaskRailDateGroup>),
    Projects(Vec<TaskRailProjectGroup>),
}

fn relative_activity(updated_at_ms: u64, now_ms: u64) -> String {
    let elapsed = now_ms.saturating_sub(updated_at_ms);
    if elapsed < 60_000 {
        "now".into()
    } else if elapsed < 3_600_000 {
        format!("{}m", elapsed / 60_000)
    } else if elapsed < 86_400_000 {
        format!("{}h", elapsed / 3_600_000)
    } else {
        format!("{}d", elapsed / 86_400_000)
    }
}

impl AppView {
    /// rail 宽由 shell_layout::resolve 按窗口带宽给出（288 / 窄窗 240）。
    pub(super) fn sidebar_element(&self, rail_width: Pixels, cx: &mut Context<Self>) -> Panel {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let can_create = self.can_create_task();
        let grouping_glyph = match self.grouping {
            TaskRailGrouping::Timeline => "◷",
            TaskRailGrouping::Projects => "▤",
        };
        let scope_label = self.scope_label();
        let grouping_menu_open = matches!(self.open_menu, Some(MenuKind::Grouping));
        let scope_menu_open = matches!(self.open_menu, Some(MenuKind::Scope));
        let now_ms = now_unix_ms();
        let rail_groups = match self.grouping {
            TaskRailGrouping::Timeline => RailView::Timeline(
                self.projection
                    .timeline_groups(self.scope_workspace_id.as_deref(), now_ms),
            ),
            TaskRailGrouping::Projects => RailView::Projects(
                self.projection
                    .project_groups(self.scope_workspace_id.as_deref()),
            ),
        };
        let connection_label = match &self.projection.connection {
            ConnectionState::Connected { .. } => match self.projection.resume.label() {
                Some(resume) => format!("Local · Connected · {resume}"),
                None => "Local · Connected".into(),
            },
            other => other.label(),
        };

        let grouping_tooltip = SharedString::from(self.grouping.accessible_name());
        let grouping_button = Button::new("task-rail-grouping")
            .variant(ButtonVariant::Raised)
            .padding(ButtonPadding::None)
            .width(px(metrics::ICON_LARGE))
            .height(px(metrics::ICON_MEDIUM))
            .text_size(font::SM)
            .label(format!("{grouping_glyph} ▾"))
            .tooltip(grouping_tooltip)
            .on_click(cx.listener(|view, event, window, cx| {
                let down = Self::click_down_position(event);
                view.on_toggle_grouping_menu(down, window, cx);
            }));
        let mut grouping = Dropdown::new(grouping_button);
        if grouping_menu_open {
            grouping = grouping.panel(self.grouping_menu_element(cx));
        }

        let scope_button = Button::new("project-scope")
            .variant(ButtonVariant::Raised)
            .text_size(font::SM)
            .label(format!("{scope_label} ▾"))
            .on_click(cx.listener(|view, event, window, cx| {
                let down = Self::click_down_position(event);
                view.on_toggle_scope_menu(down, window, cx);
            }));
        let mut scope = Dropdown::new(scope_button);
        if scope_menu_open {
            scope = scope.panel(self.scope_menu_element(cx));
        }

        let add_task_tooltip = if can_create {
            SharedString::from("New task (⌘N)")
        } else {
            SharedString::from(self.add_task_disabled_reason())
        };
        let add_task_focus = self.add_task_focus.clone();
        let add_task = Button::new("add-task")
            .track_focus(&add_task_focus)
            .variant(ButtonVariant::Icon)
            .disabled(!can_create)
            .padding(ButtonPadding::None)
            .width(px(metrics::ICON_MEDIUM))
            .height(px(metrics::ICON_MEDIUM))
            .label("+")
            .tooltip(add_task_tooltip)
            .on_click(cx.listener(|view, _event, window, cx| {
                view.on_new_session(window, cx);
            }));

        let mut sidebar = Panel::side_right(rail_width)
            // F-01：透明 titlebar 下 traffic lights 悬浮于 rail 左上，
            // 顶部先留 ≥36px 无交互安全区，再进入 F-03 的既有三行节奏。
            .child(shell_layout::rail_safe_area())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new("Pawork")
                            .size(font::BASE)
                            .color(dark().text.primary),
                    )
                    .child(grouping),
            )
            .child(scope)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(Badge::new(connection_label))
                    .child(add_task),
            );
        if !connected {
            sidebar = sidebar.child(
                Button::new("reconnect")
                    .variant(ButtonVariant::Primary)
                    .label("Reconnect")
                    .on_click(cx.listener(|view, _event, window, cx| {
                        view.on_reconnect(window, cx);
                    })),
            );
        }
        sidebar
            .child(self.task_rail_list(rail_groups, now_ms, can_create, cx))
            .child(
                div().mt_auto().pt_2().child(
                    Label::new("Local")
                        .size(font::XS)
                        .color(dark().text.tertiary),
                ),
            )
    }

    fn grouping_menu_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        let current = self.grouping;
        MenuPanel::new("grouping-menu")
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Grouping, event.position, cx);
            }))
            .children(
                [
                    (TaskRailGrouping::Timeline, "Timeline"),
                    (TaskRailGrouping::Projects, "Projects"),
                ]
                .into_iter()
                .map(|(mode, label)| {
                    let selected = current == mode;
                    MenuRow::new(SharedString::from(format!("group-{label}")))
                        .label(if selected {
                            format!("✓ {label}")
                        } else {
                            format!("  {label}")
                        })
                        .selected(selected)
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_select_grouping(mode, window, cx);
                        }))
                }),
            )
    }

    fn scope_menu_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        let current = self.scope_workspace_id.clone();
        MenuPanel::new("scope-menu")
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Scope, event.position, cx);
            }))
            .children(self.projection.project_scope_options().into_iter().map(
                |(workspace_id, label)| {
                    let selected = current == workspace_id;
                    let option_id = workspace_id.clone().unwrap_or_else(|| "all".into());
                    MenuRow::new(SharedString::from(format!("scope-{option_id}")))
                        .label(if selected {
                            format!("✓ {label}")
                        } else {
                            label
                        })
                        .selected(selected)
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_select_scope(workspace_id.clone(), window, cx);
                        }))
                },
            ))
    }

    fn task_rail_list(
        &self,
        rail: RailView,
        now_ms: u64,
        can_create: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = match &rail {
            RailView::Timeline(groups) => groups.is_empty(),
            RailView::Projects(groups) => groups.is_empty(),
        };
        let mut list = div()
            .id("task-rail")
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll()
            .gap_1();
        if empty {
            return list.child(
                div().px_2().py_2().child(
                    Label::new("No tasks")
                        .size(font::SM)
                        .color(dark().text.tertiary),
                ),
            );
        }
        match rail {
            RailView::Timeline(groups) => {
                for group in groups {
                    list = list.child(
                        div().px_1().pt_2().child(
                            Label::new(group.bucket.label().to_string())
                                .size(font::XS)
                                .color(dark().text.secondary),
                        ),
                    );
                    for project in group.projects {
                        list = list.child(self.project_block(&project, now_ms, can_create, cx));
                    }
                }
            }
            RailView::Projects(groups) => {
                for project in groups {
                    list = list.child(self.project_block(&project, now_ms, can_create, cx));
                }
            }
        }
        list
    }

    fn project_block(
        &self,
        project: &TaskRailProjectGroup,
        now_ms: u64,
        can_create: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let key = Self::project_key(project.workspace_id.as_deref());
        let expanded = !self.collapsed_projects.contains(&key);
        let workspace_id = project.workspace_id.clone();
        let header_id = SharedString::from(format!("project-{key}"));
        let add_id = SharedString::from(format!("project-add-{key}"));
        let toggle_key = key.clone();
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_1()
            .child(
                div().flex_1().min_w_0().child(
                    ListRow::project_header(header_id)
                        .child(
                            // 长项目头标题 truncate（flex_1 + min_w_0 宽度约束），
                            // 计数徽标保留在标题文本内。
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(font::SM))
                                .text_color(dark().text.emphasis)
                                .child(format!(
                                    "{} {} · {}",
                                    if expanded { "▾" } else { "▸" },
                                    project.name,
                                    project.task_count()
                                )),
                        )
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_toggle_project(toggle_key.clone(), window, cx);
                        })),
                ),
            );
        if !project.is_unassigned() {
            if let Some(workspace_id) = workspace_id {
                header = header.child(
                    Button::new(add_id)
                        .variant(ButtonVariant::Icon)
                        .disabled(!can_create)
                        .padding(ButtonPadding::None)
                        .width(px(metrics::ICON_SMALL))
                        .height(px(metrics::ICON_SMALL))
                        .label("+")
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_project_add_task(workspace_id.clone(), window, cx);
                        })),
                );
            }
        }
        let mut block = div().flex().flex_col().gap_1().child(header);
        if expanded {
            for task in &project.tasks {
                let session_id = task.session_id.clone();
                let active =
                    self.projection.active_session_id.as_deref() == Some(task.session_id.as_str());
                let running = self
                    .projection
                    .active_runs
                    .iter()
                    .any(|run| run.session_id == task.session_id);
                block = block.child(
                    ListRow::task(SharedString::from(session_id.clone()), active)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_between()
                                .gap_1()
                                // 长任务标题 truncate（nowrap + 省略号），相对时间保留。
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(if running {
                                            div()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap_1()
                                                .min_w_0()
                                                .child(
                                                    div()
                                                        .text_color(dark().semantic.success_fg)
                                                        .child("●"),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .child(task.title.clone()),
                                                )
                                        } else {
                                            div().truncate().child(task.title.clone())
                                        }),
                                )
                                .child(
                                    Label::new(relative_activity(task.updated_at_ms, now_ms))
                                        .size(font::XS)
                                        .color(dark().text.tertiary),
                                ),
                        )
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_session_clicked(&session_id, window, cx);
                        })),
                );
            }
        }
        block
    }

    pub(super) fn on_toggle_grouping_menu(
        &mut self,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_menu(MenuKind::Grouping, down_position, cx);
    }

    pub(super) fn on_select_grouping(
        &mut self,
        grouping: TaskRailGrouping,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.grouping = grouping;
        self.open_menu = None;
        cx.notify();
    }

    pub(super) fn on_toggle_scope_menu(
        &mut self,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_menu(MenuKind::Scope, down_position, cx);
    }

    pub(super) fn on_select_scope(
        &mut self,
        workspace_id: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scope_workspace_id = workspace_id;
        self.open_menu = None;
        cx.notify();
    }

    pub(super) fn on_toggle_project(
        &mut self,
        project_key: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collapsed_projects.insert(project_key.clone()) {
            self.collapsed_projects.remove(&project_key);
        }
        cx.notify();
    }

    pub(super) fn on_project_add_task(
        &mut self,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_task(Some(workspace_id), window, cx);
    }

    fn project_key(workspace_id: Option<&str>) -> String {
        workspace_id.unwrap_or(UNASSIGNED_PROJECT).to_string()
    }

    fn scope_label(&self) -> String {
        match &self.scope_workspace_id {
            None => "All projects".into(),
            Some(id) => self.projection.workspace_name(Some(id)),
        }
    }

    pub(super) fn add_task_disabled_reason(&self) -> String {
        match &self.projection.connection {
            ConnectionState::Connected { .. } => "Create task is available.".into(),
            ConnectionState::Connecting => "New task needs a live connection.".into(),
            ConnectionState::Disconnected { reason } => {
                format!("New task disabled · disconnected · {reason}")
            }
            ConnectionState::Failed { reason } => {
                format!("New task disabled · connect failed · {reason}")
            }
        }
    }
}
