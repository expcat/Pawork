//! 单行/多行 Composer 输入框。
//!
//! Adapted from gpui 0.2.2 examples/input.rs (Apache-2.0).
//! 裁剪范围：保留内容/占位符/marked_range（IME）/UTF16Selection、
//! Backspace/Delete/Home/End/左右/Paste，以及点击聚焦（波 C 多轮/IME 必需）。
//! ShowCharacterPalette、Copy/Cut/SelectAll 与拖选仍按波 B 范围删除。

use std::ops::Range;

use super::theme::{dark, font, metrics};
use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Context, CursorStyle,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable,
    GlobalElementId, LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use unicode_segmentation::*;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Home,
        End,
        Paste,
        NewLine,
        SendMessage,
    ]
);

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<Vec<ShapedLine>>,
    last_line_starts: Vec<usize>,
    last_bounds: Option<gpui::Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::with_placeholder(
            "Message Pawork… (Enter to send, Shift+Enter for newline)",
            cx,
        )
    }

    pub fn with_placeholder(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_line_starts: Vec::new(),
            last_bounds: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// IME 组合中（marked_range 存在）：Enter 属于输入法确认，不触发发送。
    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content = "".into();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_line_starts.clear();
        self.last_bounds = None;
        cx.notify();
    }

    /// 由原生 Accessibility set-value 入口替换全文；与普通输入相同地把光标
    /// 收到末尾并清除 IME marked range / 旧布局缓存。
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_line_starts.clear();
        self.last_bounds = None;
        cx.notify();
    }

    /// 按原文换行计数（空内容视为 1 行），供 Composer 高度与验收断言使用。
    pub fn visual_line_count(&self) -> usize {
        line_byte_ranges(&self.content).len().max(1)
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\r\n", "\n"), window, cx);
        }
    }

    fn new_line(&mut self, _: &NewLine, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    /// 点击 Composer 必须把焦点拉回输入框。`track_focus` 会注册自动聚焦，
    /// 但点过侧栏/时间线后仍需要显式 `window.focus`，否则键盘/IME/粘贴
    /// 进不了第二轮。
    fn on_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: gpui::Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        let lines = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let (line_index, line_start) = line_index_for_offset(&self.last_line_starts, range.start)?;
        let line = lines.get(line_index)?;
        let line_height = window.line_height();
        let local_start = range.start.saturating_sub(line_start);
        let local_end = range.end.saturating_sub(line_start).min(line.len());
        let top = bounds.top() + line_height * line_index as f32;
        Some(gpui::Bounds::from_corners(
            point(bounds.left() + line.x_for_index(local_start), top),
            point(
                bounds.left() + line.x_for_index(local_end),
                top + line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let lines = self.last_layout.as_ref()?;
        if lines.is_empty() {
            return Some(0);
        }
        let line_height = window.line_height();
        let mut index = 0usize;
        if line_height > px(metrics::ZERO) {
            index = (f32::from(line_point.y) / f32::from(line_height)) as usize;
        }
        index = index.min(lines.len() - 1);
        let line = &lines[index];
        let utf8_index = line.index_for_x(line_point.x).unwrap_or(line.len());
        let start = *self.last_line_starts.get(index).unwrap_or(&0);
        Some(self.offset_to_utf16(start + utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    lines: Vec<ShapedLine>,
    line_starts: Vec<usize>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

fn line_byte_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            ranges.push((start, idx));
            start = idx + 1;
        }
    }
    ranges.push((start, text.len()));
    ranges
}

fn line_index_for_offset(starts: &[usize], offset: usize) -> Option<(usize, usize)> {
    if starts.is_empty() {
        return None;
    }
    let mut index = 0;
    for (i, start) in starts.iter().enumerate() {
        if *start <= offset {
            index = i;
        } else {
            break;
        }
    }
    Some((index, starts[index]))
}

fn clamp_composer_height(line_height: Pixels, line_count: usize) -> Pixels {
    let desired = line_height * line_count as f32 + px(metrics::COMPOSER_TEXT_INSET);
    let min = px(metrics::COMPOSER_MIN_HEIGHT);
    let max = px(metrics::COMPOSER_MAX_HEIGHT);
    if desired < min {
        min
    } else if desired > max {
        max
    } else {
        desired
    }
}

fn runs_for_span(
    span_start: usize,
    span_end: usize,
    base: &TextRun,
    marked: Option<&Range<usize>>,
) -> Vec<TextRun> {
    let len = span_end.saturating_sub(span_start);
    if len == 0 {
        return Vec::new();
    }
    let Some(marked) = marked else {
        return vec![TextRun {
            len,
            ..base.clone()
        }];
    };
    if marked.end <= span_start || marked.start >= span_end {
        return vec![TextRun {
            len,
            ..base.clone()
        }];
    }
    let mark_start = marked.start.max(span_start) - span_start;
    let mark_end = marked.end.min(span_end) - span_start;
    let mut runs = Vec::new();
    if mark_start > 0 {
        runs.push(TextRun {
            len: mark_start,
            ..base.clone()
        });
    }
    if mark_end > mark_start {
        runs.push(TextRun {
            len: mark_end - mark_start,
            underline: Some(UnderlineStyle {
                color: Some(base.color),
                thickness: px(metrics::UNDERLINE_THICKNESS),
                wavy: false,
            }),
            ..base.clone()
        });
    }
    if mark_end < len {
        runs.push(TextRun {
            len: len - mark_end,
            ..base.clone()
        });
    }
    runs.into_iter().filter(|run| run.len > 0).collect()
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let line_count = self.input.read(cx).visual_line_count();
        style.size.height = clamp_composer_height(window.line_height(), line_count).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), dark().text.placeholder.into())
        } else {
            (content, style.color)
        };
        let marked = input.marked_range.clone();
        let base = TextRun {
            len: 0,
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let ranges = line_byte_ranges(&display_text);
        let mut lines = Vec::new();
        let mut line_starts = Vec::new();
        for (start, end) in ranges {
            let line_text: SharedString = display_text[start..end].to_string().into();
            let runs = runs_for_span(start, end, &base, marked.as_ref());
            let shaped = window
                .text_system()
                .shape_line(line_text, font_size, &runs, None);
            lines.push(shaped);
            line_starts.push(start);
        }
        if lines.is_empty() {
            lines.push(
                window
                    .text_system()
                    .shape_line("".into(), font_size, &[], None),
            );
            line_starts.push(0);
        }

        let mut selection = Vec::new();
        let cursor_quad = if selected_range.is_empty() {
            line_index_for_offset(&line_starts, cursor).and_then(|(index, start)| {
                let line = lines.get(index)?;
                let top = bounds.top() + line_height * index as f32;
                Some(fill(
                    gpui::Bounds::new(
                        point(bounds.left() + line.x_for_index(cursor - start), top),
                        size(px(metrics::CURSOR_WIDTH), line_height),
                    ),
                    gpui::blue(),
                ))
            })
        } else {
            for (index, start) in line_starts.iter().copied().enumerate() {
                let end = line_starts
                    .get(index + 1)
                    .copied()
                    .unwrap_or(display_text.len());
                let overlap_start = selected_range.start.max(start);
                let overlap_end = selected_range.end.min(end);
                if overlap_start >= overlap_end {
                    continue;
                }
                let Some(line) = lines.get(index) else {
                    continue;
                };
                let top = bounds.top() + line_height * index as f32;
                selection.push(fill(
                    gpui::Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(overlap_start - start), top),
                        point(
                            bounds.left() + line.x_for_index(overlap_end - start),
                            top + line_height,
                        ),
                    ),
                    dark().accent.selection,
                ));
            }
            None
        };
        PrepaintState {
            lines,
            line_starts,
            cursor: cursor_quad,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        let line_height = window.line_height();
        for (index, line) in prepaint.lines.iter().enumerate() {
            let origin = point(
                bounds.origin.x,
                bounds.origin.y + line_height * index as f32,
            );
            line.paint(origin, line_height, window, cx).unwrap();
        }

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(prepaint.lines.clone());
            input.last_line_starts = prepaint.line_starts.clone();
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::new_line))
            .id("composer-input")
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .w_full()
            .py_1()
            .px_2()
            .rounded_sm()
            .bg(dark().surface.raised)
            .text_size(px(font::BASE))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, TestAppContext};

    use super::{line_byte_ranges, TextInput};

    #[test]
    fn paste_three_lines_counts_three_visual_lines() {
        assert_eq!(line_byte_ranges("a\nb\nc").len(), 3);
        assert_eq!(line_byte_ranges("").len(), 1);
        assert_eq!(line_byte_ranges("single").len(), 1);
        assert_eq!(line_byte_ranges("trail\n").len(), 2);
    }

    #[gpui::test]
    fn accessibility_set_text_replaces_content_and_clears_marked_range(
        cx: &mut TestAppContext,
    ) {
        let input = cx.new(TextInput::new);
        input.update(cx, |input, cx| {
            input.marked_range = Some(0..0);
            input.set_text("AX 输入", cx);
            assert_eq!(input.text(), "AX 输入");
            assert_eq!(input.selected_range, input.content.len()..input.content.len());
            assert!(input.marked_range.is_none());
        });
    }
}
