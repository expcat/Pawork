//! 按任务保留附件与本轮搜索选择；文件字节仅由显式系统选择器授权读取。
use super::*;
use crate::controller::ComposerOptions;

impl AppView {
    pub(super) fn current_composer_options(&self) -> ComposerOptions {
        self.composer_options
            .get(&self.projection.active_session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn composer_capability_error(&self) -> Option<&'static str> {
        let options = self.current_composer_options();
        let model = self
            .projection
            .effective_model()
            .and_then(|(provider, id)| {
                self.projection
                    .models
                    .iter()
                    .find(|entry| &entry.provider_id == provider && &entry.id == id)
            });
        if options.attachments.iter().any(|a| a.image) && !model.is_some_and(|m| m.image_input) {
            return Some(i18n::t("composer.image_model_required"));
        }
        if options.web_search == Some(true) && !model.is_some_and(|m| m.web_search) {
            return Some(i18n::t("composer.search_model_required"));
        }
        None
    }

    pub(super) fn pick_composer_attachments(
        &mut self,
        images_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composer_loading || self.composer_sending {
            return;
        }
        let draft = self.projection.active_session_id.clone();
        self.composer_loading = true;
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(
                i18n::t(if images_only {
                    "files.attach_image"
                } else {
                    "composer.add_files"
                })
                .into(),
            ),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = selection.await;
            this.update_in(cx, |view, window, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        view.controller
                            .load_composer_attachments(draft, paths, images_only)
                    }
                    Ok(Ok(None)) => view.composer_loading = false,
                    other => {
                        view.composer_loading = false;
                        view.status_hint = Some(format!("{other:?}"));
                    }
                }
                view.focus_composer(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn toggle_composer_search(&mut self, cx: &mut Context<Self>) {
        if self.composer_sending {
            return;
        }
        let options = self
            .composer_options
            .entry(self.projection.active_session_id.clone())
            .or_default();
        options.web_search = Some(options.web_search != Some(true));
        cx.notify();
    }

    pub(super) fn remove_composer_attachment(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.composer_sending {
            return;
        }
        if let Some(options) = self
            .composer_options
            .get_mut(&self.projection.active_session_id)
        {
            options.attachments.retain(|a| a.id != id);
        }
        cx.notify();
    }

    pub(super) fn composer_attachments_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let options = self.current_composer_options();
        let mut row = self
            .settings_element("composer-options")
            .max_h(px(96.0))
            .overflow_y_scroll()
            .flex_none()
            .flex()
            .flex_wrap()
            .gap_2()
            .text_size(font::XS);
        for attachment in options.attachments {
            let id = format!("composer-remove-{}", attachment.id);
            let focus = self
                .settings_action_focus
                .entry(id.clone())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let attachment_id = attachment.id.clone();
            let key_id = attachment.id;
            row = row.child(
                self.settings_element(id.clone()).child(
                    Button::new(id)
                        .variant(ButtonVariant::Raised)
                        .track_focus(&focus)
                        .disabled(self.composer_sending)
                        .max_width(px(240.0))
                        .label(format!("{}  ×", attachment.name))
                        .tooltip(i18n::t("composer.remove_attachment"))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.remove_composer_attachment(&attachment_id, cx)
                        }))
                        .on_activate(cx.listener(move |view, _, _, cx| {
                            view.remove_composer_attachment(&key_id, cx);
                            cx.stop_propagation();
                        })),
                ),
            );
        }
        if options.web_search.is_some() {
            let focus = self
                .settings_action_focus
                .entry("composer-search-toggle".into())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            row = row.child(
                self.settings_element("composer-search-toggle").child(
                    Button::new("composer-search-toggle")
                        .variant(ButtonVariant::Raised)
                        .track_focus(&focus)
                        .disabled(self.composer_sending)
                        .label(i18n::t(if options.web_search == Some(true) {
                            "composer.search_on"
                        } else {
                            "composer.search_off"
                        }))
                        .on_click(cx.listener(|view, _, _, cx| view.toggle_composer_search(cx)))
                        .on_activate(cx.listener(|view, _, _, cx| {
                            view.toggle_composer_search(cx);
                            cx.stop_propagation();
                        })),
                ),
            );
        }
        if let Some(error) = self.composer_capability_error() {
            row = row.child(
                div()
                    .w_full()
                    .text_color(dark().text.secondary)
                    .child(error),
            );
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::ComposerAttachment;
    #[gpui::test]
    fn unassigned_attachment_draft_search_and_failure_preservation(cx: &mut gpui::TestAppContext) {
        let platform = Arc::new(Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("composer-local-test.sock"),
                None,
                cx,
            )
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = None;
                view.projection.connection = ConnectionState::Connected {
                    instance_id: "composer-local".into(),
                };
                view.projection.models = vec![crate::projection::ModelEntry {
                    provider_id: "test".into(),
                    id: "vision".into(),
                    image_input: true,
                    web_search: true,
                    ..Default::default()
                }];
                view.projection
                    .set_pending_model("test".into(), "vision".into());
                assert!(view.composer_action_enabled(input_area::ComposerAction::Image));
                view.handle_controller_event(
                    ControllerEvent::ComposerAttachmentsLoaded {
                        draft: None,
                        result: Ok(vec![ComposerAttachment {
                            id: "picked".into(),
                            name: "图片 截图.png".into(),
                            bytes: Arc::new(vec![1]),
                            image: true,
                        }]),
                    },
                    cx,
                );
                assert!(view.can_send(cx)); // attachment-only, no project or existing task
                view.activate_composer_action(input_area::ComposerAction::Search, window, cx);
                assert!(view.open_menu.is_none());
                assert_eq!(view.current_composer_options().web_search, Some(true));
                view.projection.models[0].web_search = false;
                assert!(!view.can_send(cx));
                view.toggle_composer_search(cx);
                assert!(view.can_send(cx));
                view.composer_sending = true;
                view.handle_controller_event(
                    ControllerEvent::OperationFailed {
                        action: "send message",
                        reason: "offline".into(),
                    },
                    cx,
                );
                assert!(view.can_send(cx));
                assert_eq!(view.current_composer_options().attachments.len(), 1);
                view.projection.active_session_id = Some("other-task".into());
                assert!(view.current_composer_options().attachments.is_empty());
                view.projection.active_session_id = None;
                assert_eq!(
                    view.current_composer_options().attachments[0].name,
                    "图片 截图.png"
                );
                view.remove_composer_attachment("picked", cx);
                assert!(!view.can_send(cx));
            })
        });
    }
}
