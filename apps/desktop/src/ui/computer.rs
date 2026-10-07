//! Inspector「后台操作」页（CU-18，GUI API 1.27）：computer-use 目标状态
//! 与授权管理。数据只来自 Host computer_target_list（登记事实视图），
//! 授权 / 撤销经 computer_target_approve / revoke 命令；Desktop 不依赖
//! computer-use、不读数据库、不直连 Provider（红线不变）。
//!
//! 语义边界：
//! - 授权按 run scope 生效，永不持久化；run 终态 Host 侧清授权并释放
//!   占用（CU-16/17）。面板如实呈现该口径，不提供任何持久授权入口。
//! - 面板交互只在 Desktop 进程内选择 / 决定，选择目标不激活目标应用；
//!   approve / revoke 只接受真实用户输入路径（鼠标 / 键盘 / AX 共用同一
//!   gate），不存在自动批准路径。
//! - 能力矩阵为 Host 按目标族裁决的后台支持度；unverified 与
//!   unsupported 同拒（CU-02）。系统权限（Screen Recording /
//!   Accessibility）状态不经 wire 传输，面板只在存在未验证能力时给出
//!   可执行指引，不冒充探测结果。

use gpui::{div, prelude::*, px, Context, ScrollHandle, SharedString, Window};

use pawork_client::{
    ComputerActionKind, ComputerAppFamily, ComputerCapabilitySupport, ComputerGrantKind,
    ComputerTargetCapability, ComputerTargetIdentity, ComputerTargetListData,
    ComputerTargetState,
};

use crate::projection::ConnectionState;
use crate::ui::accessibility::{dynamic_identifier, AxAction, AxNode, AxRect, AxRole};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::empty_state::EmptyState;
use crate::ui::components::icon::{icon_sized, Icon};
use crate::ui::components::label::Label;
use crate::ui::components::skeleton::loading_skeleton;
use crate::ui::i18n::t;
use crate::ui::theme::{dark, font, metrics};

use super::{AppView, INSPECTOR_TAB_INDEX};

/// AX / 布局 / 键盘焦点共用的动作 id 前缀（render 与 AX 同源；dispatch
/// 按前缀反查目标，与 subagent-select- 同构）。
pub(super) const COMPUTER_APPROVE_ONCE_PREFIX: &str = "computer-approve-once-";
pub(super) const COMPUTER_APPROVE_RUN_PREFIX: &str = "computer-approve-run-";
pub(super) const COMPUTER_REVOKE_PREFIX: &str = "computer-revoke-";

/// 面板拉取状态（与 ResourcesFetch 同族，reason 单列保持 Copy）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComputerFetch {
    Idle,
    Fetching,
    Ready,
    Failed,
}

/// 「后台操作」面板状态。数据绑定查询时的 session；切换活动会话即随
/// begin_loading 复位，不跨会话展示。断线保留旧数据并标记 stale（与
/// Resources / 子代理卡同口径），重连后由 refresh 重查收敛。
pub(super) struct ComputerPanelState {
    session_id: Option<String>,
    request_id: u64,
    pub fetch: ComputerFetch,
    pub fetch_error: Option<String>,
    /// Host 回执的会话 workspace 归属（None = 无项目会话，恒空列表）。
    pub workspace_id: Option<String>,
    pub targets: Vec<ComputerTargetState>,
    pub stale_reason: Option<String>,
    /// 在途授权动作的（目标 key, 派出代次）（一次一个，防双击重复派出；
    /// 代次随切会话 / 断线失效，迟到回执不覆盖新动作）。
    pub action_in_flight: Option<(String, u64)>,
    /// approve / revoke 失败原因（wire 原文，不翻译）；成功回执清除。
    pub action_error: Option<String>,
    pub scroll: ScrollHandle,
}

impl Default for ComputerPanelState {
    fn default() -> Self {
        Self {
            session_id: None,
            request_id: 0,
            fetch: ComputerFetch::Idle,
            fetch_error: None,
            workspace_id: None,
            targets: Vec::new(),
            stale_reason: None,
            action_in_flight: None,
            action_error: None,
            scroll: ScrollHandle::new(),
        }
    }
}

impl ComputerPanelState {
    /// 开始一次列表查询；返回请求代次。换会话即复位（旧会话数据不留）。
    pub(super) fn begin_loading(&mut self, session_id: &str) -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        if self.session_id.as_deref() != Some(session_id) {
            let scroll = std::mem::take(&mut self.scroll);
            *self = Self::default();
            self.scroll = scroll;
        }
        self.session_id = Some(session_id.to_string());
        self.fetch = ComputerFetch::Fetching;
        self.fetch_error = None;
        self.stale_reason = None;
        self.request_id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.request_id
    }

    pub(super) fn apply_loaded(
        &mut self,
        session_id: &str,
        request_id: u64,
        data: ComputerTargetListData,
    ) -> bool {
        if self.session_id.as_deref() != Some(session_id) || self.request_id != request_id {
            return false;
        }
        self.fetch = ComputerFetch::Ready;
        self.fetch_error = None;
        self.stale_reason = None;
        self.workspace_id = data.workspace_id.clone();
        self.targets = data.targets.clone();
        true
    }

    pub(super) fn apply_failed(&mut self, session_id: &str, request_id: u64, reason: String) {
        if self.session_id.as_deref() == Some(session_id) && self.request_id == request_id {
            self.fetch = ComputerFetch::Failed;
            self.fetch_error = Some(reason);
        }
    }

    /// 断线 / 不可查询收口：保留已加载数据，清在途标记（对应请求不会再
    /// 有回执），request_id 归零使迟到回执失效。
    pub(super) fn mark_stale(&mut self, reason: &str) {
        self.request_id = 0;
        if self.fetch == ComputerFetch::Fetching {
            self.fetch = if self.targets.is_empty() {
                ComputerFetch::Idle
            } else {
                ComputerFetch::Ready
            };
        }
        self.action_in_flight = None;
        self.stale_reason = Some(reason.to_string());
    }

    /// approve / revoke 回执落地：仅（会话 + 派出代次）双匹配的回执收敛
    /// 状态——A→B→A 切换后旧 A 的迟到回执不清除新动作的在途标记或覆盖
    /// 错误（切会话经 begin_loading 复位、断线经 mark_stale 清在途，两条
    /// 路径都使旧代次失效）。Ok 清在途与错误（权威状态由紧随的重查
    /// 收敛）；Err 清在途并记录原因（面板内呈现，不冒充成功）。返回是否
    /// 匹配落地；未匹配时调用方不触发重查。
    pub(super) fn finish_action(
        &mut self,
        session_id: &str,
        action_id: u64,
        result: Result<(), String>,
    ) -> bool {
        if self.session_id.as_deref() != Some(session_id) {
            return false;
        }
        if self.action_in_flight.as_ref().map(|(_, id)| *id) != Some(action_id) {
            return false;
        }
        self.action_in_flight = None;
        match result {
            Ok(()) => self.action_error = None,
            Err(reason) => self.action_error = Some(reason),
        }
        true
    }

    /// 当前绑定是否即活动会话（断线重连后旧会话数据不冒充当前会话）。
    pub(super) fn matches_session(&self, session_id: Option<&str>) -> bool {
        self.session_id.is_some() && self.session_id.as_deref() == session_id
    }
}

