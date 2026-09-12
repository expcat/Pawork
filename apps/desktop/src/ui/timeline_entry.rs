//! UI-3 Timeline 条目：Markdown 正文、默认收起的工具摘要与诚实 Run 终态。
//! 渲染和 AX 共用 presentation state 与高度模型；折叠不删除 reducer 事件。

use gpui::{div, prelude::*, px, Context, FontWeight, Rgba, SharedString, Window};

use crate::projection::{ConnectionState, ForkBoundary, TimelineEntry, TimelineEntryKind};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::dropdown::{Dropdown, MenuPanel, MenuRow};
use crate::ui::components::icon::{icon_sized, Icon};
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

/// 工具目标单行上限；超出截断加省略号，不伪造剩余内容。
pub(super) const HEADLINE_TARGET_MAX_CHARS: usize = 80;
/// 展开态结果预览行数（8–12 合同取中值）；全文走 `event_id:result` 展开键。
pub(super) const RESULT_PREVIEW_LINES: usize = 10;
pub(super) const RESULT_EXPAND_HEIGHT: f32 = 24.0;

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
    pub event_id: String,
    pub name: String,
    pub headline: String,
    pub headline_has_target: bool,
    pub status_label: String,
    pub status: ToolRowStatus,
    pub result_can_expand: bool,
    pub result_expanded: bool,
    pub detail: Option<String>,
}

pub(super) fn tool_result_expand_key(event_id: &str) -> String {
    format!("{event_id}:result")
}

/// 内建 8 个工具按参数键抽目标；其余 / 缺键 / 畸形 / 非字符串回退为仅名称。
pub(super) fn tool_headline(name: &str, arguments: Option<&str>) -> String {
    match tool_headline_target(name, arguments) {
        Some(target) => format!("{name} {target}"),
        None => name.to_string(),
    }
}

fn tool_argument_object(arguments: Option<&str>) -> Option<serde_json::Value> {
    let text = arguments.filter(|text| !text.is_empty())?;
    serde_json::from_str(text).ok()
}

fn json_string_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
}

fn truncate_headline_target(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn tool_headline_target(name: &str, arguments: Option<&str>) -> Option<String> {
    let value = tool_argument_object(arguments)?;
    let key = match name {
        "read_file" | "write_file" | "edit_file" | "apply_patch" | "list_directory" => "path",
        "run_command" => "command",
        "search_text" | "find_files" => "pattern",
        _ => return None,
    };
    let mut target = json_string_field(&value, key)?.to_string();
    if name == "search_text" {
        if let Some(glob) = json_string_field(&value, "glob") {
            target = format!("{target} {glob}");
        }
    }
    Some(truncate_headline_target(&target, HEADLINE_TARGET_MAX_CHARS))
}

fn preview_result_text(text: &str, expanded: bool) -> (String, bool) {
    let can_expand = text.lines().count() > RESULT_PREVIEW_LINES;
    if !can_expand || expanded {
        (text.to_string(), can_expand)
    } else {
        (
            text.lines()
                .take(RESULT_PREVIEW_LINES)
                .collect::<Vec<_>>()
                .join("\n"),
            true,
        )
    }
}

fn arguments_secondary(arguments: Option<&str>) -> String {
    arguments
        .filter(|text| !text.is_empty())
        .map(|text| {
            serde_json::from_str::<serde_json::Value>(text)
                .ok()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| text.to_string())
        })
        .unwrap_or_else(|| t("tool.arguments_missing").into())
}

fn directory_empty_label(name: &str, result: Option<&str>) -> Option<String> {
    (name == "list_directory")
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
        })
}

fn result_full_text(name: &str, result: Option<&str>) -> String {
    match directory_empty_label(name, result).as_deref().or(result) {
        Some("") => t("tool.result_empty").into(),
        Some(text) => text.to_string(),
        None => t("tool.result_missing").into(),
    }
}

