//! 浏览器面板的 GPUI 胶水；原生网页、导航与生命周期在 pawork-browser。

use std::{
    collections::HashMap,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use gpui::{
    canvas, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, ScrollHandle, Window,
};
use pawork_browser::{BrowserState, BrowserView};
use serde_json::json;

use super::{
    accessibility::{AxAction, AxNode, AxRect, AxRole},
    components::button::{Button, ButtonPadding, ButtonVariant},
    i18n::t,
    icon, icon_sized,
    theme::{dark, font},
    AppRoute, AppView, Icon, InspectorTab, TextInput,
};

/// CU-14：一张标签 = 一个系统 WebView + 归属标记。agent_owned 区分
/// Agent 创建的标签与用户已有标签；清理（close / close_tab）只作用于
/// Agent 拥有的标签，用户标签只能由用户自己关闭。
pub(super) struct BrowserTab {
    pub id: String,
    pub agent_owned: bool,
    pub native: Rc<BrowserView>,
}

/// 标签条一行的展示数据（标题 / URL / 占位），由 250ms 轮询刷新缓存。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TabLabel {
    pub id: String,
    pub label: String,
    pub agent_owned: bool,
}

/// 标签归属/选择决策的纯数据投影：原生视图在测试中不可构造，归属决策
///（关哪张、关闭后选中谁）抽成纯函数，由单测钉住。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TabMeta {
    pub id: String,
    pub agent_owned: bool,
}

/// 一组关闭的决策结果：removed 为移除顺序；active / agent 为关闭后的
/// 新面板选中与新 Agent 绑定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClosePlan {
    pub removed: Vec<String>,
    pub active: Option<String>,
    pub agent: Option<String>,
}

/// 单面板标签数上限：每标签一个原生 WKWebView，防失控 Run 耗尽内存。
pub(super) const MAX_BROWSER_TABS: usize = 16;

static NEXT_TAB: AtomicU64 = AtomicU64::new(1);

/// 进程级计数器 + pid 命名空间。标签 id 只在内存存活、不出本进程，
/// 但同进程多 AppView（测试）也不应撞 id。
fn new_tab_id() -> String {
    format!(
        "tab-{}-{}",
        std::process::id(),
        NEXT_TAB.fetch_add(1, Ordering::Relaxed)
    )
}

/// 关闭后的选中/绑定修正：被选中被关时落到原位置下一张（末尾取前一
/// 张）；Agent 绑定随其标签移除而失效。
fn close_plan_for(
    tabs: &[TabMeta],
    active: Option<&str>,
    agent: Option<&str>,
    removed: Vec<String>,
) -> ClosePlan {
    let kept: Vec<&TabMeta> = tabs
        .iter()
        .filter(|tab| !removed.contains(&tab.id))
        .collect();
    let next_active = match active {
        Some(current) if removed.iter().any(|id| id == current) => {
            let index = tabs.iter().position(|tab| tab.id == current).unwrap_or(0);
            if kept.is_empty() {
                None
            } else {
                Some(kept[index.min(kept.len() - 1)].id.clone())
            }
        }
        other => other
            .map(str::to_string)
            .filter(|id| kept.iter().any(|tab| &tab.id == id)),
    };
    let next_agent = match agent {
        Some(current) if removed.iter().any(|id| id == current) => None,
        other => other
            .map(str::to_string)
            .filter(|id| kept.iter().any(|tab| &tab.id == id)),
    };
    ClosePlan {
        removed,
        active: next_active,
        agent: next_agent,
    }
}

/// Agent 关单张（close_tab）：只允许本任务 Agent 创建的标签；用户
/// 标签明确拒绝，未知标签同样拒绝。
fn plan_agent_close(
    tabs: &[TabMeta],
    active: Option<&str>,
    agent: Option<&str>,
    target: &str,
) -> Result<ClosePlan, String> {
    let Some(tab) = tabs.iter().find(|tab| tab.id == target) else {
        return Err(format!("Unknown tab: {target}"));
    };
    if !tab.agent_owned {
        return Err(
            "Tab was opened by the user; the agent cannot close it / 标签由用户创建，Agent 不能关闭"
                .into(),
        );
    }
    Ok(close_plan_for(
        tabs,
        active,
        agent,
        vec![target.to_string()],
    ))
}

/// Agent 整体清理（close）：只移除本任务拥有的标签，用户标签保留。
fn plan_agent_close_all(tabs: &[TabMeta], active: Option<&str>, agent: Option<&str>) -> ClosePlan {
    let removed: Vec<String> = tabs
        .iter()
        .filter(|tab| tab.agent_owned)
        .map(|tab| tab.id.clone())
        .collect();
    close_plan_for(tabs, active, agent, removed)
}

/// 用户关标签：面板归用户所有，任意标签可关；Agent 绑定若指向被关
/// 标签随移除失效（Agent 下一次动作会得到真实的「无页面」错误）。
fn plan_user_close(
    tabs: &[TabMeta],
    active: Option<&str>,
    agent: Option<&str>,
    target: &str,
) -> Option<ClosePlan> {
    tabs.iter()
        .any(|tab| tab.id == target)
        .then(|| close_plan_for(tabs, active, agent, vec![target.to_string()]))
}

pub(super) struct BrowserPanel {
    pub open: bool,
    pub input: Entity<TextInput>,
    pub focus: FocusHandle,
    /// CU-14：每任务多标签。active 是面板当前展示的标签（用户视角），
    /// agent 是 Agent 动作（navigate/read/click/…）的派发目标——两者
    /// 分离：用户切换展示不劫持 Agent 派发，Agent 后台操作不打断用户
    /// 正在看的标签。
    pub tabs: Vec<BrowserTab>,
    pub active: Option<String>,
    pub agent: Option<String>,
    pub state: BrowserState,
    /// 地址栏同步的已同步标签锚点：sync_browser_state 据此识别「切换
    /// 标签」这一事件（含切到空白标签），与普通轮询区分——轮询不得
    /// 覆盖用户正在编辑的地址输入。
    synced_tab: Option<String>,
    pub labels: Vec<TabLabel>,
    error: Option<String>,
    layouts: HashMap<&'static str, ScrollHandle>,
    controls: HashMap<&'static str, FocusHandle>,
    tab_layouts: HashMap<String, ScrollHandle>,
    tab_focus: HashMap<String, FocusHandle>,
    poll: Option<gpui::Task<()>>,
}