/// 目标行身份键：run scope + 目标锚点（同 run 同目标唯一；跨 run 同名
/// 目标不撞键）。仅用于 UI 键 / AX id，不上 wire。
pub(super) fn target_key(state: &ComputerTargetState) -> String {
    let anchor = match &state.target {
        ComputerTargetIdentity::Application { app } => format!("app:{}", app.bundle_id),
        ComputerTargetIdentity::Website { browser, origin } => {
            format!("site:{}:{}", browser.bundle_id, origin)
        }
    };
    format!("{}:{}", state.scope.run_id, anchor)
}

/// 动作 id（dynamic_identifier 转义规则与 dispatch 反查同源）。
fn action_id(prefix: &str, key: &str) -> String {
    dynamic_identifier(prefix.trim_end_matches('-'), key)
}

/// 动作派出代次：进程级 static 计数器（AGENTS.md 工程经验：计数器
/// 必须进程级，不依赖时钟分辨率兜底唯一性）。只用于本地在途匹配，
/// 不上 wire。
fn next_computer_action_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// 撤销按钮可用判定（渲染与 AX 同源）：不要求仍存在 grant——一次授权
/// 消耗后 grant 即消失，但派发许可与占用可能仍在；revoke 即目标级停止，
/// 必须始终可达。
fn computer_revoke_enabled(can_act: bool, busy: bool) -> bool {
    can_act && !busy
}

/// 由动作 id 反查目标（dispatch 只拿到完整 identifier；重算同源 id
/// 比较，不做反向解码）。
pub(super) fn find_target_for_action<'a>(
    targets: &'a [ComputerTargetState],
    prefix: &str,
    identifier: &str,
) -> Option<&'a ComputerTargetState> {
    targets
        .iter()
        .find(|state| action_id(prefix, &target_key(state)) == identifier)
}

/// 目标标题（数据原文，不翻译）：应用显示 bundle id，网站显示 origin。
fn target_title(state: &ComputerTargetState) -> String {
    match &state.target {
        ComputerTargetIdentity::Application { app } => app.bundle_id.clone(),
        ComputerTargetIdentity::Website { origin, .. } => origin.clone(),
    }
}

/// 目标副行：本地化族名（网站目标附浏览器 bundle id 原文）。
fn target_subtitle(state: &ComputerTargetState) -> String {
    let app = match &state.target {
        ComputerTargetIdentity::Application { app } => app,
        ComputerTargetIdentity::Website { browser, .. } => browser,
    };
    let family = match app.family {
        ComputerAppFamily::Appkit => t("computer.family_appkit"),
        ComputerAppFamily::Chromium => t("computer.family_chromium"),
        ComputerAppFamily::Browser => t("computer.family_browser"),
    };
    match &state.target {
        ComputerTargetIdentity::Application { .. } => family.to_string(),
        ComputerTargetIdentity::Website { browser, .. } => format!(
            "{} · {}",
            t("computer.website_in").replace("{}", &browser.bundle_id),
            family
        ),
    }
}

/// 授权状态词（render 与 AX 同源）。
fn grant_label(grant: Option<ComputerGrantKind>) -> &'static str {
    match grant {
        Some(ComputerGrantKind::ForRun) => t("computer.grant_for_run"),
        Some(ComputerGrantKind::Once) => t("computer.grant_once"),
        None => t("computer.grant_none"),
    }
}

fn grant_color(grant: Option<ComputerGrantKind>) -> gpui::Rgba {
    match grant {
        Some(ComputerGrantKind::ForRun) => dark().semantic.success_fg,
        Some(ComputerGrantKind::Once) => dark().accent.primary,
        None => dark().semantic.warning_text,
    }
}

/// 目标状态摘要行（grant + 绑定 / 占用 / 暂停事实；数据段不翻译）。
fn target_state_text(state: &ComputerTargetState) -> String {
    let mut parts = vec![grant_label(state.grant).to_string()];
    if let Some(handle) = &state.bound_window {
        parts.push(format!("{} · {handle}", t("computer.bound_window")));
    }
    if state
        .occupied_by
        .as_ref()
        .is_some_and(|owner| *owner != state.scope)
    {
        parts.push(t("computer.occupied_other").to_string());
    }
    if state.dispatch_paused {
        parts.push(t("computer.paused").to_string());
    }
    parts.join(" · ")
}

