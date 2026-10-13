//! AppView → AxTree 投影与 AX action 白名单。

use gpui::{App, Context, Focusable, Window};

use crate::projection::{
    run_footer_label, run_summary_texts, ActiveRun, ApprovalModeWire, ConnectionState, DateBucket,
    ForkBoundary, ModelEntry, SessionLiveStatus, TaskRailGrouping, TaskRailProjectGroup,
    TimelineEntry, TimelineEntryKind, TimelineRow, UNASSIGNED_PROJECT,
};

use super::{AxAction, AxBridge, AxNode, AxRect, AxRequest, AxRole, AxTree};
use crate::ui::approval_card::{
    approval_card_height, APPROVAL_BUTTON_HEIGHT, APPROVAL_BUTTON_ROW_GAP_REMS,
    APPROVAL_BUTTON_SLOT_WIDTHS, APPROVAL_CARD_PAD_REMS,
};
use crate::ui::changes::{ChangesFetch, ChangesTab};
use crate::ui::components::dropdown::ANCHOR_GAP_Y;
#[cfg(test)]
use crate::ui::components::dropdown::MENU_MAX_HEIGHT;
use crate::ui::i18n::t;
use crate::ui::input_area::ModelPurposeFilter;
use crate::ui::inspector::InspectorTab;
use crate::ui::resources::ResourcesFetch;
use crate::ui::settings::{
    parse_settings_control, parse_settings_mcp_control, parse_settings_models_control,
    parse_settings_role_control, settings_text_scale_from_identifier, SettingsControl,
    SettingsModelsControl, SettingsRoleControl, SETTINGS_APPEARANCE_CONTROL_GAP,
    SETTINGS_APPEARANCE_CONTROL_HEIGHT, SETTINGS_APPEARANCE_CONTROL_WIDTH, SETTINGS_CONTROL_PREFIX,
    SETTINGS_MCP_CONTROL_PREFIX, SETTINGS_MODELS_CONTROL_PREFIX, SETTINGS_ROLE_CONTROL_PREFIX,
};
use crate::ui::shell_layout::{self, InspectorPlacement};
use crate::ui::theme::{font, metrics};
use crate::ui::timeline_entry::{
    assistant_is_streaming, display_time, failure_next_step, run_open_providers_identifier,
    tool_group_summary, tool_result_expand_key, FailureNextStep, ToolRowView,
    SUMMARY_NEXT_STEP_BUTTON_HEIGHT,
};
use crate::ui::{
    activity_header_visibility, rail_project_occurrence_key, rail_session_archive_focus_key,
    rail_session_focus_key, rail_session_rename_focus_key, terminal_can_operate, timeline,
    workspace_empty_title, AppRoute, AppView, MenuKind, SettingsPage,
};

pub(crate) const PAD: f32 = 8.0;
pub(crate) const CONTROL_HEIGHT: f32 = 28.0;
pub(crate) const ROW_HEIGHT: f32 = 32.0;

mod composer;
mod inspector;
mod press;
mod rail;
mod timeline_ax;

/// 按钮行 y 自卡底内缩（p_2 + 32px 按钮槽），随卡位置整体移动。
fn approval_button_row_y(card: AxRect, rem_px: f32) -> f32 {
    card.y + card.height - APPROVAL_CARD_PAD_REMS * rem_px - APPROVAL_BUTTON_HEIGHT
}

impl AppView {
    pub(crate) fn install_accessibility(
        &mut self,
        window: &Window,
        handler: impl Fn(AxRequest) + 'static,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ax_bridge = Some(AxBridge::install(window, handler)?);
        self.ax_error_reported = false;
        cx.notify();
        Ok(())
    }

    pub(in crate::ui) fn sync_accessibility(&mut self, window: &Window, cx: &App) {
        let tree = self.accessibility_tree(window, cx);
        let Some(bridge) = self.ax_bridge.as_mut() else {
            return;
        };
        match bridge.update(tree) {
            Ok(_) => self.ax_error_reported = false,
            Err(reason) if !self.ax_error_reported => {
                eprintln!("pawork-desktop accessibility update failed: {reason}");
                self.ax_error_reported = true;
            }
            Err(_) => {}
        }
    }

