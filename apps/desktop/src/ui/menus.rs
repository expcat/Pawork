//! 工作台浮层菜单的种类、开关、高亮与激活。

use gpui::{Context, KeyDownEvent, Pixels, Point, ScrollHandle, Window};

use crate::projection::SettingsRole;

use super::changes;
use super::input_area;
use super::inspector::InspectorTab;
use super::settings;
use super::{changes_tab_key_target, terminal_can_close, AppView};

/// 当前打开的浮层菜单（五组共享，开新即关旧，修互斥不对称）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MenuKind {
    ComposerAdd,
    Scope,
    ProjectTask,
    Model,
    /// Composer 推理强度菜单（ADR-063 / API 1.21）。
    Effort,
    /// Settings「Default models」四角色下拉（OPT-3b / ADR-055 D5）。
    SettingsRole(SettingsRole),
    /// Settings 供应商「Manage models」启用弹层（OPT-3a / ADR-055 D2；
    /// 键为 provider id，单一状态位保证至多一个弹层）。
    SettingsProviderModels(String),
    /// 条目「···」菜单，键为 timeline event_id。
    Entry(String),
    /// Inspector 折叠态的 ActivityPopover（Workspace Header Activity
    /// 触发器弹出，R6 Wave A 自 StatusBar 迁入）。
    Activity,
    /// Inspector 面板名称选择器（Changes / Terminal / Resources）。
    InspectorPanel,
}

impl AppView {
    /// 触发器 toggle：开新关旧（单一 Option<MenuKind>，修互斥不对称），
    /// 再点同一触发器关闭。外点关闭先行触发且 click 按下位置与标记相同
    /// （同一次物理点击）时视为关闭收尾，不重开；否则清陈旧标记正常处理。
    pub(super) fn toggle_menu(
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
        if matches!(&target, MenuKind::Entry(_)) && self.open_menu.as_ref() != Some(&target) {
            self.entry_menu_scroll = ScrollHandle::new();
        }
        self.open_menu = if self.open_menu.as_ref() == Some(&target) {
            None
        } else {
            Some(target)
        };
        self.menu_highlight = None;
        if matches!(self.open_menu, Some(MenuKind::Activity)) {
            self.load_activity_subagents();
        }
        cx.notify();
    }

