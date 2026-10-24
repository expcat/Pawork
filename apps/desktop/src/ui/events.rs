//! 连接成功后的快照落地、事件泵，以及 `ControllerEvent` 分发。

use gpui::Context;
use pawork_client::{AppEvent, MAX_OBSERVATION_READ_BYTES};

use crate::controller::{ControllerEvent, DesktopHandshakeInfo};
use crate::projection::{ConnectionState, ResumeApply};

use super::computer::ObservationChunkOutcome;
use super::i18n;
use super::inspector::InspectorTab;
use super::{
    now_unix_ms, AppRoute, AppView, InspectorFocusTarget, MenuKind, SettingsPage,
    INSPECTOR_TAB_INDEX,
};

impl AppView {
    pub(super) fn on_connected(
        &mut self,
        snapshot: pawork_client::Snapshot,
        resume: Option<pawork_client::ResumeOutcome>,
        handshake: DesktopHandshakeInfo,
        events: smol::channel::Receiver<ControllerEvent>,
        cx: &mut Context<Self>,
    ) {
        self.handshake_info = Some(handshake);
        let instance_id = snapshot.instance_id.as_str().to_string();
        let previous_session = self.projection.active_session_id.clone();
        self.projection
            .set_connection(ConnectionState::Connected { instance_id });
        if self.settings_page == SettingsPage::About && self.settings_about_rows().is_none() {
            self.settings_page = SettingsPage::Advanced;
        }
        let apply = match &resume {
            None => {
                self.projection.apply_fresh_snapshot(&snapshot);
                ResumeApply::Fresh
            }
            Some(outcome) => self.projection.apply_resume_outcome(outcome, &snapshot),
        };
        // CU-19：断线期间到达的 run 终态经 Replay 补发恢复——重放事件
        // 只进 projection reducer，不经 live 事件接线；把面板绑定会话
        // 内的终态同步交给停止状态机，避免已停止任务永久停在 Unknown。
        if let Some(outcome) = &resume {
            if let pawork_client::ResumeDisposition::Replay { .. } = &outcome.disposition {
                self.apply_replayed_stop_confirmations(&outcome.replayed);
            }
        }
        self.status_hint = self.projection.resume.label();
        self.timeline_changed();
        self.refresh_all_settings(cx);
        self.refresh_expanded_account_quotas(true, cx);
        self.consume_events(events, cx);
        // 连接建立即武装 1s tick：barrier 启用而无 run 时也要常驻探测。
        self.arm_run_clock(cx);
        match apply {
            ResumeApply::ReplaceBaseline => {
                if let Some(session_id) = self.projection.active_session_id.clone() {
                    self.open_session(session_id, cx);
                    return;
                }
                // 基线替换后 active 落空：焦点回落 scope 触发器。
                self.pending_scope_focus = true;
            }
            ResumeApply::Fresh => {
                if let Some(session_id) = previous_session {
                    if self
                        .projection
                        .sessions
                        .iter()
                        .any(|session| session.session_id == session_id)
                    {
                        self.open_session(session_id, cx);
                        return;
                    }
                    // 之前的 active 会话在 fresh snapshot 中消失：焦点回落
                    // scope 触发器；首次连接（previous None）不抢 composer。
                    self.pending_scope_focus = true;
                }
            }
            ResumeApply::Continued { .. } | ResumeApply::Unchanged => {}
        }
        self.reconcile_terminal_workspace(cx);
        // Replay / UpToDate 不重开 timeline，但 Inspector 查询面必须在新连接
        // 上重新取权威数据；旧内容在此之前一直带 stale 标记保留。
        self.refresh_open_inspector_tab(cx);
        cx.notify();
    }

    /// Replay 补发事件中的 run 终态 → 停止状态机（与 live 事件接线同
    /// 语义：面板绑定该会话即收敛，不要求页面可见；confirm 内部再按
    /// run 归属过滤）。抽出独立方法便于直接钉板。
    pub(super) fn apply_replayed_stop_confirmations(
        &mut self,
        replayed: &[pawork_client::AppEventEnvelope],
    ) {
        for envelope in replayed {
            if let AppEvent::RunChanged { run_id, state } = &envelope.payload {
                let terminal = matches!(
                    state,
                    pawork_client::RunState::Completed
                        | pawork_client::RunState::Cancelled
                        | pawork_client::RunState::Failed
                        | pawork_client::RunState::Interrupted
                );
                let bound_here = matches!(
                    &envelope.stream,
                    pawork_client::EventStream::Session(id)
                        if self.computer.matches_session(Some(id.as_str()))
                );
                if terminal && bound_here {
                    self.computer.confirm_stop_terminal(run_id.as_str());
                }
            }
        }
    }

    fn consume_events(
        &mut self,
        events: smol::channel::Receiver<ControllerEvent>,
        cx: &mut Context<Self>,
    ) {
        self.event_task = None;
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |view, cx| view.handle_controller_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.event_task = Some(task);
    }

