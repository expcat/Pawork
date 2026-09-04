//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Focusable, Window};

use super::{dynamic_identifier, AxAction, AxNode, AxRect, AxRole};
use crate::projection::{ConnectionState, ProviderStatusLabels};
use crate::ui::settings::{
    provider_catalog_overview_label, provider_status_lines, settings_api_key_input_identifier,
    PROVIDER_OVERVIEW_HEIGHT, SETTINGS_DEFAULT_UNAVAILABLE_NOTE,
};
use crate::ui::AppView;

impl AppView {
    pub(crate) fn settings_providers_page_ax(
        &self,
        window: &Window,
        cx: &App,
        frame: AxRect,
    ) -> AxNode {
        const HEADING_HEIGHT: f32 = 28.0;
        const SUBTITLE_HEIGHT: f32 = 20.0;
        const STATUS_HEIGHT: f32 = 20.0;
        const CARD_PAD: f32 = 8.0;
        const CARD_GAP: f32 = 4.0;
        const TEXT_ROW: f32 = 18.0;
        const CONTROL_ROW: f32 = 28.0;
        let state = &self.projection.settings_providers;
        let writes = self.settings_writes_enabled();
        // 与 render 的 820px 内容列同源（宽窗钳制）；右缘锚定元素一律
        // 以 frame.x + 16 + width 计算，不直接用 frame.width。
        let width = super::settings::settings_content_ax_width(frame);
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
                        (width - 136.0).max(0.0),
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
        page = page.child(AxNode::new(
            "settings-providers-heading",
            AxRole::StaticText,
            "Providers",
            AxRect::new(frame.x + 16.0, y, width, STATUS_HEIGHT),
        ));
        y += STATUS_HEIGHT + 8.0;
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
            let row_actions: Vec<_> = actions
                .iter()
                .copied()
                .filter(|action| {
                    !matches!(
                        action,
                        crate::ui::settings::SettingsAuthAction::VerifyApiKey
                            | crate::ui::settings::SettingsAuthAction::CancelApiKeyInput
                    )
                })
                .collect();
            let actions_in_details = remove_confirm || row_actions.len() > 2;
            let editor_row = editor_open
                && self
                    .settings_api_key_inputs
                    .contains_key(&provider.provider_id);
            let endpoint_visible = editor_open || wait.is_some() || remove_confirm;
            let auth_error = match &provider.auth {
                crate::projection::ProviderAuthState::Error { message } => Some(message.as_str()),
                _ => None,
            };
            let catalog_error = matches!(
                (&provider.auth, &provider.catalog),
                (
                    crate::projection::ProviderAuthState::Connected { .. },
                    crate::projection::ProviderCatalogState::Unavailable { .. },
                )
            );
            let mut detail_values = Vec::new();
            if let (crate::projection::ProviderAuthState::Connecting, Some(wait)) =
                (&provider.auth, wait)
            {
                detail_values.push(format!("Authorize at {}", wait.verification_url));
                if let Some(code) = &wait.user_code {
                    detail_values.push(format!("Code {code}"));
                }
                if let Some(expires) = &wait.expires_at {
                    detail_values.push(format!("Expires {expires}"));
                }
            }
            if let Some(note) = state.auth_notes.get(&provider.provider_id) {
                detail_values.push(note.clone());
            }
            if let Some(message) = auth_error {
                detail_values.push(format!("Connection error · {message}"));
            }
            if catalog_error {
                detail_values.push(provider.catalog_label());
            }
            if endpoint_visible {
                detail_values.push(format!("Endpoint · {}", provider.endpoint_label));
            }
            let detail_actions = actions_in_details.then_some(row_actions.len()).unwrap_or(0);
            let detail_visible = !detail_values.is_empty() || editor_row || detail_actions > 0;
            let text_height = detail_values.len() as f32 * TEXT_ROW;
            let control_rows = editor_row as u8 as f32 + (detail_actions > 0) as u8 as f32;
            let detail_height = if detail_visible {
                CARD_PAD
                    + text_height
                    + control_rows * CONTROL_ROW
                    + (detail_values.len() as f32 + control_rows - 1.0).max(0.0) * CARD_GAP
                    + CARD_PAD
            } else {
                0.0
            };
            let card_height = PROVIDER_OVERVIEW_HEIGHT + detail_height;
            let card_x = frame.x + 16.0;
            let model_count = self
                .projection
                .models
                .iter()
                .filter(|model| model.provider_id == provider.provider_id)
                .count();
            let catalog_summary = provider_catalog_overview_label(provider, model_count);
            let auth_methods = provider.auth_methods_label();
            let auth_methods = if auth_methods.is_empty() {
                "No auth method".to_string()
            } else {
                auth_methods
            };
            let mut card = AxNode::new(
                dynamic_identifier("settings-provider", &provider.provider_id),
                AxRole::Group,
                provider.display_name.clone(),
                AxRect::new(card_x, y, width, card_height),
            )
            .value(format!(
                "{} · {} · {}",
                auth_methods,
                provider.auth_label(),
                catalog_summary
            ))
            .child(
                AxNode::new(
                    dynamic_identifier("settings-provider-name", &provider.provider_id),
                    AxRole::StaticText,
                    provider.display_name.clone(),
                    AxRect::new(card_x + 8.0, y, 172.0, PROVIDER_OVERVIEW_HEIGHT),
                )
                .value(auth_methods),
            )
            .child(
                AxNode::new(
                    dynamic_identifier("settings-provider-connection", &provider.provider_id),
                    AxRole::StaticText,
                    "Connection",
                    // render 列：name 172 + gap 8 + auth-methods 104 + gap 8；
                    // auth-methods 已并入 name 节点 value，后续列必须平移。
                    AxRect::new(card_x + 300.0, y, 132.0, PROVIDER_OVERVIEW_HEIGHT),
                )
                .value(provider.auth_label()),
            )
            .child(
                AxNode::new(
                    dynamic_identifier("settings-provider-catalog", &provider.provider_id),
                    AxRole::StaticText,
                    "Catalog",
                    AxRect::new(card_x + 440.0, y, 132.0, PROVIDER_OVERVIEW_HEIGHT),
                )
                .value(catalog_summary),
            );

            let header_actions: Vec<_> = if actions_in_details {
                Vec::new()
            } else {
                row_actions.clone()
            };
            let mut button_x = card_x + width - 8.0 - header_actions.len() as f32 * 114.0;
            for action in header_actions {
                let identifier = action.identifier(&provider.provider_id);
                let enabled =
                    self.settings_action_enabled(action, &provider.provider_id, writes, cx);
                let focused = self
                    .settings_action_focus
                    .get(&identifier)
                    .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                let mut button = AxNode::new(
                    identifier,
                    AxRole::Button,
                    action.label(),
                    AxRect::new(
                        button_x,
                        y + (PROVIDER_OVERVIEW_HEIGHT - CONTROL_ROW) / 2.0,
                        110.0,
                        CONTROL_ROW,
                    ),
                )
                .enabled(enabled)
                .focused(focused);
                if enabled {
                    button = button.action(AxAction::Press);
                }
                card = card.child(button);
                button_x += 114.0;
            }

            let mut detail_y = y + PROVIDER_OVERVIEW_HEIGHT + CARD_PAD;
            if !detail_values.is_empty() {
                card = card.child(
                    AxNode::new(
                        dynamic_identifier("settings-provider-details", &provider.provider_id),
                        AxRole::StaticText,
                        "Provider details",
                        AxRect::new(card_x + 8.0, detail_y, width - 16.0, text_height),
                    )
                    .value(detail_values.join(" · ")),
                );
                detail_y += text_height + CARD_GAP;
            }
            if editor_row {
                if let Some(input) = self.settings_api_key_inputs.get(&provider.provider_id) {
                    let masked = input.read(cx).secure_mask().unwrap_or_default();
                    let mut input_node = AxNode::new(
                        settings_api_key_input_identifier(&provider.provider_id),
                        AxRole::TextArea,
                        "API key",
                        AxRect::new(
                            card_x + 8.0,
                            detail_y,
                            (width - 16.0 - 240.0).max(120.0),
                            CONTROL_ROW,
                        ),
                    )
                    .value(masked)
                    .enabled(writes)
                    .focused(
                        self.open_menu.is_none()
                            && input.read(cx).focus_handle(cx).is_focused(window),
                    );
                    if writes {
                        input_node = input_node
                            .action(AxAction::Focus)
                            .action(AxAction::SetValue);
                    }
                    card = card.child(input_node);
                }
                let editor_actions: Vec<_> = actions
                    .iter()
                    .copied()
                    .filter(|action| {
                        matches!(
                            action,
                            crate::ui::settings::SettingsAuthAction::VerifyApiKey
                                | crate::ui::settings::SettingsAuthAction::CancelApiKeyInput
                        )
                    })
                    .collect();
                let mut editor_button_x =
                    card_x + width - 8.0 - editor_actions.len() as f32 * 114.0;
                for action in editor_actions {
                    let identifier = action.identifier(&provider.provider_id);
                    let enabled =
                        self.settings_action_enabled(action, &provider.provider_id, writes, cx);
                    let focused = self
                        .settings_action_focus
                        .get(&identifier)
                        .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                    let mut button = AxNode::new(
                        identifier,
                        AxRole::Button,
                        action.label(),
                        AxRect::new(editor_button_x, detail_y, 110.0, CONTROL_ROW),
                    )
                    .enabled(enabled)
                    .focused(focused);
                    if enabled {
                        button = button.action(AxAction::Press);
                    }
                    card = card.child(button);
                    editor_button_x += 114.0;
                }
                detail_y += CONTROL_ROW + CARD_GAP;
            }
            if detail_actions > 0 {
                let mut detail_button_x = card_x + 8.0;
                for action in row_actions {
                    let identifier = action.identifier(&provider.provider_id);
                    let enabled =
                        self.settings_action_enabled(action, &provider.provider_id, writes, cx);
                    let focused = self
                        .settings_action_focus
                        .get(&identifier)
                        .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                    let mut button = AxNode::new(
                        identifier,
                        AxRole::Button,
                        action.label(),
                        AxRect::new(detail_button_x, detail_y, 110.0, CONTROL_ROW),
                    )
                    .enabled(enabled)
                    .focused(focused);
                    if enabled {
                        button = button.action(AxAction::Press);
                    }
                    card = card.child(button);
                    detail_button_x += 114.0;
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
            "Default model",
            AxRect::new(models_x, models_y, width, section_height),
        )
        .child(AxNode::new(
            "settings-models-title",
            AxRole::StaticText,
            "Default model",
            AxRect::new(models_x, models_y, width, HEADING_HEIGHT + SUBTITLE_HEIGHT),
        ))
        .value("Choose the model used when a new task starts");
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
                let mut value = model.display_name.clone();
                if is_default {
                    value.push_str(" · Default");
                }
                let mut default_button = AxNode::new(
                    identifier,
                    AxRole::Button,
                    "Set default",
                    AxRect::new(models_x + width - 104.0, row_y, 104.0, MODEL_ROW_HEIGHT),
                )
                .enabled(enabled)
                .focused(focused);
                if enabled {
                    default_button = default_button.action(AxAction::Press);
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
                    .child(default_button);
                row_y += MODEL_ROW_HEIGHT + CARD_GAP;
            }
            section = section.child(group);
            models_y += group_height + 8.0;
        }
        page = page.child(section);
        page
    }
}
