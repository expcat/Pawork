//! Go 逐账号额度：只消费 Host 的 canonical 读数；刷新身份与显示共用稳定 ID。
use super::*;
use crate::ui::components::switch::Switch;
use crate::ui::{now_unix_ms, AppRoute, SettingsPage};
use pawork_client::{
    ProviderAccountSelectionMode, QuotaMeasure, QuotaReset, QuotaUnit, QuotaWindow, WindowReadView,
};

pub(crate) fn quota_identifier(provider: &str, credential: &str, suffix: &str) -> String {
    dynamic_identifier(
        &format!("settings-account-quota-{suffix}"),
        &format!("{provider}:{credential}"),
    )
}

/// 保留分钟精度，长倒计时直接分解为天 / 小时 / 分钟。
fn quota_duration(minutes: u64) -> String {
    let mut parts = Vec::new();
    for (value, key) in [
        (minutes / 1440, "settings.quota.days"),
        (minutes % 1440 / 60, "settings.quota.hours"),
        (minutes % 60, "settings.quota.minutes"),
    ] {
        if value > 0 || (parts.is_empty() && key == "settings.quota.minutes") {
            parts.push(t(key).replace("{}", &value.to_string()));
        }
    }
    parts.join(" ")
}

impl AppView {
    pub(crate) fn settings_quota_supported(&self) -> bool {
        self.handshake_info
            .as_ref()
            .and_then(|i| i.api_version.split_once('.'))
            .is_some_and(|(major, minor)| {
                major == "1" && minor.parse::<u16>().is_ok_and(|n| n >= 16)
            })
    }

    pub(crate) fn settings_quota_clock_needed(&self) -> bool {
        self.route == AppRoute::Settings
            && self.settings_page == SettingsPage::Providers
            && !self.projection.settings_providers.account_quotas.is_empty()
    }

    pub(crate) fn account_quota_supported(
        &self,
        provider: &str,
        credential: &pawork_client::ProviderCredentialStatus,
    ) -> bool {
        self.settings_quota_supported()
            && provider == "opencode-go"
            && credential.kind == "api_key"
            && !credential.credential_id.is_empty()
    }

    pub(crate) fn account_quota_refresh_enabled(&self) -> bool {
        self.settings_quota_supported()
            && matches!(
                self.projection.connection,
                ConnectionState::Connected { .. }
            )
    }