fn action_name(action: ComputerActionKind) -> &'static str {
    t(match action {
        ComputerActionKind::CaptureWindow => "computer.action_capture_window",
        ComputerActionKind::AxRead => "computer.action_ax_read",
        ComputerActionKind::AxSemanticWrite => "computer.action_ax_semantic_write",
        ComputerActionKind::AxPress => "computer.action_ax_press",
        ComputerActionKind::TargetedKeys => "computer.action_targeted_keys",
        ComputerActionKind::MenuShortcut => "computer.action_menu_shortcut",
        ComputerActionKind::MenubarPress => "computer.action_menubar_press",
        ComputerActionKind::Pointer => "computer.action_pointer",
        ComputerActionKind::WindowMove => "computer.action_window_move",
        ComputerActionKind::WindowResize => "computer.action_window_resize",
    })
}

fn support_label(support: ComputerCapabilitySupport) -> &'static str {
    t(match support {
        ComputerCapabilitySupport::Supported => "computer.cap_supported",
        ComputerCapabilitySupport::Unverified => "computer.cap_unverified",
        ComputerCapabilitySupport::Unsupported => "computer.cap_unsupported",
    })
}

/// 能力 chip 字形与颜色（不靠颜色单独表达状态：✓/?/× 前缀同源进 AX）。
fn support_glyph(support: ComputerCapabilitySupport) -> (&'static str, gpui::Rgba) {
    match support {
        ComputerCapabilitySupport::Supported => ("✓", dark().semantic.success_fg),
        ComputerCapabilitySupport::Unverified => ("?", dark().semantic.warning_text),
        ComputerCapabilitySupport::Unsupported => ("×", dark().text.tertiary),
    }
}

