//! Timeline 条目渲染（R4 Wave A：F-07 消息层级 / F-08 tool group 与 Run 摘要）。
//!
//! 消息条目升级为「标签行（角色 + 时间）+ 正文」层级：正文 18px / 行高 24，
//! 空行分段（段落间隙 28），段内“- ”前缀连续行渲染为 • 列表（两级切分，
//! 不引入 markdown 引擎）。连续 ToolCall 条目由 timeline.rs 组装为 tool
//! group 面板（本文件只负责行渲染）；Run 终态摘要卡与 Timeline 页脚按
//! state-a §2.3 量图几何渲染。颜色 / 字阶 / 几何一律走 theme token 与本波
//! 冻结 metrics；wire 无 tool 耗时与 run 终态时长字段，对应列一律不画
//! （诚实显示，不伪造）。
//!
//! # 对外 contract（Worker B / timeline.rs 直接调用）
//!
//! - pub(super) fn message_entry_element(&self, entry: &TimelineEntry, menu_open: bool, can_fork: bool, cx: &mut Context<Self>) -> gpui::Div
//! - pub(super) struct ToolRowView { pub name: String, pub status_label: String, pub status: ToolRowStatus, pub detail: Option<String> }
//! - pub(super) enum ToolRowStatus { Pending, Running, Succeeded, Failed, Cancelled, Other }
//! - pub(super) fn tool_group_element(&self, rows: &[ToolRowView]) -> gpui::Div
//! - pub(super) struct RunSummaryView { pub title: String, pub description: String, pub review_changes_enabled: bool, pub review_changes_disabled_reason: Option<String> }
//! - pub(super) fn run_summary_element(&self, view: &RunSummaryView, cx: &mut Context<Self>) -> gpui::Div（内部经 cx.listener 调 AppView::on_review_changes，mod.rs 实现）
//! - pub(super) fn run_footer_element(&self, label: &str, time: &str) -> gpui::Div
//! - pub(super) fn error_entry_element(&self, entry: &TimelineEntry, menu_open: bool, can_fork: bool, cx: &mut Context<Self>) -> gpui::Div
//! - pub(super) fn display_time(timestamp: &str, now_ms: u64) -> String（epoch 串→相对时间词，render/AX 同源；解析失败原样兜底）
//!
//! 构造辅助：ToolRowView::from_parts(name, status, detail) 把 wire 原文
//! status 归类为 ToolRowStatus 并映射状态词（succeeded → “Completed”，其余
//! 原文，未知状态原样显示不伪造）。条目「···」fork 菜单（identifier 与
//! 行为冻结）迁入 message / error 条目；旧 timeline_entry_element 删除。

use gpui::{div, prelude::*, px, Context, FontWeight, Rgba, SharedString};

use crate::projection::{ConnectionState, TimelineEntry, TimelineEntryKind};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::label::Label;
use crate::ui::theme::{dark, font, metrics};

use super::task_rail::relative_activity;
use super::timeline::tool_status_label;
use super::{AppView, MenuKind, now_unix_ms};

/// 显示时间（R4 Wave A P3）：epoch 毫秒串经 task_rail::relative_activity 转
/// 相对时间词（now / Nm / Nh / Nd）；解析失败（如 fixture 任意串）原样返回，
/// 诚实兜底不伪造。render 三处与 AX 三处共用本函数（同源）。
pub(super) fn display_time(timestamp: &str, now_ms: u64) -> String {
    match timestamp.parse::<u64>() {
        Ok(updated_at_ms) => relative_activity(updated_at_ms, now_ms),
        Err(_) => timestamp.to_string(),
    }
}

/// Tool 行渲染态：wire status 归类 + 展示词（构造见 ToolRowView::from_parts）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ToolRowStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Other,
}

/// Tool activity 面板单行视图（timeline.rs 组装连续 ToolCall 后传入）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ToolRowView {
    pub name: String,
    pub status_label: String,
    pub status: ToolRowStatus,
    pub detail: Option<String>,
}

impl ToolRowView {
    /// wire 原文字段 → 渲染视图。detail 空串归一为 None（旧渲染同语义）。
    pub(super) fn from_parts(name: &str, status: &str, detail: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            status_label: tool_status_label(status),
            status: tool_row_status(status),
            detail: detail
                .map(str::to_string)
                .filter(|detail| !detail.is_empty()),
        }
    }
}

