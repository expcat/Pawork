//! Timeline 的小型 Markdown 子集；渲染与 AX 高度估算共用解析后的可见文本。

use std::ops::Range;

use gpui::{
    div, prelude::*, px, FontStyle, FontWeight, InteractiveText, Rgba, SharedString, StyledText,
    TextRun,
};

use super::components::button::{Button, ButtonPadding, ButtonVariant};
use super::components::icon::{icon, Icon};
use super::i18n::t;
use super::theme::{dark, font, metrics};
use super::AppView;

/// 代码复制走 SVG；命中区与 Header 图标按钮同为 36×36。
const TABLE_CELL_PAD_X: f32 = 6.0;
const LINK_LABEL_URL_CHARS: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum InlineStyle {
    #[default]
    Plain,
    Bold,
    Emphasis,
    Code,
    Link,
}

#[derive(Debug)]
struct Span {
    text: String,
    style: InlineStyle,
    target: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockKind {
    Paragraph,
    Heading,
    List,
    Quote,
    Code,
    Table,
}

#[derive(Debug)]
struct Block {
    kind: BlockKind,
    lines: Vec<Vec<Span>>,
    code: String,
    table: Vec<Vec<Vec<Span>>>,
    alignments: Vec<gpui::TextAlign>,
    /// Fence info string 的首个空白分隔词；空或缺失为 None。
    language: Option<String>,
}

impl Block {
    fn inset(&self) -> f32 {
        match self.kind {
            BlockKind::Code => 24.0,
            BlockKind::Quote => 14.0,
            _ => 0.0,
        }
    }
}

fn literal(text: &str, style: InlineStyle) -> Span {
    Span {
        text: text.to_owned(),
        style,
        target: None,
    }
}

/// URL 内成对括号属于地址；深度为零的右括号才结束链接。
fn link_end(text: &str, markdown: bool) -> Option<usize> {
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(index),
            ')' => depth -= 1,
            _ if !markdown && (ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | ']')) => {
                return Some(index);
            }
            _ => {}
        }
    }
    (!markdown).then_some(text.len())
}

/// 只剥离已闭合的标记；扫描按 Unicode 字符前进，代码内容不再解析。
fn inline(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut rest = text;
    let mut plain = String::new();
    while !rest.is_empty() {
        let mut matched = None;
        let mut target_url = None;
        for (marker, style) in [
            ("`", InlineStyle::Code),
            ("**", InlineStyle::Bold),
            ("*", InlineStyle::Emphasis),
        ] {
            if let Some(inner) = rest.strip_prefix(marker) {
                if let Some(end) = inner.find(marker).filter(|end| *end > 0) {
                    matched = Some((end + marker.len() * 2, inner[..end].to_owned(), style));
                    break;
                }
            }
        }
        if let Some(label) = rest.strip_prefix('[') {
            if let Some(label_end) = label.find("](") {
                let target = &label[label_end + 2..];
                if let Some(target_end) = link_end(target, true).filter(|end| *end > 0) {
                    target_url =
                        http_url(&target[..target_end]).then(|| target[..target_end].to_owned());
                    matched = Some((
                        1 + label_end + 2 + target_end + 1,
                        format!("{} ({})", &label[..label_end], &target[..target_end]),
                        InlineStyle::Link,
                    ));
                }
            }
        }
        if matched.is_none() && (rest.starts_with("https://") || rest.starts_with("http://")) {
            let end = link_end(rest, false).unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', '!', '?', '。', '，']);
            if http_url(url) {
                target_url = Some(url.to_owned());
                matched = Some((url.len(), url.to_owned(), InlineStyle::Link));
            }
        }
        if let Some((consumed, content, style)) = matched {
            if !plain.is_empty() {
                spans.push(Span {
                    text: std::mem::take(&mut plain),
                    style: InlineStyle::Plain,
                    target: None,
                });
            }
            spans.push(Span {
                text: content,
                style,
                target: target_url,
            });
            rest = &rest[consumed..];
        } else {
            let ch = rest.chars().next().expect("nonempty remainder");
            plain.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    if !plain.is_empty() || spans.is_empty() {
        spans.push(Span {
            text: plain,
            style: InlineStyle::Plain,
            target: None,
        });
    }
    spans
}

fn fence(line: &str) -> Option<(char, usize, &str)> {
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let count = line.chars().take_while(|ch| *ch == marker).count();
    (count >= 3).then(|| (marker, count, &line[count..]))
}

fn fence_language(info: &str) -> Option<String> {
    let token = info.trim().split_whitespace().next()?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}

// 分隔行完整到达前保留原文，避免流式输入丢失内容。
fn table_cells(line: &str) -> Option<Vec<String>> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = line.trim().chars().peekable();
    let mut ticks = 0;
    let mut pipe = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'|') {
            chars.next();
            cell.push('|');
        } else if ch == '`' {
            let mut count = 1;
            while chars.peek() == Some(&'`') {
                chars.next();
                count += 1;
            }
            if ticks == 0 {
                ticks = count;
            } else if ticks == count {
                ticks = 0;
            }
            cell.extend(std::iter::repeat_n('`', count));
        } else if ch == '|' && ticks == 0 {
            pipe = true;
            cells.push(cell.trim().to_owned());
            cell.clear();
        } else {
            cell.push(ch);
        }
    }
    cells.push(cell.trim().to_owned());
    if line.trim().starts_with('|') {
        cells.remove(0);
    }
    if line.trim().ends_with('|') && cells.last().is_some_and(String::is_empty) {
        cells.pop();
    }
    (pipe && !cells.is_empty()).then_some(cells)
}

