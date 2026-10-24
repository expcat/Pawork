//! AX Press 白名单。

use super::*;

impl AppView {
    pub(super) fn handle_accessibility_press(
        &mut self,
        identifier: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match identifier {
            "quick-search" => {
                window.focus(&self.quick_search.trigger);
                self.open_quick_search(window, cx);
            }
            // P0-2：AX Press 与 mouse / Enter / Space 共用直接切换路径。
            "archive-undo" => {
                if self.archive_write_enabled() {
                    self.undo_session_archive(cx)
                }
            }
            "task-rail-grouping" => self.toggle_grouping(window, cx),
            "project-scope" => {
                window.focus(&self.scope_focus);
                self.on_toggle_scope_menu(None, window, cx)
            }
            "scope-add-project" => self.on_open_project(window, cx),
            "composer-project-task" => self.on_project_task_menu(None, window, cx),
            "workspace-bind-project" => {
                window.focus(&self.welcome_bind_project_focus);
                self.on_project_task_menu(None, window, cx)
            }
            // ADR-054 D1：All-projects 态直建无归属会话（无确认浮层）；
            // AXPress 先移焦触发器，与真实点击同一路径。
            "add-task" => {
                window.focus(&self.add_task_focus);
                self.on_new_session(window, cx)
            }
            "task-usage-open" => {
                self.open_task_usage(cx);
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
            "model-search-clear" => self.clear_model_search(window, cx),
            other
                if ModelPurposeFilter::ALL
                    .iter()
                    .any(|purpose| purpose.identifier() == other) =>
            {
                if let Some(purpose) = ModelPurposeFilter::ALL
                    .into_iter()
                    .find(|purpose| purpose.identifier() == other)
                {
                    self.set_model_purpose_filter(purpose, window, cx);
                }
            }
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
            "settings-nav-subagents" => {
                window.focus(&self.settings_nav_subagents_focus);
                self.on_select_settings_page(SettingsPage::Subagents, window, cx);
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
            other if other.starts_with("settings-search-result-") => {
                self.activate_settings_search_result_id(other, window, cx);
            }
            "settings-proxy-save" => self.on_settings_proxy_save(cx),
            "settings-proxy-clear" => self.on_settings_proxy_clear(cx),
            "settings-terminal-save" => self.on_settings_terminal_save(cx),
            "settings-terminal-clear" => self.on_settings_terminal_clear(cx),
            "settings-subagents-enabled" => self.on_settings_subagents_toggle_enabled(cx),
            "settings-subagents-save" => self.on_settings_subagents_save_concurrency(cx),
            other if other.starts_with(crate::ui::settings::SETTINGS_SUBAGENT_CONTROL_PREFIX) => {
                self.dispatch_settings_subagent_control(other, cx);
            }
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
            "effort-picker" => {
                window.focus(&self.effort_focus);
                self.on_toggle_effort_menu(None, window, cx)
            }
            // 强度菜单行（「自动」+ 可选级别）：与可见行同源，仅菜单
            // 打开时可派发；未知行 fail-closed。
            other
                if other.starts_with("effort-")
                    && matches!(self.open_menu, Some(MenuKind::Effort)) =>
            {
                let Some(option) = self.effort_menu_options().into_iter().find(|option| {
                    format!("effort-{}", option.as_deref().unwrap_or("auto")) == other
                }) else {
                    return false;
                };
                self.on_select_effort(option, cx);
                window.focus(&self.effort_focus);
            }
            "model-menu-settings" => self.on_manage_composer_models(window, cx),
            "composer-search-toggle" => self.toggle_composer_search(cx),
            // RV-01：预览关闭与预览开关是独立 AX 动作；精确 id 先于前缀
            // arm 匹配（附件 id 均为 desktop-* 形态，不会撞名）。
            "composer-preview-close" => self.close_composer_attachment_preview(cx),
            other if other.starts_with("composer-preview-") => {
                self.toggle_composer_attachment_preview(
                    other.trim_start_matches("composer-preview-"),
                    cx,
                );
            }
            other if other.starts_with("composer-remove-") => {
                self.remove_composer_attachment(other.trim_start_matches("composer-remove-"), cx);
            }
            "composer-attach" => self.on_composer_add_menu(None, cx),
            other
                if matches!(self.open_menu, Some(MenuKind::ComposerAdd))
                    && super::super::super::input_area::ComposerAction::ALL
                        .iter()
                        .any(|action| action.id() == other) =>
            {
                let Some(action) = super::super::super::input_area::ComposerAction::ALL
                    .into_iter()
                    .find(|action| action.id() == other)
                else {
                    return false;
                };
                self.activate_composer_action(action, window, cx);
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
            // RV-02：附件折叠头（键 = 事件 id + -attachment-N，均 AX 安全字符，
            // 渲染 id 与 dynamic_identifier 逐字节一致）。
            other if other.starts_with("attachment-toggle-") => {
                let Some(key) = self.projection.timeline.iter().find_map(|entry| {
                    let TimelineEntryKind::UserMessage { text } = &entry.kind else {
                        return None;
                    };
                    let segments = crate::ui::attachment_blocks::split_user_message(text)?;
                    (0..crate::ui::attachment_blocks::attachment_count(&segments)).find_map(|ix| {
                        let key = crate::ui::attachment_blocks::attachment_key(&entry.event_id, ix);
                        (dynamic_identifier("attachment-toggle", &key) == other).then_some(key)
                    })
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
            other if other.starts_with("tool-result-toggle-") => {
                let Some(key) = self.projection.timeline.iter().find_map(|entry| {
                    let identifier = dynamic_identifier("tool-result-toggle", &entry.event_id);
                    (identifier == other).then(|| tool_result_expand_key(&entry.event_id))
                }) else {
                    return false;
                };
                self.toggle_timeline_detail(&key, cx);
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
            "inspector-panel" => {
                window.focus(&self.inspector_panel_focus);
                self.toggle_menu(MenuKind::InspectorPanel, None, cx)
            }
            "inspector-back" | "inspector-approval-hint" => self.on_inspector_back(window, cx),
            // Home 入口是 open 语义（激活已有页），「+」菜单行是 launch 语义
            //（终端动作新建 PTY）；两套 identifier 在菜单开启时不同时发布。
            "inspector-tab-changes" => {
                self.open_inspector_tool(InspectorTab::Changes, cx);
                self.close_open_menu(cx);
            }
            "inspector-tab-terminal" => {
                self.open_inspector_tool(InspectorTab::Terminal, cx);
                self.close_open_menu(cx);
            }
            "inspector-tab-resources" => {
                self.open_inspector_tool(InspectorTab::Resources, cx);
                self.close_open_menu(cx);
            }
            "inspector-tab-computer" => {
                self.open_inspector_tool(InspectorTab::Computer, cx);
                self.close_open_menu(cx);
            }
            "inspector-tab-browser" => {
                self.open_inspector_tool(InspectorTab::Browser, cx);
                self.close_open_menu(cx);
            }
            "inspector-tab-subagent" => {
                self.open_inspector_tool(InspectorTab::Subagent, cx);
                self.close_open_menu(cx);
            }
            "inspector-menu-changes" => {
                self.launch_inspector_tool(InspectorTab::Changes, cx);
                self.close_open_menu(cx);
            }
            "inspector-menu-terminal" => {
                self.launch_inspector_tool(InspectorTab::Terminal, cx);
                self.close_open_menu(cx);
            }
            "inspector-menu-resources" => {
                self.launch_inspector_tool(InspectorTab::Resources, cx);
                self.close_open_menu(cx);
            }
            "inspector-tab-files" => self.open_inspector_tool(InspectorTab::Files, cx),
            "inspector-menu-files" => {
                self.launch_inspector_tool(InspectorTab::Files, cx);
                self.close_open_menu(cx);
            }
            "inspector-menu-browser" => {
                self.launch_inspector_tool(InspectorTab::Browser, cx);
                self.close_open_menu(cx);
            }
            "inspector-menu-subagent" => {
                self.launch_inspector_tool(InspectorTab::Subagent, cx);
                self.close_open_menu(cx);
            }
            "inspector-menu-computer" => {
                self.launch_inspector_tool(InspectorTab::Computer, cx);
                self.close_open_menu(cx);
            }
            other if other.starts_with("inspector-open-") || other.starts_with("file-tab-") => {
                if let Some(tab) = self.panel_tabs(cx).into_iter().find(|tab| tab.id == other) {
                    self.activate_panel_tab(&tab, cx);
                }
            }
            other if other.starts_with("panel-close-") => {
                if let Some(tab) = self
                    .panel_tabs(cx)
                    .into_iter()
                    .find(|tab| tab.close_id() == other)
                {
                    self.close_panel_tab(&tab, window, cx);
                }
            }
            "changes-tab-files" => self.on_select_changes_tab(ChangesTab::Files, cx),
            "changes-tab-summary" => self.on_select_changes_tab(ChangesTab::Summary, cx),
            "changes-refresh" => self.activate_changes_refresh(cx),
            "changes-diff-scroll-left" => self.scroll_diff_horizontal(-1.0, cx),
            "changes-diff-scroll-right" => self.scroll_diff_horizontal(1.0, cx),
            "resources-refresh" => self.activate_resources_refresh(cx),
            id if id.starts_with("resources-error-details-")
                || id == "resources-query-error-details" =>
            {
                self.toggle_resources_details(id, cx)
            }
            "computer-refresh" => self.refresh_computer_targets(cx),
            // CU-18：目标授权 / 撤销按钮（identifier = 前缀 + 目标 key，
            // 与 render 同源；反查与 gate 在 computer_action_by_id）。
            other
                if other.starts_with(crate::ui::computer::COMPUTER_APPROVE_ONCE_PREFIX)
                    || other.starts_with(crate::ui::computer::COMPUTER_APPROVE_RUN_PREFIX)
                    || other.starts_with(crate::ui::computer::COMPUTER_REVOKE_PREFIX)
                    || other.starts_with(crate::ui::computer::COMPUTER_STOP_PREFIX) =>
            {
                self.computer_action_by_id(other, cx);
            }
            // CU-19：观测证据行（选择 + 加载）与预览卡查看 / 重试按钮；
            // gate 与身份匹配在各自入口内复核。
            other if other.starts_with("computer-observation-") => {
                self.computer_observation_ax_action(other, cx);
            }
            "terminal-new-tab" => self.on_new_terminal_tab(cx),
            other if other.starts_with("terminal-tab-") => {
                if let Some(id) = other.strip_prefix("terminal-tab-") {
                    self.on_select_terminal_tab(id, cx);
                }
            }
            "terminal-back-to-bottom" => self.terminal_scroll.jump_to_bottom(),
            "terminal-close" => self.on_close_terminal(window, cx),
            "activity-open-changes" => self.on_activity_open_changes(window, cx),
            // 子代理对话栏按钮（与 render 同源；chip identifier 含 agent_id）。
            "subagent-cancel" => self.cancel_selected_subagent(cx),
            "subagent-refresh" => {
                self.refresh_subagent_conversation();
                cx.notify();
            }
            "subagent-back-to-bottom" => {
                self.subagent_conversation_scroll.jump_to_bottom();
                cx.notify();
            }
            other if other.starts_with("subagent-select-") => {
                if let Some(id) = other.strip_prefix("subagent-select-") {
                    self.select_subagent_agent(id, cx);
                }
            }
            // 子代理对话栏工具行 / 长回执展开（与 render 同源 key，独立于
            // 主 Timeline 折叠集合）。
            other if other.starts_with("subagent-tool-toggle-") => {
                if let Some(id) = other.strip_prefix("subagent-tool-toggle-") {
                    self.toggle_subagent_detail(&format!("{id}:tool"), cx);
                }
            }
            other if other.starts_with("subagent-tool-result-toggle-") => {
                if let Some(id) = other.strip_prefix("subagent-tool-result-toggle-") {
                    self.toggle_subagent_detail(
                        &crate::ui::timeline_entry::tool_result_expand_key(id),
                        cx,
                    );
                }
            }
            other if other.starts_with("subagent-result-toggle-") => {
                if let Some(id) = other.strip_prefix("subagent-result-toggle-") {
                    self.toggle_subagent_detail(&format!("{id}:result"), cx);
                }
            }
            // 浮层子代理行（identifier 含 agent_id；与 render 行同源）。
            other if other.starts_with("activity-subagent-") => {
                let id = other.strip_prefix("activity-subagent-").unwrap_or("");
                let agent = self
                    .projection
                    .subagent_activity
                    .agents
                    .iter()
                    .find(|agent| agent.agent_id == id)
                    .cloned();
                if let Some(agent) = agent {
                    self.on_activity_open_subagent(&agent.agent_id, window, cx);
                }
            }
            _ => {
                if self.on_settings_quota_action(identifier, cx) {
                    return true;
                }
                if self.on_settings_account_action(identifier, window, cx) {
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
                    // ADR-063：推理强度控件（默认 cycle / 范围 chip / 重置）
                    // 走 render / 键盘同源 dispatch。
                    if self.dispatch_settings_models_control(identifier, cx) {
                        return true;
                    }
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
                        // 强度三变体已由上方 dispatch_settings_models_control 收口。
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
                if let Some(content) = self.projection.timeline.iter().find_map(|entry| {
                    let text = timeline_entry_markdown(entry)?;
                    crate::ui::markdown::message_code_copies(&entry.event_id, text)
                        .into_iter()
                        .find(|copy| copy.identifier == identifier)
                        .map(|copy| copy.content)
                }) {
                    self.copy_timeline_text(content, cx);
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
                if self.projection.timeline.iter().any(|entry| {
                    run_open_providers_identifier(&entry.event_id) == identifier
                        && entry.fork_boundary == Some(ForkBoundary::Failed)
                        && failure_next_step(
                            &run_summary_texts(entry, false)
                                .map(|(_, description)| description)
                                .unwrap_or_default(),
                        ) == FailureNextStep::OpenProviderSettings
                }) {
                    self.on_manage_composer_models(window, cx);
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
}
