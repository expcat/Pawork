//! AppView → AxTree 投影与 AX action 白名单。

use gpui::{App, Context, Focusable, Window};

use crate::projection::{
    ConnectionState, DateBucket, ModelEntry, TaskRailGrouping, TaskRailProjectGroup, TimelineEntry,
    TimelineEntryKind, UNASSIGNED_PROJECT,
};

use super::{AxAction, AxBridge, AxNode, AxRect, AxRequest, AxRole, AxTree};
use crate::ui::changes::ChangesTab;
use crate::ui::inspector::InspectorTab;
use crate::ui::inspector::TERMINAL_EMPTY_OUTPUT;
use crate::ui::shell_layout;
use crate::ui::theme::metrics;
use crate::ui::{AppView, MenuKind, WORKSPACE_EMPTY_HINT};

const PAD: f32 = 8.0;
const CONTROL_HEIGHT: f32 = 28.0;
const ROW_HEIGHT: f32 = 32.0;
const TIMELINE_ROW_HEIGHT: f32 = 52.0;
const INSPECTOR_HEADER_HEIGHT: f32 = 36.0;

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
            "add-task" => self.on_new_session(window, cx),
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
            "approve-once" => self.on_approve("approve_once", cx),
            "approve-for-run" => self.on_approve("approve_for_run", cx),
            "approve-deny" => self.on_approve("deny", cx),
            "timeline-back-to-bottom" => self.timeline_jump_to_bottom(),
            // Inspector 折叠态触发器的可见语义是弹出 ActivityPopover（mod.rs
            // status bar），摘要行才展开 Inspector；展开态由 inspector-collapse 收起。
            "inspector-toggle" => self.toggle_menu(MenuKind::Activity, None, cx),
            "inspector-collapse" => self.on_toggle_inspector(window, cx),
            "inspector-tab-changes" => self.select_inspector_tab(InspectorTab::Changes, cx),
            "inspector-tab-terminal" => self.select_inspector_tab(InspectorTab::Terminal, cx),
            "inspector-tab-resources" => self.select_inspector_tab(InspectorTab::Resources, cx),
            "changes-tab-files" => self.on_select_changes_tab(ChangesTab::Files, cx),
            "changes-tab-summary" => self.on_select_changes_tab(ChangesTab::Summary, cx),
            "changes-refresh" => self.refresh_changes(cx),
            "resources-refresh" => self.refresh_resources(cx),
            "terminal-start" => {
                if self.projection.terminal.session_id.is_some() {
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
                    self.on_fork(&event_id, cx);
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
        // 窄窗 rail=240 且 Inspector 强制折叠），AX bounds 不得偏出实际布局。
        let shell = shell_layout::resolve(viewport.width, self.inspector_open);
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
        tree.child(self.status_ax(
            AxRect::new(
                sidebar_width,
                content_height,
                (width - sidebar_width).max(0.0),
                metrics::STATUS_BAR_HEIGHT,
            ),
            shell.inspector_open,
        ))
    }

    fn sidebar_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let can_create = self.can_create_task();
        // 与可见 TaskRail 对齐：Panel p_2 上边距 8px + 36px traffic-light
        // 安全区后再进入 grouping 行（F-01）。AX 不得把首控件投影到按钮带。
        let mut y = PAD + shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT;
        let grouping = AxNode::new(
            "task-rail-grouping",
            AxRole::Button,
            self.grouping.accessible_name(),
            AxRect::new(
                (frame.width - PAD - metrics::ICON_LARGE).max(PAD),
                y,
                metrics::ICON_LARGE,
                metrics::ICON_MEDIUM,
            ),
        )
        .value(match self.grouping {
            TaskRailGrouping::Timeline => "Timeline",
            TaskRailGrouping::Projects => "Projects",
        })
        .action(AxAction::Press);
        y += metrics::ICON_MEDIUM + PAD;
        let scope_label = match &self.scope_workspace_id {
            None => "All projects".into(),
            Some(id) => self.projection.workspace_name(Some(id)),
        };
        let scope = AxNode::new(
            "project-scope",
            AxRole::Button,
            "Project scope",
            AxRect::new(PAD, y, (frame.width - PAD * 2.0).max(0.0), CONTROL_HEIGHT),
        )
        .value(scope_label)
        .action(AxAction::Press);
        y += CONTROL_HEIGHT + PAD;
        let connection = AxNode::new(
            "connection-status",
            AxRole::StaticText,
            "Connection",
            AxRect::new(PAD, y, (frame.width - 52.0).max(0.0), CONTROL_HEIGHT),
        )
        .value(self.projection.connection.label());
        let add_task = AxNode::new(
            "add-task",
            AxRole::Button,
            "New task",
            AxRect::new(
                (frame.width - PAD - metrics::ICON_MEDIUM).max(PAD),
                y,
                metrics::ICON_MEDIUM,
                metrics::ICON_MEDIUM,
            ),
        )
        .description(self.add_task_disabled_reason())
        .enabled(can_create)
        .focused(self.add_task_focus.is_focused(window))
        .action(AxAction::Press);
        y += CONTROL_HEIGHT + PAD;

        let mut sidebar = AxNode::new("task-rail", AxRole::Group, "Tasks", frame)
            .child(grouping)
            .child(scope)
            .child(connection)
            .child(add_task);
        // 与可见路径同源：Reconnect 仅 Disconnected / ConnectFailed 发布
        // （projection.show_reconnect()，同 task_rail.rs 视觉谓词）。
        if self.projection.show_reconnect() {
            sidebar = sidebar.child(
                AxNode::new(
                    "reconnect",
                    AxRole::Button,
                    "Reconnect",
                    AxRect::new(PAD, y, (frame.width - PAD * 2.0).max(0.0), CONTROL_HEIGHT),
                )
                .action(AxAction::Press),
            );
            y += CONTROL_HEIGHT + PAD;
        }

        let list_top = y;
        let list_height = (frame.height - list_top - CONTROL_HEIGHT).max(0.0);
        let list_width = (frame.width - PAD * 2.0).max(0.0);
        let mut list = AxNode::new(
            "session-list",
            AxRole::List,
            "Sessions",
            AxRect::new(PAD, list_top, list_width, list_height),
        );
        // 与可见 TaskRail（task_rail.rs）同一结构：Timeline = 日期组 → 项目块，
        // Projects = 项目块；折叠的项目只投影头部，不投影其子会话。
        let mut row_y = list_top;
        match self.grouping {
            TaskRailGrouping::Timeline => {
                for group in self
                    .projection
                    .timeline_groups(self.scope_workspace_id.as_deref(), crate::ui::now_unix_ms())
                {
                    list = list.child(AxNode::new(
                        dynamic_identifier("date-group", group.bucket.label()),
                        AxRole::StaticText,
                        group.bucket.label(),
                        AxRect::new(PAD, row_y, list_width, CONTROL_HEIGHT),
                    ));
                    row_y += CONTROL_HEIGHT;
                    for project in &group.projects {
                        let (nodes, consumed) = self.project_ax_nodes(
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
                for project in &self
                    .projection
                    .project_groups(self.scope_workspace_id.as_deref())
                {
                    let (nodes, consumed) =
                        self.project_ax_nodes(project, None, row_y, list_width, can_create);
                    row_y += consumed;
                    for node in nodes {
                        list = list.child(node);
                    }
                }
            }
        }
        sidebar = sidebar.child(list);

        if matches!(self.open_menu, Some(MenuKind::Grouping)) {
            sidebar = sidebar.child(
                AxNode::new(
                    "grouping-menu",
                    AxRole::Group,
                    "Task grouping",
                    AxRect::new(frame.width - 156.0, PAD + CONTROL_HEIGHT, 148.0, 64.0),
                )
                .child(
                    AxNode::new(
                        "group-timeline",
                        AxRole::Button,
                        "Timeline",
                        AxRect::new(frame.width - 156.0, PAD + CONTROL_HEIGHT, 148.0, 32.0),
                    )
                    .selected(self.grouping == TaskRailGrouping::Timeline)
                    .action(AxAction::Press),
                )
                .child(
                    AxNode::new(
                        "group-projects",
                        AxRole::Button,
                        "Projects",
                        AxRect::new(
                            frame.width - 156.0,
                            PAD + CONTROL_HEIGHT + 32.0,
                            148.0,
                            32.0,
                        ),
                    )
                    .selected(self.grouping == TaskRailGrouping::Projects)
                    .action(AxAction::Press),
                ),
            );
        }
        if matches!(self.open_menu, Some(MenuKind::Scope)) {
            let mut menu = AxNode::new(
                "scope-menu",
                AxRole::Group,
                "Project scope options",
                AxRect::new(PAD, 68.0, frame.width - PAD * 2.0, 200.0),
            );
            for (ix, (workspace_id, label)) in self
                .projection
                .project_scope_options()
                .into_iter()
                .enumerate()
            {
                menu = menu.child(
                    AxNode::new(
                        scope_identifier(workspace_id.as_deref()),
                        AxRole::Button,
                        label,
                        AxRect::new(
                            PAD,
                            68.0 + ix as f32 * ROW_HEIGHT,
                            frame.width - PAD * 2.0,
                            ROW_HEIGHT,
                        ),
                    )
                    .selected(self.scope_workspace_id == workspace_id)
                    .action(AxAction::Press),
                );
            }
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
        project: &TaskRailProjectGroup,
        bucket: Option<DateBucket>,
        top: f32,
        width: f32,
        can_create: bool,
    ) -> (Vec<AxNode>, f32) {
        let key = project_key(project.workspace_id.as_deref());
        let expanded = !self.collapsed_projects.contains(&key);
        let mut nodes = vec![
            AxNode::new(
                rail_project_identifier(bucket, &key),
                AxRole::Button,
                project.name.clone(),
                AxRect::new(PAD, top, width, ROW_HEIGHT),
            )
            .value(format!("{} tasks", project.task_count()))
            .description(if expanded { "Expanded" } else { "Collapsed" })
            .action(AxAction::Press),
        ];
        if !project.is_unassigned() && project.workspace_id.is_some() {
            nodes.push(
                AxNode::new(
                    rail_project_add_identifier(bucket, &key),
                    AxRole::Button,
                    format!("New task in {}", project.name),
                    AxRect::new(
                        PAD + (width - metrics::ICON_SMALL).max(0.0),
                        top,
                        metrics::ICON_SMALL,
                        metrics::ICON_SMALL,
                    ),
                )
                .enabled(can_create)
                .action(AxAction::Press),
            );
        }
        let mut consumed = ROW_HEIGHT;
        if expanded {
            for session in &project.tasks {
                let running = self
                    .projection
                    .active_runs
                    .iter()
                    .any(|run| run.session_id == session.session_id);
                nodes.push(
                    AxNode::new(
                        session_identifier(&session.session_id),
                        AxRole::ListItem,
                        session.title.clone(),
                        AxRect::new(
                            PAD + 12.0,
                            top + consumed,
                            (width - 12.0).max(0.0),
                            ROW_HEIGHT,
                        ),
                    )
                    .description(if running { "Running" } else { "Session" })
                    .selected(
                        self.projection.active_session_id.as_deref()
                            == Some(session.session_id.as_str()),
                    )
                    .action(AxAction::Press),
                );
                consumed += ROW_HEIGHT;
            }
        }
        (nodes, consumed)
    }

    fn workspace_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let input_height = (f32::from(window.line_height())
            * self.text_input.read(cx).visual_line_count() as f32
            + metrics::COMPOSER_TEXT_INSET)
            .clamp(metrics::COMPOSER_MIN_HEIGHT, metrics::COMPOSER_MAX_HEIGHT);
        let composer_height = (input_height + 68.0).min(frame.height);
        let timeline_height = (frame.height - composer_height).max(0.0);
        AxNode::new("workspace", AxRole::Group, "Workspace", frame)
            .child(self.timeline_ax(AxRect::new(frame.x, frame.y, frame.width, timeline_height)))
            .child(self.composer_ax(
                window,
                cx,
                AxRect::new(
                    frame.x,
                    frame.y + timeline_height,
                    frame.width,
                    composer_height,
                ),
            ))
    }

    fn timeline_ax(&self, frame: AxRect) -> AxNode {
        let total = self.projection.timeline.len();
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
        for (visible_ix, entry) in self.projection.timeline[start..end].iter().enumerate() {
            let row = AxRect::new(
                frame.x + PAD,
                frame.y + PAD + visible_ix as f32 * TIMELINE_ROW_HEIGHT,
                (frame.width - PAD * 2.0).max(0.0),
                TIMELINE_ROW_HEIGHT,
            );
            let (label, value) = timeline_accessible_text(entry);
            let mut node = AxNode::new(
                dynamic_identifier("timeline-entry", &entry.event_id),
                AxRole::ListItem,
                label,
                row,
            )
            .value(value)
            .description(entry.timestamp.clone())
            .child(
                AxNode::new(
                    entry_menu_identifier(&entry.event_id),
                    AxRole::Button,
                    "Entry actions",
                    AxRect::new(row.x + row.width - 32.0, row.y, 32.0, 28.0),
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
                    .action(AxAction::Press),
                );
            }
            list = list.child(node);
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
                .action(AxAction::Press),
            );
        }
        list
    }

    fn composer_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let header_y = frame.y + PAD;
        let input_y = header_y + CONTROL_HEIGHT + PAD;
        let input_height = (frame.height - CONTROL_HEIGHT - PAD * 3.0).max(0.0);
        let button_width = 72.0;
        let send_x = frame.x + frame.width - PAD - button_width;
        let cancel_x = send_x - PAD - button_width;
        let input_width = (cancel_x - PAD - (frame.x + PAD)).max(0.0);
        let current_model = self
            .projection
            .effective_model()
            .map(|(provider, model)| format!("{provider} / {model}"))
            .unwrap_or_else(|| "No model".into());
        let input_focus = self.text_input.read(cx).focus_handle(cx);
        let mut composer = AxNode::new("composer", AxRole::Group, "Composer", frame)
            .child(
                AxNode::new(
                    "model-picker",
                    AxRole::Button,
                    "Model",
                    AxRect::new(
                        frame.x + PAD,
                        header_y,
                        220.0_f32.min(frame.width),
                        CONTROL_HEIGHT,
                    ),
                )
                .value(current_model)
                .enabled(self.can_switch_model())
                .focused(self.model_focus.is_focused(window))
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "composer-input",
                    AxRole::TextArea,
                    "Message",
                    AxRect::new(frame.x + PAD, input_y, input_width, input_height),
                )
                .value(self.text_input.read(cx).text())
                .focused(input_focus.is_focused(window))
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "cancel",
                    AxRole::Button,
                    "Cancel run",
                    AxRect::new(cancel_x, input_y, button_width, CONTROL_HEIGHT),
                )
                .enabled(self.can_cancel())
                .focused(self.cancel_focus.is_focused(window))
                .action(AxAction::Press),
            )
            .child(
                AxNode::new(
                    "send",
                    AxRole::Button,
                    "Send",
                    AxRect::new(send_x, input_y, button_width, CONTROL_HEIGHT),
                )
                .enabled(self.can_send())
                .focused(self.send_focus.is_focused(window))
                .action(AxAction::Press),
            );
        if matches!(self.open_menu, Some(MenuKind::Model)) {
            let mut menu = AxNode::new(
                "model-menu",
                AxRole::Group,
                "Models",
                AxRect::new(frame.x + PAD, header_y + CONTROL_HEIGHT, 260.0, 240.0),
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
                            frame.x + PAD,
                            header_y + CONTROL_HEIGHT + ix as f32 * ROW_HEIGHT,
                            260.0,
                            ROW_HEIGHT,
                        ),
                    )
                    .value(format!("{} / {}", model.provider_id, model.id))
                    .enabled(self.can_switch_model())
                    .selected(selected)
                    .action(AxAction::Press),
                );
            }
            composer = composer.child(menu);
        }
        if matches!(self.open_menu, Some(MenuKind::WorkspaceConfirm)) {
            let mut menu = AxNode::new(
                "workspace-confirm",
                AxRole::Group,
                "Choose workspace",
                AxRect::new(frame.x + PAD, header_y + CONTROL_HEIGHT, 280.0, 220.0),
            );
            for (ix, workspace) in self.projection.workspaces.iter().enumerate() {
                menu = menu.child(
                    AxNode::new(
                        workspace_confirm_identifier(&workspace.id),
                        AxRole::Button,
                        workspace.name.clone(),
                        AxRect::new(
                            frame.x + PAD,
                            header_y + CONTROL_HEIGHT + ix as f32 * ROW_HEIGHT,
                            280.0,
                            ROW_HEIGHT,
                        ),
                    )
                    .action(AxAction::Press),
                );
            }
            composer = composer.child(menu);
        }
        composer
    }

    fn inspector_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let tab_width = 92.0;
        let mut inspector = AxNode::new("inspector", AxRole::Group, "Inspector", frame)
            .child(
                AxNode::new(
                    "inspector-tabs",
                    AxRole::TabGroup,
                    "Inspector tabs",
                    AxRect::new(
                        frame.x + PAD,
                        frame.y,
                        frame.width - 48.0,
                        INSPECTOR_HEADER_HEIGHT,
                    ),
                )
                .child(
                    AxNode::new(
                        "inspector-tab-changes",
                        AxRole::Tab,
                        "Changes",
                        AxRect::new(frame.x + PAD, frame.y + 4.0, tab_width, CONTROL_HEIGHT),
                    )
                    .selected(self.inspector_tab == InspectorTab::Changes)
                    .action(AxAction::Press),
                )
                .child(
                    AxNode::new(
                        "inspector-tab-terminal",
                        AxRole::Tab,
                        "Terminal",
                        AxRect::new(
                            frame.x + PAD + tab_width,
                            frame.y + 4.0,
                            tab_width,
                            CONTROL_HEIGHT,
                        ),
                    )
                    .selected(self.inspector_tab == InspectorTab::Terminal)
                    .action(AxAction::Press),
                )
                .child(
                    AxNode::new(
                        "inspector-tab-resources",
                        AxRole::Tab,
                        "Resources",
                        AxRect::new(
                            frame.x + PAD + tab_width * 2.0,
                            frame.y + 4.0,
                            tab_width,
                            CONTROL_HEIGHT,
                        ),
                    )
                    .selected(self.inspector_tab == InspectorTab::Resources)
                    .action(AxAction::Press),
                ),
            )
            .child(
                AxNode::new(
                    "inspector-collapse",
                    AxRole::Button,
                    "Hide inspector",
                    AxRect::new(frame.x + frame.width - 40.0, frame.y + 4.0, 32.0, 28.0),
                )
                .action(AxAction::Press),
            );
        let body = AxRect::new(
            frame.x,
            frame.y + INSPECTOR_HEADER_HEIGHT,
            frame.width,
            frame.height - INSPECTOR_HEADER_HEIGHT,
        );
        inspector = match self.inspector_tab {
            InspectorTab::Terminal => inspector.child(self.terminal_ax(window, cx, body)),
            InspectorTab::Changes => inspector.child(self.changes_ax(body)),
            InspectorTab::Resources => inspector.child(self.resources_ax(body)),
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
            tail_chars(&self.projection.terminal.output, 8_192)
        };
        let mut terminal = AxNode::new("terminal", AxRole::Group, "Terminal", frame)
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
                .description(format!(
                    "{} · {}×{}",
                    self.projection.terminal.cwd,
                    self.projection.terminal.columns,
                    self.projection.terminal.rows
                )),
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
                .focused(focus.is_focused(window))
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            )
            .child(
                AxNode::new(
                    "terminal-start",
                    AxRole::Button,
                    if self.projection.terminal.session_id.is_some() {
                        "Apply terminal size"
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
                .enabled(matches!(
                    self.projection.connection,
                    ConnectionState::Connected { .. }
                ))
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
                .action(AxAction::Press),
            );
        }
        terminal
    }

    fn changes_ax(&self, frame: AxRect) -> AxNode {
        let mut changes = AxNode::new("changes", AxRole::Group, "Changes", frame)
            .child(
                AxNode::new(
                    "changes-tabs",
                    AxRole::TabGroup,
                    "Changes tabs",
                    AxRect::new(frame.x + PAD, frame.y + PAD, 170.0, CONTROL_HEIGHT),
                )
                .child(
                    AxNode::new(
                        "changes-tab-files",
                        AxRole::Tab,
                        "Files",
                        AxRect::new(frame.x + PAD, frame.y + PAD, 72.0, CONTROL_HEIGHT),
                    )
                    .selected(self.changes.tab == ChangesTab::Files)
                    .action(AxAction::Press),
                )
                .child(
                    AxNode::new(
                        "changes-tab-summary",
                        AxRole::Tab,
                        "Summary",
                        AxRect::new(frame.x + 88.0, frame.y + PAD, 82.0, CONTROL_HEIGHT),
                    )
                    .selected(self.changes.tab == ChangesTab::Summary)
                    .action(AxAction::Press),
                ),
            )
            .child(
                AxNode::new(
                    "changes-refresh",
                    AxRole::Button,
                    "Refresh changes",
                    AxRect::new(frame.x + frame.width - 40.0, frame.y + PAD, 32.0, 28.0),
                )
                .action(AxAction::Press),
            );
        if self.changes.tab == ChangesTab::Files {
            let mut files = AxNode::new(
                "changes-file-list",
                AxRole::List,
                "Changed files",
                AxRect::new(
                    frame.x + PAD,
                    frame.y + 44.0,
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
                            frame.y + 44.0 + ix as f32 * ROW_HEIGHT,
                            frame.width - PAD * 2.0,
                            ROW_HEIGHT,
                        ),
                    )
                    .value(format!(
                        "{} · +{} / −{}",
                        file.status, file.additions, file.deletions
                    ))
                    .selected(self.changes.selected.as_deref() == Some(file.path.as_str()))
                    .action(AxAction::Press),
                );
            }
            changes = changes.child(files);
        }
        changes
    }

    fn resources_ax(&self, frame: AxRect) -> AxNode {
        let resources = AxNode::new("resources", AxRole::Group, "Resources", frame).child(
            AxNode::new(
                "resources-refresh",
                AxRole::Button,
                "Refresh resources",
                AxRect::new(frame.x + frame.width - 40.0, frame.y + PAD, 32.0, 28.0),
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

    fn status_ax(&self, frame: AxRect, inspector_open: bool) -> AxNode {
        let now = super::super::now_unix_ms();
        // F-13：run-status 信息串在状态行内居中（与 render 同源）；宽度按
        // 定稿文案留 320px，行宽不足时收缩到整行。
        let run_status_width = 320.0_f32.min(frame.width);
        let run_status_x = frame.x + ((frame.width - run_status_width) / 2.0).max(0.0);
        let mut status = AxNode::new("status-bar", AxRole::Group, "Status", frame).child(
            AxNode::new(
                "run-status",
                AxRole::StaticText,
                "Run status",
                AxRect::new(run_status_x, frame.y, run_status_width, frame.height),
            )
            .value(self.projection.run_status_label(now)),
        );
        if !inspector_open {
            status = status.child(
                AxNode::new(
                    "inspector-toggle",
                    AxRole::Button,
                    "Inspector",
                    AxRect::new(frame.x + frame.width - 120.0, frame.y, 112.0, frame.height),
                )
                .action(AxAction::Press),
            );
            // 与可见 ActivityPopover（changes.rs）对应：摘要行展开 Inspector·Changes。
            if matches!(self.open_menu, Some(MenuKind::Activity)) {
                let popover_x = frame.x + (frame.width - 328.0).max(0.0);
                status = status.child(
                    AxNode::new(
                        "activity-popover",
                        AxRole::Group,
                        "Activity",
                        AxRect::new(popover_x, frame.y - 96.0, 320.0, 96.0),
                    )
                    .child(
                        AxNode::new(
                            "activity-open-changes",
                            AxRole::Button,
                            "Open changes",
                            AxRect::new(popover_x, frame.y - 96.0, 320.0, ROW_HEIGHT),
                        )
                        .value(self.changes.activity_summary())
                        .action(AxAction::Press),
                    ),
                );
            }
        }
        status
    }
}

fn timeline_accessible_text(entry: &TimelineEntry) -> (String, String) {
    match &entry.kind {
        TimelineEntryKind::UserMessage { text } => ("You".into(), text.clone()),
        TimelineEntryKind::AssistantMessage { text } => ("Assistant".into(), text.clone()),
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
}
