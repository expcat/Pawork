//! AppView → AxTree 投影与 AX action 白名单。

use gpui::{App, Context, Focusable, Window};

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
use crate::ui::i18n::t;
use crate::ui::input_area::{
    grouped_model_menu_entries, MODEL_MENU_EMPTY_STATE_HEIGHT, MODEL_MENU_GROUP_HEADER_HEIGHT,
};
use crate::ui::inspector::{
    plain_terminal_output, terminal_empty_output, terminal_header_height,
    terminal_resize_status_label, terminal_size_for_display, terminal_stepper_ax_rects,
    InspectorTab, TERMINAL_COLUMNS_STEP, TERMINAL_ROWS_STEP,
};
use crate::ui::resources::ResourcesFetch;
use crate::ui::settings::{
    parse_settings_control, parse_settings_mcp_control, parse_settings_models_control,
    parse_settings_role_control, settings_text_scale_from_identifier, SettingsControl,
    SettingsModelsControl, SettingsRoleControl, SETTINGS_APPEARANCE_CONTROL_GAP,
    SETTINGS_APPEARANCE_CONTROL_HEIGHT, SETTINGS_APPEARANCE_CONTROL_WIDTH, SETTINGS_CONTROL_PREFIX,
    SETTINGS_MCP_CONTROL_PREFIX, SETTINGS_MODELS_CONTROL_PREFIX, SETTINGS_ROLE_CONTROL_PREFIX,
};
use crate::ui::shell_layout;
use crate::ui::theme::{font, metrics};
use crate::ui::timeline_entry::{display_time, tool_group_summary};
use crate::ui::{
    activity_header_visibility, rail_project_occurrence_key, rail_session_archive_focus_key,
    rail_session_focus_key, rail_session_rename_focus_key, terminal_can_operate,
    terminal_can_reopen, terminal_close_label, terminal_known_ended, timeline,
    workspace_empty_hint, workspace_empty_title, AppRoute, AppView, MenuKind, SettingsPage,
};

pub(crate) const PAD: f32 = 8.0;
pub(crate) const CONTROL_HEIGHT: f32 = 28.0;
pub(crate) const ROW_HEIGHT: f32 = 32.0;

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