fn table_alignments(line: &str, columns: usize) -> Option<Vec<gpui::TextAlign>> {
    let cells = table_cells(line)?;
    if cells.len() != columns {
        return None;
    }
    cells
        .iter()
        .map(|cell| {
            let dashes = cell.trim_matches(':');
            if dashes.len() < 3 || !dashes.bytes().all(|b| b == b'-') {
                return None;
            }
            Some(if cell.starts_with(':') && cell.ends_with(':') {
                gpui::TextAlign::Center
            } else if cell.ends_with(':') {
                gpui::TextAlign::Right
            } else {
                gpui::TextAlign::Left
            })
        })
        .collect()
}

fn http_url(url: &str) -> bool {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .is_some_and(|rest| {
            !rest.is_empty()
                && !rest.starts_with('/')
                && !rest.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        })
}

#[derive(Clone, Debug)]
pub(super) struct MessageAction {
    pub label: String,
    pub content: String,
    pub open: bool,
}

fn block_links(block: &Block) -> Vec<String> {
    let mut links = Vec::new();
    for span in block
        .lines
        .iter()
        .flatten()
        .chain(block.table.iter().flatten().flatten())
    {
        if let Some(target) = &span.target {
            if !links.contains(target) {
                links.push(target.clone());
            }
        }
    }
    links
}

pub(super) fn message_actions(text: &str) -> Vec<MessageAction> {
    let mut actions = Vec::new();
    let mut codes = 0;
    let mut links = Vec::new();
    for block in parse(text) {
        if block.kind == BlockKind::Code {
            codes += 1;
            actions.push(MessageAction {
                label: format!("{} {codes}", t("timeline.copy_code")),
                content: block.code.clone(),
                open: false,
            });
        }
        for link in block_links(&block) {
            if links.contains(&link) {
                continue;
            }
            links.push(link.clone());
            let number = links.len();
            let suffix = truncated_url(&link);
            actions.push(MessageAction {
                label: format!("{} {number} · {suffix}", t("timeline.open_link")),
                content: link.clone(),
                open: true,
            });
            actions.push(MessageAction {
                label: format!("{} {number} · {suffix}", t("timeline.copy_link")),
                content: link,
                open: false,
            });
        }
    }
    actions
}

fn truncated_url(url: &str) -> String {
    let display = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let count = display.chars().count();
    if count <= LINK_LABEL_URL_CHARS {
        display.to_owned()
    } else {
        let keep = LINK_LABEL_URL_CHARS.saturating_sub(1);
        format!("{}…", display.chars().take(keep).collect::<String>())
    }
}

pub(super) struct MessageCodeCopy {
    pub identifier: String,
    pub content: String,
    pub focus_key: String,
}

pub(super) fn message_code_copy_id(entry_id: &str, block_index: usize) -> String {
    format!("{entry_id}-code-{block_index}")
}

