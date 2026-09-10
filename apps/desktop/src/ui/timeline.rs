//! Timeline 容器：gpui list() 变高虚拟化 + 渲染行组装（R4 Wave A）。
//!
//! 四条滚动 / 重建合同（行为与迁移前一致；机制自 Bottom 钉底改为 Top +
//! 显式跟随，F-06：短会话从 Header 下开始，不再沉底）：
//!
//! 1. Top 对齐 + logical_scroll_top == None 时从首条渲染；内容不足视口
//!    由布局向上补齐并钳制在顶部（gpui list.rs Top 分支）。
//! 2. 跟随态由 AppView::timeline_following 单一状态表达：Top 对齐没有
//!    Bottom 的「None 即钉底」语义，reset 后显式 scroll_to 末项底；新
//!    内容只在用户位于底部时追加跟随。
//! 3. 脱钩检测走滚动事件事实（is_scrolled 在 Top 对齐下滚动过即恒真，
//!    不能再用；像素判定读取 ListState 会在 gpui 派发写借用内重入 panic，
//!    且未测高项使 max 系统性低估）：事件 visible_range 覆盖末项即贴底，
//!    末项滚出视口即脱钩；上滚脱钩后新内容不抢滚动位置，BackToBottom /
//!    滚回底部重挂。
//! 4. projection 有条目替换语义，timeline 任何变化统一 reset(new_count)
//!    （splice 不安全）；脱钩读史时恢复 reset 前偏移（item_ix 越界钳制
//!    到新末项），视口不跳。

use std::collections::HashSet;

use gpui::{
    div, list, prelude::*, px, AnyElement, Context, ListOffset, ListState, Pixels, SharedString,
    WeakEntity, Window,
};

use crate::projection::{
    run_footer_label, run_summary_texts, ConnectionState, ForkBoundary, TimelineEntry,
    TimelineEntryKind, TimelineRow,
};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::Dropdown;
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::label::Label;
use crate::ui::i18n::t;
use crate::ui::theme::{dark, font, metrics};

use super::timeline_entry::{
    default_text_line_height, display_time, estimated_wrapped_lines, message_block_line_counts,
    tool_row_height, RunSummaryTerminal, RunSummaryView, ToolRowView,
};
use super::{now_unix_ms, workspace_empty_hint, workspace_empty_title, AppView, MenuKind};

/// list() 视口外上下方向的预渲染量（px，非视觉尺寸；仅影响滚动顺滑度）。
pub(super) const TIMELINE_OVERDRAW: f32 = 200.0;

/// 挂接 list 滚动跟随（AppView::new 时设置一次；WeakEntity 不构成引用环）。
pub(super) fn install_scroll_follow(state: &ListState, view: &WeakEntity<AppView>) {
    state.set_scroll_handler({
        let view = view.clone();
        move |event, _window, cx| {
            // 贴底判定只用事件事实（visible_range 覆盖末项），handler 内
            // 禁止触碰 ListState：gpui 0.2.2 在 scroll() 写借用存活期派发
            // 本回调，任何 borrow 都会 BorrowMutError panic（审查 P0）。
            let following = event.count > 0 && event.visible_range.end >= event.count;
            view.update(cx, |view, cx| {
                if view.timeline_following != following {
                    view.timeline_following = following;
                    cx.notify();
                }
            })
            .ok();
        }
    });
}

