//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Window};

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::projection::ConnectionState;
use crate::ui::settings::{
    approval_mode_description, approval_mode_label, permissions_status_lines, APPROVAL_MODE_ALL,
    SETTINGS_PERMISSIONS_EFFECT_NOTE, SETTINGS_TRUST_UNSET,
};
use crate::ui::AppView;

impl AppView {
    /// 「权限与审批」页 AX（SET-6b）：五档审批模式（当前档只读、其余
    /// Press）、会话信任开关、Global 默认只读行、生效边界；stale 时
    /// enabled=false 且 permits 拒绝写动作，与 render 同 gate。
    pub(crate) fn settings_permissions_page_ax(&self, window: &Window, _cx: &App, frame: AxRect) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const STATUS_HEIGHT: f32 = 20.0;
        const CONTROL_ROW: f32 = 28.0;
        const MODE_ROW_HEIGHT: f32 = 44.0;
        const ROW_GAP: f32 = 4.0;
        let state = &self.projection.settings_permissions;
        let writes = self.settings_permissions_writes_enabled();
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let refresh_focused =
            self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window);
        let mut page = AxNode::new("settings-page", AxRole::Group, "权限与审批", frame)
            .child(
                AxNode::new(
                    "settings-page-title",
                    AxRole::StaticText,
                    "权限与审批",
                    AxRect::new(
                        frame.x + 16.0,
                        frame.y + 16.0,
                        (frame.width - 136.0).max(0.0),
                        HEADING_HEIGHT + SUBTITLE_HEIGHT,
                    ),
                )
                .value("当前会话的审批模式与 workspace 信任"),
            )
            .child(
                AxNode::new(
                    "settings-refresh",
                    AxRole::Button,
                    "Refresh",
                    AxRect::new(
                        frame.x + frame.width - 16.0 - 96.0,
                        frame.y + 16.0,
                        96.0,
                        CONTROL_ROW,
                    ),
                )
                .enabled(connected)
                .focused(refresh_focused)
                .action(AxAction::Press),
            );
        let mut y = frame.y + 16.0 + HEADING_HEIGHT + SUBTITLE_HEIGHT + 8.0;
        let width = (frame.width - 32.0).max(0.0);
        for (kind, label) in permissions_status_lines(state) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    "Permissions status",
                    AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
                )
                .value(label),
            );
            y += STATUS_HEIGHT + 8.0;
        }

        // ① 五档审批模式：每档一行（label · description，当前档标「当前」），
        // 非 current 档发布 Press 按钮（enabled 与 render 同源）。
        let current_mode_label = state
            .approval_mode
            .map(approval_mode_label)
            .unwrap_or("未知");
        page = page.child(
            AxNode::new(
                "settings-approval-mode-header",
                AxRole::StaticText,
                "审批模式",
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
            )
            .value(format!("当前 · {current_mode_label}")),
        );
        y += STATUS_HEIGHT + 8.0;
        for mode in APPROVAL_MODE_ALL {
            let current = state.approval_mode == Some(mode);
            let mut value = format!(
                "{} · {}",
                approval_mode_label(mode),
                approval_mode_description(mode)
            );
            if current {
                value.push_str(" · 当前");
            }
            let button_id = format!("settings-approval-mode-{}", mode.as_str());
            let select_enabled = writes && !current;
            let select_focused = self
                .settings_permissions_focus
                .get(&button_id)
                .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
            let mut row = AxNode::new(
                format!("settings-approval-mode-row-{}", mode.as_str()),
                AxRole::StaticText,
                approval_mode_label(mode),
                AxRect::new(frame.x + 16.0, y, (width - 96.0).max(60.0), MODE_ROW_HEIGHT),
            )
            .value(value);
            if current {
                row = row.description("当前档位");
            }
            page = page.child(row);
            if !current {
                page = page.child(
                    AxNode::new(
                        button_id,
                        AxRole::Button,
                        format!("选择 {}", approval_mode_label(mode)),
                        AxRect::new(
                            frame.x + frame.width - 16.0 - 88.0,
                            y + (MODE_ROW_HEIGHT - CONTROL_ROW) / 2.0,
                            88.0,
                            CONTROL_ROW,
                        ),
                    )
                    .enabled(select_enabled)
                    .focused(select_focused)
                    .action(AxAction::Press),
                );
            }
            y += MODE_ROW_HEIGHT + ROW_GAP;
        }

        // ② 会话信任开关：状态行 + 切换按钮（与 render 同 gate，缺 Host
        // attached workspace_id 时禁用）。
        let workspace_attached = state.workspace_id.is_some();
        let trust_enabled = writes && workspace_attached;
        let trust_label = if state.workspace_trusted {
            "取消信任"
        } else {
            "信任 workspace"
        };
        let trust_state = if state.workspace_trusted {
            "已信任"
        } else {
            "未信任"
        };
        let trust_focused = self
            .settings_permissions_focus
            .get("settings-workspace-trust")
            .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
        page = page
            .child(
                AxNode::new(
                    "settings-workspace-trust-status",
                    AxRole::StaticText,
                    "会话信任",
                    AxRect::new(
                        frame.x + 16.0,
                        y,
                        (width - 180.0).max(60.0),
                        MODE_ROW_HEIGHT,
                    ),
                )
                .value(format!(
                    "当前 · {trust_state} · 信任当前 workspace（仅本会话，不写盘）"
                )),
            )
            .child(
                AxNode::new(
                    "settings-workspace-trust",
                    AxRole::Button,
                    trust_label,
                    AxRect::new(
                        frame.x + frame.width - 16.0 - 116.0,
                        y + (MODE_ROW_HEIGHT - CONTROL_ROW) / 2.0,
                        116.0,
                        CONTROL_ROW,
                    ),
                )
                .enabled(trust_enabled)
                .focused(trust_focused)
                .action(AxAction::Press),
            );
        y += MODE_ROW_HEIGHT + 8.0;

        // ③ Global 默认只读行。
        let global_text = match state.trust_workspaces_global {
            None => SETTINGS_TRUST_UNSET,
            Some(true) => "已设置：信任所有 workspace",
            Some(false) => "已设置：不信任所有 workspace",
        };
        page = page.child(
            AxNode::new(
                "settings-trust-global",
                AxRole::StaticText,
                "Global 默认",
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
            )
            .value(global_text),
        );
        y += STATUS_HEIGHT + 8.0;

        // ④ 生效边界（与 render 同源文案）。
        page.child(
            AxNode::new(
                "settings-permissions-effect",
                AxRole::StaticText,
                "Effect",
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(SETTINGS_PERMISSIONS_EFFECT_NOTE),
        )
    }
}
