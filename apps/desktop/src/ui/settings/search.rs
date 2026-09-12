//! GUI2-06：设置查找目录、匹配与定位。只导航，不改任何设置值。

use std::time::Duration;

use gpui::{point, px, Context, Focusable, KeyDownEvent, SharedString, Window};

use super::*;
use crate::projection::ConnectionState;
use crate::ui::accessibility::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::theme::metrics;

/// 目录条目种类：页面标题 vs 页内行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsSearchKind {
    Page,
    Row,
}

/// 静态设置查找目录。匹配标题、公开说明与中英文别名；不含凭证。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SettingsSearchEntry {
    pub page: SettingsPage,
    pub row_id: &'static str,
    pub title_key: &'static str,
    pub title_en: &'static str,
    pub title_zh: &'static str,
    pub description_en: &'static str,
    pub description_zh: &'static str,
    pub aliases: &'static str,
    pub kind: SettingsSearchKind,
}

impl SettingsSearchEntry {
    pub fn matches(self, query: &str) -> bool {
        let q = query.trim();
        if q.is_empty() {
            return true;
        }
        let q = q.to_lowercase();
        [
            self.title_en,
            self.title_zh,
            self.description_en,
            self.description_zh,
            self.aliases,
        ]
        .into_iter()
        .any(|text| text.to_lowercase().contains(&q))
    }

    pub fn display_title(self) -> &'static str {
        t(self.title_key)
    }

    pub fn result_id(self) -> String {
        format!("settings-search-result-{:?}-{}", self.page, self.row_id)
    }
}

const fn page_entry(
    page: SettingsPage,
    title_key: &'static str,
    title_en: &'static str,
    title_zh: &'static str,
    aliases: &'static str,
) -> SettingsSearchEntry {
    SettingsSearchEntry {
        page,
        row_id: "settings-page-title",
        title_key,
        title_en,
        title_zh,
        description_en: "",
        description_zh: "",
        aliases,
        kind: SettingsSearchKind::Page,
    }
}

const fn row_entry(
    page: SettingsPage,
    row_id: &'static str,
    title_key: &'static str,
    title_en: &'static str,
    title_zh: &'static str,
    description_en: &'static str,
    description_zh: &'static str,
    aliases: &'static str,
) -> SettingsSearchEntry {
    SettingsSearchEntry {
        page,
        row_id,
        title_key,
        title_en,
        title_zh,
        description_en,
        description_zh,
        aliases,
        kind: SettingsSearchKind::Row,
    }
}

