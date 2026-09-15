//! 终端显示的 gpui 胶水层。
//!
//! 行缓冲解析（CR / 退格 / EL / SGR）、按键到 PTY 字节映射与面板像素到列行
//! 估算在 [pawork_terminal]；本模块只把属性分段映射成主题色 StyledText runs，
//! 并把 gpui Keystroke 转成平台无关的 KeyEvent。

use gpui::{font, FontWeight, Keystroke, Rgba, SharedString, StyledText, TextRun};

use super::theme::{dark, font as theme_font};

pub(crate) use pawork_terminal::{
    should_passthrough_terminal_key, size_from_bounds_scaled, TERMINAL_LINE_HEIGHT,
};

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

/// 可见行缓冲：CR 覆盖同一行，退格删格，SGR 进入着色 runs。
pub(crate) fn render_terminal_lines(raw: &str) -> Vec<(SharedString, StyledText)> {
    pawork_terminal::render_lines(raw)
        .into_iter()
        .map(|line| {
            let runs = line
                .spans
                .iter()
                .map(|span| run_for(span.attrs, span.len))
                .collect::<Vec<_>>();
            (
                SharedString::from(line.text.clone()),
                StyledText::new(line.text).with_runs(runs),
            )
        })
        .collect()
}

/// AX / 纯文本路径：保留 CR 覆盖与退格，剥离其余控制序列。
pub(crate) fn plain_terminal_output(raw: &str) -> String {
    pawork_terminal::plain_output(raw)
}

/// 焦点在终端输入框时，把 gpui 按键映射成 PTY 字节。
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