/// timeline 数据 / 宽度变化 → reset 计数并恢复视口（render 前调用）。
/// count 以渲染行（timeline_rows）为单位；approval 卡为 list 末项。
fn sync_list(view: &mut AppView, row_count: usize) {
    let count = row_count + usize::from(view.projection.pending_approval.is_some());
    if view.timeline_list_rev == view.timeline_rev && view.timeline_list_count == count {
        return;
    }
    // 条目「···」菜单浮层锚在条目内：reset 使高度缓存失效、条目可能被虚拟化
    // 卸载，开着则先关（close on reset）。
    if matches!(view.open_menu, Some(MenuKind::Entry(_))) {
        view.open_menu = None;
    }
    let previous = view.timeline_list.logical_scroll_top();
    view.timeline_list.reset(count);
    if view.timeline_following {
        // Top 对齐无自动钉底：跟随态显式滚到新末项底（布局从末项向上补齐）。
        view.timeline_list.scroll_to(ListOffset {
            item_ix: count,
            offset_in_item: Pixels::ZERO,
        });
    } else {
        // 脱钩读史：恢复 reset 前偏移（item_ix 越界钳制到新末项）。
        view.timeline_list.scroll_to(ListOffset {
            item_ix: previous.item_ix.min(count),
            offset_in_item: previous.offset_in_item,
        });
    }
    view.timeline_list_rev = view.timeline_rev;
    view.timeline_list_count = count;
}

/// ToolCall 已知状态本地化；未知 wire 状态原样显示不伪造。render 与 AX 共用。
pub(super) fn tool_status_label(status: &str) -> String {
    match status {
        "succeeded" => t("tool.status_completed"),
        "running" => t("tool.group_running"),
        "pending" => t("tool.group_pending"),
        "failed" => t("tool.group_failed"),
        "cancelled" => t("tool.group_cancelled"),
        other => other,
    }
    .into()
}

/// UI-3：消息间距 32px，工具组前 16px；独立终态页脚前 12px。
pub(super) fn row_top_gap(row: &TimelineRow) -> f32 {
    match row {
        TimelineRow::Message { .. } | TimelineRow::Error { .. } | TimelineRow::RunPhase { .. } => {
            metrics::MSG_ENTRY_GAP
        }
        TimelineRow::ToolGroup { .. } | TimelineRow::Thinking { .. } => metrics::TOOL_GROUP_TOP_GAP,
        TimelineRow::RunSummary { group, .. } => {
            if group.is_some() {
                metrics::TOOL_GROUP_TOP_GAP
            } else {
                metrics::TIMELINE_FOOTER_GAP
            }
        }
    }
}

/// 消息测高与 entry_shell 同源：作者/动作行 + 段落；用户消息另计卡片内边距。
fn message_entry_height(text: &str, column_width: f32, rem_px: f32, user: bool) -> f32 {
    let inset = if user { metrics::MSG_USER_INSET } else { 0.0 };
    let label = default_text_line_height(font::BODY_SM.0 * rem_px).max(24.0);
    let body_font_px = font::BODY.0 * rem_px;
    let body_line_height = (font::from_pixels(metrics::MSG_LINE_HEIGHT).0 * rem_px).round();
    let body_width = (column_width - 2.0 * inset).max(0.0);
    let blocks = message_block_line_counts(text, body_width, body_font_px);
    let body = blocks
        .iter()
        .map(|lines| *lines as f32 * body_line_height)
        .sum::<f32>()
        + metrics::MSG_PARAGRAPH_GAP * blocks.len().saturating_sub(1) as f32;
    2.0 * inset + label + metrics::MSG_LABEL_BODY_GAP + body
}

/// 思考展开区与渲染共用 12px 内边距和次级正文字号。
pub(super) fn thinking_body_height(text: &str, width: f32, rem: f32) -> f32 {
    let font_px = font::BASE.0 * rem;
    24.0 + default_text_line_height(font_px)
        * estimated_wrapped_lines(text, (width - 24.0).max(0.0), font_px).max(1) as f32
}