    pub(crate) fn handle_accessibility_request(
        &mut self,
        request: AxRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // AX clients may retain an element from an earlier tree. Revalidate against the
        // current canonical UI state so stale enabled/action snapshots cannot bypass a gate.
        if !self.accessibility_tree(window, cx).permits(&request) {
            return;
        }
        if request.identifier.starts_with("files-") {
            if matches!(request.identifier.as_str(), "files-editor" | "files-filter") {
                self.files_ax_edit(
                    &request.identifier,
                    request.action,
                    request.value,
                    window,
                    cx,
                );
            } else if request.action == AxAction::Press {
                self.files_action(&request.identifier, window, cx);
            }
            return;
        }
        if request.identifier.starts_with("browser-") {
            match request.action {
                AxAction::Focus if request.identifier == "browser-address" => {
                    self.focus_browser_address(window, cx)
                }
                AxAction::SetValue if request.identifier == "browser-address" => {
                    self.browser.input.update(cx, |input, cx| {
                        input.set_text(request.value.unwrap_or_default(), cx)
                    })
                }
                AxAction::Press => self.browser_action(&request.identifier, window, cx),
                _ => {}
            }
            return;
        }
        if self.quick_search.open {
            match request.action {
                AxAction::Focus => window.focus(&self.quick_search.focus),
                AxAction::SetValue => self.quick_search.input.update(cx, |input, cx| {
                    input.set_text(request.value.unwrap_or_default(), cx)
                }),
                AxAction::Press => self.quick_search_press(&request.identifier, window, cx),
            }
            cx.notify();
            return;
        }
        if request.identifier == "timeline-find-input" {
            match request.action {
                AxAction::Focus => window.focus(&self.timeline_navigation.focus),
                AxAction::SetValue => self.timeline_navigation.input.update(cx, |input, cx| {
                    input.set_text(request.value.unwrap_or_default(), cx)
                }),
                AxAction::Press => {}
            }
            cx.notify();
            return;
        }
        if request.action == AxAction::Press
            && (request.identifier.starts_with("timeline-nav-")
                || matches!(
                    request.identifier.as_str(),
                    "timeline-find" | "timeline-turns"
                ))
        {
            self.navigation_press(&request.identifier, window, cx);
            cx.notify();
            return;
        }
        // SET-4：settings secure 输入（Focus / SetValue 合法输入路径；发布
        // 方向只给掩码，见 settings_page_ax）。
        let settings_api_key_input =
            parse_settings_control(&request.identifier).and_then(|control| match control {
                SettingsControl::ApiKeyInput(escaped) => Some(escaped),
                _ => None,
            });
        if let Some(rename) = self.settings_account_rename.as_ref() {
            let input_id = crate::ui::settings::settings_account_rename_input_identifier(
                &rename.provider_id,
                &rename.credential_id,
            );
            if input_id == request.identifier {
                match request.action {
                    AxAction::Focus => window.focus(&rename.input.read(cx).focus_handle(cx)),
                    AxAction::SetValue => rename.input.update(cx, |input, cx| {
                        input.set_text(request.value.unwrap_or_default(), cx)
                    }),
                    AxAction::Press => return,
                }
                cx.notify();
                return;
            }
        }
        match request.action {
            AxAction::Focus => match request.identifier.as_str() {
                "model-search-input" => window.focus(&self.model_search_focus),
                _ if ModelPurposeFilter::ALL
                    .iter()
                    .any(|purpose| purpose.identifier() == request.identifier) =>
                {
                    if let Some(focus) = self.settings_action_focus.get(&request.identifier) {
                        window.focus(focus);
                    }
                }
                "workspace-bind-project" => window.focus(&self.welcome_bind_project_focus),
                "composer-input" => self.focus_composer(window, cx),
                // ADR-054 D2：行内改名编辑器聚焦（与点击行内输入框同路径）。
                "session-rename-input" => {
                    if let Some(rename) = self.session_rename.as_ref() {
                        let focus = rename.input.read(cx).focus_handle(cx);
                        window.focus(&focus);
                    }
                }
                "terminal-output" => {
                    let focus = self.terminal_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-proxy-input" => {
                    let focus = self.settings_proxy_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-search-input" => window.focus(&self.settings_search_focus),
                "settings-terminal-shell-input" => {
                    let focus = self.settings_terminal_shell_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-terminal-columns-input" => {
                    let focus = self
                        .settings_terminal_columns_input
                        .read(cx)
                        .focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-terminal-rows-input" => {
                    let focus = self.settings_terminal_rows_input.read(cx).focus_handle(cx);
                    window.focus(&focus);
                }
                "settings-subagents-concurrency-input" => {
                    let focus = self
                        .settings_subagents_concurrency_input
                        .read(cx)
                        .focus_handle(cx);
                    window.focus(&focus);
                }
                // SET-4：settings secure 输入聚焦（与点击输入框同一路径；
                // permits 已按当前树核对 enabled）。
                _ if settings_api_key_input.is_some() => {
                    let escaped = settings_api_key_input.clone().unwrap_or_default();
                    if let Some(provider_id) = self.settings_provider_id_for_escaped(&escaped) {
                        if let Some(input) = self.settings_api_key_inputs.get(&provider_id) {
                            let focus = input.read(cx).focus_handle(cx);
                            window.focus(&focus);
                        }
                    }
                }
                _ if request.identifier.starts_with("settings-provider-details-") => {
                    if let Some(input) =
                        self.settings_auth_details.iter().find_map(|(id, input)| {
                            (dynamic_identifier("settings-provider-details", id)
                                == request.identifier)
                                .then_some(input)
                        })
                    {
                        window.focus(&input.read(cx).focus_handle(cx));
                    }
                }
                _ => {
                    let Some(focus) = self
                        .subagent_focus_for_identifier(&request.identifier)
                        .cloned()
                    else {
                        return;
                    };
                    window.focus(&focus);
                }
            },
            AxAction::SetValue => {
                let value = request.value.unwrap_or_default();
                match request.identifier.as_str() {
                    "model-search-input" => self
                        .model_search_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "composer-input" => self
                        .text_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "session-rename-input" => {
                        if let Some(rename) = self.session_rename.as_ref() {
                            rename
                                .input
                                .update(cx, |input, cx| input.set_text(value, cx));
                        }
                    }
                    "settings-proxy-input" => self
                        .settings_proxy_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "settings-search-input" => self
                        .settings_search_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "settings-terminal-shell-input" => self
                        .settings_terminal_shell_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "settings-terminal-columns-input" => self
                        .settings_terminal_columns_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "settings-terminal-rows-input" => self
                        .settings_terminal_rows_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    "settings-subagents-concurrency-input" => self
                        .settings_subagents_concurrency_input
                        .update(cx, |input, cx| input.set_text(value, cx)),
                    // SET-4：AX set-value 是合法输入路径（等同键入）；
                    // 发布方向永远只给掩码（settings_page_ax）。
                    _ if settings_api_key_input.is_some() => {
                        let escaped = settings_api_key_input.clone().unwrap_or_default();
                        if let Some(provider_id) = self.settings_provider_id_for_escaped(&escaped) {
                            if let Some(input) = self.settings_api_key_inputs.get(&provider_id) {
                                input.update(cx, |input, cx| input.set_text(value, cx));
                            }
                        }
                    }
                    _ => return,
                }
            }
            AxAction::Press => {
                if !self.handle_accessibility_press(&request.identifier, window, cx) {
                    return;
                }
            }
        }
        cx.notify();
    }

    pub(in crate::ui) fn accessibility_tree(&self, window: &Window, cx: &App) -> AxTree {
        if self.quick_search.open {
            return self.quick_search_ax(window, cx);
        }
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width).max(1.0);
        let height = f32::from(viewport.height).max(1.0);
        // P2-1：Settings 壳不渲染 RunStatusBar（与 render 同源），内容用全高；
        // 工作台常驻 StatusBar。
        let settings_route = self.route == AppRoute::Settings;
        let content_height = if !self.run_status_visible() {
            height
        } else {
            (height - metrics::STATUS_BAR_HEIGHT).max(1.0)
        };
        // 与 AppView::render 共享同一壳层几何决定（GUI2-05：宽窗 Side
        // 并排，窄窗 / 150% 不够并排时显式打开走中央，未打开 Hidden）。
        let shell = shell_layout::resolve(
            viewport.width,
            self.inspector_open,
            self.text_scale == font::TextScale::Percent150,
        );
        let placement = if settings_route {
            InspectorPlacement::Hidden
        } else {
            shell.placement
        };
        let sidebar_width = shell.rail_width.min(width);
        // UI-1：render 先更新本帧过渡宽度；开合期间也按可见宽度布局，
        // Header 动作使用逻辑终态 placement.is_visible()。
        let inspector_width = self
            .inspector_render_width
            .min((width - sidebar_width).max(0.0));
        let workspace_width = (width - sidebar_width - inspector_width).max(0.0);
        let workspace_x = sidebar_width;

        // SET-3 顶层路由：Settings 壳与工作台互斥（与 render 同源）。
        let mut tree = if settings_route {
            AxTree::new(width, height)
                .child(self.settings_rail_ax(
                    window,
                    cx,
                    AxRect::new(0.0, 0.0, sidebar_width, content_height),
                ))
                .child(self.settings_page_ax(
                    window,
                    cx,
                    AxRect::new(
                        workspace_x,
                        0.0,
                        (width - sidebar_width).max(0.0),
                        content_height,
                    ),
                ))
        } else {
            let mut tree = AxTree::new(width, height)
                .child(self.sidebar_ax(
                    window,
                    cx,
                    AxRect::new(0.0, 0.0, sidebar_width, content_height),
                ))
                .child(self.workspace_ax(
                    window,
                    cx,
                    AxRect::new(workspace_x, 0.0, workspace_width, content_height),
                    placement,
                ));
            if inspector_width > 0.0 && !placement.is_center() {
                let mut inspector = self.inspector_ax(
                    window,
                    cx,
                    AxRect::new(
                        width - inspector_width,
                        0.0,
                        metrics::INSPECTOR_WIDTH,
                        content_height,
                    ),
                    placement,
                );
                // 与 shell-inspector overflow_hidden 同源：内部保持
                // 440px 排版，只裁剪右缘；完全不可见的节点不发布动作。
                fn clip_right(node: &mut AxNode, right: f32) -> bool {
                    node.bounds.width = node.bounds.width.min((right - node.bounds.x).max(0.0));
                    node.children.retain_mut(|child| clip_right(child, right));
                    node.bounds.width > 0.0 && node.bounds.height > 0.0
                }
                clip_right(&mut inspector, width);
                tree = tree.child(inspector);
            }
            tree
        };
        if self.run_status_visible() {
            // StatusBar 视觉上不覆盖左栏账户区；AX frame 与 render 同源。
            tree = tree.child(self.status_ax(AxRect::new(
                sidebar_width,
                content_height,
                (width - sidebar_width).max(0.0),
                metrics::STATUS_BAR_HEIGHT,
            )));
        }
        tree
    }

