//! UI 层：AppView 宿主（连接 / 事件消费 / 状态与动作接线）与整体渲染装配。
//! 渲染块自本模块外移（R8 波 C）：TaskRail → task_rail、Timeline 虚拟化 →
//! timeline（条目 → timeline_entry、审批卡 → approval_card）、Inspector →
//! inspector、Composer → input_area。

mod accessibility;
mod approval_card;
mod archive;
mod attachment_blocks;
mod attachments;
mod barriers;
mod browser;
mod changes;
mod components;
mod computer;

mod drawing;
mod events;
mod files;
mod goal;
#[cfg(all(test, target_os = "macos", feature = "gui4-pty"))]
mod gui4_pty;
pub(crate) mod i18n;
mod input_area;
mod inspector;
mod markdown;
mod menus;
mod plan;
mod product_access;
mod quick_search;
mod recording;
mod recovery;
mod resources;
mod settings;
mod shell_layout;
mod subagent_panel;
mod task_rail;
mod task_usage;
mod terminal_view;
pub mod text_input;
pub(crate) mod theme;
mod timeline;
mod timeline_entry;
mod timeline_navigation;
#[cfg(test)]
mod u1_probe;
mod video;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, div, point, prelude::*, px, AnyView, App, AsyncWindowContext, ClickEvent, Context,
    Corner, Entity, FocusHandle, Focusable, FontWeight, KeyBinding, KeyDownEvent, ListAlignment,
    ListState, PathPromptOptions, Pixels, Point, Render, Rgba, ScrollHandle, SharedString,
    Subscription, Window,
};
use pawork_client::AppEvent;

use crate::controller::{ControllerEvent, DesktopController, DesktopHandshakeInfo};
use crate::platform::Platform;
use crate::projection::{
    ConnectionState, DateBucket, DesktopProjection, SessionLiveStatus, SettingsRole,
    TaskRailGrouping, TerminalState, UNASSIGNED_PROJECT,
};
use barriers::BarrierSink;
use changes::ChangesPanelState;
use components::button::{Button, ButtonPadding, ButtonVariant};
use components::dropdown::Dropdown;
use components::follow_scroll::FollowScroll;
use components::status_bar::StatusBar;
pub(crate) use components::{icon, icon_sized, Assets, Icon};
use inspector::InspectorTab;
use menus::MenuKind;
use resources::ResourcesPanelState;
use shell_layout::InspectorPlacement;
use task_rail::RailStop;
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
        OpenQuickSearch,
        FindConversation,
        TaskCycleUp,
        TaskCycleDown,
        NextNeedsAttention,
        IncreaseTextSize,
        DecreaseTextSize,
        ResetTextSize,
        SaveFile,
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
    ("cmd-s", "SaveFile"),
    ("cmd-k", "OpenQuickSearch"),
    ("cmd-f", "FindConversation"),
    ("cmd-alt-up", "TaskCycleUp"),
    ("cmd-alt-down", "TaskCycleDown"),
    ("cmd-alt-n", "NextNeedsAttention"),
    ("cmd-=", "IncreaseTextSize"),
    ("cmd-+", "IncreaseTextSize"),
    ("cmd--", "DecreaseTextSize"),
    ("cmd-0", "ResetTextSize"),
];

/// Timeline 空态（P0-3）：无 active session 且条目数为 0 时，只给出一个
/// 清楚的主路径；视觉与 AX 树共用同一文案源（accessibility/app.rs）。
pub(crate) fn workspace_empty_title() -> &'static str {
    i18n::t("timeline.empty_title")
}
pub(crate) fn workspace_empty_hint() -> &'static str {
    i18n::t("timeline.empty_hint")
}

/// GUI2-03：rail Tab 前缀为快捷查找 → 全局新建 → scope → grouping；
/// 项目头 / 定向新建 / task 行按当前分组渲染序接在其后，
/// 再接 MAIN_PATH_TAB_STOP_IDS。tab_index 负档保证 rail 整体先于主路径 0 档。
pub(crate) const RAIL_TAB_STOP_IDS: &[&str] = &[
    "quick-search",
    "add-task",
    "project-scope",
    "task-rail-grouping",
];
/// scope 触发器在 Tab 链中的位次（rail 前缀四档 -21/-20/-19/-18，断线 reconnect
/// -17，行级 -16）。
pub(crate) const RAIL_TAB_INDEX_SCOPE: isize = -19;
pub(crate) const RAIL_TAB_INDEX_GROUPING: isize = -18;
pub(crate) const RAIL_TAB_INDEX_ADD_TASK: isize = -20;
/// Reconnect 仅在断线态渲染；GUI3-07 起视觉位在底部 Local 行，Tab 档仍为
/// -17（冻结）；不渲染时自动退出 Tab 链。
pub(crate) const RAIL_TAB_INDEX_RECONNECT: isize = -17;
pub(crate) const RAIL_TAB_INDEX_ROWS: isize = -16;
/// TaskRail 页脚 Settings gear（SET-3）：位于行级 -16 之后，rail 焦点链尾。
pub(crate) const RAIL_TAB_INDEX_SETTINGS: isize = -15;
/// 任务消耗入口接在 rail 页脚之后、主工作区之前。
pub(crate) const RAIL_TAB_INDEX_USAGE: isize = -14;
/// composer 在 Tab 链中的位次：链尾（1 档），主路径 0 档之后。
pub(crate) const COMPOSER_TAB_INDEX: isize = 1;

/// Inspector 控件与其它主路径控件同属 0 档；可见元素的 render 顺序决定
/// 顶层 tabs → collapse → 当前 surface 控件 → Composer 的普通 Tab 顺序。
pub(crate) const INSPECTOR_TAB_INDEX: isize = 0;

/// 顶层路由（SET-3）：Workspace 三栏装配与 Settings 壳互斥渲染。工作台
/// 状态（草稿 / Timeline / Inspector / Run）全部保存在 AppView 字段，
/// 切换路由只换渲染，不触碰任何工作台状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AppRoute {
    #[default]
    Workspace,
    Settings,
}

/// Settings 内容页（SET-6a～6g）：供应商页与 Desktop 本地页常在；Network /
/// 权限页 / 工具页 / 终端页仅在对应 Host 查询成功后显示，About 仅在当前
/// 认证握手携带权威 Host 数据目录时显示。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingsPage {
    #[default]
    Providers,
    Subagents,
    General,
    Permissions,
    Tools,
    Terminal,
    Appearance,
    Advanced,
    About,
}

