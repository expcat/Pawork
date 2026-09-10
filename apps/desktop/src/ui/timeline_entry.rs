//! UI-3 Timeline 条目：Markdown 正文、默认收起的工具摘要与诚实 Run 终态。
//! 渲染和 AX 共用 presentation state 与高度模型；折叠不删除 reducer 事件。

use gpui::{div, prelude::*, px, Context, FontWeight, Rgba, SharedString, Window};

use crate::projection::{ConnectionState, TimelineEntry, TimelineEntryKind};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::label::Label;
use crate::ui::components::list_row::ListRow;
use crate::ui::i18n::t;
use crate::ui::theme::{dark, font, metrics};

use super::task_rail::relative_activity;
use super::timeline::tool_status_label;
use super::{now_unix_ms, AppView, MenuKind};

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

/// Tool group 标题：只汇总已有状态，不编造 wire 中不存在的耗时。
pub(super) fn tool_group_summary(rows: &[ToolRowView]) -> String {
    let mut completed = 0;
    let mut running = 0;
    let mut pending = 0;
    let mut failed = 0;
    let mut cancelled = 0;
    let mut other = 0;
    for row in rows {
        match row.status {
            ToolRowStatus::Succeeded => completed += 1,
            ToolRowStatus::Running => running += 1,
            ToolRowStatus::Pending => pending += 1,
            ToolRowStatus::Failed => failed += 1,
            ToolRowStatus::Cancelled => cancelled += 1,
            ToolRowStatus::Other => other += 1,
        }
    }
    let mut states = Vec::new();
    for (count, word) in [
        (completed, t("tool.group_completed")),
        (running, t("tool.group_running")),
        (pending, t("tool.group_pending")),
        (failed, t("tool.group_failed")),
        (cancelled, t("tool.group_cancelled")),
        (other, t("tool.group_other")),
    ] {
        if count > 0 {
            states.push(format!("{count} {word}"));
        }
    }
    let count = t(if rows.len() == 1 {
        "tool.group_one"
    } else {
        "tool.group_many"
    })
    .replace("{}", &rows.len().to_string());
    if states.is_empty() {
        count
    } else {
        format!("{count} · {}", states.join(" · "))
    }
}

/// Shared by pixels, measurement and accessibility. Empty output is not missing output.
pub(super) fn tool_detail_text(arguments: Option<&str>, result: Option<&str>) -> String {
    let args = arguments
        .filter(|text| !text.is_empty())
        .map(|text| {
            serde_json::from_str::<serde_json::Value>(text)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| text.into())
        })
        .unwrap_or_else(|| t("tool.arguments_missing").into());
    let result = match result {
        Some("") => t("tool.result_empty"),
        Some(text) => text,
        None => t("tool.result_missing"),
    };
    format!(
        "{}\n{args}\n\n{}\n{result}",
        t("tool.arguments"),
        t("tool.result")
    )
}

impl ToolRowView {
    pub(super) fn from_facts(
        name: &str,
        status: &str,
        arguments: Option<&str>,
        result: Option<&str>,
    ) -> Self {
        let mut row = Self::from_parts(name, status, None);
        let directory_empty = (name == "list_directory")
            .then(|| result.and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok()))
            .flatten()
            .filter(|data| {
                data["total"].as_u64() == Some(0)
                    && data["offset"].as_u64() == Some(0)
                    && data["truncated"].as_bool() == Some(false)
            })
            .and_then(|data| {
                data["path"]
                    .as_str()
                    .map(|path| t("tool.directory_empty").replace("{}", path))
            });
        row.detail = Some(tool_detail_text(
            arguments,
            directory_empty.as_deref().or(result),
        ));
        row
    }

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

/// gpui 默认行高（TextStyle::default 的 φ 比例，经 line_height_in_pixels
/// 四舍五入）。render 未显式设 line_height 的文本按此推导高度；AX 几何
/// 公式与 render 同源共用本函数，不另造第二套行高。
pub(super) fn default_text_line_height(font_px: f32) -> f32 {
    (font_px * 1.618_034).round()
}