    fn shell_ax_rect(&self, id: &'static str) -> AxRect {
        let b = self.shell_layouts[id].bounds();
        AxRect::new(
            f32::from(b.origin.x),
            f32::from(b.origin.y),
            f32::from(b.size.width),
            f32::from(b.size.height),
        )
    }

    /// R6 Wave A：Activity 触发器迁至 Workspace Header（header_ax），
    /// StatusBar 只发布居中的 run-status；左右栏视觉不进 AX。
    fn status_ax(&self, frame: AxRect) -> AxNode {
        let now = crate::ui::now_unix_ms();
        // 用量串仍占整行框，避免与左右栏重复发布同源文案。
        let run_status_width = frame.width;
        let run_status_x = frame.x;
        let status = AxNode::new("status-bar", AxRole::Group, "Status", frame).child(
            AxNode::new(
                "run-status",
                AxRole::StaticText,
                "Run status",
                AxRect::new(run_status_x, frame.y, run_status_width, frame.height),
            )
            .value(self.projection.run_status_label(now)),
        );
        status
    }
}

fn timeline_entry_markdown(entry: &TimelineEntry) -> Option<&str> {
    match &entry.kind {
        TimelineEntryKind::UserMessage { text }
        | TimelineEntryKind::AssistantMessage { text }
        | TimelineEntryKind::Thinking { text } => Some(text),
        TimelineEntryKind::Error(message) => Some(message),
        _ => None,
    }
}

