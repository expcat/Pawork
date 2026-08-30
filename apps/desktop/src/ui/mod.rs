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
mod platform_preferences;
mod resources;
mod shell_layout;
mod task_rail;
pub mod text_input;
mod theme;
mod timeline;
mod timeline_entry;
#[cfg(test)]
mod u1_probe;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, div, point, prelude::*, px, AnyView, App, AsyncWindowContext, ClickEvent, Context,
    Corner, Entity, FocusHandle, Focusable, FontWeight, KeyBinding, KeyDownEvent, ListAlignment,
    ListState, Pixels, Point, Render, Rgba, ScrollHandle, SharedString, Window,
};
use pawork_client::AppEvent;

use crate::controller::{ControllerEvent, DesktopController};
use crate::platform::Platform;
use crate::projection::{
    ConnectionState, DateBucket, DesktopProjection, ResumeApply, SessionLiveStatus,
    TaskRailGrouping, TerminalState, UNASSIGNED_PROJECT,
};
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
        TaskCycleUp,
        TaskCycleDown,
        NextNeedsAttention,
        IncreaseTextSize,
        DecreaseTextSize,
        ResetTextSize,
    ]
);

/// 可测的 AppView 快捷键表（审批 / 取消 / 新建 / Inspector / 任务导航）。
pub(crate) const APP_VIEW_KEYBINDINGS: &[(&str, &str)] = &[
    ("cmd-.", "CancelRun"),
    ("cmd-enter", "ApproveOnce"),
    ("cmd-1", "ApproveOnce"),
    ("cmd-2", "ApproveForRun"),
    ("cmd-3", "Deny"),
    ("cmd-n", "NewTask"),
    ("cmd-i", "ToggleInspector"),
    ("cmd-alt-up", "TaskCycleUp"),
    ("cmd-alt-down", "TaskCycleDown"),
    ("cmd-alt-n", "NextNeedsAttention"),
    ("cmd-=", "IncreaseTextSize"),
    ("cmd-+", "IncreaseTextSize"),
    ("cmd--", "DecreaseTextSize"),
    ("cmd-0", "ResetTextSize"),
];

/// Timeline 空态引导（R2 Wave B）：无 active session 且条目数为 0 时居中
/// 显示；视觉与 AX 树共用同一文案源（accessibility/app.rs）。
pub(crate) const WORKSPACE_EMPTY_HINT: &str = "Select a task from the rail, or press Cmd+N to start a new one. Cycle tasks with Cmd+Opt+↓ / Cmd+Opt+↑, or jump to the next task that needs attention with Cmd+Opt+N.";

/// R3 Wave B：rail Tab 焦点顺序前缀（design §3.6：scope → grouping → 全局
/// 新建）；行为链（项目头 / 定向新建 / task 行）按当前分组渲染序接在其后，
/// 再接 MAIN_PATH_TAB_STOP_IDS。tab_index 负档保证 rail 整体先于主路径 0 档。
pub(crate) const RAIL_TAB_STOP_IDS: &[&str] = &["project-scope", "task-rail-grouping", "add-task"];
/// scope 触发器在 Tab 链中的位次（rail 前缀三档 -20/-19/-18，断线 reconnect
/// -17，行级 -16）。
pub(crate) const RAIL_TAB_INDEX_SCOPE: isize = -20;
pub(crate) const RAIL_TAB_INDEX_GROUPING: isize = -19;
pub(crate) const RAIL_TAB_INDEX_ADD_TASK: isize = -18;
/// Reconnect 仅在断线态渲染，视觉位在 add-task 与行为链之间（R6B 键盘
/// 路径补全）；不渲染时自动退出 Tab 链。
pub(crate) const RAIL_TAB_INDEX_RECONNECT: isize = -17;
pub(crate) const RAIL_TAB_INDEX_ROWS: isize = -16;
/// composer 在 Tab 链中的位次：链尾（1 档），主路径 0 档之后。
pub(crate) const COMPOSER_TAB_INDEX: isize = 1;

/// Inspector 控件与其它主路径控件同属 0 档；可见元素的 render 顺序决定
/// 顶层 tabs → collapse → 当前 surface 控件 → Composer 的普通 Tab 顺序。
pub(crate) const INSPECTOR_TAB_INDEX: isize = 0;

/// 主路径按钮的可测 tab_stop 标记。
pub(crate) const MAIN_PATH_TAB_STOP_IDS: &[&str] = &[
    "approve-once",
    "approve-for-run",
    "approve-deny",
    "composer-action",
    "add-task",
    "header-new-task",
    "reconnect",
    "model-picker",
    "timeline-back-to-bottom",
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
            .text_size(font::XS)
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
    /// Inspector 折叠态的 ActivityPopover（Workspace Header Activity
    /// 触发器弹出，R6 Wave A 自 StatusBar 迁入）。
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
        KeyBinding::new("shift-left", text_input::SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", text_input::SelectRight, Some("TextInput")),
        KeyBinding::new(
            "shift-home",
            text_input::SelectToLineStart,
            Some("TextInput"),
        ),
        KeyBinding::new("shift-end", text_input::SelectToLineEnd, Some("TextInput")),
        KeyBinding::new("cmd-a", text_input::SelectAll, Some("TextInput")),
        KeyBinding::new("ctrl-a", text_input::SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-c", text_input::Copy, Some("TextInput")),
        KeyBinding::new("ctrl-c", text_input::Copy, Some("TextInput")),
        KeyBinding::new("cmd-x", text_input::Cut, Some("TextInput")),
        KeyBinding::new("ctrl-x", text_input::Cut, Some("TextInput")),
        KeyBinding::new("cmd-z", text_input::Undo, Some("TextInput")),
        KeyBinding::new("cmd-shift-z", text_input::Redo, Some("TextInput")),
        KeyBinding::new("ctrl-z", text_input::Undo, Some("TextInput")),
        KeyBinding::new("ctrl-shift-z", text_input::Redo, Some("TextInput")),
        KeyBinding::new("cmd-.", CancelRun, Some("AppView")),
        KeyBinding::new("cmd-enter", ApproveOnce, Some("AppView")),
        KeyBinding::new("cmd-1", ApproveOnce, Some("AppView")),
        KeyBinding::new("cmd-2", ApproveForRun, Some("AppView")),
        KeyBinding::new("cmd-3", Deny, Some("AppView")),
        KeyBinding::new("cmd-n", NewTask, Some("AppView")),
        KeyBinding::new("cmd-i", ToggleInspector, Some("AppView")),
        KeyBinding::new("cmd-alt-up", TaskCycleUp, Some("AppView")),
        KeyBinding::new("cmd-alt-down", TaskCycleDown, Some("AppView")),
        KeyBinding::new("cmd-alt-n", NextNeedsAttention, Some("AppView")),
        KeyBinding::new("cmd-=", IncreaseTextSize, Some("AppView")),
        KeyBinding::new("cmd-+", IncreaseTextSize, Some("AppView")),
        KeyBinding::new("cmd--", DecreaseTextSize, Some("AppView")),
        KeyBinding::new("cmd-0", ResetTextSize, Some("AppView")),
    ]);
}

/// Workspace Header 的 Activity 触发器 / 浮层可见性（R6 Wave A · F-12）：
/// 触发器仅 Inspector 折叠态出现；浮层再叠加「菜单打开」条件。render 与
/// AX 树（accessibility/app.rs header_ax）同用此口径。
pub(super) fn activity_header_visibility(
    inspector_open: bool,
    activity_menu_open: bool,
) -> (bool, bool) {
    (!inspector_open, !inspector_open && activity_menu_open)
}

/// macOS 上 NSWindow 在 sendEvent 层把裸 Tab / Shift-Tab 送进 key-view
/// 循环：本窗口是单一 GPUI 视图、循环为空，事件被静默吞掉，GPUI 的
/// keyDown / performKeyEquivalent 都收不到（R3 Wave B Slice 3/4 真窗口
/// 取证）。旧 API setAllowsKeyboardNavigation: 已被现代 macOS 移除
///（NSInvalidArgumentException 实证），因此用 NSEvent 本地监听器在
/// NSWindow 派发前截获裸 Tab，直接驱动 GPUI 焦点链 focus_next /
/// focus_prev（design §3.6 焦点链唯一属主是 GPUI）。带 cmd / ctrl /
/// alt 的组合键原样放行，Enter / Space 不在截获范围。
#[cfg(target_os = "macos")]
fn install_appkit_tab_monitor(window: &Window, cx: &App) {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};
    use std::sync::Once;

    /// NSEvent.type == keyDown；本地监听器 mask 只订阅 keyDown。
    const NSEVENT_TYPE_KEY_DOWN: i64 = 10;
    const NSEVENT_MASK_KEY_DOWN: u64 = 1 << 10;
    /// kVK_Tab；Tab 之外一律原样放行。
    const KEY_CODE_TAB: u16 = 48;
    /// AppKit modifierFlags 位（NSShift / NSControl / NSAlternate / NSCommand）。
    const FLAG_SHIFT: u64 = 1 << 17;
    const FLAG_CONTROL: u64 = 1 << 18;
    const FLAG_ALT: u64 = 1 << 19;
    const FLAG_COMMAND: u64 = 1 << 20;
    /// Apple block ABI BlockFlags（block2-0.6.2/src/abi.rs）：BLOCK_IS_GLOBAL
    /// = 1<<28——全局块存于全局内存、无捕获、不朽，运行时不做复制 / 释放
    /// 计数；1<<30 是 BLOCK_HAS_SIGNATURE（Objective-C 类型编码），本监听
    /// 器块不设。descriptor 为最小布局 reserved + size，无签名域。
    const BLOCK_IS_GLOBAL: i32 = 1 << 28;

    thread_local! {
        static TAB_WINDOW: std::cell::RefCell<Option<AsyncWindowContext>> =
            const { std::cell::RefCell::new(None) };
    }

    #[repr(C)]
    struct BlockDescriptor {
        reserved: usize,
        size: usize,
    }

    /// Apple block ABI（无捕获全局块）。block crate 非 desktop 直接依赖
    /// 且 Cargo.toml 不在本任务写入集，故手写最小布局。
    #[repr(C)]
    struct TabMonitorBlock {
        isa: *const objc::runtime::Class,
        flags: i32,
        reserved: i32,
        invoke: unsafe extern "C" fn(*mut TabMonitorBlock, id) -> id,
        descriptor: *const BlockDescriptor,
    }

    unsafe extern "C" {
        static _NSConcreteGlobalBlock: objc::runtime::Class;
    }

    unsafe fn dispatch_tab_event(event: id) -> id {
        let kind: i64 = msg_send![event, type];
        if kind != NSEVENT_TYPE_KEY_DOWN {
            return event;
        }
        let key_code: u16 = msg_send![event, keyCode];
        if key_code != KEY_CODE_TAB {
            return event;
        }
        let flags: u64 = msg_send![event, modifierFlags];
        if flags & (FLAG_CONTROL | FLAG_ALT | FLAG_COMMAND) != 0 {
            return event;
        }
        let forward = flags & FLAG_SHIFT == 0;
        let handled = TAB_WINDOW.with_borrow_mut(|slot| {
            slot.as_mut().map(|cx| {
                cx.update(|window, _| {
                    if forward {
                        window.focus_next();
                    } else {
                        window.focus_prev();
                    }
                })
                .is_ok()
            }) == Some(true)
        });
        if handled {
            nil
        } else {
            event
        }
    }

    /// 本地监听器在 AppKit C 调用栈上执行，禁止 unwind 穿越：兜底捕获
    /// 并落日志，避免 panic in a function that cannot unwind 直接 abort。
    unsafe extern "C" fn tab_monitor_invoke(_block: *mut TabMonitorBlock, event: id) -> id {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch_tab_event(event)))
            .unwrap_or_else(|panic| {
                eprintln!("[tab-monitor] dispatch failed: {panic:?}");
                event
            })
    }

    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let descriptor: &'static BlockDescriptor = Box::leak(Box::new(BlockDescriptor {
            reserved: 0,
            size: std::mem::size_of::<TabMonitorBlock>(),
        }));
        let block: &'static TabMonitorBlock = Box::leak(Box::new(TabMonitorBlock {
            isa: std::ptr::addr_of!(_NSConcreteGlobalBlock),
            flags: BLOCK_IS_GLOBAL,
            reserved: 0,
            invoke: tab_monitor_invoke,
            descriptor,
        }));
        let cls = class!(NSEvent);
        let block_ptr = block as *const TabMonitorBlock as id;
        unsafe {
            let _: id = msg_send![cls, addLocalMonitorForEventsMatchingMask: NSEVENT_MASK_KEY_DOWN handler: block_ptr];
        }
    });
    // 进程级监听器只装一次；窗口句柄必须每次刷新。Once 包住句柄赋值会
    // 在窗口重建后把 Tab 打到失效的 AsyncWindowContext（focus_next 失败
    // 后事件原样放行，AppKit 再吞掉裸 Tab）。
    TAB_WINDOW.with_borrow_mut(|slot| *slot = Some(window.to_async(cx)));
}

