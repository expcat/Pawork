//! UI 层：AppView（Sessions 侧栏 + Timeline + Composer）与事件消费循环。

mod components;
pub mod text_input;
mod theme;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, anchored, deferred, div, point, prelude::*, px, AnyView, App, ClickEvent, Context,
    Corner, Entity, FocusHandle, Focusable, KeyBinding, KeyDownEvent, Pixels, Point, Render,
    SharedString, Styled, Window,
};
use pawork_client::AppEvent;

use crate::controller::{ControllerEvent, DesktopController};
use crate::platform::Platform;
use crate::projection::{
    ConnectionState, DesktopProjection, ModelEntry, ResumeApply, TaskRailDateGroup,
    TaskRailGrouping, TaskRailProjectGroup, TimelineEntry, TimelineEntryKind, UNASSIGNED_PROJECT,
};
use components::button::{Button, ButtonPadding, ButtonVariant};
use components::dropdown::{Dropdown, MenuPanel, MenuRow, ANCHOR_GAP_Y};
use components::follow_scroll::{BackToBottom, FollowScroll};
use components::label::{Badge, Label};
use components::list_row::ListRow;
use components::panel::Panel;
use components::status_bar::StatusBar;
use theme::{dark, font, metrics};


pub use text_input::{SendMessage, TextInput};

actions!(
    desktop_app,
    [
        ApproveOnce,
        ApproveForRun,
        Deny,
        CancelRun,
        NewTask,
        ToggleInspector,
    ]
);

/// 可测的 AppView 快捷键表（审批 / 取消 / 新建 / Inspector）。
pub(crate) const APP_VIEW_KEYBINDINGS: &[(&str, &str)] = &[
    ("cmd-.", "CancelRun"),
    ("cmd-enter", "ApproveOnce"),
    ("cmd-1", "ApproveOnce"),
    ("cmd-2", "ApproveForRun"),
    ("cmd-3", "Deny"),
    ("cmd-n", "NewTask"),
    ("cmd-i", "ToggleInspector"),
];

/// 主路径按钮的可测 tab_stop 标记。
pub(crate) const MAIN_PATH_TAB_STOP_IDS: &[&str] = &[
    "approve-once",
    "approve-for-run",
    "approve-deny",
    "cancel",
    "send",
    "add-task",
    "model-picker",
];

struct TooltipText {
    text: SharedString,
}

impl Render for TooltipText {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(dark().surface.raised)
            .border_1()
            .border_color(dark().border.strong)
            .text_size(px(font::XS))
            .text_color(dark().text.primary)
            .child(self.text.clone())
    }
}

fn tooltip_text(text: impl Into<SharedString>, cx: &mut App) -> AnyView {
    cx.new(|_| TooltipText { text: text.into() }).into()
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

enum RailView {
    Timeline(Vec<TaskRailDateGroup>),
    Projects(Vec<TaskRailProjectGroup>),
}

/// 当前打开的浮层菜单（五组共享，开新即关旧，修互斥不对称）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum MenuKind {
    Grouping,
    Scope,
    Model,
    /// 条目「···」菜单，键为 timeline event_id。
    Entry(String),
    /// 无触发器：All projects 下新建任务的条件确认浮层。
    WorkspaceConfirm,
}

fn relative_activity(updated_at_ms: u64, now_ms: u64) -> String {
    let elapsed = now_ms.saturating_sub(updated_at_ms);
    if elapsed < 60_000 {
        "now".into()
    } else if elapsed < 3_600_000 {
        format!("{}m", elapsed / 60_000)
    } else if elapsed < 86_400_000 {
        format!("{}h", elapsed / 3_600_000)
    } else {
        format!("{}d", elapsed / 86_400_000)
    }
}

