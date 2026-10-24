//! Inspector「子代理」对话栏：Activity 浮层行点击或「+」菜单进入，展示
//! 被选子代理的运行过程与其和主代理的对话内容。
//! 数据同源：代理列表 / 状态 / 结果来自 Host subagent_list，过程时间线
//! 来自子会话 session_get 分页 + live 事件（独立 protocol reducer），
//! 不与主 Timeline 共享状态。

use gpui::{div, prelude::*, px, Context, Rgba, SharedString, Window};

use crate::projection::{SubagentConversationState, SubagentInfo, TimelineEntryKind};
use crate::ui::accessibility::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::empty_state::EmptyState;
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::icon::{icon_sized, Icon};
use crate::ui::components::label::Label;
use crate::ui::i18n::t;
use crate::ui::theme::{dark, font, metrics};

use super::AppView;

/// 长回执默认折叠阈值（RV-05：字符数 / 行数任一超限即折叠为摘要行）。
const SUBAGENT_RESULT_COLLAPSE_CHARS: usize = 600;
const SUBAGENT_RESULT_COLLAPSE_LINES: usize = 10;
/// AX 正文值截断长度（过长 value 不利读屏逐行播报）。
const SUBAGENT_AX_TEXT_CHARS: usize = 300;
/// 对话栏渲染项（从子会话时间线条目预收集，避免渲染中混用可变借用）。
#[derive(Clone, Debug, PartialEq, Eq)]
enum PanelItem {
    UserMessage {
        event_id: String,
        text: String,
    },
    UserFollowUp {
        event_id: String,
        text: String,
    },
    Assistant {
        event_id: String,
        text: String,
    },
    Thinking {
        event_id: String,
        text: String,
    },
    Tool {
        event_id: String,
        name: String,
        status: String,
        detail: Option<String>,
        arguments: Option<String>,
    },
    RunState(String),
    Error(String),
}

impl PanelItem {
    /// 布局 / AX 键（RunState / Error 为过程装饰行，不发布节点）。
    fn event_id(&self) -> Option<&str> {
        match self {
            PanelItem::UserMessage { event_id, .. }
            | PanelItem::UserFollowUp { event_id, .. }
            | PanelItem::Assistant { event_id, .. }
            | PanelItem::Thinking { event_id, .. }
            | PanelItem::Tool { event_id, .. } => Some(event_id),
            PanelItem::RunState(_) | PanelItem::Error(_) => None,
        }
    }
}

fn panel_items(conversation: &SubagentConversationState) -> Vec<PanelItem> {
    let mut first_user_seen = false;
    conversation
        .timeline
        .iter()
        .map(|entry| match entry.kind.clone() {
            TimelineEntryKind::UserMessage { text } => {
                let item = if first_user_seen {
                    PanelItem::UserFollowUp {
                        event_id: entry.event_id.clone(),
                        text,
                    }
                } else {
                    first_user_seen = true;
                    PanelItem::UserMessage {
                        event_id: entry.event_id.clone(),
                        text,
                    }
                };
                item
            }
            TimelineEntryKind::AssistantMessage { text } => PanelItem::Assistant {
                event_id: entry.event_id.clone(),
                text,
            },
            TimelineEntryKind::Thinking { text } => PanelItem::Thinking {
                event_id: entry.event_id.clone(),
                text,
            },
            TimelineEntryKind::ToolCall {
                name,
                status,
                detail,
                arguments,
            } => PanelItem::Tool {
                event_id: entry.event_id.clone(),
                name,
                status,
                detail,
                arguments,
            },
            TimelineEntryKind::RunState(state) => PanelItem::RunState(state),
            TimelineEntryKind::Error(text) => PanelItem::Error(text),
        })
        .collect()
}

