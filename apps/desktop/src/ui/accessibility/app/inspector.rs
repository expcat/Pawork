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
        let scope_note = t("changes.scope_note");
        let description = match &self.changes.stale_reason {
            Some(reason) => format!("{scope_note} · {fetch_state} · stale · {reason}"),
            None => format!("{scope_note} · {fetch_state}"),
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
                        t("changes.files"),
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
                        t("changes.summary"),
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
                    t("changes.tooltip_refresh"),
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
            let (can_left, can_right) =
                crate::ui::changes::diff_horizontal_ends(&self.changes.diff_scroll);
            let controls_h =
                if crate::ui::changes::diff_horizontal_overflow(&self.changes.diff_scroll) {
                    metrics::DIFF_HEADER_HEIGHT
                } else {
                    0.0
                };
            changes = changes.child(
                AxNode::new(
                    "changes-diff-view",
                    AxRole::Group,
                    "Diff view",
                    AxRect::new(
                        frame.x + PAD,
                        diff_top,
                        frame.width - PAD * 2.0,
                        (frame.height - (diff_top - frame.y) - controls_h).max(ROW_HEIGHT),
                    ),
                )
                .description(format!(
                    "horizontal offset {horizontal_offset:.1} of {horizontal_max:.1}"
                )),
            );
            if controls_h > 0.0 {
                let y = frame.y + frame.height - controls_h;
                changes = changes
                    .child(
                        AxNode::new(
                            "changes-diff-scroll-left",
                            AxRole::Button,
                            t("changes.scroll_left"),
                            AxRect::new(frame.x + frame.width - 84.0, y, 36.0, controls_h),
                        )
                        .enabled(can_left)
                        .focused(
                            self.open_menu.is_none()
                                && self.changes_diff_scroll_left_focus.is_focused(window),
                        )
                        .action(AxAction::Press),
                    )
                    .child(
                        AxNode::new(
                            "changes-diff-scroll-right",
                            AxRole::Button,
                            t("changes.scroll_right"),
                            AxRect::new(frame.x + frame.width - 44.0, y, 36.0, controls_h),
                        )
                        .enabled(can_right)
                        .focused(
                            self.open_menu.is_none()
                                && self.changes_diff_scroll_right_focus.is_focused(window),
                        )
                        .action(AxAction::Press),
                    );
            }
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
                    t("resources.tooltip_refresh"),
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
            t("resources.mcp_title"),
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
                    "{} · {}",
                    crate::ui::settings::settings_mcp_state_label(&server.state),
                    crate::ui::resources::mcp_server_meta_text(server)
                ))
                .description(server.last_error.clone().unwrap_or_default()),
            );
        }
        resources.child(list)
    }
}