    pub(super) fn handle_controller_event(
        &mut self,
        event: ControllerEvent,
        cx: &mut Context<Self>,
    ) {
        self.controller_event_pending = true;
        // 任一新事件都会使上一轮 settle 失效；下一次静默窗口重新写入。
        self.barriers.remove_timeline_stable();
        self.barriers.remove_approval_visible();
        match event {
            ControllerEvent::WorkspaceFileResult {
                workspace_id,
                path,
                epoch,
                operation,
                result,
            } => {
                self.files_result(workspace_id, path, epoch, operation, result, cx);
            }
            ControllerEvent::BrowserRequest {
                session_id,
                run_id,
                request_id,
                action,
            } => {
                // CU-15：外部浏览器动作（用户真实 Chrome/Edge）路由到扩展
                // 桥客户端：不经内嵌 WebView，不依赖本任务面板可见性，也
                // 不占用 browser_request 槽（两条通道互不阻塞）。
                if action["action"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("ext_"))
                {
                    self.controller.external_browser_request(request_id, action);
                } else if self.browser_request.is_some()
                    || self.projection.active_session_id.as_deref() != Some(&session_id)
                    || self.projection.active_run_id.as_deref() != Some(&run_id)
                    || self.projection.active_run_id.is_none()
                {
                    self.controller.browser_respond(request_id, serde_json::json!({"ok":false,"error":"Task is no longer visible or browser is busy"}));
                } else {
                    self.browser_request = Some((session_id, run_id, request_id, action));
                    // CU-13：立即在后台任务派发。隐藏窗口 display link 停转、
                    // render 不再触发，等 render 派发会让 Host 25s 超时。
                    self.dispatch_browser_request(cx);
                }
            }
            ControllerEvent::Disconnected { reason } => {
                self.projection.settings_providers.account_quotas.clear();
                self.projection
                    .settings_providers
                    .account_mode_pending
                    .clear();
                // RV-11：断线保留子代理面板已加载内容（代理列表 / 选中
                // 项 / 对话正文与阅读位置），只清瞬时在途标记并标记过期；
                // 对话代际一并作废，断开前的迟到页不能改写保留正文。
                // 重连后由 refresh_open_inspector_tab 重查权威数据；同一会话
                // 的基线重开也不重置这段状态（见 open_session）。
                self.projection
                    .subagent_activity
                    .mark_disconnected(reason.clone());
                self.projection.subagent_conversation.mark_disconnected();
                let stale_reason = format!("connection lost · {reason}");
                self.handshake_info = None;
                if self.settings_page == SettingsPage::About {
                    self.settings_page = SettingsPage::Advanced;
                }
                self.projection
                    .set_connection(ConnectionState::Disconnected { reason });
                self.changes.mark_stale(&stale_reason);
                self.resources.mark_stale(&stale_reason);
                self.computer.mark_stale(&stale_reason);
                self.projection.mark_settings_stale(&stale_reason);
                self.terminal_pending_write.clear();
                self.terminal_queued_write.clear();
                self.terminal_pending_create_workspace = None;
                self.terminal_pending_create_cwd = None;
                self.terminal_pending_close = None;
                self.terminal_pending_resize = None;
                self.terminal_fitted_size = None;
                self.terminal_fit_resize_task = None;
                // 断连终止一切进行中分页，避免 settle barrier 永久停发。
                self.timeline_paging = false;
            }
            ControllerEvent::Snapshot(snapshot) => {
                let had_active_session = self.projection.active_session_id.is_some();
                if had_active_session {
                    self.stash_composer_draft(cx);
                }
                self.projection.merge_snapshot(&snapshot);
                if had_active_session && self.projection.active_session_id.is_none() {
                    // 当前会话归档与切走会话同样收口；先前已按原 session
                    // 保存草稿，不能把旧文本归到无会话槽或另一项目终端。
                    self.close_open_menu(cx);
                    self.timeline_paging = false;
                    self.changes.reset_for_session();
                    self.restore_composer_draft(cx);
                    self.reconcile_terminal_workspace(cx);
                    self.pending_scope_focus = true;
                } else {
                    // projection 不持有 rail scope；重复快照仍须以当前 UI
                    // scope 选择终端，避免归档后的第二次刷新回落错误项目。
                    let workspace_id = self.inspector_workspace_id();
                    self.projection
                        .select_terminal_for_workspace(workspace_id.as_deref());
                }
                // 编辑中的 session 已从快照消失（他端归档等）：静默退出
                // 编辑态，不提交。
                if self.session_rename.as_ref().is_some_and(|rename| {
                    !self
                        .projection
                        .sessions
                        .iter()
                        .any(|session| session.session_id == rename.session_id)
                }) {
                    self.session_rename = None;
                }
                self.timeline_changed();
            }
            ControllerEvent::TimelineLoaded { session_id, page } => {
                if self.projection.active_session_id.as_deref() == Some(&session_id) {
                    self.timeline_changed();
                    self.sync_navigation_session();
                    self.timeline_navigation.observe_page(&page);
                    self.projection.apply_timeline_page(&page);
                    if page.complete {
                        self.timeline_paging = false;
                    }
                }
            }
            ControllerEvent::Event(envelope) => {
                if let AppEvent::Diagnostic { code, message, .. } = &envelope.payload {
                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(message) {
                        if code == "terminal.agent_created" {
                            if let (Some(id), Some(workspace)) = (
                                data["terminal_session_id"].as_str(),
                                data["workspace_id"].as_str(),
                            ) {
                                self.projection
                                    .apply_terminal_created(workspace.into(), id.into());
                                if let Some(cwd) = data["cwd"].as_str() {
                                    self.projection.apply_terminal_cwd(id, cwd);
                                }
                                self.reconcile_terminal_workspace(cx);
                                self.inspector_tab = InspectorTab::Terminal;
                                self.remember_inspector_tab(InspectorTab::Terminal);
                                self.inspector_open = true;
                            }
                        }
                        if code == "terminal.agent_closed" {
                            if let Some(id) = data["terminal_session_id"].as_str() {
                                self.handle_controller_event(
                                    ControllerEvent::TerminalCloseSucceeded {
                                        terminal_session_id: id.into(),
                                    },
                                    cx,
                                );
                            }
                        }
                    }
                }
                self.sync_navigation_session();
                if matches!(&envelope.stream, pawork_client::EventStream::Session(id) if Some(id.as_str()) == self.projection.active_session_id.as_deref())
                {
                    self.timeline_navigation.observe_event(&envelope);
                }
                let terminal_event = matches!(envelope.payload, AppEvent::TerminalOutput { .. });
                // Run 终态（RunChanged 清空 active_run_id）后刷新 Changes：
                // 会话 diff 可能已被这轮 run 改写。
                let had_active_run = self.projection.active_run_id.is_some();
                if terminal_event {
                    self.terminal_scroll.content_arriving();
                    if self.projection.apply_event(&envelope) {
                        self.terminal_scroll.follow_new_content();
                    }
                } else if self.projection.apply_event(&envelope) {
                    self.timeline_changed();
                }
                if had_active_run && self.projection.active_run_id.is_none() {
                    self.refresh_changes(cx);
                }
                // CU-18：目标清单只覆盖活跃 run 的 registry 登记；run
                // 开始 / 终态切换时重查（页面打开才查，不常驻轮询）。
                // CU-19：run 终态事件是停止状态机的确认通道之一（面板
                // 绑定该会话即收敛，不要求页面可见）；工具完成即新截图
                // 落库，页面打开时重查证据（读取持久账本，不触发截图）。
                if let AppEvent::RunChanged { run_id, state } = &envelope.payload {
                    let terminal = matches!(
                        state,
                        pawork_client::RunState::Completed
                            | pawork_client::RunState::Cancelled
                            | pawork_client::RunState::Failed
                            | pawork_client::RunState::Interrupted
                    );
                    let bound_here = matches!(
                        &envelope.stream,
                        pawork_client::EventStream::Session(id) if self
                            .computer
                            .matches_session(Some(id.as_str()))
                    );
                    if terminal && bound_here {
                        self.computer.confirm_stop_terminal(run_id.as_str());
                    }
                }
                if self.inspector_open
                    && self.inspector_tab == InspectorTab::Computer
                    && (had_active_run != self.projection.active_run_id.is_some()
                        || matches!(envelope.payload, AppEvent::ToolCompleted { .. }))
                {
                    self.refresh_computer_targets(cx);
                }
                // 子代理观测：父会话 subagent.spawned / child 会话 Run 终态
                // 到达即刷新「子智能体」卡（浮层或对话栏打开期间）；对话栏
                // 始终喂被选子代理的 live 事件，被选子会话终态另触发一次
                // 权威重查（水合 live 通知缺省的参数 / 用量 / 终态）。
                let subagent_panel_open =
                    self.inspector_open && self.inspector_tab == InspectorTab::Subagent;
                let spawned = matches!(
                    &envelope.payload,
                    AppEvent::Diagnostic { code, .. } if code == "subagent.spawned"
                );
                let child_terminal = matches!(
                    &envelope.stream,
                    pawork_client::EventStream::Session(id) if id.as_str().starts_with("child-")
                ) && matches!(
                    envelope.payload,
                    AppEvent::RunChanged { ref state, .. }
                        if matches!(
                            state,
                            pawork_client::RunState::Completed
                                | pawork_client::RunState::Failed
                                | pawork_client::RunState::Cancelled
                        )
                );
                // 只有被选子会话的事件才算「新内容到达」：主会话流式 /
                // 工具事件不得触发 content_arriving，否则未贴底时会把
                // 对话栏永久脱钩（FollowScroll 贴底判定 + 回底浮出）。
                let selected_stream = matches!(
                    &envelope.stream,
                    pawork_client::EventStream::Session(id)
                        if Some(id.as_str())
                            == self.projection.subagent_conversation.agent_id.as_deref()
                );
                if selected_stream && subagent_panel_open {
                    self.subagent_conversation_scroll.content_arriving();
                }
                let selected_agent_live = self
                    .projection
                    .subagent_conversation
                    .apply_live_event(&envelope);
                if selected_agent_live && subagent_panel_open {
                    self.subagent_conversation_scroll.follow_new_content();
                }
                let selected_child_terminal = child_terminal
                    && matches!(
                        &envelope.stream,
                        pawork_client::EventStream::Session(id)
                            if Some(id.as_str())
                                == self.projection.subagent_conversation.agent_id.as_deref()
                    );
                if selected_child_terminal {
                    if let Some(agent_id) = self.projection.subagent_conversation.agent_id.clone() {
                        let generation = self.projection.subagent_conversation.begin_loading();
                        self.controller.load_subagent_timeline(agent_id, generation);
                    }
                }
                if (spawned || child_terminal)
                    && (subagent_panel_open || matches!(self.open_menu, Some(MenuKind::Activity)))
                {
                    self.load_activity_subagents();
                }
                // Live notifications omit persisted arguments, final output and usage.
                // Merge history after completion; the reducer enriches already-seen rows.
                if let pawork_client::EventStream::Session(session) = &envelope.stream {
                    if self.projection.active_session_id.as_deref() == Some(session.as_str())
                        && matches!(
                            &envelope.payload,
                            AppEvent::ToolCompleted { .. }
                                | AppEvent::RunChanged {
                                    state: pawork_client::RunState::Completed
                                        | pawork_client::RunState::Cancelled
                                        | pawork_client::RunState::Failed,
                                    ..
                                }
                        )
                    {
                        let run_id = match &envelope.payload {
                            AppEvent::ToolCompleted { run_id, .. }
                            | AppEvent::RunChanged { run_id, .. } => run_id.as_str(),
                            _ => unreachable!(),
                        };
                        let after = self
                            .projection
                            .timeline
                            .iter()
                            .filter(|entry| entry.run_id.as_deref() == Some(run_id))
                            .map(|entry| entry.sequence)
                            .min()
                            .map(|seq| seq.saturating_sub(1));
                        self.controller
                            .refresh_timeline(session.as_str().to_string(), after);
                    }
                }
                // SET-4 / R07：Succeeded / Removed 落地后重查权威状态
                // 与两套 model_list（加 key 后目录不能停在进页时的静态回退）。
                if self
                    .projection
                    .settings_providers
                    .take_pending_status_refresh()
                {
                    self.refresh_models_authority();
                }
            }
            ControllerEvent::SessionArchiveFinished {
                session_id,
                archived,
                result,
            } => {
                self.finish_session_archive(session_id, archived, result, cx);
            }
            ControllerEvent::SessionCreated { session_id } => {
                if let Some(pending) = self.pending_home_send.take() {
                    self.composer_send_target = Some(session_id.clone());
                    self.composer_drafts
                        .insert(session_id.clone(), pending.text.clone());
                    self.no_session_draft.clear();
                    self.composer_options.remove(&None);
                    self.composer_options
                        .insert(Some(session_id.clone()), pending.options.clone());
                    self.open_session(session_id.clone(), cx);
                    self.controller.send_message(
                        session_id,
                        pending.text,
                        pending.model,
                        pending.effort,
                        pending.options,
                    );
                } else {
                    self.open_session(session_id, cx);
                }
            }
            ControllerEvent::WorkspaceOpened {
                workspace_id,
                name,
                create_task,
            } => {
                self.scope_workspace_id = Some(workspace_id.clone());
                if create_task {
                    if let Some(existing) = self
                        .projection
                        .unstarted_session_id(Some(workspace_id.as_str()))
                        .map(str::to_string)
                    {
                        self.open_session(existing, cx);
                    } else {
                        self.controller.create_session(Some(workspace_id));
                    }
                }
                self.reconcile_terminal_workspace(cx);
                self.rail_scroll_to_active = true;
                self.status_hint = Some(i18n::t("status.project_opened").replace("{}", &name));
            }
            ControllerEvent::SessionForked { session_id } => {
                let feedback = i18n::t("status.forked").replace("{}", &session_id);
                self.open_session(session_id, cx);
                self.show_status_feedback(feedback, cx);
            }
            ControllerEvent::TerminalCreated {
                workspace_id,
                terminal_session_id,
            } => {
                let pending_create_match = self.terminal_pending_create_workspace.as_deref()
                    == Some(workspace_id.as_str());
                if pending_create_match {
                    self.terminal_pending_create_workspace = None;
                }
                let pending_create_cwd = if pending_create_match {
                    self.terminal_pending_create_cwd.take()
                } else {
                    None
                };
                self.projection
                    .apply_terminal_created(workspace_id, terminal_session_id.clone());
                if let Some(cwd) = pending_create_cwd {
                    // 回执不带 cwd；用请求值让新终端立即如实显示。
                    self.projection
                        .apply_terminal_cwd(&terminal_session_id, &cwd);
                }
                // SET-6d（ADR-050 D4）：面板已测到尺寸时用实测列×行，
                // 否则回落 terminal_settings（未查询到仍是 80×24）。
                // 投影与随后那次 terminal_resize 必须同尺寸，Host spawn
                // 尺寸与面板可能不一致，即使本地已相等也要下发。
                let (columns, rows) = self
                    .terminal_fitted_size
                    .unwrap_or_else(|| self.projection.settings_terminal.effective_size());
                self.projection
                    .apply_terminal_initial_size(&terminal_session_id, columns, rows);
                self.terminal_pending_resize = Some((terminal_session_id.clone(), columns, rows));
                self.controller
                    .terminal_resize(terminal_session_id.clone(), columns, rows);
                self.reconcile_terminal_workspace(cx);
                // 新终端从空输出开始，恢复跟随态。
                self.terminal_scroll.jump_to_bottom();
                // 程序化展开 Inspector：关闭可能悬浮的菜单（P3-1 泄漏修复）。
                // Side 并排宽度变化由 render 的 inspector_render_width 路径 reflow。
                self.close_open_menu(cx);
                self.inspector_open = true;
                self.refresh_open_inspector_tab(cx);
                self.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
            }
            ControllerEvent::TerminalCreateFailed {
                workspace_id,
                reason,
            } => {
                if self.terminal_pending_create_workspace.as_deref() == Some(workspace_id.as_str())
                {
                    self.terminal_pending_create_workspace = None;
                    self.terminal_pending_create_cwd = None;
                }
                self.projection
                    .mark_terminal_create_failed(&workspace_id, reason.clone());
                self.reconcile_terminal_workspace(cx);
            }
            ControllerEvent::TerminalWriteSucceeded {
                terminal_session_id,
            } => {
                self.projection.mark_terminal_ready(&terminal_session_id);
                self.terminal_pending_write.remove(&terminal_session_id);
                self.flush_queued_terminal_write(&terminal_session_id, cx);
            }
            ControllerEvent::TerminalWriteFailed {
                terminal_session_id,
                reason,
            } => {
                self.terminal_pending_write.remove(&terminal_session_id);
                self.terminal_queued_write.remove(&terminal_session_id);
                self.projection
                    .note_terminal_io_failed(&terminal_session_id, reason.clone());
            }
            ControllerEvent::TerminalResizeSucceeded {
                terminal_session_id,
                columns,
                rows,
            } => {
                self.projection
                    .apply_terminal_resize(&terminal_session_id, columns, rows);
                if self.terminal_pending_resize.as_ref().is_some_and(
                    |(pending_id, pending_columns, pending_rows)| {
                        pending_id == &terminal_session_id
                            && *pending_columns == columns
                            && *pending_rows == rows
                    },
                ) {
                    self.terminal_pending_resize = None;
                    self.maybe_apply_fitted_terminal_size(cx);
                }
            }
            ControllerEvent::TerminalResizeFailed {
                terminal_session_id,
                reason,
            } => {
                if self
                    .terminal_pending_resize
                    .as_ref()
                    .is_some_and(|(pending_id, _, _)| pending_id == &terminal_session_id)
                {
                    self.terminal_pending_resize = None;
                }
                self.projection
                    .note_terminal_io_failed(&terminal_session_id, reason.clone());
            }
            ControllerEvent::TerminalCloseSucceeded {
                terminal_session_id,
            } => {
                if self.terminal_pending_close.as_deref() == Some(terminal_session_id.as_str()) {
                    self.terminal_pending_close = None;
                    self.terminal_pending_write.remove(&terminal_session_id);
                    self.terminal_queued_write.remove(&terminal_session_id);
                    let was_current = self.projection.terminal.session_id.as_deref()
                        == Some(terminal_session_id.as_str());
                    self.projection.remove_terminal(&terminal_session_id);
                    // §8：关闭当前 PTY 有同项目兄弟时 remove_terminal 已选中
                    // 兄弟；session 为空说明最后一枚已关，退出 Terminal 工具
                    // 页回落到最近打开的其它工具或入口（不做无谓重建）。
                    if self.projection.terminal.session_id.is_none() {
                        self.close_inspector_tool(InspectorTab::Terminal, cx);
                    }
                    if was_current {
                        self.reconcile_terminal_input(cx);
                        self.maybe_apply_fitted_terminal_size(cx);
                        if self.inspector_tab == InspectorTab::Terminal {
                            self.terminal_scroll.jump_to_bottom();
                            self.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
                        }
                    }
                    self.status_hint = Some(i18n::t("status.terminal_closed").into());
                }
            }
            ControllerEvent::TerminalCloseFailed {
                terminal_session_id,
                reason,
            } => {
                if self
                    .terminal_pending_close
                    .as_ref()
                    .is_some_and(|pending_id| pending_id == &terminal_session_id)
                {
                    self.terminal_pending_close = None;
                }
                self.projection
                    .note_terminal_io_failed(&terminal_session_id, reason);
            }
            ControllerEvent::ComposerAttachmentsLoaded { draft, result } => {
                self.composer_loading = false;
                // 成功清错误；失败与超限都按会话落在 Composer 附件区域
                //（RV-03），不再写到窗口右下角 status_hint（含旧瞬态提示）。
                self.status_hint = None;
                match result {
                    Ok(attachments) => {
                        let options = self.composer_options.entry(draft).or_default();
                        if options.attachments.len() + options.video_urls.len() + attachments.len()
                            <= 4
                        {
                            options.attachments.extend(attachments);
                            options.attachment_error = None;
                        } else {
                            options.attachment_error =
                                Some(crate::controller::ComposerAttachmentError::TooMany {
                                    count: options.attachments.len()
                                        + options.video_urls.len()
                                        + attachments.len(),
                                });
                        }
                    }
                    Err(error) => {
                        self.composer_options
                            .entry(draft)
                            .or_default()
                            .attachment_error = Some(error);
                    }
                }
            }
            ControllerEvent::MessageSent {
                session_id,
                run_id,
                text,
            } => {
                self.composer_sending = false;
                self.composer_send_target = None;
                self.composer_send_errors.remove(&Some(session_id.clone()));
                self.composer_options.remove(&Some(session_id.clone()));
                let now = now_unix_ms();
                self.projection.note_session_run(&session_id, &run_id, now);
                // wire 无用户消息事件：发送回执即本地乐观上屏（重放后由
                // 持久化行替换）。非 active session 不 echo，重放会补。
                if self
                    .projection
                    .note_user_echo(&session_id, &run_id, &text, now)
                {
                    self.timeline_changed();
                }
                self.apply_message_sent_draft(&session_id, cx);
            }
            ControllerEvent::ModelsLoaded(models) => {
                self.projection.set_models(models);
                // 模型目录变化后回收已消失模型的「设为默认」焦点句柄。
                self.ensure_settings_api_key_inputs(cx);
            }
            ControllerEvent::ModelCatalogLoaded(models) => {
                // 全量目录（include_disabled=true）只喂「Manage models」
                // 弹层；与 projection.models（过滤口径）分列，不互相覆盖。
                self.projection
                    .settings_providers
                    .apply_model_catalog(models);
            }
            ControllerEvent::AccountModeFinished {
                provider_id,
                epoch,
                data,
            } => {
                if self
                    .projection
                    .settings_providers
                    .account_mode_pending
                    .get(&provider_id)
                    == Some(&epoch)
                {
                    self.projection
                        .settings_providers
                        .account_mode_pending
                        .remove(&provider_id);
                    if let Some(data) = data {
                        self.projection.settings_providers.apply_loaded(data);
                        self.ensure_settings_api_key_inputs(cx);
                        self.remark_settings_stale_if_disconnected();
                    }
                }
            }
            ControllerEvent::AccountQuotaLoaded {
                provider_id,
                credential_id,
                epoch,
                view,
            } => {
                if self.route == AppRoute::Settings
                    && self.settings_page == SettingsPage::Providers
                    && matches!(
                        self.projection.connection,
                        ConnectionState::Connected { .. }
                    )
                {
                    self.projection.settings_providers.apply_quota(
                        provider_id,
                        credential_id,
                        epoch,
                        view,
                    );
                }
            }
            ControllerEvent::ProviderStatusLoaded(data) => {
                self.projection.settings_providers.apply_loaded(data);
                self.refresh_expanded_account_quotas(false, cx);
                self.ensure_settings_api_key_inputs(cx);
                self.remark_settings_stale_if_disconnected();
            }
            ControllerEvent::DefaultRoleModelConfirmed { role, value } => {
                // Host Data 确认（回执即写后状态，ADR-055 D5；不重查）：
                // 按角色落地键对；conversation 另同步 Composer 已确认默认
                //（会话 / 草稿 / Run 不动）。
                self.settings_role_pending = None;
                self.projection.confirm_role_default_pair(role, value);
            }
            ControllerEvent::GeneralSettingsLoaded(data)
            | ControllerEvent::ProxyUrlConfirmed(data) => {
                let proxy_url = data.proxy_url.clone();
                self.projection.settings_general.apply_loaded(data);
                self.settings_proxy_input.update(cx, |input, cx| {
                    input.reset_text(proxy_url.unwrap_or_default(), cx)
                });
                self.remark_settings_stale_if_disconnected();
            }
            ControllerEvent::ProviderUseProxyConfirmed {
                provider_id,
                use_proxy,
            } => {
                // Host Data 确认（回执即写后状态，ADR-052 SET-6h）；
                // 不乐观更新：以回执值为准直接收敛，不重查。
                self.projection
                    .settings_providers
                    .confirm_use_proxy(&provider_id, use_proxy);
            }
            ControllerEvent::ModelEnabledConfirmed {
                provider_id,
                model_id,
                enabled,
                cleared_roles,
            } => {
                // Host Data 确认（回执即写后状态，ADR-055 D2a）：按回执
                // 收敛弹层与 Composer，不重探全通道目录。
                self.projection.settings_providers.model_write_pending = None;
                self.projection
                    .apply_model_enabled(&provider_id, &model_id, enabled);
                self.apply_model_cleared_roles(&cleared_roles);
            }
            ControllerEvent::ProviderModelsEnabledConfirmed {
                provider_id,
                enabled,
                cleared_roles,
            } => {
                self.projection.settings_providers.model_write_pending = None;
                self.projection
                    .apply_provider_models_enabled(&provider_id, enabled);
                self.apply_model_cleared_roles(&cleared_roles);
            }
            ControllerEvent::ModelReasoningConfirmed {
                provider_id,
                model_id,
                default_effort,
                manual_efforts,
            } => {
                // ADR-063：回执即写后状态——收敛目录条目的默认强度与手动
                // 范围；不清 Composer 已选强度（用户选择优先）。
                self.projection.settings_providers.model_write_pending = None;
                self.projection.apply_model_reasoning(
                    &provider_id,
                    &model_id,
                    default_effort,
                    manual_efforts,
                );
            }
            ControllerEvent::PermissionsSettingsLoaded(data) => {
                self.projection.settings_permissions.apply_loaded(data);
                self.remark_settings_stale_if_disconnected();
            }
            ControllerEvent::ApprovalModeConfirmed { mode } => {
                // Host Data 确认（回执即写后状态，ADR-048 D2）；不乐观更新。
                self.projection
                    .settings_permissions
                    .confirm_approval_mode(mode);
                self.remark_settings_stale_if_disconnected();
            }
            ControllerEvent::WorkspaceTrustConfirmed { trusted } => {
                // Host Data 确认（回执即写后状态，ADR-048 D3）；不乐观更新。
                self.projection
                    .settings_permissions
                    .confirm_workspace_trusted(trusted);
                self.remark_settings_stale_if_disconnected();
            }
            ControllerEvent::TerminalSettingsLoaded(data)
            | ControllerEvent::TerminalSettingsConfirmed(data) => {
                // SET-6d：查询与写回执同形状（回执即写后完整状态，
                // ADR-050 D3）；输入框回填 Host 权威生效值。
                self.projection.settings_terminal.apply_loaded(data.clone());
                let shell = data.shell.clone().unwrap_or_default();
                let columns = data.columns.to_string();
                let rows = data.rows.to_string();
                self.settings_terminal_shell_input
                    .update(cx, |input, cx| input.reset_text(shell, cx));
                self.settings_terminal_columns_input
                    .update(cx, |input, cx| input.reset_text(columns, cx));
                self.settings_terminal_rows_input
                    .update(cx, |input, cx| input.reset_text(rows, cx));
                self.remark_settings_stale_if_disconnected();
            }
            ControllerEvent::SubagentSettingsLoaded(data)
            | ControllerEvent::SubagentSettingsConfirmed(data) => {
                // 查询与写回执同形状（回执即写后完整状态）；并发数输入框
                // 回填 Host 权威生效值。
                self.projection
                    .settings_subagents
                    .apply_loaded(data.clone());
                let max = data.max_concurrent.to_string();
                self.settings_subagents_concurrency_input
                    .update(cx, |input, cx| input.reset_text(max, cx));
                self.remark_settings_stale_if_disconnected();
            }
            ControllerEvent::SubagentListLoaded {
                session_id,
                request_id,
                data,
            } => {
                if self
                    .projection
                    .subagent_activity
                    .apply_loaded(&session_id, request_id, data)
                {
                    // 成功回执是权威列表。原选中代理已不在其中（父会话分支
                    // 变了，或它退出展示集合）时，选中项和旧正文都失效：
                    // 清掉后正文走未选择空态，而不是停在 Loading，也不把
                    // 旧正文继续当成有效内容。查询失败不走这里，旧内容保留。
                    // 从未选中时仍自动选中列表第一项；失效清除不再改选别人。
                    let selection_dropped = self
                        .projection
                        .subagent_conversation
                        .agent_id
                        .as_ref()
                        .is_some_and(|id| {
                            !self
                                .projection
                                .subagent_activity
                                .agents
                                .iter()
                                .any(|agent| &agent.agent_id == id)
                        });
                    if !selection_dropped
                        && self.inspector_tab == InspectorTab::Subagent
                        && self.projection.subagent_conversation.agent_id.is_none()
                    {
                        let first = self
                            .projection
                            .subagent_activity
                            .display_agents()
                            .first()
                            .map(|agent| agent.agent_id.clone());
                        if let Some(id) = first {
                            self.select_subagent_agent(&id, cx);
                        }
                    }
                    if selection_dropped {
                        self.projection.subagent_conversation = Default::default();
                        self.subagent_conversation_scroll.jump_to_bottom();
                    }
                }
            }
            ControllerEvent::SubagentListFailed {
                session_id,
                request_id,
                reason,
            } => {
                self.projection
                    .subagent_activity
                    .apply_failed(&session_id, request_id, reason);
            }
            ControllerEvent::SubagentCancelFinished {
                session_id,
                agent_id,
                result,
            } => {
                let state = &mut self.projection.subagent_activity;
                if state.session_id.as_deref() == Some(&session_id)
                    && state.cancelling.as_deref() == Some(&agent_id)
                {
                    match result {
                        Ok(()) => {
                            self.status_hint = Some(i18n::t("subagents.cancel_requested").into());
                            self.refresh_subagent_conversation();
                        }
                        Err(reason) => {
                            state.cancelling = None;
                            state.error = Some(reason);
                        }
                    }
                }
            }
            ControllerEvent::SubagentTimelineLoaded {
                session_id,
                generation,
                page,
            } => {
                self.projection
                    .subagent_conversation
                    .apply_page(&session_id, generation, &page);
                if self.inspector_open && self.inspector_tab == InspectorTab::Subagent {
                    self.subagent_conversation_scroll.follow_new_content();
                }
            }
            ControllerEvent::SubagentTimelineFailed {
                session_id,
                generation,
                reason,
            } => {
                self.projection.subagent_conversation.apply_failed(
                    &session_id,
                    generation,
                    &reason,
                );
            }
            ControllerEvent::AuthStarted { provider_id, data } => {
                self.settings_copied_auth = None;
                // SET-4：登记 OAuth 等待信息并置 Connecting；进度由
                // AuthChanged 事件收敛。
                self.projection
                    .settings_providers
                    .apply_auth_started(&provider_id, data);
            }
            ControllerEvent::OperationFailed { action, reason } => {
                let composer_failed = self.composer_sending
                    && (action == "send message"
                        || (action == "create session" && self.pending_home_send.is_some()));
                if action == "load provider status" {
                    self.projection
                        .settings_providers
                        .apply_failed(reason.as_str());
                }
                if action == "set provider use proxy" {
                    let message = format!("Could not change provider proxy · {reason}");
                    self.projection.settings_providers.apply_failed(&message);
                }
                if action == "set model enabled" || action == "set provider models enabled" {
                    // 写失败保旧：目录 / 角色默认不动；清在途标记并沿
                    // settings 错误行呈现原因。
                    self.projection.settings_providers.model_write_pending = None;
                    let message = format!("Could not change model enablement · {reason}");
                    self.projection.settings_providers.apply_failed(&message);
                }
                if action == "set model reasoning" {
                    // 写失败保旧（ADR-063 同口径）。
                    self.projection.settings_providers.model_write_pending = None;
                    let message = format!("Could not change reasoning effort · {reason}");
                    self.projection.settings_providers.apply_failed(&message);
                }
                if action == "set default role model" {
                    // 写失败保旧：回执未到，角色键对不动；清在途标记并沿
                    // settings 错误行呈现原因。
                    self.settings_role_pending = None;
                    let message = format!("Could not save default model · {reason}");
                    self.projection.settings_providers.apply_failed(&message);
                }
                if action == "load general settings" || action == "set proxy url" {
                    let message = if action == "set proxy url" {
                        format!("Could not save proxy URL · {reason}")
                    } else {
                        format!("Could not load general settings · {reason}")
                    };
                    self.projection.settings_general.apply_failed(&message);
                }
                if action == "load permissions settings"
                    || action == "set approval mode"
                    || action == "set workspace trust"
                {
                    let message = match action {
                        "set approval mode" => {
                            format!("Could not switch approval mode · {reason}")
                        }
                        "set workspace trust" => {
                            format!("Could not change workspace trust · {reason}")
                        }
                        _ => format!("Could not load permissions settings · {reason}"),
                    };
                    self.projection.settings_permissions.apply_failed(&message);
                }
                if action == "load terminal settings" || action == "set terminal settings" {
                    let message = if action == "set terminal settings" {
                        format!("Could not save terminal settings · {reason}")
                    } else {
                        format!("Could not load terminal settings · {reason}")
                    };
                    self.projection.settings_terminal.apply_failed(&message);
                }
                if action == "load subagent settings" || action == "set subagent settings" {
                    let message = if action == "set subagent settings" {
                        format!("Could not save subagent settings · {reason}")
                    } else {
                        format!("Could not load subagent settings · {reason}")
                    };
                    if action == "set subagent settings" {
                        self.projection
                            .settings_subagents
                            .apply_write_failed(&message);
                    } else {
                        self.projection.settings_subagents.apply_failed(&message);
                    }
                }
                if action == "start provider auth" || action == "verify api key" {
                    // auth_start / auth_set_api_key 的 socket 级失败无对应
                    // AuthChanged 事件：重查权威状态回滚乐观 Connecting
                    //（busy / unsupported / 断连等路径；Replace 旧凭证是
                    // 否仍在也交重查裁决）。
                    self.refresh_provider_status();
                }
                if action == "test mcp server" || action == "remove mcp server" {
                    let message = if action == "remove mcp server" {
                        i18n::t("status.mcp_remove_failed").replace("{}", &reason)
                    } else {
                        i18n::t("status.mcp_test_failed").replace("{}", &reason)
                    };
                    self.resources.action_error = Some(message);
                    if action == "remove mcp server" {
                        // 写盘可能已成功、仅清密失败：重查清单与 Host 对齐。
                        self.refresh_resources(cx);
                    }
                }
                if action == "create session" {
                    self.pending_home_send = None;
                }
                if composer_failed {
                    self.composer_send_errors
                        .insert(self.composer_send_target.take(), reason.clone());
                    self.composer_sending = false;
                }
                self.status_hint = Some(if composer_failed {
                    i18n::t("composer.send_failed").to_string()
                } else {
                    i18n::t2("status.action_failed", &action, &reason)
                });
            }
            ControllerEvent::SessionOpenFailed { session_id, reason } => {
                // 分页复位按 session 匹配：A→B 快切时 A 的迟到失败不得
                // 清掉 B 的 timeline_paging（否则 settle barrier 提前放行）。
                if self.projection.active_session_id.as_deref() == Some(&session_id) {
                    self.timeline_paging = false;
                    self.status_hint =
                        Some(i18n::t("status.open_session_failed").replace("{}", &reason));
                }
            }
            ControllerEvent::DiffFilesLoaded {
                epoch,
                session_id,
                files,
                git,
            } => {
                if self.changes.apply_files(epoch, session_id, files, git) {
                    // 文件行焦点句柄随 canonical 清单建立；清单删除的路径同步
                    // 摘除，避免 Tab 链残留不可见停靠点。
                    self.changes_file_focus
                        .retain(|path, _| self.changes.files.iter().any(|file| &file.path == path));
                    for file in &self.changes.files {
                        self.changes_file_focus
                            .entry(file.path.clone())
                            .or_insert_with(|| {
                                cx.focus_handle()
                                    .tab_stop(true)
                                    .tab_index(INSPECTOR_TAB_INDEX)
                            });
                    }
                    // 清单刷新后选中文件仍在：重拉它的 diff，保持两视图一致。
                    if let Some(path) = self.changes.selected.clone() {
                        self.fetch_diff(&path, cx);
                    }
                }
            }
            ControllerEvent::DiffContentLoaded {
                epoch,
                path,
                session_id,
                file,
            } => {
                self.changes.apply_diff(epoch, &path, session_id, file);
            }
            ControllerEvent::McpServersLoaded { epoch, servers } => {
                self.resources.apply_servers(epoch, servers);
            }
            ControllerEvent::McpServersReceipt { servers } => {
                self.resources.apply_authoritative_servers(servers);
            }
            ControllerEvent::DiffFilesFailed { epoch, reason } => {
                if self.changes.mark_failed_for_epoch(epoch, &reason) {
                    self.status_hint =
                        Some(i18n::t("status.load_changes_failed").replace("{}", &reason));
                }
            }
            ControllerEvent::DiffContentFailed {
                epoch,
                path,
                reason,
            } => {
                if self
                    .changes
                    .mark_diff_failed_for_epoch(epoch, &path, &reason)
                {
                    self.status_hint =
                        Some(i18n::t("status.load_diff_failed").replace("{}", &reason));
                }
            }
            ControllerEvent::McpServersFailed { epoch, reason } => {
                if self.resources.mark_failed_for_epoch(epoch, &reason) {
                    self.status_hint =
                        Some(i18n::t("status.load_resources_failed").replace("{}", &reason));
                }
            }
            ControllerEvent::ComputerTargetsLoaded {
                session_id,
                request_id,
                data,
            } => {
                self.computer.apply_loaded(&session_id, request_id, data);
            }
            ControllerEvent::ComputerTargetsFailed {
                session_id,
                request_id,
                reason,
            } => {
                self.computer.apply_failed(&session_id, request_id, reason);
            }
            ControllerEvent::ComputerTargetActionFinished {
                session_id,
                action_id,
                result,
            } => {
                let approved = result.is_ok();
                let settled = self.computer.finish_action(&session_id, action_id, result);
                // Ok：Host 已落地授权 / 撤销，重查列表收敛权威状态
                //（回执只回显生效 target 与寿命，不带完整清单）。代次不
                // 匹配的迟到回执（切会话 / 断线后到达）不收敛状态、不触发
                // 重查。
                if settled
                    && approved
                    && self.inspector_open
                    && self.inspector_tab == InspectorTab::Computer
                {
                    self.refresh_computer_targets(cx);
                }
            }
            ControllerEvent::ObservationsLoaded {
                session_id,
                request_id,
                data,
            } => {
                // 探测页（limit=1）只习得 head：返回尾窗参数；尾窗内被
                // 单页字节预算截断时按游标同代次续读（页数有界）。满页
                // 未到 head 时 Host complete=false，apply 把窗口上沿
                // 当成该窗结束，不顺游标扫回更新事件。尾窗 0 条时改为
                // 更早的窗口。收敛落库后，面板可见时为选中证据按需自动
                // 加载首图。
                match self
                    .computer
                    .apply_observations(&session_id, request_id, &data)
                {
                    Some((after, limit)) => {
                        self.controller.observation_list(
                            session_id,
                            request_id,
                            after,
                            Some(limit),
                        );
                    }
                    None => {
                        if self.inspector_open && self.inspector_tab == InspectorTab::Computer {
                            self.computer_start_evidence_load(cx);
                        }
                    }
                }
            }
            ControllerEvent::ObservationsFailed {
                session_id,
                request_id,
                reason,
            } => {
                self.computer
                    .apply_observations_failed(&session_id, request_id, reason);
            }
            ControllerEvent::ObservationImageChunk {
                session_id,
                request_id,
                data,
            } => {
                // eof=false 时以回显偏移续读（单块上限恒 MAX_OBSERVATION_
                // READ_BYTES；总量以条目申报字节为界，面板侧 fail-closed）。
                // eof 收齐后字节移交后台执行器做真实解码校验（像素 /
                // 帧预算 + 全帧解码不占 UI 线程，结果按证据身份与请求
                // 代次落地）。
                match self
                    .computer
                    .apply_observation_chunk(&session_id, request_id, &data)
                {
                    ObservationChunkOutcome::Continue(key, offset) => {
                        if self.computer.evidence_load.is_some() {
                            self.controller.observation_image_read(
                                session_id,
                                request_id,
                                key.0,
                                key.1,
                                offset,
                                MAX_OBSERVATION_READ_BYTES,
                            );
                        }
                    }
                    ObservationChunkOutcome::Decode(task) => {
                        self.computer_dispatch_evidence_decode(session_id, task, cx);
                    }
                    ObservationChunkOutcome::Settled => {}
                }
            }
            ControllerEvent::ObservationImageFailed {
                session_id,
                request_id,
                reason,
            } => {
                self.computer
                    .apply_observation_image_failed(&session_id, request_id, reason);
            }
            ControllerEvent::ComputerStopFinished {
                session_id,
                action_id,
                run_id,
                result,
            } => {
                // 回执（Ok / Err）只证明取消命令被接受 / 拒绝，不冒充已
                // 停止：重查列表只收敛「被拒仍持有」的回落错误与新
                // run 接管退役（已停止仅由 run 终态事件确认）。代次 /
                // run 不匹配的迟到回执不触发。
                if self
                    .computer
                    .finish_stop(&session_id, action_id, &run_id, result)
                {
                    self.refresh_computer_targets(cx);
                }
            }
        }
        self.arm_run_clock(cx);
        cx.notify();
    }
}