/// AX description 的完整能力明细（十动作 × background / minimized）。
fn capability_detail(caps: &[ComputerTargetCapability]) -> String {
    caps.iter()
        .map(|cap| {
            format!(
                "{}: {} / {}",
                action_name(cap.action),
                support_label(cap.background),
                support_label(cap.minimized),
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// 是否存在未验证能力（驱动系统权限指引行）。
fn has_unverified(caps: &[ComputerTargetCapability]) -> bool {
    caps.iter().any(|cap| {
        cap.background == ComputerCapabilitySupport::Unverified
            || cap.minimized == ComputerCapabilitySupport::Unverified
    })
}

/// 最小化态说明：与后台一致时不占行；收窄时列出仍支持的动作。
fn minimized_note(caps: &[ComputerTargetCapability]) -> Option<String> {
    let background: Vec<ComputerActionKind> = caps
        .iter()
        .filter(|cap| cap.background == ComputerCapabilitySupport::Supported)
        .map(|cap| cap.action)
        .collect();
    let minimized: Vec<ComputerActionKind> = caps
        .iter()
        .filter(|cap| cap.minimized == ComputerCapabilitySupport::Supported)
        .map(|cap| cap.action)
        .collect();
    if minimized == background {
        return None;
    }
    if minimized.is_empty() {
        return Some(t("computer.minimized_none").to_string());
    }
    let names = minimized
        .iter()
        .map(|action| action_name(*action))
        .collect::<Vec<_>>()
        .join(", ");
    Some(t("computer.minimized_some").replace("{}", &names))
}

impl AppView {
    /// 授权 / 撤销动作 gate：连接 × Host 支持 × 活动 run × 面板绑定当前
    /// 会话 × 非查询失败态 × 无在途动作。渲染 disabled 与 AX enabled 同源；
    /// 鼠标 / 键盘 / AX 三通道都经此 gate，不存在自动批准路径；失败占位
    /// 期间（按钮已隐藏）AX Press 也不得派出。
    pub(super) fn computer_can_act(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.controller.supports_computer_targets()
            && self.projection.active_run_id.is_some()
            && self
                .computer
                .matches_session(self.projection.active_session_id.as_deref())
            && self.computer.fetch != ComputerFetch::Failed
            && self.computer.action_in_flight.is_none()
    }

    /// 目标列表可见条件（渲染与 AX 同源）：连接 × 有会话 × 面板已绑定
    /// 该会话 × 非失败态 × 持有数据。失败占位与列表互斥——AX 不得凭
    /// 保留数据发布视觉界面已隐藏的行与按钮。
    pub(super) fn computer_targets_visible(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_session_id.is_some()
            && self
                .computer
                .matches_session(self.projection.active_session_id.as_deref())
            && self.computer.fetch != ComputerFetch::Failed
            && !self.computer.targets.is_empty()
    }

    pub(super) fn computer_can_refresh(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.controller.supports_computer_targets()
            && self.projection.active_session_id.is_some()
            && self.computer.fetch != ComputerFetch::Fetching
    }

    /// 拉取 / 重查目标列表（打开页签、切会话、授权回执收敛、手动刷新
    /// 共用）。断线保留旧数据并标记 stale；旧 Host 由 version_required
    /// 占位如实呈现，不派出查询。
    pub(super) fn refresh_computer_targets(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            return;
        };
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.computer.mark_stale("not connected");
            cx.notify();
            return;
        }
        if !self.controller.supports_computer_targets() {
            cx.notify();
            return;
        }
        let request_id = self.computer.begin_loading(&session_id);
        if !self.controller.computer_target_list(session_id, request_id) {
            self.computer.mark_stale("computer target list unavailable");
        }
        cx.notify();
    }

    /// 面板绑定的会话（仅当它就是当前活动会话）——授权命令只发向当前
    /// 会话，旧会话数据不冒充。
    fn computer_bound_session(&self) -> Option<String> {
        let active = self.projection.active_session_id.as_deref()?;
        self.computer
            .matches_session(Some(active))
            .then(|| active.to_string())
    }

    /// 派出成功：登记在途（目标 key, 派出代次）（一次一个，防双击重复
    /// 派出）；派出失败（断线 / 旧 Host 竞态）：记录通用不可用原因
    ///（本地事实，不伪造 wire 错误），代次废弃。
    fn note_computer_action_dispatched(
        &mut self,
        key: &str,
        action_id: u64,
        dispatched: bool,
        cx: &mut Context<Self>,
    ) {
        if dispatched {
            self.computer.action_in_flight = Some((key.to_string(), action_id));
            self.computer.action_error = None;
        } else {
            self.computer.action_error = Some(t("computer.action_unavailable").to_string());
        }
        cx.notify();
    }

    fn computer_grant(&mut self, key: &str, kind: ComputerGrantKind, cx: &mut Context<Self>) {
        if !self.computer_can_act() {
            return;
        }
        let Some((run_id, target)) = self
            .computer
            .targets
            .iter()
            .find(|state| target_key(state) == key)
            .map(|state| (state.scope.run_id.clone(), state.target.clone()))
        else {
            return;
        };
        let Some(session_id) = self.computer_bound_session() else {
            return;
        };
        let action_id = next_computer_action_id();
        let dispatched =
            self.controller
                .computer_target_approve(session_id, action_id, run_id, target, kind);
        self.note_computer_action_dispatched(key, action_id, dispatched, cx);
    }

    fn computer_revoke(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.computer_can_act() {
            return;
        }
        let Some((run_id, target)) = self
            .computer
            .targets
            .iter()
            .find(|state| target_key(state) == key)
            .map(|state| (state.scope.run_id.clone(), state.target.clone()))
        else {
            return;
        };
        let Some(session_id) = self.computer_bound_session() else {
            return;
        };
        let action_id = next_computer_action_id();
        let dispatched =
            self.controller
                .computer_target_revoke(session_id, action_id, run_id, target);
        self.note_computer_action_dispatched(key, action_id, dispatched, cx);
    }

    /// 按钮共用入口：前缀决定动作（once / for_run / revoke）。
    fn computer_action_for_prefix(&mut self, prefix: &str, key: &str, cx: &mut Context<Self>) {
        match prefix {
            COMPUTER_APPROVE_ONCE_PREFIX => self.computer_grant(key, ComputerGrantKind::Once, cx),
            COMPUTER_APPROVE_RUN_PREFIX => self.computer_grant(key, ComputerGrantKind::ForRun, cx),
            _ => self.computer_revoke(key, cx),
        }
    }

    /// AX dispatch 入口：按三个动作前缀反查目标后执行（渲染与 AX 同源
    /// id，不反向解码 key）；找不到目标即列表已刷新，不动作。
    pub(super) fn computer_action_by_id(&mut self, identifier: &str, cx: &mut Context<Self>) {
        for (prefix, kind) in [
            (COMPUTER_APPROVE_ONCE_PREFIX, Some(ComputerGrantKind::Once)),
            (COMPUTER_APPROVE_RUN_PREFIX, Some(ComputerGrantKind::ForRun)),
            (COMPUTER_REVOKE_PREFIX, None),
        ] {
            if !identifier.starts_with(prefix) {
                continue;
            }
            let Some(key) = find_target_for_action(&self.computer.targets, prefix, identifier)
                .map(target_key)
            else {
                return;
            };
            match kind {
                Some(kind) => self.computer_grant(&key, kind, cx),
                None => self.computer_revoke(&key, cx),
            }
            return;
        }
    }

    pub(super) fn computer_element(
        &mut self,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let supported = self.controller.supports_computer_targets();
        let has_session = self.projection.active_session_id.is_some();
        let bound = self
            .computer
            .matches_session(self.projection.active_session_id.as_deref());
        let can_refresh = self.computer_can_refresh();
        let can_act = self.computer_can_act();
        let busy = self.computer.action_in_flight.is_some();
        // 渲染期收集 owned 副本，切断对 self 的借用（与 subagent_panel
        // 同口径），后续 entry() 需要 &mut self。
        let targets = self.computer.targets.clone();
        // 焦点句柄 / 实测布局随可见键收敛，不跨会话累积；与 AX 发布
        // 共用列表可见条件（失败态不保留按钮布局，杜绝隐藏按钮可 Press）。
        let mut visible: std::collections::HashSet<String> = std::collections::HashSet::new();
        visible.insert("computer-refresh".to_string());
        if self.computer_targets_visible() {
            for state in &targets {
                let key = target_key(state);
                visible.insert(dynamic_identifier("computer-target", &key));
                visible.insert(action_id(COMPUTER_APPROVE_ONCE_PREFIX, &key));
                visible.insert(action_id(COMPUTER_APPROVE_RUN_PREFIX, &key));
                visible.insert(action_id(COMPUTER_REVOKE_PREFIX, &key));
            }
        }
        self.computer_item_focus.retain(|id, _| visible.contains(id));
        self.computer_item_layouts.retain(|id, _| visible.contains(id));

        let refresh_layout = self
            .computer_item_layouts
            .entry("computer-refresh".to_string())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(
                Label::new(t("computer.title"))
                    .size(font::XS)
                    .color(dark().text.tertiary),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("computer-refresh-layout")
                    .track_scroll(&refresh_layout)
                    .child(
                        Button::new("computer-refresh")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                            .text_color(dark().text.secondary)
                            .disabled(!can_refresh)
                            .child(icon_sized(Icon::Refresh, px(metrics::ICON_SM)))
                            .tooltip(t("computer.tooltip_refresh"))
                            .track_focus(&self.computer_refresh_focus)
                            .on_click(cx.listener(|view, event, _window, cx| {
                                if view.consume_button_key_click("computer-refresh", event) {
                                    return;
                                }
                                view.refresh_computer_targets(cx);
                            }))
                            .on_activate(cx.listener(|view, _event, _window, cx| {
                                view.note_button_key_activate("computer-refresh");
                                view.refresh_computer_targets(cx);
                                cx.stop_propagation();
                            })),
                    ),
            );
        let body = if !connected {
            computer_placeholder("computer.unavailable").into_any_element()
        } else if !has_session {
            computer_placeholder("computer.no_task").into_any_element()
        } else if !bound {
            // 切会话后的短暂间隙：refresh 即 begin_loading 复位；旧会话
            // 数据不跨会话展示。旧 Host（minor < 27）永不派出查询、面板
            // 保持未绑定——直接给版本要求提示，不卡在 loading。
            if supported {
                computer_placeholder("computer.loading").into_any_element()
            } else {
                computer_placeholder("computer.version_required").into_any_element()
            }
        } else if self.computer.fetch == ComputerFetch::Failed {
            computer_placeholder_colored(self.computer.fetch_error.clone().unwrap_or_default())
                .into_any_element()
        } else if self.computer_targets_visible() {
            // 已持有数据即如实呈现（动作由 computer_can_act 单独门控）；
            // version_required 只兜底「旧 Host 必然无数据」的空态。列表
            // 可见条件与 AX 发布同源（失败态已在上方分支拦截）。
            self.computer_target_list_element(&targets, can_act, busy, cx)
                .into_any_element()
        } else if !supported {
            computer_placeholder("computer.version_required").into_any_element()
        } else if self.computer.fetch == ComputerFetch::Idle {
            computer_placeholder("computer.unavailable").into_any_element()
        } else {
            if self.computer.fetch == ComputerFetch::Fetching {
                computer_placeholder("computer.loading").into_any_element()
            } else if self.computer.workspace_id.is_none() {
                // Host 权威：无项目会话恒空列表。
                computer_placeholder("computer.no_workspace").into_any_element()
            } else if self.projection.active_run_id.is_none() {
                computer_placeholder("computer.no_run").into_any_element()
            } else {
                computer_placeholder("computer.empty").into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .when_some(self.computer.stale_reason.clone(), |block, reason| {
                block.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(dark().semantic.warning_text)
                        .text_size(font::XS)
                        .text_color(dark().semantic.warning_text)
                        .child(format!("{} · {reason}", t("common.stale_data"))),
                )
            })
            .when_some(self.computer.action_error.clone(), |block, reason| {
                block.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(dark().semantic.danger_text)
                        .text_size(font::XS)
                        .text_color(dark().semantic.danger_text)
                        .child(format!("{} · {reason}", t("computer.action_failed"))),
                )
            })
            .child(body)
    }

    fn computer_target_list_element(
        &mut self,
        targets: &[ComputerTargetState],
        can_act: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut list = div()
            .id("computer-target-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .track_scroll(&self.computer.scroll)
            .overflow_y_scroll();
        for state in targets {
            list = list.child(self.computer_target_card(state, can_act, busy, cx));
        }
        list.child(
            div()
                .px_1()
                .py_1()
                .text_size(font::XS)
                .text_color(dark().text.tertiary)
                .child(t("computer.scope_note")),
        )
    }

    /// 单目标卡：标题 + 授权状态 + 副行（族 / 浏览器）+ 状态行 + 能力
    /// chips + 授权按钮行。卡片外层 div 承载实测布局（AX 行几何同源）。
    fn computer_target_card(
        &mut self,
        state: &ComputerTargetState,
        can_act: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let key = target_key(state);
        let row_id = dynamic_identifier("computer-target", &key);
        let row_layout = self
            .computer_item_layouts
            .entry(row_id.clone())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let enabled = can_act && !busy;
        // 能力 chips：✓/?/× 字形 + 颜色双通道（不靠颜色单独表达状态）。
        let mut caps_row = div().flex().flex_row().flex_wrap().gap_1();
        for cap in &state.capabilities {
            let (glyph, color) = support_glyph(cap.background);
            caps_row = caps_row.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(dark().border.subtle)
                    .text_size(font::XS)
                    .text_color(color)
                    .child(format!("{glyph} {}", action_name(cap.action))),
            );
        }
        let mut card = div()
            .id(SharedString::from(format!("{row_id}-layout")))
            .track_scroll(&row_layout)
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded(px(8.0))
            .border_1()
            .border_color(dark().border.subtle)
            // 标题行：flex_row + flex_1 + min_w_0 同层承载 truncate
            //（min_w_0 截断投毒修法，见 AGENTS.md 工程经验）。
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            div()
                                .truncate()
                                .text_size(font::SM)
                                .text_color(dark().text.primary)
                                .child(target_title(state)),
                        ),
                    )
                    .child(
                        Label::new(grant_label(state.grant))
                            .size(font::XS)
                            .color(grant_color(state.grant)),
                    ),
            )
            .child(
                div().flex().flex_row().child(
                    div().flex_1().min_w_0().child(
                        div()
                            .truncate()
                            .text_size(font::XS)
                            .text_color(dark().text.tertiary)
                            .child(target_subtitle(state)),
                    ),
                ),
            )
            .child(
                div()
                    .w_full()
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .child(target_state_text(state)),
            )
            .child(caps_row);
        if let Some(note) = minimized_note(&state.capabilities) {
            card = card.child(
                div()
                    .w_full()
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(note),
            );
        }
        if has_unverified(&state.capabilities) {
            card = card.child(
                div()
                    .w_full()
                    .text_size(font::XS)
                    .text_color(dark().semantic.warning_text)
                    .child(t("computer.permission_note")),
            );
        }
        card.child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .pt_1()
                .child(self.computer_action_button(
                    COMPUTER_APPROVE_ONCE_PREFIX,
                    &key,
                    t("computer.allow_once"),
                    ButtonVariant::Primary,
                    enabled,
                    cx,
                ))
                .child(self.computer_action_button(
                    COMPUTER_APPROVE_RUN_PREFIX,
                    &key,
                    t("computer.allow_for_run"),
                    ButtonVariant::Success,
                    enabled,
                    cx,
                ))
                .child(self.computer_action_button(
                    COMPUTER_REVOKE_PREFIX,
                    &key,
                    t("computer.revoke"),
                    ButtonVariant::Danger,
                    computer_revoke_enabled(can_act, busy),
                    cx,
                )),
        )
    }

    /// 卡片授权按钮：id = 前缀 + 目标 key（dynamic_identifier 转义）；
    /// 鼠标 / 键盘 / AX 三通道共用同一 dispatch 与 gate。外层 div 承载
    /// 实测布局供 AX 发布。
    fn computer_action_button(
        &mut self,
        prefix: &'static str,
        key: &str,
        label: &'static str,
        variant: ButtonVariant,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = action_id(prefix, key);
        let focus = self
            .computer_item_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true).tab_index(INSPECTOR_TAB_INDEX))
            .clone();
        let layout = self
            .computer_item_layouts
            .entry(id.clone())
            .or_insert_with(ScrollHandle::new)
            .clone();
        let click_id = id.clone();
        let activate_id = id.clone();
        let click_key = key.to_string();
        let activate_key = key.to_string();
        div()
            .id(SharedString::from(format!("{id}-layout")))
            .track_scroll(&layout)
            .child(
                Button::new(id)
                    .variant(variant)
                    .disabled(!enabled)
                    .label(label)
                    .text_size(font::SM)
                    .height(px(28.0))
                    .radius(6.0)
                    .track_focus(&focus)
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        if view.consume_button_key_click(&click_id, event) {
                            return;
                        }
                        view.computer_action_for_prefix(prefix, &click_key, cx);
                    }))
                    .on_activate(cx.listener(move |view, _event, _window, cx| {
                        view.note_button_key_activate(&activate_id);
                        view.computer_action_for_prefix(prefix, &activate_key, cx);
                        cx.stop_propagation();
                    })),
            )
    }
}