    pub(crate) fn refresh_expanded_account_quotas(
        &mut self,
        refresh_existing: bool,
        cx: &mut Context<Self>,
    ) {
        if self.route != AppRoute::Settings || self.settings_page != SettingsPage::Providers {
            return;
        }
        let ids: Vec<_> = self
            .projection
            .settings_providers
            .providers
            .iter()
            .filter(|p| {
                self.settings_provider_card_expanded(
                    &p.provider_id,
                    self.settings_api_key_editor_visible(p),
                    self.projection
                        .settings_providers
                        .oauth_waits
                        .contains_key(&p.provider_id),
                    false,
                )
            })
            .flat_map(|p| {
                p.credentials
                    .iter()
                    .filter(|c| self.account_quota_supported(&p.provider_id, c))
                    .map(|c| (p.provider_id.clone(), c.credential_id.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (provider, credential) in ids {
            if refresh_existing
                || !self
                    .projection
                    .settings_providers
                    .account_quotas
                    .contains_key(&(provider.clone(), credential.clone()))
            {
                self.refresh_account_quota(provider, credential, cx);
            }
        }
    }

    pub(crate) fn refresh_account_quota(
        &mut self,
        provider: String,
        credential: String,
        cx: &mut Context<Self>,
    ) {
        if !self.account_quota_refresh_enabled() {
            return;
        }
        if !self
            .projection
            .settings_providers
            .providers
            .iter()
            .any(|p| {
                p.provider_id == provider
                    && p.credentials.iter().any(|c| {
                        c.credential_id == credential && self.account_quota_supported(&provider, c)
                    })
            })
        {
            return;
        }
        let epoch = self
            .projection
            .settings_providers
            .begin_quota(&provider, &credential);
        self.controller
            .load_account_quota(provider, credential, epoch);
        self.arm_run_clock(cx);
        cx.notify();
    }

    pub(crate) fn account_quota_labels(
        &self,
        provider: &str,
        credential: &str,
    ) -> Vec<(String, String)> {
        let state = self
            .projection
            .settings_providers
            .account_quotas
            .get(&(provider.into(), credential.into()));
        let now = now_unix_ms();
        [
            (QuotaWindow::Rolling5h, "rolling"),
            (QuotaWindow::Weekly, "weekly"),
            (QuotaWindow::Monthly, "monthly"),
        ]
        .into_iter()
        .map(|(window, name)| {
            let title = t(match window {
                QuotaWindow::Rolling5h => "settings.quota.rolling",
                QuotaWindow::Weekly => "settings.quota.weekly",
                _ => "settings.quota.monthly",
            });
            let entry = state
                .and_then(|s| s.view.as_ref())
                .and_then(|v| v.windows.iter().find(|e| e.window == window));
            let value = match entry.map(|e| &e.read) {
                Some(WindowReadView::Ok { snapshot, .. })
                    if snapshot.unit == QuotaUnit::Percent =>
                {
                    match (&snapshot.values.used, &snapshot.reset) {
                        (QuotaMeasure::Exact(used), QuotaReset::Absolute { at, uncertain })
                            if *used <= 100 =>
                        {
                            let stale = state.is_some_and(|s| s.stale)
                                || snapshot.provenance.fetched_at.as_unix_millis() > now
                                || snapshot.served_stale
                                || snapshot.provenance.stale
                                || now.saturating_sub(
                                    snapshot.provenance.fetched_at.as_unix_millis(),
                                ) > 30_000
                                || now >= at.as_unix_millis();
                            let minutes = at.as_unix_millis().saturating_sub(now).div_ceil(60_000);
                            let remaining = match snapshot.values.remaining {
                                QuotaMeasure::Exact(value) if value <= 100 => {
                                    t("settings.quota.remaining").replace("{}", &value.to_string())
                                }
                                _ => t("settings.quota.remaining_unknown").into(),
                            };
                            let reset = if now >= at.as_unix_millis() {
                                t("settings.quota.reset_due").into()
                            } else {
                                t("settings.quota.reset").replace("{}", &quota_duration(minutes))
                            };
                            let fetched = snapshot.provenance.fetched_at.as_unix_millis();
                            let age = if fetched > now {
                                t("settings.quota.clock_unknown").into()
                            } else {
                                t("settings.quota.updated").replace(
                                    "{}",
                                    &now.saturating_sub(fetched).div_ceil(1000).to_string(),
                                )
                            };
                            format!(
                                "{} · {}{}{}\n{}{}\n{} · {}",
                                t("settings.quota.used").replace("{}", &used.to_string()),
                                remaining,
                                if stale {
                                    format!(" · {}", t("settings.quota.stale"))
                                } else {
                                    String::new()
                                },
                                if state.is_some_and(|s| s.loading) {
                                    format!(" · {}", t("settings.quota.loading"))
                                } else {
                                    String::new()
                                },
                                reset,
                                if *uncertain {
                                    format!(" · {}", t("settings.quota.estimated"))
                                } else {
                                    String::new()
                                },
                                t("settings.quota.source").replace(
                                    "{}",
                                    if snapshot.provenance.source.is_empty() {
                                        t("settings.quota.source_unknown")
                                    } else {
                                        &snapshot.provenance.source
                                    }
                                ),
                                age,
                            )
                        }
                        _ => t("settings.providers.usage_unavailable").into(),
                    }
                }
                _ if state.is_some_and(|s| s.loading) => t("settings.quota.loading").into(),
                _ => t("settings.providers.usage_unavailable").into(),
            };
            (
                quota_identifier(provider, credential, name),
                format!("{title}\n{value}"),
            )
        })
        .collect()
    }

    pub(crate) fn account_quota_element(
        &mut self,
        provider: &str,
        credential: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut block = div().flex().flex_col().gap_3();
        for (id, label) in self.account_quota_labels(provider, credential) {
            block = block.child(
                self.settings_element(id)
                    .text_size(font::BODY_SM)
                    .text_color(dark().text.secondary)
                    .child(label),
            );
        }
        let id = quota_identifier(provider, credential, "refresh");
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click = id.clone();
        let activate = id.clone();
        let enabled = self.account_quota_refresh_enabled();
        block.child(
            div().flex().child(
                self.settings_element(id.clone()).flex_none().child(
                    Button::new(id)
                        .track_focus(&focus)
                        .variant(ButtonVariant::Raised)
                        .height(px(SETTINGS_CONTROL_HEIGHT))
                        .vcenter()
                        .radius(6.0)
                        .bordered()
                        .text_size(font::BODY_SM)
                        .label(t("settings.quota.refresh"))
                        .disabled(!enabled)
                        .on_click(cx.listener(move |view, event, _, cx| {
                            if !view.consume_button_key_click(&click, event) {
                                view.on_settings_quota_action(&click, cx);
                            }
                        }))
                        .on_activate(cx.listener(move |view, _, _, cx| {
                            view.note_button_key_activate(&activate);
                            view.on_settings_quota_action(&activate, cx);
                            cx.stop_propagation();
                        })),
                ),
            ),
        )
    }

    pub(crate) fn account_mode_enabled(&self, provider: &ProviderAuthStatusEntry) -> bool {
        self.settings_writes_enabled()
            && self.settings_quota_supported()
            && provider.provider_id == "opencode-go"
            && !matches!(provider.auth, ProviderAuthState::Connecting)
            && !self
                .projection
                .settings_providers
                .account_mode_pending
                .contains_key(&provider.provider_id)
            && (provider.selection_mode == ProviderAccountSelectionMode::WhenExhausted
                || provider
                    .credentials
                    .iter()
                    .any(|c| c.selected && c.kind == "api_key" && !c.credential_id.is_empty()))
    }

    pub(crate) fn account_mode_element(
        &mut self,
        provider: &ProviderAuthStatusEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = quota_identifier(&provider.provider_id, "", "mode");
        let focus = self
            .settings_action_focus
            .entry(id.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let click = id.clone();
        let activate = id.clone();
        let toggle = Switch::new(id.clone())
            .track_focus(&focus)
            .checked(provider.selection_mode == ProviderAccountSelectionMode::WhenExhausted)
            .disabled(!self.account_mode_enabled(provider))
            .on_click(cx.listener(move |view, event, _, cx| {
                if !view.consume_button_key_click(&click, event) {
                    view.on_settings_quota_action(&click, cx);
                }
            }))
            .on_activate(cx.listener(move |view, _, _, cx| {
                view.note_button_key_activate(&activate);
                view.on_settings_quota_action(&activate, cx);
                cx.stop_propagation();
            }));
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .child(settings_copy(t("settings.quota.auto"))),
            )
            .child(self.settings_element(id).flex_none().child(toggle))
    }

    pub(crate) fn on_settings_quota_action(
        &mut self,
        identifier: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        for p in self.projection.settings_providers.providers.clone() {
            if quota_identifier(&p.provider_id, "", "mode") == identifier {
                if self.account_mode_enabled(&p) {
                    let mode = if p.selection_mode == ProviderAccountSelectionMode::WhenExhausted {
                        ProviderAccountSelectionMode::Manual
                    } else {
                        ProviderAccountSelectionMode::WhenExhausted
                    };
                    self.projection.settings_providers.quota_epoch = self
                        .projection
                        .settings_providers
                        .quota_epoch
                        .wrapping_add(1);
                    let epoch = self.projection.settings_providers.quota_epoch;
                    self.projection
                        .settings_providers
                        .account_mode_pending
                        .insert(p.provider_id.clone(), epoch);
                    self.controller
                        .set_account_selection_mode(p.provider_id.clone(), mode, epoch);
                    cx.notify();
                }
                return true;
            }
            for c in &p.credentials {
                if quota_identifier(&p.provider_id, &c.credential_id, "refresh") == identifier {
                    self.refresh_account_quota(p.provider_id.clone(), c.credential_id.clone(), cx);
                    return true;
                }
            }
        }
        false
    }
}

#[test]
fn quota_reset_duration_keeps_minute_precision() {
    for (minutes, expected) in [
        (0, "0m"),
        (59, "59m"),
        (60, "1h"),
        (61, "1h 1m"),
        (1440, "1d"),
        (6995, "4d 20h 35m"),
    ] {
        assert_eq!(quota_duration(minutes), expected);
    }
}
