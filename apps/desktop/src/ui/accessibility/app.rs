//! AppView → AxTree 投影与 AX action 白名单。

use gpui::{App, Context, Focusable, Window};

use crate::projection::{
    run_footer_label, run_summary_texts, ConnectionState, DateBucket, ForkBoundary, ModelEntry,
    SessionLiveStatus, TaskRailGrouping, TaskRailProjectGroup, TimelineEntry, TimelineEntryKind,
    TimelineRow, UNASSIGNED_PROJECT,
};

use super::{AxAction, AxBridge, AxNode, AxRect, AxRequest, AxRole, AxTree};
use crate::ui::changes::{ChangesFetch, ChangesTab};
use crate::ui::components::dropdown::ANCHOR_GAP_Y;
use crate::ui::inspector::{
    plain_terminal_output, terminal_resize_status_label, terminal_size_for_display, InspectorTab,
    TERMINAL_COLUMNS_STEP, TERMINAL_EMPTY_OUTPUT, TERMINAL_ROWS_STEP,
};
use crate::ui::resources::ResourcesFetch;
use crate::ui::shell_layout;
use crate::ui::theme::{font, metrics};
use crate::ui::timeline_entry::display_time;
use crate::ui::{
    activity_header_visibility, rail_project_occurrence_key, rail_session_focus_key,
    terminal_can_operate, terminal_known_exited, terminal_start_enabled, timeline, AppView,
    MenuKind, WORKSPACE_EMPTY_HINT,
};

const PAD: f32 = 8.0;
const CONTROL_HEIGHT: f32 = 28.0;
const ROW_HEIGHT: f32 = 32.0;
const TIMELINE_ROW_HEIGHT: f32 = 52.0;
const ACTIVITY_CONTENT_INSET_X: f32 = 28.0;
const ACTIVITY_HEADING_OFFSET_Y: f32 = 58.0;
const ACTIVITY_SUMMARY_OFFSET_Y: f32 = 24.0;
const ACTIVITY_HEADING_HEIGHT: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct ActivityPopoverAxGeometry {
    frame: AxRect,
    heading: AxRect,
    open_changes: AxRect,
}

fn header_action_ax_rect(frame: AxRect) -> AxRect {
    let content_top = frame.y + metrics::HEADER_SAFE_STRIP;
    let content_height = (frame.height - metrics::HEADER_SAFE_STRIP).max(0.0);
    AxRect::new(
        (frame.x + frame.width - metrics::HEADER_INSET_RIGHT - metrics::HEADER_ACTION_WIDTH)
            .max(frame.x),
        content_top + ((content_height - metrics::HEADER_ACTION_HEIGHT) / 2.0).max(0.0),
        metrics::HEADER_ACTION_WIDTH,
        metrics::HEADER_ACTION_HEIGHT,
    )
}

fn activity_popover_ax_geometry(
    header_frame: AxRect,
    trigger: AxRect,
) -> ActivityPopoverAxGeometry {
    let frame = AxRect::new(
        (trigger.x + trigger.width - metrics::ACTIVITY_POPOVER_WIDTH).max(header_frame.x),
        trigger.y + trigger.height + ANCHOR_GAP_Y,
        metrics::ACTIVITY_POPOVER_WIDTH,
        metrics::ACTIVITY_POPOVER_HEIGHT,
    );
    let heading = AxRect::new(
        frame.x + ACTIVITY_CONTENT_INSET_X,
        frame.y + ACTIVITY_HEADING_OFFSET_Y,
        (frame.width - 2.0 * ACTIVITY_CONTENT_INSET_X).max(0.0),
        ACTIVITY_HEADING_HEIGHT,
    );
    let open_changes = AxRect::new(
        heading.x,
        heading.y + ACTIVITY_SUMMARY_OFFSET_Y,
        heading.width,
        ROW_HEIGHT,
    );
    ActivityPopoverAxGeometry {
        frame,
        heading,
        open_changes,
    }
}

impl AppView {
    pub(crate) fn install_accessibility(
        &mut self,
        window: &Window,
        handler: impl Fn(AxRequest) + 'static,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ax_bridge = Some(AxBridge::install(window, handler)?);
        self.ax_error_reported = false;
        cx.notify();
        Ok(())
    }

    pub(in crate::ui) fn sync_accessibility(&mut self, window: &Window, cx: &App) {
        let tree = self.accessibility_tree(window, cx);
        let Some(bridge) = self.ax_bridge.as_mut() else {
            return;
        };
        match bridge.update(tree) {
            Ok(_) => self.ax_error_reported = false,
            Err(reason) if !self.ax_error_reported => {
                eprintln!("pawork-desktop accessibility update failed: {reason}");
                self.ax_error_reported = true;
            }
            Err(_) => {}
        }
    }

    pub(crate) fn handle_accessibility_request(
        &mut self,
        request: AxRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // AX clients may retain an element from an earlier tree. Revalidate against the
        // current canonical UI state so stale enabled/action snapshots cannot bypass a gate.
        if !self.accessibility_tree(window, cx).permits(&request) {
            return;
        }
        match request.action {
            AxAction::Focus => match request.identifier.as_str() {
                "composer-input" => self.focus_composer(window, cx),
                "terminal-input" => {
                    let focus = self.terminal_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                _ => return,
            },
            AxAction::SetValue => {
                let value = request.value.unwrap_or_default();
                match request.identifier.as_str() {
                    "composer-input" => self
                        .text_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "terminal-input" => self
                        .terminal_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    _ => return,
                }
            }
            AxAction::Press => {
                if !self.handle_accessibility_press(&request.identifier, window, cx) {
                    return;
                }
            }
        }
        cx.notify();
    }

    fn handle_accessibility_press(
        &mut self,
        identifier: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match identifier {
            "task-rail-grouping" => self.on_toggle_grouping_menu(None, window, cx),
            "project-scope" => self.on_toggle_scope_menu(None, window, cx),
            "scope-add-project" | "workspace-confirm-add-project" => {
                self.on_open_project(window, cx)
            }
            "add-task" => self.on_new_session(window, cx),
            // F-05 Header 动作：与 rail 全局「+」同 handler / enable gate。
            "header-new-task" => self.on_new_session(window, cx),
            "reconnect" => self.on_reconnect(window, cx),
            "model-picker" => self.on_toggle_model_menu(None, window, cx),
            "cancel" => self.on_cancel_clicked(window, cx),
            "send" => {
                // 与键盘 Enter 路径（on_send_message）一致：IME 组合中不发送。
                if self.text_input.read(cx).is_composing() {
                    return true;
                }
                self.send_current_message(cx);
            }
            "approve-once" => self.on_approve("approve_once", window, cx),
            "approve-for-run" => self.on_approve("approve_for_run", window, cx),
            "approve-deny" => self.on_approve("deny", window, cx),
            "timeline-back-to-bottom" => self.timeline_jump_to_bottom(),
            // Inspector 折叠态触发器的可见语义是弹出 ActivityPopover（R6
            // Wave A 起位于 Workspace Header），摘要行才展开 Inspector；
            // 展开态由 inspector-collapse 收起。
            "inspector-toggle" => self.toggle_menu(MenuKind::Activity, None, cx),
            "inspector-collapse" => self.on_toggle_inspector(window, cx),
            "inspector-tab-changes" => self.select_inspector_tab(InspectorTab::Changes, cx),
            "inspector-tab-terminal" => self.select_inspector_tab(InspectorTab::Terminal, cx),
            "inspector-tab-resources" => self.select_inspector_tab(InspectorTab::Resources, cx),
            "changes-tab-files" => self.on_select_changes_tab(ChangesTab::Files, cx),
            "changes-tab-summary" => self.on_select_changes_tab(ChangesTab::Summary, cx),
            "changes-refresh" => self.refresh_changes(cx),
            "resources-refresh" => self.refresh_resources(cx),
            "terminal-resize" => self.on_apply_terminal_size(window, cx),
            "terminal-cols-dec" => self.adjust_terminal_size(-TERMINAL_COLUMNS_STEP, 0, cx),
            "terminal-cols-inc" => self.adjust_terminal_size(TERMINAL_COLUMNS_STEP, 0, cx),
            "terminal-rows-dec" => self.adjust_terminal_size(0, -TERMINAL_ROWS_STEP, cx),
            "terminal-rows-inc" => self.adjust_terminal_size(0, TERMINAL_ROWS_STEP, cx),
            "terminal-start" => {
                // 与可见按钮同一语义：可操作单槽是 Size；已知 exited 终端与
                // 未创建一样走 Start（新建终端）。
                if terminal_can_operate(&self.projection.connection, &self.projection.terminal) {
                    self.on_apply_terminal_size(window, cx);
                } else {
                    self.on_start_terminal(window, cx);
                }
            }
            "terminal-back-to-bottom" => self.terminal_scroll.jump_to_bottom(),
            "activity-open-changes" => self.on_activity_open_changes(window, cx),
            "group-timeline" => self.on_select_grouping(TaskRailGrouping::Timeline, window, cx),
            "group-projects" => self.on_select_grouping(TaskRailGrouping::Projects, window, cx),
            _ => {
                if let Some((workspace_id, _)) = self
                    .projection
                    .project_scope_options()
                    .into_iter()
                    .find(|(workspace_id, _)| {
                        scope_identifier(workspace_id.as_deref()) == identifier
                    })
                {
                    self.on_select_scope(workspace_id, window, cx);
                    return true;
                }
                if let Some(model) = self
                    .projection
                    .models
                    .iter()
                    .find(|model| model_identifier(model) == identifier)
                    .cloned()
                {
                    self.on_select_model(model, cx);
                    return true;
                }
                if let Some(workspace) = self
                    .projection
                    .workspaces
                    .iter()
                    .find(|workspace| workspace_confirm_identifier(&workspace.id) == identifier)
                    .cloned()
                {
                    self.on_confirm_workspace(workspace.id, window, cx);
                    return true;
                }
                if let Some(session) = self
                    .projection
                    .sessions
                    .iter()
                    .find(|session| session_identifier(&session.session_id) == identifier)
                    .cloned()
                {
                    self.on_session_clicked(&session.session_id, window, cx);
                    return true;
                }
                if let Some(project) = self
                    .rail_project_entries()
                    .into_iter()
                    .find(|(bucket, project)| {
                        rail_project_identifier(
                            *bucket,
                            &project_key(project.workspace_id.as_deref()),
                        ) == identifier
                    })
                    .map(|(_, project)| project)
                {
                    self.on_toggle_project(
                        project_key(project.workspace_id.as_deref()),
                        window,
                        cx,
                    );
                    return true;
                }
                if let Some(workspace_id) = self
                    .rail_project_entries()
                    .into_iter()
                    .find(|(bucket, project)| {
                        rail_project_add_identifier(
                            *bucket,
                            &project_key(project.workspace_id.as_deref()),
                        ) == identifier
                    })
                    .and_then(|(_, project)| project.workspace_id)
                {
                    self.on_project_add_task(workspace_id, window, cx);
                    return true;
                }
                if let Some(entry) = self
                    .projection
                    .timeline
                    .iter()
                    .find(|entry| entry_menu_identifier(&entry.event_id) == identifier)
                {
                    self.toggle_menu(MenuKind::Entry(entry.event_id.clone()), None, cx);
                    return true;
                }
                if let Some(event_id) = self
                    .projection
                    .timeline
                    .iter()
                    .find(|entry| fork_identifier(&entry.event_id) == identifier)
                    .map(|entry| entry.event_id.clone())
                {
                    self.close_open_menu(cx);
                    self.on_fork(&event_id, window, cx);
                    return true;
                }
                // Run 摘要卡 Review changes（enabled 由树节点校验 + 谓词
                // 双重把关，与 render 同源）。
                if self
                    .projection
                    .timeline
                    .iter()
                    .find(|entry| run_review_identifier(&entry.event_id) == identifier)
                    .filter(|entry| entry.fork_boundary == Some(ForkBoundary::Completed))
                    .is_some()
                {
                    self.on_review_changes(cx);
                    return true;
                }
                if let Some(path) = self
                    .changes
                    .files
                    .iter()
                    .find(|file| diff_file_identifier(&file.path) == identifier)
                    .map(|file| file.path.clone())
                {
                    self.on_select_diff_file(&path, cx);
                    return true;
                }
                return false;
            }
        }
        true
    }

    fn accessibility_tree(&self, window: &Window, cx: &App) -> AxTree {
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width).max(1.0);
        let height = f32::from(viewport.height).max(1.0);
        let content_height = (height - metrics::STATUS_BAR_HEIGHT).max(1.0);
        // 与 AppView::render 共享同一壳层几何决定（R2 Wave A 响应式：
        // 窄窗 rail=240 且 Inspector 强制折叠；150% 文本 rail=320），
        // AX bounds 不得偏出实际布局。
        let shell = shell_layout::resolve(
            viewport.width,
            self.inspector_open,
            self.text_scale == font::TextScale::Percent150,
        );
        let sidebar_width = shell.rail_width.min(width);
        let inspector_width = if shell.inspector_open {
            metrics::INSPECTOR_WIDTH.min((width - sidebar_width).max(0.0))
        } else {
            0.0
        };
        let workspace_width = (width - sidebar_width - inspector_width).max(0.0);
        let workspace_x = sidebar_width;

        let mut tree = AxTree::new(width, height)
            .child(self.sidebar_ax(window, AxRect::new(0.0, 0.0, sidebar_width, content_height)))
            .child(self.workspace_ax(
                window,
                cx,
                AxRect::new(workspace_x, 0.0, workspace_width, content_height),
                shell.inspector_open,
            ));
        if shell.inspector_open {
            tree = tree.child(self.inspector_ax(
                window,
                cx,
                AxRect::new(
                    width - inspector_width,
                    0.0,
                    inspector_width,
                    content_height,
                ),
            ));
        }
        // StatusBar 视觉上不覆盖左栏账户区；AX frame 与 render 同源。
        tree.child(self.status_ax(AxRect::new(
            sidebar_width,
            content_height,
            (width - sidebar_width).max(0.0),
            metrics::STATUS_BAR_HEIGHT,
        )))
    }

