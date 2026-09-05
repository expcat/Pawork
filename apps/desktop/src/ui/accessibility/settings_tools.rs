//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::Window;

use super::{dynamic_identifier, AxAction, AxNode, AxRect, AxRole};
use crate::projection::ConnectionState;
use crate::ui::i18n::t;
use crate::ui::settings::{
    settings_mcp_effect_note, settings_mcp_remove_confirm_note, tools_status_lines,
    SettingsMcpAction,
};
use crate::ui::AppView;

impl AppView {
    /// 「工具与 MCP」页 AX（SET-6c）：server 行复用 resources_ax 的形状
    /// （state · transport · tools 数 + last_error 描述），每行 Test /
    /// Remove（两步确认）按钮与 render 同 gate；stale 时 enabled=false
    /// 且 permits 拒绝写动作。几何为固定估值（SET-7 已登记与滚动的
    /// 已知缺口，与其他 Settings 页同口径）。
    pub(crate) fn settings_tools_page_ax(&self, window: &Window, frame: AxRect) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const STATUS_HEIGHT: f32 = 20.0;
        const CONTROL_ROW: f32 = 28.0;
        const CARD_PAD: f32 = 8.0;
        const NAME_ROW: f32 = 20.0;
        const META_ROW: f32 = 16.0;
        const CARD_GAP: f32 = 8.0;
        let writes = self.settings_tools_writes_enabled();
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let refresh_focused =
            self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window);
        let width = super::settings::settings_content_ax_width(frame);
        let mut page = AxNode::new("settings-page", AxRole::Group, t("settings.tools.title"), frame)
            .child(
                AxNode::new(
                    "settings-page-title",
                    AxRole::StaticText,
                    t("settings.tools.title"),
                    AxRect::new(
                        frame.x + 16.0,
                        frame.y + 16.0,
                        (width - 136.0).max(0.0),
                        HEADING_HEIGHT + SUBTITLE_HEIGHT,
                    ),
                )
                .value(t("settings.tools.subtitle")),
            )
            .child(
                AxNode::new(
                    "settings-refresh",
                    AxRole::Button,
                    t("settings.refresh"),
                    AxRect::new(
                        frame.x + 16.0 + width - 96.0,
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
        for (kind, label) in tools_status_lines(&self.resources) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    t("settings.tools.ax_status"),
                    AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
                )
                .value(label),
            );
            y += STATUS_HEIGHT + 8.0;
        }
        for server in &self.resources.servers {
            let confirming =
                self.settings_mcp_remove_confirm.as_deref() == Some(server.name.as_str());
            let mut actions = vec![SettingsMcpAction::Test];
            if confirming {
                actions.push(SettingsMcpAction::ConfirmRemove);
                actions.push(SettingsMcpAction::KeepRemove);
            } else {
                actions.push(SettingsMcpAction::Remove);
            }
            let confirm_rows = confirming as u8 as f32 * (STATUS_HEIGHT + CONTROL_ROW + CARD_GAP);
            let card_height = CARD_PAD * 2.0 + NAME_ROW + META_ROW + CONTROL_ROW + confirm_rows;
            let mut row = AxNode::new(
                dynamic_identifier("settings-mcp-server", &server.name),
                AxRole::ListItem,
                server.name.clone(),
                AxRect::new(frame.x + 16.0, y, width, card_height),
            )
            .value(
                t("settings.tools.ax_server_summary")
                    .replacen("{}", &server.state.to_string(), 1)
                    .replacen("{}", &server.transport.to_string(), 1)
                    .replacen("{}", &server.tool_count.to_string(), 1),
            );
            let mut description = server.last_error.clone().unwrap_or_default();
            if confirming {
                if description.is_empty() {
                    description = settings_mcp_remove_confirm_note().to_string();
                } else {
                    description =
                        format!("{description} · {}", settings_mcp_remove_confirm_note());
                }
            }
            row = row.description(description);
            page = page.child(row);
            // 动作按钮：与 render 同 identifier / 同 gate；焦点句柄与
            // provider 动作共用 settings_action_focus（identifier 键控）。
            let button_w = 96.0;
            for (ix, action) in actions.iter().enumerate() {
                let id = action.identifier(&server.name);
                let action_focused = self
                    .settings_action_focus
                    .get(&id)
                    .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                page = page.child(
                    AxNode::new(
                        id,
                        AxRole::Button,
                        action.label(),
                        AxRect::new(
                            frame.x + 16.0 + width - (actions.len() - ix) as f32 * (button_w + 4.0),
                            y + card_height - CARD_PAD - CONTROL_ROW,
                            button_w,
                            CONTROL_ROW,
                        ),
                    )
                    .enabled(writes && self.settings_mcp_server_action_enabled(&server.name))
                    .focused(action_focused)
                    .action(AxAction::Press),
                );
            }
            y += card_height + CARD_GAP;
        }
        // 生效边界（与 render 同源文案）。
        page.child(
            AxNode::new(
                "settings-mcp-effect",
                AxRole::StaticText,
                t("settings.tools.ax_effect"),
                AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT * 2.0),
            )
            .value(settings_mcp_effect_note()),
        )
    }
}
