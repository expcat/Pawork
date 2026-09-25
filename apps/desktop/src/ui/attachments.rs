//! 按任务保留附件与本轮搜索选择；文件字节仅由显式系统选择器授权读取。
use super::*;
use crate::controller::{ComposerAttachment, ComposerAttachmentError, ComposerOptions};

/// 文本附件预览的截断上限（内容已在内存，超出部分注明截断）。
const PREVIEW_TEXT_LIMIT: usize = 4 * 1024;

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
        if !options.video_urls.is_empty() && !model.is_some_and(|m| m.video_input) {
            return Some(i18n::t("video.model_required"));
        }
        None
    }

    pub(super) fn pick_composer_attachments(
        &mut self,
        images_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_composer_paths(images_only, false, window, cx);
    }

    pub(super) fn pick_composer_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_composer_paths(false, true, window, cx);
    }

    fn pick_composer_paths(
        &mut self,
        images_only: bool,
        folders: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composer_loading || self.composer_sending {
            return;
        }
        let draft = self.projection.active_session_id.clone();
        // 手动重选即清除上一次的附件错误（RV-03）。
        if let Some(options) = self.composer_options.get_mut(&draft) {
            options.attachment_error = None;
        }
        self.composer_loading = true;
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: !folders,
            directories: folders,
            multiple: true,
            prompt: Some(
                i18n::t(if folders {
                    "composer.add_folder"
                } else if images_only {
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

    pub(super) fn attach_external_browser(
        &mut self,
        browser: pawork_browser::ExternalBrowser,
        cx: &mut Context<Self>,
    ) {
        self.composer_loading = true;
        let draft = self.projection.active_session_id.clone();
        self.composer_options
            .entry(draft.clone())
            .or_default()
            .attachment_error = None;
        self.controller.load_browser_attachment(draft, browser);
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
        // 移除后关闭指向该附件的预览并回收解码缓存。
        if let Some((_, open_id)) = self.composer_attachment_preview.as_ref() {
            if open_id == id {
                self.composer_attachment_preview = None;
            }
        }
        self.composer_attachment_images.remove(id);
        cx.notify();
    }

    /// RV-01：点 chip 本体开/关预览；再次点击同一附件即关闭。
    pub(super) fn toggle_composer_attachment_preview(&mut self, id: &str, cx: &mut Context<Self>) {
        let session = self.projection.active_session_id.clone();
        let already_open = matches!(&self.composer_attachment_preview, Some((open_session, open_id))
            if open_session == &session && open_id == id);
        self.composer_attachment_preview = if already_open {
            None
        } else {
            Some((session, id.to_string()))
        };
        cx.notify();
    }

    pub(super) fn close_composer_attachment_preview(&mut self, cx: &mut Context<Self>) {
        if self.composer_attachment_preview.take().is_some() {
            cx.notify();
        }
    }

    /// 预览仅在会话匹配且附件仍存在时渲染（切换任务不显示旧会话浮层）。
    pub(super) fn composer_attachment_preview_target(&self) -> Option<ComposerAttachment> {
        let (session, id) = self.composer_attachment_preview.as_ref()?;
        if session != &self.projection.active_session_id {
            return None;
        }
        self.current_composer_options()
            .attachments
            .into_iter()
            .find(|attachment| &attachment.id == id)
    }

    /// 类型标签 + 大小（render 与 AX 同源）。
    pub(super) fn composer_attachment_meta_label(&self, attachment: &ComposerAttachment) -> String {
        let type_label = if attachment.image {
            i18n::t("composer.attachment_image")
        } else {
            i18n::t("composer.attachment_text")
        };
        format!(
            "{type_label} · {}",
            format_byte_size(attachment.bytes.len())
        )
    }

    /// RV-03：读取失败按类型映射本地化文案与下一步指引（含出错文件名）。
    pub(super) fn composer_attachment_error_text(&self, error: &ComposerAttachmentError) -> String {
        use ComposerAttachmentError as AttachError;
        let (key, name) = match error {
            AttachError::Browser { error } => {
                use pawork_browser::ExternalPageError;
                return match error {
                    ExternalPageError::JavaScriptDisabled(browser) => {
                        i18n::t("composer.browser_javascript_disabled")
                            .replace("{}", browser.name())
                    }
                    ExternalPageError::AutomationDenied(browser) => {
                        i18n::t("composer.browser_automation_denied").replace("{}", browser.name())
                    }
                    ExternalPageError::Other(message) => {
                        format!("{} {message}", i18n::t("composer.browser_error"))
                    }
                };
            }
            AttachError::TooMany { count } => {
                return i18n::t("composer.attach_too_many").replace("{}", &count.to_string());
            }
            AttachError::MissingName => {
                return i18n::t("composer.attach_error_missing_name").into();
            }
            AttachError::NotFile { name } => ("composer.attach_error_not_file", name),
            AttachError::Empty { name } => ("composer.attach_error_empty", name),
            AttachError::TooLarge { name, image: true } => {
                ("composer.attach_error_too_large_image", name)
            }
            AttachError::TooLarge { name, image: false } => {
                ("composer.attach_error_too_large_text", name)
            }
            AttachError::UnsupportedType {
                name,
                images_only: true,
            } => ("composer.attach_error_unsupported_image", name),
            AttachError::UnsupportedType {
                name,
                images_only: false,
            } => ("composer.attach_error_unsupported", name),
            AttachError::Io { name } => ("composer.attach_error_io", name),
            AttachError::FolderLimit { name } => ("composer.attach_error_folder_limit", name),
        };
        i18n::t(key).replace("{}", name)
    }

    /// 附件解码缓存：避免每帧 from_bytes 重建（最大 8 MiB/张）。
    fn composer_attachment_image(
        &mut self,
        attachment: &ComposerAttachment,
    ) -> Option<Arc<gpui::Image>> {
        if let Some(image) = self.composer_attachment_images.get(&attachment.id) {
            return Some(image.clone());
        }
        let format = sniff_image_format(&attachment.bytes)?;
        let image = Arc::new(gpui::Image::from_bytes(
            format,
            attachment.bytes.as_ref().clone(),
        ));
        self.composer_attachment_images
            .insert(attachment.id.clone(), image.clone());
        Some(image)
    }

    /// RV-01：预览浮层——头部名称/类型/大小 + 关闭按钮；图片等比放大，
    /// 文本等宽预览（有界滚动，超 4 KiB 注明截断）。
    fn composer_attachment_preview_element(
        &mut self,
        attachment: &ComposerAttachment,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let meta_label = self.composer_attachment_meta_label(attachment);
        let close_focus = self
            .settings_action_focus
            .entry("composer-preview-close".into())
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let body: gpui::AnyElement = if attachment.image {
            match self.composer_attachment_image(attachment) {
                Some(image) => gpui::img(gpui::ImageSource::Image(image))
                    .w(px(320.0))
                    .h(px(192.0))
                    .object_fit(gpui::ObjectFit::Contain)
                    .rounded(px(metrics::CONTROL_RADIUS))
                    .into_any_element(),
                None => div()
                    .w(px(320.0))
                    .h(px(192.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .child(i18n::t("composer.attachment_image"))
                    .into_any_element(),
            }
        } else {
            let bytes = attachment.bytes.as_ref();
            let truncated = bytes.len() > PREVIEW_TEXT_LIMIT;
            let text =
                String::from_utf8_lossy(&bytes[..bytes.len().min(PREVIEW_TEXT_LIMIT)]).into_owned();
            let mut text_column = div()
                .id("composer-preview-text")
                .w_full()
                .max_h(px(160.0))
                .overflow_y_scroll()
                .p(px(4.0))
                .rounded(px(metrics::CONTROL_RADIUS))
                .bg(dark().bg.base)
                .font_family(font::MONO)
                .text_size(font::XS)
                .child(text);
            if truncated {
                text_column = text_column.child(
                    div()
                        .pt(px(4.0))
                        .text_color(dark().text.tertiary)
                        .child(i18n::t("composer.preview_truncated")),
                );
            }
            text_column.into_any_element()
        };
        self.settings_element("composer-attachment-preview")
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .p(px(metrics::SPACE_2))
            .rounded(px(metrics::CONTROL_RADIUS))
            .bg(dark().bg.panel)
            .border_1()
            .border_color(dark().border.subtle)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(metrics::SPACE_2))
                    .child(
                        // 名称列：flex_row + flex_1 + min_w_0 同层，truncate
                        // 自身不加 min_w_0（AGENTS §11 防真窗口截断）。
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(attachment.name.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(font::XS)
                            .text_color(dark().text.secondary)
                            .child(meta_label),
                    )
                    .child(
                        self.settings_element("composer-preview-close").child(
                            Button::new("composer-preview-close")
                                .variant(ButtonVariant::Ghost)
                                .padding(ButtonPadding::None)
                                .track_focus(&close_focus)
                                .text_size(font::XS)
                                .label("×")
                                .tooltip(i18n::t("composer.preview_close"))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.close_composer_attachment_preview(cx);
                                }))
                                .on_activate(cx.listener(|view, _, _, cx| {
                                    view.close_composer_attachment_preview(cx);
                                    cx.stop_propagation();
                                })),
                        ),
                    ),
            )
            .child(body)
    }

    pub(super) fn composer_attachments_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // 回收已不存在附件的解码缓存，避免跨会话累积。
        let live_ids: HashSet<&str> = self
            .composer_options
            .values()
            .flat_map(|options| options.attachments.iter().map(|a| a.id.as_str()))
            .collect();
        self.composer_attachment_images
            .retain(|id, _| live_ids.contains(id.as_str()));
        let preview_attachment = self.composer_attachment_preview_target();
        let mut column = self
            .settings_element("composer-attachments")
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.0));
        if let Some(attachment) = preview_attachment.as_ref() {
            column = column.child(self.composer_attachment_preview_element(attachment, cx));
        }
        column.child(self.composer_attachment_chips_element(cx))
    }

    /// 附件 chip 行 + 本地化错误行（错误与附件同区域展示，RV-03）。
    fn composer_attachment_chips_element(
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
            let preview_id = format!("composer-preview-{}", attachment.id);
            let remove_id = format!("composer-remove-{}", attachment.id);
            let preview_focus = self
                .settings_action_focus
                .entry(preview_id.clone())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let remove_focus = self
                .settings_action_focus
                .entry(remove_id.clone())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let attachment_id = attachment.id.clone();
            let remove_key = attachment.id.clone();
            let preview_key = attachment.id.clone();
            let preview_activate_key = attachment.id.clone();
            let thumb: gpui::AnyElement = if attachment.image {
                match self.composer_attachment_image(&attachment) {
                    Some(image) => gpui::img(gpui::ImageSource::Image(image))
                        .w(px(22.0))
                        .h(px(22.0))
                        .flex_none()
                        .rounded(px(4.0))
                        .object_fit(gpui::ObjectFit::Contain)
                        .into_any_element(),
                    None => icon_sized(Icon::File, px(metrics::ICON_SM))
                        .flex_none()
                        .into_any_element(),
                }
            } else {
                icon_sized(Icon::File, px(metrics::ICON_SM))
                    .flex_none()
                    .into_any_element()
            };
            // 预览按钮与移除按钮各自携带实测布局，AX 按钮框取各自真实
            // 矩形（RV-01 独立动作）。
            row = row.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .flex_none()
                    .child(
                        self.settings_element(preview_id.clone()).child(
                            Button::new(preview_id.clone())
                                .variant(ButtonVariant::Raised)
                                .track_focus(&preview_focus)
                                .max_width(px(200.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(4.0))
                                        .child(thumb)
                                        .child(div().flex().flex_row().flex_1().min_w_0().child(
                                            div().truncate().child(attachment.name.clone()),
                                        )),
                                )
                                .tooltip(i18n::t("composer.preview_attachment"))
                                .on_click(cx.listener(move |view, event, _, cx| {
                                    if view.consume_button_key_click(
                                        &format!("composer-preview-{preview_key}"),
                                        event,
                                    ) {
                                        return;
                                    }
                                    view.toggle_composer_attachment_preview(&preview_key, cx);
                                }))
                                .on_activate(cx.listener(move |view, _, _, cx| {
                                    view.note_button_key_activate(&format!(
                                        "composer-preview-{preview_activate_key}"
                                    ));
                                    view.toggle_composer_attachment_preview(
                                        &preview_activate_key,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                })),
                        ),
                    )
                    .child(
                        self.settings_element(remove_id.clone()).child(
                            Button::new(remove_id.clone())
                                .variant(ButtonVariant::Raised)
                                .track_focus(&remove_focus)
                                .disabled(self.composer_sending)
                                .text_size(font::XS)
                                .label("×")
                                .tooltip(i18n::t("composer.remove_attachment"))
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.remove_composer_attachment(&attachment_id, cx);
                                }))
                                .on_activate(cx.listener(move |view, _, _, cx| {
                                    view.remove_composer_attachment(&remove_key, cx);
                                    cx.stop_propagation();
                                })),
                        ),
                    ),
            );
        }
        for (index, video) in options.video_urls.iter().enumerate() {
            row = row.child(
                self.settings_element(format!("composer-video-{index}"))
                    .w_full()
                    .text_size(font::XS)
                    .child(format!("{}: {}", i18n::t("video.title"), video.url)),
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
        if let Some(error) = options.attachment_error.as_ref() {
            let message = self.composer_attachment_error_text(error);
            row = row.child(
                div().w_full().flex().flex_row().child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_color(dark().semantic.danger_text).child(message)),
                ),
            );
        }
        row
    }
}