pub struct AppView {
    /// 持有 tokio Runtime（GUI Connection Protocol 宿主），防止提前 shutdown。
    _platform: Arc<Platform>,
    controller: Arc<DesktopController>,
    socket: PathBuf,
    projection: DesktopProjection,
    text_input: Entity<TextInput>,
    terminal_input: Entity<TextInput>,
    /// per-session Composer 草稿（不含终端）。无 active session 时走独立槽。
    composer_drafts: HashMap<String, String>,
    no_session_draft: String,
    /// Terminal 输入按 Inspector 所属 workspace 隔离；任务切换时保存/恢复，
    /// 避免未发送命令泄漏到另一 workspace。
    terminal_drafts: HashMap<String, String>,
    terminal_input_workspace: Option<String>,
    /// Timeline 虚拟化状态（Bottom 对齐钉底；跟随语义见 ui/timeline.rs）。
    timeline_list: ListState,
    timeline_following: bool,
    /// timeline 数据 / 宽度变更代次；render 时对齐到 list（reset 语义）。
    timeline_rev: u64,
    timeline_list_rev: u64,
    timeline_list_count: usize,
    terminal_scroll: FollowScroll,
    /// 单一 Terminal write 回执槽。输入仅在同一 terminal 的成功回执到达且
    /// 用户未继续编辑时清空；失败/断线保留文本，避免静默丢命令。
    terminal_pending_write: Option<(String, Option<String>, String)>,
    /// 单一 Terminal create pending（按 Inspector 所属 workspace 去重）。
    /// Host 只回 terminal id，因此双击/快速键盘连打不得发出第二个 create。
    terminal_pending_create_workspace: Option<String>,
    /// 当前连接的事件消费任务。重连前必须替换并丢弃旧 receiver，防止旧
    /// 连接迟到的 terminal 回执污染新连接上的 pending 状态。
    event_task: Option<gpui::Task<()>>,
    status_hint: Option<String>,
    text_scale: font::TextScale,
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
    /// Grouping / Scope / Model 菜单的键盘高亮行（None = 尚未移动，回落到
    /// 当前选中项；菜单关闭时复位）。
    menu_highlight: Option<usize>,
    /// 键盘 Enter 选择菜单项后，触发器在同一物理按键的 keyup 仍会合成
    /// keyboard click——记录待吞掉的种类，防「选择即重开」（与
    /// pending_outside_close 同构的衔接标记）。
    pending_keyboard_menu_select: Option<MenuKind>,
    /// 行级键盘激活（Enter / Space key_down 直接调激活 handler）后的同键
    /// keyup 合成 click 衔接标记：物理键盘下 GPUI 会对聚焦行合成
    /// ClickEvent::Keyboard（无按下位置）；Slice 5 起只要键盘 click + 有
    /// 标记即吞（不要求行键匹配，防跨行误触发），鼠标 click 有按下位置
    /// 永不吞（与 pending_keyboard_menu_select 同构）。
    pending_row_key_activate: Option<String>,
    /// 按钮键盘激活（Slice 5 P2b：rail 聚焦 Button 的 Enter / Space 行级
    /// 激活）后的同键 keyup 合成 click 衔接标记，与行级同构。
    pending_button_key_activate: Option<String>,
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
    scope_focus: FocusHandle,
    grouping_focus: FocusHandle,
    approve_once_focus: FocusHandle,
    approve_for_run_focus: FocusHandle,
    deny_focus: FocusHandle,
    /// Send / Cancel 同槽单一焦点：idle 与 running 都 track 此句柄，
    /// 状态切换不丢焦点、不留下幽灵 tab stop。
    composer_action_focus: FocusHandle,
    add_task_focus: FocusHandle,
    header_new_task_focus: FocusHandle,
    /// 断线态 Reconnect 按钮焦点（track_focus + 行级激活，R6B 键盘补全）。
    reconnect_focus: FocusHandle,
    model_focus: FocusHandle,
    timeline_back_to_bottom_focus: FocusHandle,
    /// Timeline 虚拟化 action 按 event_id 懒建稳定焦点句柄；条目卸载/重挂
    /// 不丢普通键盘焦点语义，删除后的遗留项随窗口生命周期回收。
    timeline_entry_action_focus: BTreeMap<String, FocusHandle>,
    timeline_review_changes_focus: BTreeMap<String, FocusHandle>,
    inspector_tab_focus: [FocusHandle; 3],
    inspector_collapse_focus: FocusHandle,
    inspector_activity_focus: FocusHandle,
    changes_tab_focus: [FocusHandle; 2],
    changes_refresh_focus: FocusHandle,
    changes_file_focus: BTreeMap<String, FocusHandle>,
    resources_refresh_focus: FocusHandle,
    terminal_resize_focus: FocusHandle,
    terminal_back_to_bottom_focus: FocusHandle,
    terminal_start_focus: FocusHandle,
    /// rail 行级焦点句柄（按 RailStop::focus_key 懒建，会话删除后遗留条目
    /// 无副作用，随窗口生命周期回收）。
    rail_row_focus: BTreeMap<String, FocusHandle>,
    /// rail 列表滚动句柄：grouping / scope 切换后滚动 active task 到可见。
    rail_scroll: ScrollHandle,
    /// 下一次 render 时把 active task 滚动到可见（design §3.6）。
    rail_scroll_to_active: bool,
    /// ReplaceBaseline / Fresh 后 active 落空（snapshot 语义要求聚焦 scope
    /// 触发器）：下一次 render 消费并聚焦 scope。on_connected 无 Window，
    /// 借 render 兑现；首次连接不置位，不抢 composer 焦点。
    pending_scope_focus: bool,
    /// 折叠后回焦 Header Activity；恢复后回焦当前顶层 tab。目标元素可能
    /// 要到下一帧才进入树，因此由 render 在 AX 同步前兑现。
    pending_inspector_focus: Option<InspectorFocusTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorFocusTarget {
    Activity,
    SelectedTab,
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
        let terminal_input = cx.new(|cx| {
            TextInput::with_placeholder("Terminal input… (Enter to write)", cx)
                .id("terminal-input")
                .height_clamp(
                    crate::ui::theme::metrics::COMPOSER_INPUT_MIN_HEIGHT,
                    crate::ui::theme::metrics::COMPOSER_MAX_HEIGHT,
                )
        });
        let mut view = Self {
            _platform: platform,
            controller,
            socket,
            projection: DesktopProjection::default(),
            text_input,
            terminal_input,
            composer_drafts: HashMap::new(),
            no_session_draft: String::new(),
            terminal_drafts: HashMap::new(),
            terminal_input_workspace: None,
            timeline_list: ListState::new(
                0,
                // F-06：Top 对齐让短会话从 Header 下开始；跟随 / 脱钩语义
                // 改由 AppView::timeline_following + 显式 scroll_to 承载
                // （Bottom 的 None 钉底不再存在，见 ui/timeline.rs 合同）。
                ListAlignment::Top,
                px(timeline::TIMELINE_OVERDRAW),
            ),
            timeline_following: true,
            timeline_rev: 1,
            timeline_list_rev: 0,
            timeline_list_count: 0,
            terminal_scroll: FollowScroll::new(),
            terminal_pending_write: None,
            terminal_pending_create_workspace: None,
            event_task: None,
            status_hint: None,
            text_scale: font::TextScale::default(),
            grouping: TaskRailGrouping::Timeline,
            scope_workspace_id: None,
            collapsed_projects: BTreeSet::new(),
            inspector_open: true,
            inspector_tab: InspectorTab::default(),
            changes: ChangesPanelState::default(),
            resources: ResourcesPanelState::default(),
            open_menu: None,
            menu_highlight: None,
            pending_keyboard_menu_select: None,
            pending_row_key_activate: None,
            pending_button_key_activate: None,
            pending_outside_close: None,
            run_clock_running: false,
            barriers: BarrierSink::new(barrier_dir),
            ax_bridge: None,
            ax_error_reported: false,
            timeline_paging: false,
            controller_event_pending: false,
            focus_handle: cx.focus_handle(),
            scope_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(RAIL_TAB_INDEX_SCOPE),
            grouping_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(RAIL_TAB_INDEX_GROUPING),
            approve_once_focus: cx.focus_handle().tab_stop(true),
            approve_for_run_focus: cx.focus_handle().tab_stop(true),
            deny_focus: cx.focus_handle().tab_stop(true),
            composer_action_focus: cx.focus_handle().tab_stop(true),
            add_task_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(RAIL_TAB_INDEX_ADD_TASK),
            header_new_task_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            reconnect_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(RAIL_TAB_INDEX_RECONNECT),
            model_focus: cx.focus_handle().tab_stop(true),
            timeline_back_to_bottom_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            timeline_entry_action_focus: BTreeMap::new(),
            timeline_review_changes_focus: BTreeMap::new(),
            inspector_tab_focus: std::array::from_fn(|_| {
                cx.focus_handle()
                    .tab_stop(true)
                    .tab_index(INSPECTOR_TAB_INDEX)
            }),
            inspector_collapse_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            inspector_activity_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            changes_tab_focus: std::array::from_fn(|_| {
                cx.focus_handle()
                    .tab_stop(true)
                    .tab_index(INSPECTOR_TAB_INDEX)
            }),
            changes_refresh_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            changes_file_focus: BTreeMap::new(),
            resources_refresh_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            terminal_resize_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            terminal_back_to_bottom_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            terminal_start_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            rail_row_focus: BTreeMap::new(),
            rail_scroll: ScrollHandle::new(),
            rail_scroll_to_active: false,
            pending_scope_focus: false,
            pending_inspector_focus: None,
        };
        timeline::install_scroll_follow(&view.timeline_list, &cx.weak_entity());
        // R3 Wave B Slice 4：composer 挂 1 档作为 Tab 链尾（rail 负档 →
        // 主路径 0 档 → composer 1 档 → wrap 回 rail 首停），Tab 从
        // composer 一步回到 project-scope，与 design §3.6 遍历序一致。
        view.text_input
            .read(cx)
            .focus_handle(cx)
            .tab_stop(true)
            .tab_index(COMPOSER_TAB_INDEX);
        view.terminal_input
            .read(cx)
            .focus_handle(cx)
            .tab_stop(true)
            .tab_index(INSPECTOR_TAB_INDEX);
        view.start_connect(cx);
        view
    }

