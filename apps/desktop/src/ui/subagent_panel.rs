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

/// 工具参数展示（RV-04）：JSON 参数 pretty 打印，畸形 / 缺失回退原文
/// 或占位（与主 Timeline arguments_secondary 同口径，不翻译数据内容）。
fn subagent_arguments_display(arguments: Option<&str>) -> String {
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

/// 工具结果 / 错误预览（RV-04）：与主 Timeline RESULT_PREVIEW_LINES 同
/// 口径——超过 10 行折叠为预览，展开按钮补全剩余。
fn subagent_detail_preview(text: &str, expanded: bool) -> (String, bool) {
    let can_expand = text.lines().count() > super::timeline_entry::RESULT_PREVIEW_LINES;
    if !can_expand || expanded {
        (text.to_string(), can_expand)
    } else {
        (
            text.lines()
                .take(super::timeline_entry::RESULT_PREVIEW_LINES)
                .collect::<Vec<_>>()
                .join("\n"),
            true,
        )
    }
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

/// 代理标题截断（chip 与列表行共用口径）。
pub(super) fn short_title(title: &str) -> String {
    title.chars().take(24).collect()
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
            let dot_color = subagent_status_color(&agent.status);
            chips = chips.child(
                div()
                    .id(SharedString::from(format!(
                        "subagent-chip-layout-{}",
                        agent.agent_id
                    )))
                    .track_scroll(&layout)
                    .child(
                        Button::new(format!("subagent-select-{}", agent.agent_id))
                            .disabled(!matches!(
                                self.projection.connection,
                                crate::projection::ConnectionState::Connected { .. }
                            ))
                            .track_focus(&focus)
                            .variant(if is_selected {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Raised
                            })
                            .radius(6.0)
                            .text_size(font::BODY_SM)
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
                                            .bg(dot_color),
                                    )
                                    .child(short_title(&agent.title)),
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
                        .text_size(font::SM)
                        .text_color(dark().semantic.danger_text)
                        .child(error),
                )
            })
            .when_some(activity_stale, |panel, reason| {
                panel.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(font::SM)
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
        if let Some(result) = &agent.result {
            let result_key = format!("{}:result", agent.agent_id);
            visible_keys.insert(result_key.clone());
            if subagent_result_is_long(result) {
                visible_keys.insert(format!("{result_key}:toggle"));
            }
        }
        self.subagent_item_layouts
            .retain(|key, _| visible_keys.contains(key));
        if conversation.loading && items.is_empty() {
            transcript = transcript.child(
                div()
                    .text_size(font::SM)
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
        // 元信息行：名称后跟生效模型与状态（与浮层行同源口径）。
        let status = super::changes::subagent_status_label(&agent.status);
        transcript = transcript.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_wrap()
                .child(
                    div()
                        .flex_row()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().child(agent.title.clone())),
                )
                .child(
                    Label::new(agent.model_id.clone())
                        .size(font::XS)
                        .color(dark().text.tertiary),
                )
                .child(
                    Label::new(status)
                        .size(font::XS)
                        .color(dark().text.secondary),
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
            let long = subagent_result_is_long(result);
            let expanded = !long || self.expanded_subagent_details.contains(&result_key);
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
            result_box = result_box.child(if expanded {
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
                    .text_size(font::SM)
                    .text_color(dark().text.secondary)
                    .child(subagent_result_summary(result))
                    .into_any_element()
            });
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
                div()
                    .id(SharedString::from(format!("subagent-item-{event_id}")))
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
                            .text_size(font::SM)
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
                div()
                    .id(SharedString::from(format!("subagent-item-{event_id}")))
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
                            .text_size(font::SM)
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
                div()
                    .id(SharedString::from(format!("subagent-item-{event_id}")))
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
                div()
                    .id(SharedString::from(format!("subagent-item-{event_id}")))
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
                            .text_size(font::SM)
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
                let focus = self
                    .subagent_item_focus
                    .entry(tool_key.clone())
                    .or_insert_with(|| cx.focus_handle().tab_stop(true))
                    .clone();
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
                                .text_size(font::SM)
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
                    let detail_text = detail
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| t("tool.result_missing").into());
                    let (preview, can_expand) =
                        subagent_detail_preview(&detail_text, result_expanded);
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
                                .text_size(font::XS)
                                .line_height(font::from_pixels(20.0))
                                .font_family("monospace")
                                .text_color(dark().text.secondary)
                                .child(subagent_arguments_display(arguments.as_deref())),
                        )
                        .child(
                            div()
                                .text_size(font::XS)
                                .line_height(font::from_pixels(20.0))
                                .font_family("monospace")
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
                .child(state)
                .into_any_element(),
            PanelItem::Error(text) => div()
                .text_size(font::SM)
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
        let focus = self
            .subagent_item_focus
            .entry(result_key.to_string())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
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
                    .text_size(font::BODY_SM)
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
        let focus = self
            .subagent_item_focus
            .entry(result_key.to_string())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_button_id = button_id.clone();
        let click_key = result_key.to_string();
        let activate_button_id = button_id.clone();
        let activate_key = result_key.to_string();
        Button::new(button_id)
            .variant(ButtonVariant::Ghost)
            .text_size(font::BODY_SM)
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
            }))
    }
}

fn subagent_placeholder(title: &'static str, description: String) -> gpui::AnyElement {
    EmptyState::new()
        .icon(Icon::Subagents)
        .icon_size(px(metrics::ICON_SIZE))
        .child(
            Label::new(title)
                .size(font::BASE)
                .color(dark().text.primary),
        )
        .child(
            Label::new(description)
                .size(font::SM)
                .color(dark().text.secondary),
        )
        .into_any_element()
}