/// 当前 Run 仍 active，且该助手消息是 reducer 流式未提交（最新相位
/// `run streaming_response`）的最后一条非空助手正文。
pub(super) fn assistant_is_streaming(
    timeline: &[TimelineEntry],
    entry: &TimelineEntry,
    active_run_id: Option<&str>,
) -> bool {
    let Some(run_id) = active_run_id else {
        return false;
    };
    if entry.run_id.as_deref() != Some(run_id) {
        return false;
    }
    let TimelineEntryKind::AssistantMessage { text } = &entry.kind else {
        return false;
    };
    if text.trim().is_empty() {
        return false;
    }
    let last = timeline.iter().rev().find(|candidate| {
        candidate.run_id.as_deref() == Some(run_id)
            && matches!(
                &candidate.kind,
                TimelineEntryKind::AssistantMessage { text } if !text.trim().is_empty()
            )
    });
    if last.map(|candidate| candidate.event_id.as_str()) != Some(entry.event_id.as_str()) {
        return false;
    }
    timeline
        .iter()
        .rev()
        .find_map(|candidate| {
            if candidate.run_id.as_deref() != Some(run_id) {
                return None;
            }
            match &candidate.kind {
                TimelineEntryKind::RunState(state) => Some(state.as_str()),
                _ => None,
            }
        })
        .is_some_and(|state| state == "run streaming_response")
}

fn tool_group_status_parts(rows: &[ToolRowView]) -> Vec<String> {
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
    states
}

/// Tool group 折叠标题：单工具直接 headline；多工具为数量 + 前 3 条
/// headline；状态计数与 render / AX 同源。
pub(super) fn tool_group_summary(rows: &[ToolRowView]) -> String {
    let statuses = tool_group_status_parts(rows);
    if rows.len() == 1 {
        let headline = rows[0].headline.as_str();
        let only_succeeded = rows[0].status == ToolRowStatus::Succeeded;
        if only_succeeded || statuses.is_empty() {
            return headline.to_string();
        }
        return format!("{headline} · {}", statuses.join(" · "));
    }
    let count = t("tool.group_many").replace("{}", &rows.len().to_string());
    let mut parts = vec![count];
    parts.extend(rows.iter().take(3).map(|row| row.headline.clone()));
    if rows.len() > 3 {
        parts.push("…".into());
    }
    parts.extend(statuses);
    parts.join(" · ")
}

impl ToolRowView {
    pub(super) fn from_facts(
        name: &str,
        status: &str,
        arguments: Option<&str>,
        result: Option<&str>,
    ) -> Self {
        Self::present(name, status, arguments, result, "", false)
    }

    pub(super) fn present(
        name: &str,
        status: &str,
        arguments: Option<&str>,
        result: Option<&str>,
        event_id: &str,
        result_expanded: bool,
    ) -> Self {
        let headline_target = tool_headline_target(name, arguments);
        let headline_has_target = headline_target.is_some();
        let headline = match headline_target {
            Some(target) => format!("{name} {target}"),
            None => name.to_string(),
        };
        let result_full = result_full_text(name, result);
        let (result_shown, result_can_expand) = preview_result_text(&result_full, result_expanded);
        let detail = if headline_has_target {
            result_shown
        } else {
            format!("{}\n\n{result_shown}", arguments_secondary(arguments))
        };
        Self {
            event_id: event_id.to_string(),
            name: name.to_string(),
            headline,
            headline_has_target,
            status_label: tool_status_label(status),
            status: tool_row_status(status),
            result_can_expand,
            result_expanded,
            detail: Some(detail),
        }
    }

