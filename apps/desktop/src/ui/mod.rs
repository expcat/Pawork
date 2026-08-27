//! UI 层：AppView 宿主（连接 / 事件消费 / 状态与动作接线）与整体渲染装配。
//! 渲染块自本模块外移（R8 波 C）：TaskRail → task_rail、Timeline 虚拟化 →
//! timeline（条目 → timeline_entry、审批卡 → approval_card）、Inspector →
//! inspector、Composer → input_area。

mod accessibility;
mod approval_card;
mod barriers;
mod changes;
mod components;
mod input_area;
mod inspector;
mod resources;
mod shell_layout;
mod task_rail;
pub mod text_input;
mod theme;
mod timeline;
mod timeline_entry;
#[cfg(test)]
mod u1_probe;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, AnyView, App, ClickEvent, Context, Entity, FocusHandle,
    Focusable, KeyBinding, KeyDownEvent, ListAlignment, ListState, Pixels, Point, Render,
    SharedString, Window,
};
use pawork_client::AppEvent;

use crate::controller::{ControllerEvent, DesktopController};
use crate::platform::Platform;
use crate::projection::{ConnectionState, DesktopProjection, ResumeApply, TaskRailGrouping};
use barriers::BarrierSink;
use changes::ChangesPanelState;
use components::button::{Button, ButtonPadding, ButtonVariant};
use components::dropdown::Dropdown;
use components::follow_scroll::FollowScroll;
use components::label::Badge;
use components::status_bar::StatusBar;
use inspector::InspectorTab;
use resources::ResourcesPanelState;
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