/// 工作台专属 action 在当前路由是否生效（SET-3 审查修复 1）：审批 /
/// 取消 Run / 新建任务 / Inspector / 任务导航的全局键绑定在 Settings 路由
/// 下全部旁路（Settings 壳内这些控件不渲染，键绑定也不得穿透路由）；
/// 文字缩放等应用级键不经此守卫。
fn workspace_action_active(route: AppRoute) -> bool {
    matches!(route, AppRoute::Workspace)
}

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
        KeyBinding::new("cmd-s", SaveFile, Some("AppView")),
        KeyBinding::new("cmd-k", OpenQuickSearch, Some("AppView")),
        KeyBinding::new("cmd-f", FindConversation, Some("AppView")),
        KeyBinding::new("cmd-alt-up", TaskCycleUp, Some("AppView")),
        KeyBinding::new("cmd-alt-down", TaskCycleDown, Some("AppView")),
        KeyBinding::new("cmd-alt-n", NextNeedsAttention, Some("AppView")),
        KeyBinding::new("cmd-=", IncreaseTextSize, Some("AppView")),
        KeyBinding::new("cmd-+", IncreaseTextSize, Some("AppView")),
        KeyBinding::new("cmd--", DecreaseTextSize, Some("AppView")),
        KeyBinding::new("cmd-0", ResetTextSize, Some("AppView")),
    ]);
    text_input::install_code_editor_keybindings(cx);
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
/// alt 的组合键原样放行，Enter / Space 不在截获范围。原生网页聚焦时
/// Tab 留给 WebKit；浏览器可见时 Cmd+L / K / I 仍交给宿主导航控件。
#[cfg(target_os = "macos")]
fn install_appkit_tab_monitor(window: &Window, cx: &App) {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};
    use std::sync::Once;

    /// NSEvent.type == keyDown；本地监听器 mask 只订阅 keyDown。
    const NSEVENT_TYPE_KEY_DOWN: i64 = 10;
    const NSEVENT_MASK_KEY_DOWN: u64 = 1 << 10;
    /// kVK_Tab；另保留浏览器的 Cmd+L / K / I。
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
        let flags: u64 = msg_send![event, modifierFlags];
        if matches!(key_code, 37 | 40 | 34)
            && flags & FLAG_COMMAND != 0
            && flags & (FLAG_CONTROL | FLAG_ALT | FLAG_SHIFT) == 0
        {
            let handled = TAB_WINDOW.with_borrow_mut(|slot| {
                slot.as_mut().is_some_and(|cx| {
                    cx.update(|window, cx| {
                        if let Some(Some(view)) = window.root::<AppView>() {
                            return view.update(cx, |view, cx| {
                                if view.browser_visible() {
                                    match key_code {
                                        37 => view.focus_browser_address(window, cx),
                                        40 => view.open_quick_search(window, cx),
                                        34 => view.on_toggle_inspector(window, cx),
                                        _ => unreachable!(),
                                    }
                                    return true;
                                }
                                false
                            });
                        }
                        false
                    })
                    .unwrap_or(false)
                })
            });
            if handled {
                return nil;
            }
        }
        if key_code != KEY_CODE_TAB {
            return event;
        }
        if flags & (FLAG_CONTROL | FLAG_ALT | FLAG_COMMAND) != 0 {
            return event;
        }
        let forward = flags & FLAG_SHIFT == 0;
        let handled = TAB_WINDOW.with_borrow_mut(|slot| {
            slot.as_mut().map(|cx| {
                cx.update(|window, cx| {
                    if let Some(Some(view)) = window.root::<AppView>() {
                        if view
                            .read(cx)
                            .browser
                            .active_native()
                            .is_some_and(|native| native.is_focused())
                        {
                            return false;
                        }
                    }
                    components::focus_ring::set_keyboard_focus(true, window, cx);
                    if let Some(Some(view)) = window.root::<AppView>() {
                        let key = gpui::Keystroke::parse(if forward { "tab" } else { "shift-tab" })
                            .unwrap();
                        if view.update(cx, |view, cx| {
                            view.intercept_terminal_keystroke(&key, window, cx)
                        }) {
                            return true;
                        }
                        let view = view.read(cx);
                        if forward && view.files_editor_focused(window) {
                            window.dispatch_action(Box::new(text_input::EditorIndent), cx);
                            return true;
                        }
                        if view.quick_search.open {
                            window.focus(&view.quick_search.focus);
                            return true;
                        }
                    }
                    if forward {
                        window.focus_next();
                    } else {
                        window.focus_prev();
                    }
                    true
                })
                .unwrap_or(false)
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
    if window.is_window_active() {
        TAB_WINDOW.with_borrow_mut(|slot| *slot = Some(window.to_async(cx)));
    }
}

#[derive(Clone, Debug)]
struct PendingHomeSend {
    text: String,
    model: Option<(String, String)>,
    effort: Option<String>,
    options: crate::controller::ComposerOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HomeSendPlan {
    ActiveSession(String),
    ReuseUnstarted(String),
    CreateUnassigned,
}

fn home_send_plan(
    active_session_id: Option<&str>,
    unstarted_unassigned: Option<&str>,
) -> HomeSendPlan {
    if let Some(session) = active_session_id {
        return HomeSendPlan::ActiveSession(session.to_string());
    }
    if let Some(session) = unstarted_unassigned {
        return HomeSendPlan::ReuseUnstarted(session.to_string());
    }
    HomeSendPlan::CreateUnassigned
}

pub struct AppView {
    /// 持有 tokio Runtime（GUI Connection Protocol 宿主），防止提前 shutdown。
    _platform: Arc<Platform>,
    controller: Arc<DesktopController>,
    socket: PathBuf,
    /// SET-6f：当前连接实际协商出的非 Secret 握手摘要；Connecting / 断线
    /// 时清空，避免把旧 Host 信息冒充当前状态。
    handshake_info: Option<DesktopHandshakeInfo>,
    projection: DesktopProjection,
    task_usage_focus: FocusHandle,
    text_input: Entity<TextInput>,
    terminal_input: Entity<terminal_view::TerminalInput>,
    browser: browser::BrowserPanel,
    files: files::FilesPanel,
    browser_request: Option<(String, String, String, serde_json::Value)>,
    browser_sessions: HashMap<Option<String>, browser::BrowserPanel>,
    browser_session: Option<String>,
    /// render 期捕获的窗口原始句柄：CU-13 起 Host 浏览器请求在后台任务里
    /// 派发（隐藏窗口 render 停摆），创建 WebView 不再经过 Window 参数。
    raw_window_handle: Option<raw_window_handle::RawWindowHandle>,
    terminal_action_layouts: HashMap<&'static str, ScrollHandle>,
    quick_search: quick_search::QuickSearch,
    timeline_navigation: timeline_navigation::TimelineNavigation,
    model_search_input: Entity<TextInput>,
    model_search_focus: FocusHandle,
    model_search_query: String,
    /// Composer 模型菜单用途筛选（ADR-064）：默认 Conversation（全部
    /// 文本模型），chips 单选切换；图像生成不在 Composer 执行面。
    pub(super) model_purpose_filter: input_area::ModelPurposeFilter,
    model_menu_scroll: ScrollHandle,
    pending_model_menu_scroll: bool,
    /// Activity 浮层「子智能体」卡列表滚动（限高 + 溢出滚动，ADR-063 浮层
    /// 改进）。
    activity_subagent_scroll: ScrollHandle,
    /// Activity 浮层内容与子代理行的实测布局；AX 树按真实滚动视口裁剪。
    activity_popover_layout: ScrollHandle,
    activity_open_changes_layout: ScrollHandle,
    /// Inspector「子代理」对话栏滚动（跟随语义与终端一致：脱钩读史、
    /// 贴底自动重挂）。
    subagent_conversation_scroll: FollowScroll,
    /// 对话栏 Refresh 按钮实测布局（AX Press 同源矩形）。
    subagent_refresh_layout: ScrollHandle,
    /// per-session Composer 草稿（不含终端）。无 active session 时走独立槽。
    composer_options: HashMap<Option<String>, crate::controller::ComposerOptions>,
    composer_loading: bool,
    composer_sending: bool,
    /// 发送失败归属于发起请求的草稿，切换任务后不串到当前输入区。
    composer_send_target: Option<String>,
    composer_send_errors: HashMap<Option<String>, String>,
    /// RV-01：当前打开的附件预览 =（会话, 附件 id）；会话不匹配或附件已
    /// 移除时不渲染，切换任务不受影响。
    composer_attachment_preview: Option<(Option<String>, String)>,
    /// RV-01：附件 id → 解码用 gpui::Image 缓存，render 懒建、随附件回收。
    composer_attachment_images: HashMap<String, Arc<gpui::Image>>,
    composer_drafts: HashMap<String, String>,
    no_session_draft: String,
    /// 首页无任务发送：`session_create(None)` 在途时记下正文，回执后再发。
    pending_home_send: Option<PendingHomeSend>,
    /// 切换 PTY 时清除未提交的输入法预编辑。
    terminal_input_key: Option<String>,
    /// Timeline 虚拟化状态（Bottom 对齐钉底；跟随语义见 ui/timeline.rs）。
    timeline_list: ListState,
    timeline_following: bool,
    /// timeline 数据 / 宽度变更代次；render 时对齐到 list（reset 语义）。
    timeline_rev: u64,
    timeline_list_rev: u64,
    timeline_list_count: usize,
    terminal_scroll: FollowScroll,
    /// 每个 PTY 独立串行写入，键盘连打合并到其在途队列。
    terminal_pending_write: HashSet<String>,
    terminal_queued_write: HashMap<String, String>,
    /// 单一 Terminal create pending（按 Inspector 所属 workspace 去重）。
    /// Host 只回 terminal id，因此双击/快速键盘连打不得发出第二个 create。
    terminal_pending_create_workspace: Option<String>,
    /// create 请求携带的 workspace 相对 cwd：Host 回执不带 cwd（wire 冻
    /// 结），成功后由 UI 补到新终端，避免显示退回默认 "."。
    terminal_pending_create_cwd: Option<String>,
    /// 关闭标签在途；仅匹配的 Host 成功回执可移除该 PTY。
    terminal_pending_close: Option<String>,
    /// 自动适配的在途请求；回执后继续应用最新视口尺寸。
    terminal_pending_resize: Option<(String, u16, u16)>,
    /// 输出面最近一次实测尺寸；面板变宽变窄后自动走 terminal_resize。
    terminal_fitted_size: Option<(u16, u16)>,
    /// 自动 fit resize 的去抖任务；拖拽窗口时只发最后一次尺寸。
    terminal_fit_resize_task: Option<gpui::Task<()>>,
    /// 控制键在焦点遍历之前交给 PTY；普通文字走原生输入法。
    _terminal_key_intercept: Option<Subscription>,
    /// 当前连接的事件消费任务。重连前必须替换并丢弃旧 receiver，防止旧
    /// 连接迟到的 terminal 回执污染新连接上的 pending 状态。
    event_task: Option<gpui::Task<()>>,
    status_hint: Option<String>,
    /// 复制 / 字号反馈独立于操作错误；替换任务即取消上一次收起计时。
    status_feedback: Option<String>,
    status_feedback_task: Option<gpui::Task<()>>,
    text_scale: font::TextScale,
    /// 正式窗口 bootstrap 开启磁盘存储；纯 UI 测试构造不访问用户配置。
    persist_appearance: bool,
    appearance_error: Option<String>,
    /// 界面语言（与 text_scale 同口径的本地 presentation preference：
    /// 用户目录落盘、即时生效；全局值经 i18n::language() 同源读取）。
    language: i18n::Language,
    grouping: TaskRailGrouping,
    scope_workspace_id: Option<String>,
    collapsed_projects: BTreeSet<String>,
    /// P1-2：以首个 tool event id 标识的本地折叠偏好；不进入 wire / replay。
    expanded_timeline_details: HashSet<String>,
    inspector_open: bool,
    inspector_motion: shell_layout::InspectorMotion,
    inspector_render_width: f32,
    inspector_last_placement: InspectorPlacement,
    /// Inspector 当前面板（Changes / Terminal / Resources）。
    inspector_tab: InspectorTab,
    inspector_open_tabs: Vec<InspectorTab>,
    inspector_tabs_scroll: ScrollHandle,
    inspector_reveal_selected: bool,
    inspector_item_layouts: HashMap<String, ScrollHandle>,
    inspector_item_focus: HashMap<String, FocusHandle>,
    /// Changes 面状态（Files / Summary、清单与选中 diff、滚动句柄）。
    changes: ChangesPanelState,
    /// Resources 面状态（MCP server 清单、滚动句柄）。
    resources: ResourcesPanelState,
    /// Computer-use 面状态（CU-18：目标清单、授权在途、滚动句柄）。
    computer: computer::ComputerPanelState,
    /// 当前打开的菜单；单一状态位保证至多一个打开（§8.2）。
    open_menu: Option<MenuKind>,
    /// Scope / Model 等菜单的键盘高亮行（None = 尚未移动，回落到
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
    /// Settings 四默认角色的在途写（SetDefaultRoleModel 已派出、回执 /
    /// 失败未到）：该角色下拉禁用，其余角色不受影响。
    settings_role_pending: Option<SettingsRole>,
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
    /// 空任务首页 Ghost「绑定项目」：与 Header `header-new-task` 分离，
    /// Inspector 展开时空任务欢迎态与 Header「+」可同时可见。
    welcome_bind_project_focus: FocusHandle,
    shell_layouts: HashMap<&'static str, ScrollHandle>,
    /// 断线态 Reconnect 按钮焦点（track_focus + 行级激活，R6B 键盘补全）。
    reconnect_focus: FocusHandle,
    connection_attempts: usize,
    recovery_layouts: HashMap<&'static str, ScrollHandle>,
    recovery_focus: HashMap<&'static str, FocusHandle>,
    terminal_details_open: Option<(Option<String>, String)>,
    model_focus: FocusHandle,
    /// Composer 推理强度 chip 触发器焦点（ADR-063）。
    effort_focus: FocusHandle,
    timeline_back_to_bottom_focus: FocusHandle,
    /// Timeline 虚拟化 action 按 event_id 懒建稳定焦点句柄；条目卸载/重挂
    /// 不丢普通键盘焦点语义，删除后的遗留项随窗口生命周期回收。
    timeline_entry_action_focus: BTreeMap<String, FocusHandle>,
    timeline_detail_focus: BTreeMap<String, FocusHandle>,
    timeline_review_changes_focus: BTreeMap<String, FocusHandle>,
    /// Failed banner「打开供应商设置」按 event_id 懒建；与 Review 互斥。
    timeline_open_providers_focus: BTreeMap<String, FocusHandle>,
    inspector_panel_focus: FocusHandle,
    inspector_collapse_focus: FocusHandle,
    inspector_activity_focus: FocusHandle,
    /// Inspector 折叠态 Header 重开按钮焦点（OPT-4b；与 Activity 触发器并存）。
    inspector_expand_focus: FocusHandle,
    inspector_back_focus: FocusHandle,
    inspector_approval_focus: FocusHandle,
    changes_tab_focus: [FocusHandle; 2],
    changes_refresh_focus: FocusHandle,
    changes_diff_scroll_left_focus: FocusHandle,
    changes_diff_scroll_right_focus: FocusHandle,
    changes_file_focus: BTreeMap<String, FocusHandle>,
    resources_refresh_focus: FocusHandle,
    computer_refresh_focus: FocusHandle,
    /// Computer-use 卡片授权按钮 / 卡片行的键盘焦点与实测布局（按动作
    /// id / 行 id 懒建；渲染期随可见键 retain，不跨会话累积）。
    computer_item_focus: HashMap<String, FocusHandle>,
    computer_item_layouts: HashMap<String, ScrollHandle>,
    subagent_chips_scroll: ScrollHandle,
    /// 对话栏代理切换 chip 焦点（按 agent_id 懒建；渲染期随可见代理
    /// retain，不跨会话累积）。
    subagent_agent_focus: HashMap<String, FocusHandle>,
    /// 切换代理后把选中 chip 滚入横滑视口（渲染期消费，Inspector 页签
    /// reveal 同口径）。
    subagent_reveal_selected: bool,
    /// chip 实测布局（AX 按滚动视口裁剪发布，与 Activity 行同口径）。
    subagent_agent_layouts: HashMap<String, ScrollHandle>,
    /// 对话栏条目 / 结果盒实测布局（AX 按滚动视口裁剪发布；key =
    /// `<event_id>:item|tool-header` 或 `<agent_id>:result[:toggle]`）。
    subagent_item_layouts: HashMap<String, ScrollHandle>,
    /// 对话栏工具行 / 长回执展开偏好（key 与主 Timeline 折叠集合同构
    /// 但独立成集，不污染主 Timeline 的展开态）。
    expanded_subagent_details: HashSet<String>,
    /// 对话栏工具行 / 结果盒展开按钮焦点（按 key 懒建）。
    subagent_item_focus: HashMap<String, FocusHandle>,
    /// 上次已滚入 transcript 视口的正文焦点。只在焦点变化时再滚。
    subagent_revealed_focus: Option<String>,
    subagent_refresh_focus: FocusHandle,
    subagent_back_to_bottom_focus: FocusHandle,
    terminal_back_to_bottom_focus: FocusHandle,
    terminal_close_focus: FocusHandle,
    /// rail 行级焦点句柄（按 RailStop::focus_key 懒建，会话删除后遗留条目
    /// 无副作用，随窗口生命周期回收）。
    rail_row_focus: BTreeMap<String, FocusHandle>,
    /// UI-2：整行（含动作槽）的指针悬停，与当前会话独立。
    rail_hovered_session: Option<String>,
    /// ADR-054 D2：TaskRail 行内改名编辑器（同一时刻至多一个；None =
    /// 无行处于编辑态）。输入框实体进入编辑态时懒建，退出即随状态丢弃。
    session_rename: Option<SessionRenameState>,
    archive: archive::ArchiveState,
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
    /// 顶层路由（SET-3）：Settings 壳与工作台互斥渲染，切换不动工作台状态。
    route: AppRoute,
    /// SET-6a：当前 Settings 内容页。Network 仅在 capability 到位后可选。
    settings_page: SettingsPage,
    /// TaskRail 页脚 Settings gear（可见 / 键盘 / AX 同 gate）。
    settings_focus: FocusHandle,
    /// Settings Rail「← Back to workspace」焦点（进入 Settings 后首停）。
    settings_back_focus: FocusHandle,
    /// GUI2-06：Settings 查找输入（独立实体，不参与 Composer 草稿）。
    settings_search_input: Entity<TextInput>,
    settings_search_focus: FocusHandle,
    settings_search_query: String,
    settings_search_selected: Option<String>,
    settings_search_scroll: ScrollHandle,
    settings_search_layouts: HashMap<String, ScrollHandle>,
    /// 查找定位：当前短暂标识的行 id，以及待滚入的 (page, row_id, attempts)。
    settings_locate_row: Option<String>,
    settings_locate_pending: Option<(SettingsPage, String, u8)>,
    settings_locate_task: Option<gpui::Task<()>>,
    /// SET-5：Settings 页级「刷新」按钮焦点（provider 状态 + 模型目录）。
    settings_refresh_focus: FocusHandle,
    /// SET-6a：Settings 导航「Network」焦点（内部字段保留 general）。
    settings_nav_general_focus: FocusHandle,
    /// SET-6a：Settings 导航「Models & providers」焦点（Network 选中时）。
    settings_nav_providers_focus: FocusHandle,
    /// Settings 导航「子代理」焦点。
    settings_nav_subagents_focus: FocusHandle,
    /// SET-6b：Settings 导航「权限与审批」焦点。
    settings_nav_permissions_focus: FocusHandle,
    /// SET-6c：Settings 导航「工具与 MCP」焦点。
    settings_nav_tools_focus: FocusHandle,
    /// SET-6d：Settings 导航「终端」焦点。
    settings_nav_terminal_focus: FocusHandle,
    /// SET-6e：Settings 导航「外观」焦点（本地能力，始终可用）。
    settings_nav_appearance_focus: FocusHandle,
    /// SET-6f：Settings 导航「高级」焦点（本地诊断页，始终可用）。
    settings_nav_advanced_focus: FocusHandle,
    /// SET-6g：Settings 导航「关于」焦点（Host 数据目录可用时显示）。
    settings_nav_about_focus: FocusHandle,
    /// SET-6e：三档字号选择的稳定焦点句柄。
    settings_appearance_focus: HashMap<String, FocusHandle>,
    /// SET-6a：proxy URL 内联输入（明文；非 Secret）。
    settings_proxy_input: Entity<crate::ui::text_input::TextInput>,
    /// SET-6a：proxy Save / Clear 焦点。
    settings_proxy_save_focus: FocusHandle,
    settings_proxy_clear_focus: FocusHandle,
    /// SET-6d：终端页 shell / columns / rows 输入（明文；非 Secret）。
    settings_terminal_shell_input: Entity<crate::ui::text_input::TextInput>,
    settings_terminal_columns_input: Entity<crate::ui::text_input::TextInput>,
    settings_terminal_rows_input: Entity<crate::ui::text_input::TextInput>,
    /// SET-6d：终端页 Save / Clear 焦点。
    settings_terminal_save_focus: FocusHandle,
    settings_terminal_clear_focus: FocusHandle,
    /// 子代理页并发数输入（明文；1–16）与 Save 焦点。
    settings_subagents_concurrency_input: Entity<crate::ui::text_input::TextInput>,
    settings_subagents_save_focus: FocusHandle,
    /// Settings 内容滚动句柄（供应商列表可能超出视口）。
    settings_scroll: ScrollHandle,
    settings_element_layouts: HashMap<String, ScrollHandle>,
    settings_scale_layout: ScrollHandle,
    settings_language_layout: ScrollHandle,
    project_task_focus: FocusHandle,
    composer_layouts: HashMap<&'static str, ScrollHandle>,
    scope_menu_scroll: ScrollHandle,
    entry_menu_scroll: ScrollHandle,
    pending_scope_menu_scroll: bool,
    /// SET-4：按 provider 懒建的 API key secure 输入实体（明文只留在
    /// 实体内，提交 / 取消 / 离开页面即清空，含 undo 栈）。
    settings_account_remove_confirm: Option<String>,
    settings_account_rename: Option<settings::AccountRenameState>,
    settings_api_key_inputs: HashMap<String, Entity<crate::ui::text_input::TextInput>>,
    /// 非 Secret 登录详情，支持选中复制；不作为认证状态来源。
    settings_auth_details: HashMap<String, Entity<crate::ui::text_input::TextInput>>,
    settings_copied_auth: Option<(String, settings::SettingsAuthAction)>,
    /// SET-4：connected 态经 Replace 展开内联编辑器的 provider 集合。
    settings_api_key_editors: HashSet<String>,
    /// SET-4：Remove 二次确认中的 provider（不静默删除已存凭证）。
    settings_remove_confirm: Option<String>,
    /// SET-6c：MCP server Remove 二次确认中的 server 名（写盘 + 清凭证，
    /// 不静默删除）。
    settings_mcp_remove_confirm: Option<String>,
    /// SET-4：settings 写动作按钮焦点句柄（identifier 键控，随 provider
    /// 清单回收）。
    settings_action_focus: HashMap<String, FocusHandle>,
    /// SET-6b：权限页控件焦点句柄（五档 mode 按钮 + 信任开关；固定六条，
    /// 随窗口生命周期回收）。
    settings_permissions_focus: HashMap<String, FocusHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorFocusTarget {
    Activity,
    SelectedTab,
    Composer,
}

fn inspector_focus_after_toggle(opening: bool, closing_from_center: bool) -> InspectorFocusTarget {
    if opening {
        InspectorFocusTarget::SelectedTab
    } else if closing_from_center {
        InspectorFocusTarget::Composer
    } else {
        InspectorFocusTarget::Activity
    }
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
        let terminal_input = cx.new(terminal_view::TerminalInput::new);
        cx.subscribe(&terminal_input, |view: &mut Self, _, event, cx| {
            if let terminal_view::TerminalInputEvent::Write(data) = event {
                view.send_terminal_bytes(data.clone(), cx);
            }
            cx.notify();
        })
        .detach();
        let model_search_input = cx.new(|cx| {
            TextInput::with_placeholder(i18n::t("model_search.placeholder"), cx)
                .id("model-search-input")
                .height_clamp(28.0, 40.0)
        });
        let model_search_focus = model_search_input.read(cx).focus_handle(cx).tab_stop(true);
        cx.observe(&model_search_input, |view, input, cx| {
            let query = input.read(cx).text().to_string();
            if view.model_search_query != query {
                view.model_search_query = query;
                if let Some(MenuKind::SettingsProviderModels(provider)) = &view.open_menu {
                    let id = format!(
                        "{}-list",
                        settings::settings_models_menu_identifier(provider)
                    );
                    if let Some(scroll) = view.settings_element_layouts.get(&id) {
                        scroll.set_offset(point(px(0.0), px(0.0)));
                    }
                }
                view.menu_highlight = None;
                view.model_menu_scroll.set_offset(point(px(0.0), px(0.0)));
                view.pending_model_menu_scroll = matches!(view.open_menu, Some(MenuKind::Model));
                cx.notify();
            }
        })
        .detach();
        let settings_search_input = cx.new(|cx| {
            TextInput::with_placeholder(i18n::t("settings.search.placeholder"), cx)
                .id("settings-search-input")
                .height_clamp(32.0, metrics::SETTINGS_SEARCH_INPUT_HEIGHT)
        });
        let settings_search_focus = settings_search_input
            .read(cx)
            .focus_handle(cx)
            .tab_stop(true);
        cx.observe(&settings_search_input, |view, input, cx| {
            let query = input.read(cx).text().to_owned();
            if view.settings_search_query != query {
                view.settings_search_query = query;
                view.settings_search_selected = None;
                view.settings_search_scroll
                    .set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            }
        })
        .detach();
        let mut view = Self {
            _platform: platform,
            controller,
            socket,
            handshake_info: None,
            projection: DesktopProjection::default(),
            task_usage_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(RAIL_TAB_INDEX_USAGE),
            text_input,
            terminal_input,
            browser: browser::BrowserPanel::new(cx),
            files: files::FilesPanel::default(),
            browser_request: None,
            browser_sessions: HashMap::new(),
            browser_session: None,
            raw_window_handle: None,
            quick_search: quick_search::QuickSearch::new(cx),
            timeline_navigation: timeline_navigation::TimelineNavigation::new(cx),
            model_search_input,
            model_search_focus,
            model_search_query: String::new(),
            model_purpose_filter: input_area::ModelPurposeFilter::default(),
            model_menu_scroll: ScrollHandle::new(),
            pending_model_menu_scroll: false,
            activity_subagent_scroll: ScrollHandle::new(),
            composer_options: HashMap::new(),
            composer_loading: false,
            composer_sending: false,
            composer_send_target: None,
            composer_send_errors: HashMap::new(),
            composer_attachment_preview: None,
            composer_attachment_images: HashMap::new(),
            composer_drafts: HashMap::new(),
            no_session_draft: String::new(),
            pending_home_send: None,
            terminal_action_layouts: ["terminal-output"]
                .into_iter()
                .map(|id| (id, ScrollHandle::new()))
                .collect(),
            terminal_input_key: None,
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
            subagent_conversation_scroll: FollowScroll::new(),
            subagent_refresh_layout: ScrollHandle::new(),
            activity_popover_layout: ScrollHandle::new(),
            activity_open_changes_layout: ScrollHandle::new(),
            terminal_pending_write: HashSet::new(),
            terminal_queued_write: HashMap::new(),
            terminal_pending_create_workspace: None,
            terminal_pending_create_cwd: None,
            terminal_pending_close: None,
            terminal_pending_resize: None,
            terminal_fitted_size: None,
            terminal_fit_resize_task: None,
            _terminal_key_intercept: None,
            event_task: None,
            status_hint: None,
            status_feedback: None,
            status_feedback_task: None,
            text_scale: font::TextScale::default(),
            persist_appearance: false,
            appearance_error: None,
            language: i18n::language(),
            grouping: TaskRailGrouping::Projects,
            scope_workspace_id: None,
            collapsed_projects: BTreeSet::new(),
            expanded_timeline_details: HashSet::new(),
            // OPT-4b（F6）：默认折叠（宽屏同样）；显式动作（Review changes、
            // Activity 摘要等）仍可展开。
            inspector_open: false,
            inspector_motion: shell_layout::InspectorMotion::default(),
            inspector_render_width: 0.0,
            inspector_last_placement: InspectorPlacement::Hidden,
            inspector_tab: InspectorTab::default(),
            inspector_open_tabs: Vec::new(),
            inspector_tabs_scroll: ScrollHandle::new(),
            inspector_reveal_selected: false,
            inspector_item_layouts: HashMap::new(),
            inspector_item_focus: HashMap::new(),
            changes: ChangesPanelState::default(),
            resources: ResourcesPanelState::default(),
            computer: computer::ComputerPanelState::default(),
            open_menu: None,
            menu_highlight: None,
            pending_keyboard_menu_select: None,
            pending_row_key_activate: None,
            pending_button_key_activate: None,
            pending_outside_close: None,
            settings_role_pending: None,
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
            welcome_bind_project_focus: cx.focus_handle().tab_stop(true),
            reconnect_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(RAIL_TAB_INDEX_RECONNECT),
            shell_layouts: [
                "workspace-header",
                "header-title",
                "header-new-task-layout",
                "header-activity-layout",
                "header-expand-layout",
                "workspace-empty-title",
                "workspace-empty-hint",
                "workspace-empty-action",
                "header-branch",
                "header-status",
                "connection-status",
                "rail-scope-layout",
                "rail-add-layout",
                "rail-search-layout",
                "rail-usage-layout",
                "rail-grouping-layout",
                "rail-reconnect-layout",
                "rail-settings-layout",
                "inspector-panel-layout",
                "inspector-menu-layout",
                "inspector-collapse-layout",
                "inspector-back-layout",
                "inspector-approval-hint-layout",
            ]
            .into_iter()
            .map(|id| (id, ScrollHandle::new()))
            .collect(),
            connection_attempts: 0,
            recovery_layouts: recovery::RECOVERY_IDS
                .into_iter()
                .map(|id| (id, ScrollHandle::new()))
                .collect(),
            recovery_focus: recovery::RECOVERY_IDS
                .into_iter()
                .map(|id| (id, cx.focus_handle().tab_stop(true)))
                .collect(),
            terminal_details_open: None,
            model_focus: cx.focus_handle().tab_stop(true),
            effort_focus: cx.focus_handle().tab_stop(true),
            timeline_back_to_bottom_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            timeline_entry_action_focus: BTreeMap::new(),
            timeline_detail_focus: BTreeMap::new(),
            timeline_review_changes_focus: BTreeMap::new(),
            timeline_open_providers_focus: BTreeMap::new(),
            inspector_panel_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            inspector_collapse_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            inspector_activity_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            inspector_expand_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            inspector_back_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            inspector_approval_focus: cx
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
            changes_diff_scroll_left_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            changes_diff_scroll_right_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            changes_file_focus: BTreeMap::new(),
            resources_refresh_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            computer_refresh_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            computer_item_focus: HashMap::new(),
            computer_item_layouts: HashMap::new(),
            subagent_chips_scroll: ScrollHandle::new(),
            subagent_agent_focus: HashMap::new(),
            subagent_reveal_selected: false,
            subagent_agent_layouts: HashMap::new(),
            subagent_item_layouts: HashMap::new(),
            expanded_subagent_details: HashSet::new(),
            subagent_item_focus: HashMap::new(),
            subagent_revealed_focus: None,
            subagent_refresh_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            subagent_back_to_bottom_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            terminal_back_to_bottom_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            terminal_close_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(INSPECTOR_TAB_INDEX),
            rail_row_focus: BTreeMap::new(),
            rail_hovered_session: None,
            session_rename: None,
            archive: archive::ArchiveState::default(),
            rail_scroll: ScrollHandle::new(),
            rail_scroll_to_active: false,
            pending_scope_focus: false,
            pending_inspector_focus: None,
            route: AppRoute::default(),
            settings_page: SettingsPage::default(),
            settings_focus: cx
                .focus_handle()
                .tab_stop(true)
                .tab_index(RAIL_TAB_INDEX_SETTINGS),
            settings_back_focus: cx.focus_handle().tab_stop(true),
            settings_search_input,
            settings_search_focus,
            settings_search_query: String::new(),
            settings_search_selected: None,
            settings_search_scroll: ScrollHandle::new(),
            settings_search_layouts: HashMap::new(),
            settings_locate_row: None,
            settings_locate_pending: None,
            settings_locate_task: None,
            settings_refresh_focus: cx.focus_handle().tab_stop(true),
            settings_nav_general_focus: cx.focus_handle().tab_stop(true),
            settings_nav_providers_focus: cx.focus_handle().tab_stop(true),
            settings_nav_subagents_focus: cx.focus_handle().tab_stop(true),
            settings_nav_permissions_focus: cx.focus_handle().tab_stop(true),
            settings_nav_tools_focus: cx.focus_handle().tab_stop(true),
            settings_nav_terminal_focus: cx.focus_handle().tab_stop(true),
            settings_nav_appearance_focus: cx.focus_handle().tab_stop(true),
            settings_nav_advanced_focus: cx.focus_handle().tab_stop(true),
            settings_nav_about_focus: cx.focus_handle().tab_stop(true),
            settings_appearance_focus: HashMap::new(),
            settings_proxy_input: cx.new(|cx| {
                TextInput::with_placeholder("http://127.0.0.1:7890", cx)
                    .id("settings-proxy-input")
                    .height_clamp(
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                    )
            }),
            settings_proxy_save_focus: cx.focus_handle().tab_stop(true),
            settings_proxy_clear_focus: cx.focus_handle().tab_stop(true),
            settings_terminal_shell_input: cx.new(|cx| {
                TextInput::with_placeholder("/bin/zsh", cx)
                    .id("settings-terminal-shell-input")
                    .height_clamp(
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                    )
            }),
            settings_terminal_columns_input: cx.new(|cx| {
                TextInput::with_placeholder("80", cx)
                    .id("settings-terminal-columns-input")
                    .height_clamp(
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                    )
            }),
            settings_terminal_rows_input: cx.new(|cx| {
                TextInput::with_placeholder("24", cx)
                    .id("settings-terminal-rows-input")
                    .height_clamp(
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                    )
            }),
            settings_terminal_save_focus: cx.focus_handle().tab_stop(true),
            settings_terminal_clear_focus: cx.focus_handle().tab_stop(true),
            settings_subagents_concurrency_input: cx.new(|cx| {
                TextInput::with_placeholder("4", cx)
                    .id("settings-subagents-concurrency-input")
                    .height_clamp(
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                        metrics::COMPOSER_INPUT_MIN_HEIGHT,
                    )
            }),
            settings_subagents_save_focus: cx.focus_handle().tab_stop(true),
            settings_scroll: ScrollHandle::new(),
            settings_element_layouts: HashMap::new(),
            settings_scale_layout: ScrollHandle::new(),
            settings_language_layout: ScrollHandle::new(),
            project_task_focus: cx.focus_handle().tab_stop(true),
            composer_layouts: [
                "composer-card",
                "composer-meta",
                "composer-workspace",
                "composer-context",
                "composer-project-task",
                "model-picker",
                "effort-picker",
            ]
            .into_iter()
            .map(|id| (id, ScrollHandle::new()))
            .collect(),
            scope_menu_scroll: ScrollHandle::new(),
            entry_menu_scroll: ScrollHandle::new(),
            pending_scope_menu_scroll: false,
            settings_account_remove_confirm: None,
            settings_account_rename: None,
            settings_api_key_inputs: HashMap::new(),
            settings_auth_details: HashMap::new(),
            settings_copied_auth: None,
            settings_api_key_editors: HashSet::new(),
            settings_remove_confirm: None,
            settings_mcp_remove_confirm: None,
            settings_action_focus: HashMap::new(),
            settings_permissions_focus: HashMap::new(),
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
        // 控制键先于应用快捷键和焦点遍历处理；普通文字交给原生输入法。
        let intercept_view = cx.weak_entity();
        view._terminal_key_intercept = Some(cx.intercept_keystrokes(move |event, window, cx| {
            let _ = intercept_view.update(cx, |view, cx| {
                if view.intercept_terminal_keystroke(&event.keystroke, window, cx) {
                    cx.stop_propagation();
                }
            });
        }));
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
        let title = self
            .projection
            .workspace_header_title()
            .map(|title| i18n::session_title(title).to_string());
        let branch = self.header_branch();
        let status = self.projection.workspace_header_status();
        let workspace_empty = self.projection.workspace_empty_hint_visible();
        let can_create = self.can_create_task();
        let new_task_tooltip = SharedString::from(if can_create {
            i18n::t("timeline.new_task_tooltip").to_string()
        } else {
            self.add_task_disabled_reason()
        });
        let mut new_task = Button::new("header-new-task")
            .track_focus(&self.header_new_task_focus)
            .variant(ButtonVariant::Ghost)
            .disabled(!can_create)
            .padding(ButtonPadding::None)
            .width(px(metrics::HEADER_ACTION_WIDTH))
            .height(px(metrics::HEADER_ACTION_HEIGHT))
            .center()
            .vcenter()
            .radius(metrics::HEADER_ACTION_RADIUS)
            .text_color(dark().text.emphasis)
            .child(icon(Icon::Plus))
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
                .padding(ButtonPadding::None)
                .width(px(metrics::HEADER_ACTION_WIDTH))
                .height(px(metrics::HEADER_ACTION_HEIGHT))
                .center()
                .vcenter()
                .radius(metrics::HEADER_ACTION_RADIUS)
                .text_color(dark().text.emphasis)
                .child(icon(Icon::More))
                .tooltip(i18n::t("header.tooltip_activity"))
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
        // OPT-4b（F6）：Inspector 默认折叠；折叠态 Header 最右新增重开入口，
        // 与 Activity 触发器并存（签字稿 collapsed 态右上角）；展开态不渲染
        // （折叠仍走面板内 inspector-collapse）。click / Enter / Space / AX
        // Press 共用 on_toggle_inspector，机制与 Activity 触发器一致。
        let inspector_expand = Button::new("inspector-expand")
            .track_focus(&self.inspector_expand_focus)
            .variant(ButtonVariant::Ghost)
            .padding(ButtonPadding::None)
            .width(px(metrics::HEADER_ACTION_WIDTH))
            .height(px(metrics::HEADER_ACTION_HEIGHT))
            .center()
            .vcenter()
            .radius(metrics::HEADER_ACTION_RADIUS)
            .text_color(dark().text.emphasis)
            .child(icon(Icon::Inspector))
            .tooltip(i18n::t("header.tooltip_open_inspector"))
            .on_click(cx.listener(|view, event, window, cx| {
                if view.consume_button_key_click("inspector-expand", event) {
                    return;
                }
                view.on_toggle_inspector(window, cx);
            }))
            .on_activate(cx.listener(|view, _event, window, cx| {
                if view.open_menu.is_some() {
                    // 菜单已开时让位给根节点菜单 Enter 接管（同触发器口径）。
                    view.note_button_key_activate("inspector-expand");
                    return;
                }
                view.note_button_key_activate("inspector-expand");
                view.on_toggle_inspector(window, cx);
                cx.stop_propagation();
            }));
        let navigation = self.navigation_header(cx);
        self.shell_element("workspace-header")
            .debug_selector(|| "workspace-header".into())
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .min_h(px(metrics::HEADER_HEIGHT))
            .py(px(metrics::HEADER_SAFE_STRIP))
            .pl(px(metrics::TIMELINE_CONTENT_INSET))
            .pr(px(metrics::HEADER_INSET_RIGHT))
            .gap(px(metrics::HEADER_ACTION_GAP))
            .bg(dark().bg.base)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .flex_1()
                    .min_w_0()
                    .gap(px(metrics::HEADER_TITLE_META_GAP))
                    .when_some(title, |row, title| {
                        row.child(
                            self.shell_element("header-title")
                                .flex_shrink()
                                .truncate()
                                .text_size(font::HEADER_TITLE)
                                .line_height(font::from_pixels(26.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(dark().text.primary)
                                .tooltip({
                                    let title = SharedString::from(title.clone());
                                    move |_, cx| tooltip_text(title.clone(), cx)
                                })
                                .child(title),
                        )
                    })
                    .when_some(branch, |row, branch| {
                        row.child(
                            self.header_meta_chip("header-branch")
                                .max_w(px(140.0))
                                .child(icon_sized(Icon::Branch, px(metrics::ICON_SM)))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .flex_1()
                                        .min_w_0()
                                        .child(div().truncate().child(branch)),
                                ),
                        )
                    })
                    .when_some(status, |row, status| {
                        let (dot, color) = header_status_visual(status);
                        row.child(
                            self.header_meta_chip("header-status")
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
            .child(navigation)
            .when(activity_trigger_visible, |header| {
                header.when_some(activity_trigger, |header, trigger| {
                    // OPT-4b：折叠态右上角 Activity + 重开入口并存；Activity
                    // 在左，inspector-expand 占最右动作槽。
                    header.child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_none()
                            .gap(px(metrics::HEADER_ACTION_GAP))
                            .child(self.shell_element("header-activity-layout").child(trigger))
                            .child(
                                self.shell_element("header-expand-layout")
                                    .child(inspector_expand),
                            ),
                    )
                })
            })
            .when(!activity_trigger_visible && !workspace_empty, |header| {
                header.child(self.shell_element("header-new-task-layout").child(new_task))
            })
    }

    fn inspector_back_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell_element("inspector-back-layout")
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .h(px(metrics::INSPECTOR_BACK_HEIGHT))
            .px_3()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(
                Button::new("inspector-back")
                    .variant(ButtonVariant::Ghost)
                    .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                    .text_size(font::SM)
                    .text_color(dark().text.primary)
                    .label(i18n::t("inspector.back_to_conversation"))
                    .track_focus(&self.inspector_back_focus)
                    .on_click(cx.listener(|view, event, window, cx| {
                        if view.consume_button_key_click("inspector-back", event) {
                            return;
                        }
                        view.on_inspector_back(window, cx);
                    })),
            )
    }

    fn shell_element(&self, id: &'static str) -> gpui::Stateful<gpui::Div> {
        div().id(id).track_scroll(&self.shell_layouts[id])
    }

    /// GUI3-07：Header branch / live 状态 Raised chip；无数据时调用方
    /// 不渲染，避免空容器占 gap。
    fn header_meta_chip(&self, id: &'static str) -> gpui::Stateful<gpui::Div> {
        self.shell_element(id)
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(metrics::SPACE_1))
            .px(px(metrics::HEADER_CHIP_PAD_X))
            .py(px(metrics::HEADER_CHIP_PAD_Y))
            .rounded(px(metrics::HEADER_CHIP_RADIUS))
            .bg(dark().surface.raised)
            .text_size(font::META)
            .line_height(font::META_LINE_HEIGHT)
            .text_color(dark().text.secondary)
    }

    fn welcome_visible(&self) -> bool {
        self.projection.timeline.is_empty()
            && self.projection.pending_approval.is_none()
            && self.projection.active_run_id.is_none()
            && !self.timeline_paging
    }

    fn welcome_hint(&self) -> &'static str {
        if self.projection.active_session_id.is_some() {
            i18n::t("timeline.empty_task_hint")
        } else {
            workspace_empty_hint()
        }
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

    /// active session 是否有至少一个可审阅 Changes 文件（CTA 门控）。
    fn changes_available_for_active(&self) -> bool {
        self.changes
            .has_reviewable_files_for(self.projection.active_session_id.as_deref())
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

    pub(super) fn timeline_open_providers_focus(
        &mut self,
        event_id: &str,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.timeline_open_providers_focus
            .entry(event_id.to_string())
            .or_insert_with(|| {
                cx.focus_handle()
                    .tab_stop(true)
                    .tab_index(INSPECTOR_TAB_INDEX)
            })
            .clone()
    }

    pub(super) fn timeline_detail_focus(
        &mut self,
        group_key: &str,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.timeline_detail_focus
            .entry(group_key.to_string())
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
        self.connection_attempts += 1;
        self.barriers.remove_timeline_stable();
        self.barriers.remove_approval_visible();
        self.handshake_info = None;
        if self.settings_page == SettingsPage::About {
            self.settings_page = SettingsPage::Advanced;
        }
        self.projection.settings_providers.account_quotas.clear();
        self.projection
            .settings_providers
            .account_mode_pending
            .clear();
        self.projection.set_connection(ConnectionState::Connecting);
        self.status_hint = None;
        cx.notify();
        let controller = Arc::clone(&self.controller);
        let socket = self.socket.clone();
        cx.spawn(
            async move |this, cx| match controller.connect(socket).await {
                Ok(connected) => {
                    this.update(cx, |view, cx| {
                        view.on_connected(
                            connected.snapshot,
                            connected.resume,
                            connected.handshake,
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
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }

    fn arm_run_clock(&mut self, cx: &mut Context<Self>) {
        // run 进行中驱动时长徽标重绘；barrier 启用时兼作 settle 探测心跳。
        if self.run_clock_running
            || (self.projection.active_run_id.is_none()
                && !self.barriers.is_active()
                && !self.settings_quota_clock_needed())
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
                        if view.browser_request.is_none() {
                            if let (Some(session), Some(run_id)) = (
                                view.projection.active_session_id.clone(),
                                view.projection.active_run_id.clone(),
                            ) {
                                view.controller.poll_browser(session, run_id);
                            }
                        }
                        if view.projection.active_run_id.is_some()
                            || view.settings_quota_clock_needed()
                        {
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
        // 必须在 select_session 之前记下：select 会把 active 写成目标会话。
        let reopening_current =
            self.projection.active_session_id.as_deref() == Some(session_id.as_str());
        self.cancel_file_attachment_picker();
        // task 切换会重建 Timeline；先关闭可能锚在旧条目或旧上下文上的浮层，
        // 避免快捷键切换后留下不可见但仍接管键盘的 MenuKind。
        self.close_open_menu(cx);
        // 切走会话即退出旧行内改名编辑态（不提交）。
        self.session_rename = None;
        self.stash_composer_draft(cx);
        self.projection.select_session(&session_id);
        self.sync_navigation_session();
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
        // 子代理观测按会话绑定。切到另一会话才清空并贴底，避免旧正文、
        // 选中项和脱钩阅读位置泄漏。同一会话重开（SnapshotRequired /
        // Fresh 重连）保留已加载对话，下面的 refresh 只重查权威数据。
        if !reopening_current {
            self.projection.subagent_activity = Default::default();
            self.projection.subagent_conversation = Default::default();
            self.subagent_conversation_scroll.jump_to_bottom();
        }
        self.controller.open_session(session_id);
        self.refresh_changes(cx);
        if self.inspector_open && self.inspector_tab == InspectorTab::Resources {
            self.refresh_resources(cx);
        }
        if self.inspector_open && self.inspector_tab == InspectorTab::Computer {
            self.refresh_computer_targets(cx);
        }
        if self.inspector_open && self.inspector_tab == InspectorTab::Subagent {
            self.refresh_subagent_conversation();
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
        // ADR-054 D1 / OPT-D：All projects 作用域下全局 New task 直建无归属
        // 会话，不再开 WorkspaceConfirm；项目作用域仍定向新建到该项目。
        self.create_task(self.scope_workspace_id.clone(), window, cx);
    }

    pub(super) fn on_open_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let create_task = matches!(self.open_menu, Some(MenuKind::ProjectTask));
        if !self.can_create_task() {
            self.status_hint = Some(i18n::t("status.open_project_needs_connection").into());
            cx.notify();
            return;
        }
        self.close_open_menu(cx);
        self.status_hint = Some(i18n::t("status.choose_project_folder").into());
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Project".into()),
        });
        cx.spawn_in(window, async move |this, cx| match selection.await {
            Ok(Ok(Some(mut paths))) => {
                if let Some(path) = paths.pop() {
                    this.update_in(cx, |view, _window, cx| {
                        view.status_hint = Some(i18n::t("status.opening_project").into());
                        view.controller.open_workspace(path, create_task);
                        if create_task {
                            view.focus_composer(_window, cx);
                        }
                        cx.notify();
                    })
                    .ok();
                }
            }
            Ok(Ok(None)) => {
                this.update(cx, |view, cx| {
                    view.status_hint = Some(i18n::t("status.open_project_cancelled").into());
                    cx.notify();
                })
                .ok();
            }
            Ok(Err(error)) => {
                this.update(cx, |view, cx| {
                    view.status_hint = Some(
                        i18n::t("status.open_project_failed").replace("{}", &error.to_string()),
                    );
                    cx.notify();
                })
                .ok();
            }
            Err(error) => {
                this.update(cx, |view, cx| {
                    view.status_hint = Some(
                        i18n::t("status.open_project_failed").replace("{}", &error.to_string()),
                    );
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
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
        if let Some(existing) = self
            .projection
            .unstarted_session_id(workspace_id.as_deref())
            .map(str::to_string)
        {
            if self.projection.active_session_id.as_deref() != Some(existing.as_str()) {
                self.open_session(existing, cx);
            }
            self.focus_composer(window, cx);
            cx.notify();
            return;
        }
        self.controller.create_session(workspace_id);
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn can_create_task(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        )
    }

    /// ADR-054 D2：进入行内改名。懒建单行编辑器，预填当前标题并聚焦；
    /// 行内编辑期间浮层菜单全部收口。
    fn begin_session_rename(
        &mut self,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_open_menu(cx);
        let title = self
            .projection
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .map(|session| session.title.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            TextInput::with_placeholder("", cx)
                .id("session-rename-input")
                .height_clamp(
                    metrics::RAIL_SESSION_ACTION_SIZE,
                    metrics::RAIL_SESSION_ACTION_SIZE,
                )
        });
        input.update(cx, |editor, cx| editor.set_text(title, cx));
        let focus = input.read(cx).focus_handle(cx);
        self.session_rename = Some(SessionRenameState {
            session_id: session_id.to_string(),
            input,
        });
        window.focus(&focus);
        cx.notify();
    }

    /// 行内改名编辑器是否聚焦（Enter / Escape 路由用）。
    fn session_rename_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.session_rename
            .as_ref()
            .is_some_and(|rename| rename.input.read(cx).focus_handle(cx).is_focused(window))
    }

    /// 改名提交后的收口：退出编辑态并把焦点送回该会话行（与菜单关闭
    /// 回焦触发器同规）。
    fn end_session_rename(
        &mut self,
        session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_rename = None;
        let focus_key = rail_session_focus_key(session_id);
        if let Some(handle) = self.rail_row_focus.get(&focus_key) {
            window.focus(handle);
        }
        cx.notify();
    }

    /// Enter 提交：空标题不提交（保留编辑器）；未变化仅退出；变化发
    /// SessionRename。连接 / Host 侧失败经既有 OperationFailed 通道可见。
    fn commit_session_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.session_rename.as_ref() else {
            return;
        };
        let session_id = rename.session_id.clone();
        let draft = rename.input.read(cx).text().to_string();
        let current = self
            .projection
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .map(|session| session.title.clone())
            .unwrap_or_default();
        match session_rename_decision(&current, &draft) {
            SessionRenameDecision::KeepEditing => {
                cx.notify();
            }
            SessionRenameDecision::Close => {
                self.end_session_rename(&session_id, window, cx);
            }
            SessionRenameDecision::Submit(title) => {
                self.end_session_rename(&session_id, window, cx);
                self.controller.rename_session(session_id, title);
            }
        }
    }

    /// Escape 取消：退出编辑态，不写盘、不发命令。
    fn cancel_session_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.session_rename.as_ref() else {
            return;
        };
        let session_id = rename.session_id.clone();
        self.end_session_rename(&session_id, window, cx);
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

    /// rail 焦点链上各停靠点的句柄（固定触发器 + 行级懒建句柄）。
    fn rail_stop_focus(&self, stop: &RailStop) -> Option<FocusHandle> {
        match stop {
            RailStop::Search => Some(self.quick_search.trigger.clone()),
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
                self.status_hint = Some(i18n::t("status.no_task_attention").into());
                cx.notify();
            }
        }
    }

    fn on_task_cycle_up(&mut self, _: &TaskCycleUp, window: &mut Window, cx: &mut Context<Self>) {
        if !workspace_action_active(self.route) {
            return;
        }
        self.cycle_active_task(false, window, cx);
    }

    fn on_task_cycle_down(
        &mut self,
        _: &TaskCycleDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !workspace_action_active(self.route) {
            return;
        }
        self.cycle_active_task(true, window, cx);
    }

    fn on_next_needs_attention_action(
        &mut self,
        _: &NextNeedsAttention,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !workspace_action_active(self.route) {
            return;
        }
        self.open_next_needs_attention(window, cx);
    }

    fn on_reconnect(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // render / AX 的可见性不是最终授权：旧按钮事件可能在首击已切到
        // Connecting 后才到达。入口再次复核，保证 connect single-flight。
        if !self.projection.show_reconnect() {
            return;
        }
        self.start_connect(cx);
    }

    fn run_status_visible(&self) -> bool {
        self.route == AppRoute::Workspace
    }

    /// GUI4：状态栏左栏项目 / 分支。无数据不占空洞。
    pub(super) fn status_bar_leading_text(&self) -> Option<String> {
        let project = self
            .projection
            .active_session_id
            .as_deref()
            .and_then(|id| self.projection.sessions.iter().find(|s| s.session_id == id))
            .and_then(|session| session.workspace_id.as_deref())
            .map(|workspace_id| self.projection.workspace_name(Some(workspace_id)));
        match (project, self.header_branch()) {
            (Some(project), Some(branch)) => Some(format!("{project} · {branch}")),
            (Some(project), None) => Some(project),
            (None, Some(branch)) => Some(branch),
            (None, None) => None,
        }
    }

    /// 状态栏右栏优先持久错误，其次限时反馈，否则连接文案。
    pub(super) fn status_bar_trailing_text(&self) -> String {
        self.status_hint
            .clone()
            .or_else(|| self.status_feedback.clone())
            .unwrap_or_else(|| self.connection_status_label())
    }

    fn on_manage_composer_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_page = SettingsPage::Providers;
        self.on_open_settings(window, cx);
        // AppKit may deliver the menu Return again after the route changes.
        // Keep it off the Back button so the same key cannot undo navigation.
        window.focus(&self.focus_handle);
    }

    /// 进入 Settings（SET-3）：只切路由 + 拉取只读供应商状态与模型目录。
    /// 工作台组件不渲染但状态全部保留在 AppView 字段；Run 不受影响。
    fn on_open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.route = AppRoute::Settings;
        // 路由切换时关闭任何打开的菜单，Settings 壳内没有菜单宿主。
        self.open_menu = None;
        self.menu_highlight = None;
        self.refresh_all_settings(cx);
        self.refresh_expanded_account_quotas(true, cx);
        window.focus(&self.settings_back_focus);
        cx.notify();
    }

    /// 返回工作台（SET-3）：恢复渲染即恢复进入前状态（会话 / 草稿 /
    /// Inspector / Timeline / Run 均未离开 AppView 字段），焦点回到入口。
    fn on_close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.route = AppRoute::Workspace;
        // SET-005：离开页面清空 secure 输入缓冲（含 undo 栈）与本地编辑
        // 状态；工作台状态不受影响。
        self.clear_settings_buffers(cx);
        self.settings_page = SettingsPage::Providers;
        window.focus(&self.settings_focus);
        cx.notify();
    }

    fn refresh_all_settings(&mut self, cx: &mut Context<Self>) {
        self.refresh_provider_status();
        self.controller.load_models();
        // 全量目录口径（include_disabled=true）：OPT-3a「Manage models」
        // 弹层与页级刷新同路径取权威状态。
        self.controller.load_model_catalog();
        self.refresh_general_settings();
        self.refresh_permissions_settings();
        self.refresh_terminal_settings();
        self.refresh_subagent_settings();
        self.refresh_resources(cx);
    }

    /// 认证成功 / 移除与页级 Refresh 的权威重查：provider_auth_status
    /// + 两套 model_list。单次启停回执不再走这里（ADR-055 D2a）。
    fn refresh_models_authority(&mut self) {
        self.refresh_provider_status();
        self.controller.load_models();
        self.controller.load_model_catalog();
    }

    /// 启停回执的角色清除：提示文案 + 本地清空对应角色键，不静默换绑。
    fn apply_model_cleared_roles(&mut self, cleared_roles: &[String]) {
        self.apply_model_cleared_note(cleared_roles);
        for role_name in cleared_roles {
            if let Some(role) = SettingsRole::from_wire_name(role_name) {
                self.projection.confirm_role_default_pair(role, None);
            }
        }
    }

    /// cleared_roles 诚实说明（ADR-055 D3）：Host 清除的角色默认对按
    /// 回执序列出（未知 wire 名原样保留，不臆造）；空回执清旧说明。
    fn apply_model_cleared_note(&mut self, cleared_roles: &[String]) {
        if cleared_roles.is_empty() {
            self.projection.settings_providers.model_cleared_note = None;
            return;
        }
        let roles = cleared_roles
            .iter()
            .map(|role| {
                SettingsRole::from_wire_name(role)
                    .map(|role| role.label().to_string())
                    .unwrap_or_else(|| role.clone())
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.projection.settings_providers.model_cleared_note =
            Some(i18n::t("settings.providers.models_cleared_roles").replace("{}", &roles));
    }

    fn remark_settings_stale_if_disconnected(&mut self) {
        if let ConnectionState::Disconnected { reason } = &self.projection.connection {
            let stale = format!("connection lost · {reason}");
            self.projection.mark_settings_stale(&stale);
        }
    }
    /// 拉取只读供应商状态（provider_auth_status）。断线时不进入 loading，
    /// 保留 stale 只读结果（controller 未派出时由 stale_reason 标注）。
    fn refresh_provider_status(&mut self) {
        if self.controller.load_provider_status() {
            self.projection.settings_providers.begin_loading();
        } else {
            self.projection
                .settings_providers
                .mark_stale("not connected");
        }
    }

    /// 拉取 Network 页（SET-6a / general_settings）。断线不进入 loading；
    /// 查询失败 / 未知则保持 unavailable，导航不显示该页。
    fn refresh_general_settings(&mut self) {
        if self.controller.load_general_settings() {
            self.projection.settings_general.begin_loading();
        } else {
            self.projection.settings_general.mark_stale("not connected");
        }
    }

    /// 拉取权限与审批页（SET-6b / permissions_settings）。断线不进入
    /// loading；查询失败 / 未知则保持 unavailable，导航不显示该页。
    fn refresh_permissions_settings(&mut self) {
        if self.controller.load_permissions_settings() {
            self.projection.settings_permissions.begin_loading();
        } else {
            self.projection
                .settings_permissions
                .mark_stale("not connected");
        }
    }

    /// 拉取终端页（SET-6d / terminal_settings）。断线不进入 loading；
    /// 查询失败 / 未知则保持 unavailable，导航不显示该页。
    fn refresh_terminal_settings(&mut self) {
        if self.controller.load_terminal_settings() {
            self.projection.settings_terminal.begin_loading();
        } else {
            self.projection
                .settings_terminal
                .mark_stale("not connected");
        }
    }

    /// 拉取子代理页（subagent_settings）。断线不进入 loading；查询失败 /
    /// 未知则保持 unavailable，导航不显示该页。
    fn refresh_subagent_settings(&mut self) {
        if self.controller.load_subagent_settings() {
            self.projection.settings_subagents.begin_loading();
        } else {
            self.projection
                .settings_subagents
                .mark_stale("not connected");
        }
    }

    /// Activity 浮层「子智能体」卡数据源（subagent_list）：只查当前活动
    /// 会话；无活动会话 / 断线时清空或保持旧数据，不伪造空列表。
    pub(crate) fn load_activity_subagents(&mut self) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            self.projection.subagent_activity = Default::default();
            return;
        };
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            return;
        }
        let request_id = self.projection.subagent_activity.begin_loading(&session_id);
        if !self
            .controller
            .load_subagent_list(session_id.clone(), request_id)
        {
            self.projection.subagent_activity.apply_failed(
                &session_id,
                request_id,
                "not connected".into(),
            );
        }
    }

    /// Settings 导航切页（SET-6a）。Network 未接通时 fail-closed 留在供应商页。
    pub(crate) fn on_select_settings_page(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match page {
            SettingsPage::General if !self.projection.settings_general.query.available => return,
            SettingsPage::General => {
                self.settings_page = SettingsPage::General;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_general_focus);
                }
            }
            SettingsPage::Subagents if !self.projection.settings_subagents.query.available => {
                return;
            }
            SettingsPage::Subagents => {
                self.settings_page = SettingsPage::Subagents;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_subagents_focus);
                }
            }
            SettingsPage::Permissions if !self.projection.settings_permissions.query.available => {
                return;
            }
            SettingsPage::Permissions => {
                self.settings_page = SettingsPage::Permissions;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_permissions_focus);
                }
            }
            SettingsPage::Tools if !self.resources.available => return,
            SettingsPage::Tools => {
                self.settings_page = SettingsPage::Tools;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_tools_focus);
                }
            }
            SettingsPage::Terminal if !self.projection.settings_terminal.query.available => return,
            SettingsPage::Terminal => {
                self.settings_page = SettingsPage::Terminal;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_terminal_focus);
                }
            }
            SettingsPage::Appearance => {
                self.settings_page = SettingsPage::Appearance;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_appearance_focus);
                }
            }
            SettingsPage::Advanced => {
                self.settings_page = SettingsPage::Advanced;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_advanced_focus);
                }
            }
            SettingsPage::About if self.settings_about_rows().is_none() => return,
            SettingsPage::About => {
                self.settings_page = SettingsPage::About;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_about_focus);
                }
            }
            SettingsPage::Providers => {
                self.settings_page = SettingsPage::Providers;
                if self.settings_locate_pending.is_none() {
                    window.focus(&self.settings_nav_providers_focus);
                }
            }
        }
        self.projection.settings_providers.account_quotas.clear();
        self.projection
            .settings_providers
            .account_mode_pending
            .clear();
        if self.settings_page == SettingsPage::Providers {
            self.refresh_expanded_account_quotas(true, cx);
        }
        self.settings_scroll
            .set_offset(gpui::point(px(0.0), px(0.0)));
        cx.notify();
    }

    /// Settings 页级刷新（SET-5）：重查 provider_auth_status + model_list；
    /// 失败保留现有列表并显示错误（复用 stale / OperationFailed 通道，
    /// 不新增缓存）。入口复核连接态，与可见按钮 gate 同源。
    pub(crate) fn on_refresh_settings(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            return;
        }
        self.refresh_all_settings(cx);
        self.refresh_expanded_account_quotas(true, cx);
        cx.notify();
    }

    fn on_send_message(&mut self, _: &SendMessage, window: &mut Window, cx: &mut Context<Self>) {
        if self.files_editor_focused(window) || self.files_filter_focused(window, cx) {
            return;
        }
        if self.browser.focus.is_focused(window) {
            if !self.browser.input.read(cx).is_composing() {
                self.browser_action("browser-go", window, cx);
            }
            return;
        }
        if self.quick_search.open {
            self.submit_quick_search(window, cx);
            return;
        }
        if self.settings_search_input_focused(window) {
            self.submit_settings_search(window, cx);
            return;
        }
        if self.navigation_input_focused(window) {
            self.step_find(true, cx);
            return;
        }
        // 搜索框的 Return 只选择模型；包括菜单卸载前的输入事件，不能发送草稿。
        if self.model_search_focus.is_focused(window) {
            if !self.model_search_input.read(cx).is_composing() {
                match self.open_menu.clone() {
                    Some(kind @ MenuKind::Model) => {
                        let ix = self.menu_highlight_effective(self.menu_selected_index());
                        self.activate_menu_item(kind, ix, window, cx);
                    }
                    Some(MenuKind::SettingsProviderModels(provider)) => {
                        if let Some(model) = self.settings_filtered_models(&provider).first() {
                            self.on_toggle_provider_model(provider, model.id.clone(), cx);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        // 行内改名编辑器聚焦时 Enter 属于提交改名，不是发送消息。
        if self.session_rename_input_focused(window, cx) {
            if let Some(rename) = self.session_rename.as_ref() {
                if rename.input.read(cx).is_composing() {
                    return;
                }
            }
            self.commit_session_rename(window, cx);
            return;
        }
        if self.account_rename_input_focused(window, cx) {
            if let Some(rename) = self.settings_account_rename.as_ref() {
                if rename.input.read(cx).is_composing() {
                    return;
                }
            }
            self.commit_account_rename(window, cx);
            return;
        }
        // IME 组合中的 Enter 属于输入法确认（gui-design §6）。
        if self.text_input.read(cx).is_composing() {
            return;
        }
        self.send_current_message(cx);
    }

    fn on_toggle_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let large_text = self.text_scale == font::TextScale::Percent150;
        let before = shell_layout::resolve(
            window.viewport_size().width,
            self.inspector_open,
            large_text,
        );
        self.inspector_open = !self.inspector_open;
        if !self.inspector_open {
            self.cancel_file_attachment_picker();
        }
        let after = shell_layout::resolve(
            window.viewport_size().width,
            self.inspector_open,
            large_text,
        );
        self.pending_inspector_focus = Some(inspector_focus_after_toggle(
            self.inspector_open,
            before.placement.is_center(),
        ));
        if before.placement.is_side() || after.placement.is_side() {
            self.timeline_changed();
        }
        if self.inspector_open {
            self.close_open_menu(cx);
            self.refresh_open_inspector_tab(cx);
        }
        cx.notify();
    }

    fn on_inspector_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inspector_open {
            return;
        }
        self.on_toggle_inspector(window, cx);
    }

    /// 切换 Inspector 顶层页签；切入 Changes / Resources 时拉取数据
    /// （拉取时机之一）。切页签不改 active session；各页签滚动状态独立保留。
    fn select_inspector_tab(&mut self, tab: InspectorTab, cx: &mut Context<Self>) {
        if tab != InspectorTab::Files {
            self.cancel_file_attachment_picker();
        }
        self.sync_browser_session(cx);
        if tab == InspectorTab::Browser {
            self.browser.open = true;
        }
        if self.inspector_tab == tab {
            return;
        }
        self.inspector_tab = tab;
        self.remember_inspector_tab(tab);
        self.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
        self.refresh_open_inspector_tab(cx);
        cx.notify();
    }

    /// 展开中的 Inspector 当前页签对应的数据刷新（Terminal 无查询面）。
    fn refresh_open_inspector_tab(&mut self, cx: &mut Context<Self>) {
        if !self.inspector_open {
            return;
        }
        self.remember_inspector_tab(self.inspector_tab);
        match self.inspector_tab {
            InspectorTab::Home | InspectorTab::Browser => {}
            InspectorTab::Files => self.ensure_files(cx),
            InspectorTab::Changes => self.refresh_changes(cx),
            InspectorTab::Resources => self.refresh_resources(cx),
            InspectorTab::Computer => self.refresh_computer_targets(cx),
            InspectorTab::Subagent => self.refresh_subagent_conversation(),
            InspectorTab::Terminal => {
                if self.projection.terminal.session_id.is_none()
                    && self.terminal_notice_text().is_none()
                {
                    self.ensure_terminal(cx);
                }
            }
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
    fn on_activity_open_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_open_menu(cx);
        if !self.inspector_open {
            self.inspector_open = true;
            let large_text = self.text_scale == font::TextScale::Percent150;
            if shell_layout::can_fit_side_inspector(window.viewport_size().width, large_text) {
                self.timeline_changed();
            }
        }
        self.inspector_tab = InspectorTab::Changes;
        self.remember_inspector_tab(InspectorTab::Changes);
        self.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
        self.refresh_changes(cx);
        cx.notify();
    }

    /// Activity 浮层子代理行点击 / AX Press：关闭浮层，展开 Inspector 并
    /// 切到「子代理」对话栏，选中该代理并拉取其会话时间线。
    pub(crate) fn on_activity_open_subagent(
        &mut self,
        agent_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_open_menu(cx);
        if !self.inspector_open {
            self.inspector_open = true;
            let large_text = self.text_scale == font::TextScale::Percent150;
            if shell_layout::can_fit_side_inspector(window.viewport_size().width, large_text) {
                self.timeline_changed();
            }
        }
        self.inspector_tab = InspectorTab::Subagent;
        self.remember_inspector_tab(InspectorTab::Subagent);
        self.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
        self.select_subagent_agent(agent_id, cx);
        cx.notify();
    }

    /// 选中子代理并拉取其对话（浮层行点击与对话栏切换器共用入口）。id
    /// 变化即整体复位旧代理时间线；同 id 重选也重查（终态水合同路径）。
    pub(crate) fn select_subagent_agent(&mut self, agent_id: &str, cx: &mut Context<Self>) {
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            return;
        }
        if self.projection.subagent_conversation.select_agent(agent_id) {
            // 换代理即换时间线：滚动偏移与跟随态不跨代理保留，先贴底，
            // 新内容到达后按跟随语义继续。
            self.subagent_conversation_scroll.jump_to_bottom();
            // chip 行同步 reveal：新选中项可能在横滑视口外。
            self.subagent_reveal_selected = true;
        }
        self.refresh_subagent_conversation();
        cx.notify();
    }

    /// 子代理对话栏数据刷新：被选代理的会话时间线分页 + Activity 列表
    ///（状态 / 结果权威口径同源）。断线时不派出，保留只读旧内容。
    pub(crate) fn refresh_subagent_conversation(&mut self) {
        self.load_activity_subagents();
        if let Some(agent_id) = self.projection.subagent_conversation.agent_id.clone() {
            if matches!(
                self.projection.connection,
                ConnectionState::Connected { .. }
            ) {
                let generation = self.projection.subagent_conversation.begin_loading();
                self.controller.load_subagent_timeline(agent_id, generation);
            }
        }
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
            // Side 并排的 Timeline 换行由 render 的 inspector_render_width 路径
            // reflow；中央呈现不得重置阅读位置。
        }
        self.inspector_tab = InspectorTab::Changes;
        self.remember_inspector_tab(InspectorTab::Changes);
        self.pending_inspector_focus = Some(InspectorFocusTarget::SelectedTab);
        self.refresh_changes(cx);
        let fetching = matches!(self.changes.fetch, changes::ChangesFetch::Fetching);
        if !was_available && !fetching {
            self.status_hint = Some(i18n::t("status.changes_not_available").into());
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
            (true, None) => self.changes.mark_failed(i18n::t("changes.no_workspace")),
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
            (true, None) => self
                .changes
                .mark_diff_failed(i18n::t("changes.no_workspace")),
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
        self.projection
            .select_terminal_for_workspace(workspace_id.as_deref());
        self.reconcile_terminal_input(cx);
        self.maybe_apply_fitted_terminal_size(cx);
    }

    fn reconcile_terminal_input(&mut self, cx: &mut Context<Self>) {
        let id = self.projection.terminal.session_id.clone();
        if self.terminal_input_key != id {
            self.terminal_input.update(cx, |input, cx| input.clear(cx));
            self.terminal_input_key = id;
        }
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
        if !workspace_action_active(self.route) {
            return;
        }
        self.on_approve("approve_once", window, cx);
    }

    fn on_approve_for_run(
        &mut self,
        _: &ApproveForRun,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !workspace_action_active(self.route) {
            return;
        }
        self.on_approve("approve_for_run", window, cx);
    }

    fn on_deny(&mut self, _: &Deny, window: &mut Window, cx: &mut Context<Self>) {
        if !workspace_action_active(self.route) {
            return;
        }
        self.on_approve("deny", window, cx);
    }

    fn on_cancel_run(&mut self, _: &CancelRun, window: &mut Window, cx: &mut Context<Self>) {
        if !workspace_action_active(self.route) {
            return;
        }
        self.on_cancel_clicked(window, cx);
    }

    /// 正式窗口首帧前恢复；构造纯 UI 状态不访问磁盘。
    pub(crate) fn restore_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.persist_appearance = true;
        match crate::platform::load_preferences() {
            Ok(prefs) => {
                self.language = if prefs.language == "zh" {
                    i18n::Language::Chinese
                } else {
                    i18n::Language::English
                };
                i18n::set_language(self.language);
                self.settings_search_input.update(cx, |input, cx| {
                    input.set_placeholder(i18n::t("settings.search.placeholder"), cx)
                });
                self.quick_search.input.update(cx, |input, cx| {
                    input.set_placeholder(i18n::t("quick.placeholder"), cx)
                });
                self.text_scale = match prefs.text_scale {
                    125 => font::TextScale::Percent125,
                    150 => font::TextScale::Percent150,
                    _ => font::TextScale::Percent100,
                };
                window.set_rem_size(px(self.text_scale.rem_pixels()));
            }
            Err(reason) => {
                let error = format!("{}: {reason}", i18n::t("settings.appearance.load_failed"));
                self.status_hint = Some(error.clone());
                self.appearance_error = Some(error);
            }
        }
    }

    fn save_appearance(
        &mut self,
        update: impl FnOnce(&mut crate::platform::DesktopPreferences),
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.persist_appearance {
            return true;
        }
        if let Err(reason) = crate::platform::save_preferences(update) {
            let error = format!("{}: {reason}", i18n::t("settings.appearance.save_failed"));
            self.status_hint = Some(error.clone());
            self.appearance_error = Some(error);
            cx.notify();
            return false;
        }
        self.appearance_error = None;
        true
    }

    fn set_text_scale(
        &mut self,
        scale: font::TextScale,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.save_appearance(|prefs| prefs.text_scale = scale.percent(), cx) {
            return;
        }
        self.text_scale = scale;
        window.set_rem_size(px(scale.rem_pixels()));
        self.show_status_feedback(
            i18n::t("status.text_scale").replace("{}", &scale.percent().to_string()),
            cx,
        );
        cx.notify();
    }

    /// 切换界面语言（i18n）：更新全局值与本地镜像，给出状态提示并重渲染。
    /// 与 set_text_scale 同构：仅本地 presentation，不触碰 Host / 会话状态。
    fn set_language(&mut self, language: i18n::Language, cx: &mut Context<Self>) {
        if self.language == language {
            return;
        }
        if !self.save_appearance(
            |prefs| {
                prefs.language = if language == i18n::Language::Chinese {
                    "zh"
                } else {
                    "en"
                }
                .into();
            },
            cx,
        ) {
            return;
        }
        self.language = language;
        i18n::set_language(language);
        // （composer placeholder 每次 render 经状态机重设，无需处理）。
        self.settings_search_input.update(cx, |input, cx| {
            input.set_placeholder(i18n::t("settings.search.placeholder"), cx)
        });
        self.quick_search.input.update(cx, |input, cx| {
            input.set_placeholder(i18n::t("quick.placeholder"), cx)
        });
        self.status_hint = Some(i18n::t("status.language").replace("{}", language.display_name()));
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
        if !workspace_action_active(self.route) {
            return;
        }
        self.on_new_session(window, cx);
    }

    fn on_toggle_inspector_action(
        &mut self,
        _: &ToggleInspector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !workspace_action_active(self.route) {
            return;
        }
        self.on_toggle_inspector(window, cx);
    }

    fn send_terminal_bytes(&mut self, data: String, cx: &mut Context<Self>) {
        if !terminal_can_operate(&self.projection.connection, &self.projection.terminal) {
            return;
        }
        if let Some(id) = self.projection.terminal.session_id.clone() {
            self.terminal_scroll.jump_to_bottom();
            self.write_terminal_bytes(id, data, cx);
        }
    }

    fn write_terminal_bytes(&mut self, id: String, data: String, cx: &mut Context<Self>) {
        if data.is_empty() {
            return;
        }
        if !self.terminal_pending_write.insert(id.clone()) {
            self.terminal_queued_write
                .entry(id)
                .or_default()
                .push_str(&data);
            return;
        }
        self.controller.terminal_write(id, data);
        cx.notify();
    }

    fn flush_queued_terminal_write(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(data) = self.terminal_queued_write.remove(id) {
            self.write_terminal_bytes(id.to_owned(), data, cx);
        }
    }

    fn intercept_terminal_keystroke(
        &mut self,
        key: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.open_menu.is_some()
            || self.quick_search.open
            || !self.inspector_open
            || self.inspector_tab != InspectorTab::Terminal
            || !self
                .terminal_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        {
            return false;
        }
        if key.modifiers.platform && key.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let mut screen = pawork_terminal::Screen::new();
                screen.feed(&self.projection.terminal.output);
                let text = text.replace("\r\n", "\n").replace('\n', "\r");
                let data = if screen.bracketed_paste {
                    format!("\x1b[200~{text}\x1b[201~")
                } else {
                    text
                };
                self.send_terminal_bytes(data, cx);
            }
            return true;
        }
        if self.terminal_input.read(cx).is_composing() {
            return false;
        }
        // 普通字符只由 EntityInputHandler 提交一次，保留原生 IME。
        if !key.modifiers.control
            && (key
                .key_char
                .as_ref()
                .is_some_and(|text| text.chars().all(|ch| !ch.is_control()))
                || key.key == "space")
        {
            return false;
        }
        let Some(data) = terminal_view::keystroke_to_pty_bytes(key) else {
            return false;
        };
        self.send_terminal_bytes(data, cx);
        true
    }
    fn terminal_fitted_size_from_bounds(&self, width: f32, height: f32) -> Option<(u16, u16)> {
        terminal_view::size_from_bounds_scaled(
            width,
            height,
            self.text_scale.rem_pixels() / crate::ui::theme::font::BASE_REM_PIXELS,
        )
    }

    pub(super) fn terminal_output_size_changed(&self, width: f32, height: f32) -> bool {
        self.terminal_fitted_size_from_bounds(width, height)
            .is_some_and(|size| self.terminal_fitted_size != Some(size))
    }

    pub(super) fn observe_terminal_output_size(
        &mut self,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) {
        let Some((columns, rows)) = self.terminal_fitted_size_from_bounds(width, height) else {
            return;
        };
        if self.terminal_fitted_size == Some((columns, rows)) {
            return;
        }
        self.terminal_fitted_size = Some((columns, rows));
        let timer = cx
            .background_executor()
            .timer(terminal_view::TERMINAL_RESIZE_DEBOUNCE);
        self.terminal_fit_resize_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |view, cx| {
                view.maybe_apply_fitted_terminal_size(cx);
            });
        }));
    }

    pub(super) fn maybe_apply_fitted_terminal_size(&mut self, cx: &mut Context<Self>) {
        let Some((columns, rows)) = self.terminal_fitted_size else {
            return;
        };
        if !terminal_can_operate(&self.projection.connection, &self.projection.terminal) {
            return;
        }
        if self.terminal_pending_resize.is_some() {
            return;
        }
        let Some(id) = self.projection.terminal.session_id.clone() else {
            return;
        };
        if (
            self.projection.terminal.columns,
            self.projection.terminal.rows,
        ) == (columns, rows)
        {
            return;
        }
        self.terminal_pending_resize = Some((id.clone(), columns, rows));
        self.controller.terminal_resize(id, columns, rows);
        cx.notify();
    }

    fn send_current_message(&mut self, cx: &mut Context<Self>) {
        // Settings 等非工作台页的可写 TextInput 与 Composer 共用 Enter。
        // 搜索框和账号改名在到达这里之前返回；工作台内查找不改变路由，
        // 显式 AX Send 仍走下面的发送。
        if self.route != AppRoute::Workspace {
            return;
        }
        if !self.can_send(cx) {
            return;
        }
        let text = self.text_input.read(cx).text().to_string();
        let options = self.current_composer_options();
        if text.trim().is_empty() && options.attachments.is_empty() && options.video_urls.is_empty()
        {
            return;
        }
        self.composer_sending = true;
        self.status_hint = None;
        self.composer_send_errors
            .remove(&self.projection.active_session_id);
        let model = self.projection.effective_model().cloned();
        // ADR-063：显式选择的推理强度随本轮 RunStart 发出；None（自动）
        // 省略，Host 回落模型默认。
        let effort = self.projection.effective_effort().map(str::to_string);
        let unstarted = self
            .projection
            .unstarted_session_id(None)
            .map(str::to_string);
        match home_send_plan(
            self.projection.active_session_id.as_deref(),
            unstarted.as_deref(),
        ) {
            HomeSendPlan::ActiveSession(session_id) => {
                self.composer_send_target = Some(session_id.clone());
                self.controller
                    .send_message(session_id, text, model, effort, options);
            }
            HomeSendPlan::ReuseUnstarted(session_id) => {
                self.composer_send_target = Some(session_id.clone());
                self.composer_send_errors.remove(&Some(session_id.clone()));
                self.composer_drafts
                    .insert(session_id.clone(), text.clone());
                self.no_session_draft.clear();
                self.composer_options.remove(&None);
                self.composer_options
                    .insert(Some(session_id.clone()), options.clone());
                self.open_session(session_id.clone(), cx);
                self.controller
                    .send_message(session_id, text, model, effort, options);
            }
            HomeSendPlan::CreateUnassigned => {
                self.composer_send_target = None;
                self.pending_home_send = Some(PendingHomeSend {
                    text,
                    model,
                    effort,
                    options,
                });
                self.controller.create_session(None);
                cx.notify();
            }
        }
        cx.notify();
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

    /// model 菜单能否打开：已连接、无进行中 run，且要么已有目录、要么
    /// 目录查询已完成（结果可为空——全关空态仍要从触发器上方打开说明行）。
    fn can_open_model_menu(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_run_id.is_none()
            && (!self.projection.models.is_empty() || self.projection.models_loaded)
    }

    /// 强度菜单能否打开（ADR-063）：与模型切换同 gate，且已有生效模型
    ///（无模型时强度无意义）。
    fn can_open_effort_menu(&self) -> bool {
        self.can_switch_model() && self.projection.effective_model().is_some()
    }

    fn composer_has_sendable_text(&self, cx: &App) -> bool {
        !self.text_input.read(cx).text().trim().is_empty()
            || !self.current_composer_options().attachments.is_empty()
            || !self.current_composer_options().video_urls.is_empty()
    }

    fn can_send(&self, cx: &App) -> bool {
        if self.composer_loading
            || self.composer_sending
            || self.composer_capability_error().is_some()
        {
            return false;
        }
        input_area::composer_send_allowed(
            matches!(
                self.projection.connection,
                ConnectionState::Connected { .. }
            ),
            self.projection.active_run_id.is_some(),
            self.model_catalog_empty(),
            self.composer_has_sendable_text(cx),
            self.pending_home_send.is_some(),
        )
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
        ) && timeline_entry::fork_target(&self.projection.timeline, event_id).is_some()
    }

    /// Timeline 数据 / 可视宽度变更标记：下一次 render 时对 list 做一次
    /// reset（projection 有条目替换语义，splice 不安全，见 ui/timeline.rs）。
    fn timeline_changed(&mut self) {
        self.timeline_rev += 1;
    }
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

