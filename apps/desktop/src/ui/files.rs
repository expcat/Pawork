//! Project files and in-memory drafts. File IO belongs to the Host.
use std::collections::{HashMap, HashSet};

use gpui::{
    div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString,
    Subscription, Window,
};
use serde_json::Value;

use super::{
    accessibility::{AxAction, AxNode, AxRect, AxRole},
    components::button::{Button, ButtonPadding, ButtonVariant},
    i18n::t,
    icon, icon_sized,
    inspector::{FileTab, PanelTab},
    theme::{dark, font},
    AppRoute, AppView, Icon, InspectorTab, SaveFile, TextInput,
};
use crate::{controller::FileOperation, projection::ConnectionState};

#[derive(Clone)]
struct FileEntry {
    path: String,
    name: String,
    is_dir: bool,
}

fn is_image_path(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp"
        )
    })
}

struct FileDocument {
    path: String,
    input: Entity<TextInput>,
    focus: FocusHandle,
    saved: String,
    revision: String,
    saving: Option<u64>,
    error: Option<String>,
    preview: bool,
    preview_scroll: ScrollHandle,
    _observe: Subscription,
}
impl FileDocument {
    fn is_markdown(&self) -> bool {
        self.path.rsplit_once('.').is_some_and(|(_, ext)| {
            ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
        })
    }
    fn dirty(&self, cx: &App) -> bool {
        self.input.read(cx).text() != self.saved
    }
}

/// 懒加载目录树：每个目录路径（根为 "."）独立缓存单层条目与在途请求代次；
/// 展开状态与缓存子树在刷新后按最新清单修剪。
struct FileWorkspace {
    tree: HashMap<String, Vec<FileEntry>>,
    loading: HashMap<String, u64>,
    errors: HashMap<String, String>,
    truncated: HashSet<String>,
    expanded: HashSet<String>,
    reading: Option<(String, u64)>,
    error: Option<String>,
    documents: Vec<FileDocument>,
    selected: Option<usize>,
    scroll: ScrollHandle,
    sidebar: bool,
    filter: Option<Entity<TextInput>>,
    filter_observe: Option<Subscription>,
}

impl Default for FileWorkspace {
    fn default() -> Self {
        Self {
            tree: HashMap::new(),
            loading: HashMap::new(),
            errors: HashMap::new(),
            truncated: HashSet::new(),
            expanded: HashSet::new(),
            reading: None,
            error: None,
            documents: vec![],
            selected: None,
            scroll: ScrollHandle::new(),
            sidebar: true,
            filter: None,
            filter_observe: None,
        }
    }
}

#[derive(Default)]
pub(super) struct FilesPanel {
    /// Explicit attachment picker scope; ordinary file browsing keeps opening text.
    attachment_target: Option<(String, String)>,
    workspaces: HashMap<String, FileWorkspace>,
    epoch: u64,
    layouts: HashMap<String, ScrollHandle>,
    focus: HashMap<String, FocusHandle>,
    close_prompt: bool,
}

enum FilesRow {
    Entry {
        path: String,
        name: String,
        is_dir: bool,
        depth: usize,
        expanded: bool,
    },
    Note {
        id: String,
        depth: usize,
        text: String,
        danger: bool,
    },
}

/// 把目录树按展开状态摊平成渲染 / AX 共用的行序列。筛选只覆盖已加载
/// （曾展开）的目录：名称命中的条目保留，包含命中的目录强制展开显示。
fn collect_rows(
    ws: &FileWorkspace,
    dir: &str,
    depth: usize,
    query: &str,
    rows: &mut Vec<FilesRow>,
) {
    let filtering = !query.is_empty();
    if let Some(entries) = ws.tree.get(dir) {
        for entry in entries {
            let name_match = filtering && entry.name.to_lowercase().contains(query);
            let mark = rows.len();
            let mut child_match = false;
            if entry.is_dir && (ws.expanded.contains(&entry.path) || filtering) {
                collect_rows(ws, &entry.path, depth + 1, query, rows);
                child_match = filtering && rows.len() > mark;
            }
            if filtering && !name_match && !child_match {
                rows.truncate(mark);
                continue;
            }
            rows.insert(
                mark,
                FilesRow::Entry {
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    is_dir: entry.is_dir,
                    depth,
                    expanded: if filtering {
                        child_match
                    } else {
                        ws.expanded.contains(&entry.path)
                    },
                },
            );
        }
    }
    if filtering {
        return;
    }
    if let Some(error) = ws.errors.get(dir) {
        rows.push(FilesRow::Note {
            id: format!("files-note-{dir}-error"),
            depth,
            text: error.clone(),
            danger: true,
        });
    } else if ws.loading.contains_key(dir) {
        rows.push(FilesRow::Note {
            id: format!("files-note-{dir}-loading"),
            depth,
            text: t("files.loading").into(),
            danger: false,
        });
    }
    if ws.truncated.contains(dir) {
        rows.push(FilesRow::Note {
            id: format!("files-note-{dir}-truncated"),
            depth,
            text: t("files.truncated").into(),
            danger: false,
        });
    }
}

/// 某个目录重新列举成功后，收掉清单里已不存在的子目录的展开状态与缓存，
/// 避免已删除目录继续显示旧内容。
fn prune_stale_dirs(ws: &mut FileWorkspace, path: &str, entries: &[FileEntry]) {
    let prefix = if path == "." {
        String::new()
    } else {
        format!("{path}/")
    };
    let live: HashSet<&str> = entries
        .iter()
        .filter(|entry| entry.is_dir)
        .map(|entry| entry.path.as_str())
        .collect();
    let keep = |dir: &String| {
        if !prefix.is_empty() && !dir.starts_with(&prefix) {
            return true;
        }
        let rest = &dir[prefix.len()..];
        let child = rest.split('/').next().unwrap_or("");
        let child_path = if prefix.is_empty() {
            child.to_string()
        } else {
            format!("{path}/{child}")
        };
        live.contains(child_path.as_str())
    };
    ws.expanded.retain(&keep);
    ws.tree.retain(|dir, _| keep(dir));
    ws.loading.retain(|dir, _| keep(dir));
    ws.errors.retain(|dir, _| keep(dir));
    ws.truncated.retain(&keep);
}

impl AppView {
    pub(super) fn cancel_file_attachment_picker(&mut self) {
        self.files.attachment_target = None;
    }

    fn picking_file_attachment(&self) -> bool {
        self.files
            .attachment_target
            .as_ref()
            .is_some_and(|(workspace, session)| {
                self.projection.active_workspace_id() == Some(workspace.as_str())
                    && self.projection.active_session_id.as_ref() == Some(session)
            })
    }