/// 「子代理」对话栏 AX：Group 节点携带当前面板状态摘要（被选代理 /
/// 加载 / 失败 / 空态），并按实测布局发布 chip / Refresh / 回底按钮
///（VoiceOver 可 Press；几何与 Inspector 页签同口径：tracked div 实测
/// bounds 按滚动视口裁剪，不发布无真实布局的行级节点）。
pub(super) fn subagent_ax(view: &AppView, frame: AxRect) -> AxNode {
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
                let Some(layout) = view.subagent_item_layouts.get(&key) else {
                    continue;
                };
                let Some(rect) = subagent_ax_rect(layout.bounds().intersect(&viewport)) else {
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
                            .action(AxAction::Press),
                        );
                    }
                    PanelItem::UserMessage { event_id, text } => {
                        node = node.child(
                            AxNode::new(
                                format!("subagent-msg-{event_id}"),
                                AxRole::StaticText,
                                t("subagents.task_from_main"),
                                rect,
                            )
                            .value(subagent_ax_text(text)),
                        );
                    }
                    PanelItem::UserFollowUp { event_id, text } => {
                        node = node.child(
                            AxNode::new(
                                format!("subagent-msg-{event_id}"),
                                AxRole::StaticText,
                                t("timeline.you"),
                                rect,
                            )
                            .value(subagent_ax_text(text)),
                        );
                    }
                    PanelItem::Assistant { event_id, text } => {
                        node = node.child(
                            AxNode::new(
                                format!("subagent-msg-{event_id}"),
                                AxRole::StaticText,
                                t("subagents.assistant_label"),
                                rect,
                            )
                            .value(subagent_ax_text(text)),
                        );
                    }
                    PanelItem::Thinking { event_id, text } => {
                        node = node.child(
                            AxNode::new(
                                format!("subagent-msg-{event_id}"),
                                AxRole::StaticText,
                                t("subagents.thinking_label"),
                                rect,
                            )
                            .value(subagent_ax_text(text)),
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
                            let long = subagent_result_is_long(result);
                            let expanded =
                                !long || view.expanded_subagent_details.contains(&result_key);
                            node = node.child(
                                AxNode::new(
                                    format!("subagent-result-{agent_id}"),
                                    AxRole::StaticText,
                                    t("subagents.result_to_main"),
                                    rect,
                                )
                                .value(if expanded {
                                    subagent_ax_text(result)
                                } else {
                                    subagent_result_summary(result)
                                }),
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
                                            .action(AxAction::Press),
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

    /// RV-11 主路径：断线保留子代理面板已加载的代理列表与选中对话，只清
    /// 在途瞬时标记并记录过期原因；阅读位置不重置贴底。
    #[gpui::test]
    fn disconnected_preserves_subagent_panel_content(cx: &mut gpui::TestAppContext) {
        let platform = std::sync::Arc::new(crate::platform::Platform::new());
        let socket = std::env::temp_dir().join("rv11-subagent-disconnected.sock");
        let (view, cx) = cx.add_window_view(|_, cx| AppView::new(platform, socket, None, cx));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.projection.active_session_id = Some("parent".into());
                view.projection.connection = crate::projection::ConnectionState::Connected {
                    instance_id: "test".into(),
                };
                let activity = &mut view.projection.subagent_activity;
                activity.session_id = Some("parent".into());
                activity.cancelling = Some("child-0".into());
                activity.agents = vec![crate::projection::SubagentInfo {
                    agent_id: "child-0".into(),
                    session_id: "child-0".into(),
                    parent_run_id: "run-0".into(),
                    title: "Agent 0".into(),
                    provider_id: "test-provider".into(),
                    model_id: "test-model".into(),
                    status: "running".into(),
                    result: None,
                    effort: None,
                }];
                view.projection
                    .subagent_conversation
                    .select_agent("child-0");
                let request_id = view.projection.subagent_activity.begin_loading("parent");
                view.handle_controller_event(
                    crate::controller::ControllerEvent::Disconnected {
                        reason: "socket closed".into(),
                    },
                    cx,
                );
                let activity = &view.projection.subagent_activity;
                assert_eq!(activity.agents.len(), 1, "断线保留代理列表");
                assert!(!activity.loading);
                assert!(activity.cancelling.is_none(), "在途取消不会再有回执");
                assert_eq!(
                    activity.stale_reason.as_deref(),
                    Some("socket closed"),
                    "记录原始 reason 供本地化前缀展示"
                );
                let conversation = &view.projection.subagent_conversation;
                assert_eq!(
                    conversation.agent_id.as_deref(),
                    Some("child-0"),
                    "断线保留选中代理"
                );
                assert!(!conversation.loading);
                assert!(matches!(
                    view.projection.connection,
                    crate::projection::ConnectionState::Disconnected { .. }
                ));
                view.select_subagent_agent("child-other", cx);
                assert_eq!(
                    view.projection.subagent_conversation.agent_id.as_deref(),
                    Some("child-0")
                );
                view.handle_controller_event(
                    crate::controller::ControllerEvent::SubagentListLoaded {
                        session_id: "parent".into(),
                        request_id,
                        data: pawork_client::SubagentListData { agents: Vec::new() },
                    },
                    cx,
                );
                assert_eq!(view.projection.subagent_activity.agents.len(), 1);
                assert_eq!(
                    view.projection.subagent_activity.stale_reason.as_deref(),
                    Some("socket closed")
                );
            });
        });
    }
}
