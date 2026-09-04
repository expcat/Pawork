//! AppView → AxTree 投影与 AX action 白名单。

use gpui::{App, Context, Focusable, Window};
use std::str::FromStr;

use crate::projection::{
    run_footer_label, run_summary_texts, ApprovalModeWire, ConnectionState, DateBucket,
    ForkBoundary, ModelEntry, SessionLiveStatus, TaskRailGrouping, TaskRailProjectGroup,
    TimelineEntry, TimelineEntryKind, TimelineRow, UNASSIGNED_PROJECT,
};

use super::{AxAction, AxBridge, AxNode, AxRect, AxRequest, AxRole, AxTree};
use crate::ui::approval_card::{
    approval_card_height, APPROVAL_BUTTON_HEIGHT, APPROVAL_BUTTON_ROW_GAP_REMS,
    APPROVAL_BUTTON_SLOT_WIDTHS, APPROVAL_CARD_PAD_REMS,
};
use crate::ui::changes::{ChangesFetch, ChangesTab};
use crate::ui::components::dropdown::{ANCHOR_GAP_Y, MENU_MAX_HEIGHT};
use crate::ui::input_area::{grouped_model_menu_entries, MODEL_MENU_GROUP_HEADER_HEIGHT};
use crate::ui::inspector::{
    plain_terminal_output, terminal_header_height, terminal_resize_status_label,
    terminal_size_for_display, terminal_stepper_ax_rects, InspectorTab, TERMINAL_COLUMNS_STEP,
    TERMINAL_EMPTY_OUTPUT, TERMINAL_ROWS_STEP,
};
use crate::ui::resources::ResourcesFetch;
use crate::ui::settings::{
    parse_settings_control, parse_settings_mcp_control, settings_text_scale_from_identifier,
    SettingsControl, SETTINGS_APPEARANCE_CONTROL_GAP, SETTINGS_APPEARANCE_CONTROL_HEIGHT,
    SETTINGS_APPEARANCE_CONTROL_WIDTH, SETTINGS_CONTROL_PREFIX, SETTINGS_MCP_CONTROL_PREFIX,
};
use crate::ui::shell_layout;
use crate::ui::theme::{font, metrics};
use crate::ui::timeline_entry::{display_time, tool_group_summary};
use crate::ui::{
    activity_header_visibility, rail_project_occurrence_key, rail_session_focus_key,
    terminal_can_operate, terminal_can_reopen, terminal_close_label, terminal_known_ended,
    terminal_start_enabled, timeline, AppRoute, AppView, MenuKind, SettingsPage,
    WORKSPACE_EMPTY_HINT, WORKSPACE_EMPTY_TITLE,
};

pub(crate) const PAD: f32 = 8.0;
pub(crate) const CONTROL_HEIGHT: f32 = 28.0;
pub(crate) const ROW_HEIGHT: f32 = 32.0;
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

