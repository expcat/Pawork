//! Inspector「子代理」对话栏：Activity 浮层行点击或「+」菜单进入，展示
//! 被选子代理的运行过程与其和主代理的对话内容。
//! 数据同源：代理列表 / 状态 / 结果来自 Host subagent_list，过程时间线
//! 来自子会话 session_get 分页 + live 事件（独立 protocol reducer），
//! 不与主 Timeline 共享状态。

use gpui::{Context, SharedString, Window, div, prelude::*, px};

use crate::projection::{SubagentConversationState, SubagentInfo, TimelineEntryKind};
use crate::ui::accessibility::{AxNode, AxRect, AxRole};
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
/// 思考文本盒限高：超出内部滚动，不撑爆对话栏。
const THINKING_BOX_MAX_HEIGHT: f32 = 160.0;

/// 对话栏渲染项（从子会话时间线条目预收集，避免渲染中混用可变借用）。
#[derive(Clone, Debug, PartialEq, Eq)]
enum PanelItem {
    UserMessage { event_id: String, text: String },
    Assistant { event_id: String, text: String },
    Thinking { event_id: String, text: String },
    Tool { name: String, status: String },
    RunState(String),
    Error(String),
}

fn panel_items(conversation: &SubagentConversationState) -> Vec<PanelItem> {
    conversation
        .timeline
        .iter()
        .map(|entry| match entry.kind.clone() {
            TimelineEntryKind::UserMessage { text } => PanelItem::UserMessage {
                event_id: entry.event_id.clone(),
                text,
            },
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

impl AppView {
    pub(super) fn subagent_conversation_element(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let activity = &self.projection.subagent_activity;
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
            let tooltip = format!("{} · {}", agent.title, agent.model_id);
            let click_id = agent.agent_id.clone();
            let activate_id = agent.agent_id.clone();
            chips = chips.child(
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
                    .label(short_title(&agent.title))
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
            );
        }
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
            .child(
                Button::new("subagent-refresh")
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
            );

        let body = self.subagent_transcript_element(selected.as_ref(), &conversation, items, window, cx);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
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
                            .id("subagent-result-text")
                            .text_size(font::SM)
                            .text_color(dark().text.primary)
                            .max_h(px(320.0))
                            .overflow_y_scroll()
                            .child(result.clone()),
                    ),
            );
        }
        transcript
            .when(!self.subagent_conversation_scroll.is_following(), |area| {
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
            PanelItem::Thinking { event_id, text } => div()
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
                        .id(SharedString::from(format!("subagent-thinking-{event_id}")))
                        .text_size(font::SM)
                        .text_color(dark().text.tertiary)
                        .max_h(px(THINKING_BOX_MAX_HEIGHT))
                        .overflow_y_scroll()
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
/// 加载 / 失败 / 空态）。面板内容为滚动文本，行级交互走键盘焦点链
///（chip / Refresh 均为 tab stop），不发布无真实布局的行级节点。
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
    if conversation.loading {
        value.push_str(" · ");
        value.push_str(t("subagents.loading"));
    }
    if let Some(reason) = &conversation.error {
        value.push_str(" · ");
        value.push_str(reason);
    }
    AxNode::new(
        "inspector-subagent",
        AxRole::Group,
        t("inspector.tab_subagent"),
        frame,
    )
    .value(value)
}
