//! 终端显示的 gpui 胶水层。
//!
//! 行缓冲解析（CR / 退格 / EL / SGR）、按键到 PTY 字节映射与面板像素到列行
//! 估算在 [pawork_terminal]；本模块只把属性分段映射成主题色 StyledText runs，
//! 测量字符列宽、渲染行内光标，并用原生 EntityInputHandler 接入输入法。

use gpui::{
    font, App, Bounds, Context, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    FontWeight, Keystroke, Pixels, Point, Rgba, SharedString, StyledText, TextRun, UTF16Selection,
    Window,
};
use std::ops::Range;

use super::theme::{dark, font as theme_font};

pub(crate) use pawork_terminal::{size_from_bounds_scaled, TERMINAL_LINE_HEIGHT};

/// 面板像素变化后延迟下发 terminal_resize，避免拖拽窗口时连发。
pub(crate) const TERMINAL_RESIZE_DEBOUNCE: std::time::Duration =
    std::time::Duration::from_millis(80);

const SGR_PALETTE: [u32; 16] = [
    0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666,
    0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
];

fn palette_color(index: u8) -> Rgba {
    gpui::rgb(SGR_PALETTE[index.min(15) as usize])
}

fn cell_color(attrs: pawork_terminal::Attrs) -> (Rgba, Option<Rgba>) {
    let theme = dark();
    let mut fg = attrs.fg.map(palette_color).unwrap_or(theme.text.emphasis);
    let mut bg = attrs.bg.map(palette_color);
    if attrs.inverse {
        let inverted_fg = bg.unwrap_or(theme.bg.base);
        bg = Some(fg);
        fg = inverted_fg;
    }
    if attrs.dim && !attrs.bold {
        fg = theme.text.secondary;
    }
    (fg, bg)
}

fn run_for(attrs: pawork_terminal::Attrs, len: usize) -> TextRun {
    let (color, background) = cell_color(attrs);
    let mut face = font(theme_font::MONO);
    if attrs.bold {
        face.weight = FontWeight::BOLD;
    }
    TextRun {
        len,
        font: face,
        color: color.into(),
        background_color: background.map(Into::into),
        underline: None,
        strikethrough: None,
    }
}

/// 用当前等宽字体测量非 ASCII 字符的列宽，避免 CJK 回移覆盖提示符。
pub(crate) fn terminal_screen(
    raw: &str,
    columns: u16,
    rows: u16,
    window: &Window,
) -> pawork_terminal::Screen {
    let mut screen = pawork_terminal::Screen::with_size(columns, rows);
    let mut widths = std::collections::HashMap::new();
    let mut run = run_for(pawork_terminal::Attrs::default(), 1);
    let cell = f32::from(
        window
            .text_system()
            .shape_line("m".into(), gpui::px(12.0), &[run.clone()], None)
            .width,
    )
    .max(1.0);
    screen.feed_with_width(raw, |ch| {
        if ch.is_ascii() {
            return 1;
        }
        *widths.entry(ch).or_insert_with(|| {
            run.len = ch.len_utf8();
            let width = window
                .text_system()
                .shape_line(ch.to_string().into(), gpui::px(12.0), &[run.clone()], None)
                .width;
            (f32::from(width) / cell).round().clamp(0.0, 2.0) as usize
        })
    });
    screen
}

/// 光标与预编辑文字嵌在 shell 的实际光标位置，不维护第二份命令草稿。
pub(crate) fn render_terminal_lines(
    screen: &pawork_terminal::Screen,
    preedit: &str,
    caret: bool,
) -> Vec<(SharedString, StyledText)> {
    let (cursor_row, _) = screen.cursor();
    screen
        .display_lines()
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            let mut text = line.text;
            let mut runs = line
                .spans
                .iter()
                .map(|span| run_for(span.attrs, span.len))
                .collect::<Vec<_>>();
            if runs.is_empty() {
                runs.push(run_for(pawork_terminal::Attrs::default(), text.len()));
            }
            if row == cursor_row && (caret || !preedit.is_empty()) {
                let cursor = screen.cursor_byte_offset().min(text.len());
                let char_len = text[cursor..].chars().next().map_or(0, char::len_utf8);
                let mut offset = 0;
                let mut cursor_runs = Vec::new();
                for run in runs {
                    let end = offset + run.len;
                    if offset <= cursor && cursor < end {
                        if cursor > offset {
                            cursor_runs.push(TextRun {
                                len: cursor - offset,
                                ..run.clone()
                            });
                        }
                        if !preedit.is_empty() {
                            cursor_runs.push(TextRun {
                                len: preedit.len(),
                                underline: Some(gpui::UnderlineStyle {
                                    thickness: gpui::px(1.0),
                                    color: Some(dark().text.primary.into()),
                                    wavy: false,
                                }),
                                ..run.clone()
                            });
                        }
                        cursor_runs.push(TextRun {
                            len: char_len,
                            color: if caret {
                                dark().bg.base.into()
                            } else {
                                run.color
                            },
                            background_color: if caret {
                                Some(dark().text.emphasis.into())
                            } else {
                                run.background_color
                            },
                            ..run.clone()
                        });
                        if end > cursor + char_len {
                            cursor_runs.push(TextRun {
                                len: end - cursor - char_len,
                                ..run
                            });
                        }
                    } else {
                        cursor_runs.push(run);
                    }
                    offset = end;
                }
                text.insert_str(cursor, preedit);
                runs = cursor_runs;
            }
            (
                SharedString::from(text.clone()),
                StyledText::new(text).with_runs(runs),
            )
        })
        .collect()
}

