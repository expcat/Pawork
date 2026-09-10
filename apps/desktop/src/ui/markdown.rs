//! Timeline 的小型 Markdown 子集；渲染与 AX 高度估算共用解析后的可见文本。

use gpui::{div, prelude::*, px, FontStyle, FontWeight, Rgba, StyledText, TextRun};

use super::components::button::{Button, ButtonPadding, ButtonVariant};
use super::i18n::t;
use super::theme::{dark, font, metrics};

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
            actions.push(MessageAction {
                label: format!("{} {}", t("timeline.open_link"), links.len()),
                content: link.clone(),
                open: true,
            });
            actions.push(MessageAction {
                label: format!("{} {}", t("timeline.copy_link"), links.len()),
                content: link,
                open: false,
            });
        }
    }
    actions
}

fn action_button(id: String, label: String, content: String, open: bool, is_link: bool) -> Button {
    let mut button = Button::new(id)
        .variant(ButtonVariant::Ghost)
        .label(label)
        .padding(ButtonPadding::Horizontal(6.0));
    // 继承正文以 rem 设置的行高；字号缩放时动作行与测高仍同为一行。
    if is_link {
        button = button.tooltip(content.clone());
    }
    button.on_click(move |_, _, cx| {
        cx.stop_propagation();
        if open {
            if http_url(&content) {
                cx.open_url(&content);
            }
        } else {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(content.clone()));
        }
    })
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
        if let Some((marker, count, _language)) =
            fence(line).filter(|(marker, _, tail)| *marker != '`' || !tail.contains('`'))
        {
            blocks.push(Block {
                kind: BlockKind::Code,
                lines: Vec::new(),
                code: String::new(),
                table: Vec::new(),
                alignments: Vec::new(),
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

fn styled_line(spans: Vec<Span>, kind: BlockKind, color: Rgba) -> StyledText {
    let mut text = String::new();
    let mut runs = Vec::new();
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
        text.push_str(&span.text);
    }
    StyledText::new(text).with_runs(runs)
}

/// Code and tables use the full reading column; prose keeps the user bubble cap.
pub(super) fn message_needs_full_width(text: &str) -> bool {
    parse(text)
        .iter()
        .any(|block| matches!(block.kind, BlockKind::Code | BlockKind::Table))
}

pub(super) fn message_body_element(
    entry_id: &str,
    text: &str,
    color: Rgba,
    window: &gpui::Window,
) -> gpui::Div {
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(metrics::MSG_PARAGRAPH_GAP))
        .text_size(font::BODY)
        .line_height(font::from_pixels(metrics::MSG_LINE_HEIGHT))
        .text_color(color);
    let mut message_links = Vec::new();
    for (index, block) in parse(text).into_iter().enumerate() {
        let links = block_links(&block);
        let mut element = div().flex().flex_col();
        if block.kind == BlockKind::Code {
            element = element
                .px(px(12.0))
                .bg(dark().surface.raised)
                .rounded(px(4.0))
                .child(div().flex().justify_end().child(action_button(
                    format!("{entry_id}-code-{index}"),
                    t("timeline.copy_code").into(),
                    block.code.clone(),
                    false,
                    false,
                )));
        } else if block.kind == BlockKind::Quote {
            element = element
                .pl(px(12.0))
                .border_l_2()
                .border_color(dark().border.subtle);
        }
        let mut table = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(block.alignments.len() as f32 * 160.0));
        for (row_index, row) in block.table.into_iter().enumerate() {
            let mut row_element = div()
                .flex()
                .w_full()
                .border_b_1()
                .border_color(dark().border.subtle);
            for (cell_index, cell) in row.into_iter().enumerate() {
                row_element = row_element.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(6.0))
                        .text_align(block.alignments[cell_index])
                        .min_h(font::from_pixels(metrics::MSG_LINE_HEIGHT))
                        .child(styled_line(
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
        if block.kind == BlockKind::Table {
            element = element.child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "{entry_id}-table-scroll-{index}"
                    )))
                    .overflow_x_scroll()
                    .child(table),
            );
        }
        let mut lines = div().flex().flex_col();
        for line in block.lines {
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
            lines = lines.child(
                div()
                    .min_h(font::from_pixels(metrics::MSG_LINE_HEIGHT))
                    .when(block.kind == BlockKind::Code, |line| {
                        line.whitespace_nowrap().min_w(code_width)
                    })
                    .child(styled_line(line, block.kind, color)),
            );
        }
        element = if block.kind == BlockKind::Code {
            element.child(
                lines
                    .id(gpui::SharedString::from(format!(
                        "{entry_id}-code-scroll-{index}"
                    )))
                    .items_start()
                    .overflow_x_scroll(),
            )
        } else {
            element.child(lines)
        };
        for (link_index, link) in links.into_iter().enumerate() {
            let number =
                if let Some(position) = message_links.iter().position(|target| target == &link) {
                    position + 1
                } else {
                    message_links.push(link.clone());
                    message_links.len()
                };
            element = element.child(
                div()
                    .flex()
                    .w_full()
                    .child(action_button(
                        format!("{entry_id}-link-{index}-{link_index}-open"),
                        format!("{} {number}", t("timeline.open_link")),
                        link.clone(),
                        true,
                        true,
                    ))
                    .child(action_button(
                        format!("{entry_id}-link-{index}-{link_index}-copy"),
                        format!("{} {number}", t("timeline.copy_link")),
                        link,
                        false,
                        true,
                    )),
            );
        }
        body = body.child(element);
    }
    body
}

/// 仍是平均字宽 0.6 × 字号的近似；按渲染可见文本与引用/代码缩进估算。
pub(super) fn message_block_line_counts(text: &str, width_px: f32, font_px: f32) -> Vec<usize> {
    parse(text)
        .into_iter()
        .map(|block| {
            let chars_per_line =
                (((width_px - block.inset()).max(1.0) / (font_px * 0.6)).floor() as usize).max(1);
            let actions = usize::from(block.kind == BlockKind::Code) + block_links(&block).len();
            let table_lines: usize = block
                .table
                .iter()
                .map(|row| {
                    let cell_chars = ((((width_px / row.len() as f32).max(160.0) - 12.0)
                        / (font_px * 0.6))
                        .floor() as usize)
                        .max(1);
                    row.iter()
                        .map(|cell| {
                            cell.iter()
                                .map(|span| span.text.chars().count())
                                .sum::<usize>()
                                .div_ceil(cell_chars)
                                .max(1)
                        })
                        .max()
                        .unwrap_or(1)
                })
                .sum();
            actions
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
        assert_eq!(
            message_block_line_counts(text, 600.0, 14.0),
            [1, 2, 2, 1, 3]
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
        let actions = message_actions(text);
        assert_eq!(actions.len(), 5);
        assert!(actions[0].label.ends_with(" 1"));
        assert_eq!(actions[0].content, "  let a = 1;\r\n\r\n");
        assert_eq!(actions[1].content, "https://example.test");
        assert!(actions[1].open);
        assert_eq!(actions[4].content, "https://other.test");
        assert!(!actions[4].open);
        assert_eq!(message_block_line_counts(text, 900.0, 14.0), [2, 3, 3]);
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
        let spans = inline("**中文**🙂");
        assert_eq!(spans[0].text.len(), 6);
        assert_eq!(spans[1].text, "🙂");
    }
}
