//! Go account quota: one authenticated read, with no local-ledger inference.

use pawork_auth::{
    ApiKeyCredential, ProviderAccount, ProviderAccountKind, ProviderAccountSelectionMode,
    SecretBackend,
};
use pawork_domain::{CancellationToken, ProviderError, ProviderErrorKind, Timestamp};
use pawork_protocol::{
    mask_credential_hint, QuotaAdapterKind, QuotaConfidence, QuotaFailureView, QuotaMeasure,
    QuotaOverviewQuery, QuotaOverviewView, QuotaProvenanceView, QuotaReset, QuotaScopeView,
    QuotaSnapshotView, QuotaUnit, QuotaValues, QuotaWindow, WindowReadEntry, WindowReadView,
};
use pawork_providers::{ApiKeyChannelConfig, GoUsageWindow};
use pawork_workspace::config::PaworkConfig;

use crate::{AppCore, AppError};

const WINDOWS: [QuotaWindow; 3] = [
    QuotaWindow::Rolling5h,
    QuotaWindow::Weekly,
    QuotaWindow::Monthly,
];
const MAX_AGE_MS: u64 = 30_000;

pub(crate) fn is_account_query(query: &QuotaOverviewQuery) -> bool {
    query.credential_id.is_some() || query.unit == Some(QuotaUnit::Percent)
}

fn invalid(detail: &str) -> AppError {
    AppError::ControlPlane(detail.into())
}

/// The request identity, rather than the currently selected account, owns this read.
pub(crate) async fn account_quota(
    config: &PaworkConfig,
    backend: &dyn SecretBackend,
    query: &QuotaOverviewQuery,
    cancel: CancellationToken,
) -> Result<QuotaOverviewView, AppError> {
    if !query.is_default_scope()
        || query.model_id.is_some()
        || query.unit != Some(QuotaUnit::Percent)
    {
        return Err(invalid(
            "account quota requires the local account, no model, and percent units",
        ));
    }
    let provider = query
        .provider_id
        .as_ref()
        .ok_or_else(|| invalid("provider_id is required"))?;
    let credential_id = query
        .credential_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid("credential_id is required"))?;
    if provider.as_str() != "opencode-go" {
        return Err(invalid("account quota is unavailable for this provider"));
    }
    let inventory = pawork_auth::list_provider_accounts(backend, provider)?;
    let account = inventory
        .accounts
        .iter()
        .find(|account| account.credential_id == credential_id)
        .ok_or_else(|| invalid("account not found for provider"))?;
    if account.kind != ProviderAccountKind::ApiKey {
        return Err(invalid("Go quota requires a stored API key account"));
    }
    let preset = crate::channels::api_key_channel(provider.as_str())
        .ok_or_else(|| invalid("quota provider is unavailable"))?;
    let mut channel = ApiKeyChannelConfig::new(preset)?;
    if let Some(base) = config
        .providers
        .iter()
        .find(|item| item.id == provider.as_str())
        .and_then(|item| item.base_url.as_ref())
    {
        channel = channel.with_base_url(base);
    }
    channel.http.proxy = crate::provider_assembly::provider_proxy(config, provider.as_str());
    channel = channel.with_request_timeout(std::time::Duration::from_secs(10));
    let endpoint =
        reqwest::Url::parse(&format!("{}/usage", channel.base_url.trim_end_matches('/')))
            .ok()
            .map(|mut url| {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.set_query(None);
                url.set_fragment(None);
                url.to_string()
            });
    let credential = ApiKeyCredential::from_stored(account.stored.clone())?.resolve(backend)?;
    let fetched_at = pawork_engine::now_timestamp();
    let result = pawork_providers::fetch_go_usage(channel, &credential, cancel.clone()).await;
    if cancel.is_cancelled() {
        return Err(ProviderError::cancelled("account quota cancelled").into());
    }
    // Replaced/deleted credentials must not acquire the old request's readings.
    if pawork_auth::provider_accounts_revision(backend, provider)?.unwrap_or(0)
        != inventory.revision
    {
        return Err(invalid("account changed during quota refresh"));
    }
    let scope = QuotaScopeView {
        tenant_id: query.tenant_id.clone(),
        account_id: query.account_id.clone(),
        provider_id: provider.clone(),
        credential_hint: mask_credential_hint(credential_id),
        model_id: None,
    };
    let windows = if query.windows.is_empty() {
        WINDOWS.to_vec()
    } else {
        query.windows.clone()
    };
    let windows = windows
        .into_iter()
        .map(|window| {
            let read = if !WINDOWS.contains(&window) {
                WindowReadView::Failed {
                    failures: vec![QuotaFailureView {
                        adapter_kind: None,
                        error_code: "unsupported".into(),
                        detail: "Go does not expose an overall quota window".into(),
                        retry_after_ms: None,
                    }],
                }
            } else {
                let value = result.as_ref().and_then(|usage| match window {
                    QuotaWindow::Rolling5h => usage.rolling.as_ref(),
                    QuotaWindow::Weekly => usage.weekly.as_ref(),
                    QuotaWindow::Monthly => usage.monthly.as_ref(),
                    QuotaWindow::Overall => unreachable!(),
                });
                match value {
                    Ok(GoUsageWindow {
                        used_percent,
                        resets_at,
                    }) => WindowReadView::Ok {
                        snapshot: Box::new(QuotaSnapshotView {
                            scope: scope.clone(),
                            window,
                            unit: QuotaUnit::Percent,
                            values: QuotaValues {
                                used: QuotaMeasure::Exact(*used_percent),
                                limit: QuotaMeasure::Exact(100),
                                remaining: QuotaMeasure::Exact(100 - used_percent),
                            },
                            reset: QuotaReset::Absolute {
                                at: *resets_at,
                                uncertain: false,
                            },
                            confidence: QuotaConfidence::Exact,
                            provenance: QuotaProvenanceView {
                                adapter_kind: QuotaAdapterKind::ApiKeyApi,
                                source: "opencode-go/usage".into(),
                                endpoint: endpoint.clone(),
                                fetched_at,
                                observed_at: None,
                                stale: false,
                            },
                            served_stale: false,
                        }),
                        failures: vec![],
                    },
                    Err(error) => WindowReadView::Failed {
                        failures: vec![quota_failure(error)],
                    },
                }
            };
            WindowReadEntry { window, read }
        })
        .collect();
    Ok(QuotaOverviewView {
        scope,
        windows,
        generated_at: pawork_engine::now_timestamp(),
        from_cache: false,
    })
}

