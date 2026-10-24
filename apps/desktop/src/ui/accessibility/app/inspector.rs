//! Inspector 与 terminal / changes / resources 的 AX 树。

use super::*;

impl AppView {
    fn inspector_header_ax_rects(&self, frame: AxRect) -> (AxRect, AxRect) {
        let strip_height = metrics::INSPECTOR_TAB_HEIGHT;
        let measured_panel = self.shell_ax_rect("inspector-panel-layout");
        let measured_collapse = self.shell_ax_rect("inspector-collapse-layout");
        let panel = if measured_panel.width > 0.0 && measured_panel.height > 0.0 {
            measured_panel
        } else {
            AxRect::new(
                frame.x + frame.width - PAD - 2.0 * metrics::ICON_BUTTON_SIZE - 4.0,
                frame.y,
                metrics::ICON_BUTTON_SIZE,
                strip_height,
            )
        };
        let collapse = if measured_collapse.width > 0.0 && measured_collapse.height > 0.0 {
            measured_collapse
        } else {
            AxRect::new(
                frame.x + frame.width - PAD - metrics::ICON_BUTTON_SIZE,
                frame.y + ((strip_height - metrics::ICON_BUTTON_SIZE) / 2.0).max(0.0),
                metrics::ICON_BUTTON_SIZE,
                metrics::ICON_BUTTON_SIZE,
            )
        };
        (panel, collapse)
    }