/// 占位文案按 i18n key 取主文案与说明（同源；不能按翻译后的串匹配）。
fn computer_placeholder(key: &'static str) -> EmptyState {
    let description = match key {
        "computer.unavailable" => t("computer.unavailable_desc"),
        "computer.version_required" => t("computer.version_required_desc"),
        "computer.no_task" => t("computer.no_task_desc"),
        "computer.no_workspace" => t("computer.no_workspace_desc"),
        "computer.no_run" => t("computer.no_run_desc"),
        "computer.empty" => t("computer.empty_desc"),
        "computer.loading" => t("computer.loading_desc"),
        _ => t("common.placeholder_no_details"),
    };
    let mut placeholder = computer_placeholder_content(
        t(key).to_string(),
        description.to_string(),
        dark().text.primary,
    );
    if key == "computer.loading" {
        placeholder = placeholder.child(loading_skeleton("computer.loading-skeleton"));
    }
    placeholder
}

/// 失败占位：标题本地化；reason 为 wire 数据，不翻译。
fn computer_placeholder_colored(reason: String) -> EmptyState {
    computer_placeholder_content(
        t("computer.error_title").into(),
        reason,
        dark().semantic.danger_text,
    )
}

fn computer_placeholder_content(title: String, description: String, color: gpui::Rgba) -> EmptyState {
    EmptyState::new()
        .icon(Icon::Approvals)
        .icon_size(px(metrics::ICON_SIZE))
        .child(Label::new(title).size(font::BASE).color(color))
        .child(
            Label::new(description)
                .size(font::SM)
                .color(dark().text.secondary),
        )
}