/// 公式化换行估算（AX 几何用）：平均字符宽按 0.6 × 字号估计，逐显式行
/// 折算可见行数。gpui 实际按像素 wrap（CJK 更宽），这是有界的诚实近似，
/// 不是精确布局回填。
pub(super) fn estimated_wrapped_lines(text: &str, width_px: f32, font_px: f32) -> usize {
    let chars_per_line = ((width_px / (font_px * 0.6)).floor() as usize).max(1);
    text.lines()
        .map(|line| line.chars().count().div_ceil(chars_per_line).max(1))
        .sum()
}

pub(super) use super::markdown::message_block_line_counts;
use super::markdown::message_body_element;

/// 作者和时间都是 12px 元信息，正文独占主层级。
fn message_label_element(role: &str, time: &str, role_color: Rgba) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_baseline()
        .gap_3()
        .child(
            div()
                .text_size(font::BODY_SM)
                .font_weight(FontWeight::MEDIUM)
                .text_color(role_color)
                .child(role.to_string()),
        )
        .child(
            div()
                .text_size(font::BODY_SM)
                .text_color(dark().text.secondary)
                .truncate()
                .child(time.to_string()),
        )
}

/// 条目「···」fork 菜单（identifier 与行为自旧 timeline_entry_element 冻结迁移）。
pub(super) fn entry_actions_element(
    view: &mut AppView,
    cx: &mut Context<AppView>,
    entry: &TimelineEntry,
    menu_open: bool,
    can_fork: bool,
) -> Dropdown {
    let event_id = entry.event_id.clone();
    let button_id = format!("entry-menu-{}", entry.event_id);
    let entry_focus = view.timeline_entry_focus(&event_id, cx);
    let actions_button = Button::new(button_id.clone())
        .variant(ButtonVariant::Ghost)
        .height(px(24.0))
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
                let down = AppView::click_down_position(event);
                view.toggle_menu(MenuKind::Entry(event_id.clone()), down, cx);
            }
        }))
        .on_activate(cx.listener({
            let event_id = event_id.clone();
            let button_id = button_id.clone();
            move |view, _event, _window, cx| {
                // 已开的浮层由 root menu 负责选择；这里仍留去重标记吞掉
                // 同键 keyup 合成 click，避免 Fork 后菜单被重新打开。
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
        let mut panel = MenuPanel::new(SharedString::from(format!("fork-menu-{}", entry.event_id)))
            .track_scroll(&view.entry_menu_scroll)
            .dismiss_on_outside(cx.listener({
                let kind = MenuKind::Entry(event_id.clone());
                move |view, event: &gpui::MouseDownEvent, _, cx| {
                    view.dismiss_menu_on_outside(kind.clone(), event.position, cx)
                }
            }));
        for (ix, action) in view.entry_menu_actions(&event_id).into_iter().enumerate() {
            let id = action.identifier(&event_id, ix);
            let enabled = !matches!(action.kind, EntryActionKind::Fork) || can_fork;
            panel = panel.child(
                MenuRow::new(id)
                    .label(action.label)
                    .highlighted(view.menu_highlight_effective(0) == ix)
                    .disabled(!enabled)
                    .when(enabled, |row| {
                        row.on_click(cx.listener({
                            let event_id = event_id.clone();
                            move |view, _, window, cx| {
                                view.activate_entry_action(&event_id, ix, window, cx);
                            }
                        }))
                    }),
            );
        }
        if !can_fork {
            panel = panel.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(view.fork_unavailable_reason(&event_id)),
            );
        }
        actions = actions.panel(panel);
    }
    actions
}

/// 作者和操作共用一行，正文使用完整列宽；用户消息用浅底卡片区分轮次。
fn entry_shell_element(
    view: &mut AppView,
    cx: &mut Context<AppView>,
    entry: &TimelineEntry,
    menu_open: bool,
    can_fork: bool,
    label: gpui::Div,
    body: gpui::Div,
) -> gpui::Div {
    let actions = entry_actions_element(view, cx, entry, menu_open, can_fork);
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(metrics::MSG_LABEL_BODY_GAP))
        .when(
            matches!(entry.kind, TimelineEntryKind::UserMessage { .. }),
            |element| {
                element
                    .p(px(metrics::MSG_USER_INSET))
                    .bg(dark().surface.raised)
                    .rounded(px(12.0))
            },
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .min_h(px(24.0))
                .child(label)
                .child(actions),
        )
        .child(body)
}

