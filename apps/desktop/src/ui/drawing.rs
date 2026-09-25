//! Composer 的本地手绘图片；保存仅附加原草稿，不自动发送。
use super::*;
use crate::controller::{ComposerAttachment, ComposerAttachmentError};
use gpui::{
    canvas, size, Bounds, MouseButton, MouseDownEvent, MouseMoveEvent, PathBuilder, WindowBounds,
    WindowOptions,
};

use super::accessibility::{AxRequest, AxRole};
use super::product_access::PanelAccess;

const WIDTH: usize = 640;
const HEIGHT: usize = 360;
const MAX_POINTS: usize = 16384;
type Stroke = Vec<(f32, f32)>;

struct Drawing {
    access: PanelAccess,
    owner: gpui::WeakEntity<AppView>,
    draft: Option<String>,
    strokes: Vec<Stroke>,
    dragging: bool,
    layout: ScrollHandle,
    focus: [FocusHandle; 4],
    error: Option<String>,
}

impl AppView {
    pub(super) fn open_drawing(&mut self, cx: &mut Context<Self>) {
        let owner = cx.entity().downgrade();
        let draft = self.projection.active_session_id.clone();
        let bounds = Bounds::centered(None, size(px(680.0), px(480.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(680.0), px(480.0))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(i18n::t("drawing.title").into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let focus = std::array::from_fn(|_| cx.focus_handle().tab_stop(true));
                    window.focus(&focus[0]);
                    Drawing {
                        access: PanelAccess::default(),
                        owner,
                        draft,
                        strokes: Vec::new(),
                        dragging: false,
                        layout: ScrollHandle::new(),
                        focus,
                        error: None,
                    }
                })
            },
        ) {
            self.status_hint = Some(error.to_string());
        }
    }
}

impl Drawing {
    fn point(&self, position: Point<Pixels>) -> (f32, f32) {
        let local = position - self.layout.bounds().origin;
        (
            f32::from(local.x).clamp(0.0, WIDTH as f32 - 1.0),
            f32::from(local.y).clamp(0.0, HEIGHT as f32 - 1.0),
        )
    }

    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.strokes.is_empty() {
            return;
        }
        let bytes = Arc::new(drawing_png(&self.strokes));
        let draft = self.draft.clone();
        let result = self.owner.update(cx, move |owner, cx| {
            if owner.composer_sending {
                return false;
            }
            let options = owner.composer_options.entry(draft.clone()).or_default();
            if options.attachments.len() + options.video_urls.len() >= 4 {
                options.attachment_error = Some(ComposerAttachmentError::TooMany {
                    count: options.attachments.len() + options.video_urls.len() + 1,
                });
                cx.notify();
                return false;
            }
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let id = format!(
                "drawing-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            options.attachments.push(ComposerAttachment {
                id: id.clone(),
                name: "drawing.png".into(),
                bytes,
                image: true,
            });
            options.attachment_error = None;
            owner.composer_attachment_preview = Some((draft, id));
            cx.notify();
            true
        });
        if matches!(result, Ok(true)) {
            window.remove_window();
        } else {
            self.error = Some(i18n::t("drawing.attach_failed").into());
            cx.notify();
        }
    }
}