pub(super) fn rail_session_focus_key(session_id: &str) -> String {
    format!("task-{session_id}")
}

/// 行内改名（铅笔）按钮焦点键；与 archive 一样按 session 懒建。
pub(super) fn rail_session_rename_focus_key(session_id: &str) -> String {
    format!("task-rename-{session_id}")
}

/// 行内归档按钮焦点键。
pub(super) fn rail_session_archive_focus_key(session_id: &str) -> String {
    format!("task-archive-{session_id}")
}

/// ADR-054 D2 行内改名编辑态：同一时刻至多一个 session 处于编辑。
struct SessionRenameState {
    session_id: String,
    input: Entity<TextInput>,
}

/// 行内改名的提交裁决：trim 后为空 → 保留编辑器不提交（与 Host 的空
/// title 结构化拒绝同语义）；与当前标题一致 → 仅退出编辑；否则提交。
#[derive(Debug)]
pub(crate) enum SessionRenameDecision {
    KeepEditing,
    Close,
    Submit(String),
}

pub(crate) fn session_rename_decision(current: &str, draft: &str) -> SessionRenameDecision {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        SessionRenameDecision::KeepEditing
    } else if trimmed == current {
        SessionRenameDecision::Close
    } else {
        SessionRenameDecision::Submit(trimmed.to_string())
    }
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