    pub(super) fn on_attach_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_attach_image() {
            return;
        }
        self.files.attachment_target = self
            .projection
            .active_workspace_id()
            .zip(self.projection.active_session_id.as_deref())
            .map(|(workspace, session)| (workspace.to_string(), session.to_string()));
        self.close_open_menu(cx);
        self.select_inspector_tab(InspectorTab::Files, cx);
        if !self.inspector_open {
            self.on_toggle_inspector(window, cx);
        }
        self.ensure_files(cx);
        if let Some(ws) = self
            .inspector_workspace_id()
            .and_then(|id| self.files.workspaces.get_mut(&id))
        {
            ws.sidebar = true;
            if let Some(filter) = &ws.filter {
                filter.update(cx, |input, cx| input.set_text("", cx));
            }
        }
        cx.notify();
    }

    pub(super) fn files_panel_tabs(&self, cx: &App) -> Vec<PanelTab> {
        let Some(id) = self.inspector_workspace_id() else {
            return vec![];
        };
        let Some(ws) = self.files.workspaces.get(&id) else {
            return vec![];
        };
        ws.documents
            .iter()
            .enumerate()
            .map(|(ix, doc)| PanelTab {
                id: format!("file-tab-{id}/{}", doc.path),
                label: format!("{}{}", if doc.dirty(cx) { "● " } else { "" }, doc.path),
                tool: InspectorTab::Files,
                terminal_id: None,
                file: Some(FileTab {
                    workspace_id: id.clone(),
                    path: doc.path.clone(),
                }),
                selected: self.inspector_tab == InspectorTab::Files && ws.selected == Some(ix),
                close_enabled: doc.saving.is_none()
                    && !ws
                        .reading
                        .as_ref()
                        .is_some_and(|(path, _)| path == &doc.path),
            })
            .collect()
    }

    pub(super) fn activate_file_tab(&mut self, file: &FileTab, cx: &mut Context<Self>) {
        if self.inspector_workspace_id().as_deref() != Some(file.workspace_id.as_str()) {
            return;
        }
        let Some(ws) = self.files.workspaces.get_mut(&file.workspace_id) else {
            return;
        };
        let Some(ix) = ws.documents.iter().position(|doc| doc.path == file.path) else {
            return;
        };
        ws.selected = Some(ix);
        ws.reading = None;
        self.select_inspector_tab(InspectorTab::Files, cx);
        self.pending_inspector_focus = Some(super::InspectorFocusTarget::SelectedTab);
        cx.notify();
    }

    pub(super) fn close_file_tab(
        &mut self,
        file: &FileTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.files.close_prompt {
            return;
        }
        let Some(ws) = self.files.workspaces.get(&file.workspace_id) else {
            return;
        };
        let Some(doc) = ws.documents.iter().find(|doc| doc.path == file.path) else {
            return;
        };
        if doc.saving.is_some()
            || ws
                .reading
                .as_ref()
                .is_some_and(|(path, _)| path == &file.path)
        {
            return;
        }
        if !doc.dirty(cx) {
            self.remove_file_tab(file, cx);
            return;
        }
        let file = file.clone();
        let input = doc.input.clone();
        let prompt = window.prompt(
            gpui::PromptLevel::Warning,
            t("files.close_file_title"),
            Some(&file.path),
            &[t("files.keep"), t("files.discard_close")],
            cx,
        );
        self.files.close_prompt = true;
        cx.spawn_in(window, async move |this, cx| {
            let discard = prompt.await == Ok(1);
            let _ = this.update(cx, |view, cx| {
                view.files.close_prompt = false;
                // A delayed confirmation must not discard a newly opened editor at the same path.
                let same_editor = view
                    .files
                    .workspaces
                    .get(&file.workspace_id)
                    .is_some_and(|ws| {
                        ws.documents
                            .iter()
                            .any(|doc| doc.path == file.path && doc.input == input)
                    });
                if discard && same_editor {
                    view.remove_file_tab(&file, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn remove_file_tab(&mut self, file: &FileTab, cx: &mut Context<Self>) {
        let Some(ws) = self.files.workspaces.get_mut(&file.workspace_id) else {
            return;
        };
        let Some(ix) = ws.documents.iter().position(|doc| doc.path == file.path) else {
            return;
        };
        if ws.documents[ix].saving.is_some()
            || ws
                .reading
                .as_ref()
                .is_some_and(|(path, _)| path == &file.path)
        {
            return;
        }
        let was_selected = ws.selected == Some(ix);
        ws.documents.remove(ix);
        ws.selected = if ws.documents.is_empty() {
            None
        } else {
            ws.selected.map(|selected| {
                if selected > ix {
                    selected - 1
                } else {
                    selected.min(ws.documents.len() - 1)
                }
            })
        };
        let empty = ws.documents.is_empty();
        if empty {
            ws.reading = None;
            ws.sidebar = true;
        }
        if self.inspector_workspace_id().as_deref() == Some(file.workspace_id.as_str()) {
            if empty {
                self.close_inspector_tool(InspectorTab::Files, cx);
            } else if was_selected && self.inspector_tab == InspectorTab::Files {
                self.pending_inspector_focus = Some(super::InspectorFocusTarget::SelectedTab);
            }
        }
        cx.notify();
    }

    fn files_connected(&self) -> bool {
        matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        )
    }
    fn files_document(&self) -> Option<&FileDocument> {
        let ws = self.files.workspaces.get(&self.inspector_workspace_id()?)?;
        ws.documents.get(ws.selected?)
    }
    pub(super) fn files_editor_focused(&self, window: &Window) -> bool {
        self.files_document()
            .is_some_and(|doc| !doc.preview && doc.focus.is_focused(window))
    }
    pub(super) fn files_filter_focused(&self, window: &Window, cx: &App) -> bool {
        self.inspector_workspace_id()
            .and_then(|id| self.files.workspaces.get(&id))
            .filter(|ws| ws.sidebar)
            .and_then(|ws| ws.filter.as_ref())
            .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
    }
    pub(super) fn ensure_files(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.inspector_workspace_id() else {
            return;
        };
        let ws = self.files.workspaces.entry(id).or_default();
        if ws.filter.is_none() {
            let input = cx.new(|cx| {
                TextInput::with_placeholder(t("files.filter"), cx)
                    .id("files-filter")
                    .height_clamp(32., 32.)
            });
            let scroll = ws.scroll.clone();
            ws.filter_observe = Some(cx.observe(&input, move |_, _, cx| {
                scroll.set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            }));
            ws.filter = Some(input);
        }
        if !ws.tree.contains_key(".")
            && !ws.loading.contains_key(".")
            && !ws.errors.contains_key(".")
            && self.files_connected()
        {
            self.files_list(".".into(), cx);
        }
    }
    fn files_list(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.files_connected() {
            return;
        }
        let Some(id) = self.inspector_workspace_id() else {
            return;
        };
        self.files.epoch += 1;
        let epoch = self.files.epoch;
        let ws = self.files.workspaces.entry(id.clone()).or_default();
        ws.loading.insert(path.clone(), epoch);
        ws.errors.remove(&path);
        self.controller
            .workspace_file_operation(id, path, epoch, FileOperation::List);
        cx.notify();
    }
    fn files_toggle_dir(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(id) = self.inspector_workspace_id() else {
            return;
        };
        let needs_list = {
            let ws = self.files.workspaces.entry(id).or_default();
            if ws.expanded.remove(&path) {
                cx.notify();
                return;
            }
            ws.expanded.insert(path.clone());
            // 展开失败的目录再次展开即重试；files_list 会清掉旧错误。
            !ws.tree.contains_key(&path) && !ws.loading.contains_key(&path)
        };
        if needs_list {
            self.files_list(path, cx);
        }
        cx.notify();
    }
    fn files_read(&mut self, path: String, reload: bool, cx: &mut Context<Self>) {
        let Some(id) = self.inspector_workspace_id() else {
            return;
        };
        let ws = self.files.workspaces.entry(id.clone()).or_default();
        if !reload {
            if let Some(ix) = ws.documents.iter().position(|doc| doc.path == path) {
                ws.selected = Some(ix);
                ws.reading = None;
                self.inspector_reveal_selected = true;
                cx.notify();
                return;
            }
        }
        if !self.files_connected() {
            return;
        }
        self.files.epoch += 1;
        let epoch = self.files.epoch;
        let ws = self.files.workspaces.get_mut(&id).unwrap();
        ws.error = None;
        ws.reading = Some((path.clone(), epoch));
        self.controller
            .workspace_file_operation(id, path, epoch, FileOperation::Read);
        cx.notify();
    }
    pub(super) fn files_result(
        &mut self,
        id: String,
        path: String,
        epoch: u64,
        operation: FileOperation,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.files.workspaces.get_mut(&id) else {
            return;
        };
        match operation {
            FileOperation::List => {
                if ws.loading.get(&path) != Some(&epoch) {
                    return;
                }
                ws.loading.remove(&path);
                match result.and_then(|data| {
                    let entries = data["entries"]
                        .as_array()
                        .ok_or("Invalid directory response")?
                        .iter()
                        .map(|entry| {
                            Ok(FileEntry {
                                path: entry["path"].as_str().ok_or("Invalid entry path")?.into(),
                                name: entry["name"].as_str().ok_or("Invalid entry name")?.into(),
                                is_dir: entry["is_dir"].as_bool().ok_or("Invalid entry kind")?,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let truncated = data["truncated"]
                        .as_bool()
                        .ok_or("Invalid directory response")?;
                    Ok((entries, truncated))
                }) {
                    Ok((entries, truncated)) => {
                        prune_stale_dirs(ws, &path, &entries);
                        ws.tree.insert(path.clone(), entries);
                        if truncated {
                            ws.truncated.insert(path.clone());
                        } else {
                            ws.truncated.remove(&path);
                        }
                    }
                    Err(error) => {
                        ws.errors.insert(path, error);
                    }
                }
            }
            FileOperation::Read => {
                if ws.reading.as_ref() != Some(&(path.clone(), epoch)) {
                    return;
                }
                ws.reading = None;
                match result.and_then(|data| {
                    Ok((
                        data["content"]
                            .as_str()
                            .ok_or("Invalid file content")?
                            .to_string(),
                        data["revision"]
                            .as_str()
                            .ok_or("Invalid file revision")?
                            .to_string(),
                    ))
                }) {
                    Ok((content, revision)) => {
                        let input = cx.new(|cx| {
                            let mut input = TextInput::with_placeholder("", cx)
                                .id("files-editor")
                                .code_editor();
                            input.reset_text(content.clone(), cx);
                            input
                        });
                        let focus = input.read(cx).focus_handle(cx).tab_stop(true);
                        let observe = cx.observe(&input, |_, _, cx| cx.notify());
                        let doc = FileDocument {
                            path: path.clone(),
                            input,
                            focus,
                            saved: content,
                            revision,
                            saving: None,
                            error: None,
                            preview: path.rsplit_once('.').is_some_and(|(_, ext)| {
                                ext.eq_ignore_ascii_case("md")
                                    || ext.eq_ignore_ascii_case("markdown")
                            }),
                            preview_scroll: ScrollHandle::new(),
                            _observe: observe,
                        };
                        if let Some(index) = ws.documents.iter().position(|doc| doc.path == path) {
                            // Reload is dispatched only after an explicit discard. Edits made
                            // while the read was in flight must still survive its response.
                            if ws.documents[index].dirty(cx) {
                                cx.notify();
                                return;
                            }
                            ws.documents[index] = doc;
                            ws.selected = Some(index);
                        } else {
                            ws.documents.push(doc);
                            ws.selected = Some(ws.documents.len() - 1);
                        }
                        self.inspector_reveal_selected = true;
                    }
                    Err(error) => ws.error = Some(error),
                }
            }
            FileOperation::Save { content, .. } => {
                let Some(doc) = ws.documents.iter_mut().find(|doc| doc.path == path) else {
                    return;
                };
                if doc.saving != Some(epoch) {
                    return;
                }
                doc.saving = None;
                match result.and_then(|data| {
                    data["revision"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "Invalid save receipt".to_string())
                }) {
                    Ok(revision) => {
                        doc.saved = content;
                        doc.revision = revision;
                        doc.error = None;
                    }
                    Err(error) => doc.error = Some(error),
                }
            }
        }
        cx.notify();
    }
    pub(super) fn on_save_file(&mut self, _: &SaveFile, _: &mut Window, cx: &mut Context<Self>) {
        if self.route != AppRoute::Workspace
            || !self.inspector_open
            || self.inspector_tab != InspectorTab::Files
        {
            return;
        }
        self.save_file(cx);
        cx.stop_propagation();
    }
    fn save_file(&mut self, cx: &mut Context<Self>) {
        if !self.files_connected() {
            return;
        }
        let Some(id) = self.inspector_workspace_id() else {
            return;
        };
        let Some(ws) = self.files.workspaces.get_mut(&id) else {
            return;
        };
        let Some(doc) = ws.selected.and_then(|ix| ws.documents.get_mut(ix)) else {
            return;
        };
        if ws
            .reading
            .as_ref()
            .is_some_and(|(path, _)| path == &doc.path)
        {
            return;
        }
        if doc.saving.is_some() || !doc.dirty(cx) || doc.input.read(cx).is_composing() {
            return;
        }
        let content = doc.input.read(cx).text().to_string();
        if content.len() > 128 * 1024 || content.contains('\0') {
            doc.error = Some(t("files.text_limit").into());
            cx.notify();
            return;
        }
        self.files.epoch += 1;
        doc.saving = Some(self.files.epoch);
        doc.error = None;
        self.controller.workspace_file_operation(
            id,
            doc.path.clone(),
            self.files.epoch,
            FileOperation::Save {
                content,
                revision: doc.revision.clone(),
            },
        );
        cx.notify();
    }
    pub(super) fn files_action(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.inspector_workspace_id() else {
            return;
        };
        match action {
            "files-sidebar" => {
                if let Some(ws) = self.files.workspaces.get_mut(&id) {
                    ws.sidebar = !ws.sidebar;
                    if !ws.sidebar {
                        window.focus(&self.focus_handle);
                    }
                }
            }
            "files-mode" => {
                if let Some(doc) = self
                    .files
                    .workspaces
                    .get_mut(&id)
                    .and_then(|ws| ws.selected.and_then(|ix| ws.documents.get_mut(ix)))
                    .filter(|doc| doc.is_markdown())
                {
                    if !doc.input.read(cx).is_composing() {
                        doc.preview = !doc.preview;
                        if !doc.preview {
                            window.focus(&doc.focus);
                        } else {
                            window.focus(&self.focus_handle);
                        }
                    }
                }
            }
            "files-save" => self.save_file(cx),
            "files-refresh" => {
                let mut paths = vec![".".to_string()];
                if let Some(ws) = self.files.workspaces.get(&id) {
                    paths.extend(ws.expanded.iter().cloned());
                }
                for path in paths {
                    self.files_list(path, cx);
                }
            }
            "files-reload" => {
                let Some(doc) = self.files_document() else {
                    return;
                };
                if doc.saving.is_some() || !self.files_connected() {
                    return;
                }
                let path = doc.path.clone();
                if doc.dirty(cx) {
                    let prompt = window.prompt(
                        gpui::PromptLevel::Warning,
                        t("files.discard_title"),
                        Some(&path),
                        &[t("files.keep"), t("files.discard_reload")],
                        cx,
                    );
                    cx.spawn_in(window, async move |this, cx| {
                        if prompt.await == Ok(1) {
                            let _ = this.update(cx, |view, cx| {
                                if view.inspector_workspace_id().as_ref() != Some(&id) {
                                    return;
                                }
                                if let Some(doc) =
                                    view.files.workspaces.get_mut(&id).and_then(|ws| {
                                        ws.documents.iter_mut().find(|doc| doc.path == path)
                                    })
                                {
                                    if doc.saving.is_some() {
                                        return;
                                    }
                                    doc.input.update(cx, |input, cx| {
                                        input.set_text(doc.saved.clone(), cx)
                                    });
                                }
                                view.files_read(path, true, cx);
                            });
                        }
                    })
                    .detach();
                } else {
                    self.files_read(path, true, cx);
                }
            }
            _ => {
                if let Some(path) = action.strip_prefix("files-entry-") {
                    let entry = self.files.workspaces.get(&id).and_then(|ws| {
                        ws.tree
                            .values()
                            .flatten()
                            .find(|entry| entry.path == path)
                            .cloned()
                    });
                    if let Some(entry) = entry {
                        if entry.is_dir {
                            self.files_toggle_dir(entry.path, cx);
                        } else if is_image_path(&entry.path)
                            || self.files.attachment_target.as_ref().is_some_and(
                                |(workspace, session)| {
                                    workspace == &id
                                        && self.projection.active_session_id.as_ref()
                                            == Some(session)
                                },
                            )
                        {
                            if self.projection.active_session_id.is_none()
                                || self.projection.active_workspace_id() != Some(id.as_str())
                            {
                                self.status_hint = Some(t("files.image_needs_task").into());
                            } else if !self.text_input.read(cx).is_composing() {
                                // Only the relative reference enters the draft. The Host resolves
                                // and authorizes the file when the user explicitly sends it.
                                let mut draft = self.text_input.read(cx).text().to_string();
                                if !draft.is_empty() && !draft.ends_with(char::is_whitespace) {
                                    draft.push(' ');
                                }
                                draft.push('@');
                                draft.push_str(
                                    &serde_json::to_string(&entry.path).expect("path string"),
                                );
                                draft.push(' ');
                                self.text_input
                                    .update(cx, |input, cx| input.set_text(draft, cx));
                                let supports_images = self
                                    .projection
                                    .effective_model()
                                    .and_then(|(provider, model)| {
                                        crate::projection::find_model_entry(
                                            &self.projection.models,
                                            provider,
                                            model,
                                        )
                                    })
                                    .is_some_and(|model| model.image_input);
                                self.status_hint = (is_image_path(&entry.path) && !supports_images)
                                    .then(|| t("files.image_model_hint").into());
                                self.files.attachment_target = None;
                                self.on_inspector_back(window, cx);
                                self.pending_inspector_focus =
                                    Some(super::InspectorFocusTarget::Composer);
                                self.focus_composer(window, cx);
                            }
                        } else {
                            self.files_read(entry.path, false, cx);
                        }
                    }
                }
            }
        }
        cx.notify();
    }
    fn files_button(
        &mut self,
        id: String,
        label: String,
        glyph: Option<Icon>,
        enabled: bool,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let layout = self.files.layouts.entry(id.clone()).or_default().clone();
        let focus = self
            .files
            .focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click = id.clone();
        let activate = id.clone();
        let mut button = Button::new(SharedString::from(id.clone()))
            .variant(if selected {
                ButtonVariant::Raised
            } else {
                ButtonVariant::Ghost
            })
            .padding(ButtonPadding::Horizontal(8.0))
            .height(px(30.))
            .text_size(font::SM)
            .track_focus(&focus)
            .disabled(!enabled)
            .tooltip(label.clone());
        if let Some(glyph) = glyph {
            button = button.child(icon(glyph));
        }
        if !matches!(id.as_str(), "files-refresh" | "files-sidebar") {
            button = button.child(div().truncate().child(label));
        }
        button = button
            .on_click(cx.listener(move |view, event, window, cx| {
                if !view.consume_button_key_click(&click, event) {
                    view.files_action(&click, window, cx);
                }
            }))
            .on_activate(cx.listener(move |view, _, window, cx| {
                if view.open_menu.is_none() {
                    view.note_button_key_activate(&activate);
                    view.files_action(&activate, window, cx);
                    cx.stop_propagation();
                }
            }));
        div()
            .id(SharedString::from(format!("{id}-layout")))
            .flex_none()
            .track_scroll(&layout)
            .child(button)
    }
    fn files_tree_row(
        &mut self,
        path: &str,
        name: &str,
        is_dir: bool,
        depth: usize,
        expanded: bool,
        enabled: bool,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = format!("files-entry-{path}");
        let layout = self.files.layouts.entry(id.clone()).or_default().clone();
        let focus = self
            .files
            .focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click = id.clone();
        let activate = id.clone();
        let mut label_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .flex_1()
            .min_w_0();
        if is_dir {
            label_row = label_row.child(icon_sized(
                if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                px(12.),
            ));
        }
        label_row = label_row
            .child(icon_sized(
                if is_dir {
                    Icon::Project
                } else if is_image_path(path) {
                    Icon::Link
                } else {
                    Icon::File
                },
                px(14.),
            ))
            .child(div().truncate().child(name.to_string()))
            .when(
                !is_dir && (is_image_path(path) || self.picking_file_attachment()),
                |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(font::XS)
                            .text_color(dark().text.secondary)
                            .child(t(if is_image_path(path) {
                                "files.attach_image"
                            } else {
                                "composer.add_files"
                            })),
                    )
                },
            );
        let button = Button::new(SharedString::from(id.clone()))
            .variant(if active {
                ButtonVariant::Raised
            } else {
                ButtonVariant::Ghost
            })
            .padding(ButtonPadding::Horizontal(4.0))
            .height(px(30.))
            .text_size(font::SM)
            .track_focus(&focus)
            .disabled(!enabled)
            .tooltip(path.to_string())
            .child(label_row)
            .on_click(cx.listener(move |view, event, window, cx| {
                if !view.consume_button_key_click(&click, event) {
                    view.files_action(&click, window, cx);
                }
            }))
            .on_activate(cx.listener(move |view, _, window, cx| {
                if view.open_menu.is_none() {
                    view.note_button_key_activate(&activate);
                    view.files_action(&activate, window, cx);
                    cx.stop_propagation();
                }
            }));
        div()
            .id(SharedString::from(format!("{id}-layout")))
            .flex_none()
            .track_scroll(&layout)
            .ml(px(2. + depth as f32 * 12.))
            .child(button)
    }
    pub(super) fn files_element(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.ensure_files(cx);
        let mut panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .text_size(font::SM);
        let Some(id) = self.inspector_workspace_id() else {
            return panel.child(div().p_4().child(t("files.no_project")));
        };
        let connected = self.files_connected();
        let ws = self.files.workspaces.entry(id).or_default();
        let loading = !ws.loading.is_empty();
        let reading = ws.reading.as_ref().map(|(path, _)| path.clone());
        let error = ws.error.clone();
        let scroll = ws.scroll.clone();
        let sidebar = ws.sidebar;
        let filter = ws.filter.clone().expect("file filter initialized");
        let query = filter.read(cx).text().trim().to_lowercase();
        let mut rows = Vec::new();
        collect_rows(ws, ".", 0, &query, &mut rows);
        let active_path = self.files_document().map(|doc| doc.path.clone());
        let path = active_path.clone().unwrap_or_else(|| ".".into());
        panel = panel.child(
            div()
                .flex()
                .items_center()
                .flex_none()
                .px_2()
                .py_1()
                .child(
                    div().flex_1().min_w_0().child(
                        div()
                            .truncate()
                            .text_color(dark().text.secondary)
                            .child(path),
                    ),
                )
                .child(
                    self.files_button(
                        "files-sidebar".into(),
                        t(if sidebar {
                            "files.hide_sidebar"
                        } else {
                            "files.show_sidebar"
                        })
                        .into(),
                        Some(Icon::Inspector),
                        true,
                        sidebar,
                        cx,
                    ),
                ),
        );
        if !connected {
            panel = panel.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(dark().semantic.warning_text)
                    .child(t("files.offline")),
            );
        }
        if let Some(error) = error {
            panel = panel.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(dark().semantic.danger_text)
                    .child(error),
            );
        }
        let mut content = div().flex().flex_col().flex_1().min_h_0().overflow_hidden();
        if let Some(doc) = self.files_document() {
            let dirty = doc.dirty(cx);
            let saving = doc.saving.is_some();
            let reloading = reading.as_deref() == Some(doc.path.as_str());
            let has_error = doc.error.is_some();
            let preview = doc.preview;
            let markdown = doc.is_markdown();
            let composing = doc.input.read(cx).is_composing();
            let preview_scroll = doc.preview_scroll.clone();
            let text = doc.input.read(cx).text().to_owned();
            let status = doc.error.clone().unwrap_or_else(|| {
                t(if saving {
                    "files.saving"
                } else if dirty {
                    "files.unsaved"
                } else {
                    "files.saved"
                })
                .into()
            });
            let input = doc.input.clone();
            // The toolbar spans both panes, keeping actions usable in the 440px inspector.
            let mut actions = div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .pb_1()
                .flex_none();
            if markdown {
                actions = actions.child(
                    self.files_button(
                        "files-mode".into(),
                        t(if preview {
                            "files.source"
                        } else {
                            "files.preview"
                        })
                        .into(),
                        None,
                        !composing,
                        false,
                        cx,
                    ),
                );
            }
            actions = actions
                .child(div().flex_1())
                .child(self.files_button(
                    "files-reload".into(),
                    t("files.reload").into(),
                    None,
                    connected && !saving && reading.is_none(),
                    false,
                    cx,
                ))
                .child(self.files_button(
                    "files-save".into(),
                    t("files.save").into(),
                    None,
                    connected && dirty && !saving && !reloading,
                    false,
                    cx,
                ));
            panel = panel.child(actions);
            let layout = self
                .files
                .layouts
                .entry(
                    if preview {
                        "files-preview"
                    } else {
                        "files-editor"
                    }
                    .into(),
                )
                .or_default()
                .clone();
            if preview {
                let body = super::markdown::message_body_element(
                    self,
                    cx,
                    window,
                    "files-markdown",
                    &text,
                    dark().text.primary,
                    false,
                );
                content = content.child(
                    div()
                        .id("files-preview-layout")
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .track_scroll(&layout)
                        .child(
                            div()
                                .id("files-preview-scroll")
                                .h_full()
                                .overflow_y_scroll()
                                .track_scroll(&preview_scroll)
                                .p_3()
                                .child(body),
                        ),
                );
            } else {
                content = content.child(
                    div()
                        .id("files-editor-layout")
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .track_scroll(&layout)
                        .child(input),
                );
            }
            content = content.child(
                div()
                    .id("files-status")
                    .px_3()
                    .py_1()
                    .flex_none()
                    .text_color(if has_error {
                        dark().semantic.danger_text
                    } else {
                        dark().text.secondary
                    })
                    .child(status),
            );
        } else {
            content = content.child(div().p_4().text_color(dark().text.secondary).child(t(
                if reading.is_some() {
                    "files.loading"
                } else {
                    "files.select"
                },
            )));
        }
        let mut panes = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .w_full()
            .border_t_1()
            .border_color(dark().border.subtle)
            .child(div().flex().flex_row().flex_1().min_w_0().child(content));
        if sidebar {
            let filter_layout = self
                .files
                .layouts
                .entry("files-filter".into())
                .or_default()
                .clone();
            let mut listing = div()
                .id("files-list")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .overflow_x_hidden()
                .track_scroll(&scroll)
                .px_1();
            let rows_empty = rows.is_empty();
            for row in rows {
                match row {
                    FilesRow::Entry {
                        path,
                        name,
                        is_dir,
                        depth,
                        expanded,
                    } => {
                        let active = active_path.as_deref() == Some(path.as_str());
                        listing = listing.child(self.files_tree_row(
                            &path, &name, is_dir, depth, expanded, connected, active, cx,
                        ));
                    }
                    FilesRow::Note {
                        id,
                        depth,
                        text,
                        danger,
                    } => {
                        let layout = self.files.layouts.entry(id.clone()).or_default().clone();
                        listing = listing.child(
                            div()
                                .id(SharedString::from(format!("{id}-layout")))
                                .flex_none()
                                .track_scroll(&layout)
                                .ml(px(4. + depth as f32 * 12.))
                                .pr_2()
                                .py_1()
                                .text_color(if danger {
                                    dark().semantic.danger_text
                                } else {
                                    dark().text.secondary
                                })
                                .child(text),
                        );
                    }
                }
            }
            if rows_empty {
                listing = listing.child(div().p_2().text_color(dark().text.secondary).child(t(
                    if loading {
                        "files.loading"
                    } else if !query.is_empty() {
                        "files.no_matches"
                    } else {
                        "files.empty"
                    },
                )));
            }
            let browser = div()
                .flex()
                .flex_col()
                .w(px(160.))
                .flex_none()
                .border_l_1()
                .border_color(dark().border.subtle)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_none()
                        .px_1()
                        .child(div().flex_1().min_w_0())
                        .child(self.files_button(
                            "files-refresh".into(),
                            t("files.refresh").into(),
                            Some(Icon::Refresh),
                            connected && !loading,
                            false,
                            cx,
                        )),
                )
                .child(
                    div()
                        .id("files-filter-layout")
                        .flex_none()
                        .px_1()
                        .pb_1()
                        .track_scroll(&filter_layout)
                        .child(filter),
                )
                .child(listing);
            panes = panes.child(browser);
        }
        panel.child(panes)
    }
    pub(super) fn files_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        let mut root = AxNode::new("files", AxRole::Group, t("inspector.tab_files"), frame);
        let Some(ws) = self
            .inspector_workspace_id()
            .and_then(|id| self.files.workspaces.get(&id))
        else {
            return root;
        };
        let rect = |id: &str| {
            let b = self.files.layouts.get(id)?.bounds();
            let measured = AxRect::new(
                b.origin.x.into(),
                b.origin.y.into(),
                b.size.width.into(),
                b.size.height.into(),
            );
            (measured.width > 0. && measured.height > 0.).then_some(measured)
        };
        let connected = self.files_connected();
        let mut buttons: Vec<(String, String, bool, bool)> = vec![(
            "files-sidebar".into(),
            t(if ws.sidebar {
                "files.hide_sidebar"
            } else {
                "files.show_sidebar"
            })
            .to_string(),
            true,
            ws.sidebar,
        )];
        if ws.sidebar {
            buttons.extend([(
                "files-refresh".into(),
                t("files.refresh").into(),
                connected && ws.loading.is_empty(),
                false,
            )]);
        }
        let query = ws
            .filter
            .as_ref()
            .map(|input| input.read(cx).text().trim().to_lowercase())
            .unwrap_or_default();
        if ws.sidebar {
            if let (Some(input), Some(bounds)) = (&ws.filter, rect("files-filter")) {
                root = root.child(
                    AxNode::new("files-filter", AxRole::TextArea, t("files.filter"), bounds)
                        .value(input.read(cx).text())
                        .focused(input.read(cx).focus_handle(cx).is_focused(window))
                        .action(AxAction::Focus)
                        .action(AxAction::SetValue),
                );
            }
        }
        let list_bounds = ws.scroll.bounds();
        if ws.sidebar {
            let mut rows = Vec::new();
            collect_rows(ws, ".", 0, &query, &mut rows);
            for row in &rows {
                let (id, label, path, press) = match row {
                    FilesRow::Entry {
                        path, name, is_dir, ..
                    } => (
                        format!("files-entry-{path}"),
                        if !is_dir && (is_image_path(path) || self.picking_file_attachment()) {
                            format!(
                                "{} · {name}",
                                t(if is_image_path(path) {
                                    "files.attach_image"
                                } else {
                                    "composer.add_files"
                                })
                            )
                        } else {
                            name.clone()
                        },
                        Some(path.clone()),
                        true,
                    ),
                    FilesRow::Note { id, text, .. } => (id.clone(), text.clone(), None, false),
                };
                let Some(layout) = self.files.layouts.get(&id) else {
                    continue;
                };
                let bounds = layout.bounds().intersect(&list_bounds);
                if bounds.size.height <= px(0.) || bounds.size.width <= px(0.) {
                    continue;
                }
                let rect = AxRect::new(
                    bounds.origin.x.into(),
                    bounds.origin.y.into(),
                    bounds.size.width.into(),
                    bounds.size.height.into(),
                );
                let node = if press {
                    AxNode::new(id, AxRole::Button, &label, rect)
                        .description(path.as_deref().unwrap_or_default())
                        .enabled(connected)
                        .action(AxAction::Press)
                } else {
                    AxNode::new(id, AxRole::StaticText, &label, rect)
                };
                root = root.child(node);
            }
        }
        if let Some(doc) = ws.selected.and_then(|ix| ws.documents.get(ix)) {
            let reloading = ws
                .reading
                .as_ref()
                .is_some_and(|(path, _)| path == &doc.path);
            if doc.is_markdown() {
                buttons.push((
                    "files-mode".into(),
                    t(if doc.preview {
                        "files.source"
                    } else {
                        "files.preview"
                    })
                    .into(),
                    !doc.input.read(cx).is_composing(),
                    false,
                ));
            }
            buttons.push((
                "files-save".into(),
                t("files.save").into(),
                connected && doc.dirty(cx) && doc.saving.is_none() && !reloading,
                false,
            ));
            buttons.push((
                "files-reload".into(),
                t("files.reload").into(),
                connected && doc.saving.is_none() && ws.reading.is_none(),
                false,
            ));
            if doc.preview {
                if let Some(rect) = rect("files-preview") {
                    root = root.child(
                        AxNode::new("files-preview", AxRole::StaticText, &doc.path, rect)
                            .value(doc.input.read(cx).text()),
                    );
                }
            } else if let Some(rect) = rect("files-editor") {
                root = root.child(
                    AxNode::new("files-editor", AxRole::TextArea, &doc.path, rect)
                        .value(doc.input.read(cx).text())
                        .focused(doc.focus.is_focused(window))
                        .action(AxAction::Focus)
                        .action(AxAction::SetValue),
                );
            }
            root = root.child(AxNode::new(
                "files-status",
                AxRole::StaticText,
                doc.error.clone().unwrap_or_else(|| {
                    t(if doc.saving.is_some() {
                        "files.saving"
                    } else if doc.dirty(cx) {
                        "files.unsaved"
                    } else {
                        "files.saved"
                    })
                    .into()
                }),
                frame,
            ));
        }
        if let Some(error) = &ws.error {
            root = root.child(AxNode::new("files-error", AxRole::StaticText, error, frame));
        }
        for (id, label, enabled, selected) in buttons {
            if let Some(rect) = rect(&id) {
                root = root.child(
                    AxNode::new(id.clone(), AxRole::Button, label, rect)
                        .enabled(enabled)
                        .selected(selected)
                        .focused(
                            self.files
                                .focus
                                .get(&id)
                                .is_some_and(|f| f.is_focused(window)),
                        )
                        .action(AxAction::Press),
                );
            }
        }
        root
    }
    pub(super) fn files_ax_edit(
        &mut self,
        identifier: &str,
        action: AxAction,
        value: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = if identifier == "files-filter" {
            self.inspector_workspace_id()
                .and_then(|id| self.files.workspaces.get(&id))
                .filter(|ws| ws.sidebar)
                .and_then(|ws| ws.filter.clone())
        } else {
            self.files_document()
                .filter(|doc| !doc.preview)
                .map(|doc| doc.input.clone())
        };
        if let Some(input) = input {
            match action {
                AxAction::Focus => window.focus(&input.read(cx).focus_handle(cx)),
                AxAction::SetValue => input.update(cx, |input, cx| {
                    input.set_text(value.unwrap_or_default(), cx)
                }),
                _ => {}
            }
        }
        cx.notify();
    }
    pub(crate) fn install_files_close_guard(&self, window: &Window, cx: &Context<Self>) {
        let view = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |view, cx| {
                let dirty = view.files.workspaces.values().any(|ws| {
                    ws.documents
                        .iter()
                        .any(|doc| doc.dirty(cx) || doc.saving.is_some())
                });
                if !dirty {
                    return true;
                }
                if view.files.close_prompt {
                    return false;
                }
                view.files.close_prompt = true;
                let prompt = window.prompt(
                    gpui::PromptLevel::Warning,
                    t("files.close_title"),
                    None,
                    &[t("files.keep"), t("files.close_discard")],
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    let discard = prompt.await == Ok(1);
                    let _ = this.update_in(cx, |view, window, _| {
                        view.files.close_prompt = false;
                        if discard {
                            window.remove_window();
                        }
                    });
                })
                .detach();
                false
            })
            .unwrap_or(true)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[gpui::test]
    fn image_reference_preserves_draft_and_requires_project_task(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("image-reference-test.sock"),
                None,
                cx,
            )
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.workspace_id = Some("one".into());
                view.projection.connection = ConnectionState::Connected {
                    instance_id: "images".into(),
                };
                view.projection.active_session_id = None;
                view.on_composer_add_menu(None, cx);
                assert_eq!(view.open_menu, Some(super::super::MenuKind::ComposerAdd));
                assert!(!view.can_attach_image());
                view.close_menu_and_focus_trigger(super::super::MenuKind::ComposerAdd, window, cx);
                let path = "图片/屏幕 截图.PNG";
                view.files
                    .workspaces
                    .entry("one".into())
                    .or_default()
                    .tree
                    .insert(
                        ".".into(),
                        vec![FileEntry {
                            path: path.into(),
                            name: "屏幕 截图.PNG".into(),
                            is_dir: false,
                        }],
                    );
                view.text_input
                    .update(cx, |input, cx| input.set_text("分析这张图", cx));
                view.files_action(&format!("files-entry-{path}"), window, cx);
                assert_eq!(view.text_input.read(cx).text(), "分析这张图");
                view.projection
                    .sessions
                    .push(crate::projection::SessionSummary {
                        session_id: "image-task".into(),
                        title: "Images".into(),
                        workspace_id: Some("one".into()),
                        updated_at_ms: 0,
                        parent_branch_id: None,
                        forked_from_event_id: None,
                        active: true,
                        unstarted: true,
                    });
                view.projection.active_session_id = Some("image-task".into());
                view.inspector_open = true;
                view.files_action(&format!("files-entry-{path}"), window, cx);
                assert_eq!(
                    view.text_input.read(cx).text(),
                    "分析这张图 @\"图片/屏幕 截图.PNG\" "
                );
                assert!(view.projection.active_run_id.is_none());
                assert!(view.files.workspaces["one"].reading.is_none());
                assert!(view.files.workspaces["one"].documents.is_empty());
                // The same picker attaches text without opening or sending it.
                view.files
                    .workspaces
                    .get_mut("one")
                    .unwrap()
                    .tree
                    .get_mut(".")
                    .unwrap()
                    .push(FileEntry {
                        path: "docs/需求 说明.md".into(),
                        name: "需求 说明.md".into(),
                        is_dir: false,
                    });
                view.activate_composer_action(
                    super::super::input_area::ComposerAction::ProjectFiles,
                    window,
                    cx,
                );
                view.files_action("files-entry-docs/需求 说明.md", window, cx);
                assert!(view
                    .text_input
                    .read(cx)
                    .text()
                    .ends_with("@\"docs/需求 说明.md\" "));
                assert!(view.projection.active_run_id.is_none());
                assert!(view.files.attachment_target.is_none());
                assert!(view.files.workspaces["one"].reading.is_none());
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(view.read(cx).composer_focus_handle(cx).is_focused(window));
            assert!(!view.read(cx).inspector_open);
        });
    }

    #[gpui::test]
    fn file_drafts_survive_switches_and_save_receipts(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("file-panel-test.sock"),
                None,
                cx,
            )
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = None;
                view.projection.workspace_id = Some("one".into());
                view.inspector_open = true;
                view.inspector_tab = InspectorTab::Files;
                view.inspector_motion
                    .snap(super::super::theme::metrics::INSPECTOR_WIDTH);
                let ws = view.files.workspaces.entry("one".into()).or_default();
                ws.reading = Some(("notes.txt".into(), 1));
                view.files_result(
                    "one".into(),
                    "notes.txt".into(),
                    1,
                    FileOperation::Read,
                    Ok(json!({"content":"original\n", "revision":"v1"})),
                    cx,
                );
                let doc = view.files_document().unwrap();
                doc.input
                    .update(cx, |input, cx| input.set_text("first edit\n", cx));
                assert!(view.files_document().unwrap().dirty(cx));
                // Reading another file must not block saving the current draft.
                let connection = view.projection.connection.clone();
                view.projection.connection = ConnectionState::Connected {
                    instance_id: "files".into(),
                };
                view.files.workspaces.get_mut("one").unwrap().reading =
                    Some(("notes.txt".into(), 10));
                view.save_file(cx);
                assert!(view.files_document().unwrap().saving.is_none());
                view.files.workspaces.get_mut("one").unwrap().reading =
                    Some(("other.txt".into(), 11));
                view.save_file(cx);
                assert!(view.files_document().unwrap().saving.is_some());
                view.files.workspaces.get_mut("one").unwrap().reading = None;
                view.projection.connection = connection;
                view.files.workspaces.get_mut("one").unwrap().documents[0].saving = Some(2);
                // Editing remains available during a save. A receipt acknowledges only its own bytes.
                view.files_document()
                    .unwrap()
                    .input
                    .update(cx, |input, cx| input.set_text("newer edit\n", cx));
                view.files_result(
                    "one".into(),
                    "notes.txt".into(),
                    2,
                    FileOperation::Save {
                        content: "first edit\n".into(),
                        revision: "v1".into(),
                    },
                    Ok(json!({"revision":"v2"})),
                    cx,
                );
                assert!(view.files_document().unwrap().dirty(cx));
                assert_eq!(
                    view.files_document().unwrap().input.read(cx).text(),
                    "newer edit\n"
                );
                view.close_inspector_tool(InspectorTab::Files, cx);
                view.select_inspector_tab(InspectorTab::Files, cx);
                assert!(view.files_document().unwrap().dirty(cx));
                view.projection.workspace_id = Some("two".into());
                assert!(view.files_document().is_none());
                view.projection.workspace_id = Some("one".into());
                assert_eq!(
                    view.files_document().unwrap().input.read(cx).text(),
                    "newer edit\n"
                );
                let ws = view.files.workspaces.get_mut("one").unwrap();
                ws.documents[0].saving = Some(3);
                view.files_result(
                    "one".into(),
                    "notes.txt".into(),
                    2,
                    FileOperation::Save {
                        content: "wrong\n".into(),
                        revision: "v1".into(),
                    },
                    Ok(json!({"revision":"stale"})),
                    cx,
                );
                assert_eq!(view.files_document().unwrap().revision, "v2");
                view.files_result(
                    "one".into(),
                    "notes.txt".into(),
                    3,
                    FileOperation::Save {
                        content: "newer edit\n".into(),
                        revision: "v2".into(),
                    },
                    Err("File changed on disk".into()),
                    cx,
                );
                assert!(view.files_document().unwrap().dirty(cx));
                assert_eq!(
                    view.files_document().unwrap().error.as_deref(),
                    Some("File changed on disk")
                );
            })
        });
        for width in [1440., 1080.] {
            cx.simulate_resize(gpui::size(px(width), px(900.)));
            cx.refresh().unwrap();
            cx.run_until_parked();
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let tree = view.accessibility_tree(window, cx);
                    let editor = tree.find("files-editor").expect("editor is visible");
                    assert_eq!(editor.value.as_deref(), Some("newer edit\n"));
                    assert!(
                        editor.bounds.width > 100. && editor.bounds.height > 100.,
                        "window={width}, editor={:?}",
                        editor.bounds
                    );
                    assert!(editor.bounds.x + editor.bounds.width <= width);
                    assert!(
                        !tree.find("files-save").unwrap().enabled,
                        "offline save must be disabled"
                    );
                })
            });
        }
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let ws = view.files.workspaces.get_mut("one").unwrap();
                ws.tree.insert(
                    ".".into(),
                    vec![
                        FileEntry {
                            path: "src".into(),
                            name: "src".into(),
                            is_dir: true,
                        },
                        FileEntry {
                            path: "README.md".into(),
                            name: "README.md".into(),
                            is_dir: false,
                        },
                    ],
                );
                ws.reading = Some(("README.md".into(), 4));
                view.files_result(
                    "one".into(),
                    "README.md".into(),
                    4,
                    FileOperation::Read,
                    Ok(json!({"content":"# Preview\n\nText", "revision":"md1"})),
                    cx,
                );
                let filter = view.files.workspaces["one"].filter.as_ref().unwrap();
                filter.update(cx, |input, cx| input.set_text("readme", cx));
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("files-preview").is_some());
                assert!(tree.find("files-editor").is_none());
                assert!(tree.find("files-entry-src").is_none());
                assert!(tree.find("files-entry-README.md").is_some());
                view.files_ax_edit("files-filter", AxAction::Focus, None, window, cx);
                assert!(view.files_filter_focused(window, cx));
                view.text_input
                    .update(cx, |input, cx| input.set_text("Keep chat draft", cx));
                view.on_send_message(&super::super::SendMessage, window, cx);
                assert_eq!(view.text_input.read(cx).text(), "Keep chat draft");
                view.files_action("files-mode", window, cx);
                assert!(view.files_editor_focused(window));
                view.files_document()
                    .unwrap()
                    .input
                    .update(cx, |input, cx| input.set_text("# Edited", cx));
                view.files_action("files-mode", window, cx);
                view.files_action("files-sidebar", window, cx);
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert_eq!(
                    tree.find("files-preview").unwrap().value.as_deref(),
                    Some("# Edited")
                );
                assert!(tree.find("files-filter").is_none());
                assert!(tree.find("files-entry-README.md").is_none());
                assert!(view.files_document().unwrap().dirty(cx));
            });
        });
        // Files and PTYs use the same header, including activation and close dispatch.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection
                    .apply_terminal_created("one".into(), "pty-files".into());
                let tabs = view.panel_tabs(cx);
                assert_eq!(tabs.len(), 3);
                assert!(tabs.iter().all(|tab| tab.id != "inspector-open-files"));
                let terminal = tabs.iter().find(|tab| tab.terminal_id.is_some()).unwrap();
                view.activate_panel_tab(terminal, cx);
                assert_eq!(view.inspector_tab, InspectorTab::Terminal);
                view.activate_panel_tab(&tabs[0], cx);
                assert_eq!(
                    view.files_document().unwrap().input.read(cx).text(),
                    "newer edit\n"
                );
                view.activate_panel_tab(&tabs[1], cx);
                assert_eq!(
                    view.files_document().unwrap().input.read(cx).text(),
                    "# Edited"
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("inspector-open-files").is_none());
                assert!(tree.find("files-tab-0").is_none());
                let file = tree.find("file-tab-one/README.md").unwrap();
                let terminal = tree.find("terminal-tab-pty-files").unwrap();
                assert_eq!(file.bounds.y, terminal.bounds.y);
                assert!(file.selected);
                let background = view
                    .panel_tabs(cx)
                    .into_iter()
                    .find(|tab| tab.id == "file-tab-one/notes.txt")
                    .unwrap();
                view.close_panel_tab(&background, window, cx);
            });
        });
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer(t("files.keep"));
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.files.workspaces["one"].documents.len(), 2);
                assert_eq!(view.files_document().unwrap().path, "README.md");
                let background = view
                    .panel_tabs(cx)
                    .into_iter()
                    .find(|tab| tab.id == "file-tab-one/notes.txt")
                    .unwrap();
                view.close_panel_tab(&background, window, cx);
            });
        });
        cx.simulate_prompt_answer(t("files.discard_close"));
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.files.workspaces["one"].documents.len(), 1);
                assert_eq!(view.files_document().unwrap().path, "README.md");
                let tab = view
                    .panel_tabs(cx)
                    .into_iter()
                    .find(|tab| tab.file.is_some())
                    .unwrap();
                view.close_panel_tab(&tab, window, cx);
            });
        });
        cx.simulate_prompt_answer(t("files.discard_close"));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.inspector_tab, InspectorTab::Terminal);
            assert_eq!(
                view.projection.terminal.session_id.as_deref(),
                Some("pty-files")
            );
            assert_eq!(view.panel_tabs(cx).len(), 1);
            assert_eq!(view.text_input.read(cx).text(), "Keep chat draft");
        });
    }

    #[gpui::test]
    fn file_tree_expands_lazily_and_prunes_stale_dirs(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("file-tree-test.sock"),
                None,
                cx,
            )
        });
        cx.run_until_parked();
        cx.simulate_resize(gpui::size(px(1440.), px(900.)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.connection = ConnectionState::Connected {
                    instance_id: "tree".into(),
                };
                view.projection.workspace_id = Some("one".into());
                view.inspector_open = true;
                view.inspector_tab = InspectorTab::Files;
                view.inspector_motion
                    .snap(super::super::theme::metrics::INSPECTOR_WIDTH);
                view.ensure_files(cx);
                let ws = &view.files.workspaces["one"];
                assert!(ws.loading.len() == 1 && ws.loading.contains_key("."));
                let epoch = view.files.epoch;
                view.files_result(
                    "one".into(),
                    ".".into(),
                    epoch,
                    FileOperation::List,
                    Ok(json!({"entries":[
                        {"path":"docs","name":"docs","is_dir":true},
                        {"path":"README.md","name":"README.md","is_dir":false}
                    ],"truncated":false})),
                    cx,
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("files-entry-docs").is_some());
                assert!(tree.find("files-entry-docs/review").is_none());
                // 首次展开才读取该层
                view.files_action("files-entry-docs", window, cx);
                assert!(view.files.workspaces["one"].expanded.contains("docs"));
                let epoch = view.files.epoch;
                assert_eq!(
                    view.files.workspaces["one"].loading.get("docs"),
                    Some(&epoch)
                );
                view.files_result(
                    "one".into(),
                    "docs".into(),
                    epoch,
                    FileOperation::List,
                    Ok(json!({"entries":[
                        {"path":"docs/review","name":"review","is_dir":true},
                        {"path":"docs/architecture.md","name":"architecture.md","is_dir":false}
                    ],"truncated":false})),
                    cx,
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                let docs = tree.find("files-entry-docs").unwrap();
                let nested = tree.find("files-entry-docs/review").unwrap();
                assert!(nested.bounds.x > docs.bounds.x, "nested rows indent");
                assert!(tree.find("files-entry-docs/architecture.md").is_some());
                // 筛选覆盖已加载目录：命中名与其祖先保留，其余目录隐藏
                view.files_ax_edit(
                    "files-filter",
                    AxAction::SetValue,
                    Some("arch".into()),
                    window,
                    cx,
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("files-entry-docs").is_some());
                assert!(tree.find("files-entry-docs/architecture.md").is_some());
                assert!(tree.find("files-entry-docs/review").is_none());
                assert!(tree.find("files-entry-README.md").is_none());
                view.files_ax_edit(
                    "files-filter",
                    AxAction::SetValue,
                    Some(String::new()),
                    window,
                    cx,
                );
                // 折叠后子行隐藏；再展开走缓存，不重复请求
                view.files_action("files-entry-docs", window, cx);
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("files-entry-docs/review").is_none());
                view.files_action("files-entry-docs", window, cx);
                assert!(view.files.workspaces["one"].loading.is_empty());
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                assert!(
                    view.accessibility_tree(window, cx)
                        .find("files-entry-docs/review")
                        .is_some()
                );
                // 列举失败在所属目录下就地显示，折叠再展开即重试
                view.files_action("files-entry-docs/review", window, cx);
                let epoch = view.files.epoch;
                view.files_result(
                    "one".into(),
                    "docs/review".into(),
                    epoch,
                    FileOperation::List,
                    Err("Permission denied".into()),
                    cx,
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                let note = tree.find("files-note-docs/review-error").unwrap();
                assert_eq!(note.label, "Permission denied");
                view.files_action("files-entry-docs/review", window, cx);
                view.files_action("files-entry-docs/review", window, cx);
                assert!(
                    view.files.workspaces["one"]
                        .loading
                        .contains_key("docs/review")
                );
                let epoch = view.files.epoch;
                view.files_result(
                    "one".into(),
                    "docs/review".into(),
                    epoch,
                    FileOperation::List,
                    Ok(json!({"entries":[],"truncated":false})),
                    cx,
                );
                // 刷新重读根与全部已展开目录
                view.files_action("files-refresh", window, cx);
                let loading = &view.files.workspaces["one"].loading;
                assert_eq!(loading.len(), 3);
                assert!(loading.contains_key("."));
                assert!(loading.contains_key("docs"));
                assert!(loading.contains_key("docs/review"));
                // 最新根清单不含 docs：其展开状态、缓存与在途请求一并修剪
                let root_epoch = *view.files.workspaces["one"].loading.get(".").unwrap();
                view.files_result(
                    "one".into(),
                    ".".into(),
                    root_epoch,
                    FileOperation::List,
                    Ok(json!({"entries":[
                        {"path":"README.md","name":"README.md","is_dir":false}
                    ],"truncated":false})),
                    cx,
                );
                let ws = &view.files.workspaces["one"];
                assert!(ws.expanded.is_empty());
                assert!(!ws.tree.contains_key("docs"));
                assert!(!ws.loading.contains_key("docs"));
                assert!(!ws.loading.contains_key("docs/review"));
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find("files-entry-docs").is_none());
                assert!(tree.find("files-entry-README.md").is_some());
            });
        });
    }
}
