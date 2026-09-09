//! UX-04：连接与终端在发生处给出原因和恢复入口；不改变 Host 权限。
use super::accessibility::{AxAction, AxNode, AxRect, AxRole};
use super::i18n::t;
use super::*;
use crate::projection::{ApprovalModeWire, TerminalAvailability};

pub(super) const RECOVERY_IDS: [&str; 6] = [
    "connection-notice",
    "connection-retry",
    "connection-diagnostics",
    "terminal-notice",
    "terminal-permissions",
    "terminal-details",
];

impl AppView {
    fn recovery_element(&self, id: &'static str) -> gpui::Stateful<gpui::Div> {
        div().id(id).track_scroll(&self.recovery_layouts[id])
    }

    fn recovery_bounds(&self, id: &'static str, clip: AxRect) -> AxRect {
        let b = self.recovery_layouts[id].bounds();
        let x = f32::from(b.origin.x).max(clip.x);
        let y = f32::from(b.origin.y).max(clip.y);
        AxRect::new(
            x,
            y,
            (f32::from(b.origin.x + b.size.width).min(clip.x + clip.width) - x).max(0.0),
            (f32::from(b.origin.y + b.size.height).min(clip.y + clip.height) - y).max(0.0),
        )
    }

    fn recovery_button(
        &self,
        id: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.recovery_element(id).child(
            Button::new(id)
                .variant(ButtonVariant::Raised)
                .label(label)
                .track_focus(&self.recovery_focus[id])
                .on_click(cx.listener(move |view, event, window, cx| {
                    if !view.consume_button_key_click(id, event) {
                        view.on_recovery_action(id, window, cx);
                    }
                }))
                .on_activate(cx.listener(move |view, _, window, cx| {
                    view.note_button_key_activate(id);
                    view.on_recovery_action(id, window, cx);
                    cx.stop_propagation();
                })),
        )
    }

    pub(super) fn on_recovery_action(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match id {
            "connection-retry" => self.on_reconnect(window, cx),
            "connection-diagnostics" => {
                self.on_open_settings(window, cx);
                self.on_select_settings_page(SettingsPage::Advanced, window, cx);
            }
            "terminal-permissions" => {
                let page = if self.projection.settings_permissions.available {
                    SettingsPage::Permissions
                } else {
                    SettingsPage::Advanced
                };
                self.on_open_settings(window, cx);
                self.on_select_settings_page(page, window, cx);
            }
            "terminal-details" => {
                let key = self.terminal_error_key();
                self.terminal_details_open = if self.terminal_details_open == key {
                    None
                } else {
                    key
                };
                cx.notify();
            }
            _ => {}
        }
    }

    pub(super) fn connection_notice_text(&self) -> (&'static str, &'static str) {
        match &self.projection.connection {
            ConnectionState::Connecting => (
                t(if self.connection_attempts > 1 {
                    "recovery.reconnecting"
                } else {
                    "connection.connecting"
                }),
                t("recovery.wait"),
            ),
            ConnectionState::Disconnected { .. } => {
                (t("recovery.disconnected"), t("recovery.disconnected_help"))
            }
            ConnectionState::Failed { reason } => {
                let lower = reason.to_ascii_lowercase();
                let key = if lower.contains("token file") {
                    "recovery.credentials_missing"
                } else if lower.contains("no such file") || lower.contains("connection refused") {
                    "recovery.service_missing"
                } else if lower.contains("permission denied") || lower.contains("unauthorized") {
                    "recovery.access_denied"
                } else if lower.contains("timed out") || lower.contains("timeout") {
                    "recovery.timeout"
                } else {
                    "recovery.connect_help"
                };
                (t("recovery.failed"), t(key))
            }
            ConnectionState::Connected { .. } => (t("settings.advanced.connected"), ""),
        }
    }