pub(super) fn message_code_copies(entry_id: &str, text: &str) -> Vec<MessageCodeCopy> {
    parse(text)
        .into_iter()
        .enumerate()
        .filter(|(_, block)| block.kind == BlockKind::Code)
        .map(|(index, block)| MessageCodeCopy {
            identifier: message_code_copy_id(entry_id, index),
            content: block.code,
            focus_key: format!("{entry_id}:code:{index}"),
        })
        .collect()
}

pub(super) fn message_code_block_count(text: &str) -> usize {
    parse(text)
        .iter()
        .filter(|block| block.kind == BlockKind::Code)
        .count()
}

/// 列宽 = 该列最长单元格字符宽度 × 0.6 字号 + 左右 padding；行数估算同源（按内容、不按 160）。
fn table_column_widths(table: &[Vec<Vec<Span>>], font_px: f32) -> Vec<f32> {
    let columns = table.iter().map(|row| row.len()).max().unwrap_or(0);
    (0..columns)
        .map(|col| {
            let max_chars = table
                .iter()
                .map(|row| {
                    row.get(col)
                        .map(|cell| {
                            cell.iter()
                                .map(|span| span.text.chars().count())
                                .sum::<usize>()
                        })
                        .unwrap_or(0)
                })
                .max()
                .unwrap_or(0);
            max_chars as f32 * font_px * 0.6 + TABLE_CELL_PAD_X * 2.0
        })
        .collect()
}

fn table_column_widths_shaped(
    table: &[Vec<Vec<Span>>],
    window: &gpui::Window,
    color: Rgba,
) -> Vec<f32> {
    let font_size = font::BODY.to_pixels(window.rem_size());
    let columns = table.iter().map(|row| row.len()).max().unwrap_or(0);
    (0..columns)
        .map(|col| {
            let max_width = table
                .iter()
                .enumerate()
                .map(|(row_index, row)| {
                    let Some(cell) = row.get(col) else {
                        return 0.0;
                    };
                    let kind = if row_index == 0 {
                        BlockKind::Heading
                    } else {
                        BlockKind::Paragraph
                    };
                    cell.iter()
                        .map(|span| {
                            let mut face = gpui::font(if span.style == InlineStyle::Code {
                                font::MONO
                            } else {
                                ".SystemUIFont"
                            });
                            if kind == BlockKind::Heading || span.style == InlineStyle::Bold {
                                face.weight = FontWeight::SEMIBOLD;
                            }
                            if span.style == InlineStyle::Emphasis {
                                face.style = FontStyle::Italic;
                            }
                            let run = TextRun {
                                len: span.text.len(),
                                font: face,
                                color: color.into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            f32::from(
                                window
                                    .text_system()
                                    .shape_line(span.text.clone().into(), font_size, &[run], None)
                                    .width,
                            )
                        })
                        .sum::<f32>()
                })
                .fold(0.0_f32, f32::max);
            max_width + TABLE_CELL_PAD_X * 2.0
        })
        .collect()
}

fn copy_code_button(
    view: &mut AppView,
    cx: &mut gpui::Context<AppView>,
    window: &gpui::Window,
    identifier: String,
    focus_key: String,
    content: String,
) -> gpui::Stateful<gpui::Div> {
    let focus = view.timeline_detail_focus(&focus_key, cx);
    let focused = focus.is_focused(window);
    let click_content = content.clone();
    let activate_content = content;
    view.settings_element(identifier.clone())
        .flex_none()
        .opacity(if focused { 1.0 } else { 0.0 })
        .group_hover("markdown-code", |style| style.opacity(1.0))
        .child(
            Button::new(SharedString::from(identifier.clone()))
                .variant(ButtonVariant::Ghost)
                .padding(ButtonPadding::None)
                .width(px(metrics::ICON_BUTTON_SIZE))
                .height(px(metrics::ICON_BUTTON_SIZE))
                .center()
                .vcenter()
                .text_color(dark().text.secondary)
                .child(icon(Icon::Copy))
                .tooltip(t("timeline.copy_code"))
                .track_focus(&focus)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(click_content.clone()));
                })
                .on_activate(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                        activate_content.clone(),
                    ));
                }),
        )
}

