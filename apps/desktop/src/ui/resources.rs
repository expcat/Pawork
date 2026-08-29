//! Resources 页（R8 波 D）：MCP server 只读列表（mcp_list）。
//! name / transport / state / tools 数 / last_error 全部来自 Host 响应；
//! 「已加载规则」分区无 Host 出口，本波不画（design/README.md §8.5）。

use gpui::{div, prelude::*, px, Context, ScrollHandle};

use crate::controller::McpServerEntry;
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::label::Label;
use crate::ui::theme::{dark, font, metrics};

use super::AppView;

/// MCP 清单拉取状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ResourcesFetch {
    Idle,
    Fetching,
    Ready,
    Failed(String),
}

pub(super) struct ResourcesPanelState {
    epoch: u64,
    pub fetch: ResourcesFetch,
    pub servers: Vec<McpServerEntry>,
    pub stale_reason: Option<String>,
    pub scroll: ScrollHandle,
}

impl Default for ResourcesPanelState {
    fn default() -> Self {
        Self {
            epoch: 0,
            fetch: ResourcesFetch::Idle,
            servers: Vec::new(),
            stale_reason: None,
            scroll: ScrollHandle::new(),
        }
    }
}

impl ResourcesPanelState {
    pub(super) fn begin_refresh(&mut self) -> u64 {
        self.epoch += 1;
        self.fetch = ResourcesFetch::Fetching;
        self.stale_reason = None;
        self.epoch
    }

    pub(super) fn mark_failed(&mut self, reason: &str) {
        self.fetch = ResourcesFetch::Failed(reason.into());
        self.stale_reason = None;
    }

    pub(super) fn mark_failed_for_epoch(&mut self, epoch: u64, reason: &str) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.mark_failed(reason);
        true
    }

    pub(super) fn mark_stale(&mut self, reason: &str) {
        self.epoch += 1;
        if self.fetch == ResourcesFetch::Fetching {
            self.fetch = if self.servers.is_empty() {
                ResourcesFetch::Idle
            } else {
                ResourcesFetch::Ready
            };
        }
        self.stale_reason = Some(reason.into());
    }

    pub(super) fn apply_servers(&mut self, epoch: u64, servers: Vec<McpServerEntry>) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.servers = servers;
        self.fetch = ResourcesFetch::Ready;
        self.stale_reason = None;
        true
    }
}

impl AppView {
    pub(super) fn resources_element(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(
                Label::new("MCP servers")
                    .size(font::XS)
                    .color(dark().text.tertiary),
            )
            .child(div().flex_1())
            .child(
                Button::new("resources-refresh")
                    .variant(ButtonVariant::Ghost)
                    .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                    .text_size(font::XS)
                    .text_color(dark().text.secondary)
                    .label("↻")
                    .tooltip("Refresh resources")
                    .track_focus(&self.resources_refresh_focus)
                    .on_click(cx.listener(|view, event, _window, cx| {
                        if view.consume_button_key_click("resources-refresh", event) {
                            return;
                        }
                        view.refresh_resources(cx);
                    })),
            );
        let body = match &self.resources.fetch {
            ResourcesFetch::Idle => {
                resources_placeholder("Resources unavailable.").into_any_element()
            }
            ResourcesFetch::Fetching if self.resources.servers.is_empty() => {
                resources_placeholder("Loading resources…").into_any_element()
            }
            ResourcesFetch::Failed(reason) => {
                resources_placeholder_colored(reason.clone(), dark().semantic.danger_text)
                    .into_any_element()
            }
            ResourcesFetch::Fetching | ResourcesFetch::Ready => {
                if self.resources.servers.is_empty() {
                    resources_placeholder("No MCP servers configured.").into_any_element()
                } else {
                    let mut list = div()
                        .id("mcp-server-list")
                        .flex()
                        .flex_col()
                        .track_scroll(&self.resources.scroll)
                        .overflow_y_scroll();
                    for server in &self.resources.servers {
                        let mut meta =
                            format!("{} · {} tools", server.transport, server.tool_count);
                        if let Some(error) = &server.last_error {
                            meta.push_str(&format!(" · {error}"));
                        }
                        list = list
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_baseline()
                                    .justify_between()
                                    .gap_2()
                                    .px_2()
                                    .pt_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(font::SM))
                                            .text_color(dark().text.primary)
                                            .child(server.name.clone()),
                                    )
                                    .child(Label::new(server.state.clone()).size(font::XS).color(
                                        if server.state == "failed" {
                                            dark().semantic.danger_text
                                        } else {
                                            dark().text.secondary
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .pb_2()
                                    .text_size(px(font::XS))
                                    .text_color(dark().text.tertiary)
                                    .child(meta),
                            );
                    }
                    list.into_any_element()
                }
            }
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .when_some(self.resources.stale_reason.clone(), |block, reason| {
                block.child(
                    div()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(dark().semantic.warning_text)
                        .text_size(px(font::XS))
                        .text_color(dark().semantic.warning_text)
                        .child(format!("Stale data · {reason}")),
                )
            })
            .child(body)
    }
}

fn resources_placeholder(text: impl Into<String>) -> gpui::Div {
    resources_placeholder_colored(text, dark().text.secondary)
}

fn resources_placeholder_colored(text: impl Into<String>, color: gpui::Rgba) -> gpui::Div {
    div()
        .flex()
        .flex_1()
        .items_start()
        .justify_start()
        .p_2()
        .text_size(px(font::SM))
        .text_color(color)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_state_defaults_to_idle_and_rejects_stale_epochs() {
        let mut state = ResourcesPanelState::default();
        assert_eq!(state.fetch, ResourcesFetch::Idle);
        let first = state.begin_refresh();
        let second = state.begin_refresh();
        assert!(!state.apply_servers(first, Vec::new()));
        assert!(state.apply_servers(
            second,
            vec![McpServerEntry {
                name: "fetch".into(),
                transport: "stdio".into(),
                state: "ready".into(),
                tool_count: 2,
                last_error: None,
            }]
        ));
        assert_eq!(state.fetch, ResourcesFetch::Ready);
        assert_eq!(state.servers.len(), 1);
    }

    #[test]
    fn disconnect_keeps_ready_resources_stale_and_invalidates_old_responses() {
        let mut state = ResourcesPanelState::default();
        let epoch = state.begin_refresh();
        assert!(state.apply_servers(
            epoch,
            vec![McpServerEntry {
                name: "fetch".into(),
                transport: "stdio".into(),
                state: "ready".into(),
                tool_count: 2,
                last_error: None,
            }]
        ));
        state.mark_stale("connection lost");
        assert_eq!(state.fetch, ResourcesFetch::Ready);
        assert_eq!(state.servers.len(), 1);
        assert_eq!(state.stale_reason.as_deref(), Some("connection lost"));
        assert!(!state.mark_failed_for_epoch(epoch, "late failure"));
    }
}
