//! 终端显示核心：行缓冲解析、SGR 属性、按键到 PTY 字节映射、面板像素到列行估算。
//!
//! 不是完整 VT emulator。原始字节仍由 Host 保存；本包只把可见文本整理成
//! 行覆盖语义（CR / 退格 / EL / SGR），并把控制键映射成 PTY 字节。
//! CUP / CUU / CUD 等网格寻址序列会改写滚动历史，直接丢弃，不假装网格仿真。
//! 纯库：不依赖 gpui、tokio、OS API 与任何 pawork-* 包，颜色与字体由调用方解析。

/// 等宽终端行高（12px 字号 + 4px 行距）。
pub const TERMINAL_LINE_HEIGHT: f32 = 16.0;
/// 等宽字符宽按 Menlo 12px 的常见 cell 宽估算。
pub const TERMINAL_CELL_WIDTH: f32 = 7.2;
/// 输出面内边距（与 Inspector 面板 px_2 / py_1 同源），测量列行时扣掉。
pub const TERMINAL_OUTPUT_PAD_X: f32 = 8.0;
/// 输出面内边距（与 Inspector 面板 px_2 / py_1 同源），测量列行时扣掉。
pub const TERMINAL_OUTPUT_PAD_Y: f32 = 4.0;

/// 单个 cell 的显示属性：16 色索引（0-15，由调用方映射到具体色值）与修饰位。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub fg: Option<u8>,
    pub bg: Option<u8>,
    pub bold: bool,
    pub dim: bool,
    pub inverse: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cell {
    ch: char,
    attrs: Attrs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    cells: Vec<Cell>,
}

impl Line {
    fn new() -> Self {
        Self { cells: Vec::new() }
    }

    fn is_empty(&self) -> bool {
        self.cells.iter().all(|cell| cell.ch == ' ')
    }

    fn put(&mut self, col: usize, ch: char, attrs: Attrs) {
        if col >= self.cells.len() {
            self.cells.resize(
                col + 1,
                Cell {
                    ch: ' ',
                    attrs: Attrs::default(),
                },
            );
        }
        self.cells[col] = Cell { ch, attrs };
    }

    fn clear_from(&mut self, col: usize) {
        if col < self.cells.len() {
            self.cells.truncate(col);
        }
    }

    fn text(&self) -> String {
        self.cells
            .iter()
            .map(|cell| cell.ch)
            .collect::<String>()
            .trim_end()
            .to_string()
    }
}

/// 行缓冲终端：处理 CR 覆盖、退格、SGR 与少量 CSI 擦除，其余控制序列丢弃。
#[derive(Clone, Debug)]
pub struct Screen {
    lines: Vec<Line>,
    row: usize,
    col: usize,
    attrs: Attrs,
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen {
    pub fn new() -> Self {
        Self {
            lines: vec![Line::new()],
            row: 0,
            col: 0,
            attrs: Attrs::default(),
        }
    }

    fn current_line(&mut self) -> &mut Line {
        if self.row >= self.lines.len() {
            self.lines.resize(self.row + 1, Line::new());
        }
        &mut self.lines[self.row]
    }

    fn carriage_return(&mut self) {
        self.col = 0;
    }

    fn line_feed(&mut self) {
        self.row += 1;
        if self.row >= self.lines.len() {
            self.lines.push(Line::new());
        }
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let col = self.col;
            self.current_line().put(col, ' ', Attrs::default());
        }
    }

    fn put_char(&mut self, ch: char) {
        match ch {
            '\r' => self.carriage_return(),
            '\n' => self.line_feed(),
            '\u{8}' | '\u{7f}' => {
                self.backspace();
            }
            '\t' => {
                let next = ((self.col / 8) + 1) * 8;
                while self.col < next {
                    let col = self.col;
                    let attrs = self.attrs;
                    self.current_line().put(col, ' ', attrs);
                    self.col += 1;
                }
            }
            '\u{7}' => {}
            ch if ch.is_control() => {}
            ch => {
                let col = self.col;
                let attrs = self.attrs;
                self.current_line().put(col, ch, attrs);
                self.col += 1;
            }
        }
    }