/// 原生输入法接入点。只缓存尚未提交的 IME 预编辑；提交立即交给 PTY。
pub(crate) struct TerminalInput {
    focus: FocusHandle,
    preedit: String,
    selection: Range<usize>,
    pub cursor_bounds: Option<Bounds<Pixels>>,
}

pub(crate) enum TerminalInputEvent {
    Write(String),
    Changed,
}
impl EventEmitter<TerminalInputEvent> for TerminalInput {}
impl Focusable for TerminalInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl TerminalInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle().tab_stop(true),
            preedit: String::new(),
            selection: 0..0,
            cursor_bounds: None,
        }
    }
    pub fn preedit(&self) -> &str {
        &self.preedit
    }
    pub fn is_composing(&self) -> bool {
        !self.preedit.is_empty()
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.preedit.clear();
        self.selection = 0..0;
        cx.emit(TerminalInputEvent::Changed);
        cx.notify();
    }
}
impl EntityInputHandler for TerminalInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.preedit.encode_utf16().collect::<Vec<_>>();
        let start = range.start.min(text.len());
        let range = start..range.end.min(text.len()).max(start);
        *actual = Some(range.clone());
        String::from_utf16(&text[range]).ok()
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.is_composing()
            .then(|| 0..self.preedit.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.clear(cx);
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear(cx);
        if !text.is_empty() {
            cx.emit(TerminalInputEvent::Write(text.to_owned()));
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut utf16 = self.preedit.encode_utf16().collect::<Vec<_>>();
        let range = range.unwrap_or(0..utf16.len());
        let start = range.start.min(utf16.len());
        utf16.splice(
            start..range.end.min(utf16.len()).max(start),
            text.encode_utf16(),
        );
        self.preedit = String::from_utf16_lossy(&utf16);
        self.selection = selected
            .map(|r| start + r.start..start + r.end)
            .unwrap_or(utf16.len()..utf16.len());
        cx.emit(TerminalInputEvent::Changed);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.cursor_bounds.unwrap_or(bounds))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.selection.end)
    }
}

/// 焦点在终端窗口时，把 gpui 按键映射成 PTY 字节。
pub(crate) fn keystroke_to_pty_bytes(keystroke: &Keystroke) -> Option<String> {
    pawork_terminal::key_to_pty_bytes(&pawork_terminal::KeyEvent {
        key: keystroke.key.to_string(),
        key_char: keystroke.key_char.as_ref().map(ToString::to_string),
        modifiers: pawork_terminal::Modifiers {
            control: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
            shift: keystroke.modifiers.shift,
            platform: keystroke.modifiers.platform,
            function: keystroke.modifiers.function,
        },
    })
}

pub(crate) fn terminal_text_size() -> gpui::Rems {
    theme_font::SM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_history_and_inline_composition_have_complete_text_runs() {
        let mut screen = pawork_terminal::Screen::with_size(40, 10);
        screen.feed("\r\n\r\n$ \x1b[31mecho\x1b[0m 中\x1b[1D");
        for preedit in ["", "中文"] {
            let lines = render_terminal_lines(&screen, preedit, true);
            assert_eq!(lines.len(), 3);
            assert_eq!(lines[0].0.as_ref(), " ");
            assert!(lines[2].0.contains(preedit));
        }
    }

    #[test]
    fn gpui_keystrokes_convert_to_pty_bytes() {
        let ctrl_c = gpui::Keystroke::parse("ctrl-c").unwrap();
        assert_eq!(keystroke_to_pty_bytes(&ctrl_c).as_deref(), Some("\u{3}"));
        let up = gpui::Keystroke::parse("up").unwrap();
        assert_eq!(keystroke_to_pty_bytes(&up).as_deref(), Some("\u{1b}[A"));
        let shift_a = gpui::Keystroke {
            modifiers: gpui::Modifiers {
                shift: true,
                ..gpui::Modifiers::none()
            },
            key: "a".into(),
            key_char: Some("A".into()),
        };
        assert_eq!(keystroke_to_pty_bytes(&shift_a).as_deref(), Some("A"));
        let cmd_a = gpui::Keystroke::parse("cmd-a").unwrap();
        assert_eq!(keystroke_to_pty_bytes(&cmd_a), None);
    }
}