    fn sidebar_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let can_create = self.can_create_task();
        // 与可见 TaskRail 对齐（R3 Wave A，几何单一来源 theme::metrics）：
        // Panel p_2(8) + 36px traffic-light 安全区 + gap_2(8) 后进入标题行，
        // 三行节奏 32 / 20；AX 不得把首控件投影到按钮带。
        let inset = metrics::RAIL_CONTENT_INSET;
        let mut y = PAD + shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT + PAD;
        let grouping = AxNode::new(
            "task-rail-grouping",
            AxRole::Button,
            self.grouping.accessible_name(),
            AxRect::new(
                (frame.width - inset - metrics::RAIL_ICON_BUTTON_SIZE).max(inset),
                // 标题行高 36、按钮 28：render items_center → 按钮顶 +4。
                y + (metrics::RAIL_TITLE_ROW_HEIGHT - metrics::RAIL_ICON_BUTTON_SIZE) / 2.0,
                metrics::RAIL_ICON_BUTTON_SIZE,
                metrics::RAIL_ICON_BUTTON_SIZE,
            ),
        )
        .value(match self.grouping {
            TaskRailGrouping::Timeline => "Timeline",
            TaskRailGrouping::Projects => "Projects",
        })
        // R7 Wave A：菜单打开时 AX 焦点移交高亮项（macOS 菜单惯例），
        // 触发器让出 focused，树内保持唯一焦点。
        .focused(self.open_menu.is_none() && self.grouping_focus.is_focused(window))
        .action(AxAction::Press);
        y += metrics::RAIL_TITLE_ROW_HEIGHT + metrics::RAIL_TITLE_SCOPE_GAP;
        let scope_label = match &self.scope_workspace_id {
            None => "All projects".into(),
            Some(id) => self.projection.workspace_name(Some(id)),
        };
        let scope = AxNode::new(
            "project-scope",
            AxRole::Button,
            "Project scope",
            AxRect::new(
                inset,
                y,
                (frame.width - inset * 2.0).max(0.0),
                metrics::RAIL_TOP_ROW_HEIGHT,
            ),
        )
        .value(scope_label)
        // R7 Wave A：scope 菜单打开时 AX 焦点移交高亮项（同 grouping）。
        .focused(self.open_menu.is_none() && self.scope_focus.is_focused(window))
        .action(AxAction::Press);
        y += metrics::RAIL_TOP_ROW_HEIGHT + metrics::RAIL_SCOPE_CONNECTION_GAP;
        let connection = AxNode::new(
            "connection-status",
            AxRole::StaticText,
            "Connection",
            AxRect::new(
                inset,
                y,
                (frame.width
                    - inset * 2.0
                    - metrics::RAIL_ICON_BUTTON_SIZE
                    - metrics::RAIL_CONNECTION_ADD_GAP)
                    .max(0.0),
                metrics::RAIL_TOP_ROW_HEIGHT,
            ),
        )
        // 与 render 同源（ADR-042）：连接行可见文案带 Local 前缀与 resume 相位。
        .value(self.connection_status_label());
        let add_task = AxNode::new(
            "add-task",
            AxRole::Button,
            "New task",
            AxRect::new(
                (frame.width - inset - metrics::RAIL_ICON_BUTTON_SIZE).max(inset),
                y + (metrics::RAIL_TOP_ROW_HEIGHT - metrics::RAIL_ICON_BUTTON_SIZE) / 2.0,
                metrics::RAIL_ICON_BUTTON_SIZE,
                metrics::RAIL_ICON_BUTTON_SIZE,
            ),
        )
        .description(self.add_task_disabled_reason())
        .enabled(can_create)
        .focused(self.open_menu.is_none() && self.add_task_focus.is_focused(window))
        .action(AxAction::Press);
        y += metrics::RAIL_TOP_ROW_HEIGHT;

        let mut sidebar = AxNode::new("task-rail", AxRole::Group, "Tasks", frame)
            .child(grouping)
            .child(scope)
            .child(connection)
            .child(add_task);
        // 与可见路径同源：Reconnect 仅 Disconnected / ConnectFailed 发布
        // （projection.show_reconnect()，同 task_rail.rs 视觉谓词）。
        if self.projection.show_reconnect() {
            // render 侧 Reconnect 包在 mt_2(8) 容器里：rect 前先补 8px 上距，
            // 否则按钮 frame 比可见位置高 8px（ADR-042 同源约束）。
            y += PAD;
            sidebar = sidebar.child(
                AxNode::new(
                    "reconnect",
                    AxRole::Button,
                    "Reconnect",
                    AxRect::new(
                        inset,
                        y,
                        (frame.width - inset * 2.0).max(0.0),
                        metrics::RAIL_TOP_ROW_HEIGHT,
                    ),
                )
                .focused(self.open_menu.is_none() && self.reconnect_focus.is_focused(window))
                .action(AxAction::Press),
            );
            y += metrics::RAIL_TOP_ROW_HEIGHT;
        }

