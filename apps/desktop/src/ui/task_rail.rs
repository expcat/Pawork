//! Sessions 侧栏（TaskRail）：分组 / 范围菜单、项目块与任务列表。
//!
//! R3 Wave A（F-03/F-04）按 state-a/c 量图还原视觉与结构：顶部三行
//! （标题 / scope / 连接）、日期桶 → 项目头 → 任务行的列表节奏，以及诚实
//! 状态点语义（Needs input 琥珀 > Running 蓝 > 空心灰不声明语义；wire 无
//! 每会话终态字段，不画终态绿点）。几何常量与 AX 树共享 theme::metrics。

use gpui::{div, prelude::*, px, Context, FontWeight, Pixels, Point, Rgba, SharedString, Window};

use crate::projection::{
    ConnectionState, SessionLiveStatus, TaskRailDateGroup, TaskRailGrouping, TaskRailProjectGroup,
    UNASSIGNED_PROJECT,
};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::label::Label;
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

/// 状态圆点（Ø10，量图 10–11）：实心 = 语义色；空心灰 = 不声明语义的
/// 设计 ○ 槽位（描边 text.tertiary，不填充）。
fn status_dot(filled: bool, color: Rgba) -> gpui::Div {
    let size = metrics::RAIL_STATUS_DOT_SIZE;
    let dot = div().w(px(size)).h(px(size)).rounded_full().flex_none();
    if filled {
        dot.bg(color)
    } else {
        dot.border_1().border_color(color)
    }
}