/// 按 GUI2-01 组装 rail 焦点链：全局新建 → scope → grouping →（按当前
/// 分组渲染序）项目头 / 定向新建 / task 行。折叠项目只保留头部行。
fn rail_focus_stops(
    grouping: TaskRailGrouping,
    scope: Option<&str>,
    collapsed: &BTreeSet<String>,
    projection: &DesktopProjection,
    now_ms: u64,
) -> Vec<RailStop> {
    let mut stops = vec![
        RailStop::Search,
        RailStop::AddTask,
        RailStop::Scope,
        RailStop::Grouping,
    ];
    let projects = match grouping {
        TaskRailGrouping::Timeline => projection
            .timeline_groups(scope, now_ms)
            .into_iter()
            .flat_map(|group| {
                let bucket = group.bucket;
                let skip_header = group.skip_project_header(scope);
                group
                    .projects
                    .into_iter()
                    .map(move |project| (Some(bucket), project, skip_header))
            })
            .collect::<Vec<_>>(),
        TaskRailGrouping::Projects => projection
            .project_groups(scope)
            .into_iter()
            .map(|project| (None, project, false))
            .collect(),
    };
    for (bucket, project, skip_header) in projects {
        let key = rail_project_key(project.workspace_id.as_deref());
        if !skip_header {
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
        }
        if skip_header || !collapsed.contains(&key) {
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

fn changes_tab_key_target(current: usize, key: &str) -> Option<usize> {
    match key {
        "left" | "up" | "right" | "down" => Some((current + 1) % 2),
        "enter" | "space" => Some(current),
        _ => None,
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

/// Host 快照或 live TerminalExited 已证明终态的终端。`failed` 只来自
/// TerminalExitReason::Failed；瞬态 write/resize 错误不会改 runtime_state，
/// 因此可安全开放 Close，而不会把仍可操作的 running 终端误判为终态。
pub(crate) fn terminal_known_ended(terminal: &TerminalState) -> bool {
    matches!(
        terminal.runtime_state.as_deref(),
        Some("exited") | Some("killed") | Some("failed")
    )
}

/// 只有 Host 已证明进程退出或被终止的终端可直接 New。`failed` 表示
/// forwarder 断流，进程可能仍在运行，必须先 Close 清理后再 Start。
pub(crate) fn terminal_can_reopen(terminal: &TerminalState) -> bool {
    matches!(
        terminal.runtime_state.as_deref(),
        Some("exited") | Some("killed")
    )
}

/// 关闭标签的同源 gate：运行中或明确终态，且连接可用。
pub(crate) fn terminal_can_close(connection: &ConnectionState, terminal: &TerminalState) -> bool {
    matches!(connection, ConnectionState::Connected { .. })
        && (terminal_can_operate(connection, terminal) || terminal_known_ended(terminal))
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_navigation_session();
        self.sync_browser_session(cx);
        self.ensure_browser_poll(cx);
        // CU-13：Host 浏览器请求在 handle_controller_event 里即时派发，
        // render 只负责刷新创建 WebView 所需的窗口原始句柄。
        // 测试平台窗口没有真实句柄（window_handle 会 panic），与
        // product_access 同款 cfg!(test) 守门。原生 WebView 只有 macOS
        // 实现；gpui 0.2.2 的 X11 窗口 window_handle 是 unimplemented!()，
        // 其它平台取句柄会在首帧 panic。
        if !cfg!(test) && cfg!(target_os = "macos") {
            self.raw_window_handle = raw_window_handle::HasWindowHandle::window_handle(window)
                .ok()
                .map(|handle| handle.as_raw());
        }
        if !self.browser_visible() {
            for tab in &self.browser.tabs {
                tab.native.set_visible(false);
            }
        }
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
            self.inspector_reveal_selected = true;
            match target {
                InspectorFocusTarget::Activity => window.focus(&self.inspector_activity_focus),
                InspectorFocusTarget::SelectedTab => {
                    if self.inspector_tab == InspectorTab::Terminal
                        && self.projection.terminal.session_id.is_some()
                    {
                        window.focus(&self.terminal_input.read(cx).focus_handle(cx));
                    } else if let Some(tab) =
                        self.panel_tabs(cx).into_iter().find(|tab| tab.selected)
                    {
                        let focus = self
                            .inspector_item_focus
                            .entry(tab.id)
                            .or_insert_with(|| cx.focus_handle().tab_stop(true))
                            .clone();
                        window.focus(&focus);
                    } else {
                        window.focus(&self.inspector_panel_focus);
                    }
                }
                InspectorFocusTarget::Composer => {
                    window.focus(&self.composer_focus_handle(cx));
                }
            }
        }
        if let Some(MenuKind::SettingsProviderModels(provider)) = &self.open_menu {
            let models = self.settings_filtered_models(provider);
            let focused = models.iter().position(|model| {
                self.settings_action_focus
                    .get(&settings::settings_model_switch_identifier(
                        provider, &model.id,
                    ))
                    .is_some_and(|focus| focus.is_focused(window))
            });
            if focused != self.menu_highlight {
                self.menu_highlight = focused;
                if let (Some(ix), Some(scroll)) = (
                    focused,
                    self.settings_element_layouts.get(&format!(
                        "{}-list",
                        settings::settings_models_menu_identifier(provider)
                    )),
                ) {
                    scroll.scroll_to_item(ix);
                }
            }
        }
        if self.pending_model_menu_scroll {
            if !matches!(self.open_menu, Some(MenuKind::Model)) || self.model_menu_row_count() == 0
            {
                self.pending_model_menu_scroll = false;
            } else if self.model_menu_scroll.bounds().size.height > px(0.0) {
                let highlight = self.menu_highlight_effective(self.menu_selected_index());
                if let Some(child) = self.model_menu_scroll_child_index(highlight) {
                    self.model_menu_scroll.scroll_to_item(child);
                }
                self.pending_model_menu_scroll = false;
            } else {
                cx.defer_in(window, |_view, _window, cx| cx.notify());
            }
        }
        if self.settings_locate_pending.is_some() {
            self.finish_settings_locate(window, cx);
        }
        if self.pending_scope_menu_scroll {
            if !matches!(
                self.open_menu,
                Some(MenuKind::Scope | MenuKind::ProjectTask)
            ) {
                self.pending_scope_menu_scroll = false;
            } else if self.scope_menu_scroll.bounds().size.height > px(0.0) {
                let selected = self.menu_highlight_effective(self.menu_selected_index());
                self.scope_menu_scroll.scroll_to_item(selected);
                self.pending_scope_menu_scroll = false;
            } else {
                // 新句柄还没有 bounds/overflow，完成首帧后再发滚入请求。
                cx.defer_in(window, |_view, _window, cx| cx.notify());
            }
        }
        // 这些控件的 AX 读取 GPUI 实测布局；首帧及滚动后的 prepaint 完成后
        // 再同步一次，不依赖网络事件或 Run 时钟刷新，也不产生重绘循环。
        if self.ax_bridge.is_some() {
            let view = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                let _ = view.update(cx, |view, cx| view.sync_accessibility(window, cx));
            });
        }
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let can_open_model_menu = self.can_open_model_menu();
        // can_open_model_menu 翻假期间归一化：打开中的 model 菜单随之关闭，
        // 避免条件恢复后面板无需点击自行重现。全关空态仍保持可开（说明行）。
        if matches!(self.open_menu, Some(MenuKind::Model)) && !can_open_model_menu {
            self.open_menu = None;
        }
        // 强度菜单同口径（ADR-063）：运行中 / 断线 / 无模型时关闭。
        if matches!(self.open_menu, Some(MenuKind::Effort)) && !self.can_open_effort_menu() {
            self.open_menu = None;
        }
        let now_ms = now_unix_ms();
        let run_metrics = self
            .projection
            .run_usage_display(self.projection.status_run_id(), now_ms);
        // GUI2-05：窄窗显式打开走中央呈现；InspectorMotion 只用于宽窗并排。
        let shell = shell_layout::resolve(
            window.viewport_size().width,
            self.inspector_open,
            self.text_scale == font::TextScale::Percent150,
        );
        let placement = if self.route == AppRoute::Workspace {
            shell.placement
        } else {
            InspectorPlacement::Hidden
        };
        let can_fit_side = self.route == AppRoute::Workspace
            && shell_layout::can_fit_side_inspector(
                window.viewport_size().width,
                self.text_scale == font::TextScale::Percent150,
            );
        let now = std::time::Instant::now();
        let (inspector_width, inspector_animating) = match placement {
            InspectorPlacement::Center => {
                self.inspector_motion.snap(0.0);
                (0.0, false)
            }
            InspectorPlacement::Side => {
                if self.inspector_last_placement.is_center() {
                    self.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
                    (metrics::INSPECTOR_WIDTH, false)
                } else {
                    self.inspector_motion.width(true, true, now)
                }
            }
            InspectorPlacement::Hidden => self.inspector_motion.width(false, can_fit_side, now),
        };
        self.inspector_last_placement = placement;
        if inspector_width != self.inspector_render_width {
            // 包括最后一帧与窄窗归零，避免文本沿用过渡中间帧的测高。
            self.inspector_render_width = inspector_width;
            self.timeline_changed();
        }
        if inspector_animating {
            window.request_animation_frame();
        }
        self.sync_accessibility(window, cx);

        let (activity_trigger_visible, activity_popover_open) = activity_header_visibility(
            placement.is_visible(),
            matches!(self.open_menu, Some(MenuKind::Activity)),
        );

        // SET-3 顶层路由：Settings 壳与工作台互斥渲染。工作台状态全部
        // 保留在 AppView 字段，返回即原样恢复。
        let (sidebar, main) = match self.route {
            AppRoute::Workspace => {
                let sidebar = self.sidebar_element(px(shell.rail_width), window, cx);
                let header = self.workspace_header_element(
                    activity_trigger_visible,
                    activity_popover_open,
                    cx,
                );
                let workspace = if placement.is_center() {
                    div()
                        .id("shell-workspace")
                        .debug_selector(|| "shell-workspace".into())
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(header)
                        .child(self.inspector_back_bar(cx))
                        .child(
                            div()
                                .id("shell-inspector")
                                .debug_selector(|| "shell-inspector".into())
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .min_h_0()
                                .child(self.inspector_element(
                                    connected,
                                    InspectorPlacement::Center,
                                    window,
                                    cx,
                                )),
                        )
                } else {
                    let timeline_area = self.timeline_area(window, cx);
                    let composer = self.composer_element(window, cx);
                    div()
                        .id("shell-workspace")
                        .debug_selector(|| "shell-workspace".into())
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        // 并排路径高度链（P0 滚动修复）：Workspace / 并排
                        // Inspector 的滚动容器都以「列内视口高度」为约束；
                        // 缺 min_h_0 时 min-height:auto 会随内容（尤其子
                        // 代理对话栏的非虚拟化长 transcript）把整行撑高，
                        // list / overflow 滚动区视口等于内容高 → 左右两栏
                        // 滚轮都失效。与中央路径（上方 min_h_0）同口径。
                        .min_h_0()
                        .child(header)
                        .child(timeline_area)
                        .child(composer)
                };

                let mut main = div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(workspace);
                if inspector_width > 0.0 {
                    main = main.child(
                        div()
                            .id("shell-inspector")
                            .debug_selector(|| "shell-inspector".into())
                            .w(px(inspector_width))
                            .flex_none()
                            .overflow_hidden()
                            .flex()
                            .h_full()
                            .min_h_0()
                            .child(self.inspector_element(
                                connected,
                                InspectorPlacement::Side,
                                window,
                                cx,
                            )),
                    );
                }
                (sidebar, main)
            }
            AppRoute::Settings => {
                let sidebar = self.settings_rail_element(px(shell.rail_width), window, cx);
                let main = div().flex().flex_row().flex_1().min_w_0().min_h_0().child(
                    div()
                        .id("shell-settings")
                        .debug_selector(|| "shell-settings".into())
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .child(self.settings_page_element(cx)),
                );
                (sidebar, main)
            }
        };
        let search_open = self.quick_search.open;
        let search_overlay = search_open.then(|| self.quick_search_element(window, cx));
        div()
            .relative()
            .key_context("AppView")
            .track_focus(&self.focus_handle)
            .child(components::focus_ring::track_pointer_input())
            .child(self.browser_pointer_focus())
            .capture_key_down(components::focus_ring::key_down)
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
            .on_scroll_wheel(cx.listener(|view, _event, _window, cx| {
                if matches!(
                    view.open_menu,
                    Some(MenuKind::Scope | MenuKind::ProjectTask)
                ) || (view.route == AppRoute::Settings
                    && view.settings_page == SettingsPage::Appearance)
                {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(Self::on_send_message))
            .on_action(cx.listener(Self::on_save_file))
            .on_action(cx.listener(Self::on_quick_search))
            .on_action(cx.listener(Self::on_find_conversation))
            .when(!search_open, |root| {
                root.on_action(cx.listener(Self::on_approve_once))
                    .on_action(cx.listener(Self::on_approve_for_run))
                    .on_action(cx.listener(Self::on_deny))
                    .on_action(cx.listener(Self::on_cancel_run))
                    .on_action(cx.listener(Self::on_new_task_action))
                    .on_action(cx.listener(Self::on_toggle_inspector_action))
                    .on_action(cx.listener(Self::on_task_cycle_up))
                    .on_action(cx.listener(Self::on_task_cycle_down))
                    .on_action(cx.listener(Self::on_next_needs_attention_action))
            })
            .on_action(cx.listener(Self::on_increase_text_size))
            .on_action(cx.listener(Self::on_decrease_text_size))
            .on_action(cx.listener(Self::on_reset_text_size))
            .capture_key_down(cx.listener(|view, event, window, cx| {
                if view.quick_search.open {
                    view.handle_quick_search_key(event, window, cx);
                } else {
                    view.navigation_key(event, window, cx);
                }
            }))
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
                    // main 高度收敛到列视口（同 P0 高度链）。
                    .min_h_0()
                    .child(main)
                    // F-13：信息串居中；Inspector/Activity 触发器已随
                    // F-12（R6 Wave A）迁至 Workspace Header。P2-1：
                    // StatusBar 只在工作台渲染（常驻），Settings 壳不显示
                    // RunStatusBar（render 与 AX 同源）。
                    .when(self.run_status_visible(), |column| {
                        let mut bar = StatusBar::new()
                            .centered(timeline_entry::run_metrics_element(&run_metrics))
                            .trailing(
                                div()
                                    .id("status-bar-trailing")
                                    .flex()
                                    .flex_row()
                                    .flex_1()
                                    .min_w_0()
                                    .justify_end()
                                    .child(div().truncate().child(self.status_bar_trailing_text())),
                            );
                        if let Some(text) = self.status_bar_leading_text() {
                            bar = bar.leading(
                                div()
                                    .id("status-bar-leading")
                                    .flex()
                                    .flex_row()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().truncate().child(text)),
                            );
                        }
                        column.child(bar)
                    }),
            )
            .children(search_overlay.map(gpui::deferred))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::SessionSummary;

    fn replayed_run_event(
        sequence: u64,
        session: &str,
        run: &str,
        state: &str,
    ) -> pawork_client::AppEventEnvelope {
        serde_json::from_value(serde_json::json!({
            "api_version": { "major": 1, "minor": 27 },
            "instance_id": "instance-1",
            "event_id": format!("app-{sequence}"),
            "global_sequence": sequence,
            "stream": { "type": "session", "id": session },
            "stream_sequence": sequence,
            "timestamp": 1_000 + sequence,
            "source": { "type": "core" },
            "payload": { "type": "run_changed", "data": { "run_id": run, "state": state } }
        }))
        .expect("decode AppEventEnvelope")
    }

    /// CU-19：断线期间到达的 run 终态经 Replay 补发恢复——重放事件
    /// 不经 live 事件接线，须把面板绑定会话内的终态交给停止状态机，
    /// 已停止任务不得永久停在 Unknown。
    #[gpui::test]
    fn replayed_run_terminal_confirms_stop_state(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("cu19-replay-stop.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let request = view.computer.begin_loading("s1");
                view.computer.apply_loaded(
                    "s1",
                    request,
                    serde_json::from_value(serde_json::json!({
                        "session_id": "s1",
                        "workspace_id": "ws-1",
                        "targets": [],
                    }))
                    .expect("empty list shape"),
                );
                view.computer.stop = Some(computer::ComputerStop {
                    key: "k".into(),
                    run_id: "r1".into(),
                    action_id: 7,
                    phase: computer::ComputerStopPhase::Unknown,
                });
                // 非终态 + 异会话终态：都不收敛。
                view.apply_replayed_stop_confirmations(&[
                    replayed_run_event(1, "s1", "r1", "streaming_response"),
                    replayed_run_event(2, "s2", "r1", "completed"),
                ]);
                assert_eq!(
                    view.computer.stop.as_ref().unwrap().phase,
                    computer::ComputerStopPhase::Unknown
                );
                // 绑定会话内的 run 终态 → 已停止（与 live 接线同语义）。
                view.apply_replayed_stop_confirmations(&[replayed_run_event(
                    3,
                    "s1",
                    "r1",
                    "cancelled",
                )]);
                assert_eq!(
                    view.computer.stop.as_ref().unwrap().phase,
                    computer::ComputerStopPhase::Stopped
                );
                cx.notify();
            });
        });
    }

    fn session(id: &str, workspace: Option<&str>) -> SessionSummary {
        SessionSummary {
            session_id: id.into(),
            title: format!("Task {id}"),
            updated_at_ms: 1_000,
            workspace_id: workspace.map(str::to_string),
            parent_branch_id: None,
            forked_from_event_id: None,
            active: false,
            unstarted: false,
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
            ("cmd-alt-up", "TaskCycleUp"),
            ("cmd-alt-down", "TaskCycleDown"),
            ("cmd-alt-n", "NextNeedsAttention"),
        ] {
            assert!(
                APP_VIEW_KEYBINDINGS
                    .iter()
                    .any(|(bound, bound_action)| *bound == key && *bound_action == action),
                "missing binding {key} -> {action}"
            );
        }
        assert!(workspace_action_active(AppRoute::Workspace));
        assert!(!workspace_action_active(AppRoute::Settings));
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

    #[gpui::test]
    fn terminal_bytes_queue_in_order_per_pty(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        let view = cx.new(|cx| {
            AppView::new(
                Arc::new(Platform::new()),
                std::env::temp_dir().join("terminal-queue-review.sock"),
                None,
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.terminal_pending_write.extend(["a".into(), "b".into()]);
            for (id, data) in [("a", "x"), ("b", "y"), ("a", "x"), ("a", "\r")] {
                view.write_terminal_bytes(id.into(), data.into(), cx);
            }
            assert_eq!(
                view.terminal_queued_write.get("a").map(String::as_str),
                Some("xx\r")
            );
            assert_eq!(
                view.terminal_queued_write.get("b").map(String::as_str),
                Some("y")
            );
            view.handle_controller_event(
                ControllerEvent::TerminalWriteSucceeded {
                    terminal_session_id: "a".into(),
                },
                cx,
            );
            assert!(!view.terminal_queued_write.contains_key("a"));
            assert!(view.terminal_pending_write.contains("a"));
            view.handle_controller_event(
                ControllerEvent::TerminalWriteFailed {
                    terminal_session_id: "b".into(),
                    reason: "closed".into(),
                },
                cx,
            );
            assert!(view.terminal_pending_write.contains("a"));
            assert!(!view.terminal_pending_write.contains("b"));
            assert!(!view.terminal_queued_write.contains_key("b"));
        });
    }

    #[gpui::test]
    fn archived_active_session_restores_drafts_and_terminal_scope(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        use serde_json::json;

        let platform = Arc::new(Platform::new());
        let socket = std::env::temp_dir().join("opt2-archive-snapshot-test.sock");
        let view = cx.new(|cx| AppView::new(platform, socket, None, cx));
        let mut snapshot: pawork_client::Snapshot = serde_json::from_value(json!({
            "instance_id": "test", "snapshot_sequence": 1, "generated_at": 1,
            "sections": [
                {"kind": "workspaces", "revision": 1, "data": [
                    {"id": "ws-a", "name": "A", "trusted": true},
                    {"id": "ws-b", "name": "B", "trusted": true}
                ]},
                {"kind": "session_tree", "revision": 1, "data": [
                    {"session_id": "s-a", "title": "A", "workspace_id": "ws-a", "updated_at_ms": 1}
                ]},
                {"kind": "terminal_sessions", "revision": 1, "data": [
                    {"terminal_session_id": "term-a", "owner_session": "ws-a", "state": "running"},
                    {"terminal_session_id": "term-b", "owner_session": "ws-b", "state": "running"}
                ]}
            ]
        }))
        .unwrap();
        view.update(cx, |view, cx| {
            view.projection.merge_snapshot(&snapshot);
            view.projection.select_session("s-a");
            // 切 scope 保留当前会话；归档后应回到 scope B，而非默认 A。
            view.scope_workspace_id = Some("ws-b".into());
            view.reconcile_terminal_workspace(cx);
            view.text_input
                .update(cx, |input, cx| input.set_text("draft for A", cx));
            view.no_session_draft = "no session draft".into();
            view.timeline_paging = true;
            view.changes.selected = Some("old-file".into());
        });
        snapshot.sections[1].data = Some(json!([]));
        // 回执与 SessionMetaChanged 各触发一次刷新，两次均不串草稿/终端。
        for _ in 0..2 {
            view.update(cx, |view, cx| {
                view.handle_controller_event(ControllerEvent::Snapshot(snapshot.clone()), cx);
                assert!(view.projection.active_session_id.is_none());
                assert!(!view.timeline_paging);
                assert!(view.changes.selected.is_none());
                assert_eq!(
                    view.composer_drafts.get("s-a").map(String::as_str),
                    Some("draft for A")
                );
                assert_eq!(view.text_input.read(cx).text(), "no session draft");
                assert_eq!(view.terminal_input_key.as_deref(), Some("term-b"));
                assert_eq!(
                    view.projection.terminal.session_id.as_deref(),
                    Some("term-b")
                );
            });
        }
    }

    /// ADR-054 D2：行内改名提交裁决——空标题（含纯空白）不提交，保持
    /// 编辑器；未变化仅退出；变化才发 SessionRename。
    #[test]
    fn session_rename_decision_gates_empty_and_unchanged_titles() {
        assert!(matches!(
            session_rename_decision("Old", "   "),
            SessionRenameDecision::KeepEditing
        ));
        assert!(matches!(
            session_rename_decision("Old", ""),
            SessionRenameDecision::KeepEditing
        ));
        assert!(matches!(
            session_rename_decision("Old", "Old"),
            SessionRenameDecision::Close
        ));
        match session_rename_decision("Old", "  New title  ") {
            SessionRenameDecision::Submit(title) => assert_eq!(title, "New title"),
            other => panic!("expected submit, got {other:?}"),
        }
    }

    #[test]
    fn home_send_plan_prefers_active_then_unassigned_unstarted() {
        assert_eq!(
            home_send_plan(Some("s-1"), Some("s-open")),
            HomeSendPlan::ActiveSession("s-1".into())
        );
        assert_eq!(
            home_send_plan(None, Some("s-open")),
            HomeSendPlan::ReuseUnstarted("s-open".into())
        );
        assert_eq!(home_send_plan(None, None), HomeSendPlan::CreateUnassigned);
    }

    /// GUI2-01：全局新建 → scope → grouping → 项目头 / 定向新建 → task 行；
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
            &keys[..4],
            [
                "quick-search",
                "add-task",
                "project-scope",
                "task-rail-grouping"
            ]
        );
        assert_eq!(
            RAIL_TAB_STOP_IDS,
            [
                "quick-search",
                "add-task",
                "project-scope",
                "task-rail-grouping"
            ]
        );
        // Projects 模式：Alpha 头 + 定向新建 + 两行任务；折叠的 Unassigned
        // 只剩头部（无定向新建）。
        assert_eq!(
            keys[4..],
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
            timeline_keys[4..],
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

        let mut unassigned_only = DesktopProjection::default();
        unassigned_only.sessions = vec![session("s-3", None)];
        let skipped = rail_focus_stops(
            TaskRailGrouping::Timeline,
            None,
            &BTreeSet::new(),
            &unassigned_only,
            60_000,
        );
        let skipped_keys: Vec<String> = skipped.iter().map(|stop| stop.focus_key()).collect();
        assert_eq!(skipped_keys[4..], ["task-s-3"]);

        let scoped = rail_focus_stops(
            TaskRailGrouping::Timeline,
            Some("ws-a"),
            &BTreeSet::new(),
            &projection,
            60_000,
        );
        let scoped_keys: Vec<String> = scoped.iter().map(|stop| stop.focus_key()).collect();
        assert_eq!(scoped_keys[4..], ["task-s-1", "task-s-2"]);
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
        assert_eq!(changes_tab_key_target(0, "down"), Some(1));
        assert_eq!(changes_tab_key_target(1, "up"), Some(0));
        assert_eq!(changes_tab_key_target(0, "escape"), None);
    }

    #[test]
    fn inspector_toggle_focus_moves_between_panel_and_activity() {
        assert_eq!(
            inspector_focus_after_toggle(false, false),
            InspectorFocusTarget::Activity
        );
        assert_eq!(
            inspector_focus_after_toggle(true, false),
            InspectorFocusTarget::SelectedTab
        );
        assert_eq!(
            inspector_focus_after_toggle(false, true),
            InspectorFocusTarget::Composer
        );
    }

    /// G2 / ADR-045：已知 exited/killed 终端的 Start 恢复为「新建终端」
    /// 入口；failed 只开放 Close，避免在旧进程可能仍运行时直接 New。
    /// 状态未知的终端不猜生命周期。
    #[test]
    fn known_terminal_end_states_use_safe_recovery_actions() {
        use crate::projection::TerminalAvailability;

        let connected = ConnectionState::Connected {
            instance_id: "instance-1".into(),
        };
        let disconnected = ConnectionState::Disconnected {
            reason: "closed".into(),
        };
        let exited = TerminalState {
            session_id: Some("term-a".into()),
            workspace_id: Some("ws-a".into()),
            runtime_state: Some("exited".into()),
            availability: TerminalAvailability::Stale {
                reason: "terminal exited".into(),
            },
            ..TerminalState::default()
        };
        assert!(!terminal_can_operate(&connected, &exited));
        assert!(terminal_can_reopen(&exited));
        assert!(terminal_can_close(&connected, &exited));

        let failed = TerminalState {
            session_id: Some("term-failed".into()),
            runtime_state: Some("failed".into()),
            availability: TerminalAvailability::Stale {
                reason: "terminal failed".into(),
            },
            ..TerminalState::default()
        };
        assert!(!terminal_can_reopen(&failed));
        assert!(terminal_can_close(&connected, &failed));
        assert!(!terminal_can_close(&disconnected, &failed));

        let unknown_state = TerminalState {
            session_id: Some("term-b".into()),
            runtime_state: None,
            ..TerminalState::default()
        };
        assert!(!terminal_can_operate(&connected, &unknown_state));
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

    /// GUI3-08：正式 AppView 绘制记录。测试窗不泵显示链路，每个采样都是
    /// 主动 Window::refresh 之后的下一帧；时间用真实 Instant。
    #[gpui::test]
    fn surfaces_motion_draws_intermediate_switch_and_inspector(cx: &mut gpui::TestAppContext) {
        use std::time::{Duration, Instant};

        use gpui::{px, size};

        fn draw_frame(cx: &mut gpui::VisualTestContext) {
            cx.update(|window, _| window.refresh());
        }

        fn inspector_width(view: &gpui::Entity<AppView>, cx: &mut gpui::VisualTestContext) -> f32 {
            cx.update(|_, cx| view.read(cx).inspector_render_width)
        }

        /// 侧栏容器在树上且宽度与正式 render 值一致时用几何；否则只用 render 值。
        /// 窄窗中央 shell-inspector 是整列，不能当成侧栏没归零。
        fn inspector_observation(
            view: &gpui::Entity<AppView>,
            cx: &mut gpui::VisualTestContext,
        ) -> (f32, f32, Option<f32>, &'static str) {
            let render = inspector_width(view, cx);
            let bounds = cx
                .debug_bounds("shell-inspector")
                .map(|bounds| f32::from(bounds.size.width));
            match bounds {
                Some(width) if (width - render).abs() < 1.0 => (width, render, bounds, "bounds"),
                _ => (render, render, bounds, "render"),
            }
        }

        fn thumb_left(cx: &mut gpui::VisualTestContext) -> f32 {
            let track = cx
                .debug_bounds("settings-subagents-enabled")
                .expect("switch track bounds");
            let thumb = cx
                .debug_bounds("settings-subagents-enabled-thumb-dot")
                .expect("switch thumb bounds");
            f32::from(thumb.origin.x) - f32::from(track.origin.x)
        }

        fn sleep_until(start: Instant, millis: u64) {
            if let Some(rest) = Duration::from_millis(millis).checked_sub(start.elapsed()) {
                std::thread::sleep(rest);
            }
        }

        fn save_samples(inspector: &[serde_json::Value], switch_samples: &[serde_json::Value]) {
            let samples = serde_json::json!({
                "kind": "gpui-draw-record",
                "display_link": "not pumped; each sample is an explicit Window::refresh draw",
                "inspector": inspector,
                "switch": switch_samples,
            });
            eprintln!("{samples}");
        }

        let platform = std::sync::Arc::new(Platform::new());
        let socket = std::env::temp_dir().join(format!(
            "gui3-08-surfaces-motion-{}.sock",
            std::process::id()
        ));
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.run_until_parked();
        cx.simulate_resize(size(px(1440.0), px(1024.0)));
        cx.run_until_parked();
        draw_frame(cx);
        let viewport = cx.update(|window, _| f32::from(window.viewport_size().width));
        assert!(
            viewport >= 1288.0,
            "side inspector needs a wide viewport, got {viewport}"
        );

        let mut inspector = Vec::new();
        let mut switch_samples = Vec::new();
        let opened = Instant::now();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.on_toggle_inspector(window, cx));
        });
        let (open_width, open_render, open_bounds, open_measure) = inspector_observation(&view, cx);
        inspector.push(serde_json::json!({
            "label": "open",
            "elapsed_ms": (opened.elapsed().as_secs_f32() * 1000.0 * 10.0).round() / 10.0,
            "value": (open_width * 10.0).round() / 10.0,
            "render_width": (open_render * 10.0).round() / 10.0,
            "bounds_width": open_bounds.map(|width| (width * 10.0).round() / 10.0),
            "measure": open_measure,
        }));
        assert!(
            open_width < 15.0,
            "inspector open frame should start near 0, got {open_width}"
        );

        sleep_until(opened, 45);
        draw_frame(cx);
        let (mid_width, mid_render, mid_bounds, mid_measure) = inspector_observation(&view, cx);
        let mid_elapsed = opened.elapsed().as_secs_f32() * 1000.0;
        inspector.push(serde_json::json!({
            "label": "opening",
            "elapsed_ms": (mid_elapsed * 10.0).round() / 10.0,
            "value": (mid_width * 10.0).round() / 10.0,
            "render_width": (mid_render * 10.0).round() / 10.0,
            "bounds_width": mid_bounds.map(|width| (width * 10.0).round() / 10.0),
            "measure": mid_measure,
        }));
        assert!(
            (80.0..400.0).contains(&mid_width),
            "inspector intermediate width {mid_width} at {mid_elapsed:.1}ms"
        );

        let closing = Instant::now();
        let mut pre_motion = shell_layout::InspectorMotion::default();
        let mut t_pre = closing;
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                pre_motion = view.inspector_motion;
                t_pre = Instant::now();
                view.on_toggle_inspector(window, cx);
            });
        });
        let t_post = Instant::now();
        let (close_width, close_render, close_bounds, close_measure) =
            inspector_observation(&view, cx);
        let before = pre_motion.sample(t_pre);
        let after = pre_motion.sample(t_post);
        let low = before.min(after);
        let high = before.max(after);
        let baseline_ms = t_pre.saturating_duration_since(opened).as_secs_f32() * 1000.0;
        let reverse_ms = closing.elapsed().as_secs_f32() * 1000.0;
        let post_ms = t_post.saturating_duration_since(opened).as_secs_f32() * 1000.0;
        inspector.push(serde_json::json!({
            "label": "previous",
            "elapsed_ms": (mid_elapsed * 10.0).round() / 10.0,
            "value": (mid_width * 10.0).round() / 10.0,
        }));
        inspector.push(serde_json::json!({
            "label": "baseline",
            "elapsed_ms": (baseline_ms * 10.0).round() / 10.0,
            "value": (before * 10.0).round() / 10.0,
        }));
        inspector.push(serde_json::json!({
            "label": "reverse",
            "elapsed_ms": (reverse_ms * 10.0).round() / 10.0,
            "value": (close_width * 10.0).round() / 10.0,
            "render_width": (close_render * 10.0).round() / 10.0,
            "bounds_width": close_bounds.map(|width| (width * 10.0).round() / 10.0),
            "measure": close_measure,
            "sample_pre": (before * 10.0).round() / 10.0,
            "sample_post": (after * 10.0).round() / 10.0,
            "t_post_ms": (post_ms * 10.0).round() / 10.0,
        }));
        save_samples(&inspector, &switch_samples);
        assert!(
            close_width + 0.5 >= low && close_width <= high + 0.5,
            "inspector reverse {close_width} outside [{low}, {high}], previous {mid_width}"
        );

        sleep_until(closing, 60);
        draw_frame(cx);
        let (down_width, down_render, down_bounds, down_measure) = inspector_observation(&view, cx);
        inspector.push(serde_json::json!({
            "label": "closing",
            "elapsed_ms": (closing.elapsed().as_secs_f32() * 1000.0 * 10.0).round() / 10.0,
            "value": (down_width * 10.0).round() / 10.0,
            "render_width": (down_render * 10.0).round() / 10.0,
            "bounds_width": down_bounds.map(|width| (width * 10.0).round() / 10.0),
            "measure": down_measure,
        }));
        assert!(
            down_width + 30.0 < close_width,
            "inspector should be visibly closing: {close_width} -> {down_width}"
        );

        sleep_until(closing, 220);
        draw_frame(cx);
        let (closed_width, closed_render, closed_bounds, closed_measure) =
            inspector_observation(&view, cx);
        inspector.push(serde_json::json!({
            "label": "closed",
            "elapsed_ms": (closing.elapsed().as_secs_f32() * 1000.0 * 10.0).round() / 10.0,
            "value": (closed_width * 10.0).round() / 10.0,
            "render_width": (closed_render * 10.0).round() / 10.0,
            "bounds_width": closed_bounds.map(|width| (width * 10.0).round() / 10.0),
            "measure": closed_measure,
        }));
        assert!(
            closed_width < 2.0,
            "inspector should be closed, got {closed_width}"
        );

        let reopened = Instant::now();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.on_toggle_inspector(window, cx));
        });
        sleep_until(reopened, 40);
        draw_frame(cx);
        let (reopen_width, reopen_render, reopen_bounds, reopen_measure) =
            inspector_observation(&view, cx);
        inspector.push(serde_json::json!({
            "label": "reopen",
            "elapsed_ms": (reopened.elapsed().as_secs_f32() * 1000.0 * 10.0).round() / 10.0,
            "value": (reopen_width * 10.0).round() / 10.0,
            "render_width": (reopen_render * 10.0).round() / 10.0,
            "bounds_width": reopen_bounds.map(|width| (width * 10.0).round() / 10.0),
            "measure": reopen_measure,
        }));
        assert!(
            reopen_width > 30.0 && reopen_width < 420.0,
            "resize should interrupt an in-progress open, got {reopen_width}"
        );
        cx.simulate_resize(size(px(1080.0), px(720.0)));
        cx.run_until_parked();
        draw_frame(cx);
        let (snapped, snapped_render, snapped_bounds, snapped_measure) =
            inspector_observation(&view, cx);
        inspector.push(serde_json::json!({
            "label": "narrow",
            "elapsed_ms": (reopened.elapsed().as_secs_f32() * 1000.0 * 10.0).round() / 10.0,
            "value": (snapped * 10.0).round() / 10.0,
            "render_width": (snapped_render * 10.0).round() / 10.0,
            "bounds_width": snapped_bounds.map(|width| (width * 10.0).round() / 10.0),
            "measure": snapped_measure,
        }));
        save_samples(&inspector, &switch_samples);
        assert_eq!(
            snapped, 0.0,
            "narrow layout must snap the side inspector to 0"
        );

        let shown = Instant::now();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.route = AppRoute::Settings;
                view.handle_controller_event(
                    ControllerEvent::SubagentSettingsLoaded(
                        crate::projection::SubagentSettingsData {
                            enabled: true,
                            ..Default::default()
                        },
                    ),
                    cx,
                );
                view.on_select_settings_page(SettingsPage::Subagents, window, cx);
            });
        });
        let first = thumb_left(cx);
        switch_samples.push(serde_json::json!({
            "label": "first",
            "elapsed_ms": (shown.elapsed().as_secs_f32() * 1000.0 * 10.0).round() / 10.0,
            "value": (first * 10.0).round() / 10.0,
        }));
        assert!(
            (16.0..=20.0).contains(&first),
            "checked thumb should first-paint at rest, got {first}"
        );

        sleep_until(shown, 25);
        draw_frame(cx);
        let held = thumb_left(cx);
        switch_samples.push(serde_json::json!({
            "label": "same",
            "elapsed_ms": (shown.elapsed().as_secs_f32() * 1000.0 * 10.0).round() / 10.0,
            "value": (held * 10.0).round() / 10.0,
        }));
        save_samples(&inspector, &switch_samples);
        assert!(
            (held - first).abs() < 1.5,
            "same-value redraw restarted the thumb: {first} -> {held}"
        );

        let toggled = Instant::now();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.handle_controller_event(
                    ControllerEvent::SubagentSettingsConfirmed(
                        crate::projection::SubagentSettingsData {
                            enabled: false,
                            ..Default::default()
                        },
                    ),
                    cx,
                );
            });
        });
        let mut previous = held;
        for (label, at_ms) in [
            ("off-0", 0u64),
            ("off-40", 40),
            ("off-90", 90),
            ("off-140", 140),
        ] {
            if at_ms > 0 {
                sleep_until(toggled, at_ms);
                draw_frame(cx);
            }
            let x = thumb_left(cx);
            let elapsed = toggled.elapsed().as_secs_f32() * 1000.0;
            switch_samples.push(serde_json::json!({
                "label": label,
                "elapsed_ms": (elapsed * 10.0).round() / 10.0,
                "value": (x * 10.0).round() / 10.0,
            }));
            match label {
                "off-0" => assert!(
                    (x - previous).abs() < 1.5,
                    "switch reverse jumped from {previous} to {x}"
                ),
                "off-40" => {
                    assert!(
                        x < previous - 1.0 && x > 2.5,
                        "{label} should be between the reverse start and the rest position, previous {previous} now {x} at {elapsed:.1}ms"
                    );
                    previous = x;
                }
                "off-90" => {
                    assert!(
                        x < previous && x > 1.0,
                        "{label} should keep easing toward rest, previous {previous} now {x} at {elapsed:.1}ms"
                    );
                    previous = x;
                }
                "off-140" => assert!(
                    (x - 2.0).abs() < 1.5,
                    "unchecked thumb should rest near 2, got {x} at {elapsed:.1}ms"
                ),
                _ => unreachable!(),
            }
        }
        save_samples(&inspector, &switch_samples);

        // 静态骨架与流式 caret 只读布局框。断线时 refresh_* 会 mark_stale，
        // 所以这里直接 begin_refresh / 写入时间线，不向 Host 发请求。
        // 调用方骨架 id 作前缀。debug_bounds 只收 &'static，不能用临时 format!。
        const CHANGES_BARS: [&str; 4] = [
            "changes.loading-skeleton/skeleton-bar-0",
            "changes.loading-skeleton/skeleton-bar-1",
            "changes.loading-skeleton/skeleton-bar-2",
            "changes.loading-skeleton/skeleton-bar-3",
        ];
        const RESOURCES_BARS: [&str; 4] = [
            "resources.loading-skeleton/skeleton-bar-0",
            "resources.loading-skeleton/skeleton-bar-1",
            "resources.loading-skeleton/skeleton-bar-2",
            "resources.loading-skeleton/skeleton-bar-3",
        ];
        fn bar_bounds(
            cx: &mut gpui::VisualTestContext,
            selectors: [&'static str; 4],
        ) -> Option<[(f32, f32); 4]> {
            let mut sizes = [(0.0, 0.0); 4];
            for (size, selector) in sizes.iter_mut().zip(selectors) {
                let Some(bounds) = cx.debug_bounds(selector) else {
                    return None;
                };
                *size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            }
            Some(sizes)
        }
        fn selectors_absent(
            cx: &mut gpui::VisualTestContext,
            selectors: [&'static str; 4],
        ) -> bool {
            selectors
                .into_iter()
                .all(|selector| cx.debug_bounds(selector).is_none())
        }
        fn assert_skeleton(sizes: [(f32, f32); 4]) {
            const WIDTHS: [f32; 4] = [220.0, 168.0, 196.0, 132.0];
            for (index, (width, height)) in sizes.into_iter().enumerate() {
                assert!(
                    (width - WIDTHS[index]).abs() < 0.5,
                    "bar {index} width {width}"
                );
                assert!(
                    (height - theme::metrics::SKELETON_BAR_HEIGHT).abs() < 0.5,
                    "bar {index} height {height}"
                );
            }
        }
        fn round1(value: f32) -> f32 {
            (value * 10.0).round() / 10.0
        }
        fn size_json(sizes: [(f32, f32); 4]) -> serde_json::Value {
            serde_json::Value::Array(
                sizes
                    .into_iter()
                    .map(|(width, height)| serde_json::json!([round1(width), round1(height)]))
                    .collect(),
            )
        }

        cx.simulate_resize(size(px(1440.0), px(1024.0)));
        cx.run_until_parked();
        let changes_epoch = cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.route = AppRoute::Workspace;
                if view.inspector_open {
                    view.on_toggle_inspector(window, cx);
                }
                view.on_toggle_inspector(window, cx);
                view.inspector_motion.snap(theme::metrics::INSPECTOR_WIDTH);
                view.inspector_tab = InspectorTab::Changes;
                view.changes.tab = changes::ChangesTab::Files;
                assert!(view.changes.files.is_empty());
                assert!(!matches!(
                    view.projection.connection,
                    ConnectionState::Connected { .. }
                ));
                let epoch = view.changes.begin_refresh();
                assert!(matches!(
                    view.changes.fetch,
                    changes::ChangesFetch::Fetching
                ));
                assert!(view.changes.stale_reason.is_none());
                cx.notify();
                epoch
            })
        });
        draw_frame(cx);
        let side = cx
            .debug_bounds("shell-inspector")
            .expect("snapped side inspector");
        assert!(
            (f32::from(side.size.width) - theme::metrics::INSPECTOR_WIDTH).abs() < 1.0,
            "sidebar width {}",
            f32::from(side.size.width)
        );
        let changes_bars = bar_bounds(cx, CHANGES_BARS).expect("changes loading skeleton");
        assert_skeleton(changes_bars);
        let held_at = Instant::now();
        sleep_until(held_at, 40);
        draw_frame(cx);
        let changes_held = bar_bounds(cx, CHANGES_BARS).expect("skeleton after redraw");
        for (index, (before, after)) in changes_bars.into_iter().zip(changes_held).enumerate() {
            assert!(
                (before.0 - after.0).abs() < 0.5 && (before.1 - after.1).abs() < 0.5,
                "static skeleton bar {index} moved {before:?} -> {after:?}"
            );
        }
        assert!(
            selectors_absent(cx, RESOURCES_BARS),
            "resources skeleton selector cached before that panel draws"
        );

        let resources_epoch = cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.inspector_tab = InspectorTab::Resources;
                assert!(view.resources.servers.is_empty());
                let epoch = view.resources.begin_refresh();
                assert!(matches!(
                    view.resources.fetch,
                    resources::ResourcesFetch::Fetching
                ));
                assert!(view.resources.stale_reason.is_none());
                cx.notify();
                epoch
            })
        });
        draw_frame(cx);
        let resources_bars = bar_bounds(cx, RESOURCES_BARS).expect("resources loading skeleton");
        assert_skeleton(resources_bars);

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(view
                    .changes
                    .apply_files(changes_epoch, None, Vec::new(), None));
                assert!(view.resources.apply_servers(resources_epoch, Vec::new()));
                // 骨架只挂在 changes.loading / changes.diff_loading / resources.loading。
                // Ready 且列表为空走空占位；diff Idle 不走 diff_loading。
                // GPUI 0.2.2 的 debug_bounds 在 clear 后仍残留，不用于消失结论。
                assert!(
                    matches!(view.changes.fetch, changes::ChangesFetch::Ready)
                        && view.changes.files.is_empty()
                        && matches!(view.changes.diff, changes::DiffFetch::Idle),
                    "changes empty ready is the empty branch, not Fetching"
                );
                assert!(
                    matches!(view.resources.fetch, resources::ResourcesFetch::Ready)
                        && view.resources.servers.is_empty(),
                    "resources empty ready is the empty branch, not Fetching"
                );
                cx.notify();
            });
        });

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_run_id = Some("run-static".into());
                view.projection.timeline.entries = vec![
                    crate::projection::TimelineEntry {
                        sequence: 1,
                        event_id: "a-static".into(),
                        kind: crate::projection::TimelineEntryKind::AssistantMessage {
                            text: "hello".into(),
                        },
                        fork_boundary: None,
                        timestamp: "1".into(),
                        run_id: Some("run-static".into()),
                    },
                    crate::projection::TimelineEntry {
                        sequence: 2,
                        event_id: "r-static".into(),
                        kind: crate::projection::TimelineEntryKind::RunState(
                            "run streaming_response".into(),
                        ),
                        fork_boundary: None,
                        timestamp: "2".into(),
                        run_id: Some("run-static".into()),
                    },
                ];
                view.timeline_changed();
                cx.notify();
            });
        });
        draw_frame(cx);
        cx.refresh().unwrap();
        cx.run_until_parked();
        let caret = cx.debug_bounds("streaming-caret").expect("streaming caret");
        let caret_width = f32::from(caret.size.width);
        let caret_height = f32::from(caret.size.height);
        assert!(
            (caret_width - theme::metrics::STREAM_CARET_WIDTH).abs() < 0.5,
            "caret width {caret_width}"
        );
        assert!(
            (caret_height - theme::metrics::STREAM_CARET_HEIGHT).abs() < 0.5,
            "caret height {caret_height}"
        );

        let record = serde_json::json!({
            "kind": "gpui-layout-bounds",
            "host_request": false,
            "pixels": "none",
            "display_link": "not pumped; explicit Window::refresh; not a real window",
            "geometry": {
                "skeleton_widths": [220.0, 168.0, 196.0, 132.0],
                "skeleton_height": theme::metrics::SKELETON_BAR_HEIGHT,
                "caret": [theme::metrics::STREAM_CARET_WIDTH, theme::metrics::STREAM_CARET_HEIGHT],
            },
            "layout_bounds": {
                "changes": {
                    "source": "changes.loading-skeleton",
                    "selectors": CHANGES_BARS,
                    "bounds": size_json(changes_bars),
                },
                "changes_redraw": {
                    "source": "changes.loading-skeleton",
                    "selectors": CHANGES_BARS,
                    "bounds": size_json(changes_held),
                },
                "resources": {
                    "source": "resources.loading-skeleton",
                    "selectors": RESOURCES_BARS,
                    "bounds": size_json(resources_bars),
                },
                "caret": [round1(caret_width), round1(caret_height)],
            },
            "terminal": "终态状态和源码挂载门控确认；旧 debug_bounds 不用于消失结论",
        });
        eprintln!("{record}");

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_run_id = None;
                view.projection.timeline.entries.pop();
                view.timeline_changed();
                let entries = &view.projection.timeline.entries;
                let assistant = entries.first().expect("assistant message remains");
                // caret 只在 assistant_is_streaming 为真的最后一行挂上。
                assert!(view.projection.active_run_id.is_none());
                assert!(
                    !super::timeline_entry::assistant_is_streaming(
                        entries,
                        assistant,
                        view.projection.active_run_id.as_deref(),
                    ),
                    "caret mounts only while assistant_is_streaming"
                );
                cx.notify();
            });
        });
    }
}