/// 长回执折叠时的摘要行（RV-05）：首个非空行，截断到固定字符数。
fn subagent_result_summary(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut chars = first.chars();
    let head: String = chars.by_ref().take(120).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn subagent_result_is_long(text: &str) -> bool {
    text.chars().count() > SUBAGENT_RESULT_COLLAPSE_CHARS
        || text.lines().count() > SUBAGENT_RESULT_COLLAPSE_LINES
}

/// Host 回执取最后一条非空助手正文（最多 2000 字符），已有正文时不再重复。
fn subagent_result_matches_reply(result: &str, conversation: &SubagentConversationState) -> bool {
    conversation
        .timeline
        .iter()
        .rev()
        .find_map(|entry| {
            if let TimelineEntryKind::AssistantMessage { text } = &entry.kind {
                (!text.trim().is_empty()).then_some(text.trim())
            } else {
                None
            }
        })
        .is_some_and(|text| {
            text == result.trim()
                || (result.chars().count() == 2000 && text.starts_with(result.trim()))
        })
}

fn subagent_run_label(state: &str) -> String {
    let (phase, reason) = state.split_once(" · ").unwrap_or((state, ""));
    let label = match phase {
        "run started" => t("run.phase_started"),
        "run preparing_context" => t("run.phase_preparing_context"),
        "run waiting_for_provider" => t("run.phase_waiting_for_provider"),
        "run streaming_response" => t("run.phase_streaming_response"),
        "run collecting_tool_calls" => t("run.phase_collecting_tool_calls"),
        "run waiting_for_approval" => t("run.phase_waiting_for_approval"),
        "run executing_tools" => t("run.phase_executing_tools"),
        "run appending_tool_results" => t("run.phase_appending_tool_results"),
        "run completed" => t("run.footer_completed"),
        "run cancelled" => t("run.footer_cancelled"),
        "run failed" => t("run.footer_failed"),
        "run interrupted" => t("run.phase_interrupted"),
        _ => return state.to_string(),
    };
    if reason.is_empty() {
        label.to_string()
    } else {
        format!("{label} · {reason}")
    }
}

/// AX 值截断（保留前 N 字符 + 省略号，不伪造内容）。
fn subagent_ax_text(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(SUBAGENT_AX_TEXT_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// tracked 布局相交视口后的 AX 矩形；零面积（未进入视口 / 未测量）
/// 不发布（RV-07：几何不伪造）。
fn subagent_ax_rect(rect: gpui::Bounds<gpui::Pixels>) -> Option<AxRect> {
    if rect.size.width <= gpui::px(0.0) || rect.size.height <= gpui::px(0.0) {
        return None;
    }
    Some(AxRect::new(
        rect.origin.x.into(),
        rect.origin.y.into(),
        rect.size.width.into(),
        rect.size.height.into(),
    ))
}

/// 工具结果原文。空结果用与主 Timeline 相同的占位，不编造内容。
fn subagent_tool_detail_text(detail: Option<&str>) -> String {
    detail
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| t("tool.result_missing").into())
}

fn subagent_tool_result_layout_key(event_id: &str) -> String {
    format!(
        "{}:toggle",
        super::timeline_entry::tool_result_expand_key(event_id)
    )
}

fn subagent_item_focus_handle(
    view: &mut AppView,
    cx: &mut Context<AppView>,
    key: &str,
) -> gpui::FocusHandle {
    view.subagent_item_focus
        .entry(key.to_string())
        .or_insert_with(|| {
            cx.focus_handle()
                .tab_stop(true)
                .tab_index(super::INSPECTOR_TAB_INDEX)
        })
        .clone()
}

fn subagent_item_focused(view: &AppView, window: &Window, key: &str) -> bool {
    view.open_menu.is_none()
        && view
            .subagent_item_focus
            .get(key)
            .is_some_and(|focus| focus.is_focused(window))
}

/// 焦点 key → 实测布局 key。工具标题、结果展开按钮和结果正文不共用一块 bounds。
fn subagent_focus_layout_key(focus_key: &str) -> String {
    if let Some(event_id) = focus_key.strip_suffix(":tool") {
        format!("{event_id}:tool-header")
    } else if let Some(agent_id) = focus_key.strip_suffix(":result-text") {
        format!("{agent_id}:result")
    } else if focus_key.ends_with(":result") {
        format!("{focus_key}:toggle")
    } else {
        focus_key.to_string()
    }
}

/// 把已聚焦条目滚进 transcript 视口。None 表示布局还没测到。
/// Some(true) 表示这次改了滚动偏移。
fn reveal_subagent_layout(scroll: &gpui::ScrollHandle, item: &gpui::ScrollHandle) -> Option<bool> {
    let viewport = scroll.bounds();
    let item_bounds = item.bounds();
    let viewport_h = f32::from(viewport.size.height);
    let item_h = f32::from(item_bounds.size.height);
    let item_w = f32::from(item_bounds.size.width);
    if viewport_h <= 1.0 || item_h <= 1.0 || item_w <= 1.0 {
        return None;
    }
    let viewport_top = f32::from(viewport.origin.y);
    let viewport_bottom = viewport_top + viewport_h;
    let item_top = f32::from(item_bounds.origin.y);
    let item_bottom = item_top + item_h;
    const EDGE: f32 = 0.5;
    if item_top >= viewport_top - EDGE && item_bottom <= viewport_bottom + EDGE {
        return Some(false);
    }
    let delta = if item_h >= viewport_h || item_top < viewport_top {
        viewport_top - item_top
    } else {
        viewport_bottom - item_bottom
    };
    let mut offset = scroll.offset();
    let max = f32::from(scroll.max_offset().height);
    let y = (f32::from(offset.y) + delta).clamp(-max, 0.0);
    offset.y = px(y);
    scroll.set_offset(offset);
    Some(true)
}

/// 本帧 prepaint 落定后，条目是否与 transcript 视口有正面积相交。
/// None 表示视口或条目还没测到，调用方不得据此 notify。
fn subagent_layout_settled_visible(
    scroll: &gpui::ScrollHandle,
    item: &gpui::ScrollHandle,
) -> Option<bool> {
    let viewport = scroll.bounds();
    let item_bounds = item.bounds();
    if f32::from(viewport.size.width) <= 1.0
        || f32::from(viewport.size.height) <= 1.0
        || f32::from(item_bounds.size.width) <= 1.0
        || f32::from(item_bounds.size.height) <= 1.0
    {
        return None;
    }
    let visible = item_bounds.intersect(&viewport);
    Some(f32::from(visible.size.width) > 0.0 && f32::from(visible.size.height) > 0.0)
}

/// 代理标题截断（chip 与列表行共用口径）。
pub(super) fn short_title(title: &str) -> String {
    title.chars().take(24).collect()
}

/// 上一帧 chip 行视口留给标题的宽度。首帧视口为 0 时不钳制。
/// 渲染读不到本帧 prepaint 才写入的宽度；调用方在布局结束后比较，
/// 差超过 0.5px 时 notify 一次，下一帧用新宽度，宽度不动就停。
/// 水平 chrome 是 Button px_2（左右各 0.5rem）、标题与状态点的 gap_1，
/// 再加上 Ø6 状态点。文字至少留 12px，避免宽度被收成不可读的一条。
fn subagent_chip_text_max(window: &Window, port_width: f32) -> Option<f32> {
    if port_width <= 0.0 {
        return None;
    }
    let rem = f32::from(window.rem_size());
    let chrome = rem + rem * 0.25 + 6.0;
    Some((port_width - chrome).max(12.0))
}

/// 标题列宽：完整 short_title 的测量宽向上取整。等于字形宽时 shape_text
/// 会把最后一个字形折到下一行，而元素仍按一行排，高度对不上。
/// 超出视口文字区时改用文字区宽并换行。
fn subagent_chip_label_width(window: &Window, title: &str, text_max: Option<f32>) -> f32 {
    let full = super::input_area::shape_label_width(window, title, font::BASE).ceil();
    match text_max {
        Some(max) => {
            if full <= max {
                full
            } else {
                max
            }
        }
        None => full,
    }
}

/// 子代理状态 → chip 语义点颜色（与 Header 状态点 / TaskRail 同族映射：
/// 运行蓝、等待琥珀、完成绿、失败红、其余弱化）。
fn subagent_status_color(status: &str) -> Rgba {
    match status {
        "running" => dark().accent.primary,
        "waiting" => dark().semantic.warning_text,
        "completed" => dark().semantic.success_fg,
        "failed" => dark().semantic.danger_text,
        _ => dark().text.tertiary,
    }
}

impl AppView {
    /// focus_next / AX Focus 都不滚动。正文获得焦点后，用上一帧布局尝试
    /// 把对应条目滚进 transcript。gpui 0.2.2 的 set_offset 清不掉
    /// scroll_to_bottom，同帧 follow_new_content 会在 prepaint 里盖掉这次
    /// 偏移，所以这里不登记成功，也不按中间偏移重估 following。defer 等到
    /// 本帧滚动落定：正面积相交才登记并按最终偏移重估；被盖掉或再次离屏
    /// 则不登记，notify 让下一帧重试。未测量不 notify。同一焦点确认可见后
    /// 不反复抢手动滚动。高于视口的条目只要相交就算可见，避免死循环。
    fn reveal_focused_subagent_body(&mut self, window: &Window, cx: &mut Context<Self>) {
        let focused = self
            .subagent_item_focus
            .iter()
            .find_map(|(key, handle)| handle.is_focused(window).then(|| key.clone()));
        let Some(focus_key) = focused else {
            self.subagent_revealed_focus = None;
            return;
        };
        if self.subagent_revealed_focus.as_deref() == Some(focus_key.as_str()) {
            return;
        }
        let layout_key = subagent_focus_layout_key(&focus_key);
        let Some(item) = self.subagent_item_layouts.get(&layout_key).cloned() else {
            return;
        };
        if reveal_subagent_layout(self.subagent_conversation_scroll.handle(), &item).is_none() {
            return;
        }
        cx.defer_in(window, move |view, window, cx| {
            let still_focused = view
                .subagent_item_focus
                .iter()
                .any(|(key, handle)| key == &focus_key && handle.is_focused(window));
            if !still_focused || view.subagent_revealed_focus.as_deref() == Some(focus_key.as_str())
            {
                return;
            }
            let layout_key = subagent_focus_layout_key(&focus_key);
            let Some(item) = view.subagent_item_layouts.get(&layout_key).cloned() else {
                return;
            };
            match subagent_layout_settled_visible(view.subagent_conversation_scroll.handle(), &item)
            {
                Some(true) => {
                    view.subagent_revealed_focus = Some(focus_key);
                    let was_following = view.subagent_conversation_scroll.is_following();
                    view.subagent_conversation_scroll.sync_following();
                    if view.subagent_conversation_scroll.is_following() != was_following {
                        cx.notify();
                    }
                }
                Some(false) => cx.notify(),
                None => {}
            }
        });
    }

    pub(super) fn can_refresh_subagents(&self) -> bool {
        self.projection.active_session_id.is_some()
            && matches!(
                self.projection.connection,
                crate::projection::ConnectionState::Connected { .. }
            )
            && !self.projection.subagent_activity.loading
    }

    pub(super) fn can_cancel_subagent(&self) -> bool {
        let state = &self.projection.subagent_activity;
        matches!(
            self.projection.connection,
            crate::projection::ConnectionState::Connected { .. }
        ) && self.projection.active_session_id == state.session_id
            && state.cancelling.is_none()
            && state.agents.iter().any(|agent| {
                Some(&agent.agent_id) == self.projection.subagent_conversation.agent_id.as_ref()
                    && matches!(agent.status.as_str(), "running" | "waiting")
            })
    }

    pub(super) fn cancel_selected_subagent(&mut self, cx: &mut Context<Self>) {
        if !self.can_cancel_subagent() {
            return;
        }
        let session_id = self
            .projection
            .active_session_id
            .clone()
            .expect("bound session");
        let agent_id = self
            .projection
            .subagent_conversation
            .agent_id
            .clone()
            .expect("selected agent");
        if self
            .controller
            .cancel_subagent(session_id, agent_id.clone())
        {
            self.projection.subagent_activity.cancelling = Some(agent_id);
            self.projection.subagent_activity.error = None;
        }
        cx.notify();
    }

    /// 子代理对话栏展开偏好切换（RV-04 工具行 / RV-05 长回执共用；独立
    /// 于主 Timeline 的 expanded_timeline_details，不污染其折叠态）。
    pub(super) fn toggle_subagent_detail(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.expanded_subagent_details.remove(key) {
            self.expanded_subagent_details.insert(key.to_string());
        }
        cx.notify();
    }

    /// 正文 StaticText 与展开按钮的 GPUI 焦点。AX Focus 与 Tab 链共用这些句柄。
    pub(super) fn subagent_focus_for_identifier(
        &self,
        identifier: &str,
    ) -> Option<&gpui::FocusHandle> {
        let key = if let Some(id) = identifier.strip_prefix("subagent-tool-result-toggle-") {
            super::timeline_entry::tool_result_expand_key(id)
        } else if let Some(id) = identifier.strip_prefix("subagent-tool-toggle-") {
            format!("{id}:tool")
        } else if let Some(id) = identifier.strip_prefix("subagent-result-toggle-") {
            format!("{id}:result")
        } else if let Some(id) = identifier.strip_prefix("subagent-result-") {
            format!("{id}:result-text")
        } else if let Some(id) = identifier.strip_prefix("subagent-msg-") {
            format!("{id}:item")
        } else {
            return None;
        };
        self.subagent_item_focus.get(&key)
    }

    pub(super) fn subagent_conversation_element(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_refresh = self.can_refresh_subagents();
        let can_cancel = self.can_cancel_subagent();
        let activity = &self.projection.subagent_activity;
        let activity_error = activity.error.clone();
        let activity_stale = activity.stale_reason.clone();
        let cancel_pending = activity.cancelling.is_some()
            && activity.cancelling == self.projection.subagent_conversation.agent_id;
        let bound = activity.session_id.is_some()
            && self.projection.active_session_id == activity.session_id;
        let selected_id = self.projection.subagent_conversation.agent_id.clone();
        // display_agents 返回借用引用；收集 owned 副本切断对 self 的不可变
        // 借用，后续渲染调用（&mut self）才能通过借用检查。
        let agents: Vec<SubagentInfo> = if bound {
            activity.display_agents().into_iter().cloned().collect()
        } else {
            Vec::new()
        };
        let selected: Option<SubagentInfo> = selected_id
            .as_deref()
            .and_then(|id| agents.iter().cloned().find(|agent| agent.agent_id == id));
        let conversation = self.projection.subagent_conversation.clone();
        let items = panel_items(&conversation);
        // 焦点句柄 / 实测布局随可见代理收敛，不跨会话累积。
        let visible_ids: std::collections::HashSet<&String> =
            agents.iter().map(|agent| &agent.agent_id).collect();
        self.subagent_agent_focus
            .retain(|id, _| visible_ids.contains(id));
        self.subagent_agent_layouts
            .retain(|id, _| visible_ids.contains(id));

        // 顶栏：代理切换 chip（滚动）+ Refresh（同主 Timeline 的权威重查）。
        let mut chips = div()
            .id("subagent-agent-chips")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.subagent_chips_scroll);
        if agents.is_empty() {
            // 空态映射（RV-11）：只有查询真在进行中才显示「加载中」；
            // 断线 / 从未取到数据时如实提示内容可能过期，不伪造持续加载。
            let empty_label = if !bound && self.projection.active_session_id.is_none() {
                t("subagents.no_session")
            } else if activity.loading {
                t("subagents.loading")
            } else if activity.stale_reason.is_some()
                || !matches!(
                    self.projection.connection,
                    crate::projection::ConnectionState::Connected { .. }
                )
            {
                t("subagents.stale_banner")
            } else {
                t("subagents.empty")
            };
            chips = chips.child(
                div()
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(empty_label),
            );
        }
        // 视口宽来自上一帧。首帧为 0 时不钳制；默认 flex_shrink 会把超宽
        // chip 压扁，所以每枚 chip 用 flex_none，放不下就横滑。
        // bounds 更新不会自动重绘。本帧 prepaint 写入新宽度后，若与这次
        // 测量差超过 0.5px，defer notify 一次：真窗口下一帧、测试的 effect
        // flush 都会用新宽度再画。这次若消费了 reveal，说明滚动是按旧宽度
        // 算的，下一帧再 reveal 一次；宽度不动就停，不逐帧循环。
        let port_width = f32::from(self.subagent_chips_scroll.bounds().size.width);
        let text_max = subagent_chip_text_max(window, port_width);
        let reveal_for_layout = self.subagent_reveal_selected;
        cx.defer_in(window, move |view, _window, cx| {
            let laid_out = f32::from(view.subagent_chips_scroll.bounds().size.width);
            if (laid_out - port_width).abs() > 0.5 {
                if reveal_for_layout {
                    view.subagent_reveal_selected = true;
                }
                cx.notify();
            }
        });
        for agent in &agents {
            let is_selected = Some(agent.agent_id.as_str()) == selected_id.as_deref();
            let focus = self
                .subagent_agent_focus
                .entry(agent.agent_id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let layout = self
                .subagent_agent_layouts
                .entry(agent.agent_id.clone())
                .or_insert_with(gpui::ScrollHandle::new)
                .clone();
            let status_label = super::changes::subagent_status_label(&agent.status);
            let tooltip = format!("{} · {} · {}", agent.title, agent.model_id, status_label);
            let click_id = agent.agent_id.clone();
            let activate_id = agent.agent_id.clone();
            let selector_id = agent.agent_id.clone();
            let dot_color = subagent_status_color(&agent.status);
            let title = short_title(&agent.title);
            let label_width = subagent_chip_label_width(window, &title, text_max);
            chips = chips.child(
                div()
                    .id(SharedString::from(format!(
                        "subagent-chip-layout-{}",
                        agent.agent_id
                    )))
                    .flex_none()
                    .track_scroll(&layout)
                    .debug_selector({
                        let selector_id = selector_id.clone();
                        move || format!("subagent-chip-{selector_id}")
                    })
                    .child(
                        Button::new(format!("subagent-select-{}", agent.agent_id))
                            .disabled(!matches!(
                                self.projection.connection,
                                crate::projection::ConnectionState::Connected { .. }
                            ))
                            .track_focus(&focus)
                            .variant(if is_selected {
                                ButtonVariant::Raised
                            } else {
                                ButtonVariant::Ghost
                            })
                            .radius(6.0)
                            .text_size(font::BASE)
                            // 竞品共识（Claude Code Agent map / Cursor
                            // Agents Window）：并发列表每项带状态点；
                            // chip 名称前缀 Ø6 语义点，颜色同 Header 映射。
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .w(px(6.0))
                                            .h(px(6.0))
                                            .rounded_full()
                                            .flex_none()
                                            .bg(dot_color)
                                            .debug_selector({
                                                let selector_id = selector_id.clone();
                                                move || format!("subagent-dot-{selector_id}")
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(px(label_width))
                                            .debug_selector({
                                                let selector_id = selector_id.clone();
                                                move || format!("subagent-label-{selector_id}")
                                            })
                                            .child(title),
                                    ),
                            )
                            .tooltip(tooltip)
                            .on_click(cx.listener(move |view, event, _window, cx| {
                                let id = format!("subagent-select-{click_id}");
                                if view.consume_button_key_click(&id, event) {
                                    return;
                                }
                                view.select_subagent_agent(&click_id, cx);
                            }))
                            .on_activate(cx.listener(move |view, _event, _window, cx| {
                                view.note_button_key_activate(&format!(
                                    "subagent-select-{activate_id}",
                                ));
                                view.select_subagent_agent(&activate_id, cx);
                                cx.stop_propagation();
                            })),
                    ),
            );
        }
        // 选中 chip 滚入视口（与 Inspector 页签 reveal 同口径）：代理多时
        // 选中项可能在横滑视口外，切换后按需 reveal，不逐帧抢滚动位置。
        if std::mem::take(&mut self.subagent_reveal_selected) {
            if let Some(index) = agents
                .iter()
                .position(|agent| Some(agent.agent_id.as_str()) == selected_id.as_deref())
            {
                self.subagent_chips_scroll.scroll_to_item(index);
            }
        }
        let cancel_focus = self
            .settings_action_focus
            .entry("subagent-cancel".into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let show_cancel = selected
            .as_ref()
            .is_some_and(|a| matches!(a.status.as_str(), "running" | "waiting"));
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(chips)
            .when(show_cancel, |header| {
                header.child(
                    self.settings_element("subagent-cancel").child(
                        Button::new("subagent-cancel")
                            .track_focus(&cancel_focus)
                            .variant(ButtonVariant::Ghost)
                            .disabled(!can_cancel)
                            .child(t(if cancel_pending {
                                "subagents.cancelling"
                            } else {
                                "subagents.cancel"
                            }))
                            .on_click(cx.listener(|view, event, _, cx| {
                                if !view.consume_button_key_click("subagent-cancel", event) {
                                    view.cancel_selected_subagent(cx);
                                }
                            }))
                            .on_activate(cx.listener(|view, _, _, cx| {
                                view.note_button_key_activate("subagent-cancel");
                                view.cancel_selected_subagent(cx);
                                cx.stop_propagation();
                            })),
                    ),
                )
            })
            .child(
                div()
                    .id("subagent-refresh-layout")
                    .track_scroll(&self.subagent_refresh_layout)
                    .child(
                        Button::new("subagent-refresh")
                            .disabled(!can_refresh)
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                            .text_color(dark().text.secondary)
                            .child(icon_sized(Icon::Refresh, px(metrics::ICON_SM)))
                            .tooltip(t("subagents.refresh_tooltip"))
                            .track_focus(&self.subagent_refresh_focus)
                            .on_click(cx.listener(|view, event, _window, cx| {
                                if view.consume_button_key_click("subagent-refresh", event) {
                                    return;
                                }
                                view.refresh_subagent_conversation();
                                cx.notify();
                            }))
                            .on_activate(cx.listener(|view, _event, _window, cx| {
                                view.note_button_key_activate("subagent-refresh");
                                view.refresh_subagent_conversation();
                                cx.notify();
                                cx.stop_propagation();
                            })),
                    ),
            );

        let body =
            self.subagent_transcript_element(selected.as_ref(), &conversation, items, window, cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .when_some(activity_error, |panel, error| {
                panel.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(font::BASE)
                        .line_height(font::CONTROL_LINE_HEIGHT)
                        .text_color(dark().semantic.danger_text)
                        .child(error),
                )
            })
            .when_some(activity_stale, |panel, reason| {
                panel.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(font::BASE)
                        .line_height(font::CONTROL_LINE_HEIGHT)
                        .text_color(dark().text.secondary)
                        // RV-11：断线保留内容但如实标注过期（本地化前缀 +
                        // 原始 reason；数据内容不翻译）。
                        .child(format!("{} · {reason}", t("subagents.stale_banner"))),
                )
            })
            .child(body)
    }

    /// 对话主体：未选代理给入口提示；已选给 元信息 + 主→子任务盒 +
    /// 子代理过程 + 子→主结果盒。loading / error 如实分层，不伪造内容。
    fn subagent_transcript_element(
        &mut self,
        selected: Option<&SubagentInfo>,
        conversation: &SubagentConversationState,
        items: Vec<PanelItem>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(agent) = selected else {
            // 已选代理但列表尚未返回该行（加载中 / 旧快照）：显示
            // loading，不误报「未选择代理」。
            if self.projection.subagent_conversation.agent_id.is_some() {
                return subagent_placeholder(
                    t("subagents.loading"),
                    t("subagents.panel_empty_desc").to_string(),
                );
            }
            return subagent_placeholder(
                t("subagents.panel_empty"),
                t("subagents.panel_empty_desc").to_string(),
            );
        };
        self.reveal_focused_subagent_body(window, cx);
        let mut transcript = div()
            .id("subagent-transcript")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(self.subagent_conversation_scroll.handle())
            .px_2()
            .py_2()
            .gap_2()
            .on_scroll_wheel(cx.listener(|view, _, _, cx| {
                view.subagent_conversation_scroll.on_scroll_wheel();
                cx.notify();
            }));
        // 布局句柄随可见条目收敛（切换代理 / 重查后旧代理的 event_id 不
        // 再保留）；长回执展开入口句柄仅在折叠阈值以上时存在（RV-07）。
        let mut visible_keys: std::collections::HashSet<String> = items
            .iter()
            .filter_map(|item| match item {
                PanelItem::Tool { event_id, .. } => Some(format!("{event_id}:tool-header")),
                other => other.event_id().map(|id| format!("{id}:item")),
            })
            .collect();
        let result_repeated = agent
            .result
            .as_deref()
            .is_some_and(|result| subagent_result_matches_reply(result, conversation));
        if let Some(result) = &agent.result {
            let result_key = format!("{}:result", agent.agent_id);
            visible_keys.insert(result_key.clone());
            if !result_repeated && subagent_result_is_long(result) {
                visible_keys.insert(format!("{result_key}:toggle"));
            }
        }
        for item in &items {
            if let PanelItem::Tool {
                event_id, detail, ..
            } = item
            {
                if self
                    .expanded_subagent_details
                    .contains(&format!("{event_id}:tool"))
                    && super::timeline_entry::preview_result_text(
                        &subagent_tool_detail_text(detail.as_deref()),
                        false,
                    )
                    .1
                {
                    visible_keys.insert(subagent_tool_result_layout_key(event_id));
                }
            }
        }
        self.subagent_item_layouts
            .retain(|key, _| visible_keys.contains(key));
        if conversation.loading && items.is_empty() {
            transcript = transcript.child(
                div()
                    .text_size(font::BASE)
                    .line_height(font::CONTROL_LINE_HEIGHT)
                    .text_color(dark().text.secondary)
                    .child(t("subagents.loading")),
            );
            return transcript.into_any_element();
        }
        if let Some(reason) = &conversation.error {
            if items.is_empty() {
                return subagent_placeholder(t("subagents.load_failed"), reason.clone());
            }
            transcript = transcript.child(
                div()
                    .text_size(font::XS)
                    .text_color(dark().semantic.danger_text)
                    .child(format!("{} · {reason}", t("subagents.load_failed"))),
            );
        }
        // 标题完整换行，模型与状态另起一行，避免大字号下标题被硬切。
        let status = super::changes::subagent_status_label(&agent.status);
        transcript = transcript.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div().flex().flex_row().child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(font::BODY)
                            .line_height(font::SECTION_LINE_HEIGHT)
                            .child(agent.title.clone()),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Label::new(format!(
                                "{} / {} · {}",
                                agent.provider_id,
                                agent.model_id,
                                agent
                                    .effort
                                    .as_deref()
                                    .unwrap_or(t("settings.subagents.effort_default"))
                            ))
                            .size(font::XS)
                            .color(dark().text.tertiary),
                        )
                        .child(
                            Label::new(status)
                                .size(font::XS)
                                .color(dark().text.secondary),
                        ),
                ),
        );
        for item in items {
            transcript = transcript.child(self.subagent_panel_item_element(item, window, cx));
        }
        // 结果盒（主代理视角的子代理回执；completed/failed 才有）。
        if let Some(result) = &agent.result {
            // RV-05：与正文同源 Markdown 渲染；长回执默认折叠为摘要行 +
            // 展开入口，展开后全文随外层 transcript 滚动（内嵌滚动区会
            // 吃滚轮，指针停在上面时外层脱钩/跟滚失效）。
            let result_key = format!("{}:result", agent.agent_id);
            let long = !result_repeated && subagent_result_is_long(result);
            let expanded =
                !result_repeated && (!long || self.expanded_subagent_details.contains(&result_key));
            let result_layout = self
                .subagent_item_layouts
                .entry(result_key.clone())
                .or_insert_with(gpui::ScrollHandle::new)
                .clone();
            let mut result_box = div()
                .id(SharedString::from(format!(
                    "subagent-item-{}-result",
                    agent.agent_id
                )))
                .track_scroll(&result_layout)
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .border_1()
                .border_color(dark().border.subtle)
                .rounded(px(4.0))
                .bg(dark().surface.raised)
                .child(
                    Label::new(t("subagents.result_to_main"))
                        .size(font::XS)
                        .color(dark().text.secondary),
                );
            if long {
                result_box = result_box.child(self.subagent_result_toggle_element(
                    &agent.agent_id,
                    &result_key,
                    expanded,
                    cx,
                ));
            }
            let result_text_focus =
                subagent_item_focus_handle(self, cx, &format!("{}:result-text", agent.agent_id));
            let result_body = if result_repeated {
                div()
                    .text_size(font::BASE)
                    .line_height(font::CONTROL_LINE_HEIGHT)
                    .text_color(dark().text.secondary)
                    .child(t("subagents.result_in_transcript"))
                    .into_any_element()
            } else if expanded {
                super::markdown::message_body_element(
                    self,
                    cx,
                    window,
                    &format!("subagent-result-{}", agent.agent_id),
                    result,
                    dark().text.primary,
                    false,
                )
                .into_any_element()
            } else {
                div()
                    .text_size(font::BASE)
                    .line_height(font::CONTROL_LINE_HEIGHT)
                    .text_color(dark().text.secondary)
                    .child(subagent_result_summary(result))
                    .into_any_element()
            };
            result_box = result_box.child(
                div()
                    .id(SharedString::from(format!(
                        "subagent-result-text-{}",
                        agent.agent_id
                    )))
                    .track_focus(&result_text_focus)
                    .child(result_body),
            );
            transcript = transcript.child(result_box);
        }
        transcript
            // 回底浮出条件与主 Timeline 同口径：脱钩且内容真溢出（短对话
            // 脱钩时不盖正文；max_offset 未测得时保守不显示）。
            .when(
                !self.subagent_conversation_scroll.is_following()
                    && f32::from(
                        self.subagent_conversation_scroll
                            .handle()
                            .max_offset()
                            .height,
                    ) > metrics::SCROLL_EPSILON,
                |area| {
                    area.child(BackToBottom::new(
                        Button::new("subagent-back-to-bottom")
                            .variant(ButtonVariant::Raised)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(icon_sized(Icon::ArrowDown, px(metrics::ICON_SM)))
                                    .child(t("timeline.ax_back_to_bottom")),
                            )
                            .track_focus(&self.subagent_back_to_bottom_focus)
                            .on_click(cx.listener(|view, event, _, cx| {
                                if view.consume_button_key_click("subagent-back-to-bottom", event) {
                                    return;
                                }
                                view.subagent_conversation_scroll.jump_to_bottom();
                                cx.notify();
                            }))
                            .on_activate(cx.listener(|view, _event, _, cx| {
                                view.note_button_key_activate("subagent-back-to-bottom");
                                view.subagent_conversation_scroll.jump_to_bottom();
                                cx.notify();
                                cx.stop_propagation();
                            })),
                    ))
                },
            )
            .into_any_element()
    }

    fn subagent_panel_item_element(
        &mut self,
        item: PanelItem,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match item {
            // 主代理 → 子代理：任务盒（子会话首条 user 消息即 spawn 任务）。
            PanelItem::UserMessage { event_id, text } => {
                let layout = self
                    .subagent_item_layouts
                    .entry(format!("{event_id}:item"))
                    .or_insert_with(gpui::ScrollHandle::new)
                    .clone();
                let focus = subagent_item_focus_handle(self, cx, &format!("{event_id}:item"));
                div()
                    .id(SharedString::from(format!("subagent-item-{event_id}")))
                    .track_focus(&focus)
                    .track_scroll(&layout)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(dark().border.subtle)
                    .rounded(px(4.0))
                    .bg(dark().surface.raised)
                    .child(
                        Label::new(t("subagents.task_from_main"))
                            .size(font::XS)
                            .color(dark().text.secondary),
                    )
                    .child(
                        div()
                            .text_size(font::BASE)
                            .line_height(font::CONTROL_LINE_HEIGHT)
                            .text_color(dark().text.primary)
                            .child(text),
                    )
                    .into_any_element()
            }
            // 后续 user 回合不是 spawn 任务：普通用户消息样式，不冒充
            // 「主代理 → 子代理」任务盒。
            PanelItem::UserFollowUp { event_id, text } => {
                let layout = self
                    .subagent_item_layouts
                    .entry(format!("{event_id}:item"))
                    .or_insert_with(gpui::ScrollHandle::new)
                    .clone();
                let focus = subagent_item_focus_handle(self, cx, &format!("{event_id}:item"));
                div()
                    .id(SharedString::from(format!("subagent-item-{event_id}")))
                    .track_focus(&focus)
                    .track_scroll(&layout)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        Label::new(t("timeline.you"))
                            .size(font::XS)
                            .color(dark().text.tertiary),
                    )
                    .child(
                        div()
                            .text_size(font::BASE)
                            .line_height(font::CONTROL_LINE_HEIGHT)
                            .text_color(dark().text.primary)
                            .child(text),
                    )
                    .into_any_element()
            }
            // 子代理回复：复用主 Timeline 的 Markdown 渲染（代码块 / 列表 /
            // 表格同源，含复制入口）。
            PanelItem::Assistant { event_id, text } => {
                let layout = self
                    .subagent_item_layouts
                    .entry(format!("{event_id}:item"))
                    .or_insert_with(gpui::ScrollHandle::new)
                    .clone();
                let focus = subagent_item_focus_handle(self, cx, &format!("{event_id}:item"));
                div()
                    .id(SharedString::from(format!("subagent-item-{event_id}")))
                    .track_focus(&focus)
                    .track_scroll(&layout)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        Label::new(t("subagents.assistant_label"))
                            .size(font::XS)
                            .color(dark().text.tertiary),
                    )
                    .child(super::markdown::message_body_element(
                        self,
                        cx,
                        window,
                        &format!("subagent-{event_id}"),
                        &text,
                        dark().text.primary,
                        false,
                    ))
                    .into_any_element()
            }
            PanelItem::Thinking { event_id, text } => {
                let layout = self
                    .subagent_item_layouts
                    .entry(format!("{event_id}:item"))
                    .or_insert_with(gpui::ScrollHandle::new)
                    .clone();
                let focus = subagent_item_focus_handle(self, cx, &format!("{event_id}:item"));
                div()
                    .id(SharedString::from(format!("subagent-item-{event_id}")))
                    .track_focus(&focus)
                    .track_scroll(&layout)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pl_2()
                    .border_l_2()
                    .border_color(dark().border.subtle)
                    .child(
                        Label::new(t("subagents.thinking_label"))
                            .size(font::XS)
                            .color(dark().text.tertiary),
                    )
                    .child(
                        div()
                            .text_size(font::BASE)
                            .line_height(font::CONTROL_LINE_HEIGHT)
                            .text_color(dark().text.tertiary)
                            // 思考全文随外层 transcript 滚动（同主 Timeline 展开
                            // 思考盒）：不做内嵌滚动，避免滚轮被内层吃掉。
                            .child(text),
                    )
                    .into_any_element()
            }
            // RV-04 工具行：默认折叠一行（图标 + 目标 + 状态 + chevron），
            // 点击 / Enter 展开检查参数 JSON 与结果 / 错误原文——detail
            // 来自真实事件（含权限拒绝原文），不自行分类编造。
            PanelItem::Tool {
                event_id,
                name,
                status,
                detail,
                arguments,
            } => {
                let tool_key = format!("{event_id}:tool");
                let expanded = self.expanded_subagent_details.contains(&tool_key);
                let result_key = super::timeline_entry::tool_result_expand_key(&event_id);
                let result_expanded = self.expanded_subagent_details.contains(&result_key);
                let headline = super::timeline_entry::tool_headline(&name, arguments.as_deref());
                let status_label = super::timeline::tool_status_label(&status);
                let status_color = match status.as_str() {
                    "failed" | "cancelled" => dark().semantic.danger_text,
                    _ => dark().text.secondary,
                };
                let header_layout = self
                    .subagent_item_layouts
                    .entry(format!("{event_id}:tool-header"))
                    .or_insert_with(gpui::ScrollHandle::new)
                    .clone();
                let focus = subagent_item_focus_handle(self, cx, &tool_key);
                let button_id = format!("subagent-tool-toggle-{event_id}");
                let click_button_id = button_id.clone();
                let click_key = tool_key.clone();
                let activate_button_id = button_id.clone();
                let activate_key = tool_key.clone();
                let mut element = div().flex().flex_col().gap_1().py_1().child(
                    div()
                        .id(SharedString::from(format!(
                            "subagent-tool-header-{event_id}"
                        )))
                        .track_scroll(&header_layout)
                        .child(
                            Button::new(button_id)
                                .variant(ButtonVariant::Ghost)
                                .text_size(font::BASE)
                                .text_color(dark().text.secondary)
                                .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
                                .track_focus(&focus)
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .w(px(12.0))
                                                .flex_none()
                                                .text_color(dark().text.secondary)
                                                .child(if expanded {
                                                    icon_sized(Icon::ChevronDown, px(12.0))
                                                } else {
                                                    icon_sized(Icon::ChevronRight, px(12.0))
                                                }),
                                        )
                                        .child(icon_sized(Icon::Tools, px(metrics::ICON_SM)))
                                        .child(
                                            div()
                                                .flex_row()
                                                .flex_1()
                                                .min_w_0()
                                                .child(div().truncate().child(headline)),
                                        )
                                        .child(
                                            Label::new(status_label)
                                                .size(font::XS)
                                                .color(status_color),
                                        ),
                                )
                                .on_click(cx.listener(move |view, event, _window, cx| {
                                    if view.consume_button_key_click(&click_button_id, event) {
                                        return;
                                    }
                                    view.toggle_subagent_detail(&click_key, cx);
                                }))
                                .on_activate(cx.listener(move |view, _event, _window, cx| {
                                    view.note_button_key_activate(&activate_button_id);
                                    view.toggle_subagent_detail(&activate_key, cx);
                                    cx.stop_propagation();
                                })),
                        ),
                );
                if expanded {
                    let detail_text = subagent_tool_detail_text(detail.as_deref());
                    let (preview, can_expand) =
                        super::timeline_entry::preview_result_text(&detail_text, result_expanded);
                    let mut expanded_area = div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pl_2()
                        .border_l_1()
                        .border_color(dark().border.subtle)
                        // 参数与结果 / 错误原文均按主 Timeline 工具行口径：
                        // 等宽小号，随外层 transcript 滚动。
                        .child(
                            div()
                                .text_size(font::BASE)
                                .line_height(font::DESCRIPTION_LINE_HEIGHT)
                                .font_family(font::MONO)
                                .text_color(dark().text.secondary)
                                .child(super::timeline_entry::arguments_secondary(
                                    arguments.as_deref(),
                                )),
                        )
                        .child(
                            div()
                                .text_size(font::BASE)
                                .line_height(font::DESCRIPTION_LINE_HEIGHT)
                                .font_family(font::MONO)
                                .text_color(dark().text.secondary)
                                .child(preview),
                        );
                    if can_expand {
                        expanded_area =
                            expanded_area.child(self.subagent_tool_result_toggle_element(
                                &event_id,
                                &result_key,
                                result_expanded,
                                cx,
                            ));
                    }
                    element = element.child(expanded_area);
                }
                element.into_any_element()
            }
            PanelItem::RunState(state) => div()
                .text_size(font::XS)
                .text_color(dark().text.tertiary)
                .child(subagent_run_label(&state))
                .into_any_element(),
            PanelItem::Error(text) => div()
                .text_size(font::BASE)
                .line_height(font::CONTROL_LINE_HEIGHT)
                .text_color(dark().semantic.danger_text)
                .child(text)
                .into_any_element(),
        }
    }

    /// RV-05 长回执展开 / 收起（key 独立于主 Timeline 折叠集合，可来回
    /// 切；展开后全文随外层 transcript 滚到底）。
    fn subagent_result_toggle_element(
        &mut self,
        agent_id: &str,
        result_key: &str,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let button_id = format!("subagent-result-toggle-{agent_id}");
        let focus = subagent_item_focus_handle(self, cx, result_key);
        let toggle_layout = self
            .subagent_item_layouts
            .entry(format!("{result_key}:toggle"))
            .or_insert_with(gpui::ScrollHandle::new)
            .clone();
        let click_button_id = button_id.clone();
        let click_key = result_key.to_string();
        let activate_button_id = button_id.clone();
        let activate_key = result_key.to_string();
        div()
            .id("subagent-result-toggle-layout")
            .track_scroll(&toggle_layout)
            .child(
                Button::new(button_id)
                    .variant(ButtonVariant::Ghost)
                    .text_size(font::BASE)
                    .text_color(dark().text.secondary)
                    .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
                    .label(if expanded {
                        t("tool.result_collapse")
                    } else {
                        t("tool.result_expand")
                    })
                    .track_focus(&focus)
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        if view.consume_button_key_click(&click_button_id, event) {
                            return;
                        }
                        view.toggle_subagent_detail(&click_key, cx);
                    }))
                    .on_activate(cx.listener(move |view, _event, _window, cx| {
                        view.note_button_key_activate(&activate_button_id);
                        view.toggle_subagent_detail(&activate_key, cx);
                        cx.stop_propagation();
                    })),
            )
    }

    /// RV-04 工具结果预览「展开全文」（与主 Timeline RESULT_PREVIEW_LINES
    /// 同口径；key 复用 tool_result_expand_key，存面板独立集合）。
    fn subagent_tool_result_toggle_element(
        &mut self,
        event_id: &str,
        result_key: &str,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let button_id = format!("subagent-tool-result-toggle-{event_id}");
        let focus = subagent_item_focus_handle(self, cx, result_key);
        let toggle_layout = self
            .subagent_item_layouts
            .entry(subagent_tool_result_layout_key(event_id))
            .or_insert_with(gpui::ScrollHandle::new)
            .clone();
        let click_button_id = button_id.clone();
        let click_key = result_key.to_string();
        let activate_button_id = button_id.clone();
        let activate_key = result_key.to_string();
        div()
            .id(SharedString::from(format!(
                "subagent-tool-result-toggle-layout-{event_id}"
            )))
            .track_scroll(&toggle_layout)
            .child(
                Button::new(button_id)
                    .variant(ButtonVariant::Ghost)
                    .text_size(font::BASE)
                    .text_color(dark().text.secondary)
                    .padding(ButtonPadding::Horizontal(metrics::PADDING_XS))
                    .label(if expanded {
                        t("tool.result_collapse")
                    } else {
                        t("tool.result_expand")
                    })
                    .track_focus(&focus)
                    .on_click(cx.listener(move |view, event, _window, cx| {
                        if view.consume_button_key_click(&click_button_id, event) {
                            return;
                        }
                        view.toggle_subagent_detail(&click_key, cx);
                    }))
                    .on_activate(cx.listener(move |view, _event, _window, cx| {
                        view.note_button_key_activate(&activate_button_id);
                        view.toggle_subagent_detail(&activate_key, cx);
                        cx.stop_propagation();
                    })),
            )
    }
}

