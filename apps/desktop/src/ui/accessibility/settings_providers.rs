//! Settings AX 导航与页分发。identifier / Press gate / 几何与 render 同源。

use gpui::{App, Focusable, Window};

use super::{dynamic_identifier, AxAction, AxNode, AxRect, AxRole};
use crate::projection::{ConnectionState, ModelEntry, ProviderStatusLabels};
use crate::ui::components::dropdown::{ANCHOR_GAP_Y, MENU_MAX_HEIGHT};
use crate::ui::components::switch::SWITCH_TRACK_WIDTH;
use crate::ui::i18n::t;
use crate::ui::settings::{
    provider_catalog_overview_label, provider_credential_kind_label,
    provider_credential_status_label, provider_status_lines, settings_api_key_input_identifier,
    settings_default_unavailable_note, settings_manage_models_identifier,
    settings_model_switch_identifier, settings_models_disable_all_identifier,
    settings_models_enable_all_identifier, settings_models_menu_identifier,
    settings_models_refresh_identifier, settings_provider_expand_identifier,
    settings_role_candidates, settings_role_clear_identifier, settings_role_description_label,
    settings_role_item_identifier, settings_role_trigger_identifier,
    settings_use_proxy_identifier, SettingsRole, PROVIDER_OVERVIEW_HEIGHT,
    SETTINGS_CONTENT_PAD, SETTINGS_MODELS_MENU_EMPTY_HEIGHT, SETTINGS_MODELS_MENU_HEADER_HEIGHT,
    SETTINGS_MODELS_MENU_MAX_HEIGHT, SETTINGS_MODELS_MENU_ROW_HEIGHT, SETTINGS_MODELS_MENU_WIDTH,
    SETTINGS_PROVIDER_CREDENTIALS_HEADER_HEIGHT, SETTINGS_PROVIDER_CREDENTIAL_ROW_HEIGHT,
    SETTINGS_PROVIDER_ROW_HEIGHT, SETTINGS_ROLE_LABEL_WIDTH, SETTINGS_ROLE_MENU_EMPTY_HEIGHT,
    SETTINGS_ROLE_MENU_GROUP_HEADER_HEIGHT, SETTINGS_ROLE_MENU_WIDTH, SETTINGS_ROLE_ROW_HEIGHT,
};
use crate::ui::theme::metrics;
use crate::ui::AppView;
use crate::ui::MenuKind;

