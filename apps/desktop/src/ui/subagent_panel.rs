//! Inspector「子代理」对话栏：Activity 浮层行点击或「+」菜单进入，展示
//! 被选子代理的运行过程与其和主代理的对话内容。
//! 数据同源：代理列表 / 状态 / 结果来自 Host subagent_list，过程时间线
//! 来自子会话 session_get 分页 + live 事件（独立 protocol reducer），
//! 不与主 Timeline 共享状态。

use gpui::{Context, Rgba, SharedString, Window, div, prelude::*, px};

use crate::projection::{SubagentConversationState, SubagentInfo, TimelineEntryKind};
use crate::ui::accessibility::{AxAction, AxNode, AxRect, AxRole};
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::empty_state::EmptyState;
use crate::ui::components::follow_scroll::BackToBottom;
use crate::ui::components::icon::{Icon, icon_sized};
use crate::ui::components::label::Label;
use crate::ui::i18n::t;
use crate::ui::theme::{dark, font, metrics};

use super::AppView;

/// 代理切换 chip 高度（与设置页 chip 同族，保持紧凑）。
const SUBAGENT_CHIP_HEIGHT: f32 = 24.0;
/// 对话栏渲染项（从子会话时间线条目预收集，避免渲染中混用可变借用）。
#[derive(Clone, Debug, PartialEq, Eq)]
enum PanelItem {
    UserMessage { event_id: String, text: String },
    UserFollowUp { event_id: String, text: String },
    Assistant { event_id: String, text: String },
    Thinking { event_id: String, text: String },
    Tool { name: String, status: String },
    RunState(String),
    Error(String),
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
            TimelineEntryKind::ToolCall { name, status, .. } => PanelItem::Tool { name, status },
            TimelineEntryKind::RunState(state) => PanelItem::RunState(state),
            TimelineEntryKind::Error(text) => PanelItem::Error(text),
        })
        .collect()
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

    pub(super) fn subagent_conversation_element(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_refresh = self.can_refresh_subagents();
        let can_cancel = self.can_cancel_subagent();
        let activity = &self.projection.subagent_activity;
        let activity_error = activity.error.clone();
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
        let visible_ids: std::collections::HashSet<&String> = agents
            .iter()
            .map(|agent| &agent.agent_id)
            .collect();
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
            chips = chips.child(
                div()
                    .text_size(font::XS)
                    .text_color(dark().text.tertiary)
                    .child(if bound {
                        t("subagents.empty")
                    } else if self.projection.active_session_id.is_none() {
                        t("subagents.no_session")
                    } else {
                        t("subagents.loading")
                    }),
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
            let tooltip = format!(
                "{} · {} · {}",
                agent.title, agent.model_id, status_label
            );
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
                            .track_focus(&focus)
                            .variant(if is_selected {
                                ButtonVariant::Primary
                            } else {
                                ButtonVariant::Raised
                            })
                            .height(px(SUBAGENT_CHIP_HEIGHT))
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

        let body = self.subagent_transcript_element(selected.as_ref(), &conversation, items, window, cx);
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
                if self
                    .projection
                    .subagent_conversation
                    .agent_id
                    .is_some()
                {
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
                return subagent_placeholder(
                    t("subagents.load_failed"),
                    reason.clone(),
                );
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
                .child(Label::new(status).size(font::XS).color(dark().text.secondary)),
        );
        for item in items {
            transcript = transcript.child(
                self.subagent_panel_item_element(item, window, cx),
            );
        }
        // 结果盒（主代理视角的子代理回执；completed/failed 才有）。
        if let Some(result) = &agent.result {
            transcript = transcript.child(
                div()
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
                    )
                    .child(
                        div()
                            .text_size(font::SM)
                            .text_color(dark().text.primary)
                            // 结果全文随外层 transcript 滚动：内嵌滚动区会
                            // 吃滚轮，指针停在上面时外层脱钩/跟滚失效。
                            .child(result.clone()),
                    ),
            );
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
            })
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
            PanelItem::UserMessage { event_id: _, text } => div()
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
                .into_any_element(),
            // 后续 user 回合不是 spawn 任务：普通用户消息样式，不冒充
            // 「主代理 → 子代理」任务盒。
            PanelItem::UserFollowUp { event_id: _, text } => div()
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
                .into_any_element(),
            // 子代理回复：复用主 Timeline 的 Markdown 渲染（代码块 / 列表 /
            // 表格同源，含复制入口）。
            PanelItem::Assistant { event_id, text } => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    Label::new(t("subagents.assistant_label"))
                        .size(font::XS)
                        .color(dark().text.tertiary),
                )
                .child(
                    super::markdown::message_body_element(
                        self,
                        cx,
                        window,
                        &format!("subagent-{event_id}"),
                        &text,
                        dark().text.primary,
                        false,
                    ),
                )
                .into_any_element(),
            PanelItem::Thinking { event_id: _, text } => div()
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
                .into_any_element(),
            PanelItem::Tool { name, status } => div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_1()
                .text_size(font::SM)
                .child(icon_sized(Icon::Tools, px(metrics::ICON_SM)))
                .child(
                    div()
                        .flex_row()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().child(name)),
                )
                .child(
                    Label::new(super::timeline::tool_status_label(&status))
                        .size(font::XS)
                        .color(dark().text.secondary),
                )
                .into_any_element(),
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
}

fn subagent_placeholder(title: &'static str, description: String) -> gpui::AnyElement {
    EmptyState::new()
        .icon(Icon::Subagents)
        .icon_size(px(metrics::ICON_SIZE))
        .child(Label::new(title).size(font::BASE).color(dark().text.primary))
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
    let bound = activity.session_id.is_some()
        && view.projection.active_session_id == activity.session_id;
    let mut value = if !bound {
        if view.projection.active_session_id.is_none() {
            t("subagents.no_session").to_string()
        } else {
            t("subagents.loading").to_string()
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
                    t(if activity.cancelling.is_some()
                        && activity.cancelling == conversation.agent_id
                    {
                        "subagents.cancelling"
                    } else {
                        "subagents.cancel"
                    }),
                    view.settings_menu_element_bounds("subagent-cancel", "subagent-cancel"),
                )
                .enabled(view.can_cancel_subagent())
                .action(AxAction::Press),
            );
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