    fn apply_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.attrs = Attrs::default();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.attrs = Attrs::default(),
                1 => self.attrs.bold = true,
                2 => self.attrs.dim = true,
                7 => self.attrs.inverse = true,
                22 => {
                    self.attrs.bold = false;
                    self.attrs.dim = false;
                }
                27 => self.attrs.inverse = false,
                30..=37 => self.attrs.fg = Some((params[i] - 30) as u8),
                39 => self.attrs.fg = None,
                40..=47 => self.attrs.bg = Some((params[i] - 40) as u8),
                49 => self.attrs.bg = None,
                90..=97 => self.attrs.fg = Some((params[i] - 82) as u8),
                100..=107 => self.attrs.bg = Some((params[i] - 92) as u8),
                38 | 48 => {
                    let is_fg = params[i] == 38;
                    if i + 1 < params.len() && params[i + 1] == 5 && i + 2 < params.len() {
                        let idx = params[i + 2].min(15) as u8;
                        if is_fg {
                            self.attrs.fg = Some(idx);
                        } else {
                            self.attrs.bg = Some(idx);
                        }
                        i += 2;
                    } else if i + 1 < params.len() && params[i + 1] == 2 {
                        i += 4.min(params.len().saturating_sub(i) - 1);
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn erase_in_line(&mut self, param: u16) {
        match param {
            1 => {
                for col in 0..=self.col {
                    self.current_line().put(col, ' ', Attrs::default());
                }
            }
            2 => *self.current_line() = Line::new(),
            _ => {
                let col = self.col;
                self.current_line().clear_from(col);
            }
        }
    }

    /// 追加一段原始输出（已按 UTF-8 解码）。
    pub fn feed(&mut self, raw: &str) {
        let mut chars = raw.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\u{1b}' {
                self.put_char(ch);
                continue;
            }
            match chars.next() {
                Some('[') => {
                    let mut body = String::new();
                    let mut final_byte = None;
                    for sequence_char in chars.by_ref() {
                        if ('@'..='~').contains(&sequence_char) {
                            final_byte = Some(sequence_char);
                            break;
                        }
                        body.push(sequence_char);
                    }
                    let params = parse_params(&body);
                    match final_byte {
                        Some('m') => self.apply_sgr(&params),
                        Some('K') => self.erase_in_line(params.first().copied().unwrap_or(0)),
                        Some('G') => {
                            let col = params.first().copied().unwrap_or(1).max(1) as usize;
                            self.col = col.saturating_sub(1);
                        }
                        Some('C') => {
                            self.col += params.first().copied().unwrap_or(1).max(1) as usize;
                        }
                        Some('D') => {
                            self.col = self
                                .col
                                .saturating_sub(params.first().copied().unwrap_or(1).max(1) as usize);
                        }
                        _ => {}
                    }
                }
                Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
                    let mut saw_escape = false;
                    for sequence_char in chars.by_ref() {
                        if sequence_char == '\u{7}' || (saw_escape && sequence_char == '\\') {
                            break;
                        }
                        saw_escape = sequence_char == '\u{1b}';
                    }
                }
                Some(_) | None => {}
            }
        }
    }

    /// 可见行：裁掉尾部空行，每行给出裁过尾随空格的文本与按字节长度的属性分段。
    /// 空行以单空格占位，保持行高与纯文本行数。
    fn styled_lines(&self) -> Vec<StyledLine> {
        let mut lines = self.lines.clone();
        while lines.last().is_some_and(Line::is_empty) && lines.len() > 1 {
            lines.pop();
        }
        lines
            .into_iter()
            .map(|line| {
                let text = line.text();
                if text.is_empty() {
                    return StyledLine {
                        text: " ".to_string(),
                        spans: Vec::new(),
                    };
                }
                let mut spans = Vec::new();
                let mut start = 0usize;
                let mut current = line.cells.first().map(|cell| cell.attrs);
                let mut byte_len = 0usize;
                let visible = text.chars().count();
                for (index, cell) in line.cells.iter().enumerate() {
                    if index >= visible {
                        break;
                    }
                    let ch_len = cell.ch.len_utf8();
                    if current != Some(cell.attrs) {
                        if let Some(attrs) = current {
                            spans.push(Span {
                                len: byte_len - start,
                                attrs,
                            });
                        }
                        start = byte_len;
                        current = Some(cell.attrs);
                    }
                    byte_len += ch_len;
                }
                if let Some(attrs) = current {
                    spans.push(Span {
                        len: byte_len - start,
                        attrs,
                    });
                }
                StyledLine { text, spans }
            })
            .collect()
    }
}

fn parse_params(body: &str) -> Vec<u16> {
    if body.is_empty() {
        return Vec::new();
    }
    body.split(';')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// 一段同属性文本；len 为 UTF-8 字节长度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub len: usize,
    pub attrs: Attrs,
}

/// 一行可见文本与其属性分段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledLine {
    pub text: String,
    pub spans: Vec<Span>,
}

