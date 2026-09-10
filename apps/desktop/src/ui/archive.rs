//! UX-08：窗口内归档撤销；只使用已有 session_archive 写口。
use super::accessibility::{AxAction, AxNode, AxRect, AxRole};
use super::i18n::t;
use super::*;

#[derive(Default)]
pub(super) struct ArchiveState {
    // 也保留未确认请求：回执丢失时仍可用幂等反归档找回任务。
    entries: Vec<(String, String)>,
    pending: Option<(String, bool)>,
    failed: bool,
    restored: bool,
    notice_layout: ScrollHandle,
    undo_layout: ScrollHandle,
}

impl AppView {
    pub(super) fn archive_write_enabled(&self) -> bool {
        self.archive.pending.is_none()
            && matches!(
                self.projection.connection,
                ConnectionState::Connected { .. }
            )
    }

    pub(super) fn on_session_archive(&mut self, session_id: String, cx: &mut Context<Self>) {
        if !self.archive_write_enabled() {
            return;
        }
        let Some(session) = self
            .projection
            .sessions
            .iter()
            .find(|s| s.session_id == session_id)
        else {
            return;
        };
        if !self.archive.entries.iter().any(|(id, _)| id == &session_id) {
            self.archive
                .entries
                .push((session_id.clone(), session.title.clone()));
        }
        self.close_open_menu(cx);
        self.session_rename = None;
        self.archive.pending = Some((session_id.clone(), true));
        self.archive.failed = false;
        self.archive.restored = false;
        self.controller.archive_session(session_id, true);
        cx.notify();
    }

    pub(super) fn undo_session_archive(&mut self, cx: &mut Context<Self>) {
        if !self.archive_write_enabled() {
            return;
        }
        let Some((id, _)) = self.archive.entries.last() else {
            return;
        };
        let id = id.clone();
        self.close_open_menu(cx);
        self.archive.pending = Some((id.clone(), false));
        self.archive.failed = false;
        self.controller.archive_session(id, false);
        cx.notify();
    }

    pub(super) fn finish_session_archive(
        &mut self,
        id: String,
        archived: bool,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        if self.archive.pending.as_ref() != Some(&(id.clone(), archived)) {
            return;
        }
        self.archive.pending = None;
        self.archive.failed = result.is_err();
        if let Ok(title) = result {
            if archived {
                // 归档开始时已经登记；外部线程或恢复失败 / 重试仍能从提示找回。
                if let Some(entry) = self.archive.entries.iter_mut().find(|(key, _)| key == &id) {
                    entry.1 = title;
                } else {
                    self.archive.entries.push((id, title));
                }
                self.pending_scope_focus = true;
            } else {
                self.archive.entries.retain(|(key, _)| key != &id);
                self.archive.restored = true;
                // 按已刷新列表打开原任务，不创建副本；解除会隐藏它的项目筛选。
                if self.projection.sessions.iter().any(|s| s.session_id == id) {
                    self.scope_workspace_id = None;
                    self.open_session(id, cx);
                    self.rail_scroll_to_active = true;
                }
                self.pending_scope_focus = true;
            }
        }
        cx.notify();
    }

    pub(super) fn archive_notice_text(&self) -> Option<String> {
        if self.archive.pending.is_some() {
            return Some(t("archive.pending").into());
        }
        if self.archive.failed {
            return Some(t("archive.failed").into());
        }
        if let Some((_, title)) = self.archive.entries.last() {
            return Some(format!(
                "{}\n{}",
                t("archive.done").replace("{}", title),
                t("archive.kept")
            ));
        }
        self.archive.restored.then(|| t("archive.restored").into())
    }