/// Run 终态摘要卡视图（终态判定与数据归 Worker B 组装层）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RunSummaryView {
    pub title: String,
    pub description: String,
    /// 终态种类驱动状态圆视觉（Completed ✓ / Failed ✕ / Cancelled —），
    /// 禁止恒绿 ✓ 对失败/取消宣称成功（审查 P2）。
    pub terminal: RunSummaryTerminal,
    pub review_changes_enabled: bool,
    pub review_changes_disabled_reason: Option<String>,
}

/// Run 摘要卡终态种类（与 projection ForkBoundary 一一对应，展示层枚举）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunSummaryTerminal {
    Completed,
    Cancelled,
    Failed,
}

/// wire status 词 → 渲染态分类（未知词归 Other，原样显示不伪造）。
fn tool_row_status(status: &str) -> ToolRowStatus {
    match status {
        "succeeded" => ToolRowStatus::Succeeded,
        "failed" => ToolRowStatus::Failed,
        "cancelled" => ToolRowStatus::Cancelled,
        "running" => ToolRowStatus::Running,
        "pending" => ToolRowStatus::Pending,
        _ => ToolRowStatus::Other,
    }
}

/// 消息正文块（F-07 两级切分）：段内普通行 / “- ”前缀连续列表项。
#[derive(Clone, Debug, PartialEq, Eq)]
enum MessageBlock {
    Paragraph(Vec<String>),
    List(Vec<String>),
}

/// 空行分段（两个换行）；段内按“- ”前缀把连续行切成段落 / 列表交替块。
fn split_message_blocks(text: &str) -> Vec<MessageBlock> {
    fn push_paragraph(buffer: &mut Vec<String>, blocks: &mut Vec<MessageBlock>) {
        if !buffer.is_empty() {
            blocks.push(MessageBlock::Paragraph(std::mem::take(buffer)));
        }
    }
    fn push_list(buffer: &mut Vec<String>, blocks: &mut Vec<MessageBlock>) {
        if !buffer.is_empty() {
            blocks.push(MessageBlock::List(std::mem::take(buffer)));
        }
    }

    let mut blocks = Vec::new();
    for chunk in text.split("\n\n") {
        let mut paragraph: Vec<String> = Vec::new();
        let mut list: Vec<String> = Vec::new();
        for line in chunk.lines() {
            if let Some(item) = line.strip_prefix("- ") {
                push_paragraph(&mut paragraph, &mut blocks);
                list.push(item.to_string());
            } else {
                push_list(&mut list, &mut blocks);
                paragraph.push(line.to_string());
            }
        }
        push_paragraph(&mut paragraph, &mut blocks);
        push_list(&mut list, &mut blocks);
    }
    blocks
}

/// 标签行：角色（18px medium）+ 时间（17px tertiary，display_time 相对词）。
fn message_label_element(role: &str, time: &str, role_color: Rgba) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_baseline()
        .gap_3()
        .child(
            div()
                .text_size(px(font::BODY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(role_color)
                .child(role.to_string()),
        )
        .child(
            div()
                .text_size(px(font::BODY_SM))
                .text_color(dark().text.tertiary)
                .truncate()
                .child(time.to_string()),
        )
}

/// 正文：段落 / 列表块渲染（行高 24，块间 28；列表项 • 前缀）。
fn message_body_element(text: &str, color: Rgba) -> gpui::Div {
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(metrics::MSG_PARAGRAPH_GAP))
        .text_size(px(font::BODY))
        .line_height(px(metrics::MSG_LINE_HEIGHT))
        .text_color(color);
    for block in split_message_blocks(text) {
        let mut block_element = div().flex().flex_col();
        match block {
            MessageBlock::Paragraph(lines) => {
                for line in lines {
                    block_element = block_element.child(div().child(line));
                }
            }
            MessageBlock::List(items) => {
                for item in items {
                    block_element = block_element.child(div().child(format!("• {item}")));
                }
            }
        }
        body = body.child(block_element);
    }
    body
}

