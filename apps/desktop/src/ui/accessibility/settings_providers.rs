//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Focusable, Window};

use super::{dynamic_identifier, AxAction, AxNode, AxRect, AxRole};
use crate::projection::{ConnectionState, ProviderStatusLabels};
use crate::ui::settings::{
    provider_status_lines, settings_api_key_input_identifier, SETTINGS_DEFAULT_UNAVAILABLE_NOTE,
};
use crate::ui::AppView;

impl AppView {
    pub(crate) fn settings_providers_page_ax(&self, window: &Window, cx: &App, frame: AxRect) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const STATUS_HEIGHT: f32 = 20.0;
        const CARD_PAD: f32 = 16.0;
        const CARD_GAP: f32 = 4.0;
        const TEXT_ROW: f32 = 18.0;
        const HEADER_ROW: f32 = 20.0;
        const CONTROL_ROW: f32 = 28.0;
        let state = &self.projection.settings_providers;
        let writes = self.settings_writes_enabled();
        // SET-5：页级刷新按钮（连接态 gate，与 render 同源）。
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let refresh_focused =
            self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window);
        let mut page = AxNode::new("settings-page", AxRole::Group, "Models & providers", frame)
            .child(
                AxNode::new(
                    "settings-page-title",
                    AxRole::StaticText,
                    "Models & providers",
                    AxRect::new(
                        frame.x + 16.0,
                        frame.y + 16.0,
                        (frame.width - 136.0).max(0.0),
                        HEADING_HEIGHT + SUBTITLE_HEIGHT,
                    ),
                )
                .value("Connection status and catalog source for each provider"),
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
        // 与 render 同源（SET-3 修复 2）：stale / loading / error / 空态各自
        // 独立发布，stale 与 error 可同时存在，不再三选一合并。
        for (kind, label) in provider_status_lines(state) {
            page = page.child(
                AxNode::new(
                    format!("settings-status-{kind}"),
                    AxRole::StaticText,
                    "Provider status",
                    AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
                )
                .value(label),
            );
            y += STATUS_HEIGHT + 8.0;
        }
        for provider in state.providers.iter() {
            let wait = state.oauth_waits.get(&provider.provider_id);
            let editor_open = self.settings_api_key_editor_visible(provider);
            let remove_confirm =
                self.settings_remove_confirm.as_deref() == Some(provider.provider_id.as_str());
            let actions = crate::ui::settings::settings_auth_actions(
                provider,
                editor_open,
                remove_confirm,
                wait.is_some(),
            );
            // 卡高估值：header + auth/endpoint/catalog 三行 + 可选等待 /
            // note 行 + 可选输入 / 动作控件行。
            let mut text_rows = 3.0;
            if let Some(wait) = wait {
                text_rows += 1.0
                    + wait.user_code.is_some() as u8 as f32
                    + wait.expires_at.is_some() as u8 as f32;
            }
            if state.auth_notes.contains_key(&provider.provider_id) {
                text_rows += 1.0;
            }
            let editor_row = editor_open
                && self
                    .settings_api_key_inputs
                    .contains_key(&provider.provider_id);
            let action_row = !actions.is_empty();
            let control_rows = editor_row as u8 as f32 + action_row as u8 as f32;
            let children = 1.0 + text_rows + control_rows;
            let card_height = CARD_PAD
                + HEADER_ROW
                + text_rows * TEXT_ROW
                + control_rows * CONTROL_ROW
                + (children - 1.0).max(0.0) * CARD_GAP;

            let card_x = frame.x + 16.0;
            let mut value = format!(
                "{} · {} · {}",
                provider.auth_methods_label(),
                provider.auth_label(),
                provider.catalog_label()
            );
            if let (crate::projection::ProviderAuthState::Connecting, Some(wait)) =
                (&provider.auth, wait)
            {
                value.push_str(&format!(" · Authorize at {}", wait.verification_url));
                if let Some(code) = &wait.user_code {
                    value.push_str(&format!(" · Code {code}"));
                }
                if let Some(expires) = &wait.expires_at {
                    value.push_str(&format!(" · Expires {expires}"));
                }
            }
            if let Some(note) = state.auth_notes.get(&provider.provider_id) {
                value.push_str(&format!(" · {note}"));
            }
            let mut card = AxNode::new(
                dynamic_identifier("settings-provider", &provider.provider_id),
                AxRole::Group,
                provider.display_name.clone(),
                AxRect::new(card_x, y, width, card_height),
            )
            .child(
                AxNode::new(
                    dynamic_identifier("settings-provider-summary", &provider.provider_id),
                    AxRole::StaticText,
                    provider.display_name.clone(),
                    AxRect::new(
                        card_x + 8.0,
                        y + 8.0,
                        (width - 16.0).max(0.0),
                        HEADER_ROW + text_rows * TEXT_ROW,
                    ),
                )
                .value(value)
                .description(provider.endpoint_label.clone()),
            );

            // 控件行自卡底向上推导（与 render 的行序一致：editor 在上、
            // 动作行在下）。
            let mut control_y = y + card_height - 8.0 - CONTROL_ROW;
            if action_row {
                let mut button_x = card_x + 8.0;
                for action in &actions {
                    let identifier = action.identifier(&provider.provider_id);
                    // 与 render 同源的逐按钮启用谓词（空输入 Verify 在 AX
                    // 侧同样拒绝，不只依赖入口复核）。
                    let enabled =
                        self.settings_action_enabled(*action, &provider.provider_id, writes, cx);
                    let focused = self
                        .settings_action_focus
                        .get(&identifier)
                        .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                    card = card.child(
                        AxNode::new(
                            identifier,
                            AxRole::Button,
                            action.label(),
                            AxRect::new(button_x, control_y, 110.0, CONTROL_ROW),
                        )
                        .enabled(enabled)
                        .focused(focused)
                        .action(AxAction::Press),
                    );
                    button_x += 110.0 + 4.0;
                }
                control_y -= CONTROL_ROW + CARD_GAP;
            }
            if editor_row {
                if let Some(input) = self.settings_api_key_inputs.get(&provider.provider_id) {
                    // SET-010：AX value 恒为掩码（或空），明文不进语义树。
                    let masked = input.read(cx).secure_mask().unwrap_or_default();
                    card = card.child(
                        AxNode::new(
                            settings_api_key_input_identifier(
                                &provider.provider_id,
                            ),
                            AxRole::TextArea,
                            "API key",
                            AxRect::new(
                                card_x + 8.0,
                                control_y,
                                (width - 16.0 - 240.0).max(120.0),
                                CONTROL_ROW,
                            ),
                        )
                        .value(masked)
                        .enabled(writes)
                        .focused(
                            self.open_menu.is_none()
                                && input.read(cx).focus_handle(cx).is_focused(window),
                        )
                        .action(AxAction::Focus)
                        .action(AxAction::SetValue),
                    );
                }
            }
            page = page.child(card);
            y += card_height + 8.0;
        }

        // SET-5「模型与默认项」区（与 render 同源）：分组模型行、默认
        // 徽标并入行 value、失效默认显式提示行、「设为默认」按钮与可见
        // 路径同 identifier / 同 gate（stale 或未连接 provider 时
        // enabled=false 且 permits 拒绝）。高度按行数固定估值。
        const MODEL_GROUP_HEADER: f32 = 20.0;
        const MODEL_ROW_HEIGHT: f32 = 28.0;
        let default = state.default_model.clone();
        let unavailable = self.projection.default_model_unavailable();
        let groups = crate::projection::group_models_by_provider(&self.projection.models);
        let mut section_height = HEADING_HEIGHT + SUBTITLE_HEIGHT + 8.0;
        if unavailable {
            section_height += STATUS_HEIGHT + 8.0;
        }
        if groups.is_empty() {
            section_height += STATUS_HEIGHT + 8.0;
        }
        for (_, models) in &groups {
            section_height +=
                8.0 + MODEL_GROUP_HEADER + models.len() as f32 * (MODEL_ROW_HEIGHT + CARD_GAP);
        }
        let models_x = frame.x + 16.0;
        let mut models_y = y + 8.0;
        let mut section = AxNode::new(
            "settings-models",
            AxRole::Group,
            "Models & defaults",
            AxRect::new(models_x, models_y, width, section_height),
        )
        .child(AxNode::new(
            "settings-models-title",
            AxRole::StaticText,
            "Models & defaults",
            AxRect::new(models_x, models_y, width, HEADING_HEIGHT + SUBTITLE_HEIGHT),
        ))
        .value("Runnable models per provider; the default applies to new runs");
        models_y += HEADING_HEIGHT + SUBTITLE_HEIGHT + 8.0;
        if unavailable {
            section = section.child(
                AxNode::new(
                    "settings-models-unavailable",
                    AxRole::StaticText,
                    "Default model",
                    AxRect::new(models_x, models_y, width, STATUS_HEIGHT),
                )
                .value(SETTINGS_DEFAULT_UNAVAILABLE_NOTE),
            );
            models_y += STATUS_HEIGHT + 8.0;
        }
        if groups.is_empty() {
            section = section.child(
                AxNode::new(
                    "settings-models-empty",
                    AxRole::StaticText,
                    "Models",
                    AxRect::new(models_x, models_y, width, STATUS_HEIGHT),
                )
                .value("No models reported by the host."),
            );
            models_y += STATUS_HEIGHT + 8.0;
        }
        for (provider_id, models) in groups {
            // 组头显示名取 provider 权威清单（与 render 同源回落）。
            let display_name = state
                .providers
                .iter()
                .find(|entry| entry.provider_id == provider_id)
                .map(|entry| entry.display_name.clone())
                .unwrap_or_else(|| provider_id.to_string());
            let group_height =
                MODEL_GROUP_HEADER + models.len() as f32 * (MODEL_ROW_HEIGHT + CARD_GAP);
            let mut group = AxNode::new(
                dynamic_identifier("settings-model-group", &provider_id),
                AxRole::Group,
                display_name.clone(),
                AxRect::new(models_x, models_y, width, group_height),
            )
            .child(AxNode::new(
                dynamic_identifier("settings-model-group-title", &provider_id),
                AxRole::StaticText,
                display_name,
                AxRect::new(models_x, models_y, width, MODEL_GROUP_HEADER),
            ));
            let mut row_y = models_y + MODEL_GROUP_HEADER;
            for model in models {
                let is_default = default.as_ref().is_some_and(|(provider, id)| {
                    provider == &model.provider_id && id == &model.id
                });
                let identifier = crate::ui::settings::settings_set_default_identifier(
                    &model.provider_id,
                    &model.id,
                );
                // 与 render 同源的启用谓词（入口派发前 permits 已核对）。
                let enabled = self.settings_set_default_enabled(&model.provider_id, &model.id);
                let focused = self
                    .settings_action_focus
                    .get(&identifier)
                    .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                let mut value = format!("{} · {}", model.display_name, model.id);
                if is_default {
                    value.push_str(" · Default");
                }
                group = group
                    .child(
                        AxNode::new(
                            dynamic_identifier(
                                "settings-model",
                                &format!("{}:{}", model.provider_id, model.id),
                            ),
                            AxRole::StaticText,
                            model.display_name.clone(),
                            AxRect::new(
                                models_x + 8.0,
                                row_y,
                                (width - 136.0).max(60.0),
                                MODEL_ROW_HEIGHT,
                            ),
                        )
                        .value(value),
                    )
                    .child(
                        AxNode::new(
                            identifier,
                            AxRole::Button,
                            "Set default",
                            AxRect::new(
                                frame.x + frame.width - 16.0 - 104.0,
                                row_y,
                                104.0,
                                MODEL_ROW_HEIGHT,
                            ),
                        )
                        .enabled(enabled)
                        .focused(focused)
                        .action(AxAction::Press),
                    );
                row_y += MODEL_ROW_HEIGHT + CARD_GAP;
            }
            section = section.child(group);
            models_y += group_height + 8.0;
        }
        page = page.child(section);
        page
    }
}