fn subagent_placeholder(title: &'static str, description: String) -> gpui::AnyElement {
    EmptyState::labeled(Icon::Subagents, title, description, dark().text.primary)
        .top_aligned()
        .into_any_element()
}

/// 「子代理」对话栏 AX：Group 节点携带当前面板状态摘要（被选代理 /
/// 加载 / 失败 / 空态），并按实测布局发布 chip / Refresh / 回底按钮
///（VoiceOver 可 Press；几何与 Inspector 页签同口径：tracked div 实测
/// bounds 按滚动视口裁剪，不发布无真实布局的行级节点）。
pub(super) fn subagent_ax(view: &AppView, window: &Window, frame: AxRect) -> AxNode {
    let activity = &view.projection.subagent_activity;
    let conversation = &view.projection.subagent_conversation;
    let bound =
        activity.session_id.is_some() && view.projection.active_session_id == activity.session_id;
    let mut value = if !bound {
        if view.projection.active_session_id.is_none() {
            t("subagents.no_session").to_string()
        } else if activity.loading {
            t("subagents.loading").to_string()
        } else {
            // 断线 / 从未取到列表（RV-11）：不伪造持续加载。
            t("subagents.stale_banner").to_string()
        }
    } else if let Some(agent_id) = conversation.agent_id.as_deref() {
        match activity
            .display_agents()
            .iter()
            .find(|agent| agent.agent_id == agent_id)
        {
            Some(agent) => format!(
                "{} · {} · {}",
                agent.title,
                agent.model_id,
                crate::ui::changes::subagent_status_label(&agent.status)
            ),
            None => t("subagents.loading").to_string(),
        }
    } else {
        t("subagents.panel_empty").to_string()
    };
    if let Some(reason) = &activity.error {
        value.push_str(" · ");
        value.push_str(reason);
    }
    if conversation.loading {
        value.push_str(" · ");
        value.push_str(t("subagents.loading"));
    }
    if let Some(reason) = &conversation.error {
        value.push_str(" · ");
        value.push_str(reason);
    }
    if let Some(reason) = &activity.stale_reason {
        value.push_str(" · ");
        value.push_str(&format!("{} · {reason}", t("subagents.stale_banner")));
    }
    let mut node = AxNode::new(
        "inspector-subagent",
        AxRole::Group,
        t("inspector.tab_subagent"),
        frame,
    )
    .value(value);
    if bound {
        let chips_bounds = view.subagent_chips_scroll.bounds();
        for agent in activity.display_agents() {
            let Some(layout) = view.subagent_agent_layouts.get(&agent.agent_id) else {
                continue;
            };
            let rect = layout.bounds().intersect(&chips_bounds);
            if rect.size.width <= gpui::px(0.0) || rect.size.height <= gpui::px(0.0) {
                continue;
            }
            node = node.child(
                AxNode::new(
                    format!("subagent-select-{}", agent.agent_id),
                    AxRole::Button,
                    t("subagents.open_conversation"),
                    AxRect::new(
                        rect.origin.x.into(),
                        rect.origin.y.into(),
                        rect.size.width.into(),
                        rect.size.height.into(),
                    ),
                )
                .value(format!(
                    "{} · {} · {}",
                    agent.title,
                    agent.model_id,
                    crate::ui::changes::subagent_status_label(&agent.status)
                ))
                .enabled(matches!(
                    view.projection.connection,
                    crate::projection::ConnectionState::Connected { .. }
                ))
                .action(AxAction::Press),
            );
        }
        let refresh = view.subagent_refresh_layout.bounds();
        if refresh.size.width > gpui::px(0.0) && refresh.size.height > gpui::px(0.0) {
            node = node.child(
                AxNode::new(
                    "subagent-refresh",
                    AxRole::Button,
                    t("subagents.refresh_tooltip"),
                    AxRect::new(
                        refresh.origin.x.into(),
                        refresh.origin.y.into(),
                        refresh.size.width.into(),
                        refresh.size.height.into(),
                    ),
                )
                .enabled(view.can_refresh_subagents())
                .action(AxAction::Press),
            );
        }
        if activity.agents.iter().any(|a| {
            Some(&a.agent_id) == conversation.agent_id.as_ref()
                && matches!(a.status.as_str(), "running" | "waiting")
        }) {
            node = node.child(
                AxNode::new(
                    "subagent-cancel",
                    AxRole::Button,
                    t(
                        if activity.cancelling.is_some()
                            && activity.cancelling == conversation.agent_id
                        {
                            "subagents.cancelling"
                        } else {
                            "subagents.cancel"
                        },
                    ),
                    view.settings_menu_element_bounds("subagent-cancel", "subagent-cancel"),
                )
                .enabled(view.can_cancel_subagent())
                .action(AxAction::Press),
            );
        }
        // RV-07 对话正文：按实际阅读顺序发布节点；tracked 条目布局按
        // transcript 滚动视口裁剪，未测量 / 零面积项不发布（几何不伪造）。
        // 正文与展开按钮进入 Inspector Tab 链，并声明 Focus，使 AX
        // setFocused 与键盘 focus_next 落在同一句柄上。
        // 工具标题与长结果展开按钮分别按各自 bounds 裁剪。
        if let Some(agent_id) = conversation.agent_id.as_deref() {
            let viewport = view.subagent_conversation_scroll.handle().bounds();
            for item in panel_items(conversation) {
                let key = match &item {
                    PanelItem::Tool { event_id, .. } => format!("{event_id}:tool-header"),
                    other => match other.event_id() {
                        Some(event_id) => format!("{event_id}:item"),
                        None => continue,
                    },
                };
                let rect = view
                    .subagent_item_layouts
                    .get(&key)
                    .and_then(|layout| subagent_ax_rect(layout.bounds().intersect(&viewport)));
                if rect.is_none() && !matches!(item, PanelItem::Tool { .. }) {
                    continue;
                };
                match &item {
                    PanelItem::Tool {
                        event_id,
                        name,
                        status,
                        detail,
                        arguments,
                    } => {
                        if let Some(rect) = rect {
                            let headline =
                                super::timeline_entry::tool_headline(name, arguments.as_deref());
                            let mut tool_value = format!(
                                "{headline} · {}",
                                super::timeline::tool_status_label(status)
                            );
                            if view
                                .expanded_subagent_details
                                .contains(&format!("{event_id}:tool"))
                            {
                                if let Some(text) = detail.as_deref().filter(|d| !d.is_empty()) {
                                    tool_value.push_str(" · ");
                                    tool_value.push_str(&subagent_ax_text(text));
                                }
                            }
                            node = node.child(
                                AxNode::new(
                                    format!("subagent-tool-toggle-{event_id}"),
                                    AxRole::Button,
                                    t("timeline.tool"),
                                    rect,
                                )
                                .value(tool_value)
                                .focused(subagent_item_focused(
                                    view,
                                    window,
                                    &format!("{event_id}:tool"),
                                ))
                                .action(AxAction::Press)
                                .action(AxAction::Focus),
                            );
                        }
                        if view
                            .expanded_subagent_details
                            .contains(&format!("{event_id}:tool"))
                        {
                            let detail_text = subagent_tool_detail_text(detail.as_deref());
                            let result_key =
                                super::timeline_entry::tool_result_expand_key(event_id);
                            let result_expanded =
                                view.expanded_subagent_details.contains(&result_key);
                            if super::timeline_entry::preview_result_text(
                                &detail_text,
                                result_expanded,
                            )
                            .1
                            {
                                if let Some(toggle_layout) = view
                                    .subagent_item_layouts
                                    .get(&subagent_tool_result_layout_key(event_id))
                                {
                                    if let Some(toggle_rect) = subagent_ax_rect(
                                        toggle_layout.bounds().intersect(&viewport),
                                    ) {
                                        node = node.child(
                                            AxNode::new(
                                                format!("subagent-tool-result-toggle-{event_id}"),
                                                AxRole::Button,
                                                if result_expanded {
                                                    t("tool.result_collapse")
                                                } else {
                                                    t("tool.result_expand")
                                                },
                                                toggle_rect,
                                            )
                                            .focused(subagent_item_focused(
                                                view,
                                                window,
                                                &result_key,
                                            ))
                                            .action(AxAction::Press)
                                            .action(AxAction::Focus),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    PanelItem::UserMessage { event_id, text } => {
                        let Some(rect) = rect else {
                            continue;
                        };
                        node = node.child(
                            AxNode::new(
                                format!("subagent-msg-{event_id}"),
                                AxRole::StaticText,
                                t("subagents.task_from_main"),
                                rect,
                            )
                            .value(subagent_ax_text(text))
                            .focused(subagent_item_focused(
                                view,
                                window,
                                &format!("{event_id}:item"),
                            ))
                            .action(AxAction::Focus),
                        );
                    }
                    PanelItem::UserFollowUp { event_id, text } => {
                        let Some(rect) = rect else {
                            continue;
                        };
                        node = node.child(
                            AxNode::new(
                                format!("subagent-msg-{event_id}"),
                                AxRole::StaticText,
                                t("timeline.you"),
                                rect,
                            )
                            .value(subagent_ax_text(text))
                            .focused(subagent_item_focused(
                                view,
                                window,
                                &format!("{event_id}:item"),
                            ))
                            .action(AxAction::Focus),
                        );
                    }
                    PanelItem::Assistant { event_id, text } => {
                        let Some(rect) = rect else {
                            continue;
                        };
                        node = node.child(
                            AxNode::new(
                                format!("subagent-msg-{event_id}"),
                                AxRole::StaticText,
                                t("subagents.assistant_label"),
                                rect,
                            )
                            .value(subagent_ax_text(text))
                            .focused(subagent_item_focused(
                                view,
                                window,
                                &format!("{event_id}:item"),
                            ))
                            .action(AxAction::Focus),
                        );
                    }
                    PanelItem::Thinking { event_id, text } => {
                        let Some(rect) = rect else {
                            continue;
                        };
                        node = node.child(
                            AxNode::new(
                                format!("subagent-msg-{event_id}"),
                                AxRole::StaticText,
                                t("subagents.thinking_label"),
                                rect,
                            )
                            .value(subagent_ax_text(text))
                            .focused(subagent_item_focused(
                                view,
                                window,
                                &format!("{event_id}:item"),
                            ))
                            .action(AxAction::Focus),
                        );
                    }
                    PanelItem::RunState(_) | PanelItem::Error(_) => {}
                }
            }
            // 结果盒（RV-05/07）：值随折叠态取摘要或全文截断；长回执的
            // 展开入口发布 Button+Press（与 render 同源 handler）。
            if let Some(agent) = activity
                .display_agents()
                .iter()
                .find(|agent| agent.agent_id == agent_id)
            {
                if let Some(result) = agent.result.as_deref() {
                    let result_key = format!("{agent_id}:result");
                    if let Some(layout) = view.subagent_item_layouts.get(&result_key) {
                        if let Some(rect) = subagent_ax_rect(layout.bounds().intersect(&viewport)) {
                            let repeated = subagent_result_matches_reply(result, conversation);
                            let long = !repeated && subagent_result_is_long(result);
                            let expanded = !repeated
                                && (!long || view.expanded_subagent_details.contains(&result_key));
                            node = node.child(
                                AxNode::new(
                                    format!("subagent-result-{agent_id}"),
                                    AxRole::StaticText,
                                    t("subagents.result_to_main"),
                                    rect,
                                )
                                .value(if repeated {
                                    t("subagents.result_in_transcript").to_string()
                                } else if expanded {
                                    subagent_ax_text(result)
                                } else {
                                    subagent_result_summary(result)
                                })
                                .focused(subagent_item_focused(
                                    view,
                                    window,
                                    &format!("{agent_id}:result-text"),
                                ))
                                .action(AxAction::Focus),
                            );
                            if long {
                                if let Some(toggle_layout) = view
                                    .subagent_item_layouts
                                    .get(&format!("{result_key}:toggle"))
                                {
                                    if let Some(toggle_rect) = subagent_ax_rect(
                                        toggle_layout.bounds().intersect(&viewport),
                                    ) {
                                        node = node.child(
                                            AxNode::new(
                                                format!("subagent-result-toggle-{agent_id}"),
                                                AxRole::Button,
                                                if expanded {
                                                    t("tool.result_collapse")
                                                } else {
                                                    t("tool.result_expand")
                                                },
                                                toggle_rect,
                                            )
                                            .focused(subagent_item_focused(
                                                view,
                                                window,
                                                &result_key,
                                            ))
                                            .action(AxAction::Press)
                                            .action(AxAction::Focus),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // 回底与终端面板同口径：脱钩且真溢出才发布，矩形取面板右下角
    //（绝对定位 BackToBottom 的固定几何）。
    let transcript_overflows = f32::from(
        view.subagent_conversation_scroll
            .handle()
            .max_offset()
            .height,
    ) > metrics::SCROLL_EPSILON;
    if !view.subagent_conversation_scroll.is_following() && transcript_overflows {
        node = node.child(
            AxNode::new(
                "subagent-back-to-bottom",
                AxRole::Button,
                t("timeline.ax_back_to_bottom"),
                AxRect::new(
                    frame.x + frame.width - 140.0,
                    frame.y + frame.height - 40.0,
                    132.0,
                    32.0,
                ),
            )
            .action(AxAction::Press),
        );
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RV-11：断线保留子代理列表、选中正文和脱钩阅读位置；刷新 / 取消 /
    /// 换代理以及过期回执都不改写。同一会话重连重查失败也不删内容。
    /// 成功列表不再包含原选中代理时清除失效选中与旧正文，不停留 Loading。
    /// 换会话才清空。
    #[gpui::test]
    fn disconnected_preserves_subagent_panel_content(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("rv11-subagent-disconnected.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(800.0)));

        let mut body = String::from("RV11-BODY-KEEP");
        for index in 0..80 {
            body.push(char::from(10));
            body.push_str("kept-line-");
            body.push_str(&index.to_string());
        }
        let body = body;

        let (list_request, loaded_generation, in_flight_request) = cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = Some("parent".into());
                view.projection.connection = crate::projection::ConnectionState::Connected {
                    instance_id: "instance-1".into(),
                };
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.select_inspector_tab(super::super::inspector::InspectorTab::Subagent, cx);
                let list_request = view.projection.subagent_activity.request_id;
                assert_ne!(list_request, 0, "打开页签会发出列表查询");
                assert_eq!(
                    view.projection.subagent_activity.error.as_deref(),
                    Some("not connected"),
                    "没有 Host client 时列表查询当场失败"
                );
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentListLoaded {
                        session_id: "parent".into(),
                        request_id: list_request,
                        data: pawork_client::SubagentListData {
                            agents: vec![running_agent("child-0"), running_agent("child-1")],
                        },
                    },
                    cx,
                );
                assert_eq!(
                    view.projection.subagent_conversation.agent_id.as_deref(),
                    Some("child-0"),
                    "列表到达且未选中时代理自动选中第一个运行中的子代理"
                );
                assert_eq!(view.projection.subagent_activity.agents.len(), 2);
                assert_eq!(
                    view.projection.subagent_activity.error.as_deref(),
                    Some("not connected"),
                    "选中触发的第二次刷新失败，但不删除已有代理"
                );
                let in_flight_request = view.projection.subagent_activity.request_id;
                assert_ne!(in_flight_request, 0);
                assert_ne!(in_flight_request, list_request);
                view.projection.subagent_activity.cancelling = Some("child-0".into());
                let loaded_generation = view.projection.subagent_conversation.load_generation;
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentTimelineLoaded {
                        session_id: "child-0".into(),
                        generation: loaded_generation,
                        page: body_page(&body),
                    },
                    cx,
                );
                assert!(assistant_text(&view.projection.subagent_conversation)
                    .starts_with("RV11-BODY-KEEP"));
                view.projection.subagent_activity.error = None;
                (list_request, loaded_generation, in_flight_request)
            })
        });
        let _ = list_request;

        cx.refresh().unwrap();
        cx.run_until_parked();

        let wheel_at = cx.update(|_, cx| {
            let current = view.read(cx);
            let scroll = current.subagent_conversation_scroll.handle();
            let max = f32::from(scroll.max_offset().height);
            assert!(
                max > 200.0,
                "fixture must overflow the transcript, max_offset={max}"
            );
            assert!(
                current.subagent_conversation_scroll.is_following()
                    && current.subagent_conversation_scroll.is_scrolled_to_bottom(),
                "loaded transcript starts following the bottom"
            );
            let bounds = scroll.bounds();
            assert!(
                f32::from(bounds.size.width) > 16.0 && f32::from(bounds.size.height) > 16.0,
                "transcript viewport {bounds:?}"
            );
            gpui::point(
                bounds.origin.x + gpui::px(8.0),
                bounds.origin.y + gpui::px(8.0),
            )
        });

        cx.simulate_event(gpui::ScrollWheelEvent {
            position: wheel_at,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-160.0))),
            ..Default::default()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| {
            let current = view.read(cx);
            assert!(
                current.subagent_conversation_scroll.is_following()
                    && current.subagent_conversation_scroll.is_scrolled_to_bottom(),
                "negative wheel at the bottom stays clamped"
            );
        });

        cx.simulate_event(gpui::ScrollWheelEvent {
            position: wheel_at,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(160.0))),
            ..Default::default()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();

        let (parked_following, parked_offset) = cx.update(|_, cx| {
            let current = view.read(cx);
            assert!(
                !current.subagent_conversation_scroll.is_following()
                    && !current.subagent_conversation_scroll.is_scrolled_to_bottom(),
                "positive wheel leaves the bottom and unhooks"
            );
            (
                current.subagent_conversation_scroll.is_following(),
                current.subagent_conversation_scroll.handle().offset(),
            )
        });

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let generation_before = view.projection.subagent_conversation.load_generation;
                view.handle_controller_event(
                    crate::controller::ControllerEvent::Disconnected {
                        reason: "socket closed".into(),
                    },
                    cx,
                );
                let activity = &view.projection.subagent_activity;
                assert_eq!(
                    activity
                        .agents
                        .iter()
                        .map(|agent| agent.agent_id.as_str())
                        .collect::<Vec<_>>(),
                    vec!["child-0", "child-1"]
                );
                assert!(!activity.loading);
                assert!(activity.cancelling.is_none());
                assert_eq!(activity.request_id, 0);
                assert_eq!(activity.stale_reason.as_deref(), Some("socket closed"));
                assert!(activity.error.is_none());
                let conversation = &view.projection.subagent_conversation;
                assert_eq!(conversation.agent_id.as_deref(), Some("child-0"));
                assert!(!conversation.loading);
                assert!(conversation.error.is_none());
                assert_eq!(conversation.load_generation, generation_before + 1);
                assert_eq!(assistant_text(conversation), body);
                assert_eq!(
                    view.subagent_conversation_scroll.is_following(),
                    parked_following
                );
                assert_eq!(
                    view.subagent_conversation_scroll.handle().offset(),
                    parked_offset
                );

                view.refresh_subagent_conversation();
                view.cancel_selected_subagent(cx);
                view.select_subagent_agent("child-1", cx);
                assert_eq!(view.projection.subagent_activity.request_id, 0);
                assert_eq!(
                    view.projection.subagent_activity.stale_reason.as_deref(),
                    Some("socket closed")
                );
                assert!(view.projection.subagent_activity.cancelling.is_none());
                assert_eq!(
                    view.projection.subagent_conversation.agent_id.as_deref(),
                    Some("child-0")
                );
                assert_eq!(
                    view.projection.subagent_conversation.load_generation,
                    generation_before + 1
                );

                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentListLoaded {
                        session_id: "parent".into(),
                        request_id: in_flight_request,
                        data: pawork_client::SubagentListData { agents: Vec::new() },
                    },
                    cx,
                );
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentListLoaded {
                        session_id: "other-session".into(),
                        request_id: in_flight_request,
                        data: pawork_client::SubagentListData {
                            agents: vec![running_agent("child-foreign")],
                        },
                    },
                    cx,
                );
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentTimelineLoaded {
                        session_id: "child-0".into(),
                        generation: loaded_generation,
                        page: timeline_page(vec![history_item(
                            99,
                            "assistant_message",
                            serde_json::json!({
                                "text": "RV11-OLD-GEN",
                                "message_id": "m-old"
                            }),
                        )]),
                    },
                    cx,
                );
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentTimelineFailed {
                        session_id: "child-1".into(),
                        generation: view.projection.subagent_conversation.load_generation,
                        reason: "other agent".into(),
                    },
                    cx,
                );
                assert_eq!(view.projection.subagent_activity.agents.len(), 2);
                assert_eq!(assistant_text(&view.projection.subagent_conversation), body);
                assert!(view.projection.subagent_conversation.error.is_none());
                assert!(!assistant_text(&view.projection.subagent_conversation)
                    .contains("RV11-OLD-GEN"));
            });
        });

        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let current = view.read(cx);
            assert_eq!(
                current.subagent_conversation_scroll.handle().offset(),
                parked_offset
            );
            assert!(!current.subagent_conversation_scroll.is_following());
            let tree = current.accessibility_tree(window, cx);
            let group = tree.find("inspector-subagent").expect("subagent group");
            let value = group.value.clone().unwrap_or_default();
            let banner = t("subagents.stale_banner");
            assert_eq!(banner, "Disconnected · content may be outdated");
            assert!(value.contains(banner), "{value}");
            assert!(value.contains(" · socket closed"), "{value}");
            assert!(!value.contains("Loading"), "{value}");
            for id in [
                "subagent-refresh",
                "subagent-cancel",
                "subagent-select-child-0",
                "subagent-select-child-1",
            ] {
                let node = tree
                    .find(id)
                    .unwrap_or_else(|| panic!("{id} missing from AX"));
                assert!(!node.enabled, "{id} must be disabled while disconnected");
            }
        });

        {
            let mut reconnect =
                |disposition: pawork_client::ResumeDisposition,
                 snapshot: Option<pawork_client::Snapshot>| {
                    cx.update(|_, cx| {
                        view.update(cx, |view, cx| {
                            let (sender, receiver) = smol::channel::bounded(4);
                            view.on_connected(
                                rv11_snapshot(),
                                Some(pawork_client::ResumeOutcome {
                                    disposition,
                                    replayed: Vec::new(),
                                    snapshot,
                                }),
                                rv11_handshake(),
                                receiver,
                                cx,
                            );
                            drop(sender);
                        });
                    });
                    cx.run_until_parked();
                };
            reconnect(
                pawork_client::ResumeDisposition::UpToDate {
                    current_sequence: pawork_client::GlobalSequence(4),
                },
                None,
            );
            reconnect(
                pawork_client::ResumeDisposition::SnapshotRequired {
                    earliest_available_sequence: pawork_client::GlobalSequence(1),
                },
                Some(rv11_snapshot()),
            );
        }

        let live_generation = cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                assert!(matches!(
                    view.projection.connection,
                    crate::projection::ConnectionState::Connected { .. }
                ));
                assert_eq!(view.projection.subagent_activity.agents.len(), 2);
                assert_eq!(
                    view.projection.subagent_activity.error.as_deref(),
                    Some("not connected")
                );
                assert!(view.projection.subagent_activity.stale_reason.is_none());
                assert_eq!(
                    view.projection.subagent_conversation.agent_id.as_deref(),
                    Some("child-0")
                );
                assert_eq!(assistant_text(&view.projection.subagent_conversation), body);
                assert!(!view.subagent_conversation_scroll.is_following());
                assert_eq!(
                    view.subagent_conversation_scroll.handle().offset(),
                    parked_offset
                );
                let live_generation = view.projection.subagent_conversation.load_generation;
                assert!(live_generation > loaded_generation);
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentTimelineFailed {
                        session_id: "child-0".into(),
                        generation: live_generation.saturating_sub(1),
                        reason: "previous generation".into(),
                    },
                    cx,
                );
                assert!(view.projection.subagent_conversation.error.is_none());
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentTimelineLoaded {
                        session_id: "child-0".into(),
                        generation: live_generation,
                        page: body_page(&body),
                    },
                    cx,
                );
                let conversation = &view.projection.subagent_conversation;
                assert_eq!(conversation.timeline.len(), 1);
                assert_eq!(
                    assistant_text(conversation)
                        .matches("RV11-BODY-KEEP")
                        .count(),
                    1
                );
                assert!(conversation.error.is_none());
                assert!(!conversation.loading);
                assert!(!view.subagent_conversation_scroll.is_following());
                assert_eq!(
                    view.subagent_conversation_scroll.handle().offset(),
                    parked_offset
                );
                live_generation
            })
        });

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let request_id = view.projection.subagent_activity.begin_loading("parent");
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentListLoaded {
                        session_id: "parent".into(),
                        request_id,
                        data: pawork_client::SubagentListData {
                            agents: vec![running_agent("child-1")],
                        },
                    },
                    cx,
                );
                assert_eq!(
                    view.projection
                        .subagent_activity
                        .agents
                        .iter()
                        .map(|agent| agent.agent_id.as_str())
                        .collect::<Vec<_>>(),
                    vec!["child-1"],
                    "权威列表移除原选中代理后只保留仍在列表中的代理"
                );
                assert!(!view.projection.subagent_activity.loading);
                assert!(view.projection.subagent_activity.error.is_none());
                assert!(view.projection.subagent_activity.stale_reason.is_none());
                assert!(view.projection.subagent_conversation.agent_id.is_none());
                assert!(view.projection.subagent_conversation.timeline.is_empty());
                assert!(!view.projection.subagent_conversation.loading);
                assert!(view.projection.subagent_conversation.error.is_none());
                assert!(!assistant_text(&view.projection.subagent_conversation)
                    .contains("RV11-BODY-KEEP"));
                assert!(view.subagent_conversation_scroll.is_following());
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentTimelineLoaded {
                        session_id: "child-0".into(),
                        generation: live_generation,
                        page: body_page("RV11-REMOVED-BODY"),
                    },
                    cx,
                );
                assert!(view.projection.subagent_conversation.agent_id.is_none());
                assert!(view.projection.subagent_conversation.timeline.is_empty());
                assert!(!view.projection.subagent_conversation.loading);
            });
        });

        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let current = view.read(cx);
            let tree = current.accessibility_tree(window, cx);
            let group = tree.find("inspector-subagent").expect("subagent group");
            let value = group.value.clone().unwrap_or_default();
            assert_eq!(value, "No subagent selected", "{value}");
            assert!(!value.contains("Loading"), "{value}");
            assert!(!value.contains("RV11-BODY-KEEP"), "{value}");
            assert!(tree.find("subagent-select-child-0").is_none());
            let kept = tree
                .find("subagent-select-child-1")
                .expect("remaining agent chip");
            assert!(kept.enabled);
        });

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.select_subagent_agent("child-1", cx);
                assert_eq!(
                    view.projection.subagent_conversation.agent_id.as_deref(),
                    Some("child-1")
                );
                assert!(view.projection.subagent_conversation.timeline.is_empty());
                assert!(view.subagent_conversation_scroll.is_following());
                let parent_request = view.projection.subagent_activity.request_id;
                view.open_session("other-task".into(), cx);
                assert!(view.projection.subagent_conversation.agent_id.is_none());
                assert!(view.projection.subagent_conversation.timeline.is_empty());
                assert!(view.projection.subagent_activity.agents.is_empty());
                assert_eq!(
                    view.projection.subagent_activity.session_id.as_deref(),
                    Some("other-task")
                );
                assert!(view.subagent_conversation_scroll.is_following());
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentTimelineLoaded {
                        session_id: "child-0".into(),
                        generation: live_generation,
                        page: timeline_page(vec![history_item(
                            7,
                            "assistant_message",
                            serde_json::json!({
                                "text": "RV11-LEAK",
                                "message_id": "m-leak"
                            }),
                        )]),
                    },
                    cx,
                );
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentListLoaded {
                        session_id: "parent".into(),
                        request_id: parent_request,
                        data: pawork_client::SubagentListData {
                            agents: vec![running_agent("child-0"), running_agent("child-1")],
                        },
                    },
                    cx,
                );
                assert!(view.projection.subagent_conversation.agent_id.is_none());
                assert!(view.projection.subagent_conversation.timeline.is_empty());
                assert!(view.projection.subagent_activity.agents.is_empty());
                assert_eq!(
                    view.projection.subagent_activity.session_id.as_deref(),
                    Some("other-task")
                );
                assert!(
                    !assistant_text(&view.projection.subagent_conversation).contains("RV11-LEAK")
                );
            });
        });
    }

    fn running_agent(id: &str) -> crate::projection::SubagentInfo {
        crate::projection::SubagentInfo {
            agent_id: id.into(),
            session_id: id.into(),
            parent_run_id: "run-0".into(),
            title: format!("Agent {id}"),
            provider_id: "test-provider".into(),
            model_id: "test-model".into(),
            status: "running".into(),
            result: None,
            effort: None,
        }
    }

    fn body_page(text: &str) -> pawork_client::TimelinePage {
        timeline_page(vec![history_item(
            1,
            "assistant_message",
            serde_json::json!({ "text": text, "message_id": "m-rv11" }),
        )])
    }

    fn assistant_text(conversation: &SubagentConversationState) -> String {
        conversation
            .timeline
            .iter()
            .find_map(|entry| match &entry.kind {
                crate::projection::TimelineEntryKind::AssistantMessage { text } => {
                    Some(text.clone())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    fn rv11_snapshot() -> pawork_client::Snapshot {
        serde_json::from_value(serde_json::json!({
            "instance_id": "instance-1",
            "snapshot_sequence": 4,
            "generated_at": 4,
            "sections": [{
                "kind": "session_tree",
                "revision": 1,
                "data": [{ "session_id": "parent", "title": "Parent", "updated_at_ms": 4 }]
            }]
        }))
        .expect("rv11 snapshot")
    }

    fn rv11_handshake() -> crate::controller::DesktopHandshakeInfo {
        crate::controller::DesktopHandshakeInfo {
            runtime_id: "runtime-rv11".into(),
            api_version: "1.9".into(),
            capabilities: vec!["events".into(), "snapshots".into()],
            host_data_dir: None,
        }
    }

    fn history_item(sequence: u64, kind: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut item = serde_json::json!({
            "sequence": sequence,
            "event_id": format!("hist-{sequence}"),
            "kind": kind,
            "run_id": "r-child",
            "timestamp": "2000"
        });
        if let Some(fields) = extra.as_object() {
            for (key, value) in fields {
                item[key] = value.clone();
            }
        }
        item
    }

    fn tool_context(tool_call_id: &str) -> String {
        serde_json::json!({ "_pawork_tool_call_id": tool_call_id }).to_string()
    }

    fn timeline_page(items: Vec<serde_json::Value>) -> pawork_client::TimelinePage {
        serde_json::from_value(serde_json::json!({
            "items": items,
            "head_sequence": items.len() as u64,
            "complete": true
        }))
        .expect("timeline page")
    }

    fn tool_fact(items: &[PanelItem], name: &str) -> (String, Option<String>, Option<String>) {
        items
            .iter()
            .find_map(|item| match item {
                PanelItem::Tool {
                    name: tool_name,
                    status,
                    detail,
                    arguments,
                    ..
                } if tool_name == name => Some((status.clone(), detail.clone(), arguments.clone())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing tool {name}"))
    }

    /// RV-04 主路径：历史工具事实进子代理时间线后，标题含目标，参数
    /// pretty，默认折叠；AX Button+Press 走正式 handler，展开值带结果
    /// 原文，再折叠不改时间线，重复展开一致。
    #[gpui::test]
    fn subagent_tool_row_expands_call_and_receipt(cx: &mut gpui::TestAppContext) {
        let arguments = r#"{"path":"src/lib.rs","mode":"ro"}"#;
        let receipt = "fn main() {}";
        let mut conversation = SubagentConversationState::default();
        assert!(conversation.select_agent("child-0"));
        let generation = conversation.begin_loading();
        conversation.apply_page(
            "child-0",
            generation,
            &timeline_page(vec![
                history_item(
                    1,
                    "tool_started",
                    serde_json::json!({
                        "tool_name": "read_file",
                        "status": "running",
                        "detail": tool_context("call-read")
                    }),
                ),
                history_item(
                    2,
                    "other",
                    serde_json::json!({
                        "status": "tool_arguments",
                        "text": arguments,
                        "detail": tool_context("call-read")
                    }),
                ),
                history_item(
                    3,
                    "tool_completed",
                    serde_json::json!({
                        "tool_name": "read_file",
                        "status": "succeeded",
                        "text": receipt,
                        "detail": tool_context("call-read")
                    }),
                ),
            ]),
        );
        let (status, detail, stored_arguments) =
            tool_fact(&panel_items(&conversation), "read_file");
        assert_eq!(status, "succeeded");
        assert_eq!(detail.as_deref(), Some(receipt));
        assert_eq!(stored_arguments.as_deref(), Some(arguments));
        assert_eq!(
            super::super::timeline_entry::tool_headline("read_file", stored_arguments.as_deref()),
            "read_file src/lib.rs"
        );
        let pretty = super::super::timeline_entry::arguments_secondary(stored_arguments.as_deref());
        assert_eq!(
            pretty,
            serde_json::to_string_pretty(&serde_json::json!({"path": "src/lib.rs", "mode": "ro"}))
                .unwrap()
        );
        assert_ne!(pretty, arguments);

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("rv04-subagent-tool-row.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = Some("parent".into());
                view.projection.connection = crate::projection::ConnectionState::Connected {
                    instance_id: "test".into(),
                };
                view.projection.subagent_activity.session_id = Some("parent".into());
                view.projection.subagent_activity.agents = vec![crate::projection::SubagentInfo {
                    agent_id: "child-0".into(),
                    session_id: "child-0".into(),
                    parent_run_id: "run-0".into(),
                    title: "Agent 0".into(),
                    provider_id: "test-provider".into(),
                    model_id: "test-model".into(),
                    status: "completed".into(),
                    result: None,
                    effort: None,
                }];
                view.projection.subagent_conversation = conversation;
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.inspector_tab = super::super::inspector::InspectorTab::Subagent;
                view.close_open_menu(cx);
                assert!(view.expanded_subagent_details.is_empty());
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();

        let identifier = "subagent-tool-toggle-hist-1";
        let collapsed = cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let node = tree
                .find(identifier)
                .expect("collapsed tool row publishes a button");
            assert_eq!(node.role, AxRole::Button);
            assert!(node.enabled);
            assert!(node.actions.contains(&AxAction::Press));
            node.value.clone().unwrap_or_default()
        });
        assert_eq!(collapsed, "read_file src/lib.rs · Completed");
        assert!(!collapsed.contains(receipt));

        macro_rules! press_tool {
            () => {
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.handle_accessibility_request(
                            crate::ui::accessibility::AxRequest {
                                identifier: identifier.into(),
                                action: AxAction::Press,
                                value: None,
                            },
                            window,
                            cx,
                        );
                    });
                });
            };
        }
        press_tool!();
        let expanded = cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(view.expanded_subagent_details.contains("hist-1:tool"));
            view.accessibility_tree(window, cx)
                .find(identifier)
                .unwrap()
                .value
                .clone()
                .unwrap_or_default()
        });
        assert_eq!(expanded, format!("{collapsed} · {receipt}"));

        press_tool!();
        cx.update(|_, cx| {
            view.update(cx, |view, _cx| {
                assert!(!view.expanded_subagent_details.contains("hist-1:tool"));
                let (status, detail, stored_arguments) = tool_fact(
                    &panel_items(&view.projection.subagent_conversation),
                    "read_file",
                );
                assert_eq!(status, "succeeded");
                assert_eq!(detail.as_deref(), Some(receipt));
                assert_eq!(stored_arguments.as_deref(), Some(arguments));
            });
        });
        press_tool!();
        let expanded_again = cx.update(|window, cx| {
            view.read(cx)
                .accessibility_tree(window, cx)
                .find(identifier)
                .unwrap()
                .value
                .clone()
                .unwrap_or_default()
        });
        assert_eq!(expanded_again, expanded);
    }

    /// RV-04 关键失败路径：权限拒绝与执行失败都保持 Host 原文。
    /// 审批在完成前，请求只带 pending + reason，标题回退 tool；拒绝是 approval deny。
    #[test]
    fn subagent_tool_denial_stays_distinct_from_execution_failure() {
        let denial = "tool call denied by user";
        let execution = (0..12)
            .map(|ix| format!("exec-line-{ix}"))
            .collect::<Vec<_>>()
            .join("\n");
        let malformed = "{not json";
        let mut conversation = SubagentConversationState::default();
        assert!(conversation.select_agent("child-0"));
        let generation = conversation.begin_loading();
        conversation.apply_page(
            "child-0",
            generation,
            &timeline_page(vec![
                history_item(
                    1,
                    "tool_started",
                    serde_json::json!({
                        "tool_name": "write_file",
                        "status": "running",
                        "detail": tool_context("call-deny")
                    }),
                ),
                history_item(
                    2,
                    "other",
                    serde_json::json!({
                        "status": "tool_arguments",
                        "text": r#"{"path":"secret.txt"}"#,
                        "detail": tool_context("call-deny")
                    }),
                ),
                history_item(
                    3,
                    "approval_requested",
                    serde_json::json!({
                        "status": "pending",
                        "detail": "tool `write_file` requires approval"
                    }),
                ),
                history_item(
                    4,
                    "approval_responded",
                    serde_json::json!({ "status": "deny" }),
                ),
                history_item(
                    5,
                    "tool_completed",
                    serde_json::json!({
                        "tool_name": "write_file",
                        "status": "failed",
                        "text": denial,
                        "detail": tool_context("call-deny")
                    }),
                ),
                history_item(
                    6,
                    "tool_started",
                    serde_json::json!({
                        "tool_name": "run_command",
                        "status": "running",
                        "detail": tool_context("call-exec")
                    }),
                ),
                history_item(
                    7,
                    "other",
                    serde_json::json!({
                        "status": "tool_arguments",
                        "text": malformed,
                        "detail": tool_context("call-exec")
                    }),
                ),
                history_item(
                    8,
                    "tool_completed",
                    serde_json::json!({
                        "tool_name": "run_command",
                        "status": "failed",
                        "text": execution,
                        "detail": tool_context("call-exec")
                    }),
                ),
            ]),
        );
        let items = panel_items(&conversation);
        let (deny_status, deny_detail, deny_arguments) = tool_fact(&items, "write_file");
        let (exec_status, exec_detail, exec_arguments) = tool_fact(&items, "run_command");
        assert_eq!(deny_status, "failed");
        assert_eq!(exec_status, "failed");
        assert_eq!(deny_detail.as_deref(), Some(denial));
        assert_eq!(exec_detail.as_deref(), Some(execution.as_str()));
        assert_ne!(deny_detail, exec_detail);
        assert_eq!(
            super::super::timeline::tool_status_label(&deny_status),
            super::super::timeline::tool_status_label(&exec_status)
        );
        assert_ne!(
            super::super::timeline::tool_status_label(&deny_status),
            denial
        );
        assert!(!deny_detail.unwrap().contains("失败"));
        assert_eq!(
            super::super::timeline_entry::tool_headline("write_file", deny_arguments.as_deref()),
            "write_file secret.txt"
        );
        assert_eq!(
            super::super::timeline_entry::arguments_secondary(deny_arguments.as_deref()),
            serde_json::to_string_pretty(&serde_json::json!({"path": "secret.txt"})).unwrap()
        );
        assert_eq!(exec_arguments.as_deref(), Some(malformed));
        assert_eq!(
            super::super::timeline_entry::arguments_secondary(exec_arguments.as_deref()),
            malformed
        );
        assert_eq!(
            super::super::timeline_entry::tool_headline("run_command", exec_arguments.as_deref()),
            "run_command"
        );
        let states: Vec<&str> = items
            .iter()
            .filter_map(|item| match item {
                PanelItem::RunState(state) => Some(state.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            states,
            vec![
                "approval requested · tool · tool `write_file` requires approval",
                "approval deny",
            ]
        );
        for state in &states {
            assert_eq!(subagent_run_label(state), (*state).to_string());
        }
        let (preview, can_expand) =
            super::super::timeline_entry::preview_result_text(&execution, false);
        assert!(can_expand);
        assert_eq!(preview.lines().count(), 10);
        assert!(preview.contains("exec-line-0"));
        assert!(!preview.contains("exec-line-11"));
        let (full, can_expand_again) =
            super::super::timeline_entry::preview_result_text(&execution, true);
        assert!(can_expand_again);
        assert_eq!(full, execution);
        assert_eq!(
            super::super::timeline_entry::preview_result_text(&execution, true),
            (full.clone(), true)
        );
        assert_eq!(
            super::super::timeline_entry::preview_result_text(&denial, false),
            (denial.to_string(), false)
        );
    }

    fn ax_mentions(nodes: &[AxNode], needle: &str) -> usize {
        fn walk(node: &AxNode, needle: &str) -> usize {
            let mut count = node.label.matches(needle).count();
            if let Some(value) = &node.value {
                count += value.matches(needle).count();
            }
            if let Some(description) = &node.description {
                count += description.matches(needle).count();
            }
            for child in &node.children {
                count += walk(child, needle);
            }
            count
        }
        nodes.iter().map(|node| walk(node, needle)).sum()
    }

    fn receipt_fixture() -> String {
        let fence = "`".repeat(3);
        let padding = (0..12)
            .map(|ix| format!("pad-{ix}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Receipt body is readable.\n\n- list-alpha\n\n{fence}rust\nfn kept() {{}}\n{fence}\n{}\n{padding}",
            "x".repeat(520)
        )
    }

    /// RV-05 主路径：长回执走与 Timeline 相同的 Markdown，默认折叠；AX Press
    /// 展开后全文随外层滚动，不另开结果盒滚动区。助手正文落地后结果盒不再
    /// 渲染第二份。
    #[gpui::test]
    fn subagent_result_receipt_reads_markdown_and_folds(cx: &mut gpui::TestAppContext) {
        let receipt = receipt_fixture();
        assert!(receipt.chars().count() > 600);
        assert!(receipt.lines().count() > 10);
        assert_eq!(
            subagent_result_summary(&receipt),
            "Receipt body is readable."
        );

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("rv05-subagent-result.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(800.0)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = Some("parent".into());
                view.projection.connection = crate::projection::ConnectionState::Connected {
                    instance_id: "test".into(),
                };
                assert!(view
                    .projection
                    .subagent_conversation
                    .select_agent("child-0"));
                let request_id = view.projection.subagent_activity.begin_loading("parent");
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentListLoaded {
                        session_id: "parent".into(),
                        request_id,
                        data: pawork_client::SubagentListData {
                            agents: vec![crate::projection::SubagentInfo {
                                agent_id: "child-0".into(),
                                session_id: "child-0".into(),
                                parent_run_id: "run-0".into(),
                                title: "Agent 0".into(),
                                provider_id: "test-provider".into(),
                                model_id: "test-model".into(),
                                status: "completed".into(),
                                result: Some(receipt.clone()),
                                effort: None,
                            }],
                        },
                    },
                    cx,
                );
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.inspector_tab = super::super::inspector::InspectorTab::Subagent;
                view.close_open_menu(cx);
                assert!(view.expanded_subagent_details.is_empty());
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();

        let result_id = "subagent-result-child-0";
        let toggle_id = "subagent-result-toggle-child-0";
        let collapsed = cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            assert_eq!(ax_mentions(&tree.children, "fn kept"), 0);
            assert_eq!(ax_mentions(&tree.children, "list-alpha"), 0);
            let node = tree.find(result_id).expect("collapsed result box");
            assert_eq!(node.role, AxRole::StaticText);
            let toggle = tree.find(toggle_id).expect("collapsed result toggle");
            assert_eq!(toggle.role, AxRole::Button);
            assert!(toggle.actions.contains(&AxAction::Press));
            assert_eq!(toggle.label, "Show full result");
            node.value.clone().unwrap_or_default()
        });
        assert_eq!(collapsed, "Receipt body is readable.");

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: toggle_id.into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();

        let (wheel_at, result_before, outer_before) = cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(view.expanded_subagent_details.contains("child-0:result"));
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let node = tree.find(result_id).expect("expanded result box");
            let expected = subagent_ax_text(&receipt);
            assert_eq!(node.value.as_deref(), Some(expected.as_str()));
            assert!(expected.contains("list-alpha"));
            assert!(expected.contains("fn kept() {}"));
            let toggle = tree.find(toggle_id).expect("expanded result toggle");
            assert_eq!(toggle.label, "Show less");
            assert!(toggle.actions.contains(&AxAction::Press));
            let copies =
                super::super::markdown::message_code_copies("subagent-result-child-0", &receipt);
            assert_eq!(copies.len(), 1);
            assert!(copies[0].content.contains("fn kept() {}"));
            let code_bounds = view.settings_element_layouts[&copies[0].identifier].bounds();
            assert!(f32::from(code_bounds.size.width) > 0.0);
            assert!(f32::from(code_bounds.size.height) > 0.0);
            let result = view
                .subagent_item_layouts
                .get("child-0:result")
                .expect("result scroll handle");
            let result_max = f32::from(result.max_offset().height);
            let outer_max = f32::from(
                view.subagent_conversation_scroll
                    .handle()
                    .max_offset()
                    .height,
            );
            assert!(
                result_max <= crate::ui::theme::metrics::SCROLL_EPSILON,
                "result box must not be its own vertical scroller, max_offset.y={result_max}"
            );
            assert!(
                outer_max > 1.0,
                "expanded receipt must overflow the transcript, max_offset.y={outer_max}"
            );
            let viewport = view.subagent_conversation_scroll.handle().bounds();
            let visible = result.bounds().intersect(&viewport);
            assert!(f32::from(visible.size.width) > 16.0);
            assert!(f32::from(visible.size.height) > 16.0);
            let at = gpui::point(
                visible.origin.x + gpui::px(8.0),
                visible.origin.y + gpui::px(4.0),
            );
            assert!(at.y < result.bounds().origin.y + gpui::px(28.0));
            (
                at,
                result.offset(),
                view.subagent_conversation_scroll.handle().offset(),
            )
        });
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: wheel_at,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-80.0))),
            ..Default::default()
        });
        cx.update(|_, cx| {
            let view = view.read(cx);
            let result_after = view
                .subagent_item_layouts
                .get("child-0:result")
                .expect("result scroll handle")
                .offset();
            let outer_after = view.subagent_conversation_scroll.handle().offset();
            assert_eq!(
                result_after, result_before,
                "wheel inside the result box must not move an inner offset"
            );
            assert_ne!(
                outer_after, outer_before,
                "wheel inside the result box must scroll the outer transcript"
            );
        });

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: toggle_id.into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(!view.expanded_subagent_details.contains("child-0:result"));
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            assert_eq!(
                tree.find(result_id)
                    .and_then(|node| node.value.clone())
                    .as_deref(),
                Some("Receipt body is readable.")
            );
            assert_eq!(
                tree.find(toggle_id).map(|node| node.label.as_str()),
                Some("Show full result")
            );
            assert_eq!(ax_mentions(&tree.children, "fn kept"), 0);
            assert_eq!(ax_mentions(&tree.children, "list-alpha"), 0);
        });

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                // 滚轮已脱钩。贴底后再喂时间线，结果盒与助手正文才能同时进 AX 视口。
                view.subagent_conversation_scroll.jump_to_bottom();
                let generation = view.projection.subagent_conversation.begin_loading();
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentTimelineLoaded {
                        session_id: "child-0".into(),
                        generation,
                        page: timeline_page(vec![history_item(
                            1,
                            "assistant_message",
                            serde_json::json!({
                                "text": receipt,
                                "message_id": "m-receipt"
                            }),
                        )]),
                    },
                    cx,
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(view.projection.timeline.is_empty());
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            assert_eq!(
                tree.find(result_id)
                    .and_then(|node| node.value.clone())
                    .as_deref(),
                Some(crate::ui::i18n::t("subagents.result_in_transcript"))
            );
            assert!(tree.find(toggle_id).is_none());
            assert_eq!(ax_mentions(&tree.children, "fn kept() {}"), 1);
            let event_id = view
                .projection
                .subagent_conversation
                .timeline
                .iter()
                .find_map(|entry| match &entry.kind {
                    crate::projection::TimelineEntryKind::AssistantMessage { text }
                        if text.trim() == receipt =>
                    {
                        Some(entry.event_id.as_str())
                    }
                    _ => None,
                })
                .expect("assistant receipt");
            let copies = super::super::markdown::message_code_copies(
                &format!("subagent-{event_id}"),
                &receipt,
            );
            assert_eq!(copies.len(), 1);
            let code_bounds = view.settings_element_layouts[&copies[0].identifier].bounds();
            assert!(f32::from(code_bounds.size.width) > 0.0);
            assert!(f32::from(code_bounds.size.height) > 0.0);
        });
    }

    /// RV-05 边界：Host 回执最多 2000 个字符。相等、两侧空白，或恰好 2000
    /// 字符的前缀算同一份；1999 与其它字符串不算。空助手正文跳过。
    #[test]
    fn subagent_result_dedup_follows_host_prefix_boundary() {
        let body = "甲".repeat(2500);
        let prefix_2000: String = body.chars().take(2000).collect();
        let prefix_1999: String = body.chars().take(1999).collect();
        let other_2000 = "乙".repeat(2000);
        assert_eq!(prefix_2000.chars().count(), 2000);
        assert_eq!(prefix_1999.chars().count(), 1999);
        assert_eq!(other_2000.chars().count(), 2000);

        let untouched = SubagentConversationState::default();
        assert!(!subagent_result_matches_reply(&prefix_2000, &untouched));
        assert!(!subagent_result_matches_reply(&body, &untouched));

        let mut blanks = SubagentConversationState::default();
        assert!(blanks.select_agent("child-0"));
        let generation = blanks.begin_loading();
        blanks.apply_page(
            "child-0",
            generation,
            &timeline_page(vec![
                history_item(1, "assistant_message", serde_json::json!({ "text": " \n" })),
                history_item(2, "assistant_message", serde_json::json!({ "text": "\t" })),
            ]),
        );
        assert!(!subagent_result_matches_reply(&body, &blanks));
        assert!(!subagent_result_matches_reply(&prefix_2000, &blanks));

        let stored = format!("\n{body} \n");
        let mut conversation = SubagentConversationState::default();
        assert!(conversation.select_agent("child-0"));
        let generation = conversation.begin_loading();
        conversation.apply_page(
            "child-0",
            generation,
            &timeline_page(vec![
                history_item(1, "assistant_message", serde_json::json!({ "text": " \n" })),
                history_item(
                    2,
                    "assistant_message",
                    serde_json::json!({ "text": stored, "message_id": "m-body" }),
                ),
                history_item(3, "assistant_message", serde_json::json!({ "text": "\t" })),
            ]),
        );
        assert!(subagent_result_matches_reply(&prefix_2000, &conversation));
        assert!(!subagent_result_matches_reply(&prefix_1999, &conversation));
        assert!(subagent_result_matches_reply(&body, &conversation));
        assert!(subagent_result_matches_reply(
            &format!("  {body}\n"),
            &conversation
        ));
        assert!(!subagent_result_matches_reply("other", &conversation));
        assert!(!subagent_result_matches_reply(&other_2000, &conversation));
    }

    fn spoken_ax(node: &AxNode) -> String {
        match &node.value {
            Some(value) => format!("{} · {value}", node.label),
            None => node.label.clone(),
        }
    }

    fn focused_subagent_key(view: &AppView, window: &Window) -> Option<String> {
        view.subagent_item_focus
            .iter()
            .find_map(|(key, focus)| focus.is_focused(window).then(|| key.clone()))
    }

    /// RV-07：正文进入 Inspector Tab 链，AX Focus 与 focus_next / focus_prev
    /// 落在同一句柄。GPUI 测试没有 NSWindow，直接调用产品路径截获 Tab 后
    /// 使用的那一对方法；按钮用 Enter 激活。朗读文本是 label · value。
    /// 结果对的键盘顺序是展开按钮然后文本，AX 阅读顺序相反。原生 bridge
    /// 与 VoiceOver 不在这条模拟里。
    #[gpui::test]
    fn subagent_body_keyboard_focus_reads_and_presses(cx: &mut gpui::TestAppContext) {
        let receipt = (0..12)
            .map(|index| format!("receipt-line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(receipt.lines().count(), 12);
        let result = std::iter::once("Distinct agent result".to_string())
            .chain((1..11).map(|index| format!("agent-result-line-{index}-{}", "x".repeat(40))))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(result.lines().count(), 11);
        assert!(result.chars().count() > SUBAGENT_AX_TEXT_CHARS);
        let summary = subagent_result_summary(&result);
        let truncated = subagent_ax_text(&result);
        assert_eq!(summary, "Distinct agent result");
        assert_ne!(truncated, summary);
        assert!(truncated.ends_with('…'));
        assert!(truncated.contains("agent-result-line-1"));
        assert!(!truncated.contains("agent-result-line-10"));

        let mut conversation = SubagentConversationState::default();
        assert!(conversation.select_agent("child-0"));
        let generation = conversation.begin_loading();
        conversation.apply_page(
            "child-0",
            generation,
            &timeline_page(vec![
                history_item(
                    1,
                    "user_message",
                    serde_json::json!({ "text": "Inspect the crate" }),
                ),
                history_item(
                    2,
                    "thinking_delta",
                    serde_json::json!({ "text": "Check the public API" }),
                ),
                history_item(
                    3,
                    "assistant_delta",
                    serde_json::json!({ "text": "Done looking" }),
                ),
                history_item(
                    4,
                    "tool_started",
                    serde_json::json!({
                        "tool_name": "read_file",
                        "status": "running",
                        "detail": tool_context("call-read")
                    }),
                ),
                history_item(
                    5,
                    "other",
                    serde_json::json!({
                        "status": "tool_arguments",
                        "text": r#"{"path":"src/lib.rs"}"#,
                        "detail": tool_context("call-read")
                    }),
                ),
                history_item(
                    6,
                    "tool_completed",
                    serde_json::json!({
                        "tool_name": "read_file",
                        "status": "succeeded",
                        "text": receipt,
                        "detail": tool_context("call-read")
                    }),
                ),
                history_item(
                    7,
                    "user_message",
                    serde_json::json!({ "text": "Send the summary" }),
                ),
            ]),
        );
        let items = panel_items(&conversation);
        assert_eq!(
            items
                .iter()
                .filter_map(|item| item.event_id())
                .collect::<Vec<_>>(),
            vec!["hist-1", "hist-2:thinking", "hist-3", "hist-4", "hist-7",]
        );

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("rv07-subagent-body-focus.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1600.0)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = Some("parent".into());
                view.projection.connection = crate::projection::ConnectionState::Connected {
                    instance_id: "test".into(),
                };
                view.projection.subagent_activity.session_id = Some("parent".into());
                view.projection.subagent_activity.agents = vec![crate::projection::SubagentInfo {
                    agent_id: "child-0".into(),
                    session_id: "child-0".into(),
                    parent_run_id: "run-0".into(),
                    title: "Agent 0".into(),
                    provider_id: "test-provider".into(),
                    model_id: "test-model".into(),
                    status: "completed".into(),
                    result: Some(result.clone()),
                    effort: None,
                }];
                view.projection.subagent_conversation = conversation;
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.inspector_tab = super::super::inspector::InspectorTab::Subagent;
                view.close_open_menu(cx);
                assert!(view.expanded_subagent_details.is_empty());
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();

        let collapsed_tool = "Tool · read_file src/lib.rs · Completed";
        let expected_order = [
            "subagent-select-child-0",
            "subagent-refresh",
            "subagent-msg-hist-1",
            "subagent-msg-hist-2:thinking",
            "subagent-msg-hist-3",
            "subagent-tool-toggle-hist-4",
            "subagent-msg-hist-7",
            "subagent-result-child-0",
            "subagent-result-toggle-child-0",
        ];
        let expected_spoken = [
            "Open subagent conversation · Agent 0 · test-model · Done",
            "Refresh subagent conversations",
            "Main agent → subagent · Inspect the crate",
            "Thinking · Check the public API",
            "Subagent · Done looking",
            collapsed_tool,
            "You · Send the summary",
            "Subagent → main agent · Distinct agent result",
            "Show full result",
        ];
        cx.update(|window, cx| {
            let view = view.read(cx);
            let tree = view.accessibility_tree(window, cx);
            tree.validate().unwrap();
            let group = tree.find("inspector-subagent").expect("subagent group");
            assert_eq!(
                group.value.as_deref(),
                Some("Agent 0 · test-model · Done"),
                "group value stays the header summary"
            );
            assert_eq!(
                group
                    .children
                    .iter()
                    .map(|node| node.identifier.as_str())
                    .collect::<Vec<_>>(),
                expected_order
            );
            assert_eq!(
                group.children.iter().map(spoken_ax).collect::<Vec<_>>(),
                expected_spoken
            );
            let text_at = expected_order
                .iter()
                .position(|id| *id == "subagent-result-child-0")
                .unwrap();
            let toggle_at = expected_order
                .iter()
                .position(|id| *id == "subagent-result-toggle-child-0")
                .unwrap();
            assert!(
                text_at < toggle_at,
                "AX reads result text before its toggle"
            );
            assert!(group.children.iter().all(|node| !node.focused));
            assert!(tree.find("subagent-tool-result-toggle-hist-4").is_none());
            for node in &group.children {
                let body = node.identifier.starts_with("subagent-msg-")
                    || node.identifier.starts_with("subagent-tool-")
                    || node.identifier.starts_with("subagent-result");
                if !body {
                    continue;
                }
                assert!(
                    node.actions.contains(&AxAction::Focus),
                    "{}",
                    node.identifier
                );
                let press = node.actions.contains(&AxAction::Press);
                assert_eq!(press, node.role == AxRole::Button, "{}", node.identifier);
            }
            assert!(group
                .children
                .iter()
                .filter(|node| node.role == AxRole::StaticText)
                .all(|node| node.value.as_deref() != Some("Agent 0 · test-model · Done")));
        });

        let keys = [
            "hist-1:item",
            "hist-2:thinking:item",
            "hist-3:item",
            "hist-4:tool",
            "hist-7:item",
            "child-0:result",
            "child-0:result-text",
        ];
        let ax_ids = [
            "subagent-msg-hist-1",
            "subagent-msg-hist-2:thinking",
            "subagent-msg-hist-3",
            "subagent-tool-toggle-hist-4",
            "subagent-msg-hist-7",
            "subagent-result-toggle-child-0",
            "subagent-result-child-0",
        ];
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: ax_ids[0].into(),
                        action: AxAction::Focus,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
            for (index, (key, ax_id)) in keys.into_iter().zip(ax_ids).enumerate() {
                if index > 0 {
                    window.focus_next();
                }
                let view = view.read(cx);
                assert_eq!(
                    focused_subagent_key(view, window).as_deref(),
                    Some(key),
                    "focus_next step {index}"
                );
                let tree = view.accessibility_tree(window, cx);
                assert!(
                    tree.find(ax_id).is_some_and(|node| node.focused),
                    "{ax_id} focused"
                );
                assert_eq!(
                    tree.focused().map(|node| node.identifier.as_str()),
                    Some(ax_id)
                );
            }
            for (index, (key, ax_id)) in keys.into_iter().zip(ax_ids).rev().skip(1).enumerate() {
                window.focus_prev();
                let view = view.read(cx);
                assert_eq!(
                    focused_subagent_key(view, window).as_deref(),
                    Some(key),
                    "focus_prev step {index}"
                );
                let tree = view.accessibility_tree(window, cx);
                assert!(tree.find(ax_id).is_some_and(|node| node.focused));
            }
        });

        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: "subagent-tool-toggle-hist-4".into(),
                        action: AxAction::Focus,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
            assert_eq!(
                focused_subagent_key(view.read(cx), window).as_deref(),
                Some("hist-4:tool")
            );
        });
        cx.simulate_keystrokes("enter");
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert!(view.expanded_subagent_details.contains("hist-4:tool"));
            assert_eq!(
                focused_subagent_key(view, window).as_deref(),
                Some("hist-4:tool")
            );
            let value = view
                .accessibility_tree(window, cx)
                .find("subagent-tool-toggle-hist-4")
                .and_then(|node| node.value.clone())
                .unwrap_or_default();
            // 节点 value 不含 label。展开后是 headline · status · 截断后的原文，
            // 不是 10 行预览；朗读形态仍是 label · value。
            assert_eq!(
                value,
                format!(
                    "read_file src/lib.rs · Completed · {}",
                    subagent_ax_text(&receipt)
                )
            );
            assert!(value.contains("receipt-line-0"));
            assert!(value.contains("receipt-line-11"));
        });
        cx.update(|window, _| window.focus_next());
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(
                focused_subagent_key(view, window).as_deref(),
                Some("hist-4:result"),
                "next stop is the tool result toggle"
            );
            let tree = view.accessibility_tree(window, cx);
            let node = tree
                .find("subagent-tool-result-toggle-hist-4")
                .expect("expanded tool publishes the result toggle");
            assert_eq!(node.label, "Show full result");
            assert!(node.focused);
            assert!(node.actions.contains(&AxAction::Press));
        });
        cx.simulate_keystrokes("enter");
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            {
                let current = view.read(cx);
                assert!(current.expanded_subagent_details.contains(
                    &super::super::timeline_entry::tool_result_expand_key("hist-4")
                ));
                assert_eq!(
                    current
                        .accessibility_tree(window, cx)
                        .find("subagent-tool-result-toggle-hist-4")
                        .map(|node| node.label.as_str()),
                    Some("Show less")
                );
            }
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: "subagent-tool-result-toggle-hist-4".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
            let current = view.read(cx);
            assert!(!current.expanded_subagent_details.contains(
                &super::super::timeline_entry::tool_result_expand_key("hist-4")
            ));
            assert_eq!(
                focused_subagent_key(current, window).as_deref(),
                Some("hist-4:result")
            );
            assert_eq!(
                current
                    .accessibility_tree(window, cx)
                    .find("subagent-tool-result-toggle-hist-4")
                    .map(|node| node.label.as_str()),
                Some("Show full result")
            );
            window.focus_next();
            window.focus_next();
        });
        cx.update(|window, cx| {
            assert_eq!(
                focused_subagent_key(view.read(cx), window).as_deref(),
                Some("child-0:result")
            );
        });
        cx.simulate_keystrokes("enter");
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.focus_next();
            let view = view.read(cx);
            assert!(view.expanded_subagent_details.contains("child-0:result"));
            assert_eq!(
                focused_subagent_key(view, window).as_deref(),
                Some("child-0:result-text")
            );
            assert_eq!(
                view.accessibility_tree(window, cx)
                    .find("subagent-result-child-0")
                    .and_then(|node| node.value.clone())
                    .as_deref(),
                Some(truncated.as_str())
            );
            window.focus_prev();
        });
        cx.simulate_keystrokes("enter");
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.focus_next();
            let (expanded, focused) = {
                let current = view.read(cx);
                assert!(!current.expanded_subagent_details.contains("child-0:result"));
                assert_eq!(
                    focused_subagent_key(current, window).as_deref(),
                    Some("child-0:result-text")
                );
                let tree = current.accessibility_tree(window, cx);
                assert!(tree.find("subagent-result-child-0").is_some_and(|node| {
                    node.focused && node.value.as_deref() == Some(summary.as_str())
                }));
                (
                    current.expanded_subagent_details.clone(),
                    focused_subagent_key(current, window),
                )
            };
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: "subagent-not-a-node".into(),
                        action: AxAction::Focus,
                        value: None,
                    },
                    window,
                    cx,
                );
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: "subagent-msg-hist-1".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
            let current = view.read(cx);
            assert_eq!(current.expanded_subagent_details, expanded);
            assert_eq!(focused_subagent_key(current, window), focused);
            assert!(current.expanded_subagent_details.contains("hist-4:tool"));
            assert!(!current.expanded_subagent_details.contains("hist-1:item"));
        });
    }

    /// RV-07：长对话里 focus_next 必须把离屏正文滚进 transcript，AX 才能发布。
    /// 工具标题离屏时，仍可见的结果展开按钮按自己的 bounds 留下。
    /// 滚动到按钮的那一步是场景布置，不是被测的滚入路径。
    #[gpui::test]
    fn subagent_offscreen_focus_reveals_and_keeps_visible_tool_toggle(
        cx: &mut gpui::TestAppContext,
    ) {
        fn fully_below(
            item: gpui::Bounds<gpui::Pixels>,
            viewport: gpui::Bounds<gpui::Pixels>,
        ) -> bool {
            f32::from(item.top()) >= f32::from(viewport.bottom()) - 0.5
        }
        fn fully_above(
            item: gpui::Bounds<gpui::Pixels>,
            viewport: gpui::Bounds<gpui::Pixels>,
        ) -> bool {
            f32::from(item.bottom()) <= f32::from(viewport.top()) + 0.5
        }
        fn intersects(
            item: gpui::Bounds<gpui::Pixels>,
            viewport: gpui::Bounds<gpui::Pixels>,
        ) -> bool {
            let visible = item.intersect(&viewport);
            f32::from(visible.size.width) > 0.5 && f32::from(visible.size.height) > 0.5
        }

        let receipt = (0..16)
            .map(|index| format!("tool-line-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut history = Vec::new();
        for sequence in 1..=36 {
            history.push(history_item(
                sequence,
                "user_message",
                serde_json::json!({ "text": format!("row-{sequence}") }),
            ));
        }
        history.push(history_item(
            37,
            "tool_started",
            serde_json::json!({
                "tool_name": "read_file",
                "status": "running",
                "detail": tool_context("call-read")
            }),
        ));
        history.push(history_item(
            38,
            "other",
            serde_json::json!({
                "status": "tool_arguments",
                "text": r#"{"path":"src/lib.rs"}"#,
                "detail": tool_context("call-read")
            }),
        ));
        history.push(history_item(
            39,
            "tool_completed",
            serde_json::json!({
                "tool_name": "read_file",
                "status": "succeeded",
                "text": receipt,
                "detail": tool_context("call-read")
            }),
        ));
        for sequence in 40..=70 {
            history.push(history_item(
                sequence,
                "user_message",
                serde_json::json!({ "text": format!("after-{sequence}") }),
            ));
        }
        let mut conversation = SubagentConversationState::default();
        assert!(conversation.select_agent("child-0"));
        let generation = conversation.begin_loading();
        conversation.apply_page("child-0", generation, &timeline_page(history));

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("rv07-subagent-offscreen-focus.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(560.0)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = Some("parent".into());
                view.projection.connection = crate::projection::ConnectionState::Connected {
                    instance_id: "test".into(),
                };
                view.projection.subagent_activity.session_id = Some("parent".into());
                view.projection.subagent_activity.agents = vec![crate::projection::SubagentInfo {
                    agent_id: "child-0".into(),
                    session_id: "child-0".into(),
                    parent_run_id: "run-0".into(),
                    title: "Agent 0".into(),
                    provider_id: "test-provider".into(),
                    model_id: "test-model".into(),
                    status: "completed".into(),
                    result: None,
                    effort: None,
                }];
                view.projection.subagent_conversation = conversation;
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.inspector_tab = super::super::inspector::InspectorTab::Subagent;
                view.close_open_menu(cx);
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();

        let (previous, target, offset_before) = cx.update(|window, cx| {
            let view = view.read(cx);
            let scroll = view.subagent_conversation_scroll.handle();
            let overflow = f32::from(scroll.max_offset().height);
            assert!(
                overflow > metrics::SCROLL_EPSILON,
                "fixture must overflow the transcript, max_offset={overflow}"
            );
            let viewport = scroll.bounds();
            let keys = panel_items(&view.projection.subagent_conversation)
                .iter()
                .filter_map(|item| match item {
                    PanelItem::Tool { event_id, .. } => Some(format!("{event_id}:tool")),
                    PanelItem::RunState(_) | PanelItem::Error(_) => None,
                    other => other.event_id().map(|id| format!("{id}:item")),
                })
                .collect::<Vec<_>>();
            let index = keys.iter().position(|key| {
                view.subagent_item_layouts
                    .get(key)
                    .is_some_and(|layout| fully_below(layout.bounds(), viewport))
            });
            let Some(index) = index else {
                panic!(
                    "no body item is fully below the viewport, height={}",
                    f32::from(viewport.size.height)
                );
            };
            assert!(index > 0, "the first body item should still be on screen");
            let target = keys[index].clone();
            let previous = keys[index - 1].clone();
            let ax_id = format!(
                "subagent-msg-{}",
                target.strip_suffix(":item").expect("message focus key")
            );
            assert!(
                view.accessibility_tree(window, cx).find(&ax_id).is_none(),
                "{ax_id} is offscreen and must not be published yet"
            );
            let previous_focus = view
                .subagent_item_focus
                .get(&previous)
                .cloned()
                .expect("previous tab stop");
            window.focus(&previous_focus);
            window.focus_next();
            (previous, target, scroll.offset())
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = view.read(cx);
            assert_eq!(
                focused_subagent_key(view, window).as_deref(),
                Some(target.as_str())
            );
            assert_ne!(previous, target);
            let viewport = view.subagent_conversation_scroll.handle().bounds();
            let item = view
                .subagent_item_layouts
                .get(&target)
                .expect("focused layout")
                .bounds();
            assert!(
                intersects(item, viewport),
                "focus_next must scroll the item into the transcript: item={item:?} viewport={viewport:?}"
            );
            assert_ne!(
                view.subagent_conversation_scroll.handle().offset().y,
                offset_before.y,
                "the product path must move the transcript, not leave the test to scroll"
            );
            let ax_id = format!(
                "subagent-msg-{}",
                target.strip_suffix(":item").unwrap()
            );
            let tree = view.accessibility_tree(window, cx);
            let node = tree
                .find(&ax_id)
                .unwrap_or_else(|| panic!("{ax_id} must be published once focused"));
            assert!(node.focused);
            assert!(node.actions.contains(&AxAction::Focus));
        });

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.toggle_subagent_detail("hist-37:tool", cx);
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.blur();
            view.update(cx, |view, _cx| {
                let scroll = view.subagent_conversation_scroll.handle();
                let viewport = scroll.bounds();
                let toggle = view
                    .subagent_item_layouts
                    .get(&subagent_tool_result_layout_key("hist-37"))
                    .expect("expanded tool publishes a result toggle layout")
                    .bounds();
                let delta = f32::from(viewport.origin.y) - f32::from(toggle.origin.y);
                let mut offset = scroll.offset();
                let max = f32::from(scroll.max_offset().height);
                let y = (f32::from(offset.y) + delta).clamp(-max, 0.0);
                offset.y = px(y);
                scroll.set_offset(offset);
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            {
                let current = view.read(cx);
                let viewport = current.subagent_conversation_scroll.handle().bounds();
                let header = current
                    .subagent_item_layouts
                    .get("hist-37:tool-header")
                    .expect("tool header layout")
                    .bounds();
                let toggle = current
                    .subagent_item_layouts
                    .get(&subagent_tool_result_layout_key("hist-37"))
                    .expect("tool result toggle layout")
                    .bounds();
                assert!(
                    fully_above(header, viewport),
                    "header must be offscreen: header={header:?} viewport={viewport:?}"
                );
                assert!(
                    intersects(toggle, viewport),
                    "toggle must stay visible: toggle={toggle:?} viewport={viewport:?}"
                );
                let tree = current.accessibility_tree(window, cx);
                assert!(tree.find("subagent-tool-toggle-hist-37").is_none());
                let node = tree
                    .find("subagent-tool-result-toggle-hist-37")
                    .expect("visible result toggle stays in the AX tree");
                assert_eq!(node.label, "Show full result");
                assert!(node.actions.contains(&AxAction::Focus));
                assert!(node.actions.contains(&AxAction::Press));
            }
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: "subagent-tool-result-toggle-hist-37".into(),
                        action: AxAction::Focus,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.update(|window, cx| {
            {
                let current = view.read(cx);
                let viewport = current.subagent_conversation_scroll.handle().bounds();
                let toggle = current
                    .subagent_item_layouts
                    .get(&subagent_tool_result_layout_key("hist-37"))
                    .expect("toggle layout")
                    .bounds();
                assert!(intersects(toggle, viewport));
                assert_eq!(
                    focused_subagent_key(current, window).as_deref(),
                    Some("hist-37:result")
                );
                let tree = current.accessibility_tree(window, cx);
                let node = tree
                    .find("subagent-tool-result-toggle-hist-37")
                    .expect("focused toggle is still published");
                assert!(node.focused);
            }
            view.update(cx, |view, cx| {
                view.handle_accessibility_request(
                    crate::ui::accessibility::AxRequest {
                        identifier: "subagent-tool-result-toggle-hist-37".into(),
                        action: AxAction::Press,
                        value: None,
                    },
                    window,
                    cx,
                );
            });
            let view = view.read(cx);
            assert!(view.expanded_subagent_details.contains("hist-37:result"));
            assert_eq!(
                view.accessibility_tree(window, cx)
                    .find("subagent-tool-result-toggle-hist-37")
                    .map(|node| node.label.as_str()),
                Some("Show less")
            );
        });
    }

    /// RV-07：同帧 follow_new_content 把滚底标志留到 prepaint，会盖掉 reveal
    /// 的 set_offset。滚底落定后不得登记成功，也不得按被盖掉的中间偏移重估
    /// following。下一帧目标进入视口后才登记，AX 随之发布且 focused。
    #[gpui::test]
    fn subagent_same_frame_follow_does_not_confirm_offscreen_reveal(cx: &mut gpui::TestAppContext) {
        #[derive(Clone, Debug)]
        struct Snap {
            revealed: Option<String>,
            following: bool,
            at_bottom: bool,
            offset_y: f32,
            intersects: bool,
        }

        let mut history = Vec::new();
        for sequence in 1..=64 {
            history.push(history_item(
                sequence,
                "user_message",
                serde_json::json!({ "text": format!("row-{sequence}") }),
            ));
        }
        let mut conversation = SubagentConversationState::default();
        assert!(conversation.select_agent("child-0"));
        let generation = conversation.begin_loading();
        conversation.apply_page("child-0", generation, &timeline_page(history));

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("rv07-subagent-same-frame-follow.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(560.0)));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = Some("parent".into());
                view.projection.connection = crate::projection::ConnectionState::Connected {
                    instance_id: "test".into(),
                };
                view.projection.subagent_activity.session_id = Some("parent".into());
                view.projection.subagent_activity.agents = vec![crate::projection::SubagentInfo {
                    agent_id: "child-0".into(),
                    session_id: "child-0".into(),
                    parent_run_id: "run-0".into(),
                    title: "Agent 0".into(),
                    provider_id: "test-provider".into(),
                    model_id: "test-model".into(),
                    status: "completed".into(),
                    result: None,
                    effort: None,
                }];
                view.projection.subagent_conversation = conversation;
                view.inspector_open = true;
                view.inspector_motion.width(
                    true,
                    true,
                    std::time::Instant::now() - std::time::Duration::from_secs(1),
                );
                view.inspector_tab = super::super::inspector::InspectorTab::Subagent;
                view.close_open_menu(cx);
                cx.notify();
            });
        });
        cx.refresh().unwrap();
        cx.run_until_parked();

        let target = cx.update(|window, cx| {
            let view = view.read(cx);
            let scroll = view.subagent_conversation_scroll.handle();
            let max = f32::from(scroll.max_offset().height);
            let offset_y = f32::from(scroll.offset().y);
            assert!(
                max > metrics::SCROLL_EPSILON,
                "fixture must overflow the transcript, max_offset={max}"
            );
            assert!(view.subagent_conversation_scroll.is_following());
            assert!(
                !view.subagent_conversation_scroll.is_scrolled_to_bottom(),
                "fixture must start away from the bottom"
            );
            let viewport = scroll.bounds();
            let shift = -max - offset_y;
            let target = panel_items(&view.projection.subagent_conversation)
                .iter()
                .filter_map(|item| match item {
                    PanelItem::UserMessage { event_id, .. }
                    | PanelItem::UserFollowUp { event_id, .. } => Some(format!("{event_id}:item")),
                    _ => None,
                })
                .find(|key| {
                    let Some(bounds) = view
                        .subagent_item_layouts
                        .get(key)
                        .map(|layout| layout.bounds())
                    else {
                        return false;
                    };
                    let top = f32::from(bounds.top());
                    let height = f32::from(bounds.size.height);
                    let below = top >= f32::from(viewport.bottom()) - 0.5;
                    let above_after = top + shift + height <= f32::from(viewport.top()) + 0.5;
                    below && above_after && height > 1.0
                });
            let target = target.unwrap_or_else(|| {
                panic!(
                    "need a row that is below the viewport now and above it after scroll-to-bottom"
                )
            });
            let ax_id = format!(
                "subagent-msg-{}",
                target.strip_suffix(":item").expect("message focus key")
            );
            assert!(
                view.accessibility_tree(window, cx).find(&ax_id).is_none(),
                "{ax_id} is offscreen and must not be published yet"
            );
            target
        });

        let snaps = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Snap>::new()));
        let _observe = {
            let snaps = snaps.clone();
            let watched = target.clone();
            cx.update(|_, cx| {
                cx.observe(&view, move |entity, cx| {
                    let current = entity.read(cx);
                    let scroll = current.subagent_conversation_scroll.handle();
                    let viewport = scroll.bounds();
                    let intersects = current
                        .subagent_item_layouts
                        .get(&watched)
                        .map(|layout| layout.bounds())
                        .is_some_and(|bounds| {
                            let visible = bounds.intersect(&viewport);
                            f32::from(visible.size.width) > 0.0
                                && f32::from(visible.size.height) > 0.0
                        });
                    snaps.borrow_mut().push(Snap {
                        revealed: current.subagent_revealed_focus.clone(),
                        following: current.subagent_conversation_scroll.is_following(),
                        at_bottom: current.subagent_conversation_scroll.is_scrolled_to_bottom(),
                        offset_y: f32::from(scroll.offset().y),
                        intersects,
                    });
                })
            })
        };
        cx.update(|window, cx| {
            let focus = view
                .read(cx)
                .subagent_item_focus
                .get(&target)
                .cloned()
                .expect("offscreen body focus handle");
            window.focus(&focus);
            view.update(cx, |view, cx| {
                view.subagent_conversation_scroll.follow_new_content();
                cx.notify();
            });
        });

        let recorded = snaps.borrow().clone();
        let overwritten = recorded.iter().find(|snap| {
            snap.revealed.is_none() && snap.at_bottom && snap.following && !snap.intersects
        });
        let overwritten = overwritten.unwrap_or_else(|| {
            panic!(
                "same-frame scroll-to-bottom recorded reveal or synced the intermediate offset: {recorded:?}"
            )
        });
        cx.update(|window, cx| {
            let current = view.read(cx);
            assert_eq!(
                current.subagent_revealed_focus.as_deref(),
                Some(target.as_str())
            );
            assert_eq!(
                focused_subagent_key(current, window).as_deref(),
                Some(target.as_str())
            );
            assert_eq!(
                current.subagent_conversation_scroll.is_following(),
                current.subagent_conversation_scroll.is_scrolled_to_bottom(),
                "following must match the settled offset"
            );
            assert!(
                !current.subagent_conversation_scroll.is_scrolled_to_bottom(),
                "the later frame must leave the bottom to show the focused row"
            );
            let offset_y = f32::from(current.subagent_conversation_scroll.handle().offset().y);
            assert!(
                (offset_y - overwritten.offset_y).abs() > 1.0,
                "settled offset {offset_y} must move off the same-frame bottom {}",
                overwritten.offset_y
            );
            let viewport = current.subagent_conversation_scroll.handle().bounds();
            let item = current
                .subagent_item_layouts
                .get(&target)
                .expect("focused layout")
                .bounds();
            let visible = item.intersect(&viewport);
            assert!(
                f32::from(visible.size.width) > 0.0 && f32::from(visible.size.height) > 0.0,
                "focused row must intersect the viewport: item={item:?} viewport={viewport:?}"
            );
            let ax_id = format!(
                "subagent-msg-{}",
                target.strip_suffix(":item").expect("message focus key")
            );
            let tree = current.accessibility_tree(window, cx);
            let node = tree
                .find(&ax_id)
                .unwrap_or_else(|| panic!("{ax_id} must be published once focused"));
            assert!(node.focused);
            assert!(node.actions.contains(&AxAction::Focus));
        });
        drop(_observe);
    }

    /// RV-06：100/125/150 与 1440×1024 侧栏 / 1080×720 中央下，子代理 chip
    /// 按正文增高。超宽 emoji 放在第一枚并保持选中：scroll 为 0 时它贴着
    /// 视口左缘，换行后 chip 不宽于视口，标题、状态点和焦点环不再被裁。
    /// 一次 notify 内自行收敛，不靠测试里的额外 refresh。最后一个 chip 的
    /// reveal 不在这条验收里：gpui 的 scroll_to_active_item 对照上一帧 port。
    /// 只验证布局和字形测量，不是真实像素验收。
    #[gpui::test]
    fn subagent_chip_labels_fit_text_scales(cx: &mut gpui::TestAppContext) {
        fn agent(id: &str, title: &str, status: &str) -> crate::projection::SubagentInfo {
            crate::projection::SubagentInfo {
                agent_id: id.into(),
                session_id: id.into(),
                parent_run_id: "run-0".into(),
                title: title.into(),
                provider_id: "test-provider".into(),
                model_id: "test-model".into(),
                status: status.into(),
                result: None,
                effort: None,
            }
        }
        fn px_of(value: impl Into<f32>) -> f32 {
            value.into()
        }
        fn contains(
            outer: gpui::Bounds<gpui::Pixels>,
            inner: gpui::Bounds<gpui::Pixels>,
            tol: f32,
        ) -> bool {
            px_of(inner.left()) >= px_of(outer.left()) - tol
                && px_of(inner.right()) <= px_of(outer.right()) + tol
                && px_of(inner.top()) >= px_of(outer.top()) - tol
                && px_of(inner.bottom()) <= px_of(outer.bottom()) + tol
        }

        let wide_title = "😀".repeat(24);
        let titles = [
            ("wide", wide_title.clone(), "running"),
            ("run", "测".repeat(30), "running"),
            ("wait", "glyphypq".to_string(), "waiting"),
            ("done", "A".to_string(), "completed"),
            ("fail", "签".repeat(28), "failed"),
        ];
        for (id, raw, _) in &titles {
            let title = super::short_title(raw);
            assert!(!title.is_empty(), "{id} title empty");
            assert!(title.chars().count() <= 24, "{id} title longer than 24");
            assert!(raw.starts_with(&title), "{id} title is not a prefix");
            assert_ne!(title, "…", "{id} title collapsed to an ellipsis");
        }

        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("rv06-subagent-chip-font.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = Some("parent".into());
                view.projection.connection = crate::projection::ConnectionState::Connected {
                    instance_id: "test".into(),
                };
                view.projection.subagent_activity.session_id = Some("parent".into());
                view.projection.subagent_activity.agents = titles
                    .iter()
                    .map(|(id, title, status)| agent(id, title, status))
                    .collect();
                assert!(view.projection.subagent_conversation.select_agent("wide"));
                view.subagent_reveal_selected = true;
                view.inspector_open = true;
                view.inspector_tab = super::super::inspector::InspectorTab::Subagent;
                view.close_open_menu(cx);
                cx.notify();
            });
        });

        let mut done_heights = Vec::new();
        for (width, height, side) in [(1440.0_f32, 1024.0_f32, true), (1080.0, 720.0, false)] {
            for scale in [
                font::TextScale::Percent100,
                font::TextScale::Percent125,
                font::TextScale::Percent150,
            ] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
                cx.run_until_parked();
                cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(scale.rem_pixels()));
                    view.update(cx, |view, cx| {
                        view.text_scale = scale;
                        view.inspector_open = true;
                        view.inspector_tab = super::super::inspector::InspectorTab::Subagent;
                        if view.projection.subagent_conversation.agent_id.as_deref() != Some("wide")
                        {
                            view.projection.subagent_conversation.select_agent("wide");
                        }
                        let large = scale == font::TextScale::Percent150;
                        // Rust 1.96 的 duration_since 对更早的 Instant 返回 0，
                        // 回拨开始时间不会结束动画。这里直接落到最终宽度。
                        if super::super::shell_layout::can_fit_side_inspector(
                            gpui::px(width),
                            large,
                        ) {
                            view.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
                        } else {
                            view.inspector_motion.snap(0.0);
                        }
                        // 一次 notify。宽度收敛和选中 chip 滚入都由产品自己完成。
                        view.subagent_reveal_selected = true;
                        cx.notify();
                    });
                });

                let run_chip = cx.debug_bounds("subagent-chip-run").expect("run chip");
                let run_label = cx.debug_bounds("subagent-label-run").expect("run label");
                let wait_label = cx.debug_bounds("subagent-label-wait").expect("wait label");
                let done_label = cx.debug_bounds("subagent-label-done").expect("done label");
                let fail_label = cx.debug_bounds("subagent-label-fail").expect("fail label");
                let wide_chip = cx.debug_bounds("subagent-chip-wide").expect("wide chip");
                let wide_dot = cx.debug_bounds("subagent-dot-wide").expect("wide dot");
                let wide_label = cx.debug_bounds("subagent-label-wide").expect("wide label");
                let done_chip = cx.debug_bounds("subagent-chip-done").expect("done chip");
                let inspector = cx.debug_bounds("shell-inspector").expect("inspector");
                let chips = [
                    cx.debug_bounds("subagent-chip-run").expect("run"),
                    cx.debug_bounds("subagent-chip-wait").expect("wait"),
                    cx.debug_bounds("subagent-chip-done").expect("done"),
                    cx.debug_bounds("subagent-chip-fail").expect("fail"),
                    cx.debug_bounds("subagent-chip-wide").expect("wide"),
                ];

                let done_h = cx.update(|window, cx| {
                    let view = view.read(cx);
                    let large = scale == font::TextScale::Percent150;
                    let shell = super::super::shell_layout::resolve(
                        window.viewport_size().width,
                        view.inspector_open,
                        large,
                    );
                    assert_eq!(shell.placement.is_side(), side, "{width}x{height} placement");
                    if side {
                        assert!(
                            (px_of(inspector.size.width) - metrics::INSPECTOR_WIDTH).abs() <= 1.0,
                            "side inspector width {} origin {} viewport {}",
                            px_of(inspector.size.width),
                            px_of(inspector.origin.x),
                            px_of(window.viewport_size().width)
                        );
                    } else {
                        assert!(
                            shell.placement.is_center(),
                            "1080 inspector must be centered"
                        );
                        assert!(px_of(inspector.size.width) > 400.0);
                    }
                    let port = view.subagent_chips_scroll.bounds();
                    let text_max = super::subagent_chip_text_max(window, px_of(port.size.width));
                    let rem = px_of(window.rem_size());
                    let expected_h =
                        (20.0 * f32::from(scale.percent()) / 100.0 + 2.0 * 0.25 * rem)
                            .max(metrics::ICON_BUTTON_MIN_SIZE);
                    eprintln!(
                        "RV06 scale={}% window={width}x{height} placement={} port={:.1}x{:.1} wide={:.1}x{:.1} wide_label={:.1}x{:.1} done_h={:.1} expected_h={:.1} offset_x={:.1} wide_left={:.1} port_left={:.1}",
                        scale.percent(),
                        if side { "side" } else { "center" },
                        px_of(port.size.width),
                        px_of(port.size.height),
                        px_of(wide_chip.size.width),
                        px_of(wide_chip.size.height),
                        px_of(wide_label.size.width),
                        px_of(wide_label.size.height),
                        px_of(done_chip.size.height),
                        expected_h,
                        px_of(view.subagent_chips_scroll.offset().x),
                        px_of(wide_chip.left()),
                        px_of(port.left()),
                    );
                    let wide_short = super::short_title(&wide_title);
                    let wide_full = super::super::input_area::shape_label_width(
                        window,
                        &wide_short,
                        font::BASE,
                    )
                    .ceil();
                    let wide_max = text_max.expect("chip port width");
                    if side {
                        assert!(
                            wide_full > wide_max,
                            "emoji short_title {wide_full} must exceed side text area {wide_max}"
                        );
                    }
                    if wide_full > wide_max {
                        assert!(
                            px_of(wide_label.size.height) > px_of(done_label.size.height) + 4.0,
                            "emoji title must wrap: label {} one_line {}",
                            px_of(wide_label.size.height),
                            px_of(done_label.size.height)
                        );
                    } else {
                        assert!(
                            (px_of(wide_label.size.height) - px_of(done_label.size.height)).abs()
                                <= 2.0,
                            "emoji title stays one line when it fits"
                        );
                    }
                    assert!(
                        px_of(view.subagent_chips_scroll.offset().x).abs() <= 1.0,
                        "wide-first chip must stay at scroll 0, offset {}",
                        px_of(view.subagent_chips_scroll.offset().x)
                    );
                    assert!(
                        (px_of(wide_chip.left()) - px_of(port.left())).abs() <= 1.0,
                        "wide chip must sit on the port left edge: chip={wide_chip:?} port={port:?}"
                    );
                    assert!(
                        px_of(wide_chip.size.width) <= px_of(port.size.width) + 1.0,
                        "wrapped wide chip is wider than the port: chip={wide_chip:?} port={port:?}"
                    );
                    assert!(
                        contains(port, wide_chip, 1.0),
                        "selected wide chip is outside the port: chip={wide_chip:?} port={port:?}"
                    );
                    assert!(contains(wide_chip, wide_label, 1.0), "wide label leaves its chip");
                    assert!(contains(port, wide_dot, 1.0), "wide dot leaves the port");
                    assert!((px_of(wide_dot.size.width) - 6.0).abs() <= 0.5);
                    assert!((px_of(wide_dot.size.height) - 6.0).abs() <= 0.5);
                    let ring = metrics::FOCUS_RING_WIDTH - 0.5;
                    assert!(px_of(wide_dot.left()) >= px_of(wide_chip.left()) + ring);
                    assert!(px_of(wide_chip.right()) >= px_of(wide_dot.right()) + ring);
                    assert!(px_of(wide_dot.top()) >= px_of(wide_chip.top()) + ring);
                    assert!(px_of(wide_chip.bottom()) >= px_of(wide_dot.bottom()) + ring);
                    assert!(
                        px_of(wide_dot.right()) <= px_of(wide_label.left()) + 0.5,
                        "dot overlaps label"
                    );
                    assert!(px_of(done_chip.size.width) + 8.0 < px_of(run_chip.size.width));

                    let mut ordered = chips;
                    ordered.sort_by(|left, right| {
                        px_of(left.left())
                            .partial_cmp(&px_of(right.left()))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for pair in ordered.windows(2) {
                        assert!(
                            px_of(pair[1].left()) + 0.5 >= px_of(pair[0].right()),
                            "chips overlap: {:?} {:?}",
                            pair[0],
                            pair[1]
                        );
                    }

                    let font_size = font::BASE.to_pixels(window.rem_size());
                    let line_height = font::CONTROL_LINE_HEIGHT.to_pixels(window.rem_size());
                    let style = window.text_style();
                    for (id, raw, _) in &titles {
                        let short = super::short_title(raw);
                        let label = match *id {
                            "run" => run_label,
                            "wait" => wait_label,
                            "done" => done_label,
                            "fail" => fail_label,
                            "wide" => wide_label,
                            _ => unreachable!(),
                        };
                        let expected = super::subagent_chip_label_width(window, &short, text_max);
                        assert!(
                            (px_of(label.size.width) - expected).abs() <= 1.0,
                            "{id} label width {} != {expected}",
                            px_of(label.size.width)
                        );
                        let run = gpui::TextRun {
                            len: short.len(),
                            font: style.font(),
                            color: style.color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let lines = window
                            .text_system()
                            .shape_text(
                                short.clone().into(),
                                font_size,
                                &[run],
                                Some(gpui::px(px_of(label.size.width))),
                                None,
                            )
                            .expect("shape chip label");
                        let shaped: String = lines.iter().map(|line| line.text.to_string()).collect();
                        assert_eq!(shaped, short, "{id} label text was clipped");
                        let shaped_h: f32 = lines
                            .iter()
                            .map(|line| px_of(line.size(line_height).height))
                            .sum();
                        assert!(
                            (shaped_h - px_of(label.size.height)).abs() <= 2.0,
                            "{id} label height {} != shaped {shaped_h}",
                            px_of(label.size.height)
                        );
                    }
                    assert!(
                        (px_of(done_chip.size.height) - expected_h).abs() <= 1.5,
                        "done chip height {} expected {expected_h}",
                        px_of(done_chip.size.height)
                    );
                    px_of(done_chip.size.height)
                });
                done_heights.push((width, scale.percent(), done_h));

                let before = cx
                    .debug_bounds("subagent-chip-wide")
                    .expect("wide before focus");
                cx.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        let handle = view
                            .subagent_agent_focus
                            .get("wide")
                            .expect("wide focus")
                            .clone();
                        window.focus(&handle);
                        super::super::components::focus_ring::set_keyboard_focus(true, window, cx);
                    });
                });
                let after = cx
                    .debug_bounds("subagent-chip-wide")
                    .expect("wide after focus");
                let focused_port = cx.update(|_, cx| view.read(cx).subagent_chips_scroll.bounds());
                assert!(
                    (px_of(before.origin.x) - px_of(after.origin.x)).abs() <= 0.5
                        && (px_of(before.origin.y) - px_of(after.origin.y)).abs() <= 0.5
                        && (px_of(before.size.width) - px_of(after.size.width)).abs() <= 0.5
                        && (px_of(before.size.height) - px_of(after.size.height)).abs() <= 0.5,
                    "focus ring changed chip bounds {before:?} -> {after:?}"
                );
                assert!(
                    contains(focused_port, after, 1.0),
                    "focused wide chip leaves the port: chip={after:?} port={focused_port:?}"
                );
            }
        }
        for window_width in [1440.0_f32, 1080.0] {
            let heights: Vec<f32> = done_heights
                .iter()
                .filter(|(width, _, _)| *width == window_width)
                .map(|(_, _, height)| *height)
                .collect();
            assert_eq!(heights.len(), 3);
            assert!(
                heights[0] < heights[1] && heights[1] < heights[2],
                "done chip height must grow with text scale at {window_width}: {heights:?}"
            );
        }

        // 中央栏回到侧栏：一次 notify，不额外 refresh。窄视口必须把 emoji 换行。
        cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(1024.0)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.set_rem_size(gpui::px(font::TextScale::Percent150.rem_pixels()));
            view.update(cx, |view, cx| {
                view.text_scale = font::TextScale::Percent150;
                view.inspector_motion.snap(metrics::INSPECTOR_WIDTH);
                view.subagent_reveal_selected = true;
                cx.notify();
            });
        });
        let wide_chip = cx.debug_bounds("subagent-chip-wide").expect("wide chip");
        let wide_label = cx.debug_bounds("subagent-label-wide").expect("wide label");
        let wide_dot = cx.debug_bounds("subagent-dot-wide").expect("wide dot");
        let done_label = cx.debug_bounds("subagent-label-done").expect("done label");
        let (port, wide_full, wide_max, offset_x) = cx.update(|window, cx| {
            let view = view.read(cx);
            let port = view.subagent_chips_scroll.bounds();
            let offset_x = px_of(view.subagent_chips_scroll.offset().x);
            let short = super::short_title(&wide_title);
            let full =
                super::super::input_area::shape_label_width(window, &short, font::BASE).ceil();
            let max = super::subagent_chip_text_max(window, px_of(port.size.width)).expect("port");
            (port, full, max, offset_x)
        });
        eprintln!(
            "RV06 boundary 1440x1024 scale=150% wide={:.1}x{:.1} label={:.1}x{:.1} dot={:.1}x{:.1} port={:.1}x{:.1} full={wide_full:.1} text_max={wide_max:.1} offset_x={offset_x:.1} wide_left={:.1} port_left={:.1}",
            px_of(wide_chip.size.width),
            px_of(wide_chip.size.height),
            px_of(wide_label.size.width),
            px_of(wide_label.size.height),
            px_of(wide_dot.size.width),
            px_of(wide_dot.size.height),
            px_of(port.size.width),
            px_of(port.size.height),
            px_of(wide_chip.left()),
            px_of(port.left())
        );
        assert!(
            offset_x.abs() <= 1.0,
            "wide-first chip must stay at scroll 0 after returning to the side inspector, offset {offset_x}"
        );
        assert!(
            (px_of(wide_chip.left()) - px_of(port.left())).abs() <= 1.0,
            "wide chip must sit on the port left edge: chip={wide_chip:?} port={port:?}"
        );
        assert!(
            px_of(wide_chip.size.width) <= px_of(port.size.width) + 1.0,
            "wrapped wide chip is wider than the port: chip={wide_chip:?} port={port:?}"
        );
        assert!(
            wide_full > wide_max,
            "emoji {wide_full} must exceed text area {wide_max}"
        );
        assert!(
            px_of(wide_label.size.height) > px_of(done_label.size.height) + 4.0,
            "emoji title must wrap after returning to the side inspector: wide {} one_line {}",
            px_of(wide_label.size.height),
            px_of(done_label.size.height)
        );
        assert!(
            contains(port, wide_chip, 1.0),
            "selected chip must fit after one notify: chip={wide_chip:?} port={port:?}"
        );
        assert!(
            contains(port, wide_label, 1.0),
            "wrapped title leaves the port"
        );
        assert!(contains(port, wide_dot, 1.0), "status dot leaves the port");
    }
}