/// Run 摘要卡高度（run_summary_element 同源）：py_6×2 + max(左列, 40 槽)；
/// 左列 = max(Ø40, 标题行) + gap_4 + 说明（line_clamp 2，行高 1.5rem）。
fn run_summary_card_height(
    terminal: &TimelineEntry,
    column_width: f32,
    rem_px: f32,
    review_changes_visible: bool,
) -> f32 {
    let description = run_summary_texts(terminal, review_changes_visible)
        .map(|(_, description)| description)
        .unwrap_or_default();
    let desc_font_px = font::BODY_SM.0 * rem_px;
    // 说明列宽估计：卡内容（pl 15 + pr_5）- gap_6 - 168 按钮槽 - Ø40 占位 - gap_4。
    let review_slot = if review_changes_visible {
        1.5 * rem_px + metrics::SUMMARY_BUTTON_WIDTH
    } else {
        0.0
    };
    let desc_width = (column_width
        - (metrics::TOOL_GROUP_INNER_INSET
            + 1.25 * rem_px
            + review_slot
            + metrics::SUMMARY_CHECK_CIRCLE
            + 1.0 * rem_px))
        .max(0.0);
    let desc_lines = estimated_wrapped_lines(&description, desc_width, desc_font_px).clamp(1, 2);
    let body_line_height = (font::from_pixels(metrics::MSG_LINE_HEIGHT).0 * rem_px).round();
    let left_column = metrics::SUMMARY_CHECK_CIRCLE
        .max(default_text_line_height(font::BODY.0 * rem_px))
        + 1.0 * rem_px
        + desc_lines as f32 * body_line_height;
    let content = if review_changes_visible {
        left_column.max(metrics::SUMMARY_BUTTON_HEIGHT)
    } else {
        left_column
    };
    3.0 * rem_px + content
}

/// Tool group 的稳定 presentation key：首个 tool event id。历史 replay 与
/// live reducer 使用同一 event id，因此折叠偏好不会依赖行下标。
pub(super) fn tool_group_key<'a>(
    entry_indices: &[usize],
    timeline: &'a [TimelineEntry],
) -> Option<&'a str> {
    entry_indices
        .first()
        .and_then(|entry_index| timeline.get(*entry_index))
        .map(|entry| entry.event_id.as_str())
}

fn tool_group_is_collapsed(
    entry_indices: &[usize],
    timeline: &[TimelineEntry],
    expanded_timeline_details: &HashSet<String>,
) -> bool {
    tool_group_key(entry_indices, timeline)
        .is_some_and(|key| !expanded_timeline_details.contains(key))
}

pub(super) fn tool_entry_height(entry: &TimelineEntry, width: f32, rem: f32) -> f32 {
    if let TimelineEntryKind::ToolCall {
        name,
        status,
        detail,
        arguments,
    } = &entry.kind
    {
        tool_row_height(
            &ToolRowView::from_facts(name, status, arguments.as_deref(), detail.as_deref()),
            width,
            rem,
        )
    } else {
        0.0
    }
}

pub(super) fn review_available_for_run(
    timeline: &[TimelineEntry],
    entry: &TimelineEntry,
    available: bool,
) -> bool {
    available
        && entry.fork_boundary == Some(ForkBoundary::Completed)
        && entry.run_id.is_some()
        && entry.run_id.as_deref()
            == timeline
                .iter()
                .rev()
                .find_map(|entry| entry.run_id.as_deref())
}

pub(super) fn run_summary_card_visible(entry: &TimelineEntry, review: bool) -> bool {
    entry.fork_boundary == Some(ForkBoundary::Failed) || review
}