        let list_top = y + metrics::RAIL_LIST_TOP_GAP;
        let list_height = (frame.height - list_top - CONTROL_HEIGHT).max(0.0);
        let list_width = (frame.width - inset * 2.0).max(0.0);
        let mut list = AxNode::new(
            "session-list",
            AxRole::List,
            "Sessions",
            AxRect::new(inset, list_top, list_width, list_height),
        );
        // 与可见 TaskRail（task_rail.rs）同一结构：Timeline = 日期组 → 项目块，
        // Projects = 项目块；折叠的项目只投影头部，不投影其子会话。行高与
        // 组间距走 theme::metrics（桶头距上组 42 / 项目块间 8）。
        let mut row_y = list_top;
        match self.grouping {
            TaskRailGrouping::Timeline => {
                for (group_index, group) in self
                    .projection
                    .timeline_groups(self.scope_workspace_id.as_deref(), crate::ui::now_unix_ms())
                    .into_iter()
                    .enumerate()
                {
                    if group_index > 0 {
                        row_y += metrics::RAIL_BUCKET_TOP_GAP;
                    }
                    list = list.child(AxNode::new(
                        dynamic_identifier("date-group", group.bucket.label()),
                        AxRole::StaticText,
                        group.bucket.label(),
                        AxRect::new(inset, row_y, list_width, metrics::RAIL_BUCKET_HEADER_HEIGHT),
                    ));
                    row_y += metrics::RAIL_BUCKET_HEADER_HEIGHT;
                    for (project_index, project) in group.projects.iter().enumerate() {
                        // 与 render 同源：桶头→首项目 2，项目块间 8。
                        row_y += if project_index == 0 {
                            metrics::RAIL_BUCKET_TO_PROJECT_GAP
                        } else {
                            metrics::RAIL_PROJECT_BLOCK_GAP
                        };
                        let (nodes, consumed) = self.project_ax_nodes(
                            window,
                            project,
                            Some(group.bucket),
                            row_y,
                            list_width,
                            can_create,
                        );
                        row_y += consumed;
                        for node in nodes {
                            list = list.child(node);
                        }
                    }
                }
            }
            TaskRailGrouping::Projects => {
                for (project_index, project) in self
                    .projection
                    .project_groups(self.scope_workspace_id.as_deref())
                    .iter()
                    .enumerate()
                {
                    if project_index > 0 {
                        row_y += metrics::RAIL_PROJECT_BLOCK_GAP;
                    }
                    let (nodes, consumed) =
                        self.project_ax_nodes(window, project, None, row_y, list_width, can_create);
                    row_y += consumed;
                    for node in nodes {
                        list = list.child(node);
                    }
                }
            }
        }
        sidebar = sidebar.child(list);

