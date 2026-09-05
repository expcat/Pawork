//! Resources 页（R8 波 D）：MCP server 只读列表（mcp_list）。
//! name / transport / state / tools 数 / last_error 全部来自 Host 响应；
//! 「已加载规则」分区无 Host 出口，本波不画（design/README.md §8.5）。

use gpui::{div, prelude::*, Context, ScrollHandle};

use crate::controller::McpServerEntry;
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::label::Label;
use crate::ui::i18n::t;
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
    /// 至少成功拉取过一次（SET-6c：Settings「工具与 MCP」页导航 gate，
    /// 语义与 settings_general / settings_permissions 的 available 一致）。
    pub available: bool,
    /// SET-6c：mcp_test / mcp_server_remove 失败文案（Settings 页可见；
    /// 工作台 Composer footer 仍走 status_hint）。成功回执才清除。
    pub action_error: Option<String>,
    pub scroll: ScrollHandle,
}

impl Default for ResourcesPanelState {
    fn default() -> Self {
        Self {
            epoch: 0,
            fetch: ResourcesFetch::Idle,
            servers: Vec::new(),
            stale_reason: None,
            available: false,
            action_error: None,
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
        self.available = true;
        true
    }

    /// mcp_test / mcp_server_remove 的 Data 回执（SET-6c / ADR-049）：回执
    /// 即 Host 权威写后状态。bump epoch 使在途 mcp_list 失效，避免 Refresh
    /// 迟到响应覆盖写后清单；成功回执清除 action_error。
    pub(super) fn apply_authoritative_servers(&mut self, servers: Vec<McpServerEntry>) {
        self.epoch += 1;
        self.servers = servers;
        self.fetch = ResourcesFetch::Ready;
        self.stale_reason = None;
        self.available = true;
        self.action_error = None;
    }
}

/// MCP server 清单行头（name + state；Inspector Resources 与 Settings
/// 「工具与 MCP」页共用同一渲染形状）。
pub(super) fn mcp_server_name_row(server: &McpServerEntry) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_baseline()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(font::SM)
                .text_color(dark().text.primary)
                .child(server.name.clone()),
        )
        .child(
            Label::new(server.state.clone())
                .size(font::XS)
                .color(if server.state == "failed" {
                    dark().semantic.danger_text
                } else {
                    dark().text.secondary
                }),
        )
}

/// MCP server 清单行 meta 文案（transport · tools 数 · last_error）。
pub(super) fn mcp_server_meta_text(server: &McpServerEntry) -> String {
    let mut meta = format!("{} · {} tools", server.transport, server.tool_count);
    if let Some(error) = &server.last_error {
        meta.push_str(&format!(" · {error}"));
    }
    meta
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
                Label::new(t("resources.mcp_title"))
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
                    .tooltip(t("resources.tooltip_refresh"))
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
                resources_placeholder("resources.unavailable").into_any_element()
            }
            ResourcesFetch::Fetching if self.resources.servers.is_empty() => {
                resources_placeholder("resources.loading").into_any_element()
            }
            ResourcesFetch::Failed(reason) => {
                resources_placeholder_colored(reason.clone(), dark().semantic.danger_text)
                    .into_any_element()
            }
            ResourcesFetch::Fetching | ResourcesFetch::Ready => {
                if self.resources.servers.is_empty() {
                    resources_placeholder("resources.empty").into_any_element()
                } else {
                    let mut list = div()
                        .id("mcp-server-list")
                        .flex()
                        .flex_col()
                        .track_scroll(&self.resources.scroll)
                        .overflow_y_scroll();
                    for server in &self.resources.servers {
                        list = list.child(mcp_server_name_row(server).px_2().pt_2()).child(
                            div()
                                .px_2()
                                .pb_2()
                                .text_size(font::XS)
                                .text_color(dark().text.tertiary)
                                .child(mcp_server_meta_text(server)),
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
                        .text_size(font::XS)
                        .text_color(dark().semantic.warning_text)
                        .child(format!("Stale data · {reason}")),
                )
            })
            .child(body)
    }
}

/// 占位文案按 i18n key 取主文案与说明（同源；不能按翻译后的串匹配）。
fn resources_placeholder(key: &'static str) -> gpui::Div {
    let description = match key {
        "resources.unavailable" => t("resources.unavailable_desc"),
        "resources.loading" => t("resources.loading_desc"),
        "resources.empty" => t("resources.empty_desc"),
        _ => t("common.placeholder_no_details"),
    };
    resources_placeholder_content(
        t(key).to_string(),
        description.to_string(),
        dark().text.primary,
    )
}

/// 失败占位：标题本地化；reason 为 wire 数据，不翻译。
fn resources_placeholder_colored(text: impl Into<String>, color: gpui::Rgba) -> gpui::Div {
    resources_placeholder_content(t("resources.error_title").into(), text.into(), color)
}

fn resources_placeholder_content(
    title: String,
    description: String,
    color: gpui::Rgba,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .child(Label::new(title).size(font::BASE).color(color))
        .child(
            Label::new(description)
                .size(font::SM)
                .color(dark().text.secondary),
        )
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

    #[test]
    fn authoritative_receipt_lands_and_marks_page_available() {
        let mut state = ResourcesPanelState::default();
        assert!(!state.available);
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
        assert!(state.available);
        let inflight = state.begin_refresh();
        state.stale_reason = Some("connection lost".into());
        state.action_error = Some("Could not test MCP server · boom".into());
        // 回执落地、解除 stale、清除 action_error，并 bump epoch 使在途
        // mcp_list 失效。
        state.apply_authoritative_servers(Vec::new());
        assert_eq!(state.fetch, ResourcesFetch::Ready);
        assert!(state.stale_reason.is_none());
        assert!(state.servers.is_empty());
        assert!(state.available);
        assert!(state.action_error.is_none());
        assert!(
            !state.apply_servers(
                inflight,
                vec![McpServerEntry {
                    name: "stale".into(),
                    transport: "stdio".into(),
                    state: "ready".into(),
                    tool_count: 1,
                    last_error: None,
                }]
            ),
            "in-flight mcp_list must not overwrite a later receipt"
        );
        assert!(state.servers.is_empty());
    }
}