/// 可见行缓冲：CR 覆盖同一行，退格删格，SGR 进入属性分段。
pub fn render_lines(raw: &str) -> Vec<StyledLine> {
    let mut screen = Screen::new();
    screen.feed(raw);
    screen.styled_lines()
}

/// 纯文本路径：保留 CR 覆盖与退格，剥离其余控制序列。
pub fn plain_output(raw: &str) -> String {
    render_lines(raw)
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 按面板像素估算列 × 行，并钳制在 stepper 边界内。
pub fn size_from_bounds(width: f32, height: f32) -> Option<(u16, u16)> {
    size_from_bounds_scaled(width, height, 1.0)
}

/// rem_scale 为当前窗口 rem_px / 16；字号放大时 cell 变大，列数变少。
pub fn size_from_bounds_scaled(width: f32, height: f32, rem_scale: f32) -> Option<(u16, u16)> {
    let rem_scale = rem_scale.max(0.5);
    let content_width = (width - TERMINAL_OUTPUT_PAD_X * 2.0 * rem_scale).max(0.0);
    let content_height = (height - TERMINAL_OUTPUT_PAD_Y * 2.0 * rem_scale).max(0.0);
    if content_width < 40.0 || content_height < 24.0 {
        return None;
    }
    let cell = TERMINAL_CELL_WIDTH * rem_scale;
    let line = TERMINAL_LINE_HEIGHT * rem_scale;
    let columns = ((content_width / cell).floor() as i32).clamp(20, 500) as u16;
    let rows = ((content_height / line).floor() as i32).clamp(6, 200) as u16;
    Some((columns, rows))
}

/// 按键修饰位，与平台无关；由调用方从自己的按键事件转换。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
    pub function: bool,
}

/// 一次按键；key 为键名（如 "up" / "a"），key_char 为实际输入字符。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: String,
    pub key_char: Option<String>,
    pub modifiers: Modifiers,
}

impl KeyEvent {
    pub fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
            key_char: None,
            modifiers: Modifiers::default(),
        }
    }
}

/// 输入框已有正文或选区时，把可打印字符 / 方向键留给输入框；
/// 中断类控制键（Ctrl-C/D/Z）始终直通，选中文本时的 Ctrl-C 除外（复制）。
pub fn should_passthrough_terminal_key(
    key: &str,
    control: bool,
    has_text: bool,
    has_selection: bool,
) -> bool {
    if key == "c" && control && has_selection {
        return false;
    }
    if control && matches!(key, "c" | "d" | "z") {
        return true;
    }
    if has_text || has_selection {
        return false;
    }
    true
}

/// 把按键映射成 PTY 字节。
///
/// 输入框空且无选区时，可打印字符与 Backspace/Delete 也直通；有草稿时
/// 这些键仍走输入框，由 Enter 整行提交（草稿让路由 should_passthrough_terminal_key 决定）。
pub fn key_to_pty_bytes(key: &KeyEvent) -> Option<String> {
    let key_name = key.key.as_str();
    let modifiers = &key.modifiers;
    if modifiers.platform || modifiers.function {
        return None;
    }
    if modifiers.control && !modifiers.alt {
        let ctrl = match key_name {
            "c" => "\u{3}",
            "d" => "\u{4}",
            "z" => "\u{1a}",
            "l" => "\u{c}",
            "u" => "\u{15}",
            "w" => "\u{17}",
            "r" => "\u{12}",
            _ => return None,
        };
        return Some(ctrl.to_string());
    }
    if modifiers.alt {
        return None;
    }
    match key_name {
        "tab" if !modifiers.shift => Some("\t".to_string()),
        "tab" if modifiers.shift => Some("\u{1b}[Z".to_string()),
        "up" => Some("\u{1b}[A".to_string()),
        "down" => Some("\u{1b}[B".to_string()),
        "left" => Some("\u{1b}[D".to_string()),
        "right" => Some("\u{1b}[C".to_string()),
        "home" => Some("\u{1b}[H".to_string()),
        "end" => Some("\u{1b}[F".to_string()),
        "escape" => Some("\u{1b}".to_string()),
        "backspace" => Some("\u{7f}".to_string()),
        "delete" => Some("\u{1b}[3~".to_string()),
        _ => printable_pty_bytes(key),
    }
}