    pub(super) fn connection_notice_element(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, help) = self.connection_notice_text();
        self.recovery_element("connection-notice")
            .flex_none()
            .p_4()
            .child(div().text_size(font::TITLE).child(title))
            .when(
                self.connection_attempts > 1
                    && matches!(self.projection.connection, ConnectionState::Failed { .. }),
                |area| {
                    area.child(
                        div().mt_2().text_size(font::SM).child(
                            t("recovery.attempt_failed")
                                .replace("{}", &self.connection_attempts.to_string()),
                        ),
                    )
                },
            )
            .child(
                div()
                    .mt_2()
                    .whitespace_normal()
                    .text_size(font::BODY)
                    .child(help),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .mt_3()
                    .when(self.projection.show_reconnect(), |row| {
                        row.child(self.recovery_button("connection-retry", t("recovery.retry"), cx))
                    })
                    .child(self.recovery_button(
                        "connection-diagnostics",
                        t("recovery.diagnostics"),
                        cx,
                    )),
            )
    }

    pub(super) fn terminal_read_only(&self) -> bool {
        let permissions = &self.projection.settings_permissions;
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) || permissions.stale_reason.is_some()
        {
            return false;
        }
        if permissions.available {
            return permissions.approval_mode == Some(ApprovalModeWire::ReadOnly);
        }
        // 权限查询未知时，当前连接刚收到的明确拒绝也是事实。
        self.terminal_error_key()
            .is_some_and(|(_, reason)| reason.contains("审批档 read_only 禁止创建终端"))
    }

    pub(super) fn terminal_create_blocked(&self) -> bool {
        self.terminal_read_only() || self.inspector_workspace_id().is_none()
    }

    pub(super) fn terminal_start_available(&self) -> bool {
        let terminal = &self.projection.terminal;
        terminal_start_enabled(
            &self.projection.connection,
            terminal,
            self.terminal_pending_create_workspace.as_ref(),
            self.terminal_pending_resize.is_some(),
        ) && ((terminal.session_id.is_some() && !terminal_can_reopen(terminal))
            || !self.terminal_create_blocked())
    }

    fn terminal_error_key(&self) -> Option<(Option<String>, String)> {
        let terminal = &self.projection.terminal;
        if let Some(reason) = &terminal.last_error {
            return Some((terminal.workspace_id.clone(), reason.clone()));
        }
        match &terminal.availability {
            TerminalAvailability::Failed { reason } => Some((
                self.projection.terminal.workspace_id.clone(),
                reason.clone(),
            )),
            _ => None,
        }
    }

    pub(super) fn terminal_notice_text(&self) -> Option<String> {
        let terminal = &self.projection.terminal;
        if !matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        ) {
            return Some(t("recovery.terminal_offline").into());
        }
        if self.terminal_pending_create_workspace.as_ref() == terminal.workspace_id.as_ref()
            && self.terminal_pending_create_workspace.is_some()
        {
            return Some(t("recovery.terminal_starting").into());
        }
        if let Some((_, reason)) = self.terminal_error_key() {
            // Host Policy 的已知拒绝原因；未知错误不从字符串猜安全模式。
            return Some(
                t(if reason.contains("审批档 read_only 禁止创建终端") {
                    if self.projection.settings_permissions.available {
                        "recovery.terminal_read_only"
                    } else {
                        "recovery.terminal_read_only_unknown"
                    }
                } else {
                    "recovery.terminal_failed"
                })
                .into(),
            );
        }
        if terminal.session_id.is_none() || terminal_can_reopen(terminal) {
            if self.terminal_read_only() {
                return Some(t("recovery.terminal_read_only").into());
            }
            if self.inspector_workspace_id().is_none() {
                return Some(t("recovery.terminal_project").into());
            }
        }
        None
    }

    pub(super) fn terminal_notice_element(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.terminal_error_key();
        self.recovery_element("terminal-notice")
            .flex_none()
            .p_3()
            .text_size(font::SM)
            .child(
                div()
                    .whitespace_normal()
                    .child(self.terminal_notice_text().unwrap_or_default()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .mt_2()
                    .when(self.terminal_read_only(), |row| {
                        row.child(self.recovery_button(
                            "terminal-permissions",
                            t(if self.projection.settings_permissions.available {
                                "recovery.permissions"
                            } else {
                                "recovery.diagnostics"
                            }),
                            cx,
                        ))
                    })
                    .when(error.is_some(), |row| {
                        row.child(self.recovery_button(
                            "terminal-details",
                            t(if self.terminal_details_open == error {
                                "recovery.hide_details"
                            } else {
                                "recovery.details"
                            }),
                            cx,
                        ))
                    }),
            )
            .when(
                error.is_some() && self.terminal_details_open == error,
                |area| area.child(div().mt_2().whitespace_normal().child(error.unwrap().1)),
            )
    }

    pub(super) fn recovery_ax(&self, window: &Window, clip: AxRect, terminal: bool) -> AxNode {
        let (id, text) = if terminal {
            (
                "terminal-notice",
                self.terminal_notice_text().unwrap_or_default(),
            )
        } else {
            let (title, help) = self.connection_notice_text();
            ("connection-notice", format!("{title}\n{help}"))
        };
        let mut node = AxNode::new(id, AxRole::Group, text, self.recovery_bounds(id, clip));
        if !terminal
            && self.connection_attempts > 1
            && matches!(self.projection.connection, ConnectionState::Failed { .. })
        {
            node = node.value(
                t("recovery.attempt_failed").replace("{}", &self.connection_attempts.to_string()),
            );
        }
        let mut actions = Vec::new();
        if terminal {
            if self.terminal_read_only() {
                actions.push((
                    "terminal-permissions",
                    t(if self.projection.settings_permissions.available {
                        "recovery.permissions"
                    } else {
                        "recovery.diagnostics"
                    }),
                ));
            }
            if let Some(key) = self.terminal_error_key() {
                let expanded = self.terminal_details_open.as_ref() == Some(&key);
                actions.push((
                    "terminal-details",
                    t(if expanded {
                        "recovery.hide_details"
                    } else {
                        "recovery.details"
                    }),
                ));
                if expanded {
                    node = node.value(key.1);
                }
            }
        } else {
            if self.projection.show_reconnect() {
                actions.push(("connection-retry", t("recovery.retry")));
            }
            actions.push(("connection-diagnostics", t("recovery.diagnostics")));
        }
        for (id, label) in actions {
            let bounds = self.recovery_bounds(id, clip);
            if bounds.width > 0.0 && bounds.height > 0.0 {
                node = node.child(
                    AxNode::new(id, AxRole::Button, label, bounds)
                        .focused(self.recovery_focus[id].is_focused(window))
                        .action(AxAction::Press),
                );
            }
        }
        node
    }
}
