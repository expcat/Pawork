//! Provider 页与浮层 AX：语义取权威投影，几何取 GPUI 实际布局。

use gpui::{App, Focusable, Window};

use super::{dynamic_identifier, AxAction, AxNode, AxRect, AxRole};
use crate::projection::{
    ConnectionState, ProviderAuthState, ProviderCatalogState, ProviderStatusLabels,
};
use crate::ui::i18n::t;
use crate::ui::settings::{
    provider_catalog_overview_label, provider_credential_kind_label,
    provider_credential_status_label, provider_status_lines, quota_identifier,
    settings_account_action_identifier, settings_account_rename_identifier,
    settings_account_rename_input_identifier, settings_api_key_input_identifier,
    settings_credential_row_identifier, settings_default_unavailable_note,
    settings_manage_models_identifier, settings_model_effort_chip_identifier,
    settings_model_effort_default_identifier, settings_model_effort_reset_identifier,
    settings_model_switch_identifier, settings_models_disable_all_identifier,
    settings_models_enable_all_identifier, settings_models_menu_identifier,
    settings_models_refresh_identifier, settings_provider_expand_identifier,
    settings_role_candidates, settings_role_clear_identifier, settings_role_description_label,
    settings_role_item_identifier, settings_role_trigger_identifier, settings_use_proxy_identifier,
    SettingsAuthAction, SettingsRole,
};
use crate::ui::{AppView, MenuKind};

/// 保留可见后代（例如越过触发器的浮层），移除未渲染与完全离屏的控件。
fn visible(mut node: AxNode) -> AxNode {
    node.children = node
        .children
        .into_iter()
        .map(visible)
        .filter(|child| {
            (child.bounds.width > 0.0 && child.bounds.height > 0.0) || !child.children.is_empty()
        })
        .collect();
    node
}