fn parse(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut fenced = None;
    let mut separated = true;
    let mut source = text.split_inclusive('\n').peekable();
    while let Some(original) = source.next() {
        let raw = original.trim_end_matches('\n').trim_end_matches('\r');
        let line = raw.trim_start();
        if let Some((marker, count)) = fenced {
            if fence(line).is_some_and(|(end_marker, end_count, tail)| {
                end_marker == marker && end_count >= count && tail.trim().is_empty()
            }) {
                fenced = None;
                separated = true;
            } else {
                blocks
                    .last_mut()
                    .expect("open code block")
                    .code
                    .push_str(original);
                blocks
                    .last_mut()
                    .expect("open code block")
                    .lines
                    .push(vec![literal(raw, InlineStyle::Code)]);
            }
            continue;
        }
        if let Some((marker, count, info)) =
            fence(line).filter(|(marker, _, tail)| *marker != '`' || !tail.contains('`'))
        {
            blocks.push(Block {
                kind: BlockKind::Code,
                lines: Vec::new(),
                code: String::new(),
                table: Vec::new(),
                alignments: Vec::new(),
                language: fence_language(info),
            });
            fenced = Some((marker, count));
            continue;
        }
        if line.is_empty() {
            separated = true;
            continue;
        }
        if let Some(header) = table_cells(raw) {
            if let Some(alignments) = source
                .peek()
                .and_then(|next| table_alignments(next, header.len()))
            {
                source.next();
                let mut rows = vec![header.into_iter().map(|cell| inline(&cell)).collect()];
                while let Some(cells) = source
                    .peek()
                    .and_then(|next| table_cells(next))
                    .filter(|cells| cells.len() == alignments.len())
                {
                    source.next();
                    rows.push(cells.into_iter().map(|cell| inline(&cell)).collect());
                }
                blocks.push(Block {
                    kind: BlockKind::Table,
                    lines: Vec::new(),
                    code: String::new(),
                    table: rows,
                    alignments,
                    language: None,
                });
                separated = true;
                continue;
            }
        }
        let hashes = line.bytes().take_while(|ch| *ch == b'#').count();
        let (kind, content) = if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            (BlockKind::Heading, line[hashes..].trim_start().to_owned())
        } else if let Some(quote) = line.strip_prefix('>') {
            (
                BlockKind::Quote,
                quote.strip_prefix(' ').unwrap_or(quote).to_owned(),
            )
        } else if let Some(item) = ["- ", "* ", "+ "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        {
            (BlockKind::List, format!("• {item}"))
        } else {
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && (line[digits..].starts_with(". ") || line[digits..].starts_with(") "))
            {
                (BlockKind::List, line.to_owned())
            } else {
                (BlockKind::Paragraph, raw.to_owned())
            }
        };
        let spans = inline(&content);
        if !separated
            && kind != BlockKind::Heading
            && blocks.last().is_some_and(|block| block.kind == kind)
        {
            blocks.last_mut().expect("matching block").lines.push(spans);
        } else {
            blocks.push(Block {
                kind,
                lines: vec![spans],
                code: String::new(),
                table: Vec::new(),
                alignments: Vec::new(),
                language: None,
            });
        }
        separated = false;
    }
    for block in &mut blocks {
        if block.lines.is_empty() && block.kind != BlockKind::Table {
            block.lines.push(vec![literal("", InlineStyle::Code)]);
        }
    }
    blocks
}

fn styled_line(
    spans: Vec<Span>,
    kind: BlockKind,
    color: Rgba,
) -> (StyledText, Vec<(Range<usize>, String)>) {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut links = Vec::new();
    let mut start = 0usize;
    for span in spans {
        let mut face = gpui::font(if span.style == InlineStyle::Code {
            font::MONO
        } else {
            ".SystemUIFont"
        });
        if kind == BlockKind::Heading || span.style == InlineStyle::Bold {
            face.weight = FontWeight::SEMIBOLD;
        }
        if span.style == InlineStyle::Emphasis {
            face.style = FontStyle::Italic;
        }
        runs.push(TextRun {
            len: span.text.len(),
            font: face,
            color: color.into(),
            background_color: (span.style == InlineStyle::Code)
                .then(|| dark().surface.raised.into()),
            underline: (span.style == InlineStyle::Link).then(|| gpui::UnderlineStyle {
                thickness: px(1.0),
                color: Some(color.into()),
                wavy: false,
            }),
            strikethrough: None,
        });
        let end = start + span.text.len();
        if let Some(url) = span.target.filter(|url| http_url(url)) {
            links.push((start..end, url));
        }
        text.push_str(&span.text);
        start = end;
    }
    (StyledText::new(text).with_runs(runs), links)
}