/// Tool 行状态槽：succeeded = ✓（Ø14，success_fg）+ 状态词；
/// running / pending = accent 点静态表示（本波不做动画）；failed /
/// cancelled = danger_text 状态词；未知状态原样 tertiary。不画耗时列
/// （wire 无 tool duration 字段）。
fn tool_status_element(row: &ToolRowView) -> gpui::Div {
    let word_color = match row.status {
        ToolRowStatus::Succeeded => dark().text.emphasis,
        ToolRowStatus::Failed | ToolRowStatus::Cancelled => dark().semantic.danger_text,
        ToolRowStatus::Running | ToolRowStatus::Pending => dark().text.secondary,
        ToolRowStatus::Other => dark().text.tertiary,
    };
    let word = div()
        .text_size(font::BODY_SM)
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

/// 展开后工具名与状态独占一行，输出在下方换行，不与下一条工具重叠。
fn tool_row_element(row: &ToolRowView) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .h(px(metrics::TOOL_ROW_HEIGHT))
                .flex_none()
                .px_3()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(font::BASE)
                        .text_color(dark().text.primary)
                        .child(row.name.clone()),
                )
                .child(tool_status_element(row)),
        )
        .when_some(row.detail.clone(), |element, detail| {
            element.child(
                div()
                    .px_3()
                    .pb_3()
                    .text_size(font::XS)
                    .line_height(font::from_pixels(20.0))
                    .font_family("monospace")
                    .text_color(dark().text.secondary)
                    .child(detail),
            )
        })
}

/// AX 首帧估算；稳定帧仍优先使用 GPUI 的实际条目 bounds。
pub(super) fn tool_row_height(row: &ToolRowView, width: f32, rem: f32) -> f32 {
    metrics::TOOL_ROW_HEIGHT
        + row.detail.as_deref().map_or(0.0, |detail| {
            estimated_wrapped_lines(detail, (width - 1.5 * rem).max(1.0), font::XS.0 * rem) as f32
                * (20.0 / 16.0 * rem).round()
                + 0.75 * rem
        })
}

pub(super) enum EntryActionKind {
    Copy(String),
    Open(String),
    Fork,
}

pub(super) struct EntryAction {
    pub label: String,
    pub kind: EntryActionKind,
}

impl EntryAction {
    pub(super) fn identifier(&self, event_id: &str, ix: usize) -> String {
        match self.kind {
            EntryActionKind::Fork => super::accessibility::dynamic_identifier("fork", event_id),
            _ => super::accessibility::dynamic_identifier(
                "entry-action",
                &format!("{event_id}:{ix}"),
            ),
        }
    }
}

/// 回复使用同一 Run 的闭合边界，不把 message 事件冒充合法切点。
pub(super) fn fork_target<'a>(timeline: &'a [TimelineEntry], event_id: &str) -> Option<&'a str> {
    let entry = timeline.iter().find(|entry| entry.event_id == event_id)?;
    if entry.is_fork_boundary() {
        return Some(&entry.event_id);
    }
    if !matches!(entry.kind, TimelineEntryKind::AssistantMessage { .. }) {
        return None;
    }
    let run_id = entry.run_id.as_ref()?;
    timeline
        .iter()
        .find(|candidate| {
            candidate.run_id.as_ref() == Some(run_id)
                && candidate.sequence >= entry.sequence
                && candidate.is_fork_boundary()
        })
        .map(|entry| entry.event_id.as_str())
}