        if matches!(self.open_menu, Some(MenuKind::Grouping)) {
            // 浮层贴 grouping 角标下方：标题行顶 = PAD + 36 安全区 + PAD，
            // 行高 36；旧 CONTROL_HEIGHT=28 的锚点会把菜单抬到 traffic-light 带。
            let grouping_menu_y = PAD
                + shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT
                + PAD
                + metrics::RAIL_TITLE_ROW_HEIGHT;
            let grouping_menu_x = (frame.width - inset - 148.0).max(inset);
            let highlight = self.menu_highlight_effective(match self.grouping {
                TaskRailGrouping::Timeline => 0,
                TaskRailGrouping::Projects => 1,
            });
            sidebar = sidebar.child(
                AxNode::new(
                    "grouping-menu",
                    AxRole::Group,
                    "Task grouping",
                    AxRect::new(grouping_menu_x, grouping_menu_y, 148.0, 64.0),
                )
                .child(
                    AxNode::new(
                        "group-timeline",
                        AxRole::Button,
                        "Timeline",
                        AxRect::new(grouping_menu_x, grouping_menu_y, 148.0, 32.0),
                    )
                    .selected(self.grouping == TaskRailGrouping::Timeline)
                    .focused(highlight == 0)
                    .action(AxAction::Press),
                )
                .child(
                    AxNode::new(
                        "group-projects",
                        AxRole::Button,
                        "Projects",
                        AxRect::new(grouping_menu_x, grouping_menu_y + 32.0, 148.0, 32.0),
                    )
                    .selected(self.grouping == TaskRailGrouping::Projects)
                    .focused(highlight == 1)
                    .action(AxAction::Press),
                ),
            );
        }
        if matches!(self.open_menu, Some(MenuKind::Scope)) {
            let scope_menu_y = PAD
                + shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT
                + PAD
                + metrics::RAIL_TITLE_ROW_HEIGHT
                + metrics::RAIL_TITLE_SCOPE_GAP
                + metrics::RAIL_TOP_ROW_HEIGHT;
            let mut menu = AxNode::new(
                "scope-menu",
                AxRole::Group,
                "Project scope options",
                AxRect::new(inset, scope_menu_y, list_width, 200.0),
            );
            let options = self.projection.project_scope_options();
            let highlight = self.menu_highlight_effective(
                options
                    .iter()
                    .position(|(workspace_id, _)| *workspace_id == self.scope_workspace_id)
                    .unwrap_or(0),
            );
            for (ix, (workspace_id, label)) in self
                .projection
                .project_scope_options()
                .into_iter()
                .enumerate()
            {
                let selected = self.scope_workspace_id == workspace_id;
                menu = menu.child(
                    AxNode::new(
                        scope_identifier(workspace_id.as_deref()),
                        AxRole::Button,
                        label,
                        AxRect::new(
                            inset,
                            scope_menu_y + ix as f32 * ROW_HEIGHT,
                            list_width,
                            ROW_HEIGHT,
                        ),
                    )
                    .selected(selected)
                    .focused(ix == highlight)
                    .action(AxAction::Press),
                );
            }
            let add_ix = options.len();
            menu = menu.child(
                AxNode::new(
                    "scope-add-project",
                    AxRole::Button,
                    "Add project…",
                    AxRect::new(
                        inset,
                        scope_menu_y + add_ix as f32 * ROW_HEIGHT,
                        list_width,
                        ROW_HEIGHT,
                    ),
                )
                .focused(add_ix == highlight)
                .action(AxAction::Press),
            );
            sidebar = sidebar.child(menu);
        }
        sidebar
    }

    /// 当前分组模式下的项目块序列（Timeline 带日期桶）。AX 树构建与 Press
    /// 白名单共用同一来源，保证 identifier 一致。
    fn rail_project_entries(&self) -> Vec<(Option<DateBucket>, TaskRailProjectGroup)> {
        match self.grouping {
            TaskRailGrouping::Timeline => self
                .projection
                .timeline_groups(self.scope_workspace_id.as_deref(), crate::ui::now_unix_ms())
                .into_iter()
                .flat_map(|group| {
                    let bucket = group.bucket;
                    group
                        .projects
                        .into_iter()
                        .map(move |project| (Some(bucket), project))
                        .collect::<Vec<_>>()
                })
                .collect(),
            TaskRailGrouping::Projects => self
                .projection
                .project_groups(self.scope_workspace_id.as_deref())
                .into_iter()
                .map(|project| (None, project))
                .collect(),
        }
    }

    /// 项目块 AX 投影（对齐 task_rail.rs project_block）：折叠头 + 项目内新建
    /// 按钮 + 展开时的会话行。返回（节点序列, 占用高度）。Timeline 模式下同一
    /// 项目可出现于多个日期组，identifier 以日期桶限定保证全树唯一。
    fn project_ax_nodes(
        &self,
        window: &Window,
        project: &TaskRailProjectGroup,
        bucket: Option<DateBucket>,
        top: f32,
        width: f32,
        can_create: bool,
    ) -> (Vec<AxNode>, f32) {
        let inset = metrics::RAIL_CONTENT_INSET;
        let key = project_key(project.workspace_id.as_deref());
        let expanded = !self.collapsed_projects.contains(&key);
        let header_focus_key = rail_project_occurrence_key("project", bucket, &key);
        let mut nodes = vec![AxNode::new(
            rail_project_identifier(bucket, &key),
            AxRole::Button,
            project.name.clone(),
            AxRect::new(inset, top, width, metrics::RAIL_TASK_ROW_HEIGHT),
        )
        .value(format!("{} tasks", project.task_count()))
        .description(if expanded { "Expanded" } else { "Collapsed" })
        .focused(
            self.open_menu.is_none()
                && self
                    .rail_row_focus
                    .get(&header_focus_key)
                    .is_some_and(|handle| handle.is_focused(window)),
        )
        .action(AxAction::Press)];
        if !project.is_unassigned() && project.workspace_id.is_some() {
            let add_focus_key = rail_project_occurrence_key("project-add", bucket, &key);
            nodes.push(
                AxNode::new(
                    rail_project_add_identifier(bucket, &key),
                    AxRole::Button,
                    format!("New task in {}", project.name),
                    AxRect::new(
                        inset + (width - metrics::RAIL_ICON_BUTTON_SIZE).max(0.0),
                        top + (metrics::RAIL_TASK_ROW_HEIGHT - metrics::RAIL_ICON_BUTTON_SIZE)
                            / 2.0,
                        metrics::RAIL_ICON_BUTTON_SIZE,
                        metrics::RAIL_ICON_BUTTON_SIZE,
                    ),
                )
                .enabled(can_create)
                .focused(
                    self.open_menu.is_none()
                        && self
                            .rail_row_focus
                            .get(&add_focus_key)
                            .is_some_and(|handle| handle.is_focused(window)),
                )
                .action(AxAction::Press),
            );
        }
        // 与 render 同源：项目头 → 首个任务行 2，任务行间 0。
        let mut consumed = metrics::RAIL_TASK_ROW_HEIGHT;
        if expanded && !project.tasks.is_empty() {
            consumed += metrics::RAIL_PROJECT_TO_TASK_GAP;
        }
        if expanded {
            for session in &project.tasks {
                // 状态词与可见状态点同源（ADR-042）：Needs input 优先于
                // Running；无 live 状态不声明语义（不伪造终态）。
                let status = self.projection.session_live_status(&session.session_id);
                let unread = self.projection.session_unread(&session.session_id);
                let focus_key = rail_session_focus_key(&session.session_id);
                nodes.push(
                    AxNode::new(
                        session_identifier(&session.session_id),
                        AxRole::ListItem,
                        session.title.clone(),
                        AxRect::new(inset, top + consumed, width, metrics::RAIL_TASK_ROW_HEIGHT),
                    )
                    .description(session_status_description(status, unread))
                    .focused(
                        self.open_menu.is_none()
                            && self
                                .rail_row_focus
                                .get(&focus_key)
                                .is_some_and(|handle| handle.is_focused(window)),
                    )
                    .selected(
                        self.projection.active_session_id.as_deref()
                            == Some(session.session_id.as_str()),
                    )
                    .action(AxAction::Press),
                );
                consumed += metrics::RAIL_TASK_ROW_HEIGHT;
            }
        }
        (nodes, consumed)
    }

    fn workspace_ax(
        &self,
        window: &Window,
        cx: &App,
        frame: AxRect,
        inspector_open: bool,
    ) -> AxNode {
        let input_height = (f32::from(window.line_height())
            * self.text_input.read(cx).visual_line_count() as f32
            + metrics::COMPOSER_TEXT_INSET)
            .clamp(
                metrics::COMPOSER_INPUT_MIN_HEIGHT,
                Self::composer_input_ax_max(),
            );
        let composer_height = Self::composer_panel_height(input_height).min(frame.height);
        let header_height = metrics::HEADER_HEIGHT.min(frame.height);
        let timeline_height = (frame.height - composer_height - header_height).max(0.0);
        AxNode::new("workspace", AxRole::Group, "Workspace", frame)
            .child(self.header_ax(
                window,
                AxRect::new(frame.x, frame.y, frame.width, header_height),
                inspector_open,
            ))
            .child(self.timeline_ax(
                window,
                AxRect::new(
                    frame.x,
                    frame.y + header_height,
                    frame.width,
                    timeline_height,
                ),
            ))
            .child(self.composer_ax(
                window,
                cx,
                AxRect::new(
                    frame.x,
                    frame.y + header_height + timeline_height,
                    frame.width,
                    composer_height,
                ),
            ))
    }

    /// F-05 Header 语义树（与 render 同源谓词 / metrics）：标题 / branch /
    /// live 终态 / 新建任务按钮。各项可见条件与 render 完全一致（无数据
    /// 诚实隐藏；几何共享 HEADER_* 常量，文本宽度为近似值）。
    fn header_ax(&self, window: &Window, frame: AxRect, inspector_open: bool) -> AxNode {
        let content_top = frame.y + metrics::HEADER_SAFE_STRIP;
        let content_height = (frame.height - metrics::HEADER_SAFE_STRIP).max(0.0);
        let row_height = metrics::HEADER_STATUS_DOT_SIZE + 14.0;
        let row_y = content_top + ((content_height - row_height) / 2.0).max(0.0);
        let mut header = AxNode::new("workspace-header", AxRole::Group, "Workspace header", frame);
        let mut x = frame.x + metrics::TIMELINE_CONTENT_INSET;
        if let Some(title) = self.projection.workspace_header_title() {
            let width = 340.0_f32.min((frame.x + frame.width - x).max(0.0));
            header = header.child(AxNode::new(
                "header-title",
                AxRole::StaticText,
                title,
                AxRect::new(x, row_y, width, row_height),
            ));
            x += width + metrics::HEADER_TITLE_META_GAP;
        }
        if let Some(branch) = self.header_branch() {
            let width = 120.0_f32.min((frame.x + frame.width - x).max(0.0));
            header = header.child(
                AxNode::new(
                    "header-branch",
                    AxRole::StaticText,
                    branch,
                    AxRect::new(x, row_y, width, row_height),
                )
                .description("Git branch"),
            );
            x += width + 24.0;
        }
        if let Some(status) = self.projection.workspace_header_status() {
            let width = 150.0_f32.min((frame.x + frame.width - x).max(0.0));
            header = header.child(
                AxNode::new(
                    "header-status",
                    AxRole::StaticText,
                    status.label(),
                    AxRect::new(x, row_y, width, row_height),
                )
                .description("Live status"),
            );
        }
        let action = header_action_ax_rect(frame);
        // R6 Wave A（F-12）：与 render 同用 activity_header_visibility 口径；
        // 折叠态 Activity 占 Header 最右动作槽，浮层右缘与触发器右缘对齐；
        // 展开态该槽恢复 New task。
        let (trigger_visible, popover_visible) = activity_header_visibility(
            inspector_open,
            matches!(self.open_menu, Some(MenuKind::Activity)),
        );
        if trigger_visible {
            header = header.child(
                AxNode::new("inspector-toggle", AxRole::Button, "Activity", action)
                    .focused(
                        self.open_menu.is_none()
                            && self.inspector_activity_focus.is_focused(window),
                    )
                    .action(AxAction::Press),
            );
            if popover_visible {
                let geometry = activity_popover_ax_geometry(frame, action);
                header = header.child(
                    AxNode::new(
                        "activity-popover",
                        AxRole::Group,
                        "Activity",
                        geometry.frame,
                    )
                    .child(AxNode::new(
                        "activity-changes-heading",
                        AxRole::StaticText,
                        "Changes",
                        geometry.heading,
                    ))
                    .child(
                        AxNode::new(
                            "activity-open-changes",
                            AxRole::Button,
                            "Open changes",
                            geometry.open_changes,
                        )
                        .value(self.changes.activity_summary())
                        .focused(self.menu_highlight_effective(0) == 0)
                        .action(AxAction::Press),
                    ),
                );
            }
            header
        } else {
            header.child(
                AxNode::new("header-new-task", AxRole::Button, "New task", action)
                    .enabled(self.can_create_task())
                    .focused(
                        self.open_menu.is_none() && self.header_new_task_focus.is_focused(window),
                    )
                    .action(AxAction::Press),
            )
        }
    }

    fn timeline_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let rows = self.projection.timeline_rows();
        let total = rows.len();
        let empty_hint_visible = self.projection.workspace_empty_hint_visible();
        let capacity = ((frame.height / TIMELINE_ROW_HEIGHT).ceil() as usize).max(1);
        let start = if self.timeline_following {
            total.saturating_sub(capacity)
        } else {
            self.timeline_list.logical_scroll_top().item_ix.min(total)
        };
        let end = (start + capacity).min(total);
        let mut list = AxNode::new("timeline", AxRole::List, "Timeline", frame);
        if empty_hint_visible {
            // 空态引导只读节点：与 timeline_area 可见条件同源（projection
            // 谓词），无 action；垂直居中，宽度占满 timeline 区。
            let hint_y = frame.y + ((frame.height - ROW_HEIGHT) / 2.0).max(0.0);
            list = list.child(AxNode::new(
                "workspace-empty-hint",
                AxRole::StaticText,
                WORKSPACE_EMPTY_HINT,
                AxRect::new(frame.x, hint_y, frame.width, ROW_HEIGHT),
            ));
        }
        for (visible_ix, row) in rows[start..end].iter().enumerate() {
            let rect = AxRect::new(
                frame.x + PAD,
                frame.y + PAD + visible_ix as f32 * TIMELINE_ROW_HEIGHT,
                (frame.width - PAD * 2.0).max(0.0),
                TIMELINE_ROW_HEIGHT,
            );
            list = list.child(self.timeline_row_ax(window, row, rect));
        }
        if let Some(pending) = self.projection.pending_approval.as_ref() {
            let approval_height = 112.0_f32.min(frame.height);
            let approval = AxRect::new(
                frame.x + PAD,
                (frame.y + frame.height - approval_height - PAD).max(frame.y),
                (frame.width - PAD * 2.0).max(0.0),
                approval_height,
            );
            let enabled = self.can_approve();
            list = list.child(
                AxNode::new("approval-card", AxRole::Group, "Approval", approval)
                    .value(format!("{} · {}", pending.tool_name, pending.reason))
                    .child(
                        AxNode::new(
                            "approve-once",
                            AxRole::Button,
                            "Allow once",
                            AxRect::new(approval.x, approval.y + 72.0, 104.0, 32.0),
                        )
                        .enabled(enabled)
                        .focused(
                            self.open_menu.is_none() && self.approve_once_focus.is_focused(window),
                        )
                        .action(AxAction::Press),
                    )
                    .child(
                        AxNode::new(
                            "approve-for-run",
                            AxRole::Button,
                            "Allow for run",
                            AxRect::new(approval.x + 112.0, approval.y + 72.0, 116.0, 32.0),
                        )
                        .enabled(enabled)
                        .focused(
                            self.open_menu.is_none()
                                && self.approve_for_run_focus.is_focused(window),
                        )
                        .action(AxAction::Press),
                    )
                    .child(
                        AxNode::new(
                            "approve-deny",
                            AxRole::Button,
                            "Deny",
                            AxRect::new(approval.x + 236.0, approval.y + 72.0, 72.0, 32.0),
                        )
                        .enabled(enabled)
                        .focused(self.open_menu.is_none() && self.deny_focus.is_focused(window))
                        .action(AxAction::Press),
                    ),
            );
        }
        if !self.timeline_following {
            list = list.child(
                AxNode::new(
                    "timeline-back-to-bottom",
                    AxRole::Button,
                    "Back to bottom",
                    AxRect::new(
                        frame.x + frame.width - 140.0,
                        frame.y + frame.height - 40.0,
                        132.0,
                        32.0,
                    ),
                )
                .focused(
                    self.open_menu.is_none()
                        && self.timeline_back_to_bottom_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );
        }
        list
    }

    /// 渲染行 → AX 节点（与 timeline.rs 组装同源）。消息 / 错误条目保持
    /// 既有 timeline-entry / entry-menu / fork identifier；tool 组与 Run
    /// 摘要区域为新结构节点。
    fn timeline_row_ax(&self, window: &Window, row: &TimelineRow, rect: AxRect) -> AxNode {
        match row {
            TimelineRow::Message { entry_index } => {
                self.timeline_entry_ax(window, &self.projection.timeline[*entry_index], rect, true)
            }
            TimelineRow::Error { entry_index } => {
                self.timeline_entry_ax(window, &self.projection.timeline[*entry_index], rect, true)
            }
            // 中间相位单行（§4.5）：无「···」菜单（非 fork 边界，原菜单
            // 亦不可用），只保留条目语义。
            TimelineRow::RunPhase { entry_index } => {
                self.timeline_entry_ax(window, &self.projection.timeline[*entry_index], rect, false)
            }
            TimelineRow::ToolGroup { entry_indices } => {
                let mut group = AxNode::new(
                    dynamic_identifier(
                        "tool-group",
                        &self.projection.timeline[entry_indices[0]].event_id,
                    ),
                    AxRole::Group,
                    format!("Tool activity · {} tools", entry_indices.len()),
                    rect,
                );
                for (ix, &entry_index) in entry_indices.iter().enumerate() {
                    let entry = &self.projection.timeline[entry_index];
                    let TimelineEntryKind::ToolCall {
                        name,
                        status,
                        detail,
                    } = &entry.kind
                    else {
                        continue;
                    };
                    let tool_rect = AxRect::new(
                        rect.x,
                        rect.y + ix as f32 * metrics::TOOL_ROW_HEIGHT,
                        rect.width,
                        metrics::TOOL_ROW_HEIGHT,
                    );
                    group = group.child(
                        AxNode::new(
                            dynamic_identifier("tool-row", &entry.event_id),
                            AxRole::ListItem,
                            format!("Tool · {name}"),
                            tool_rect,
                        )
                        .value(timeline::tool_status_label(status))
                        .description(detail.clone().unwrap_or_default()),
                    );
                }
                group
            }
            TimelineRow::RunSummary { group, terminal } => {
                let terminal_entry = &self.projection.timeline[*terminal];
                let now_ms = crate::ui::now_unix_ms();
                let mut region = AxNode::new(
                    dynamic_identifier("run-summary", &terminal_entry.event_id),
                    AxRole::Group,
                    "Run summary",
                    rect,
                );
                let mut y = rect.y;
                if let Some(entry_indices) = group {
                    for &entry_index in entry_indices {
                        let entry = &self.projection.timeline[entry_index];
                        let TimelineEntryKind::ToolCall {
                            name,
                            status,
                            detail,
                        } = &entry.kind
                        else {
                            continue;
                        };
                        region = region.child(
                            AxNode::new(
                                dynamic_identifier("tool-row", &entry.event_id),
                                AxRole::ListItem,
                                format!("Tool · {name}"),
                                AxRect::new(rect.x, y, rect.width, metrics::TOOL_ROW_HEIGHT),
                            )
                            .value(timeline::tool_status_label(status))
                            .description(detail.clone().unwrap_or_default()),
                        );
                        y += metrics::TOOL_ROW_HEIGHT;
                    }
                    y += metrics::SUMMARY_CARD_GAP;
                }
                let (title, description) =
                    run_summary_texts(terminal_entry).unwrap_or(("Run", String::new()));
                region = region.child(
                    AxNode::new(
                        dynamic_identifier("run-summary-card", &terminal_entry.event_id),
                        AxRole::StaticText,
                        title,
                        AxRect::new(rect.x, y, rect.width, metrics::SUMMARY_CHECK_CIRCLE),
                    )
                    .description(description),
                );
                let review_enabled = terminal_entry.fork_boundary == Some(ForkBoundary::Completed)
                    && self.changes_available_for_active();
                region = region.child(
                    AxNode::new(
                        run_review_identifier(&terminal_entry.event_id),
                        AxRole::Button,
                        "Review changes",
                        AxRect::new(
                            (rect.x + rect.width - metrics::SUMMARY_BUTTON_WIDTH).max(rect.x),
                            y,
                            metrics::SUMMARY_BUTTON_WIDTH,
                            metrics::SUMMARY_BUTTON_HEIGHT,
                        ),
                    )
                    .enabled(review_enabled)
                    .focused(
                        self.open_menu.is_none()
                            && self
                                .timeline_review_changes_focus
                                .get(&terminal_entry.event_id)
                                .is_some_and(|focus| focus.is_focused(window)),
                    )
                    .action(AxAction::Press),
                );
                y += metrics::SUMMARY_CHECK_CIRCLE + metrics::TIMELINE_FOOTER_GAP;
                if let Some(label) = run_footer_label(terminal_entry) {
                    region = region.child(AxNode::new(
                        dynamic_identifier("run-footer", &terminal_entry.event_id),
                        AxRole::StaticText,
                        format!(
                            "{label} · {}",
                            display_time(&terminal_entry.timestamp, now_ms)
                        ),
                        AxRect::new(rect.x, y, rect.width, ROW_HEIGHT),
                    ));
                }
                // 终态条目保留「···」fork 菜单语义（identifier 冻结）。
                let menu_row = AxRect::new(
                    rect.x + rect.width - 32.0,
                    rect.y + rect.height - 28.0,
                    32.0,
                    28.0,
                );
                let mut entry_node = AxNode::new(
                    dynamic_identifier("timeline-entry", &terminal_entry.event_id),
                    AxRole::ListItem,
                    "Run",
                    rect,
                )
                .value(run_footer_label(terminal_entry).unwrap_or_default())
                .description(display_time(&terminal_entry.timestamp, now_ms))
                .child(
                    AxNode::new(
                        entry_menu_identifier(&terminal_entry.event_id),
                        AxRole::Button,
                        "Entry actions",
                        menu_row,
                    )
                    .focused(
                        self.open_menu.is_none()
                            && self
                                .timeline_entry_action_focus
                                .get(&terminal_entry.event_id)
                                .is_some_and(|focus| focus.is_focused(window)),
                    )
                    .action(AxAction::Press),
                );
                if matches!(&self.open_menu, Some(MenuKind::Entry(id)) if id == &terminal_entry.event_id)
                {
                    let fork_node = AxNode::new(
                        fork_identifier(&terminal_entry.event_id),
                        AxRole::Button,
                        "Fork",
                        AxRect::new(menu_row.x - 80.0, menu_row.y + 28.0, 112.0, 30.0),
                    )
                    .enabled(
                        matches!(
                            self.projection.connection,
                            ConnectionState::Connected { .. }
                        ) && self.projection.active_session_id.is_some()
                            && terminal_entry.is_fork_boundary(),
                    )
                    .focused(self.menu_highlight_effective(0) == 0)
                    .action(AxAction::Press);
                    entry_node = entry_node.child(fork_node);
                }
                region.child(entry_node)
            }
        }
    }

    /// 消息 / 错误 / 中间相位条目节点（identifier 与迁移前一致）。
    fn timeline_entry_ax(
        &self,
        window: &Window,
        entry: &TimelineEntry,
        row: AxRect,
        with_menu: bool,
    ) -> AxNode {
        let now_ms = crate::ui::now_unix_ms();
        let (label, value) = timeline_accessible_text(entry);
        let mut node = AxNode::new(
            dynamic_identifier("timeline-entry", &entry.event_id),
            AxRole::ListItem,
            label,
            row,
        )
        .value(value)
        .description(display_time(&entry.timestamp, now_ms));
        if with_menu {
            node = node.child(
                AxNode::new(
                    entry_menu_identifier(&entry.event_id),
                    AxRole::Button,
                    "Entry actions",
                    AxRect::new(row.x + row.width - 32.0, row.y, 32.0, 28.0),
                )
                .focused(
                    self.open_menu.is_none()
                        && self
                            .timeline_entry_action_focus
                            .get(&entry.event_id)
                            .is_some_and(|focus| focus.is_focused(window)),
                )
                .action(AxAction::Press),
            );
            if matches!(&self.open_menu, Some(MenuKind::Entry(id)) if id == &entry.event_id) {
                node = node.child(
                    AxNode::new(
                        fork_identifier(&entry.event_id),
                        AxRole::Button,
                        "Fork",
                        AxRect::new(row.x + row.width - 112.0, row.y + 28.0, 112.0, 30.0),
                    )
                    .enabled(
                        matches!(
                            self.projection.connection,
                            ConnectionState::Connected { .. }
                        ) && self.projection.active_session_id.is_some()
                            && entry.is_fork_boundary(),
                    )
                    .focused(self.menu_highlight_effective(0) == 0)
                    .action(AxAction::Press),
                );
            }
        }
        node
    }

    fn composer_input_ax_max() -> f32 {
        (metrics::COMPOSER_PANEL_MAX_HEIGHT
            - metrics::COMPOSER_BORDER
            - metrics::COMPOSER_PAD * 2.0
            - metrics::COMPOSER_GAP
            - metrics::COMPOSER_SEND_SIZE)
            .max(metrics::COMPOSER_INPUT_MIN_HEIGHT)
    }

    fn composer_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let pad = metrics::COMPOSER_PAD;
        let input_y = frame.y + pad;
        let footer_y = frame.y + frame.height - pad - metrics::COMPOSER_SEND_SIZE;
        let input_height =
            (footer_y - metrics::COMPOSER_GAP - input_y).max(metrics::COMPOSER_INPUT_MIN_HEIGHT);
        let action_x = frame.x + frame.width - pad - metrics::COMPOSER_SEND_SIZE;
        let input_width = (action_x - pad - (frame.x + pad)).max(0.0);
        let running = self.projection.active_run_id.is_some();
        let current_model = self
            .projection
            .effective_model()
            .map(|(provider, model)| format!("{provider} / {model}"))
            .unwrap_or_else(|| "No model".into());
        let input_focus = self.text_input.read(cx).focus_handle(cx);
        // AXValue 恒为纯文本：空输入即空串，placeholder 不得回退进 value
        // （R4 U2 composer-cleared / R5 r5-1 契约）。
        let input_value = self.text_input.read(cx).text().to_string();
        let mut composer = AxNode::new("composer", AxRole::Group, "Composer", frame)
            .child(
                AxNode::new(
                    "composer-input",
                    AxRole::TextArea,
                    "Message",
                    AxRect::new(frame.x + pad, input_y, input_width, input_height),
                )
                .value(input_value)
                .focused(self.open_menu.is_none() && input_focus.is_focused(window))
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "model-picker",
                    AxRole::Button,
                    "Model",
                    AxRect::new(
                        frame.x + pad,
                        footer_y,
                        220.0_f32.min(frame.width),
                        metrics::COMPOSER_FOOTER_CONTROL,
                    ),
                )
                .value(current_model)
                .enabled(self.can_switch_model())
                // R7 Wave A：model 菜单打开时 AX 焦点移交高亮项（同 grouping）。
                .focused(self.open_menu.is_none() && self.model_focus.is_focused(window))
                .action(AxAction::Press),
            );
        if let Some(hint) = self.status_hint.as_ref() {
            let hint_x = frame.x + pad + 228.0;
            let hint_width = (action_x - pad - hint_x).max(0.0);
            composer = composer.child(
                AxNode::new(
                    "composer-status-hint",
                    AxRole::StaticText,
                    "Status",
                    AxRect::new(
                        hint_x,
                        footer_y,
                        hint_width,
                        metrics::COMPOSER_FOOTER_CONTROL,
                    ),
                )
                .value(hint.clone()),
            );
        }
        let action_rect = AxRect::new(
            action_x,
            footer_y,
            metrics::COMPOSER_SEND_SIZE,
            metrics::COMPOSER_SEND_SIZE,
        );
        if running {
            composer = composer.child(
                AxNode::new("cancel", AxRole::Button, "Cancel run", action_rect)
                    .enabled(self.can_cancel())
                    .focused(
                        self.open_menu.is_none() && self.composer_action_focus.is_focused(window),
                    )
                    .action(AxAction::Press),
            );
        } else {
            composer = composer.child(
                AxNode::new("send", AxRole::Button, "Send", action_rect)
                    .enabled(self.can_send(cx))
                    .focused(
                        self.open_menu.is_none() && self.composer_action_focus.is_focused(window),
                    )
                    .action(AxAction::Press),
            );
        }
        if matches!(self.open_menu, Some(MenuKind::Model)) {
            let selected_ix = self
                .projection
                .effective_model()
                .and_then(|(provider, id)| {
                    self.projection
                        .models
                        .iter()
                        .position(|model| model.provider_id == *provider && model.id == *id)
                })
                .unwrap_or(0);
            let highlight = self.menu_highlight_effective(selected_ix);
            let mut menu = AxNode::new(
                "model-menu",
                AxRole::Group,
                "Models",
                AxRect::new(
                    frame.x + pad,
                    footer_y + metrics::COMPOSER_SEND_SIZE,
                    260.0,
                    240.0,
                ),
            );
            for (ix, model) in self.projection.models.iter().enumerate() {
                let selected = self
                    .projection
                    .effective_model()
                    .is_some_and(|current| current.0 == model.provider_id && current.1 == model.id);
                menu = menu.child(
                    AxNode::new(
                        model_identifier(model),
                        AxRole::Button,
                        model.display_name.clone(),
                        AxRect::new(
                            frame.x + pad,
                            footer_y + metrics::COMPOSER_SEND_SIZE + ix as f32 * ROW_HEIGHT,
                            260.0,
                            ROW_HEIGHT,
                        ),
                    )
                    .value(format!("{} / {}", model.provider_id, model.id))
                    .enabled(self.can_switch_model())
                    .selected(selected)
                    .focused(ix == highlight)
                    .action(AxAction::Press),
                );
            }
            composer = composer.child(menu);
        }
        if matches!(self.open_menu, Some(MenuKind::WorkspaceConfirm)) {
            let highlight = self.menu_highlight_effective(0);
            let mut menu = AxNode::new(
                "workspace-confirm",
                AxRole::Group,
                "Choose workspace",
                AxRect::new(
                    frame.x + pad,
                    footer_y + metrics::COMPOSER_SEND_SIZE,
                    280.0,
                    220.0,
                ),
            );
            for (ix, workspace) in self
                .projection
                .project_scope_options()
                .into_iter()
                .filter_map(|(id, name)| id.map(|id| (id, name)))
                .enumerate()
            {
                menu = menu.child(
                    AxNode::new(
                        workspace_confirm_identifier(&workspace.0),
                        AxRole::Button,
                        workspace.1,
                        AxRect::new(
                            frame.x + pad,
                            footer_y + metrics::COMPOSER_SEND_SIZE + ix as f32 * ROW_HEIGHT,
                            280.0,
                            ROW_HEIGHT,
                        ),
                    )
                    .focused(ix == highlight)
                    .action(AxAction::Press),
                );
            }
            let add_ix = self
                .projection
                .project_scope_options()
                .into_iter()
                .filter(|(id, _)| id.is_some())
                .count();
            menu = menu.child(
                AxNode::new(
                    "workspace-confirm-add-project",
                    AxRole::Button,
                    "Add project…",
                    AxRect::new(
                        frame.x + pad,
                        footer_y + metrics::COMPOSER_SEND_SIZE + add_ix as f32 * ROW_HEIGHT,
                        280.0,
                        ROW_HEIGHT,
                    ),
                )
                .focused(add_ix == highlight)
                .action(AxAction::Press),
            );
            composer = composer.child(menu);
        }
        composer
    }

    fn inspector_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let tab_width = metrics::INSPECTOR_TAB_WIDTH;
        let strip_height = metrics::INSPECTOR_TAB_HEIGHT;
        let tab_x = frame.x + 12.0;
        let collapse_y = frame.y + ((strip_height - CONTROL_HEIGHT) / 2.0).max(0.0);
        let mut inspector = AxNode::new("inspector", AxRole::Group, "Inspector", frame)
            .child(
                AxNode::new(
                    "inspector-tabs",
                    AxRole::TabGroup,
                    "Inspector tabs",
                    AxRect::new(tab_x, frame.y, tab_width * 3.0, strip_height),
                )
                .child(
                    AxNode::new(
                        "inspector-tab-changes",
                        AxRole::Tab,
                        "Changes",
                        AxRect::new(tab_x, frame.y, tab_width, strip_height),
                    )
                    .selected(self.inspector_tab == InspectorTab::Changes)
                    .focused(
                        self.open_menu.is_none() && self.inspector_tab_focus[0].is_focused(window),
                    )
                    .action(AxAction::Press),
                )
                .child(
                    AxNode::new(
                        "inspector-tab-terminal",
                        AxRole::Tab,
                        "Terminal",
                        AxRect::new(tab_x + tab_width, frame.y, tab_width, strip_height),
                    )
                    .selected(self.inspector_tab == InspectorTab::Terminal)
                    .focused(
                        self.open_menu.is_none() && self.inspector_tab_focus[1].is_focused(window),
                    )
                    .action(AxAction::Press),
                )
                .child(
                    AxNode::new(
                        "inspector-tab-resources",
                        AxRole::Tab,
                        "Resources",
                        AxRect::new(tab_x + tab_width * 2.0, frame.y, tab_width, strip_height),
                    )
                    .selected(self.inspector_tab == InspectorTab::Resources)
                    .focused(
                        self.open_menu.is_none() && self.inspector_tab_focus[2].is_focused(window),
                    )
                    .action(AxAction::Press),
                ),
            )
            .child(
                AxNode::new(
                    "inspector-collapse",
                    AxRole::Button,
                    "Hide inspector",
                    AxRect::new(
                        frame.x + frame.width - 40.0,
                        collapse_y,
                        32.0,
                        CONTROL_HEIGHT,
                    ),
                )
                .focused(
                    self.open_menu.is_none() && self.inspector_collapse_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );
        let body = AxRect::new(
            frame.x,
            frame.y + strip_height,
            frame.width,
            frame.height - strip_height,
        );
        inspector = match self.inspector_tab {
            InspectorTab::Terminal => inspector.child(self.terminal_ax(window, cx, body)),
            InspectorTab::Changes => inspector.child(self.changes_ax(window, body)),
            InspectorTab::Resources => inspector.child(self.resources_ax(window, body)),
        };
        inspector
    }

    fn terminal_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let input_height = 40.0;
        let input_y = frame.y + frame.height - input_height - PAD;
        let button_width = 72.0;
        let focus = self.terminal_input.read(cx).focus_handle(cx);
        let output = if self.projection.terminal.output.is_empty() {
            // 与可见 Terminal 页占位同源（TERMINAL_EMPTY_OUTPUT）。
            TERMINAL_EMPTY_OUTPUT.to_string()
        } else {
            tail_chars(
                &plain_terminal_output(&self.projection.terminal.output),
                8_192,
            )
        };
        let owner = self
            .projection
            .terminal
            .workspace_id
            .as_deref()
            .unwrap_or("unassigned");
        let (columns, rows) =
            terminal_size_for_display(&self.projection.terminal, self.terminal_size_draft);
        let mut terminal_description = format!(
            "workspace {owner} · {} · {}×{} · {}",
            self.projection.terminal.cwd,
            columns,
            rows,
            self.projection.terminal.availability_label()
        );
        if self.projection.terminal.dropped_events > 0 {
            terminal_description.push_str(&format!(
                " · {} output events dropped",
                self.projection.terminal.dropped_events
            ));
        }
        if let Some(resize_status) =
            terminal_resize_status_label(&self.projection.terminal, self.terminal_size_draft)
        {
            terminal_description.push_str(&format!(" · {resize_status}"));
        }
        let terminal_operable =
            terminal_can_operate(&self.projection.connection, &self.projection.terminal);
        let terminal_start_enabled = terminal_start_enabled(
            &self.projection.connection,
            &self.projection.terminal,
            self.terminal_pending_create_workspace.as_ref(),
            self.terminal_pending_resize.is_some(),
        );
        let terminal_resize_enabled = terminal_operable && self.terminal_pending_resize.is_none();
        let mut terminal = AxNode::new("terminal", AxRole::Group, "Terminal", frame)
            // G1：头部尺寸组 = 列 stepper 对 + apply + 行 stepper 对，与可见
            // 控件同 gate / 同 id；apply 仍是唯一下发入口。
            .child(
                AxNode::new(
                    "terminal-cols-dec",
                    AxRole::Button,
                    "Fewer terminal columns",
                    AxRect::new(
                        frame.x + frame.width - PAD - 192.0,
                        frame.y,
                        28.0,
                        CONTROL_HEIGHT,
                    ),
                )
                .focused(
                    self.open_menu.is_none() && self.terminal_cols_dec_focus.is_focused(window),
                )
                .enabled(terminal_operable)
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "terminal-cols-inc",
                    AxRole::Button,
                    "More terminal columns",
                    AxRect::new(
                        frame.x + frame.width - PAD - 162.0,
                        frame.y,
                        28.0,
                        CONTROL_HEIGHT,
                    ),
                )
                .focused(
                    self.open_menu.is_none() && self.terminal_cols_inc_focus.is_focused(window),
                )
                .enabled(terminal_operable)
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "terminal-resize",
                    AxRole::Button,
                    "Apply terminal size",
                    AxRect::new(
                        frame.x + frame.width - PAD - 132.0,
                        frame.y,
                        72.0,
                        CONTROL_HEIGHT,
                    ),
                )
                .focused(self.open_menu.is_none() && self.terminal_resize_focus.is_focused(window))
                .enabled(terminal_resize_enabled)
                .value(format!("{columns}×{rows}"))
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "terminal-rows-dec",
                    AxRole::Button,
                    "Fewer terminal rows",
                    AxRect::new(
                        frame.x + frame.width - PAD - 58.0,
                        frame.y,
                        28.0,
                        CONTROL_HEIGHT,
                    ),
                )
                .focused(
                    self.open_menu.is_none() && self.terminal_rows_dec_focus.is_focused(window),
                )
                .enabled(terminal_operable)
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "terminal-rows-inc",
                    AxRole::Button,
                    "More terminal rows",
                    AxRect::new(
                        frame.x + frame.width - PAD - 28.0,
                        frame.y,
                        28.0,
                        CONTROL_HEIGHT,
                    ),
                )
                .focused(
                    self.open_menu.is_none() && self.terminal_rows_inc_focus.is_focused(window),
                )
                .enabled(terminal_operable)
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "terminal-output",
                    AxRole::StaticText,
                    "Terminal output",
                    AxRect::new(
                        frame.x + PAD,
                        frame.y + CONTROL_HEIGHT,
                        frame.width - PAD * 2.0,
                        frame.height - input_height - CONTROL_HEIGHT - PAD * 2.0,
                    ),
                )
                .value(output)
                .description(terminal_description),
            )
            .child(
                AxNode::new(
                    "terminal-input",
                    AxRole::TextArea,
                    "Terminal input",
                    AxRect::new(
                        frame.x + PAD,
                        input_y,
                        frame.width - PAD * 3.0 - button_width,
                        input_height,
                    ),
                )
                .value(self.terminal_input.read(cx).text())
                .focused(self.open_menu.is_none() && focus.is_focused(window))
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "terminal-start",
                    AxRole::Button,
                    if self.projection.terminal.session_id.is_some() {
                        if terminal_known_exited(&self.projection.terminal) {
                            "Start new terminal"
                        } else {
                            "Apply terminal size"
                        }
                    } else {
                        "Start terminal"
                    },
                    AxRect::new(
                        frame.x + frame.width - PAD - button_width,
                        input_y,
                        button_width,
                        input_height,
                    ),
                )
                .focused(self.open_menu.is_none() && self.terminal_start_focus.is_focused(window))
                .enabled(terminal_start_enabled)
                .action(AxAction::Press),
            );
        // 与可见回到底部按钮（inspector.rs）一致：仅在滚动脱钩时发布。
        if !self.terminal_scroll.is_following() {
            terminal = terminal.child(
                AxNode::new(
                    "terminal-back-to-bottom",
                    AxRole::Button,
                    "Back to bottom",
                    AxRect::new(
                        frame.x + frame.width - 140.0,
                        (input_y - 40.0).max(frame.y + CONTROL_HEIGHT),
                        132.0,
                        32.0,
                    ),
                )
                .focused(
                    self.open_menu.is_none()
                        && self.terminal_back_to_bottom_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );
        }
        terminal
    }

    fn changes_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let strip_height = metrics::CHANGES_TAB_HEIGHT;
        let tab_width = metrics::CHANGES_TAB_WIDTH;
        let tab_x = frame.x + 12.0;
        let refresh_y = frame.y + ((strip_height - CONTROL_HEIGHT) / 2.0).max(0.0);
        let body_top = frame.y + strip_height;
        let fetch_state = match &self.changes.fetch {
            ChangesFetch::Idle => "idle".to_string(),
            ChangesFetch::Fetching => "loading".to_string(),
            ChangesFetch::Ready => format!("ready · {} files", self.changes.files.len()),
            ChangesFetch::Failed(reason) => format!("failed · {reason}"),
        };
        let description = match &self.changes.stale_reason {
            Some(reason) => format!(
                "Host latest-session diff; workspace context is not a filter · {fetch_state} · stale · {reason}"
            ),
            None => format!(
                "Host latest-session diff; workspace context is not a filter · {fetch_state}"
            ),
        };
        let mut changes = AxNode::new("changes", AxRole::Group, "Changes", frame)
            .description(description)
            .child(
                AxNode::new(
                    "changes-tabs",
                    AxRole::TabGroup,
                    "Changes tabs",
                    AxRect::new(tab_x, frame.y, tab_width * 2.0, strip_height),
                )
                .child(
                    AxNode::new(
                        "changes-tab-files",
                        AxRole::Tab,
                        "Files",
                        AxRect::new(tab_x, frame.y, tab_width, strip_height),
                    )
                    .selected(self.changes.tab == ChangesTab::Files)
                    .focused(
                        self.open_menu.is_none() && self.changes_tab_focus[0].is_focused(window),
                    )
                    .action(AxAction::Press),
                )
                .child(
                    AxNode::new(
                        "changes-tab-summary",
                        AxRole::Tab,
                        "Summary",
                        AxRect::new(tab_x + tab_width, frame.y, tab_width, strip_height),
                    )
                    .selected(self.changes.tab == ChangesTab::Summary)
                    .focused(
                        self.open_menu.is_none() && self.changes_tab_focus[1].is_focused(window),
                    )
                    .action(AxAction::Press),
                ),
            )
            .child(
                AxNode::new(
                    "changes-refresh",
                    AxRole::Button,
                    "Refresh changes",
                    AxRect::new(
                        frame.x + frame.width - 40.0,
                        refresh_y,
                        32.0,
                        CONTROL_HEIGHT,
                    ),
                )
                .focused(self.open_menu.is_none() && self.changes_refresh_focus.is_focused(window))
                .action(AxAction::Press),
            );
        if self.changes.tab == ChangesTab::Files {
            let mut files = AxNode::new(
                "changes-file-list",
                AxRole::List,
                "Changed files",
                AxRect::new(
                    frame.x + PAD,
                    body_top,
                    frame.width - PAD * 2.0,
                    metrics::CHANGES_FILE_LIST_MAX_HEIGHT,
                ),
            );
            for (ix, file) in self.changes.files.iter().enumerate() {
                files = files.child(
                    AxNode::new(
                        diff_file_identifier(&file.path),
                        AxRole::ListItem,
                        file.path.clone(),
                        AxRect::new(
                            frame.x + PAD,
                            body_top + ix as f32 * ROW_HEIGHT,
                            frame.width - PAD * 2.0,
                            ROW_HEIGHT,
                        ),
                    )
                    .value(format!(
                        "{} · +{} / −{}",
                        file.status, file.additions, file.deletions
                    ))
                    .selected(self.changes.selected.as_deref() == Some(file.path.as_str()))
                    .focused(
                        self.open_menu.is_none()
                            && self
                                .changes_file_focus
                                .get(&file.path)
                                .is_some_and(|focus| focus.is_focused(window)),
                    )
                    .action(AxAction::Press),
                );
            }
            changes = changes.child(files);
            let diff_top = body_top + metrics::CHANGES_FILE_LIST_MAX_HEIGHT;
            let horizontal_offset = f32::from(self.changes.diff_scroll.offset().x);
            let horizontal_max = f32::from(self.changes.diff_scroll.max_offset().width);
            changes = changes.child(
                AxNode::new(
                    "changes-diff-view",
                    AxRole::Group,
                    "Diff view",
                    AxRect::new(
                        frame.x + PAD,
                        diff_top,
                        frame.width - PAD * 2.0,
                        (frame.height - (diff_top - frame.y)).max(ROW_HEIGHT),
                    ),
                )
                .description(format!(
                    "horizontal offset {horizontal_offset:.1} of {horizontal_max:.1}"
                )),
            );
        }
        changes
    }

    fn resources_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let fetch_state = match &self.resources.fetch {
            ResourcesFetch::Idle => "idle".to_string(),
            ResourcesFetch::Fetching => "loading".to_string(),
            ResourcesFetch::Ready => format!("ready · {} servers", self.resources.servers.len()),
            ResourcesFetch::Failed(reason) => format!("failed · {reason}"),
        };
        let description = match &self.resources.stale_reason {
            Some(reason) => format!("Host MCP servers · {fetch_state} · stale · {reason}"),
            None => format!("Host MCP servers · {fetch_state}"),
        };
        let resources = AxNode::new("resources", AxRole::Group, "Resources", frame)
            .description(description)
            .child(
                AxNode::new(
                    "resources-refresh",
                    AxRole::Button,
                    "Refresh resources",
                    AxRect::new(frame.x + frame.width - 40.0, frame.y + PAD, 32.0, 28.0),
                )
                .focused(
                    self.open_menu.is_none() && self.resources_refresh_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );
        let mut list = AxNode::new(
            "mcp-server-list",
            AxRole::List,
            "MCP servers",
            AxRect::new(
                frame.x + PAD,
                frame.y + 44.0,
                frame.width - PAD * 2.0,
                frame.height - 52.0,
            ),
        );
        for (ix, server) in self.resources.servers.iter().enumerate() {
            list = list.child(
                AxNode::new(
                    dynamic_identifier("mcp-server", &server.name),
                    AxRole::ListItem,
                    server.name.clone(),
                    AxRect::new(
                        frame.x + PAD,
                        frame.y + 44.0 + ix as f32 * (ROW_HEIGHT + 12.0),
                        frame.width - PAD * 2.0,
                        ROW_HEIGHT + 12.0,
                    ),
                )
                .value(format!(
                    "{} · {} · {} tools",
                    server.state, server.transport, server.tool_count
                ))
                .description(server.last_error.clone().unwrap_or_default()),
            );
        }
        resources.child(list)
    }

    /// R6 Wave A：Activity 触发器迁至 Workspace Header（header_ax），
    /// StatusBar 只保留居中的 run-status 信息串。
    fn status_ax(&self, frame: AxRect) -> AxNode {
        let now = super::super::now_unix_ms();
        // F-13：run-status 信息串在状态行内居中（与 render 同源）；宽度按
        // 定稿文案留 320px，行宽不足时收缩到整行。
        let run_status_width = 320.0_f32.min(frame.width);
        let run_status_x = frame.x + ((frame.width - run_status_width) / 2.0).max(0.0);
        let status = AxNode::new("status-bar", AxRole::Group, "Status", frame).child(
            AxNode::new(
                "run-status",
                AxRole::StaticText,
                "Run status",
                AxRect::new(run_status_x, frame.y, run_status_width, frame.height),
            )
            .value(self.projection.run_status_label(now)),
        );
        status
    }
}