impl Render for Drawing {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        install_appkit_tab_monitor(window, cx);
        self.access.begin();
        let mut actions = div().flex().gap_3();
        for (index, (id, label)) in [
            ("drawing-undo", "drawing.undo"),
            ("drawing-clear", "drawing.clear"),
            ("drawing-attach", "drawing.attach"),
            ("drawing-cancel", "common.cancel"),
        ]
        .iter()
        .enumerate()
        {
            let enabled = index == 3 || !self.strokes.is_empty();
            let focus = self.focus[index].clone().tab_stop(enabled);
            let button = Button::new(*id)
                .label(i18n::t(label))
                .track_focus(&focus)
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, window, cx| {
                    if AppView::click_down_position(event).is_some() {
                        view.action(index, window, cx);
                    }
                }))
                .on_activate(
                    cx.listener(move |view, _, window, cx| view.action(index, window, cx)),
                );
            actions = actions.child(self.access.wrap(
                id,
                i18n::t(label),
                AxRole::Button,
                None,
                enabled,
                self.focus[index].is_focused(window),
                button,
            ));
        }
        let hint = self
            .error
            .clone()
            .unwrap_or_else(|| i18n::t("drawing.hint").into());
        let hint = self.access.wrap(
            "drawing-status",
            &hint,
            AxRole::StaticText,
            None,
            false,
            false,
            hint.clone(),
        );
        self.access.sync(window, cx, Self::ax_action);
        let strokes = self.strokes.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(dark().bg.base)
            .text_color(dark().text.primary)
            .child(hint)
            .child(
                div()
                    .id("drawing-canvas")
                    .track_scroll(&self.layout)
                    .w(px(WIDTH as f32))
                    .h(px(HEIGHT as f32))
                    .flex_none()
                    .bg(gpui::rgb(0xffffff))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, event: &MouseDownEvent, _, cx| {
                            if view.strokes.len() < 1024
                                && view.strokes.iter().map(Vec::len).sum::<usize>() < MAX_POINTS
                            {
                                view.strokes.push(vec![view.point(event.position)]);
                                view.dragging = true;
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, cx| {
                        if view.dragging
                            && event.pressed_button == Some(MouseButton::Left)
                            && view.strokes.iter().map(Vec::len).sum::<usize>() < MAX_POINTS
                        {
                            let point = view.point(event.position);
                            if let Some(stroke) = view.strokes.last_mut() {
                                if stroke.len() < 4096 {
                                    stroke.push(point);
                                    cx.notify();
                                }
                            }
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|view, _, _, _| view.dragging = false),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|view, _, _, _| view.dragging = false),
                    )
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                for stroke in &strokes {
                                    let mut path = PathBuilder::stroke(px(3.0));
                                    if let Some(&(x, y)) = stroke.first() {
                                        path.move_to(bounds.origin + point(px(x), px(y)));
                                        if stroke.len() == 1 {
                                            path.line_to(
                                                bounds.origin + point(px(x + 0.1), px(y + 0.1)),
                                            );
                                        }
                                        for &(x, y) in &stroke[1..] {
                                            path.line_to(bounds.origin + point(px(x), px(y)));
                                        }
                                        if let Ok(path) = path.build() {
                                            window.paint_path(path, gpui::rgb(0x111111));
                                        }
                                    }
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
            .child(actions)
    }
}

/// 固定 RGB 画布，PNG 使用无压缩 DEFLATE；大小固定 <700 KiB，无新增图像依赖。
fn drawing_png(strokes: &[Stroke]) -> Vec<u8> {
    let mut pixels = vec![255; WIDTH * HEIGHT * 3];
    let mut dot = |x: f32, y: f32| {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let x = x.round() as i32 + dx;
                let y = y.round() as i32 + dy;
                if x >= 0 && x < WIDTH as i32 && y >= 0 && y < HEIGHT as i32 {
                    let offset = (y as usize * WIDTH + x as usize) * 3;
                    pixels[offset..offset + 3].fill(17);
                }
            }
        }
    };
    for stroke in strokes {
        if let Some(&(x, y)) = stroke.first() {
            dot(x, y);
        }
        for segment in stroke.windows(2) {
            let (x, y) = segment[0];
            let (tx, ty) = segment[1];
            let steps = ((tx - x).abs().max((ty - y).abs()).ceil() as usize).max(1);
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                dot(x + (tx - x) * t, y + (ty - y) * t);
            }
        }
    }
    let mut raw = Vec::with_capacity((WIDTH * 3 + 1) * HEIGHT);
    for row in pixels.chunks_exact(WIDTH * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    for (i, chunk) in raw.chunks(65535).enumerate() {
        zlib.push(u8::from((i + 1) * 65535 >= raw.len()));
        let len = chunk.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&(WIDTH as u32).to_be_bytes());
    header.extend_from_slice(&(HEIGHT as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    for (kind, data) in [
        (b"IHDR", header.as_slice()),
        (b"IDAT", zlib.as_slice()),
        (b"IEND", &[][..]),
    ] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        let mut crc = !0u32;
        for byte in kind.iter().chain(data) {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        png.extend_from_slice(&(!crc).to_be_bytes());
    }
    png
}

impl Drawing {
    fn action(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        match index {
            0 => {
                self.strokes.pop();
            }
            1 => self.strokes.clear(),
            2 => self.attach(window, cx),
            3 => window.remove_window(),
            _ => {}
        }
        cx.notify();
    }
    fn ax_action(&mut self, request: AxRequest, window: &mut Window, cx: &mut Context<Self>) {
        if !self.access.permits(&request) {
            return;
        }
        if let Some(index) = [
            "drawing-undo",
            "drawing-clear",
            "drawing-attach",
            "drawing-cancel",
        ]
        .iter()
        .position(|id| *id == request.identifier)
        {
            self.action(index, window, cx);
        }
    }
}
