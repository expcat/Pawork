//! Resources 页（R8 波 D）：MCP server 只读列表（mcp_list）。
//! name / transport / state / tools 数 / last_error 全部来自 Host 响应；
//! 「已加载规则」分区无 Host 出口，本波不画（docs/gui-design.md §8）。

use std::collections::{HashMap, HashSet};

use gpui::{div, prelude::*, px, Context, FocusHandle, ScrollHandle, SharedString};

use crate::controller::McpServerEntry;
use crate::ui::components::button::{Button, ButtonPadding, ButtonVariant};
use crate::ui::components::empty_state::{stale_notice, EmptyState};
use crate::ui::components::icon::{icon_sized, Icon};
use crate::ui::components::label::Label;
use crate::ui::components::skeleton::loading_skeleton;
use crate::ui::i18n::t;
use crate::ui::settings::settings_mcp_state_label;
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
    /// 工作台 StatusBar 右栏仍走 status_hint）。成功回执才清除。
    pub action_error: Option<String>,
    pub scroll: ScrollHandle,
    pub layouts: HashMap<String, ScrollHandle>,
    pub details_open: HashSet<String>,
    pub details_focus: HashMap<String, FocusHandle>,
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
            layouts: HashMap::new(),
            details_open: HashSet::new(),
            details_focus: HashMap::new(),
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

/// MCP server 清单行头（name + 本地化状态）。
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
                .text_size(font::BASE)
                .line_height(font::CONTROL_LINE_HEIGHT)
                .text_color(dark().text.primary)
                .child(server.name.clone()),
        )
        .child(
            Label::new(settings_mcp_state_label(&server.state))
                .size(font::XS)
                .color(if server.state == "failed" {
                    dark().semantic.danger_text
                } else {
                    dark().text.secondary
                }),
        )
}

/// MCP server 清单行 meta 文案（transport · tools 数）；错误独立换行显示。
pub(super) fn mcp_server_meta_text(server: &McpServerEntry) -> String {
    format!(
        "{} · {}",
        server.transport,
        t("resources.tool_count").replace("{}", &server.tool_count.to_string())
    )
}

pub(super) fn resources_server_id(name: &str) -> String {
    super::accessibility::dynamic_identifier("mcp-server", name)
}

impl AppView {
    pub(super) fn resources_can_refresh(&self) -> bool {
        matches!(
            self.projection.connection,
            crate::projection::ConnectionState::Connected { .. }
        ) && self.resources.fetch != ResourcesFetch::Fetching
    }

    pub(super) fn activate_resources_refresh(&mut self, cx: &mut Context<Self>) {
        if self.resources_can_refresh() {
            self.refresh_resources(cx);
        }
    }

    pub(super) fn toggle_resources_details(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.resources.details_open.remove(id) {
            self.resources.details_open.insert(id.into());
        }
        cx.notify();
    }