/// 按钮行 y 自卡底内缩（p_2 + 32px 按钮槽），随卡位置整体移动。
fn approval_button_row_y(card: AxRect, rem_px: f32) -> f32 {
    card.y + card.height - APPROVAL_CARD_PAD_REMS * rem_px - APPROVAL_BUTTON_HEIGHT
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
        // SET-4：settings secure 输入（Focus / SetValue 合法输入路径；发布
        // 方向只给掩码，见 settings_page_ax）。
        let settings_api_key_input =
            parse_settings_control(&request.identifier).and_then(|control| match control {
                SettingsControl::ApiKeyInput(escaped) => Some(escaped),
                _ => None,
            });
        match request.action {
            AxAction::Focus => match request.identifier.as_str() {
                "composer-input" => self.focus_composer(window, cx),
                "terminal-input" => {
                    let focus = self.terminal_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-proxy-input" => {
                    let focus = self.settings_proxy_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-terminal-shell-input" => {
                    let focus = self.settings_terminal_shell_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-terminal-columns-input" => {
                    let focus = self
                        .settings_terminal_columns_input
                        .read(cx)
                        .focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-terminal-rows-input" => {
                    let focus = self.settings_terminal_rows_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                // SET-4：settings secure 输入聚焦（与点击输入框同一路径；
                // permits 已按当前树核对 enabled）。
                _ if settings_api_key_input.is_some() => {
                    let escaped = settings_api_key_input.clone().unwrap_or_default();
                    if let Some(provider_id) = self.settings_provider_id_for_escaped(&escaped) {
                        if let Some(input) = self.settings_api_key_inputs.get(&provider_id) {
                            let focus = input.read(cx).focus_handle(cx);
                            window.focus(&focus);
                        }
                    }
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
                    "settings-proxy-input" => self
                        .settings_proxy_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "settings-terminal-shell-input" => self
                        .settings_terminal_shell_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "settings-terminal-columns-input" => self
                        .settings_terminal_columns_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "settings-terminal-rows-input" => self
                        .settings_terminal_rows_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    // SET-4：AX set-value 是合法输入路径（等同键入）；
                    // 发布方向永远只给掩码（settings_page_ax）。
                    _ if settings_api_key_input.is_some() => {
                        let escaped = settings_api_key_input.clone().unwrap_or_default();
                        if let Some(provider_id) = self.settings_provider_id_for_escaped(&escaped) {
                            if let Some(input) = self.settings_api_key_inputs.get(&provider_id) {
                                input.update(cx, |input, cx| input.set_text(value, cx));
                            }
                        }
                    }
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
            // P0-2：AX Press 与 mouse / Enter / Space 共用直接切换路径。
            "task-rail-grouping" => self.toggle_grouping(window, cx),
            "project-scope" => {
                window.focus(&self.scope_focus);
                self.on_toggle_scope_menu(None, window, cx)
            }
            "scope-add-project" | "workspace-confirm-add-project" => {
                self.on_open_project(window, cx)
            }
            // D1 同类：All-projects 态会开 WorkspaceConfirm 菜单，AXPress
            // 同样先移焦触发器，否则焦点滞留 composer、Enter 误触 SendMessage
            //（已解析 workspace 时 create_task 随后聚焦 composer，无害）。
            "add-task" => {
                window.focus(&self.add_task_focus);
                self.on_new_session(window, cx)
            }
            // F-05 Header 动作：与 rail 全局「+」同 handler / enable gate。
            "header-new-task" => {
                window.focus(&self.header_new_task_focus);
                self.on_new_session(window, cx)
            }
            "reconnect" => self.on_reconnect(window, cx),
            // SET-3：Settings 进出与可见 / 键盘路径同一 handler。
            "open-settings" => self.on_open_settings(window, cx),
            "settings-back" => self.on_close_settings(window, cx),
            // SET-5：页级刷新与可见按钮同一 handler（permits 按当前树核对
            // disabled）。
            "settings-refresh" => self.on_refresh_settings(cx),
            "settings-nav-general" => {
                window.focus(&self.settings_nav_general_focus);
                self.on_select_settings_page(SettingsPage::General, window, cx);
            }
            "settings-nav-providers" => {
                window.focus(&self.settings_nav_providers_focus);
                self.on_select_settings_page(SettingsPage::Providers, window, cx);
            }
            "settings-nav-permissions" => {
                window.focus(&self.settings_nav_permissions_focus);
                self.on_select_settings_page(SettingsPage::Permissions, window, cx);
            }
            "settings-nav-tools" => {
                window.focus(&self.settings_nav_tools_focus);
                self.on_select_settings_page(SettingsPage::Tools, window, cx);
            }
            "settings-nav-terminal" => {
                window.focus(&self.settings_nav_terminal_focus);
                self.on_select_settings_page(SettingsPage::Terminal, window, cx);
            }
            "settings-nav-appearance" => {
                window.focus(&self.settings_nav_appearance_focus);
                self.on_select_settings_page(SettingsPage::Appearance, window, cx);
            }
            "settings-nav-advanced" => {
                window.focus(&self.settings_nav_advanced_focus);
                self.on_select_settings_page(SettingsPage::Advanced, window, cx);
            }
            "settings-nav-about" => {
                window.focus(&self.settings_nav_about_focus);
                self.on_select_settings_page(SettingsPage::About, window, cx);
            }
            "settings-proxy-save" => self.on_settings_proxy_save(cx),
            "settings-proxy-clear" => self.on_settings_proxy_clear(cx),
            "settings-terminal-save" => self.on_settings_terminal_save(cx),
            "settings-terminal-clear" => self.on_settings_terminal_clear(cx),
            other if other.starts_with("settings-text-scale-") => {
                let Some(scale) = settings_text_scale_from_identifier(other) else {
                    return false;
                };
                self.on_settings_text_scale(scale, window, cx);
            }
            // SET-6b：五档选择与可见按钮同源派发（入口复核 gate）；未知
            // wire 串（含静态行 id）fail-closed。
            other if other.starts_with("settings-approval-mode-") => {
                let wire = other
                    .strip_prefix("settings-approval-mode-")
                    .unwrap_or_default();
                match wire.parse::<ApprovalModeWire>() {
                    Ok(mode) => self.on_settings_approval_mode(mode, cx),
                    Err(_) => return false,
                }
            }
            "settings-workspace-trust" => {
                let trusted = !self.projection.settings_permissions.workspace_trusted;
                self.on_settings_workspace_trust(trusted, cx);
            }
            "model-picker" => {
                window.focus(&self.model_focus);
                self.on_toggle_model_menu(None, window, cx)
            }
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
            other if other.starts_with("tool-group-toggle-") => {
                let Some(group_key) = self
                    .projection
                    .timeline_rows()
                    .iter()
                    .filter_map(|row| match row {
                        TimelineRow::ToolGroup { entry_indices }
                        | TimelineRow::RunSummary {
                            group: Some(entry_indices),
                            ..
                        } => timeline::tool_group_key(entry_indices, &self.projection.timeline),
                        _ => None,
                    })
                    .find(|key| tool_group_toggle_identifier(key) == other)
                    .map(str::to_string)
                else {
                    return false;
                };
                self.toggle_tool_group(&group_key, cx);
            }
            // Inspector 折叠态触发器的可见语义是弹出 ActivityPopover（R6
            // Wave A 起位于 Workspace Header），摘要行才展开 Inspector；
            // 展开态由 inspector-collapse 收起。
            "inspector-toggle" => {
                window.focus(&self.inspector_activity_focus);
                self.toggle_menu(MenuKind::Activity, None, cx)
            }
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
            "terminal-close" => self.on_close_terminal(window, cx),
            "activity-open-changes" => self.on_activity_open_changes(window, cx),
            _ => {
                // SET-4/5：settings 写动作与「设为默认」均与可见按钮同源
                // 派发（入口复核 gate；permits 已按当前树核对 disabled）。
                if identifier.starts_with(SETTINGS_CONTROL_PREFIX) {
                    match parse_settings_control(identifier) {
                        Some(SettingsControl::Action(action, escaped)) => {
                            if let Some(provider_id) =
                                self.settings_provider_id_for_escaped(&escaped)
                            {
                                self.on_settings_action(action, provider_id, cx);
                                return true;
                            }
                        }
                        Some(SettingsControl::SetDefaultModel(escaped)) => {
                            if let Some((provider_id, model_id)) =
                                self.settings_default_target_for_escaped(&escaped)
                            {
                                self.on_settings_set_default(provider_id, model_id, cx);
                                return true;
                            }
                        }
                        _ => {}
                    }
                    return false;
                }
                if identifier.starts_with(SETTINGS_MCP_CONTROL_PREFIX) {
                    if let Some((action, escaped)) = parse_settings_mcp_control(identifier) {
                        if let Some(name) = self.settings_mcp_server_for_escaped(&escaped) {
                            self.on_settings_mcp_action(action, name, cx);
                            return true;
                        }
                    }
                    return false;
                }
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
                    // D1：条目「···」触发器同样与点击同源移焦（句柄尚缺时
                    // 保持当前焦点，不伪造）。
                    if let Some(focus) = self.timeline_entry_action_focus.get(&entry.event_id) {
                        window.focus(focus);
                    }
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

        // SET-3 顶层路由：Settings 壳与工作台互斥（与 render 同源）。
        let tree = if self.route == AppRoute::Settings {
            AxTree::new(width, height)
                .child(
                    self.settings_rail_ax(
                        window,
                        AxRect::new(0.0, 0.0, sidebar_width, content_height),
                    ),
                )
                .child(self.settings_page_ax(
                    window,
                    cx,
                    AxRect::new(
                        workspace_x,
                        0.0,
                        (width - sidebar_width).max(0.0),
                        content_height,
                    ),
                ))
        } else {
            let mut tree = AxTree::new(width, height)
                .child(
                    self.sidebar_ax(window, AxRect::new(0.0, 0.0, sidebar_width, content_height)),
                )
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
            tree
        };
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
            self.grouping.toggle_action_label(),
            AxRect::new(
                (frame.width - inset - metrics::RAIL_ICON_BUTTON_SIZE).max(inset),
                // 标题行高 36、按钮 28：render items_center → 按钮顶 +4。
                y + (metrics::RAIL_TITLE_ROW_HEIGHT - metrics::RAIL_ICON_BUTTON_SIZE) / 2.0,
                metrics::RAIL_ICON_BUTTON_SIZE,
                metrics::RAIL_ICON_BUTTON_SIZE,
            ),
        )
        .value(self.grouping.view_label())
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

        // TR-12 页脚（SET-3）：右下角 Settings gear，与可见按钮同 gate
        //（render：content px(RAIL_INNER_PAD) + Panel p_2 → 右缘 inset 20，
        // mt_auto 钉底）。
        sidebar = sidebar.child(
            AxNode::new(
                "open-settings",
                AxRole::Button,
                "Settings",
                AxRect::new(
                    (frame.width - inset - metrics::RAIL_ICON_BUTTON_SIZE).max(inset),
                    (frame.height - PAD - metrics::RAIL_ICON_BUTTON_SIZE).max(list_top),
                    metrics::RAIL_ICON_BUTTON_SIZE,
                    metrics::RAIL_ICON_BUTTON_SIZE,
                ),
            )
            .focused(self.open_menu.is_none() && self.settings_focus.is_focused(window))
            .action(AxAction::Press),
        );

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

    /// Settings 左栏（SET-3）：返回按钮 + 首页导航项。几何与

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
        } else if self.projection.workspace_empty_hint_visible() {
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
        // P4 片 3：与 timeline.rs render 同源——列 = pl(TIMELINE_CONTENT_INSET)
        // + 可读列宽；行高 / 行间距按内容公式化推导（gpui list 实际按像素
        // 布局，此为同源公式）。滚动位置沿用已验证安全的 logical_scroll_top()
        // 只读，不触碰写借用。
        let rem_px = f32::from(window.rem_size());
        let column_x = frame.x + metrics::TIMELINE_CONTENT_INSET;
        let column_width = (frame.width - metrics::TIMELINE_CONTENT_INSET)
            .min(metrics::TIMELINE_READABLE_WIDTH)
            .max(0.0);
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
                        &self.collapsed_tool_groups,
                        self.changes_available_for_active(),
                    ),
                )
            })
            .collect();
        let approval_height = self.projection.pending_approval.as_ref().map(|pending| {
            approval_card_height(
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
            timeline::timeline_following_window(&item_layouts, viewport_height)
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
                // render 的 mt(gap) 属于 item，故内容 rect 再内缩 gap。
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
        if empty_hint_visible {
            // P0-3：与 timeline_area 同源的 title / description / Primary
            // action。Header 同态不发布重复 New task 节点，保证 identifier
            // 唯一；disabled 时不发布 Press action。
            let group_height = 112.0_f32.min(frame.height);
            let group_y = frame.y + ((frame.height - group_height) / 2.0).max(0.0);
            let content_x = frame.x + metrics::TIMELINE_CONTENT_INSET;
            let content_width = (frame.width - metrics::TIMELINE_CONTENT_INSET * 2.0).max(0.0);
            let button_width = 112.0_f32.min(content_width);
            let button_x = content_x + ((content_width - button_width) / 2.0).max(0.0);
            let can_create = self.can_create_task();
            let mut new_task = AxNode::new(
                "header-new-task",
                AxRole::Button,
                "New task",
                AxRect::new(button_x, group_y + 76.0, button_width, 36.0),
            )
            .description(self.add_task_disabled_reason())
            .enabled(can_create)
            .focused(self.open_menu.is_none() && self.header_new_task_focus.is_focused(window));
            if can_create {
                new_task = new_task.action(AxAction::Press);
            }
            list = list
                .child(AxNode::new(
                    "workspace-empty-title",
                    AxRole::StaticText,
                    WORKSPACE_EMPTY_TITLE,
                    AxRect::new(content_x, group_y, content_width, 28.0),
                ))
                .child(AxNode::new(
                    "workspace-empty-hint",
                    AxRole::StaticText,
                    WORKSPACE_EMPTY_HINT,
                    AxRect::new(content_x, group_y + 36.0, content_width, 24.0),
                ))
                .child(new_task);
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
            // P4 片 3：卡高与 approval_card.rs 布局同源（标题 / reason 行
            // 数 + 可选 detail + p_2 + 32px 按钮行）。P4 片 2F（D2）及
            // review：卡作为真实 list 末项参与 start/offset/可见性计算；
            // 滚离底部时不发布不可见审批动作，跟随溢出时首项可部分可见。
            let approval = AxRect::new(column_x, approval_top, column_width, approval_height);
            let button_row_y = approval_button_row_y(approval, rem_px);
            let button_gap = APPROVAL_BUTTON_ROW_GAP_REMS * rem_px;
            let enabled = self.can_approve();
            let focused = [
                self.open_menu.is_none() && self.approve_once_focus.is_focused(window),
                self.open_menu.is_none() && self.approve_for_run_focus.is_focused(window),
                self.open_menu.is_none() && self.deny_focus.is_focused(window),
            ];
            let mut approval_node =
                AxNode::new("approval-card", AxRole::Group, "Approval", approval)
                    .value(format!("{} · {}", pending.tool_name, pending.reason));
            let mut button_x = approval.x;
            for (ix, (id, label)) in [
                ("approve-once", "Allow once"),
                ("approve-for-run", "Allow for run"),
                ("approve-deny", "Deny"),
            ]
            .into_iter()
            .enumerate()
            {
                let width = APPROVAL_BUTTON_SLOT_WIDTHS[ix];
                approval_node = approval_node.child({
                    let mut button = AxNode::new(
                        id,
                        AxRole::Button,
                        label,
                        AxRect::new(button_x, button_row_y, width, APPROVAL_BUTTON_HEIGHT),
                    )
                    .enabled(enabled)
                    .focused(focused[ix]);
                    if enabled {
                        button = button.action(AxAction::Press);
                    }
                    button
                });
                button_x += width + button_gap;
            }
            list = list.child(approval_node);
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
                let group_key = timeline::tool_group_key(entry_indices, &self.projection.timeline)
                    .unwrap_or_default();
                let rows = self.tool_row_views(entry_indices);
                let collapsed = self.collapsed_tool_groups.contains(group_key);
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
                                .timeline_tool_group_focus
                                .get(group_key)
                                .is_some_and(|focus| focus.is_focused(window)),
                    )
                    .action(AxAction::Press),
                );
                if collapsed {
                    return group;
                }
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
                        rect.y
                            + metrics::TOOL_GROUP_HEADER_HEIGHT
                            + ix as f32 * metrics::TOOL_ROW_HEIGHT,
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
                    let group_key =
                        timeline::tool_group_key(entry_indices, &self.projection.timeline)
                            .unwrap_or_default();
                    let rows = self.tool_row_views(entry_indices);
                    let collapsed = self.collapsed_tool_groups.contains(group_key);
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
                                    .timeline_tool_group_focus
                                    .get(group_key)
                                    .is_some_and(|focus| focus.is_focused(window)),
                        )
                        .action(AxAction::Press),
                    );
                    y += metrics::TOOL_GROUP_HEADER_HEIGHT;
                    if !collapsed {
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
                    }
                    y += metrics::SUMMARY_CARD_GAP;
                }
                let review_enabled = terminal_entry.fork_boundary == Some(ForkBoundary::Completed)
                    && self.changes_available_for_active();
                let (title, description) = run_summary_texts(terminal_entry, review_enabled)
                    .unwrap_or(("Run", String::new()));
                region = region.child(
                    AxNode::new(
                        dynamic_identifier("run-summary-card", &terminal_entry.event_id),
                        AxRole::StaticText,
                        title,
                        AxRect::new(rect.x, y, rect.width, metrics::SUMMARY_CHECK_CIRCLE),
                    )
                    .description(description),
                );
                if review_enabled {
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
            let entries = grouped_model_menu_entries(&self.projection.models);
            let selected_ix = self
                .projection
                .effective_model()
                .and_then(|(provider, id)| {
                    entries
                        .iter()
                        .position(|model| model.provider_id == *provider && model.id == *id)
                })
                .unwrap_or(0);
            let highlight = self.menu_highlight_effective(selected_ix);
            let groups = crate::projection::group_models_by_provider(&self.projection.models);
            let content_height = metrics::MENU_PADDING * 2.0
                + groups.len() as f32 * MODEL_MENU_GROUP_HEADER_HEIGHT
                + entries.len() as f32 * metrics::MENU_ROW_HEIGHT;
            let menu_height = content_height.min(MENU_MAX_HEIGHT);
            let menu_x = frame.x + pad;
            let menu_y = (footer_y - ANCHOR_GAP_Y - menu_height).max(0.0);
            let mut menu = AxNode::new(
                "model-menu",
                AxRole::Group,
                "Models",
                AxRect::new(menu_x, menu_y, 260.0, menu_height),
            );
            let mut y = menu_y + metrics::MENU_PADDING;
            let mut item_ix = 0;
            // render 面板在 MENU_MAX_HEIGHT 内自滚且初始停在顶部；AX 只发布
            // 与首帧可见窗口相交的子节点，不把裁剪区外的行塞进树（滚动后
            // 的 AX 窗口跟随是后续候选）。
            let menu_bottom = menu_y + menu_height;
            for (provider_id, models) in groups {
                if y < menu_bottom {
                    menu = menu.child(AxNode::new(
                        dynamic_identifier("model-provider", &provider_id),
                        AxRole::StaticText,
                        provider_id,
                        AxRect::new(menu_x, y, 260.0, MODEL_MENU_GROUP_HEADER_HEIGHT),
                    ));
                }
                y += MODEL_MENU_GROUP_HEADER_HEIGHT;
                for model in models {
                    let selected = self.projection.effective_model().is_some_and(|current| {
                        current.0 == model.provider_id && current.1 == model.id
                    });
                    let can_switch = self.can_switch_model();
                    if y < menu_bottom {
                        let mut item = AxNode::new(
                            model_identifier(&model),
                            AxRole::Button,
                            model.display_name.clone(),
                            AxRect::new(menu_x, y, 260.0, metrics::MENU_ROW_HEIGHT),
                        )
                        .value(format!("{} / {}", model.provider_id, model.id))
                        .enabled(can_switch)
                        .selected(selected)
                        .focused(item_ix == highlight);
                        if can_switch {
                            item = item.action(AxAction::Press);
                        }
                        menu = menu.child(item);
                    }
                    item_ix += 1;
                    y += metrics::MENU_ROW_HEIGHT;
                }
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
        let rem_px = f32::from(window.rem_size());
        let header_height = terminal_header_height(rem_px);
        // P4 片 3：五按钮 rect 与 inspector.rs 可见 stepper 行同源
        //（terminal_stepper_ax_rects：px_2 / py_1 / gap_1 + 冻结槽位）。
        let stepper = terminal_stepper_ax_rects(frame.x, frame.x + frame.width, frame.y, rem_px);
        let stepper_rect =
            |ix: usize| AxRect::new(stepper[ix].0, stepper[ix].1, stepper[ix].2, stepper[ix].3);
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
        // 与可见按钮（inspector.rs）同 gate：running → Stop，已知
        // exited/killed/failed → Close，其余状态不发布节点。
        let terminal_close_label =
            terminal_close_label(&self.projection.connection, &self.projection.terminal).map(
                |label| match label {
                    "Stop" => "Stop terminal",
                    _ => "Close terminal",
                },
            );
        let mut terminal = AxNode::new("terminal", AxRole::Group, "Terminal", frame)
            // G1：头部尺寸组 = 列 stepper 对 + apply + 行 stepper 对，与可见
            // 控件同 gate / 同 id；apply 仍是唯一下发入口。
            .child(
                AxNode::new(
                    "terminal-cols-dec",
                    AxRole::Button,
                    "Fewer terminal columns",
                    stepper_rect(0),
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
                    stepper_rect(1),
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
                    stepper_rect(2),
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
                    stepper_rect(3),
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
                    stepper_rect(4),
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
                        frame.y + header_height,
                        frame.width - PAD * 2.0,
                        frame.height - input_height - header_height - PAD * 2.0,
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
                        if terminal_can_reopen(&self.projection.terminal) {
                            "Start new terminal"
                        } else if terminal_known_ended(&self.projection.terminal) {
                            "Start terminal"
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
        if let Some(close_label) = terminal_close_label {
            terminal = terminal.child(
                AxNode::new(
                    "terminal-close",
                    AxRole::Button,
                    close_label,
                    AxRect::new(
                        frame.x + frame.width - PAD * 2.0 - button_width * 2.0,
                        input_y,
                        button_width,
                        input_height,
                    ),
                )
                .focused(self.open_menu.is_none() && self.terminal_close_focus.is_focused(window))
                .enabled(self.terminal_pending_close.is_none())
                .action(AxAction::Press),
            );
        }
        // 与可见回到底部按钮（inspector.rs）一致：仅在滚动脱钩时发布。
        if !self.terminal_scroll.is_following() {
            terminal = terminal.child(
                AxNode::new(
                    "terminal-back-to-bottom",
                    AxRole::Button,
                    "Back to bottom",
                    AxRect::new(
                        frame.x + frame.width - 140.0,
                        (input_y - 40.0).max(frame.y + header_height),
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

pub(crate) fn dynamic_identifier(prefix: &str, raw: &str) -> String {
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

fn tool_group_toggle_identifier(event_id: &str) -> String {
    dynamic_identifier("tool-group-toggle", event_id)
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
    /// 必须钉住生产 render 所用的 40×37 槽、右侧 25px inset、8px gap 与
    /// 320×144 内容收缩外框；Connected 真窗口证据受环境阻塞时仍能防止静默漂移。
    #[test]
    fn activity_header_ax_geometry_matches_render_anchor_contract() {
        let header = AxRect::new(240.0, 0.0, 840.0, metrics::HEADER_HEIGHT);
        let trigger = header_action_ax_rect(header);
        assert_eq!(trigger, AxRect::new(1015.0, 51.5, 40.0, 37.0));

        let popover = activity_popover_ax_geometry(header, trigger);
        assert_eq!(popover.frame, AxRect::new(735.0, 96.5, 320.0, 144.0));
        assert_eq!(popover.heading, AxRect::new(763.0, 154.5, 264.0, 20.0));
        assert_eq!(
            popover.open_changes,
            AxRect::new(763.0, 178.5, 264.0, ROW_HEIGHT)
        );
    }

    /// P4 片 3：Terminal stepper 五按钮几何与 render 同源公式一致——右缘
    /// px_2 对齐、gap_1 间距、冻结 28/28/72/28/28 槽位且互不重叠，间距与
    /// 内边距随字号档（rem）缩放而槽位 px 不变。
    #[test]
    fn terminal_stepper_ax_geometry_matches_shared_formula() {
        let rects = crate::ui::inspector::terminal_stepper_ax_rects(100.0, 540.0, 200.0, 16.0);
        // 100%：px_2=8 / py_1=4 / gap_1=4。
        assert_eq!(rects[4], (540.0 - 8.0 - 28.0, 204.0, 28.0, 28.0));
        assert_eq!(rects[3].0, rects[4].0 - 4.0 - 28.0);
        assert_eq!(rects[2].2, 72.0);
        assert_eq!(rects[2].0, rects[3].0 - 4.0 - 72.0);
        assert_eq!(rects[1].0, rects[2].0 - 4.0 - 28.0);
        assert_eq!(rects[0].0, rects[1].0 - 4.0 - 28.0);
        for pair in rects.windows(2) {
            assert!(pair[0].0 + pair[0].2 <= pair[1].0);
        }
        // 125%：gap / 内边距随 rem 缩放（5 / 10），槽位 px 不变。
        let scaled = crate::ui::inspector::terminal_stepper_ax_rects(100.0, 540.0, 200.0, 20.0);
        assert_eq!(scaled[4].0, 540.0 - 10.0 - 28.0);
        assert_eq!(scaled[3].0, scaled[4].0 - 5.0 - 28.0);
        assert_eq!(scaled[4].1, 205.0);
    }

    /// P4 片 3：审批卡高度随 reason / detail 行数变化（公式与
    /// approval_card.rs render 同源），单行 reason 的 100% 值逐项可推导。
    #[test]
    fn approval_card_ax_height_scales_with_reason_lines() {
        let short = approval_card_height("Use bash", None, 618.0, 16.0);
        let wrapped = approval_card_height(&"x".repeat(160), None, 618.0, 16.0);
        let with_detail =
            approval_card_height(&"x".repeat(160), Some(&"y".repeat(160)), 618.0, 16.0);
        assert!(short < wrapped && wrapped < with_detail);
        // pad 16 + 标题 19 + reason 19 + 按钮行 32。
        assert_eq!(short, 86.0);
        // 125%：p_2=10、SM=15px（行高 24）→ 20 + 24 + 24 + 32。
        assert_eq!(approval_card_height("Use bash", None, 618.0, 20.0), 100.0);
    }

    /// P4 片 3：Timeline 行 rect 相邻不重叠、行间距与 row_top_gap 一致，
    /// 行高按内容公式化（tool 组 = 44 标题 + 行数×52；消息 = 标签 + 12 + 正文）。
    #[test]
    fn timeline_row_layouts_stack_with_content_heights_and_gaps() {
        use crate::projection::{TimelineEntry, TimelineEntryKind, TimelineRow};
        use crate::ui::timeline::{
            row_top_gap, timeline_following_window, timeline_row_height, timeline_visible_item_tops,
        };

        fn entry(seq: u64, kind: TimelineEntryKind) -> TimelineEntry {
            TimelineEntry {
                sequence: seq,
                event_id: format!("e{seq}"),
                kind,
                fork_boundary: None,
                timestamp: "1800000000000".into(),
                run_id: None,
            }
        }
        let timeline = vec![
            entry(
                1,
                TimelineEntryKind::UserMessage {
                    text: "Plan:".into(),
                },
            ),
            entry(
                2,
                TimelineEntryKind::ToolCall {
                    name: "read".into(),
                    status: "succeeded".into(),
                    detail: None,
                },
            ),
            entry(
                3,
                TimelineEntryKind::ToolCall {
                    name: "bash".into(),
                    status: "running".into(),
                    detail: None,
                },
            ),
            entry(4, TimelineEntryKind::RunState("Running".into())),
        ];
        let rows = vec![
            TimelineRow::Message { entry_index: 0 },
            TimelineRow::ToolGroup {
                entry_indices: vec![1, 2],
            },
            TimelineRow::RunPhase { entry_index: 3 },
        ];
        let layouts: Vec<(f32, f32)> = rows
            .iter()
            .map(|row| {
                (
                    row_top_gap(row),
                    timeline_row_height(
                        row,
                        &timeline,
                        618.0,
                        16.0,
                        &std::collections::HashSet::new(),
                        false,
                    ),
                )
            })
            .collect();
        // 100%：消息 = 标签 26 + 12 + 正文 24；tool 组 = 44 + 2×52；相位 = 19。
        assert_eq!(layouts[0].1, 62.0);
        assert_eq!(layouts[1].1, 148.0);
        assert_eq!(layouts[2].1, 19.0);
        let mut collapsed = std::collections::HashSet::new();
        collapsed.insert("e2".to_string());
        assert_eq!(
            timeline_row_height(&rows[1], &timeline, 618.0, 16.0, &collapsed, false),
            metrics::TOOL_GROUP_HEADER_HEIGHT
        );
        // 行间距与 row_top_gap 同源：消息→tool 组 48，tool 组→相位 40。
        assert_eq!(layouts[1].0, metrics::TOOL_GROUP_TOP_GAP);
        assert_eq!(layouts[2].0, metrics::MSG_ENTRY_GAP);
        let tops = timeline_visible_item_tops(&layouts, 100.0, 400.0, 0, 0.0);
        assert_eq!(tops[0], (0, 100.0));
        assert_eq!(tops[1], (1, 100.0 + 62.0 + 48.0));
        assert_eq!(tops[2], (2, tops[1].1 + 148.0 + 40.0));
        for pair in tops.windows(2) {
            assert!(pair[0].1 + layouts[pair[0].0].1 <= pair[1].1);
        }
        // 125%：消息行高随字号档缩放（标签 32 + 12 + 正文 30）。
        assert_eq!(
            timeline_row_height(
                &rows[0],
                &timeline,
                618.0,
                20.0,
                &std::collections::HashSet::new(),
                false,
            ),
            74.0
        );
        // 跟随态窗口：视口装不下全部（62+48+148+40+19=317>200）时，
        // 首个部分可见项仍保留，item 1 内偏移 55；全部装得下则从 0 开始。
        assert_eq!(timeline_following_window(&layouts, 200.0), (1, 55.0));
        assert_eq!(timeline_following_window(&layouts, 400.0), (0, 0.0));
        let tail = timeline_visible_item_tops(&layouts, 100.0, 200.0, 1, 55.0);
        assert_eq!(tail[0], (1, 93.0));
        assert_eq!(tail[1], (2, 281.0));
    }

    /// P4 片 2F（D2）：审批卡 AX 位置按内容流推导（渲染为 timeline list
    /// 末项）——短内容卡顶 = 内容顶 + Σ行 + MSG_ENTRY_GAP，不贴视口底；
    /// 内容+卡超出视口（跟随态滚到底）才贴底；按钮行随卡底同步移动。
    #[test]
    fn approval_card_ax_position_follows_content_flow_until_overflow() {
        use crate::ui::timeline::{timeline_following_window, timeline_visible_item_tops};

        let frame = AxRect::new(288.0, 56.0, 712.0, 800.0);
        let content_top = frame.y + metrics::TIMELINE_TOP_GAP;
        let viewport_height = frame.height - metrics::TIMELINE_TOP_GAP;
        let card_height = approval_card_height("Use bash", None, 618.0, 16.0);
        // 短内容：approval 与 rows 组成同一 item 序列，全量远小于视口。
        let short = vec![
            (0.0f32, 62.0f32),
            (metrics::MSG_ENTRY_GAP, 104.0),
            (metrics::MSG_ENTRY_GAP, card_height),
        ];
        let flow_top = content_top + 62.0 + metrics::MSG_ENTRY_GAP + 104.0 + metrics::MSG_ENTRY_GAP;
        let short_window = timeline_following_window(&short, viewport_height);
        assert_eq!(short_window, (0, 0.0));
        let short_tops = timeline_visible_item_tops(
            &short,
            content_top,
            viewport_height,
            short_window.0,
            short_window.1,
        );
        let short_y = short_tops[2].1;
        assert_eq!(short_y, flow_top);
        assert!(short_y + card_height <= frame.y + frame.height);
        // 空时间线：卡即首项，无上间距。
        let empty = vec![(0.0, card_height)];
        assert_eq!(
            timeline_visible_item_tops(&empty, content_top, viewport_height, 0, 0.0)[0].1,
            content_top
        );
        // 长内容：跟随态保留首个部分可见项，approval 末项贴视口底。
        let tall = vec![
            (0.0f32, 700.0f32),
            (metrics::MSG_ENTRY_GAP, 300.0),
            (metrics::MSG_ENTRY_GAP, card_height),
        ];
        let tall_window = timeline_following_window(&tall, viewport_height);
        let tall_tops = timeline_visible_item_tops(
            &tall,
            content_top,
            viewport_height,
            tall_window.0,
            tall_window.1,
        );
        let pinned_y = frame.y + frame.height - card_height;
        assert_eq!(tall_tops.last(), Some(&(2, pinned_y)));
        // 脱钩读史时 approval 在视口外，不得发布不可见审批动作。
        let detached = timeline_visible_item_tops(&tall, content_top, viewport_height, 0, 0.0);
        assert!(detached.iter().all(|(ix, _)| *ix != 2));
        // 按钮行 y = 卡底 − p_2 − 32：两臂均自卡底推导（随卡移动）。
        assert_eq!(
            approval_button_row_y(AxRect::new(288.0, short_y, 618.0, card_height), 16.0),
            short_y + card_height - APPROVAL_CARD_PAD_REMS * 16.0 - APPROVAL_BUTTON_HEIGHT
        );
        assert_eq!(
            approval_button_row_y(AxRect::new(288.0, pinned_y, 618.0, card_height), 16.0),
            pinned_y + card_height - APPROVAL_CARD_PAD_REMS * 16.0 - APPROVAL_BUTTON_HEIGHT
        );
    }

    /// P0-2：grouping AXPress 直接双向切换且不生成菜单，并保留 session / scope /
    /// draft / collapsed projects；其余菜单触发器仍先移焦，Escape-style 关闭后
    /// 回到来源。AppView 不作窗口根渲染，仅驱动 AX dispatch 本身。
    #[gpui::test]
    fn ax_press_direct_grouping_and_menu_triggers_keep_focus_contract(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;

        struct AxPressHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxPressHost {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div()
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("p4-2f-ax-press.sock");
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            AxPressHost { view }
        });
        let view = cx.update(|_window, cx| host.read(cx).view.clone());

        cx.update(|window, cx| {
            view.update(cx, |view, _cx| {
                view.projection.active_session_id = Some("s-keep".into());
                view.scope_workspace_id = Some("ws-keep".into());
                view.composer_drafts.insert("s-keep".into(), "draft".into());
                view.collapsed_projects.insert("ws-keep".into());
                // 直接切换也必须关闭此前打开的其它浮层与高亮。
                view.open_menu = Some(MenuKind::Scope);
                view.menu_highlight = Some(0);
            });
            let composer = view.read(cx).composer_focus_handle(cx);
            window.focus(&composer);
        });
        for expected in [TaskRailGrouping::Projects, TaskRailGrouping::Timeline] {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.handle_accessibility_request(
                        AxRequest {
                            identifier: "task-rail-grouping".into(),
                            action: AxAction::Press,
                            value: None,
                        },
                        window,
                        cx,
                    );
                });
            });
            cx.update(|window, cx| {
                let view = view.read(cx);
                assert_eq!(view.grouping, expected);
                assert!(view.open_menu.is_none());
                assert!(view.menu_highlight.is_none());
                assert!(view.grouping_focus.is_focused(window));
                assert_eq!(view.projection.active_session_id.as_deref(), Some("s-keep"));
                assert_eq!(view.scope_workspace_id.as_deref(), Some("ws-keep"));
                assert_eq!(
                    view.composer_drafts.get("s-keep").map(String::as_str),
                    Some("draft")
                );
                assert!(view.collapsed_projects.contains("ws-keep"));
                assert!(view.rail_scroll_to_active);
            });
        }

        // 恢复 All projects，让两个 New Task 入口走 WorkspaceConfirm 菜单路径。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.scope_workspace_id = None;
                view.projection.active_session_id = None;
            });
        });
        for identifier in ["project-scope", "add-task", "header-new-task"] {
            cx.update(|window, cx| {
                // add-task / header-new-task 经 can_create_task 门控：注入
                // Connected 使 AX press 不被 fail-closed 拒绝（真实点击同理）。
                view.update(cx, |view, _cx| {
                    view.projection.set_connection(ConnectionState::Connected {
                        instance_id: "test".into(),
                    });
                });
                let composer = view.read(cx).composer_focus_handle(cx);
                window.focus(&composer);
            });
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.handle_accessibility_request(
                        AxRequest {
                            identifier: identifier.to_string(),
                            action: AxAction::Press,
                            value: None,
                        },
                        window,
                        cx,
                    );
                });
            });
            cx.update(|window, cx| {
                let view = view.read(cx);
                assert!(
                    !view.composer_focus_handle(cx).is_focused(window),
                    "{identifier}: AX press must move focus off the composer"
                );
                let trigger_focused = match identifier {
                    "project-scope" => view.scope_focus.is_focused(window),
                    "add-task" => view.add_task_focus.is_focused(window),
                    _ => view.header_new_task_focus.is_focused(window),
                };
                assert!(
                    trigger_focused,
                    "{identifier}: AX press must focus the trigger like a click"
                );
            });
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let kind = view.open_menu.clone().expect("AX press should open a menu");
                    view.close_menu_and_focus_trigger(kind, window, cx);
                });
            });
            cx.update(|window, cx| {
                let view = view.read(cx);
                let trigger_focused = match identifier {
                    "project-scope" => view.scope_focus.is_focused(window),
                    "add-task" => view.add_task_focus.is_focused(window),
                    _ => view.header_new_task_focus.is_focused(window),
                };
                assert!(
                    trigger_focused,
                    "{identifier}: Escape-style close must restore the originating trigger"
                );
            });
        }
    }

    /// SET-6e/6f/6g：外观与高级页都是 Desktop 本地能力，离线也必须可达；
    /// About 仅在当前握手携带非空 Host 路径时出现，丢失后退回高级页；
    /// 高级页不伪装旧握手，外观字号 AX Press 与可见 / 键盘路径同源。
    #[gpui::test]
    fn settings_local_pages_ax_are_available_offline_and_update_state(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;

        struct AxAppearanceHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxAppearanceHost {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div()
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("set6f-ax-local-pages.sock");
        let endpoint = socket.display().to_string();
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            AxAppearanceHost { view }
        });
        let view = cx.update(|_window, cx| host.read(cx).view.clone());
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.route = AppRoute::Settings;
                view.projection.set_connection(ConnectionState::Failed {
                    reason: "host unavailable".into(),
                });
            });
        });

        cx.update(|window, cx| {
            let tree = view.read(cx).accessibility_tree(window, cx);
            tree.validate().expect("offline Settings AX tree validates");
            assert!(tree.find("settings-nav-appearance").is_some());
            assert!(tree.find("settings-nav-advanced").is_some());
            assert!(tree.find("settings-nav-about").is_none());
            assert!(!tree.permits(&AxRequest {
                identifier: "settings-nav-about".into(),
                action: AxAction::Press,
                value: None,
            }));
            assert!(tree.permits(&AxRequest {
                identifier: "settings-nav-advanced".into(),
                action: AxAction::Press,
                value: None,
            }));
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: "settings-nav-advanced".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.settings_page, SettingsPage::Advanced);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("advanced page AX tree validates");
            assert_eq!(
                tree.find("settings-advanced-connection")
                    .and_then(|node| node.value.as_deref()),
                Some("Connect failed · host unavailable")
            );
            assert_eq!(
                tree.find("settings-advanced-runtime")
                    .and_then(|node| node.value.as_deref()),
                Some("Unavailable · connect to the Host")
            );
            assert_eq!(
                tree.find("settings-advanced-endpoint")
                    .and_then(|node| node.value.as_deref()),
                Some(endpoint.as_str())
            );
            assert!(tree.find("reconnect").is_some());
            assert!(tree.permits(&AxRequest {
                identifier: "reconnect".into(),
                action: AxAction::Press,
                value: None,
            }));
        });
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "runtime-6f".into(),
                });
                view.projection.resume = crate::projection::ResumeState::UpToDate {
                    current_sequence: 42,
                };
                view.handshake_info = Some(crate::controller::DesktopHandshakeInfo {
                    runtime_id: "runtime-6f".into(),
                    api_version: "1.9".into(),
                    capabilities: vec!["events".into(), "snapshots".into()],
                    host_data_dir: None,
                });
            });
        });
        cx.update(|window, cx| {
            let tree = view.read(cx).accessibility_tree(window, cx);
            tree.validate()
                .expect("connected advanced page AX tree validates");
            assert_eq!(
                tree.find("settings-advanced-connection")
                    .and_then(|node| node.value.as_deref()),
                Some("Connected")
            );
            assert_eq!(
                tree.find("settings-advanced-runtime")
                    .and_then(|node| node.value.as_deref()),
                Some("runtime-6f")
            );
            assert_eq!(
                tree.find("settings-advanced-api")
                    .and_then(|node| node.value.as_deref()),
                Some("1.9")
            );
            assert_eq!(
                tree.find("settings-advanced-capabilities")
                    .and_then(|node| node.value.as_deref()),
                Some("events, snapshots")
            );
            assert_eq!(
                tree.find("settings-advanced-resume")
                    .and_then(|node| node.value.as_deref()),
                Some("Up to date · 42")
            );
            assert_eq!(
                tree.find("settings-advanced-last-ack")
                    .and_then(|node| node.value.as_deref()),
                Some("Unavailable")
            );
            assert!(tree.find("reconnect").is_none());
            assert!(tree.find("settings-nav-about").is_none());
            assert!(tree.permits(&AxRequest {
                identifier: "settings-nav-appearance".into(),
                action: AxAction::Press,
                value: None,
            }));
        });
        // 只有当前握手给出非空权威路径时才发布 About；页面三行与
        // render 共用数据源，不从 endpoint 或本机默认目录推断。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.handshake_info
                    .as_mut()
                    .expect("connected handshake exists")
                    .host_data_dir = Some(" /tmp/pawork-set6g ".into());
            });
        });
        cx.update(|window, cx| {
            let tree = view.read(cx).accessibility_tree(window, cx);
            tree.validate().expect("About navigation AX tree validates");
            assert!(tree.find("settings-nav-about").is_some());
            assert!(tree.permits(&AxRequest {
                identifier: "settings-nav-about".into(),
                action: AxAction::Press,
                value: None,
            }));
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: "settings-nav-about".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.settings_page, SettingsPage::About);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("About page AX tree validates");
            assert_eq!(
                tree.find("settings-about-desktop-build")
                    .and_then(|node| node.value.as_deref()),
                Some(env!("CARGO_PKG_VERSION"))
            );
            assert_eq!(
                tree.find("settings-about-api")
                    .and_then(|node| node.value.as_deref()),
                Some("1.9")
            );
            assert_eq!(
                tree.find("settings-about-data-dir")
                    .and_then(|node| node.value.as_deref()),
                Some(" /tmp/pawork-set6g ")
            );
        });
        // 最终业务入口也必须 fail-closed：即使迟到的旧 Reconnect 事件被
        // 派发，Connected 状态也不能启动第二条连接。
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.on_reconnect(window, cx));
        });
        cx.update(|_window, cx| {
            let view = view.read(cx);
            assert!(matches!(
                &view.projection.connection,
                ConnectionState::Connected { .. }
            ));
            assert!(view.handshake_info.is_some());
        });
        // 通过真实 ControllerEvent 消费路径证明旧握手在断线时清空，且
        // Advanced 页重新发布 Reconnect；不直接改测试状态绕过生命周期。
        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.handle_controller_event(
                    crate::controller::ControllerEvent::Disconnected {
                        reason: "connection closed".into(),
                    },
                    cx,
                );
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(view.handshake_info.is_none());
            assert_eq!(view.settings_page, SettingsPage::Advanced);
            let tree = view.accessibility_tree(window, cx);
            tree.validate()
                .expect("disconnected advanced page AX tree validates");
            assert_eq!(
                tree.find("settings-advanced-connection")
                    .and_then(|node| node.value.as_deref()),
                Some("Disconnected · connection closed")
            );
            assert_eq!(
                tree.find("settings-advanced-runtime")
                    .and_then(|node| node.value.as_deref()),
                Some("Unavailable · connect to the Host")
            );
            assert!(tree.find("settings-nav-about").is_none());
            assert!(tree.find("settings-about-data-dir").is_none());
            assert!(tree.find("reconnect").is_some());
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: "settings-nav-appearance".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.settings_page, SettingsPage::Appearance);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("appearance page AX tree validates");
            let scale_100 = tree.find("settings-text-scale-100").unwrap();
            let scale_125 = tree.find("settings-text-scale-125").unwrap();
            let scale_150 = tree.find("settings-text-scale-150").unwrap();
            assert!(scale_100.selected);
            assert!(!scale_125.selected);
            assert!(!scale_150.selected);
            for node in [scale_100, scale_125, scale_150] {
                assert_eq!(node.bounds.width, SETTINGS_APPEARANCE_CONTROL_WIDTH);
                assert_eq!(node.bounds.height, SETTINGS_APPEARANCE_CONTROL_HEIGHT);
            }
            assert_eq!(
                scale_125.bounds.x - scale_100.bounds.x,
                SETTINGS_APPEARANCE_CONTROL_WIDTH + SETTINGS_APPEARANCE_CONTROL_GAP
            );
            assert_eq!(
                scale_150.bounds.x - scale_125.bounds.x,
                SETTINGS_APPEARANCE_CONTROL_WIDTH + SETTINGS_APPEARANCE_CONTROL_GAP
            );
            assert!(!tree.permits(&AxRequest {
                identifier: "settings-text-scale-175".into(),
                action: AxAction::Press,
                value: None,
            }));
            assert!(tree.permits(&AxRequest {
                identifier: "settings-text-scale-150".into(),
                action: AxAction::Press,
                value: None,
            }));
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: "settings-text-scale-150".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.text_scale, font::TextScale::Percent150);
            assert_eq!(f32::from(window.rem_size()), 24.0);
            let tree = view.accessibility_tree(window, cx);
            assert!(!tree.find("settings-text-scale-100").unwrap().selected);
            assert!(tree.find("settings-text-scale-150").unwrap().selected);
        });
    }

    /// SET-4（SET-010）：Settings secure API key 输入的 AX value 只发布掩码，
    /// 全树不携带明文；断线 stale 后写动作按钮与输入 enabled=false 且
    /// permits 拒绝（可见 / 键盘 / AX 三路径同 gate）。
    #[gpui::test]
    fn settings_ax_masks_api_key_and_gates_writes_when_stale(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;

        struct AxSettingsHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxSettingsHost {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div()
            }
        }

        fn assert_no_secret(node: &AxNode, secret: &str) {
            assert!(!node.identifier.contains(secret));
            assert!(!node.label.contains(secret));
            if let Some(value) = &node.value {
                assert!(!value.contains(secret), "AX value leaked secret: {value}");
            }
            if let Some(description) = &node.description {
                assert!(!description.contains(secret));
            }
            for child in &node.children {
                assert_no_secret(child, secret);
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("set4-ax-settings.sock");
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            AxSettingsHost { view }
        });
        let view = cx.update(|_window, cx| host.read(cx).view.clone());
        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.route = AppRoute::Settings;
                view.projection.settings_providers.apply_loaded(
                    crate::projection::ProviderAuthStatusData {
                        providers: vec![
                            crate::projection::ProviderAuthStatusEntry {
                                provider_id: "kimi".into(),
                                display_name: "Kimi".into(),
                                endpoint_label: "https://api.moonshot.cn".into(),
                                auth_methods: vec!["api_key".into()],
                                auth: crate::projection::ProviderAuthState::None,
                                catalog: crate::projection::ProviderCatalogState::Unavailable {
                                    error: "offline".into(),
                                    fetched_at: None,
                                },
                            },
                            crate::projection::ProviderAuthStatusEntry {
                                provider_id: "connected".into(),
                                display_name: "Connected provider".into(),
                                endpoint_label: "https://provider.example".into(),
                                auth_methods: vec!["api_key".into()],
                                auth: crate::projection::ProviderAuthState::Connected {
                                    method: "api_key".into(),
                                    masked_credential: Some("masked-fragment-sentinel".into()),
                                },
                                catalog: crate::projection::ProviderCatalogState::FixedFallback {
                                    snapshot_label: "test@v1".into(),
                                    fetched_at: None,
                                },
                            },
                        ],
                        default: None,
                    },
                );
                view.ensure_settings_api_key_inputs(cx);
                view.settings_api_key_editors.insert("kimi".into());
                view.settings_api_key_inputs
                    .get("kimi")
                    .expect("api_key provider gets a secure input")
                    .update(cx, |input, cx| input.set_text("sk-live-plaintext", cx));
            });
        });

        let input_id = crate::ui::settings::settings_api_key_input_identifier("kimi");
        let verify_id = crate::ui::settings::SettingsAuthAction::VerifyApiKey.identifier("kimi");
        let expected_mask = "•".repeat("sk-live-plaintext".chars().count());

        // 连接态：掩码 value 发布、按钮 enabled、Press 许可。
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("settings AX tree validates");
            let secret = "sk-live-plaintext";
            for child in &tree.children {
                assert_no_secret(child, secret);
                assert_no_secret(child, "masked-fragment-sentinel");
            }
            let connected = tree
                .find(&dynamic_identifier("settings-provider", "connected"))
                .expect("connected provider has an AX summary");
            assert!(connected.value.as_deref().is_some_and(|value| {
                value.contains("Connected") && !value.contains("masked-fragment-sentinel")
            }));
            // 列几何与 render 同源：auth-methods 列并入 name value 后，
            // connection / catalog 必须平移 112px（104 列 + 8 间距）。
            let connection = tree
                .find(&dynamic_identifier(
                    "settings-provider-connection",
                    "connected",
                ))
                .expect("connection column has an AX node");
            let catalog = tree
                .find(&dynamic_identifier(
                    "settings-provider-catalog",
                    "connected",
                ))
                .expect("catalog column has an AX node");
            assert_eq!(connection.bounds.x, connected.bounds.x + 300.0);
            assert_eq!(catalog.bounds.x, connected.bounds.x + 440.0);
            let input = tree.find(&input_id).expect("secure input has an AX node");
            assert_eq!(input.role, AxRole::TextArea);
            assert_eq!(input.value.as_deref(), Some(expected_mask.as_str()));
            assert!(input.enabled);
            let verify = tree.find(&verify_id).expect("verify button has an AX node");
            assert!(verify.enabled);
            assert!(tree.permits(&AxRequest {
                identifier: verify_id.clone(),
                action: AxAction::Press,
                value: None,
            }));
        });

        // 断线 stale：写动作与 secure 输入同时禁用，permits 拒绝。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection
                    .settings_providers
                    .mark_stale("socket closed");
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            assert!(!tree.find(&verify_id).expect("verify button").enabled);
            assert!(!tree.find(&input_id).expect("secure input").enabled);
            assert!(!tree.permits(&AxRequest {
                identifier: verify_id.clone(),
                action: AxAction::Press,
                value: None,
            }));
        });
    }

    /// P0-4 修复：model 菜单内容超过 MENU_MAX_HEIGHT 时，render 面板内部
    /// 滚动而 AX 只发布与裁剪后菜单框相交的子节点，树内不得出现框外 rect。
    #[gpui::test]
    fn model_menu_ax_culls_rows_outside_clipped_frame(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;

        struct AxMenuHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxMenuHost {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div()
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("p0-4-model-menu-cull.sock");
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            AxMenuHost { view }
        });
        let view = cx.update(|_window, cx| host.read(cx).view.clone());
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                let mut models = Vec::new();
                for provider in ["alpha", "beta"] {
                    for ix in 0..4 {
                        models.push(ModelEntry {
                            provider_id: provider.into(),
                            id: format!("{provider}-{ix}"),
                            display_name: format!("{provider} model {ix}"),
                            context_window_tokens: None,
                        });
                    }
                }
                view.projection.set_models(models);
                view.open_menu = Some(MenuKind::Model);
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("model menu AX tree validates");
            let menu = tree.find("model-menu").expect("model menu has an AX node");
            assert!(menu.bounds.height <= MENU_MAX_HEIGHT);
            // 8 模型 + 2 组头的完整内容确实超过 240px，裁剪路径被真实走到。
            let full_content = metrics::MENU_PADDING * 2.0
                + 2.0 * MODEL_MENU_GROUP_HEADER_HEIGHT
                + 8.0 * metrics::MENU_ROW_HEIGHT;
            assert!(full_content > MENU_MAX_HEIGHT);
            let bottom = menu.bounds.y + menu.bounds.height;
            for child in &menu.children {
                assert!(
                    child.bounds.y < bottom,
                    "{} starts at {} outside menu bottom {}",
                    child.identifier,
                    child.bounds.y,
                    bottom
                );
            }
            // 裁剪确实发生：完整内容 2 组头 + 8 行不可能全部入树。
            assert!(menu.children.len() < 10);
            assert!(!menu.children.is_empty());
        });
    }
}
