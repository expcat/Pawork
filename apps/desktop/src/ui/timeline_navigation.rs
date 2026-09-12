//! GUI2-04：只查已加载正文；查找与回合目录共用消息身份定位。
use super::accessibility::{AxAction, AxNode, AxRect, AxRole};
use super::i18n::t;
use super::*;
use crate::projection::{TimelineEntry, TimelineEntryKind, TimelineRow};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::icon::{icon, Icon};
use crate::ui::theme::{dark, font, metrics};

#[derive(Clone, Debug, PartialEq, Eq)]
enum MessageKey {
    Event(String),
    Message(Option<String>, String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NavigationMode {
    Find,
    Turns,
}

pub(super) struct TimelineNavigation {
    mode: Option<NavigationMode>,
    session: Option<String>,
    pub input: Entity<TextInput>,
    pub focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    query: String,
    selected: Option<MessageKey>,
    anchor: Option<MessageKey>,
    pending_jump: bool,
    /// 手动滚动可离开定位点，但只有显式回底才退出定位阅读模式。
    pub reading: bool,
    identities: HashMap<String, MessageKey>,
    focuses: HashMap<String, FocusHandle>,
    scroll: ScrollHandle,
    layouts: HashMap<String, ScrollHandle>,
    focused_turn: Option<String>,
}

impl TimelineNavigation {
    pub fn new(cx: &mut Context<AppView>) -> Self {
        let input = cx.new(|cx| {
            TextInput::with_placeholder(t("find.placeholder"), cx)
                .id("timeline-find-input")
                .height_clamp(32.0, 48.0)
        });
        let focus = input.read(cx).focus_handle(cx).tab_stop(true);
        cx.observe(&input, |view, input, cx| {
            let query = input.read(cx).text().to_owned();
            if view.timeline_navigation.query != query {
                view.timeline_navigation.query = query;
                view.timeline_navigation.selected = None;
                view.timeline_navigation.anchor = None;
                view.timeline_navigation.pending_jump = true;
                cx.notify();
            }
        })
        .detach();
        Self {
            mode: None,
            session: None,
            input,
            focus,
            return_focus: None,
            query: String::new(),
            selected: None,
            anchor: None,
            pending_jump: false,
            reading: false,
            identities: HashMap::new(),
            focuses: HashMap::new(),
            scroll: ScrollHandle::new(),
            layouts: HashMap::new(),
            focused_turn: None,
        }
    }

    pub fn clear(&mut self) {
        self.mode = None;
        self.selected = None;
        self.anchor = None;
        self.query.clear();
        self.identities.clear();
        self.return_focus = None;
        self.pending_jump = false;
        self.reading = false;
        self.focuses.clear();
        self.layouts.clear();
        self.focused_turn = None;
    }

    pub fn user_scrolled(&mut self) {
        self.anchor = None;
    }

    pub fn observe_page(&mut self, page: &pawork_client::TimelinePage) {
        for item in &page.items {
            if let Some(message) = &item.message_id {
                let key = MessageKey::Message(item.run_id.clone(), message.clone());
                if item.kind == pawork_client::TimelineItemKind::UserMessage {
                    if let Some(run) = &item.run_id {
                        let echo = MessageKey::Event(format!("local-echo-{run}"));
                        if self.selected.as_ref() == Some(&echo) {
                            self.selected = Some(key.clone());
                        }
                        if self.anchor.as_ref() == Some(&echo) {
                            self.anchor = Some(key.clone());
                        }
                    }
                }
                self.identities.insert(item.event_id.clone(), key);
            }
        }
    }

    pub fn observe_event(&mut self, envelope: &pawork_client::AppEventEnvelope) {
        if let AppEvent::AssistantDelta {
            run_id, message_id, ..
        } = &envelope.payload
        {
            self.identities.insert(
                envelope.event_id.as_str().into(),
                MessageKey::Message(Some(run_id.as_str().into()), message_id.as_str().into()),
            );
        }
    }

    fn key(&self, entry: &TimelineEntry) -> MessageKey {
        self.identities
            .get(&entry.event_id)
            .cloned()
            .unwrap_or_else(|| MessageKey::Event(entry.event_id.clone()))
    }
}

impl AppView {
    fn navigation_element(&mut self, id: &str) -> gpui::Stateful<gpui::Div> {
        div().id(SharedString::from(id.to_owned())).track_scroll(
            self.timeline_navigation
                .layouts
                .entry(id.to_owned())
                .or_default(),
        )
    }

    pub(super) fn navigation_rect(&self, id: &str) -> AxRect {
        self.timeline_navigation
            .layouts
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
    fn navigation_messages(&self, turns: bool) -> Vec<(MessageKey, String)> {
        let query = self.timeline_navigation.query.trim().to_lowercase();
        self.projection
            .timeline
            .iter()
            .filter_map(|entry| {
                let text = match &entry.kind {
                    TimelineEntryKind::UserMessage { text } => text,
                    TimelineEntryKind::AssistantMessage { text } if !turns => text,
                    _ => return None,
                };
                if text.trim().is_empty()
                    || (!turns && (query.is_empty() || !text.to_lowercase().contains(&query)))
                {
                    return None;
                }
                Some((
                    self.timeline_navigation.key(entry),
                    text.chars()
                        .take(72)
                        .map(|c| if c.is_whitespace() { ' ' } else { c })
                        .collect(),
                ))
            })
            .collect()
    }

    pub(super) fn turns_available(&self) -> bool {
        if self.navigation_messages(true).len() < 2 {
            return false;
        }
        let bounds = self.timeline_list.viewport_bounds();
        let height = f32::from(bounds.size.height);
        if height <= 0.0 {
            return false;
        }
        let width = f32::from(bounds.size.width);
        let rows = self.projection.timeline_rows();
        let total: f32 = rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                timeline::timeline_row_height(
                    row,
                    &self.projection.timeline,
                    width,
                    self.text_scale.rem_pixels(),
                    &self.expanded_timeline_details,
                    self.changes_available_for_active(),
                ) + if i > 0 {
                    timeline::row_top_gap(row)
                } else {
                    0.0
                }
            })
            .sum();
        // 目录展开不改变其入口的判定阈值。
        let panel = if self.timeline_navigation.mode.is_some() {
            self.navigation_rect("timeline-navigation").height
        } else {
            0.0
        };
        total > height + panel
    }

    pub(super) fn sync_navigation_session(&mut self) {
        if self.timeline_navigation.session != self.projection.active_session_id {
            self.timeline_navigation.clear();
            self.timeline_navigation.session = self.projection.active_session_id.clone();
        }
    }

    pub(super) fn on_find_conversation(
        &mut self,
        _: &FindConversation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.route == AppRoute::Workspace && !self.quick_search.open {
            self.open_timeline_navigation(NavigationMode::Find, window, cx);
        }
    }

    fn open_timeline_navigation(
        &mut self,
        mode: NavigationMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.projection.active_session_id.is_none()
            || (mode == NavigationMode::Turns && !self.turns_available())
        {
            return;
        }
        self.sync_navigation_session();
        if self.timeline_navigation.mode.is_none() {
            self.timeline_navigation.return_focus = window.focused(cx);
        }
        self.close_open_menu(cx);
        self.timeline_navigation.mode = Some(mode);
        if mode == NavigationMode::Find {
            let query = self.timeline_navigation.query.clone();
            self.timeline_navigation.input.update(cx, |input, cx| {
                input.set_placeholder(t("find.placeholder"), cx);
                input.set_text(query, cx);
            });
            window.focus(&self.timeline_navigation.focus);
        } else {
            let focus = self.navigation_focus("timeline-nav-close", cx);
            window.focus(&focus);
        }
        cx.notify();
    }

    pub(super) fn close_timeline_navigation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.timeline_navigation.mode = None;
        if let Some(focus) = self.timeline_navigation.return_focus.take() {
            window.focus(&focus);
        }
        cx.notify();
    }