impl AppView {
    pub(super) fn entry_menu_actions(&self, event_id: &str) -> Vec<EntryAction> {
        let mut actions = Vec::new();
        if let Some(entry) = self
            .projection
            .timeline
            .iter()
            .find(|e| e.event_id == event_id)
        {
            if let TimelineEntryKind::AssistantMessage { text }
            | TimelineEntryKind::UserMessage { text } = &entry.kind
            {
                actions.push(EntryAction {
                    label: t("timeline.copy_message").into(),
                    kind: EntryActionKind::Copy(text.clone()),
                });
                actions.extend(
                    super::markdown::message_actions(text)
                        .into_iter()
                        .map(|action| EntryAction {
                            label: action.label,
                            kind: if action.open {
                                EntryActionKind::Open(action.content)
                            } else {
                                EntryActionKind::Copy(action.content)
                            },
                        }),
                );
            }
        }
        actions.push(EntryAction {
            label: t("timeline.fork_turn").into(),
            kind: EntryActionKind::Fork,
        });
        actions
    }

    pub(super) fn fork_unavailable_reason(&self, event_id: &str) -> String {
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            t("timeline.fork_offline").into()
        } else if fork_target(&self.projection.timeline, event_id).is_none() {
            t("timeline.fork_boundary").into()
        } else {
            t("timeline.fork_no_session").into()
        }
    }

    pub(super) fn activate_entry_action(
        &mut self,
        event_id: &str,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = self.entry_menu_actions(event_id).into_iter().nth(ix) else {
            return;
        };
        if matches!(action.kind, EntryActionKind::Fork) && !self.can_fork_entry(event_id) {
            return;
        }
        self.close_menu_and_focus_trigger(MenuKind::Entry(event_id.into()), window, cx);
        match action.kind {
            EntryActionKind::Copy(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
            }
            EntryActionKind::Open(url) => cx.open_url(&url),
            EntryActionKind::Fork => self.on_fork(event_id, window, cx),
        }
    }

    /// 消息条目：低强调作者行 + Markdown 正文。
    pub(super) fn message_entry_element(
        &mut self,
        entry: &TimelineEntry,
        menu_open: bool,
        can_fork: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let time = display_time(&entry.timestamp, now_unix_ms());
        let (role, label_color, body) = match &entry.kind {
            TimelineEntryKind::UserMessage { text } => (
                t("timeline.you"),
                dark().text.secondary,
                message_body_element(&entry.event_id, text, dark().text.emphasis),
            ),
            TimelineEntryKind::Thinking { text } => (
                t("timeline.thinking"),
                dark().text.secondary,
                message_body_element(&entry.event_id, text, dark().text.secondary),
            ),
            TimelineEntryKind::AssistantMessage { text } => (
                "Pawork",
                dark().text.secondary,
                message_body_element(&entry.event_id, text, dark().text.emphasis),
            ),
            // 兜底臂（Worker B 组装层不会把 tool / run 态交给消息条目）：
            // 保持旧单行语义，避免意外调用时崩溃。
            TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
                arguments,
            } => (t("timeline.tool"), dark().text.secondary, {
                let mut element = div()
                    .py_1()
                    .text_color(dark().text.secondary)
                    .child(format!("{name} · {status}"));
                let detail = tool_detail_text(arguments.as_deref(), detail.as_deref());
                {
                    element = element.child(
                        div()
                            .text_size(font::XS)
                            .text_color(dark().text.tertiary)
                            .child(detail.to_string()),
                    );
                }
                element
            }),
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
                message_body_element(&entry.event_id, message, dark().semantic.danger_text),
            ),
        };
        entry_shell_element(
            self,
            cx,
            entry,
            menu_open,
            can_fork,
            message_label_element(role, &time, label_color),
            body,
        )
    }

    /// 思考与工具摘要共用焦点、鼠标和键盘折叠交互。
    fn timeline_detail_header(
        &mut self,
        group_key: &str,
        prefix: &str,
        label: String,
        cx: &mut Context<Self>,
    ) -> ListRow {
        let collapsed = !self.expanded_timeline_details.contains(group_key);
        let row_id = format!("{prefix}-{group_key}");
        let click_id = row_id.clone();
        let click_key = group_key.to_string();
        let activate_id = row_id.clone();
        let activate_key = group_key.to_string();
        let focus = self.timeline_detail_focus(group_key, cx);
        ListRow::project_header(row_id)
            .height(metrics::TOOL_GROUP_HEADER_HEIGHT)
            .track_focus(&focus)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .px_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(12.0))
                            .flex_none()
                            .text_size(font::BODY_SM)
                            .text_color(dark().text.secondary)
                            .child(if collapsed { "›" } else { "⌄" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(font::BODY_SM)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(dark().text.secondary)
                            .child(label),
                    ),
            )
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_row_key_click(&click_id, event) {
                    return;
                }
                view.toggle_timeline_detail(&click_key, cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_row_key_activate(&activate_id);
                view.toggle_timeline_detail(&activate_key, cx);
                cx.stop_propagation();
            }))
    }

    pub(super) fn thinking_entry_element(
        &mut self,
        key: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let expanded = self.expanded_timeline_details.contains(key);
        let header =
            self.timeline_detail_header(key, "thinking-toggle", t("timeline.thinking").into(), cx);
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(header)
            .when(expanded, |panel| {
                panel.child(
                    div()
                        .p(px(12.0))
                        .text_size(font::BASE)
                        .text_color(dark().text.secondary)
                        .child(text.to_string()),
                )
            })
    }

    /// Tool activity：36px 轻量摘要，默认折叠。
    pub(super) fn tool_group_element(
        &mut self,
        group_key: &str,
        rows: &[ToolRowView],
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let collapsed = !self.expanded_timeline_details.contains(group_key);
        let header = self.timeline_detail_header(
            group_key,
            "tool-group-toggle",
            tool_group_summary(rows),
            cx,
        );
        let mut panel = div()
            .flex()
            .flex_col()
            .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
            .when(!collapsed, |panel| {
                panel.border_l_1().border_color(dark().border.subtle)
            })
            .rounded(px(metrics::TOOL_GROUP_RADIUS))
            .overflow_hidden()
            .child(
                div()
                    .h(px(metrics::TOOL_GROUP_HEADER_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .when(!collapsed, |header| {
                        header.border_b_1().border_color(dark().border.subtle)
                    })
                    .child(header),
            );
        if collapsed {
            return panel;
        }
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
    /// 右侧主按钮 “Review changes”（168×40 r8，点击切 Inspector Changes）；
    /// 仅有当前 session 的真实 Changes 时才渲染；“Open in editor” 无 Host
    /// capability 不画。无权威数据时 description 为组装层通用说明。
    pub(super) fn run_summary_element(
        &mut self,
        view: &RunSummaryView,
        event_id: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let button_id = format!("run-review-{event_id}");
        let review_focus = self.timeline_review_focus(event_id, cx);
        let button = Button::new(button_id.clone())
            .variant(ButtonVariant::Primary)
            .width(px(metrics::SUMMARY_BUTTON_WIDTH))
            .height(px(metrics::SUMMARY_BUTTON_HEIGHT))
            .center()
            .radius(metrics::SUMMARY_BUTTON_RADIUS)
            .text_size(font::BODY_SM)
            .label(t("timeline.review_changes"))
            .track_focus(&review_focus);
        let button = if view.review_changes_enabled {
            Some(
                button
                    .on_click(cx.listener({
                        let event_id = event_id.to_string();
                        let button_id = button_id.clone();
                        move |view, event, _window, cx| {
                            if view.consume_button_key_click(&button_id, event) {
                                return;
                            }
                            // click 与普通键盘最终复用同一 Review handler；两条
                            // 路径仅在同一个 render enable gate 下挂接。键盘入口
                            // 额外按 event_id 复核，防虚拟化条目状态变更后越权。
                            let _ = &event_id;
                            view.on_review_changes(cx);
                        }
                    }))
                    .on_activate(cx.listener({
                        let event_id = event_id.to_string();
                        move |view, _event, _window, cx| {
                            view.activate_review_changes_from_keyboard(&event_id, cx);
                            cx.stop_propagation();
                        }
                    })),
            )
        } else {
            None
        };
        let (circle_bg, circle_fg, circle_glyph) = match view.terminal {
            RunSummaryTerminal::Completed => (dark().semantic.success_fg, dark().bg.base, "✓"),
            RunSummaryTerminal::Failed => (dark().semantic.danger_bg, dark().text.on_accent, "✕"),
            RunSummaryTerminal::Cancelled => (dark().surface.disabled, dark().text.tertiary, "—"),
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
            .text_size(font::BODY)
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
                            .text_size(font::BODY)
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
                            .text_size(font::BODY_SM)
                            .line_height(font::from_pixels(metrics::MSG_LINE_HEIGHT))
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
            .when_some(button, |card, button| card.child(button))
    }

    /// F-08 Timeline 页脚：终态词（左）+ 终态时间（右），17px secondary。
    /// 无终态时长字段，不画 “· 2m 14s”（量图耗时属演示数据）。
    pub(super) fn run_footer_element(&self, label: &str, time: &str) -> gpui::Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap_2()
            .justify_between()
            .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
            .child(
                Label::new(label.to_string())
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            )
            .child(
                Label::new(time.to_string())
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            )
    }

    /// F-07/F-08 错误条目：danger_text 新条目层级（标签行 + 正文），
    /// 不加假 retry 按钮（retry 条件属 Wave B 场景）。
    pub(super) fn error_entry_element(
        &mut self,
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
            self,
            cx,
            entry,
            menu_open,
            can_fork,
            message_label_element("Error", &time, dark().semantic.danger_text),
            message_body_element(&entry.event_id, &message, dark().semantic.danger_text),
        )
    }

    pub(super) fn on_fork(&mut self, event_id: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        let boundary = fork_target(&self.projection.timeline, event_id).map(str::to_owned);
        let Some(boundary) = boundary else {
            self.status_hint = Some(
                "Fork is only available on a finished run (completed, cancelled, or failed)."
                    .into(),
            );
            cx.notify();
            return;
        };
        self.controller.fork_session(session_id, boundary);
        // Fork 响应会重建 Timeline 并卸载当前条目触发器；先把焦点交回
        // Composer，避免菜单选择后留下悬空的行级 FocusHandle。
        self.focus_composer(window, cx);
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
        let row = ToolRowView::from_facts(
            "list_directory",
            "succeeded",
            Some(r#"{"path":"."}"#),
            Some(r#"{"path":".","total":0,"offset":0,"truncated":false}"#),
        );
        assert!(row
            .detail
            .unwrap()
            .contains("Directory . · 0 entries (empty)"));
        assert!(ToolRowView::from_facts("read", "succeeded", None, Some(""))
            .detail
            .unwrap()
            .contains("Empty result"));
        assert!(ToolRowView::from_facts("read", "succeeded", None, None)
            .detail
            .unwrap()
            .contains("Result not provided"));

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

        let rows = vec![
            ToolRowView::from_parts("read_file", "succeeded", None),
            ToolRowView::from_parts("bash", "running", None),
            ToolRowView::from_parts("edit_file", "failed", None),
        ];
        assert_eq!(
            tool_group_summary(&rows),
            "3 tools · 1 completed · 1 running · 1 failed"
        );
    }

    /// 摘要卡视图构造：字段直存，禁用原因为独立通道。
    #[test]
    fn run_summary_view_construction() {
        let view = RunSummaryView {
            title: "Ready for review".into(),
            description: "Run finished.".into(),
            terminal: RunSummaryTerminal::Completed,
            review_changes_enabled: false,
        };
        assert_eq!(view.title, "Ready for review");
        assert!(!view.review_changes_enabled);
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