    /// 浮层外按下鼠标：关闭并留下衔接标记（种类 + 按下位置，供 toggle_menu
    /// 判定同一次物理点击）。
    pub(super) fn dismiss_menu_on_outside(
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
    pub(super) fn close_open_menu(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn handle_root_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quick_search.open {
            self.handle_quick_search_key(event, window, cx);
            return;
        }
        if self.handle_settings_search_key(event, window, cx) {
            cx.stop_propagation();
            return;
        }
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
        // 行内改名：Escape 取消并回焦会话行（进入编辑态时浮层已收口，
        // 菜单分支不会先于这里接管）。
        if key == "escape" && self.session_rename_input_focused(window, cx) {
            self.cancel_session_rename(window, cx);
            cx.stop_propagation();
            return;
        }
        if key == "escape" && self.account_rename_input_focused(window, cx) {
            self.cancel_account_rename(window, cx);
            cx.stop_propagation();
            return;
        }
        // RV-01：附件预览浮层开着时 Escape 只关预览（不动草稿与菜单）。
        if key == "escape" && self.composer_attachment_preview.is_some() {
            self.close_composer_attachment_preview(cx);
            cx.stop_propagation();
            return;
        }
        if self.handle_settings_models_key(event, window, cx) {
            return;
        }
        let menu = self.open_menu.clone();
        if let Some(kind) = menu {
            if self.model_search_focus.is_focused(window)
                && (key == "space" || self.model_search_input.read(cx).is_composing())
            {
                return;
            }
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
                    if matches!(kind, MenuKind::Scope) && highlight == selected {
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
        if key == "escape" {
            self.cancel_file_attachment_picker();
        }
        if self.handle_inspector_key(event, window, cx) {
            return;
        }
        if key == "escape" {
            self.close_open_menu(cx);
        }
    }

    /// Inspector 的普通键盘路径。所有判断都基于与 render/AX 共用的
    /// FocusHandle：面板选择器走既有菜单 ↑/↓/Enter/Esc；Changes 二级页签
    /// 与文件行用方向键；Enter/Space 与 click 复用同一 action。返回 true
    /// 表示已消费。
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

        let tabs = self.panel_tabs(cx);
        if let Some(index) = tabs.iter().position(|tab| {
            self.inspector_item_focus
                .get(&tab.id)
                .is_some_and(|focus| focus.is_focused(window))
        }) {
            let target = match key {
                "left" => Some((index + tabs.len() - 1) % tabs.len()),
                "right" => Some((index + 1) % tabs.len()),
                "home" => Some(0),
                "end" => Some(tabs.len() - 1),
                _ => None,
            };
            if let Some(target) = target {
                self.activate_panel_tab(&tabs[target], cx);
                self.pending_inspector_focus = None;
                self.inspector_reveal_selected = true;
                if let Some(focus) = self.inspector_item_focus.get(&tabs[target].id) {
                    window.focus(focus);
                }
                cx.stop_propagation();
                cx.notify();
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
        } else if self.inspector_panel_focus.is_focused(window) && activate {
            Some("inspector-panel")
        } else if self.inspector_back_focus.is_focused(window) && activate {
            Some("inspector-back")
        } else if self.inspector_approval_focus.is_focused(window) && activate {
            Some("inspector-approval-hint")
        } else if self.inspector_activity_focus.is_focused(window) && activate {
            Some("inspector-toggle")
        } else if self.inspector_expand_focus.is_focused(window) && activate {
            Some("inspector-expand")
        } else if self.changes_refresh_focus.is_focused(window) && activate {
            Some("changes-refresh")
        } else if self.changes_diff_scroll_left_focus.is_focused(window)
            && activate
            && changes::diff_horizontal_ends(&self.changes.diff_scroll).0
        {
            Some("changes-diff-scroll-left")
        } else if self.changes_diff_scroll_right_focus.is_focused(window)
            && activate
            && changes::diff_horizontal_ends(&self.changes.diff_scroll).1
        {
            Some("changes-diff-scroll-right")
        } else if self.resources_refresh_focus.is_focused(window) && activate {
            Some("resources-refresh")
        } else if self.terminal_back_to_bottom_focus.is_focused(window) && activate {
            Some("terminal-back-to-bottom")
        } else if self.terminal_close_focus.is_focused(window)
            && activate
            && self.terminal_pending_close.is_none()
            && terminal_can_close(&self.projection.connection, &self.projection.terminal)
        {
            Some("terminal-close")
        } else {
            None
        };
        let Some(action) = action else {
            return false;
        };
        self.note_button_key_activate(action);
        match action {
            "inspector-collapse" => self.on_toggle_inspector(window, cx),
            "inspector-panel" => self.toggle_menu(MenuKind::InspectorPanel, None, cx),
            "inspector-back" | "inspector-approval-hint" => self.on_inspector_back(window, cx),
            "inspector-toggle" => self.toggle_menu(MenuKind::Activity, None, cx),
            "inspector-expand" => self.on_toggle_inspector(window, cx),
            "changes-refresh" => self.activate_changes_refresh(cx),
            "changes-diff-scroll-left" => self.scroll_diff_horizontal(-1.0, cx),
            "changes-diff-scroll-right" => self.scroll_diff_horizontal(1.0, cx),
            "resources-refresh" => self.activate_resources_refresh(cx),
            "terminal-back-to-bottom" => {
                self.terminal_scroll.jump_to_bottom();
                cx.notify();
            }
            "terminal-close" => self.on_close_terminal(window, cx),
            _ => unreachable!(),
        }
        cx.stop_propagation();
        true
    }

    /// 关闭菜单并把焦点送回触发器（design §3.6：Escape 关闭后焦点回触发器）。
    pub(super) fn close_menu_and_focus_trigger(
        &mut self,
        kind: MenuKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let trigger = match kind {
            MenuKind::ComposerAdd => self
                .settings_action_focus
                .get("composer-attach")
                .cloned()
                .unwrap_or_else(|| self.focus_handle.clone()),
            MenuKind::Scope => self.scope_focus.clone(),
            MenuKind::ProjectTask => self.project_task_focus.clone(),
            MenuKind::Model => self.model_focus.clone(),
            MenuKind::Effort => self.effort_focus.clone(),
            MenuKind::SettingsRole(role) => self
                .settings_action_focus
                .get(&settings::settings_role_trigger_identifier(role))
                .cloned()
                .unwrap_or_else(|| self.focus_handle.clone()),
            MenuKind::SettingsProviderModels(provider_id) => self
                .settings_action_focus
                .get(&settings::settings_manage_models_identifier(&provider_id))
                .cloned()
                .unwrap_or_else(|| self.focus_handle.clone()),
            MenuKind::Entry(event_id) => self
                .timeline_entry_action_focus
                .get(&event_id)
                .cloned()
                .unwrap_or_else(|| self.focus_handle.clone()),
            MenuKind::Activity => self.inspector_activity_focus.clone(),
            MenuKind::InspectorPanel => self.inspector_panel_focus.clone(),
        };
        self.open_menu = None;
        self.menu_highlight = None;
        window.focus(&trigger);
        cx.notify();
    }

    /// 菜单高亮行数。所有可点击 MenuRow 均进入同一普通键盘分派。
    pub(super) fn menu_item_count(&self) -> usize {
        match self.open_menu.as_ref() {
            Some(MenuKind::ComposerAdd) => input_area::ComposerAction::ALL.len(),
            Some(MenuKind::Scope | MenuKind::ProjectTask) => self.project_menu_options().len() + 1,
            Some(MenuKind::Model) => self.model_menu_row_count() + 1,
            Some(MenuKind::Effort) => self.effort_menu_options().len(),
            // 清除行始终可选；空候选时仍可移除已保存的默认角色。
            Some(MenuKind::SettingsRole(_)) => {
                let entries = settings::settings_role_menu_entries(
                    &self.projection.models,
                    &self.projection.settings_providers.providers,
                );
                entries.len() + 1
            }
            Some(MenuKind::Entry(event_id)) => self.entry_menu_actions(event_id).len(),
            Some(MenuKind::Activity) => 1,
            Some(MenuKind::InspectorPanel) => InspectorTab::ALL.len(),
            // 弹层行是可聚焦 Switch（Tab / Enter / Space 自理），不进
            // MenuRow 键盘分派；↑/↓ 无可移动项。
            Some(MenuKind::SettingsProviderModels(_)) => 0,
            None => 0,
        }
    }

    /// 当前选中项在菜单中的行位（键盘高亮的回落起点）。
    pub(super) fn menu_selected_index(&self) -> usize {
        match self.open_menu.as_ref() {
            Some(MenuKind::ComposerAdd) => input_area::ComposerAction::ALL
                .iter()
                .position(|action| self.composer_action_enabled(*action))
                .unwrap_or(0),
            Some(MenuKind::Scope | MenuKind::ProjectTask) => self
                .project_menu_options()
                .iter()
                .position(|(workspace_id, _)| *workspace_id == self.scope_workspace_id)
                .unwrap_or(0),
            Some(MenuKind::Model) => self
                .projection
                .effective_model()
                .and_then(|(provider, id)| {
                    self.filtered_model_entries()
                        .iter()
                        .position(|model| model.provider_id == *provider && model.id == *id)
                })
                .unwrap_or(0),
            // 行 0 = 自动（None）；显式值落在候选行时为其位 +1。
            Some(MenuKind::Effort) => self
                .projection
                .effective_effort()
                .and_then(|current| {
                    self.effort_menu_options()
                        .iter()
                        .position(|option| option.as_deref() == Some(current))
                })
                .unwrap_or(0),
            // 行 0 = 清除；当前值落在候选行时为其位 +1，未设置回落清除行。
            Some(MenuKind::SettingsRole(role)) => {
                let entries = settings::settings_role_menu_entries(
                    &self.projection.models,
                    &self.projection.settings_providers.providers,
                );
                self.projection
                    .settings_providers
                    .role_value(*role)
                    .and_then(|(provider, model)| {
                        entries
                            .iter()
                            .position(|entry| entry.provider_id == *provider && entry.id == *model)
                    })
                    .map(|ix| ix + 1)
                    .unwrap_or(0)
            }
            // Switch 自带键盘激活；无 MenuRow 高亮行。
            Some(MenuKind::SettingsProviderModels(_)) => 0,
            Some(MenuKind::InspectorPanel) => InspectorTab::ALL
                .iter()
                .position(|tab| *tab == self.inspector_tab)
                .unwrap_or(0),
            Some(MenuKind::Entry(_) | MenuKind::Activity) | None => 0,
        }
    }

    /// 生效高亮行：未移动过（None）回落到当前选中项。
    pub(super) fn menu_highlight_effective(&self, selected_ix: usize) -> usize {
        self.menu_highlight.unwrap_or(selected_ix)
    }

    pub(super) fn move_menu_highlight(&mut self, forward: bool) {
        let len = self.menu_item_count();
        if len == 0 {
            return;
        }
        let current = self
            .menu_highlight
            .unwrap_or_else(|| self.menu_selected_index())
            .min(len - 1);
        let mut next = if forward {
            (current + 1) % len
        } else {
            (current + len - 1) % len
        };
        if matches!(self.open_menu, Some(MenuKind::ComposerAdd)) {
            while next != current
                && !self.composer_action_enabled(input_area::ComposerAction::ALL[next])
            {
                next = if forward {
                    (next + 1) % len
                } else {
                    (next + len - 1) % len
                };
            }
        }
        self.menu_highlight = Some(next);
        match self.open_menu {
            Some(MenuKind::Scope | MenuKind::ProjectTask) => {
                self.scope_menu_scroll.scroll_to_item(next)
            }
            Some(MenuKind::Model) => {
                if let Some(child) = self.model_menu_scroll_child_index(next) {
                    self.model_menu_scroll.scroll_to_item(child);
                }
            }
            Some(MenuKind::Entry(_)) => self.entry_menu_scroll.scroll_to_item(next),
            Some(MenuKind::SettingsRole(role)) => {
                self.scroll_settings_role_menu_to_item(role, next)
            }
            _ => {}
        }
    }

    /// Enter 选择高亮行：等价点击对应 MenuRow（复用既有 select 路径，含
    /// 单开互斥与菜单关闭）。
    pub(super) fn activate_menu_item(
        &mut self,
        kind: MenuKind,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            MenuKind::ComposerAdd => {
                if self.menu_highlight.is_none() {
                    return;
                }
                if let Some(action) = input_area::ComposerAction::ALL.get(ix) {
                    self.activate_composer_action(*action, window, cx);
                }
            }
            MenuKind::Scope | MenuKind::ProjectTask => {
                let options = self.project_menu_options();
                if let Some((workspace_id, _)) = options.get(ix).cloned() {
                    self.on_select_scope(workspace_id, window, cx);
                } else if ix == options.len() {
                    self.on_open_project(window, cx);
                }
            }
            MenuKind::Model => {
                // AppKit 开菜单的 Return 可重复投递到新搜索框。未输入/移动时
                // 保持当前项；选中后焦点落根容器，后续同键不能重开或发送草稿。
                if self.model_search_query.is_empty()
                    && self.menu_highlight.is_none()
                    && ix == self.menu_selected_index()
                {
                    return;
                }
                if ix == self.model_menu_row_count() {
                    self.on_manage_composer_models(window, cx);
                    return;
                }
                if let Some(model) = self.filtered_model_entries().get(ix) {
                    self.on_select_model(model.clone(), cx);
                    window.focus(&self.focus_handle);
                }
            }
            MenuKind::SettingsRole(role) => {
                let entries = settings::settings_role_menu_entries(
                    &self.projection.models,
                    &self.projection.settings_providers.providers,
                );
                if ix == 0 {
                    self.on_select_settings_role(role, None, cx);
                } else if let Some(model) = entries.get(ix - 1) {
                    self.on_select_settings_role(
                        role,
                        Some((model.provider_id.clone(), model.id.clone())),
                        cx,
                    );
                }
            }
            MenuKind::Effort => {
                let options = self.effort_menu_options();
                if let Some(option) = options.get(ix) {
                    self.on_select_effort(option.clone(), cx);
                    window.focus(&self.focus_handle);
                }
            }
            // 弹层控件自带键盘激活（Switch / 按钮），无 MenuRow 行。
            MenuKind::SettingsProviderModels(_) => {}
            MenuKind::Entry(event_id) => {
                self.activate_entry_action(&event_id, ix, window, cx);
            }
            MenuKind::Activity => {
                if ix == 0 {
                    self.on_activity_open_changes(window, cx);
                }
            }
            MenuKind::InspectorPanel => {
                if let Some(&tab) = InspectorTab::ALL.get(ix) {
                    self.launch_inspector_tool(tab, cx);
                    self.close_menu_and_focus_trigger(MenuKind::InspectorPanel, window, cx);
                }
            }
        }
    }
}
