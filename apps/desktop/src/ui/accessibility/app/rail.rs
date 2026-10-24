//! Task rail、workspace 壳与 header 的 AX 树。

use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
struct ActivityPopoverAxGeometry {
    heading: AxRect,
    subagents: AxRect,
}

fn activity_popover_ax_geometry(
    header_frame: AxRect,
    trigger: AxRect,
    rem_px: f32,
) -> ActivityPopoverAxGeometry {
    let scale = rem_px / font::BASE_REM_PIXELS;
    let menu_inset = metrics::MENU_PADDING + 1.0;
    let width = metrics::ACTIVITY_POPOVER_WIDTH + 2.0 * menu_inset;
    let inset_x = menu_inset + 1.5 * rem_px + 1.0; // p_4 + section border + p_2
    let frame = AxRect::new(
        (trigger.x + trigger.width - width).max(header_frame.x),
        trigger.y + trigger.height + ANCHOR_GAP_Y,
        width,
        metrics::ACTIVITY_POPOVER_HEIGHT * scale + 96.0 * scale + 2.0 * menu_inset,
    );
    let heading = AxRect::new(
        frame.x + inset_x,
        // p_4 + 标题行 + divider + gap_2 + section border + p_2
        frame.y + menu_inset + 2.0 * rem_px + 1.5 * font::BODY.0 * rem_px + 2.0,
        (frame.width - 2.0 * inset_x).max(0.0),
        1.5 * font::BODY_SM.0 * rem_px,
    );
    let subagents = AxRect::new(
        heading.x,
        heading.y + heading.height + 0.25 * rem_px,
        heading.width,
        3.0 * font::XS.0 * rem_px,
    );
    ActivityPopoverAxGeometry { heading, subagents }
}

impl AppView {
    pub(super) fn sidebar_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let can_create = self.can_create_task();
        // 控件与列表矩形读取实际布局，页脚入口不参与会话行坐标推算。
        let grouping = AxNode::new(
            "task-rail-grouping",
            AxRole::Button,
            self.grouping.toggle_action_label(),
            self.shell_ax_rect("rail-grouping-layout"),
        )
        .value(self.grouping.view_label())
        .focused(self.open_menu.is_none() && self.grouping_focus.is_focused(window))
        .action(AxAction::Press);
        let scope_label = self.scope_label();
        let scope = AxNode::new(
            "project-scope",
            AxRole::Button,
            t("rail.filter_label").replace("{}", &scope_label),
            self.shell_ax_rect("rail-scope-layout"),
        )
        .value(scope_label)
        // R7 Wave A：scope 菜单打开时 AX 焦点移交高亮项（同 grouping）。
        .focused(self.open_menu.is_none() && self.scope_focus.is_focused(window))
        .action(AxAction::Press);
        let connection = AxNode::new(
            "connection-status",
            AxRole::StaticText,
            "Connection",
            self.shell_ax_rect("connection-status"),
        )
        .value(self.connection_status_label());
        let add_task = AxNode::new(
            "add-task",
            AxRole::Button,
            t("timeline.new_task"),
            self.shell_ax_rect("rail-add-layout"),
        )
        .description(self.add_task_disabled_reason())
        .enabled(can_create)
        .focused(self.open_menu.is_none() && self.add_task_focus.is_focused(window))
        .action(AxAction::Press);
        let mut sidebar = AxNode::new("task-rail", AxRole::Group, "Tasks", frame)
            .child(
                AxNode::new(
                    "quick-search",
                    AxRole::Button,
                    t("quick.title"),
                    self.shell_ax_rect("rail-search-layout"),
                )
                .focused(self.quick_search.trigger.is_focused(window))
                .action(AxAction::Press),
            )
            .child(add_task)
            .child(scope)
            .child(grouping)
            .child(
                AxNode::new(
                    "task-usage-open",
                    AxRole::Button,
                    t("usage.title"),
                    self.shell_ax_rect("rail-usage-layout"),
                )
                .enabled(true)
                .focused(self.task_usage_focus.is_focused(window))
                .action(AxAction::Press),
            )
            .child(connection);
        // 与可见路径同源：Reconnect 仅 Disconnected / ConnectFailed 发布
        // （projection.show_reconnect()）。GUI3-07：底部 Local 行 Ghost，
        // 几何读实际布局。
        if self.projection.show_reconnect() {
            sidebar = sidebar.child(
                AxNode::new(
                    "reconnect",
                    AxRole::Button,
                    t("rail.reconnect"),
                    self.shell_ax_rect("rail-reconnect-layout"),
                )
                .description("ghost")
                .focused(self.open_menu.is_none() && self.reconnect_focus.is_focused(window))
                .action(AxAction::Press),
            );
        }