    fn navigation_focus(&mut self, id: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.timeline_navigation
            .focuses
            .entry(id.into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    pub(super) fn navigation_input_focused(&self, window: &Window) -> bool {
        self.timeline_navigation.mode == Some(NavigationMode::Find)
            && self.timeline_navigation.focus.is_focused(window)
    }

    pub(super) fn navigation_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused = self.navigation_input_focused(window)
            || self
                .timeline_navigation
                .focuses
                .iter()
                .any(|(id, f)| id.starts_with("timeline-nav-") && f.is_focused(window));
        if self.timeline_navigation.mode.is_none()
            || !focused
            || self.timeline_navigation.input.read(cx).is_composing()
        {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_timeline_navigation(window, cx),
            "enter"
                if self.navigation_input_focused(window) && !event.keystroke.modifiers.platform =>
            {
                self.step_find(!event.keystroke.modifiers.shift, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    pub(super) fn step_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.timeline_navigation.input.read(cx).is_composing() {
            return;
        }
        let results = self.navigation_messages(false);
        if results.is_empty() {
            return;
        }
        let current = results
            .iter()
            .position(|(key, _)| Some(key) == self.timeline_navigation.selected.as_ref());
        let next = match current {
            None => 0,
            Some(i) if forward => (i + 1) % results.len(),
            Some(i) => (i + results.len() - 1) % results.len(),
        };
        self.locate_message(results[next].0.clone());
        cx.notify();
    }

    fn locate_message(&mut self, key: MessageKey) {
        self.timeline_navigation.selected = Some(key.clone());
        self.timeline_navigation.anchor = Some(key);
        self.timeline_navigation.pending_jump = true;
        self.timeline_navigation.reading = true;
        self.timeline_following = false;
    }

    /// reset 后和显式跳转时重求行号，不缓存位置；新正文提交沿用 message_id。
    pub(super) fn navigation_message_selected(&self, entry: &TimelineEntry) -> bool {
        self.timeline_navigation.mode == Some(NavigationMode::Find)
            && self.timeline_navigation.selected.as_ref()
                == Some(&self.timeline_navigation.key(entry))
    }

    pub(super) fn sync_navigation_location(&mut self, reset: bool) {
        let ids: HashSet<&str> = self
            .projection
            .timeline
            .iter()
            .map(|entry| entry.event_id.as_str())
            .collect();
        self.timeline_navigation
            .identities
            .retain(|id, _| ids.contains(id.as_str()));
        if self.timeline_navigation.mode == Some(NavigationMode::Find) {
            let results = self.navigation_messages(false);
            if !results
                .iter()
                .any(|(key, _)| Some(key) == self.timeline_navigation.selected.as_ref())
            {
                self.timeline_navigation.selected = None;
                if let Some((key, _)) = results.first() {
                    self.locate_message(key.clone());
                } else {
                    self.timeline_navigation.anchor = None;
                }
            }
        }
        if reset || self.timeline_navigation.pending_jump {
            if let Some(key) = &self.timeline_navigation.anchor {
                let rows = self.projection.timeline_rows();
                if let Some(ix) = rows.iter().position(|row| match row {
                    TimelineRow::Message { entry_index } => {
                        self.timeline_navigation
                            .key(&self.projection.timeline[*entry_index])
                            == *key
                    }
                    _ => false,
                }) {
                    self.timeline_list.scroll_to(gpui::ListOffset {
                        item_ix: ix,
                        offset_in_item: Pixels::ZERO,
                    });
                }
            }
            self.timeline_navigation.pending_jump = false;
        }
    }

    fn navigation_button(
        &mut self,
        id: &str,
        label: &str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focus = self.navigation_focus(id, cx);
        let click_id = id.to_owned();
        let key_id = id.to_owned();
        let is_turn = id.starts_with("timeline-nav-turn-");
        let width = f32::from(self.timeline_list.viewport_bounds().size.width).max(200.0);
        self.navigation_element(id).child(
            Button::new(SharedString::from(id.to_owned()))
                .variant(ButtonVariant::Ghost)
                .label(SharedString::from(label.to_owned()))
                .height(px(36.0))
                .max_width(px(if is_turn { width } else { 180.0 }))
                .track_focus(&focus)
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, window, cx| {
                    if !view.consume_button_key_click(&click_id, event) {
                        view.navigation_press(&click_id, window, cx);
                    }
                }))
                .on_activate(cx.listener(move |view, _, window, cx| {
                    view.note_button_key_activate(&key_id);
                    view.navigation_press(&key_id, window, cx);
                    cx.stop_propagation();
                })),
        )
    }

    /// GUI3-07：Header Find / 回合入口为 36×36 图标按钮；tooltip 与 AX
    /// name 仍走 i18n 文案，不用字形当 name。
    fn navigation_header_icon_button(
        &mut self,
        id: &'static str,
        icon_kind: Icon,
        label_key: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focus = self.navigation_focus(id, cx);
        let click_id = id.to_owned();
        let key_id = id.to_owned();
        self.navigation_element(id).child(
            Button::new(SharedString::from(id))
                .variant(ButtonVariant::Ghost)
                .padding(ButtonPadding::None)
                .width(px(metrics::ICON_BUTTON_SIZE))
                .height(px(metrics::ICON_BUTTON_SIZE))
                .center()
                .vcenter()
                .radius(metrics::CONTROL_RADIUS)
                .text_color(dark().text.emphasis)
                .child(icon(icon_kind))
                .tooltip(t(label_key))
                .track_focus(&focus)
                .on_click(cx.listener(move |view, event, window, cx| {
                    if !view.consume_button_key_click(&click_id, event) {
                        view.navigation_press(&click_id, window, cx);
                    }
                }))
                .on_activate(cx.listener(move |view, _, window, cx| {
                    view.note_button_key_activate(&key_id);
                    view.navigation_press(&key_id, window, cx);
                    cx.stop_propagation();
                })),
        )
    }

    fn navigation_icon_button(
        &mut self,
        id: &'static str,
        icon_kind: Icon,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focus = self.navigation_focus(id, cx);
        let click_id = id.to_owned();
        let key_id = id.to_owned();
        self.navigation_element(id).child(
            Button::new(SharedString::from(id))
                .variant(ButtonVariant::Ghost)
                .padding(ButtonPadding::None)
                .width(px(metrics::ICON_BUTTON_SIZE))
                .height(px(metrics::ICON_BUTTON_SIZE))
                .center()
                .vcenter()
                .radius(metrics::CONTROL_RADIUS)
                .child(icon(icon_kind))
                .track_focus(&focus)
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, window, cx| {
                    if !view.consume_button_key_click(&click_id, event) {
                        view.navigation_press(&click_id, window, cx);
                    }
                }))
                .on_activate(cx.listener(move |view, _, window, cx| {
                    view.note_button_key_activate(&key_id);
                    view.navigation_press(&key_id, window, cx);
                    cx.stop_propagation();
                })),
        )
    }