    pub(super) fn archive_notice_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.rail_row_focus_handle("archive-undo", cx);
        div()
            .id("archive-notice")
            .track_scroll(&self.archive.notice_layout)
            .flex_none()
            .mt_2()
            .text_size(font::SM)
            .text_color(dark().text.secondary)
            .child(
                div()
                    .whitespace_normal()
                    .child(self.archive_notice_text().unwrap_or_default()),
            )
            .when(!self.archive.entries.is_empty(), |area| {
                area.child(
                    div()
                        .id("archive-undo-layout")
                        .track_scroll(&self.archive.undo_layout)
                        .mt_2()
                        .child(
                            Button::new("archive-undo")
                                .variant(ButtonVariant::Raised)
                                .track_focus(&focus)
                                .disabled(!self.archive_write_enabled())
                                .label(t("archive.undo"))
                                .on_click(cx.listener(|view, event, _, cx| {
                                    if !view.consume_button_key_click("archive-undo", event) {
                                        view.undo_session_archive(cx);
                                    }
                                }))
                                .on_activate(cx.listener(|view, _, _, cx| {
                                    view.note_button_key_activate("archive-undo");
                                    view.undo_session_archive(cx);
                                    cx.stop_propagation();
                                })),
                        ),
                )
            })
    }

    pub(super) fn archive_notice_ax(&self, window: &Window, clip: AxRect) -> Option<AxNode> {
        let text = self.archive_notice_text()?;
        let rect = |layout: &ScrollHandle| {
            let b = layout.bounds();
            let x = f32::from(b.origin.x).max(clip.x);
            let y = f32::from(b.origin.y).max(clip.y);
            AxRect::new(
                x,
                y,
                (f32::from(b.origin.x + b.size.width).min(clip.x + clip.width) - x).max(0.0),
                (f32::from(b.origin.y + b.size.height).min(clip.y + clip.height) - y).max(0.0),
            )
        };
        let mut node = AxNode::new(
            "archive-notice",
            AxRole::Group,
            text,
            rect(&self.archive.notice_layout),
        );
        if !self.archive.entries.is_empty() {
            let enabled = self.archive_write_enabled();
            let mut button = AxNode::new(
                "archive-undo",
                AxRole::Button,
                t("archive.undo"),
                rect(&self.archive.undo_layout),
            )
            .enabled(enabled)
            .focused(
                self.rail_row_focus
                    .get("archive-undo")
                    .is_some_and(|f| f.is_focused(window)),
            );
            if enabled {
                button = button.action(AxAction::Press);
            }
            node = node.child(button);
        }
        Some(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn archive_undo_keeps_identity_drafts_and_retry(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        use serde_json::json;
        let view = cx.new(|cx| {
            AppView::new(
                Arc::new(Platform::new()),
                "/tmp/ux08-test.sock".into(),
                None,
                cx,
            )
        });
        let mut snapshot: pawork_client::Snapshot = serde_json::from_value(json!({
            "instance_id":"test", "snapshot_sequence":1, "generated_at":1,
            "sections":[{"kind":"session_tree", "revision":1, "data":[
                {"session_id":"s-a", "title":"Original", "workspace_id":"ws-a", "updated_at_ms":1}
            ]}]
        }))
        .unwrap();
        let original = snapshot.clone();
        view.update(cx, |view, cx| {
            view.projection.connection = ConnectionState::Connected {
                instance_id: "test".into(),
            };
            view.projection.merge_snapshot(&snapshot);
            view.projection.select_session("s-a");
            view.text_input
                .update(cx, |input, cx| input.set_text("draft", cx));
            view.on_session_archive("s-a".into(), cx);
            view.on_session_archive("s-a".into(), cx);
            assert_eq!(view.archive.entries.len(), 1);
            assert!(!view.archive_write_enabled());
            snapshot.sections[0].data = Some(json!([]));
            view.handle_controller_event(ControllerEvent::Snapshot(snapshot.clone()), cx);
            view.finish_session_archive("s-a".into(), true, Ok("Original".into()), cx);
            assert!(view.projection.active_session_id.is_none());
            assert_eq!(view.composer_drafts["s-a"], "draft");
            assert!(view.archive_notice_text().unwrap().contains("Original"));
            view.undo_session_archive(cx);
            view.undo_session_archive(cx);
            assert_eq!(view.archive.pending, Some(("s-a".into(), false)));
            view.finish_session_archive("s-a".into(), false, Err("lost response".into()), cx);
            assert!(view.archive.failed);
            assert_eq!(view.archive.entries.len(), 1);
            view.projection.connection = ConnectionState::Disconnected {
                reason: "test".into(),
            };
            view.undo_session_archive(cx);
            assert!(view.archive.pending.is_none());
            view.projection.connection = ConnectionState::Connected {
                instance_id: "test".into(),
            };
            view.undo_session_archive(cx);
            view.handle_controller_event(ControllerEvent::Snapshot(original), cx);
            view.finish_session_archive("s-a".into(), false, Ok("Original".into()), cx);
            assert!(view.archive.entries.is_empty());
            assert_eq!(view.projection.sessions.len(), 1);
            assert_eq!(view.projection.active_session_id.as_deref(), Some("s-a"));
            assert_eq!(
                view.projection.sessions[0].workspace_id.as_deref(),
                Some("ws-a")
            );
            assert_eq!(view.text_input.read(cx).text(), "draft");
            view.undo_session_archive(cx);
            assert!(view.archive.pending.is_none());
        });
    }
}