        let bounds = self.rail_scroll.bounds();
        let inset = f32::from(bounds.origin.x);
        let list_top = f32::from(bounds.origin.y);
        let list_height = f32::from(bounds.size.height);
        let list_width = f32::from(bounds.size.width);
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
                        group.bucket.display_label(),
                        AxRect::new(inset, row_y, list_width, metrics::RAIL_BUCKET_HEADER_HEIGHT),
                    ));
                    row_y += metrics::RAIL_BUCKET_HEADER_HEIGHT;
                    let skip_header = group.skip_project_header(self.scope_workspace_id.as_deref());
                    for (project_index, project) in group.projects.iter().enumerate() {
                        // 与 render 同源：桶头→首项目 2，项目块间 8。
                        row_y += if project_index == 0 {
                            metrics::RAIL_BUCKET_TO_PROJECT_GAP
                        } else {
                            metrics::RAIL_PROJECT_BLOCK_GAP
                        };
                        let (nodes, consumed) = self.project_ax_nodes(
                            window,
                            cx,
                            project,
                            Some(group.bucket),
                            row_y,
                            list_width,
                            can_create,
                            skip_header,
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
                    let (nodes, consumed) = self.project_ax_nodes(
                        window, cx, project, None, row_y, list_width, can_create, false,
                    );
                    row_y += consumed;
                    for node in nodes {
                        list = list.child(node);
                    }
                }
            }
        }
        sidebar = sidebar.child(list);
        if let Some(notice) = self.archive_notice_ax(window, frame) {
            sidebar = sidebar.child(notice);
        }

        // TR-12 页脚（SET-3）：右下角 Settings gear，与可见按钮同 gate
        //（render：content px(RAIL_INNER_PAD) + Panel p_2 → 右缘 inset 20，
        // mt_auto 钉底）。
        sidebar = sidebar.child(
            AxNode::new(
                "open-settings",
                AxRole::Button,
                t("rail.tooltip_settings"),
                self.shell_ax_rect("rail-settings-layout"),
            )
            .focused(self.open_menu.is_none() && self.settings_focus.is_focused(window))
            .action(AxAction::Press),
        );

        if matches!(
            self.open_menu,
            Some(MenuKind::Scope | MenuKind::ProjectTask)
        ) {
            let bounds = self.scope_menu_scroll.bounds();
            let rect = |bounds: gpui::Bounds<gpui::Pixels>| {
                AxRect::new(
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    bounds.size.width.into(),
                    bounds.size.height.into(),
                )
            };
            let mut menu = AxNode::new(
                "scope-menu",
                AxRole::Group,
                if matches!(self.open_menu, Some(MenuKind::ProjectTask)) {
                    self.composer_project_task_label()
                } else {
                    "Project filter options"
                },
                rect(bounds),
            );
            let options = self.project_menu_options();
            let highlight = self.menu_highlight_effective(self.menu_selected_index());
            let rows = options
                .into_iter()
                .map(|(workspace_id, label)| {
                    (
                        scope_identifier(workspace_id.as_deref()),
                        label,
                        self.scope_workspace_id == workspace_id,
                    )
                })
                .chain(std::iter::once((
                    "scope-add-project".to_string(),
                    t("common.add_project").to_string(),
                    false,
                )));
            for (ix, (identifier, label, selected)) in rows.enumerate() {
                let Some(mut row) = self.scope_menu_scroll.bounds_for_item(ix) else {
                    continue;
                };
                row.origin += self.scope_menu_scroll.offset();
                let row = row.intersect(&bounds);
                if row.size.width <= gpui::px(0.0) || row.size.height <= gpui::px(0.0) {
                    continue;
                }
                menu = menu.child(
                    AxNode::new(identifier, AxRole::Button, label, rect(row))
                        .selected(selected)
                        .focused(ix == highlight)
                        .action(AxAction::Press),
                );
            }
            sidebar = sidebar.child(menu);
        }
        sidebar
    }

    /// 当前分组模式下的项目块序列（Timeline 带日期桶）。AX 树构建与 Press
    /// 白名单共用同一来源，保证 identifier 一致。
    pub(super) fn rail_project_entries(&self) -> Vec<(Option<DateBucket>, TaskRailProjectGroup)> {
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

    /// Settings 左栏（SET-3）：返回按钮 + 首页导航项。几何与

    /// 项目块 AX 投影（对齐 task_rail.rs project_block）：折叠头 + 项目内新建
    /// 按钮 + 展开时的会话行。skip_header 时不发布项目头。返回（节点序列, 占用高度）。
    fn project_ax_nodes(
        &self,
        window: &Window,
        cx: &App,
        project: &TaskRailProjectGroup,
        bucket: Option<DateBucket>,
        top: f32,
        width: f32,
        can_create: bool,
        skip_header: bool,
    ) -> (Vec<AxNode>, f32) {
        let inset = metrics::RAIL_CONTENT_INSET;
        let key = project_key(project.workspace_id.as_deref());
        let expanded = skip_header || !self.collapsed_projects.contains(&key);
        let row_height = metrics::rail_task_row_height(self.text_scale);
        let mut nodes = Vec::new();
        let mut consumed = 0.0;
        if !skip_header {
            let header_focus_key = rail_project_occurrence_key("project", bucket, &key);
            nodes.push(
                AxNode::new(
                    rail_project_identifier(bucket, &key),
                    AxRole::Button,
                    project.name.clone(),
                    AxRect::new(
                        inset,
                        top,
                        if !project.is_unassigned() && project.workspace_id.is_some() {
                            (width
                                - metrics::RAIL_TRAILING_INSET
                                - metrics::RAIL_SESSION_ACTION_SIZE)
                                .max(0.0)
                        } else {
                            width
                        },
                        metrics::RAIL_TASK_ROW_HEIGHT,
                    ),
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
                .action(AxAction::Press),
            );
            let count_y =
                top + (metrics::RAIL_TASK_ROW_HEIGHT - metrics::RAIL_SESSION_ACTION_SIZE) / 2.0;
            nodes.push(AxNode::new(
                rail_project_count_identifier(bucket, &key),
                AxRole::StaticText,
                project.task_count().to_string(),
                AxRect::new(
                    inset + metrics::rail_trailing_origin_x(width),
                    count_y,
                    metrics::RAIL_SESSION_ACTION_SIZE,
                    metrics::RAIL_SESSION_ACTION_SIZE,
                ),
            ));
            if !project.is_unassigned() && project.workspace_id.is_some() {
                let add_focus_key = rail_project_occurrence_key("project-add", bucket, &key);
                nodes.push(
                    AxNode::new(
                        rail_project_add_identifier(bucket, &key),
                        AxRole::Button,
                        t("rail.new_in_project").replace("{}", &project.name),
                        AxRect::new(
                            inset + metrics::rail_trailing_plus_x(width),
                            count_y,
                            metrics::RAIL_SESSION_ACTION_SIZE,
                            metrics::RAIL_SESSION_ACTION_SIZE,
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
            consumed = metrics::RAIL_TASK_ROW_HEIGHT;
            if expanded && !project.tasks.is_empty() {
                consumed += metrics::RAIL_PROJECT_TO_TASK_GAP;
            }
        }
        if expanded {
            for session in &project.tasks {
                // 状态词与可见状态点同源（ADR-042）：Needs input 优先于
                // Running；无 live 状态不声明语义（不伪造终态）。
                let status = self.projection.session_live_status(&session.session_id);
                let unread = self.projection.session_unread(&session.session_id);
                let focus_key = rail_session_focus_key(&session.session_id);
                let is_active = self.projection.active_session_id.as_deref()
                    == Some(session.session_id.as_str());
                let renaming = self
                    .session_rename
                    .as_ref()
                    .is_some_and(|state| state.session_id == session.session_id);
                let show_actions = self.session_actions_visible(&session.session_id, window);
                let trailing_width = if show_actions {
                    metrics::rail_trailing_width()
                } else if renaming {
                    0.0
                } else {
                    metrics::RAIL_SESSION_META_WIDTH
                };
                let row_top = top + consumed;
                let action_y = row_top + (row_height - metrics::RAIL_SESSION_ACTION_SIZE) / 2.0;
                let mut row = AxNode::new(
                    session_identifier(&session.session_id),
                    AxRole::ListItem,
                    crate::ui::i18n::session_title(&session.title).to_string(),
                    AxRect::new(inset, row_top, width, row_height),
                )
                .description(session_status_description(status, unread))
                .focused(
                    self.open_menu.is_none()
                        && self
                            .rail_row_focus
                            .get(&focus_key)
                            .is_some_and(|handle| handle.is_focused(window)),
                )
                .selected(is_active);
                if let Some(status) = status {
                    row = row.child(AxNode::new(
                        session_status_dot_identifier(&session.session_id),
                        AxRole::StaticText,
                        status.label(),
                        AxRect::new(
                            inset + metrics::RAIL_TASK_PAD_X,
                            row_top + (row_height - metrics::RAIL_STATUS_DOT_SIZE) / 2.0,
                            metrics::RAIL_STATUS_DOT_SIZE,
                            metrics::RAIL_STATUS_DOT_SIZE,
                        ),
                    ));
                }
                let (title_left, title_width) = metrics::rail_session_title_layout(
                    width,
                    !skip_header || status.is_some(),
                    trailing_width,
                );
                if renaming {
                    // 行内改名：行不再激活打开，发布编辑器（Focus / SetValue
                    // 与 composer 输入同构；AXValue 即草稿纯文本）。
                    if let Some(state) = self.session_rename.as_ref() {
                        row = row.child(
                            AxNode::new(
                                "session-rename-input",
                                AxRole::TextArea,
                                t("taskrail.rename"),
                                AxRect::new(inset + title_left, row_top, title_width, row_height),
                            )
                            .value(state.input.read(cx).text().to_string())
                            .focused(
                                self.open_menu.is_none()
                                    && state.input.read(cx).focus_handle(cx).is_focused(window),
                            )
                            .action(AxAction::Focus)
                            .action(AxAction::SetValue),
                        );
                    }
                } else {
                    row = row.action(AxAction::Press).child(AxNode::new(
                        session_title_identifier(&session.session_id),
                        AxRole::StaticText,
                        crate::ui::i18n::session_title(&session.title).to_string(),
                        AxRect::new(inset + title_left, row_top, title_width, row_height),
                    ));
                    // UI-2：悬停 / 行或动作聚焦 / 当前会话，与 render 共用可见性。
                    if show_actions {
                        let rename_focus_key = rail_session_rename_focus_key(&session.session_id);
                        let archive_focus_key = rail_session_archive_focus_key(&session.session_id);
                        row = row
                            .child(
                                AxNode::new(
                                    session_rename_identifier(&session.session_id),
                                    AxRole::Button,
                                    t("taskrail.rename"),
                                    AxRect::new(
                                        (inset + metrics::rail_trailing_origin_x(width)).max(inset),
                                        action_y,
                                        metrics::RAIL_SESSION_ACTION_SIZE,
                                        metrics::RAIL_SESSION_ACTION_SIZE,
                                    ),
                                )
                                .enabled(can_create)
                                .focused(
                                    self.open_menu.is_none()
                                        && self
                                            .rail_row_focus
                                            .get(&rename_focus_key)
                                            .is_some_and(|handle| handle.is_focused(window)),
                                )
                                .action(AxAction::Press),
                            )
                            .child(
                                AxNode::new(
                                    session_archive_identifier(&session.session_id),
                                    AxRole::Button,
                                    t("taskrail.archive"),
                                    AxRect::new(
                                        (inset + metrics::rail_trailing_plus_x(width)).max(inset),
                                        action_y,
                                        metrics::RAIL_SESSION_ACTION_SIZE,
                                        metrics::RAIL_SESSION_ACTION_SIZE,
                                    ),
                                )
                                .enabled(can_create)
                                .focused(
                                    self.open_menu.is_none()
                                        && self
                                            .rail_row_focus
                                            .get(&archive_focus_key)
                                            .is_some_and(|handle| handle.is_focused(window)),
                                )
                                .action(AxAction::Press),
                            );
                    }
                }
                nodes.push(row);
                consumed += row_height;
            }
        }
        (nodes, consumed)
    }

    pub(super) fn workspace_ax(
        &self,
        window: &Window,
        cx: &App,
        frame: AxRect,
        placement: InspectorPlacement,
    ) -> AxNode {
        let header_height = self
            .shell_ax_rect("workspace-header")
            .height
            .max(metrics::HEADER_HEIGHT)
            .min(frame.height);
        let mut workspace =
            AxNode::new("workspace", AxRole::Group, "Workspace", frame).child(self.header_ax(
                window,
                AxRect::new(frame.x, frame.y, frame.width, header_height),
                placement.is_visible(),
            ));
        if placement.is_center() {
            let measured_back = self.shell_ax_rect("inspector-back-layout");
            let back_rect = if measured_back.width > 0.0 && measured_back.height > 0.0 {
                measured_back
            } else {
                AxRect::new(
                    frame.x,
                    frame.y + header_height,
                    frame.width,
                    metrics::INSPECTOR_BACK_HEIGHT,
                )
            };
            workspace = workspace.child(
                AxNode::new(
                    "inspector-back",
                    AxRole::Button,
                    t("inspector.back_to_conversation"),
                    back_rect,
                )
                .focused(self.open_menu.is_none() && self.inspector_back_focus.is_focused(window))
                .action(AxAction::Press),
            );
            let inspector_y = back_rect.y + back_rect.height;
            let inspector_height = (frame.y + frame.height - inspector_y).max(0.0);
            workspace = workspace.child(self.inspector_ax(
                window,
                cx,
                AxRect::new(frame.x, inspector_y, frame.width, inspector_height),
                placement,
            ));
            return workspace;
        }

        let input = self.text_input.read(cx);
        let input_height = input.viewport_height().unwrap_or_else(|| {
            (f32::from(window.line_height()) * input.visual_line_count() as f32
                + metrics::COMPOSER_TEXT_INSET)
                .clamp(
                    metrics::COMPOSER_INPUT_MIN_HEIGHT,
                    Self::composer_input_ax_max(),
                )
        });
        let composer_height = self
            .composer_outer_height(input_height, window)
            .min(frame.height);
        let navigation_height = if self.navigation_open() {
            self.navigation_rect("timeline-navigation").height
        } else {
            0.0
        };
        let timeline_height = (frame.height - composer_height - header_height).max(0.0);
        workspace = workspace
            .child(self.timeline_ax(
                window,
                AxRect::new(
                    frame.x,
                    frame.y + header_height + navigation_height,
                    frame.width,
                    (timeline_height - navigation_height).max(0.0),
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
            ));
        if self.navigation_open() {
            workspace = workspace.child(self.navigation_ax(window, cx));
        }
        workspace
    }

    /// F-05 Header 语义树（与 render 同源谓词 / metrics）：标题 / branch /
    /// live 终态 / 新建任务按钮。各项可见条件与 render 完全一致（无数据
    /// 诚实隐藏；标题、chip 与动作框均取自真实布局）。
    fn header_ax(&self, window: &Window, frame: AxRect, inspector_open: bool) -> AxNode {
        let mut header = AxNode::new("workspace-header", AxRole::Group, "Workspace header", frame);
        for node in self.navigation_header_ax(window) {
            header = header.child(node);
        }
        if let Some(title) = self.projection.workspace_header_title() {
            header = header.child(AxNode::new(
                "header-title",
                AxRole::StaticText,
                crate::ui::i18n::session_title(title),
                self.shell_ax_rect("header-title"),
            ));
        }
        if let Some(branch) = self.header_branch() {
            header = header.child(
                AxNode::new(
                    "header-branch",
                    AxRole::StaticText,
                    branch,
                    self.shell_ax_rect("header-branch"),
                )
                .description("Git branch"),
            );
        }
        if let Some(status) = self.projection.workspace_header_status() {
            header = header.child(
                AxNode::new(
                    "header-status",
                    AxRole::StaticText,
                    status.label(),
                    self.shell_ax_rect("header-status"),
                )
                .description("Live status"),
            );
        }
        // R6 Wave A（F-12）+ OPT-4b：与 render 同用 activity_header_visibility
        // 口径；折叠态 Activity 左移一格、inspector-expand 占最右动作槽，
        // 浮层右缘与触发器右缘对齐；展开态最右槽恢复 New task。
        let (trigger_visible, popover_visible) = activity_header_visibility(
            inspector_open,
            matches!(self.open_menu, Some(MenuKind::Activity)),
        );
        if trigger_visible {
            // 折叠态 Activity 槽位于重开按钮左侧一格（gap 与 render 同源）。
            let trigger_rect = self.shell_ax_rect("header-activity-layout");
            header = header.child(
                AxNode::new(
                    "inspector-toggle",
                    AxRole::Button,
                    t("header.tooltip_activity"),
                    trigger_rect,
                )
                .focused(
                    self.open_menu.is_none() && self.inspector_activity_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );
            if popover_visible {
                let rem_px = f32::from(window.rem_size());
                let geometry = activity_popover_ax_geometry(frame, trigger_rect, rem_px);
                let popover_bounds = self.activity_popover_layout.bounds();
                let menu_inset = metrics::MENU_PADDING + 1.0;
                let popover_frame = AxRect::new(
                    f32::from(popover_bounds.origin.x) - menu_inset,
                    f32::from(popover_bounds.origin.y) - menu_inset,
                    f32::from(popover_bounds.size.width) + 2.0 * menu_inset,
                    f32::from(popover_bounds.size.height) + 2.0 * menu_inset,
                );
                let mut popover = AxNode::new(
                    "activity-popover",
                    AxRole::Group,
                    t("changes.tab_activity"),
                    popover_frame,
                )
                .child(AxNode::new(
                    "activity-changes-heading",
                    AxRole::StaticText,
                    t("changes.tab_changes"),
                    geometry.heading,
                ))
                .child(
                    AxNode::new(
                        "activity-subagents",
                        AxRole::StaticText,
                        t("subagents.title"),
                        geometry.subagents,
                    )
                    .value(self.activity_subagent_ax_value()),
                );
                // 子代理行按钮发布真实布局，并按列表滚动视口裁剪；
                // Press 进入「子代理」对话栏。
                let scroll_bounds = self.activity_subagent_scroll.bounds();
                let scroll_offset = self.activity_subagent_scroll.offset();
                for (ix, agent) in self
                    .projection
                    .subagent_activity
                    .display_agents()
                    .into_iter()
                    .enumerate()
                {
                    let Some(mut row) = self.activity_subagent_scroll.bounds_for_item(ix) else {
                        continue;
                    };
                    row.origin += scroll_offset;
                    let row = row.intersect(&scroll_bounds);
                    if row.size.width <= gpui::px(0.0) || row.size.height <= gpui::px(0.0) {
                        continue;
                    }
                    let rect = AxRect::new(
                        row.origin.x.into(),
                        row.origin.y.into(),
                        row.size.width.into(),
                        row.size.height.into(),
                    );
                    let mut value = format!("{} · {}", agent.title, agent.model_id);
                    if let Some(effort) = &agent.effort {
                        value.push_str(" · ");
                        value.push_str(effort);
                    }
                    value.push_str(" · ");
                    value.push_str(crate::ui::changes::subagent_status_label(&agent.status));
                    popover = popover.child(
                        AxNode::new(
                            format!("activity-subagent-{}", agent.agent_id),
                            AxRole::Button,
                            t("subagents.open_conversation"),
                            rect,
                        )
                        .value(value)
                        .action(AxAction::Press),
                    );
                }
                header = header.child(popover.child({
                    let bounds = self.activity_open_changes_layout.bounds();
                    AxNode::new(
                        "activity-open-changes",
                        AxRole::Button,
                        "Open changes",
                        AxRect::new(
                            bounds.origin.x.into(),
                            bounds.origin.y.into(),
                            bounds.size.width.into(),
                            bounds.size.height.into(),
                        ),
                    )
                    .value(self.changes.activity_summary())
                    .focused(self.menu_highlight_effective(0) == 0)
                    .action(AxAction::Press)
                }));
            }
            // OPT-4b：折叠态重开 Inspector 的专用入口，占最右动作槽；展开态
            // 不发布（折叠走面板内 inspector-collapse）。
            header.child(
                AxNode::new(
                    "inspector-expand",
                    AxRole::Button,
                    t("header.tooltip_open_inspector"),
                    self.shell_ax_rect("header-expand-layout"),
                )
                .focused(self.open_menu.is_none() && self.inspector_expand_focus.is_focused(window))
                .action(AxAction::Press),
            )
        } else if self.projection.workspace_empty_hint_visible() {
            header
        } else {
            header.child(
                AxNode::new(
                    "header-new-task",
                    AxRole::Button,
                    t("timeline.new_task"),
                    self.shell_ax_rect("header-new-task-layout"),
                )
                .enabled(self.can_create_task())
                .focused(self.open_menu.is_none() && self.header_new_task_focus.is_focused(window))
                .action(AxAction::Press),
            )
        }
    }

    /// Activity 浮层「子智能体」卡的 AX 摘要值（与 render 卡同口径：
    /// 绑定会话给计数 / 空态，未绑定给无会话 / 加载 / 空态）。
    fn activity_subagent_ax_value(&self) -> String {
        let activity = &self.projection.subagent_activity;
        let bound = activity.session_id.is_some()
            && self.projection.active_session_id == activity.session_id;
        if bound {
            if let Some(error) = &activity.error {
                return format!("{} · {error}", t("subagents.load_failed"));
            }
        }
        if !bound {
            return if self.projection.active_session_id.is_none() {
                t("subagents.no_session").to_string()
            } else if activity.loading {
                t("subagents.loading").to_string()
            } else {
                t("subagents.empty").to_string()
            };
        }
        if activity.agents.is_empty() {
            return t("subagents.empty").to_string();
        }
        let (running, done) = activity.counts();
        let mut value = t("subagents.summary")
            .replacen("{}", &running.to_string(), 1)
            .replacen("{}", &done.to_string(), 1);
        // 每代理一行摘要（与 render 行同序同字段：名称 + 模型 + 强度 + 状态）。
        for agent in activity.display_agents() {
            value.push('\n');
            value.push_str(&agent.title);
            value.push_str(" · ");
            value.push_str(&agent.model_id);
            if let Some(effort) = &agent.effort {
                value.push_str(" · ");
                value.push_str(effort);
            }
            value.push_str(" · ");
            value.push_str(crate::ui::changes::subagent_status_label(&agent.status));
        }
        value
    }
}