    /// F-05 Workspace Header（骨架常存）：任务标题 / branch / live 终态 /
    /// 右侧新建任务动作。诚实口径——无 active session 隐藏标题；branch 仅
    /// 在 Changes 流程已取回 GitDiffInfo.branch 且无 session_mismatch 时
    /// 显示（wire WorkspaceSummary 无 branch，不伪造）；终态只显示 live
    /// 可派生态（Running / Needs input / Blocked），空闲会话隐藏该项
    /// （wire 无终态字段，不画 Completed 绿点）。
    fn workspace_header_element(
        &mut self,
        activity_trigger_visible: bool,
        activity_popover_open: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let title = self.projection.workspace_header_title().map(str::to_string);
        let branch = self.header_branch();
        let status = self.projection.workspace_header_status();
        let can_create = self.can_create_task();
        let new_task_tooltip = SharedString::from(if can_create {
            "New task (Cmd+N)".to_string()
        } else {
            self.add_task_disabled_reason()
        });
        let mut new_task = Button::new("header-new-task")
            .track_focus(&self.header_new_task_focus)
            .variant(ButtonVariant::Ghost)
            .bordered()
            .disabled(!can_create)
            .padding(ButtonPadding::None)
            .width(px(metrics::HEADER_ACTION_WIDTH))
            .height(px(metrics::HEADER_ACTION_HEIGHT))
            .center()
            .vcenter()
            .radius(metrics::HEADER_ACTION_RADIUS)
            .text_size(font::BODY)
            .text_color(dark().text.emphasis)
            .label("+")
            .tooltip(new_task_tooltip);
        if can_create {
            new_task = new_task
                .on_click(cx.listener(|view, event, window, cx| {
                    if view.consume_button_key_click("header-new-task", event) {
                        return;
                    }
                    view.on_new_session(window, cx);
                }))
                .on_activate(cx.listener(|view, _event, window, cx| {
                    if view.open_menu.is_some() {
                        view.note_button_key_activate("header-new-task");
                        return;
                    }
                    view.note_button_key_activate("header-new-task");
                    view.on_new_session(window, cx);
                    cx.stop_propagation();
                }));
        }
        // R6 Wave A（F-12）：折叠态 Activity 触发器自 StatusBar 迁入 Header，
        // 占用最右动作槽并向下弹出 ActivityPopover；展开态该槽恢复 New task，
        // 折叠仍走 Inspector 面板内的 inspector-collapse。
        let activity_trigger = activity_trigger_visible.then(|| {
            let trigger = Button::new("inspector-toggle")
                .variant(ButtonVariant::Ghost)
                .bordered()
                .padding(ButtonPadding::None)
                .width(px(metrics::HEADER_ACTION_WIDTH))
                .height(px(metrics::HEADER_ACTION_HEIGHT))
                .center()
                .vcenter()
                .radius(metrics::HEADER_ACTION_RADIUS)
                .text_size(font::BODY)
                .text_color(dark().text.emphasis)
                .label("⋯")
                .tooltip("Activity")
                .track_focus(&self.inspector_activity_focus)
                .on_click(cx.listener(|view, event, _window, cx| {
                    if view.consume_button_key_click("inspector-toggle", event) {
                        return;
                    }
                    let down = Self::click_down_position(event);
                    view.toggle_menu(MenuKind::Activity, down, cx);
                }))
                .on_activate(cx.listener(|view, _event, _window, cx| {
                    if view.open_menu.is_some() {
                        view.note_button_key_activate("inspector-toggle");
                        return;
                    }
                    view.note_button_key_activate("inspector-toggle");
                    view.toggle_menu(MenuKind::Activity, None, cx);
                    cx.stop_propagation();
                }));
            let mut dropdown = Dropdown::new(trigger).panel_anchor(
                Corner::TopRight,
                point(
                    px(metrics::HEADER_ACTION_WIDTH),
                    px(metrics::HEADER_ACTION_HEIGHT),
                ),
            );
            if activity_popover_open {
                dropdown = dropdown.panel(self.activity_popover_element(cx));
            }
            dropdown
        });
        div()
            .id("workspace-header")
            .debug_selector(|| "workspace-header".into())
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(px(metrics::HEADER_HEIGHT))
            .pt(px(metrics::HEADER_SAFE_STRIP))
            .pl(px(metrics::TIMELINE_CONTENT_INSET))
            .pr(px(metrics::HEADER_INSET_RIGHT))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .gap(px(metrics::HEADER_TITLE_META_GAP))
                    .when_some(title, |row, title| {
                        row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(font::HEADER_TITLE)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(dark().text.primary)
                                .child(title),
                        )
                    })
                    .when_some(branch, |row, branch| {
                        row.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_1()
                                .text_size(font::BODY_SM)
                                .text_color(dark().text.secondary)
                                .child("⑂")
                                .child(branch),
                        )
                    })
                    .when_some(status, |row, status| {
                        let (dot, color) = header_status_visual(status);
                        row.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_6()
                                .text_size(font::BODY_SM)
                                .text_color(dark().text.secondary)
                                .child(
                                    div()
                                        .w(px(dot))
                                        .h(px(dot))
                                        .rounded_full()
                                        .flex_none()
                                        .bg(color),
                                )
                                .child(status.label()),
                        )
                    }),
            )
            .when(activity_trigger_visible, |header| {
                header.when_some(activity_trigger, |header, trigger| header.child(trigger))
            })
            .when(!activity_trigger_visible, |header| header.child(new_task))
    }

    /// Header branch 诚实数据源：host diff_* 固定解析 latest 会话，仅当
    /// Changes 已取回 GitDiffInfo.branch 且无 session_mismatch（数据会话
    /// 与 active 一致）时显示；无 active session 时数据必属他会话，直接
    /// 隐藏（审查 P3）。
    fn header_branch(&self) -> Option<String> {
        let active = self.projection.active_session_id.as_deref()?;
        let data_session = self.changes.session_id.as_deref();
        let mismatched = matches!(data_session, Some(data) if data != active);
        if mismatched {
            return None;
        }
        self.changes
            .git
            .as_ref()
            .and_then(|git| git.branch.clone())
            .filter(|branch| !branch.is_empty())
    }

    /// Changes 数据是否对 active session 可用（Review changes 门控）。
    fn changes_available_for_active(&self) -> bool {
        self.changes.session_id.is_some()
            && self.changes.session_id == self.projection.active_session_id
            && matches!(self.changes.fetch, changes::ChangesFetch::Ready)
            && self.changes.stale_reason.is_none()
    }

    pub fn composer_focus_handle(&self, cx: &App) -> FocusHandle {
        self.text_input.read(cx).focus_handle(cx)
    }

    /// Timeline 虚拟化控件在 render 时按稳定 event_id 取得焦点句柄。
    pub(super) fn timeline_entry_focus(
        &mut self,
        event_id: &str,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.timeline_entry_action_focus
            .entry(event_id.to_string())
            .or_insert_with(|| {
                cx.focus_handle()
                    .tab_stop(true)
                    .tab_index(INSPECTOR_TAB_INDEX)
            })
            .clone()
    }

    pub(super) fn timeline_review_focus(
        &mut self,
        event_id: &str,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.timeline_review_changes_focus
            .entry(event_id.to_string())
            .or_insert_with(|| {
                cx.focus_handle()
                    .tab_stop(true)
                    .tab_index(INSPECTOR_TAB_INDEX)
            })
            .clone()
    }

    pub(super) fn open_entry_menu_from_keyboard(&mut self, event_id: &str, cx: &mut Context<Self>) {
        let button_id = format!("entry-menu-{event_id}");
        self.note_button_key_activate(&button_id);
        self.toggle_menu(MenuKind::Entry(event_id.to_string()), None, cx);
    }

    pub(super) fn activate_review_changes_from_keyboard(
        &mut self,
        event_id: &str,
        cx: &mut Context<Self>,
    ) {
        let button_id = format!("run-review-{event_id}");
        self.note_button_key_activate(&button_id);
        if self.projection.timeline.iter().any(|entry| {
            entry.event_id == event_id
                && entry.fork_boundary == Some(crate::projection::ForkBoundary::Completed)
        }) && self.changes_available_for_active()
        {
            self.on_review_changes(cx);
        }
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

    fn handle_controller_event(&mut self, event: ControllerEvent, cx: &mut Context<Self>) {
        self.controller_event_pending = true;
        // 任一新事件都会使上一轮 settle 失效；下一次静默窗口重新写入。
        self.barriers.remove_timeline_stable();
        self.barriers.remove_approval_visible();
        match event {
            ControllerEvent::Disconnected { reason } => {
                let stale_reason = format!("connection lost · {reason}");
                self.projection
                    .set_connection(ConnectionState::Disconnected { reason });
                self.changes.mark_stale(&stale_reason);
                self.resources.mark_stale(&stale_reason);
                self.terminal_pending_write = None;
                self.terminal_pending_create_workspace = None;
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
                workspace_id,
                terminal_session_id,
            } => {
                if self.terminal_pending_create_workspace.as_deref() == Some(workspace_id.as_str())
                {
                    self.terminal_pending_create_workspace = None;
                }
                self.projection
                    .apply_terminal_created(workspace_id, terminal_session_id.clone());
                self.reconcile_terminal_workspace(cx);
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
            ControllerEvent::TerminalCreateFailed {
                workspace_id,
                reason,
            } => {
                if self.terminal_pending_create_workspace.as_deref() == Some(workspace_id.as_str())
                {
                    self.terminal_pending_create_workspace = None;
                }
                self.projection
                    .mark_terminal_create_failed(&workspace_id, reason.clone());
                self.reconcile_terminal_workspace(cx);
                self.status_hint = Some(format!("Create terminal failed: {reason}"));
            }
            ControllerEvent::TerminalWriteSucceeded {
                terminal_session_id,
            } => {
                self.projection.mark_terminal_ready(&terminal_session_id);
                if let Some((pending_id, pending_workspace, pending_text)) =
                    self.terminal_pending_write.take()
                {
                    if pending_id == terminal_session_id {
                        if let Some(workspace_id) = pending_workspace.as_deref() {
                            if self.terminal_drafts.get(workspace_id) == Some(&pending_text) {
                                self.terminal_drafts.remove(workspace_id);
                            }
                        }
                    }
                    if pending_id == terminal_session_id
                        && self.projection.terminal.session_id.as_deref()
                            == Some(terminal_session_id.as_str())
                        && self.terminal_input.read(cx).text() == pending_text
                    {
                        self.terminal_input.update(cx, |input, cx| input.clear(cx));
                    }
                }
                self.status_hint = Some("Terminal input sent.".into());
            }
            ControllerEvent::TerminalWriteFailed {
                terminal_session_id,
                reason,
            } => {
                self.terminal_pending_write = None;
                self.projection
                    .mark_terminal_failed(&terminal_session_id, reason.clone());
                self.status_hint = Some(format!("Terminal write failed: {reason}"));
            }
            ControllerEvent::TerminalResizeSucceeded {
                terminal_session_id,
                columns,
                rows,
            } => {
                self.projection
                    .apply_terminal_resize(&terminal_session_id, columns, rows);
                self.status_hint = Some(format!("Terminal size · {columns}×{rows}"));
            }
            ControllerEvent::TerminalResizeFailed {
                terminal_session_id,
                reason,
            } => {
                self.projection
                    .mark_terminal_failed(&terminal_session_id, reason.clone());
                self.status_hint = Some(format!("Terminal resize failed: {reason}"));
            }
            ControllerEvent::MessageSent {
                session_id,
                run_id,
                text,
            } => {
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
            }
            ControllerEvent::OperationFailed { action, reason } => {
                if action == "open session" {
                    self.timeline_paging = false;
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
            ControllerEvent::DiffFilesFailed { epoch, reason } => {
                if self.changes.mark_failed_for_epoch(epoch, &reason) {
                    self.status_hint = Some(format!("Load changes failed: {reason}"));
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
                    self.status_hint = Some(format!("Load diff failed: {reason}"));
                }
            }
            ControllerEvent::McpServersFailed { epoch, reason } => {
                if self.resources.mark_failed_for_epoch(epoch, &reason) {
                    self.status_hint = Some(format!("Load resources failed: {reason}"));
                }
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
        cx.spawn(async move |this, cx| {
            loop {
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
        // task 切换会重建 Timeline；先关闭可能锚在旧条目或旧上下文上的浮层，
        // 避免快捷键切换后留下不可见但仍接管键盘的 MenuKind。
        self.close_open_menu(cx);
        self.stash_composer_draft(cx);
        self.projection.select_session(&session_id);
        self.reconcile_terminal_workspace(cx);
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
        if self.inspector_open && self.inspector_tab == InspectorTab::Resources {
            self.refresh_resources(cx);
        }
        self.restore_composer_draft(cx);
        cx.notify();
    }

    fn on_session_clicked(
        &mut self,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.projection.active_session_id.as_deref() == Some(session_id) {
            // AX / click 激活当前行也要完成与切换路径一致的浮层收口；否则
            // 菜单仍会接管键盘，Composer 虽被 focus 却不会在 AX 树中发布。
            self.close_open_menu(cx);
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
                self.menu_highlight = None;
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

    /// 行级键盘激活前置：记录行键，供同键 keyup 合成 click 吞除匹配。
    fn note_row_key_activate(&mut self, row_key: &str) {
        self.pending_row_key_activate = Some(row_key.to_string());
    }

    /// 键盘激活后的 keyup 合成 click（无按下位置）：只要有未消费的键盘激活
    /// 标记即吞——标记匹配即消费；不匹配视为跨行错位或陈旧标记，一并吞除
    /// 防跨行误触发（Slice 5 修复：旧实现按行键匹配，标记落在他行时该行
    /// 会被误激活）。鼠标 click 有按下位置永不吞、不动标记。判定收归自由
    /// 函数 should_swallow_keyboard_click 供回归测试。
    fn consume_row_key_click(&mut self, _row_key: &str, event: &ClickEvent) -> bool {
        if !should_swallow_keyboard_click(
            Self::click_down_position(event).is_none(),
            self.pending_row_key_activate.as_deref(),
        ) {
            return false;
        }
        self.pending_row_key_activate = None;
        true
    }

    /// 按钮键盘激活后的同键 keyup 合成 click 吞除（与行级同构，独立按钮
    /// 标记字段，Slice 5 P2b）：防「Enter 开菜单 / 新建任务后 keyup 合成
    /// click 把刚开的菜单关掉或重复新建」。
    fn consume_button_key_click(&mut self, _button_id: &str, event: &ClickEvent) -> bool {
        if !should_swallow_keyboard_click(
            Self::click_down_position(event).is_none(),
            self.pending_button_key_activate.as_deref(),
        ) {
            return false;
        }
        self.pending_button_key_activate = None;
        true
    }

    /// 按钮键盘激活前置：记录按钮 id，供同键 keyup 合成 click 吞除。
    fn note_button_key_activate(&mut self, button_id: &str) {
        self.pending_button_key_activate = Some(button_id.to_string());
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
        // 键盘 Enter 选择菜单项后的触发器 keyup 合成点击：视为选择收尾，
        // 不重开菜单（down 无位置，只吞 keyboard click）。
        if let Some(kind) = self.pending_keyboard_menu_select.take() {
            if kind == target && down_position.is_none() {
                cx.notify();
                return;
            }
        }
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
        self.menu_highlight = None;
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
        self.menu_highlight = None;
        self.pending_outside_close = Some((kind, position));
        cx.notify();
    }

    /// 直接关闭当前菜单（Escape / 选择选项 / Fork 后）。
    fn close_open_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_highlight = None;
        if self.open_menu.take().is_some() {
            cx.notify();
        }
    }

    /// 根节点键盘裁决：所有可交互 MenuKind 打开时
    /// ↑/↓ 移动高亮、Enter 选择、Escape 关闭并把焦点送回触发器（design
    /// §3.6 / §8.2 菜单方向键缺口；Slice 5 修订——接管不再以触发器聚焦
    /// 硬门控，菜单开着即接管，Tab 移焦 / 外点后键盘仍归菜单，spec §3.3）；
    /// 其余情况下 Escape 沿用既有关闭路径。面板经 deferred 绘制不可聚焦，
    /// 根节点是唯一可达层；子层（rail ↑/↓、行级与按钮激活）在菜单打开时
    /// 让位（不 stop_propagation）保证冒泡到达此处。
    fn handle_root_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        // Tab 遍历（design §3.6，Slice 4 修复）：GPUI 无默认 tab cycle
        //（Slice 3 驱动取证 tab-no-traverse），根节点把 Tab / Shift-Tab
        // 显式映射到 focus_next / focus_prev，沿 tab_index 档位链（rail
        // 负档 → 主路径 0 档）走焦。TextInput 未绑定 Tab，冒泡到此只移焦
        // 不插字符；带 cmd / ctrl / alt 的组合键不接管。
        let modifiers = &event.keystroke.modifiers;
        if key == "tab" && !modifiers.control && !modifiers.alt && !modifiers.platform {
            if modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            cx.stop_propagation();
            return;
        }
        let menu = self.open_menu.clone();
        if let Some(kind) = menu {
            match key {
                "up" => {
                    self.move_menu_highlight(false);
                    cx.notify();
                }
                "down" => {
                    self.move_menu_highlight(true);
                    cx.notify();
                }
                "enter" | "space" => {
                    // Return 在 AppKit 走 key equivalent 双路投递（driven
                    // 实证：第二路 keydown 夹在同键 keyup 之后到达），按钮
                    // 行级激活第一路已开菜单，第二路到根节点时高亮仍落在
                    // 当前项——此时 no-op 保持菜单开启（不闪关）；高亮在
                    // 其它项（↓/↑ 移动后）才执行选择关闭。环绕回当前项
                    // 的 Enter 同样视为 no-op（语义：选择即当前态）。
                    let selected = self.menu_selected_index();
                    let highlight = self.menu_highlight_effective(selected);
                    if matches!(kind, MenuKind::Grouping | MenuKind::Scope | MenuKind::Model)
                        && highlight == selected
                    {
                        cx.stop_propagation();
                        return;
                    }
                    // 阻断触发器在同键 keyup 的合成点击重开菜单由
                    // pending_keyboard_menu_select 吞掉；此处先记标记再激活。
                    self.pending_keyboard_menu_select = Some(kind.clone());
                    self.activate_menu_item(kind, highlight, window, cx);
                }
                "escape" => self.close_menu_and_focus_trigger(kind, window, cx),
                _ => {}
            }
            if matches!(key, "up" | "down" | "enter" | "space" | "escape") {
                cx.stop_propagation();
            }
            return;
        }
        if self.handle_inspector_key(event, window, cx) {
            return;
        }
        if key == "escape" {
            self.close_open_menu(cx);
        }
    }

    /// Inspector 的普通键盘路径。所有判断都基于与 render/AX 共用的
    /// FocusHandle：tabs 用 ←/→，文件行用 ↑/↓，Enter/Space 与 click 复用
    /// 同一 action。返回 true 表示已消费。
    fn handle_inspector_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.modifiers.modified() {
            return false;
        }
        let key = event.keystroke.key.as_str();
        let activate = key == "enter" || key == "space";

        if let Some(ix) = self
            .inspector_tab_focus
            .iter()
            .position(|focus| focus.is_focused(window))
        {
            let target = inspector_tab_key_target(ix, key);
            if let Some(target) = target {
                let tab = [
                    InspectorTab::Changes,
                    InspectorTab::Terminal,
                    InspectorTab::Resources,
                ][target];
                self.note_button_key_activate(tab.button_id());
                self.select_inspector_tab(tab, cx);
                window.focus(&self.inspector_tab_focus[target]);
                cx.stop_propagation();
                return true;
            }
        }

        if let Some(ix) = self
            .changes_tab_focus
            .iter()
            .position(|focus| focus.is_focused(window))
        {
            let target = changes_tab_key_target(ix, key);
            if let Some(target) = target {
                let tab = [changes::ChangesTab::Files, changes::ChangesTab::Summary][target];
                let id = if target == 0 {
                    "changes-tab-files"
                } else {
                    "changes-tab-summary"
                };
                self.note_button_key_activate(id);
                self.on_select_changes_tab(tab, cx);
                window.focus(&self.changes_tab_focus[target]);
                cx.stop_propagation();
                return true;
            }
        }

        let focused_file = self.changes.files.iter().position(|file| {
            self.changes_file_focus
                .get(&file.path)
                .is_some_and(|focus| focus.is_focused(window))
        });
        if let Some(ix) = focused_file {
            let target = match key {
                "up" => Some((ix + self.changes.files.len() - 1) % self.changes.files.len()),
                "down" => Some((ix + 1) % self.changes.files.len()),
                _ if activate => Some(ix),
                _ => None,
            };
            if let Some(target) = target {
                let path = self.changes.files[target].path.clone();
                self.pending_row_key_activate = Some(path.clone());
                self.on_select_diff_file(&path, cx);
                if let Some(focus) = self.changes_file_focus.get(&path) {
                    window.focus(focus);
                }
                cx.stop_propagation();
                return true;
            }
        }

        let action = if self.inspector_collapse_focus.is_focused(window) && activate {
            Some("inspector-collapse")
        } else if self.inspector_activity_focus.is_focused(window) && activate {
            Some("inspector-toggle")
        } else if self.changes_refresh_focus.is_focused(window) && activate {
            Some("changes-refresh")
        } else if self.resources_refresh_focus.is_focused(window) && activate {
            Some("resources-refresh")
        } else if self.terminal_resize_focus.is_focused(window)
            && activate
            && terminal_can_operate(&self.projection.connection, &self.projection.terminal)
        {
            Some("terminal-resize")
        } else if self.terminal_back_to_bottom_focus.is_focused(window) && activate {
            Some("terminal-back-to-bottom")
        } else if self.terminal_start_focus.is_focused(window)
            && activate
            && terminal_start_enabled(
                &self.projection.connection,
                &self.projection.terminal,
                self.terminal_pending_create_workspace.as_ref(),
            )
        {
            Some("terminal-start")
        } else {
            None
        };
        let Some(action) = action else {
            return false;
        };
        self.note_button_key_activate(action);
        match action {
            "inspector-collapse" => self.on_toggle_inspector(window, cx),
            "inspector-toggle" => self.toggle_menu(MenuKind::Activity, None, cx),
            "changes-refresh" => self.refresh_changes(cx),
            "resources-refresh" => self.refresh_resources(cx),
            "terminal-resize" => self.on_apply_terminal_size(window, cx),
            "terminal-back-to-bottom" => {
                self.terminal_scroll.jump_to_bottom();
                cx.notify();
            }
            "terminal-start" if self.projection.terminal.session_id.is_some() => {
                self.on_apply_terminal_size(window, cx)
            }
            "terminal-start" => self.on_start_terminal(window, cx),
            _ => unreachable!(),
        }
        cx.stop_propagation();
        true
    }

    /// 关闭菜单并把焦点送回触发器（design §3.6：Escape 关闭后焦点回触发器）。
    fn close_menu_and_focus_trigger(
        &mut self,
        kind: MenuKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let trigger = match kind {
            MenuKind::Grouping => self.grouping_focus.clone(),
            MenuKind::Scope => self.scope_focus.clone(),
            MenuKind::Model => self.model_focus.clone(),
            MenuKind::Entry(event_id) => self
                .timeline_entry_action_focus
                .get(&event_id)
                .cloned()
                .unwrap_or_else(|| self.focus_handle.clone()),
            MenuKind::WorkspaceConfirm => self.add_task_focus.clone(),
            MenuKind::Activity => self.inspector_activity_focus.clone(),
        };
        self.open_menu = None;
        self.menu_highlight = None;
        window.focus(&trigger);
        cx.notify();
    }

    /// 菜单高亮行数。所有可点击 MenuRow 均进入同一普通键盘分派。
    fn menu_item_count(&self) -> usize {
        match self.open_menu.as_ref() {
            Some(MenuKind::Grouping) => 2,
            Some(MenuKind::Scope) => self.projection.project_scope_options().len(),
            Some(MenuKind::Model) => self.projection.models.len(),
            Some(MenuKind::Entry(_)) => 1,
            Some(MenuKind::WorkspaceConfirm) => self
                .projection
                .project_scope_options()
                .into_iter()
                .filter(|(id, _)| id.is_some())
                .count(),
            Some(MenuKind::Activity) => 1,
            None => 0,
        }
    }

    /// 当前选中项在菜单中的行位（键盘高亮的回落起点）。
    fn menu_selected_index(&self) -> usize {
        match self.open_menu.as_ref() {
            Some(MenuKind::Grouping) => match self.grouping {
                TaskRailGrouping::Timeline => 0,
                TaskRailGrouping::Projects => 1,
            },
            Some(MenuKind::Scope) => self
                .projection
                .project_scope_options()
                .iter()
                .position(|(workspace_id, _)| *workspace_id == self.scope_workspace_id)
                .unwrap_or(0),
            Some(MenuKind::Model) => self
                .projection
                .effective_model()
                .and_then(|(provider, id)| {
                    self.projection
                        .models
                        .iter()
                        .position(|model| model.provider_id == *provider && model.id == *id)
                })
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// 生效高亮行：未移动过（None）回落到当前选中项。
    fn menu_highlight_effective(&self, selected_ix: usize) -> usize {
        self.menu_highlight.unwrap_or(selected_ix)
    }

    fn move_menu_highlight(&mut self, forward: bool) {
        let len = self.menu_item_count();
        if len == 0 {
            return;
        }
        let current = self
            .menu_highlight
            .unwrap_or_else(|| self.menu_selected_index())
            .min(len - 1);
        let next = if forward {
            (current + 1) % len
        } else {
            (current + len - 1) % len
        };
        self.menu_highlight = Some(next);
    }

    /// Enter 选择高亮行：等价点击对应 MenuRow（复用既有 select 路径，含
    /// 单开互斥与菜单关闭）。
    fn activate_menu_item(
        &mut self,
        kind: MenuKind,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            MenuKind::Grouping => {
                let mode = if ix == 0 {
                    TaskRailGrouping::Timeline
                } else {
                    TaskRailGrouping::Projects
                };
                self.on_select_grouping(mode, window, cx);
            }
            MenuKind::Scope => {
                if let Some((workspace_id, _)) =
                    self.projection.project_scope_options().get(ix).cloned()
                {
                    self.on_select_scope(workspace_id, window, cx);
                }
            }
            MenuKind::Model => {
                if let Some(model) = self.projection.models.get(ix).cloned() {
                    self.on_select_model(model, cx);
                }
            }
            MenuKind::Entry(event_id) => {
                if ix == 0 && self.can_fork_entry(&event_id) {
                    self.close_open_menu(cx);
                    self.on_fork(&event_id, window, cx);
                }
            }
            MenuKind::WorkspaceConfirm => {
                if let Some((workspace_id, _)) = self
                    .projection
                    .project_scope_options()
                    .into_iter()
                    .filter_map(|(id, name)| id.map(|id| (id, name)))
                    .nth(ix)
                {
                    self.on_confirm_workspace(workspace_id, window, cx);
                }
            }
            MenuKind::Activity => {
                if ix == 0 {
                    self.on_activity_open_changes(window, cx);
                }
            }
        }
    }

    /// 当前分组模式下按 design §3.6 顺序的 rail 焦点链（scope → grouping →
    /// 全局新建 → 项目头 / 定向新建 → task 行）；折叠项目只保留头部。
    fn rail_stops(&self) -> Vec<RailStop> {
        rail_focus_stops(
            self.grouping,
            self.scope_workspace_id.as_deref(),
            &self.collapsed_projects,
            &self.projection,
            now_unix_ms(),
        )
    }

    /// rail 焦点链上各停靠点的句柄（固定触发器 + 行级懒建句柄）。
    fn rail_stop_focus(&self, stop: &RailStop) -> Option<FocusHandle> {
        match stop {
            RailStop::Scope => Some(self.scope_focus.clone()),
            RailStop::Grouping => Some(self.grouping_focus.clone()),
            RailStop::AddTask => Some(self.add_task_focus.clone()),
            RailStop::ProjectHeader { .. }
            | RailStop::ProjectAdd { .. }
            | RailStop::Task { .. } => self.rail_row_focus.get(&stop.focus_key()).cloned(),
        }
    }

    /// 行级焦点句柄懒建（render 期创建，tab_stop + 行档 tab_index）。
    fn rail_row_focus_handle(&mut self, key: &str, cx: &Context<Self>) -> FocusHandle {
        if let Some(handle) = self.rail_row_focus.get(key) {
            return handle.clone();
        }
        let handle = cx
            .focus_handle()
            .tab_stop(true)
            .tab_index(RAIL_TAB_INDEX_ROWS);
        self.rail_row_focus.insert(key.to_string(), handle.clone());
        handle
    }

    /// 当前 rail 可见任务序列（分组 + 展开态决定；task cycling 与
    /// next-needs-attention 共用）。
    fn visible_rail_sessions(&self) -> Vec<String> {
        self.rail_stops()
            .into_iter()
            .filter_map(|stop| match stop {
                RailStop::Task { session_id } => Some(session_id),
                _ => None,
            })
            .collect()
    }

    /// cmd-alt-up / cmd-alt-down：按当前 rail 可见顺序循环切换 active task
    ///（空列表 no-op 安全；切换后与 click / AX 路径同样聚焦 Composer）。
    fn cycle_active_task(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sessions = self.visible_rail_sessions();
        let active_ix = self
            .projection
            .active_session_id
            .as_deref()
            .and_then(|active| sessions.iter().position(|session| session == active));
        let Some(target) = cycle_index(sessions.len(), active_ix, forward) else {
            return;
        };
        // target 即当前 active（单会话 rail 环绕回原行）时不重开会话，
        // 但仍完成 cycling 的焦点合同；否则快捷键从 rail 控件发起时焦点
        // 会滞留在原控件，与 click / AX 激活当前 task 的终态不等价。
        if active_ix == Some(target) {
            self.close_open_menu(cx);
            self.focus_composer(window, cx);
            return;
        }
        self.open_session(sessions[target].clone(), cx);
        self.focus_composer(window, cx);
    }

    /// cmd-alt-n：按 rail 顺序找下一个 NeedsInput > Blocked > Unread 会话并
    /// 打开；无候选时经 status_hint 如实提示。
    fn open_next_needs_attention(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let candidates: Vec<(String, Option<Attention>)> = self
            .visible_rail_sessions()
            .into_iter()
            .map(|session_id| {
                let attention = attention_for(
                    self.projection.session_live_status(&session_id),
                    self.projection.session_unread(&session_id),
                );
                (session_id, attention)
            })
            .collect();
        match next_attention_session(&candidates, self.projection.active_session_id.as_deref()) {
            Some(session_id) => {
                self.open_session(session_id, cx);
                self.focus_composer(window, cx);
            }
            None => {
                self.status_hint = Some("No task needs attention.".into());
                cx.notify();
            }
        }
    }

    fn on_task_cycle_up(&mut self, _: &TaskCycleUp, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_active_task(false, window, cx);
    }

    fn on_task_cycle_down(
        &mut self,
        _: &TaskCycleDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_active_task(true, window, cx);
    }

    fn on_next_needs_attention_action(
        &mut self,
        _: &NextNeedsAttention,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_next_needs_attention(window, cx);
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
        self.pending_inspector_focus = Some(inspector_focus_after_toggle(self.inspector_open));
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
        self.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
        self.refresh_changes(cx);
        cx.notify();
    }

    /// Run 摘要卡「Review changes」：展开 Inspector 并切到 Changes 页
    /// （真实本地能力；折叠态可达）。Changes 数据不可用时 status_hint
    /// 如实说明，不伪造可用。
    pub(crate) fn on_review_changes(&mut self, cx: &mut Context<Self>) {
        self.close_open_menu(cx);
        // 先快照可用性：refresh 会把面板置 Fetching，事后再查必误判
        // 「不可用」（审查 P2）；仅真不可用（断连 / 无 workspace / 失败）
        // 才提示，拉取进行中不报。
        let was_available = self.changes_available_for_active();
        if !self.inspector_open {
            self.inspector_open = true;
            self.timeline_changed();
        }
        self.inspector_tab = InspectorTab::Changes;
        self.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
        self.refresh_changes(cx);
        let fetching = matches!(self.changes.fetch, changes::ChangesFetch::Fetching);
        if !was_available && !fetching {
            self.status_hint = Some("Changes data is not available yet.".into());
        }
        cx.notify();
    }

    /// 拉取会话 diff 文件清单（diff_list_files）。失败时诚实标记状态。
    fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let workspace = self.inspector_workspace_id();
        match (connected, workspace) {
            (true, Some(workspace)) => {
                let epoch = self.changes.begin_refresh();
                self.controller.diff_list_files(workspace, epoch);
            }
            (true, None) => self.changes.mark_failed("no workspace"),
            _ => self.changes.mark_stale("not connected"),
        }
        cx.notify();
    }

    /// 拉取选中文件的 diff（diff_get）。
    fn fetch_diff(&mut self, path: &str, cx: &mut Context<Self>) {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let workspace = self.inspector_workspace_id();
        match (connected, workspace) {
            (true, Some(workspace)) => {
                let epoch = self.changes.begin_diff_fetch(path);
                self.controller.diff_get(workspace, path.to_string(), epoch);
            }
            (true, None) => self.changes.mark_diff_failed("no workspace"),
            _ => self.changes.mark_diff_failed("not connected"),
        }
        cx.notify();
    }

    /// Inspector 面板的归属 workspace。active task 存在时它是唯一事实源；
    /// 没有 active task 时才用 rail scope，再回落 snapshot 默认 workspace。
    pub(super) fn inspector_workspace_id(&self) -> Option<String> {
        if self.projection.active_session_id.is_some() {
            return self.projection.active_workspace_id().map(str::to_string);
        }
        self.scope_workspace_id
            .clone()
            .or_else(|| self.projection.workspace_id.clone())
    }

    pub(super) fn reconcile_terminal_workspace(&mut self, cx: &mut Context<Self>) {
        let workspace_id = self.inspector_workspace_id();
        if self.terminal_input_workspace != workspace_id {
            let visible_text = self.terminal_input.read(cx).text().to_string();
            if let Some(previous) = self.terminal_input_workspace.as_ref() {
                self.terminal_drafts
                    .insert(previous.clone(), visible_text.clone());
            }

            let draft = workspace_id
                .as_ref()
                .and_then(|workspace| self.terminal_drafts.get(workspace))
                .cloned()
                .unwrap_or_default();
            if self.terminal_input_workspace.is_some() || visible_text.is_empty() {
                self.terminal_input
                    .update(cx, |input, cx| input.reset_text(draft, cx));
            } else if let Some(workspace) = workspace_id.as_ref() {
                // 初次建立 workspace 归属时保留用户已输入但尚未归属的文本。
                self.terminal_drafts.insert(workspace.clone(), visible_text);
            }
            self.terminal_input_workspace = workspace_id.clone();
        }
        self.projection
            .select_terminal_for_workspace(workspace_id.as_deref());
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
            self.resources.mark_stale("not connected");
        }
        cx.notify();
    }

    fn on_approve_once(&mut self, _: &ApproveOnce, window: &mut Window, cx: &mut Context<Self>) {
        self.on_approve("approve_once", window, cx);
    }

    fn on_approve_for_run(
        &mut self,
        _: &ApproveForRun,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_approve("approve_for_run", window, cx);
    }

    fn on_deny(&mut self, _: &Deny, window: &mut Window, cx: &mut Context<Self>) {
        self.on_approve("deny", window, cx);
    }

    fn on_cancel_run(&mut self, _: &CancelRun, window: &mut Window, cx: &mut Context<Self>) {
        self.on_cancel_clicked(window, cx);
    }

    fn set_text_scale(
        &mut self,
        scale: font::TextScale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text_scale = scale;
        window.set_rem_size(px(scale.rem_pixels()));
        self.status_hint = Some(format!("Text size · {}%", scale.percent()));
        cx.notify();
    }

    fn on_increase_text_size(
        &mut self,
        _: &IncreaseTextSize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_text_scale(self.text_scale.increase(), window, cx);
    }

    fn on_decrease_text_size(
        &mut self,
        _: &DecreaseTextSize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_text_scale(self.text_scale.decrease(), window, cx);
    }

    fn on_reset_text_size(
        &mut self,
        _: &ResetTextSize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_text_scale(font::TextScale::default(), window, cx);
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
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.status_hint = Some("Terminal needs a live connection; input was kept.".into());
            cx.notify();
            return;
        }
        if self.terminal_pending_write.is_some() {
            self.status_hint = Some("Waiting for the previous terminal write.".into());
            cx.notify();
            return;
        }
        if self.projection.terminal.session_id.is_some()
            && !terminal_can_operate(&self.projection.connection, &self.projection.terminal)
        {
            self.status_hint =
                Some("Terminal is not ready; input was kept and nothing was written.".into());
            cx.notify();
            return;
        }
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
            text.clone()
        } else {
            format!("{text}\n")
        };
        self.terminal_pending_write = Some((
            id.clone(),
            self.projection.terminal.workspace_id.clone(),
            text,
        ));
        self.controller.terminal_write(id, data);
        cx.notify();
    }

    fn send_current_message(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            self.status_hint = Some("Open a session first.".into());
            cx.notify();
            return;
        };
        if !self.can_send(cx) {
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
        // 最终入口再次复核，避免快捷键绕过 render/AX 的 disabled gate。
        if !self.can_cancel() {
            return;
        }
        let Some(run_id) = self.projection.active_run_id.clone() else {
            return;
        };
        self.controller.cancel_run(run_id);
        cx.notify();
    }

    fn on_approve(&mut self, decision: &str, window: &mut Window, cx: &mut Context<Self>) {
        // mouse / Button key / global shortcut / AX 都汇入此处，统一 fail-closed。
        if !self.can_approve() {
            return;
        }
        let Some(pending) = self.projection.pending_approval.clone() else {
            return;
        };
        self.controller
            .approve(pending.run_id, pending.tool_call_id, decision);
        // 审批卡会在响应事件后卸载；mouse / Button key / shortcut / AX
        // 统一关闭旧浮层并把焦点交回 Composer，避免焦点悬挂在消失按钮。
        self.close_open_menu(cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn can_switch_model(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_run_id.is_none()
            && !self.projection.models.is_empty()
    }

    fn composer_has_sendable_text(&self, cx: &App) -> bool {
        !self.text_input.read(cx).text().trim().is_empty()
    }

    fn can_send(&self, cx: &App) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_session_id.is_some()
            && self.projection.active_run_id.is_none()
            && self.composer_has_sendable_text(cx)
    }

    fn stash_composer_draft(&mut self, cx: &App) {
        let text = self.text_input.read(cx).text().to_string();
        match self.projection.active_session_id.clone() {
            Some(session_id) => {
                self.composer_drafts.insert(session_id, text);
            }
            None => self.no_session_draft = text,
        }
    }

    fn restore_composer_draft(&mut self, cx: &mut Context<Self>) {
        let draft = match self.projection.active_session_id.as_deref() {
            Some(session_id) => self
                .composer_drafts
                .get(session_id)
                .cloned()
                .unwrap_or_default(),
            None => self.no_session_draft.clone(),
        };
        self.text_input
            .update(cx, |input, cx| input.reset_text(draft, cx));
    }

    /// MessageSent：发送方草稿条目恒清；可见 Composer 只在回执属于
    /// 当前 active session 时清空。无 session 槽只在无 active 时写入，
    /// 因此这里不再另清 no_session_draft。
    fn apply_message_sent_draft(&mut self, session_id: &str, cx: &mut Context<Self>) {
        self.composer_drafts.remove(session_id);
        if Self::message_sent_clears_visible_composer(
            self.projection.active_session_id.as_deref(),
            session_id,
        ) {
            self.text_input.update(cx, |input, cx| input.clear(cx));
        }
    }

    fn message_sent_clears_visible_composer(active: Option<&str>, receipt: &str) -> bool {
        active == Some(receipt)
    }

    fn can_approve(&self) -> bool {
        live_action_enabled(
            &self.projection.connection,
            self.projection.pending_approval.is_some(),
        )
    }

    fn can_cancel(&self) -> bool {
        live_action_enabled(
            &self.projection.connection,
            self.projection.active_run_id.is_some(),
        )
    }

    fn can_fork_entry(&self, event_id: &str) -> bool {
        live_action_enabled(
            &self.projection.connection,
            self.projection.active_session_id.is_some(),
        ) && self
            .projection
            .timeline
            .iter()
            .any(|entry| entry.event_id == event_id && entry.is_fork_boundary())
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

/// 所有需要 live connection 的 action 共用 fail-closed 基础 gate。render、
/// 普通键盘、快捷键与 AX 最终 handler 必须在入口处再次复核。
fn live_action_enabled(connection: &ConnectionState, target_present: bool) -> bool {
    matches!(connection, ConnectionState::Connected { .. }) && target_present
}

/// Header 终态点视觉（F-05）：Ø10 语义点，与 TaskRail 状态点同色映射
/// （NeedsInput 琥珀 > Running 蓝 > Blocked 红；gui-design §R6 状态一致性）。
fn header_status_visual(status: SessionLiveStatus) -> (f32, Rgba) {
    let color = match status {
        SessionLiveStatus::Running => dark().accent.primary,
        SessionLiveStatus::NeedsInput => dark().semantic.warning_text,
        SessionLiveStatus::Blocked => dark().semantic.danger_text,
    };
    (metrics::HEADER_STATUS_DOT_SIZE, color)
}

/// rail 焦点链停靠点（design §3.6 顺序的静态语义；focus_key 唯一标识，
/// ProjectHeader/ProjectAdd 以日期桶限定避免 Timeline 同项目多桶重复）。
#[derive(Clone, Debug, PartialEq, Eq)]
enum RailStop {
    Scope,
    Grouping,
    AddTask,
    ProjectHeader {
        bucket: Option<DateBucket>,
        key: String,
    },
    ProjectAdd {
        bucket: Option<DateBucket>,
        key: String,
    },
    Task {
        session_id: String,
    },
}

impl RailStop {
    fn focus_key(&self) -> String {
        match self {
            Self::Scope => RAIL_TAB_STOP_IDS[0].into(),
            Self::Grouping => RAIL_TAB_STOP_IDS[1].into(),
            Self::AddTask => RAIL_TAB_STOP_IDS[2].into(),
            Self::ProjectHeader { bucket, key } => {
                rail_project_occurrence_key("project", *bucket, key)
            }
            Self::ProjectAdd { bucket, key } => {
                rail_project_occurrence_key("project-add", *bucket, key)
            }
            Self::Task { session_id } => rail_session_focus_key(session_id),
        }
    }
}

pub(super) fn rail_session_focus_key(session_id: &str) -> String {
    format!("task-{session_id}")
}

pub(super) fn rail_project_occurrence_key(
    prefix: &str,
    bucket: Option<DateBucket>,
    key: &str,
) -> String {
    match bucket {
        Some(bucket) => format!("{prefix}-{}:{key}", bucket.label()),
        None => format!("{prefix}-{key}"),
    }
}

pub(super) fn rail_project_key(workspace_id: Option<&str>) -> String {
    workspace_id.unwrap_or(UNASSIGNED_PROJECT).to_string()
}

/// 按 design §3.6 组装 rail 焦点链：scope → grouping → 全局新建 →（按当前
/// 分组渲染序）项目头 / 定向新建 / task 行。折叠项目只保留头部行。
fn rail_focus_stops(
    grouping: TaskRailGrouping,
    scope: Option<&str>,
    collapsed: &BTreeSet<String>,
    projection: &DesktopProjection,
    now_ms: u64,
) -> Vec<RailStop> {
    let mut stops = vec![RailStop::Scope, RailStop::Grouping, RailStop::AddTask];
    let projects = match grouping {
        TaskRailGrouping::Timeline => projection
            .timeline_groups(scope, now_ms)
            .into_iter()
            .flat_map(|group| {
                let bucket = group.bucket;
                group
                    .projects
                    .into_iter()
                    .map(move |project| (Some(bucket), project))
            })
            .collect::<Vec<_>>(),
        TaskRailGrouping::Projects => projection
            .project_groups(scope)
            .into_iter()
            .map(|project| (None, project))
            .collect(),
    };
    for (bucket, project) in projects {
        let key = rail_project_key(project.workspace_id.as_deref());
        stops.push(RailStop::ProjectHeader {
            bucket,
            key: key.clone(),
        });
        if !project.is_unassigned() && project.workspace_id.is_some() {
            stops.push(RailStop::ProjectAdd {
                bucket,
                key: key.clone(),
            });
        }
        if !collapsed.contains(&key) {
            for task in &project.tasks {
                stops.push(RailStop::Task {
                    session_id: task.session_id.clone(),
                });
            }
        }
    }
    stops
}

/// next-needs-attention 的候选优先级（NeedsInput > Blocked > Unread）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Attention {
    NeedsInput,
    Blocked,
    Unread,
}

fn attention_for(status: Option<SessionLiveStatus>, unread: bool) -> Option<Attention> {
    match status {
        Some(SessionLiveStatus::NeedsInput) => Some(Attention::NeedsInput),
        Some(SessionLiveStatus::Blocked) => Some(Attention::Blocked),
        // Running 不是 needs-attention 目标，但 unread 事件仍值得跳转。
        Some(SessionLiveStatus::Running) | None => unread.then_some(Attention::Unread),
    }
}

/// 按列表顺序（active 之后循环起算）选最高优先级候选；同级取 rail 顺序
/// 更早者。active 自身不作为候选。
fn next_attention_session(
    candidates: &[(String, Option<Attention>)],
    active: Option<&str>,
) -> Option<String> {
    let len = candidates.len();
    if len == 0 {
        return None;
    }
    let active_ix = active.and_then(|active| {
        candidates
            .iter()
            .position(|(session_id, _)| session_id == active)
    });
    let scan = match active_ix {
        Some(_) => len.saturating_sub(1),
        None => len,
    };
    let start = active_ix.map_or(0, |ix| (ix + 1) % len);
    let mut best: Option<(Attention, usize)> = None;
    for step in 0..scan {
        let ix = (start + step) % len;
        if let Some(attention) = candidates[ix].1 {
            if best.is_none_or(|(current, _)| attention < current) {
                best = Some((attention, ix));
            }
        }
    }
    best.map(|(_, ix)| candidates[ix].0.clone())
}

/// task cycling 目标行位：无 active 时 down 取首行 / up 取末行，有 active
/// 时循环步进；空列表安全返回 None。
fn cycle_index(len: usize, active: Option<usize>, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match active {
        None if forward => 0,
        None => len - 1,
        Some(ix) if forward => (ix + 1) % len,
        Some(ix) => (ix + len - 1) % len,
    })
}

/// 键盘合成 click 吞除判定（P3b 回归对象，Slice 5 修复）：无按下位置
/// （键盘合成）且存在未消费的键盘激活标记即吞；行键 / 按钮 id 不参与匹配
/// ——标记不匹配视为跨行 / 跨元素错位或陈旧标记，一并吞除防误触发；鼠标
/// click 有按下位置永不吞（consume_row_key_click 布尔反转教训：勿把判定
/// 写成「仅匹配才吞」导致鼠标路径或错位行被误放行 / 误吞）。
fn should_swallow_keyboard_click(keyboard_click: bool, marker: Option<&str>) -> bool {
    keyboard_click && marker.is_some()
}

fn inspector_tab_key_target(current: usize, key: &str) -> Option<usize> {
    match key {
        "left" => Some((current + 2) % 3),
        "right" => Some((current + 1) % 3),
        "enter" | "space" => Some(current),
        _ => None,
    }
}

fn changes_tab_key_target(current: usize, key: &str) -> Option<usize> {
    match key {
        "left" | "up" | "right" | "down" => Some((current + 1) % 2),
        "enter" | "space" => Some(current),
        _ => None,
    }
}

fn inspector_focus_after_toggle(open: bool) -> InspectorFocusTarget {
    if open {
        InspectorFocusTarget::SelectedTab
    } else {
        InspectorFocusTarget::Activity
    }
}

/// Terminal 面板的三态共用谓词：视觉按钮、AX 动作与键盘路径必须使用同一
/// 可操作策略，不能只让 AX 看起来禁用而 Enter 仍写入 Stale/Failed 终端。
pub(crate) fn terminal_can_operate(connection: &ConnectionState, terminal: &TerminalState) -> bool {
    matches!(connection, ConnectionState::Connected { .. })
        && terminal.session_id.is_some()
        && matches!(
            &terminal.availability,
            crate::projection::TerminalAvailability::Ready
        )
}

/// 底部 Start/Size 的单槽谓词。已有 Stale/Failed 终端时禁止继续按 Size，
/// 未创建时由 create-pending 去重防快速双击生成多个终端。
pub(crate) fn terminal_start_enabled(
    connection: &ConnectionState,
    terminal: &TerminalState,
    pending_create_workspace: Option<&String>,
) -> bool {
    if !matches!(connection, ConnectionState::Connected { .. }) {
        return false;
    }
    if terminal.session_id.is_some() {
        return terminal_can_operate(connection, terminal);
    }
    pending_create_workspace.is_none()
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // AppKit posts a workspace notification when display accessibility
        // preferences change. Refresh the palette before constructing this
        // frame and keep the current window available to the callback.
        platform_preferences::install(window, cx);
        // 一次性安装 AppKit Tab 本地监听器：NSWindow 会吞掉裸 Tab
        //（key-view 循环为空），监听器在派发前截获并驱动 GPUI 焦点链。
        // 监听器进程级只装一次；每次 render 刷新 thread_local 窗口句柄，
        // 避免窗口重建后 Tab 打到失效上下文。纯注册无同步重入，render
        // 期调用安全。
        #[cfg(target_os = "macos")]
        install_appkit_tab_monitor(window, cx);
        // on_connected 无 Window 只能置标记：在 AX 同步前消费，把落空的
        // active 焦点回落到 scope 触发器（design §3.6 焦点链首停）。
        if self.pending_scope_focus {
            self.pending_scope_focus = false;
            window.focus(&self.scope_focus);
        }
        if let Some(target) = self.pending_inspector_focus.take() {
            match target {
                InspectorFocusTarget::Activity => window.focus(&self.inspector_activity_focus),
                InspectorFocusTarget::SelectedTab => {
                    window.focus(&self.inspector_tab_focus[self.inspector_tab as usize])
                }
            }
        }
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
        // ActivityPopover 抽屉；150% 文本缩放时 rail 扩到 320，1080 宽仍
        // 留 760px Workspace。偏好值保留，加宽后自动恢复（shell_layout）。
        let shell = shell_layout::resolve(
            window.viewport_size().width,
            self.inspector_open,
            self.text_scale == font::TextScale::Percent150,
        );
        let inspector_open = shell.inspector_open;
        let (activity_trigger_visible, activity_popover_open) = activity_header_visibility(
            inspector_open,
            matches!(self.open_menu, Some(MenuKind::Activity)),
        );

        let sidebar = self.sidebar_element(px(shell.rail_width), cx);
        let header =
            self.workspace_header_element(activity_trigger_visible, activity_popover_open, cx);
        let timeline_area = self.timeline_area(cx);
        let composer = self.composer_element(cx);
        let workspace = div()
            .id("shell-workspace")
            .debug_selector(|| "shell-workspace".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(header)
            .child(timeline_area)
            .child(composer);

        let mut main = div().flex().flex_row().flex_1().min_w_0().child(workspace);
        if inspector_open {
            main = main.child(
                div()
                    .id("shell-inspector")
                    .debug_selector(|| "shell-inspector".into())
                    .flex()
                    .child(self.inspector_element(connected, cx)),
            );
        }
        div()
            .key_context("AppView")
            .track_focus(&self.focus_handle)
            .flex()
            .size_full()
            .bg(dark().bg.base)
            .text_color(dark().text.primary)
            // 根节点键盘裁决（R3 Wave B）：菜单打开且触发器聚焦时 ↑/↓ 移高亮、
            // Enter 选择、Escape 关闭并焦点回触发器；其余情况 Escape 沿用既有
            // 关闭路径；Tab / Shift-Tab 映射 focus_next / focus_prev 走
            // tab_index 焦点链（Slice 4）。面板经 deferred 绘制、不可聚焦，
            // 组件层 on_key_down 不可达，根节点为唯一机制。
            .on_key_down(cx.listener(
                |view: &mut Self,
                 event: &KeyDownEvent,
                 window: &mut Window,
                 cx: &mut Context<Self>| {
                    view.handle_root_key(event, window, cx);
                },
            ))
            .on_action(cx.listener(Self::on_send_message))
            .on_action(cx.listener(Self::on_approve_once))
            .on_action(cx.listener(Self::on_approve_for_run))
            .on_action(cx.listener(Self::on_deny))
            .on_action(cx.listener(Self::on_cancel_run))
            .on_action(cx.listener(Self::on_new_task_action))
            .on_action(cx.listener(Self::on_toggle_inspector_action))
            .on_action(cx.listener(Self::on_task_cycle_up))
            .on_action(cx.listener(Self::on_task_cycle_down))
            .on_action(cx.listener(Self::on_next_needs_attention_action))
            .on_action(cx.listener(Self::on_increase_text_size))
            .on_action(cx.listener(Self::on_decrease_text_size))
            .on_action(cx.listener(Self::on_reset_text_size))
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
                    .min_w_0()
                    .child(main)
                    .child(
                        // F-13：信息串居中；Inspector/Activity 触发器已随
                        // F-12（R6 Wave A）迁至 Workspace Header。
                        StatusBar::new().centered(Badge::new(run_status)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::SessionSummary;

    fn session(id: &str, workspace: Option<&str>) -> SessionSummary {
        SessionSummary {
            session_id: id.into(),
            title: format!("Task {id}"),
            updated_at_ms: 1_000,
            workspace_id: workspace.map(str::to_string),
            parent_branch_id: None,
            forked_from_event_id: None,
            active: false,
        }
    }

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
        for (key, action) in [
            ("cmd-=", "IncreaseTextSize"),
            ("cmd-+", "IncreaseTextSize"),
            ("cmd--", "DecreaseTextSize"),
            ("cmd-0", "ResetTextSize"),
        ] {
            assert!(
                APP_VIEW_KEYBINDINGS
                    .iter()
                    .any(|(bound, bound_action)| *bound == key && *bound_action == action),
                "missing text scale binding {key} -> {action}"
            );
        }
    }

    /// R6 Wave A（F-12）：折叠态 Activity 触发器随 Workspace Header；浮层
    /// 仅在折叠且菜单打开时出现；展开态无触发器（折叠走 inspector-collapse）。
    #[test]
    fn activity_header_visibility_follows_inspector_state() {
        assert_eq!(activity_header_visibility(true, false), (false, false));
        assert_eq!(activity_header_visibility(true, true), (false, false));
        assert_eq!(activity_header_visibility(false, false), (true, false));
        assert_eq!(activity_header_visibility(false, true), (true, true));
    }

    #[test]
    fn message_sent_clears_visible_composer_only_for_active_session() {
        assert!(AppView::message_sent_clears_visible_composer(
            Some("s-a"),
            "s-a"
        ));
        assert!(!AppView::message_sent_clears_visible_composer(
            Some("s-a"),
            "s-b"
        ));
        assert!(!AppView::message_sent_clears_visible_composer(None, "s-b"));
    }

    #[test]
    fn main_path_buttons_are_marked_tab_stops() {
        for id in [
            "approve-once",
            "approve-for-run",
            "approve-deny",
            "composer-action",
            "add-task",
            "header-new-task",
            "reconnect",
            "model-picker",
            "timeline-back-to-bottom",
        ] {
            assert!(
                MAIN_PATH_TAB_STOP_IDS.contains(&id),
                "missing tab_stop marker for {id}"
            );
        }
        assert!(!MAIN_PATH_TAB_STOP_IDS.contains(&"send"));
        assert!(!MAIN_PATH_TAB_STOP_IDS.contains(&"cancel"));
        assert!(COMPOSER_TAB_INDEX == 1);
    }

    #[test]
    fn composer_action_slot_is_single_tab_stop() {
        assert!(MAIN_PATH_TAB_STOP_IDS.contains(&"composer-action"));
        assert_eq!(crate::ui::theme::metrics::COMPOSER_SEND_SIZE, 32.0);
        let height =
            AppView::composer_panel_height(crate::ui::theme::metrics::COMPOSER_INPUT_MIN_HEIGHT);
        assert!(height <= 94.0 && height >= 88.0);
    }

    #[test]
    fn all_projects_new_task_requires_workspace_confirm() {
        assert!(resolve_new_task_workspace(None).is_none());
        assert_eq!(resolve_new_task_workspace(Some("ws-a")), Some("ws-a"));
    }

    #[test]
    fn keybinding_table_includes_task_cycling_and_attention() {
        for (key, action) in [
            ("cmd-alt-up", "TaskCycleUp"),
            ("cmd-alt-down", "TaskCycleDown"),
            ("cmd-alt-n", "NextNeedsAttention"),
        ] {
            assert!(
                APP_VIEW_KEYBINDINGS
                    .iter()
                    .any(|(binding, name)| *binding == key && *name == action),
                "missing {key} -> {action}"
            );
        }
    }

    #[test]
    fn workspace_empty_hint_mentions_cycling_and_attention() {
        assert!(WORKSPACE_EMPTY_HINT.contains("Cmd+Opt"));
        assert!(WORKSPACE_EMPTY_HINT.contains("Cmd+N"));
    }

    /// design §3.6：scope → grouping → 全局新建 → 项目头 / 定向新建 → task 行；
    /// 折叠项目只保留头部；Timeline 同项目跨桶的头部键以桶限定去重。
    #[test]
    fn rail_focus_stops_follow_design_tab_order() {
        let mut projection = DesktopProjection::default();
        projection.sessions = vec![
            session("s-1", Some("ws-a")),
            session("s-2", Some("ws-a")),
            session("s-3", None),
        ];
        projection.workspaces = vec![crate::projection::WorkspaceSummary {
            id: "ws-a".into(),
            name: "Alpha".into(),
        }];
        let collapsed = BTreeSet::from(["Unassigned".to_string()]);

        let stops = rail_focus_stops(
            TaskRailGrouping::Projects,
            None,
            &collapsed,
            &projection,
            60_000,
        );
        let keys: Vec<String> = stops.iter().map(|stop| stop.focus_key()).collect();
        assert_eq!(
            &keys[..3],
            ["project-scope", "task-rail-grouping", "add-task"]
        );
        assert_eq!(
            RAIL_TAB_STOP_IDS,
            ["project-scope", "task-rail-grouping", "add-task"]
        );
        // Projects 模式：Alpha 头 + 定向新建 + 两行任务；折叠的 Unassigned
        // 只剩头部（无定向新建）。
        assert_eq!(
            keys[3..],
            [
                "project-ws-a",
                "project-add-ws-a",
                "task-s-1",
                "task-s-2",
                "project-Unassigned"
            ]
        );

        let timeline = rail_focus_stops(
            TaskRailGrouping::Timeline,
            None,
            &BTreeSet::new(),
            &projection,
            60_000,
        );
        let timeline_keys: Vec<String> = timeline.iter().map(|stop| stop.focus_key()).collect();
        assert_eq!(
            timeline_keys[3..],
            [
                // 三个 session 同桶（updated_at 相同 → Today）。
                "project-Today:ws-a",
                "project-add-Today:ws-a",
                "task-s-1",
                "task-s-2",
                "project-Today:Unassigned",
                "task-s-3",
            ]
        );
    }

    #[test]
    fn task_cycling_index_wraps_and_handles_empty() {
        assert_eq!(cycle_index(0, None, true), None);
        assert_eq!(cycle_index(3, None, true), Some(0));
        assert_eq!(cycle_index(3, None, false), Some(2));
        assert_eq!(cycle_index(3, Some(2), true), Some(0));
        assert_eq!(cycle_index(3, Some(0), false), Some(2));
        assert_eq!(cycle_index(3, Some(1), true), Some(2));
    }

    /// P3b 回归：键盘合成 click（无按下位置）只要有未消费激活标记就吞——
    /// 标记行键匹配与否都吞（跨行错位防误触发）；鼠标真实 click（有按下
    /// 位置）永不吞；无标记永不吞。旧实现只按行键匹配吞，标记落在他行时
    /// 该行会被误激活（consume_row_key_click 布尔反转教训）。
    #[test]
    fn keyboard_click_swallow_disregards_marker_identity() {
        assert!(should_swallow_keyboard_click(true, Some("row-a")));
        assert!(should_swallow_keyboard_click(true, Some("row-b")));
        assert!(!should_swallow_keyboard_click(true, None));
        assert!(!should_swallow_keyboard_click(false, Some("row-a")));
        assert!(!should_swallow_keyboard_click(false, Some("row-b")));
        assert!(!should_swallow_keyboard_click(false, None));
    }

    #[test]
    fn inspector_keyboard_targets_wrap_and_preserve_activation() {
        assert_eq!(inspector_tab_key_target(0, "left"), Some(2));
        assert_eq!(inspector_tab_key_target(2, "right"), Some(0));
        assert_eq!(inspector_tab_key_target(1, "enter"), Some(1));
        assert_eq!(inspector_tab_key_target(1, "space"), Some(1));
        assert_eq!(changes_tab_key_target(0, "down"), Some(1));
        assert_eq!(changes_tab_key_target(1, "up"), Some(0));
        assert_eq!(changes_tab_key_target(0, "escape"), None);
    }

    #[test]
    fn inspector_toggle_focus_moves_between_panel_and_activity() {
        assert_eq!(
            inspector_focus_after_toggle(false),
            InspectorFocusTarget::Activity
        );
        assert_eq!(
            inspector_focus_after_toggle(true),
            InspectorFocusTarget::SelectedTab
        );
    }

    #[test]
    fn terminal_action_gate_matches_visual_ax_and_keyboard() {
        use crate::projection::TerminalAvailability;

        let connected = ConnectionState::Connected {
            instance_id: "instance-1".into(),
        };
        let disconnected = ConnectionState::Disconnected {
            reason: "closed".into(),
        };
        let mut terminal = TerminalState {
            session_id: Some("term-a".into()),
            availability: TerminalAvailability::Ready,
            ..TerminalState::default()
        };
        assert!(terminal_can_operate(&connected, &terminal));
        assert!(terminal_start_enabled(&connected, &terminal, None));
        assert!(!terminal_can_operate(&disconnected, &terminal));
        assert!(!terminal_start_enabled(&disconnected, &terminal, None));

        terminal.availability = TerminalAvailability::Stale {
            reason: "connection lost".into(),
        };
        assert!(!terminal_can_operate(&connected, &terminal));
        assert!(!terminal_start_enabled(&connected, &terminal, None));

        terminal.session_id = None;
        terminal.availability = TerminalAvailability::Stale {
            reason: "not started".into(),
        };
        assert!(terminal_start_enabled(&connected, &terminal, None));
        let pending = "ws-a".to_string();
        assert!(!terminal_start_enabled(
            &connected,
            &terminal,
            Some(&pending)
        ));
    }

    /// cmd-alt-n：NeedsInput > Blocked > Unread；active 之后循环起算，
    /// active 自身不作为候选；无候选返回 None。
    #[test]
    fn next_attention_session_prefers_input_then_blocked_then_unread() {
        let list = vec![
            ("s-unread".to_string(), Some(Attention::Unread)),
            ("s-plain".to_string(), None),
            ("s-blocked".to_string(), Some(Attention::Blocked)),
            ("s-input".to_string(), Some(Attention::NeedsInput)),
        ];
        assert_eq!(
            next_attention_session(&list, Some("s-blocked")),
            Some("s-input".to_string())
        );
        let no_input = vec![
            ("s-unread".to_string(), Some(Attention::Unread)),
            ("s-blocked".to_string(), Some(Attention::Blocked)),
        ];
        assert_eq!(
            next_attention_session(&no_input, None),
            Some("s-blocked".to_string())
        );
        let unread_only = vec![
            ("s-a".to_string(), None),
            ("s-b".to_string(), Some(Attention::Unread)),
        ];
        assert_eq!(next_attention_session(&unread_only, Some("s-b")), None);
        assert_eq!(
            next_attention_session(&unread_only, Some("s-a")),
            Some("s-b".to_string())
        );
        assert_eq!(next_attention_session(&[(String::new(), None)], None), None);
        // running + unread 归 Unread，running 无 unread 不进候选。
        assert_eq!(
            attention_for(Some(SessionLiveStatus::Running), true),
            Some(Attention::Unread)
        );
        assert_eq!(attention_for(Some(SessionLiveStatus::Running), false), None);
    }
}