    fn resources_error_element(
        &mut self,
        id: String,
        summary: &'static str,
        reason: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let expanded = self.resources.details_open.contains(&id);
        let focus = self
            .resources
            .details_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let layout = self
            .resources
            .layouts
            .entry(id.clone())
            .or_default()
            .clone();
        let click = id.clone();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .text_size(font::DESCRIPTION)
            .line_height(font::DESCRIPTION_LINE_HEIGHT)
            .child(
                div()
                    .text_color(dark().semantic.danger_text)
                    .child(t(summary)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{id}-layout")))
                    .flex_none()
                    .track_scroll(&layout)
                    .child(
                        Button::new(SharedString::from(id.clone()))
                            .variant(ButtonVariant::Ghost)
                            .label(t(if expanded {
                                "resources.hide_details"
                            } else {
                                "resources.details"
                            }))
                            .track_focus(&focus)
                            .on_click(cx.listener(move |view, event, _, cx| {
                                if !view.consume_button_key_click(&click, event) {
                                    view.toggle_resources_details(&click, cx);
                                }
                            }))
                            .on_activate(cx.listener(move |view, _, _, cx| {
                                if view.open_menu.is_none() {
                                    view.note_button_key_activate(&id);
                                    view.toggle_resources_details(&id, cx);
                                    cx.stop_propagation();
                                }
                            })),
                    ),
            )
            .when(expanded, |body| {
                body.child(
                    div()
                        .whitespace_normal()
                        .text_color(dark().text.secondary)
                        .child(reason),
                )
            })
    }

    pub(super) fn resources_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let servers = self.resources.servers.clone();
        let failure = match &self.resources.fetch {
            ResourcesFetch::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let mut ids = vec!["resources-refresh".to_string()];
        if failure.is_some() {
            ids.push("resources-query-error-details".into());
        }
        for server in &servers {
            let id = resources_server_id(&server.name);
            ids.push(id.clone());
            if server.last_error.is_some() {
                ids.push(format!("resources-error-details-{id}"));
            }
        }
        self.resources.layouts.retain(|id, _| ids.contains(id));
        self.resources
            .details_focus
            .retain(|id, _| ids.contains(id));
        self.resources.details_open.retain(|id| ids.contains(id));
        for id in ids {
            self.resources.layouts.entry(id).or_default();
        }
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .flex_none()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(
                Label::new(t("resources.mcp_title"))
                    .size(font::BODY)
                    .line_height(font::SECTION_LINE_HEIGHT)
                    .color(dark().text.primary),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("resources-refresh-layout")
                    .flex_none()
                    .track_scroll(&self.resources.layouts["resources-refresh"])
                    .child(
                        Button::new("resources-refresh")
                            .variant(ButtonVariant::Ghost)
                            .padding(ButtonPadding::Horizontal(metrics::PADDING_SM))
                            .text_color(dark().text.secondary)
                            .child(icon_sized(Icon::Refresh, px(metrics::ICON_SM)))
                            .tooltip(t("resources.tooltip_refresh"))
                            .track_focus(&self.resources_refresh_focus)
                            .disabled(!self.resources_can_refresh())
                            .on_click(cx.listener(|view, event, _, cx| {
                                if !view.consume_button_key_click("resources-refresh", event) {
                                    view.activate_resources_refresh(cx);
                                }
                            }))
                            .on_activate(cx.listener(|view, _, _, cx| {
                                if view.open_menu.is_none() {
                                    view.note_button_key_activate("resources-refresh");
                                    view.activate_resources_refresh(cx);
                                    cx.stop_propagation();
                                }
                            })),
                    ),
            );
        let mut list = div()
            .id("mcp-server-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .track_scroll(&self.resources.scroll)
            .overflow_y_scroll();
        if let Some(reason) = failure {
            list = list.child(div().p_3().flex_none().child(self.resources_error_element(
                "resources-query-error-details".into(),
                "resources.query_error",
                reason,
                cx,
            )));
            if !servers.is_empty() {
                list = list.child(stale_notice(t("resources.retained")));
            }
        } else if servers.is_empty() {
            list = list.child(resources_placeholder(match self.resources.fetch {
                ResourcesFetch::Idle => "resources.unavailable",
                ResourcesFetch::Fetching => "resources.loading",
                _ => "resources.empty",
            }));
        }
        for server in &servers {
            let id = resources_server_id(&server.name);
            let mut row = div()
                .id(SharedString::from(format!("{id}-layout")))
                .track_scroll(&self.resources.layouts[&id])
                .flex_none()
                .px_3()
                .py_2()
                .child(mcp_server_name_row(server))
                .child(
                    div()
                        .whitespace_normal()
                        .text_size(font::META)
                        .line_height(font::META_LINE_HEIGHT)
                        .text_color(dark().text.tertiary)
                        .child(mcp_server_meta_text(server)),
                );
            if let Some(error) = &server.last_error {
                row = row.child(self.resources_error_element(
                    format!("resources-error-details-{id}"),
                    "resources.server_error",
                    error.clone(),
                    cx,
                ));
            }
            list = list.child(row);
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(header)
            .when_some(self.resources.stale_reason.clone(), |body, reason| {
                body.child(stale_notice(reason))
            })
            .child(list)
    }
}

/// 占位文案按 i18n key 取主文案与说明（同源；不能按翻译后的串匹配）。
fn resources_placeholder(key: &'static str) -> EmptyState {
    let description = match key {
        "resources.unavailable" => t("resources.unavailable_desc"),
        "resources.loading" => t("resources.loading_desc"),
        "resources.empty" => t("resources.empty_desc"),
        _ => t("common.placeholder_no_details"),
    };
    let mut placeholder = EmptyState::labeled(
        Icon::Resources,
        t(key).to_string(),
        description.to_string(),
        dark().text.primary,
    )
    .top_aligned();
    if key == "resources.loading" {
        placeholder = placeholder.child(loading_skeleton("resources.loading-skeleton"));
    }
    placeholder
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