pub fn install_keybindings(cx: &mut App) {
    use text_input::{Backspace, Delete, End, Home, Left, NewLine, Paste, Right};

    // 绑定只对 key_context "TextInput" 聚焦时生效；Enter 冒泡到 AppView，
    // 由 AppView 结合 composing / 发送可用性决定是否发送（gui-design §6）。
    cx.bind_keys([
        KeyBinding::new("enter", SendMessage, Some("TextInput")),
        KeyBinding::new("shift-enter", NewLine, Some("TextInput")),
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("cmd-v", Paste, Some("TextInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
        KeyBinding::new("cmd-.", CancelRun, Some("AppView")),
        KeyBinding::new("cmd-enter", ApproveOnce, Some("AppView")),
        KeyBinding::new("cmd-1", ApproveOnce, Some("AppView")),
        KeyBinding::new("cmd-2", ApproveForRun, Some("AppView")),
        KeyBinding::new("cmd-3", Deny, Some("AppView")),
        KeyBinding::new("cmd-n", NewTask, Some("AppView")),
        KeyBinding::new("cmd-i", ToggleInspector, Some("AppView")),
    ]);
}

pub struct AppView {
    /// 持有 tokio Runtime（GUI Connection Protocol 宿主），防止提前 shutdown。
    _platform: Arc<Platform>,
    controller: Arc<DesktopController>,
    socket: PathBuf,
    projection: DesktopProjection,
    text_input: Entity<TextInput>,
    terminal_input: Entity<TextInput>,
    timeline_scroll: FollowScroll,
    terminal_scroll: FollowScroll,
    status_hint: Option<String>,
    grouping: TaskRailGrouping,
    scope_workspace_id: Option<String>,
    collapsed_projects: BTreeSet<String>,
    inspector_open: bool,
    /// 当前打开的菜单；单一状态位保证至多一个打开（§8.2）。
    open_menu: Option<MenuKind>,
    /// 同一次物理点击里「外点关闭先于触发器 click」的衔接标记（菜单种类 +
    /// 按下位置）：触发器 toggle 仅当 click 的按下位置与标记相同（同一次
    /// 物理点击，ClickEvent 自带 down）才视为「再点触发器关闭」的收尾不再
    /// 重开；位置不等或键盘触发则为新点击，清标记后正常 toggle
    /// （见 dismiss_menu_on_outside）。
    pending_outside_close: Option<(MenuKind, Point<Pixels>)>,
    run_clock_running: bool,
    focus_handle: FocusHandle,
    approve_once_focus: FocusHandle,
    approve_for_run_focus: FocusHandle,
    deny_focus: FocusHandle,
    cancel_focus: FocusHandle,
    send_focus: FocusHandle,
    add_task_focus: FocusHandle,
    model_focus: FocusHandle,
}

impl AppView {
    pub fn new(platform: Arc<Platform>, socket: PathBuf, cx: &mut Context<Self>) -> Self {
        let controller = Arc::new(DesktopController::new(platform.handle()));
        let text_input = cx.new(|cx| TextInput::new(cx));
        let terminal_input =
            cx.new(|cx| TextInput::with_placeholder("Terminal input… (Enter to write)", cx));
        let mut view = Self {
            _platform: platform,
            controller,
            socket,
            projection: DesktopProjection::default(),
            text_input,
            terminal_input,
            timeline_scroll: FollowScroll::new(),
            terminal_scroll: FollowScroll::new(),
            status_hint: None,
            grouping: TaskRailGrouping::Timeline,
            scope_workspace_id: None,
            collapsed_projects: BTreeSet::new(),
            inspector_open: true,
            open_menu: None,
            pending_outside_close: None,
            run_clock_running: false,
            focus_handle: cx.focus_handle(),
            approve_once_focus: cx.focus_handle().tab_stop(true),
            approve_for_run_focus: cx.focus_handle().tab_stop(true),
            deny_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            send_focus: cx.focus_handle().tab_stop(true),
            add_task_focus: cx.focus_handle().tab_stop(true),
            model_focus: cx.focus_handle().tab_stop(true),
        };
        view.start_connect(cx);
        view
    }

    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.text_input.read(cx).focus_handle(cx)
    }

    fn focus_composer(&self, window: &mut Window, cx: &App) {
        window.focus(&self.composer_focus_handle(cx));
    }

    fn start_connect(&mut self, cx: &mut Context<Self>) {
        self.projection.set_connection(ConnectionState::Connecting);
        self.status_hint = None;
        let controller = Arc::clone(&self.controller);
        let socket = self.socket.clone();
        cx.spawn(
            async move |this, cx| match controller.connect(socket).await {
                Ok(connected) => {
                    this.update(cx, |view, cx| {
                        view.on_connected(
                            connected.snapshot,
                            connected.resume,
                            connected.events,
                            cx,
                        );
                    })
                    .ok();
                }
                Err(reason) => {
                    this.update(cx, |view, cx| {
                        view.projection
                            .set_connection(ConnectionState::Failed { reason });
                        view.status_hint = Some("Connect failed. Click Reconnect to retry.".into());
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

    fn on_connected(
        &mut self,
        snapshot: pawork_client::Snapshot,
        resume: Option<pawork_client::ResumeOutcome>,
        events: smol::channel::Receiver<ControllerEvent>,
        cx: &mut Context<Self>,
    ) {
        let instance_id = snapshot.instance_id.as_str().to_string();
        let previous_session = self.projection.active_session_id.clone();
        self.projection
            .set_connection(ConnectionState::Connected { instance_id });
        let apply = match &resume {
            None => {
                self.projection.apply_fresh_snapshot(&snapshot);
                ResumeApply::Fresh
            }
            Some(outcome) => self.projection.apply_resume_outcome(outcome, &snapshot),
        };
        self.status_hint = self.projection.resume.label();
        self.controller.load_models();
        self.consume_events(events, cx);
        match apply {
            ResumeApply::ReplaceBaseline => {
                if let Some(session_id) = self.projection.active_session_id.clone() {
                    self.open_session(session_id, cx);
                    return;
                }
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
                }
            }
            ResumeApply::Continued { .. } | ResumeApply::Unchanged => {}
        }
        cx.notify();
    }

    fn consume_events(
        &mut self,
        events: smol::channel::Receiver<ControllerEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |view, cx| view.handle_controller_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn handle_controller_event(&mut self, event: ControllerEvent, cx: &mut Context<Self>) {
        match event {
            ControllerEvent::Disconnected { reason } => {
                self.projection
                    .set_connection(ConnectionState::Disconnected { reason });
                self.status_hint = Some("Connection lost. Click Reconnect.".into());
            }
            ControllerEvent::Snapshot(snapshot) => {
                self.projection.merge_snapshot(&snapshot);
            }
            ControllerEvent::TimelineLoaded { session_id, page } => {
                if self.projection.active_session_id.as_deref() == Some(&session_id) {
                    self.timeline_scroll.content_arriving();
                    self.projection.apply_timeline_page(&page);
                    self.timeline_scroll.follow_new_content();
                }
            }
            ControllerEvent::Event(envelope) => {
                let terminal_event = matches!(envelope.payload, AppEvent::TerminalOutput { .. });
                if terminal_event {
                    self.terminal_scroll.content_arriving();
                    if self.projection.apply_event(&envelope) {
                        self.terminal_scroll.follow_new_content();
                    }
                } else {
                    self.timeline_scroll.content_arriving();
                    if self.projection.apply_event(&envelope) {
                        self.timeline_scroll.follow_new_content();
                    }
                }
            }
            ControllerEvent::SessionCreated { session_id } => {
                self.open_session(session_id, cx);
            }
            ControllerEvent::SessionForked { session_id } => {
                self.status_hint = Some(format!("Forked · {session_id}"));
                self.open_session(session_id, cx);
            }
            ControllerEvent::TerminalCreated {
                terminal_session_id,
            } => {
                self.projection.terminal.session_id = Some(terminal_session_id.clone());
                self.controller.terminal_resize(
                    terminal_session_id,
                    self.projection.terminal.columns,
                    self.projection.terminal.rows,
                );
                // 新终端从空输出开始，恢复跟随态。
                self.terminal_scroll.jump_to_bottom();
                self.inspector_open = true;
            }
            ControllerEvent::MessageSent { session_id, run_id } => {
                if self.projection.active_session_id.as_deref() == Some(&session_id) {
                    self.projection.active_run_id = Some(run_id);
                }
                self.text_input.update(cx, |input, cx| input.clear(cx));
            }
            ControllerEvent::ModelsLoaded(models) => {
                self.projection.set_models(models);
            }
            ControllerEvent::OperationFailed { action, reason } => {
                self.status_hint = Some(format!("{action} failed: {reason}"));
            }
        }
        self.arm_run_clock(cx);
        cx.notify();
    }

    fn arm_run_clock(&mut self, cx: &mut Context<Self>) {
        if self.run_clock_running || self.projection.active_run_id.is_none() {
            return;
        }
        self.run_clock_running = true;
        cx.spawn(async move |this, cx| loop {
            smol::Timer::after(Duration::from_secs(1)).await;
            let keep = this
                .update(cx, |view, cx| {
                    if view.projection.active_run_id.is_some() {
                        cx.notify();
                        true
                    } else {
                        view.run_clock_running = false;
                        false
                    }
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        })
        .detach();
    }

    fn open_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        self.projection.select_session(&session_id);
        self.status_hint = None;
        self.timeline_scroll.reset();
        // 打开 / 切换 session 时补终端跟随重置（§8.3）。
        self.terminal_scroll.jump_to_bottom();
        self.controller.open_session(session_id);
        cx.notify();
    }

    fn on_session_clicked(
        &mut self,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.projection.active_session_id.as_deref() == Some(session_id) {
            self.focus_composer(window, cx);
            return;
        }
        self.open_session(session_id.to_string(), cx);
        self.focus_composer(window, cx);
    }

    fn on_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match resolve_new_task_workspace(self.scope_workspace_id.as_deref()) {
            Some(workspace) => self.create_task(Some(workspace.to_string()), window, cx),
            None => {
                self.open_menu = Some(MenuKind::WorkspaceConfirm);
                self.status_hint =
                    Some("All projects: confirm a workspace before creating a task.".into());
                cx.notify();
            }
        }
    }

    fn on_confirm_workspace(
        &mut self,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_menu = None;
        self.create_task(Some(workspace_id), window, cx);
    }

    fn on_project_add_task(
        &mut self,
        workspace_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_task(Some(workspace_id), window, cx);
    }

    fn create_task(
        &mut self,
        workspace_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_create_task() {
            self.status_hint = Some(self.add_task_disabled_reason());
            cx.notify();
            return;
        }
        let Some(workspace) = workspace_id else {
            self.status_hint = Some("Choose a project before creating a task.".into());
            cx.notify();
            return;
        };
        self.controller.create_session(workspace);
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn can_create_task(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        )
    }

    fn add_task_disabled_reason(&self) -> String {
        match &self.projection.connection {
            ConnectionState::Connected { .. } => "Create task is available.".into(),
            ConnectionState::Connecting => "New task needs a live connection.".into(),
            ConnectionState::Disconnected { reason } => {
                format!("New task disabled · disconnected · {reason}")
            }
            ConnectionState::Failed { reason } => {
                format!("New task disabled · connect failed · {reason}")
            }
        }
    }

    fn on_toggle_grouping_menu(
        &mut self,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_menu(MenuKind::Grouping, down_position, cx);
    }

    fn on_select_grouping(
        &mut self,
        grouping: TaskRailGrouping,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.grouping = grouping;
        self.open_menu = None;
        cx.notify();
    }

    fn on_toggle_scope_menu(
        &mut self,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_menu(MenuKind::Scope, down_position, cx);
    }

    fn on_select_scope(
        &mut self,
        workspace_id: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scope_workspace_id = workspace_id;
        self.open_menu = None;
        cx.notify();
    }

    fn on_toggle_project(
        &mut self,
        project_key: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.collapsed_projects.insert(project_key.clone()) {
            self.collapsed_projects.remove(&project_key);
        }
        cx.notify();
    }

    /// 提取 ClickEvent 的按下位置（键盘触发无位置，永不判为同一次物理点击）。
    fn click_down_position(event: &ClickEvent) -> Option<Point<Pixels>> {
        match event {
            ClickEvent::Mouse(mouse) => Some(mouse.down.position),
            ClickEvent::Keyboard(_) => None,
        }
    }

    /// 触发器 toggle：开新关旧（单一 Option<MenuKind>，修互斥不对称），
    /// 再点同一触发器关闭。外点关闭先行触发且 click 按下位置与标记相同
    /// （同一次物理点击）时视为关闭收尾，不重开；否则清陈旧标记正常处理。
    fn toggle_menu(
        &mut self,
        target: MenuKind,
        down_position: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        if let Some((closed, press)) = self.pending_outside_close.take() {
            if closed == target && down_position == Some(press) {
                cx.notify();
                return;
            }
        }
        self.open_menu = if self.open_menu.as_ref() == Some(&target) {
            None
        } else {
            Some(target)
        };
        cx.notify();
    }

    /// 浮层外按下鼠标：关闭并留下衔接标记（种类 + 按下位置，供 toggle_menu
    /// 判定同一次物理点击）。
    fn dismiss_menu_on_outside(
        &mut self,
        kind: MenuKind,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_menu = None;
        self.pending_outside_close = Some((kind, position));
        cx.notify();
    }

    /// 直接关闭当前菜单（Escape / 选择选项 / Fork 后）。
    fn close_open_menu(&mut self, cx: &mut Context<Self>) {
        if self.open_menu.take().is_some() {
            cx.notify();
        }
    }

    fn project_key(workspace_id: Option<&str>) -> String {
        workspace_id.unwrap_or(UNASSIGNED_PROJECT).to_string()
    }

    fn scope_label(&self) -> String {
        match &self.scope_workspace_id {
            None => "All projects".into(),
            Some(id) => self.projection.workspace_name(Some(id)),
        }
    }

    fn on_reconnect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.start_connect(cx);
    }

    fn on_send_clicked(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.send_current_message(cx);
    }

    fn on_send_message(&mut self, _: &SendMessage, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .terminal_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            if self.terminal_input.read(cx).is_composing() {
                return;
            }
            self.send_terminal_input(cx);
            return;
        }
        // IME 组合中的 Enter 属于输入法确认（gui-design §6）。
        if self.text_input.read(cx).is_composing() {
            return;
        }
        self.send_current_message(cx);
    }

    fn on_fork(&mut self, event_id: &str, cx: &mut Context<Self>) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            self.status_hint = Some("Open a session before forking.".into());
            cx.notify();
            return;
        };
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.status_hint = Some("Fork needs a live connection.".into());
            cx.notify();
            return;
        }
        // 入口级防线：渲染层已按边界禁用 Fork，这里再按 reducer 的单点判型
        // 复核——connected + active session + run 终止边界缺一不可。
        let forkable = self
            .projection
            .timeline
            .iter()
            .any(|entry| entry.event_id == event_id && entry.is_fork_boundary());
        if !forkable {
            self.status_hint = Some(
                "Fork is only available on a finished run (completed, cancelled, or failed)."
                    .into(),
            );
            cx.notify();
            return;
        }
        self.controller
            .fork_session(session_id, event_id.to_string());
        cx.notify();
    }

    fn on_toggle_inspector(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.inspector_open = !self.inspector_open;
        cx.notify();
    }

    fn on_approve_once(&mut self, _: &ApproveOnce, _: &mut Window, cx: &mut Context<Self>) {
        self.on_approve("approve_once", cx);
    }

    fn on_approve_for_run(&mut self, _: &ApproveForRun, _: &mut Window, cx: &mut Context<Self>) {
        self.on_approve("approve_for_run", cx);
    }

    fn on_deny(&mut self, _: &Deny, _: &mut Window, cx: &mut Context<Self>) {
        self.on_approve("deny", cx);
    }

    fn on_cancel_run(&mut self, _: &CancelRun, window: &mut Window, cx: &mut Context<Self>) {
        self.on_cancel_clicked(window, cx);
    }

    fn on_new_task_action(&mut self, _: &NewTask, window: &mut Window, cx: &mut Context<Self>) {
        self.on_new_session(window, cx);
    }

    fn on_toggle_inspector_action(
        &mut self,
        _: &ToggleInspector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_toggle_inspector(window, cx);
    }

    fn on_start_terminal(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_terminal(cx);
        cx.notify();
    }

    fn on_apply_terminal_size(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.projection.terminal.session_id.clone() {
            self.controller.terminal_resize(
                id,
                self.projection.terminal.columns,
                self.projection.terminal.rows,
            );
        }
        cx.notify();
    }

    fn ensure_terminal(&mut self, _cx: &mut Context<Self>) {
        if self.projection.terminal.session_id.is_some() {
            return;
        }
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.status_hint = Some("Terminal needs a live connection.".into());
            return;
        }
        let Some(workspace) = self
            .scope_workspace_id
            .clone()
            .or_else(|| self.projection.workspace_id.clone())
        else {
            self.status_hint = Some("Choose a project before opening Terminal.".into());
            return;
        };
        self.controller
            .terminal_create(workspace, Some(self.projection.terminal.cwd.clone()));
    }

    fn send_terminal_input(&mut self, cx: &mut Context<Self>) {
        if self.projection.terminal.session_id.is_none() {
            self.ensure_terminal(cx);
            self.status_hint = Some("Starting terminal…".into());
            cx.notify();
            return;
        }
        let Some(id) = self.projection.terminal.session_id.clone() else {
            return;
        };
        let text = self.terminal_input.read(cx).text().to_string();
        if text.trim().is_empty() {
            return;
        }
        let data = if text.ends_with('\n') {
            text
        } else {
            format!("{text}\n")
        };
        self.controller.terminal_write(id, data);
        self.terminal_input.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    fn send_current_message(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            self.status_hint = Some("Open a session first.".into());
            cx.notify();
            return;
        };
        if !self.can_send() {
            return;
        }
        let text = self.text_input.read(cx).text().to_string();
        if text.trim().is_empty() {
            return;
        }
        let model = self.projection.effective_model().cloned();
        self.controller.send_message(session_id, text, model);
    }

    fn on_cancel_clicked(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(run_id) = self.projection.active_run_id.clone() else {
            return;
        };
        self.controller.cancel_run(run_id);
        cx.notify();
    }

    fn on_approve(&mut self, decision: &str, cx: &mut Context<Self>) {
        let Some(pending) = self.projection.pending_approval.clone() else {
            return;
        };
        self.controller
            .approve(pending.run_id, pending.tool_call_id, decision);
        cx.notify();
    }

    fn on_toggle_model_menu(
        &mut self,
        down_position: Option<Point<Pixels>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_switch_model() {
            return;
        }
        self.toggle_menu(MenuKind::Model, down_position, cx);
    }

    fn on_select_model(&mut self, model: ModelEntry, cx: &mut Context<Self>) {
        self.projection
            .set_pending_model(model.provider_id, model.id);
        self.open_menu = None;
        cx.notify();
    }

    fn can_switch_model(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_run_id.is_none()
            && !self.projection.models.is_empty()
    }

    fn can_send(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_session_id.is_some()
            && self.projection.active_run_id.is_none()
    }

    fn can_approve(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.pending_approval.is_some()
    }

    fn can_cancel(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_run_id.is_some()
    }

    fn approve_disabled_reason(&self) -> String {
        if self.projection.pending_approval.is_none() {
            "No pending approval.".into()
        } else {
            "Approval needs a live connection.".into()
        }
    }

    fn cancel_disabled_reason(&self) -> String {
        if self.projection.active_run_id.is_none() {
            "No active run to cancel.".into()
        } else {
            "Cancel needs a live connection.".into()
        }
    }

    fn model_disabled_reason(&self) -> String {
        if self.projection.active_run_id.is_some() {
            "Model switch is disabled while a run is in progress.".into()
        } else if self.projection.models.is_empty() {
            "Model catalog is still loading.".into()
        } else {
            "Model switch needs a live connection.".into()
        }
    }

    fn model_label(&self) -> String {
        match self.projection.effective_model() {
            Some((provider, id)) => self
                .projection
                .models
                .iter()
                .find(|entry| entry.provider_id == *provider && entry.id == *id)
                .map(|entry| format!("{} / {}", entry.provider_id, entry.display_name))
                .unwrap_or_else(|| format!("{provider} / {id}")),
            None if self.projection.models.is_empty() => "Model · loading".into(),
            None => "Model · select".into(),
        }
    }

    /// model 菜单面板（从 composer 内联抽出，与其他组同构的浮层 + MenuRow）。
    fn model_menu_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        MenuPanel::new("model-menu")
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Model, event.position, cx);
            }))
            .children(self.projection.models.iter().cloned().map(|model| {
                let selected = self
                    .projection
                    .effective_model()
                    .is_some_and(|(provider, id)| {
                        provider == &model.provider_id && id == &model.id
                    });
                let label = format!("{} / {}", model.provider_id, model.display_name);
                MenuRow::new(SharedString::from(format!(
                    "model-{}-{}",
                    model.provider_id, model.id
                )))
                .label(label)
                .selected(selected)
                .on_click(cx.listener(move |view, _event, _window, cx| {
                    view.on_select_model(model.clone(), cx);
                }))
            }))
    }

    /// Composer 禁用原因（文本说明，不只靠颜色，gui-design §6）。
    fn composer_hint(&self) -> String {
        if self.projection.active_run_id.is_some() {
            return "Run in progress — sending and model switch are disabled. Cancel remains available.".into();
        }
        match &self.projection.connection {
            ConnectionState::Connected { .. } => {
                if self.projection.active_session_id.is_none() {
                    "Open a session to send messages.".into()
                } else {
                    "Enter to send · Shift+Enter for newline".into()
                }
            }
            ConnectionState::Connecting => "Waiting for connection…".into(),
            ConnectionState::Disconnected { .. } => {
                "Disconnected — click Reconnect before sending.".into()
            }
            ConnectionState::Failed { .. } => "Connect failed — click Reconnect.".into(),
        }
    }

    fn grouping_menu_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        let current = self.grouping;
        MenuPanel::new("grouping-menu")
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Grouping, event.position, cx);
            }))
            .children(
                [
                    (TaskRailGrouping::Timeline, "Timeline"),
                    (TaskRailGrouping::Projects, "Projects"),
                ]
                .into_iter()
                .map(|(mode, label)| {
                    let selected = current == mode;
                    MenuRow::new(SharedString::from(format!("group-{label}")))
                        .label(if selected {
                            format!("✓ {label}")
                        } else {
                            format!("  {label}")
                        })
                        .selected(selected)
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_select_grouping(mode, window, cx);
                        }))
                }),
            )
    }

    fn scope_menu_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        let current = self.scope_workspace_id.clone();
        MenuPanel::new("scope-menu")
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::Scope, event.position, cx);
            }))
            .children(self.projection.project_scope_options().into_iter().map(
                |(workspace_id, label)| {
                    let selected = current == workspace_id;
                    let option_id = workspace_id.clone().unwrap_or_else(|| "all".into());
                    MenuRow::new(SharedString::from(format!("scope-{option_id}")))
                        .label(if selected {
                            format!("✓ {label}")
                        } else {
                            label
                        })
                        .selected(selected)
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_select_scope(workspace_id.clone(), window, cx);
                        }))
                },
            ))
    }

    fn task_rail_list(
        &self,
        rail: RailView,
        now_ms: u64,
        can_create: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = match &rail {
            RailView::Timeline(groups) => groups.is_empty(),
            RailView::Projects(groups) => groups.is_empty(),
        };
        let mut list = div()
            .id("task-rail")
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll()
            .gap_1();
        if empty {
            return list.child(
                div().px_2().py_2().child(
                    Label::new("No tasks")
                        .size(font::SM)
                        .color(dark().text.tertiary),
                ),
            );
        }
        match rail {
            RailView::Timeline(groups) => {
                for group in groups {
                    list = list.child(
                        div().px_1().pt_2().child(
                            Label::new(group.bucket.label().to_string())
                                .size(font::XS)
                                .color(dark().text.secondary),
                        ),
                    );
                    for project in group.projects {
                        list = list.child(self.project_block(&project, now_ms, can_create, cx));
                    }
                }
            }
            RailView::Projects(groups) => {
                for project in groups {
                    list = list.child(self.project_block(&project, now_ms, can_create, cx));
                }
            }
        }
        list
    }

    fn project_block(
        &self,
        project: &TaskRailProjectGroup,
        now_ms: u64,
        can_create: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let key = Self::project_key(project.workspace_id.as_deref());
        let expanded = !self.collapsed_projects.contains(&key);
        let workspace_id = project.workspace_id.clone();
        let header_id = SharedString::from(format!("project-{key}"));
        let add_id = SharedString::from(format!("project-add-{key}"));
        let toggle_key = key.clone();
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_1()
            .child(
                ListRow::project_header(header_id)
                    .child(
                        div()
                            .text_size(px(font::SM))
                            .text_color(dark().text.emphasis)
                            .child(format!(
                                "{} {} · {}",
                                if expanded { "▾" } else { "▸" },
                                project.name,
                                project.task_count()
                            )),
                    )
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.on_toggle_project(toggle_key.clone(), window, cx);
                    })),
            );
        if !project.is_unassigned() {
            if let Some(workspace_id) = workspace_id {
                header = header.child(
                    Button::new(add_id)
                        .variant(ButtonVariant::Icon)
                        .disabled(!can_create)
                        .padding(ButtonPadding::None)
                        .width(px(metrics::ICON_SMALL))
                        .height(px(metrics::ICON_SMALL))
                        .label("+")
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_project_add_task(workspace_id.clone(), window, cx);
                        })),
                );
            }
        }
        let mut block = div().flex().flex_col().gap_1().child(header);
        if expanded {
            for task in &project.tasks {
                let session_id = task.session_id.clone();
                let active =
                    self.projection.active_session_id.as_deref() == Some(task.session_id.as_str());
                let running = self
                    .projection
                    .active_runs
                    .iter()
                    .any(|run| run.session_id == task.session_id);
                block = block.child(
                    ListRow::task(SharedString::from(session_id.clone()), active)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_between()
                                .child(format!("{}{}", if running { "● " } else { "" }, task.title))
                                .child(
                                    Label::new(relative_activity(task.updated_at_ms, now_ms))
                                        .size(font::XS)
                                        .color(dark().text.tertiary),
                                ),
                        )
                        .on_click(cx.listener(move |view, _event, window, cx| {
                            view.on_session_clicked(&session_id, window, cx);
                        })),
                );
            }
        }
        block
    }

    fn timeline_entry_element(
        entry: &TimelineEntry,
        menu_open: bool,
        can_fork: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let body = match &entry.kind {
            TimelineEntryKind::UserMessage { text } => div()
                .py_1()
                .text_color(dark().text.primary)
                .child(format!("You: {text}")),
            TimelineEntryKind::AssistantMessage { text } => div()
                .py_1()
                .text_color(dark().text.assistant)
                .child(format!("Assistant: {text}")),
            TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
            } => {
                let mut element = div()
                    .py_1()
                    .text_color(dark().text.tool)
                    .child(format!("{name} · {status}"));
                if let Some(detail) = detail {
                    if !detail.is_empty() {
                        element = element.child(
                            div()
                                .text_size(px(font::XS))
                                .text_color(dark().text.tertiary)
                                .child(detail.clone()),
                        );
                    }
                }
                element
            }
            TimelineEntryKind::RunState(state) => div()
                .py_1()
                .text_color(dark().text.disabled)
                .child(state.clone()),
            TimelineEntryKind::Error(message) => div()
                .py_1()
                .text_color(dark().semantic.danger_text)
                .child(format!("Error: {message}")),
        };
        let event_id = entry.event_id.clone();
        let actions_button = Button::new(format!("entry-menu-{}", entry.event_id))
            .variant(ButtonVariant::Ghost)
            .text_size(font::XS)
            .text_color(dark().text.secondary)
            .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
            .label("···")
            .on_click(cx.listener({
                let event_id = event_id.clone();
                move |view, event, _window, cx| {
                    let down = Self::click_down_position(event);
                    view.toggle_menu(MenuKind::Entry(event_id.clone()), down, cx);
                }
            }));
        let mut actions = Dropdown::new(actions_button);
        if menu_open {
            let fork_id = event_id.clone();
            actions = actions.panel(
                MenuPanel::new(SharedString::from(format!("fork-menu-{}", entry.event_id)))
                    .dismiss_on_outside(cx.listener({
                        let kind = MenuKind::Entry(event_id.clone());
                        move |view, event: &gpui::MouseDownEvent, _, cx| {
                            view.dismiss_menu_on_outside(kind.clone(), event.position, cx)
                        }
                    }))
                    .child(
                        MenuRow::new(SharedString::from(format!("fork-{}", entry.event_id)))
                            .label("Fork")
                            .disabled(!can_fork)
                            .when(can_fork, |row| {
                                row.on_click(cx.listener(move |view, _event, _window, cx| {
                                    view.close_open_menu(cx);
                                    view.on_fork(&fork_id, cx);
                                }))
                            }),
                    ),
            );
        }
        div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap_2()
            .child(div().flex_1().child(body))
            .child(actions)
    }

    fn inspector_element(&self, connected: bool, cx: &mut Context<Self>) -> Panel {
        let terminal = &self.projection.terminal;
        let output = if terminal.output.is_empty() {
            "Terminal output will appear here. No local PTY — host streams TerminalOutput."
                .to_string()
        } else {
            terminal.output.clone()
        };
        let size_label = format!("{}×{}", terminal.columns, terminal.rows);
        let cwd = terminal.cwd.clone();
        let started = terminal.session_id.is_some();
        Panel::side_left(px(metrics::INSPECTOR_WIDTH))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(dark().border.subtle)
                    .child(
                        div()
                            .id("inspector-tab-terminal")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(dark().surface.raised)
                            .text_size(px(font::SM))
                            .child("Terminal"),
                    )
                    .child(
                        Button::new("inspector-collapse")
                            .variant(ButtonVariant::Ghost)
                            .text_size(font::SM)
                            .text_color(dark().text.secondary)
                            .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                            .label("⟩")
                            .on_click(cx.listener(|view, _event, window, cx| {
                                view.on_toggle_inspector(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .text_size(px(font::XS))
                    .text_color(dark().text.secondary)
                    .child(format!("cwd {cwd}"))
                    .child(
                        Button::new("terminal-resize")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::None)
                            .label(size_label)
                            .on_click(cx.listener(|view, _event, window, cx| {
                                view.on_apply_terminal_size(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("terminal-output-area")
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(
                        div()
                            .id("terminal-output")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .track_scroll(self.terminal_scroll.handle())
                            .overflow_y_scroll()
                            .px_2()
                            .py_1()
                            .text_size(px(font::SM))
                            .text_color(dark().text.emphasis)
                            .on_scroll_wheel(cx.listener(|view, _event, _window, cx| {
                                view.terminal_scroll.on_scroll_wheel();
                                cx.notify();
                            }))
                            .child(output),
                    )
                    .when(!self.terminal_scroll.is_following(), |area| {
                        area.child(BackToBottom::new(
                            Button::new("terminal-back-to-bottom")
                                .variant(ButtonVariant::Raised)
                                .label("↓ 回到底部")
                                .on_click(cx.listener(|view, _event, _window, cx| {
                                    view.terminal_scroll.jump_to_bottom();
                                    cx.notify();
                                })),
                        ))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .p_2()
                    .border_t_1()
                    .border_color(dark().border.subtle)
                    .child(div().flex_1().child(self.terminal_input.clone()))
                    .child(
                        // 迁移前 terminal-start 未设文字色（继承 text.primary），
                        // 禁用态亦保持同色，显式钉住避免 Raised 默认的 disabled 色。
                        Button::new("terminal-start")
                            .variant(ButtonVariant::Raised)
                            .disabled(!connected)
                            .text_size(font::XS)
                            .text_color(dark().text.primary)
                            .disabled_text_color(dark().text.primary)
                            .label(if started { "Size" } else { "Start" })
                            .on_click(cx.listener(move |view, _event, window, cx| {
                                if started {
                                    view.on_apply_terminal_size(window, cx);
                                } else {
                                    view.on_start_terminal(window, cx);
                                }
                            })),
                    ),
            )
    }

    fn composer_workspace_label(&self) -> String {
        if let Some(session_id) = &self.projection.active_session_id {
            if let Some(session) = self
                .projection
                .sessions
                .iter()
                .find(|session| &session.session_id == session_id)
            {
                return format!(
                    "Workspace · {}",
                    self.projection
                        .workspace_name(session.workspace_id.as_deref())
                );
            }
        }
        match self.scope_workspace_id.as_deref() {
            Some(id) => format!("Workspace · {}", self.projection.workspace_name(Some(id))),
            None => "Workspace · confirm in All projects".into(),
        }
    }

    fn workspace_confirm_element(&self, cx: &mut Context<Self>) -> MenuPanel {
        let choices: Vec<(String, String)> = self
            .projection
            .project_scope_options()
            .into_iter()
            .filter_map(|(id, name)| id.map(|id| (id, name)))
            .collect();
        let mut panel = MenuPanel::new("workspace-confirm")
            .dismiss_on_outside(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                view.dismiss_menu_on_outside(MenuKind::WorkspaceConfirm, event.position, cx);
            }));
        if choices.is_empty() {
            return panel.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(font::SM))
                    .text_color(dark().semantic.warning_text)
                    .child("Add a workspace before creating a task."),
            );
        }
        for (id, name) in choices {
            let pick = id.clone();
            panel = panel.child(
                MenuRow::new(SharedString::from(format!("workspace-confirm-{id}")))
                    .label(name)
                    .on_click(cx.listener(move |view, _event, window, cx| {
                        view.on_confirm_workspace(pick.clone(), window, cx);
                    })),
            );
        }
        panel
    }
}

fn resolve_new_task_workspace(scope_workspace_id: Option<&str>) -> Option<&str> {
    scope_workspace_id
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let can_send = self.can_send();
        let can_cancel = self.can_cancel();
        let can_switch_model = self.can_switch_model();
        // can_switch_model 翻假期间归一化：打开中的 model 菜单随之关闭，
        // 避免条件恢复后面板无需点击自行重现。
        if matches!(self.open_menu, Some(MenuKind::Model)) && !can_switch_model {
            self.open_menu = None;
        }
        let can_approve = self.can_approve();
        let model_label = self.model_label();
        let model_menu_open = matches!(self.open_menu, Some(MenuKind::Model)) && can_switch_model;
        let connection_label = match &self.projection.connection {
            ConnectionState::Connected { .. } => match self.projection.resume.label() {
                Some(resume) => format!("Local · Connected · {resume}"),
                None => "Local · Connected".into(),
            },
            other => other.label(),
        };
        let composer_hint = self.composer_hint();
        let context_meter = self.projection.context_meter_label();
        let now_ms = now_unix_ms();
        let run_status = self.projection.run_status_label(now_ms);
        let can_create = self.can_create_task();
        let grouping_glyph = match self.grouping {
            TaskRailGrouping::Timeline => "◷",
            TaskRailGrouping::Projects => "▤",
        };
        let scope_label = self.scope_label();
        let grouping_menu_open = matches!(self.open_menu, Some(MenuKind::Grouping));
        let scope_menu_open = matches!(self.open_menu, Some(MenuKind::Scope));
        let rail_groups = match self.grouping {
            TaskRailGrouping::Timeline => RailView::Timeline(
                self.projection
                    .timeline_groups(self.scope_workspace_id.as_deref(), now_ms),
            ),
            TaskRailGrouping::Projects => RailView::Projects(
                self.projection
                    .project_groups(self.scope_workspace_id.as_deref()),
            ),
        };

        let grouping_tooltip = SharedString::from(self.grouping.accessible_name());
        let grouping_button = Button::new("task-rail-grouping")
            .variant(ButtonVariant::Raised)
            .padding(ButtonPadding::None)
            .width(px(metrics::ICON_LARGE))
            .height(px(metrics::ICON_MEDIUM))
            .text_size(font::SM)
            .label(format!("{grouping_glyph} ▾"))
            .tooltip(grouping_tooltip)
            .on_click(cx.listener(|view, event, window, cx| {
                let down = Self::click_down_position(event);
                view.on_toggle_grouping_menu(down, window, cx);
            }));
        let mut grouping = Dropdown::new(grouping_button);
        if grouping_menu_open {
            grouping = grouping.panel(self.grouping_menu_element(cx));
        }

        let scope_button = Button::new("project-scope")
            .variant(ButtonVariant::Raised)
            .text_size(font::SM)
            .label(format!("{scope_label} ▾"))
            .on_click(cx.listener(|view, event, window, cx| {
                let down = Self::click_down_position(event);
                view.on_toggle_scope_menu(down, window, cx);
            }));
        let mut scope = Dropdown::new(scope_button);
        if scope_menu_open {
            scope = scope.panel(self.scope_menu_element(cx));
        }

        let add_task_tooltip = if can_create {
            SharedString::from("New task (⌘N)")
        } else {
            SharedString::from(self.add_task_disabled_reason())
        };
        let add_task_focus = self.add_task_focus.clone();
        let add_task = Button::new("add-task")
            .track_focus(&add_task_focus)
            .variant(ButtonVariant::Icon)
            .disabled(!can_create)
            .padding(ButtonPadding::None)
            .width(px(metrics::ICON_MEDIUM))
            .height(px(metrics::ICON_MEDIUM))
            .label("+")
            .tooltip(add_task_tooltip)
            .on_click(cx.listener(|view, _event, window, cx| {
                view.on_new_session(window, cx);
            }));

        let mut sidebar = Panel::side_right(px(metrics::SIDEBAR_WIDTH))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new("Pawork")
                            .size(font::BASE)
                            .color(dark().text.primary),
                    )
                    .child(grouping),
            )
            .child(scope)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(Badge::new(connection_label))
                    .child(add_task),
            );
        if !connected {
            sidebar = sidebar.child(
                Button::new("reconnect")
                    .variant(ButtonVariant::Primary)
                    .label("Reconnect")
                    .on_click(cx.listener(|view, _event, window, cx| {
                        view.on_reconnect(window, cx);
                    })),
            );
        }
        let sidebar = sidebar
            .child(self.task_rail_list(rail_groups, now_ms, can_create, cx))
            .child(
                div().mt_auto().pt_2().child(
                    Label::new("Local")
                        .size(font::XS)
                        .color(dark().text.tertiary),
                ),
            );

        let fork_available = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_session_id.is_some();
        let open_entry_menu = match &self.open_menu {
            Some(MenuKind::Entry(event_id)) => Some(event_id.clone()),
            _ => None,
        };
        let timeline = div()
            .id("timeline")
            .flex()
            .flex_col()
            .track_scroll(self.timeline_scroll.handle())
            .flex_1()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .gap_1()
            // 用户滚动重估跟随：滚回底部自动重挂（§8.3）。
            .on_scroll_wheel(cx.listener(|view, _event, _window, cx| {
                view.timeline_scroll.on_scroll_wheel();
                cx.notify();
            }))
            .children(self.projection.timeline.iter().map(|entry| {
                Self::timeline_entry_element(
                    entry,
                    open_entry_menu.as_deref() == Some(entry.event_id.as_str()),
                    fork_available && entry.is_fork_boundary(),
                    cx,
                )
            }))
            .when(self.projection.pending_approval.is_some(), |timeline| {
                let pending = self
                    .projection
                    .pending_approval
                    .as_ref()
                    .expect("pending approval exists");
                let mut card = div()
                    .p_2()
                    .rounded_md()
                    .border_l_1()
                    .border_color(dark().semantic.warning_border)
                    .bg(dark().semantic.warning_bg)
                    .child(
                        div()
                            .text_size(px(font::SM))
                            .text_color(dark().semantic.warning_text)
                            .child(format!("Approval · {}", pending.tool_name)),
                    )
                    .child(
                        div()
                            .text_size(px(font::SM))
                            .text_color(dark().text.primary)
                            .child(pending.reason.clone()),
                    );
                if let Some(detail) = pending.detail.clone() {
                    if !detail.is_empty() {
                        card = card.child(
                            div()
                                .text_size(px(font::XS))
                                .text_color(dark().text.detail)
                                .child(detail),
                        );
                    }
                }
                let buttons = [
                    (
                        "approve-once",
                        "Allow once",
                        "approve_once",
                        ButtonVariant::Primary,
                    ),
                    (
                        "approve-for-run",
                        "Allow for run",
                        "approve_for_run",
                        ButtonVariant::Success,
                    ),
                    ("approve-deny", "Deny", "deny", ButtonVariant::Danger),
                ];
                let approve_once_focus = self.approve_once_focus.clone();
                let approve_for_run_focus = self.approve_for_run_focus.clone();
                let deny_focus = self.deny_focus.clone();
                let approve_disabled = SharedString::from(self.approve_disabled_reason());
                let row = div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .children(buttons.into_iter().map(|(id, label, decision, variant)| {
                        let decision = decision.to_string();
                        let focus = match id {
                            "approve-once" => approve_once_focus.clone(),
                            "approve-for-run" => approve_for_run_focus.clone(),
                            _ => deny_focus.clone(),
                        };
                        let tooltip = if can_approve {
                            SharedString::from(match id {
                                "approve-once" => "Allow once (⌘1 / ⌘↩)",
                                "approve-for-run" => "Allow for run (⌘2)",
                                _ => "Deny (⌘3)",
                            })
                        } else {
                            approve_disabled.clone()
                        };
                        let mut button = Button::new(id)
                            .variant(variant)
                            .disabled(!can_approve)
                            .track_focus(&focus)
                            .label(label)
                            .tooltip(tooltip);
                        if can_approve {
                            button =
                                button.on_click(cx.listener(move |view, _event, _window, cx| {
                                    view.on_approve(&decision, cx);
                                }));
                        }
                        button
                    }));
                timeline.child(card.child(row))
            });

        // 脱钩时右下浮出回底控件（§8.3）；跟随态隐藏。
        let timeline_following = self.timeline_scroll.is_following();
        let timeline_area = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .child(timeline)
            .when(!timeline_following, |area| {
                area.child(BackToBottom::new(
                    Button::new("timeline-back-to-bottom")
                        .variant(ButtonVariant::Raised)
                        .label("↓ 回到底部")
                        .on_click(cx.listener(|view, _event, _window, cx| {
                            view.timeline_scroll.jump_to_bottom();
                            cx.notify();
                        })),
                ))
            });

        let model_tooltip = if can_switch_model {
            SharedString::from("Select model")
        } else {
            SharedString::from(self.model_disabled_reason())
        };
        let model_focus = self.model_focus.clone();
        let mut model_button = Button::new("model-picker")
            .track_focus(&model_focus)
            .variant(ButtonVariant::Raised)
            .disabled(!can_switch_model)
            .label(model_label)
            .tooltip(model_tooltip);
        if can_switch_model {
            model_button = model_button.on_click(cx.listener(|view, event, window, cx| {
                let down = Self::click_down_position(event);
                view.on_toggle_model_menu(down, window, cx);
            }));
        }
        let mut model_picker = Dropdown::new(model_button);
        if model_menu_open {
            model_picker = model_picker.panel(self.model_menu_element(cx));
        }

        let composer = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(dark().border.subtle)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(model_picker)
                    .child(
                        div().flex_1().child(
                            Label::new(context_meter)
                                .size(font::XS)
                                .color(dark().text.secondary),
                        ),
                    )
                    .child(
                        Label::new(self.composer_workspace_label())
                            .size(font::XS)
                            .color(dark().text.secondary),
                    ),
            )
            .when(
                matches!(self.open_menu, Some(MenuKind::WorkspaceConfirm)),
                |composer| {
                    // 无触发器：锚在 label 行正下方（原 in-flow 位置），浮层化不占布局流。
                    composer.child(
                        deferred(
                            anchored()
                                .anchor(Corner::TopLeft)
                                .offset(point(px(0.), px(ANCHOR_GAP_Y)))
                                .child(self.workspace_confirm_element(cx)),
                        )
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(metrics::COMPOSER_MIN_HEIGHT))
                            .max_h(px(metrics::COMPOSER_MAX_HEIGHT))
                            .child(self.text_input.clone()),
                    )
                    .child({
                        let cancel_focus = self.cancel_focus.clone();
                        let cancel_tooltip = if can_cancel {
                            SharedString::from("Cancel run (⌘.)")
                        } else {
                            SharedString::from(self.cancel_disabled_reason())
                        };
                        let mut cancel = Button::new("cancel")
                            .variant(ButtonVariant::Danger)
                            .disabled(!can_cancel)
                            .track_focus(&cancel_focus)
                            .padding(ButtonPadding::Wide)
                            .label("Cancel")
                            .tooltip(cancel_tooltip);
                        if can_cancel {
                            cancel = cancel.on_click(cx.listener(|view, _event, window, cx| {
                                view.on_cancel_clicked(window, cx);
                            }));
                        }
                        cancel
                    })
                    .child({
                        let send_focus = self.send_focus.clone();
                        let send_tooltip = if can_send {
                            SharedString::from("Send message (Enter)")
                        } else {
                            SharedString::from(composer_hint.clone())
                        };
                        let mut send = Button::new("send")
                            .variant(ButtonVariant::Primary)
                            .disabled(!can_send)
                            .track_focus(&send_focus)
                            .padding(ButtonPadding::Wide)
                            .label("Send")
                            .tooltip(send_tooltip);
                        if can_send {
                            send = send.on_click(cx.listener(|view, _event, window, cx| {
                                view.on_send_clicked(window, cx);
                            }));
                        }
                        send
                    }),
            )
            .child(
                Label::new(self.status_hint.clone().unwrap_or(composer_hint))
                    .size(font::XS)
                    .color(dark().text.secondary),
            );

        let inspector_open = self.inspector_open;
        let workspace = div()
            .flex()
            .flex_col()
            .flex_1()
            .child(timeline_area)
            .child(composer);

        let mut main = div().flex().flex_row().flex_1().child(workspace);
        if inspector_open {
            main = main.child(self.inspector_element(connected, cx));
        }

        div()
            .key_context("AppView")
            .track_focus(&self.focus_handle)
            .flex()
            .size_full()
            .bg(dark().bg.base)
            .text_color(dark().text.primary)
            // Escape 关闭浮层菜单（不动全局 keybinding 字面量）：焦点在窗口内任意
            // 元素时经冒泡到达根节点；面板经 deferred 绘制、不可聚焦，组件层
            // on_key_down 不可达，根节点为唯一机制。
            .on_key_down(cx.listener(
                |view: &mut Self,
                 event: &KeyDownEvent,
                 _window: &mut Window,
                 cx: &mut Context<Self>| {
                    if event.keystroke.key == "escape" {
                        view.close_open_menu(cx);
                    }
                },
            ))
            .on_action(cx.listener(Self::on_send_message))
            .on_action(cx.listener(Self::on_approve_once))
            .on_action(cx.listener(Self::on_approve_for_run))
            .on_action(cx.listener(Self::on_deny))
            .on_action(cx.listener(Self::on_cancel_run))
            .on_action(cx.listener(Self::on_new_task_action))
            .on_action(cx.listener(Self::on_toggle_inspector_action))
            .child(sidebar)
            .child(
                div().flex().flex_col().flex_1().child(main).child(
                    StatusBar::new().child(Badge::new(run_status)).child(
                        Button::new("inspector-toggle")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                            .label(if inspector_open {
                                "Hide inspector"
                            } else {
                                "Inspector"
                            })
                            .on_click(cx.listener(|view, _event, window, cx| {
                                view.on_toggle_inspector(window, cx);
                            })),
                    ),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybinding_table_includes_approval_and_cancel() {
        let actions: Vec<&str> = APP_VIEW_KEYBINDINGS
            .iter()
            .map(|(_, action)| *action)
            .collect();
        assert!(actions.contains(&"ApproveOnce"));
        assert!(actions.contains(&"ApproveForRun"));
        assert!(actions.contains(&"Deny"));
        assert!(actions.contains(&"CancelRun"));
        assert!(APP_VIEW_KEYBINDINGS
            .iter()
            .any(|(key, action)| *key == "cmd-." && *action == "CancelRun"));
        assert!(APP_VIEW_KEYBINDINGS
            .iter()
            .any(|(key, action)| *key == "cmd-enter" && *action == "ApproveOnce"));
    }

    #[test]
    fn main_path_buttons_are_marked_tab_stops() {
        for id in [
            "approve-once",
            "approve-for-run",
            "approve-deny",
            "cancel",
            "send",
            "add-task",
            "model-picker",
        ] {
            assert!(
                MAIN_PATH_TAB_STOP_IDS.contains(&id),
                "missing tab_stop marker for {id}"
            );
        }
    }

    #[test]
    fn all_projects_new_task_requires_workspace_confirm() {
        assert!(resolve_new_task_workspace(None).is_none());
        assert_eq!(resolve_new_task_workspace(Some("ws-a")), Some("ws-a"));
    }
}