    pub(super) fn inspector_ax(
        &self,
        window: &Window,
        cx: &App,
        frame: AxRect,
        placement: InspectorPlacement,
    ) -> AxNode {
        let strip_height = metrics::INSPECTOR_TAB_HEIGHT;
        let (panel_rect, collapse_rect) = self.inspector_header_ax_rects(frame);
        let mut inspector = AxNode::new("inspector", AxRole::Group, "Inspector", frame).child(
            AxNode::new(
                "inspector-panel",
                AxRole::Button,
                t("inspector.add_tool"),
                panel_rect,
            )
            .value(self.inspector_tab.label())
            .focused(self.open_menu.is_none() && self.inspector_panel_focus.is_focused(window))
            .action(AxAction::Press),
        );
        if matches!(self.open_menu, Some(MenuKind::InspectorPanel)) {
            let highlight = self.menu_highlight_effective(self.menu_selected_index());
            let bounds = self.shell_layouts["inspector-menu-layout"].bounds();
            let menu_frame = AxRect::new(
                bounds.origin.x.into(),
                bounds.origin.y.into(),
                bounds.size.width.into(),
                bounds.size.height.into(),
            );
            let mut menu = AxNode::new(
                "inspector-panel-menu",
                AxRole::Group,
                t("inspector.add_tool"),
                menu_frame,
            );
            for (ix, tab) in InspectorTab::ALL.into_iter().enumerate() {
                menu = menu.child(
                    AxNode::new(
                        tab.menu_id(),
                        AxRole::Button,
                        if tab == InspectorTab::Terminal {
                            t("inspector.terminal_new_tab")
                        } else {
                            tab.label()
                        },
                        AxRect::new(
                            menu_frame.x + metrics::MENU_PADDING,
                            menu_frame.y
                                + metrics::MENU_PADDING
                                + ix as f32 * metrics::MENU_ROW_HEIGHT,
                            (menu_frame.width - 2.0 * metrics::MENU_PADDING).max(0.0),
                            metrics::MENU_ROW_HEIGHT,
                        ),
                    )
                    .selected(tab == self.inspector_tab && tab != InspectorTab::Terminal)
                    .focused(highlight == ix)
                    .action(AxAction::Press),
                );
            }
            inspector = inspector.child(menu);
        }
        inspector = inspector.child(
            AxNode::new(
                "inspector-collapse",
                AxRole::Button,
                t("inspector.hide"),
                collapse_rect,
            )
            .focused(self.open_menu.is_none() && self.inspector_collapse_focus.is_focused(window))
            .action(AxAction::Press),
        );
        let scroll_bounds = self.inspector_tabs_scroll.bounds();
        for tab in self.panel_tabs(cx) {
            for (id, label, selected, enabled) in [
                (tab.id.clone(), tab.label.clone(), tab.selected, true),
                (
                    tab.close_id(),
                    format!("{} · {}", t("inspector.close_tab"), tab.label),
                    false,
                    tab.close_enabled,
                ),
            ] {
                let Some(layout) = self.inspector_item_layouts.get(&id) else {
                    continue;
                };
                let rect = layout.bounds().intersect(&scroll_bounds);
                if rect.size.width <= gpui::px(0.0) || rect.size.height <= gpui::px(0.0) {
                    continue;
                }
                let focused = if id == "terminal-close" {
                    self.terminal_close_focus.is_focused(window)
                } else {
                    self.inspector_item_focus
                        .get(&id)
                        .is_some_and(|focus| focus.is_focused(window))
                };
                inspector = inspector.child(
                    AxNode::new(
                        id,
                        AxRole::Button,
                        label,
                        AxRect::new(
                            rect.origin.x.into(),
                            rect.origin.y.into(),
                            rect.size.width.into(),
                            rect.size.height.into(),
                        ),
                    )
                    .selected(selected)
                    .enabled(enabled)
                    .focused(self.open_menu.is_none() && focused)
                    .action(AxAction::Press),
                );
            }
        }
        let mut body_y = frame.y + strip_height;
        if placement.is_center() && self.projection.pending_approval.is_some() {
            let measured_hint = self.shell_ax_rect("inspector-approval-hint-layout");
            let hint_rect = if measured_hint.width > 0.0 && measured_hint.height > 0.0 {
                measured_hint
            } else {
                AxRect::new(frame.x, body_y, frame.width, metrics::INSPECTOR_BACK_HEIGHT)
            };
            inspector = inspector.child(
                AxNode::new(
                    "inspector-approval-hint",
                    AxRole::Button,
                    t("inspector.return_for_approval"),
                    hint_rect,
                )
                .focused(
                    self.open_menu.is_none() && self.inspector_approval_focus.is_focused(window),
                )
                .action(AxAction::Press),
            );
            body_y = hint_rect.y + hint_rect.height;
        }
        let body = AxRect::new(
            frame.x,
            body_y,
            frame.width,
            (frame.height - (body_y - frame.y)).max(0.0),
        );
        inspector = match self.inspector_tab {
            InspectorTab::Home => {
                if self.open_menu != Some(MenuKind::InspectorPanel) {
                    for tool in InspectorTab::ALL {
                        let id = tool.button_id();
                        let Some(layout) = self.inspector_item_layouts.get(id) else {
                            continue;
                        };
                        let rect = layout.bounds();
                        inspector = inspector.child(
                            AxNode::new(
                                id,
                                AxRole::Button,
                                tool.label(),
                                AxRect::new(
                                    rect.origin.x.into(),
                                    rect.origin.y.into(),
                                    rect.size.width.into(),
                                    rect.size.height.into(),
                                ),
                            )
                            .description(tool.hint())
                            .focused(
                                self.inspector_item_focus
                                    .get(id)
                                    .is_some_and(|focus| focus.is_focused(window)),
                            )
                            .action(AxAction::Press),
                        );
                    }
                }
                inspector
            }
            InspectorTab::Terminal => inspector.child(self.terminal_ax(window, cx, body)),
            InspectorTab::Changes => inspector.child(self.changes_ax(window, body)),
            InspectorTab::Resources => inspector.child(self.resources_ax(window, body)),
            InspectorTab::Computer => {
                inspector.child(crate::ui::computer::computer_ax(self, window, body))
            }
            InspectorTab::Browser => inspector.child(self.browser_ax(window, cx, body)),
            InspectorTab::Files => inspector.child(self.files_ax(window, cx, body)),
            InspectorTab::Subagent => {
                inspector.child(crate::ui::subagent_panel::subagent_ax(self, window, body))
            }
        };
        inspector
    }