/// OPT-4b：折叠态 Activity 触发器槽——位于最右动作槽（inspector-expand）
/// 左侧一格，与 render 的 HEADER_ACTION_GAP 排列同源。
fn header_activity_ax_rect(frame: AxRect) -> AxRect {
    let right = header_action_ax_rect(frame);
    AxRect::new(
        (right.x - metrics::HEADER_ACTION_WIDTH - metrics::HEADER_ACTION_GAP).max(frame.x),
        right.y,
        metrics::HEADER_ACTION_WIDTH,
        metrics::HEADER_ACTION_HEIGHT,
    )
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
        metrics::ACTIVITY_POPOVER_HEIGHT * scale + 2.0 * menu_inset,
    );
    let heading = AxRect::new(
        frame.x + inset_x,
        // p_4 + 标题行 + divider + gap_2 + section border + p_2
        frame.y + menu_inset + 2.0 * rem_px + 1.5 * font::BODY.0 * rem_px + 2.0,
        (frame.width - 2.0 * inset_x).max(0.0),
        1.5 * font::BODY_SM.0 * rem_px,
    );
    let open_changes = AxRect::new(
        heading.x,
        heading.y + heading.height + 0.25 * rem_px,
        heading.width,
        metrics::MENU_ROW_HEIGHT,
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
        if let Some(input) = self.settings_account_names.iter().find_map(|(id, input)| {
            (dynamic_identifier("settings-account-name", id) == request.identifier)
                .then_some(input.clone())
        }) {
            match request.action {
                AxAction::Focus => window.focus(&input.read(cx).focus_handle(cx)),
                AxAction::SetValue => input.update(cx, |input, cx| {
                    input.set_text(request.value.unwrap_or_default(), cx)
                }),
                AxAction::Press => return,
            }
            cx.notify();
            return;
        }
        match request.action {
            AxAction::Focus => match request.identifier.as_str() {
                "composer-input" => self.focus_composer(window, cx),
                // ADR-054 D2：行内改名编辑器聚焦（与点击行内输入框同路径）。
                "session-rename-input" => {
                    if let Some(rename) = self.session_rename.as_ref() {
                        let focus = rename.input.read(cx).focus_handle(cx);
                        window.focus(&focus);
                    }
                }
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
                _ if request.identifier.starts_with("settings-provider-details-") => {
                    if let Some(input) =
                        self.settings_auth_details.iter().find_map(|(id, input)| {
                            (dynamic_identifier("settings-provider-details", id)
                                == request.identifier)
                                .then_some(input)
                        })
                    {
                        window.focus(&input.read(cx).focus_handle(cx));
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
                    "session-rename-input" => {
                        if let Some(rename) = self.session_rename.as_ref() {
                            rename
                                .input
                                .update(cx, |input, cx| input.set_text(value, cx));
                        }
                    }
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
            "scope-add-project" => self.on_open_project(window, cx),
            "composer-project-task" => self.on_project_task_menu(None, window, cx),
            // ADR-054 D1：All-projects 态直建无归属会话（无确认浮层）；
            // AXPress 先移焦触发器，与真实点击同一路径。
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
            "connection-retry"
            | "connection-diagnostics"
            | "terminal-permissions"
            | "terminal-details" => self.on_recovery_action(identifier, window, cx),
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
            // i18n：语言按钮 AX press 与可见按钮同源派发；未知 id fail-closed。
            other if other.starts_with("settings-language-") => {
                let Some(language) = crate::ui::i18n::language_from_identifier(other) else {
                    return false;
                };
                self.on_settings_language(language, cx);
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
            other if other.starts_with("thinking-toggle-") => {
                let Some(key) = self.projection.timeline_rows().iter().find_map(|row| {
                    let TimelineRow::Thinking { entry_index } = row else {
                        return None;
                    };
                    let key = &self.projection.timeline[*entry_index].event_id;
                    (dynamic_identifier("thinking-toggle", key) == other).then(|| key.clone())
                }) else {
                    return false;
                };
                self.toggle_timeline_detail(&key, cx);
            }
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
                self.toggle_timeline_detail(&group_key, cx);
            }
            // Inspector 折叠态触发器的可见语义是弹出 ActivityPopover（R6
            // Wave A 起位于 Workspace Header），摘要行才展开 Inspector；
            // 展开态由 inspector-collapse 收起。
            "inspector-toggle" => {
                window.focus(&self.inspector_activity_focus);
                self.toggle_menu(MenuKind::Activity, None, cx)
            }
            "inspector-collapse" => self.on_toggle_inspector(window, cx),
            // OPT-4b：折叠态 Header 最右重开入口；Press 先移焦触发器，再与
            // click / Enter / Space 共用 on_toggle_inspector。
            "inspector-expand" => {
                window.focus(&self.inspector_expand_focus);
                self.on_toggle_inspector(window, cx)
            }
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
                if self.on_settings_quota_action(identifier, cx) {
                    return true;
                }
                if self.on_settings_account_action(identifier, cx) {
                    return true;
                }
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
                        Some(SettingsControl::UseProxy(escaped)) => {
                            if let Some(provider_id) =
                                self.settings_provider_id_for_escaped(&escaped)
                            {
                                self.on_settings_toggle_provider_use_proxy(provider_id, cx);
                                return true;
                            }
                        }
                        Some(SettingsControl::Expand(escaped)) => {
                            if let Some(provider_id) =
                                self.settings_provider_id_for_escaped(&escaped)
                            {
                                self.on_toggle_settings_provider_expanded(provider_id, cx);
                                return true;
                            }
                        }
                        _ => {}
                    }
                    return false;
                }
                if identifier.starts_with(SETTINGS_ROLE_CONTROL_PREFIX) {
                    // OPT-3b：四默认角色下拉（触发器 / 清除 / 候选行）与
                    // 可见菜单同入口派发；入口复核 gate，未知 pair fail-closed。
                    match parse_settings_role_control(identifier) {
                        Some(SettingsRoleControl::Trigger(role)) => {
                            self.on_toggle_settings_role_menu(role, None, window, cx);
                            return true;
                        }
                        Some(SettingsRoleControl::Clear(role)) => {
                            self.on_select_settings_role(role, None, cx);
                            return true;
                        }
                        Some(SettingsRoleControl::Item(role, escaped)) => {
                            if let Some((provider_id, model_id)) =
                                self.settings_default_target_for_escaped(&escaped)
                            {
                                self.on_select_settings_role(
                                    role,
                                    Some((provider_id, model_id)),
                                    cx,
                                );
                                return true;
                            }
                        }
                        None => {}
                    }
                    return false;
                }
                if identifier.starts_with(SETTINGS_MODELS_CONTROL_PREFIX) {
                    // OPT-3a：「Manage models」弹层（触发器 / 单模型
                    // Switch / Enable-Disable all / Refresh）与可见控件同
                    // 入口派发；入口复核 gate，未知 pair fail-closed。
                    match parse_settings_models_control(identifier) {
                        Some(SettingsModelsControl::Manage(escaped)) => {
                            if let Some(provider_id) =
                                self.settings_provider_id_for_escaped(&escaped)
                            {
                                self.on_toggle_settings_models_menu(provider_id, None, window, cx);
                                return true;
                            }
                        }
                        Some(SettingsModelsControl::Toggle(escaped)) => {
                            if let Some((provider_id, model_id)) =
                                self.settings_model_target_for_escaped(&escaped)
                            {
                                self.on_toggle_provider_model(provider_id, model_id, cx);
                                return true;
                            }
                        }
                        Some(SettingsModelsControl::EnableAll(escaped)) => {
                            if let Some(provider_id) =
                                self.settings_provider_id_for_escaped(&escaped)
                            {
                                self.on_toggle_provider_models_all(provider_id, true, cx);
                                return true;
                            }
                        }
                        Some(SettingsModelsControl::DisableAll(escaped)) => {
                            if let Some(provider_id) =
                                self.settings_provider_id_for_escaped(&escaped)
                            {
                                self.on_toggle_provider_models_all(provider_id, false, cx);
                                return true;
                            }
                        }
                        Some(SettingsModelsControl::RefreshCatalog(_)) => {
                            self.on_refresh_settings(cx);
                            return true;
                        }
                        None => {}
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
                // ADR-054 D2/D3：行级改名 / 归档与可见按钮同源派发（permits
                // 已按当前树核对 enabled）。
                if let Some(session) = self
                    .projection
                    .sessions
                    .iter()
                    .find(|session| session_rename_identifier(&session.session_id) == identifier)
                    .cloned()
                {
                    self.begin_session_rename(&session.session_id, window, cx);
                    return true;
                }
                if let Some(session) = self
                    .projection
                    .sessions
                    .iter()
                    .find(|session| session_archive_identifier(&session.session_id) == identifier)
                    .cloned()
                {
                    self.on_session_archive(session.session_id, cx);
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
                if let Some((event_id, ix)) = self.projection.timeline.iter().find_map(|entry| {
                    self.entry_menu_actions(&entry.event_id)
                        .iter()
                        .enumerate()
                        .find(|(ix, action)| action.identifier(&entry.event_id, *ix) == identifier)
                        .map(|(ix, _)| (entry.event_id.clone(), ix))
                }) {
                    self.activate_entry_action(&event_id, ix, window, cx);
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
        // P2-1：Settings 壳不渲染 RunStatusBar（与 render 同源），内容用全高；
        // 工作台保留底部 24px StatusBar。
        let settings_route = self.route == AppRoute::Settings;
        let content_height = if settings_route {
            height
        } else {
            (height - metrics::STATUS_BAR_HEIGHT).max(1.0)
        };
        // 与 AppView::render 共享同一壳层几何决定（R2 Wave A 响应式：
        // 窄窗 rail=240 且 Inspector 强制折叠；150% 文本 rail=320），
        // AX bounds 不得偏出实际布局。
        let shell = shell_layout::resolve(
            viewport.width,
            self.inspector_open,
            self.text_scale == font::TextScale::Percent150,
        );
        let sidebar_width = shell.rail_width.min(width);
        // UI-1：render 先更新本帧过渡宽度；开合期间也按可见宽度布局，
        // Header 动作仍使用下方逻辑终态 shell.inspector_open。
        let inspector_width = self
            .inspector_render_width
            .min((width - sidebar_width).max(0.0));
        let workspace_width = (width - sidebar_width - inspector_width).max(0.0);
        let workspace_x = sidebar_width;

        // SET-3 顶层路由：Settings 壳与工作台互斥（与 render 同源）。
        let mut tree =
            if settings_route {
                AxTree::new(width, height)
                    .child(self.settings_rail_ax(
                        window,
                        AxRect::new(0.0, 0.0, sidebar_width, content_height),
                    ))
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
                    .child(self.sidebar_ax(
                        window,
                        cx,
                        AxRect::new(0.0, 0.0, sidebar_width, content_height),
                    ))
                    .child(self.workspace_ax(
                        window,
                        cx,
                        AxRect::new(workspace_x, 0.0, workspace_width, content_height),
                        shell.inspector_open,
                    ));
                if inspector_width > 0.0 {
                    let mut inspector = self.inspector_ax(
                        window,
                        cx,
                        AxRect::new(
                            width - inspector_width,
                            0.0,
                            metrics::INSPECTOR_WIDTH,
                            content_height,
                        ),
                    );
                    // 与 shell-inspector overflow_hidden 同源：内部保持
                    // 440px 排版，只裁剪右缘；完全不可见的节点不发布动作。
                    fn clip_right(node: &mut AxNode, right: f32) -> bool {
                        node.bounds.width = node.bounds.width.min((right - node.bounds.x).max(0.0));
                        node.children.retain_mut(|child| clip_right(child, right));
                        node.bounds.width > 0.0 && node.bounds.height > 0.0
                    }
                    clip_right(&mut inspector, width);
                    tree = tree.child(inspector);
                }
                tree
            };
        if !settings_route {
            // StatusBar 视觉上不覆盖左栏账户区；AX frame 与 render 同源。
            tree = tree.child(self.status_ax(AxRect::new(
                sidebar_width,
                content_height,
                (width - sidebar_width).max(0.0),
                metrics::STATUS_BAR_HEIGHT,
            )));
        }
        tree
    }

    fn sidebar_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let can_create = self.can_create_task();
        // 与可见 TaskRail 对齐（R3 Wave A，几何单一来源 theme::metrics）：
        // Panel p_2(8) + 36px traffic-light 安全区 + gap_2(8) 后进入标题行，
        // 三行节奏 32 / 20；AX 不得把首控件投影到按钮带。
        let rem_px = f32::from(window.rem_size());
        let inset = 0.5 * rem_px + metrics::RAIL_INNER_PAD;
        let mut y = rem_px + shell_layout::TRAFFIC_LIGHT_SAFE_HEIGHT;
        let grouping = AxNode::new(
            "task-rail-grouping",
            AxRole::Button,
            self.grouping.toggle_action_label(),
            AxRect::new(
                (frame.width - inset - metrics::RAIL_ICON_BUTTON_SIZE).max(inset),
                // 标题行高 36、按钮同取 36（OPT-D）：render items_center → 顶 +0。
                y + (metrics::RAIL_TITLE_ROW_HEIGHT - metrics::RAIL_ICON_BUTTON_SIZE) / 2.0,
                metrics::RAIL_ICON_BUTTON_SIZE,
                metrics::RAIL_ICON_BUTTON_SIZE,
            ),
        )
        .value(self.grouping.view_label())
        .focused(self.open_menu.is_none() && self.grouping_focus.is_focused(window))
        .action(AxAction::Press);
        y += metrics::RAIL_TITLE_ROW_HEIGHT + metrics::RAIL_TITLE_SCOPE_GAP;
        let scope_label = self.scope_label();
        let scope = AxNode::new(
            "project-scope",
            AxRole::Button,
            t("rail.filter_label").replace("{}", &scope_label),
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
            t("timeline.new_task"),
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
                    t("rail.reconnect"),
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
                        group.bucket.display_label(),
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
                            cx,
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
                    let (nodes, consumed) = self
                        .project_ax_nodes(window, cx, project, None, row_y, list_width, can_create);
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
                t("rail.tooltip_settings"),
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
                    t("composer.project_task")
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
        cx: &App,
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
            AxRect::new(
                inset,
                top,
                if !project.is_unassigned() && project.workspace_id.is_some() {
                    (width - metrics::RAIL_ICON_BUTTON_SIZE - 8.0).max(0.0)
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
                let is_active = self.projection.active_session_id.as_deref()
                    == Some(session.session_id.as_str());
                let renaming = self
                    .session_rename
                    .as_ref()
                    .is_some_and(|state| state.session_id == session.session_id);
                let row_top = top + consumed;
                let action_y = row_top
                    + (metrics::RAIL_TASK_ROW_HEIGHT - metrics::RAIL_SESSION_ACTION_SIZE) / 2.0;
                let mut row = AxNode::new(
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
                .selected(is_active);
                if renaming {
                    // 行内改名：行不再激活打开，发布编辑器（Focus / SetValue
                    // 与 composer 输入同构；AXValue 即草稿纯文本）。
                    if let Some(state) = self.session_rename.as_ref() {
                        row = row.child(
                            AxNode::new(
                                "session-rename-input",
                                AxRole::TextArea,
                                t("taskrail.rename"),
                                AxRect::new(
                                    inset,
                                    row_top,
                                    (width - metrics::RAIL_SESSION_ACTION_SIZE * 2.0).max(0.0),
                                    metrics::RAIL_TASK_ROW_HEIGHT,
                                ),
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
                    row = row.action(AxAction::Press);
                    // UI-2：悬停 / 行或动作聚焦 / 当前会话，与 render 共用可见性。
                    if self.session_actions_visible(&session.session_id, window) {
                        let rename_focus_key = rail_session_rename_focus_key(&session.session_id);
                        let archive_focus_key = rail_session_archive_focus_key(&session.session_id);
                        row = row
                            .child(
                                AxNode::new(
                                    session_rename_identifier(&session.session_id),
                                    AxRole::Button,
                                    t("taskrail.rename"),
                                    AxRect::new(
                                        (inset + width
                                            - 8.0
                                            - metrics::RAIL_SESSION_ACTION_SIZE * 2.0)
                                            .max(inset),
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
                                        (inset + width - 8.0 - metrics::RAIL_SESSION_ACTION_SIZE)
                                            .max(inset),
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
        // R6 Wave A（F-12）+ OPT-4b：与 render 同用 activity_header_visibility
        // 口径；折叠态 Activity 左移一格、inspector-expand 占最右动作槽，
        // 浮层右缘与触发器右缘对齐；展开态最右槽恢复 New task。
        let (trigger_visible, popover_visible) = activity_header_visibility(
            inspector_open,
            matches!(self.open_menu, Some(MenuKind::Activity)),
        );
        if trigger_visible {
            // 折叠态 Activity 槽位于重开按钮左侧一格（gap 与 render 同源）。
            let trigger_rect = header_activity_ax_rect(frame);
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
                let geometry =
                    activity_popover_ax_geometry(frame, trigger_rect, f32::from(window.rem_size()));
                header = header.child(
                    AxNode::new(
                        "activity-popover",
                        AxRole::Group,
                        t("changes.tab_activity"),
                        geometry.frame,
                    )
                    .child(AxNode::new(
                        "activity-changes-heading",
                        AxRole::StaticText,
                        t("changes.tab_changes"),
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
            // OPT-4b：折叠态重开 Inspector 的专用入口，占最右动作槽；展开态
            // 不发布（折叠走面板内 inspector-collapse）。
            header.child(
                AxNode::new(
                    "inspector-expand",
                    AxRole::Button,
                    t("header.tooltip_open_inspector"),
                    action,
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
                    action,
                )
                .enabled(self.can_create_task())
                .focused(self.open_menu.is_none() && self.header_new_task_focus.is_focused(window))
                .action(AxAction::Press),
            )
        }
    }

    fn timeline_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        let rows = self.projection.timeline_rows();
        let total = rows.len();
        let empty_hint_visible = self.projection.workspace_empty_hint_visible();
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
                t("timeline.new_task"),
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
                    workspace_empty_title(),
                    AxRect::new(content_x, group_y, content_width, 28.0),
                ))
                .child(AxNode::new(
                    "workspace-empty-hint",
                    AxRole::StaticText,
                    workspace_empty_hint(),
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
                ("approve-once", t("approval.allow_once")),
                ("approve-for-run", t("approval.allow_for_run")),
                ("approve-deny", t("approval.deny")),
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

    /// 渲染行 → AX 节点（与 timeline.rs 组装同源）。消息 / 错误条目保持
    /// 既有 timeline-entry / entry-menu / fork identifier；tool 组与 Run
    /// 摘要区域为新结构节点。
    fn timeline_row_ax(&self, window: &Window, row: &TimelineRow, rect: AxRect) -> AxNode {
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
                        AxRect::new(
                            rect.x,
                            rect.y,
                            rect.width,
                            metrics::TOOL_GROUP_HEADER_HEIGHT,
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
                if expanded {
                    if let TimelineEntryKind::Thinking { text } = &entry.kind {
                        group = group.child(
                            AxNode::new(
                                dynamic_identifier("thinking-text", key),
                                AxRole::StaticText,
                                t("timeline.thinking"),
                                AxRect::new(
                                    rect.x + 12.0,
                                    rect.y + metrics::TOOL_GROUP_HEADER_HEIGHT + 12.0,
                                    (rect.width - 24.0).max(0.0),
                                    (rect.height - metrics::TOOL_GROUP_HEADER_HEIGHT - 24.0)
                                        .max(0.0),
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
                        tool_y,
                        rect.width,
                        timeline::tool_entry_height(
                            entry,
                            rect.width,
                            f32::from(window.rem_size()),
                        ),
                    );
                    tool_y += tool_rect.height;
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
                                    AxRect::new(
                                        rect.x,
                                        y,
                                        rect.width,
                                        timeline::tool_entry_height(
                                            entry,
                                            rect.width,
                                            f32::from(window.rem_size()),
                                        ),
                                    ),
                                )
                                .value(timeline::tool_status_label(status))
                                .description(detail.clone().unwrap_or_default()),
                            );
                            y += timeline::tool_entry_height(
                                entry,
                                rect.width,
                                f32::from(window.rem_size()),
                            );
                        }
                    }
                    y += if timeline::run_summary_card_visible(
                        terminal_entry,
                        self.changes_available_for_active(),
                    ) {
                        metrics::SUMMARY_CARD_GAP
                    } else {
                        metrics::TIMELINE_FOOTER_GAP
                    };
                }
                let review_enabled = terminal_entry.fork_boundary == Some(ForkBoundary::Completed)
                    && self.changes_available_for_active();
                let (title, description) = run_summary_texts(terminal_entry, review_enabled)
                    .unwrap_or(("Run", String::new()));
                let show_card = timeline::run_summary_card_visible(terminal_entry, review_enabled);
                if show_card {
                    region = region.child(
                        AxNode::new(
                            dynamic_identifier("run-summary-card", &terminal_entry.event_id),
                            AxRole::StaticText,
                            title,
                            AxRect::new(rect.x, y, rect.width, metrics::SUMMARY_CHECK_CIRCLE),
                        )
                        .description(description),
                    );
                }
                if review_enabled {
                    region = region.child(
                        AxNode::new(
                            run_review_identifier(&terminal_entry.event_id),
                            AxRole::Button,
                            t("timeline.review_changes"),
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
                if show_card {
                    y += metrics::SUMMARY_CHECK_CIRCLE + metrics::TIMELINE_FOOTER_GAP;
                }
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
                    entry_node = self.entry_actions_ax(entry_node, &terminal_entry.event_id);
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
            let inset = if matches!(entry.kind, TimelineEntryKind::UserMessage { .. }) {
                metrics::MSG_USER_INSET
            } else {
                0.0
            };
            node = node.child(
                AxNode::new(
                    entry_menu_identifier(&entry.event_id),
                    AxRole::Button,
                    "Entry actions",
                    AxRect::new(row.x + row.width - inset - 32.0, row.y + inset, 32.0, 24.0),
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
            let fork = matches!(
                action.kind,
                super::super::timeline_entry::EntryActionKind::Fork
            );
            let enabled = !fork || self.can_fork_entry(event_id);
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
            if fork && !enabled {
                child = child.description(self.fork_unavailable_reason(event_id));
            }
            node = node.child(child);
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
        let width = (frame.width - metrics::COMPOSER_OUTER_X * 2.0)
            .max(0.0)
            .min(metrics::TIMELINE_READABLE_WIDTH);
        let column_x = frame.x + (frame.width - width) / 2.0;
        let meta_height = Self::composer_meta_height(window);
        let card_height = frame.height
            - metrics::COMPOSER_OUTER_TOP
            - metrics::COMPOSER_OUTER_BOTTOM
            - metrics::COMPOSER_META_GAP
            - self.composer_meta_layout_height(window)
            - (metrics::COMPOSER_META_GAP + meta_height) * self.composer_notes().len() as f32;
        let card = AxRect::new(
            column_x,
            frame.y + metrics::COMPOSER_OUTER_TOP,
            width,
            card_height,
        );
        let actual_card = self.composer_layouts["composer-card"].bounds();
        let card = if actual_card.size.height > gpui::px(0.0) {
            AxRect::new(
                actual_card.origin.x.into(),
                actual_card.origin.y.into(),
                actual_card.size.width.into(),
                actual_card.size.height.into(),
            )
        } else {
            card
        };
        let pad = metrics::COMPOSER_PAD + metrics::COMPOSER_BORDER / 2.0;
        let input_y = card.y + pad;
        let footer_y = card.y + card.height - pad - metrics::COMPOSER_SEND_SIZE;
        let input_height = (footer_y - metrics::COMPOSER_GAP - input_y).max(0.0);
        let action_x = card.x + card.width - pad - metrics::COMPOSER_SEND_SIZE;
        let input_width = (card.width - pad * 2.0).max(0.0);
        let meta_y = card.y + card.height + metrics::COMPOSER_META_GAP;
        let meta_width = (width - metrics::SPACE_2) / 2.0;
        let running = self.projection.active_run_id.is_some();
        let current_model = self.model_label();
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
                    AxRect::new(card.x + pad, input_y, input_width, input_height),
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
                        card.x + pad,
                        footer_y,
                        metrics::COMPOSER_MODEL_WIDTH,
                        metrics::COMPOSER_FOOTER_CONTROL,
                    ),
                )
                .value(current_model)
                .enabled(self.can_open_model_menu())
                // R7 Wave A：model 菜单打开时 AX 焦点移交高亮项（同 grouping）。
                .focused(self.open_menu.is_none() && self.model_focus.is_focused(window))
                .action(AxAction::Press),
            )
            // OPT-D：项目上下文槽（无项目 chip / Workspace · 名称）与文件
            // 工具不可用提示——纯状态文本，与 footer 可见文案同源。
            .child(
                AxNode::new(
                    "composer-workspace",
                    AxRole::StaticText,
                    "Workspace context",
                    AxRect::new(column_x, meta_y, meta_width, meta_height),
                )
                .value(if self.composer_workspace_no_project() {
                    t("composer.no_project_chip").to_string()
                } else {
                    self.composer_workspace_label()
                }),
            );
        composer = composer.child(
            AxNode::new(
                "composer-context",
                AxRole::StaticText,
                "Context",
                AxRect::new(
                    column_x + meta_width + metrics::SPACE_2,
                    meta_y,
                    meta_width,
                    meta_height,
                ),
            )
            .value(self.projection.context_meter_label()),
        );
        if self.composer_file_tools_unavailable_visible() {
            composer = composer.child(AxNode::new(
                "composer-file-tools-hint",
                AxRole::StaticText,
                t("composer.file_tools_unavailable"),
                frame,
            ));
        }
        if self.composer_project_task_visible() {
            let mut node = AxNode::new(
                "composer-project-task",
                AxRole::Button,
                t("composer.project_task"),
                frame,
            )
            .enabled(self.can_create_task())
            .focused(self.open_menu.is_none() && self.project_task_focus.is_focused(window));
            if self.can_create_task() {
                node = node.action(AxAction::Press);
            }
            composer = composer.child(node);
        }
        // 元信息可以换行，AX 只使用同帧实测控件框。
        for node in &mut composer.children {
            if let Some(handle) = self.composer_layouts.get(node.identifier.as_str()) {
                let bounds = handle.bounds();
                node.bounds = AxRect::new(
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    bounds.size.width.into(),
                    bounds.size.height.into(),
                );
            }
        }
        for (index, (id, note)) in self.composer_notes().into_iter().enumerate() {
            composer = composer.child(
                AxNode::new(
                    id,
                    AxRole::StaticText,
                    if id == "composer-file-tools-hint" {
                        "File tools"
                    } else {
                        "Status"
                    },
                    AxRect::new(
                        column_x,
                        meta_y
                            + self.composer_meta_layout_height(window)
                            + metrics::COMPOSER_META_GAP
                            + (metrics::COMPOSER_META_GAP + meta_height) * index as f32,
                        width,
                        meta_height,
                    ),
                )
                .value(note),
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
            let content_height = if entries.is_empty() {
                metrics::MENU_PADDING * 2.0 + MODEL_MENU_EMPTY_STATE_HEIGHT
            } else {
                metrics::MENU_PADDING * 2.0
                    + groups.len() as f32 * MODEL_MENU_GROUP_HEADER_HEIGHT
                    + entries.len() as f32 * metrics::MENU_ROW_HEIGHT
            };
            let menu_height = content_height.min(MENU_MAX_HEIGHT);
            let menu_x = card.x + pad;
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
            if entries.is_empty() {
                // 全关空态：菜单只发布一行说明（StaticText，无 Press——
                // disabled 节点不发布 Press），不编造可选模型。
                menu = menu.child(
                    AxNode::new(
                        "model-menu-empty",
                        AxRole::StaticText,
                        t("composer.model_none_available"),
                        AxRect::new(menu_x, y, 260.0, MODEL_MENU_EMPTY_STATE_HEIGHT),
                    )
                    .value(t("composer.model_menu_empty")),
                );
            } else {
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
            }
            composer = composer.child(menu);
        }
        composer
    }

    fn inspector_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let tab_width = metrics::INSPECTOR_TAB_WIDTH;
        let strip_height = metrics::INSPECTOR_TAB_HEIGHT;
        let tab_x = frame.x + 12.0;
        // OPT-4a：折叠按钮 36×36（render 同源 ICON_BUTTON_SIZE；pr_2 在
        // 100% 字号下为 8px，与 PAD 近似口径沿用）。
        let collapse_y = frame.y + ((strip_height - metrics::ICON_BUTTON_SIZE) / 2.0).max(0.0);
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
                        t("inspector.tab_changes"),
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
                        t("inspector.tab_terminal"),
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
                        t("inspector.tab_resources"),
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
                        frame.x + frame.width - PAD - metrics::ICON_BUTTON_SIZE,
                        collapse_y,
                        metrics::ICON_BUTTON_SIZE,
                        metrics::ICON_BUTTON_SIZE,
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
            // 与可见 Terminal 页占位同源（terminal_empty_output()）。
            if self.terminal_notice_text().is_some() {
                String::new()
            } else {
                terminal_empty_output().to_string()
            }
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
        let terminal_start_enabled = self.terminal_start_available();
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
        if self.terminal_notice_text().is_some() {
            let clip = AxRect::new(
                frame.x,
                frame.y + header_height,
                frame.width,
                (input_y - frame.y - header_height).max(0.0),
            );
            terminal = terminal.child(self.recovery_ax(window, clip, true));
        }
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
                    t("timeline.ax_back_to_bottom"),
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
        let now = crate::ui::now_unix_ms();
        // UI-1：四组元信息居中，共享状态栏可用区域。
        let run_status_width = frame.width;
        let run_status_x = frame.x;
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
        TimelineEntryKind::Thinking { text } => (t("timeline.thinking").into(), text.clone()),
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

fn session_rename_identifier(session_id: &str) -> String {
    dynamic_identifier("session-rename", session_id)
}

fn session_archive_identifier(session_id: &str) -> String {
    dynamic_identifier("session-archive", session_id)
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

fn model_identifier(model: &ModelEntry) -> String {
    dynamic_identifier("model", &format!("{}:{}", model.provider_id, model.id))
}

fn entry_menu_identifier(event_id: &str) -> String {
    dynamic_identifier("entry-menu", event_id)
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

    #[gpui::test]
    fn recovery_keeps_errors_local_and_preserves_drafts(cx: &mut gpui::TestAppContext) {
        use crate::controller::ControllerEvent;
        use gpui::px;
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                std::sync::Arc::new(crate::platform::Platform::new()),
                std::env::temp_dir().join("ux04-missing.sock"),
                None,
                cx,
            )
        });
        cx.run_until_parked();
        cx.simulate_resize(gpui::size(px(1440.0), px(1024.0)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.connection = ConnectionState::Failed {
                    reason: "connection refused".into(),
                };
                view.status_hint = Some("unrelated feedback".into());
                view.text_input
                    .update(cx, |input, cx| input.set_text("keep this draft", cx));
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let v = view.read(cx);
            let tree = v.accessibility_tree(window, cx);
            tree.validate().unwrap();
            assert!(tree.find("workspace-empty-title").is_none());
            assert!(tree.find("connection-retry").is_some());
            assert!(tree.find("connection-diagnostics").is_some());
            assert_eq!(v.model_label(), t("recovery.model_failed"));
            assert!(v.connection_notice_text().1.contains("address"));
        });
        cx.update(|window, cx| view.update(cx, |v, cx| {
            v.on_recovery_action("connection-diagnostics", window, cx);
            assert_eq!(v.settings_page, SettingsPage::Advanced);
            assert_eq!(v.text_input.read(cx).text(), "keep this draft");
            v.on_close_settings(window, cx);
            v.projection.connection = ConnectionState::Connecting;
            v.connection_attempts = 2;
            assert_eq!(v.model_label(), t("recovery.reconnecting"));
            v.projection.connection = ConnectionState::Connected { instance_id: "ux04".into() };
            v.projection.set_models(Vec::new());
            assert_eq!(v.model_label(), t("composer.model_none_available"));
            v.projection.workspace_id = Some("ws-a".into());
            v.projection.terminal.workspace_id = Some("ws-a".into());
            v.projection.settings_permissions.query.mark_ready();
            v.projection.settings_permissions.approval_mode = Some(ApprovalModeWire::ReadOnly);
            v.inspector_open = true;
            v.inspector_motion.width(true, true, std::time::Instant::now() - std::time::Duration::from_secs(1));
            v.inspector_tab = InspectorTab::Terminal;
            v.on_start_terminal(window, cx);
            assert!(v.terminal_pending_create_workspace.is_none());
            assert!(!v.terminal_start_available());
            assert_eq!(v.terminal_notice_text().as_deref(), Some(t("recovery.terminal_read_only")));
            v.status_hint = Some("unrelated feedback".into());
            v.handle_controller_event(ControllerEvent::TerminalCreateFailed {
                workspace_id: "ws-a".into(),
                reason: "ProtocolError: 审批档 read_only 禁止创建终端:ADR-041 D2 决议该档拒绝创建交互 shell(fail-closed)".into(),
            }, cx);
            assert_eq!(v.status_hint.as_deref(), Some("unrelated feedback"));
            assert_eq!(v.text_input.read(cx).text(), "keep this draft");
            v.projection.settings_permissions.available = false;
            assert!(!v.terminal_start_available(), "explicit Host read-only rejection also blocks repeated creation");
            assert_eq!(v.terminal_notice_text().as_deref(), Some(t("recovery.terminal_read_only_unknown")));
            cx.notify();
        }));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let v = view.read(cx);
            let tree = v.accessibility_tree(window, cx);
            tree.validate().unwrap();
            assert!(!tree.find("terminal-start").unwrap().enabled);
            assert!(tree
                .find("terminal-notice")
                .unwrap()
                .label
                .contains("Read-only"));
            assert!(tree.find("terminal-details").is_some());
        });
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.on_recovery_action("terminal-details", window, cx);
                assert!(v.terminal_details_open.is_some());
                v.projection
                    .settings_permissions
                    .query
                    .mark_stale("connection changed");
                assert!(
                    !v.terminal_read_only(),
                    "old permissions cannot describe a new connection"
                );
                v.projection
                    .apply_terminal_created("ws-a".into(), "term-a".into());
                v.handle_controller_event(
                    ControllerEvent::TerminalWriteFailed {
                        terminal_session_id: "term-a".into(),
                        reason: "write failed".into(),
                    },
                    cx,
                );
                assert!(terminal_can_operate(
                    &v.projection.connection,
                    &v.projection.terminal
                ));
                assert!(v.terminal_notice_text().is_some());
                assert_eq!(v.status_hint.as_deref(), Some("unrelated feedback"));
                v.handle_controller_event(
                    ControllerEvent::TerminalWriteSucceeded {
                        terminal_session_id: "term-a".into(),
                    },
                    cx,
                );
                assert!(v.projection.terminal.last_error.is_none());
            })
        });
    }

    #[gpui::test]
    fn reply_actions_copy_exact_content_and_use_closed_turn_boundary(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::ui::timeline_entry::{fork_target, EntryActionKind};
        use gpui::{px, ClipboardItem};
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                std::sync::Arc::new(crate::platform::Platform::new()),
                std::env::temp_dir().join("ux02-actions.sock"),
                None,
                cx,
            )
        });
        cx.run_until_parked();
        let text = "| 名称 | 数量 |\n| --- | ---: |\n| 中文 | 2 |\n\n```rust\n  let 中文 = 2;\n```\n[文档](https://example.test/docs)";
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.connection = ConnectionState::Connected {
                    instance_id: "ux02".into(),
                };
                view.projection.active_session_id = Some("task".into());
                view.projection.timeline.entries = vec![
                    TimelineEntry {
                        sequence: 1,
                        event_id: "reply".into(),
                        kind: TimelineEntryKind::AssistantMessage { text: text.into() },
                        fork_boundary: None,
                        timestamp: "1".into(),
                        run_id: Some("run".into()),
                    },
                    TimelineEntry {
                        sequence: 2,
                        event_id: "other".into(),
                        kind: TimelineEntryKind::RunState("Completed".into()),
                        fork_boundary: Some(ForkBoundary::Completed),
                        timestamp: "2".into(),
                        run_id: Some("other-run".into()),
                    },
                ];
                assert!(fork_target(&view.projection.timeline, "reply").is_none());
                let mut terminal = view.projection.timeline[1].clone();
                terminal.event_id = "terminal".into();
                terminal.run_id = Some("run".into());
                terminal.sequence = 3;
                view.projection.timeline.entries.push(terminal);
                assert_eq!(
                    fork_target(&view.projection.timeline, "reply"),
                    Some("terminal")
                );
                assert!(view.can_fork_entry("reply"));
                view.timeline_changed();
                view.toggle_menu(MenuKind::Entry("reply".into()), None, cx);
            })
        });
        cx.simulate_resize(gpui::size(px(1440.0), px(1024.0)));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.toggle_menu(MenuKind::Entry("reply".into()), None, cx);
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let actions = view.entry_menu_actions("reply");
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                let copy = actions[0].identifier("reply", 0);
                assert!(
                    tree.find(&copy).is_some(),
                    "copy action is visible in actual menu layout"
                );
                let request = AxRequest {
                    identifier: copy,
                    action: AxAction::Press,
                    value: None,
                };
                assert!(tree.permits(&request));
                view.handle_accessibility_request(request, window, cx);
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some(text.into())
                );
                assert!(view.open_menu.is_none());
                let code_ix = actions
                    .iter()
                    .position(
                        |a| matches!(&a.kind, EntryActionKind::Copy(s) if s == "  let 中文 = 2;\n"),
                    )
                    .unwrap();
                view.toggle_menu(MenuKind::Entry("reply".into()), None, cx);
                view.activate_menu_item(MenuKind::Entry("reply".into()), code_ix, window, cx);
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some("  let 中文 = 2;\n".into())
                );
                view.projection.connection = ConnectionState::Disconnected {
                    reason: "test".into(),
                };
                assert!(!view.can_fork_entry("reply"));
                cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
                view.activate_entry_action("reply", actions.len() - 1, window, cx);
                assert_eq!(
                    cx.read_from_clipboard().and_then(|item| item.text()),
                    Some("unchanged".into())
                );
            })
        });
    }

    /// UI-4：实际 GPUI 布局与 AX 操作区对齐，覆盖窄窗、大字号、长草稿。
    #[gpui::test]
    fn composer_layout_keeps_controls_in_card_and_ax_aligned(cx: &mut gpui::TestAppContext) {
        use crate::ui::theme::font::TextScale;
        use gpui::{px, size};
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("ui4-layout.sock"),
                None,
                cx,
            )
        });
        for (width, height, scale, lines) in [
            (1440.0, 1024.0, TextScale::Percent100, 1),
            (1080.0, 720.0, TextScale::Percent125, 3),
            (1080.0, 720.0, TextScale::Percent150, 80),
        ] {
            cx.simulate_resize(size(px(width), px(height)));
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.text_scale = scale;
                    window.set_rem_size(px(scale.rem_pixels()));
                    view.text_input.update(cx, |input, cx| {
                        input
                            .reset_text(&vec!["验收草稿自然换行".repeat(20); lines].join("\n"), cx);
                    });
                    cx.notify();
                })
            });
            cx.refresh().unwrap();
            cx.run_until_parked();
            let workspace = cx.debug_bounds("shell-workspace").unwrap();
            let card = cx.debug_bounds("composer-card").unwrap();
            let meta = cx.debug_bounds("composer-meta").unwrap();
            let model = cx.debug_bounds("composer-model-slot").unwrap();
            let action = cx.debug_bounds("composer-action-slot").unwrap();
            assert!(card.size.width <= px(metrics::TIMELINE_READABLE_WIDTH));
            assert!(card.left() >= workspace.left() + px(metrics::COMPOSER_OUTER_X));
            assert!(card.right() <= workspace.right() - px(metrics::COMPOSER_OUTER_X));
            assert!(meta.bottom() <= workspace.bottom() - px(metrics::COMPOSER_OUTER_BOTTOM));
            assert!(card.size.height <= px(metrics::COMPOSER_PANEL_MAX_HEIGHT));
            assert!(model.right() < action.left());
            cx.update(|window, cx| {
                let tree = view.read(cx).accessibility_tree(window, cx);
                for (id, actual) in [("model-picker", model), ("send", action)] {
                    let node = tree.find(id).unwrap();
                    for (expected, measured) in [
                        (node.bounds.x, f32::from(actual.left())),
                        (node.bounds.y, f32::from(actual.top())),
                        (node.bounds.width, f32::from(actual.size.width)),
                        (node.bounds.height, f32::from(actual.size.height)),
                    ] {
                        assert!(
                            (expected - measured).abs() < 1.0,
                            "{id} at {scale:?}: AX {expected}, render {measured}; card={card:?} workspace={workspace:?} input={:?}", view.read(cx).text_input.read(cx).viewport_height()
                        );
                    }
                }
                let context = tree.find("composer-context").unwrap();
                assert_eq!(context.value.as_deref(), Some("Context · unavailable"));
                assert!(context.bounds.y >= f32::from(meta.top()) && context.bounds.y + context.bounds.height <= f32::from(meta.bottom()) + 1.0);
                assert!(!tree.find("send").unwrap().enabled, "offline never sends");
            });
        }
    }

    /// UX-03：项目筛选不重绑任务，项目新建入口、限制和上下文共用可换行的元信息。
    #[gpui::test]
    fn project_task_guidance_preserves_context_and_wraps(cx: &mut gpui::TestAppContext) {
        use crate::projection::{SessionSummary, WorkspaceSummary};
        use crate::ui::theme::font::TextScale;
        use gpui::{px, size};
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("ux03-layout.sock"),
                None,
                cx,
            )
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.projection.workspaces = vec![WorkspaceSummary {
                    id: "project".into(),
                    name: "Example".into(),
                }];
                view.projection.sessions = vec![SessionSummary {
                    session_id: "old".into(),
                    title: "Draft".into(),
                    updated_at_ms: 1,
                    workspace_id: None,
                    parent_branch_id: None,
                    forked_from_event_id: None,
                    active: true,
                }];
                view.projection.active_session_id = Some("old".into());
                view.text_input
                    .update(cx, |input, cx| input.reset_text("保留原任务草稿", cx));
                view.on_select_scope(Some("project".into()), window, cx);
                assert!(view.active_task_hidden_by_filter());
                assert!(view.composer_workspace_no_project());
                assert_eq!(view.projection.sessions[0].workspace_id, None);
                assert_eq!(view.text_input.read(cx).text(), "保留原任务草稿");
                view.on_project_task_menu(None, window, cx);
                assert_eq!(
                    view.project_menu_options(),
                    vec![(Some("project".into()), "Example".into())]
                );
                assert_eq!(view.menu_item_count(), 2);
                view.close_menu_and_focus_trigger(MenuKind::ProjectTask, window, cx);
                assert!(view.project_task_focus.is_focused(window));
                view.on_select_scope(None, window, cx);
                assert!(!view.active_task_hidden_by_filter());
            })
        });
        for scale in [
            TextScale::Percent100,
            TextScale::Percent125,
            TextScale::Percent150,
        ] {
            for width in [1440.0, 1080.0] {
                cx.simulate_resize(size(px(width), px(900.0)));
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.text_scale = scale;
                        window.set_rem_size(px(scale.rem_pixels()));
                        cx.notify();
                    })
                });
                cx.refresh().unwrap();
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let view = view.read(cx);
                    let tree = view.accessibility_tree(window, cx);
                    let meta = view.composer_layouts["composer-meta"].bounds();
                    let bounds: Vec<_> = [
                        "composer-workspace",
                        "composer-file-tools-hint",
                        "composer-project-task",
                        "composer-context",
                    ]
                    .iter()
                    .map(|id| {
                        let node = tree.find(id).unwrap();
                        assert!(node.bounds.width > 0.0 && node.bounds.height > 0.0, "{id}");
                        assert!(
                            node.bounds.x >= f32::from(meta.left()) - 1.0
                                && node.bounds.x + node.bounds.width
                                    <= f32::from(meta.right()) + 1.0,
                            "{id}: {:?} meta={meta:?}",
                            node.bounds
                        );
                        assert!(
                            node.bounds.y >= f32::from(meta.top()) - 1.0
                                && node.bounds.y + node.bounds.height
                                    <= f32::from(meta.bottom()) + 1.0,
                            "{id}"
                        );
                        node.bounds
                    })
                    .collect();
                    for (i, a) in bounds.iter().enumerate() {
                        for b in &bounds[i + 1..] {
                            assert!(
                                a.x + a.width <= b.x + 1.0
                                    || b.x + b.width <= a.x + 1.0
                                    || a.y + a.height <= b.y + 1.0
                                    || b.y + b.height <= a.y + 1.0,
                                "overlap at {width} {scale:?}"
                            );
                        }
                    }
                });
            }
        }
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                window.focus(&view.project_task_focus);
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| assert_eq!(view.read(cx).open_menu, Some(MenuKind::ProjectTask)));
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| {
            assert!(
                view.read(cx).open_menu.is_none(),
                "second Enter confirms selection"
            );
            assert_eq!(view.read(cx).text_input.read(cx).text(), "保留原任务草稿");
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.status_hint = Some("keep status".into());
                view.projection.sessions[0].workspace_id = Some("project".into());
                assert!(!view.composer_file_tools_unavailable_visible());
                assert!(!view.composer_project_task_visible());
                assert_eq!(
                    view.composer_notes(),
                    vec![("composer-status-hint", "keep status".into())]
                );
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("composer-file-tools-hint").is_none());
                assert!(tree.find("composer-project-task").is_none());
                view.projection.active_session_id = None;
                assert!(
                    view.composer_project_task_visible(),
                    "empty state offers project creation"
                );
            })
        });
    }

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
        assert_eq!(height, metrics::COMPOSER_PANEL_MIN_HEIGHT);
        assert_eq!(crate::ui::theme::metrics::COMPOSER_SEND_SIZE, 36.0);
    }

    /// R6 Wave A：折叠态 Header Activity 的 AX 触发器与 Popover 锚点公式
    /// 必须钉住生产 render 所用的 40×37 槽、右侧 25px inset、8px gap 与
    /// 320×144 基准内容、面板内边距，以及大字号下的摘要命中范围。
    #[test]
    fn activity_header_ax_geometry_matches_render_anchor_contract() {
        let header = AxRect::new(240.0, 0.0, 840.0, metrics::HEADER_HEIGHT);
        let trigger = header_action_ax_rect(header);
        assert_eq!(trigger, AxRect::new(1016.0, 33.5, 40.0, 37.0));
        // OPT-4b：折叠态 Activity 左移一格（40 槽 + 4 间距），重开按钮占最右。
        let toggle = header_activity_ax_rect(header);
        assert_eq!(toggle, AxRect::new(972.0, 33.5, 40.0, 37.0));

        let popover = activity_popover_ax_geometry(header, trigger, 16.0);
        assert_eq!(popover.frame, AxRect::new(718.0, 78.5, 338.0, 162.0));
        assert_eq!(popover.heading, AxRect::new(752.0, 145.5, 270.0, 18.0));
        assert_eq!(
            popover.open_changes,
            AxRect::new(752.0, 167.5, 270.0, metrics::MENU_ROW_HEIGHT)
        );
        let large = activity_popover_ax_geometry(header, trigger, 24.0);
        assert_eq!(large.frame.height, 234.0);
        assert!(
            large.open_changes.y + large.open_changes.height < large.frame.y + large.frame.height
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
    /// 行高按内容公式化（工具摘要 36；用户卡额外包含 40px 内边距）。
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
        // 默认仅摘要；显式展开增加两条工具行，字号影响正文高度。
        assert_eq!(layouts[0].1, 102.0);
        assert_eq!(layouts[1].1, metrics::TOOL_GROUP_HEADER_HEIGHT);
        assert_eq!(layouts[2].1, 19.0);
        let expanded = std::collections::HashSet::from(["e2".to_string()]);
        assert_eq!(
            timeline_row_height(&rows[1], &timeline, 618.0, 16.0, &expanded, false),
            metrics::TOOL_GROUP_HEADER_HEIGHT + 2.0 * metrics::TOOL_ROW_HEIGHT
        );
        assert_eq!(layouts[1].0, metrics::TOOL_GROUP_TOP_GAP);
        assert_eq!(layouts[2].0, metrics::MSG_ENTRY_GAP);
        let tops = timeline_visible_item_tops(&layouts, 100.0, 400.0, 0, 0.0);
        assert_eq!(tops[0], (0, 100.0));
        for pair in tops.windows(2) {
            assert!(pair[0].1 + layouts[pair[0].0].1 <= pair[1].1);
        }
        assert_eq!(
            timeline_row_height(
                &rows[0],
                &timeline,
                618.0,
                20.0,
                &std::collections::HashSet::new(),
                false
            ),
            109.0
        );
        assert_eq!(timeline_following_window(&layouts, 400.0), (0, 0.0));
        let (start, offset) = timeline_following_window(&layouts, 80.0);
        assert!(start > 0 || offset > 0.0);
        let tail = timeline_visible_item_tops(&layouts, 100.0, 80.0, start, offset);
        assert_eq!(tail.last().map(|item| item.0), Some(2));
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

    /// UI-2：真实 hover / Tab / click 驱动非当前会话动作，不先打开会话。
    #[gpui::test]
    fn session_actions_follow_hover_and_keyboard_without_opening(cx: &mut gpui::TestAppContext) {
        use gpui::{prelude::*, px, AppContext, Modifiers};
        struct RailHost(gpui::Entity<AppView>);
        impl gpui::Render for RailHost {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let rail = self
                    .0
                    .update(cx, |view, cx| view.sidebar_element(px(288.0), window, cx));
                gpui::div()
                    .size_full()
                    .flex()
                    .child(rail)
                    .on_key_down(cx.listener(|host, event, window, cx| {
                        host.0
                            .update(cx, |view, cx| view.handle_root_key(event, window, cx));
                    }))
            }
        }
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (host, cx) = cx.add_window_view(|_, cx| {
            RailHost(cx.new(|cx| {
                AppView::new(
                    platform,
                    std::env::temp_dir().join("ui2-rail.sock"),
                    None,
                    cx,
                )
            }))
        });
        let view = cx.update(|_, cx| host.read(cx).0.clone());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.projection.sessions = ["current", "other"]
                    .map(|id| crate::projection::SessionSummary {
                        session_id: id.into(),
                        title: format!("{id} long session title"),
                        updated_at_ms: crate::ui::now_unix_ms(),
                        workspace_id: None,
                        parent_branch_id: None,
                        forked_from_event_id: None,
                        active: id == "current",
                    })
                    .into();
                view.projection.active_session_id = Some("current".into());
                window.focus(&view.scope_focus);
                cx.notify();
            })
        });
        cx.simulate_resize(gpui::size(px(1440.0), px(1024.0)));
        cx.refresh().unwrap();
        cx.run_until_parked();
        let row_bounds = cx.update(|window, cx| {
            let tree = view.read(cx).accessibility_tree(window, cx);
            assert!(tree.find(&session_rename_identifier("other")).is_none());
            tree.find(&session_identifier("other")).unwrap().bounds
        });
        cx.simulate_mouse_move(
            gpui::point(px(row_bounds.x + 30.0), px(row_bounds.y + 22.0)),
            None,
            Modifiers::none(),
        );
        cx.refresh().unwrap();
        cx.run_until_parked();
        let rename_bounds = cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            assert_eq!(
                view.projection.active_session_id.as_deref(),
                Some("current")
            );
            assert_eq!(
                tree.find(&session_identifier("other")).unwrap().bounds,
                row_bounds
            );
            let rename = tree
                .find(&session_rename_identifier("other"))
                .expect("hover reveals rename");
            assert!(rename.enabled);
            rename.bounds
        });
        cx.simulate_mouse_move(
            gpui::point(px(rename_bounds.x + 16.0), px(rename_bounds.y + 16.0)),
            None,
            Modifiers::none(),
        );
        cx.run_until_parked();
        cx.simulate_click(
            gpui::point(px(rename_bounds.x + 16.0), px(rename_bounds.y + 16.0)),
            Modifiers::none(),
        );
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.session_rename.as_ref().unwrap().session_id, "other");
                assert_eq!(
                    view.projection.active_session_id.as_deref(),
                    Some("current")
                );
                assert!(view
                    .accessibility_tree(window, cx)
                    .find(&session_archive_identifier("other"))
                    .is_none());
                view.cancel_session_rename(window, cx);
            })
        });
        cx.simulate_mouse_move(gpui::point(px(700.0), px(500.0)), None, Modifiers::none());
        cx.refresh().unwrap();
        cx.run_until_parked();
        // Cancel 回到该行；指针移走仍保持动作，Tab 可进入动作自身。
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let focus = &view.rail_row_focus[&rail_session_rename_focus_key("other")];
                assert!(
                    focus.is_focused(window),
                    "Tab reaches rename without opening the session"
                );
                assert!(view.session_actions_visible("other", window));
                view.projection
                    .set_connection(ConnectionState::Disconnected {
                        reason: "test".into(),
                    });
                let tree = view.accessibility_tree(window, cx);
                assert!(
                    !tree
                        .find(&session_rename_identifier("other"))
                        .unwrap()
                        .enabled
                );
                assert!(
                    !tree
                        .find(&session_archive_identifier("other"))
                        .unwrap()
                        .enabled
                );
                window.focus(&view.scope_focus);
                assert!(view
                    .accessibility_tree(window, cx)
                    .find(&session_rename_identifier("other"))
                    .is_none());
            })
        });
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

        // OPT-4b（F6）：Inspector 默认折叠；显式动作（Review changes /
        // Activity 摘要 / inspector-expand）仍可展开。
        cx.update(|_window, cx| {
            assert!(!view.read(cx).inspector_open);
        });

        // UI-1：折叠过渡中保留可见页签，部分可见的页签裁到窗口右缘；
        // 完全隐藏的操作（含旧 AX 客户端保留的对象）不可再调用。
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.inspector_render_width = 160.0;
                let tree = view.accessibility_tree(window, cx);
                let inspector = tree.find("inspector").unwrap();
                let workspace = tree.find("workspace").unwrap();
                let terminal = tree.find("inspector-tab-terminal").unwrap();
                assert_eq!(inspector.bounds.width, 160.0);
                assert_eq!(
                    workspace.bounds.x + workspace.bounds.width,
                    inspector.bounds.x
                );
                assert_eq!(
                    terminal.bounds.x + terminal.bounds.width,
                    tree.viewport.width
                );
                assert!(terminal.bounds.width < metrics::INSPECTOR_TAB_WIDTH);
                assert!(tree.find("inspector-tab-resources").is_none());
                assert!(!tree.permits(&AxRequest {
                    identifier: "inspector-collapse".into(),
                    action: AxAction::Press,
                    value: None,
                }));
                assert!(tree.find("inspector-expand").is_some());
                view.inspector_render_width = 0.0;
                assert!(view
                    .accessibility_tree(window, cx)
                    .find("inspector")
                    .is_none());
            });
        });

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

        // 恢复 All projects：scope 仍开菜单；两个全局 New Task 入口按
        // ADR-054 D1 直建无归属会话（无确认浮层）。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.scope_workspace_id = None;
                view.projection.active_session_id = None;
            });
        });
        for identifier in ["project-scope"] {
            cx.update(|window, cx| {
                // scope 菜单触发器同源注入 Connected（真实点击同理）。
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

        // ADR-054 D1：All projects 下 add-task / header-new-task 的 AX press
        // 与真实点击同路径直建无归属会话——不开任何确认菜单，流程收口
        // 后焦点回 Composer（新任务待输入）。测试宿主无连接 client，
        // create_session 走 not-connected 诚实失败通道（emit_reliable 在
        // 未连接时为 no-op），不伪造成功。
        for identifier in ["add-task", "header-new-task"] {
            cx.update(|window, cx| {
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
                    view.open_menu.is_none(),
                    "{identifier}: direct create must not open a confirm menu"
                );
                assert!(
                    view.composer_focus_handle(cx).is_focused(window),
                    "{identifier}: direct create hands focus to the composer"
                );
            });
        }
        // UI-3：思考只读详情默认收起；AX 与鼠标/键盘共用展开状态。
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.timeline.entries = vec![TimelineEntry {
                    sequence: 1,
                    event_id: "thinking-test".into(),
                    kind: TimelineEntryKind::Thinking {
                        text: "核对输入，再组织回答。".into(),
                    },
                    fork_boundary: None,
                    timestamp: "1".into(),
                    run_id: Some("run-1".into()),
                }];
                let row = view.projection.timeline_rows().remove(0);
                assert!(matches!(row, TimelineRow::Thinking { .. }));
                let rect = AxRect::new(300.0, 100.0, 560.0, metrics::TOOL_GROUP_HEADER_HEIGHT);
                let collapsed = view.timeline_row_ax(window, &row, rect);
                assert_eq!(
                    collapsed.children.len(),
                    1,
                    "collapsed thinking must not expose body"
                );
                assert_eq!(
                    timeline::timeline_row_height(
                        &row,
                        &view.projection.timeline,
                        rect.width,
                        16.0,
                        &view.expanded_timeline_details,
                        false
                    ),
                    rect.height
                );
                assert!(view.handle_accessibility_press(
                    "thinking-toggle-thinking-test",
                    window,
                    cx
                ));
                assert!(!view.timeline_following, "expansion preserves the viewport");
                let height = timeline::timeline_row_height(
                    &row,
                    &view.projection.timeline,
                    rect.width,
                    16.0,
                    &view.expanded_timeline_details,
                    false,
                );
                assert!(height > rect.height);
                let expanded = view.timeline_row_ax(window, &row, AxRect { height, ..rect });
                assert_eq!(expanded.children.len(), 2);
                assert_eq!(
                    expanded.children[1].value.as_deref(),
                    Some("核对输入，再组织回答。")
                );
                assert!(view.handle_accessibility_press(
                    "thinking-toggle-thinking-test",
                    window,
                    cx
                ));
                assert!(view.expanded_timeline_details.is_empty());
            });
        });
    }

    /// 多项目菜单使用实测滚动几何；键盘高亮不能停在视口之外。
    #[gpui::test]
    fn scope_menu_ax_follows_scrolling_and_keyboard_highlight(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (view, cx) = cx.add_window_view(|_window, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("opt4-scope.sock"),
                None,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.workspaces = (0..10)
                    .map(|ix| crate::projection::WorkspaceSummary {
                        id: format!("ws-{ix}"),
                        name: format!("Project {ix}"),
                    })
                    .collect();
                view.scope_workspace_id = Some("ws-9".into());
                view.on_toggle_scope_menu(None, window, cx);
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let tree = view.read(cx).accessibility_tree(window, cx);
            let menu = tree.find("scope-menu").unwrap();
            assert!(menu.bounds.height <= MENU_MAX_HEIGHT);
            let current = tree
                .find("scope-ws-9")
                .expect("current project visible on first open");
            assert!(current.selected);
            assert!(current.bounds.y + current.bounds.height <= menu.bounds.y + menu.bounds.height);
            assert!(tree.find("scope-all").is_none());
        });
        // 从当前项目向下到最后一项 Add project，必须滚入视口。
        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.move_menu_highlight(true);
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let tree = view.read(cx).accessibility_tree(window, cx);
            let menu = tree.find("scope-menu").unwrap();
            let add = tree
                .find("scope-add-project")
                .expect("last row scrolled into view");
            assert!(add.focused);
            assert!(add.bounds.y >= menu.bounds.y);
            assert!(add.bounds.y + add.bounds.height <= menu.bounds.y + menu.bounds.height);
            assert!(tree.find("scope-all").is_none());
        });
        // 与滚轮同源修改 offset，原点回到顶部后 AX 也恢复首项。
        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.scope_menu_scroll
                    .set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
                cx.notify();
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let tree = view.read(cx).accessibility_tree(window, cx);
            assert!(tree.find("scope-all").is_some());
            assert!(tree.find("scope-add-project").is_none());
        });
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
                self.view.clone()
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
            // P2-1：Settings 壳不发布 RunStatusBar。
            assert!(tree.find("status-bar").is_none());
            assert!(tree.find("run-status").is_none());
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
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(view.settings_page, SettingsPage::Appearance);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("appearance page AX tree validates");
            let scale_100 = tree.find("settings-text-scale-100").unwrap();
            let scale_125 = tree.find("settings-text-scale-125").unwrap();
            let scale_150 = tree.find("settings-text-scale-150").unwrap();
            let title = tree.find("settings-page-title").unwrap();
            assert_eq!(scale_100.bounds.x, title.bounds.x);
            assert_eq!(
                tree.find("settings-language-en").unwrap().bounds.x,
                title.bounds.x
            );
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
        cx.refresh().unwrap();
        cx.run_until_parked();
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
        use gpui::{prelude::*, px, AppContext};

        struct AxSettingsHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxSettingsHost {
            fn render(
                &mut self,
                window: &mut Window,
                cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                window.set_rem_size(px(16.0));
                gpui::div()
                    .size_full()
                    .flex()
                    .child(self.view.update(cx, |view, cx| {
                        view.ensure_settings_api_key_inputs(cx);
                        view.settings_page_element(cx)
                    }))
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
        // Host 直接装配子 view 的元素，须在每次验收前通知 Host 重绘；
        // 单独 refresh window 不会使已缓存的 Host render 失效。
        // 语义断言覆盖完整页面；窄窗与菜单裁剪另在本测试内实际滚动。
        cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
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
                                credentials: Vec::new(),
                                selection_mode: Default::default(),
                                auth: crate::projection::ProviderAuthState::None,
                                catalog: crate::projection::ProviderCatalogState::Unavailable {
                                    error: "offline".into(),
                                    fetched_at: None,
                                },
                                use_proxy: true,
                            },
                            crate::projection::ProviderAuthStatusEntry {
                                provider_id: "connected".into(),
                                display_name: "Connected provider".into(),
                                endpoint_label: "https://provider.example".into(),
                                auth_methods: vec!["api_key".into()],
                                credentials: Vec::new(),
                                selection_mode: Default::default(),
                                auth: crate::projection::ProviderAuthState::Connected {
                                    method: "api_key".into(),
                                    masked_credential: Some("masked-fragment-sentinel".into()),
                                },
                                catalog: crate::projection::ProviderCatalogState::FixedFallback {
                                    snapshot_label: "test@v1".into(),
                                    fetched_at: None,
                                },
                                use_proxy: true,
                            },
                        ],
                        default: None,
                        role_defaults: Default::default(),
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
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
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
            // 名称、连接与目录的 AX 框直接对照本次 prepaint 的真实元素。
            for prefix in [
                "settings-provider-name",
                "settings-provider-connection",
                "settings-provider-catalog",
            ] {
                let id = dynamic_identifier(prefix, "connected");
                let actual = view.settings_element_layouts[&id].bounds();
                let node = tree
                    .find(&id)
                    .expect("rendered overview text has an AX node");
                assert_eq!(
                    node.bounds,
                    AxRect::new(
                        actual.origin.x.into(),
                        actual.origin.y.into(),
                        actual.size.width.into(),
                        actual.size.height.into()
                    )
                );
            }
            let input = tree.find(&input_id).expect("secure input has an AX node");
            assert_eq!(input.role, AxRole::TextArea);
            assert_eq!(input.value.as_deref(), Some(expected_mask.as_str()));
            assert!(input.enabled);
            let verify = tree.find(&verify_id).expect("verify button has an AX node");
            assert!(verify.enabled);
            for id in [&input_id, &verify_id] {
                let actual = view.settings_element_layouts[id].bounds();
                let node = tree.find(id).expect("rendered auth control");
                assert_eq!(
                    node.bounds,
                    AxRect::new(
                        actual.origin.x.into(),
                        actual.origin.y.into(),
                        actual.size.width.into(),
                        actual.size.height.into()
                    )
                );
            }
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
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
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
                            enabled: true,
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

    /// OPT-3 切片 1：全关空态的诚实呈现——加载中不得误报「无已启用
    /// 模型」；目录查询完成且为空时 picker 仍可开菜单看说明行、发送
    /// fail-closed，且不发布任何可选模型行。
    #[gpui::test]
    fn model_menu_empty_state_is_honest_and_fail_closed(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;

        struct AxEmptyHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxEmptyHost {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div()
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("opt3-model-menu-empty.sock");
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            AxEmptyHost { view }
        });
        let view = cx.update(|_window, cx| host.read(cx).view.clone());
        // 已连接 + 激活会话，但目录查询未完成：保持 loading 语义。
        cx.update(|_window, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.projection.active_session_id = Some("s-1".into());
                view.text_input
                    .update(cx, |input, cx| input.reset_text("hello", cx));
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("loading AX tree validates");
            let picker = tree
                .find("model-picker")
                .expect("model picker has an AX node");
            assert!(!picker.enabled);
            assert_eq!(picker.value.as_deref(), Some(t("composer.model_loading")));
            // 加载中不误报全关：发送仍可用（现有行为，Host 侧权威默认）。
            assert!(tree.find("send").expect("send has an AX node").enabled);
        });
        // 目录查询完成且结果为空：进入全关空态。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection.set_models(Vec::new());
                view.open_menu = Some(MenuKind::Model);
            });
        });
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("empty-state AX tree validates");
            let picker = tree
                .find("model-picker")
                .expect("model picker has an AX node");
            assert!(picker.enabled);
            assert_eq!(picker.value.as_deref(), Some("No enabled models"));
            assert!(tree.permits(&AxRequest {
                identifier: "model-picker".into(),
                action: AxAction::Press,
                value: None,
            }));
            // 全关空态 fail-closed：无已启用模型不发送。
            let send = tree.find("send").expect("send has an AX node");
            assert!(!send.enabled);
            assert!(!tree.permits(&AxRequest {
                identifier: "send".into(),
                action: AxAction::Press,
                value: None,
            }));
            let menu = tree.find("model-menu").expect("model menu has an AX node");
            let empty = tree
                .find("model-menu-empty")
                .expect("empty menu publishes its explanation");
            assert_eq!(empty.role, AxRole::StaticText);
            // 说明行不发布 Press（disabled 节点无 Press 契约）。
            assert!(empty.actions.is_empty());
            assert_eq!(empty.value.as_deref(), Some(t("composer.model_menu_empty")));
            // 菜单不发布任何可选模型行：不编造模型。
            assert_eq!(menu.children.len(), 1);
            assert!(menu
                .children
                .iter()
                .all(|child| child.role != AxRole::Button));
        });
    }

    /// OPT-3b / ADR-055 D5：Default models 四角色区 AX 形状——四个触发器
    /// identifier / role / label / value 稳定，菜单发布清除与已连接
    /// provider 候选（未连接组不出现），Vision / Search 如实标注只保存；
    /// stale 时触发器禁用且 Press 被拒。
    #[gpui::test]
    fn settings_role_defaults_ax_shape_pins_triggers_menu_and_filter(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{prelude::*, px, AppContext};

        use crate::projection::{
            ProviderAuthState, ProviderAuthStatusEntry, ProviderCatalogState, SettingsRole,
        };
        use crate::ui::settings::{
            settings_role_clear_identifier, settings_role_item_identifier,
            settings_role_trigger_identifier,
        };

        struct AxRolesHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxRolesHost {
            fn render(
                &mut self,
                window: &mut Window,
                cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                window.set_rem_size(px(16.0));
                gpui::div()
                    .size_full()
                    .flex()
                    .child(self.view.update(cx, |view, cx| {
                        view.ensure_settings_api_key_inputs(cx);
                        view.settings_page_element(cx)
                    }))
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("opt3b-role-defaults-ax.sock");
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            AxRolesHost { view }
        });
        // Host 直接装配子 view 的元素，须在每次验收前通知 Host 重绘；
        // 单独 refresh window 不会使已缓存的 Host render 失效。
        // 语义断言覆盖完整页面；窄窗与菜单裁剪另在本测试内实际滚动。
        cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
        let view = cx.update(|_window, cx| host.read(cx).view.clone());
        let provider = |provider_id: &str, auth: ProviderAuthState| ProviderAuthStatusEntry {
            provider_id: provider_id.to_string(),
            display_name: provider_id.to_string(),
            endpoint_label: String::new(),
            auth_methods: vec!["api_key".to_string()],
            credentials: Vec::new(),
            selection_mode: Default::default(),
            auth,
            catalog: ProviderCatalogState::Unavailable {
                error: "offline".to_string(),
                fetched_at: None,
            },
            use_proxy: true,
        };
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.route = AppRoute::Settings;
                view.projection.settings_providers.apply_loaded(
                    crate::projection::ProviderAuthStatusData {
                        providers: vec![
                            provider(
                                "kimi",
                                ProviderAuthState::Connected {
                                    method: "api_key".to_string(),
                                    masked_credential: None,
                                },
                            ),
                            provider("glm", ProviderAuthState::None),
                        ],
                        default: None,
                        role_defaults: Default::default(),
                    },
                );
                view.projection.set_models(vec![
                    ModelEntry {
                        provider_id: "kimi".into(),
                        id: "kimi-k2".into(),
                        display_name: "Kimi K2".into(),
                        context_window_tokens: None,
                        enabled: true,
                    },
                    ModelEntry {
                        provider_id: "glm".into(),
                        id: "glm-4.7".into(),
                        display_name: "GLM 4.7".into(),
                        context_window_tokens: None,
                        enabled: true,
                    },
                    ModelEntry {
                        provider_id: "ghost".into(),
                        id: "ghost-x".into(),
                        display_name: "Ghost X".into(),
                        context_window_tokens: None,
                        enabled: true,
                    },
                ]);
                view.open_menu = Some(MenuKind::SettingsRole(SettingsRole::Naming));
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);

            tree.validate().expect("role defaults AX tree validates");
            for role in SettingsRole::ALL {
                let identifier = settings_role_trigger_identifier(role);
                let trigger = tree
                    .find(&identifier)
                    .unwrap_or_else(|| panic!("{identifier} missing from AX tree"));
                assert_eq!(trigger.role, AxRole::Button);
                assert_eq!(trigger.label, role.label());
                assert_eq!(trigger.value.as_deref(), Some("Not set"));
                assert!(trigger.enabled);
                // 与 render 同源：四角色区位于 Providers 列表之上。
                let heading = tree
                    .find("settings-providers-heading")
                    .expect("providers heading has an AX node");
                assert!(
                    trigger.bounds.y < heading.bounds.y,
                    "{identifier} must sit above the providers list"
                );
                assert!(tree.permits(&AxRequest {
                    identifier: identifier.clone(),
                    action: AxAction::Press,
                    value: None,
                }));
            }
            // Vision / Search 落地期只保存：说明如实标注，不暗示已生效。
            let vision = tree
                .find(&settings_role_trigger_identifier(SettingsRole::Vision))
                .expect("vision trigger has an AX node");
            assert!(vision
                .description
                .as_deref()
                .is_some_and(|description| description.contains("image routing")));
            let search = tree
                .find(&settings_role_trigger_identifier(SettingsRole::Search))
                .expect("search trigger has an AX node");
            assert!(search
                .description
                .as_deref()
                .is_some_and(|description| description.contains("search routing")));

            // 菜单展开：清除行 + 已连接 provider 候选可选。
            let clear_id = settings_role_clear_identifier(SettingsRole::Naming);
            let clear = tree.find(&clear_id).expect("clear row has an AX node");
            assert_eq!(clear.label, "Clear");
            assert!(tree.permits(&AxRequest {
                identifier: clear_id,
                action: AxAction::Press,
                value: None,
            }));
            let item_id = settings_role_item_identifier(SettingsRole::Naming, "kimi", "kimi-k2");
            let item = tree.find(&item_id).expect("kimi item has an AX node");
            assert_eq!(item.value.as_deref(), Some("kimi / kimi-k2"));
            assert!(tree.permits(&AxRequest {
                identifier: item_id,
                action: AxAction::Press,
                value: None,
            }));
            // 未连接 provider（glm）与清单缺失 provider（ghost）不出现。
            assert!(tree
                .find(&settings_role_item_identifier(
                    SettingsRole::Naming,
                    "glm",
                    "glm-4.7"
                ))
                .is_none());
            assert!(tree
                .find(&settings_role_item_identifier(
                    SettingsRole::Naming,
                    "ghost",
                    "ghost-x"
                ))
                .is_none());
        });
        // 两个供应商分组的长菜单：打开时当前末项可见，上下键跨越
        // Clear / 分组边界时新高亮项始终可见，AX 框仍来自真实菜单视口。
        let original_models = cx.update(|_, cx| view.read(cx).projection.models.clone());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.settings_providers.providers[1].auth =
                    ProviderAuthState::Connected {
                        method: "api_key".into(),
                        masked_credential: None,
                    };
                let models = ["kimi", "glm"]
                    .into_iter()
                    .flat_map(|provider| {
                        (0..12).map(move |ix| ModelEntry {
                            provider_id: provider.into(),
                            id: format!("long-{ix}"),
                            display_name: format!("Model {ix}"),
                            context_window_tokens: None,
                            enabled: true,
                        })
                    })
                    .collect();
                view.projection.set_models(models);
                view.projection.settings_providers.confirm_role_default(
                    SettingsRole::Naming,
                    Some(("glm".into(), "long-11".into())),
                );
                view.open_menu = None;
                view.on_toggle_settings_role_menu(SettingsRole::Naming, None, window, cx);
            });
        });
        // 首帧建立视口；defer 定位当前项，第二帧执行滚动。
        for _ in 0..2 {
            cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
            cx.refresh().unwrap();
            cx.run_until_parked();
        }
        let last_id = settings_role_item_identifier(SettingsRole::Naming, "glm", "long-11");
        let clear_id = settings_role_clear_identifier(SettingsRole::Naming);
        let menu_id = "settings-role-menu-naming";
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            let last = tree
                .find(&last_id)
                .expect("current last model scrolls into view on open");
            assert!(last.selected && last.focused);
            assert!(tree.find(&clear_id).is_none());
        });
        for (forward, expected_id) in [
            (true, clear_id.clone()), // last → Clear
            (false, last_id.clone()), // Clear → last
            (
                false,
                settings_role_item_identifier(SettingsRole::Naming, "glm", "long-10"),
            ),
        ] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.move_menu_highlight(forward);
                    cx.notify();
                })
            });
            cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
            cx.refresh().unwrap();
            cx.run_until_parked();
            cx.update(|window, cx| {
                let view = view.read(cx);
                let tree = view.accessibility_tree(window, cx);
                let node = tree
                    .find(&expected_id)
                    .expect("keyboard highlight remains visible");
                assert!(node.focused);
                let actual = view.settings_element_layouts[&expected_id].bounds();
                let menu = view.settings_element_layouts[menu_id].bounds();
                assert!(actual.top() >= menu.top() && actual.bottom() <= menu.bottom());
                assert_eq!(
                    node.bounds,
                    AxRect::new(
                        actual.origin.x.into(),
                        actual.origin.y.into(),
                        actual.size.width.into(),
                        actual.size.height.into()
                    )
                );
            });
        }
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_models(original_models);
                view.projection.settings_providers.providers[1].auth = ProviderAuthState::None;
                view.open_menu = None;
                view.settings_element_layouts.remove(menu_id);
                cx.notify();
            })
        });

        // 清除路径分别从 AX 和键盘进入，每次先渲染对应的真实空菜单。
        let role = SettingsRole::Naming;
        let saved = Some(("kimi".to_string(), "kimi-k2".to_string()));
        for empty_catalog in [false, true] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    if empty_catalog {
                        view.projection.set_models(vec![]);
                        view.projection.settings_providers.providers[0].auth =
                            ProviderAuthState::Connected {
                                method: "api_key".into(),
                                masked_credential: None,
                            };
                    } else {
                        view.projection.settings_providers.providers[0].auth =
                            ProviderAuthState::None;
                    }
                    view.projection
                        .settings_providers
                        .confirm_role_default(role, saved.clone());
                    view.settings_role_pending = None;
                    view.open_menu = Some(MenuKind::SettingsRole(role));
                    view.menu_highlight = None;
                    assert_eq!(view.menu_item_count(), 1);
                    view.move_menu_highlight(true);
                    assert_eq!(view.menu_highlight, Some(0));
                    cx.notify();
                });
            });
            cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
            cx.refresh().unwrap();
            cx.run_until_parked();
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let tree = view.accessibility_tree(window, cx);
                    tree.validate().expect("empty role menu AX tree validates");
                    let clear_id = settings_role_clear_identifier(role);
                    let clear = tree
                        .find(&clear_id)
                        .expect("Clear survives empty candidates");
                    let empty = tree
                        .find("settings-role-menu-empty-naming")
                        .expect("empty candidates retain the explanation");
                    assert!(empty.bounds.y >= clear.bounds.y + clear.bounds.height);
                    let request = AxRequest {
                        identifier: clear_id,
                        action: AxAction::Press,
                        value: None,
                    };
                    assert!(tree.permits(&request));
                    if empty_catalog {
                        view.activate_menu_item(MenuKind::SettingsRole(role), 0, window, cx);
                    } else {
                        view.handle_accessibility_request(request, window, cx);
                    }
                    assert_eq!(view.settings_role_pending, Some(role));
                    assert!(view.open_menu.is_none());
                    assert_eq!(
                        view.projection.settings_providers.role_value(role),
                        saved.as_ref()
                    );
                });
            });
        }
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.settings_role_pending = None;
                view.open_menu = Some(MenuKind::SettingsRole(role));
                cx.notify();
            });
        });
        // 断线 stale：角色写禁用，Press fail-closed。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection
                    .settings_providers
                    .mark_stale("socket closed");
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            let identifier = settings_role_trigger_identifier(SettingsRole::Naming);
            let trigger = tree
                .find(&identifier)
                .expect("naming trigger still present");
            assert!(!trigger.enabled);
            assert!(!tree.permits(&AxRequest {
                identifier,
                action: AxAction::Press,
                value: None,
            }));
        });
    }

    /// OPT-3a / ADR-055 D2-D3：Manage models 弹层与代理 Switch 的 AX 形
    /// 状——Manage 入口 gate（未连接禁用不发布 Press）、弹层 Group + 每模
    /// 型 Switch（checked 进 value/selected）、Enable/Disable all 与空目
    /// 录 Refresh 的可操作性、代理 Switch value 与 Press；stale 关闸。
    #[gpui::test]
    fn settings_models_menu_ax_pins_gates_switches_and_empty_state(cx: &mut gpui::TestAppContext) {
        use gpui::{prelude::*, px, AppContext};

        use crate::projection::{
            ModelEntry, ProviderAuthState, ProviderAuthStatusEntry, ProviderCatalogState,
        };
        use crate::ui::settings::{
            settings_manage_models_identifier, settings_model_switch_identifier,
            settings_models_disable_all_identifier, settings_models_enable_all_identifier,
            settings_models_menu_identifier, settings_models_refresh_identifier,
            settings_use_proxy_identifier,
        };

        struct AxModelsHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxModelsHost {
            fn render(
                &mut self,
                window: &mut Window,
                cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                window.set_rem_size(px(16.0));
                gpui::div()
                    .size_full()
                    .flex()
                    .child(self.view.update(cx, |view, cx| {
                        view.ensure_settings_api_key_inputs(cx);
                        view.settings_page_element(cx)
                    }))
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("opt3a-models-menu-ax.sock");
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            AxModelsHost { view }
        });
        // Host 直接装配子 view 的元素，须在每次验收前通知 Host 重绘；
        // 单独 refresh window 不会使已缓存的 Host render 失效。
        // 语义断言覆盖完整页面；窄窗与菜单裁剪另在本测试内实际滚动。
        cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
        let view = cx.update(|_window, cx| host.read(cx).view.clone());
        let catalog_remote = || ProviderCatalogState::Remote {
            fetched_at: "2026-09-05T00:00:00Z".into(),
        };
        let provider = |provider_id: &str, auth: ProviderAuthState| ProviderAuthStatusEntry {
            provider_id: provider_id.to_string(),
            display_name: provider_id.to_string(),
            endpoint_label: String::new(),
            auth_methods: vec!["api_key".to_string()],
            credentials: Vec::new(),
            selection_mode: Default::default(),
            auth,
            catalog: catalog_remote(),
            use_proxy: true,
        };
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.route = AppRoute::Settings;
                view.projection.settings_providers.apply_loaded(
                    crate::projection::ProviderAuthStatusData {
                        providers: vec![
                            provider(
                                "kimi",
                                ProviderAuthState::Connected {
                                    method: "api_key".to_string(),
                                    masked_credential: None,
                                },
                            ),
                            provider("glm", ProviderAuthState::None),
                            provider(
                                "empty",
                                ProviderAuthState::Connected {
                                    method: "api_key".to_string(),
                                    masked_credential: None,
                                },
                            ),
                        ],
                        default: None,
                        role_defaults: Default::default(),
                    },
                );
                view.projection.settings_providers.apply_model_catalog(vec![
                    ModelEntry {
                        provider_id: "kimi".into(),
                        id: "kimi-k2".into(),
                        display_name: "Kimi K2".into(),
                        context_window_tokens: None,
                        enabled: true,
                    },
                    ModelEntry {
                        provider_id: "kimi".into(),
                        id: "kimi-k2-thinking".into(),
                        display_name: "Kimi K2 Thinking".into(),
                        context_window_tokens: None,
                        enabled: false,
                    },
                ]);
                view.projection.settings_general.proxy_url = Some("http://127.0.0.1:7890".into());
                // ADR-056 D4：Manage models / 代理 Switch 迁入展开区——
                // 断言前先显式展开三张卡（chevron 状态存 projection）。
                view.projection
                    .settings_providers
                    .expanded_providers
                    .extend(["kimi", "glm", "empty"].map(String::from));
                view.open_menu = Some(MenuKind::SettingsProviderModels("kimi".into()));
            });
        });
        let press = |identifier: String| AxRequest {
            identifier,
            action: AxAction::Press,
            value: None,
        };
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);

            tree.validate().expect("models menu AX tree validates");
            // 已连接 + 目录可用：Manage 可按；未连接禁用且不发布 Press。
            let manage = tree
                .find(&settings_manage_models_identifier("kimi"))
                .expect("kimi manage trigger has an AX node");
            assert!(manage.enabled);
            assert!(tree.permits(&press(settings_manage_models_identifier("kimi"))));
            let glm_manage = tree
                .find(&settings_manage_models_identifier("glm"))
                .expect("glm manage trigger has an AX node");
            assert!(!glm_manage.enabled);
            assert!(!tree.permits(&press(settings_manage_models_identifier("glm"))));

            // 弹层：Group + 每模型 Switch（value/selected 与 enabled 同源）
            // + Enable all / Disable all 可按。
            let menu = tree
                .find(&settings_models_menu_identifier("kimi"))
                .expect("models menu group has an AX node");
            assert_eq!(menu.role, AxRole::Group);
            let on = tree
                .find(&settings_model_switch_identifier("kimi", "kimi-k2"))
                .expect("kimi-k2 switch has an AX node");
            assert_eq!(on.value.as_deref(), Some("On"));
            assert!(on.selected);
            assert!(on.enabled);
            assert!(tree.permits(&press(settings_model_switch_identifier("kimi", "kimi-k2"))));
            let off = tree
                .find(&settings_model_switch_identifier(
                    "kimi",
                    "kimi-k2-thinking",
                ))
                .expect("kimi-k2-thinking switch has an AX node");
            assert_eq!(off.value.as_deref(), Some("Off"));
            assert!(!off.selected);
            assert!(tree.permits(&press(settings_model_switch_identifier(
                "kimi",
                "kimi-k2-thinking"
            ))));
            for identifier in [
                settings_models_enable_all_identifier("kimi"),
                settings_models_disable_all_identifier("kimi"),
            ] {
                let button = tree
                    .find(&identifier)
                    .unwrap_or_else(|| panic!("{identifier} missing from AX tree"));
                assert!(button.enabled);
                assert!(tree.permits(&press(identifier)));
            }

            // 代理 Switch（OPT-3c）：全局 proxy_url 已配置时出现，value
            // On / selected=true / 可按。
            let proxy = tree
                .find(&settings_use_proxy_identifier("kimi"))
                .expect("kimi proxy switch has an AX node");
            assert_eq!(proxy.value.as_deref(), Some("On"));
            assert!(proxy.selected);
            assert!(proxy.enabled);
            assert!(tree.permits(&press(settings_use_proxy_identifier("kimi"))));
        });

        // 目录超过菜单视口：真滚动后 AX 只发布当前可见 Switch，重绘不归零。
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let mut catalog = view.projection.settings_providers.model_catalog.clone();
                catalog.extend((0..16).map(|ix| ModelEntry {
                    provider_id: "kimi".into(),
                    id: format!("scroll-{ix}"),
                    display_name: format!("Scroll model {ix}"),
                    context_window_tokens: None,
                    enabled: true,
                }));
                view.projection
                    .settings_providers
                    .apply_model_catalog(catalog);
                cx.notify();
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        let last_id = settings_model_switch_identifier("kimi", "scroll-15");
        let menu_id = settings_models_menu_identifier("kimi");
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert!(
                    tree.find(&last_id).is_none(),
                    "offscreen last model has no AX action"
                );
                view.settings_element_layouts[&menu_id].scroll_to_bottom();
                cx.notify();
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        let scrolled_offset = cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            assert!(tree
                .find(&settings_model_switch_identifier("kimi", "kimi-k2"))
                .is_none());
            let last = tree.find(&last_id).expect("last model scrolls into view");
            let viewport = view.settings_element_layouts[&menu_id].bounds();
            let actual = view.settings_element_layouts[&last_id]
                .bounds()
                .intersect(&viewport);
            assert_eq!(
                last.bounds,
                AxRect::new(
                    actual.origin.x.into(),
                    actual.origin.y.into(),
                    actual.size.width.into(),
                    actual.size.height.into()
                )
            );
            assert!(tree.permits(&press(last_id.clone())));
            let offset = view.settings_element_layouts[&menu_id].offset();
            assert!(offset.y < px(0.0));
            offset
        });
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(
                view.read(cx).settings_element_layouts[&menu_id].offset(),
                scrolled_offset
            )
        });

        // 空目录弹层（connected + catalog 可用但目录无条目）：诚实空态 +
        // Refresh 可按；Enable / Disable all 禁用不发布 Press。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.open_menu = Some(MenuKind::SettingsProviderModels("empty".into()));
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("empty menu AX tree validates");
            let empty = tree
                .find("settings-models-empty-empty")
                .expect("empty catalog note has an AX node");
            assert_eq!(
                empty.value.as_deref(),
                Some("This provider's catalog is empty.")
            );
            let refresh = tree
                .find(&settings_models_refresh_identifier("empty"))
                .expect("refresh button has an AX node");
            assert!(refresh.enabled);
            assert!(tree.permits(&press(settings_models_refresh_identifier("empty"))));
            for identifier in [
                settings_models_enable_all_identifier("empty"),
                settings_models_disable_all_identifier("empty"),
            ] {
                let button = tree
                    .find(&identifier)
                    .unwrap_or_else(|| panic!("{identifier} missing from AX tree"));
                assert!(!button.enabled);
                assert!(!tree.permits(&press(identifier)));
            }
        });

        // 断线 stale：Manage 与弹层开关全部关闸，Press fail-closed。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection
                    .settings_providers
                    .mark_stale("socket closed");
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            let manage = tree
                .find(&settings_manage_models_identifier("kimi"))
                .expect("kimi manage trigger still present");
            assert!(!manage.enabled);
            assert!(!tree.permits(&press(settings_manage_models_identifier("kimi"))));
            let proxy = tree
                .find(&settings_use_proxy_identifier("kimi"))
                .expect("kimi proxy switch still present");
            assert!(!proxy.enabled);
            assert!(!tree.permits(&press(settings_use_proxy_identifier("kimi"))));
        });
    }

    /// ADR-056 D4/D5：展开卡 AX——默认折叠只有 chevron（value Collapsed）；
    /// chevron Press 展开后 Credentials 行（kind + masked + Connected /
    /// Expired）与 Usage 行（恒「Usage unavailable」，无数字）入树；空
    /// 凭证列表给诚实空态；proxy_url 未配置时不发布代理 Switch。
    #[gpui::test]
    fn settings_provider_expanded_card_ax_pins_credentials_and_usage(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{prelude::*, px, AppContext};

        use crate::projection::ProviderAuthStatusEntry;
        use crate::ui::settings::{
            provider_credential_kind_label, settings_provider_expand_identifier,
            settings_use_proxy_identifier,
        };

        struct AxExpandedHost {
            view: gpui::Entity<AppView>,
        }
        impl gpui::Render for AxExpandedHost {
            fn render(
                &mut self,
                window: &mut Window,
                cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                window.set_rem_size(px(self.view.read(cx).text_scale.rem_pixels()));
                gpui::div()
                    .size_full()
                    .flex()
                    .child(self.view.update(cx, |view, cx| {
                        view.ensure_settings_api_key_inputs(cx);
                        view.settings_page_element(cx)
                    }))
            }
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("opt3d-expanded-card-ax.sock");
        let (host, cx) = cx.add_window_view(|_window, cx| {
            let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
            AxExpandedHost { view }
        });
        // Host 直接装配子 view 的元素，须在每次验收前通知 Host 重绘；
        // 单独 refresh window 不会使已缓存的 Host render 失效。
        // 语义断言覆盖完整页面；窄窗与菜单裁剪另在本测试内实际滚动。
        cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
        let view = cx.update(|_window, cx| host.read(cx).view.clone());
        // ProviderCredentialStatus 未在 pawork-client re-export 面上，测试
        // 经 wire JSON 解码构造（与投影层 fail-closed 解析同一路径）。
        let provider = |provider_id: &str, credentials: serde_json::Value| {
            serde_json::from_value::<ProviderAuthStatusEntry>(serde_json::json!({
                "provider_id": provider_id,
                "display_name": provider_id,
                "endpoint_label": "",
                "auth_methods": ["api_key"],
                "credentials": credentials,
                "auth": { "type": "connected", "method": "api_key",
                          "masked_credential": null },
                "catalog": { "type": "remote",
                             "fetched_at": "2026-09-05T00:00:00Z" },
                "use_proxy": true,
            }))
            .expect("decode ProviderAuthStatusEntry")
        };
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.route = AppRoute::Settings;
                view.text_scale = crate::ui::theme::font::TextScale::Percent100;
                view.projection.settings_providers.apply_loaded(
                    crate::projection::ProviderAuthStatusData {
                        providers: vec![
                            provider(
                                "dual",
                                serde_json::json!([
                                    { "kind": "api_key", "masked_credential": "sk-…ab12",
                                      "expired": false, "expires_at": null },
                                    { "kind": "oauth", "masked_credential": "oauth-…wxyz",
                                      "expired": true,
                                      "expires_at": "2026-09-01T00:00:00Z" },
                                ]),
                            ),
                            provider("empty", serde_json::json!([])),
                        ],
                        default: None,
                        role_defaults: Default::default(),
                    },
                );
            });
        });
        let expand_dual = settings_provider_expand_identifier("dual");
        let expand_empty = settings_provider_expand_identifier("empty");

        // 默认折叠：chevron 常驻（value Collapsed、可按），展开区节点
        // （Credentials / Usage / 代理 Switch）不入树。
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);

            tree.validate().expect("collapsed cards AX tree validates");
            let chevron = tree
                .find(&expand_dual)
                .expect("collapsed card pins its chevron");
            assert_eq!(chevron.value.as_deref(), Some("Collapsed"));
            assert!(chevron.enabled);
            assert!(tree.permits(&AxRequest {
                identifier: expand_dual.clone(),
                action: AxAction::Press,
                value: None,
            }));
            assert!(tree
                .find(&dynamic_identifier("settings-provider-credentials", "dual"))
                .is_none());
            assert!(tree
                .find(&dynamic_identifier("settings-provider-usage", "dual"))
                .is_none());
            assert!(tree.find(&settings_use_proxy_identifier("dual")).is_none());
        });

        // chevron Press（AX 与 render / 键盘同入口）：dual 展开。
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: expand_dual.clone(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().expect("expanded card AX tree validates");
            let chevron = tree
                .find(&expand_dual)
                .expect("expanded card keeps its chevron");
            assert_eq!(chevron.value.as_deref(), Some("Expanded"));
            let credentials = tree
                .find(&dynamic_identifier("settings-provider-credentials", "dual"))
                .expect("expanded card pins the credentials group");
            assert_eq!(credentials.label, "Credentials");
            // 区头部标题 / 副标题入 AX label/value（同 manage 行：标题
            // 文案面非省略的回归钉板；render 截断 gpui 测试体系不可观测）。
            let credentials_header = tree
                .find(&dynamic_identifier(
                    "settings-provider-credentials-header",
                    "dual",
                ))
                .expect("expanded card pins the credentials header text");
            assert_eq!(
                credentials_header.label,
                t("settings.providers.credentials_title")
            );
            assert_eq!(
                credentials_header.value.as_deref(),
                Some(t("settings.providers.credentials_subtitle"))
            );
            // 凭证行：kind + masked + 状态词；api_key 在前（Host 固定序）。
            let api_key = tree
                .find(&dynamic_identifier(
                    "settings-provider-credential-0",
                    "dual",
                ))
                .expect("api_key credential row pinned");
            assert_eq!(api_key.label, provider_credential_kind_label("api_key"));
            assert_eq!(
                api_key.value.as_deref(),
                Some("API key · sk-…ab12 · Connected")
            );
            let oauth = tree
                .find(&dynamic_identifier(
                    "settings-provider-credential-1",
                    "dual",
                ))
                .expect("oauth credential row pinned");
            assert_eq!(oauth.label, provider_credential_kind_label("oauth"));
            assert_eq!(
                oauth.value.as_deref(),
                Some("OAuth · oauth-…wxyz · Expired")
            );
            // Usage 行：固定槽位 + 诚实空态，value 不含任何数字 / 百分比。
            let usage = tree
                .find(&dynamic_identifier("settings-provider-usage", "dual"))
                .expect("expanded card pins the usage row");
            assert_eq!(usage.label, "Usage");
            assert_eq!(usage.value.as_deref(), Some("Usage unavailable"));
            // 展开区行标题 / 副标题入 AX value（标题文案面非省略的回归
            // 钉板；render 文本截断 gpui 测试体系不可观测，见 providers.rs
            // min_w_0 注释）。
            let manage_text = tree
                .find(&dynamic_identifier("settings-provider-manage-text", "dual"))
                .expect("expanded card pins the manage models row text");
            assert_eq!(manage_text.label, t("settings.providers.manage_models"));
            assert_eq!(
                manage_text.value.as_deref(),
                Some(t("settings.providers.catalog_scope"))
            );
            // proxy_url 未配置：代理 Switch 不发布（gate 与 render 同源）。
            assert!(tree
                .find(&dynamic_identifier("settings-provider-proxy-text", "dual"))
                .is_none());
            assert!(tree.find(&settings_use_proxy_identifier("dual")).is_none());
            // 未展开的 empty 卡仍只有 chevron。
            assert!(tree
                .find(&dynamic_identifier(
                    "settings-provider-credentials-empty",
                    "empty"
                ))
                .is_none());
        });

        // empty 卡展开：空凭证列表发布诚实空态，不渲染假行。
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: expand_empty.clone(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate()
                .expect("empty expanded card AX tree validates");
            let empty = tree
                .find(&dynamic_identifier(
                    "settings-provider-credentials-empty",
                    "empty",
                ))
                .expect("empty credentials state pinned");
            assert_eq!(empty.value.as_deref(), Some("No stored credentials"));
            assert!(tree
                .find(&dynamic_identifier(
                    "settings-provider-credential-0",
                    "empty"
                ))
                .is_none());
        });

        // 配置全局 proxy_url 后：Proxy 行文本（标题 + 副标题）与 Switch
        // 随 gate 打开发布（与 render 同源）。
        cx.update(|_window, cx| {
            view.update(cx, |view, _cx| {
                view.projection
                    .settings_general
                    .apply_loaded(pawork_client::GeneralSettingsData {
                        proxy_url: Some("http://127.0.0.1:7890".into()),
                    });
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            let proxy_text = tree
                .find(&dynamic_identifier("settings-provider-proxy-text", "dual"))
                .expect("proxy row text pinned once proxy_url is set");
            assert_eq!(proxy_text.label, t("settings.providers.proxy_title"));
            assert_eq!(
                proxy_text.value.as_deref(),
                Some(t("settings.providers.proxy_subtitle"))
            );
            let proxy_switch = tree
                .find(&settings_use_proxy_identifier("dual"))
                .expect("proxy switch pinned once proxy_url is set");
            assert!(proxy_switch.enabled);
        });
        // GUI 1.15 reuses this layout path with stable per-account IDs.
        cx.update(|_, cx| {
            view.update(cx, |view, _| {
                view.handshake_info = Some(crate::controller::DesktopHandshakeInfo {
                    runtime_id: "test".into(),
                    api_version: "1.15".into(),
                    capabilities: vec![],
                    host_data_dir: None,
                });
                let credentials = &mut view.projection.settings_providers.providers[0].credentials;
                credentials[0].credential_id = "cred_first".into();
                credentials[0].display_name = "Personal".into();
                credentials[0].selected = true;
                credentials[1].credential_id = "cred_second".into();
                credentials[1].display_name = "Work".into();
            })
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        let remove_second =
            crate::ui::settings::settings_account_action_identifier("dual", "cred_second", true);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                let use_first = crate::ui::settings::settings_account_action_identifier(
                    "dual",
                    "cred_first",
                    false,
                );
                let remove_first = crate::ui::settings::settings_account_action_identifier(
                    "dual",
                    "cred_first",
                    true,
                );
                let use_second = crate::ui::settings::settings_account_action_identifier(
                    "dual",
                    "cred_second",
                    false,
                );
                assert!(!tree.find(&use_first).unwrap().enabled);
                assert!(!tree.find(&remove_first).unwrap().enabled);
                assert!(tree.find(&use_second).unwrap().enabled);
                assert!(tree
                    .find("settings-account-name-dual")
                    .unwrap()
                    .actions
                    .contains(&AxAction::SetValue));
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: remove_second.clone(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.settings_account_remove_confirm.as_deref(),
                    Some(remove_second.as_str())
                );
                // Removing/reordering the earlier row must preserve the target ID.
                view.projection.settings_providers.providers[0]
                    .credentials
                    .remove(0);
            })
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert_eq!(
                    tree.find(&remove_second).unwrap().label,
                    t("settings.providers.action_confirm_remove")
                );
                let keep = tree.find(&format!("{remove_second}-keep")).unwrap();
                assert!(keep.bounds.width > 0.0 && keep.bounds.width < 150.0);
                assert!(tree
                    .find(&crate::ui::settings::settings_credential_row_identifier(
                        "dual",
                        &view.projection.settings_providers.providers[0].credentials[0],
                        0
                    ))
                    .is_some());
                view.handle_accessibility_request(
                    AxRequest {
                        identifier: format!("{remove_second}-keep"),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
                assert!(view.settings_account_remove_confirm.is_none());
            })
        });
        // 窄窗三档字号：真实页面滚动后，离屏标题消失，底部控件框跟随 prepaint。
        for scale in [
            crate::ui::theme::font::TextScale::Percent100,
            crate::ui::theme::font::TextScale::Percent125,
            crate::ui::theme::font::TextScale::Percent150,
        ] {
            cx.simulate_resize(gpui::size(px(1080.0), px(720.0)));
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.text_scale = scale;
                    view.projection
                        .settings_providers
                        .expanded_providers
                        .remove("empty");
                    cx.notify();
                });
            });
            cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
            cx.refresh().unwrap();
            cx.run_until_parked();
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.settings_scroll.scroll_to_bottom();
                    cx.notify();
                });
            });
            cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
            cx.refresh().unwrap();
            cx.run_until_parked();
            cx.update(|window, cx| {
                let view = view.read(cx);
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("settings-page-title").is_none());
                for id in [
                    dynamic_identifier("settings-provider-usage", "dual"),
                    expand_empty.clone(),
                ] {
                    let node = tree.find(&id).expect("bottom provider content is visible");
                    let actual = view.settings_element_layouts[&id]
                        .bounds()
                        .intersect(&view.settings_scroll.bounds());
                    assert_eq!(
                        node.bounds,
                        AxRect::new(
                            actual.origin.x.into(),
                            actual.origin.y.into(),
                            actual.size.width.into(),
                            actual.size.height.into()
                        ),
                        "{id} at {scale:?}"
                    );
                }
                assert!(tree.permits(&AxRequest {
                    identifier: expand_empty.clone(),
                    action: AxAction::Press,
                    value: None
                }));
            });
        }

        // G2：同一真实布局路径显示账号三窗，epoch 与离页清理拒绝迟到结果。
        cx.simulate_resize(gpui::size(px(1440.0), px(2400.0)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.text_scale = crate::ui::theme::font::TextScale::Percent100;
                view.settings_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
                view.handshake_info.as_mut().unwrap().api_version = "1.16".into();
                let p = &mut view.projection.settings_providers.providers[0];
                p.provider_id = "opencode-go".into();
                p.credentials[0].kind = "api_key".into();
                p.credentials[0].selected = true;
                view.projection.settings_providers.expanded_providers.insert("opencode-go".into());
                let scope = serde_json::json!({"tenant_id":"local", "account_id":"local/default", "provider_id":"opencode-go"});
                let now = crate::ui::now_unix_ms();
                let windows: Vec<_> = ["rolling5h", "weekly", "monthly"].into_iter().map(|window| serde_json::json!({
                    "window":window, "read":{"status":"ok", "snapshot":{
                        "scope":scope, "window":window, "unit":{"kind":"percent"},
                        "values":{"used":{"kind":"exact","value":42},"limit":{"kind":"exact","value":100},"remaining":{"kind":"exact","value":58}},
                        "reset":{"kind":"absolute","at":now+3_600_000,"uncertain":false},
                        "confidence":"exact", "provenance":{"adapter_kind":"api_key_api","source":"Go usage","fetched_at":now-31_000}
                    }}
                })).collect();
                let quota: pawork_client::QuotaOverviewView = serde_json::from_value(serde_json::json!({
                    "scope":scope, "windows":windows, "generated_at":now
                })).unwrap();
                let state = &mut view.projection.settings_providers;
                let old = state.begin_quota("opencode-go", "cred_second");
                let latest = state.begin_quota("opencode-go", "cred_second");
                state.apply_quota("opencode-go".into(), "cred_second".into(), old, Some(quota.clone()));
                assert!(state.account_quotas[&("opencode-go".into(), "cred_second".into())].loading);
                state.apply_quota("opencode-go".into(), "cred_second".into(), latest, Some(quota));
                cx.notify();
            });
        });
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                for suffix in ["rolling", "weekly", "monthly"] {
                    let id =
                        crate::ui::settings::quota_identifier("opencode-go", "cred_second", suffix);
                    let node = tree.find(&id).expect("quota window in actual layout");
                    assert!(node.value.as_ref().unwrap().contains("Used 42%"));
                    assert!(node.value.as_ref().unwrap().contains("Stale"));
                    assert!(node.bounds.width > 0.0 && node.bounds.height > 0.0);
                }
                let mode = crate::ui::settings::quota_identifier("opencode-go", "", "mode");
                assert!(tree.find(&mode).unwrap().enabled);
                assert_eq!(tree.find(&mode).unwrap().value.as_deref(), Some("Off"));
                let refresh = crate::ui::settings::quota_identifier("opencode-go", "cred_second", "refresh");
                for (id, max_width) in [(&refresh, 220.0), (&mode, 50.0)] {
                    let node = tree.find(id).unwrap();
                    let actual = view.settings_element_layouts[id].bounds();
                    assert!(node.bounds.width > 0.0 && node.bounds.width < max_width);
                    assert_eq!(node.bounds.width, f32::from(actual.size.width));
                }
                let key = ("opencode-go".to_string(), "cred_second".to_string());
                // A refresh preserves the previous reading and reports its failure immediately.
                let epoch = view.projection.settings_providers.begin_quota(&key.0, &key.1);
                assert!(view.projection.settings_providers.account_quotas[&key].view.is_some());
                assert!(view.account_quota_labels(&key.0, &key.1)[0].1.contains("Loading"));
                view.projection.settings_providers.apply_quota(key.0.clone(), key.1.clone(), epoch, None);
                assert!(view.projection.settings_providers.account_quotas[&key].stale);
                assert!(view.account_quota_labels(&key.0, &key.1)[0].1.contains("Used 42%"));
                let previous = view.projection.settings_providers.account_quotas[&key].view.clone().unwrap();
                let mut failed = previous.clone();
                for window in &mut failed.windows {
                    window.read = pawork_client::WindowReadView::Failed { failures: vec![] };
                }
                let epoch = view.projection.settings_providers.begin_quota(&key.0, &key.1);
                view.projection.settings_providers.apply_quota(key.0.clone(), key.1.clone(), epoch, Some(failed.clone()));
                assert!(view.projection.settings_providers.account_quotas[&key].stale);
                assert_eq!(view.projection.settings_providers.account_quotas[&key].view.as_ref(), Some(&previous));
                let first = view.projection.settings_providers.begin_quota(&key.0, "first-failure");
                view.projection.settings_providers.apply_quota(key.0.clone(), "first-failure".into(), first, Some(failed.clone()));
                assert!(view.account_quota_labels(&key.0, "first-failure")[0].1.contains("unavailable"));
                // 一窗成功时采用本次 typed 结果，失败窗不冒用旧读数。
                failed.windows[0] = previous.windows[0].clone();
                let partial = view.projection.settings_providers.begin_quota(&key.0, &key.1);
                view.projection.settings_providers.apply_quota(key.0.clone(), key.1.clone(), partial, Some(failed.clone()));
                assert_eq!(view.projection.settings_providers.account_quotas[&key].view.as_ref(), Some(&failed));
                assert!(view.account_quota_labels(&key.0, &key.1)[1].1.contains("unavailable"));
                let entry = view.projection.settings_providers.account_quotas.get_mut(&key).unwrap();
                entry.stale = false;
                if let pawork_client::WindowReadView::Ok { snapshot, .. } = &mut entry.view.as_mut().unwrap().windows[0].read {
                    snapshot.provenance.fetched_at = serde_json::from_value(serde_json::json!(crate::ui::now_unix_ms() + 60_000)).unwrap();
                }
                assert!(view.account_quota_labels(&key.0, &key.1)[0].1.contains("Stale"));
                let p = &mut view.projection.settings_providers.providers[0];
                p.selection_mode = pawork_client::ProviderAccountSelectionMode::WhenExhausted;
                p.credentials.clear();
                assert!(view.account_mode_enabled(&view.projection.settings_providers.providers[0]));
                view.projection.settings_providers.account_mode_pending.insert("opencode-go".into(), epoch);
                assert!(!view.account_mode_enabled(&view.projection.settings_providers.providers[0]));
                view.clear_settings_buffers(cx);
                view.projection.settings_providers.apply_quota(
                    "opencode-go".into(),
                    "cred_second".into(),
                    1,
                    None,
                );
                assert!(view.projection.settings_providers.account_quotas.is_empty());
                view.handshake_info.as_mut().unwrap().api_version = "1.15".into();
                assert!(!view.settings_quota_supported());
            });
        });
    }
}