/// Timeline 空态引导（R2 Wave B）：无 active session 且条目数为 0 时居中
/// 显示；视觉与 AX 树共用同一文案源（accessibility/app.rs）。
pub(crate) const WORKSPACE_EMPTY_HINT: &str =
    "Select a task from the rail, or press ⌘N to start a new one.";

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
    /// Inspector 折叠态的 ActivityPopover（StatusBar Inspector 触发器弹出）。
    Activity,
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
    /// Timeline 虚拟化状态（Bottom 对齐钉底；跟随语义见 ui/timeline.rs）。
    timeline_list: ListState,
    timeline_following: bool,
    /// timeline 数据 / 宽度变更代次；render 时对齐到 list（reset 语义）。
    timeline_rev: u64,
    timeline_list_rev: u64,
    timeline_list_count: usize,
    terminal_scroll: FollowScroll,
    status_hint: Option<String>,
    grouping: TaskRailGrouping,
    scope_workspace_id: Option<String>,
    collapsed_projects: BTreeSet<String>,
    inspector_open: bool,
    /// Inspector 顶层页签（Changes / Terminal / Resources）。
    inspector_tab: InspectorTab,
    /// Changes 面状态（Files / Summary、清单与选中 diff、滚动句柄）。
    changes: ChangesPanelState,
    /// Resources 面状态（MCP server 清单、滚动句柄）。
    resources: ResourcesPanelState,
    /// 当前打开的菜单；单一状态位保证至多一个打开（§8.2）。
    open_menu: Option<MenuKind>,
    /// 同一次物理点击里「外点关闭先于触发器 click」的衔接标记（菜单种类 +
    /// 按下位置）：触发器 toggle 仅当 click 的按下位置与标记相同（同一次
    /// 物理点击，ClickEvent 自带 down）才视为「再点触发器关闭」的收尾不再
    /// 重开；位置不等或键盘触发则为新点击，清标记后正常 toggle
    /// （见 dismiss_menu_on_outside）。
    pending_outside_close: Option<(MenuKind, Point<Pixels>)>,
    run_clock_running: bool,
    /// R1 Wave B fixture barrier 状态（PAWORK_UI_BARRIER_DIR 未设置则
    /// 零开销直通；发射语义见 ui/barriers.rs）。
    barriers: BarrierSink,
    /// macOS 原生 accessibility bridge；非 macOS 为零行为占位。
    ax_bridge: Option<accessibility::AxBridge>,
    /// 避免同一 AX 投影错误在每帧重复刷屏。
    ax_error_reported: bool,
    /// session_get 分页是否进行中（open_session 置位，complete / 失败复位）。
    timeline_paging: bool,
    /// 距上个 1s tick 是否有新 ControllerEvent（有则本 tick 视为未静默）。
    controller_event_pending: bool,
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
    pub fn new(
        platform: Arc<Platform>,
        socket: PathBuf,
        barrier_dir: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
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
            timeline_list: ListState::new(
                0,
                ListAlignment::Bottom,
                px(timeline::TIMELINE_OVERDRAW),
            ),
            timeline_following: true,
            timeline_rev: 1,
            timeline_list_rev: 0,
            timeline_list_count: 0,
            terminal_scroll: FollowScroll::new(),
            status_hint: None,
            grouping: TaskRailGrouping::Timeline,
            scope_workspace_id: None,
            collapsed_projects: BTreeSet::new(),
            inspector_open: true,
            inspector_tab: InspectorTab::default(),
            changes: ChangesPanelState::default(),
            resources: ResourcesPanelState::default(),
            open_menu: None,
            pending_outside_close: None,
            run_clock_running: false,
            barriers: BarrierSink::new(barrier_dir),
            ax_bridge: None,
            ax_error_reported: false,
            timeline_paging: false,
            controller_event_pending: false,
            focus_handle: cx.focus_handle(),
            approve_once_focus: cx.focus_handle().tab_stop(true),
            approve_for_run_focus: cx.focus_handle().tab_stop(true),
            deny_focus: cx.focus_handle().tab_stop(true),
            cancel_focus: cx.focus_handle().tab_stop(true),
            send_focus: cx.focus_handle().tab_stop(true),
            add_task_focus: cx.focus_handle().tab_stop(true),
            model_focus: cx.focus_handle().tab_stop(true),
        };
        timeline::install_scroll_follow(&view.timeline_list, &cx.weak_entity());
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
        self.barriers.remove_timeline_stable();
        self.barriers.remove_approval_visible();
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
        self.timeline_changed();
        self.controller.load_models();
        self.consume_events(events, cx);
        // 连接建立即武装 1s tick：barrier 启用而无 run 时也要常驻探测。
        self.arm_run_clock(cx);
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
        self.controller_event_pending = true;
        // 任一新事件都会使上一轮 settle 失效；下一次静默窗口重新写入。
        self.barriers.remove_timeline_stable();
        self.barriers.remove_approval_visible();
        match event {
            ControllerEvent::Disconnected { reason } => {
                self.projection
                    .set_connection(ConnectionState::Disconnected { reason });
                // 断连终止一切进行中分页，避免 settle barrier 永久停发。
                self.timeline_paging = false;
                self.status_hint = Some("Connection lost. Click Reconnect.".into());
            }
            ControllerEvent::Snapshot(snapshot) => {
                self.projection.merge_snapshot(&snapshot);
                self.timeline_changed();
            }
            ControllerEvent::TimelineLoaded { session_id, page } => {
                if self.projection.active_session_id.as_deref() == Some(&session_id) {
                    self.timeline_changed();
                    self.projection.apply_timeline_page(&page);
                    if page.complete {
                        self.timeline_paging = false;
                    }
                }
            }
            ControllerEvent::Event(envelope) => {
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
                if !self.inspector_open {
                    // Inspector 即将展开：Timeline 宽度变化 → 条目换行高度变，
                    // 须 reset。
                    self.timeline_changed();
                }
                // 程序化展开 Inspector：关闭可能悬浮的菜单（P3-1 泄漏修复）。
                self.close_open_menu(cx);
                self.inspector_open = true;
                self.refresh_open_inspector_tab(cx);
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
                if action == "open session" {
                    self.timeline_paging = false;
                }
                // 查询失败回写对应面板状态：避免 Changes / Resources 永远停在
                // Loading（status_hint 仍照常提示）；仅当前仍在 Fetching 才落
                // Failed，防止旧请求的失败覆盖新一轮刷新。
                match action {
                    "load changes" => {
                        if self.changes.fetch == changes::ChangesFetch::Fetching {
                            self.changes.mark_failed(&reason);
                        }
                    }
                    "load diff" => {
                        if self.changes.diff == changes::DiffFetch::Fetching {
                            self.changes.mark_diff_failed(&reason);
                        }
                    }
                    "load resources" => {
                        if self.resources.fetch == resources::ResourcesFetch::Fetching {
                            self.resources.mark_failed(&reason);
                        }
                    }
                    _ => {}
                }
                self.status_hint = Some(format!("{action} failed: {reason}"));
            }
            ControllerEvent::DiffFilesLoaded {
                epoch,
                session_id,
                files,
                git,
            } => {
                if self.changes.apply_files(epoch, session_id, files, git) {
                    // 清单刷新后选中文件仍在：重拉它的 diff，保持两视图一致。
                    if let Some(path) = self.changes.selected.clone() {
                        self.fetch_diff(&path, cx);
                    }
                }
            }
            ControllerEvent::DiffContentLoaded { epoch, path, file } => {
                self.changes.apply_diff(epoch, &path, file);
            }
            ControllerEvent::McpServersLoaded { epoch, servers } => {
                self.resources.apply_servers(epoch, servers);
            }
        }
        self.arm_run_clock(cx);
        cx.notify();
    }

    fn arm_run_clock(&mut self, cx: &mut Context<Self>) {
        // run 进行中驱动时长徽标重绘；barrier 启用时兼作 settle 探测心跳。
        if self.run_clock_running
            || (self.projection.active_run_id.is_none() && !self.barriers.is_active())
        {
            return;
        }
        self.run_clock_running = true;
        cx.spawn(async move |this, cx| loop {
            smol::Timer::after(Duration::from_secs(1)).await;
            let keep = this
                .update(cx, |view, cx| {
                    view.emit_settle_barriers();
                    if view.projection.active_run_id.is_some() {
                        cx.notify();
                        true
                    } else if view.barriers.is_active() {
                        // barrier 常驻心跳：无 run 时静默续 tick（不重绘）。
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

    /// 1s tick 的 barrier 发射（PAWORK_UI_BARRIER_DIR 未设置时零开销直通）。
    /// 静默条件：已连接 && 无进行中 timeline 分页 && 本 tick 窗口内无未消费
    /// ControllerEvent（时间线已静默 ≥1s，见 Wave B brief §6/§7）。
    fn emit_settle_barriers(&mut self) {
        if !self.barriers.is_active() {
            return;
        }
        if std::mem::take(&mut self.controller_event_pending) {
            return;
        }
        let settled = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && !self.timeline_paging;
        if settled {
            let session_id = self
                .projection
                .active_session_id
                .clone()
                .unwrap_or_default();
            let entry_count = self.projection.timeline.len();
            self.barriers
                .write_timeline_stable(&session_id, entry_count);
        }
        let pending_approval = self
            .projection
            .pending_approval
            .as_ref()
            .map(|pending| (pending.tool_name.clone(), pending.run_id.clone()));
        match pending_approval {
            Some((tool_name, run_id)) => {
                if settled {
                    self.barriers.write_approval_visible(&tool_name, &run_id);
                }
            }
            // 审批卡消失 → 删除 barrier 文件（镜像消失语义，仅 barrier 目录内）。
            None => self.barriers.remove_approval_visible(),
        }
    }

    fn open_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        self.projection.select_session(&session_id);
        // session_get 分页开始：complete / open session 失败前不写 settle barrier。
        self.timeline_paging = true;
        self.barriers.remove_timeline_stable();
        self.barriers.remove_approval_visible();
        self.status_hint = None;
        self.timeline_changed();
        // 打开 / 切换 session 时补跟随重置（§8.3）：终端滚底 + Timeline 回
        // 跟随态。缺后者时旧会话脱钩读史的偏移与 following=false 会泄漏进
        // 新会话（sync_list 按旧 item_ix 恢复视口，新输出不再自动滚底）。
        self.terminal_scroll.jump_to_bottom();
        self.timeline_following = true;
        // 会话切换：清空旧会话 diff 状态并重新拉取（拉取时机之一）。
        self.changes.reset_for_session();
        self.controller.open_session(session_id);
        self.refresh_changes(cx);
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

    fn on_reconnect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.start_connect(cx);
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

    fn on_toggle_inspector(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.inspector_open = !self.inspector_open;
        // 宽度变化改变条目换行高度：list 高度缓存须失效（reset）。
        self.timeline_changed();
        if self.inspector_open {
            // 展开时关闭可能悬浮的菜单（如 ActivityPopover），避免面板
            // 叠在已展开的 Inspector 上（P3-1 泄漏修复）。
            self.close_open_menu(cx);
            // Inspector 展开：刷新当前页签数据（拉取时机之一）。
            self.refresh_open_inspector_tab(cx);
        }
        cx.notify();
    }

    /// 切换 Inspector 顶层页签；切入 Changes / Resources 时拉取数据
    /// （拉取时机之一）。切页签不改 active session；各页签滚动状态独立保留。
    fn select_inspector_tab(&mut self, tab: InspectorTab, cx: &mut Context<Self>) {
        if self.inspector_tab == tab {
            return;
        }
        self.inspector_tab = tab;
        self.refresh_open_inspector_tab(cx);
        cx.notify();
    }

    /// 展开中的 Inspector 当前页签对应的数据刷新（Terminal 无查询面）。
    fn refresh_open_inspector_tab(&mut self, cx: &mut Context<Self>) {
        if !self.inspector_open {
            return;
        }
        match self.inspector_tab {
            InspectorTab::Changes => self.refresh_changes(cx),
            InspectorTab::Resources => self.refresh_resources(cx),
            InspectorTab::Terminal => {}
        }
    }

    fn on_select_changes_tab(&mut self, tab: changes::ChangesTab, cx: &mut Context<Self>) {
        if self.changes.tab == tab {
            return;
        }
        self.changes.tab = tab;
        cx.notify();
    }

    fn on_select_diff_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.changes.selected.as_deref() == Some(path) {
            return;
        }
        self.fetch_diff(path, cx);
    }

    /// ActivityPopover 摘要行：展开 Inspector 并定位 Changes 页。
    fn on_activity_open_changes(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.close_open_menu(cx);
        if !self.inspector_open {
            self.inspector_open = true;
            self.timeline_changed();
        }
        self.inspector_tab = InspectorTab::Changes;
        self.refresh_changes(cx);
        cx.notify();
    }

    /// 拉取会话 diff 文件清单（diff_list_files）。失败时诚实标记状态。
    fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let workspace = self
            .scope_workspace_id
            .clone()
            .or_else(|| self.projection.workspace_id.clone());
        match (connected, workspace) {
            (true, Some(workspace)) => {
                let epoch = self.changes.begin_refresh();
                self.controller.diff_list_files(workspace, epoch);
            }
            (true, None) => self.changes.mark_failed("no workspace"),
            _ => self.changes.mark_failed("not connected"),
        }
        cx.notify();
    }

    /// 拉取选中文件的 diff（diff_get）。
    fn fetch_diff(&mut self, path: &str, cx: &mut Context<Self>) {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let workspace = self
            .scope_workspace_id
            .clone()
            .or_else(|| self.projection.workspace_id.clone());
        match (connected, workspace) {
            (true, Some(workspace)) => {
                let epoch = self.changes.begin_diff_fetch(path);
                self.controller
                    .diff_get(workspace, path.to_string(), epoch);
            }
            (true, None) => self.changes.mark_diff_failed("no workspace"),
            _ => self.changes.mark_diff_failed("not connected"),
        }
        cx.notify();
    }

    /// 拉取 MCP server 清单（mcp_list）。
    fn refresh_resources(&mut self, cx: &mut Context<Self>) {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        if connected {
            let epoch = self.resources.begin_refresh();
            self.controller.mcp_list(epoch);
        } else {
            self.resources.mark_failed("not connected");
        }
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

    /// Timeline 数据 / 可视宽度变更标记：下一次 render 时对 list 做一次
    /// reset（projection 有条目替换语义，splice 不安全，见 ui/timeline.rs）。
    fn timeline_changed(&mut self) {
        self.timeline_rev += 1;
    }
}

fn resolve_new_task_workspace(scope_workspace_id: Option<&str>) -> Option<&str> {
    scope_workspace_id
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_accessibility(window, cx);
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let can_switch_model = self.can_switch_model();
        // can_switch_model 翻假期间归一化：打开中的 model 菜单随之关闭，
        // 避免条件恢复后面板无需点击自行重现。
        if matches!(self.open_menu, Some(MenuKind::Model)) && !can_switch_model {
            self.open_menu = None;
        }
        let now_ms = now_unix_ms();
        let run_status = self.projection.run_status_label(now_ms);
        // R2 Wave A 响应式合同：窄窗（≤1279）rail 240 + Inspector 折叠为
        // ActivityPopover 抽屉；偏好值保留，加宽后自动恢复（shell_layout）。
        let shell = shell_layout::resolve(window.viewport_size().width, self.inspector_open);
        let inspector_open = shell.inspector_open;
        let activity_popover_open =
            !inspector_open && matches!(self.open_menu, Some(MenuKind::Activity));

        let sidebar = self.sidebar_element(px(shell.rail_width), cx);
        let timeline_area = self.timeline_area(cx);
        let composer = self.composer_element(cx);
        let workspace = div()
            .id("shell-workspace")
            .debug_selector(|| "shell-workspace".into())
            .flex()
            .flex_col()
            .flex_1()
            .child(timeline_area)
            .child(composer);

        let mut main = div().flex().flex_row().flex_1().child(workspace);
        if inspector_open {
            main = main.child(
                div()
                    .id("shell-inspector")
                    .debug_selector(|| "shell-inspector".into())
                    .flex()
                    .child(self.inspector_element(connected, cx)),
            );
        }
        // Inspector 展开时触发器直接折叠；折叠时同一触发器弹出
        // ActivityPopover（§8.5），摘要行点击才展开并定位 Changes。
        let inspector_trigger = if inspector_open {
            Button::new("inspector-toggle")
                .variant(ButtonVariant::Ghost)
                .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                .label("Hide inspector")
                .on_click(cx.listener(|view, _event, window, cx| {
                    view.on_toggle_inspector(window, cx);
                }))
                .into_any_element()
        } else {
            let trigger = Button::new("inspector-toggle")
                .variant(ButtonVariant::Ghost)
                .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                .label("Inspector")
                .on_click(cx.listener(|view, event, _window, cx| {
                    let down = Self::click_down_position(event);
                    view.toggle_menu(MenuKind::Activity, down, cx);
                }));
            let mut dropdown = Dropdown::new(trigger);
            if activity_popover_open {
                dropdown = dropdown.panel(self.activity_popover_element(cx));
            }
            dropdown.into_any_element()
        };

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
            .child(
                div()
                    .id("shell-rail")
                    .debug_selector(|| "shell-rail".into())
                    .flex()
                    .child(sidebar),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(main)
                    .child(
                        StatusBar::new()
                            // F-13：信息串居中；Inspector trigger 留在
                            // 最右（F-12 迁移到 Workspace Header 后再撤）。
                            .centered(Badge::new(run_status))
                            .child(inspector_trigger),
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