/// Timeline 单行内容高度公式（render 组装同源；AX 行 rect 共用）。文本
/// 行数按 estimated_wrapped_lines 公式化估算，固定槽位走 theme metrics；
/// ToolGroup 行走 border-box（分隔线不增高）。
pub(super) fn timeline_row_height(
    row: &TimelineRow,
    timeline: &[TimelineEntry],
    column_width: f32,
    rem_px: f32,
    expanded_timeline_details: &HashSet<String>,
    review_changes_available: bool,
) -> f32 {
    match row {
        TimelineRow::Thinking { entry_index } => {
            let entry = &timeline[*entry_index];
            let TimelineEntryKind::Thinking { text } = &entry.kind else {
                return 0.0;
            };
            metrics::TOOL_GROUP_HEADER_HEIGHT
                + if expanded_timeline_details.contains(&entry.event_id) {
                    thinking_body_height(text, column_width, rem_px)
                } else {
                    0.0
                }
        }
        TimelineRow::Message { entry_index } | TimelineRow::Error { entry_index } => {
            let text = match &timeline[*entry_index].kind {
                TimelineEntryKind::UserMessage { text }
                | TimelineEntryKind::AssistantMessage { text }
                | TimelineEntryKind::Error(text) => text,
                _ => "",
            };
            message_entry_height(
                text,
                column_width,
                rem_px,
                matches!(
                    timeline[*entry_index].kind,
                    TimelineEntryKind::UserMessage { .. }
                ),
            )
        }
        TimelineRow::RunPhase { entry_index } => {
            // 非终态中间相位保持单行（§4.5）；与 render 相同只认 RunState。
            let TimelineEntryKind::RunState(state) = &timeline[*entry_index].kind else {
                return 0.0;
            };
            let font_px = font::BODY_SM.0 * rem_px;
            default_text_line_height(font_px)
                * estimated_wrapped_lines(state, column_width, font_px).max(1) as f32
        }
        TimelineRow::ToolGroup { entry_indices } => {
            metrics::TOOL_GROUP_HEADER_HEIGHT
                + if tool_group_is_collapsed(entry_indices, timeline, expanded_timeline_details) {
                    0.0
                } else {
                    entry_indices
                        .iter()
                        .map(|&ix| tool_entry_height(&timeline[ix], column_width, rem_px))
                        .sum::<f32>()
                }
        }
        TimelineRow::RunSummary { group, terminal } => {
            let review_changes_available =
                review_available_for_run(timeline, &timeline[*terminal], review_changes_available);
            let mut height = 0.0;
            if let Some(group) = group {
                height += metrics::TOOL_GROUP_HEADER_HEIGHT;
                if !tool_group_is_collapsed(group, timeline, expanded_timeline_details) {
                    height += group
                        .iter()
                        .map(|&ix| tool_entry_height(&timeline[ix], column_width, rem_px))
                        .sum::<f32>();
                }
                height +=
                    if run_summary_card_visible(&timeline[*terminal], review_changes_available) {
                        metrics::SUMMARY_CARD_GAP
                    } else {
                        metrics::TIMELINE_FOOTER_GAP
                    };
            }
            let review_changes_visible = review_changes_available
                && timeline[*terminal].fork_boundary == Some(ForkBoundary::Completed);
            if run_summary_card_visible(&timeline[*terminal], review_changes_visible) {
                height += run_summary_card_height(
                    &timeline[*terminal],
                    column_width,
                    rem_px,
                    review_changes_visible,
                );
                height += metrics::TIMELINE_FOOTER_GAP;
            }
            // 页脚行（BODY_SM 标签恒高于「···」按钮）。
            height += default_text_line_height(font::BODY_SM.0 * rem_px);
            height
        }
    }
}

/// 公式化可见窗口：自 start item 起按（上间距, 内容高）堆叠，返回
/// （item 序号, 内容 top）。全局首项无上间距；其余项的上间距归属该项
///（与 render 的 `pt` 一致）。`offset_in_first_item` 对应 GPUI
/// `ListOffset::offset_in_item`，因此首项可只露出尾部；完全不可见的项不发布。
pub(super) fn timeline_visible_item_tops(
    layouts: &[(f32, f32)],
    content_top: f32,
    viewport_height: f32,
    start: usize,
    offset_in_first_item: f32,
) -> Vec<(usize, f32)> {
    let bottom = content_top + viewport_height;
    let mut tops = Vec::new();
    let mut item_top = content_top - offset_in_first_item.max(0.0);
    for (ix, (gap, height)) in layouts.iter().enumerate().skip(start) {
        let top = item_top + if ix > 0 { *gap } else { 0.0 };
        if top >= bottom {
            break;
        }
        if top + height > content_top {
            tops.push((ix, top));
        }
        item_top = top + height;
    }
    tops
}