/// tracked 布局的 AX 矩形（可选按滚动视口裁剪）；未测量 / 零面积不发布
///（RV-07：几何不伪造）。
fn measured_ax_rect(
    view: &AppView,
    id: &str,
    viewport: Option<&gpui::Bounds<gpui::Pixels>>,
) -> Option<AxRect> {
    let bounds = view.computer_item_layouts.get(id)?.bounds();
    let rect = match viewport {
        Some(viewport) => bounds.intersect(viewport),
        None => bounds,
    };
    if rect.size.width <= px(0.0) || rect.size.height <= px(0.0) {
        return None;
    }
    Some(AxRect::new(
        rect.origin.x.into(),
        rect.origin.y.into(),
        rect.size.width.into(),
        rect.size.height.into(),
    ))
}

/// 「后台操作」页 AX 投影：组 description 承载 fetch / stale / 动作错误
/// 状态；行与按钮只发布实测几何（视口裁剪）。enabled 与渲染 disabled
/// 同源（computer_can_refresh / computer_can_act）。
pub(super) fn computer_ax(view: &AppView, window: &Window, frame: AxRect) -> AxNode {
    let fetch_state = match view.computer.fetch {
        ComputerFetch::Idle => "idle".to_string(),
        ComputerFetch::Fetching => "loading".to_string(),
        ComputerFetch::Ready => format!("ready · {} targets", view.computer.targets.len()),
        ComputerFetch::Failed => format!(
            "failed · {}",
            view.computer.fetch_error.as_deref().unwrap_or_default()
        ),
    };
    let mut description = format!("Computer use targets · {fetch_state}");
    if let Some(reason) = &view.computer.stale_reason {
        description.push_str(&format!(" · stale · {reason}"));
    }
    if let Some(error) = &view.computer.action_error {
        description.push_str(&format!(" · {error}"));
    }
    let mut node = AxNode::new("computer", AxRole::Group, t("inspector.tab_computer"), frame)
        .description(description);
    if let Some(rect) = measured_ax_rect(view, "computer-refresh", None) {
        node = node.child(
            AxNode::new("computer-refresh", AxRole::Button, t("computer.tooltip_refresh"), rect)
                .enabled(view.computer_can_refresh())
                .focused(
                    view.open_menu.is_none() && view.computer_refresh_focus.is_focused(window),
                )
                .action(AxAction::Press),
        );
    }
    let can_act = view.computer_can_act();
    let busy = view.computer.action_in_flight.is_some();
    // 与渲染共用列表可见条件：失败占位期间不凭保留数据发布行与按钮。
    if view.computer_targets_visible() {
        let viewport = view.computer.scroll.bounds();
        if viewport.size.width > px(0.0) && viewport.size.height > px(0.0) {
            let mut list = AxNode::new(
                "computer-target-list",
                AxRole::List,
                t("computer.title"),
                AxRect::new(
                    viewport.origin.x.into(),
                    viewport.origin.y.into(),
                    viewport.size.width.into(),
                    viewport.size.height.into(),
                ),
            );
            for state in &view.computer.targets {
                let key = target_key(state);
                let row_id = dynamic_identifier("computer-target", &key);
                let Some(rect) = measured_ax_rect(view, &row_id, Some(&viewport)) else {
                    continue;
                };
                let mut item = AxNode::new(row_id, AxRole::ListItem, target_title(state), rect)
                    .value(format!(
                        "{} · {}",
                        target_subtitle(state),
                        target_state_text(state)
                    ))
                    .description(capability_detail(&state.capabilities));
                for (prefix, label, enabled) in [
                    (
                        COMPUTER_APPROVE_ONCE_PREFIX,
                        t("computer.allow_once"),
                        can_act && !busy,
                    ),
                    (
                        COMPUTER_APPROVE_RUN_PREFIX,
                        t("computer.allow_for_run"),
                        can_act && !busy,
                    ),
                    (
                        COMPUTER_REVOKE_PREFIX,
                        t("computer.revoke"),
                        computer_revoke_enabled(can_act, busy),
                    ),
                ] {
                    let id = action_id(prefix, &key);
                    let Some(rect) = measured_ax_rect(view, &id, Some(&viewport)) else {
                        continue;
                    };
                    item = item.child(
                        AxNode::new(id.clone(), AxRole::Button, label, rect)
                            .enabled(enabled)
                            .focused(
                                view.open_menu.is_none()
                                    && view
                                        .computer_item_focus
                                        .get(&id)
                                        .is_some_and(|focus| focus.is_focused(window)),
                            )
                            .action(AxAction::Press),
                    );
                }
                list = list.child(item);
            }
            node = node.child(list);
        }
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_client::{ComputerAppIdentity, ComputerTargetScope};

    fn caps(background: ComputerCapabilitySupport) -> Vec<ComputerTargetCapability> {
        [
            ComputerActionKind::CaptureWindow,
            ComputerActionKind::AxRead,
            ComputerActionKind::AxSemanticWrite,
            ComputerActionKind::AxPress,
            ComputerActionKind::TargetedKeys,
            ComputerActionKind::MenuShortcut,
            ComputerActionKind::MenubarPress,
            ComputerActionKind::Pointer,
            ComputerActionKind::WindowMove,
            ComputerActionKind::WindowResize,
        ]
        .into_iter()
        .map(|action| ComputerTargetCapability {
            action,
            background,
            minimized: background,
        })
        .collect()
    }

    fn fixture_targets(run: &str) -> Vec<ComputerTargetState> {
        let scope = || ComputerTargetScope {
            workspace_id: "ws-1".into(),
            run_id: run.into(),
        };
        vec![
            ComputerTargetState {
                target: ComputerTargetIdentity::Application {
                    app: ComputerAppIdentity {
                        bundle_id: "com.apple.TextEdit".into(),
                        family: ComputerAppFamily::Appkit,
                    },
                },
                scope: scope(),
                grant: Some(ComputerGrantKind::ForRun),
                bound_window: Some("window-7".into()),
                occupied_by: None,
                dispatch_paused: false,
                capabilities: caps(ComputerCapabilitySupport::Supported),
            },
            ComputerTargetState {
                target: ComputerTargetIdentity::Website {
                    browser: ComputerAppIdentity {
                        bundle_id: "com.microsoft.edgemac".into(),
                        family: ComputerAppFamily::Browser,
                    },
                    origin: "https://example.test".into(),
                },
                scope: scope(),
                grant: None,
                bound_window: None,
                occupied_by: None,
                dispatch_paused: true,
                capabilities: caps(ComputerCapabilitySupport::Unverified),
            },
        ]
    }

    /// SessionId 未从 client re-export：Data 经冻结 serde 形状构造
    ///（与 controller 的构造口径同族）。
    fn list_data(session: &str, run: &str) -> ComputerTargetListData {
        serde_json::from_value(serde_json::json!({
            "session_id": session,
            "workspace_id": "ws-1",
            "targets": serde_json::to_value(fixture_targets(run)).expect("targets serialize"),
        }))
        .expect("frozen ComputerTargetListData shape")
    }

    #[test]
    fn computer_panel_rejects_stale_receipts_and_resets_on_session_switch() {
        let mut state = ComputerPanelState::default();
        let first = state.begin_loading("s1");
        let second = state.begin_loading("s1");
        assert!(!state.apply_loaded("s1", first, list_data("s1", "r1")));
        assert!(state.apply_loaded("s1", second, list_data("s1", "r1")));
        assert_eq!(state.targets.len(), 2);
        // 换会话复位：旧会话数据不跨会话展示。
        let third = state.begin_loading("s2");
        assert!(state.targets.is_empty());
        assert!(state.workspace_id.is_none());
        assert!(!state.apply_loaded("s1", third, list_data("s1", "r1")));
    }

    #[test]
    fn computer_panel_mark_stale_keeps_data_and_invalidates_old_responses() {
        let mut state = ComputerPanelState::default();
        let request = state.begin_loading("s1");
        assert!(state.apply_loaded("s1", request, list_data("s1", "r1")));
        state.action_in_flight = Some(("key".into(), 3));
        state.mark_stale("connection lost");
        assert_eq!(state.fetch, ComputerFetch::Ready);
        assert_eq!(state.targets.len(), 2);
        assert!(state.action_in_flight.is_none(), "在途动作不会再有回执");
        assert_eq!(state.stale_reason.as_deref(), Some("connection lost"));
        // request_id 归零：断线前排出的请求迟到回执失效。
        assert!(!state.apply_loaded("s1", request, list_data("s1", "r1")));
    }

    /// 回执收敛要求（会话 + 派出代次）双匹配：A→B→A 切换后旧 A 的
    /// 迟到回执不清除新动作的在途标记或覆盖错误；只有匹配回执落地
    ///（返回 true，调用方才触发重查）。
    #[test]
    fn computer_panel_finish_action_requires_matching_generation() {
        let mut state = ComputerPanelState::default();
        state.begin_loading("s1");
        state.action_in_flight = Some(("key".into(), 7));
        // 跨会话回执不落地。
        assert!(!state.finish_action("s2", 7, Ok(())));
        assert_eq!(state.action_in_flight, Some(("key".into(), 7)));
        // 同会话但代次不匹配（旧 A 回执迟到）不落地。
        assert!(!state.finish_action("s1", 6, Ok(())));
        assert_eq!(state.action_in_flight, Some(("key".into(), 7)));
        // 匹配回执收敛：Err 落在面板呈现，Ok 清错误。
        assert!(state.finish_action("s1", 7, Err("unknown_run".into())));
        assert!(state.action_in_flight.is_none());
        assert_eq!(state.action_error.as_deref(), Some("unknown_run"));
        state.action_in_flight = Some(("key".into(), 9));
        assert!(state.finish_action("s1", 9, Ok(())));
        assert!(state.action_in_flight.is_none());
        assert!(state.action_error.is_none());
        // 新一轮在途登记后，上一轮的迟到回执不再收敛。
        state.action_in_flight = Some(("key".into(), 11));
        assert!(!state.finish_action("s1", 9, Ok(())));
        assert_eq!(state.action_in_flight, Some(("key".into(), 11)));
    }

    #[test]
    fn computer_action_id_roundtrip_uses_dynamic_escape() {
        let targets = fixture_targets("r1");
        let site = &targets[1];
        let key = target_key(site);
        assert!(key.contains("https://example.test"));
        for prefix in [
            COMPUTER_APPROVE_ONCE_PREFIX,
            COMPUTER_APPROVE_RUN_PREFIX,
            COMPUTER_REVOKE_PREFIX,
        ] {
            let id = action_id(prefix, &key);
            assert!(id.starts_with(prefix));
            assert_eq!(
                find_target_for_action(&targets, prefix, &id).map(target_key),
                Some(key.clone()),
                "同源 id 反查命中同一目标"
            );
        }
        // 其他前缀的 id 不在本前缀命名空间命中。
        let once_id = action_id(COMPUTER_APPROVE_ONCE_PREFIX, &key);
        assert!(find_target_for_action(&targets, COMPUTER_REVOKE_PREFIX, &once_id).is_none());
    }

    /// AX 主路径：行 / 按钮按实测几何发布；无 client 的测试环境授权
    /// 动作禁用（gate 含 Host 支持判定），被拒的 Press 不产生在途 /
    /// 错误状态。
    #[gpui::test]
    fn computer_panel_ax_publishes_targets_and_gates_actions(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("cu18-computer-panel.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        let app_key = target_key(&fixture_targets("r1")[0]);
        let site_key = target_key(&fixture_targets("r1")[1]);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.set_connection(ConnectionState::Connected {
                    instance_id: "test".into(),
                });
                view.projection.active_session_id = Some("s1".into());
                view.projection.active_run_id = Some("r1".into());
                let request_id = view.computer.begin_loading("s1");
                assert!(view.computer.apply_loaded("s1", request_id, list_data("s1", "r1")));
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.inspector_tab = crate::ui::inspector::InspectorTab::Computer;
                view.close_open_menu(cx);
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                let group = tree.find("computer").expect("computer group");
                assert!(
                    group
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .contains("2 targets"),
                    "group description carries fetch state: {:?}",
                    group.description
                );
                let app_row_id = dynamic_identifier("computer-target", &app_key);
                let app_row = tree.find(&app_row_id).expect("application row");
                assert_eq!(app_row.label, "com.apple.TextEdit");
                let approve_id = action_id(COMPUTER_APPROVE_ONCE_PREFIX, &app_key);
                let approve = tree.find(&approve_id).expect("approve once button");
                assert!(!approve.enabled, "无 client 时授权动作禁用");
                // 撤销按钮与授权按钮同一 gate（撤销不要求仍存在 grant：
                // 一次授权消耗后许可与占用可能仍在，revoke 即目标级停止）。
                let revoke_site_id = action_id(COMPUTER_REVOKE_PREFIX, &site_key);
                assert!(tree
                    .find(&revoke_site_id)
                    .is_some_and(|node| !node.enabled));
                // 禁用节点的 Press 被 permits 复核拒绝（
                // handle_accessibility_request 内部按当前树重核），不产生
                // 任何在途 / 错误状态。
                let request = crate::ui::accessibility::AxRequest {
                    identifier: approve_id,
                    action: AxAction::Press,
                    value: None,
                };
                view.handle_accessibility_request(request, window, cx);
                assert!(view.computer.action_in_flight.is_none());
                assert!(view.computer.action_error.is_none());
            });
        });
        // 查询失败态：AX 与渲染共用列表可见条件——行与按钮不再发布，
        // 凭旧 id 的迟到 Press 不派出（隐藏按钮不可经 AX 绕过）。
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.computer.fetch = ComputerFetch::Failed;
                view.computer.fetch_error = Some("boom".into());
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                tree.validate().unwrap();
                let app_row_id = dynamic_identifier("computer-target", &app_key);
                let approve_id = action_id(COMPUTER_APPROVE_ONCE_PREFIX, &app_key);
                assert!(tree.find(&app_row_id).is_none(), "失败态不发布目标行");
                assert!(tree.find(&approve_id).is_none(), "失败态不发布动作按钮");
                let request = crate::ui::accessibility::AxRequest {
                    identifier: approve_id,
                    action: AxAction::Press,
                    value: None,
                };
                view.handle_accessibility_request(request, window, cx);
                assert!(view.computer.action_in_flight.is_none());
                assert!(view.computer.action_error.is_none());
            });
        });
    }
}
