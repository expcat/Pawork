//! GUI2-03：仅搜索当前 Snapshot 与已有安全导航；不查询历史、不执行写操作。
use super::accessibility::{AxAction, AxNode, AxRect, AxRole, AxTree};
use super::i18n::t;
use super::settings::{settings_page_title_key, settings_search_entries, SettingsSearchKind};
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SearchTarget {
    Task(String),
    Page(SettingsPage),
    SettingsRow {
        page: SettingsPage,
        row_id: &'static str,
    },
    Panel(InspectorTab),
}

#[derive(Clone)]
pub(super) struct SearchResult {
    pub target: SearchTarget,
    pub title: String,
    pub detail: String,
    pub group: &'static str,
}

impl SearchResult {
    fn id(&self) -> String {
        match &self.target {
            SearchTarget::Task(id) => format!("quick-task-{id}"),
            SearchTarget::Page(page) => format!("quick-page-{page:?}"),
            SearchTarget::SettingsRow { page, row_id } => {
                format!("quick-settings-{page:?}-{row_id}")
            }
            SearchTarget::Panel(tab) => format!("quick-panel-{tab:?}"),
        }
    }

    fn type_icon(&self) -> Icon {
        match &self.target {
            SearchTarget::Task(_) => Icon::Task,
            SearchTarget::Page(_) => Icon::Page,
            SearchTarget::SettingsRow { .. } => Icon::SettingsRow,
            SearchTarget::Panel(InspectorTab::Changes) => Icon::Changes,
            SearchTarget::Panel(InspectorTab::Terminal) => Icon::Terminal,
            SearchTarget::Panel(InspectorTab::Resources) => Icon::Resources,
        }
    }
}

fn quick_search_accent_mark() -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .w(px(metrics::SETTINGS_LOCATE_MARK_WIDTH))
        .bg(dark().accent.primary)
}

// 页面标题与设置行均来自 settings_search_entries()；Cmd+K 断线 gate 仍只允许 Appearance / Advanced。

pub(super) struct QuickSearch {
    pub open: bool,
    pub input: Entity<TextInput>,
    pub focus: FocusHandle,
    pub trigger: FocusHandle,
    return_focus: Option<FocusHandle>,
    query: String,
    selected: Option<SearchTarget>,
    scroll: ScrollHandle,
    layouts: HashMap<String, ScrollHandle>,
}

impl QuickSearch {
    pub fn new(cx: &mut Context<AppView>) -> Self {
        let input = cx.new(|cx| {
            TextInput::with_placeholder(t("quick.placeholder"), cx)
                .id("quick-search-input")
                .height_clamp(32.0, 48.0)
        });
        let focus = input.read(cx).focus_handle(cx).tab_stop(true);
        cx.observe(&input, |view, input, cx| {
            let query = input.read(cx).text().to_owned();
            if view.quick_search.query != query {
                view.quick_search.query = query;
                view.quick_search.selected = None;
                view.quick_search.scroll.set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            }
        })
        .detach();
        Self {
            open: false,
            input,
            focus,
            trigger: cx.focus_handle().tab_stop(true).tab_index(-21),
            return_focus: None,
            query: String::new(),
            selected: None,
            scroll: ScrollHandle::new(),
            layouts: HashMap::new(),
        }
    }

    fn element(&mut self, id: &str) -> gpui::Stateful<gpui::Div> {
        let layout = self.layouts.entry(id.to_owned()).or_default();
        div()
            .id(SharedString::from(id.to_owned()))
            .track_scroll(layout)
    }

    fn rect(&self, id: &str) -> AxRect {
        self.layouts
            .get(id)
            .map(|s| {
                let b = s.bounds();
                AxRect::new(
                    b.origin.x.into(),
                    b.origin.y.into(),
                    b.size.width.into(),
                    b.size.height.into(),
                )
            })
            .unwrap_or_default()
    }
}

impl AppView {
    fn search_page_available(&self, page: SettingsPage) -> bool {
        self.settings_page_available(page)
    }

