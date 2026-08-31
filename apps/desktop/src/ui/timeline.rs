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

use gpui::{
    div, list, prelude::*, px, AnyElement, Context, ListOffset, ListState, Pixels, SharedString,
    WeakEntity, Window,
};

use crate::projection::{
    run_footer_label, run_summary_texts, ConnectionState, ForkBoundary, TimelineEntry,
    TimelineEntryKind, TimelineRow,
};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::label::Label;
use crate::ui::theme::{dark, font, metrics};

use super::timeline_entry::{display_time, RunSummaryTerminal, RunSummaryView, ToolRowView};
use super::{now_unix_ms, AppView, MenuKind, WORKSPACE_EMPTY_HINT};

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

/// ToolCall 状态词映射（§4.4）：succeeded → "Completed" 允许，其余 wire
/// 原文；未知状态原样显示不伪造。render 与 AX 共用。
pub(super) fn tool_status_label(status: &str) -> String {
    if status == "succeeded" {
        "Completed".into()
    } else {
        status.to_string()
    }
}

/// 行与前一行的垂直间距（§4.2/§4.3 量图冻结值）：消息 / 错误 / 中间相位
/// 40；独立 tool 组 48；摘要区域带组时 48（组面板即区域首元素），无组按
/// 普通 entry 保持 40。12 只用于 tool panel → summary 的组内间距。
fn row_top_gap(row: &TimelineRow) -> f32 {
    match row {
        TimelineRow::Message { .. } | TimelineRow::Error { .. } | TimelineRow::RunPhase { .. } => {
            metrics::MSG_ENTRY_GAP
        }
        TimelineRow::ToolGroup { .. } => metrics::TOOL_GROUP_TOP_GAP,
        TimelineRow::RunSummary { group, .. } => {
            if group.is_some() {
                metrics::TOOL_GROUP_TOP_GAP
            } else {
                metrics::MSG_ENTRY_GAP
            }
        }
    }
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
                                .mt(px(gap))
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
                                .mt(px(metrics::MSG_ENTRY_GAP))
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
        .pl(px(metrics::TIMELINE_CONTENT_INSET))
        .pt(px(metrics::TIMELINE_TOP_GAP));
        // 空态引导（gui-design 空态原则）：无 active session 且条目数为 0 时
        // 居中一句 tertiary 提示；Disconnected 保留旧条目时不进入本分支。
        let content = if empty_hint_visible {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Label::new(WORKSPACE_EMPTY_HINT)
                        .size(font::BASE)
                        .color(dark().text.tertiary),
                )
                .into_any_element()
        } else {
            entries.into_any_element()
        };
        // 脱钩时右下浮出回底控件（§8.3）；跟随态隐藏。
        let following = self.timeline_following;
        let back_to_bottom_focus = self.timeline_back_to_bottom_focus.clone();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .child(content)
            .when(!following, |area| {
                area.child(BackToBottom::new(
                    Button::new("timeline-back-to-bottom")
                        .variant(ButtonVariant::Raised)
                        .label("↓ 回到底部")
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
            TimelineRow::Message { entry_index } | TimelineRow::Error { entry_index } => {
                let entry = self.projection.timeline[*entry_index].clone();
                let menu_open = self.entry_menu_open(&entry);
                let can_fork = fork_available && entry.is_fork_boundary();
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
                self.tool_group_element(&rows).into_any_element()
            }
            TimelineRow::RunSummary { group, terminal } => {
                let entry = self.projection.timeline[*terminal].clone();
                let summary = self.run_summary_view(&entry);
                let mut region = div().flex().flex_col();
                if let Some(entry_indices) = group {
                    let rows = self.tool_row_views(entry_indices);
                    region = region.child(self.tool_group_element(&rows)).child(
                        div()
                            .mt(px(metrics::SUMMARY_CARD_GAP))
                            .child(self.run_summary_element(&summary, &entry.event_id, cx)),
                    );
                } else {
                    region = region.child(self.run_summary_element(&summary, &entry.event_id, cx));
                }
                let footer_label = run_footer_label(&entry).unwrap_or("Run");
                let footer_time = display_time(&entry.timestamp, now_unix_ms());
                let footer = self.run_footer_element(footer_label, &footer_time);
                let menu = self.entry_menu_dropdown(&entry, fork_available, cx);
                region
                    .child(
                        div()
                            .mt(px(metrics::TIMELINE_FOOTER_GAP))
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
    fn tool_row_views(&self, entry_indices: &[usize]) -> Vec<ToolRowView> {
        entry_indices
            .iter()
            .map(|&ix| {
                let entry = &self.projection.timeline[ix];
                let TimelineEntryKind::ToolCall {
                    name,
                    status,
                    detail,
                } = &entry.kind
                else {
                    return ToolRowView::from_parts("tool", "", None);
                };
                ToolRowView::from_parts(name, status, detail.as_deref())
            })
            .collect()
    }

    /// RunSummaryView 数据（诚实口径：完成态才有 Review changes 动作；
    /// Changes 数据不可用时禁用并给原因）。
    fn run_summary_view(&self, entry: &TimelineEntry) -> RunSummaryView {
        let (title, description) = run_summary_texts(entry)
            .unwrap_or(("Run", "The run reached a terminal state.".to_string()));
        let completed = entry.fork_boundary == Some(ForkBoundary::Completed);
        let review_changes_enabled = completed && self.changes_available_for_active();
        let review_changes_disabled_reason = if review_changes_enabled {
            None
        } else if !completed {
            Some("Review is available after a completed run.".to_string())
        } else {
            Some("Changes data is not available yet.".to_string())
        };
        RunSummaryView {
            title: title.into(),
            description,
            terminal: match entry.fork_boundary {
                Some(ForkBoundary::Failed) => RunSummaryTerminal::Failed,
                Some(ForkBoundary::Cancelled) => RunSummaryTerminal::Cancelled,
                _ => RunSummaryTerminal::Completed,
            },
            review_changes_enabled,
            review_changes_disabled_reason,
        }
    }

    /// 终态条目的「···」fork 菜单（与消息条目同构；identifier 冻结）。
    fn entry_menu_dropdown(
        &mut self,
        entry: &TimelineEntry,
        fork_available: bool,
        cx: &mut Context<Self>,
    ) -> Dropdown {
        let menu_open = self.entry_menu_open(entry);
        let event_id = entry.event_id.clone();
        let button_id = format!("entry-menu-{}", entry.event_id);
        let entry_focus = self.timeline_entry_focus(&event_id, cx);
        let actions_button = Button::new(button_id.clone())
            .variant(ButtonVariant::Ghost)
            .text_size(font::XS)
            .text_color(dark().text.secondary)
            .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
            .label("···")
            .track_focus(&entry_focus)
            .on_click(cx.listener({
                let event_id = event_id.clone();
                let button_id = button_id.clone();
                move |view, event, _window, cx| {
                    if view.consume_button_key_click(&button_id, event) {
                        return;
                    }
                    let down = Self::click_down_position(event);
                    view.toggle_menu(MenuKind::Entry(event_id.clone()), down, cx);
                }
            }))
            .on_activate(cx.listener({
                let event_id = event_id.clone();
                let button_id = button_id.clone();
                move |view, _event, _window, cx| {
                    // 菜单已开时让位给根节点的 Entry 菜单选择；同时吞掉
                    // 同键 keyup 合成 click，避免选 Fork 后重开浮层。
                    if view.open_menu.is_some() {
                        view.note_button_key_activate(&button_id);
                        return;
                    }
                    view.open_entry_menu_from_keyboard(&event_id, cx);
                    cx.stop_propagation();
                }
            }));
        let mut actions = Dropdown::new(actions_button);
        if menu_open {
            let can_fork = fork_available && entry.is_fork_boundary();
            let fork_id = event_id.clone();
            actions = actions.panel(
                MenuPanel::new(SharedString::from(format!("fork-menu-{}", entry.event_id)))
                    .dismiss_on_outside(cx.listener({
                        let kind = MenuKind::Entry(event_id.clone());
                        move |view, event: &gpui::MouseDownEvent, _, cx| {
                            view.dismiss_menu_on_outside(kind.clone(), event.position, cx)
                        }
                    }))
                    .child(
                        MenuRow::new(SharedString::from(format!("fork-{}", entry.event_id)))
                            .label("Fork")
                            .disabled(!can_fork)
                            .when(can_fork, |row| {
                                row.on_click(cx.listener(move |view, _event, window, cx| {
                                    view.close_open_menu(cx);
                                    view.on_fork(&fork_id, window, cx);
                                }))
                            }),
                    ),
            );
        }
        actions
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