/// 条目「···」fork 菜单（identifier 与行为自旧 timeline_entry_element 冻结迁移）。
fn entry_actions_element(
    cx: &mut Context<AppView>,
    entry: &TimelineEntry,
    menu_open: bool,
    can_fork: bool,
) -> Dropdown {
    let event_id = entry.event_id.clone();
    let actions_button = Button::new(format!("entry-menu-{}", entry.event_id))
        .variant(ButtonVariant::Ghost)
        .text_size(font::XS)
        .text_color(dark().text.secondary)
        .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
        .label("···")
        .on_click(cx.listener({
            let event_id = event_id.clone();
            move |view, event, _window, cx| {
                let down = AppView::click_down_position(event);
                view.toggle_menu(MenuKind::Entry(event_id.clone()), down, cx);
            }
        }));
    let mut actions = Dropdown::new(actions_button);
    if menu_open {
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
                            row.on_click(cx.listener(move |view, _event, _window, cx| {
                                view.close_open_menu(cx);
                                view.on_fork(&fork_id, cx);
                            }))
                        }),
                ),
        );
    }
    actions
}

/// 条目壳层：左列（标签行 + 正文，min_w_0 保证正文在可读列宽内 wrap）+
/// 右侧「···」菜单。行宽不超 TIMELINE_READABLE_WIDTH（防无限拉宽）。
fn entry_shell_element(
    cx: &mut Context<AppView>,
    entry: &TimelineEntry,
    menu_open: bool,
    can_fork: bool,
    label: gpui::Div,
    body: gpui::Div,
) -> gpui::Div {
    let actions = entry_actions_element(cx, entry, menu_open, can_fork);
    div()
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .gap_2()
        .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(metrics::MSG_LABEL_BODY_GAP))
                .child(label)
                .child(body),
        )
        .child(actions)
}

/// Tool 行图标槽（19px）：无既有 glyph 体系，按工具名首字母块呈现（禁 emoji）。
fn tool_icon_element(name: &str) -> gpui::Div {
    let glyph = name
        .chars()
        .next()
        .map(|first| first.to_uppercase().to_string())
        .unwrap_or_default();
    div()
        .w(px(metrics::TOOL_ICON_SIZE))
        .h(px(metrics::TOOL_ICON_SIZE))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .text_size(px(font::BASE))
        .text_color(dark().text.secondary)
        .child(glyph)
}

/// Tool 行状态槽：succeeded = ✓（Ø14，success_fg）+ 状态词；
/// running / pending = accent 点静态表示（本波不做动画）；failed /
/// cancelled = danger_text 状态词；未知状态原样 tertiary。不画耗时列
/// （wire 无 tool duration 字段）。
fn tool_status_element(row: &ToolRowView) -> gpui::Div {
    let word_color = match row.status {
        ToolRowStatus::Succeeded => dark().text.emphasis,
        ToolRowStatus::Failed | ToolRowStatus::Cancelled => dark().semantic.danger_text,
        ToolRowStatus::Running | ToolRowStatus::Pending | ToolRowStatus::Other => {
            dark().text.tertiary
        }
    };
    let word = div()
        .text_size(px(font::BODY_SM))
        .text_color(word_color)
        .child(row.status_label.clone());
    match row.status {
        ToolRowStatus::Succeeded => div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(metrics::TOOL_CHECK_SIZE))
                    .text_color(dark().semantic.success_fg)
                    .child("✓"),
            )
            .child(word),
        ToolRowStatus::Running | ToolRowStatus::Pending => div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(metrics::TOOL_CHECK_SIZE))
                    .h(px(metrics::TOOL_CHECK_SIZE))
                    .flex_none()
                    .rounded_full()
                    .bg(dark().accent.primary),
            )
            .child(word),
        ToolRowStatus::Failed | ToolRowStatus::Cancelled | ToolRowStatus::Other => {
            div().flex().flex_none().items_center().child(word)
        }
    }
}

/// Tool 面板单行：图标槽 + 名称（truncate）+ detail（单行 truncate）+ 状态。
fn tool_row_element(row: &ToolRowView) -> gpui::Div {
    let mut middle = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .flex_1()
        .min_w_0();
    middle = middle.child(
        div()
            .min_w_0()
            .truncate()
            .text_size(px(font::BODY))
            .text_color(dark().text.emphasis)
            .child(row.name.clone()),
    );
    if let Some(detail) = row.detail.as_deref() {
        middle = middle.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(font::BODY_SM))
                .text_color(dark().text.tertiary)
                .child(detail.to_string()),
        );
    }
    div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(metrics::TOOL_ROW_HEIGHT))
        .pl(px(metrics::TOOL_GROUP_INNER_INSET))
        .pr_3()
        .gap_3()
        .child(tool_icon_element(&row.name))
        .child(middle)
        .child(tool_status_element(row))
}