fn timeline_accessible_text(entry: &TimelineEntry) -> (String, String) {
    match &entry.kind {
        TimelineEntryKind::UserMessage { text } => ("You".into(), text.clone()),
        TimelineEntryKind::AssistantMessage { text } => ("Pawork".into(), text.clone()),
        TimelineEntryKind::ToolCall {
            name,
            status,
            detail,
        } => (
            format!("Tool · {name}"),
            detail
                .as_ref()
                .filter(|detail| !detail.is_empty())
                .map(|detail| format!("{status} · {detail}"))
                .unwrap_or_else(|| status.clone()),
        ),
        TimelineEntryKind::RunState(state) => ("Run".into(), state.clone()),
        TimelineEntryKind::Error(message) => ("Error".into(), message.clone()),
    }
}

/// 会话行 AX description：可见状态点的状态词 + unread 语义同源映射
/// （ADR-042——新增可见状态须同批补 AX）；无 live 状态不声明语义（不伪造
/// 终态），unread 与标题 semibold 视觉同源。
fn session_status_description(status: Option<SessionLiveStatus>, unread: bool) -> String {
    let word = match status {
        Some(SessionLiveStatus::NeedsInput) => SessionLiveStatus::NeedsInput.label(),
        Some(SessionLiveStatus::Running) => SessionLiveStatus::Running.label(),
        Some(SessionLiveStatus::Blocked) => SessionLiveStatus::Blocked.label(),
        None => "Session",
    };
    if unread {
        format!("{word} · Unread")
    } else {
        word.to_string()
    }
}