fn timeline_accessible_text(entry: &TimelineEntry, generating: bool) -> (String, String) {
    match &entry.kind {
        // RV-02：附件消息朗读清洗后的文本（附件头本地化、控制说明不朗读，
        // 内容保留）；无附件消息原样。
        TimelineEntryKind::UserMessage { text } => (
            t("timeline.you").into(),
            crate::ui::attachment_blocks::accessible_text(text),
        ),
        TimelineEntryKind::AssistantMessage { text } => (
            if generating {
                format!("Pawork · {}", t("tool.generating"))
            } else {
                "Pawork".into()
            },
            text.clone(),
        ),
        TimelineEntryKind::Thinking { text } => (t("timeline.thinking").into(), text.clone()),
        TimelineEntryKind::ToolCall {
            name,
            status,
            detail,
            arguments,
        } => {
            let row =
                ToolRowView::from_facts(name, status, arguments.as_deref(), detail.as_deref());
            (
                format!("Tool · {}", row.headline),
                format!(
                    "{} · {}",
                    timeline::tool_status_label(status),
                    row.detail.unwrap_or_default()
                ),
            )
        }
        TimelineEntryKind::RunState(state) => ("Run".into(), state.clone()),
        TimelineEntryKind::Error(message) => ("Error".into(), message.clone()),
    }
}