fn quota_failure(error: &ProviderError) -> QuotaFailureView {
    let code = match error.kind {
        ProviderErrorKind::Authentication => "unauthorized",
        ProviderErrorKind::Authorization => "forbidden",
        ProviderErrorKind::RateLimited | ProviderErrorKind::QuotaExceeded => "rate_limited",
        ProviderErrorKind::Timeout => "timeout",
        ProviderErrorKind::Cancelled => "cancelled",
        ProviderErrorKind::InvalidRequest | ProviderErrorKind::MalformedResponse => "parse",
        _ => "unavailable",
    };
    QuotaFailureView {
        adapter_kind: Some(QuotaAdapterKind::ApiKeyApi),
        error_code: code.into(),
        detail: format!("Go quota {code}"),
        retry_after_ms: error.retry_after_ms,
    }
}

/// A score requires all three complete, fresh, authoritative windows.
fn headroom(view: &QuotaOverviewView, now: Timestamp) -> Option<u64> {
    let mut remaining = 100;
    for window in WINDOWS {
        let entry = view.windows.iter().find(|entry| entry.window == window)?;
        let WindowReadView::Ok { snapshot, failures } = &entry.read else {
            return None;
        };
        let age = now
            .as_unix_millis()
            .checked_sub(snapshot.provenance.fetched_at.as_unix_millis())?;
        let QuotaReset::Absolute {
            at,
            uncertain: false,
        } = snapshot.reset
        else {
            return None;
        };
        if age > MAX_AGE_MS
            || at <= now
            || snapshot.served_stale
            || snapshot.provenance.stale
            || !failures.is_empty()
            || snapshot.unit != QuotaUnit::Percent
            || snapshot.confidence != QuotaConfidence::Exact
            || snapshot.provenance.adapter_kind != QuotaAdapterKind::ApiKeyApi
        {
            return None;
        }
        let QuotaMeasure::Exact(value @ 0..=100) = snapshot.values.remaining else {
            return None;
        };
        remaining = remaining.min(value);
    }
    Some(remaining)
}

pub(crate) struct AccountSelectionChange {
    pub previous_id: String,
    pub account: ProviderAccount,
}

impl AppCore {
    /// Shared GUI/CLI Run boundary. Network failures never silently change identity.
    pub(crate) async fn select_account_for_run(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<AccountSelectionChange>, AppError> {
        if self.provider_id.as_str() != "opencode-go" || cancel.is_cancelled() {
            return Ok(None);
        }
        let inventory =
            pawork_auth::list_provider_accounts(self.backend.as_ref(), &self.provider_id)?;
        if inventory.selection_mode != ProviderAccountSelectionMode::WhenExhausted {
            return Ok(None);
        }
        let Some(selected) = inventory.selected_credential_id.as_deref() else {
            return Ok(None);
        };
        let query = |credential_id: &str| QuotaOverviewQuery {
            provider_id: Some(self.provider_id.clone()),
            credential_id: Some(credential_id.into()),
            unit: Some(QuotaUnit::Percent),
            ..QuotaOverviewQuery::default_local()
        };
        let Ok(current) = account_quota(
            &self.config,
            self.backend.as_ref(),
            &query(selected),
            cancel.clone(),
        )
        .await
        else {
            return Ok(None);
        };
        if headroom(&current, pawork_engine::now_timestamp()) != Some(0) {
            return Ok(None);
        }
        let mut best: Option<(u64, &ProviderAccount, QuotaOverviewView)> = None;
        for account in &inventory.accounts {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            if account.credential_id == selected || account.kind != ProviderAccountKind::ApiKey {
                continue;
            }
            let Ok(view) = account_quota(
                &self.config,
                self.backend.as_ref(),
                &query(&account.credential_id),
                cancel.clone(),
            )
            .await
            else {
                continue;
            };
            if let Some(score) =
                headroom(&view, pawork_engine::now_timestamp()).filter(|score| *score > 0)
            {
                if best
                    .as_ref()
                    .is_none_or(|(previous, _, _)| score > *previous)
                {
                    best = Some((score, account, view));
                }
            }
        }
        let Some((_, account, view)) = best else {
            return Ok(None);
        };
        let now = pawork_engine::now_timestamp();
        if cancel.is_cancelled()
            || headroom(&current, now) != Some(0)
            || headroom(&view, now).is_none_or(|score| score == 0)
        {
            return Ok(None);
        }
        if !pawork_auth::select_provider_account_if_revision(
            self.backend.as_ref(),
            &self.provider_id,
            inventory.revision,
            selected,
            &account.credential_id,
        )? {
            return Ok(None);
        }
        Ok(Some(AccountSelectionChange {
            previous_id: selected.into(),
            account: account.clone(),
        }))
    }
}