    /// wire 原文字段 → 渲染视图。detail 空串归一为 None（旧渲染同语义）。
    pub(super) fn from_parts(name: &str, status: &str, detail: Option<&str>) -> Self {
        Self {
            event_id: String::new(),
            name: name.to_string(),
            headline: name.to_string(),
            headline_has_target: false,
            status_label: tool_status_label(status),
            status: tool_row_status(status),
            result_can_expand: false,
            result_expanded: false,
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

/// GUI3-04：失败 / 完成摘要卡状态圆（原 Ø40 收为 Ø20；几何 token 保持 px）。
pub(super) const SUMMARY_STATUS_CIRCLE: f32 = 20.0;
/// 状态圆内字形（合同 12–14px，取 13）。
pub(super) const SUMMARY_STATUS_GLYPH_PX: f32 = 13.0;
/// 摘要卡水平内边距（原 pl 15 / pr_5）。
pub(super) const SUMMARY_BANNER_PAD_X: f32 = 12.0;
/// py_3 = 0.75 rem（原 py_6 = 1.5 rem）。
pub(super) const SUMMARY_BANNER_PAD_Y_REMS: f32 = 0.75;
/// gap_2 = 0.5 rem（标题 / 原因 / CTA 竖向；圆与标题横向）。
pub(super) const SUMMARY_BANNER_GAP_REMS: f32 = 0.5;
/// 认证失败 Raised CTA 行高。
pub(super) const SUMMARY_NEXT_STEP_BUTTON_HEIGHT: f32 = 28.0;

/// 失败卡下一步（UI 启发式，不改协议）。命中认证类关键词才给供应商设置入口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FailureNextStep {
    OpenProviderSettings,
    None,
}

/// 原因原文（大小写不敏感）是否指向认证失败。不加假 retry。
pub(super) fn failure_next_step(reason: &str) -> FailureNextStep {
    let haystack = reason.to_ascii_lowercase();
    const KEYWORDS: &[&str] = &[
        "401",
        "403",
        "authentication",
        "unauthorized",
        "invalid api key",
    ];
    if KEYWORDS.iter().any(|keyword| haystack.contains(keyword)) {
        FailureNextStep::OpenProviderSettings
    } else {
        FailureNextStep::None
    }
}

pub(super) fn run_open_providers_identifier(event_id: &str) -> String {
    format!("run-open-providers-{event_id}")
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
fn message_label_element(role: &str, time: &str, role_color: Rgba, generating: bool) -> gpui::Div {
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
        .when(generating, |row| {
            row.child(
                div()
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(t("tool.generating").to_string()),
            )
        })
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
    let mut actions_button = Button::new(button_id.clone())
        .variant(ButtonVariant::Ghost)
        .height(px(24.0))
        .text_size(font::XS)
        .text_color(dark().text.secondary)
        .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
        .child(icon_sized(Icon::More, px(metrics::ICON_SM)));
    if entry.fork_boundary.is_some() {
        actions_button =
            actions_button.tooltip(view.projection.run_usage_label(entry.run_id.as_deref()));
    }
    let actions_button = actions_button
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
            let enabled = match action.kind {
                EntryActionKind::Fork => can_fork,
                EntryActionKind::Info => false,
                _ => true,
            };
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

/// 用户气泡靠右收缩；作者与操作留在气泡外，助手正文保持开放排版。
fn entry_shell_element(
    view: &mut AppView,
    cx: &mut Context<AppView>,
    window: &Window,
    entry: &TimelineEntry,
    menu_open: bool,
    can_fork: bool,
    label: gpui::Div,
    body: gpui::Div,
) -> gpui::Div {
    let user_text = match &entry.kind {
        TimelineEntryKind::UserMessage { text } => Some(text.as_str()),
        _ => None,
    };
    let focus = view.timeline_entry_focus(&entry.event_id, cx);
    let actions = entry_actions_element(view, cx, entry, menu_open, can_fork);
    let action_slot = view
        .settings_element(format!("message-actions-{}", entry.event_id))
        .flex_none()
        .opacity(if menu_open || focus.is_focused(window) {
            1.0
        } else {
            0.0
        })
        .group_hover("timeline-message", |style| style.opacity(1.0))
        .child(actions);
    let mut header = div().flex().items_center().gap_3().min_h(px(24.0));
    if user_text.is_some() {
        header = header.justify_end().child(
            div()
                .text_size(font::BODY_SM)
                .text_color(dark().text.tertiary)
                .child(display_time(&entry.timestamp, now_unix_ms())),
        );
    } else {
        header = header.justify_between().child(label);
    }
    let content = if let Some(text) = user_text {
        let wide = super::markdown::message_needs_full_width(text);
        div().flex().justify_end().child(
            view.settings_element(format!("message-bubble-{}", entry.event_id))
                .flex()
                .flex_col()
                .max_w(gpui::relative(if wide { 1.0 } else { 0.8 }))
                .when(wide, |bubble| bubble.w_full())
                .px(px(metrics::MSG_USER_INSET_X))
                .py(px(metrics::MSG_USER_INSET_Y))
                .bg(dark().surface.raised)
                .rounded(px(12.0))
                .child(body.w_auto()),
        )
    } else {
        body
    };
    div()
        .group("timeline-message")
        .flex()
        .flex_col()
        .w_full()
        .gap(px(metrics::MSG_LABEL_BODY_GAP))
        .child(header.child(action_slot))
        .child(content)
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
                icon_sized(Icon::Check, px(metrics::TOOL_CHECK_SIZE))
                    .text_color(dark().semantic.success_fg),
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

/// 展开后 headline 独占主行；参数 JSON 仅在未能抽出目标时作为次级；
/// 结果预览与「展开全文」共用 `expanded_timeline_details`。
fn tool_row_element(view: &mut AppView, row: &ToolRowView, cx: &mut Context<AppView>) -> gpui::Div {
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
                    div().flex().flex_row().flex_1().min_w_0().child(
                        div()
                            .truncate()
                            .text_size(font::BASE)
                            .text_color(dark().text.primary)
                            .child(row.headline.clone()),
                    ),
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
        .when(
            row.result_can_expand && !row.event_id.is_empty(),
            |element| {
                element.child(
                    div()
                        .px_3()
                        .child(tool_result_expand_element(view, row, cx)),
                )
            },
        )
}

fn tool_result_expand_element(
    view: &mut AppView,
    row: &ToolRowView,
    cx: &mut Context<AppView>,
) -> Button {
    let event_id = row.event_id.clone();
    let key = tool_result_expand_key(&event_id);
    let button_id = format!("tool-result-toggle-{event_id}");
    let focus = view.timeline_detail_focus(&key, cx);
    let expanded = row.result_expanded;
    Button::new(button_id.clone())
        .variant(ButtonVariant::Ghost)
        .height(px(RESULT_EXPAND_HEIGHT))
        .text_size(font::BODY_SM)
        .text_color(dark().text.secondary)
        .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
        .label(if expanded {
            t("tool.result_collapse")
        } else {
            t("tool.result_expand")
        })
        .track_focus(&focus)
        .on_click(cx.listener({
            let button_id = button_id.clone();
            let key = key.clone();
            move |view, event, _window, cx| {
                if view.consume_button_key_click(&button_id, event) {
                    return;
                }
                view.toggle_timeline_detail(&key, cx);
            }
        }))
        .on_activate(cx.listener({
            let button_id = button_id.clone();
            let key = key.clone();
            move |view, _event, _window, cx| {
                view.note_button_key_activate(&button_id);
                view.toggle_timeline_detail(&key, cx);
                cx.stop_propagation();
            }
        }))
}

/// AX 首帧估算；稳定帧仍优先使用 GPUI 的实际条目 bounds。
pub(super) fn tool_row_height(row: &ToolRowView, width: f32, rem: f32) -> f32 {
    let expand = if row.result_can_expand && !row.event_id.is_empty() {
        RESULT_EXPAND_HEIGHT
    } else {
        0.0
    };
    metrics::TOOL_ROW_HEIGHT
        + row.detail.as_deref().map_or(0.0, |detail| {
            estimated_wrapped_lines(detail, (width - 1.5 * rem).max(1.0), font::XS.0 * rem) as f32
                * (20.0 / 16.0 * rem).round()
                + 0.75 * rem
        })
        + expand
}

pub(super) enum EntryActionKind {
    Copy(String),
    Open(String),
    Fork,
    Info,
}

pub(super) struct EntryAction {
    pub label: String,
    pub kind: EntryActionKind,
}

impl EntryAction {
    pub(super) fn identifier(&self, event_id: &str, ix: usize) -> String {
        match self.kind {
            EntryActionKind::Fork => super::accessibility::dynamic_identifier("fork", event_id),
            EntryActionKind::Info => {
                super::accessibility::dynamic_identifier("run-usage", event_id)
            }
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
        if let Some(entry) = self
            .projection
            .timeline
            .iter()
            .find(|e| e.event_id == event_id)
        {
            if entry.fork_boundary.is_some() {
                actions.push(EntryAction {
                    label: self.projection.run_usage_label(entry.run_id.as_deref()),
                    kind: EntryActionKind::Info,
                });
            }
        }
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
        if matches!(action.kind, EntryActionKind::Info) {
            return;
        }
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
            EntryActionKind::Info => {}
        }
    }

    /// 消息条目：低强调作者行 + Markdown 正文。
    pub(super) fn message_entry_element(
        &mut self,
        entry: &TimelineEntry,
        menu_open: bool,
        can_fork: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let time = display_time(&entry.timestamp, now_unix_ms());
        let generating = assistant_is_streaming(
            &self.projection.timeline,
            entry,
            self.projection.active_run_id.as_deref(),
        );
        let (role, label_color, body) = match &entry.kind {
            TimelineEntryKind::UserMessage { text } => (
                t("timeline.you"),
                dark().text.secondary,
                message_body_element(
                    self,
                    cx,
                    window,
                    &entry.event_id,
                    text,
                    dark().text.emphasis,
                    false,
                ),
            ),
            TimelineEntryKind::Thinking { text } => (
                t("timeline.thinking"),
                dark().text.secondary,
                message_body_element(
                    self,
                    cx,
                    window,
                    &entry.event_id,
                    text,
                    dark().text.secondary,
                    false,
                ),
            ),
            TimelineEntryKind::AssistantMessage { text } => (
                "Pawork",
                dark().text.secondary,
                message_body_element(
                    self,
                    cx,
                    window,
                    &entry.event_id,
                    text,
                    dark().text.emphasis,
                    generating,
                ),
            ),
            // 兜底臂（Worker B 组装层不会把 tool / run 态交给消息条目）：
            // 保持旧单行语义，避免意外调用时崩溃。
            TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
                arguments,
            } => (t("timeline.tool"), dark().text.secondary, {
                let row =
                    ToolRowView::from_facts(name, status, arguments.as_deref(), detail.as_deref());
                let mut element = div()
                    .py_1()
                    .text_color(dark().text.secondary)
                    .child(row.headline.clone());
                if let Some(detail) = row.detail {
                    element = element.child(
                        div()
                            .text_size(font::XS)
                            .text_color(dark().text.tertiary)
                            .child(detail),
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
                message_body_element(
                    self,
                    cx,
                    window,
                    &entry.event_id,
                    message,
                    dark().semantic.danger_text,
                    false,
                ),
            ),
        };
        entry_shell_element(
            self,
            cx,
            window,
            entry,
            menu_open,
            can_fork,
            message_label_element(role, &time, label_color, generating),
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
                    .px_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(12.0))
                            .flex_none()
                            .text_color(dark().text.secondary)
                            .child(if collapsed {
                                icon_sized(Icon::ChevronRight, px(12.0))
                            } else {
                                icon_sized(Icon::ChevronDown, px(12.0))
                            }),
                    )
                    .child(
                        div().flex().flex_row().flex_1().min_w_0().child(
                            div()
                                .truncate()
                                .text_size(font::BODY_SM)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(dark().text.secondary)
                                .child(label),
                        ),
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
            let mut element = tool_row_element(self, row, cx);
            if index > 0 {
                element = element
                    .border_t(px(metrics::TOOL_ROW_DIVIDER))
                    .border_color(dark().border.subtle);
            }
            panel = panel.child(element);
        }
        panel
    }

    /// GUI3-04 Run 摘要卡：Ø20 状态圆 + 单行标题 + 原因（失败不截断）。
    /// Failed 为紧凑 banner；认证类原因给 Raised「打开供应商设置」
    ///（复用 on_manage_composer_models，不加假 retry）。Completed 有可审阅
    /// Changes 时仍是 Review changes（Primary 168×40 / gate / 文案不变），
    /// 仅圆径与内边距与 banner 对齐。
    pub(super) fn run_summary_element(
        &mut self,
        view: &RunSummaryView,
        event_id: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if view.terminal == RunSummaryTerminal::Failed {
            self.failed_run_summary_banner(view, event_id, cx)
        } else {
            self.completed_run_summary_card(view, event_id, cx)
        }
    }

    fn run_summary_card_shell() -> gpui::Div {
        div()
            .max_w(px(metrics::TIMELINE_READABLE_WIDTH))
            .border_1()
            .border_color(dark().border.subtle)
            .rounded(px(metrics::TOOL_GROUP_RADIUS))
            .px(px(SUMMARY_BANNER_PAD_X))
            .py_3()
    }

    fn run_summary_status_circle(terminal: RunSummaryTerminal) -> gpui::Div {
        let (circle_bg, circle_fg, status_icon) = match terminal {
            RunSummaryTerminal::Completed => (
                dark().semantic.success_fg,
                dark().bg.base,
                Some(Icon::Check),
            ),
            RunSummaryTerminal::Failed => (
                dark().semantic.danger_bg,
                dark().text.on_accent,
                Some(Icon::Cancel),
            ),
            RunSummaryTerminal::Cancelled => (dark().surface.disabled, dark().text.tertiary, None),
        };
        let mut circle = div()
            .w(px(SUMMARY_STATUS_CIRCLE))
            .h(px(SUMMARY_STATUS_CIRCLE))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(circle_bg)
            .text_color(circle_fg);
        circle = match status_icon {
            Some(status_icon) => circle
                .child(icon_sized(status_icon, px(SUMMARY_STATUS_GLYPH_PX)).text_color(circle_fg)),
            None => circle
                .text_size(font::from_pixels(SUMMARY_STATUS_GLYPH_PX))
                .child("—"),
        };
        circle
    }

    fn run_summary_title_row(&self, view: &RunSummaryView) -> gpui::Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(Self::run_summary_status_circle(view.terminal))
            .child(
                div().flex().flex_row().flex_1().min_w_0().child(
                    div()
                        .text_size(font::BODY)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(dark().text.primary)
                        .child(view.title.clone()),
                ),
            )
    }

    fn run_summary_text_indent() -> gpui::Div {
        div()
            .w(px(SUMMARY_STATUS_CIRCLE))
            .h(px(SUMMARY_STATUS_CIRCLE))
            .flex_none()
    }

    fn failed_run_summary_banner(
        &mut self,
        view: &RunSummaryView,
        event_id: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let next_step = failure_next_step(&view.description);
        let reason = div()
            .flex()
            .flex_row()
            .gap_2()
            .child(Self::run_summary_text_indent())
            .child(
                div().flex().flex_row().flex_1().min_w_0().child(
                    div()
                        .flex_1()
                        .text_size(font::BODY_SM)
                        .text_color(dark().text.secondary)
                        .child(view.description.clone()),
                ),
            );
        Self::run_summary_card_shell()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.run_summary_title_row(view))
            .child(reason)
            .when(next_step == FailureNextStep::OpenProviderSettings, |card| {
                card.child(self.open_providers_cta(event_id, cx))
            })
    }

    fn open_providers_cta(&mut self, event_id: &str, cx: &mut Context<Self>) -> gpui::Div {
        let button_id = run_open_providers_identifier(event_id);
        let focus = self.timeline_open_providers_focus(event_id, cx);
        let button = Button::new(button_id.clone())
            .variant(ButtonVariant::Raised)
            .height(px(SUMMARY_NEXT_STEP_BUTTON_HEIGHT))
            .vcenter()
            .radius(metrics::CONTROL_RADIUS)
            .text_size(font::BODY_SM)
            .label(t("run.open_provider_settings"))
            .track_focus(&focus)
            .on_click(cx.listener({
                let button_id = button_id.clone();
                let event_id = event_id.to_string();
                move |view, event, window, cx| {
                    if view.consume_button_key_click(&button_id, event) {
                        return;
                    }
                    let _ = &event_id;
                    view.on_manage_composer_models(window, cx);
                }
            }))
            .on_activate(cx.listener({
                let event_id = event_id.to_string();
                move |view, _event, window, cx| {
                    view.activate_open_providers_from_keyboard(&event_id, window, cx);
                    cx.stop_propagation();
                }
            }));
        div()
            .flex()
            .flex_row()
            .gap_2()
            .child(Self::run_summary_text_indent())
            .child(button)
    }

    fn activate_open_providers_from_keyboard(
        &mut self,
        event_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.note_button_key_activate(&run_open_providers_identifier(event_id));
        let allowed = self.projection.timeline.iter().any(|entry| {
            if entry.event_id != event_id || entry.fork_boundary != Some(ForkBoundary::Failed) {
                return false;
            }
            let reason = crate::projection::run_summary_texts(entry, false)
                .map(|(_, description)| description)
                .unwrap_or_default();
            failure_next_step(&reason) == FailureNextStep::OpenProviderSettings
        });
        if allowed {
            self.on_manage_composer_models(window, cx);
        }
    }

    fn completed_run_summary_card(
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
        let left_column = div().flex().flex_row().flex_1().min_w_0().child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .gap_2()
                .child(self.run_summary_title_row(view))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(Self::run_summary_text_indent())
                        .child(
                            div().flex().flex_row().flex_1().min_w_0().child(
                                div()
                                    .flex_1()
                                    .text_size(font::BODY_SM)
                                    .line_height(font::from_pixels(metrics::MSG_LINE_HEIGHT))
                                    .line_clamp(2)
                                    .text_color(dark().text.secondary)
                                    .child(view.description.clone()),
                            ),
                        ),
                ),
        );
        Self::run_summary_card_shell()
            .flex()
            .flex_row()
            .items_center()
            .gap_6()
            .child(left_column)
            .when_some(button, |card, button| card.child(button))
    }

    /// F-08 Timeline 页脚：终态词（左）+ 终态时间（右），17px secondary。
    /// 用量收进「···」菜单，不占页脚。
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
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let message = match &entry.kind {
            TimelineEntryKind::Error(message) => message.clone(),
            _ => String::new(),
        };
        let time = display_time(&entry.timestamp, now_unix_ms());
        let body = message_body_element(
            self,
            cx,
            window,
            &entry.event_id,
            &message,
            dark().semantic.danger_text,
            false,
        );
        entry_shell_element(
            self,
            cx,
            window,
            entry,
            menu_open,
            can_fork,
            message_label_element("Error", &time, dark().semantic.danger_text, false),
            body,
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

    /// 状态词映射合同：succeeded → Completed；running → In progress；未知词原样。
    #[test]
    fn tool_status_label_maps_succeeded_only() {
        assert_eq!(tool_status_label("succeeded"), "Completed");
        assert_eq!(tool_status_label("running"), "In progress");
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

    /// 内建 8 工具按参数键抽目标；MCP / 畸形 / 非字符串 / 超长回退不伪造。
    #[test]
    fn tool_headline_extracts_builtin_targets_and_falls_back() {
        assert_eq!(
            tool_headline("read_file", Some(r#"{"path":"a.rs"}"#)),
            "read_file a.rs"
        );
        assert_eq!(
            tool_headline("write_file", Some(r#"{"path":"src/new_feature.rs"}"#)),
            "write_file src/new_feature.rs"
        );
        assert_eq!(
            tool_headline(
                "edit_file",
                Some(r#"{"path":"b.rs","old_string":"a\nb","new_string":"a\nb\nc"}"#)
            ),
            "edit_file b.rs"
        );
        assert_eq!(
            tool_headline("apply_patch", Some(r#"{"path":"patch.diff"}"#)),
            "apply_patch patch.diff"
        );
        assert_eq!(
            tool_headline("list_directory", Some(r#"{"path":"crates"}"#)),
            "list_directory crates"
        );
        assert_eq!(
            tool_headline("run_command", Some(r#"{"command":"cargo test"}"#)),
            "run_command cargo test"
        );
        assert_eq!(
            tool_headline("search_text", Some(r#"{"pattern":"TODO"}"#)),
            "search_text TODO"
        );
        assert_eq!(
            tool_headline("search_text", Some(r#"{"pattern":"TODO","glob":"*.rs"}"#)),
            "search_text TODO *.rs"
        );
        assert_eq!(
            tool_headline("find_files", Some(r#"{"pattern":"**/*.rs"}"#)),
            "find_files **/*.rs"
        );
        assert_eq!(
            tool_headline("mcp_search", Some(r#"{"path":"a.rs"}"#)),
            "mcp_search"
        );
        assert_eq!(tool_headline("read_file", Some("{not json")), "read_file");
        assert_eq!(
            tool_headline("read_file", Some(r#"{"path":1}"#)),
            "read_file"
        );
        assert_eq!(
            tool_headline("read_file", Some(r#"{"file":"a.rs"}"#)),
            "read_file"
        );
        assert_eq!(tool_headline("run_command", None), "run_command");
        let command = "a".repeat(90);
        let args = format!(r#"{{"command":"{command}"}}"#);
        let headline = tool_headline("run_command", Some(&args));
        assert!(headline.starts_with("run_command "));
        assert!(headline.ends_with('…'));
        assert_eq!(
            headline.chars().count(),
            "run_command ".chars().count() + HEADLINE_TARGET_MAX_CHARS + 1
        );
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
        assert_eq!(row.headline, "list_directory .");
        assert!(row.headline_has_target);
        assert!(row
            .detail
            .unwrap()
            .contains("Directory . · 0 entries (empty)"));
        let empty = ToolRowView::from_facts("read", "succeeded", None, Some(""));
        assert!(!empty.headline_has_target);
        assert!(empty.detail.unwrap().contains("Empty result"));
        assert!(ToolRowView::from_facts("read", "succeeded", None, None)
            .detail
            .unwrap()
            .contains("Result not provided"));

        let extracted = ToolRowView::from_facts(
            "write_file",
            "succeeded",
            Some(r#"{"path":"src/new_feature.rs"}"#),
            Some("ok"),
        );
        assert_eq!(extracted.headline, "write_file src/new_feature.rs");
        assert_eq!(extracted.detail.as_deref(), Some("ok"));
        assert!(!extracted.detail.unwrap_or_default().contains("old_string"));

        let long_result = (0..15)
            .map(|ix| format!("line-{ix}"))
            .collect::<Vec<_>>()
            .join("\n");
        let preview = ToolRowView::from_facts("bash", "succeeded", None, Some(&long_result));
        assert!(preview.result_can_expand);
        let preview_detail = preview.detail.as_deref().unwrap();
        assert!(preview_detail.contains("line-0"));
        assert!(preview_detail.contains(&format!("line-{}", RESULT_PREVIEW_LINES - 1)));
        assert!(!preview_detail.contains("line-14"));
        let full = ToolRowView::present("bash", "succeeded", None, Some(&long_result), "t1", true);
        assert!(full.detail.as_deref().unwrap().contains("line-14"));
        let extracted_preview = ToolRowView::from_facts(
            "write_file",
            "succeeded",
            Some(r#"{"path":"a.rs"}"#),
            Some(&long_result),
        );
        assert_eq!(
            extracted_preview.detail.as_deref().unwrap().lines().count(),
            RESULT_PREVIEW_LINES
        );

        let view = ToolRowView::from_parts("read_file", "succeeded", Some("src/main.rs"));
        assert_eq!(view.name, "read_file");
        assert_eq!(view.headline, "read_file");
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
            "3 tools · read_file · bash · edit_file · 1 completed · 1 running · 1 failed"
        );
        let mut four = rows.clone();
        four.push(ToolRowView::from_parts("find_files", "succeeded", None));
        assert_eq!(
            tool_group_summary(&four),
            "4 tools · read_file · bash · edit_file · … · 2 completed · 1 running · 1 failed"
        );
        let single = ToolRowView::from_facts(
            "write_file",
            "succeeded",
            Some(r#"{"path":"src/new_feature.rs"}"#),
            Some("ok"),
        );
        assert_eq!(
            tool_group_summary(&[single]),
            "write_file src/new_feature.rs"
        );
        let running = ToolRowView::from_facts(
            "run_command",
            "running",
            Some(r#"{"command":"cargo test"}"#),
            None,
        );
        assert_eq!(
            tool_group_summary(&[running]),
            "run_command cargo test · 1 running"
        );
    }

    #[test]
    fn assistant_is_streaming_requires_active_run_and_streaming_phase() {
        let streaming = TimelineEntry {
            sequence: 2,
            event_id: "a1".into(),
            kind: TimelineEntryKind::AssistantMessage {
                text: "hello".into(),
            },
            fork_boundary: None,
            timestamp: "1".into(),
            run_id: Some("run-1".into()),
        };
        let phase = TimelineEntry {
            sequence: 1,
            event_id: "r1".into(),
            kind: TimelineEntryKind::RunState("run streaming_response".into()),
            fork_boundary: None,
            timestamp: "1".into(),
            run_id: Some("run-1".into()),
        };
        let timeline = vec![phase.clone(), streaming.clone()];
        assert!(assistant_is_streaming(&timeline, &streaming, Some("run-1")));
        assert!(!assistant_is_streaming(&timeline, &streaming, None));
        let committed_phase = TimelineEntry {
            sequence: 3,
            event_id: "r2".into(),
            kind: TimelineEntryKind::RunState("run completed".into()),
            fork_boundary: Some(crate::projection::ForkBoundary::Completed),
            timestamp: "2".into(),
            run_id: Some("run-1".into()),
        };
        let done = vec![phase, streaming.clone(), committed_phase];
        assert!(!assistant_is_streaming(&done, &streaming, Some("run-1")));
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

    /// GUI3-04：认证类原因命中供应商设置；timeout / 其它不命中。UI 启发式。
    #[test]
    fn failure_next_step_classifies_auth_keywords() {
        for reason in [
            "HTTP 401",
            "http 401 unauthorized",
            "403",
            "Authentication required",
            "AUTHENTICATION failed",
            "unauthorized",
            "Unauthorized",
            "Invalid API Key",
            "invalid api key from provider",
        ] {
            assert_eq!(
                failure_next_step(reason),
                FailureNextStep::OpenProviderSettings,
                "{reason}"
            );
        }
        for reason in [
            "provider timeout",
            "The run failed.",
            "rate limited",
            "internal error",
            "api key invalid",
            "",
        ] {
            assert_eq!(failure_next_step(reason), FailureNextStep::None, "{reason}");
        }
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
