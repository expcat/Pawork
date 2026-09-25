//! Remote video references are edited locally and sent only with the next explicit turn.
use super::accessibility::{AxRequest, AxRole};
use super::product_access::{edit_input, PanelAccess};
use super::*;
use gpui::{size, Bounds, WindowBounds, WindowOptions};

struct VideoView {
    owner: gpui::WeakEntity<AppView>,
    draft: Option<String>,
    urls: Entity<TextInput>,
    focus: [FocusHandle; 2],
    access: PanelAccess,
    error: Option<String>,
}
impl AppView {
    pub(super) fn open_video(&mut self, cx: &mut Context<Self>) {
        let owner = cx.entity().downgrade();
        let draft = self.projection.active_session_id.clone();
        let text = self
            .current_composer_options()
            .video_urls
            .iter()
            .map(|v| v.url.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let bounds = Bounds::centered(None, size(px(700.0), px(420.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(i18n::t("video.title").into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let urls = cx.new(|cx| {
                        let mut field = TextInput::with_placeholder(i18n::t("video.urls"), cx)
                            .id("video-urls")
                            .tab_stop(true)
                            .code_editor()
                            .height_clamp(160.0, 240.0);
                        field.set_text(text, cx);
                        field
                    });
                    window.focus(&urls.focus_handle(cx));
                    VideoView {
                        owner,
                        draft,
                        urls,
                        focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
                        access: PanelAccess::default(),
                        error: None,
                    }
                })
            },
        ) {
            self.status_hint = Some(error.to_string());
        }
    }
}
impl VideoView {
    fn action(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == 1 {
            window.remove_window();
            return;
        }
        let videos: Vec<_> = self
            .urls
            .read(cx)
            .text()
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|url| {
                let extension = url
                    .split(['?', '#'])
                    .next()
                    .unwrap_or(url)
                    .rsplit('.')
                    .next()
                    .unwrap_or("");
                pawork_client::VideoContent {
                    url: url.into(),
                    media_type: match extension.to_ascii_lowercase().as_str() {
                        "webm" => "video/webm",
                        "mov" => "video/quicktime",
                        "mpeg" | "mpg" => "video/mpeg",
                        "avi" => "video/x-msvideo",
                        _ => "video/mp4",
                    }
                    .into(),
                }
            })
            .collect();
        if videos.len() > 4 || videos.iter().any(|v| v.validate().is_err()) {
            self.error = Some(i18n::t("video.invalid").into());
            cx.notify();
            return;
        }
        let draft = self.draft.clone();
        let saved = self.owner.update(cx, move |view, cx| {
            if view.composer_sending {
                return false;
            }
            let options = view.composer_options.entry(draft).or_default();
            if options.attachments.len() + videos.len() > 4 {
                return false;
            }
            options.video_urls = videos;
            cx.notify();
            true
        });
        if matches!(saved, Ok(true)) {
            window.remove_window();
        } else {
            self.error = Some(i18n::t("video.invalid").into());
            cx.notify();
        }
    }
    fn ax_action(&mut self, request: AxRequest, window: &mut Window, cx: &mut Context<Self>) {
        if !self.access.permits(&request) {
            return;
        }
        match request.identifier.as_str() {
            "video-urls" => edit_input(&self.urls, request, window, cx),
            "video-save" => self.action(0, window, cx),
            "video-cancel" => self.action(1, window, cx),
            _ => {}
        }
    }
}
impl Render for VideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        install_appkit_tab_monitor(window, cx);
        self.access.begin();
        let urls = self.access.wrap(
            "video-urls",
            i18n::t("video.urls"),
            AxRole::TextArea,
            Some(self.urls.read(cx).text().into()),
            true,
            self.urls.focus_handle(cx).is_focused(window),
            self.urls.clone(),
        );
        let mut actions = div().flex().gap_3();
        for (i, (id, label)) in [
            ("video-save", "video.save"),
            ("video-cancel", "common.cancel"),
        ]
        .iter()
        .enumerate()
        {
            let button = Button::new(*id)
                .label(i18n::t(label))
                .track_focus(&self.focus[i])
                .on_click(cx.listener(move |view, event, window, cx| {
                    if AppView::click_down_position(event).is_some() {
                        view.action(i, window, cx);
                    }
                }))
                .on_activate(cx.listener(move |view, _, window, cx| view.action(i, window, cx)));
            actions = actions.child(self.access.wrap(
                id,
                i18n::t(label),
                AxRole::Button,
                None,
                true,
                self.focus[i].is_focused(window),
                button,
            ));
        }
        let status = self
            .error
            .clone()
            .unwrap_or_else(|| i18n::t("video.hint").into());
        let status = self.access.wrap(
            "video-status",
            &status,
            AxRole::StaticText,
            None,
            false,
            false,
            status.clone(),
        );
        self.access.sync(window, cx, Self::ax_action);
        div()
            .id("video-panel")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .bg(dark().bg.base)
            .text_color(dark().text.primary)
            .child(status)
            .child(urls)
            .child(actions)
    }
}