/// 跟随态窗口：从末项向前累加（上间距 + 内容高），返回 GPUI Top list
/// 等价的（首项序号, 首项内偏移）。溢出的首项仍保留为部分可见；全部装得
/// 下时回到 `(0, 0)`，保持短内容从顶部开始。
pub(super) fn timeline_following_window(
    layouts: &[(f32, f32)],
    viewport_height: f32,
) -> (usize, f32) {
    let mut accumulated = 0.0;
    for ix in (0..layouts.len()).rev() {
        let (gap, height) = layouts[ix];
        let outer = height + if ix > 0 { gap } else { 0.0 };
        accumulated += outer;
        if accumulated >= viewport_height {
            return (ix, (accumulated - viewport_height).max(0.0));
        }
    }
    (0, 0.0)
}

impl AppView {
    pub(super) fn timeline_area(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let rows = self.projection.timeline_rows();
        sync_list(self, rows.len());
        let empty_hint_visible = self.projection.workspace_empty_hint_visible();
        let fork_available = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) && self.projection.active_session_id.is_some();
        // 条目交互经 cx.processor 捕获实体：list() 的 render_item 只收 index，
        // 无 ViewContext，Entity::update 模式在条内重建 listener。
        let entries = list(
            self.timeline_list.clone(),
            cx.processor(
                move |view: &mut AppView,
                      ix: usize,
                      _window: &mut Window,
                      cx: &mut Context<AppView>| {
                    let len = rows.len();
                    if ix < len {
                        let element = view.timeline_row_element(&rows[ix], fork_available, cx);
                        let gap = row_top_gap(&rows[ix]);
                        if ix > 0 {
                            div()
                                .pt(px(gap))
                                .w_full()
                                .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
                                .min_w_0()
                                .child(element)
                                .into_any_element()
                        } else {
                            div()
                                .w_full()
                                .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
                                .min_w_0()
                                .child(element)
                                .into_any_element()
                        }
                    } else {
                        let card = view.approval_card_element(cx);
                        if len > 0 {
                            div()
                                .pt(px(metrics::MSG_ENTRY_GAP))
                                .w_full()
                                .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
                                .min_w_0()
                                .child(card)
                                .into_any_element()
                        } else {
                            div()
                                .w_full()
                                .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
                                .min_w_0()
                                .child(card)
                                .into_any_element()
                        }
                    }
                },
            ),
        )
        .flex_1()
        .w_full()
        .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
        .pt(px(metrics::TIMELINE_TOP_GAP))
        .pb(px(metrics::MSG_ENTRY_GAP));
        // P0-3 空态：无 active session 且条目数为 0 时只给出一个清楚的
        // Primary New task 路径；Disconnected 保留旧条目时不进入本分支。
        let offline = !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let content = if empty_hint_visible && offline {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .justify_center()
                .px_4()
                .child(self.connection_notice_element(cx))
                .into_any_element()
        } else if empty_hint_visible {
            let can_create = self.can_create_task();
            let tooltip = SharedString::from(if can_create {
                t("timeline.new_task_tooltip").to_string()
            } else {
                self.add_task_disabled_reason()
            });
            let new_task = Button::new("header-new-task")
                .variant(ButtonVariant::Primary)
                .track_focus(&self.header_new_task_focus)
                .height(px(36.0))
                .padding(ButtonPadding::Horizontal(metrics::SPACE_4))
                .center()
                .label(t("timeline.new_task"))
                .tooltip(tooltip)
                .disabled(!can_create)
                .on_click(cx.listener(|view, event, window, cx| {
                    if view.consume_button_key_click("header-new-task", event) {
                        return;
                    }
                    view.on_new_session(window, cx);
                }))
                .on_activate(cx.listener(|view, _event, window, cx| {
                    if view.open_menu.is_some() {
                        view.note_button_key_activate("header-new-task");
                        return;
                    }
                    view.note_button_key_activate("header-new-task");
                    view.on_new_session(window, cx);
                    cx.stop_propagation();
                }));
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(metrics::SPACE_2))
                .child(Label::new(workspace_empty_title()).size(font::TITLE))
                .child(
                    Label::new(workspace_empty_hint())
                        .size(font::BODY)
                        .color(dark().text.secondary),
                )
                .child(div().mt(px(metrics::SPACE_2)).child(new_task))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .pl(px(metrics::TIMELINE_CONTENT_INSET))
                .pr(px(metrics::TIMELINE_CONTENT_INSET))
                .child(entries)
                .into_any_element()
        };
        // 脱钩时右下浮出回底控件（§8.3）；跟随态隐藏。
        let following = self.timeline_following;
        let back_to_bottom_focus = self.timeline_back_to_bottom_focus.clone();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .when(offline && !empty_hint_visible, |area| {
                area.child(self.connection_notice_element(cx))
            })
            .child(content)
            .when(!following, |area| {
                area.child(BackToBottom::new(
                    Button::new("timeline-back-to-bottom")
                        .variant(ButtonVariant::Raised)
                        .label(t("timeline.back_to_bottom"))
                        .track_focus(&back_to_bottom_focus)
                        .on_click(cx.listener(|view, event, _window, cx| {
                            if view.consume_button_key_click("timeline-back-to-bottom", event) {
                                return;
                            }
                            view.timeline_jump_to_bottom();
                            cx.notify();
                        }))
                        .on_activate(cx.listener(|view, _event, _window, cx| {
                            view.note_button_key_activate("timeline-back-to-bottom");
                            view.timeline_jump_to_bottom();
                            cx.notify();
                            cx.stop_propagation();
                        })),
                ))
            })
    }

    /// 单个渲染行（消息 / 错误 / tool 组 / 中间相位 / Run 摘要区域）。
    /// 消息与错误条目的视觉与 fork 菜单由 timeline_entry builders 承载；
    /// 摘要区域的终态条目保留「···」fork 菜单（既有行为 / identifier）。
    fn timeline_row_element(
        &mut self,
        row: &TimelineRow,
        fork_available: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match row {
            TimelineRow::Thinking { entry_index } => {
                let entry = self.projection.timeline[*entry_index].clone();
                let TimelineEntryKind::Thinking { text } = &entry.kind else {
                    return div().into_any_element();
                };
                self.thinking_entry_element(&entry.event_id, text, cx)
                    .into_any_element()
            }
            TimelineRow::Message { entry_index } | TimelineRow::Error { entry_index } => {
                let entry = self.projection.timeline[*entry_index].clone();
                let menu_open = self.entry_menu_open(&entry);
                let can_fork = self.can_fork_entry(&entry.event_id);
                let element = match &entry.kind {
                    TimelineEntryKind::Error(_) => {
                        self.error_entry_element(&entry, menu_open, can_fork, cx)
                    }
                    _ => self.message_entry_element(&entry, menu_open, can_fork, cx),
                };
                element.into_any_element()
            }
            TimelineRow::RunPhase { entry_index } => {
                let entry = &self.projection.timeline[*entry_index];
                let TimelineEntryKind::RunState(state) = &entry.kind else {
                    return div().into_any_element();
                };
                // 非终态中间相位保持单行（§4.5），作为可读的次级状态，
                // 不与正文争抢主层级，也不纳入摘要。
                div()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(state.clone())
                    .into_any_element()
            }
            TimelineRow::ToolGroup { entry_indices } => {
                let rows = self.tool_row_views(entry_indices);
                let Some(group_key) =
                    tool_group_key(entry_indices, &self.projection.timeline).map(str::to_string)
                else {
                    return div().into_any_element();
                };
                self.tool_group_element(&group_key, &rows, cx)
                    .into_any_element()
            }
            TimelineRow::RunSummary { group, terminal } => {
                let entry = self.projection.timeline[*terminal].clone();
                let summary = self.run_summary_view(&entry);
                let mut region = div().flex().flex_col();
                if let Some(entry_indices) = group {
                    let rows = self.tool_row_views(entry_indices);
                    if let Some(group_key) =
                        tool_group_key(entry_indices, &self.projection.timeline).map(str::to_string)
                    {
                        region = region.child(self.tool_group_element(&group_key, &rows, cx));
                    }
                }
                let show_card = run_summary_card_visible(&entry, summary.review_changes_enabled);
                if show_card {
                    region = region.child(
                        div()
                            .when(group.is_some(), |item| {
                                item.mt(px(metrics::SUMMARY_CARD_GAP))
                            })
                            .child(self.run_summary_element(&summary, &entry.event_id, cx)),
                    );
                }
                let footer_label = format!(
                    "{} · {}",
                    if show_card {
                        ""
                    } else {
                        run_footer_label(&entry).unwrap_or("Run")
                    },
                    self.projection.run_usage_label(entry.run_id.as_deref())
                )
                .trim_start_matches(" · ")
                .to_string();
                let footer_time = display_time(&entry.timestamp, now_unix_ms());
                let footer = self.run_footer_element(&footer_label, &footer_time);
                let menu = self.entry_menu_dropdown(&entry, fork_available, cx);
                region
                    .child(
                        div()
                            .when(show_card || group.is_some(), |footer| {
                                footer.mt(px(metrics::TIMELINE_FOOTER_GAP))
                            })
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(footer))
                            .child(menu),
                    )
                    .into_any_element()
            }
        }
    }

    fn entry_menu_open(&self, entry: &TimelineEntry) -> bool {
        matches!(&self.open_menu, Some(MenuKind::Entry(open_id)) if open_id == &entry.event_id)
    }

    /// 组装 ToolRowView（含状态词映射）；索引由 timeline_rows 保证指向
    /// ToolCall 条目。
    pub(super) fn tool_row_views(&self, entry_indices: &[usize]) -> Vec<ToolRowView> {
        entry_indices
            .iter()
            .map(|&ix| {
                let entry = &self.projection.timeline[ix];
                let TimelineEntryKind::ToolCall {
                    name,
                    status,
                    detail,
                    arguments,
                } = &entry.kind
                else {
                    return ToolRowView::from_parts("tool", "", None);
                };
                ToolRowView::from_facts(name, status, arguments.as_deref(), detail.as_deref())
            })
            .collect()
    }

    pub(super) fn toggle_timeline_detail(&mut self, group_key: &str, cx: &mut Context<Self>) {
        if !self.expanded_timeline_details.remove(group_key) {
            self.expanded_timeline_details.insert(group_key.to_string());
        }
        // 用户展开详情时保留当前视口，不因旧贴底状态把标题推到屏幕外。
        self.timeline_following = false;
        self.timeline_changed();
        cx.notify();
    }

    /// RunSummaryView 数据（诚实口径：仅完成态 + active session
    /// 非空 Changes 显示 Review changes，其余完成态使用轻量摘要）。
    fn run_summary_view(&self, entry: &TimelineEntry) -> RunSummaryView {
        let completed = entry.fork_boundary == Some(ForkBoundary::Completed);
        let review_changes_enabled = completed
            && review_available_for_run(
                &self.projection.timeline,
                entry,
                self.changes_available_for_active(),
            );
        let (title, description) = run_summary_texts(entry, review_changes_enabled)
            .unwrap_or(("Run", "The run reached a terminal state.".to_string()));
        RunSummaryView {
            title: title.into(),
            description,
            terminal: match entry.fork_boundary {
                Some(ForkBoundary::Failed) => RunSummaryTerminal::Failed,
                Some(ForkBoundary::Cancelled) => RunSummaryTerminal::Cancelled,
                _ => RunSummaryTerminal::Completed,
            },
            review_changes_enabled,
        }
    }

    /// 终态条目的「···」fork 菜单（与消息条目同构；identifier 冻结）。
    fn entry_menu_dropdown(
        &mut self,
        entry: &TimelineEntry,
        fork_available: bool,
        cx: &mut Context<Self>,
    ) -> Dropdown {
        super::timeline_entry::entry_actions_element(
            self,
            cx,
            entry,
            self.entry_menu_open(entry),
            fork_available && self.can_fork_entry(&entry.event_id),
        )
    }

    /// 回底并重挂跟随：scroll_to 越界钳制到末项底。
    pub(super) fn timeline_jump_to_bottom(&mut self) {
        self.timeline_list.scroll_to(ListOffset {
            item_ix: self.timeline_list.item_count(),
            offset_in_item: Pixels::ZERO,
        });
        self.timeline_following = true;
    }
}