/// 八页标题 + 任务覆盖的设置行。Cmd+K 页面分组由此派生。
pub(crate) fn settings_search_entries() -> &'static [SettingsSearchEntry] {
    const ENTRIES: &[SettingsSearchEntry] = &[
        page_entry(
            SettingsPage::Providers,
            "settings.nav.providers",
            "Models & providers",
            "模型与提供商",
            "providers models 供应商 模型 提供商",
        ),
        page_entry(
            SettingsPage::General,
            "settings.nav.general",
            "Network",
            "网络",
            "network 网络",
        ),
        page_entry(
            SettingsPage::Permissions,
            "settings.nav.permissions",
            "Approvals",
            "审批",
            "permissions approvals 权限 审批",
        ),
        page_entry(
            SettingsPage::Tools,
            "settings.nav.tools",
            "Tools & MCP",
            "工具与 MCP",
            "tools mcp 工具",
        ),
        page_entry(
            SettingsPage::Terminal,
            "settings.nav.terminal",
            "Terminal",
            "终端",
            "terminal 终端",
        ),
        page_entry(
            SettingsPage::Appearance,
            "settings.nav.appearance",
            "Appearance",
            "外观",
            "appearance 外观",
        ),
        page_entry(
            SettingsPage::Advanced,
            "settings.nav.advanced",
            "Advanced",
            "高级",
            "advanced 高级",
        ),
        page_entry(
            SettingsPage::About,
            "settings.nav.about",
            "About",
            "关于",
            "about 关于",
        ),
        row_entry(
            SettingsPage::Appearance,
            "settings-appearance-text-size",
            "settings.appearance.text_size",
            "Text size",
            "字号",
            "Choose the workspace text size.",
            "选择工作台字号。",
            "font size scale 文字大小 字体",
        ),
        row_entry(
            SettingsPage::Appearance,
            "settings-appearance-language",
            "settings.appearance.language",
            "Language",
            "语言",
            "Interface language.",
            "界面语言。",
            "language locale 界面语言",
        ),
        row_entry(
            SettingsPage::Providers,
            "settings-providers-heading",
            "settings.providers.section_providers",
            "Providers",
            "供应商",
            "Provider accounts, API keys, and OAuth.",
            "供应商账号、API key 与 OAuth。",
            "provider api key oauth 供应商 认证 密钥",
        ),
        row_entry(
            SettingsPage::Providers,
            "settings-default-roles",
            "settings.roles.title",
            "Default models",
            "默认模型",
            "Default model roles and manage models.",
            "默认角色模型与管理模型。",
            "default model manage models 默认模型 管理模型 模型",
        ),
        row_entry(
            SettingsPage::General,
            "settings-proxy-heading",
            "settings.network.proxy_title",
            "HTTP proxy",
            "HTTP 代理",
            "Host outbound HTTP proxy.",
            "Pawork 服务出站 HTTP 代理。",
            "proxy proxy_url 代理",
        ),
        row_entry(
            SettingsPage::Permissions,
            "settings-approval-mode-header",
            "settings.permissions.mode_title",
            "Approval mode",
            "审批模式",
            "Default approval mode for new runs.",
            "新运行的默认审批模式。",
            "approval 审批 审批模式",
        ),
        row_entry(
            SettingsPage::Permissions,
            "settings-workspace-trust-status",
            "settings.permissions.session_trust_title",
            "Workspace trust",
            "项目信任",
            "Remember trust for this workspace.",
            "记住当前项目信任。",
            "trust session 会话信任 信任",
        ),
        row_entry(
            SettingsPage::Tools,
            "settings-mcp-effect",
            "settings.nav.tools",
            "MCP",
            "MCP",
            "MCP servers reported by the Host.",
            "Pawork 服务报告的 MCP 服务器。",
            "mcp tools 工具服务器",
        ),
        row_entry(
            SettingsPage::Terminal,
            "settings-terminal-shell-current",
            "settings.terminal.shell_label",
            "Terminal shell",
            "终端 shell",
            "Default shell for new terminals.",
            "新终端的默认 shell。",
            "shell 终端 shell",
        ),
        row_entry(
            SettingsPage::Terminal,
            "settings-terminal-size-current",
            "settings.terminal.size_label",
            "Terminal size",
            "终端尺寸",
            "Default columns and rows for new terminals.",
            "新终端的默认列与行。",
            "size columns rows 尺寸",
        ),
        row_entry(
            SettingsPage::Advanced,
            "settings-advanced-connection",
            "settings.advanced.row_connection",
            "Connection diagnostics",
            "连接诊断",
            "Connection, runtime, and resume diagnostics.",
            "连接、运行时与恢复诊断。",
            "diagnostics connection 连接 诊断",
        ),
        row_entry(
            SettingsPage::About,
            "settings-about-data-dir",
            "settings.about.row_data_dir",
            "Data directory",
            "数据目录",
            "Host data directory from the current handshake.",
            "当前握手声明的 Host 数据目录。",
            "data directory host_data_dir 数据目录",
        ),
    ];
    ENTRIES
}

pub(crate) fn settings_page_title_key(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::Providers => "settings.nav.providers",
        SettingsPage::General => "settings.nav.general",
        SettingsPage::Permissions => "settings.nav.permissions",
        SettingsPage::Tools => "settings.nav.tools",
        SettingsPage::Terminal => "settings.nav.terminal",
        SettingsPage::Appearance => "settings.nav.appearance",
        SettingsPage::Advanced => "settings.nav.advanced",
        SettingsPage::About => "settings.nav.about",
    }
}