/// 魔数嗅探映射 gpui 解码格式；与 controller 校验共用同一组前缀。
fn sniff_image_format(bytes: &[u8]) -> Option<gpui::ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(gpui::ImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(gpui::ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(gpui::ImageFormat::Gif)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(gpui::ImageFormat::Webp)
    } else {
        None
    }
}

/// 字节数格式化为 B / KiB / MiB（附件 chip 与预览共用）。
fn format_byte_size(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    let size = bytes as f64;
    if size >= KIB * KIB {
        format!("{:.1} MiB", size / (KIB * KIB))
    } else if size >= KIB {
        format!("{:.1} KiB", size / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn attachment_preview_keyboard_and_accessible_close(cx: &mut gpui::TestAppContext) {
        let platform = Arc::new(Platform::new());
        let (view, cx) = cx.add_window_view(|_, cx| {
            AppView::new(
                platform,
                std::env::temp_dir().join("attachment-preview.sock"),
                None,
                cx,
            )
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.composer_options
                    .entry(None)
                    .or_default()
                    .attachments
                    .push(ComposerAttachment {
                        id: "preview-test".into(),
                        name: "preview.txt".into(),
                        bytes: Arc::new(b"preview content".to_vec()),
                        image: false,
                    });
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.update(|window, cx| {
            window.focus(&view.read(cx).settings_action_focus["composer-preview-preview-test"]);
        });
        cx.refresh().unwrap();
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| assert!(view.read(cx).composer_attachment_preview_target().is_some()));
        cx.refresh().unwrap();
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let tree = view.accessibility_tree(window, cx);
                let close = tree
                    .find("composer-preview-close")
                    .expect("preview close AX action");
                assert!(close.bounds.width > 0.0 && close.bounds.height > 0.0);
                view.handle_accessibility_request(
                    accessibility::AxRequest {
                        identifier: "composer-preview-close".into(),
                        action: accessibility::AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
                assert!(view.composer_attachment_preview_target().is_none());
                assert_eq!(view.current_composer_options().attachments.len(), 1);
            });
        });
    }

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
                // RV-03：读取失败按会话落在 ComposerOptions，不再写 status_hint。
                view.handle_controller_event(
                    ControllerEvent::ComposerAttachmentsLoaded {
                        draft: None,
                        result: Err(ComposerAttachmentError::TooLarge {
                            name: "big.png".into(),
                            image: true,
                        }),
                    },
                    cx,
                );
                assert_eq!(
                    view.composer_options
                        .get(&None)
                        .and_then(|o| o.attachment_error.clone()),
                    Some(ComposerAttachmentError::TooLarge {
                        name: "big.png".into(),
                        image: true,
                    })
                );
                assert!(view.status_hint.is_none());
                // RV-01：预览仅会话与附件都匹配时可见。
                view.composer_attachment_preview = Some((None, "picked".into()));
                assert!(view.composer_attachment_preview_target().is_none());
                view.composer_attachment_preview =
                    Some((Some("other-task".into()), "picked".into()));
                assert!(view.composer_attachment_preview_target().is_none());
            })
        });
    }
}
