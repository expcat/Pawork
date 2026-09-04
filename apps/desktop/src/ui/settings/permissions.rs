//! Settings permissions 页。

use super::*;

impl AppView {
    /// 「权限与审批」页（SET-6b / ADR-048）：① 五档审批模式显式选择
    /// （当前值高亮；切换发 `set_approval_mode`，等回执才改生效值）；
    /// ② 会话信任开关（发 `workspace_trust`，workspace_id 取当前
    /// attached）；③ `trust_workspaces_global` 只读行；④ 生效边界诚实
    /// 标注仅当前会话、不持久化。
    pub(super) fn settings_permissions_page_element(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let writes = self.settings_permissions_writes_enabled();
        let state = self.projection.settings_permissions.clone();
        let status_lines = permissions_status_lines(&state);
        let refresh_focus = self.settings_refresh_focus.clone();
        let refresh = Button::new("settings-refresh")
            .track_focus(&refresh_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label("Refresh")
            .tooltip("Refresh permissions settings")
            .disabled(!connected)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-refresh", event) {
                    return;
                }
                view.on_refresh_settings(cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-refresh");
                view.on_refresh_settings(cx);
                cx.stop_propagation();
            }));

        let mut content = div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_MAX_WIDTH))
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div().font_weight(FontWeight::MEDIUM).child(
                                    Label::new("权限与审批")
                                        .size(font::TITLE)
                                        .color(dark().text.primary),
                                ),
                            )
                            .child(
                                Label::new("当前会话的审批模式与 workspace 信任")
                                    .size(font::BODY_SM)
                                    .color(dark().text.secondary),
                            ),
                    )
                    .child(div().flex_1())
                    .child(div().flex_none().pt_1().child(refresh)),
            );
        for (kind, line) in status_lines {
            let color = if kind == "error" {
                dark().semantic.danger_text
            } else {
                dark().text.secondary
            };
            content = content.child(status_line(&line, color));
        }

        // ① 五档审批模式：当前值高亮只读，其余档位显式「选择」。
        let current_mode_label = state
            .approval_mode
            .map(approval_mode_label)
            .unwrap_or("未知");
        content = content
            .child(
                div().font_weight(FontWeight::MEDIUM).child(
                    Label::new("审批模式")
                        .size(font::BODY)
                        .color(dark().text.primary),
                ),
            )
            .child(
                Label::new(format!("当前 · {current_mode_label}"))
                    .size(font::BODY_SM)
                    .color(dark().text.secondary),
            );
        let mut modes = div().flex().flex_col().min_w_0().gap_1();
        for mode in APPROVAL_MODE_ALL {
            modes = modes.child(self.settings_approval_mode_row(mode, &state, writes, cx));
        }
        content = content.child(modes);

        // ② 会话信任开关：workspace_id 取 Host permissions_settings 透出的
        // attached id；缺 id 禁用（fail-closed，不猜注册表首项）。
        let trust_enabled = writes && state.workspace_id.is_some();
        let trust_label = if state.workspace_trusted {
            "取消信任"
        } else {
            "信任 workspace"
        };
        let trust_focus = self
            .settings_permissions_focus
            .entry("settings-workspace-trust".to_string())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let trust_toggle = Button::new("settings-workspace-trust")
            .track_focus(&trust_focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_ACTION_HEIGHT))
            .vcenter()
            .radius(4.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(trust_label)
            .tooltip("Toggle session workspace trust")
            .disabled(!trust_enabled)
            .on_click(cx.listener(|view, event, _window, cx| {
                if view.consume_button_key_click("settings-workspace-trust", event) {
                    return;
                }
                let trusted = view.projection.settings_permissions.workspace_trusted;
                view.on_settings_workspace_trust(!trusted, cx);
            }))
            .on_activate(cx.listener(|view, _event, _window, cx| {
                view.note_button_key_activate("settings-workspace-trust");
                let trusted = view.projection.settings_permissions.workspace_trusted;
                view.on_settings_workspace_trust(!trusted, cx);
                cx.stop_propagation();
            }));
        let trust_state_label = if state.workspace_trusted {
            "已信任"
        } else {
            "未信任"
        };
        content = content.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .min_w_0()
                .p(px(PROVIDER_CARD_PAD))
                .rounded(px(4.0))
                .border_1()
                .border_color(dark().border.subtle)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            Label::new("会话信任")
                                .size(font::BODY)
                                .color(dark().text.primary),
                        )
                        .child(
                            Label::new("信任当前 workspace（仅本会话，不写盘）")
                                .size(font::BODY_SM)
                                .color(dark().text.secondary),
                        ),
                )
                .child(
                    div().flex_none().child(
                        Label::new(format!("当前 · {trust_state_label}"))
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    ),
                )
                .child(div().flex_none().child(trust_toggle)),
        );

        // ③ Global 默认只读行（本片不写 Global trust）。
        let global_text = match state.trust_workspaces_global {
            None => SETTINGS_TRUST_UNSET,
            Some(true) => "已设置：信任所有 workspace",
            Some(false) => "已设置：不信任所有 workspace",
        };
        content = content.child(
            Label::new(format!("Global 默认（只读）· {global_text}"))
                .size(font::BODY_SM)
                .color(dark().text.secondary),
        );

        // ④ 生效边界诚实文案。
        content = content.child(
            Label::new(SETTINGS_PERMISSIONS_EFFECT_NOTE)
                .size(font::BODY_SM)
                .color(dark().text.secondary),
        );

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .p_4()
            .child(
                div()
                    .id("settings-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .track_scroll(&self.settings_scroll)
                    .child(content),
            )
    }

    /// 单个审批模式行：当前档高亮只读（accent 边框 + 「当前」徽标），
    /// 其余档位「选择」按钮（可见 / 键盘 / AX 三路径同 identifier、同
    /// gate）。
    pub(super) fn settings_approval_mode_row(
        &mut self,
        mode: ApprovalModeWire,
        state: &SettingsPermissionsState,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let current = state.approval_mode == Some(mode);
        let enabled = writes && !current;
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .min_w_0()
            .p(px(PROVIDER_CARD_PAD))
            .rounded(px(4.0))
            .border_1()
            .border_color(if current {
                dark().accent.primary
            } else {
                dark().border.subtle
            })
            .when(current, |el| el.bg(dark().surface.raised))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        Label::new(approval_mode_label(mode))
                            .size(font::BODY)
                            .color(dark().text.primary),
                    )
                    .child(
                        Label::new(approval_mode_description(mode))
                            .size(font::BODY_SM)
                            .color(dark().text.secondary),
                    ),
            );
        if current {
            row = row.child(
                div().flex_none().child(
                    Label::new("当前")
                        .size(font::XS)
                        .color(dark().accent.primary),
                ),
            );
        } else {
            let id = format!("settings-approval-mode-{}", mode.as_str());
            let focus = self
                .settings_permissions_focus
                .entry(id.clone())
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let click_id = id.clone();
            let activate_id = id.clone();
            let select = Button::new(id)
                .track_focus(&focus)
                .variant(ButtonVariant::Raised)
                .height(px(SETTINGS_ACTION_HEIGHT))
                .vcenter()
                .radius(4.0)
                .bordered()
                .text_size(font::BODY_SM)
                .label("选择")
                .disabled(!enabled)
                .on_click(cx.listener(move |view, event, _window, cx| {
                    if view.consume_button_key_click(&click_id, event) {
                        return;
                    }
                    view.on_settings_approval_mode(mode, cx);
                }))
                .on_activate(cx.listener(move |view, _event, _window, cx| {
                    view.note_button_key_activate(&activate_id);
                    view.on_settings_approval_mode(mode, cx);
                    cx.stop_propagation();
                }));
            row = row.child(div().flex_none().child(select));
        }
        row.into_any_element()
    }

    /// 切换审批模式（SET-6b；三路径同源）。入口级复核 gate 与当前值；
    /// 确认回执由 ApprovalModeConfirmed 收敛。
    pub(crate) fn on_settings_approval_mode(
        &mut self,
        mode: ApprovalModeWire,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_permissions_writes_enabled()
            || self.projection.settings_permissions.approval_mode == Some(mode)
        {
            return;
        }
        self.controller.set_approval_mode(mode);
        cx.notify();
    }

    /// 会话信任切换（SET-6b；三路径同源）。workspace_id 取 Host 查询透出的
    /// attached id（ADR-048 D1 实现期修订；缺 id fail-closed）；确认回执由
    /// WorkspaceTrustConfirmed 收敛。
    pub(crate) fn on_settings_workspace_trust(&mut self, trusted: bool, cx: &mut Context<Self>) {
        if !self.settings_permissions_writes_enabled()
            || self.projection.settings_permissions.workspace_trusted == trusted
        {
            return;
        }
        let Some(workspace_id) = self.projection.settings_permissions.workspace_id.clone() else {
            return;
        };
        self.controller.set_workspace_trust(&workspace_id, trusted);
        cx.notify();
    }
}