impl AppView {
    pub(crate) fn settings_providers_page_ax(
        &self,
        window: &Window,
        cx: &App,
        frame: AxRect,
    ) -> AxNode {
        let state = &self.projection.settings_providers;
        let writes = self.settings_writes_enabled();
        let text = |id: String, name: &str, value: String| {
            let bounds = self.settings_element_bounds(&id);
            AxNode::new(id, AxRole::StaticText, name, bounds).value(value)
        };
        let group = |id: String, name: &str| {
            let bounds = self.settings_element_bounds(&id);
            AxNode::new(id, AxRole::Group, name, bounds)
        };
        let button = |id: String, name: &str, enabled: bool| {
            let bounds = self.settings_element_bounds(&id);
            let focused = self.open_menu.is_none()
                && self
                    .settings_action_focus
                    .get(&id)
                    .is_some_and(|focus| focus.is_focused(window));
            let node = AxNode::new(id, AxRole::Button, name, bounds)
                .enabled(enabled)
                .focused(focused);
            if enabled {
                node.action(AxAction::Press)
            } else {
                node
            }
        };
        let connected = matches!(
            self.projection.connection,
            ConnectionState::Connected { .. }
        );
        let mut refresh = AxNode::new(
            "settings-refresh",
            AxRole::Button,
            t("settings.refresh"),
            self.settings_element_bounds("settings-refresh"),
        )
        .enabled(connected)
        .focused(self.open_menu.is_none() && self.settings_refresh_focus.is_focused(window));
        if connected {
            refresh = refresh.action(AxAction::Press);
        }
        let mut page = AxNode::new(
            "settings-page",
            AxRole::Group,
            t("settings.providers.title"),
            frame,
        )
        .child(text(
            "settings-page-title".into(),
            t("settings.providers.title"),
            t("settings.providers.subtitle").into(),
        ))
        .child(refresh);
        for (kind, label) in provider_status_lines(state) {
            page = page.child(text(
                format!("settings-status-{kind}"),
                t("settings.providers.ax_status"),
                label,
            ));
        }
        let mut section = group("settings-default-roles".into(), t("settings.roles.title"))
            .value(t("settings.roles.subtitle"))
            .child(text(
                "settings-default-roles-title".into(),
                t("settings.roles.title"),
                t("settings.roles.subtitle").into(),
            ));
        if self.projection.default_model_unavailable() {
            section = section.child(text(
                "settings-default-roles-unavailable".into(),
                t("settings.roles.title"),
                settings_default_unavailable_note().into(),
            ));
        }
        let mut roles = group(
            "settings-default-roles-card".into(),
            t("settings.roles.title"),
        );
        for role in SettingsRole::ALL {
            roles = roles
                .child(text(
                    format!("settings-role-label-{}", role.wire_name()),
                    role.label(),
                    settings_role_description_label(role).into(),
                ))
                .child(
                    button(
                        settings_role_trigger_identifier(role),
                        role.label(),
                        self.settings_role_menu_enabled(role),
                    )
                    .value(self.settings_role_value_label(role))
                    .description(settings_role_description_label(role)),
                );
            if matches!(self.open_menu, Some(MenuKind::SettingsRole(open)) if open == role) {
                roles = roles.child(self.settings_role_menu_ax(role));
            }
        }
        page = page.child(text(
            "settings-providers-heading".into(),
            t("settings.providers.section_providers"),
            String::new(),
        ));
        for provider in &self.settings_ordered_providers() {
            let id = &provider.provider_id;
            let wait = state.oauth_waits.get(id);
            let editor = self.settings_api_key_editor_visible(provider);
            let remove = self.settings_remove_confirm.as_deref() == Some(id.as_str());
            let expanded = self.settings_provider_card_expanded(id, editor, wait.is_some(), remove);
            let actions =
                self.settings_provider_auth_actions(provider, editor, remove, wait.is_some());
            let count = self
                .projection
                .models
                .iter()
                .filter(|model| model.provider_id == *id)
                .count();
            let catalog = provider_catalog_overview_label(provider, count);
            let methods = provider.auth_methods_label();
            let methods = if methods.is_empty() {
                t("settings.providers.no_auth_method").to_string()
            } else {
                methods
            };
            let mut card = group(
                dynamic_identifier("settings-provider", id),
                &provider.display_name,
            )
            .value(format!(
                "{} · {} · {}",
                methods,
                provider.auth_label(),
                catalog
            ))
            .child(text(
                dynamic_identifier("settings-provider-name", id),
                &provider.display_name,
                methods,
            ))
            .child(text(
                dynamic_identifier("settings-provider-connection", id),
                t("settings.providers.ax_connection"),
                provider.auth_label(),
            ))
            .child(text(
                dynamic_identifier("settings-provider-catalog", id),
                t("settings.providers.ax_catalog"),
                catalog,
            ))
            .child(
                button(
                    settings_provider_expand_identifier(id),
                    if expanded {
                        t("settings.providers.collapse_tooltip")
                    } else {
                        t("settings.providers.expand_tooltip")
                    },
                    true,
                )
                .value(if expanded {
                    t("settings.providers.expanded")
                } else {
                    t("settings.providers.collapsed")
                }),
            );
            if expanded {
                let mut credentials = group(
                    dynamic_identifier("settings-provider-credentials", id),
                    self.settings_credentials_title(),
                )
                .child(text(
                    dynamic_identifier("settings-provider-credentials-header", id),
                    self.settings_credentials_title(),
                    self.settings_credentials_subtitle().into(),
                ));
                if provider.credentials.is_empty() {
                    credentials = credentials.child(text(
                        dynamic_identifier("settings-provider-credentials-empty", id),
                        self.settings_credentials_title(),
                        t("settings.providers.credentials_empty").into(),
                    ));
                }
                for (ix, credential) in provider.credentials.iter().enumerate() {
                    let name = if credential.display_name.is_empty() {
                        provider_credential_kind_label(&credential.kind)
                    } else {
                        credential.display_name.clone()
                    };
                    credentials = credentials.child(text(
                        settings_credential_row_identifier(id, credential, ix),
                        &name,
                        format!(
                            "{} · {} · {}{}",
                            provider_credential_kind_label(&credential.kind),
                            credential.masked_credential,
                            provider_credential_status_label(credential.expired),
                            if credential.selected {
                                format!(" · {}", t("settings.providers.account_selected"))
                            } else {
                                String::new()
                            }
                        ),
                    ));
                    if self.account_quota_supported(id, credential) {
                        for (quota_id, label) in
                            self.account_quota_labels(id, &credential.credential_id)
                        {
                            credentials = credentials.child(text(
                                quota_id,
                                t("settings.providers.usage_title"),
                                label,
                            ));
                        }
                        credentials = credentials.child(button(
                            quota_identifier(id, &credential.credential_id, "refresh"),
                            t("settings.quota.refresh"),
                            self.account_quota_refresh_enabled(),
                        ));
                    }
                    if self.settings_accounts_supported() && !credential.credential_id.is_empty() {
                        for remove in [false, true] {
                            credentials = credentials.child(button(
                                settings_account_action_identifier(
                                    id,
                                    &credential.credential_id,
                                    remove,
                                ),
                                self.settings_account_action_label(
                                    &settings_account_action_identifier(
                                        id,
                                        &credential.credential_id,
                                        remove,
                                    ),
                                    remove,
                                ),
                                self.settings_account_action_enabled(provider, credential, remove),
                            ));
                        }
                        if self.settings_account_rename_supported() {
                            credentials = credentials.child(button(
                                settings_account_rename_identifier(id, &credential.credential_id),
                                t("settings.providers.account_rename"),
                                writes
                                    && !matches!(provider.auth, ProviderAuthState::Connecting)
                                    && !credential.credential_id.is_empty(),
                            ));
                            if self.settings_account_rename.as_ref().is_some_and(|rename| {
                                rename.provider_id == *id
                                    && rename.credential_id == credential.credential_id
                            }) {
                                if let Some(rename) = &self.settings_account_rename {
                                    let input_id = settings_account_rename_input_identifier(
                                        id,
                                        &credential.credential_id,
                                    );
                                    let mut node = AxNode::new(
                                        input_id.clone(),
                                        AxRole::TextArea,
                                        t("settings.providers.account_alias"),
                                        self.settings_element_bounds(&input_id),
                                    )
                                    .value(rename.input.read(cx).text())
                                    .enabled(writes)
                                    .focused(
                                        rename.input.read(cx).focus_handle(cx).is_focused(window),
                                    );
                                    if writes {
                                        node =
                                            node.action(AxAction::Focus).action(AxAction::SetValue);
                                    }
                                    credentials = credentials.child(node);
                                }
                            }
                        }
                    }
                }
                if let Some(remove_id) = &self.settings_account_remove_confirm {
                    if provider.credentials.iter().any(|c| {
                        settings_account_action_identifier(id, &c.credential_id, true) == *remove_id
                    }) {
                        credentials = credentials.child(button(
                            format!("{remove_id}-keep"),
                            t("settings.providers.action_keep"),
                            true,
                        ));
                    }
                }
                if id == "opencode-go" && self.settings_quota_supported() {
                    credentials = credentials.child(text(
                        quota_identifier(id, "", "scope"),
                        t("settings.quota.scope"),
                        String::new(),
                    ));
                }
                let mut details = Vec::new();
                if let (ProviderAuthState::Connecting, Some(wait)) = (&provider.auth, wait) {
                    for action in [
                        SettingsAuthAction::OpenOauth,
                        SettingsAuthAction::CopyOauthUrl,
                        SettingsAuthAction::CopyOauthCode,
                    ] {
                        if action == SettingsAuthAction::CopyOauthCode && wait.user_code.is_none() {
                            continue;
                        }
                        credentials = credentials.child(button(
                            action.identifier(id),
                            if self.settings_copied_auth.as_ref().is_some_and(
                                |(provider, copied)| provider == id && *copied == action,
                            ) {
                                t("settings.providers.copied")
                            } else {
                                action.label()
                            },
                            self.settings_action_enabled(action, id, writes, cx),
                        ));
                    }
                    details.push(
                        t("settings.providers.authorize_at").replace("{}", &wait.verification_url),
                    );
                    if let Some(code) = &wait.user_code {
                        details.push(t("settings.providers.oauth_code").replace("{}", code));
                    }
                    if let Some(expires) = &wait.expires_at {
                        details.push(t("settings.providers.oauth_expires").replace("{}", expires));
                    }
                }
                if let Some(note) = state.auth_notes.get(id) {
                    details.push(note.clone());
                }
                if let ProviderAuthState::Error { message } = &provider.auth {
                    details.push(t("settings.providers.connection_error").replace("{}", message));
                }
                if matches!(
                    (&provider.auth, &provider.catalog),
                    (
                        ProviderAuthState::Connected { .. },
                        ProviderCatalogState::Unavailable { .. }
                    )
                ) {
                    details.push(provider.catalog_label());
                }
                if editor || wait.is_some() || remove {
                    details.push(
                        t("settings.providers.endpoint_row")
                            .replace("{}", &provider.endpoint_label),
                    );
                }
                if !details.is_empty() {
                    let details_id = dynamic_identifier("settings-provider-details", id);
                    let focused = self
                        .settings_auth_details
                        .get(id)
                        .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window));
                    credentials = credentials.child(
                        AxNode::new(
                            details_id.clone(),
                            AxRole::TextArea,
                            t("settings.providers.ax_details"),
                            self.settings_element_bounds(&details_id),
                        )
                        .value(details.join("\n"))
                        .focused(focused)
                        .action(AxAction::Focus),
                    );
                }
                if editor {
                    if let Some(input) = self.settings_api_key_inputs.get(id) {
                        let input_id = settings_api_key_input_identifier(id);
                        let mut node = AxNode::new(
                            input_id.clone(),
                            AxRole::TextArea,
                            t("settings.providers.ax_api_key"),
                            self.settings_element_bounds(&input_id),
                        )
                        .value(input.read(cx).secure_mask().unwrap_or_default())
                        .enabled(writes)
                        .focused(
                            self.open_menu.is_none()
                                && input.read(cx).focus_handle(cx).is_focused(window),
                        );
                        if writes {
                            node = node.action(AxAction::Focus).action(AxAction::SetValue);
                        }
                        credentials = credentials.child(node);
                    }
                }
                for action in actions {
                    if matches!(
                        action,
                        SettingsAuthAction::VerifyApiKey | SettingsAuthAction::CancelApiKeyInput
                    ) && (!editor || !self.settings_api_key_inputs.contains_key(id))
                    {
                        continue;
                    }
                    credentials = credentials.child(button(
                        action.identifier(id),
                        self.settings_auth_action_label(action),
                        self.settings_action_enabled(action, id, writes, cx),
                    ));
                }
                if id == "opencode-go" && self.settings_quota_supported() {
                    credentials = credentials.child(
                        button(
                            quota_identifier(id, "", "mode"),
                            t("settings.quota.auto"),
                            self.account_mode_enabled(provider),
                        )
                        .selected(
                            provider.selection_mode
                                == pawork_client::ProviderAccountSelectionMode::WhenExhausted,
                        )
                        .value(t(
                            if provider.selection_mode
                                == pawork_client::ProviderAccountSelectionMode::WhenExhausted
                            {
                                "settings.providers.switch_on"
                            } else {
                                "settings.providers.switch_off"
                            },
                        )),
                    );
                }
                card = card.child(credentials);
                if self.projection.settings_general.proxy_url.is_some() {
                    card = card
                        .child(text(
                            dynamic_identifier("settings-provider-proxy-text", id),
                            t("settings.providers.proxy_title"),
                            t("settings.providers.proxy_subtitle").into(),
                        ))
                        .child(
                            button(
                                settings_use_proxy_identifier(id),
                                t("settings.providers.ax_use_proxy"),
                                writes,
                            )
                            .selected(provider.use_proxy)
                            .value(if provider.use_proxy {
                                t("settings.providers.switch_on")
                            } else {
                                t("settings.providers.switch_off")
                            }),
                        );
                }
                card = card
                    .child(text(
                        dynamic_identifier("settings-provider-manage-text", id),
                        t("settings.providers.manage_models"),
                        t("settings.providers.catalog_scope").into(),
                    ))
                    .child(
                        button(
                            settings_manage_models_identifier(id),
                            t("settings.providers.manage_models"),
                            self.settings_manage_models_enabled(provider),
                        )
                        .description(t("settings.providers.manage_models_tooltip")),
                    );
                if matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == id)
                {
                    card = card.child(self.settings_models_menu_ax(id, window));
                }
                card = card.child(text(
                    dynamic_identifier("settings-provider-usage", id),
                    t("settings.providers.usage_title"),
                    t("settings.providers.usage_unavailable").into(),
                ));
            }
            page = page.child(card);
        }
        visible(page.child(section.child(roles)))
    }

    fn settings_role_menu_ax(&self, role: SettingsRole) -> AxNode {
        let menu_id = format!("settings-role-menu-{}", role.wire_name());
        let bounds = |id: &str| self.settings_menu_element_bounds(id, &menu_id);
        let candidates = settings_role_candidates(
            &self.projection.models,
            &self.projection.settings_providers.providers,
        );
        let current = self.projection.settings_providers.role_value(role);
        let highlight = self.menu_highlight_effective(self.menu_selected_index());
        let enabled = self.settings_role_menu_enabled(role);
        let mut menu = AxNode::new(
            menu_id.clone(),
            AxRole::Group,
            t("settings.roles.ax_menu"),
            bounds(&menu_id),
        );
        let clear_id = settings_role_clear_identifier(role);
        let mut clear = AxNode::new(
            clear_id.clone(),
            AxRole::Button,
            t("settings.roles.clear"),
            bounds(&clear_id),
        )
        .selected(current.is_none())
        .enabled(enabled)
        .focused(highlight == 0);
        if enabled {
            clear = clear.action(AxAction::Press);
        }
        menu = menu.child(clear);
        if candidates.is_empty() {
            let id = format!("settings-role-menu-empty-{}", role.wire_name());
            return visible(
                menu.child(
                    AxNode::new(
                        id.clone(),
                        AxRole::StaticText,
                        t("settings.roles.empty_title"),
                        bounds(&id),
                    )
                    .value(t("settings.roles.empty_hint")),
                ),
            );
        }
        let mut ix = 1;
        for (provider_id, models) in candidates {
            let display = self
                .projection
                .settings_providers
                .providers
                .iter()
                .find(|provider| provider.provider_id == provider_id)
                .map(|provider| provider.display_name.clone())
                .unwrap_or_else(|| provider_id.to_string());
            let group_id = dynamic_identifier(
                &format!("settings-role-menu-group-{}", role.wire_name()),
                &provider_id,
            );
            menu = menu.child(AxNode::new(
                group_id.clone(),
                AxRole::StaticText,
                display,
                bounds(&group_id),
            ));
            for model in models {
                let id = settings_role_item_identifier(role, &model.provider_id, &model.id);
                let selected = current.is_some_and(|(provider, id)| {
                    provider == &model.provider_id && id == &model.id
                });
                let mut node = AxNode::new(
                    id.clone(),
                    AxRole::Button,
                    model.display_name.clone(),
                    bounds(&id),
                )
                .value(format!("{} / {}", model.provider_id, model.id))
                .selected(selected)
                .enabled(enabled)
                .focused(ix == highlight);
                if enabled {
                    node = node.action(AxAction::Press);
                }
                menu = menu.child(node);
                ix += 1;
            }
        }
        visible(menu)
    }

    fn settings_models_menu_ax(&self, provider_id: &str, window: &Window) -> AxNode {
        let menu_id = settings_models_menu_identifier(provider_id);
        let bounds = |id: &str| self.settings_menu_element_bounds(id, &menu_id);
        let models: Vec<_> = self
            .projection
            .settings_providers
            .model_catalog
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .collect();
        let filtered_models = self.settings_filtered_models(provider_id);
        let list_id = format!("{menu_id}-list");
        let pending = self
            .projection
            .settings_providers
            .model_write_pending
            .as_ref()
            .is_some_and(|pending| pending.targets(provider_id));
        let writes = self.settings_writes_enabled();
        let button = |id: String, name: &str, enabled: bool| {
            let focused = self
                .settings_action_focus
                .get(&id)
                .is_some_and(|focus| focus.is_focused(window));
            let node = AxNode::new(id.clone(), AxRole::Button, name, bounds(&id))
                .enabled(enabled)
                .focused(focused);
            if enabled {
                node.action(AxAction::Press)
            } else {
                node
            }
        };
        let mut menu = AxNode::new(
            menu_id.clone(),
            AxRole::Group,
            t("settings.providers.models_title"),
            bounds(&menu_id),
        )
        .child(self.model_search_ax(window, &menu_id));
        let heading_id = format!("settings-models-heading-{provider_id}");
        menu = menu.child(
            AxNode::new(
                heading_id.clone(),
                AxRole::StaticText,
                t("settings.providers.models_title"),
                bounds(&heading_id),
            )
            .value(format!(
                "{} · {}",
                self.model_provider_status(provider_id),
                crate::ui::i18n::t2(
                    "settings.providers.models_count",
                    &models
                        .iter()
                        .filter(|model| model.enabled)
                        .count()
                        .to_string(),
                    &models.len().to_string(),
                )
            )),
        );
        let scope_id = format!("settings-models-scope-{provider_id}");
        menu = menu.child(AxNode::new(
            scope_id.clone(),
            AxRole::StaticText,
            t("settings.providers.catalog_scope"),
            bounds(&scope_id),
        ));
        for (id, label) in [
            (
                settings_models_enable_all_identifier(provider_id),
                t("settings.providers.models_enable_all"),
            ),
            (
                settings_models_disable_all_identifier(provider_id),
                t("settings.providers.models_disable_all"),
            ),
        ] {
            menu = menu.child(button(id, label, writes && !pending && !models.is_empty()));
        }
        menu = menu.child(button(
            settings_models_refresh_identifier(provider_id),
            t("settings.providers.models_refresh"),
            writes,
        ));
        if models.is_empty() {
            let id = format!("settings-models-empty-{provider_id}");
            return visible(
                menu.child(
                    AxNode::new(
                        id.clone(),
                        AxRole::StaticText,
                        t("settings.providers.models_empty_title"),
                        bounds(&id),
                    )
                    .value(t("settings.providers.models_empty_hint")),
                ),
            );
        }
        if filtered_models.is_empty() {
            let id = format!("settings-models-no-results-{provider_id}");
            return visible(menu.child(AxNode::new(
                id.clone(),
                AxRole::StaticText,
                t("model_search.no_results"),
                bounds(&id),
            )));
        }
        for model in filtered_models {
            let id = settings_model_switch_identifier(provider_id, &model.id);
            let mut node = button(id.clone(), &model.display_name, writes && !pending)
                .value(if model.enabled {
                    t("settings.providers.switch_on")
                } else {
                    t("settings.providers.switch_off")
                })
                .description(format!("{}/{}", model.provider_id, model.id))
                .selected(model.enabled);
            node.bounds = self.settings_menu_element_bounds(&id, &list_id);
            menu = menu.child(node);
            // ADR-063：推理强度行（默认 cycle + 范围 chips + 手动范围时
            // 重置），identifier 与 render 同源，几何取滚动列表实测布局。
            let effort_enabled = writes && !pending;
            let default_id = settings_model_effort_default_identifier(provider_id, &model.id);
            let default_focused = self
                .settings_action_focus
                .get(&default_id)
                .is_some_and(|focus| focus.is_focused(window));
            let mut default_node = AxNode::new(
                default_id.clone(),
                AxRole::Button,
                t("settings.providers.effort_default"),
                self.settings_menu_element_bounds(&default_id, &list_id),
            )
            .enabled(effort_enabled)
            .focused(default_focused)
            .value(
                model
                    .default_effort
                    .clone()
                    .unwrap_or_else(|| t("settings.providers.effort_default_auto").to_string()),
            );
            if effort_enabled {
                default_node = default_node.action(AxAction::Press);
            }
            menu = menu.child(default_node);
            let selected: Vec<String> = model
                .effective_efforts()
                .map(|efforts| efforts.to_vec())
                .unwrap_or_default();
            for level in model.effort_options() {
                let chip_id = settings_model_effort_chip_identifier(&level, provider_id, &model.id);
                let chip_focused = self
                    .settings_action_focus
                    .get(&chip_id)
                    .is_some_and(|focus| focus.is_focused(window));
                let mut chip = AxNode::new(
                    chip_id.clone(),
                    AxRole::Button,
                    t("settings.providers.effort_allowed"),
                    self.settings_menu_element_bounds(&chip_id, &list_id),
                )
                .enabled(effort_enabled)
                .focused(chip_focused)
                .selected(selected.iter().any(|l| l == &level))
                .value(level.clone());
                if effort_enabled {
                    chip = chip.action(AxAction::Press);
                }
                menu = menu.child(chip);
            }
            if model.manual_efforts.is_some() {
                let reset_id = settings_model_effort_reset_identifier(provider_id, &model.id);
                let reset_focused = self
                    .settings_action_focus
                    .get(&reset_id)
                    .is_some_and(|focus| focus.is_focused(window));
                let mut reset = AxNode::new(
                    reset_id.clone(),
                    AxRole::Button,
                    t("settings.subagents.reset"),
                    self.settings_menu_element_bounds(&reset_id, &list_id),
                )
                .enabled(effort_enabled)
                .focused(reset_focused);
                if effort_enabled {
                    reset = reset.action(AxAction::Press);
                }
                menu = menu.child(reset);
            }
        }
        visible(menu)
    }
}