    pub(super) fn navigation_header(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(metrics::HEADER_ACTION_GAP))
            .when(self.projection.active_session_id.is_some(), |row| {
                row.child(self.navigation_header_icon_button(
                    "timeline-find",
                    Icon::Find,
                    "find.title",
                    cx,
                ))
            })
            .when(self.turns_available(), |row| {
                row.child(self.navigation_header_icon_button(
                    "timeline-turns",
                    Icon::Turns,
                    "find.turns",
                    cx,
                ))
            })
    }

    fn find_count(&self) -> String {
        let results = self.navigation_messages(false);
        let current = results
            .iter()
            .position(|(key, _)| Some(key) == self.timeline_navigation.selected.as_ref())
            .map_or(0, |i| i + 1);
        format!("{current} / {}", results.len())
    }

    pub(super) fn navigation_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let mode = self.timeline_navigation.mode?;
        let has_results = !self.navigation_messages(false).is_empty();
        let mut controls = div()
            .flex()
            .items_center()
            .gap_1()
            .when(mode == NavigationMode::Find, |row| {
                row.child(
                    self.navigation_element("timeline-nav-input-layout")
                        .flex_1()
                        .min_w_0()
                        .child(self.timeline_navigation.input.clone()),
                )
                .child(
                    self.navigation_element("timeline-nav-count")
                        .text_size(font::BODY_SM)
                        .child(self.find_count()),
                )
                .child(self.navigation_icon_button(
                    "timeline-nav-prev",
                    Icon::ArrowUp,
                    has_results,
                    cx,
                ))
                .child(self.navigation_icon_button(
                    "timeline-nav-next",
                    Icon::ArrowDown,
                    has_results,
                    cx,
                ))
            })
            .when(mode == NavigationMode::Turns, |row| {
                row.child(div().flex_1().child(t("find.turns")))
            })
            .child(self.navigation_icon_button("timeline-nav-close", Icon::Cancel, true, cx));
        controls = controls.flex_none();
        let scope = self.navigation_scope();
        let mut panel = self
            .navigation_element("timeline-navigation")
            .flex_none()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(controls)
            .child(
                self.navigation_element("timeline-nav-scope")
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(scope),
            );
        if mode == NavigationMode::Turns {
            let mut list = div()
                .id("timeline-turn-list")
                .max_h(px(160.0))
                .overflow_y_scroll()
                .track_scroll(&self.timeline_navigation.scroll)
                .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()));
            for (index, (key, title)) in self.navigation_messages(true).into_iter().enumerate() {
                let id = self.turn_identifier(&key);
                if self
                    .timeline_navigation
                    .focuses
                    .get(&id)
                    .is_some_and(|f| f.is_focused(window))
                    && self.timeline_navigation.focused_turn.as_ref() != Some(&id)
                {
                    self.timeline_navigation.focused_turn = Some(id.clone());
                    self.timeline_navigation.scroll.scroll_to_item(index);
                }
                list = list.child(self.navigation_button(&id, &title, true, cx));
            }
            panel = panel.child(list);
        }
        Some(panel.into_any_element())
    }

    fn turn_identifier(&self, key: &MessageKey) -> String {
        format!("timeline-nav-turn-{key:?}")
    }

    fn navigation_scope(&self) -> &'static str {
        if self.timeline_paging {
            t("find.loading")
        } else if self.timeline_navigation.mode == Some(NavigationMode::Turns) {
            t("find.turn_scope")
        } else {
            t("find.scope")
        }
    }

    pub(super) fn navigation_press(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            "timeline-find" | "timeline-turns" => {
                let mode = if id == "timeline-find" {
                    NavigationMode::Find
                } else {
                    NavigationMode::Turns
                };
                let focus = self.navigation_focus(id, cx);
                window.focus(&focus);
                self.timeline_navigation.return_focus = Some(focus);
                self.open_timeline_navigation(mode, window, cx);
            }
            "timeline-nav-close" => self.close_timeline_navigation(window, cx),
            "timeline-nav-prev" => self.step_find(false, cx),
            "timeline-nav-next" => self.step_find(true, cx),
            _ => {
                if self.timeline_navigation.mode == Some(NavigationMode::Turns) {
                    if let Some((key, _)) = self
                        .navigation_messages(true)
                        .into_iter()
                        .find(|(key, _)| self.turn_identifier(key) == id)
                    {
                        self.locate_message(key);
                        self.close_timeline_navigation(window, cx);
                    }
                }
            }
        }
    }

    pub(super) fn navigation_ax(&self, window: &Window, cx: &App) -> AxNode {
        let mut node = AxNode::new(
            "timeline-navigation",
            AxRole::Group,
            t("find.title"),
            self.navigation_rect("timeline-navigation"),
        );
        let mut controls = Vec::new();
        if self.timeline_navigation.mode == Some(NavigationMode::Find) {
            node = node
                .child(
                    AxNode::new(
                        "timeline-find-input",
                        AxRole::TextArea,
                        t("find.placeholder"),
                        self.navigation_rect("timeline-nav-input-layout"),
                    )
                    .value(self.timeline_navigation.input.read(cx).text())
                    .focused(self.timeline_navigation.focus.is_focused(window))
                    .action(AxAction::Focus)
                    .action(AxAction::SetValue),
                )
                .child(AxNode::new(
                    "timeline-nav-count",
                    AxRole::StaticText,
                    self.find_count(),
                    self.navigation_rect("timeline-nav-count"),
                ));
            let enabled = !self.navigation_messages(false).is_empty();
            controls.extend([
                (
                    "timeline-nav-prev".into(),
                    t("find.previous").into(),
                    enabled,
                ),
                ("timeline-nav-next".into(), t("find.next").into(), enabled),
            ]);
        } else if self.timeline_navigation.mode == Some(NavigationMode::Turns) {
            controls.extend(
                self.navigation_messages(true)
                    .into_iter()
                    .map(|(key, text)| (self.turn_identifier(&key), text, true)),
            );
        }
        controls.push(("timeline-nav-close".into(), t("find.close").into(), true));
        for (id, label, enabled) in controls {
            let mut rect = self.navigation_rect(&id);
            if id.starts_with("timeline-nav-turn-") {
                let bounds = self.timeline_navigation.scroll.bounds();
                let top = rect.y.max(bounds.top().into());
                let bottom = (rect.y + rect.height).min(bounds.bottom().into());
                rect.y = top;
                rect.height = (bottom - top).max(0.0);
            }
            if rect.height <= 0.0 {
                continue;
            }
            let mut button = AxNode::new(&id, AxRole::Button, label, rect)
                .enabled(enabled)
                .focused(
                    self.timeline_navigation
                        .focuses
                        .get(&id)
                        .is_some_and(|f| f.is_focused(window)),
                );
            if enabled {
                button = button.action(AxAction::Press);
            }
            node = node.child(button);
        }
        node.child(AxNode::new(
            "timeline-nav-scope",
            AxRole::StaticText,
            self.navigation_scope(),
            self.navigation_rect("timeline-nav-scope"),
        ))
    }

    pub(super) fn navigation_open(&self) -> bool {
        self.timeline_navigation.mode.is_some()
    }
    pub(super) fn navigation_header_ax(&self, window: &Window) -> Vec<AxNode> {
        [
            (
                "timeline-find",
                "find.title",
                self.projection.active_session_id.is_some(),
            ),
            ("timeline-turns", "find.turns", self.turns_available()),
        ]
        .into_iter()
        .filter(|(_, _, enabled)| *enabled)
        .map(|(id, label, _)| {
            AxNode::new(id, AxRole::Button, t(label), self.navigation_rect(id))
                .action(AxAction::Press)
                .focused(
                    self.timeline_navigation
                        .focuses
                        .get(id)
                        .is_some_and(|f| f.is_focused(window)),
                )
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[gpui::test]
    fn conversation_find_identity_reading_and_turns(cx: &mut gpui::TestAppContext) {
        let platform = Arc::new(Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                "/tmp/conversation-find-test.sock".into(),
                None,
                cx,
            )
        });
        cx.update(|window, cx| {
            install_keybindings(cx);
            view.update(cx, |v, cx| {
                v.projection.active_session_id = Some("s-1".into());
                v.sync_navigation_session();
                v.text_input
                    .update(cx, |input, cx| input.set_text("保留草稿", cx));
                for i in 0..12 {
                    v.projection.timeline.entries.push(TimelineEntry {
                        sequence: i,
                        event_id: format!("u-{i}"),
                        run_id: Some(format!("r-{i}")),
                        kind: TimelineEntryKind::UserMessage {
                            text: format!("第 {i} 回合 中文 MATCH {}", "正文 ".repeat(30)),
                        },
                        timestamp: "1000".into(),
                        fork_boundary: None,
                    });
                }
                v.timeline_paging = true;
                v.timeline_changed();
                v.focus_composer(window, cx);
            });
        });
        cx.refresh().unwrap();
        cx.simulate_keystrokes("cmd-f");
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |v, cx| {
                v.timeline_navigation
                    .input
                    .update(cx, |i, cx| i.set_text("match", cx))
            })
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        for (width, height) in [(1440., 1024.), (1080., 720.)] {
            for scale in [
                font::TextScale::Percent100,
                font::TextScale::Percent125,
                font::TextScale::Percent150,
            ] {
                cx.simulate_resize(gpui::size(px(width), px(height)));
                cx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.text_scale = scale;
                        window.set_rem_size(px(scale.rem_pixels()));
                        cx.notify();
                    })
                });
                cx.refresh().unwrap();
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let v = view.read(cx);
                    assert_eq!(v.navigation_messages(false).len(), 12);
                    assert!(v.turns_available());
                    assert!(!v.timeline_following);
                    let tree = super::super::accessibility::AxTree::new(width, height)
                        .child(v.navigation_ax(window, cx));
                    assert!(tree.validate().is_ok(), "{:?}", tree.validate());
                    for id in [
                        "timeline-find-input",
                        "timeline-nav-count",
                        "timeline-nav-prev",
                        "timeline-nav-next",
                        "timeline-nav-close",
                    ] {
                        let b = tree.children[0]
                            .children
                            .iter()
                            .find(|n| n.identifier == id)
                            .unwrap()
                            .bounds;
                        assert!(
                            b.width > 0.
                                && b.height > 0.
                                && b.x >= 0.
                                && b.y >= 0.
                                && b.x + b.width <= width
                                && b.y + b.height <= height,
                            "{id}: {b:?}"
                        );
                    }
                    assert_eq!(
                        tree.children[0]
                            .children
                            .iter()
                            .find(|n| n.identifier == "timeline-nav-scope")
                            .unwrap()
                            .label,
                        t("find.loading")
                    );
                    assert_eq!(v.text_input.read(cx).text(), "保留草稿");
                });
            }
        }
        cx.simulate_keystrokes("enter");
        cx.refresh().unwrap();
        cx.update(|_, cx| {
            let v = view.read(cx);
            assert_eq!(v.find_count(), "2 / 12");
        });
        cx.simulate_keystrokes("shift-enter");
        cx.refresh().unwrap();
        cx.update(|_, cx| {
            let v = view.read(cx);
            assert_eq!(v.find_count(), "1 / 12");
        });
        cx.simulate_keystrokes("escape");
        cx.refresh().unwrap();
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                assert!(!v.navigation_open());
                assert!(v.composer_focus_handle(cx).is_focused(window));
                assert!(!v.timeline_following);
                v.open_timeline_navigation(NavigationMode::Turns, window, cx);
            })
        });
        cx.refresh().unwrap();
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                let (key, _) = v.navigation_messages(true)[5].clone();
                v.navigation_press(&v.turn_identifier(&key), window, cx);
            })
        });
        cx.refresh().unwrap();
        cx.update(|_, cx| {
            view.update(cx, |v, _| {
                assert_eq!(v.timeline_list.logical_scroll_top().item_ix, 5);
                // 行插入与宽度/折叠 reset 仍按身份回到同一消息。
                v.projection.timeline.entries.insert(
                    0,
                    TimelineEntry {
                        sequence: 0,
                        event_id: "tool".into(),
                        run_id: None,
                        kind: TimelineEntryKind::Thinking {
                            text: "match excluded".into(),
                        },
                        timestamp: "1".into(),
                        fork_boundary: None,
                    },
                );
                v.timeline_changed();
            })
        });
        cx.refresh().unwrap();
        cx.update(|window, cx| view.update(cx, |v, cx| {
            assert_eq!(v.timeline_list.logical_scroll_top().item_ix, 6);
            // 用真实 reducer 验证 delta → committed 的事件身份替换。
            let delta: pawork_client::AppEventEnvelope = serde_json::from_value(json!({
                "api_version":{"major":1,"minor":1},"instance_id":"test","event_id":"delta-20","global_sequence":20,
                "stream":{"type":"session","id":"s-1"},"stream_sequence":20,"timestamp":1020,"source":{"type":"core"},
                "payload":{"type":"assistant_delta","data":{"run_id":"r-live","message_id":"m-live","delta":"流式 needle"}}
            })).unwrap();
            v.timeline_navigation.observe_event(&delta);
            v.projection.apply_event(&delta);
            v.open_timeline_navigation(NavigationMode::Find, window, cx);
            v.timeline_navigation.input.update(cx, |i, cx| i.set_text("needle", cx));
            v.timeline_changed();
        }));
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| view.update(cx, |v, _| {
            assert_eq!(v.navigation_messages(false).len(), 1);
            let key = v.timeline_navigation.selected.clone();
            let page: pawork_client::TimelinePage = serde_json::from_value(json!({"items":[{
                "sequence":21,"event_id":"committed-21","kind":"assistant_message","run_id":"r-live","message_id":"m-live","timestamp":"1021","text":"流式 needle 完成"}],"head_sequence":21,"complete":true})).unwrap();
            v.timeline_navigation.observe_page(&page);
            v.projection.apply_timeline_page(&page);
            assert_eq!(v.navigation_messages(false)[0].0, key.unwrap());
            v.timeline_changed();
        }));
        cx.refresh().unwrap();
        cx.update(|window, cx| {
            view.update(cx, |v, cx| {
                assert!(!v.timeline_following);
                assert_eq!(v.navigation_messages(false).len(), 1);
                v.timeline_navigation
                    .input
                    .update(cx, |i, cx| i.set_text("不存在的内容", cx));
                v.timeline_paging = false;
                v.close_timeline_navigation(window, cx);
                v.timeline_jump_to_bottom();
                assert!(v.timeline_following && !v.timeline_navigation.reading);
                v.projection.active_session_id = Some("s-2".into());
                v.sync_navigation_session();
                assert!(
                    !v.navigation_open()
                        && v.timeline_navigation.selected.is_none()
                        && v.timeline_navigation.anchor.is_none()
                );
                assert_eq!(v.text_input.read(cx).text(), "保留草稿");
            })
        });
    }
}