    fn terminal_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let action_rect = |id: &str| {
            let b = self.terminal_action_layouts[id].bounds();
            AxRect::new(
                b.origin.x.into(),
                b.origin.y.into(),
                b.size.width.into(),
                b.size.height.into(),
            )
        };
        let focus = self.terminal_input.read(cx).focus_handle(cx);
        let screen = crate::ui::terminal_view::terminal_screen(
            &self.projection.terminal.output,
            self.projection.terminal.columns,
            self.projection.terminal.rows,
            window,
        );
        let output = tail_chars(
            &screen
                .display_lines()
                .into_iter()
                .map(|line| line.text)
                .collect::<Vec<_>>()
                .join("\n"),
            8_192,
        );
        let terminal_description = self.terminal_context_text();
        let mut terminal = AxNode::new(
            "terminal",
            AxRole::Group,
            t("inspector.tab_terminal"),
            frame,
        );
        let output_rect = action_rect("terminal-output");
        terminal = terminal.child(
            AxNode::new(
                "terminal-output",
                AxRole::TextArea,
                t("inspector.output"),
                output_rect,
            )
            .value(output)
            .description(terminal_description)
            .focused(self.open_menu.is_none() && focus.is_focused(window))
            .action(AxAction::Focus),
        );
        if self.terminal_notice_text().is_some() {
            terminal = terminal.child(self.recovery_ax(window, output_rect, true));
        }
        // 与可见回到底部按钮（inspector.rs）一致：仅在滚动脱钩时发布。
        if !self.terminal_scroll.is_following() {
            terminal = terminal.child(
                AxNode::new(
                    "terminal-back-to-bottom",
                    AxRole::Button,
                    t("timeline.ax_back_to_bottom"),
                    AxRect::new(
                        frame.x + frame.width - 140.0,
                        (output_rect.y + output_rect.height - 40.0).max(output_rect.y),
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
        let state = if self.inspector_workspace_id().is_none() {
            t("changes.no_workspace").into()
        } else {
            match &self.changes.fetch {
                ChangesFetch::Idle => "idle".into(),
                ChangesFetch::Fetching => "loading".into(),
                ChangesFetch::Ready => format!("ready · {} files", self.changes.files.len()),
                ChangesFetch::Failed(reason) => format!("failed · {reason}"),
            }
        };
        let mut description = format!("{} · {state}", t("changes.scope_note"));
        if let Some(reason) = &self.changes.stale_reason {
            description.push_str(&format!(" · stale · {reason}"));
        }
        let mut changes = AxNode::new("changes", AxRole::Group, t("inspector.tab_changes"), frame)
            .description(description);
        let rect = |id: &str| {
            self.changes
                .layouts
                .get(id)
                .and_then(|layout| panel_layout_rect(layout, frame))
        };
        if let Some(bounds) = rect("changes-tabs") {
            let mut tabs = AxNode::new(
                "changes-tabs",
                AxRole::TabGroup,
                t("inspector.tab_changes"),
                bounds,
            );
            for (id, tab, focus) in [
                (
                    "changes-tab-files",
                    ChangesTab::Files,
                    &self.changes_tab_focus[0],
                ),
                (
                    "changes-tab-summary",
                    ChangesTab::Summary,
                    &self.changes_tab_focus[1],
                ),
            ] {
                if let Some(bounds) = rect(id) {
                    tabs = tabs.child(
                        AxNode::new(
                            id,
                            AxRole::Tab,
                            t(if tab == ChangesTab::Files {
                                "changes.files"
                            } else {
                                "changes.summary"
                            }),
                            bounds,
                        )
                        .selected(self.changes.tab == tab)
                        .focused(self.open_menu.is_none() && focus.is_focused(window))
                        .action(AxAction::Press),
                    );
                }
            }
            changes = changes.child(tabs);
        }
        if let Some(bounds) = rect("changes-refresh") {
            changes = changes.child(
                AxNode::new(
                    "changes-refresh",
                    AxRole::Button,
                    t("changes.tooltip_refresh"),
                    bounds,
                )
                .enabled(self.changes_can_refresh())
                .focused(self.open_menu.is_none() && self.changes_refresh_focus.is_focused(window))
                .action(AxAction::Press)
                .action(AxAction::Focus),
            );
        }
        if self.changes.tab == ChangesTab::Files
            && self.inspector_workspace_id().is_some()
            && matches!(
                self.changes.fetch,
                ChangesFetch::Ready | ChangesFetch::Fetching
            )
            && !self.changes.files.is_empty()
        {
            if let Some(bounds) = panel_layout_rect(&self.changes.list_scroll, frame) {
                let mut files = AxNode::new(
                    "changes-file-list",
                    AxRole::List,
                    t("changes.files"),
                    bounds,
                );
                for file in &self.changes.files {
                    let Some(layout) = self
                        .changes
                        .layouts
                        .get(&format!("changes-file-{}", file.path))
                    else {
                        continue;
                    };
                    let Some(row) = panel_layout_rect(layout, bounds) else {
                        continue;
                    };
                    files = files.child(
                        AxNode::new(
                            diff_file_identifier(&file.path),
                            AxRole::ListItem,
                            file.path.clone(),
                            row,
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
            }
            if matches!(self.changes.diff, crate::ui::changes::DiffFetch::Ready(_)) {
                if let Some(bounds) = panel_layout_rect(&self.changes.diff_scroll, frame) {
                    changes = changes.child(
                        AxNode::new("changes-diff-view", AxRole::Group, "Diff view", bounds)
                            .description(format!(
                                "horizontal offset {:.1} of {:.1}",
                                f32::from(self.changes.diff_scroll.offset().x),
                                f32::from(self.changes.diff_scroll.max_offset().width)
                            )),
                    );
                }
                if crate::ui::changes::diff_horizontal_overflow(&self.changes.diff_scroll) {
                    let (left, right) =
                        crate::ui::changes::diff_horizontal_ends(&self.changes.diff_scroll);
                    for (id, label, enabled, focus) in [
                        (
                            "changes-diff-scroll-left",
                            "changes.scroll_left",
                            left,
                            &self.changes_diff_scroll_left_focus,
                        ),
                        (
                            "changes-diff-scroll-right",
                            "changes.scroll_right",
                            right,
                            &self.changes_diff_scroll_right_focus,
                        ),
                    ] {
                        if let Some(bounds) = rect(id) {
                            changes = changes.child(
                                AxNode::new(id, AxRole::Button, t(label), bounds)
                                    .enabled(enabled)
                                    .focused(self.open_menu.is_none() && focus.is_focused(window))
                                    .action(AxAction::Press),
                            );
                        }
                    }
                }
            }
        }
        changes
    }

    fn resources_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let state = match &self.resources.fetch {
            ResourcesFetch::Idle => "idle".into(),
            ResourcesFetch::Fetching => "loading".into(),
            ResourcesFetch::Ready => format!("ready · {} servers", self.resources.servers.len()),
            ResourcesFetch::Failed(reason) => format!("failed · {reason}"),
        };
        let mut description = format!("Host MCP servers · {state}");
        if let Some(reason) = &self.resources.stale_reason {
            description.push_str(&format!(" · stale · {reason}"));
        }
        let rect = |id: &str, clip| {
            self.resources
                .layouts
                .get(id)
                .and_then(|layout| panel_layout_rect(layout, clip))
        };
        let mut resources = AxNode::new(
            "resources",
            AxRole::Group,
            t("inspector.tab_resources"),
            frame,
        )
        .description(description);
        if let Some(bounds) = rect("resources-refresh", frame) {
            resources = resources.child(
                AxNode::new(
                    "resources-refresh",
                    AxRole::Button,
                    t("resources.tooltip_refresh"),
                    bounds,
                )
                .enabled(self.resources_can_refresh())
                .focused(
                    self.open_menu.is_none() && self.resources_refresh_focus.is_focused(window),
                )
                .action(AxAction::Press)
                .action(AxAction::Focus),
            );
        }
        let Some(bounds) = panel_layout_rect(&self.resources.scroll, frame) else {
            return resources;
        };
        let mut list = AxNode::new(
            "mcp-server-list",
            AxRole::List,
            t("resources.mcp_title"),
            bounds,
        );
        for server in &self.resources.servers {
            let id = crate::ui::resources::resources_server_id(&server.name);
            if let Some(row) = rect(&id, bounds) {
                list = list.child(
                    AxNode::new(id, AxRole::ListItem, server.name.clone(), row)
                        .value(format!(
                            "{} · {}",
                            crate::ui::settings::settings_mcp_state_label(&server.state),
                            crate::ui::resources::mcp_server_meta_text(server)
                        ))
                        .description(server.last_error.clone().unwrap_or_default()),
                );
            }
        }
        for (id, focus) in &self.resources.details_focus {
            if let Some(row) = rect(id, bounds) {
                let expanded = self.resources.details_open.contains(id);
                list = list.child(
                    AxNode::new(
                        id.clone(),
                        AxRole::Button,
                        t(if expanded {
                            "resources.hide_details"
                        } else {
                            "resources.details"
                        }),
                        row,
                    )
                    .value(if expanded { "expanded" } else { "collapsed" })
                    .focused(self.open_menu.is_none() && focus.is_focused(window))
                    .action(AxAction::Press)
                    .action(AxAction::Focus),
                );
            }
        }
        resources.child(list)
    }
}

/// Publish only measured, visible layout; large text and expanded errors may change every row.
fn panel_layout_rect(layout: &gpui::ScrollHandle, clip: AxRect) -> Option<AxRect> {
    let bounds = layout.bounds().intersect(&gpui::Bounds::new(
        gpui::point(gpui::px(clip.x), gpui::px(clip.y)),
        gpui::size(gpui::px(clip.width), gpui::px(clip.height)),
    ));
    (bounds.size.width > gpui::px(0.0) && bounds.size.height > gpui::px(0.0)).then(|| {
        AxRect::new(
            bounds.origin.x.into(),
            bounds.origin.y.into(),
            bounds.size.width.into(),
            bounds.size.height.into(),
        )
    })
}