impl AppView {
    /// rail 宽由 shell_layout::resolve 按窗口带宽给出（288 / 窄窗 240）。
    pub(super) fn sidebar_element(&self, rail_width: Pixels, cx: &mut Context<Self>) -> Panel {
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
        let connection_label = self.connection_status_label();

        // F-03 标题行：角标钮（ghost 档 hover surface.raised，§8.1），hit area
        // 28×28 ≥ 24；identifier / tooltip / accessible name 冻结不变。
        let grouping_tooltip = SharedString::from(self.grouping.accessible_name());
        let grouping_button = Button::new("task-rail-grouping")
            .variant(ButtonVariant::Ghost)
            .padding(ButtonPadding::None)
            .width(px(metrics::RAIL_ICON_BUTTON_SIZE))
            .height(px(metrics::RAIL_ICON_BUTTON_SIZE))
            .center()
            .radius(4.0)
            .text_size(font::BASE)
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

        // F-03 scope 行：全宽 raised 行 + 1px 描边 + 圆角 4 + 高 36 + 字阶 18。
        let scope_button = Button::new("project-scope")
            .variant(ButtonVariant::Raised)
            // 文字贴左内缩 12（量图：box x20 → 文字 x32），垂直居中。
            .padding(ButtonPadding::Horizontal(metrics::RAIL_INNER_PAD))
            .height(px(metrics::RAIL_TOP_ROW_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY)
            .label(format!("{scope_label} ▾"))
            .on_click(cx.listener(|view, event, window, cx| {
                let down = Self::click_down_position(event);
                view.on_toggle_scope_menu(down, window, cx);
            }));
        let mut scope = Dropdown::new(scope_button);
        if scope_menu_open {
            scope = scope.panel(self.scope_menu_element(cx));
        }

        // F-03 全局 AddTaskButton：28×28 角标，glyph ~13px；禁用原因 tooltip
        // 逻辑不动。
        let add_task_tooltip = if can_create {
            SharedString::from("New task (⌘N)")
        } else {
            SharedString::from(self.add_task_disabled_reason())
        };
        let add_task_focus = self.add_task_focus.clone();
        let add_task = Button::new("add-task")
            .track_focus(&add_task_focus)
            .variant(ButtonVariant::Ghost)
            .disabled(!can_create)
            .padding(ButtonPadding::None)
            .width(px(metrics::RAIL_ICON_BUTTON_SIZE))
            .height(px(metrics::RAIL_ICON_BUTTON_SIZE))
            .center()
            .radius(4.0)
            .text_size(font::BASE)
            .label("+")
            .tooltip(add_task_tooltip)
            .on_click(cx.listener(|view, _event, window, cx| {
                view.on_new_session(window, cx);
            }));

        // F-03 连接行：状态点 + 文案 17px text.secondary（四态文字不只靠颜色，
        // TR-05）；Connected 绿点冻结为 semantic.success_fg，Connecting 蓝
        // （accent.primary），断线 / 失败空心灰。
        let (connection_dot_filled, connection_dot_color) = match &self.projection.connection {
            ConnectionState::Connected { .. } => (true, dark().semantic.success_fg),
            ConnectionState::Connecting => (true, dark().accent.primary),
            ConnectionState::Disconnected { .. } | ConnectionState::Failed { .. } => {
                (false, dark().text.tertiary)
            }
        };

        // 内容统一 inset 20（Panel p_2 帧 8 + 内层 12）；三行节奏 32 / 20。
        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(metrics::RAIL_INNER_PAD))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(px(metrics::RAIL_TITLE_ROW_HEIGHT))
                    .child(
                        div()
                            .text_size(px(font::TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(dark().text.primary)
                            .child("Pawork"),
                    )
                    .child(grouping),
            )
            .child(
                div()
                    .mt(px(metrics::RAIL_TITLE_SCOPE_GAP))
                    .h(px(metrics::RAIL_TOP_ROW_HEIGHT))
                    .child(scope),
            )
            .child(
                div()
                    .mt(px(metrics::RAIL_SCOPE_CONNECTION_GAP))
                    .h(px(metrics::RAIL_TOP_ROW_HEIGHT))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(status_dot(connection_dot_filled, connection_dot_color))
                            .child(
                                Label::new(connection_label)
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            ),
                    )
                    .child(add_task),
            );
        // F-02 壳层校准：Reconnect 仅在 Disconnected / ConnectFailed 出现；
        // Connecting 属进行中，不给重复入口。
        if self.projection.show_reconnect() {
            content = content.child(
                div().mt_2().child(
                    Button::new("reconnect")
                        .variant(ButtonVariant::Primary)
                        .height(px(metrics::RAIL_TOP_ROW_HEIGHT))
                        .center()
                        .text_size(font::BODY_SM)
                        .label("Reconnect")
                        .on_click(cx.listener(|view, _event, window, cx| {
                            view.on_reconnect(window, cx);
                        })),
                ),
            );
        }
        content = content.child(self.task_rail_list(rail_groups, now_ms, can_create, cx));
        // TR-12 honest-hidden：只保留「Local」本机身份行，不画头像 / 姓名 /
        // 齿轮 / quota（无权威账户 capability）。
        content = content.child(
            div().mt_auto().pt_2().child(
                Label::new("Local")
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            ),
        );

        Panel::side_right(rail_width)
            // F-01：透明 titlebar 下 traffic lights 悬浮于 rail 左上，
            // 顶部先留 ≥36px 无交互安全区，再进入 F-03 的三行节奏。
            .child(shell_layout::rail_safe_area())
            .child(content)
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
            .min_h_0()
            .overflow_y_scroll()
            .mt(px(metrics::RAIL_LIST_TOP_GAP));
        if empty {
            return list.child(
                div()
                    .h(px(metrics::RAIL_TASK_ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .child(
                        Label::new("No tasks")
                            .size(font::BODY_SM)
                            .color(dark().text.tertiary),
                    ),
            );
        }
        match rail {
            RailView::Timeline(groups) => {
                // Timeline：日期桶头（18 medium text.secondary，桶头距上组 20）
                // → 项目块（桶头→首项目 2，项目块间 8）。
                for (group_index, group) in groups.iter().enumerate() {
                    let mut bucket = div().flex().flex_col();
                    if group_index > 0 {
                        bucket = bucket.mt(px(metrics::RAIL_BUCKET_TOP_GAP));
                    }
                    bucket = bucket.child(
                        div()
                            .h(px(metrics::RAIL_BUCKET_HEADER_HEIGHT))
                            .flex()
                            .items_center()
                            .pl_2()
                            .child(
                                div()
                                    .text_size(px(font::BODY))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(dark().text.secondary)
                                    .child(group.bucket.label().to_string()),
                            ),
                    );
                    for (project_index, project) in group.projects.iter().enumerate() {
                        let mut block = self.project_block(project, now_ms, can_create, cx);
                        // 桶头→首项目 2；项目块间 8（量图反推行位锚点）。
                        block = block.mt(px(if project_index == 0 {
                            metrics::RAIL_BUCKET_TO_PROJECT_GAP
                        } else {
                            metrics::RAIL_PROJECT_BLOCK_GAP
                        }));
                        bucket = bucket.child(block);
                    }
                    list = list.child(bucket);
                }
            }
            RailView::Projects(groups) => {
                for (project_index, project) in groups.iter().enumerate() {
                    let mut block = self.project_block(project, now_ms, can_create, cx);
                    if project_index > 0 {
                        block = block.mt(px(metrics::RAIL_PROJECT_BLOCK_GAP));
                    }
                    list = list.child(block);
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
        // F-04 项目头：chevron + 名称（18 medium emphasis）+ 独立右对齐计数 +
        // 定向「+」（28×28；Unassigned 无 +）。折叠态只显示头。
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                ListRow::project_header(header_id)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .text_size(px(font::BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(dark().text.emphasis)
                            .child(if expanded { "▾" } else { "▸" })
                            // 长项目头标题 truncate（flex_1 + min_w_0）。
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(project.name.clone()),
                            ),
                    )
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.on_toggle_project(toggle_key.clone(), window, cx);
                    })),
            )
            .child(
                Label::new(project.task_count().to_string())
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );
        if !project.is_unassigned() {
            if let Some(workspace_id) = workspace_id {
                header = header.child(
                    Button::new(add_id)
                        .variant(ButtonVariant::Ghost)
                        .disabled(!can_create)
                        .padding(ButtonPadding::None)
                        .width(px(metrics::RAIL_ICON_BUTTON_SIZE))
                        .height(px(metrics::RAIL_ICON_BUTTON_SIZE))
                        .center()
                        .radius(4.0)
                        .text_size(font::BASE)
                        .label("+")
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_project_add_task(workspace_id.clone(), window, cx);
                        })),
                );
            }
        }
        let mut block = div().flex().flex_col().child(header);
        if expanded {
            for (task_index, task) in project.tasks.iter().enumerate() {
                let session_id = task.session_id.clone();
                let active =
                    self.projection.active_session_id.as_deref() == Some(task.session_id.as_str());
                // 状态点语义（诚实，只消费 wire 已有数据）：Needs input
                // （pending approval 按 session_id 归属）琥珀优先于 Running 蓝；
                // 其余空心灰圆不声明语义；终态绿点不画（wire 无来源）。
                let (dot_filled, dot_color) =
                    match self.projection.session_live_status(&task.session_id) {
                        Some(SessionLiveStatus::NeedsInput) => (true, dark().semantic.warning_text),
                        Some(SessionLiveStatus::Running) => (true, dark().accent.primary),
                        None => (false, dark().text.tertiary),
                    };
                // 项目头 → 首个任务行 2（量图行位锚点）；任务行间 0。
                let row_gap = if task_index == 0 {
                    metrics::RAIL_PROJECT_TO_TASK_GAP
                } else {
                    0.0
                };
                block = block.child(
                    div().mt(px(row_gap)).child(
                        ListRow::task(SharedString::from(session_id.clone()), active)
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .min_w_0()
                                    .flex_1()
                                    .child(status_dot(dot_filled, dot_color))
                                    // 长任务标题单行 truncate；相对时间右对齐保留。
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(font::BODY))
                                            .text_color(dark().text.emphasis)
                                            .child(task.title.clone()),
                                    ),
                            )
                            .child(
                                Label::new(relative_activity(task.updated_at_ms, now_ms))
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            )
                            .on_click(cx.listener(move |view, _event, window, cx| {
                                view.on_session_clicked(&session_id, window, cx);
                            })),
                    ),
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

    /// 连接行可见文案（render 与 AX 值同源，ADR-042）。
    pub(super) fn connection_status_label(&self) -> String {
        match &self.projection.connection {
            ConnectionState::Connected { .. } => match self.projection.resume.label() {
                Some(resume) => format!("Local · Connected · {resume}"),
                None => "Local · Connected".into(),
            },
            other => other.label(),
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
