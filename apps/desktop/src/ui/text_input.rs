//! 单行/多行 Composer 输入框。
//!
//! Adapted from gpui 0.2.2 examples/input.rs (Apache-2.0).
//! 裁剪范围：保留内容/占位符/marked_range（IME）/UTF16Selection、
//! Backspace/Delete/Home/End/左右/Paste，以及点击聚焦（波 C 多轮/IME 必需）。
//! R5 Wave B：shift 选择、鼠标点选/拖选、Copy/Cut/SelectAll、Undo/Redo、overflow scroll。

use std::borrow::Cow;
use std::ops::Range;

use super::theme::{dark, font, metrics};
use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, ClipboardItem, Context,
    CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    Focusable, GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ScrollHandle, ShapedLine, SharedString, Style, TextRun,
    UTF16Selection, UnderlineStyle, Window,
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
        SelectLeft,
        SelectRight,
        SelectToLineStart,
        SelectToLineEnd,
        SelectAll,
        End,
        Paste,
        NewLine,
        Cut,
        Copy,
        Undo,
        Redo,
        SendMessage,
    ]
);

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    element_id: SharedString,
    secure: bool,
    min_height: f32,
    max_height: f32,
    selected_range: Range<usize>,
    selection_reversed: bool,
    is_selecting: bool,
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    scroll: ScrollHandle,
    pending_caret_scroll: bool,
    last_line_height: Pixels,
    marked_range: Option<Range<usize>>,
    last_layout: Option<Vec<ShapedLine>>,
    last_line_starts: Vec<usize>,
    last_bounds: Option<gpui::Bounds<Pixels>>,
}

/// secure 模式掩码字符（U+2022，3 字节 UTF-8）；每个 grapheme 一个。
const SECURE_MASK: &str = "•";

/// SET-010：API key 单行语义。secure 输入剔除 CR/LF（粘贴 / AX set-value / IME 共用）。
fn sanitize_secure<'a>(secure: bool, text: &'a str) -> Cow<'a, str> {
    if !secure {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace(['\r', '\n'], ""))
}

#[derive(Clone)]
struct EditSnapshot {
    content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
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
            element_id: SharedString::from("composer-input"),
            secure: false,
            min_height: metrics::COMPOSER_INPUT_MIN_HEIGHT,
            max_height: composer_input_max_height(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_line_starts: Vec::new(),
            last_bounds: None,
            is_selecting: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            scroll: ScrollHandle::new(),
            pending_caret_scroll: false,
            last_line_height: px(metrics::ZERO),
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
        self.push_undo();
        self.content = "".into();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_line_starts.clear();
        self.last_bounds = None;
        cx.notify();
    }

