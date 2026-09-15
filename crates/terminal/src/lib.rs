//! 终端显示核心：行缓冲解析、SGR 属性、按键到 PTY 字节映射、面板像素到列行估算。
//!
//! 不是完整 VT emulator。原始字节仍由 Host 保存；本包只把可见文本整理成
//! 行覆盖语义（CR / 退格 / EL / SGR），并把控制键映射成 PTY 字节。
//! 支持 shell 编辑所需的光标移动、清屏与自动折行；不实现备用屏幕等完整 VT 功能。
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
    text: String,
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
        self.cells.iter().all(|cell| cell.text.trim().is_empty())
    }

    fn put(&mut self, col: usize, ch: char, attrs: Attrs) {
        self.put_width(col, ch, 1, attrs);
    }

    fn put_width(&mut self, col: usize, ch: char, width: usize, attrs: Attrs) {
        let blank = || Cell {
            text: " ".into(),
            attrs: Attrs::default(),
        };
        if width == 0 {
            let end = col.min(self.cells.len());
            if let Some(cell) = self.cells[..end]
                .iter_mut()
                .rev()
                .find(|cell| !cell.text.is_empty())
            {
                cell.text.push(ch);
            }
            return;
        }
        if col + width > self.cells.len() {
            self.cells.resize_with(col + width, blank);
        }
        // 写到宽字符后半格或覆盖其首格时，清理旧宽字符留下的半格。
        if self.cells[col].text.is_empty() && col > 0 {
            self.cells[col - 1] = blank();
        }
        if self
            .cells
            .get(col + 1)
            .is_some_and(|cell| cell.text.is_empty())
        {
            self.cells[col + 1] = blank();
        }
        self.cells[col] = Cell {
            text: ch.to_string(),
            attrs,
        };
        if width == 2 {
            self.cells[col + 1] = Cell {
                text: String::new(),
                attrs,
            };
        }
    }

    fn clear_from(&mut self, col: usize) {
        if col < self.cells.len() {
            self.cells.truncate(col);
        }
    }

    fn text(&self) -> String {
        self.cells
            .iter()
            .map(|cell| cell.text.as_str())
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
    columns: usize,
    rows: usize,
    top: usize,
    pub cursor_visible: bool,
    pub bracketed_paste: bool,
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
            columns: usize::MAX,
            rows: 24,
            top: 0,
            cursor_visible: true,
            bracketed_paste: false,
        }
    }

    /// 使用 PTY 当前尺寸重建可见屏幕及滚动历史。
    pub fn with_size(columns: u16, rows: u16) -> Self {
        Self {
            columns: usize::from(columns.max(1)),
            rows: usize::from(rows.max(1)),
            ..Self::new()
        }
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col.min(self.columns.saturating_sub(1)))
    }

    /// 光标前的 UTF-8 字节数，供显示层在宽字符之后插入光标/IME。
    pub fn cursor_byte_offset(&self) -> usize {
        let (row, col) = self.cursor();
        self.lines.get(row).map_or(0, |line| {
            line.cells
                .iter()
                .take(col)
                .map(|cell| cell.text.len())
                .sum::<usize>()
                + col.saturating_sub(line.cells.len())
        })
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
        self.top = self.top.max(self.row.saturating_sub(self.rows - 1));
        if self.row >= self.lines.len() {
            self.lines.push(Line::new());
        }
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        }
    }

    fn put_char(&mut self, ch: char, width: usize) {
        match ch {
            '\r' => self.carriage_return(),
            '\n' => self.line_feed(),
            '\u{8}' => {
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
                if width > 0 && self.col.saturating_add(width) > self.columns {
                    self.line_feed();
                }
                let col = self.col;
                let attrs = self.attrs;
                self.current_line().put_width(col, ch, width, attrs);
                self.col += width;
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
        self.feed_with_width(raw, |_| 1);
    }

    /// 字符 cell 宽由渲染方提供（0/1/2），本库不依赖字体或 OS。
    pub fn feed_with_width(&mut self, raw: &str, mut width: impl FnMut(char) -> usize) {
        let mut chars = raw.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\u{1b}' {
                self.put_char(ch, width(ch).min(2));
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
                    if let Some(mode) = body.strip_prefix('?').and_then(|s| s.parse::<u16>().ok()) {
                        if matches!(final_byte, Some('h' | 'l')) {
                            let enabled = final_byte == Some('h');
                            match mode {
                                25 => self.cursor_visible = enabled,
                                2004 => self.bracketed_paste = enabled,
                                _ => {}
                            }
                        }
                        continue;
                    }
                    let Some(params) = parse_params(&body) else {
                        continue;
                    };
                    match final_byte {
                        Some('m') => self.apply_sgr(&params),
                        Some('K') => self.erase_in_line(params.first().copied().unwrap_or(0)),
                        Some('H' | 'f') => {
                            self.row = self.top
                                + usize::from(params.first().copied().unwrap_or(1).max(1) - 1)
                                    .min(self.rows - 1);
                            self.col = usize::from(params.get(1).copied().unwrap_or(1).max(1) - 1)
                                .min(self.columns.saturating_sub(1).min(499));
                            self.current_line();
                        }
                        Some('A') => {
                            self.row = self
                                .row
                                .saturating_sub(usize::from(
                                    params.first().copied().unwrap_or(1).max(1),
                                ))
                                .max(self.top)
                        }
                        Some('B') => {
                            self.row = (self.row
                                + usize::from(params.first().copied().unwrap_or(1).max(1)))
                            .min(self.top + self.rows - 1);
                            self.current_line();
                        }
                        Some('J') => match params.first().copied().unwrap_or(0) {
                            2 | 3 => {
                                for line in &mut self.lines[self.top..] {
                                    *line = Line::new();
                                }
                            }
                            0 => {
                                self.erase_in_line(0);
                                self.lines.truncate(self.row + 1);
                            }
                            _ => {}
                        },
                        Some('G') => {
                            let col = params.first().copied().unwrap_or(1).max(1) as usize;
                            self.col = col
                                .saturating_sub(1)
                                .min(self.current_line().cells.len().max(499))
                                .min(self.columns.saturating_sub(1));
                        }
                        Some('C') => {
                            // 稀疏寻址最多填到面板支持的 500 列；已有长行可正常回移。
                            self.col = (self.col
                                + params.first().copied().unwrap_or(1).max(1) as usize)
                                .min(self.current_line().cells.len().max(499))
                                .min(self.columns.saturating_sub(1));
                        }
                        Some('D') => {
                            self.col = self.col.saturating_sub(
                                params.first().copied().unwrap_or(1).max(1) as usize,
                            );
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
                Some(intermediate) if (' '..='/').contains(&intermediate) => {
                    // ESC ( B 等字符集指定：中间字节与终止字节一起跳过。
                    for sequence_char in chars.by_ref() {
                        if !(' '..='/').contains(&sequence_char) {
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            }
        }
    }

    /// 可见行：裁掉尾部空行，每行给出裁过尾随空格的文本与按字节长度的属性分段。
    /// 空行以单空格占位，保持行高与纯文本行数。
    pub fn display_lines(&self) -> Vec<StyledLine> {
        self.styled_lines_with_cursor(true)
    }

    fn styled_lines(&self) -> Vec<StyledLine> {
        self.styled_lines_with_cursor(false)
    }

    fn styled_lines_with_cursor(&self, keep_cursor: bool) -> Vec<StyledLine> {
        let mut lines = self.lines.clone();
        if keep_cursor {
            let (row, col) = self.cursor();
            if lines.len() <= row {
                lines.resize(row + 1, Line::new());
            }
            if lines[row].cells.len() <= col {
                lines[row].put(col, ' ', Attrs::default());
            }
        }
        while lines.last().is_some_and(Line::is_empty)
            && lines.len() > if keep_cursor { self.row + 1 } else { 1 }
        {
            lines.pop();
        }
        lines
            .into_iter()
            .enumerate()
            .map(|(row, line)| {
                let text = if keep_cursor && row == self.row {
                    line.cells.iter().map(|c| c.text.as_str()).collect()
                } else {
                    line.text()
                };
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
                for cell in &line.cells {
                    if byte_len >= text.len() {
                        break;
                    }
                    let ch_len = cell.text.len();
                    if ch_len == 0 {
                        continue;
                    }
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

fn parse_params(body: &str) -> Option<Vec<u16>> {
    if body.is_empty() {
        return Some(Vec::new());
    }
    body.split(';')
        .map(|part| {
            if part.is_empty() {
                Some(0)
            } else {
                part.parse().ok()
            }
        })
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

/// 可见行缓冲：CR 覆盖同一行，退格回移光标，SGR 进入属性分段。
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

/// 按面板像素估算列 × 行，并钳制在 PTY 边界内。
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

/// 把终端按键映射为 PTY 字节；可打印字符与 IME 提交由调用方统一路由。
pub fn key_to_pty_bytes(key: &KeyEvent) -> Option<String> {
    let key_name = key.key.as_str();
    let modifiers = &key.modifiers;
    if modifiers.platform || modifiers.function {
        return None;
    }
    if modifiers.control && !modifiers.alt {
        let ch = key_name.as_bytes();
        return (ch.len() == 1 && (b'@'..=b'_').contains(&ch[0].to_ascii_uppercase()))
            .then(|| ((ch[0].to_ascii_uppercase() & 0x1f) as char).to_string());
    }
    if modifiers.alt {
        return None;
    }
    match key_name {
        "enter" => Some("\r".to_string()),
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
        assert_eq!(plain_output("abc\u{8}"), "abc");
        assert_eq!(plain_output("abc\u{8}\u{8}X"), "aXc");
        assert_eq!(plain_output("abc\u{8} \u{8}"), "ab");
        assert_eq!(plain_output("abc\u{7f}"), "abc");
    }

    #[test]
    fn vt_control_sequences_are_stripped() {
        assert_eq!(plain_output("\u{1b}(Bhello\r\u{1b}[?1K"), "hello");
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
    fn cursor_tracks_shell_redraw_and_wrap() {
        let mut screen = Screen::with_size(20, 6);
        screen.feed("$ echo hi\u{1b}[2D");
        assert_eq!(screen.cursor(), (0, 7));
        assert_eq!(screen.display_lines()[0].text, "$ echo hi");
        screen.feed("\r\n$ ");
        assert_eq!(screen.cursor(), (1, 2));
        assert_eq!(screen.display_lines()[1].text, "$  ");
        screen.feed("12345678901234567890");
        assert_eq!(screen.cursor(), (2, 2));
        screen.feed("\u{1b}[H\u{1b}[2J$ ");
        assert_eq!(screen.cursor(), (0, 2));
        assert_eq!(screen.display_lines().len(), 1);
        let mut sparse_cursor = Screen::with_size(20, 6);
        sparse_cursor.feed("\x1b[10G");
        assert_eq!(sparse_cursor.cursor_byte_offset(), 9);
        assert_eq!(sparse_cursor.display_lines()[0].text.len(), 10);
        let mut wide = Screen::with_size(20, 6);
        wide.feed_with_width("$ 中文\x1b[2D新", |ch| if ch.is_ascii() { 1 } else { 2 });
        assert_eq!(wide.cursor(), (0, 6));
        assert_eq!(wide.cursor_byte_offset(), "$ 中新".len());
        assert_eq!(wide.display_lines()[0].text, "$ 中新 ");
        screen.feed("\u{1b}[?2004h");
        assert!(screen.bracketed_paste);
        let sparse = plain_output(&format!("{}X", "\u{1b}[65535C".repeat(100)));
        assert_eq!(sparse.len(), 500);
        assert!(sparse.ends_with('X'));
        assert_eq!(
            plain_output("first\nsecond\u{1b}[Hrewritten"),
            "rewritten\nsecond"
        );
        assert_eq!(plain_output("alpha\nbeta\u{1b}[1Azzz"), "alphzzz\nbeta");
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