    pub(super) fn quick_search_results(&self) -> Vec<SearchResult> {
        let query = self.quick_search.query.trim();
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let mut results = Vec::new();
        if connected {
            let matches = |text: &str| {
                query.is_empty() || text.to_lowercase().contains(&query.to_lowercase())
            };
            let mut tasks = self.projection.sessions.iter().collect::<Vec<_>>();
            tasks.sort_by(|a, b| {
                b.updated_at_ms
                    .cmp(&a.updated_at_ms)
                    .then_with(|| a.session_id.cmp(&b.session_id))
            });
            for task in tasks {
                let project = self.projection.workspace_name(task.workspace_id.as_deref());
                let title = i18n::session_title(&task.title).to_owned();
                if matches(&format!("{title} {project}")) {
                    results.push(SearchResult {
                        target: SearchTarget::Task(task.session_id.clone()),
                        title,
                        detail: project,
                        group: "quick.tasks",
                    });
                }
            }
        }
        for entry in settings_search_entries() {
            if !self.search_page_available(entry.page) {
                continue;
            }
            match entry.kind {
                SettingsSearchKind::Page => {
                    if query.is_empty() || entry.matches(query) {
                        results.push(SearchResult {
                            target: SearchTarget::Page(entry.page),
                            title: entry.display_title().into(),
                            detail: t("quick.settings").into(),
                            group: "quick.pages",
                        });
                    }
                }
                SettingsSearchKind::Row => {
                    if !query.is_empty() && entry.matches(query) {
                        results.push(SearchResult {
                            target: SearchTarget::SettingsRow {
                                page: entry.page,
                                row_id: entry.row_id,
                            },
                            title: entry.display_title().into(),
                            detail: t(settings_page_title_key(entry.page)).into(),
                            group: "quick.pages",
                        });
                    }
                }
            }
        }
        if connected {
            let matches = |text: &str| {
                query.is_empty() || text.to_lowercase().contains(&query.to_lowercase())
            };
            for (tab, key, aliases) in [
                (InspectorTab::Changes, "quick.changes", "changes diff 变更"),
                (InspectorTab::Terminal, "quick.terminal", "terminal 终端"),
                (
                    InspectorTab::Resources,
                    "quick.resources",
                    "resources mcp 资源",
                ),
            ] {
                if matches(&format!("{} {aliases}", t(key))) {
                    results.push(SearchResult {
                        target: SearchTarget::Panel(tab),
                        title: t(key).into(),
                        detail: t("quick.panel").into(),
                        group: "quick.actions",
                    });
                }
            }
        }
        results
    }

