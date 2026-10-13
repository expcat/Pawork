//! 供应商账号动作、重命名、凭证区，以及 OAuth 与 API key。

use std::collections::HashSet;

use gpui::{Focusable, Window};

use super::*;
use crate::ui::MenuKind;

/// 授权按钮只交给浏览器 HTTP(S) 链接，不启动任意系统协议。
fn oauth_url_can_open(url: &str) -> bool {
    url.strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .is_some_and(|rest| {
            !rest
                .split(['/', '?', '#'])
                .next()
                .unwrap_or_default()
                .is_empty()
                && !url.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        })
}

fn fallback_api_key_account_name(secret: &str) -> String {
    let chars: Vec<char> = secret.trim().chars().collect();
    match chars.len() {
        0..=4 => "••••".into(),
        5..=12 => {
            let head: String = chars[..2].iter().collect();
            let tail: String = chars[chars.len() - 2..].iter().collect();
            format!("{head}***{tail}")
        }
        _ => {
            let head: String = chars[..4].iter().collect();
            let tail: String = chars[chars.len() - 4..].iter().collect();
            format!("{head}***{tail}")
        }
    }
}

#[test]
fn fallback_api_key_account_name_masks_head_and_tail() {
    assert_eq!(fallback_api_key_account_name("abcd"), "••••");
    assert_eq!(fallback_api_key_account_name("sk-abcdef"), "sk***ef");
    assert_eq!(
        fallback_api_key_account_name("sk-abcdEFGHwxyz"),
        "sk-a***wxyz"
    );
}

#[test]
fn oauth_browser_links_reject_non_web_targets() {
    assert!(oauth_url_can_open(
        "https://accounts.x.ai/activate?code=ABCD-1234"
    ));
    assert!(oauth_url_can_open("http://localhost:1455/authorize"));
    for url in [
        "",
        "https://",
        "https:///missing-host",
        "javascript:alert(1)",
        "file:///tmp/auth",
        "pawork://authorize",
        "https://example.com/\nother",
    ] {
        assert!(!oauth_url_can_open(url), "{url:?}");
    }
}

impl AppView {
    pub(crate) fn settings_api_minor(&self) -> u16 {
        self.handshake_info
            .as_ref()
            .and_then(|info| info.api_version.split_once('.'))
            .and_then(|(major, minor)| (major == "1").then(|| minor.parse::<u16>().ok()).flatten())
            .unwrap_or(0)
    }

    pub(crate) fn settings_accounts_supported(&self) -> bool {
        self.settings_api_minor() >= 15
    }

    pub(crate) fn settings_account_rename_supported(&self) -> bool {
        self.settings_api_minor() >= 17
    }