impl AppView {
    /// F-07 消息条目：标签行（You / Pawork + 时间）+ 正文（段落 / 列表两级）。
    pub(super) fn message_entry_element(
        &self,
        entry: &TimelineEntry,
        menu_open: bool,
        can_fork: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let time = display_time(&entry.timestamp, now_unix_ms());
        let (role, label_color, body) = match &entry.kind {
            TimelineEntryKind::UserMessage { text } => (
                "You",
                dark().text.emphasis,
                message_body_element(text, dark().text.emphasis),
            ),
            TimelineEntryKind::AssistantMessage { text } => (
                "Pawork",
                dark().text.emphasis,
                message_body_element(text, dark().text.emphasis),
            ),
            // 兜底臂（Worker B 组装层不会把 tool / run 态交给消息条目）：
            // 保持旧单行语义，避免意外调用时崩溃。
            TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
            } => (
                "Tool",
                dark().text.secondary,
                {
                    let mut element = div()
                        .py_1()
                        .text_color(dark().text.secondary)
                        .child(format!("{name} · {status}"));
                    if let Some(detail) = detail.as_deref().filter(|d| !d.is_empty()) {
                        element = element.child(
                            div()
                                .text_size(px(font::XS))
                                .text_color(dark().text.tertiary)
                                .child(detail.to_string()),
                        );
                    }
                    element
                },
            ),
            TimelineEntryKind::RunState(state) => (
                "Run",
                dark().text.disabled,
                div()
                    .py_1()
                    .text_color(dark().text.disabled)
                    .child(state.clone()),
            ),
            TimelineEntryKind::Error(message) => (
                "Error",
                dark().semantic.danger_text,
                message_body_element(message, dark().semantic.danger_text),
            ),
        };
        entry_shell_element(
            cx,
            entry,
            menu_open,
            can_fork,
            message_label_element(role, &time, label_color),
            body,
        )
    }

    /// F-08 Tool activity 面板：1px border.subtle / r5 / 无标题；行高 52，
    /// 行间 2px 分隔线；内左 inset 15。组默认展开（折叠交互属 Wave B）。
    pub(super) fn tool_group_element(&self, rows: &[ToolRowView]) -> gpui::Div {
        let mut panel = div()
            .flex()
            .flex_col()
            .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
            .border_1()
            .border_color(dark().border.subtle)
            .rounded(px(metrics::TOOL_GROUP_RADIUS));
        for (index, row) in rows.iter().enumerate() {
            let mut element = tool_row_element(row);
            if index > 0 {
                element = element
                    .border_t(px(metrics::TOOL_ROW_DIVIDER))
                    .border_color(dark().border.subtle);
            }
            panel = panel.child(element);
        }
        panel
    }

    /// F-08 Run 摘要卡：Ø40 success_fg 圆 + 深色 ✓ + 标题 + 说明（两行内）+
    /// 右侧主按钮 “Review changes”（168×40 r8，点击切 Inspector Changes；
    /// 数据不可用时 disabled 并给原因；“Open in editor” 无 Host capability
    /// 不画）。无权威数据时 description 为组装层给的一句通用完成说明。
    pub(super) fn run_summary_element(
        &self,
        view: &RunSummaryView,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut button = Button::new("run-summary-review-changes")
            .variant(ButtonVariant::Primary)
            .width(px(metrics::SUMMARY_BUTTON_WIDTH))
            .height(px(metrics::SUMMARY_BUTTON_HEIGHT))
            .center()
            .radius(metrics::SUMMARY_BUTTON_RADIUS)
            .text_size(font::BODY_SM)
            .label("Review changes")
            .disabled(!view.review_changes_enabled);
        if let Some(reason) = view
            .review_changes_disabled_reason
            .as_deref()
            .filter(|reason| !reason.is_empty())
        {
            button = button.tooltip(reason.to_string());
        }
        if view.review_changes_enabled {
            button = button.on_click(cx.listener(|view, _event, _window, cx| {
                view.on_review_changes(cx);
            }));
        }
        let (circle_bg, circle_fg, circle_glyph) = match view.terminal {
            RunSummaryTerminal::Completed => (
                dark().semantic.success_fg,
                dark().bg.base,
                "✓",
            ),
            RunSummaryTerminal::Failed => (
                dark().semantic.danger_bg,
                dark().text.on_accent,
                "✕",
            ),
            RunSummaryTerminal::Cancelled => (
                dark().surface.disabled,
                dark().text.tertiary,
                "—",
            ),
        };
        let check_circle = div()
            .w(px(metrics::SUMMARY_CHECK_CIRCLE))
            .h(px(metrics::SUMMARY_CHECK_CIRCLE))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(circle_bg)
            .text_size(px(font::BODY))
            .text_color(circle_fg)
            .child(circle_glyph);
        let left_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_4()
                    .child(check_circle)
                    .child(
                        div()
                            .text_size(px(font::BODY))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(dark().text.primary)
                            .child(view.title.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .child(div().w(px(metrics::SUMMARY_CHECK_CIRCLE)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(font::BODY_SM))
                            .line_height(px(metrics::MSG_LINE_HEIGHT))
                            .line_clamp(2)
                            .text_color(dark().text.secondary)
                            .child(view.description.clone()),
                    ),
            );
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_6()
            .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
            .border_1()
            .border_color(dark().border.subtle)
            .rounded(px(metrics::TOOL_GROUP_RADIUS))
            .pl(px(metrics::TOOL_GROUP_INNER_INSET))
            .pr_5()
            .py_6()
            .child(left_column)
            .child(button)
    }

    /// F-08 Timeline 页脚：终态词（左）+ 终态时间（右），17px tertiary。
    /// 无终态时长字段，不画 “· 2m 14s”（量图耗时属演示数据）。
    pub(super) fn run_footer_element(&self, label: &str, time: &str) -> gpui::Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
            .child(
                Label::new(label.to_string())
                    .size(font::BODY_SM)
                    .color(dark().text.tertiary),
            )
            .child(
                Label::new(time.to_string())
                    .size(font::BODY_SM)
                    .color(dark().text.tertiary),
            )
    }

    /// F-07/F-08 错误条目：danger_text 新条目层级（标签行 + 正文），
    /// 不加假 retry 按钮（retry 条件属 Wave B 场景）。
    pub(super) fn error_entry_element(
        &self,
        entry: &TimelineEntry,
        menu_open: bool,
        can_fork: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let message = match &entry.kind {
            TimelineEntryKind::Error(message) => message.clone(),
            _ => String::new(),
        };
        let time = display_time(&entry.timestamp, now_unix_ms());
        entry_shell_element(
            cx,
            entry,
            menu_open,
            can_fork,
            message_label_element("Error", &time, dark().semantic.danger_text),
            message_body_element(&message, dark().semantic.danger_text),
        )
    }

    pub(super) fn on_fork(&mut self, event_id: &str, cx: &mut Context<Self>) {
        let Some(session_id) = self.projection.active_session_id.clone() else {
            self.status_hint = Some("Open a session before forking.".into());
            cx.notify();
            return;
        };
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            self.status_hint = Some("Fork needs a live connection.".into());
            cx.notify();
            return;
        }
        // 入口级防线：渲染层已按边界禁用 Fork，这里再按 reducer 的单点判型
        // 复核——connected + active session + run 终止边界缺一不可。
        let forkable = self
            .projection
            .timeline
            .iter()
            .any(|entry| entry.event_id == event_id && entry.is_fork_boundary());
        if !forkable {
            self.status_hint = Some(
                "Fork is only available on a finished run (completed, cancelled, or failed)."
                    .into(),
            );
            cx.notify();
            return;
        }
        self.controller
            .fork_session(session_id, event_id.to_string());
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态词映射合同：succeeded → Completed；其余 wire 原文；未知词原样。
    #[test]
    fn tool_status_label_maps_succeeded_only() {
        assert_eq!(tool_status_label("succeeded"), "Completed");
        assert_eq!(tool_status_label("running"), "running");
        assert_eq!(tool_status_label("failed"), "failed");
        assert_eq!(tool_status_label("approve_once"), "approve_once");
    }

    /// wire status 词分类：五类已知词精确归类，未知词（含审批决策词）归 Other。
    #[test]
    fn tool_row_status_classifies_wire_words() {
        assert_eq!(tool_row_status("succeeded"), ToolRowStatus::Succeeded);
        assert_eq!(tool_row_status("failed"), ToolRowStatus::Failed);
        assert_eq!(tool_row_status("cancelled"), ToolRowStatus::Cancelled);
        assert_eq!(tool_row_status("running"), ToolRowStatus::Running);
        assert_eq!(tool_row_status("pending"), ToolRowStatus::Pending);
        assert_eq!(tool_row_status("approve_once"), ToolRowStatus::Other);
        assert_eq!(tool_row_status(""), ToolRowStatus::Other);
    }

    /// 视图构造：字段映射 + detail 空串归一 None（旧渲染同语义）。
    #[test]
    fn tool_row_view_from_parts_normalizes_detail() {
        let view = ToolRowView::from_parts("read_file", "succeeded", Some("src/main.rs"));
        assert_eq!(view.name, "read_file");
        assert_eq!(view.status_label, "Completed");
        assert_eq!(view.status, ToolRowStatus::Succeeded);
        assert_eq!(view.detail.as_deref(), Some("src/main.rs"));

        let view = ToolRowView::from_parts("bash", "running", Some(""));
        assert_eq!(view.status, ToolRowStatus::Running);
        assert_eq!(view.detail, None);

        let view = ToolRowView::from_parts("bash", "running", None);
        assert_eq!(view.detail, None);
    }

    /// 摘要卡视图构造：字段直存，禁用原因为独立通道。
    #[test]
    fn run_summary_view_construction() {
        let view = RunSummaryView {
            title: "Ready for review".into(),
            description: "Run finished.".into(),
            terminal: RunSummaryTerminal::Completed,
            review_changes_enabled: false,
            review_changes_disabled_reason: Some("Changes unavailable.".into()),
        };
        assert_eq!(view.title, "Ready for review");
        assert!(!view.review_changes_enabled);
        assert_eq!(
            view.review_changes_disabled_reason.as_deref(),
            Some("Changes unavailable."),
        );
    }

    /// 段落 / 列表切分：空行分段；“- ”前缀连续行为列表项（前缀剥离）；
    /// 混合段内按连续行交替成块。
    #[test]
    fn message_blocks_split_paragraphs_and_lists() {
        use MessageBlock::{List, Paragraph};

        assert_eq!(
            split_message_blocks("Refine the header.\n\nThen ship it."),
            vec![
                Paragraph(vec!["Refine the header.".into()]),
                Paragraph(vec!["Then ship it.".into()]),
            ]
        );
        assert_eq!(
            split_message_blocks("- first\n- second\n- third"),
            vec![List(vec![
                "first".into(),
                "second".into(),
                "third".into()
            ])]
        );
        assert_eq!(
            split_message_blocks("Plan:\n- a\n- b\nOutro"),
            vec![
                Paragraph(vec!["Plan:".into()]),
                List(vec!["a".into(), "b".into()]),
                Paragraph(vec!["Outro".into()]),
            ]
        );
    }

    /// 边界：空文本 / 纯空行不产生块；单换行保留为段内行；首尾空行剪除。
    #[test]
    fn message_blocks_handle_edges() {
        use MessageBlock::Paragraph;

        assert_eq!(split_message_blocks(""), Vec::new());
        assert_eq!(split_message_blocks("\n\n"), Vec::new());
        assert_eq!(
            split_message_blocks("a\n\n"),
            vec![Paragraph(vec!["a".into()])]
        );
        assert_eq!(
            split_message_blocks("one\ntwo"),
            vec![Paragraph(vec!["one".into(), "two".into()])]
        );
    }

    /// 相对时间词合同（与 task_rail::relative_activity 同源）：now / 1m /
    /// 5m / 3h / 2d 代表值 + 分钟级精确边界。
    #[test]
    fn display_time_maps_epoch_strings_to_relative_words() {
        let now_ms = 1_800_000_000_000u64;
        assert_eq!(display_time("1800000000000", now_ms), "now");
        assert_eq!(display_time("1799999999999", now_ms), "now");
        assert_eq!(display_time("1799999940000", now_ms), "1m");
        assert_eq!(display_time("1799999700000", now_ms), "5m");
        assert_eq!(display_time("1799989200000", now_ms), "3h");
        assert_eq!(display_time("1799827200000", now_ms), "2d");
    }

    /// 诚实兜底：非法串 / 空串原样返回，不伪造相对时间。
    #[test]
    fn display_time_falls_back_verbatim_on_invalid_input() {
        let now_ms = 1_800_000_000_000u64;
        assert_eq!(display_time("not-a-timestamp", now_ms), "not-a-timestamp");
        assert_eq!(display_time("", now_ms), "");
    }
}
