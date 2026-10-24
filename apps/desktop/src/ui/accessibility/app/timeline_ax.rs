//! Timeline AX 树。

use super::*;

impl AppView {
    pub(super) fn timeline_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let rows = self.projection.timeline_rows();
        let total = rows.len();
        let empty_hint_visible = self.welcome_visible();
        // UI-3：与 timeline.rs render 同源——列在 Workspace 中居中，
        // 两侧至少留 CONTENT_INSET；行高 / 间距按内容推导（gpui list 按像素
        // 布局，此为同源公式）。滚动位置沿用已验证安全的 logical_scroll_top()
        // 只读，不触碰写借用。
        let rem_px = f32::from(window.rem_size());
        let column_width = (frame.width - 2.0 * metrics::TIMELINE_CONTENT_INSET)
            .min(metrics::TIMELINE_READABLE_WIDTH)
            .max(0.0);
        let column_x = frame.x + (frame.width - column_width) / 2.0;
        let layouts: Vec<(f32, f32)> = rows
            .iter()
            .map(|row| {
                (
                    timeline::row_top_gap(row),
                    timeline::timeline_row_height(
                        row,
                        &self.projection.timeline,
                        column_width,
                        rem_px,
                        &self.expanded_timeline_details,
                        self.changes_available_for_active(),
                        self.projection.active_run_id.as_deref(),
                    ),
                )
            })
            .collect();
        let approval_height = self.projection.pending_approval.as_ref().map(|pending| {
            approval_card_height(
                &pending.tool_name,
                &pending.reason,
                pending.detail.as_deref(),
                column_width,
                rem_px,
            )
        });
        // render 的 list count = timeline rows + 可选 approval 末项；AX 使用
        // 同一 item 序列，approval 的 MSG_ENTRY_GAP 也属于末项自身。
        let mut item_layouts = layouts.clone();
        if let Some(height) = approval_height {
            item_layouts.push((
                if rows.is_empty() {
                    0.0
                } else {
                    metrics::MSG_ENTRY_GAP
                },
                height,
            ));
        }
        let viewport_height = (frame.height - metrics::TIMELINE_TOP_GAP).max(0.0);
        let measured_viewport = self.timeline_list.viewport_bounds();
        let has_measured_layout = f32::from(measured_viewport.size.height) > 0.0;
        let (start, offset_in_first_item) = if has_measured_layout {
            // AX 同步发生在本帧 list 构建前；上一帧的 ListState 是已完成
            // prepaint 的真实滚动事实，可安全只读（handler 内仍禁止读取）。
            let scroll = self.timeline_list.logical_scroll_top();
            (
                scroll.item_ix.min(item_layouts.len()),
                f32::from(scroll.offset_in_item),
            )
        } else if self.timeline_following {
            // bottom padding 只影响贴底补齐；滚动时仍可画到完整视口底边。
            timeline::timeline_following_window(
                &item_layouts,
                (viewport_height - metrics::MSG_ENTRY_GAP).max(0.0),
            )
        } else {
            (0, 0.0)
        };
        let content_top = frame.y + metrics::TIMELINE_TOP_GAP;
        let formula_items = timeline::timeline_visible_item_tops(
            &item_layouts,
            content_top,
            viewport_height,
            start,
            offset_in_first_item,
        );
        let content_bottom = content_top + viewport_height;
        let measured_items = if has_measured_layout {
            // 稳定帧直接沿 ListState 已测 item 连续枚举；若仍先用公式裁出
            // 候选，长文本（尤其 CJK）估高偏差可能漏掉实际可见的后续项。
            // bounds_for_item 对未渲染项返回 None，故在可见/overdraw 实测段
            // 结束处自然停止，成本只随已测窗口增长。
            let mut items = Vec::new();
            let mut saw_bounds = false;
            for ix in start..item_layouts.len() {
                let Some(bounds) = self.timeline_list.bounds_for_item(ix) else {
                    break;
                };
                saw_bounds = true;
                let gap = if ix > 0 { item_layouts[ix].0 } else { 0.0 };
                // GPUI bounds_for_item 返回 item 外框且不含 list padding；
                // render 的 pt(gap) 计入 item 高度，故内容 rect 再内缩 gap。
                let top = f32::from(bounds.origin.y) + metrics::TIMELINE_TOP_GAP + gap;
                let height = (f32::from(bounds.size.height) - gap).max(0.0);
                if top >= content_bottom {
                    break;
                }
                if top + height > content_top {
                    items.push((ix, top, height));
                }
            }
            saw_bounds.then_some(items)
        } else {
            None
        };
        let visible_items: Vec<(usize, f32, f32)> = measured_items.unwrap_or_else(|| {
            formula_items
                .into_iter()
                .map(|(ix, top)| (ix, top, item_layouts[ix].1))
                .collect()
        });
        let mut list = AxNode::new("timeline", AxRole::List, "Timeline", frame);
        let offline = !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        if offline {
            list = list.child(self.recovery_ax(window, frame, false));
        }
        if empty_hint_visible && !offline {
            let can_create = self.can_create_task();
            let mut new_task = AxNode::new(
                "header-new-task",
                AxRole::Button,
                t("timeline.new_task"),
                self.shell_ax_rect("workspace-empty-action"),
            )
            .description(self.add_task_disabled_reason())
            .enabled(can_create)
            .focused(self.open_menu.is_none() && self.header_new_task_focus.is_focused(window));
            if can_create {
                new_task = new_task.action(AxAction::Press);
            }
            list = list.child(AxNode::new(
                "workspace-empty-title",
                AxRole::StaticText,
                workspace_empty_title(),
                self.shell_ax_rect("workspace-empty-title"),
            ));
            if self.welcome_hint_visible() {
                list = list.child(AxNode::new(
                    "workspace-empty-hint",
                    AxRole::StaticText,
                    self.welcome_hint(),
                    self.shell_ax_rect("workspace-empty-hint"),
                ));
            }
            if self.projection.active_session_id.is_none() {
                list = list.child(new_task);
            }
            if self.welcome_bind_project_visible() {
                let mut bind = AxNode::new(
                    "workspace-bind-project",
                    AxRole::Button,
                    t("timeline.bind_project"),
                    self.shell_ax_rect("workspace-empty-action"),
                )
                .enabled(can_create)
                .focused(
                    self.open_menu.is_none() && self.welcome_bind_project_focus.is_focused(window),
                );
                if can_create {
                    bind = bind.action(AxAction::Press).action(AxAction::Focus);
                }
                list = list.child(bind);
            }
        }
        for &(ix, top, height) in visible_items.iter().filter(|(ix, _, _)| *ix < total) {
            let rect = AxRect::new(column_x, top, column_width, height);
            list = list.child(self.timeline_row_ax(window, &rows[ix], rect));
        }
        let approval_top = visible_items
            .iter()
            .find_map(|(ix, top, height)| (*ix == total).then_some((*top, *height)));
        if let (Some(pending), Some((approval_top, approval_height))) =
            (self.projection.pending_approval.as_ref(), approval_top)
        {
            // 卡仍是 list 末项；按钮只发布实际绘制且位于视口内的框。
            let measured = |id: &str| {
                self.settings_element_layouts.get(id).map(|layout| {
                    let b = layout.bounds().intersect(&measured_viewport);
                    AxRect::new(
                        b.origin.x.into(),
                        b.origin.y.into(),
                        f32::from(b.size.width).max(0.0),
                        f32::from(b.size.height).max(0.0),
                    )
                })
            };
            let approval = measured("approval-card").unwrap_or(AxRect::new(
                column_x,
                approval_top,
                column_width,
                approval_height,
            ));
            let enabled = self.can_approve();
            let focused = [
                self.open_menu.is_none() && self.approve_once_focus.is_focused(window),
                self.open_menu.is_none() && self.approve_for_run_focus.is_focused(window),
                self.open_menu.is_none() && self.deny_focus.is_focused(window),
            ];
            let mut approval_node =
                AxNode::new("approval-card", AxRole::Group, "Approval", approval)
                    .value(format!("{} · {}", pending.tool_name, pending.reason));
            for (ix, (id, label)) in [
                ("approve-once", t("approval.allow_once")),
                ("approve-for-run", t("approval.allow_for_run")),
                ("approve-deny", t("approval.deny")),
            ]
            .into_iter()
            .enumerate()
            {
                let Some(bounds) = measured(id).filter(|b| b.width > 0.0 && b.height > 0.0) else {
                    continue;
                };
                approval_node = approval_node.child({
                    let mut button = AxNode::new(id, AxRole::Button, label, bounds)
                        .enabled(enabled)
                        .focused(focused[ix]);
                    if enabled {
                        button = button.action(AxAction::Press);
                    }
                    button
                });
            }
            list = list.child(approval_node);
        }
        if !self.timeline_following && self.timeline_content_overflows(window) {
            list = list.child(
                AxNode::new(
                    "timeline-back-to-bottom",
                    AxRole::Button,
                    t("timeline.ax_back_to_bottom"),
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

    fn tool_row_ax_node(
        &self,
        window: &Window,
        entry: &TimelineEntry,
        row: &ToolRowView,
        rect: AxRect,
    ) -> AxNode {
        let rem = f32::from(window.rem_size());
        let height =
            timeline::tool_entry_height(entry, rect.width, rem, &self.expanded_timeline_details);
        let mut node = AxNode::new(
            dynamic_identifier("tool-row", &entry.event_id),
            AxRole::ListItem,
            format!("Tool · {}", row.headline),
            AxRect::new(rect.x, rect.y, rect.width, height),
        )
        .value(row.status_label.clone())
        .description(row.detail.clone().unwrap_or_default());
        if row
            .detail
            .as_deref()
            .is_some_and(|detail| !crate::ui::markdown::source_links(detail).is_empty())
        {
            node = node.child(
                AxNode::new(
                    entry_menu_identifier(&entry.event_id),
                    AxRole::Button,
                    t("timeline.actions"),
                    AxRect::new(rect.x + rect.width - 36.0, rect.y, 24.0, 24.0),
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
                node = self.entry_actions_ax(node, &entry.event_id);
            }
        }
        if row.result_can_expand && !row.event_id.is_empty() {
            let key = tool_result_expand_key(&entry.event_id);
            node = node.child(
                AxNode::new(
                    dynamic_identifier("tool-result-toggle", &entry.event_id),
                    AxRole::Button,
                    if row.result_expanded {
                        t("tool.result_collapse")
                    } else {
                        t("tool.result_expand")
                    },
                    AxRect::new(
                        rect.x,
                        rect.y + height - crate::ui::timeline_entry::RESULT_EXPAND_HEIGHT,
                        rect.width,
                        crate::ui::timeline_entry::RESULT_EXPAND_HEIGHT,
                    ),
                )
                .focused(
                    self.open_menu.is_none()
                        && self
                            .timeline_detail_focus
                            .get(&key)
                            .is_some_and(|focus| focus.is_focused(window)),
                )
                .action(AxAction::Press),
            );
        }
        node
    }

    /// 渲染行 → AX 节点（与 timeline.rs 组装同源）。消息 / 错误条目保持
    /// 既有 timeline-entry / entry-menu / fork identifier；tool 组与 Run
    /// 摘要区域为新结构节点。
    pub(super) fn timeline_row_ax(
        &self,
        window: &Window,
        row: &TimelineRow,
        rect: AxRect,
    ) -> AxNode {
        match row {
            TimelineRow::Thinking { entry_index } => {
                let entry = &self.projection.timeline[*entry_index];
                let key = &entry.event_id;
                let expanded = self.expanded_timeline_details.contains(key);
                let mut group = AxNode::new(
                    dynamic_identifier("thinking", key),
                    AxRole::Group,
                    t("timeline.thinking"),
                    rect,
                )
                .child(
                    AxNode::new(
                        dynamic_identifier("thinking-toggle", key),
                        AxRole::Button,
                        t("timeline.thinking"),
                        AxRect::new(rect.x, rect.y, rect.width, metrics::THINKING_HEADER_HEIGHT),
                    )
                    .description(if expanded { "Expanded" } else { "Collapsed" })
                    .focused(
                        self.open_menu.is_none()
                            && self
                                .timeline_detail_focus
                                .get(key)
                                .is_some_and(|focus| focus.is_focused(window)),
                    )
                    .action(AxAction::Press),
                );
                if expanded {
                    if let TimelineEntryKind::Thinking { text } = &entry.kind {
                        group = group.child(
                            AxNode::new(
                                dynamic_identifier("thinking-text", key),
                                AxRole::StaticText,
                                t("timeline.thinking"),
                                AxRect::new(
                                    rect.x + 12.0,
                                    rect.y + metrics::THINKING_HEADER_HEIGHT + 12.0,
                                    (rect.width - 24.0).max(0.0),
                                    (rect.height - metrics::THINKING_HEADER_HEIGHT - 24.0).max(0.0),
                                ),
                            )
                            .value(text.clone()),
                        );
                    }
                }
                group
            }
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
                let group_key = timeline::tool_group_key(entry_indices, &self.projection.timeline)
                    .unwrap_or_default();
                let rows = self.tool_row_views(entry_indices);
                let collapsed = !self.expanded_timeline_details.contains(group_key);
                let mut group = AxNode::new(
                    dynamic_identifier("tool-group", group_key),
                    AxRole::Group,
                    "Tool activity",
                    rect,
                )
                .child(
                    AxNode::new(
                        tool_group_toggle_identifier(group_key),
                        AxRole::Button,
                        "Tool activity",
                        AxRect::new(
                            rect.x,
                            rect.y,
                            rect.width,
                            metrics::TOOL_GROUP_HEADER_HEIGHT,
                        ),
                    )
                    .value(tool_group_summary(&rows))
                    .description(if collapsed { "Collapsed" } else { "Expanded" })
                    .focused(
                        self.open_menu.is_none()
                            && self
                                .timeline_detail_focus
                                .get(group_key)
                                .is_some_and(|focus| focus.is_focused(window)),
                    )
                    .action(AxAction::Press),
                );
                if collapsed {
                    return group;
                }
                let mut tool_y = rect.y + metrics::TOOL_GROUP_HEADER_HEIGHT;
                for &entry_index in entry_indices {
                    let entry = &self.projection.timeline[entry_index];
                    let Some(row) = tool_call_row_view(
                        entry,
                        &self.expanded_timeline_details,
                        &self.projection.timeline,
                    ) else {
                        continue;
                    };
                    let rem = f32::from(window.rem_size());
                    let height = timeline::tool_entry_height(
                        entry,
                        rect.width,
                        rem,
                        &self.expanded_timeline_details,
                    );
                    group = group.child(self.tool_row_ax_node(
                        window,
                        entry,
                        &row,
                        AxRect::new(rect.x, tool_y, rect.width, height),
                    ));
                    tool_y += height;
                }
                group
            }
            TimelineRow::RunSummary { group, terminal } => {
                let terminal_entry = &self.projection.timeline[*terminal];
                let review_available = timeline::review_available_for_run(
                    &self.projection.timeline,
                    terminal_entry,
                    self.changes_available_for_active(),
                );
                let now_ms = crate::ui::now_unix_ms();
                let mut region = AxNode::new(
                    dynamic_identifier("run-summary", &terminal_entry.event_id),
                    AxRole::Group,
                    "Run summary",
                    rect,
                );
                let mut y = rect.y;
                if let Some(entry_indices) = group {
                    let group_key =
                        timeline::tool_group_key(entry_indices, &self.projection.timeline)
                            .unwrap_or_default();
                    let rows = self.tool_row_views(entry_indices);
                    let collapsed = !self.expanded_timeline_details.contains(group_key);
                    region = region.child(
                        AxNode::new(
                            tool_group_toggle_identifier(group_key),
                            AxRole::Button,
                            "Tool activity",
                            AxRect::new(rect.x, y, rect.width, metrics::TOOL_GROUP_HEADER_HEIGHT),
                        )
                        .value(tool_group_summary(&rows))
                        .description(if collapsed { "Collapsed" } else { "Expanded" })
                        .focused(
                            self.open_menu.is_none()
                                && self
                                    .timeline_detail_focus
                                    .get(group_key)
                                    .is_some_and(|focus| focus.is_focused(window)),
                        )
                        .action(AxAction::Press),
                    );
                    y += metrics::TOOL_GROUP_HEADER_HEIGHT;
                    if !collapsed {
                        for &entry_index in entry_indices {
                            let entry = &self.projection.timeline[entry_index];
                            let Some(row) = tool_call_row_view(
                                entry,
                                &self.expanded_timeline_details,
                                &self.projection.timeline,
                            ) else {
                                continue;
                            };
                            let rem = f32::from(window.rem_size());
                            let height = timeline::tool_entry_height(
                                entry,
                                rect.width,
                                rem,
                                &self.expanded_timeline_details,
                            );
                            region = region.child(self.tool_row_ax_node(
                                window,
                                entry,
                                &row,
                                AxRect::new(rect.x, y, rect.width, height),
                            ));
                            y += height;
                        }
                    }
                    y += if timeline::run_summary_card_visible(terminal_entry, review_available) {
                        metrics::SUMMARY_CARD_GAP
                    } else {
                        metrics::TIMELINE_FOOTER_GAP
                    };
                }
                let review_enabled = terminal_entry.fork_boundary == Some(ForkBoundary::Completed)
                    && review_available;
                let (title, description) = run_summary_texts(terminal_entry, review_enabled)
                    .unwrap_or(("Run", String::new()));
                let show_card = timeline::run_summary_card_visible(terminal_entry, review_enabled);
                let rem = f32::from(window.rem_size());
                let layout = if show_card {
                    Some(timeline::run_summary_card_layout(
                        terminal_entry,
                        rect.width,
                        rem,
                        review_enabled,
                    ))
                } else {
                    None
                };
                if let Some(layout) = layout.as_ref() {
                    let card_rect = AxRect::new(rect.x, y, rect.width, layout.height);
                    region = region.child(
                        AxNode::new(
                            dynamic_identifier("run-summary-card", &terminal_entry.event_id),
                            AxRole::StaticText,
                            title,
                            card_rect,
                        )
                        .description(description),
                    );
                    if review_enabled {
                        region = region.child(
                            AxNode::new(
                                run_review_identifier(&terminal_entry.event_id),
                                AxRole::Button,
                                t("timeline.review_changes"),
                                AxRect::new(
                                    (rect.x + rect.width - metrics::SUMMARY_BUTTON_WIDTH)
                                        .max(rect.x),
                                    y,
                                    metrics::SUMMARY_BUTTON_WIDTH,
                                    metrics::SUMMARY_BUTTON_HEIGHT,
                                ),
                            )
                            .focused(
                                self.open_menu.is_none()
                                    && self
                                        .timeline_review_changes_focus
                                        .get(&terminal_entry.event_id)
                                        .is_some_and(|focus| focus.is_focused(window)),
                            )
                            .action(AxAction::Press),
                        );
                    }
                    let cta_id = run_open_providers_identifier(&terminal_entry.event_id);
                    let cta_bounds = self
                        .settings_element_layouts
                        .get(&cta_id)
                        .map(|handle| {
                            let b = handle
                                .bounds()
                                .intersect(&self.timeline_list.viewport_bounds());
                            AxRect::new(
                                b.origin.x.into(),
                                b.origin.y.into(),
                                f32::from(b.size.width).max(0.0),
                                f32::from(b.size.height).max(0.0),
                            )
                        })
                        .filter(|b| b.width > 0.0 && b.height > 0.0);
                    if let Some(bounds) = cta_bounds
                        .filter(|_| layout.next_step == FailureNextStep::OpenProviderSettings)
                    {
                        region = region.child(
                            AxNode::new(
                                cta_id,
                                AxRole::Button,
                                t("run.open_provider_settings"),
                                bounds,
                            )
                            .focused(
                                self.open_menu.is_none()
                                    && self
                                        .timeline_open_providers_focus
                                        .get(&terminal_entry.event_id)
                                        .is_some_and(|focus| focus.is_focused(window)),
                            )
                            .action(AxAction::Press),
                        );
                    }
                    y += layout.height + metrics::TIMELINE_FOOTER_GAP;
                }
                if let Some(label) = run_footer_label(terminal_entry) {
                    let footer_label = self.projection.run_footer_display_label(
                        terminal_entry.run_id.as_deref(),
                        label,
                        now_ms,
                    );
                    region = region.child(AxNode::new(
                        dynamic_identifier("run-footer", &terminal_entry.event_id),
                        AxRole::StaticText,
                        format!(
                            "{} · {}",
                            footer_label,
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
                        t("timeline.actions"),
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
                    entry_node = self.entry_actions_ax(entry_node, &terminal_entry.event_id);
                }
                region.child(entry_node)
            }
            TimelineRow::LiveRunMetrics => {
                let now_ms = crate::ui::now_unix_ms();
                AxNode::new(
                    "run-live-metrics",
                    AxRole::StaticText,
                    t("run.duration"),
                    rect,
                )
                .value(
                    self.projection
                        .run_usage_display(self.projection.active_run_id.as_deref(), now_ms)
                        .status_label(),
                )
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
        let generating = assistant_is_streaming(
            &self.projection.timeline,
            entry,
            self.projection.active_run_id.as_deref(),
        );
        let (label, value) = timeline_accessible_text(entry, generating);
        let mut node = AxNode::new(
            dynamic_identifier("timeline-entry", &entry.event_id),
            AxRole::ListItem,
            label,
            row,
        )
        .value(value)
        .description(display_time(&entry.timestamp, now_ms));
        // RV-02：附件折叠头按分段布局累加器发布（与渲染 / 测高同源公式，
        // 气泡强制整宽后附件块 x = 列宽内缩气泡 padding + border）。
        if let TimelineEntryKind::UserMessage { text } = &entry.kind {
            if let Some(segments) = crate::ui::attachment_blocks::split_user_message(text) {
                let rem_px = f32::from(window.rem_size());
                let (_, layouts) = crate::ui::timeline::user_segment_layouts(
                    &entry.event_id,
                    &segments,
                    row.width,
                    rem_px,
                    &self.expanded_timeline_details,
                );
                for (segment, layout) in segments.iter().zip(layouts.iter()) {
                    let Some(key) = &layout.attachment_key else {
                        continue;
                    };
                    let name = match segment {
                        crate::ui::attachment_blocks::AttachmentSegment::File { name, .. } => {
                            name.clone()
                        }
                        _ => continue,
                    };
                    let expanded = self.expanded_timeline_details.contains(key);
                    node = node.child(
                        AxNode::new(
                            dynamic_identifier("attachment-toggle", key),
                            AxRole::Button,
                            format!(
                                "{name} · {}",
                                crate::ui::attachment_blocks::attachment_meta(segment)
                            ),
                            AxRect::new(
                                row.x + metrics::MSG_USER_INSET_X + 1.0,
                                row.y + layout.top + 1.0,
                                (row.width - 2.0 * metrics::MSG_USER_INSET_X - 2.0).max(0.0),
                                metrics::THINKING_HEADER_HEIGHT,
                            ),
                        )
                        .description(if expanded { "Expanded" } else { "Collapsed" })
                        .focused(
                            self.open_menu.is_none()
                                && self
                                    .timeline_detail_focus
                                    .get(key)
                                    .is_some_and(|focus| focus.is_focused(window)),
                        )
                        .action(AxAction::Press),
                    );
                }
            }
        }
        if with_menu {
            let measured = self
                .settings_element_layouts
                .get(&format!("message-actions-{}", entry.event_id))
                .map(|layout| {
                    let b = layout.bounds();
                    AxRect::new(
                        b.origin.x.into(),
                        b.origin.y.into(),
                        b.size.width.into(),
                        b.size.height.into(),
                    )
                })
                .unwrap_or(AxRect::new(0., 0., 0., 0.));
            let actions_rect = if measured.width > 0.0 && measured.height > 0.0 {
                measured
            } else {
                AxRect::new(row.x + row.width - 32.0, row.y, 32.0, 24.0)
            };
            node = node.child(
                AxNode::new(
                    entry_menu_identifier(&entry.event_id),
                    AxRole::Button,
                    t("timeline.actions"),
                    actions_rect,
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
                node = self.entry_actions_ax(node, &entry.event_id);
            }
        }
        if let Some(text) = timeline_entry_markdown(entry) {
            node = self.message_code_ax(node, &entry.event_id, text, window);
        }
        node
    }

    fn message_code_ax(
        &self,
        mut node: AxNode,
        event_id: &str,
        text: &str,
        window: &Window,
    ) -> AxNode {
        for copy in crate::ui::markdown::message_code_copies(event_id, text) {
            let measured = self
                .settings_element_layouts
                .get(&copy.identifier)
                .map(|layout| {
                    let b = layout.bounds();
                    AxRect::new(
                        b.origin.x.into(),
                        b.origin.y.into(),
                        b.size.width.into(),
                        b.size.height.into(),
                    )
                })
                .unwrap_or(AxRect::new(0., 0., 0., 0.));
            node = node.child(
                AxNode::new(
                    copy.identifier,
                    AxRole::Button,
                    t("timeline.copy_code"),
                    measured,
                )
                .focused(
                    self.open_menu.is_none()
                        && self
                            .timeline_detail_focus
                            .get(&copy.focus_key)
                            .is_some_and(|focus| focus.is_focused(window)),
                )
                .action(AxAction::Press),
            );
        }
        node
    }

    fn entry_actions_ax(&self, mut node: AxNode, event_id: &str) -> AxNode {
        let bounds = self.entry_menu_scroll.bounds();
        let highlight = self.menu_highlight_effective(0);
        for (ix, action) in self.entry_menu_actions(event_id).into_iter().enumerate() {
            let Some(mut row) = self.entry_menu_scroll.bounds_for_item(ix) else {
                continue;
            };
            row.origin += self.entry_menu_scroll.offset();
            let row = row.intersect(&bounds);
            if row.size.width <= gpui::px(0.0) || row.size.height <= gpui::px(0.0) {
                continue;
            }
            let enabled = match action.kind {
                super::super::super::timeline_entry::EntryActionKind::Fork => {
                    self.can_fork_entry(event_id)
                }
                super::super::super::timeline_entry::EntryActionKind::Info => false,
                _ => true,
            };
            let mut child = AxNode::new(
                action.identifier(event_id, ix),
                AxRole::Button,
                action.label,
                AxRect::new(
                    row.origin.x.into(),
                    row.origin.y.into(),
                    row.size.width.into(),
                    row.size.height.into(),
                ),
            )
            .enabled(enabled)
            .focused(ix == highlight);
            if enabled {
                child = child.action(AxAction::Press);
            }
            if matches!(
                action.kind,
                super::super::super::timeline_entry::EntryActionKind::Fork
            ) && !enabled
            {
                child = child.description(self.fork_unavailable_reason(event_id));
            }
            node = node.child(child);
        }
        node
    }
}