fn line_element(
    element_id: String,
    spans: Vec<Span>,
    kind: BlockKind,
    color: Rgba,
) -> gpui::AnyElement {
    let (styled, links) = styled_line(spans, kind, color);
    if links.is_empty() {
        return styled.into_any_element();
    }
    let ranges: Vec<Range<usize>> = links.iter().map(|(range, _)| range.clone()).collect();
    let urls: Vec<String> = links.into_iter().map(|(_, url)| url).collect();
    InteractiveText::new(SharedString::from(element_id), styled)
        .on_click(ranges, move |index, _, cx| {
            if let Some(url) = urls.get(index) {
                if http_url(url) {
                    cx.open_url(url);
                }
            }
        })
        .into_any_element()
}

/// Code and tables use the full reading column; prose keeps the user bubble cap.
pub(super) fn message_needs_full_width(text: &str) -> bool {
    parse(text)
        .iter()
        .any(|block| matches!(block.kind, BlockKind::Code | BlockKind::Table))
}

fn streaming_caret(color: Rgba) -> impl IntoElement {
    div()
        .w(px(metrics::STREAM_CARET_WIDTH))
        .h(px(metrics::STREAM_CARET_HEIGHT))
        .rounded(px(1.0))
        .bg(color)
        .flex_none()
}

pub(super) fn message_body_element(
    view: &mut AppView,
    cx: &mut gpui::Context<AppView>,
    window: &gpui::Window,
    entry_id: &str,
    text: &str,
    color: Rgba,
    streaming: bool,
) -> gpui::Div {
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(metrics::MSG_PARAGRAPH_GAP))
        .text_size(font::BODY)
        .line_height(font::from_pixels(metrics::MSG_LINE_HEIGHT))
        .text_color(color);
    let blocks = parse(text);
    let last_block = blocks.len().saturating_sub(1);
    for (index, block) in blocks.into_iter().enumerate() {
        let mut element = div().flex().flex_col();
        if block.kind == BlockKind::Code {
            let language_label = block
                .language
                .as_deref()
                .filter(|name| !name.is_empty())
                .map(|name| name.to_owned())
                .unwrap_or_else(|| t("timeline.code_block").to_string());
            element = element
                .group("markdown-code")
                .px(px(12.0))
                .bg(dark().surface.raised)
                .rounded(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(metrics::ICON_BUTTON_SIZE))
                        .child(
                            div().flex().flex_row().flex_1().min_w_0().child(
                                div()
                                    .text_size(font::BODY_SM)
                                    .text_color(dark().text.secondary)
                                    .truncate()
                                    .child(language_label),
                            ),
                        )
                        .child(copy_code_button(
                            view,
                            cx,
                            window,
                            message_code_copy_id(entry_id, index),
                            format!("{entry_id}:code:{index}"),
                            block.code.clone(),
                        )),
                );
        } else if block.kind == BlockKind::Quote {
            element = element
                .pl(px(12.0))
                .border_l_2()
                .border_color(dark().border.subtle);
        }
        if block.kind == BlockKind::Table {
            let widths = table_column_widths_shaped(&block.table, window, color);
            let table_width: f32 = widths.iter().sum();
            let mut table = div().flex().flex_col().w(px(table_width));
            for (row_index, row) in block.table.into_iter().enumerate() {
                let mut row_element = div()
                    .flex()
                    .flex_row()
                    .w(px(table_width))
                    .border_b_1()
                    .border_color(dark().border.subtle);
                if row_index == 0 {
                    row_element = row_element.bg(dark().surface.hover);
                }
                for (cell_index, cell) in row.into_iter().enumerate() {
                    let width = widths
                        .get(cell_index)
                        .copied()
                        .unwrap_or(TABLE_CELL_PAD_X * 2.0);
                    row_element = row_element.child(
                        div()
                            .w(px(width))
                            .flex_none()
                            .px(px(TABLE_CELL_PAD_X))
                            .text_align(block.alignments[cell_index])
                            .min_h(font::from_pixels(metrics::MSG_LINE_HEIGHT))
                            .whitespace_nowrap()
                            .child(line_element(
                                format!("{entry_id}-cell-{index}-{row_index}-{cell_index}"),
                                cell,
                                if row_index == 0 {
                                    BlockKind::Heading
                                } else {
                                    BlockKind::Paragraph
                                },
                                color,
                            )),
                    );
                }
                table = table.child(row_element);
            }
            element = element.child(
                div()
                    .id(SharedString::from(format!(
                        "{entry_id}-table-scroll-{index}"
                    )))
                    .overflow_x_scroll()
                    .child(table),
            );
        }
        let mut lines = div().flex().flex_col();
        let last_line = block.lines.len().saturating_sub(1);
        let attach_caret = streaming
            && index == last_block
            && block.kind != BlockKind::Code
            && block.kind != BlockKind::Table;
        for (line_index, line) in block.lines.into_iter().enumerate() {
            // Taffy can clamp an auto-width row to the viewport even when its
            // nowrap text overflows. Give the scroll child its shaped width.
            let code_width = if block.kind == BlockKind::Code {
                let text: String = line.iter().map(|span| span.text.as_str()).collect();
                let run = TextRun {
                    len: text.len(),
                    font: gpui::font(font::MONO),
                    color: color.into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                window
                    .text_system()
                    .shape_line(
                        text.into(),
                        font::BODY.to_pixels(window.rem_size()),
                        &[run],
                        None,
                    )
                    .width
            } else {
                px(0.)
            };
            let mut line_row = div()
                .min_h(font::from_pixels(metrics::MSG_LINE_HEIGHT))
                .when(block.kind == BlockKind::Code, |line| {
                    line.whitespace_nowrap().min_w(code_width)
                })
                .when(attach_caret && line_index == last_line, |row| {
                    row.flex().flex_row().items_center().gap(px(metrics::SPACE_1))
                })
                .child(line_element(
                    format!("{entry_id}-md-{index}-{line_index}"),
                    line,
                    block.kind,
                    color,
                ));
            if attach_caret && line_index == last_line {
                line_row = line_row.child(streaming_caret(color));
            }
            lines = lines.child(line_row);
        }
        element = if block.kind == BlockKind::Code {
            element.child(
                lines
                    .id(SharedString::from(format!(
                        "{entry_id}-code-scroll-{index}"
                    )))
                    .items_start()
                    .overflow_x_scroll(),
            )
        } else if block.kind == BlockKind::Table {
            element
        } else {
            element.child(lines)
        };
        body = body.child(element);
    }
    body
}