    pub(crate) fn settings_credentials_title(&self) -> &'static str {
        t(if self.settings_accounts_supported() {
            "settings.providers.accounts_title"
        } else {
            "settings.providers.credentials_title"
        })
    }

    pub(crate) fn settings_credentials_subtitle(&self) -> &'static str {
        t(if self.settings_accounts_supported() {
            "settings.providers.account_subtitle"
        } else {
            "settings.providers.credentials_subtitle"
        })
    }

    pub(crate) fn settings_provider_auth_actions(
        &self,
        provider: &ProviderAuthStatusEntry,
        editor: bool,
        remove: bool,
        wait: bool,
    ) -> Vec<SettingsAuthAction> {
        settings_auth_actions(provider, editor, remove, wait)
            .into_iter()
            .filter(|action| {
                !self.settings_accounts_supported()
                    || !matches!(
                        action,
                        SettingsAuthAction::Remove
                            | SettingsAuthAction::ConfirmRemove
                            | SettingsAuthAction::KeepRemove
                    )
            })
            .collect()
    }

    pub(crate) fn settings_auth_action_label(&self, action: SettingsAuthAction) -> &'static str {
        if self.settings_accounts_supported() {
            match action {
                SettingsAuthAction::ConnectApiKey | SettingsAuthAction::ReplaceApiKey => {
                    return t("settings.providers.add_api_key");
                }
                SettingsAuthAction::ConnectOauth | SettingsAuthAction::ReplaceOauth => {
                    return t("settings.providers.add_oauth");
                }
                _ => {}
            }
        }
        action.label()
    }

    fn settings_new_account_display_name(&self, api_key: Option<&str>) -> Option<String> {
        if !self.settings_accounts_supported() {
            return None;
        }
        if self.settings_account_rename_supported() {
            return Some(String::new());
        }
        Some(match api_key {
            Some(secret) => fallback_api_key_account_name(secret),
            None => "OAuth".into(),
        })
    }

    pub(crate) fn settings_account_action_enabled(
        &self,
        provider: &ProviderAuthStatusEntry,
        credential: &pawork_client::ProviderCredentialStatus,
        remove: bool,
    ) -> bool {
        self.settings_accounts_supported()
            && self.settings_writes_enabled()
            && !matches!(provider.auth, ProviderAuthState::Connecting)
            && !credential.credential_id.is_empty()
            && if remove {
                !credential.selected || provider.credentials.len() == 1
            } else {
                !credential.selected
            }
    }

    pub(crate) fn settings_account_action_label(
        &self,
        identifier: &str,
        remove: bool,
    ) -> &'static str {
        t(if !remove {
            "settings.providers.account_use"
        } else if self.settings_account_remove_confirm.as_deref() == Some(identifier) {
            "settings.providers.action_confirm_remove"
        } else {
            "settings.providers.action_remove"
        })
    }

    fn settings_account_button(
        &mut self,
        provider: &ProviderAuthStatusEntry,
        credential: &pawork_client::ProviderCredentialStatus,
        remove: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = settings_account_action_identifier(
            &provider.provider_id,
            &credential.credential_id,
            remove,
        );
        let enabled = self.settings_account_action_enabled(provider, credential, remove);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = id.clone();
        let activate_id = id.clone();
        let button = Button::new(id.clone())
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(self.settings_account_action_label(&id, remove))
            .disabled(!enabled)
            .tooltip(
                if remove && credential.selected && provider.credentials.len() > 1 {
                    t("settings.providers.account_remove_hint")
                } else {
                    ""
                },
            )
            .on_click(cx.listener(move |view, event, window, cx| {
                if !view.consume_button_key_click(&click_id, event) {
                    view.on_settings_account_action(&click_id, window, cx);
                }
            }))
            .on_activate(cx.listener(move |view, _, window, cx| {
                view.note_button_key_activate(&activate_id);
                view.on_settings_account_action(&activate_id, window, cx);
                cx.stop_propagation();
            }));
        self.settings_element(id).flex_none().child(button)
    }

    fn settings_account_rename_button(
        &mut self,
        provider: &ProviderAuthStatusEntry,
        credential: &pawork_client::ProviderCredentialStatus,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id =
            settings_account_rename_identifier(&provider.provider_id, &credential.credential_id);
        let enabled = self.settings_account_rename_supported()
            && self.settings_writes_enabled()
            && !matches!(provider.auth, ProviderAuthState::Connecting)
            && !credential.credential_id.is_empty();
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = id.clone();
        let activate_id = id.clone();
        let button = Button::new(id.clone())
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(t("settings.providers.account_rename"))
            .disabled(!enabled)
            .on_click(cx.listener(move |view, event, window, cx| {
                if !view.consume_button_key_click(&click_id, event) {
                    view.on_settings_account_action(&click_id, window, cx);
                }
            }))
            .on_activate(cx.listener(move |view, _, window, cx| {
                view.note_button_key_activate(&activate_id);
                view.on_settings_account_action(&activate_id, window, cx);
                cx.stop_propagation();
            }));
        self.settings_element(id).flex_none().child(button)
    }

    pub(crate) fn account_rename_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.settings_account_rename
            .as_ref()
            .is_some_and(|rename| rename.input.read(cx).focus_handle(cx).is_focused(window))
    }

    fn begin_account_rename(
        &mut self,
        provider_id: &str,
        credential_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_open_menu(cx);
        let title = self
            .projection
            .settings_providers
            .providers
            .iter()
            .find(|provider| provider.provider_id == provider_id)
            .and_then(|provider| {
                provider
                    .credentials
                    .iter()
                    .find(|credential| credential.credential_id == credential_id)
            })
            .map(|credential| {
                if credential.display_name.is_empty() {
                    provider_credential_kind_label(&credential.kind)
                } else {
                    credential.display_name.clone()
                }
            })
            .unwrap_or_default();
        let input = cx.new(|cx| {
            TextInput::with_placeholder(t("settings.providers.account_alias"), cx)
                .id(settings_account_rename_input_identifier(
                    provider_id,
                    credential_id,
                ))
                .height_clamp(
                    metrics::COMPOSER_INPUT_MIN_HEIGHT,
                    metrics::COMPOSER_INPUT_MIN_HEIGHT,
                )
        });
        input.update(cx, |editor, cx| editor.set_text(title, cx));
        let focus = input.read(cx).focus_handle(cx);
        self.settings_account_rename = Some(AccountRenameState {
            provider_id: provider_id.to_string(),
            credential_id: credential_id.to_string(),
            input,
        });
        window.focus(&focus);
        cx.notify();
    }

    fn end_account_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.settings_account_rename.take() else {
            return;
        };
        let focus_key =
            settings_account_rename_identifier(&rename.provider_id, &rename.credential_id);
        if let Some(handle) = self.settings_action_focus.get(&focus_key) {
            window.focus(handle);
        }
        cx.notify();
    }

    pub(crate) fn commit_account_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.settings_account_rename.as_ref() else {
            return;
        };
        let provider_id = rename.provider_id.clone();
        let credential_id = rename.credential_id.clone();
        let draft = rename.input.read(cx).text().to_string();
        let current = self
            .projection
            .settings_providers
            .providers
            .iter()
            .find(|provider| provider.provider_id == provider_id)
            .and_then(|provider| {
                provider
                    .credentials
                    .iter()
                    .find(|credential| credential.credential_id == credential_id)
                    .map(|credential| credential.display_name.clone())
            })
            .unwrap_or_default();
        match crate::ui::session_rename_decision(&current, &draft) {
            crate::ui::SessionRenameDecision::KeepEditing => cx.notify(),
            crate::ui::SessionRenameDecision::Close => self.end_account_rename(window, cx),
            crate::ui::SessionRenameDecision::Submit(name) => {
                self.end_account_rename(window, cx);
                self.controller
                    .auth_account_rename(provider_id, credential_id, name);
            }
        }
    }

    pub(crate) fn cancel_account_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.end_account_rename(window, cx);
    }

    pub(crate) fn on_settings_account_action(
        &mut self,
        identifier: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .settings_account_remove_confirm
            .as_ref()
            .is_some_and(|id| format!("{id}-keep") == identifier)
        {
            self.settings_account_remove_confirm = None;
            cx.notify();
            return true;
        }
        for provider in self.projection.settings_providers.providers.clone() {
            for credential in &provider.credentials {
                if settings_account_rename_identifier(
                    &provider.provider_id,
                    &credential.credential_id,
                ) == identifier
                {
                    if self.settings_account_rename_supported()
                        && self.settings_writes_enabled()
                        && !matches!(provider.auth, ProviderAuthState::Connecting)
                        && !credential.credential_id.is_empty()
                    {
                        self.begin_account_rename(
                            &provider.provider_id,
                            &credential.credential_id,
                            window,
                            cx,
                        );
                    }
                    return true;
                }
                for remove in [false, true] {
                    if settings_account_action_identifier(
                        &provider.provider_id,
                        &credential.credential_id,
                        remove,
                    ) == identifier
                    {
                        if self.settings_account_action_enabled(&provider, credential, remove) {
                            if remove
                                && self.settings_account_remove_confirm.as_deref()
                                    != Some(identifier)
                            {
                                self.settings_account_remove_confirm = Some(identifier.to_string());
                            } else {
                                self.settings_account_remove_confirm = None;
                                self.settings_account_rename = None;
                                self.projection
                                    .settings_providers
                                    .account_quotas
                                    .retain(|(id, _), _| id != &provider.provider_id);
                                self.projection
                                    .settings_providers
                                    .account_mode_pending
                                    .remove(&provider.provider_id);
                                self.controller.auth_account_change(
                                    provider.provider_id.clone(),
                                    credential.credential_id.clone(),
                                    remove,
                                );
                            }
                            cx.notify();
                        }
                        return true;
                    }
                }
            }
        }
        false
    }
    /// Credentials 区（ADR-056 D4）：标题 + 副标题；每条存储凭证一行
    ///（类型标签 + masked + 状态点 Connected/Expired）；空列表诚实空态。
    /// Connect / Replace / Remove 动作按钮与 API key 编辑器、OAuth 等待、
    /// Remove 二次确认等流程详情均在本区内渲染，行为不变。
    pub(super) fn settings_provider_credentials_block(
        &mut self,
        provider: &ProviderAuthStatusEntry,
        actions: &[SettingsAuthAction],
        row_actions: &[SettingsAuthAction],
        oauth_waits: &std::collections::HashMap<String, AuthStartData>,
        auth_notes: &std::collections::HashMap<String, String>,
        writes: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let provider_id = provider.provider_id.clone();
        let editor_open = self.settings_api_key_editor_visible(provider);
        let remove_confirm = self.settings_remove_confirm.as_deref() == Some(provider_id.as_str());
        let oauth_waiting = oauth_waits.contains_key(&provider_id);
        let endpoint_visible = editor_open || oauth_waiting || remove_confirm;
        let auth_error = match &provider.auth {
            ProviderAuthState::Error { message } => Some(message.as_str()),
            _ => None,
        };
        let catalog_error = matches!(
            (&provider.auth, &provider.catalog),
            (
                ProviderAuthState::Connected { .. },
                crate::projection::ProviderCatalogState::Unavailable { .. },
            )
        );
        let mut block = self
            .settings_element(dynamic_identifier(
                "settings-provider-credentials",
                &provider_id,
            ))
            .flex()
            .flex_col()
            .px_4()
            .py_4()
            .gap_3()
            .border_b_1()
            .border_color(dark().border.subtle)
            .child(
                self.settings_element(dynamic_identifier(
                    "settings-provider-credentials-header",
                    &provider_id,
                ))
                .flex()
                .flex_col()
                .gap_1()
                .child(settings_label(self.settings_credentials_title()))
                .child(settings_copy(self.settings_credentials_subtitle())),
            );
        if provider.credentials.is_empty() {
            block = block.child(self.settings_note(
                dynamic_identifier("settings-provider-credentials-empty", &provider_id),
                t("settings.providers.credentials_empty"),
            ));
        }
        for (ix, credential) in provider.credentials.iter().enumerate() {
            let name = if credential.display_name.is_empty() {
                provider_credential_kind_label(&credential.kind)
            } else {
                credential.display_name.clone()
            };
            let renaming = self.settings_account_rename.as_ref().is_some_and(|rename| {
                rename.provider_id == provider_id
                    && rename.credential_id == credential.credential_id
            });
            let mut header = div().flex().flex_row().items_center().gap_2().child(
                div()
                    .flex_none()
                    .size(px(8.0))
                    .rounded_full()
                    .bg(if credential.expired {
                        dark().semantic.danger_text
                    } else if credential.selected {
                        dark().semantic.success_fg
                    } else {
                        dark().text.tertiary
                    }),
            );
            if renaming {
                if let Some(input) = self
                    .settings_account_rename
                    .as_ref()
                    .map(|rename| rename.input.clone())
                {
                    header = header.child(
                        self.settings_element(settings_account_rename_input_identifier(
                            &provider_id,
                            &credential.credential_id,
                        ))
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_w_0()
                        .child(input),
                    );
                }
            } else {
                header = header.child(
                    div().flex().flex_row().flex_1().min_w_0().child(
                        div()
                            .truncate()
                            .text_size(font::BODY)
                            .text_color(dark().text.primary)
                            .child(name),
                    ),
                );
            }
            header = header
                .child(
                    div()
                        .flex_none()
                        .px_2()
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(dark().border.subtle)
                        .child(
                            Label::new(provider_credential_kind_label(&credential.kind))
                                .size(font::BODY_SM)
                                .color(dark().text.tertiary),
                        ),
                )
                .when(credential.selected, |row| {
                    row.child(
                        Label::new(t("settings.providers.account_selected_badge"))
                            .size(font::BODY_SM)
                            .color(dark().semantic.success_fg),
                    )
                });
            if self.settings_accounts_supported() && !credential.credential_id.is_empty() {
                let use_account = self.settings_account_button(provider, credential, false, cx);
                let remove_account = self.settings_account_button(provider, credential, true, cx);
                header = header.child(use_account);
                if self.settings_account_rename_supported() {
                    let rename_account =
                        self.settings_account_rename_button(provider, credential, cx);
                    header = header.child(rename_account);
                }
                header = header.child(remove_account);
            }
            let mut row = self
                .settings_element(settings_credential_row_identifier(
                    &provider_id,
                    credential,
                    ix,
                ))
                .flex()
                .flex_col()
                .gap_2()
                .py_3()
                .when(ix > 0, |row| {
                    row.border_t_1().border_color(dark().border.subtle)
                })
                .child(header)
                .child(
                    div().flex().flex_row().flex_none().child(
                        div().flex_1().min_w_0().child(
                            div()
                                .truncate()
                                .text_size(font::BODY_SM)
                                .text_color(if credential.expired {
                                    dark().semantic.danger_text
                                } else {
                                    dark().text.tertiary
                                })
                                .child(format!(
                                    "{} · {}",
                                    credential.masked_credential,
                                    provider_credential_status_label(credential.expired)
                                )),
                        ),
                    ),
                );
            if self.account_quota_supported(&provider_id, credential) {
                row = row.child(self.account_quota_element(
                    &provider_id,
                    &credential.credential_id,
                    cx,
                ));
            }
            let remove_id =
                settings_account_action_identifier(&provider_id, &credential.credential_id, true);
            if self.settings_account_remove_confirm.as_deref() == Some(&remove_id) {
                let keep_id = format!("{remove_id}-keep");
                let focus = self
                    .settings_action_focus
                    .entry(keep_id.clone())
                    .or_insert_with(|| cx.focus_handle().tab_stop(true))
                    .clone();
                let click_id = keep_id.clone();
                let activate_id = keep_id.clone();
                row = row.child(
                    div().flex().child(
                        self.settings_element(keep_id.clone()).flex_none().child(
                            Button::new(keep_id)
                                .track_focus(&focus)
                                .variant(ButtonVariant::Raised)
                                .label(t("settings.providers.action_keep"))
                                .height(px(SETTINGS_CONTROL_HEIGHT))
                                .radius(6.0)
                                .bordered()
                                .text_size(font::BODY_SM)
                                .on_click(cx.listener(move |view, event, window, cx| {
                                    if !view.consume_button_key_click(&click_id, event) {
                                        view.on_settings_account_action(&click_id, window, cx);
                                    }
                                }))
                                .on_activate(cx.listener(move |view, _, window, cx| {
                                    view.note_button_key_activate(&activate_id);
                                    view.on_settings_account_action(&activate_id, window, cx);
                                    cx.stop_propagation();
                                })),
                        ),
                    ),
                );
            }
            block = block.child(row);
        }
        if provider_id == "opencode-go" && self.settings_quota_supported() {
            block = block.child(self.account_mode_element(provider, cx)).child(
                self.settings_element(quota_identifier(&provider_id, "", "scope"))
                    .child(settings_copy(t("settings.quota.scope"))),
            );
        }
        // 登录详情保持原文，长 URL 可横滚、所有可见信息可选中复制。
        let mut details = Vec::new();
        if let (ProviderAuthState::Connecting, Some(wait)) =
            (&provider.auth, oauth_waits.get(&provider_id))
        {
            let mut actions = div().flex().flex_wrap().gap_2();
            for action in [
                SettingsAuthAction::OpenOauth,
                SettingsAuthAction::CopyOauthUrl,
                SettingsAuthAction::CopyOauthCode,
            ] {
                if action == SettingsAuthAction::CopyOauthCode && wait.user_code.is_none() {
                    continue;
                }
                actions = actions.child(self.settings_action_button(
                    action,
                    &provider_id,
                    self.settings_action_enabled(action, &provider_id, writes, cx),
                    "",
                    cx,
                ));
            }
            block = block.child(actions);
            details
                .push(t("settings.providers.authorize_at").replace("{}", &wait.verification_url));
            if let Some(code) = &wait.user_code {
                details.push(t("settings.providers.oauth_code").replace("{}", code));
            }
            if let Some(expires) = &wait.expires_at {
                details.push(t("settings.providers.oauth_expires").replace("{}", expires));
            }
        }
        if let Some(note) = auth_notes.get(&provider_id) {
            details.push(note.clone());
        }
        if let Some(message) = auth_error {
            details.push(t("settings.providers.connection_error").replace("{}", message));
        }
        if catalog_error {
            details.push(provider.catalog_label());
        }
        if endpoint_visible {
            details
                .push(t("settings.providers.endpoint_row").replace("{}", &provider.endpoint_label));
        }
        if !details.is_empty() {
            let details_id = dynamic_identifier("settings-provider-details", &provider_id);
            let value = details.join("\n");
            let input = self
                .settings_auth_details
                .entry(provider_id.clone())
                .or_insert_with(|| {
                    cx.new(|cx| {
                        crate::ui::text_input::TextInput::with_placeholder("", cx)
                            .id(details_id.clone())
                            .read_only()
                            .height_clamp(28., 180.)
                    })
                })
                .clone();
            if input.read(cx).text() != value {
                input.update(cx, |input, cx| input.reset_text(value, cx));
            }
            block = block.child(
                self.settings_element(details_id)
                    .w_full()
                    .min_w_0()
                    .text_color(if auth_error.is_some() || catalog_error {
                        dark().semantic.danger_text
                    } else {
                        dark().text.secondary
                    })
                    .child(input),
            );
        } else {
            self.settings_auth_details.remove(&provider_id);
        }

        // API key secure 输入（内联）：none / error 常驻；connected 由
        // Replace 展开后出现；Verify 空输入禁用，明文不进 projection。
        if editor_open {
            if let Some(input) = self.settings_api_key_inputs.get(&provider_id).cloned() {
                let verify_enabled = self.settings_action_enabled(
                    SettingsAuthAction::VerifyApiKey,
                    &provider_id,
                    writes,
                    cx,
                );
                let mut editor = div().flex().flex_col().gap_3();
                editor = editor.child(
                    self.settings_element(settings_api_key_input_identifier(&provider_id))
                        .child(input),
                );
                let mut editor_actions = div().flex().flex_wrap().gap_2();
                for action in [
                    SettingsAuthAction::VerifyApiKey,
                    SettingsAuthAction::CancelApiKeyInput,
                ] {
                    if !actions.contains(&action) {
                        continue;
                    }
                    let (enabled, tooltip) = if action == SettingsAuthAction::VerifyApiKey {
                        (
                            verify_enabled,
                            if writes && !verify_enabled {
                                t("settings.providers.api_key_empty")
                            } else {
                                ""
                            },
                        )
                    } else {
                        (writes, "")
                    };
                    editor_actions = editor_actions.child(self.settings_action_button(
                        action,
                        &provider_id,
                        enabled,
                        tooltip,
                        cx,
                    ));
                }
                block = block.child(editor.child(editor_actions));
            }
        }

        // 认证动作按钮（Connect / Replace / Remove / Cancel 等）位于
        // Credentials 区下方；行为不变。
        if !row_actions.is_empty() {
            let mut row = div().flex().flex_row().gap_2().flex_wrap();
            for action in row_actions {
                let tooltip = if *action == SettingsAuthAction::Remove {
                    t("settings.providers.tooltip_remove_credential")
                } else {
                    ""
                };
                row = row.child(self.settings_action_button(
                    *action,
                    &provider_id,
                    self.settings_action_enabled(*action, &provider_id, writes, cx),
                    tooltip,
                    cx,
                ));
            }
            block = block.child(row);
        }
        block
    }
    /// 写操作总 gate：断线 / stale 一律禁写（可见 / 键盘 / AX 三路径共用）。
    pub(crate) fn settings_action_enabled(
        &self,
        action: SettingsAuthAction,
        provider_id: &str,
        writes: bool,
        cx: &App,
    ) -> bool {
        if matches!(
            action,
            SettingsAuthAction::OpenOauth
                | SettingsAuthAction::CopyOauthUrl
                | SettingsAuthAction::CopyOauthCode
        ) {
            let state = &self.projection.settings_providers;
            if !state.providers.iter().any(|entry| {
                entry.provider_id == provider_id
                    && matches!(entry.auth, ProviderAuthState::Connecting)
            }) {
                return false;
            }
            let Some(wait) = state.oauth_waits.get(provider_id) else {
                return false;
            };
            return match action {
                SettingsAuthAction::OpenOauth => {
                    writes && oauth_url_can_open(&wait.verification_url)
                }
                SettingsAuthAction::CopyOauthUrl => !wait.verification_url.is_empty(),
                SettingsAuthAction::CopyOauthCode => {
                    wait.user_code.as_ref().is_some_and(|code| !code.is_empty())
                }
                _ => unreachable!(),
            };
        }
        if !writes {
            return false;
        }
        if action != SettingsAuthAction::VerifyApiKey {
            return true;
        }
        self.settings_api_key_inputs
            .get(provider_id)
            .is_some_and(|input| !input.read(cx).text().trim().is_empty())
    }

    /// API key 内联编辑器只在 Connect / Replace 后展开；普通 provider
    /// 概览始终保持紧凑，connecting（验证中）不显示。
    pub(crate) fn settings_api_key_editor_visible(
        &self,
        provider: &ProviderAuthStatusEntry,
    ) -> bool {
        if !provider
            .auth_methods
            .iter()
            .any(|method| method == "api_key")
        {
            return false;
        }
        match provider.auth {
            ProviderAuthState::None
            | ProviderAuthState::Error { .. }
            | ProviderAuthState::Connected { .. } => self
                .settings_api_key_editors
                .contains(&provider.provider_id),
            ProviderAuthState::Connecting => false,
        }
    }

    /// 写动作按钮：可见 / 键盘（on_activate）/ AX（同名 identifier Press）
    /// 三路径汇入同一 on_settings_action；disabled 时三者同时失效。
    pub(super) fn settings_action_button(
        &mut self,
        action: SettingsAuthAction,
        provider_id: &str,
        enabled: bool,
        tooltip: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = action.identifier(provider_id);
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click_id = id.clone();
        let click_provider = provider_id.to_string();
        let activate_id = id.clone();
        let activate_provider = provider_id.to_string();
        let button = Button::new(id.clone())
            .track_focus(&focus)
            .variant(ButtonVariant::Raised)
            .height(px(SETTINGS_CONTROL_HEIGHT))
            .vcenter()
            .radius(6.0)
            .bordered()
            .text_size(font::BODY_SM)
            .label(
                if self
                    .settings_copied_auth
                    .as_ref()
                    .is_some_and(|(id, copied)| id == provider_id && *copied == action)
                {
                    t("settings.providers.copied")
                } else {
                    self.settings_auth_action_label(action)
                },
            )
            .disabled(!enabled)
            .on_click(cx.listener(move |view, event, _window, cx| {
                if view.consume_button_key_click(&click_id, event) {
                    return;
                }
                view.on_settings_action(action, click_provider.clone(), cx);
            }))
            .on_activate(cx.listener(move |view, _event, _window, cx| {
                view.note_button_key_activate(&activate_id);
                view.on_settings_action(action, activate_provider.clone(), cx);
                cx.stop_propagation();
            }));
        let button = if tooltip.is_empty() {
            button
        } else {
            button.tooltip(tooltip)
        };
        self.settings_element(id).flex_none().child(button)
    }

    /// settings 写动作统一入口（三路径同源；入口级复核 gate 与 descriptor）。
    pub(crate) fn on_settings_action(
        &mut self,
        action: SettingsAuthAction,
        provider_id: String,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_action_enabled(action, &provider_id, self.settings_writes_enabled(), cx) {
            return;
        }
        match action {
            SettingsAuthAction::OpenOauth
            | SettingsAuthAction::CopyOauthUrl
            | SettingsAuthAction::CopyOauthCode => {
                let Some(wait) = self
                    .projection
                    .settings_providers
                    .oauth_waits
                    .get(&provider_id)
                else {
                    return;
                };
                if action == SettingsAuthAction::OpenOauth {
                    cx.open_url(&wait.verification_url);
                } else {
                    let value = if action == SettingsAuthAction::CopyOauthCode {
                        wait.user_code.clone().unwrap_or_default()
                    } else {
                        wait.verification_url.clone()
                    };
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
                    self.settings_copied_auth = Some((provider_id, action));
                    cx.notify();
                }
            }
            SettingsAuthAction::ConnectOauth | SettingsAuthAction::ReplaceOauth => {
                self.on_settings_connect_oauth(provider_id, cx);
            }
            SettingsAuthAction::CancelOauth => {
                self.controller.auth_cancel(provider_id);
            }
            SettingsAuthAction::ConnectApiKey | SettingsAuthAction::ReplaceApiKey => {
                self.settings_api_key_editors.insert(provider_id);
            }
            SettingsAuthAction::VerifyApiKey => {
                self.on_settings_verify_api_key(provider_id, cx);
            }
            SettingsAuthAction::CancelApiKeyInput => {
                self.on_settings_cancel_api_key_input(provider_id, cx);
            }
            SettingsAuthAction::Remove => {
                self.settings_remove_confirm = Some(provider_id);
            }
            SettingsAuthAction::ConfirmRemove => {
                self.settings_remove_confirm = None;
                self.controller.auth_remove(provider_id);
            }
            SettingsAuthAction::KeepRemove => {
                self.settings_remove_confirm = None;
            }
        }
    }
    /// 启动 OAuth：descriptor 复核后登记 Replace 基线并置 Connecting。
    fn on_settings_connect_oauth(&mut self, provider_id: String, cx: &mut Context<Self>) {
        // descriptor 复核：provider 必须存在且声明 oauth（未知 id fail-closed）。
        let declares = self
            .projection
            .settings_providers
            .providers
            .iter()
            .any(|entry| {
                entry.provider_id == provider_id
                    && entry.auth_methods.iter().any(|method| method == "oauth")
            });
        if !declares {
            return;
        }
        // Replace 基线：Connected 起点的写流程终态不清旧凭证（交重查）。
        self.projection
            .settings_providers
            .begin_auth_flow(&provider_id);
        // 乐观置 Connecting；AuthStarted 回执补 URL 详情，失败经
        // OperationFailed 触发状态重查回滚。
        if let Some(entry) = self
            .projection
            .settings_providers
            .providers
            .iter_mut()
            .find(|entry| entry.provider_id == provider_id)
        {
            entry.auth = ProviderAuthState::Connecting;
        }
        let name = self.settings_new_account_display_name(None);
        self.controller.auth_start(provider_id, name);
        cx.notify();
    }

    fn on_settings_verify_api_key(&mut self, provider_id: String, cx: &mut Context<Self>) {
        // descriptor 复核：provider 必须声明 api_key。
        let declares = self
            .projection
            .settings_providers
            .providers
            .iter()
            .any(|entry| {
                entry.provider_id == provider_id
                    && entry.auth_methods.iter().any(|method| method == "api_key")
            });
        let Some(input) = self.settings_api_key_inputs.get(&provider_id).cloned() else {
            return;
        };
        let key = input.read(cx).text().trim().to_string();
        if !declares || key.is_empty() {
            return;
        }
        // 清空输入缓冲（含 undo 栈，SET-005「提交后清空 UI 缓冲」）；
        // 明文只进 controller 调用栈。
        input.update(cx, |input, cx| input.reset_text("", cx));
        self.settings_api_key_editors.remove(&provider_id);
        // Replace 基线：Connected 起点的写流程终态不清旧凭证（交重查）。
        self.projection
            .settings_providers
            .begin_auth_flow(&provider_id);
        if let Some(entry) = self
            .projection
            .settings_providers
            .providers
            .iter_mut()
            .find(|entry| entry.provider_id == provider_id)
        {
            entry.auth = ProviderAuthState::Connecting;
        }
        let name = self.settings_new_account_display_name(Some(&key));
        self.controller.auth_set_api_key(provider_id, key, name);
        cx.notify();
    }

    fn on_settings_cancel_api_key_input(&mut self, provider_id: String, cx: &mut Context<Self>) {
        if let Some(input) = self.settings_api_key_inputs.get(&provider_id).cloned() {
            input.update(cx, |input, cx| input.reset_text("", cx));
        }
        self.settings_api_key_editors.remove(&provider_id);
    }

    /// 按当前 provider 清单懒建 / 回收 secure 输入实体与焦点句柄（含
    /// 「设为默认」按钮随模型目录的回收）。
    pub(crate) fn ensure_settings_api_key_inputs(&mut self, cx: &mut Context<Self>) {
        if let Some(rename) = &self.settings_account_rename {
            let alive = self
                .projection
                .settings_providers
                .providers
                .iter()
                .any(|provider| {
                    provider.provider_id == rename.provider_id
                        && provider
                            .credentials
                            .iter()
                            .any(|credential| credential.credential_id == rename.credential_id)
                });
            if !alive {
                self.settings_account_rename = None;
            }
        }
        self.settings_auth_details.retain(|id, _| {
            self.projection
                .settings_providers
                .providers
                .iter()
                .any(|entry| &entry.provider_id == id)
        });
        let ids: Vec<String> = self
            .projection
            .settings_providers
            .providers
            .iter()
            .filter(|entry| entry.auth_methods.iter().any(|method| method == "api_key"))
            .map(|entry| entry.provider_id.clone())
            .collect();
        self.settings_api_key_inputs
            .retain(|id, _| ids.iter().any(|current| current == id));
        self.settings_api_key_editors
            .retain(|id| ids.iter().any(|current| current == id));
        // 焦点句柄回收按「当前 provider × 全部动作」的精确 identifier
        // 白名单比对，不用子串匹配（会误伤 id 段重叠的无关条目）。
        let mut action_ids = HashSet::new();
        for entry in &self.projection.settings_providers.providers {
            action_ids.insert(quota_identifier(&entry.provider_id, "", "mode"));
            for credential in &entry.credentials {
                action_ids.insert(quota_identifier(
                    &entry.provider_id,
                    &credential.credential_id,
                    "refresh",
                ));
                for remove in [false, true] {
                    action_ids.insert(settings_account_action_identifier(
                        &entry.provider_id,
                        &credential.credential_id,
                        remove,
                    ));
                }
                action_ids.insert(format!(
                    "{}-keep",
                    settings_account_action_identifier(
                        &entry.provider_id,
                        &credential.credential_id,
                        true
                    )
                ));
            }
            for action in SettingsAuthAction::ALL {
                action_ids.insert(action.identifier(&entry.provider_id));
            }
            // 展开 chevron 随 provider 清单常驻（折叠 / 展开均渲染）。
            action_ids.insert(settings_provider_expand_identifier(&entry.provider_id));
            // 展开区控件（代理 Switch / Manage models 触发器 / 弹层内
            // 控件）只在卡片有效展开时渲染；可见性与回收白名单同源
            //（代理开关另受全局代理配置 gate）。
            let editor_open = self.settings_api_key_editor_visible(entry);
            let oauth_waiting = self
                .projection
                .settings_providers
                .oauth_waits
                .contains_key(&entry.provider_id);
            let remove_confirm = self
                .settings_remove_confirm
                .as_deref()
                .is_some_and(|id| id == entry.provider_id);
            if self.settings_provider_card_expanded(
                &entry.provider_id,
                editor_open,
                oauth_waiting,
                remove_confirm,
            ) {
                if self.projection.settings_general.proxy_url.is_some() {
                    action_ids.insert(settings_use_proxy_identifier(&entry.provider_id));
                }
                action_ids.insert(settings_manage_models_identifier(&entry.provider_id));
                // 弹层内控件（Enable / Disable all、Refresh、每模型
                // Switch）只在该 provider 弹层打开时存在；句柄随目录条目
                // 建立、随白名单回收。
                if matches!(&self.open_menu, Some(MenuKind::SettingsProviderModels(open)) if open == &entry.provider_id)
                {
                    action_ids.insert("model-search-clear".to_string());
                    action_ids.insert(settings_models_enable_all_identifier(&entry.provider_id));
                    action_ids.insert(settings_models_disable_all_identifier(&entry.provider_id));
                    action_ids.insert(settings_models_refresh_identifier(&entry.provider_id));
                    for model in self
                        .projection
                        .settings_providers
                        .model_catalog
                        .iter()
                        .filter(|model| model.provider_id == entry.provider_id)
                    {
                        action_ids.insert(settings_model_switch_identifier(
                            &entry.provider_id,
                            &model.id,
                        ));
                    }
                }
            }
        }
        // 四默认角色下拉触发器随页常驻（OPT-3b）。
        for role in SettingsRole::ALL {
            action_ids.insert(settings_role_trigger_identifier(role));
        }
        self.settings_action_focus
            .retain(|id, _| action_ids.contains(id));
        for id in ids {
            self.settings_api_key_inputs
                .entry(id.clone())
                .or_insert_with(|| {
                    let element_id = format!("settings-api-key-input-{id}");
                    cx.new(|cx| {
                        TextInput::with_placeholder(t("settings.providers.api_key_placeholder"), cx)
                            .id(element_id)
                            .secure()
                            .height_clamp(
                                metrics::COMPOSER_INPUT_MIN_HEIGHT,
                                metrics::COMPOSER_INPUT_MIN_HEIGHT,
                            )
                    })
                });
        }
    }
}