/// 会话行 AX description：可见状态点的状态词 + unread 语义同源映射
/// （ADR-042——新增可见状态须同批补 AX）；无 live 状态不声明语义（不伪造
/// 终态），unread 与标题 semibold 视觉同源。
fn session_status_description(status: Option<SessionLiveStatus>, unread: bool) -> String {
    let word = match status {
        Some(SessionLiveStatus::NeedsInput) => SessionLiveStatus::NeedsInput.label(),
        Some(SessionLiveStatus::Running) => SessionLiveStatus::Running.label(),
        Some(SessionLiveStatus::Blocked) => SessionLiveStatus::Blocked.label(),
        None => "Session",
    };
    if unread {
        format!("{word} · Unread")
    } else {
        word.to_string()
    }
}

fn project_key(workspace_id: Option<&str>) -> String {
    workspace_id.unwrap_or(UNASSIGNED_PROJECT).to_string()
}

pub(crate) fn dynamic_identifier(prefix: &str, raw: &str) -> String {
    let mut identifier = String::with_capacity(prefix.len() + raw.len() + 1);
    identifier.push_str(prefix);
    identifier.push('-');
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.') {
            identifier.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(identifier, "_{byte:02x}");
        }
    }
    identifier
}

fn session_identifier(session_id: &str) -> String {
    dynamic_identifier("session", session_id)
}

fn session_title_identifier(session_id: &str) -> String {
    dynamic_identifier("session-title", session_id)
}

fn session_status_dot_identifier(session_id: &str) -> String {
    dynamic_identifier("status-dot", session_id)
}

fn session_rename_identifier(session_id: &str) -> String {
    dynamic_identifier("session-rename", session_id)
}

fn session_archive_identifier(session_id: &str) -> String {
    dynamic_identifier("session-archive", session_id)
}

/// 项目头 identifier。Timeline 模式下同一项目可出现于多个日期组，以日期桶
/// 限定避免重复 id（AxTree::validate 拒绝重复）；Projects 模式保持 project-{key}。
fn rail_project_identifier(bucket: Option<DateBucket>, key: &str) -> String {
    match bucket {
        Some(bucket) => dynamic_identifier("project", &format!("{}:{}", bucket.label(), key)),
        None => dynamic_identifier("project", key),
    }
}

fn rail_project_add_identifier(bucket: Option<DateBucket>, key: &str) -> String {
    match bucket {
        Some(bucket) => dynamic_identifier("project-add", &format!("{}:{}", bucket.label(), key)),
        None => dynamic_identifier("project-add", key),
    }
}

fn rail_project_count_identifier(bucket: Option<DateBucket>, key: &str) -> String {
    match bucket {
        Some(bucket) => dynamic_identifier("project-count", &format!("{}:{}", bucket.label(), key)),
        None => dynamic_identifier("project-count", key),
    }
}

fn scope_identifier(workspace_id: Option<&str>) -> String {
    dynamic_identifier("scope", workspace_id.unwrap_or("all"))
}

fn model_identifier(model: &ModelEntry) -> String {
    dynamic_identifier("model", &format!("{}:{}", model.provider_id, model.id))
}

fn entry_menu_identifier(event_id: &str) -> String {
    dynamic_identifier("entry-menu", event_id)
}

fn run_review_identifier(event_id: &str) -> String {
    dynamic_identifier("run-review-changes", event_id)
}

fn tool_group_toggle_identifier(event_id: &str) -> String {
    dynamic_identifier("tool-group-toggle", event_id)
}

fn tool_call_row_view(
    entry: &TimelineEntry,
    expanded_timeline_details: &std::collections::HashSet<String>,
    timeline: &[TimelineEntry],
) -> Option<ToolRowView> {
    let TimelineEntryKind::ToolCall {
        name,
        detail,
        arguments,
        ..
    } = &entry.kind
    else {
        return None;
    };
    Some(ToolRowView::present(
        name,
        timeline::tool_display_status(entry, timeline),
        arguments.as_deref(),
        detail.as_deref(),
        &entry.event_id,
        expanded_timeline_details.contains(&tool_result_expand_key(&entry.event_id)),
    ))
}

fn diff_file_identifier(path: &str) -> String {
    dynamic_identifier("changes-file", path)
}

fn tail_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    if count <= limit {
        return value.to_string();
    }
    format!("…{}", value.chars().skip(count - limit).collect::<String>())
}

#[cfg(test)]
mod tests;
