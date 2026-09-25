//! Skills are recorded from durable operations; plugin records stay distinct from MCP.
use super::accessibility::{AxRequest, AxRole};
use super::product_access::{edit_input, PanelAccess};
use super::*;
use gpui::AnyElement;
use gpui::{size, Bounds, WindowBounds, WindowOptions};
use pawork_client::{AppCommand, AppQuery};
use std::collections::BTreeSet;

struct RecordingView {
    access: PanelAccess,
    owner: gpui::WeakEntity<AppView>,
    controller: Arc<DesktopController>,
    session: Option<String>,
    workspace: Option<String>,
    plugins: bool,
    operations: Vec<serde_json::Value>,
    selected: BTreeSet<String>,
    name: Entity<TextInput>,
    description: Entity<TextInput>,
    content: Entity<TextInput>,
    busy: bool,
    preview_ready: bool,
    message: String,
    focus: HashMap<String, FocusHandle>,
}
impl AppView {
    pub(super) fn open_recording(&mut self, plugins: bool, cx: &mut Context<Self>) {
        let controller = self.controller.clone();
        let session = self.projection.active_session_id.clone();
        let workspace = self.projection.active_workspace_id().map(str::to_owned);
        let owner = cx.entity().downgrade();
        let bounds = Bounds::centered(None, size(px(760.0), px(740.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(
                        i18n::t(if plugins {
                            "record.plugins"
                        } else {
                            "record.title"
                        })
                        .into(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|cx| {
                    let name = cx.new(|cx| {
                        TextInput::with_placeholder(i18n::t("record.name"), cx)
                            .id("record-name")
                            .tab_stop(true)
                    });
                    let description = cx.new(|cx| {
                        TextInput::with_placeholder(i18n::t("record.description"), cx)
                            .id("record-description")
                            .tab_stop(true)
                    });
                    let content = cx.new(|cx| {
                        TextInput::with_placeholder(i18n::t("record.preview"), cx)
                            .id("record-content")
                            .tab_stop(true)
                            .code_editor()
                            .height_clamp(180.0, 280.0)
                    });
                    let mut view = RecordingView {
                        access: PanelAccess::default(),
                        owner,
                        controller,
                        session,
                        workspace,
                        plugins,
                        operations: Vec::new(),
                        selected: BTreeSet::new(),
                        name,
                        description,
                        content,
                        busy: false,
                        preview_ready: false,
                        message: String::new(),
                        focus: HashMap::new(),
                    };
                    view.refresh(cx);
                    view
                })
            },
        ) {
            self.status_hint = Some(error.to_string());
        }
    }
}
impl RecordingView {
    fn request(
        &mut self,
        request: Result<AppCommand, AppQuery>,
        saving: bool,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.message.clear();
        let task = self.controller.product_request(request);
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Err(error) => view.message = error,
                    Ok(data) if saving => {
                        view.message = format!(
                            "{} {}",
                            i18n::t("record.saved"),
                            data["path"].as_str().unwrap_or("")
                        );
                        view.preview_ready = false;
                    }
                    Ok(data) if view.plugins => {
                        if data["plugins"].as_array().is_some_and(Vec::is_empty) {
                            view.message = i18n::t("record.plugins_empty").into();
                        } else {
                            view.message = "Invalid plugin response".into();
                        }
                    }
                    Ok(data) => {
                        if let Some(operations) = data["operations"].as_array() {
                            view.operations = operations.clone();
                            view.selected.retain(|id| {
                                view.operations
                                    .iter()
                                    .any(|op| op["event_id"].as_str() == Some(id.as_str()))
                            });
                            let content = data["content"].as_str().unwrap_or("").to_string();
                            view.preview_ready = !content.is_empty();
                            view.content
                                .update(cx, |input, cx| input.set_text(content, cx));
                            view.message = if data["truncated"] == true {
                                i18n::t("record.recent")
                            } else if view.operations.is_empty() {
                                i18n::t("record.empty")
                            } else {
                                i18n::t("record.hint")
                            }
                            .into();
                        } else {
                            view.message = "Invalid recording response".into();
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.preview_ready = false;
        self.selected.clear();
        if self.plugins {
            self.request(Err(AppQuery::PluginList), false, cx);
        } else if let Some(session) = &self.session {
            self.request(
                Err(AppQuery::SkillRecordPreview {
                    session_id: session.clone().into(),
                    event_ids: Vec::new(),
                }),
                false,
                cx,
            );
        } else {
            self.message = i18n::t("record.session_required").into();
        }
    }
    fn action(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match id {
            "refresh" => self.refresh(cx),
            "skills" => {
                self.plugins = false;
                self.refresh(cx);
            }
            "mcp" => {
                let _ = self.owner.update(cx, |owner, cx| {
                    owner.select_inspector_tab(InspectorTab::Resources, cx);
                    owner.inspector_open = true;
                    cx.notify();
                });
            }
            "preview" if !self.selected.is_empty() => {
                if let Some(session) = &self.session {
                    self.request(
                        Err(AppQuery::SkillRecordPreview {
                            session_id: session.clone().into(),
                            event_ids: self.selected.iter().cloned().map(Into::into).collect(),
                        }),
                        false,
                        cx,
                    );
                }
            }
            "save" if self.preview_ready => {
                if let Some(workspace) = &self.workspace {
                    self.request(
                        Ok(AppCommand::SkillRecordSave {
                            workspace_id: workspace.clone().into(),
                            name: self.name.read(cx).text().trim().into(),
                            description: self.description.read(cx).text().trim().into(),
                            content: self.content.read(cx).text().into(),
                        }),
                        true,
                        cx,
                    );
                }
            }
            _ => {}
        }
    }
    fn button(
        &mut self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self
            .focus
            .entry(id.into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
            .tab_stop(!self.busy && enabled);
        let button = Button::new(format!("record-{id}"))
            .label(i18n::t(label))
            .track_focus(&focus)
            .disabled(self.busy || !enabled)
            .on_click(cx.listener(move |view, event, _, cx| {
                if AppView::click_down_position(event).is_some() {
                    view.action(id, cx);
                }
            }))
            .on_activate(cx.listener(move |view, _, _, cx| view.action(id, cx)));
        self.access
            .wrap(
                &format!("record-{id}"),
                i18n::t(label),
                AxRole::Button,
                None,
                !self.busy && enabled,
                false,
                button,
            )
            .into_any_element()
    }
}
impl Render for RecordingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title(i18n::t(if self.plugins {
            "record.plugins"
        } else {
            "record.title"
        }));
        #[cfg(target_os = "macos")]
        install_appkit_tab_monitor(window, cx);
        self.access.begin();
        let status = self.access.wrap(
            "record-status",
            &self.message,
            AxRole::StaticText,
            None,
            false,
            false,
            self.message.clone(),
        );
        let refresh = self.button("refresh", "plan.refresh", true, cx);
        let mut body = div()
            .id("record-body")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(dark().bg.base)
            .text_color(dark().text.primary)
            .child(status)
            .child(refresh);
        if self.plugins {
            let skills = self.button(
                "skills",
                "record.title",
                self.session.is_some() && self.workspace.is_some(),
                cx,
            );
            let mcp = self.button("mcp", "record.mcp", true, cx);
            self.access.sync(window, cx, Self::ax_action);
            return body
                .child(i18n::t("record.runtime"))
                .child(skills)
                .child(mcp);
        }
        if self.busy {
            self.access.sync(window, cx, Self::ax_action);
            return body.child(i18n::t("plan.loading"));
        }
        let mut list = div()
            .id("record-operations")
            .h(px(180.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for operation in &self.operations {
            let Some(id) = operation["event_id"].as_str() else {
                continue;
            };
            let id = id.to_owned();
            let focus = self
                .focus
                .entry(id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone()
                .tab_stop(operation["recordable"] == true);
            let selected = self.selected.contains(&id);
            let id_click = id.clone();
            let label = format!(
                "{} {}{}",
                if selected { "✓" } else { "○" },
                operation["name"].as_str().unwrap_or(""),
                if operation["is_error"] == true {
                    " · failed"
                } else {
                    ""
                }
            );
            let button = Button::new(format!("record-operation-{id}"))
                .label(label.clone())
                .variant(ButtonVariant::Ghost)
                .track_focus(&focus)
                .disabled(operation["recordable"] != true)
                .on_click(cx.listener(move |view, event, _, cx| {
                    if AppView::click_down_position(event).is_some() {
                        view.select(&id_click, cx);
                    }
                }))
                .on_activate(cx.listener(move |view, _, _, cx| view.select(&id, cx)));
            let operation_id = operation["event_id"].as_str().unwrap();
            list = list.child(self.access.wrap(
                &format!("record-operation-{operation_id}"),
                &label,
                AxRole::Button,
                None,
                operation["recordable"] == true,
                focus.is_focused(window),
                button,
            ));
        }
        let preview = self.button("preview", "record.preview", !self.selected.is_empty(), cx);
        let save = self.button(
            "save",
            "record.save",
            self.preview_ready && self.workspace.is_some(),
            cx,
        );
        let name = self.access.wrap(
            "record-name",
            i18n::t("record.name"),
            AxRole::TextArea,
            Some(self.name.read(cx).text().into()),
            true,
            self.name.focus_handle(cx).is_focused(window),
            self.name.clone(),
        );
        let description = self.access.wrap(
            "record-description",
            i18n::t("record.description"),
            AxRole::TextArea,
            Some(self.description.read(cx).text().into()),
            true,
            self.description.focus_handle(cx).is_focused(window),
            self.description.clone(),
        );
        let content = self.access.wrap(
            "record-content",
            "SKILL.md",
            AxRole::TextArea,
            Some(self.content.read(cx).text().into()),
            true,
            self.content.focus_handle(cx).is_focused(window),
            self.content.clone(),
        );
        self.access.sync(window, cx, Self::ax_action);
        body = body
            .child(list)
            .child(preview)
            .child(name)
            .child(description)
            .child(content)
            .child(save);
        body
    }
}
impl RecordingView {
    fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.selected.remove(id) && self.selected.len() < 64 {
            self.selected.insert(id.into());
        }
        self.preview_ready = false;
        cx.notify();
    }
}

impl RecordingView {
    fn ax_action(&mut self, request: AxRequest, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.access.permits(&request) {
            return;
        }
        let input = match request.identifier.as_str() {
            "record-name" => Some(&self.name),
            "record-description" => Some(&self.description),
            "record-content" => Some(&self.content),
            _ => None,
        };
        if let Some(input) = input {
            edit_input(input, request, window, cx);
        } else if let Some(id) = request.identifier.strip_prefix("record-operation-") {
            self.select(id, cx);
        } else if let Some(id) = request.identifier.strip_prefix("record-") {
            self.action(id, cx);
        }
    }
}
