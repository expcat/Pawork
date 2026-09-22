//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Window};

use super::{AxAction, AxNode, AxRect, AxRole};
use crate::projection::ConnectionState;
use crate::ui::i18n::t;
use crate::ui::settings::{
    approval_mode_description, approval_mode_label, permissions_status_lines,
    settings_permissions_effect_note, settings_trust_unset, APPROVAL_MODE_ALL,
};
use crate::ui::AppView;

impl AppView {
    /// 「权限与审批」页 AX（SET-6b）：五档审批模式（当前档只读、其余
    /// Press）、会话信任开关、Global 默认只读行、生效边界；stale 时
    /// enabled=false 且 permits 拒绝写动作，与 render 同 gate。
    pub(crate) fn settings_permissions_page_ax(
        &self,
        window: &Window,
        _cx: &App,
        frame: AxRect,
    ) -> AxNode {
        let state = &self.projection.settings_permissions;
        let writes = self.settings_permissions_writes_enabled();
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let refresh_focused =
            self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window);

        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.permissions.title"),
            frame,
        )
        .child(
            AxNode::new(
                "settings-page-title",
                AxRole::StaticText,
                t("settings.permissions.title"),
                self.settings_element_bounds("settings-page-title"),
            )
            .value(t("settings.permissions.subtitle")),
        )
        .child(
            AxNode::new(
                "settings-refresh",
                AxRole::Button,
                t("settings.refresh"),
                self.settings_element_bounds("settings-refresh"),
            )
            .enabled(connected)
            .focused(refresh_focused)
            .action(AxAction::Press),
        );

        for (kind, label) in permissions_status_lines(state) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    t("settings.permissions.ax_status"),
                    self.settings_element_bounds(&format!("settings-status-{kind}")),
                )
                .value(label),
            );
        }

        // ① 五档审批模式：每档是一个整行 radio；selected、enabled 与
        // Press 均与 render 同源。
        let current_mode_label = state
            .approval_mode
            .map(approval_mode_label)
            .unwrap_or(t("settings.permissions.unknown_mode"));
        page = page.child(
            AxNode::new(
                "settings-approval-mode-header",
                AxRole::StaticText,
                t("settings.permissions.mode_title"),
                self.settings_element_bounds("settings-approval-mode-header"),
            )
            .value(t("settings.current").replace("{}", current_mode_label)),
        );

        for mode in APPROVAL_MODE_ALL {
            let current = state.approval_mode == Some(mode);
            let mut value = format!(
                "{} · {}",
                approval_mode_label(mode),
                approval_mode_description(mode)
            );
            if current {
                value.push_str(&format!(" · {}", t("settings.permissions.state_current")));
            }
            let button_id = format!("settings-approval-mode-{}", mode.as_str());
            let select_enabled = writes && !current;
            let select_focused = self
                .settings_permissions_focus
                .get(&button_id)
                .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
            let mut row = AxNode::new(
                button_id.clone(),
                AxRole::Tab,
                approval_mode_label(mode),
                self.settings_element_bounds(&button_id),
            )
            .value(value)
            .selected(current)
            .enabled(current || select_enabled)
            .focused(select_focused);
            if current {
                row = row.description(t("settings.permissions.ax_current_mode_desc"));
            } else if select_enabled {
                row = row.action(AxAction::Press);
            }
            page = page.child(row);
        }

        // ② 会话信任开关：状态行 + 切换按钮（与 render 同 gate，缺 Host
        // attached workspace_id 时禁用）。
        let workspace_attached = state.workspace_id.is_some();
        let trust_enabled = writes && workspace_attached;
        let trust_label = if state.workspace_trusted {
            t("settings.permissions.trust_remove")
        } else {
            t("settings.permissions.trust_add")
        };
        let trust_state = if state.workspace_trusted {
            t("settings.permissions.trust_state_trusted")
        } else {
            t("settings.permissions.trust_state_untrusted")
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
                    t("settings.permissions.session_trust_title"),
                    self.settings_element_bounds("settings-workspace-trust-status"),
                )
                .value(format!(
                    "{} · {}",
                    t("settings.current").replace("{}", trust_state),
                    t("settings.permissions.session_trust_desc")
                )),
            )
            .child(
                AxNode::new(
                    "settings-workspace-trust",
                    AxRole::Button,
                    trust_label,
                    self.settings_element_bounds("settings-workspace-trust"),
                )
                .enabled(trust_enabled)
                .focused(trust_focused)
                .action(AxAction::Press),
            );

        // ③ Global 默认只读行。
        let global_text = match state.trust_workspaces_global {
            None => settings_trust_unset(),
            Some(true) => t("settings.permissions.global_trust_all"),
            Some(false) => t("settings.permissions.global_distrust_all"),
        };
        page = page.child(
            AxNode::new(
                "settings-trust-global",
                AxRole::StaticText,
                t("settings.permissions.ax_global_title"),
                self.settings_element_bounds("settings-trust-global"),
            )
            .value(global_text),
        );

        // ④ 生效边界（与 render 同源文案）。
        page.child(
            AxNode::new(
                "settings-permissions-effect",
                AxRole::StaticText,
                t("settings.permissions.ax_effect"),
                self.settings_element_bounds("settings-permissions-effect"),
            )
            .value(settings_permissions_effect_note()),
        )
    }
}