fn project_key(workspace_id: Option<&str>) -> String {
    workspace_id.unwrap_or(UNASSIGNED_PROJECT).to_string()
}

fn dynamic_identifier(prefix: &str, raw: &str) -> String {
    let mut identifier = String::with_capacity(prefix.len() + raw.len() + 1);
    identifier.push_str(prefix);
    identifier.push('-');
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.') {
            identifier.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(identifier, "_{byte:02x}");
        }
    }
    identifier
}

fn session_identifier(session_id: &str) -> String {
    dynamic_identifier("session", session_id)
}

/// 项目头 identifier。Timeline 模式下同一项目可出现于多个日期组，以日期桶
/// 限定避免重复 id（AxTree::validate 拒绝重复）；Projects 模式保持 project-{key}。
fn rail_project_identifier(bucket: Option<DateBucket>, key: &str) -> String {
    match bucket {
        Some(bucket) => dynamic_identifier("project", &format!("{}:{}", bucket.label(), key)),
        None => dynamic_identifier("project", key),
    }
}

fn rail_project_add_identifier(bucket: Option<DateBucket>, key: &str) -> String {
    match bucket {
        Some(bucket) => dynamic_identifier("project-add", &format!("{}:{}", bucket.label(), key)),
        None => dynamic_identifier("project-add", key),
    }
}