/// 代理 Switch 组（轨道 + 状态文案）在概览行的 AX 估值宽度。
const SWITCH_GROUP_WIDTH: f32 = SWITCH_TRACK_WIDTH + 4.0 + 24.0;

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
        // 与 render 的全宽内容列同源（OPT-4c：Rail 外全宽、两侧 32px）；
        // 右缘锚定元素一律以 frame.x + SETTINGS_CONTENT_PAD + width
        // 计算，不直接用 frame.width。
        let width = super::settings::settings_content_ax_width(frame);
        // SET-5：页级刷新按钮（连接态 gate，与 render 同源）。
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let refresh_focused =
            self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window);
        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.providers.title"),
            frame,
        )
        .child(
            AxNode::new(
                "settings-page-title",
                AxRole::StaticText,
                t("settings.providers.title"),
                AxRect::new(
                    frame.x + SETTINGS_CONTENT_PAD,
                    frame.y + 16.0,
                    (width - 136.0).max(0.0),
                    HEADING_HEIGHT + SUBTITLE_HEIGHT,
                ),
            )
            .value(t("settings.providers.subtitle")),
        )
        .child(
            AxNode::new(
                "settings-refresh",
                AxRole::Button,
                t("settings.refresh"),
                AxRect::new(
                    frame.x + SETTINGS_CONTENT_PAD + width - 96.0,
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
                    t("settings.providers.ax_status"),
                    AxRect::new(frame.x + SETTINGS_CONTENT_PAD, y, width, STATUS_HEIGHT),
                )
                .value(label),
            );
            y += STATUS_HEIGHT + 8.0;
        }
        // OPT-3b / ADR-055 D5「Default models」四角色区（与 render 同源）：
        // 触发器 identifier / gate / 值文案 / 几何同源；菜单展开时发布
        // 可选行（Press 派发与 render 点击同入口）。高度按固定行高估值。
        let roles_unavailable = self.projection.default_model_unavailable();
        let roles_header_height = HEADING_HEIGHT + SUBTITLE_HEIGHT;
        let mut roles_height = roles_header_height + 8.0;
        if roles_unavailable {
            roles_height += STATUS_HEIGHT + 8.0;
        }
        let roles_card_height =
            CARD_PAD + SettingsRole::ALL.len() as f32 * SETTINGS_ROLE_ROW_HEIGHT + CARD_PAD;
        roles_height += roles_card_height;
        let roles_x = frame.x + SETTINGS_CONTENT_PAD;
        // render 行内布局：8px 卡片内边距 + 172px 角色名 + 8px gap。
        let menu_x = roles_x + CARD_PAD + SETTINGS_ROLE_LABEL_WIDTH + 8.0;
        let mut section = AxNode::new(
            "settings-default-roles",
            AxRole::Group,
            t("settings.roles.title"),
            AxRect::new(roles_x, y, width, roles_height),
        )
        .child(AxNode::new(
            "settings-default-roles-title",
            AxRole::StaticText,
            t("settings.roles.title"),
            AxRect::new(roles_x, y, width, roles_header_height),
        ))
        .value(t("settings.roles.subtitle"));
        y += roles_header_height + 8.0;
        if roles_unavailable {
            section = section.child(
                AxNode::new(
                    "settings-default-roles-unavailable",
                    AxRole::StaticText,
                    t("settings.roles.title"),
                    AxRect::new(roles_x, y, width, STATUS_HEIGHT),
                )
                .value(settings_default_unavailable_note()),
            );
            y += STATUS_HEIGHT + 8.0;
        }
        let mut card = AxNode::new(
            "settings-default-roles-card",
            AxRole::Group,
            t("settings.roles.title"),
            AxRect::new(roles_x, y, width, roles_card_height),
        );
        let mut row_y = y + CARD_PAD;
        for role in SettingsRole::ALL {
            let identifier = settings_role_trigger_identifier(role);
            let enabled = self.settings_role_menu_enabled(role);
            let focused = self
                .settings_action_focus
                .get(&identifier)
                .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
            let mut trigger = AxNode::new(
                identifier,
                AxRole::Button,
                role.label(),
                AxRect::new(
                    menu_x,
                    row_y + (SETTINGS_ROLE_ROW_HEIGHT - CONTROL_ROW) / 2.0,
                    SETTINGS_ROLE_MENU_WIDTH,
                    CONTROL_ROW,
                ),
            )
            .value(self.settings_role_value_label(role))
            .description(settings_role_description_label(role))
            .enabled(enabled)
            .focused(focused);
            if enabled {
                trigger = trigger.action(AxAction::Press);
            }
            card = card
                .child(
                    AxNode::new(
                        format!("settings-role-label-{}", role.wire_name()),
                        AxRole::StaticText,
                        role.label(),
                        AxRect::new(
                            roles_x + CARD_PAD,
                            row_y,
                            SETTINGS_ROLE_LABEL_WIDTH,
                            SETTINGS_ROLE_ROW_HEIGHT,
                        ),
                    )
                    .value(settings_role_description_label(role)),
                )
                .child(trigger);
            if matches!(self.open_menu, Some(MenuKind::SettingsRole(open)) if open == role) {
                let menu_y = row_y + (SETTINGS_ROLE_ROW_HEIGHT + CONTROL_ROW) / 2.0 + ANCHOR_GAP_Y;
                card = card.child(self.settings_role_menu_ax(role, menu_x, menu_y));
            }
            row_y += SETTINGS_ROLE_ROW_HEIGHT;
        }
        page = page.child(section.child(card));
        y += roles_card_height + 8.0;

        page = page.child(AxNode::new(
            "settings-providers-heading",
            AxRole::StaticText,
            t("settings.providers.section_providers"),
            AxRect::new(frame.x + SETTINGS_CONTENT_PAD, y, width, STATUS_HEIGHT),
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
            // 流程详情（OAuth 等待 / 瞬态反馈 / 错误 / endpoint）迁入
            // Credentials 区；文案与 render 同源。
            let mut detail_values = Vec::new();
            if let (crate::projection::ProviderAuthState::Connecting, Some(wait)) =
                (&provider.auth, wait)
            {
                detail_values.push(
                    t("settings.providers.authorize_at").replace("{}", &wait.verification_url),
                );
                if let Some(code) = &wait.user_code {
                    detail_values.push(t("settings.providers.oauth_code").replace("{}", code));
                }
                if let Some(expires) = &wait.expires_at {
                    detail_values
                        .push(t("settings.providers.oauth_expires").replace("{}", expires));
                }
            }
            if let Some(note) = state.auth_notes.get(&provider.provider_id) {
                detail_values.push(note.clone());
            }
            if let Some(message) = auth_error {
                detail_values.push(t("settings.providers.connection_error").replace("{}", message));
            }
            if catalog_error {
                detail_values.push(provider.catalog_label());
            }
            if endpoint_visible {
                detail_values.push(
                    t("settings.providers.endpoint_row").replace("{}", &provider.endpoint_label),
                );
            }
            // 有效展开态（与 render 同源）：显式展开 ∨ 流程态。
            let expanded = self.settings_provider_card_expanded(
                &provider.provider_id,
                editor_open,
                wait.is_some(),
                remove_confirm,
            );
            // 展开区高度估值（与 render 行高同源常数）：Proxy 行（gate 同
            // render）→ Manage models 行 → Credentials 区（头部 + 凭证行 /
            // 空态 + 详情行 + 编辑器 + 动作按钮）→ Usage 行。
            let proxy_visible = self.projection.settings_general.proxy_url.is_some();
            let credential_rows = provider.credentials.len().max(1) as f32;
            let control_rows = editor_row as u8 as f32 + (!row_actions.is_empty()) as u8 as f32;
            let credentials_height = CARD_PAD
                + SETTINGS_PROVIDER_CREDENTIALS_HEADER_HEIGHT
                + credential_rows * SETTINGS_PROVIDER_CREDENTIAL_ROW_HEIGHT
                + detail_values.len() as f32 * TEXT_ROW
                + control_rows * CONTROL_ROW
                + (credential_rows + detail_values.len() as f32 + control_rows).max(0.0) * CARD_GAP
                + CARD_PAD;
            let expanded_height = usize::from(proxy_visible) as f32
                * SETTINGS_PROVIDER_ROW_HEIGHT
                + SETTINGS_PROVIDER_ROW_HEIGHT
                + credentials_height
                + SETTINGS_PROVIDER_ROW_HEIGHT;
            let card_height = PROVIDER_OVERVIEW_HEIGHT
                + if expanded { expanded_height } else { 0.0 };
            let card_x = frame.x + SETTINGS_CONTENT_PAD;
            let model_count = self
                .projection
                .models
                .iter()
                .filter(|model| model.provider_id == provider.provider_id)
                .count();
            let catalog_summary = provider_catalog_overview_label(provider, model_count);
            let auth_methods = provider.auth_methods_label();
            let auth_methods = if auth_methods.is_empty() {
                t("settings.providers.no_auth_method").to_string()
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
                    t("settings.providers.ax_connection"),
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
                    t("settings.providers.ax_catalog"),
                    AxRect::new(card_x + 440.0, y, 132.0, PROVIDER_OVERVIEW_HEIGHT),
                )
                .value(catalog_summary),
            );

            // 展开 / 折叠 chevron（卡头最右 36×36 图标位，OPT-4a 可见
            // 字形 20px；本地视图动作，不受写总闸限制，Press 与 render /
            // 键盘同入口）。
            let expand_identifier = settings_provider_expand_identifier(&provider.provider_id);
            let expand_focused = self
                .settings_action_focus
                .get(&expand_identifier)
                .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
            let chevron = AxNode::new(
                expand_identifier,
                AxRole::Button,
                if expanded {
                    t("settings.providers.collapse_tooltip")
                } else {
                    t("settings.providers.expand_tooltip")
                },
                AxRect::new(
                    card_x + width - 8.0 - metrics::ICON_BUTTON_SIZE,
                    y + (PROVIDER_OVERVIEW_HEIGHT - metrics::ICON_BUTTON_SIZE) / 2.0,
                    metrics::ICON_BUTTON_SIZE,
                    metrics::ICON_BUTTON_SIZE,
                ),
            )
            .value(if expanded {
                t("settings.providers.expanded")
            } else {
                t("settings.providers.collapsed")
            })
            .enabled(true)
            .focused(expand_focused)
            .action(AxAction::Press);
            card = card.child(chevron);

            if expanded {
                // ── Proxy 行（gate 与 render 同源：仅全局 proxy_url 已配置）──
                let mut row_y = y + PROVIDER_OVERVIEW_HEIGHT;
                if proxy_visible {
                    // 行文本（标题 + 副标题）：与 render 同一 i18n 词条，
                    // 钉住「标题实际非省略」的文案面（ADR-056 D4）。
                    card = card.child(
                        AxNode::new(
                            dynamic_identifier(
                                "settings-provider-proxy-text",
                                &provider.provider_id,
                            ),
                            AxRole::StaticText,
                            t("settings.providers.proxy_title"),
                            AxRect::new(
                                card_x + 8.0,
                                row_y,
                                width - 16.0,
                                SETTINGS_PROVIDER_ROW_HEIGHT,
                            ),
                        )
                        .value(t("settings.providers.proxy_subtitle")),
                    );
                    let identifier = settings_use_proxy_identifier(&provider.provider_id);
                    let enabled = writes;
                    let focused = self
                        .settings_action_focus
                        .get(&identifier)
                        .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                    let value = if provider.use_proxy {
                        t("settings.providers.switch_on")
                    } else {
                        t("settings.providers.switch_off")
                    };
                    let mut toggle = AxNode::new(
                        identifier,
                        AxRole::Button,
                        t("settings.providers.ax_use_proxy"),
                        AxRect::new(
                            card_x + width - 8.0 - SWITCH_GROUP_WIDTH,
                            row_y + (SETTINGS_PROVIDER_ROW_HEIGHT - CONTROL_ROW) / 2.0,
                            SWITCH_GROUP_WIDTH,
                            CONTROL_ROW,
                        ),
                    )
                    .value(value)
                    .selected(provider.use_proxy)
                    .enabled(enabled)
                    .focused(focused);
                    if enabled {
                        toggle = toggle.action(AxAction::Press);
                    }
                    card = card.child(toggle);
                    row_y += SETTINGS_PROVIDER_ROW_HEIGHT;
                }

                // ── Manage models 行（触发器 gate 与 render 同源）──
                card = card.child(
                    AxNode::new(
                        dynamic_identifier(
                            "settings-provider-manage-text",
                            &provider.provider_id,
                        ),
                        AxRole::StaticText,
                        t("settings.providers.manage_models"),
                        AxRect::new(
                            card_x + 8.0,
                            row_y,
                            width - 16.0,
                            SETTINGS_PROVIDER_ROW_HEIGHT,
                        ),
                    )
                    .value(t("settings.providers.manage_models_tooltip")),
                );
                let manage_identifier = settings_manage_models_identifier(&provider.provider_id);
                let manage_enabled = self.settings_manage_models_enabled(provider);
                let manage_focused = self
                    .settings_action_focus
                    .get(&manage_identifier)
                    .is_some_and(|focus| self.open_menu.is_none() && focus.is_focused(window));
                let manage_button_x = card_x + width - 8.0 - 110.0;
                let mut manage = AxNode::new(
                    manage_identifier,
                    AxRole::Button,
                    t("settings.providers.manage_models"),
                    AxRect::new(
                        manage_button_x,
                        row_y + (SETTINGS_PROVIDER_ROW_HEIGHT - CONTROL_ROW) / 2.0,
                        110.0,
                        CONTROL_ROW,
                    ),
                )
                .description(t("settings.providers.manage_models_tooltip"))
                .enabled(manage_enabled)
                .focused(manage_focused);
                if manage_enabled {
                    manage = manage.action(AxAction::Press);
                }
                card = card.child(manage);
                if matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &provider.provider_id)
                {
                    let menu_y = row_y
                        + (SETTINGS_PROVIDER_ROW_HEIGHT + CONTROL_ROW) / 2.0
                        + ANCHOR_GAP_Y;
                    card = card.child(self.settings_models_menu_ax(
                        &provider.provider_id,
                        manage_button_x,
                        menu_y,
                        window,
                    ));
                }
                row_y += SETTINGS_PROVIDER_ROW_HEIGHT;

                // ── Credentials 区（组 + 头部 + 凭证行 / 空态 + 流程详情 +
                //    编辑器 + 动作按钮；几何为 render 行高估值）──
                let mut cred_y = row_y + CARD_PAD;
                let mut credentials = AxNode::new(
                    dynamic_identifier("settings-provider-credentials", &provider.provider_id),
                    AxRole::Group,
                    t("settings.providers.credentials_title"),
                    AxRect::new(
                        card_x,
                        row_y,
                        width,
                        credentials_height,
                    ),
                )
                .child(
                    AxNode::new(
                        dynamic_identifier(
                            "settings-provider-credentials-header",
                            &provider.provider_id,
                        ),
                        AxRole::StaticText,
                        t("settings.providers.credentials_title"),
                        AxRect::new(
                            card_x + 8.0,
                            cred_y,
                            width - 16.0,
                            SETTINGS_PROVIDER_CREDENTIALS_HEADER_HEIGHT,
                        ),
                    )
                    .value(t("settings.providers.credentials_subtitle")),
                );
                cred_y += SETTINGS_PROVIDER_CREDENTIALS_HEADER_HEIGHT + CARD_GAP;
                if provider.credentials.is_empty() {
                    // 空列表诚实空态：不发布假凭证行。
                    credentials = credentials.child(
                        AxNode::new(
                            dynamic_identifier(
                                "settings-provider-credentials-empty",
                                &provider.provider_id,
                            ),
                            AxRole::StaticText,
                            t("settings.providers.credentials_title"),
                            AxRect::new(
                                card_x + 8.0,
                                cred_y,
                                width - 16.0,
                                SETTINGS_PROVIDER_CREDENTIAL_ROW_HEIGHT,
                            ),
                        )
                        .value(t("settings.providers.credentials_empty")),
                    );
                    cred_y += SETTINGS_PROVIDER_CREDENTIAL_ROW_HEIGHT + CARD_GAP;
                } else {
                    for (ix, credential) in provider.credentials.iter().enumerate() {
                        let status =
                            provider_credential_status_label(credential.expired);
                        credentials = credentials.child(
                            AxNode::new(
                                dynamic_identifier(
                                    &format!("settings-provider-credential-{ix}"),
                                    &provider.provider_id,
                                ),
                                AxRole::StaticText,
                                provider_credential_kind_label(&credential.kind),
                                AxRect::new(
                                    card_x + 8.0,
                                    cred_y,
                                    width - 16.0,
                                    SETTINGS_PROVIDER_CREDENTIAL_ROW_HEIGHT,
                                ),
                            )
                            .value(format!(
                                "{} · {}",
                                credential.masked_credential, status
                            )),
                        );
                        cred_y += SETTINGS_PROVIDER_CREDENTIAL_ROW_HEIGHT + CARD_GAP;
                    }
                }
                if !detail_values.is_empty() {
                    let text_height = detail_values.len() as f32 * TEXT_ROW;
                    credentials = credentials.child(
                        AxNode::new(
                            dynamic_identifier("settings-provider-details", &provider.provider_id),
                            AxRole::StaticText,
                            t("settings.providers.ax_details"),
                            AxRect::new(card_x + 8.0, cred_y, width - 16.0, text_height),
                        )
                        .value(detail_values.join(" · ")),
                    );
                    cred_y += text_height + CARD_GAP;
                }
                if editor_row {
                    if let Some(input) = self.settings_api_key_inputs.get(&provider.provider_id) {
                        let masked = input.read(cx).secure_mask().unwrap_or_default();
                        let mut input_node = AxNode::new(
                            settings_api_key_input_identifier(&provider.provider_id),
                            AxRole::TextArea,
                            t("settings.providers.ax_api_key"),
                            AxRect::new(
                                card_x + 8.0,
                                cred_y,
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
                        credentials = credentials.child(input_node);
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
                            .is_some_and(|focus| {
                                self.open_menu.is_none() && focus.is_focused(window)
                            });
                        let mut button = AxNode::new(
                            identifier,
                            AxRole::Button,
                            action.label(),
                            AxRect::new(editor_button_x, cred_y, 110.0, CONTROL_ROW),
                        )
                        .enabled(enabled)
                        .focused(focused);
                        if enabled {
                            button = button.action(AxAction::Press);
                        }
                        credentials = credentials.child(button);
                        editor_button_x += 114.0;
                    }
                    cred_y += CONTROL_ROW + CARD_GAP;
                }
                if !row_actions.is_empty() {
                    let mut detail_button_x = card_x + 8.0;
                    for action in &row_actions {
                        let identifier = action.identifier(&provider.provider_id);
                        let enabled =
                            self.settings_action_enabled(*action, &provider.provider_id, writes, cx);
                        let focused = self
                            .settings_action_focus
                            .get(&identifier)
                            .is_some_and(|focus| {
                                self.open_menu.is_none() && focus.is_focused(window)
                            });
                        let mut button = AxNode::new(
                            identifier,
                            AxRole::Button,
                            action.label(),
                            AxRect::new(detail_button_x, cred_y, 110.0, CONTROL_ROW),
                        )
                        .enabled(enabled)
                        .focused(focused);
                        if enabled {
                            button = button.action(AxAction::Press);
                        }
                        credentials = credentials.child(button);
                        detail_button_x += 114.0;
                    }
                }
                card = card.child(credentials);

                // ── Usage 行（恒为诚实空态：固定槽位 + Usage unavailable，
                //    不发布任何数字 / 百分比 / 填充值）──
                let usage_y = row_y + credentials_height;
                card = card.child(
                    AxNode::new(
                        dynamic_identifier("settings-provider-usage", &provider.provider_id),
                        AxRole::StaticText,
                        t("settings.providers.usage_title"),
                        AxRect::new(
                            card_x + 8.0,
                            usage_y,
                            width - 16.0,
                            SETTINGS_PROVIDER_ROW_HEIGHT,
                        ),
                    )
                    .value(t("settings.providers.usage_unavailable")),
                );
            }
            page = page.child(card);
            y += card_height + 8.0;
        }
        page
    }

    /// 角色菜单展开态 AX（与 render 浮层同源）：清除行 + 已连接 provider
    /// 分组的启用模型；空候选保留清除行并发布诚实说明，不编造模型。
    /// 面板高度按内容估值并在 MENU_MAX_HEIGHT 内裁剪，与 render 自滚一致。
    fn settings_role_menu_ax(&self, role: SettingsRole, menu_x: f32, menu_y: f32) -> AxNode {
        let candidates = settings_role_candidates(
            &self.projection.models,
            &self.projection.settings_providers.providers,
        );
        let current = self.projection.settings_providers.role_value(role);
        let highlight = self.menu_highlight_effective(self.menu_selected_index());
        let content_height = if candidates.is_empty() {
            metrics::MENU_PADDING * 2.0 + metrics::MENU_ROW_HEIGHT + SETTINGS_ROLE_MENU_EMPTY_HEIGHT
        } else {
            let entry_count: usize = candidates.iter().map(|(_, models)| models.len()).sum();
            metrics::MENU_PADDING * 2.0
                + candidates.len() as f32 * SETTINGS_ROLE_MENU_GROUP_HEADER_HEIGHT
                + (entry_count + 1) as f32 * metrics::MENU_ROW_HEIGHT
        };
        let menu_height = content_height.min(MENU_MAX_HEIGHT);
        let mut menu = AxNode::new(
            format!("settings-role-menu-{}", role.wire_name()),
            AxRole::Group,
            t("settings.roles.ax_menu"),
            AxRect::new(menu_x, menu_y, SETTINGS_ROLE_MENU_WIDTH, menu_height),
        );
        let mut item_y = menu_y + metrics::MENU_PADDING;
        let menu_bottom = menu_y + menu_height;
        let can_write = self.settings_role_menu_enabled(role);
        let mut clear = AxNode::new(
            settings_role_clear_identifier(role),
            AxRole::Button,
            t("settings.roles.clear"),
            AxRect::new(
                menu_x,
                item_y,
                SETTINGS_ROLE_MENU_WIDTH,
                metrics::MENU_ROW_HEIGHT,
            ),
        )
        .selected(current.is_none())
        .enabled(can_write)
        .focused(0 == highlight);
        if can_write {
            clear = clear.action(AxAction::Press);
        }
        menu = menu.child(clear);
        item_y += metrics::MENU_ROW_HEIGHT;
        if candidates.is_empty() {
            return menu.child(
                AxNode::new(
                    format!("settings-role-menu-empty-{}", role.wire_name()),
                    AxRole::StaticText,
                    t("settings.roles.empty_title"),
                    AxRect::new(
                        menu_x,
                        item_y,
                        SETTINGS_ROLE_MENU_WIDTH,
                        SETTINGS_ROLE_MENU_EMPTY_HEIGHT,
                    ),
                )
                .value(t("settings.roles.empty_hint")),
            );
        }
        let mut item_ix = 1;
        for (provider_id, models) in candidates {
            // 组头显示名取 provider 权威清单（与 render 同源回落）。
            let display_name = self
                .projection
                .settings_providers
                .providers
                .iter()
                .find(|entry| entry.provider_id == provider_id)
                .map(|entry| entry.display_name.clone())
                .unwrap_or_else(|| provider_id.to_string());
            if item_y < menu_bottom {
                menu = menu.child(AxNode::new(
                    dynamic_identifier(
                        &format!("settings-role-menu-group-{}", role.wire_name()),
                        &provider_id,
                    ),
                    AxRole::StaticText,
                    display_name,
                    AxRect::new(
                        menu_x,
                        item_y,
                        SETTINGS_ROLE_MENU_WIDTH,
                        SETTINGS_ROLE_MENU_GROUP_HEADER_HEIGHT,
                    ),
                ));
            }
            item_y += SETTINGS_ROLE_MENU_GROUP_HEADER_HEIGHT;
            for model in models {
                let selected = current.is_some_and(|(provider, id)| {
                    provider == &model.provider_id && id == &model.id
                });
                if item_y < menu_bottom {
                    let mut item = AxNode::new(
                        settings_role_item_identifier(role, &model.provider_id, &model.id),
                        AxRole::Button,
                        model.display_name.clone(),
                        AxRect::new(
                            menu_x,
                            item_y,
                            SETTINGS_ROLE_MENU_WIDTH,
                            metrics::MENU_ROW_HEIGHT,
                        ),
                    )
                    .value(format!("{} / {}", model.provider_id, model.id))
                    .enabled(can_write)
                    .selected(selected)
                    .focused(item_ix == highlight);
                    if can_write {
                        item = item.action(AxAction::Press);
                    }
                    menu = menu.child(item);
                }
                item_ix += 1;
                item_y += metrics::MENU_ROW_HEIGHT;
            }
        }
        menu
    }

    /// 「Manage models」弹层 AX（与 render 浮层同源，OPT-3a / ADR-055
    /// D2/D3）：Enable all / Disable all + 每模型 Switch（checked 进 value
    /// 与 selected）；空目录只发布诚实空态 + Refresh（复用页级刷新）。
    /// 面板高度按内容估值并在弹层上限内裁剪，与 render 自滚一致。
    fn settings_models_menu_ax(
        &self,
        provider_id: &str,
        menu_x: f32,
        menu_y: f32,
        window: &Window,
    ) -> AxNode {
        let models: Vec<&ModelEntry> = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .collect();
        let pending = self
            .projection
            .settings_providers
            .model_write_pending
            .as_ref()
            .is_some_and(|pending| pending.targets(provider_id));
        let writes = self.settings_writes_enabled();
        let content_height = if models.is_empty() {
            SETTINGS_MODELS_MENU_EMPTY_HEIGHT
        } else {
            SETTINGS_MODELS_MENU_HEADER_HEIGHT
                + models.len() as f32 * SETTINGS_MODELS_MENU_ROW_HEIGHT
        };
        let menu_height =
            (metrics::MENU_PADDING * 2.0 + content_height).min(SETTINGS_MODELS_MENU_MAX_HEIGHT);
        let mut menu = AxNode::new(
            settings_models_menu_identifier(provider_id),
            AxRole::Group,
            t("settings.providers.models_title"),
            AxRect::new(menu_x, menu_y, SETTINGS_MODELS_MENU_WIDTH, menu_height),
        );
        // 头行右侧 Enable all / Disable all（render 从右向左：Disable all
        // 最右；两者 100px 槽 + 4px 间隙估值）。
        let header_y = menu_y + metrics::MENU_PADDING;
        for (identifier, label, button_x) in [
            (
                settings_models_disable_all_identifier(provider_id),
                t("settings.providers.models_disable_all"),
                menu_x + SETTINGS_MODELS_MENU_WIDTH - metrics::MENU_PADDING - 100.0,
            ),
            (
                settings_models_enable_all_identifier(provider_id),
                t("settings.providers.models_enable_all"),
                menu_x + SETTINGS_MODELS_MENU_WIDTH - metrics::MENU_PADDING - 204.0,
            ),
        ] {
            let enabled = writes && !pending && !models.is_empty();
            let focused = self
                .settings_action_focus
                .get(&identifier)
                .is_some_and(|focus| focus.is_focused(window));
            let mut node = AxNode::new(
                identifier,
                AxRole::Button,
                label,
                AxRect::new(
                    button_x,
                    header_y,
                    100.0,
                    SETTINGS_MODELS_MENU_HEADER_HEIGHT,
                ),
            )
            .enabled(enabled)
            .focused(focused);
            if enabled {
                node = node.action(AxAction::Press);
            }
            menu = menu.child(node);
        }
        if models.is_empty() {
            let refresh_identifier = settings_models_refresh_identifier(provider_id);
            let refresh_focused = self
                .settings_action_focus
                .get(&refresh_identifier)
                .is_some_and(|focus| focus.is_focused(window));
            let mut refresh = AxNode::new(
                refresh_identifier,
                AxRole::Button,
                t("settings.providers.models_refresh"),
                AxRect::new(
                    menu_x + metrics::MENU_PADDING,
                    header_y + SETTINGS_MODELS_MENU_HEADER_HEIGHT + 36.0,
                    110.0,
                    28.0,
                ),
            )
            .enabled(writes)
            .focused(refresh_focused);
            if writes {
                refresh = refresh.action(AxAction::Press);
            }
            return menu
                .child(
                    AxNode::new(
                        format!("settings-models-empty-{provider_id}"),
                        AxRole::StaticText,
                        t("settings.providers.models_empty_title"),
                        AxRect::new(
                            menu_x + metrics::MENU_PADDING,
                            header_y + SETTINGS_MODELS_MENU_HEADER_HEIGHT,
                            SETTINGS_MODELS_MENU_WIDTH - metrics::MENU_PADDING * 2.0,
                            36.0,
                        ),
                    )
                    .value(t("settings.providers.models_empty_hint")),
                )
                .child(refresh);
        }
        let mut item_y = menu_y + metrics::MENU_PADDING + SETTINGS_MODELS_MENU_HEADER_HEIGHT;
        let menu_bottom = menu_y + menu_height;
        for model in models {
            if item_y >= menu_bottom {
                break;
            }
            let identifier = settings_model_switch_identifier(provider_id, &model.id);
            let enabled = writes && !pending;
            let focused = self
                .settings_action_focus
                .get(&identifier)
                .is_some_and(|focus| focus.is_focused(window));
            let value = if model.enabled {
                t("settings.providers.switch_on")
            } else {
                t("settings.providers.switch_off")
            };
            let mut item = AxNode::new(
                identifier,
                AxRole::Button,
                model.display_name.clone(),
                AxRect::new(
                    menu_x + metrics::MENU_PADDING,
                    item_y,
                    SETTINGS_MODELS_MENU_WIDTH - metrics::MENU_PADDING * 2.0,
                    SETTINGS_MODELS_MENU_ROW_HEIGHT,
                ),
            )
            .value(value)
            .description(format!("{}/{}", model.provider_id, model.id))
            .selected(model.enabled)
            .enabled(enabled)
            .focused(focused);
            if enabled {
                item = item.action(AxAction::Press);
            }
            menu = menu.child(item);
            item_y += SETTINGS_MODELS_MENU_ROW_HEIGHT;
        }
        menu
    }
}