    pub(super) fn open_quick_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_search.open {
            return;
        }
        if self.navigation_open() {
            self.close_timeline_navigation(window, cx);
        }
        self.quick_search.return_focus = window.focused(cx);
        self.close_open_menu(cx);
        self.quick_search.input.update(cx, |input, cx| {
            input.set_placeholder(t("quick.placeholder"), cx);
            input.set_text(String::new(), cx);
        });
        self.quick_search.query.clear();
        self.quick_search.selected = None;
        self.quick_search.scroll.set_offset(point(px(0.0), px(0.0)));
        self.quick_search.open = true;
        window.focus(&self.quick_search.focus);
        cx.notify();
    }

    pub(super) fn on_quick_search(
        &mut self,
        _: &OpenQuickSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quick_search.open {
            self.close_quick_search(window, cx);
        } else {
            self.open_quick_search(window, cx);
        }
    }

    pub(super) fn close_quick_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quick_search.open = false;
        if let Some(focus) = self.quick_search.return_focus.take() {
            window.focus(&focus);
        } else {
            window.focus(&self.focus_handle);
        }
        cx.notify();
    }

    fn quick_selected_index(&self, results: &[SearchResult]) -> usize {
        self.quick_search
            .selected
            .as_ref()
            .and_then(|target| results.iter().position(|r| &r.target == target))
            .unwrap_or(0)
    }

    pub(super) fn activate_quick_result(
        &mut self,
        target: SearchTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 重查当前结果与连接 gate；旧鼠标/AX 节点不能绕过断线或 Snapshot 更新。
        if !self.quick_search.open
            || !self
                .quick_search_results()
                .iter()
                .any(|r| r.target == target)
        {
            return;
        }
        self.quick_search.open = false;
        self.quick_search.return_focus = None;
        match target {
            SearchTarget::Task(id) => {
                let workspace = self
                    .projection
                    .sessions
                    .iter()
                    .find(|s| s.session_id == id)
                    .and_then(|s| s.workspace_id.clone());
                let clear_filter = self
                    .scope_workspace_id
                    .as_ref()
                    .is_some_and(|scope| Some(scope) != workspace.as_ref());
                if self.route == AppRoute::Settings {
                    self.on_close_settings(window, cx);
                }
                if clear_filter {
                    self.scope_workspace_id = None;
                }
                self.collapsed_projects
                    .remove(&rail_project_key(workspace.as_deref()));
                self.on_session_clicked(&id, window, cx);
                self.rail_scroll_to_active = true;
                if clear_filter {
                    self.status_hint = Some(t("quick.filter_cleared").into());
                }
            }
            SearchTarget::Page(page) => {
                self.on_open_settings(window, cx);
                self.on_select_settings_page(page, window, cx);
            }
            SearchTarget::SettingsRow { page, row_id } => {
                self.on_open_settings(window, cx);
                self.locate_settings_entry(page, row_id, window, cx);
                cx.notify();
                return;
            }
            SearchTarget::Panel(tab) => {
                if self.route == AppRoute::Settings {
                    self.on_close_settings(window, cx);
                }
                self.select_inspector_tab(tab, cx);
                if !self.inspector_open {
                    self.on_toggle_inspector(window, cx);
                }
            }
        }
        // AppKit Return 可重复投递；导航后不让同一按键激活 Back / Composer。
        window.focus(&self.focus_handle);
        cx.notify();
    }

    pub(super) fn handle_quick_search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.quick_search.open {
            return;
        }
        let key = event.keystroke.key.as_str();
        if self.quick_search.input.read(cx).is_composing() {
            return;
        }
        match key {
            "escape" => self.close_quick_search(window, cx),
            "tab" => window.focus(&self.quick_search.focus),
            "up" | "down" if !event.keystroke.modifiers.modified() => {
                let results = self.quick_search_results();
                if !results.is_empty() {
                    let current = self.quick_selected_index(&results);
                    let next = if key == "up" {
                        current.saturating_sub(1)
                    } else {
                        (current + 1).min(results.len() - 1)
                    };
                    self.quick_search.selected = Some(results[next].target.clone());
                    // 每个分组标题占一个列表 child。
                    let groups = results[..=next]
                        .iter()
                        .enumerate()
                        .filter(|(i, r)| *i == 0 || results[*i - 1].group != r.group)
                        .count();
                    self.quick_search.scroll.scroll_to_item(next + groups);
                    cx.notify();
                }
            }
            "enter" if !event.keystroke.modifiers.modified() => {
                self.submit_quick_search(window, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    pub(super) fn submit_quick_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_search.input.read(cx).is_composing() {
            return;
        }
        let results = self.quick_search_results();
        if let Some(result) = results.get(self.quick_selected_index(&results)) {
            self.activate_quick_result(result.target.clone(), window, cx);
        }
    }

    pub(super) fn quick_search_element(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let results = self.quick_search_results();
        let selected = self.quick_selected_index(&results);
        self.quick_search.layouts.retain(|id, _| {
            !id.starts_with("quick-task-") || results.iter().any(|r| r.id() == *id)
        });
        let mut list = div()
            .id("quick-search-results")
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.quick_search.scroll)
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
        let mut group = "";
        for (index, result) in results.iter().enumerate() {
            if result.group != group {
                group = result.group;
                list = list.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(font::BODY_SM)
                        .text_color(dark().text.secondary)
                        .child(t(group)),
                );
            }
            let target = result.target.clone();
            let type_icon = result.type_icon();
            let highlighted = index == selected;
            list = list.child(
                self.quick_search
                    .element(&result.id())
                    .relative()
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .bg(if highlighted {
                        dark().surface.hover
                    } else {
                        dark().surface.raised
                    })
                    .hover(|s| s.bg(dark().surface.hover))
                    .when(highlighted, |row| row.child(quick_search_accent_mark()))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.activate_quick_result(target.clone(), window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(px(metrics::SPACE_2))
                            .child(
                                icon_sized(type_icon, px(metrics::ICON_SM))
                                    .text_color(dark().text.secondary)
                                    .mt(px(2.0)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div().flex().flex_row().child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(font::BODY)
                                                .child(result.title.clone()),
                                        ),
                                    )
                                    .child(
                                        div().flex().flex_row().child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(font::BODY_SM)
                                                .text_color(dark().text.secondary)
                                                .child(result.detail.clone()),
                                        ),
                                    ),
                            ),
                    ),
            );
        }
        if results.is_empty() {
            list = list
                .child(
                    self.quick_search
                        .element("quick-search-empty")
                        .p_3()
                        .text_size(font::BODY_SM)
                        .child(t("quick.empty")),
                )
                .child(
                    self.quick_search
                        .element("quick-search-clear")
                        .p_3()
                        .cursor_pointer()
                        .text_size(font::BODY_SM)
                        .child(t("quick.clear"))
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.quick_search
                                .input
                                .update(cx, |input, cx| input.set_text(String::new(), cx));
                            window.focus(&view.quick_search.focus);
                        })),
                );
        }
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let panel = self
            .quick_search
            .element("quick-search-dialog")
            .w(px(
                640.0_f32.min(f32::from(window.viewport_size().width) - 48.0)
            ))
            .max_h(window.viewport_size().height * 0.6)
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(dark().surface.raised)
            .border_1()
            .border_color(dark().border.strong)
            .rounded_lg()
            .shadow_lg()
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex_none()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_size(font::TITLE).child(t("quick.title")))
                            .child(
                                self.quick_search
                                    .element("quick-search-close")
                                    .px_2()
                                    .cursor_pointer()
                                    .text_size(font::BODY_SM)
                                    .child("Esc")
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.close_quick_search(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        self.quick_search
                            .element("quick-search-input-layout")
                            .child(self.quick_search.input.clone()),
                    )
                    .child(
                        self.quick_search
                            .element("quick-search-scope")
                            .text_size(font::BODY_SM)
                            .text_color(dark().text.secondary)
                            .child(t(if connected {
                                "quick.scope"
                            } else {
                                "quick.offline"
                            })),
                    ),
            )
            .child(list);
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .justify_center()
            .items_start()
            .pt(window.viewport_size().height * 0.12)
            .bg(gpui::rgba(0x00000088))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|view, _, window, cx| view.close_quick_search(window, cx)),
            )
            .child(panel)
            .into_any_element()
    }

    pub(super) fn quick_search_ax(&self, window: &Window, cx: &App) -> AxTree {
        let q = &self.quick_search;
        let mut dialog = AxNode::new(
            "quick-search-dialog",
            AxRole::Group,
            t("quick.title"),
            q.rect("quick-search-dialog"),
        )
        .child(
            AxNode::new(
                "quick-search-input",
                AxRole::TextArea,
                t("quick.placeholder"),
                q.rect("quick-search-input-layout"),
            )
            .value(q.input.read(cx).text())
            .focused(q.focus.is_focused(window))
            .action(AxAction::Focus)
            .action(AxAction::SetValue),
        )
        .child(
            AxNode::new(
                "quick-search-close",
                AxRole::Button,
                t("quick.close"),
                q.rect("quick-search-close"),
            )
            .action(AxAction::Press),
        )
        .child(AxNode::new(
            "quick-search-scope",
            AxRole::StaticText,
            t(
                if matches!(
                    self.projection.connection,
                    ConnectionState::Connected { .. }
                ) {
                    "quick.scope"
                } else {
                    "quick.offline"
                },
            ),
            q.rect("quick-search-scope"),
        ));
        let results = self.quick_search_results();
        let selected = self.quick_selected_index(&results);
        let viewport = q.scroll.bounds();
        for (i, result) in results.iter().enumerate() {
            let mut rect = q.rect(&result.id());
            let top = rect.y.max(viewport.top().into());
            let bottom = (rect.y + rect.height).min(viewport.bottom().into());
            if bottom <= top {
                continue;
            }
            rect.y = top;
            rect.height = bottom - top;
            dialog = dialog.child(
                AxNode::new(result.id(), AxRole::ListItem, &result.title, rect)
                    .value(&result.detail)
                    .selected(i == selected)
                    .action(AxAction::Press),
            );
        }
        if results.is_empty() {
            dialog = dialog
                .child(AxNode::new(
                    "quick-search-empty",
                    AxRole::StaticText,
                    t("quick.empty"),
                    q.rect("quick-search-empty"),
                ))
                .child(
                    AxNode::new(
                        "quick-search-clear",
                        AxRole::Button,
                        t("quick.clear"),
                        q.rect("quick-search-clear"),
                    )
                    .action(AxAction::Press),
                );
        }
        AxTree::new(
            window.viewport_size().width.into(),
            window.viewport_size().height.into(),
        )
        .child(dialog)
    }

    pub(super) fn quick_search_press(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            "quick-search-close" => self.close_quick_search(window, cx),
            "quick-search-clear" => self
                .quick_search
                .input
                .update(cx, |input, cx| input.set_text(String::new(), cx)),
            _ => {
                if let Some(result) = self
                    .quick_search_results()
                    .into_iter()
                    .find(|r| r.id() == id)
                {
                    self.activate_quick_result(result.target, window, cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::accessibility::AxRequest;
    use super::*;
    use crate::projection::{SessionSummary, WorkspaceSummary};

    fn populate(view: &mut AppView) {
        view.projection.set_connection(ConnectionState::Connected {
            instance_id: "test".into(),
        });
        view.projection.workspaces = vec![
            WorkspaceSummary {
                id: "a".into(),
                name: "中文项目".into(),
            },
            WorkspaceSummary {
                id: "b".into(),
                name: "Beta".into(),
            },
        ];
        view.projection.sessions = (0..40)
            .map(|i| SessionSummary {
                session_id: format!("s-{i:02}"),
                title: if i < 2 {
                    "同名 Task".into()
                } else {
                    format!("Task {i:02}")
                },
                workspace_id: Some(if i == 0 { "a" } else { "b" }.into()),
                updated_at_ms: 100 - i,
                parent_branch_id: None,
                forked_from_event_id: None,
                active: false,
            })
            .collect();
        view.projection.active_session_id = Some("s-00".into());
    }

    #[gpui::test]
    fn quick_find_navigation_layout_and_drafts(cx: &mut gpui::TestAppContext) {
        let platform = Arc::new(Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(platform, "/tmp/quick-find-test.sock".into(), None, cx)
        });
        cx.update(|window, cx| {
            install_keybindings(cx);
            view.update(cx, |view, cx| {
                populate(view);
                view.text_input
                    .update(cx, |input, cx| input.set_text("保留草稿", cx));
                view.focus_composer(window, cx);
            });
        });
        cx.refresh().unwrap();
        cx.simulate_keystrokes("cmd-k");
        cx.run_until_parked();
        for (width, height) in [(1440.0, 1024.0), (1080.0, 720.0)] {
            for scale in [
                font::TextScale::Percent100,
                font::TextScale::Percent125,
                font::TextScale::Percent150,
            ] {
                cx.simulate_resize(gpui::size(px(width), px(height)));
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.text_scale = scale;
                        window.set_rem_size(px(scale.rem_pixels()));
                        cx.notify();
                    })
                });
                cx.refresh().unwrap();
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let v = view.read(cx);
                    assert!(v.quick_search.open);
                    let tree = v.quick_search_ax(window, cx);
                    assert!(tree.validate().is_ok());
                    let b = tree.children[0].bounds;
                    assert!(
                        b.width > 0.0 && b.width <= 640.0 && b.height <= height * 0.6 + 1.0,
                        "{b:?}"
                    );
                    assert!(b.y + b.height <= height);
                    let results = v.quick_search_results();
                    assert_eq!(results[0].target, SearchTarget::Task("s-00".into()));
                    assert_ne!(results[0].detail, results[1].detail);
                    assert!(tree.children[0]
                        .children
                        .iter()
                        .all(|node| node.identifier != "quick-task-s-39"));
                });
            }
        }
        for _ in 0..30 {
            cx.simulate_keystrokes("down");
        }
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let v = view.read(cx);
            assert!(v.quick_search_ax(window, cx).children[0]
                .children
                .iter()
                .any(|node| node.identifier == "quick-task-s-30"));
            assert_eq!(v.projection.active_session_id.as_deref(), Some("s-00"));
        });
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                assert!(!v.quick_search.open);
                assert!(v.text_input.read(cx).focus_handle(cx).is_focused(window));
                v.open_quick_search(window, cx);
                v.quick_search
                    .input
                    .update(cx, |input, cx| input.set_text("中文", cx));
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |v, cx| {
                assert_eq!(v.quick_search_results().len(), 1);
                v.quick_search
                    .input
                    .update(cx, |input, cx| input.set_text("tAsK", cx));
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |v, cx| {
                assert_eq!(v.quick_search_results().len(), 40);
                v.quick_search
                    .input
                    .update(cx, |input, cx| input.set_text("不存在", cx));
            })
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| {
            view.update(cx, |v, cx| {
                assert!(v.quick_search.open);
                assert_eq!(v.text_input.read(cx).text(), "保留草稿");
                assert_eq!(v.projection.active_session_id.as_deref(), Some("s-00"));
                v.scope_workspace_id = Some("a".into());
                v.quick_search
                    .input
                    .update(cx, |input, cx| input.set_text("Beta", cx));
            })
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| {
            let v = view.read(cx);
            assert!(!v.quick_search.open);
            assert_eq!(v.projection.active_session_id.as_deref(), Some("s-01"));
            assert!(v.scope_workspace_id.is_none());
            assert_eq!(v.composer_drafts["s-00"], "保留草稿");
            assert!(v
                .status_hint
                .as_ref()
                .is_some_and(|s| s == t("quick.filter_cleared")));
        });
    }

    #[gpui::test]
    fn quick_find_disconnect_rejects_stale_actions(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                Arc::new(Platform::new()),
                "/tmp/quick-find-offline.sock".into(),
                None,
                cx,
            )
        });
        cx.update(|window, cx| {
            install_keybindings(cx);
            view.update(cx, |v, cx| {
                populate(v);
                v.text_input
                    .update(cx, |input, cx| input.set_text("不要发送", cx));
                v.open_quick_search(window, cx);
                v.projection.set_connection(ConnectionState::Disconnected {
                    reason: "offline".into(),
                });
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        for key in [
            "cmd-n",
            "cmd-enter",
            "cmd-1",
            "cmd-2",
            "cmd-3",
            "cmd-.",
            "cmd-i",
            "cmd-alt-down",
        ] {
            cx.simulate_keystrokes(key);
        }
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                assert!(v.quick_search.open);
                assert_eq!(v.quick_search_results().len(), 2);
                assert!(v.quick_search_results().iter().all(|r| matches!(
                    r.target,
                    SearchTarget::Page(SettingsPage::Appearance | SettingsPage::Advanced)
                )));
                v.activate_quick_result(SearchTarget::Task("s-01".into()), window, cx);
                v.handle_accessibility_request(
                    AxRequest {
                        identifier: "add-task".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
                assert!(v.quick_search.open);
                assert_eq!(v.projection.active_session_id.as_deref(), Some("s-00"));
                assert_eq!(v.projection.sessions.len(), 40);
                assert_eq!(v.text_input.read(cx).text(), "不要发送");
                assert!(!v.inspector_open);
                v.activate_quick_result(SearchTarget::Page(SettingsPage::Advanced), window, cx);
                assert_eq!(v.route, AppRoute::Settings);
                assert_eq!(v.settings_page, SettingsPage::Advanced);
                assert_eq!(v.text_input.read(cx).text(), "不要发送");
            })
        });
    }
}