fn scope_identifier(workspace_id: Option<&str>) -> String {
    dynamic_identifier("scope", workspace_id.unwrap_or("all"))
}

fn workspace_confirm_identifier(workspace_id: &str) -> String {
    dynamic_identifier("workspace-confirm", workspace_id)
}

fn model_identifier(model: &ModelEntry) -> String {
    dynamic_identifier("model", &format!("{}:{}", model.provider_id, model.id))
}

fn entry_menu_identifier(event_id: &str) -> String {
    dynamic_identifier("entry-menu", event_id)
}

fn fork_identifier(event_id: &str) -> String {
    dynamic_identifier("fork", event_id)
}

fn run_review_identifier(event_id: &str) -> String {
    dynamic_identifier("run-review-changes", event_id)
}

fn diff_file_identifier(path: &str) -> String {
    dynamic_identifier("changes-file", path)
}

fn tail_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_string();
    }
    format!("…{}", value.chars().skip(count - limit).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_identifier_is_stable_and_collision_resistant_for_escape_marker() {
        assert_eq!(dynamic_identifier("session", "abc-1"), "session-abc-1");
        assert_eq!(dynamic_identifier("session", "a b"), "session-a_20b");
        assert_eq!(dynamic_identifier("session", "a_20b"), "session-a_5f20b");
        assert_ne!(
            dynamic_identifier("session", "a b"),
            dynamic_identifier("session", "a_20b")
        );
    }

    #[test]
    fn tail_chars_preserves_utf8_boundaries() {
        assert_eq!(tail_chars("abc", 8), "abc");
        assert_eq!(tail_chars("一二三四", 2), "…三四");
    }

    #[test]
    fn rail_project_identifiers_are_bucket_scoped_and_projects_mode_stable() {
        let today = rail_project_identifier(Some(DateBucket::Today), "ws");
        let earlier = rail_project_identifier(Some(DateBucket::Earlier), "ws");
        let projects_mode = rail_project_identifier(None, "ws");
        assert_ne!(today, earlier);
        assert_ne!(today, projects_mode);
        assert_ne!(earlier, projects_mode);
        // Projects 模式 identifier 与既有 U2 定位口径保持 project-{key}。
        assert_eq!(projects_mode, "project-ws");
        assert_eq!(rail_project_add_identifier(None, "ws"), "project-add-ws");
    }

    /// R3 Wave A：会话行 AX description 携带可见状态点的状态词；无 live
    /// 状态保持中性「Session」（不伪造终态）；R3 Wave B 增 Blocked 与
    /// unread 语义词。
    #[test]
    fn session_ax_description_carries_live_status_word() {
        assert_eq!(
            session_status_description(Some(SessionLiveStatus::NeedsInput), false),
            "Needs input"
        );
        assert_eq!(
            session_status_description(Some(SessionLiveStatus::Running), false),
            "Running"
        );
        assert_eq!(
            session_status_description(Some(SessionLiveStatus::Blocked), false),
            "Blocked"
        );
        assert_eq!(session_status_description(None, false), "Session");
        assert_eq!(session_status_description(None, true), "Session · Unread");
        assert_eq!(
            session_status_description(Some(SessionLiveStatus::NeedsInput), true),
            "Needs input · Unread"
        );
    }

    #[test]
    fn composer_ax_panel_formula_drops_plus_68_drift() {
        let input = crate::ui::theme::metrics::COMPOSER_INPUT_MIN_HEIGHT;
        let height = crate::ui::AppView::composer_panel_height(input);
        assert!(height <= 94.0);
        assert_ne!(height, input + 68.0);
        assert_eq!(crate::ui::theme::metrics::COMPOSER_SEND_SIZE, 32.0);
    }

    /// R6 Wave A：折叠态 Header Activity 的 AX 触发器与 Popover 锚点公式
    /// 必须钉住生产 render 所用的 40×37 槽、右侧 25px inset、4px gap 与
    /// 320×320 外框；Connected 真窗口证据受环境阻塞时仍能防止静默漂移。
    #[test]
    fn activity_header_ax_geometry_matches_render_anchor_contract() {
        let header = AxRect::new(240.0, 0.0, 840.0, metrics::HEADER_HEIGHT);
        let trigger = header_action_ax_rect(header);
        assert_eq!(trigger, AxRect::new(1015.0, 51.5, 40.0, 37.0));

        let popover = activity_popover_ax_geometry(header, trigger);
        assert_eq!(popover.frame, AxRect::new(735.0, 92.5, 320.0, 320.0));
        assert_eq!(popover.heading, AxRect::new(763.0, 150.5, 264.0, 20.0));
        assert_eq!(
            popover.open_changes,
            AxRect::new(763.0, 174.5, 264.0, ROW_HEIGHT)
        );
    }
}