    /// Composer 动态 hint：空内容时显示，颜色走 theme placeholder token。
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder == placeholder {
            return;
        }
        self.placeholder = placeholder;
        cx.notify();
    }

    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// 覆盖 GPUI element id（Composer 默认 `composer-input`；Terminal 用
    /// `terminal-input`，避免与 Composer AX 节点冲突）。
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.element_id = id.into();
        self
    }

    /// 覆盖高度钳制。Composer 走面板预算；Terminal 独立 28–220，不被
    /// Composer 面板预算截断。
    pub fn height_clamp(mut self, min: f32, max: f32) -> Self {
        self.min_height = min;
        self.max_height = max.max(min);
        self
    }

    /// SET-010 secure 模式：渲染按 grapheme 掩码，Copy/Cut 不写剪贴板，
    /// 换行被剔除（API key 单行语义）；明文只留在本实体内存与编辑栈，
    /// AX value 由宿主发布掩码（本组件不参与 AX 树构建）。
    pub fn secure(mut self) -> Self {
        self.secure = true;
        self
    }

    /// secure 掩码串（非 secure 或空内容返回 None；空内容走 placeholder）。
    pub(crate) fn secure_mask(&self) -> Option<String> {
        if !self.secure || self.content.is_empty() {
            return None;
        }
        Some(self.content.graphemes(true).map(|_| SECURE_MASK).collect())
    }

    /// 显示文本字节长（secure：grapheme 数 × 掩码字节长）。
    fn display_text_len(&self) -> usize {
        if !self.secure {
            return self.content.len();
        }
        self.content.graphemes(true).count() * SECURE_MASK.len()
    }

    /// content 字节偏移 → 显示文本字节偏移（grapheme 一一对应）。
    fn to_display_offset(&self, offset: usize) -> usize {
        if !self.secure {
            return offset;
        }
        let offset = offset.min(self.content.len());
        self.content[..offset].graphemes(true).count() * SECURE_MASK.len()
    }

    /// 显示文本字节偏移 → content 字节偏移（越界回落末尾）。
    fn from_display_offset(&self, offset: usize) -> usize {
        if !self.secure {
            return offset;
        }
        self.content
            .grapheme_indices(true)
            .nth(offset / SECURE_MASK.len())
            .map(|(index, _)| index)
            .unwrap_or(self.content.len())
    }

    /// 由原生 Accessibility set-value 入口替换全文；与普通输入相同地把光标
    /// 收到末尾并清除 IME marked range / 旧布局缓存。
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.push_undo();
        self.content = sanitize_secure(self.secure, text.into().as_ref())
            .into_owned()
            .into();
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

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn restore_snapshot(&mut self, snap: EditSnapshot) {
        self.content = snap.content;
        self.selected_range = snap.selected_range;
        self.selection_reversed = snap.selection_reversed;
        self.marked_range = None;
        self.last_layout = None;
        self.last_line_starts.clear();
        self.last_bounds = None;
        self.is_selecting = false;
        self.pending_caret_scroll = true;
    }

    fn push_undo(&mut self) {
        let snap = if let Some(marked) = &self.marked_range {
            let content = format!(
                "{}{}",
                &self.content[..marked.start],
                &self.content[marked.end..]
            );
            EditSnapshot {
                content: content.into(),
                selected_range: marked.start..marked.start,
                selection_reversed: false,
            }
        } else {
            self.snapshot()
        };
        self.undo_stack.push(snap);
        self.redo_stack.clear();
    }

    #[cfg(test)]
    pub(crate) fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    #[cfg(test)]
    pub(crate) fn selected_range(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    pub(crate) fn reset_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_line_starts.clear();
        self.last_bounds = None;
        self.is_selecting = false;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.pending_caret_scroll = true;
        cx.notify();
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

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_to_line_start(
        &mut self,
        _: &SelectToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(0, cx);
    }

    fn select_to_line_end(&mut self, _: &SelectToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        // SET-010：secure 输入不把明文写入剪贴板（no-op 而非报错）。
        if self.secure {
            return;
        }
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.content[self.selected_range.clone()].to_string(),
        ));
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        // SET-010：secure 输入禁止 Cut 泄漏明文。
        if self.secure {
            return;
        }
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.content[self.selected_range.clone()].to_string(),
        ));
        self.replace_text_in_range(None, "", window, cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(prev) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.snapshot());
        self.restore_snapshot(prev);
        cx.notify();
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.snapshot());
        self.restore_snapshot(next);
        cx.notify();
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
            let text = if self.secure {
                // API key 单行语义：剔除粘贴带入的换行（含尾随换行）。
                text.replace(['\r', '\n'], "")
            } else {
                text.replace("\r\n", "\n")
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn new_line(&mut self, _: &NewLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.secure {
            return;
        }
        self.replace_text_in_range(None, "\n", window, cx);
    }

    /// 点击 Composer 必须把焦点拉回输入框。`track_focus` 会注册自动聚焦，
    /// 但点过侧栏/时间线后仍需要显式 `window.focus`，否则键盘/IME/粘贴
    /// 进不了第二轮。
    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position, window);
        if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position, window), cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.pending_caret_scroll = true;
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
        self.pending_caret_scroll = true;
        cx.notify()
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>, window: &Window) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        let Some(lines) = self.last_layout.as_ref() else {
            return 0;
        };
        if lines.is_empty() {
            return 0;
        }
        let line_height = if self.last_line_height > px(metrics::ZERO) {
            self.last_line_height
        } else {
            window.line_height()
        };
        let viewport = self.scroll.bounds();
        let hit_bounds = if viewport.size.height > px(metrics::ZERO) {
            viewport
        } else {
            bounds
        };
        if position.y < hit_bounds.top() {
            return 0;
        }
        if position.y > hit_bounds.bottom() {
            return self.content.len();
        }
        // last_bounds 已归一化为布局原点（见 PrepaintState::content_bounds），
        // 内容 y = 视口内 y − scroll offset；该式在稳态帧与滚动后一帧的
        // 过渡态下均成立。
        let content_y = f32::from(position.y - bounds.top()) - f32::from(self.scroll.offset().y);
        let mut index = 0usize;
        if line_height > px(metrics::ZERO) {
            index = (content_y.max(0.0) / f32::from(line_height)) as usize;
        }
        index = index.min(lines.len() - 1);
        let line = &lines[index];
        let start = *self.last_line_starts.get(index).unwrap_or(&0);
        let local = line.closest_index_for_x(position.x - bounds.left());
        // last_layout 为显示文本（secure 掩码）空间：换算回 content 偏移。
        let display_index = (start + local).min(self.display_text_len());
        self.from_display_offset(display_index)
    }

    fn scroll_caret_into_view(&mut self) {
        if self.last_line_starts.is_empty() || self.last_line_height <= px(metrics::ZERO) {
            return;
        }
        // 视口必须取 ScrollHandle 记录的容器高；last_bounds 是完整内容高，
        // 拿它当视口会让「caret 超出视口」永不成立（R5 Wave B 评审 F3 死症）。
        let viewport = f32::from(self.scroll.bounds().size.height);
        if viewport <= 0.0 {
            // 容器尚未 prepaint，等下一帧（pending 标记保留）。
            return;
        }
        let Some((line_index, _)) =
            line_index_for_offset(&self.last_line_starts, self.cursor_offset())
        else {
            return;
        };
        let line_top = f32::from(self.last_line_height) * line_index as f32;
        let line_bottom = line_top + f32::from(self.last_line_height);
        let current = f32::from(self.scroll.offset().y);
        let mut next = current;
        if line_top + current < 0.0 {
            next = -line_top;
        } else if line_bottom + current > viewport {
            next = viewport - line_bottom;
        }
        if (next - current).abs() > f32::EPSILON {
            self.scroll.set_offset(point(px(metrics::ZERO), px(next)));
        }
        self.pending_caret_scroll = false;
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

        let new_text = sanitize_secure(self.secure, new_text);
        self.push_undo();
        self.content = (self.content[0..range.start].to_owned()
            + new_text.as_ref()
            + &self.content[range.end..])
            .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
        self.pending_caret_scroll = true;
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

        let new_text = sanitize_secure(self.secure, new_text);
        self.content = (self.content[0..range.start].to_owned()
            + new_text.as_ref()
            + &self.content[range.end..])
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
        // IME 标记范围以 content 偏移表达；布局在显示文本空间。
        let range = self.to_display_offset(range.start)..self.to_display_offset(range.end);
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
        // 与 paint 时行高一致：平台调用发生在 paint 之外，window.line_height()
        // 拿不到元素 text_size 上下文（与 index_for_mouse_position 同一回退）。
        let line_height = if self.last_line_height > px(metrics::ZERO) {
            self.last_line_height
        } else {
            window.line_height()
        };
        let mut index = 0usize;
        if line_height > px(metrics::ZERO) {
            // 与 index_for_mouse_position 同一坐标语义：last_bounds 为归一化
            // 布局原点，内容 y 须再减 scroll offset。
            let content_y = f32::from(line_point.y) - f32::from(self.scroll.offset().y);
            index = (content_y.max(0.0) / f32::from(line_height)) as usize;
        }
        index = index.min(lines.len() - 1);
        let line = &lines[index];
        let utf8_index = line.index_for_x(line_point.x).unwrap_or(line.len());
        let start = *self.last_line_starts.get(index).unwrap_or(&0);
        Some(self.offset_to_utf16(self.from_display_offset(start + utf8_index)))
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
    /// 去掉滚动平移的布局原点 bounds（origin − element_offset）。GPUI 0.2.2
    /// 滚动容器子元素 prepaint bounds 是否含 scroll offset 取决于帧时序，
    /// 归一化后鼠标 / IME 坐标映射在任何帧状态下都一致。
    content_bounds: gpui::Bounds<Pixels>,
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

fn composer_input_max_height() -> f32 {
    // 面板预算扣掉 border / pad / gap / footer 动作槽后，才是输入区可增长高度。
    (metrics::COMPOSER_PANEL_MAX_HEIGHT
        - metrics::COMPOSER_BORDER
        - metrics::COMPOSER_PAD * 2.0
        - metrics::COMPOSER_GAP
        - metrics::COMPOSER_SEND_SIZE)
        .max(metrics::COMPOSER_INPUT_MIN_HEIGHT)
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
        // 完整内容高交给父 overflow 视口；此处只保底单行，不 clamp 到 max。
        let min_height = self.input.read(cx).min_height;
        let desired = window.line_height() * line_count as f32 + px(metrics::COMPOSER_TEXT_INSET);
        let min = px(min_height);
        style.size.height = if desired < min { min } else { desired }.into();
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
        let content_bounds = gpui::Bounds {
            origin: bounds.origin - window.element_offset(),
            size: bounds.size,
        };
        let input = self.input.read(cx);
        // SET-010 secure：布局/光标/选择一律在显示文本（掩码）空间进行，
        // 偏移经 grapheme 映射换算；content 明文不进任何布局产物。
        let display_text: SharedString = if let Some(mask) = input.secure_mask() {
            mask.into()
        } else if input.content.is_empty() {
            input.placeholder.clone()
        } else {
            input.content.clone()
        };
        let selected_range = input.to_display_offset(input.selected_range.start)
            ..input.to_display_offset(input.selected_range.end);
        let cursor = input.to_display_offset(input.cursor_offset());
        let style = window.text_style();

        let text_color = if input.content.is_empty() {
            dark().text.placeholder.into()
        } else {
            style.color
        };
        let marked = input
            .marked_range
            .as_ref()
            .map(|range| input.to_display_offset(range.start)..input.to_display_offset(range.end));
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
            content_bounds,
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

        self.input.update(cx, |input, cx| {
            input.last_layout = Some(prepaint.lines.clone());
            input.last_line_starts = prepaint.line_starts.clone();
            input.last_bounds = Some(prepaint.content_bounds);
            input.last_line_height = line_height;
            if input.pending_caret_scroll {
                let before = input.scroll.offset();
                input.scroll_caret_into_view();
                if input.scroll.offset() != before {
                    // offset 变更发生在容器本帧 paint 之后，需再排一帧生效。
                    cx.notify();
                }
            }
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 固定单行输入也必须容纳当前字号的行高与 py_1 内边距。
        let line_min_height = (font::BASE.0 * 1.5 + 0.5) * f32::from(window.rem_size());
        div()
            .flex()
            .key_context("TextInput")
            .max_h(px(self.max_height.max(line_min_height)))
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
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_to_line_start))
            .on_action(cx.listener(Self::select_to_line_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .id(self.element_id.clone())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .py_1()
            .px_2()
            .rounded_sm()
            .bg(dark().surface.raised)
            .text_size(font::BASE)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
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
    use gpui::{
        point, px, size, AppContext, EntityInputHandler, Focusable, Modifiers, TestAppContext,
        VisualTestContext,
    };

    use super::{line_byte_ranges, TextInput};

    const PROBE_WINDOW: gpui::Size<gpui::Pixels> = size(px(640.), px(360.));

    /// 真窗口挂载单个 TextInput（与 u1_probe 同路径）：overflow scroll 与
    /// 点击映射的断言依赖真实布局 / paint。
    fn mount_input(cx: &mut TestAppContext) -> (gpui::Entity<TextInput>, &mut VisualTestContext) {
        cx.update(|cx| crate::ui::install_keybindings(cx));
        let (input, cx) = cx.add_window_view(|_window, cx| TextInput::new(cx));
        cx.simulate_resize(PROBE_WINDOW);
        cx.refresh().expect("refresh after mount");
        cx.run_until_parked();
        (input, cx)
    }

    fn focus_input(input: &gpui::Entity<TextInput>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let handle = input.read(cx).focus_handle(cx);
            window.focus(&handle);
            window.activate_window();
        });
        cx.refresh().expect("refresh after focus");
        cx.run_until_parked();
    }

    fn eighty_lines() -> String {
        (0..80)
            .map(|i| format!("line-{i:02}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn paste_three_lines_counts_three_visual_lines() {
        assert_eq!(line_byte_ranges("a\nb\nc").len(), 3);
        assert_eq!(line_byte_ranges("").len(), 1);
        assert_eq!(line_byte_ranges("single").len(), 1);
        assert_eq!(line_byte_ranges("trail\n").len(), 2);
    }

    #[gpui::test]
    fn accessibility_set_text_replaces_content_and_clears_marked_range(cx: &mut TestAppContext) {
        let input = cx.new(TextInput::new);
        input.update(cx, |input, cx| {
            input.marked_range = Some(0..0);
            input.set_text("AX 输入", cx);
            assert_eq!(input.text(), "AX 输入");
            assert_eq!(
                input.selected_range,
                input.content.len()..input.content.len()
            );
            assert!(input.marked_range.is_none());
        });
    }

    #[gpui::test]
    fn secure_input_exposes_only_grapheme_mask(cx: &mut TestAppContext) {
        let input = cx.new(|cx| TextInput::new(cx).secure());
        input.update(cx, |input, cx| {
            input.set_text("sk-live\nplaintext\r", cx);
            assert_eq!(input.text(), "sk-liveplaintext");
            let masked = input.secure_mask().expect("secure input exposes the mask");
            assert!(!masked.contains("sk-liveplaintext"));
            assert_eq!(masked.chars().count(), "sk-liveplaintext".chars().count());
        });
        // 非 secure 输入不发布掩码（走普通明文渲染路径）。
        let plain = cx.new(|cx| TextInput::with_placeholder("visible", cx));
        plain.update(cx, |plain, cx| {
            plain.set_text("visible", cx);
            assert_eq!(plain.secure_mask(), None);
        });
    }

    #[gpui::test]
    fn set_placeholder_updates_empty_hint(cx: &mut TestAppContext) {
        let input = cx.new(TextInput::new);
        input.update(cx, |input, cx| {
            assert!(input.placeholder().contains("Enter to send"));
            input.set_placeholder(
                "Run in progress — sending is disabled. Cancel remains available.",
                cx,
            );
            assert_eq!(
                input.placeholder(),
                "Run in progress — sending is disabled. Cancel remains available."
            );
        });
    }

    #[test]
    fn composer_viewport_budget_keeps_panel_within_max() {
        // R5 Wave B 起输入区不再自我 clamp：TextElement 按完整内容高布局，
        // 视口由父容器 max_h = composer_input_max_height() 兑现；此处锁定
        // 预算算术，面板总高合同（≤220）由该 max_h 保证。
        let viewport_max = super::composer_input_max_height();
        use crate::ui::theme::metrics;
        assert!(viewport_max >= metrics::COMPOSER_INPUT_MIN_HEIGHT);
        assert!(viewport_max < metrics::COMPOSER_MAX_HEIGHT);
        let panel = metrics::COMPOSER_BORDER
            + metrics::COMPOSER_PAD * 2.0
            + viewport_max
            + metrics::COMPOSER_GAP
            + metrics::COMPOSER_SEND_SIZE;
        assert!(panel <= metrics::COMPOSER_PANEL_MAX_HEIGHT + 0.5);
    }

    #[gpui::test]
    fn terminal_height_clamp_is_independent_of_composer_budget(cx: &mut TestAppContext) {
        // Terminal 独立 28–220，不被 Composer 面板预算（163）截断。
        let terminal = cx.new(|cx| TextInput::new(cx).height_clamp(28.0, 220.0));
        terminal.update(cx, |terminal, _| {
            assert_eq!(terminal.min_height, 28.0);
            assert_eq!(terminal.max_height, 220.0);
            assert!(terminal.max_height > super::composer_input_max_height());
        });
        let composer = cx.new(TextInput::new);
        composer.update(cx, |composer, _| {
            assert_eq!(
                composer.min_height,
                crate::ui::theme::metrics::COMPOSER_INPUT_MIN_HEIGHT
            );
            assert_eq!(composer.max_height, super::composer_input_max_height());
        });
    }
    #[gpui::test]
    fn shift_select_via_action_handlers(cx: &mut TestAppContext) {
        let (input, cx) = mount_input(cx);
        focus_input(&input, cx);
        input.update(cx, |input, cx| {
            input.reset_text("abcd", cx);
            input.move_to(2, cx);
        });
        cx.dispatch_action(super::SelectLeft);
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |i, _| i.selected_range()), 1..2);
        cx.dispatch_action(super::SelectLeft);
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |i, _| i.selected_range()), 0..2);
        cx.dispatch_action(super::SelectRight);
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |i, _| i.selected_range()), 1..2);
        let selected = input.read_with(cx, |i, _| i.text()[i.selected_range()].to_string());
        assert_eq!(selected, "b");
    }

    #[gpui::test]
    fn ime_commit_stacks_single_undo_via_input_handler(cx: &mut TestAppContext) {
        // 平台 IME 真实路径：replace_and_mark_text_in_range（拼音中间态）→
        // replace_text_in_range（commit）。中间态不入 undo 栈，commit 恰好
        // 单次入栈，undo 一步回到 commit 前。
        let (input, cx) = mount_input(cx);
        focus_input(&input, cx);
        input.update(cx, |input, cx| input.reset_text("", cx));
        let before = input.read_with(cx, |i, _| i.undo_len());
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "ni", None, window, cx);
            });
        });
        let (composing, mid) = input.read_with(cx, |i, _| (i.is_composing(), i.undo_len()));
        assert!(composing, "marked 区间存在即组合中");
        assert_eq!(mid, before, "拼音中间态不得入 undo 栈");
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "你", window, cx);
            });
        });
        cx.run_until_parked();
        let (text, stacked, composing) = input.read_with(cx, |i, _| {
            (i.text().to_string(), i.undo_len(), i.is_composing())
        });
        assert_eq!(text, "你");
        assert!(!composing);
        assert_eq!(stacked, before + 1, "commit 必须恰好单次入栈");
        cx.dispatch_action(super::Undo);
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |i, _| i.text().to_string()), "");
        cx.dispatch_action(super::Redo);
        cx.run_until_parked();
        assert_eq!(input.read_with(cx, |i, _| i.text().to_string()), "你");
    }

    #[gpui::test]
    fn overflow_scroll_engages_and_caret_scrolls_into_view(cx: &mut TestAppContext) {
        let (input, cx) = mount_input(cx);
        input.update(cx, |input, cx| {
            input.reset_text(eighty_lines(), cx);
            input.move_to(0, cx);
        });
        cx.refresh().expect("refresh after paste");
        cx.run_until_parked();
        let (max_offset, viewport, offset_at_top) = input.read_with(cx, |i, _| {
            (
                i.scroll.max_offset(),
                i.scroll.bounds().size.height,
                i.scroll.offset(),
            )
        });
        assert!(
            max_offset.height > px(0.),
            "80 行内容必须产生真实可滚动区间"
        );
        assert!(
            viewport <= px(super::composer_input_max_height() + 0.5),
            "composer 视口不得突破面板预算：{viewport:?}"
        );
        assert!(
            viewport >= px(crate::ui::theme::metrics::COMPOSER_INPUT_MIN_HEIGHT),
            "composer 视口不得小于单行保底：{viewport:?}"
        );
        assert_eq!(
            offset_at_top,
            point(px(0.), px(0.)),
            "caret 在首行时不需要滚动"
        );
        let end = input.read_with(cx, |i, _| i.text().len());
        input.update(cx, |input, cx| input.move_to(end, cx));
        cx.refresh().expect("refresh after caret to end");
        cx.run_until_parked();
        let (offset, max_offset) =
            input.read_with(cx, |i, _| (i.scroll.offset(), i.scroll.max_offset()));
        assert!(
            offset.y < px(0.),
            "caret 移到末行后必须向上滚动（GPUI offset 语义：向下滚动为负）：{offset:?}"
        );
        assert!(
            f32::from(offset.y).abs() <= f32::from(max_offset.height) + 0.5,
            "滚动量不得超出 max_offset：offset={offset:?} max={max_offset:?}"
        );
    }

    #[gpui::test]
    fn click_maps_to_visible_line_with_scroll_offset(cx: &mut TestAppContext) {
        let (input, cx) = mount_input(cx);
        input.update(cx, |input, cx| {
            input.reset_text(eighty_lines(), cx);
        });
        cx.refresh().expect("refresh after paste");
        cx.run_until_parked();
        // reset_text 把 caret 放到末尾 → 内容首帧后已滚到底（offset<0）。
        let (offset, viewport_top, elem_top, left, line_height, starts) =
            input.read_with(cx, |i, _| {
                let b = i.last_bounds.expect("painted bounds");
                (
                    i.scroll.offset(),
                    i.scroll.bounds().top(),
                    b.top(),
                    b.left(),
                    i.last_line_height,
                    i.last_line_starts.clone(),
                )
            });
        assert!(offset.y < px(0.), "前置：必须已处于滚动态");
        assert_eq!(starts.len(), 80);
        // 精确期望行：last_bounds 已归一化为布局原点（与帧时序无关），
        // 内容 y = 点击 y − 元素顶 − offset.y；双重计入 offset 会撞
        // ≈2×expected，漏计会撞首行，均无法通过相等断言。
        let click = point(left + px(8.), viewport_top + line_height / 2.0);
        let expected_line = ((f32::from(click.y - elem_top) - f32::from(offset.y)).max(0.0)
            / f32::from(line_height)) as usize;
        assert!(
            (1..=78).contains(&expected_line),
            "前置：期望行必须是被滚入视口的中间行（实测 expected={expected_line}）"
        );
        cx.simulate_click(click, Modifiers::none());
        cx.run_until_parked();
        let caret = input.read_with(cx, |i, _| i.selected_range().start);
        let (line, _) = super::line_index_for_offset(&starts, caret).expect("caret maps to a line");
        assert_eq!(
            line, expected_line,
            "滚动态点击必须精确映回物理可见行（caret={caret}）"
        );
        // IME/AX 的 character_index_for_point 必须与鼠标映射同一坐标语义。
        let ime_index = cx
            .update(|window, cx| {
                input.update(cx, |input, cx| {
                    input.character_index_for_point(click, window, cx)
                })
            })
            .expect("point inside bounds");
        // 内容全 ASCII，UTF-16 偏移 = 字节偏移。
        let (ime_line, _) =
            super::line_index_for_offset(&starts, ime_index).expect("ime index maps to a line");
        assert_eq!(ime_line, expected_line, "IME 映射必须与鼠标映射一致");
        // 再点视口末行：caret 必须严格更靠后且不越出末行。
        let viewport_bottom = input.read_with(cx, |i, _| i.scroll.bounds().bottom());
        let click = point(left + px(8.), viewport_bottom - line_height / 2.0);
        cx.simulate_click(click, Modifiers::none());
        cx.run_until_parked();
        let caret_bottom = input.read_with(cx, |i, _| i.selected_range().start);
        let (line_bottom, _) =
            super::line_index_for_offset(&starts, caret_bottom).expect("caret maps to a line");
        assert!(line_bottom > line, "视口末行点击必须比首行更靠后");
        assert!(line_bottom <= 79, "不得越出末行");
    }

    #[gpui::test]
    fn reset_text_restores_draft_without_undo_history(cx: &mut TestAppContext) {
        let input = cx.new(TextInput::new);
        input.update(cx, |input, cx| {
            input.reset_text("draft-A", cx);
            input.set_text("mutated", cx);
            assert_eq!(input.undo_len(), 1);
            input.reset_text("draft-B", cx);
            assert_eq!(input.text(), "draft-B");
            assert_eq!(input.undo_len(), 0);
            input.reset_text("", cx);
            assert_eq!(input.text(), "");
        });
    }
}