pub(crate) fn filter_settings_search_entries(
    query: &str,
    available: impl Fn(SettingsPage) -> bool,
) -> Vec<&'static SettingsSearchEntry> {
    settings_search_entries()
        .iter()
        .filter(|entry| available(entry.page) && entry.matches(query))
        .collect()
}

impl AppView {
    /// Cmd+K / 快捷查找：断线仅 Appearance / Advanced。
    pub(crate) fn settings_page_available(&self, page: SettingsPage) -> bool {
        if matches!(page, SettingsPage::Appearance | SettingsPage::Advanced) {
            return true;
        }
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            return false;
        }
        match page {
            SettingsPage::Providers => true,
            SettingsPage::General => self.projection.settings_general.query.available,
            SettingsPage::Permissions => self.projection.settings_permissions.query.available,
            SettingsPage::Tools => self.resources.available,
            SettingsPage::Terminal => self.projection.settings_terminal.query.available,
            SettingsPage::About => self.settings_about_rows().is_some(),
            SettingsPage::Appearance | SettingsPage::Advanced => true,
        }
    }

    /// Settings 导航与壳内查找：与可见导航项同源（Providers 始终在）。
    pub(crate) fn settings_nav_page_available(&self, page: SettingsPage) -> bool {
        match page {
            SettingsPage::Providers | SettingsPage::Appearance | SettingsPage::Advanced => true,
            SettingsPage::General => self.projection.settings_general.query.available,
            SettingsPage::Permissions => self.projection.settings_permissions.query.available,
            SettingsPage::Tools => self.resources.available,
            SettingsPage::Terminal => self.projection.settings_terminal.query.available,
            SettingsPage::About => self.settings_about_rows().is_some(),
        }
    }

    pub(crate) fn settings_search_query_active(&self) -> bool {
        !self.settings_search_query.trim().is_empty()
    }

    pub(crate) fn settings_search_input_focused(&self, window: &Window) -> bool {
        self.settings_search_focus.is_focused(window)
    }

    pub(crate) fn settings_search_results(&self) -> Vec<&'static SettingsSearchEntry> {
        let query = self.settings_search_query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut rows = Vec::new();
        let mut pages = Vec::new();
        for entry in settings_search_entries() {
            if !self.settings_nav_page_available(entry.page) || !entry.matches(query) {
                continue;
            }
            match entry.kind {
                SettingsSearchKind::Row => rows.push(entry),
                SettingsSearchKind::Page => pages.push(entry),
            }
        }
        rows.extend(pages);
        rows
    }

    fn settings_search_selected_index(&self, results: &[&SettingsSearchEntry]) -> usize {
        self.settings_search_selected
            .as_ref()
            .and_then(|id| results.iter().position(|entry| entry.result_id() == *id))
            .unwrap_or(0)
    }

    fn settings_search_layout(&mut self, id: &str) -> gpui::Stateful<gpui::Div> {
        let layout = self
            .settings_search_layouts
            .entry(id.to_owned())
            .or_default();
        div()
            .id(SharedString::from(id.to_owned()))
            .debug_selector({
                let id = id.to_owned();
                move || id.clone().into()
            })
            .track_scroll(layout)
    }

    fn settings_search_rect(&self, id: &str) -> AxRect {
        self.settings_search_layouts
            .get(id)
            .map(|handle| {
                let bounds = handle.bounds();
                AxRect::new(
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    bounds.size.width.into(),
                    bounds.size.height.into(),
                )
            })
            .unwrap_or_default()
    }

    pub(super) fn settings_search_input_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let _ = cx;
        self.settings_search_layout("settings-search-input-layout")
            .w_full()
            .h(px(metrics::SETTINGS_SEARCH_INPUT_HEIGHT))
            .overflow_hidden()
            .px(px(metrics::RAIL_INNER_PAD))
            .flex()
            .items_center()
            .child(self.settings_search_input.clone())
    }

    pub(super) fn settings_search_results_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let results = self.settings_search_results();
        let selected = self.settings_search_selected_index(&results);
        let rows: Vec<(
            String,
            SettingsPage,
            &'static str,
            &'static str,
            &'static str,
        )> = results
            .iter()
            .map(|entry| {
                (
                    entry.result_id(),
                    entry.page,
                    entry.row_id,
                    entry.display_title(),
                    t(settings_page_title_key(entry.page)),
                )
            })
            .collect();
        self.settings_search_layouts.retain(|id, _| {
            id == "settings-search-input-layout"
                || id == "settings-search-empty"
                || rows.iter().any(|(result_id, _, _, _, _)| result_id == id)
        });
        let mut list = div()
            .id("settings-search-results")
            .w_full()
            .max_h(px(240.0))
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.settings_search_scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
        if rows.is_empty() {
            return list
                .child(
                    self.settings_search_layout("settings-search-empty")
                        .px_3()
                        .py_2()
                        .text_size(font::BODY_SM)
                        .text_color(dark().text.secondary)
                        .child(t("settings.search.empty")),
                )
                .into_any_element();
        }
        for (index, (id, page, row_id, title, detail)) in rows.into_iter().enumerate() {
            let result_id = id.clone();
            let mut row = self
                .settings_search_layout(&id)
                .px_3()
                .py_2()
                .cursor_pointer()
                .hover(|style| style.bg(dark().surface.hover))
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.settings_search_selected = Some(result_id.clone());
                    view.locate_settings_entry(page, row_id, window, cx);
                }))
                .child(
                    div().flex().flex_row().child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().text_size(font::BODY).child(title)),
                    ),
                )
                .child(
                    div().flex().flex_row().child(
                        div().flex_1().min_w_0().child(
                            div()
                                .truncate()
                                .text_size(font::BODY_SM)
                                .text_color(dark().text.secondary)
                                .child(detail),
                        ),
                    ),
                );
            if index == selected {
                row = row.bg(dark().surface.hover);
            }
            list = list.child(row);
        }
        list.into_any_element()
    }

    pub(crate) fn reset_settings_search(&mut self, cx: &mut Context<Self>) {
        self.settings_search_input.update(cx, |input, cx| {
            input.set_text(String::new(), cx);
        });
        self.settings_search_query.clear();
        self.settings_search_selected = None;
        self.settings_search_scroll
            .set_offset(point(px(0.0), px(0.0)));
        self.settings_locate_row = None;
        self.settings_locate_pending = None;
        self.settings_locate_task = None;
    }

    pub(crate) fn clear_settings_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reset_settings_search(cx);
        window.focus(&self.settings_search_focus);
        cx.notify();
    }

    pub(crate) fn submit_settings_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_search_input.read(cx).is_composing() {
            return;
        }
        let results = self.settings_search_results();
        if let Some(entry) = results.get(self.settings_search_selected_index(&results)) {
            self.locate_settings_entry(entry.page, entry.row_id, window, cx);
        }
    }

    pub(crate) fn handle_settings_search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.route != AppRoute::Settings || !self.settings_search_input_focused(window) {
            return false;
        }
        if self.settings_search_input.read(cx).is_composing() {
            return false;
        }
        let key = event.keystroke.key.as_str();
        match key {
            "escape" => {
                self.clear_settings_search(window, cx);
            }
            "up" | "down" if !event.keystroke.modifiers.modified() => {
                let results = self.settings_search_results();
                if results.is_empty() {
                    return true;
                }
                let current = self.settings_search_selected_index(&results);
                let next = if key == "up" {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(results.len() - 1)
                };
                self.settings_search_selected = Some(results[next].result_id());
                self.settings_search_scroll.scroll_to_item(next);
                cx.notify();
            }
            "enter" if !event.keystroke.modifiers.modified() => {
                self.submit_settings_search(window, cx);
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn locate_settings_entry(
        &mut self,
        page: SettingsPage,
        row_id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_nav_page_available(page) && !self.settings_page_available(page) {
            return;
        }
        self.settings_locate_row = Some(row_id.to_string());
        self.settings_locate_pending = Some((page, row_id.to_string(), 0));
        let timer = cx.background_executor().timer(Duration::from_secs(2));
        self.settings_locate_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |view, cx| {
                view.settings_locate_row = None;
                cx.notify();
            });
        }));
        self.on_select_settings_page(page, window, cx);
    }

    pub(crate) fn activate_settings_search_result_id(
        &mut self,
        identifier: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = self
            .settings_search_results()
            .into_iter()
            .find(|entry| entry.result_id() == identifier)
        {
            self.locate_settings_entry(entry.page, entry.row_id, window, cx);
        }
    }

    pub(crate) fn finish_settings_locate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((page, row_id, attempts)) = self.settings_locate_pending.clone() else {
            return;
        };
        if attempts == 0 {
            self.settings_locate_pending = Some((page, row_id, 1));
            cx.defer_in(window, |_view, _window, cx| cx.notify());
            return;
        }
        if self.settings_page != page {
            if attempts >= 8 {
                self.settings_locate_pending = None;
            } else {
                self.settings_locate_pending = Some((page, row_id, attempts + 1));
                cx.defer_in(window, |_view, _window, cx| cx.notify());
            }
            return;
        }
        if self.try_scroll_settings_row(&row_id) {
            self.focus_settings_row(&row_id, window, cx);
            self.settings_locate_pending = None;
            cx.notify();
            return;
        }
        if attempts >= 8 {
            self.settings_locate_pending = None;
            return;
        }
        self.settings_locate_pending = Some((page, row_id, attempts + 1));
        cx.defer_in(window, |_view, _window, cx| cx.notify());
    }

    fn try_scroll_settings_row(&self, row_id: &str) -> bool {
        let Some(handle) = self.settings_element_layouts.get(row_id) else {
            return false;
        };
        let row = handle.bounds();
        if f32::from(row.size.height) <= 0.0 {
            return false;
        }
        let viewport = self.settings_scroll.bounds();
        if f32::from(viewport.size.height) <= 0.0 {
            return false;
        }
        let row_top: f32 = row.origin.y.into();
        let row_bottom = row_top + f32::from(row.size.height);
        let view_top: f32 = viewport.origin.y.into();
        let view_bottom = view_top + f32::from(viewport.size.height);
        let delta = if row_top < view_top {
            row_top - view_top
        } else if row_bottom > view_bottom {
            row_bottom - view_bottom
        } else {
            return true;
        };
        let offset = self.settings_scroll.offset();
        self.settings_scroll
            .set_offset(point(px(0.0), offset.y - px(delta)));
        true
    }

    fn focus_settings_row(&mut self, row_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match row_id {
            "settings-appearance-text-size" => {
                let id = settings_text_scale_identifier(self.text_scale);
                if let Some(focus) = self.settings_appearance_focus.get(id) {
                    window.focus(focus);
                    return;
                }
            }
            "settings-appearance-language" => {
                if let Some(focus) = self
                    .settings_appearance_focus
                    .get(self.language.identifier())
                {
                    window.focus(focus);
                    return;
                }
            }
            "settings-proxy-heading" => {
                window.focus(&self.settings_proxy_input.read(cx).focus_handle(cx));
                return;
            }
            "settings-approval-mode-header" => {
                if let Some(mode) = self.projection.settings_permissions.approval_mode {
                    let id = format!("settings-approval-mode-{}", mode.as_str());
                    if let Some(focus) = self.settings_permissions_focus.get(&id) {
                        window.focus(focus);
                        return;
                    }
                }
            }
            "settings-workspace-trust-status" => {
                if let Some(focus) = self
                    .settings_permissions_focus
                    .get("settings-workspace-trust")
                {
                    window.focus(focus);
                    return;
                }
            }
            "settings-terminal-shell-current" => {
                window.focus(&self.settings_terminal_shell_input.read(cx).focus_handle(cx));
                return;
            }
            "settings-terminal-size-current" => {
                window.focus(
                    &self
                        .settings_terminal_columns_input
                        .read(cx)
                        .focus_handle(cx),
                );
                return;
            }
            "settings-providers-heading"
            | "settings-default-roles"
            | "settings-page-title"
            | "settings-mcp-effect"
            | "settings-advanced-connection"
            | "settings-about-data-dir" => {}
            _ => {}
        }
        window.focus(&self.settings_search_focus);
    }

    pub(crate) fn settings_search_rail_ax(
        &self,
        window: &Window,
        cx: &gpui::App,
        search_rect: AxRect,
    ) -> Vec<AxNode> {
        let mut nodes = vec![AxNode::new(
            "settings-search-input",
            AxRole::TextArea,
            t("settings.search.placeholder"),
            search_rect,
        )
        .value(self.settings_search_input.read(cx).text())
        .focused(self.settings_search_focus.is_focused(window))
        .action(AxAction::Focus)
        .action(AxAction::SetValue)];
        if !self.settings_search_query_active() {
            return nodes;
        }
        let results = self.settings_search_results();
        let selected = self.settings_search_selected_index(&results);
        let viewport = self.settings_search_scroll.bounds();
        if results.is_empty() {
            nodes.push(AxNode::new(
                "settings-search-empty",
                AxRole::StaticText,
                t("settings.search.empty"),
                self.settings_search_rect("settings-search-empty"),
            ));
            return nodes;
        }
        for (index, entry) in results.iter().enumerate() {
            let id = entry.result_id();
            let mut rect = self.settings_search_rect(&id);
            let top = rect.y.max(f32::from(viewport.top()));
            let bottom = (rect.y + rect.height).min(f32::from(viewport.bottom()));
            if bottom <= top {
                continue;
            }
            rect.y = top;
            rect.height = bottom - top;
            nodes.push(
                AxNode::new(id, AxRole::ListItem, entry.display_title(), rect)
                    .value(t(settings_page_title_key(entry.page)))
                    .selected(index == selected)
                    .action(AxAction::Press),
            );
        }
        nodes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_search_entries_cover_required_topics_aliases_and_case() {
        let entries = settings_search_entries();
        assert!(entries
            .iter()
            .any(|e| e.page == SettingsPage::Appearance && e.matches("字号")));
        assert!(entries
            .iter()
            .any(|e| e.page == SettingsPage::Appearance && e.matches("FONT SIZE")));
        assert!(entries
            .iter()
            .any(|e| e.page == SettingsPage::Providers && e.matches("模型")));
        assert!(entries
            .iter()
            .any(|e| e.page == SettingsPage::Providers && e.matches("model")));
        assert!(entries
            .iter()
            .any(|e| e.page == SettingsPage::General && e.matches("代理")));
        assert!(entries
            .iter()
            .any(|e| e.page == SettingsPage::General && e.matches("Proxy")));
        assert!(entries
            .iter()
            .any(|e| e.page == SettingsPage::Permissions && e.matches("审批")));
        assert!(entries
            .iter()
            .any(|e| e.page == SettingsPage::Permissions && e.matches("approval")));
        assert!(entries
            .iter()
            .any(|e| e.kind == SettingsSearchKind::Page && e.page == SettingsPage::Providers));
        assert_eq!(
            entries
                .iter()
                .filter(|e| e.kind == SettingsSearchKind::Page)
                .count(),
            8
        );
    }

    #[test]
    fn settings_search_entries_filter_by_page_gate() {
        let local = |page| matches!(page, SettingsPage::Appearance | SettingsPage::Advanced);
        let hits = filter_settings_search_entries("代理", local);
        assert!(hits.is_empty());
        let font = filter_settings_search_entries("字号", local);
        assert!(font.iter().all(|e| e.page == SettingsPage::Appearance));
        let proxy_all = filter_settings_search_entries("proxy", |_| true);
        assert!(proxy_all.iter().any(|e| e.page == SettingsPage::General));
    }
}