impl BrowserPanel {
    pub fn new(cx: &mut Context<AppView>) -> Self {
        let input = cx.new(|cx| {
            TextInput::with_placeholder(t("browser.placeholder"), cx)
                .id("browser-address")
                .height_clamp(32.0, 32.0)
        });
        let focus = input.read(cx).focus_handle(cx).tab_stop(true);
        Self {
            open: false,
            input,
            focus,
            tabs: Vec::new(),
            active: None,
            agent: None,
            state: BrowserState::default(),
            synced_tab: None,
            labels: Vec::new(),
            error: None,
            layouts: [
                "browser-back",
                "browser-forward",
                "browser-reload",
                "browser-address",
                "browser-go",
                "browser-tabs",
                "browser-new-tab",
                "browser-page",
                "browser-status",
            ]
            .into_iter()
            .map(|id| (id, ScrollHandle::new()))
            .collect(),
            controls: [
                "browser-back",
                "browser-forward",
                "browser-reload",
                "browser-go",
                "browser-new-tab",
            ]
            .into_iter()
            .map(|id| (id, cx.focus_handle().tab_stop(true)))
            .collect(),
            tab_layouts: HashMap::new(),
            tab_focus: HashMap::new(),
            poll: None,
        }
    }

    pub fn close(&mut self, cx: &mut Context<AppView>) {
        self.open = false;
        // 显式隐藏再释放：派发在途时 Rc 仍有克隆，Drop 延迟到派发结束。
        for tab in self.tabs.drain(..) {
            tab.native.set_visible(false);
        }
        self.active = None;
        self.agent = None;
        self.labels.clear();
        self.tab_layouts.clear();
        self.tab_focus.clear();
        self.poll = None;
        self.state = BrowserState::default();
        self.error = None;
        self.input.update(cx, |input, cx| input.set_text("", cx));
    }

    pub fn active_native(&self) -> Option<&Rc<BrowserView>> {
        let id = self.active.as_deref()?;
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| &tab.native)
    }

    pub fn agent_native(&self) -> Option<&Rc<BrowserView>> {
        let id = self.agent.as_deref()?;
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| &tab.native)
    }

    fn tab_metas(&self) -> Vec<TabMeta> {
        self.tabs
            .iter()
            .map(|tab| TabMeta {
                id: tab.id.clone(),
                agent_owned: tab.agent_owned,
            })
            .collect()
    }

    /// 创建标签。用户标签（「+」/ 地址栏）切换展示并隐藏先前选中标签
    ///（新建视图本就以 hidden 创建）；Agent 标签只绑定派发目标，不动
    /// 用户正在看的标签——set_visible(false) 对聚焦网页会 focus_parent，
    /// Agent 切换展示等于打断用户输入（CU-14 复审①）。
    fn push_tab(&mut self, native: Rc<BrowserView>, agent_owned: bool) -> Result<String, String> {
        if self.tabs.len() >= MAX_BROWSER_TABS {
            return Err(format!(
                "标签数已达上限 {MAX_BROWSER_TABS} / Tab limit reached"
            ));
        }
        let id = new_tab_id();
        self.tabs.push(BrowserTab {
            id: id.clone(),
            agent_owned,
            native,
        });
        if agent_owned {
            self.agent = Some(id.clone());
        } else {
            if let Some(previous) = self.active_native() {
                previous.set_visible(false);
            }
            self.active = Some(id.clone());
            self.state = BrowserState::default();
        }
        Ok(id)
    }

    /// 执行关闭决策：释放原生视图、修剪标签条布局/焦点缓存、按决策
    /// 修正选中与 Agent 绑定。标签清零时停掉轮询。
    fn apply_close_plan(&mut self, plan: ClosePlan) {
        for id in &plan.removed {
            if let Some(index) = self.tabs.iter().position(|tab| &tab.id == id) {
                let tab = self.tabs.remove(index);
                tab.native.set_visible(false);
            }
            self.tab_layouts.remove(&format!("browser-tab-select-{id}"));
            self.tab_layouts.remove(&format!("browser-tab-close-{id}"));
            self.tab_focus.remove(&format!("browser-tab-select-{id}"));
            self.tab_focus.remove(&format!("browser-tab-close-{id}"));
        }
        self.labels
            .retain(|label| !plan.removed.contains(&label.id));
        // 展示随关闭决策换标签时，上一标签的错误随之失效（CU-14
        // 复审②）；地址栏由 sync_browser_state 的 synced_tab 锚点跟进。
        if self.active != plan.active {
            self.error = None;
        }
        self.active = plan.active;
        self.agent = plan.agent;
        if self.tabs.is_empty() {
            self.poll = None;
            self.error = None;
        }
    }

    /// Agent 选中标签（select_tab）：只改 Agent 派发目标，不动面板
    /// 展示——展示切换只能由用户触发。set_visible(false) 对聚焦网页
    /// 会 focus_parent，Agent 切展示等于打断用户输入（CU-14 复审①）。
    fn select_agent_tab(&mut self, id: &str) -> Result<(), String> {
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return Err(format!("Unknown tab: {id}"));
        }
        self.agent = Some(id.to_string());
        Ok(())
    }

    /// 用户在标签条选中：只改面板展示，不动 Agent 派发目标。
    fn user_select(&mut self, id: &str) {
        if self.active.as_deref() == Some(id) || !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        if let Some(previous) = self.active_native() {
            previous.set_visible(false);
        }
        self.active = Some(id.to_string());
        // 错误归属旧标签（CU-14 复审②）；输入框由 sync_browser_state
        // 的 synced_tab 锚点换成新标签地址（含空地址）。
        self.error = None;
    }

    /// Host tabs 动作的回执：逐标签 url/title/归属/页面代际。
    fn tabs_json(&self) -> serde_json::Value {
        let tabs: Vec<serde_json::Value> = self
            .tabs
            .iter()
            .map(|tab| {
                let state = tab.native.state();
                json!({
                    "tab_id": tab.id,
                    "url": state.url,
                    "title": state.title,
                    "loading": state.loading,
                    "active": self.active.as_deref() == Some(tab.id.as_str()),
                    "agent_owned": tab.agent_owned,
                    "generation": tab.native.generation(),
                })
            })
            .collect();
        json!({
            "tabs": tabs,
            "active_tab_id": self.active,
            "agent_tab_id": self.agent,
        })
    }

    fn tab_json(&self, id: &str) -> Option<serde_json::Value> {
        let tab = self.tabs.iter().find(|tab| tab.id == id)?;
        let state = tab.native.state();
        Some(json!({
            "tab_id": tab.id,
            "url": state.url,
            "title": state.title,
            "loading": state.loading,
            "agent_owned": tab.agent_owned,
            "generation": tab.native.generation(),
        }))
    }

    fn actions(&self) -> [(&'static str, &'static str, Icon, bool); 4] {
        [
            (
                "browser-back",
                "browser.back",
                Icon::ChevronLeft,
                self.state.can_go_back,
            ),
            (
                "browser-forward",
                "browser.forward",
                Icon::ChevronRight,
                self.state.can_go_forward,
            ),
            (
                "browser-reload",
                if self.state.loading {
                    "browser.stop"
                } else {
                    "browser.reload"
                },
                if self.state.loading {
                    Icon::Cancel
                } else {
                    Icon::Refresh
                },
                self.active_native().is_some(),
            ),
            ("browser-go", "browser.go", Icon::ChevronRight, true),
        ]
    }

    fn status(&self) -> String {
        self.error
            .as_ref()
            .or(self.state.error.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                if self.state.loading {
                    t("browser.loading").into()
                } else {
                    self.state.title.clone()
                }
            })
    }
}