fn printable_pty_bytes(key: &KeyEvent) -> Option<String> {
    let modifiers = &key.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return None;
    }
    let typed = key
        .key_char
        .as_deref()
        .filter(|text| !text.is_empty() && text.chars().all(|ch| !ch.is_control()))?;
    Some(typed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carriage_return_overwrites_the_same_line() {
        assert_eq!(
            plain_output("downloading 10%\rdownloading 100%\n"),
            "downloading 100%"
        );
    }

    #[test]
    fn backspace_moves_the_cursor_back() {
        assert_eq!(plain_output("ab\u{8}c"), "ac");
        assert_eq!(plain_output("abc\u{8}"), "ab");
    }

    #[test]
    fn vt_control_sequences_are_stripped() {
        assert_eq!(
            plain_output("\u{1b}[?2004hpwd\u{1b}[?2004l\r\n/workspace\r\n"),
            "pwd\n/workspace"
        );
    }

    #[test]
    fn sgr_color_does_not_leak_into_plain_text() {
        assert_eq!(plain_output("\u{1b}[32mgreen\u{1b}[0m rest"), "green rest");
    }

    #[test]
    fn sgr_marks_spans_with_color_indexes() {
        let lines = render_lines("\u{1b}[31;1mERR\u{1b}[0m ok");
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(line.text, "ERR ok");
        assert_eq!(
            line.spans,
            vec![
                Span {
                    len: 3,
                    attrs: Attrs {
                        fg: Some(1),
                        bold: true,
                        ..Attrs::default()
                    },
                },
                Span {
                    len: 3,
                    attrs: Attrs::default()
                },
            ]
        );
    }

    #[test]
    fn erase_in_line_clears_from_cursor() {
        assert_eq!(plain_output("hello\r\u{1b}[Kworld"), "world");
    }

    #[test]
    fn cursor_position_sequences_do_not_rewrite_history() {
        assert_eq!(
            plain_output("first\nsecond\u{1b}[Hrewritten"),
            "first\nsecondrewritten"
        );
        assert_eq!(
            plain_output("alpha\nbeta\u{1b}[1Azzz"),
            "alpha\nbetazzz"
        );
    }

    #[test]
    fn size_from_bounds_clamps_to_existing_limits() {
        assert_eq!(size_from_bounds(576.0, 384.0), Some((77, 23)));
        assert_eq!(size_from_bounds(10.0, 10.0), None);
        let (columns, rows) = size_from_bounds(10_000.0, 10_000.0).unwrap();
        assert_eq!((columns, rows), (500, 200));
        assert_eq!(size_from_bounds_scaled(576.0, 384.0, 1.5), Some((51, 15)));
    }

    #[test]
    fn passthrough_respects_draft_and_selection() {
        assert!(should_passthrough_terminal_key("up", false, false, false));
        assert!(!should_passthrough_terminal_key("up", false, true, false));
        assert!(!should_passthrough_terminal_key("tab", false, true, false));
        assert!(should_passthrough_terminal_key("c", true, true, false));
        assert!(!should_passthrough_terminal_key("c", true, true, true));
        assert!(should_passthrough_terminal_key("a", false, false, false));
        assert!(!should_passthrough_terminal_key("a", false, true, false));
        assert!(should_passthrough_terminal_key("backspace", false, false, false));
        assert!(!should_passthrough_terminal_key("backspace", false, true, false));
    }

    #[test]
    fn keys_map_to_pty_bytes() {
        let ctrl_c = KeyEvent {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::default()
            },
            ..KeyEvent::new("c")
        };
        assert_eq!(key_to_pty_bytes(&ctrl_c).as_deref(), Some("\u{3}"));
        assert_eq!(
            key_to_pty_bytes(&KeyEvent::new("up")).as_deref(),
            Some("\u{1b}[A")
        );
        assert_eq!(
            key_to_pty_bytes(&KeyEvent::new("left")).as_deref(),
            Some("\u{1b}[D")
        );
        assert_eq!(
            key_to_pty_bytes(&KeyEvent::new("backspace")).as_deref(),
            Some("\u{7f}")
        );
        let letter = KeyEvent {
            key_char: Some("a".to_string()),
            ..KeyEvent::new("a")
        };
        assert_eq!(key_to_pty_bytes(&letter).as_deref(), Some("a"));
        let shift_a = KeyEvent {
            key_char: Some("A".to_string()),
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            ..KeyEvent::new("a")
        };
        assert_eq!(key_to_pty_bytes(&shift_a).as_deref(), Some("A"));
        let cmd_a = KeyEvent {
            modifiers: Modifiers {
                platform: true,
                ..Modifiers::default()
            },
            ..KeyEvent::new("a")
        };
        assert_eq!(key_to_pty_bytes(&cmd_a), None);
    }
}
