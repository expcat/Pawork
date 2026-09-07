//! Timeline 的小型 Markdown 子集；渲染与 AX 高度估算共用解析后的可见文本。

use gpui::{div, prelude::*, px, FontStyle, FontWeight, Rgba, StyledText, TextRun};

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockKind {
    Paragraph,
    Heading,
    List,
    Quote,
    Code,
}

#[derive(Debug)]
struct Block {
    kind: BlockKind,
    lines: Vec<Vec<Span>>,
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
    }
}

/// 只剥离已闭合的标记；扫描按 Unicode 字符前进，代码内容不再解析。
fn inline(text: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut rest = text;
    let mut plain = String::new();
    while !rest.is_empty() {
        let mut matched = None;
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
                if let Some(target_end) = target.find(')').filter(|end| *end > 0) {
                    // 当前无打开链接动作，目标直接可读，避免把地址丢在不可访问的元数据里。
                    matched = Some((
                        1 + label_end + 2 + target_end + 1,
                        format!("{} ({})", &label[..label_end], &target[..target_end]),
                        InlineStyle::Link,
                    ));
                }
            }
        }
        if let Some((consumed, content, style)) = matched {
            if !plain.is_empty() {
                spans.push(Span {
                    text: std::mem::take(&mut plain),
                    style: InlineStyle::Plain,
                });
            }
            spans.push(Span {
                text: content,
                style,
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

fn parse(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut fenced = None;
    let mut separated = true;
    for raw in text.lines() {
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
            });
            fenced = Some((marker, count));
            continue;
        }
        if line.is_empty() {
            separated = true;
            continue;
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
            });
        }
        separated = false;
    }
    for block in &mut blocks {
        if block.lines.is_empty() {
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

pub(super) fn message_body_element(text: &str, color: Rgba) -> gpui::Div {
    let mut body = div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(metrics::MSG_PARAGRAPH_GAP))
        .text_size(font::BODY)
        .line_height(font::from_pixels(metrics::MSG_LINE_HEIGHT))
        .text_color(color);
    for block in parse(text) {
        let mut element = div().flex().flex_col().w_full();
        if block.kind == BlockKind::Code {
            element = element
                .px(px(12.0))
                .bg(dark().surface.raised)
                .rounded(px(4.0));
        } else if block.kind == BlockKind::Quote {
            element = element
                .pl(px(12.0))
                .border_l_2()
                .border_color(dark().border.subtle);
        }
        for line in block.lines {
            element = element.child(
                div()
                    .w_full()
                    .min_h(font::from_pixels(metrics::MSG_LINE_HEIGHT))
                    .child(styled_line(line, block.kind, color)),
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
            block
                .lines
                .iter()
                .map(|line| {
                    line.iter()
                        .map(|span| span.text.chars().count())
                        .sum::<usize>()
                        .div_ceil(chars_per_line)
                        .max(1)
                })
                .sum()
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
            [1, 1, 2, 1, 2]
        );
    }

    #[test]
    fn streaming_unicode_and_unclosed_markers_preserve_content() {
        for text in ["中文 **未完成", "🙂 `代码", "[链接](尚未闭合"] {
            assert_eq!(
                inline(text)
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>(),
                text
            );
        }
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