impl AppView {
    pub(super) fn browser_pointer_focus(&self) -> impl IntoElement {
        let native = self.browser.active_native().cloned();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_mouse_event(move |_: &gpui::MouseDownEvent, phase, _, _| {
                    // WebKit 内的点击由 AppKit 分发；这里仅接到 GPUI 壳上的点击。
                    if phase == gpui::DispatchPhase::Capture {
                        if let Some(native) = &native {
                            if native.is_focused() {
                                native.focus_parent();
                            }
                        }
                    }
                });
            },
        )
        .absolute()
    }

    pub(super) fn focus_browser_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(native) = self.browser.active_native() {
            native.focus_parent();
        }
        window.focus(&self.browser.focus);
        window.dispatch_action(Box::new(super::text_input::SelectAll), cx);
        cx.notify();
    }

    pub(super) fn sync_browser_session(&mut self, cx: &mut Context<Self>) {
        let session = self.projection.active_session_id.clone();
        if self.browser_session == session {
            return;
        }
        for tab in &self.browser.tabs {
            tab.native.set_visible(false);
        }
        self.browser.poll = None;
        let next = self
            .browser_sessions
            .remove(&session)
            .unwrap_or_else(|| BrowserPanel::new(cx));
        let previous = std::mem::replace(&mut self.browser, next);
        if previous.open {
            self.browser_sessions
                .insert(self.browser_session.take(), previous);
        }
        self.browser_session = session;
        self.inspector_open_tabs
            .retain(|tool| *tool != InspectorTab::Browser);
        if self.browser.open {
            self.remember_inspector_tab(InspectorTab::Browser);
        } else if self.inspector_tab == InspectorTab::Browser {
            self.inspector_tab = InspectorTab::Home;
        }
    }

    /// CU-13：轮询改在 app 级后台任务上运行（不依赖 window），隐藏窗口
    /// display link 停转后浏览器状态同步与 Host 请求派发仍然工作。
    pub(super) fn ensure_browser_poll(&mut self, cx: &mut Context<Self>) {
        if !self.browser.tabs.is_empty() && self.browser.poll.is_none() {
            self.browser.poll = Some(cx.spawn(async move |this, cx| loop {
                smol::Timer::after(Duration::from_millis(250)).await;
                if this
                    .update(cx, |view, cx| view.sync_browser_state(cx))
                    .is_err()
                {
                    break;
                }
            }));
        }
    }

    pub(super) fn browser_visible(&self) -> bool {
        self.route == AppRoute::Workspace
            && self.inspector_open
            && self.inspector_tab == InspectorTab::Browser
            && self.open_menu.is_none()
            && !self.quick_search.open
    }

    pub(super) fn browser_action(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.route != AppRoute::Workspace
            || !self.inspector_open
            || self.inspector_tab != InspectorTab::Browser
        {
            return;
        }
        // CU-14 标签条动作是用户动作：新建 / 选中 / 关闭可作用于任意
        // 标签（面板归用户）；Agent 的 close_tab 走 Host 派发，另有归属闸。
        if action == "browser-new-tab" {
            match self.create_native_browser() {
                Ok(native) => match self.browser.push_tab(Rc::new(native), false) {
                    Ok(_) => {
                        self.ensure_browser_poll(cx);
                        self.browser.error = None;
                        self.focus_browser_address(window, cx);
                    }
                    Err(error) => self.browser.error = Some(error),
                },
                Err(error) => self.browser.error = Some(error),
            }
            self.sync_browser_state(cx);
            cx.notify();
            return;
        }
        if let Some(id) = action.strip_prefix("browser-tab-close-") {
            let metas = self.browser.tab_metas();
            if let Some(plan) = plan_user_close(
                &metas,
                self.browser.active.as_deref(),
                self.browser.agent.as_deref(),
                id,
            ) {
                self.browser.apply_close_plan(plan);
            }
            self.sync_browser_state(cx);
            cx.notify();
            return;
        }
        if let Some(id) = action.strip_prefix("browser-tab-select-") {
            self.browser.user_select(id);
            self.sync_browser_state(cx);
            cx.notify();
            return;
        }
        if action == "browser-go" {
            let url = match pawork_browser::normalize_url(self.browser.input.read(cx).text()) {
                Ok(url) => url,
                Err(error) => {
                    self.browser.error = Some(error);
                    cx.notify();
                    return;
                }
            };
            // 地址栏导航作用于面板选中标签；无标签时创建的用户标签。
            if self.browser.active_native().is_none() {
                match self.create_native_browser() {
                    Ok(native) => {
                        if let Err(error) = self.browser.push_tab(Rc::new(native), false) {
                            self.browser.error = Some(error);
                            cx.notify();
                            return;
                        }
                    }
                    Err(error) => {
                        self.browser.error = Some(error);
                        cx.notify();
                        return;
                    }
                }
                self.ensure_browser_poll(cx);
            }
            self.browser.error = self
                .browser
                .active_native()
                .and_then(|native| native.navigate(&url).err());
            if self.browser.error.is_none() {
                self.browser
                    .input
                    .update(cx, |input, cx| input.set_text(url, cx));
                // 输入框已是目标地址：钉住 synced_tab 锚点，避免随后
                // 的 sync 把「新建标签」当切换事件、以导航在途的空地址
                // 清空刚输入的 URL（CU-14 复审②）。
                self.browser.synced_tab = self.browser.active.clone();
            }
        } else if let Some(native) = self.browser.active_native() {
            match action {
                "browser-back" if self.browser.state.can_go_back => native.back(),
                "browser-forward" if self.browser.state.can_go_forward => native.forward(),
                "browser-reload" if self.browser.state.loading => native.stop(),
                "browser-reload" => native.reload(),
                _ => return,
            }
            self.browser.error = None;
        }
        self.sync_browser_state(cx);
        cx.notify();
    }

    /// 由渲染入口（interactive 导航）与后台派发共用的 WebView 创建：
    /// 句柄来自 render 期缓存的原始窗口句柄，不再需要 Window 参数。
    fn create_native_browser(&self) -> Result<BrowserView, String> {
        let raw = self
            .raw_window_handle
            .ok_or_else(|| "Window handle unavailable".to_string())?;
        // SAFETY: 单窗口应用；raw 句柄在 render 里每帧从活窗口刷新，
        // 本调用只读句柄创建子视图，不持有窗口所有权。
        let handle = unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) };
        BrowserView::new(handle)
    }

    fn sync_browser_state(&mut self, cx: &mut Context<Self>) {
        // CU-14：逐标签读真实原生状态；面板状态跟随选中标签，标签条
        // 文案缓存（标题 / URL / 占位）同一趟刷新，变化才重绘。
        if self.browser.tabs.is_empty() {
            if self.browser.state != BrowserState::default() {
                self.browser.state = BrowserState::default();
                cx.notify();
            }
            if !self.browser.labels.is_empty() {
                self.browser.labels.clear();
                cx.notify();
            }
            // CU-14 复审②：标签清零时地址栏与错误一并清空——空 URL
            // 不走下方 url_changed 分支，不清会残留上一标签的地址。
            if self.browser.error.take().is_some() {
                cx.notify();
            }
            if !self.browser.input.read(cx).text().is_empty() {
                self.browser
                    .input
                    .update(cx, |input, cx| input.set_text("", cx));
            }
            self.browser.synced_tab = None;
            return;
        }
        let states: Vec<(String, BrowserState, bool)> = self
            .browser
            .tabs
            .iter()
            .map(|tab| (tab.id.clone(), tab.native.state(), tab.agent_owned))
            .collect();
        let state = self
            .browser
            .active
            .as_deref()
            .and_then(|active| {
                states
                    .iter()
                    .find(|(id, _, _)| id == active)
                    .map(|(_, state, _)| state.clone())
            })
            .unwrap_or_default();
        let labels: Vec<TabLabel> = states
            .into_iter()
            .map(|(id, state, agent_owned)| TabLabel {
                id,
                agent_owned,
                label: if !state.title.is_empty() {
                    state.title
                } else if !state.url.is_empty() {
                    state.url
                } else {
                    t("browser.untitled").into()
                },
            })
            .collect();
        if labels != self.browser.labels {
            self.browser.labels = labels;
            cx.notify();
        }
        let url_changed = self.browser.state.url != state.url;
        if self.browser.state != state {
            self.browser.state = state;
            cx.notify();
        }
        // CU-14 复审②：synced_tab 锚点识别「切换标签」这一事件（含
        // 切到空白标签）——切标签时允许空地址覆盖输入框；普通轮询
        // 只在地址变化时同步，不动用户正在编辑的输入。错误归属由
        // 切换动作本身清理（user_select / apply_close_plan），这里
        // 不清——地址栏导航失败的错误与输入必须原样保留。
        let tab_changed = self.browser.synced_tab.as_deref() != self.browser.active.as_deref();
        if (url_changed || tab_changed)
            && self.browser.error.is_none()
            && self.browser.input.read(cx).text() != self.browser.state.url
        {
            let url = self.browser.state.url.clone();
            self.browser
                .input
                .update(cx, |input, cx| input.set_text(url, cx));
        }
        self.browser.synced_tab = self.browser.active.clone();
    }

    pub(super) fn browser_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.browser.input.update(cx, |input, cx| {
            input.set_placeholder(t("browser.placeholder"), cx)
        });
        let mut toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .p_2()
            .flex_none()
            .w_full();
        for (id, label, glyph, enabled) in self.browser.actions() {
            if id == "browser-go" {
                toolbar = toolbar.child(
                    div()
                        .id("browser-address-layout")
                        .flex_1()
                        .min_w_0()
                        .track_scroll(&self.browser.layouts["browser-address"])
                        .child(self.browser.input.clone()),
                );
            }
            toolbar = toolbar.child(
                div()
                    .id(gpui::SharedString::from(format!("{id}-layout")))
                    .flex_none()
                    .track_scroll(&self.browser.layouts[id])
                    .child(
                        Button::new(id)
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::None)
                            .width(px(28.0))
                            .height(px(32.0))
                            .center()
                            .child(icon(glyph))
                            .tooltip(t(label))
                            .track_focus(&self.browser.controls[id])
                            .disabled(!enabled)
                            .on_click(cx.listener(move |view, event, window, cx| {
                                if !view.consume_button_key_click(id, event) {
                                    view.browser_action(id, window, cx);
                                }
                            }))
                            .on_activate(cx.listener(move |view, _, window, cx| {
                                if view.open_menu.is_none() {
                                    view.note_button_key_activate(id);
                                    view.browser_action(id, window, cx);
                                    cx.stop_propagation();
                                }
                            })),
                    ),
            );
        }
        let native = self.browser.active_native().cloned();
        let visible = self.browser_visible();
        let empty = native.is_none();
        let status = self.browser.status();
        // CU-14：有标签时显示标签条；每条 = 选中按钮（截断标题，Agent
        // 标签带色点）+ 独立关闭按钮；末尾「+」新建用户标签。
        let strip = if self.browser.labels.is_empty() {
            None
        } else {
            Some(self.browser_tab_strip(cx))
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(toolbar)
            .when_some(strip, |body, strip| body.child(strip))
            .child(
                div()
                    .id("browser-page-layout")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .track_scroll(&self.browser.layouts["browser-page"])
                    .when(empty, |body| {
                        body.flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .p_4()
                            .text_size(font::SM)
                            .text_color(dark().text.secondary)
                            .child(icon(Icon::Network))
                            .child(t("browser.empty"))
                    })
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                // GPUI 的父级裁剪不会自动约束 AppKit 子视图。
                                if let Some(native) = &native {
                                    let clipped = bounds.intersect(&window.content_mask().bounds);
                                    native.set_bounds(
                                        f32::from(clipped.origin.x) as f64,
                                        f32::from(clipped.origin.y) as f64,
                                        f32::from(clipped.size.width).max(0.0) as f64,
                                        f32::from(clipped.size.height).max(0.0) as f64,
                                    );
                                    native.set_visible(
                                        visible
                                            && clipped.size.width > px(0.0)
                                            && clipped.size.height > px(0.0),
                                    );
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .when(!status.is_empty(), |body| {
                body.child(
                    div()
                        .id("browser-status-layout")
                        .track_scroll(&self.browser.layouts["browser-status"])
                        .flex_none()
                        .px_2()
                        .py_1()
                        .text_size(font::SM)
                        .text_color(
                            if self.browser.error.is_some() || self.browser.state.error.is_some() {
                                dark().semantic.warning_text
                            } else {
                                dark().text.secondary
                            },
                        )
                        .child(status),
                )
            })
    }

    /// CU-14 标签条：每标签 = 选中按钮（截断标题，Agent 创建的标签带
    /// 色点标记）+ 独立关闭按钮，末尾「+」新建用户标签。鼠标点击、AX
    /// Press 与键盘激活都汇入 browser_action。
    fn browser_tab_strip(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut strip = div()
            .id("browser-tabs")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .pb_1()
            .flex_none()
            .w_full()
            .overflow_x_scroll()
            .track_scroll(&self.browser.layouts["browser-tabs"]);
        for label in self.browser.labels.clone() {
            let select_id = format!("browser-tab-select-{}", label.id);
            let close_id = format!("browser-tab-close-{}", label.id);
            let selected = self.browser.active.as_deref() == Some(label.id.as_str());
            let layout = self
                .browser
                .tab_layouts
                .entry(select_id.clone())
                .or_default()
                .clone();
            let close_layout = self
                .browser
                .tab_layouts
                .entry(close_id.clone())
                .or_default()
                .clone();
            let focus = self
                .browser
                .tab_focus
                .entry(select_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let close_focus = self
                .browser
                .tab_focus
                .entry(close_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let shown = if label.agent_owned {
                format!("{} · {}", label.label, t("browser.tab_agent"))
            } else {
                label.label.clone()
            };
            let click_select = select_id.clone();
            let key_select = select_id.clone();
            let click_close = close_id.clone();
            let key_close = close_id.clone();
            strip = strip.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_none()
                    .h(px(28.0))
                    .rounded(px(6.0))
                    .when(selected, |row| row.bg(dark().surface.raised))
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!("{select_id}-layout")))
                            .flex()
                            .flex_row()
                            .min_w_0()
                            .track_scroll(&layout)
                            .child(
                                Button::new(select_id)
                                    .variant(ButtonVariant::Ghost)
                                    .track_focus(&focus)
                                    .padding(ButtonPadding::Horizontal(6.0))
                                    .width(px(160.0))
                                    .height(px(28.0))
                                    .vcenter()
                                    .text_size(font::SM)
                                    .tooltip(shown.clone())
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_1()
                                            .when(label.agent_owned, |row| {
                                                row.child(
                                                    div()
                                                        .flex_none()
                                                        .w(px(6.0))
                                                        .h(px(6.0))
                                                        .rounded(px(3.0))
                                                        .bg(dark().accent.primary),
                                                )
                                            })
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_color(if selected {
                                                        dark().text.primary
                                                    } else {
                                                        dark().text.secondary
                                                    })
                                                    .child(label.label.clone()),
                                            ),
                                    )
                                    .on_click(cx.listener(move |view, event, window, cx| {
                                        if !view.consume_button_key_click(&click_select, event) {
                                            view.browser_action(&click_select, window, cx);
                                        }
                                    }))
                                    .on_activate(cx.listener(move |view, _, window, cx| {
                                        if view.open_menu.is_none() {
                                            view.note_button_key_activate(&key_select);
                                            view.browser_action(&key_select, window, cx);
                                            cx.stop_propagation();
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id(gpui::SharedString::from(format!("{close_id}-layout")))
                            .flex_none()
                            .track_scroll(&close_layout)
                            .child(
                                Button::new(close_id)
                                    .variant(ButtonVariant::Ghost)
                                    .padding(ButtonPadding::None)
                                    .width(px(24.0))
                                    .height(px(28.0))
                                    .center()
                                    .child(icon_sized(Icon::Cancel, px(12.0)))
                                    .tooltip(format!("{} · {}", t("browser.tab_close"), shown))
                                    .track_focus(&close_focus)
                                    .on_click(cx.listener(move |view, event, window, cx| {
                                        if !view.consume_button_key_click(&click_close, event) {
                                            view.browser_action(&click_close, window, cx);
                                        }
                                    }))
                                    .on_activate(cx.listener(move |view, _, window, cx| {
                                        if view.open_menu.is_none() {
                                            view.note_button_key_activate(&key_close);
                                            view.browser_action(&key_close, window, cx);
                                            cx.stop_propagation();
                                        }
                                    })),
                            ),
                    ),
            );
        }
        strip.child(
            div()
                .id("browser-new-tab-layout")
                .flex_none()
                .track_scroll(&self.browser.layouts["browser-new-tab"])
                .child(
                    Button::new("browser-new-tab")
                        .variant(ButtonVariant::Ghost)
                        .padding(ButtonPadding::None)
                        .width(px(28.0))
                        .height(px(28.0))
                        .center()
                        .child(icon(Icon::Plus))
                        .tooltip(t("browser.new_tab"))
                        .track_focus(&self.browser.controls["browser-new-tab"])
                        .on_click(cx.listener(|view, event, window, cx| {
                            if !view.consume_button_key_click("browser-new-tab", event) {
                                view.browser_action("browser-new-tab", window, cx);
                            }
                        }))
                        .on_activate(cx.listener(|view, _, window, cx| {
                            if view.open_menu.is_none() {
                                view.note_button_key_activate("browser-new-tab");
                                view.browser_action("browser-new-tab", window, cx);
                                cx.stop_propagation();
                            }
                        })),
                ),
        )
    }

    pub(super) fn browser_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let rect = |id: &str| {
            let bounds = self.browser.layouts[id].bounds();
            AxRect::new(
                bounds.origin.x.into(),
                bounds.origin.y.into(),
                bounds.size.width.into(),
                bounds.size.height.into(),
            )
        };
        // 标签条节点按面板可视范围裁剪；被 overflow 裁掉的零面积不发布。
        let clip = |rect: AxRect| -> Option<AxRect> {
            let x = rect.x.max(frame.x);
            let y = rect.y.max(frame.y);
            let right = (rect.x + rect.width).min(frame.x + frame.width);
            let bottom = (rect.y + rect.height).min(frame.y + frame.height);
            (right > x && bottom > y).then(|| AxRect::new(x, y, right - x, bottom - y))
        };
        let mut node = AxNode::new("browser", AxRole::Group, t("inspector.tab_browser"), frame)
            .child(
                AxNode::new(
                    "browser-address",
                    AxRole::TextArea,
                    t("browser.address"),
                    rect("browser-address"),
                )
                .value(self.browser.input.read(cx).text())
                .focused(self.browser.focus.is_focused(window))
                .action(AxAction::Focus)
                .action(AxAction::SetValue),
            );
        for (id, label, _, enabled) in self.browser.actions() {
            node = node.child(
                AxNode::new(id, AxRole::Button, t(label), rect(id))
                    .enabled(enabled)
                    .focused(self.browser.controls[id].is_focused(window))
                    .action(AxAction::Press),
            );
        }
        // CU-14：标签条与工具栏同一 AX 组，逐标签发布选中 / 关闭按钮，
        // 坐标取标签条实测布局并按面板可视范围裁剪。
        if !self.browser.labels.is_empty() {
            let mut tabs_node = AxNode::new(
                "browser-tabs",
                AxRole::Group,
                t("browser.tabs"),
                rect("browser-tabs"),
            );
            for label in &self.browser.labels {
                let select_id = format!("browser-tab-select-{}", label.id);
                let close_id = format!("browser-tab-close-{}", label.id);
                let name = if label.agent_owned {
                    format!("{} · {}", label.label, t("browser.tab_agent"))
                } else {
                    label.label.clone()
                };
                let selected = self.browser.active.as_deref() == Some(label.id.as_str());
                let entries = [
                    (select_id, name.clone(), selected),
                    (
                        close_id,
                        format!("{} · {}", t("browser.tab_close"), name),
                        false,
                    ),
                ];
                for (id, text, selected) in entries {
                    let Some(layout) = self.browser.tab_layouts.get(&id) else {
                        continue;
                    };
                    let bounds = layout.bounds();
                    let Some(rect) = clip(AxRect::new(
                        bounds.origin.x.into(),
                        bounds.origin.y.into(),
                        bounds.size.width.into(),
                        bounds.size.height.into(),
                    )) else {
                        continue;
                    };
                    let focused = self
                        .browser
                        .tab_focus
                        .get(&id)
                        .is_some_and(|focus| focus.is_focused(window));
                    tabs_node = tabs_node.child(
                        AxNode::new(id, AxRole::Button, text, rect)
                            .selected(selected)
                            .focused(focused)
                            .action(AxAction::Press),
                    );
                }
            }
            // 「+」与标签按钮同规则裁剪（CU-14 复审④）：溢出被裁掉的
            // 零面积不发布，否则 AX 报告不可见按钮。
            if let Some(rect) = clip(rect("browser-new-tab")) {
                tabs_node = tabs_node.child(
                    AxNode::new(
                        "browser-new-tab",
                        AxRole::Button,
                        t("browser.new_tab"),
                        rect,
                    )
                    .focused(self.browser.controls["browser-new-tab"].is_focused(window))
                    .action(AxAction::Press),
                );
            }
            node = node.child(tabs_node);
        }
        let status = self.browser.status();
        if !status.is_empty() {
            node = node.child(AxNode::new(
                "browser-status",
                AxRole::StaticText,
                status,
                rect("browser-status"),
            ));
        }
        node
    }
}

impl AppView {
    /// 派发 Host 浏览器请求。CU-13 起在 handle_controller_event 收到请求时
    /// 立即以 app 级后台任务执行：隐藏窗口 render 停摆，不能等下一帧。
    /// 全程不打开/切换面板、不抢焦点；派发前先同步浏览器归属（切任务后
    /// 首帧前到达的请求不得绑定旧任务页面），中途切任务走重取模式落空后
    /// 明确报错。
    pub(super) fn dispatch_browser_request(&mut self, cx: &mut Context<Self>) {
        let Some((session, run_id, request_id, action)) = self.browser_request.take() else {
            return;
        };
        // 标签管理动作在主线程直接得出回执；页面动作绑定派发时的
        // BrowserView 跨 await 执行（CU-13 复审①，切任务后不重读面板）。
        enum BrowserOp {
            Immediate(serde_json::Value),
            Page {
                name: String,
                native: Rc<BrowserView>,
                /// new_tab 创建的标签 id，随回执带给模型。
                tab_id: Option<String>,
            },
        }
        cx.spawn(async move |this, cx| {
            let operation = this.update(cx, |view, cx| {
                // CU-13 复审 2：浏览器归属（browser_session ↔ 当前面板的
                // BrowserPanel）只在 render 或 Inspector 切换时同步；请求
                // 可能在切任务后、下一帧前到达。先按 projection 同步，
                // 再核对 browser_session 与请求 session 一致，保证绑定
                // （以及 close）作用于请求所属任务的页面，不是旧任务。
                view.sync_browser_session(cx);
                if view.projection.active_session_id.as_deref() != Some(&session)
                    || view.browser_session.as_deref() != Some(&session)
                    || view.projection.active_run_id.as_deref() != Some(&run_id)
                {
                    return Err("Task is no longer visible".to_string());
                }
                let name = action["action"].as_str().unwrap_or("").to_string();
                if name == "close" {
                    // CU-14：close 只释放本任务 Agent 创建的标签，用户
                    // 标签保留；标签清零后退回 CU-13 的整面板关闭语义。
                    let plan = plan_agent_close_all(
                        &view.browser.tab_metas(),
                        view.browser.active.as_deref(),
                        view.browser.agent.as_deref(),
                    );
                    let closed = plan.removed.clone();
                    view.browser.apply_close_plan(plan);
                    if view.browser.tabs.is_empty() {
                        view.browser.close(cx);
                        view.inspector_open_tabs
                            .retain(|tab| *tab != InspectorTab::Browser);
                        if view.inspector_tab == InspectorTab::Browser {
                            view.inspector_tab = InspectorTab::Home;
                        }
                    }
                    view.sync_browser_state(cx);
                    cx.notify();
                    return Ok(BrowserOp::Immediate(
                        json!({"closed": true, "closed_tabs": closed}),
                    ));
                }
                // CU-14：tabs 只读列举本任务标签（含归属与页面代际），
                // 不创建视图、不改选中。
                if name == "tabs" {
                    return Ok(BrowserOp::Immediate(view.browser.tabs_json()));
                }
                // 面板保持用户当前状态（可后台隐藏执行）；open 标记只让
                // Browser 标签可发现并跨任务记住，不强制弹出、不切页签。
                view.browser.open = true;
                view.remember_inspector_tab(InspectorTab::Browser);
                match name.as_str() {
                // CU-14：Agent 显式开新标签。归属 Agent 并成为派发
                // 目标；不动面板展示（用户正在看的标签不被 Agent 抢，
                // CU-14 复审①）、不前置系统窗口、不动键盘焦点。
                    "new_tab" => {
                        let native = match view.create_native_browser() {
                            Ok(native) => Rc::new(native),
                            Err(error) => return Err(error),
                        };
                        let id = view.browser.push_tab(native, true)?;
                        view.ensure_browser_poll(cx);
                        view.sync_browser_state(cx);
                        let native = view.browser.agent_native().expect("new tab").clone();
                        match action["url"].as_str() {
                            Some(url) => {
                                if let Err(error) = native.navigate(url) {
                                    // 创建即失败的导航不留空壳标签。
                                    let metas = view.browser.tab_metas();
                                    if let Some(plan) = plan_user_close(
                                        &metas,
                                        view.browser.active.as_deref(),
                                        view.browser.agent.as_deref(),
                                        &id,
                                    ) {
                                        view.browser.apply_close_plan(plan);
                                    }
                                    view.sync_browser_state(cx);
                                    cx.notify();
                                    return Err(error);
                                }
                                cx.notify();
                                Ok(BrowserOp::Page {
                                    name: "navigate".into(),
                                    native,
                                    tab_id: Some(id),
                                })
                            }
                            None => {
                                let generation = native.generation();
                                cx.notify();
                                Ok(BrowserOp::Immediate(json!({
                                    "tab_id": id,
                                    "generation": generation,
                                })))
                            }
                        }
                    }
                    "select_tab" => {
                        let id = action["tab_id"].as_str().unwrap_or("");
                        view.browser.select_agent_tab(id)?;
                        let mut data = view.browser.tab_json(id).expect("selected tab");
                        data["active"] = json!(view.browser.active.as_deref() == Some(id));
                        view.sync_browser_state(cx);
                        cx.notify();
                        Ok(BrowserOp::Immediate(data))
                    }
                    "close_tab" => {
                        let id = action["tab_id"].as_str().unwrap_or("");
                        // 归属闸：只关本任务 Agent 创建的标签；用户标签
                        // 与未知标签在这里得到明确错误。
                        let plan = plan_agent_close(
                            &view.browser.tab_metas(),
                            view.browser.active.as_deref(),
                            view.browser.agent.as_deref(),
                            id,
                        )?;
                        view.browser.apply_close_plan(plan);
                        let data = json!({
                            "closed": id,
                            "active_tab_id": view.browser.active,
                            "agent_tab_id": view.browser.agent,
                        });
                        view.sync_browser_state(cx);
                        cx.notify();
                        Ok(BrowserOp::Immediate(data))
                    }
                    "navigate" | "back" | "forward" | "reload" | "read" | "click" | "type"
                    | "screenshot" => {
                        // CU-14：页面动作作用于 Agent 绑定标签；无绑定时
                        // navigate 创建 Agent 标签（不占用用户已有标签）。
                        if view.browser.agent_native().is_none() {
                            if name != "navigate" {
                                return Err("Browser has no page; navigate first".into());
                            }
                            let native = match view.create_native_browser() {
                                Ok(native) => Rc::new(native),
                                Err(error) => return Err(error),
                            };
                            view.browser.push_tab(native, true)?;
                            view.ensure_browser_poll(cx);
                            view.sync_browser_state(cx);
                        }
                        let native = view.browser.agent_native().expect("agent tab").clone();
                        match name.as_str() {
                            "navigate" => native.navigate(action["url"].as_str().unwrap_or(""))?,
                            "back" if native.state().can_go_back => native.back(),
                            "forward" if native.state().can_go_forward => native.forward(),
                            "reload" => native.reload(),
                            "read" | "click" | "type" | "screenshot" => {}
                            _ => return Err("Unsupported action or unavailable history".into()),
                        }
                        cx.notify();
                        Ok(BrowserOp::Page {
                            name,
                            native,
                            tab_id: None,
                        })
                    }
                    _ => Err("Unsupported action".into()),
                }
            });
            let result: Result<serde_json::Value, String> = match operation {
                Ok(Ok(BrowserOp::Immediate(data))) => Ok(data),
                Ok(Ok(BrowserOp::Page {
                    name,
                    native,
                    tab_id,
                })) => {
                    if matches!(name.as_str(), "navigate" | "back" | "forward" | "reload") {
                        wait_browser_idle(&this, cx, &native, Duration::from_secs(15)).await;
                    }
                    let state = this
                        .update(cx, |_view, _cx| native.state())
                        .map_err(|e| e.to_string());
                    match state {
                        Err(error) => Err(error),
                        Ok(state) => {
                            if let Some(error) = state.error {
                                Err(error)
                            } else if state.loading {
                                Err("Page is still loading; read again later".into())
                            } else {
                                let primary =
                                    browser_script_call(&this, cx, &native, &action, &name).await;
                                match primary {
                                    // CU-13：元素动作成功后回一次新鲜观测
                                    //（等 ≤5s 收敛 loading 再 read），让模型
                                    // 一次调用拿到动作结果与最新页面，无需
                                    // 凭旧句柄猜状态。
                                    Ok(mut data) if matches!(name.as_str(), "click" | "type") => {
                                        wait_browser_idle(&this, cx, &native, Duration::from_secs(5)).await;
                                        // CU-13 复审：等待期间切任务时保留动作
                                        // 事实，观测明确记 observation_error，
                                        // 不把别的任务页面并入原 Run 的结果。
                                        let still_owned = this
                                            .update(cx, |view, _cx| {
                                                view.projection.active_session_id.as_deref()
                                                    == Some(&session)
                                                    && view.projection.active_run_id.as_deref()
                                                        == Some(&run_id)
                                                    && view
                                                        .browser
                                                        .agent_native()
                                                        .is_some_and(|current| Rc::ptr_eq(current, &native))
                                            })
                                            .unwrap_or(false);
                                        let observation = if still_owned {
                                            browser_script_call(&this, cx, &native, &action, "read")
                                                .await
                                        } else {
                                            Err("任务已切换，动作后观测跳过 / Task switched during action — observation skipped".into())
                                        };
                                        if let Some(obj) = data.as_object_mut() {
                                            match observation {
                                                Ok(observation) => {
                                                    obj.insert("observation".into(), observation);
                                                }
                                                Err(error) => {
                                                    obj.insert(
                                                        "observation".into(),
                                                        serde_json::Value::Null,
                                                    );
                                                    obj.insert(
                                                        "observation_error".into(),
                                                        error.into(),
                                                    );
                                                }
                                            }
                                        }
                                        Ok(data)
                                    }
                                    // new_tab 的导航回执带上标签 id。
                                    Ok(mut data) => {
                                        if let (Some(id), Some(obj)) =
                                            (tab_id, data.as_object_mut())
                                        {
                                            obj.insert("tab_id".into(), id.into());
                                        }
                                        Ok(data)
                                    }
                                    other => other,
                                }
                            }
                        }
                    }
                }
                Ok(Err(error)) => Err(error),
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |view, cx| {
                let reply = match result {
                    Ok(data) => serde_json::json!({"ok":true,"data":data}),
                    Err(error) => serde_json::json!({"ok":false,"error":error}),
                };
                view.controller.browser_respond(request_id, reply);
                cx.notify();
            });
        })
        .detach();
    }
}

/// 等页面收敛 loading（导航后给 200ms 让加载开始），超时也返回——
/// 调用方随后按 state 决定是否报错。
async fn wait_browser_idle(
    this: &gpui::WeakEntity<AppView>,
    cx: &mut gpui::AsyncApp,
    native: &Rc<BrowserView>,
    timeout: Duration,
) {
    smol::Timer::after(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    loop {
        let loading = this.update(cx, |_view, _cx| native.state().loading);
        if !matches!(loading, Ok(true)) || started.elapsed() >= timeout {
            break;
        }
        smol::Timer::after(Duration::from_millis(100)).await;
    }
}

/// 在真实 WebView 上跑一段观测/动作脚本并等回调（5s 超时）。
/// click/type 优先走 read 签发的句柄（代际 + TTL + 一次性由
/// pawork-browser 校验），无句柄回退 CSS selector。
async fn browser_script_call(
    this: &gpui::WeakEntity<AppView>,
    cx: &mut gpui::AsyncApp,
    native: &Rc<BrowserView>,
    action: &serde_json::Value,
    name: &str,
) -> Result<serde_json::Value, String> {
    let (send, receive) = smol::channel::bounded(1);
    let callback = move |result| {
        let _ = send.try_send(result);
    };
    let started = this
        .update(cx, |_view, _cx| {
            match name {
                "click" => {
                    if let Some(handle) = action["handle"].as_str() {
                        native.click_handle(handle, callback)
                    } else {
                        native.click(action["selector"].as_str().unwrap_or(""), callback)
                    }
                }
                "type" => {
                    let text = action["text"].as_str().unwrap_or("");
                    if let Some(handle) = action["handle"].as_str() {
                        native.type_handle(handle, text, callback)
                    } else {
                        native.type_text(action["selector"].as_str().unwrap_or(""), text, callback)
                    }
                }
                "screenshot" => native.snapshot(callback),
                _ => native.read_page(callback),
            }
            Ok(())
        })
        .map_err(|error| error.to_string())
        .and_then(|started| started);
    started?;
    let result = smol::future::or(
        async {
            receive
                .recv()
                .await
                .unwrap_or_else(|_| Err("Browser callback closed".into()))
        },
        async {
            smol::Timer::after(Duration::from_secs(5)).await;
            Err("Browser did not return a result".into())
        },
    )
    .await;
    result.and_then(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{plan_agent_close, plan_agent_close_all, plan_user_close, ClosePlan, TabMeta};

    fn tab(id: &str, agent_owned: bool) -> TabMeta {
        TabMeta {
            id: id.to_string(),
            agent_owned,
        }
    }

    #[test]
    fn agent_close_own_tab_fixes_selection_and_agent_binding() {
        let tabs = vec![
            tab("tab-user", false),
            tab("tab-a1", true),
            tab("tab-a2", true),
        ];

        // Agent 关掉自己正在展示的标签：选中落到原位置下一张，绑定失效。
        let plan = plan_agent_close(&tabs, Some("tab-a1"), Some("tab-a1"), "tab-a1").unwrap();
        assert_eq!(
            plan,
            ClosePlan {
                removed: vec!["tab-a1".to_string()],
                active: Some("tab-a2".to_string()),
                agent: None,
            }
        );

        // 被选中的标签在末尾：关闭后选中前一张。
        let plan = plan_agent_close(&tabs, Some("tab-a2"), Some("tab-a1"), "tab-a2").unwrap();
        assert_eq!(plan.active.as_deref(), Some("tab-a1"));
        assert_eq!(plan.agent.as_deref(), Some("tab-a1"));

        // 被关的不是选中标签：选中与绑定保持不动。
        let plan = plan_agent_close(&tabs, Some("tab-user"), Some("tab-a2"), "tab-a1").unwrap();
        assert_eq!(plan.active.as_deref(), Some("tab-user"));
        assert_eq!(plan.agent.as_deref(), Some("tab-a2"));

        // 整体清理只移除 Agent 标签，用户标签保留并成为选中。
        let plan = plan_agent_close_all(&tabs, Some("tab-a1"), Some("tab-a1"));
        assert_eq!(
            plan.removed,
            vec!["tab-a1".to_string(), "tab-a2".to_string()]
        );
        assert_eq!(plan.active.as_deref(), Some("tab-user"));
        assert_eq!(plan.agent, None);

        // 全为 Agent 标签时整体清理清空面板。
        let plan = plan_agent_close_all(&tabs[1..], Some("tab-a1"), Some("tab-a2"));
        assert_eq!(plan.active, None);
        assert_eq!(plan.agent, None);

        // 用户可关任意标签；Agent 绑定指向被关标签时随之失效。
        let plan = plan_user_close(&tabs, Some("tab-user"), Some("tab-a1"), "tab-a1").unwrap();
        assert_eq!(plan.agent, None);
        assert_eq!(plan.active.as_deref(), Some("tab-user"));
        assert!(plan_user_close(&tabs, Some("tab-user"), None, "tab-user").is_some());
        assert!(plan_user_close(&tabs, Some("tab-user"), None, "tab-missing").is_none());
    }

    #[test]
    fn agent_close_rejects_user_and_unknown_tabs() {
        let tabs = vec![tab("tab-user", false), tab("tab-a1", true)];

        let error = plan_agent_close(&tabs, Some("tab-user"), None, "tab-user").unwrap_err();
        assert!(error.contains("user"));

        let error = plan_agent_close(&tabs, Some("tab-user"), None, "tab-missing").unwrap_err();
        assert!(error.contains("Unknown tab"));
    }
}