/// 仍是平均字宽 0.6 × 字号的近似；按渲染可见文本与引用/代码缩进估算。
/// 代码头按 ICON_BUTTON_SIZE 计入一行（timeline 测高再补像素差）；表格按内容列宽不折行。
pub(super) fn message_block_line_counts(text: &str, width_px: f32, font_px: f32) -> Vec<usize> {
    parse(text)
        .into_iter()
        .map(|block| {
            let chars_per_line =
                (((width_px - block.inset()).max(1.0) / (font_px * 0.6)).floor() as usize).max(1);
            let header = usize::from(block.kind == BlockKind::Code);
            let table_lines = block.table.len();
            header
                + table_lines
                + block
                    .lines
                    .iter()
                    .map(|line| {
                        if block.kind == BlockKind::Code {
                            return 1;
                        }
                        line.iter()
                            .map(|span| span.text.chars().count())
                            .sum::<usize>()
                            .div_ceil(chars_per_line)
                            .max(1)
                    })
                    .sum::<usize>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_blocks_and_visible_inline_text_share_line_counts() {
        let text = "# Title\nParagraph **bold** *em* `code` [docs](https://example.test)\n\n- item\n2. second\n> quote\n```rust\nlet x = **raw**;\n\n```";
        let blocks = parse(text);
        assert_eq!(
            blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
            [
                BlockKind::Heading,
                BlockKind::Paragraph,
                BlockKind::List,
                BlockKind::Quote,
                BlockKind::Code
            ]
        );
        let paragraph = &blocks[1].lines[0];
        assert_eq!(
            paragraph
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            "Paragraph bold em code docs (https://example.test)"
        );
        assert_eq!(
            paragraph
                .iter()
                .filter(|span| span.style != InlineStyle::Plain)
                .map(|span| span.style)
                .collect::<Vec<_>>(),
            [
                InlineStyle::Bold,
                InlineStyle::Emphasis,
                InlineStyle::Code,
                InlineStyle::Link
            ]
        );
        assert_eq!(blocks[4].lines[0][0].text, "let x = **raw**;");
        assert_eq!(blocks[4].language.as_deref(), Some("rust"));
        assert_eq!(
            message_block_line_counts(text, 600.0, 14.0),
            [1, 1, 2, 1, 3]
        );
        assert_eq!(parse("```\nlet x = 1;\n```")[0].language, None);
        assert_eq!(
            parse("``` rust extra\nlet x = 1;\n```")[0]
                .language
                .as_deref(),
            Some("rust")
        );

        let text = "| 名称 | 值 |\n| :--- | ---: |\n| 中文\\|字 | `a|b` |\n\n```rust\r\n  let a = 1;\r\n\r\n```\n[文档](https://example.test) https://other.test。";
        let blocks = parse(text);
        assert_eq!(blocks[0].kind, BlockKind::Table);
        assert_eq!(
            blocks[0].alignments,
            [gpui::TextAlign::Left, gpui::TextAlign::Right]
        );
        assert_eq!(blocks[0].table[1][0][0].text, "中文|字");
        assert_eq!(blocks[0].table[1][1][0].text, "a|b");
        let widths = table_column_widths(&blocks[0].table, 14.0);
        assert_eq!(widths.len(), 2);
        assert!(
            widths.iter().all(|width| (*width - 160.0).abs() > 1.0),
            "content-sized columns, not 160px: {widths:?}"
        );
        assert!(widths.iter().all(|width| *width > TABLE_CELL_PAD_X * 2.0));
        let actions = message_actions(text);
        assert_eq!(actions.len(), 5);
        assert!(actions[0].label.ends_with(" 1"));
        assert_eq!(actions[0].content, "  let a = 1;\r\n\r\n");
        assert_eq!(actions[1].content, "https://example.test");
        assert!(actions[1].open);
        assert!(actions[1].label.contains("1"));
        assert!(actions[1].label.contains("example.test"));
        assert_eq!(actions[4].content, "https://other.test");
        assert!(!actions[4].open);
        assert!(actions[4].label.contains("other.test"));
        assert_eq!(message_block_line_counts(text, 900.0, 14.0), [2, 3, 1]);
        let url = "https://en.wikipedia.org/wiki/Function_(mathematics)";
        for source in [format!("[定义]({url})"), format!("({url}).")] {
            let actions = message_actions(&source);
            assert_eq!(actions.len(), 2);
            assert!(actions[0].open);
            assert!(!actions[1].open);
            assert!(actions.iter().all(|action| action.content == url));
        }
        let nested = "https://example.test/a_(b_(c))";
        let nested_actions = message_actions(&format!("[嵌套]({nested})"));
        assert_eq!(nested_actions.len(), 2);
        assert!(nested_actions.iter().all(|action| action.content == nested));
    }

    #[test]
    fn streaming_unicode_and_unclosed_markers_preserve_content() {
        for text in [
            "中文 **未完成",
            "🙂 `代码",
            "[链接](尚未闭合",
            "[定义](https://example.test/a_(b)",
        ] {
            assert_eq!(
                inline(text)
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>(),
                text
            );
        }
        assert!(message_actions(
            "[bad](javascript:alert) [file](file:///tmp/a) `https://code.test`"
        )
        .is_empty());
        assert_eq!(
            parse("| 中文 | 值 |\n| --- | --")[0].kind,
            BlockKind::Paragraph
        );
        assert_eq!(
            message_actions("```\n  unfinished")[0].content,
            "  unfinished"
        );
        let single_line = parse("```hello```\nnormal");
        assert_eq!(single_line[0].kind, BlockKind::Paragraph);
        assert!(single_line[0].lines[0]
            .iter()
            .any(|span| span.text.contains("hello")));
        let blocks = parse("```\n中文 **原样**\n🙂");
        assert_eq!(blocks[0].lines[0][0].text, "中文 **原样**");
        assert_eq!(blocks[0].lines[1][0].text, "🙂");
        assert_eq!(blocks[0].language, None);
        let spans = inline("**中文**🙂");
        assert_eq!(spans[0].text.len(), 6);
        assert_eq!(spans[1].text, "🙂");
    }
}
