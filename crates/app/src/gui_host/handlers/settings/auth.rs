use std::time::Duration;

use pawork_auth::AuthError;
use pawork_domain::ProviderId;
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppResponse, AuthChangeState, AuthStartData,
};
use serde_json::json;

use crate::gui_host::GuiHostAdapter;
use crate::gui_server::GuiHostError;
use crate::{channels, AppError, OAuthLogin};

use super::{
    cancel_oauth_flight_if_present, flight_begin, flight_end, iso8601_utc, now_millis,
    settings_data,
};

/// OAuth 授权等待上限：设备码 / PKCE 回调超时后下发 Expired / Failed。
const OAUTH_WAIT_TIMEOUT: Duration = Duration::from_secs(600);
/// API key 验证请求超时。
const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) async fn auth_set_api_key(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let (provider_id, api_key, display_name) = match command {
        AppCommand::AuthSetApiKey {
            provider_id,
            api_key,
        } => (provider_id, api_key, None),
        AppCommand::AuthAccountAddApiKey {
            provider_id,
            api_key,
            display_name,
        } => {
            pawork_auth::validate_account_name(display_name)
                .map_err(|e| GuiHostAdapter::app_error(e.into()))?;
            (provider_id, api_key, Some(display_name.as_str()))
        }
        _ => unreachable!("API key handler"),
    };
    let id = provider_id.as_str();
    let candidate = api_key.as_str().trim();
    if candidate.is_empty() {
        return Err(GuiHostAdapter::host_error(
            "invalid_secret",
            "API key is empty",
        ));
    }
    let preset = channels::api_key_channel(id)
        .filter(|preset| pawork_providers::is_enabled(*preset))
        .ok_or_else(|| {
            GuiHostAdapter::host_error(
                "unsupported",
                format!("provider {id} is unknown or does not declare api_key auth"),
            )
        })?;
    let flight = flight_begin(&adapter.auth_flights, id, false)?;
    let outcome = verify_and_store(adapter, preset, provider_id, candidate, display_name).await;
    flight_end(&adapter.auth_flights, id, &flight);
    match outcome {
        Ok((masked, account)) => {
            adapter.bus.publish_provider_auth(
                adapter.instance.clone(),
                provider_id,
                AuthChangeState::Succeeded {
                    method: "api_key".into(),
                    masked_credential: masked.clone(),
                },
            );
            let mut data = json!({"provider_id": id, "method": "api_key", "masked_credential": masked, "verified_at": iso8601_utc(now_millis())});
            if let Some((account, selected)) = account {
                data["credential_id"] = json!(account.credential_id);
                data["display_name"] = json!(account.display_name);
                data["selected"] = json!(selected);
            }
            Ok(AppResponse::Data(data))
        }
        Err(error) => {
            adapter.bus.publish_provider_auth(
                adapter.instance.clone(),
                provider_id,
                AuthChangeState::Failed {
                    error: error.message.clone(),
                },
            );
            Err(error)
        }
    }
}

/// verify-then-replace：先内存验证候选 key（不持久化），成功才原子替换。
/// 锁内只取 config 快照与 backend，验证前放锁：验证网络请求最长
/// VERIFY_TIMEOUT，不得跨网络等待持读锁阻塞写操作（与 oauth_finish 纪律一致）。
async fn verify_and_store(
    adapter: &GuiHostAdapter,
    preset: &'static pawork_providers::ChannelPreset,
    provider_id: &ProviderId,
    candidate: &str,
    display_name: Option<&str>,
) -> Result<(String, Option<(pawork_auth::ProviderAccount, bool)>), GuiHostError> {
    let (channel_config, backend) = {
        let core = adapter.core.read().await;
        let base_override = core
            .config()
            .providers
            .iter()
            .find(|provider| provider.id == provider_id.as_str())
            .and_then(|provider| provider.base_url.clone());
        let mut channel_config = pawork_providers::ApiKeyChannelConfig::new(preset)
            .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
        channel_config.http.proxy =
            crate::provider_assembly::provider_proxy(core.config(), provider_id.as_str());
        if let Some(base_url) = base_override {
            channel_config = channel_config.with_base_url(base_url);
        }
        (
            channel_config.with_request_timeout(VERIFY_TIMEOUT),
            core.auth_backend().clone(),
        )
    };
    pawork_providers::verify_api_key(channel_config, candidate)
        .await
        .map_err(|error| {
            GuiHostAdapter::host_error(
                "auth_verify",
                format!("API key verification failed: {error}"),
            )
        })?;
    if let Some(name) = display_name {
        let account = pawork_auth::add_api_key_account(
            backend.as_ref(),
            provider_id,
            name,
            candidate,
            crate::auth::activate_first_account(provider_id.as_str()),
        )
        .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
        let selected = crate::auth::effective_provider_account(backend.as_ref(), provider_id)
            .map_err(GuiHostAdapter::app_error)?
            .is_some_and(|effective| effective.credential_id == account.credential_id);
        Ok((
            account.stored.masked.as_str().into(),
            Some((account, selected)),
        ))
    } else {
        let stored = pawork_auth::store_default_api_key(backend.as_ref(), provider_id, candidate)
            .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
        Ok((stored.masked.as_str().into(), None))
    }
}

pub(crate) async fn auth_start(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let (provider_id, flow, display_name) = match command {
        AppCommand::AuthStart { provider_id, flow } => (provider_id, flow, None),
        AppCommand::AuthAccountStart {
            provider_id,
            flow,
            display_name,
        } => {
            let name = pawork_auth::validate_account_name(display_name)
                .map_err(|e| GuiHostAdapter::app_error(e.into()))?;
            (provider_id, flow, Some(name.to_string()))
        }
        _ => unreachable!("OAuth start handler"),
    };
    let id = provider_id.as_str();
    if flow != "oauth" {
        return Err(GuiHostAdapter::host_error(
            "unsupported",
            format!("auth flow {flow} is not supported; use oauth"),
        ));
    }
    let channel = channels::first_party_channel(id).ok_or_else(|| {
        GuiHostAdapter::host_error("unknown_provider", format!("provider {id} is unknown"))
    })?;
    if channel.oauth_preset().is_none() {
        return Err(GuiHostAdapter::host_error(
            "unsupported",
            format!("provider {id} has no OAuth flow; it declares api_key auth"),
        ));
    }
    let (backend, http) = {
        let core = adapter.core.read().await;
        (
            core.auth_backend().clone(),
            crate::provider_assembly::provider_http(core.config(), id)
                .map_err(GuiHostAdapter::app_error)?,
        )
    };
    let flight = flight_begin(&adapter.auth_flights, id, true)?;
    let login = {
        let core = adapter.core.read().await;
        core.oauth_begin(id).await
    };
    let login = match login {
        Ok(login) => login,
        Err(error) => {
            flight_end(&adapter.auth_flights, id, &flight);
            return Err(GuiHostAdapter::app_error(error));
        }
    };
    let (verification_url, user_code, expires_at) = match &login {
        OAuthLogin::Pkce { auth_url, .. } => (auth_url.clone(), None, None),
        OAuthLogin::Device { prompt, .. } => (
            prompt.verification_uri.clone(),
            Some(prompt.user_code.clone()),
            Some(iso8601_utc(
                now_millis().saturating_add(prompt.expires_in.saturating_mul(1000)),
            )),
        ),
    };
    adapter.bus.publish_provider_auth(
        adapter.instance.clone(),
        provider_id,
        AuthChangeState::Pending,
    );

    // 后台等待授权并完成 token 交换；不持 core 锁，进度经 AuthChanged 下发。
    let bus = adapter.bus.clone();
    let instance = adapter.instance.clone();
    let flights = adapter.auth_flights.clone();
    let provider = provider_id.clone();
    tokio::spawn(async move {
        let exchanged = tokio::select! {
            result = crate::auth::oauth_exchange(login, &http, OAUTH_WAIT_TIMEOUT) => result,
            () = flight.token.cancelled() => return,
        };
        // Serialize cancellation with the short persistence step. A cancelled flight
        // must never write tokens later or emit a late success.
        let mut flights = flights.lock().expect("auth flights poisoned");
        if !flights
            .get(provider.as_str())
            .is_some_and(|current| std::sync::Arc::ptr_eq(current, &flight))
        {
            return;
        }
        let outcome = exchanged.and_then(|(_, tokens)| {
            if let Some(name) = display_name {
                pawork_auth::add_oauth_account(
                    backend.as_ref(),
                    &provider,
                    &name,
                    &tokens,
                    crate::auth::activate_first_account(provider.as_str()),
                )
                .map(|account| account.stored)
                .map_err(Into::into)
            } else {
                pawork_auth::store_default_oauth_token(backend.as_ref(), provider.clone(), &tokens)
                    .map_err(Into::into)
            }
        });
        let state = match outcome {
            Ok(stored) => AuthChangeState::Succeeded {
                method: "oauth".into(),
                masked_credential: stored.masked.as_str().to_string(),
            },
            Err(AppError::Auth(AuthError::ExpiredToken)) => AuthChangeState::Expired,
            Err(error) => AuthChangeState::Failed {
                error: error.to_string(),
            },
        };
        flights.remove(provider.as_str());
        bus.publish_provider_auth(instance, &provider, state);
    });

    Ok(settings_data(AuthStartData {
        verification_url,
        user_code,
        expires_at,
    }))
}

pub(crate) async fn auth_cancel(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::AuthCancel { provider_id } = command else {
        unreachable!("auth_cancel handler receives AuthCancel")
    };
    let id = provider_id.as_str();
    // D3：AuthCancel 只取消「进行中的 OAuth 等待」。api_key 验证是单次
    // 同步请求，无法中途停止；若允许取消移除 flight，验证仍会跑完写盘并
    // 下发终态，既违背 Cancelled 语义又破坏单飞守卫。按活跃 flight 的
    // 种类标记放行（不按通道声明推断）：验证 flight 拒绝且登记保留，
    // OAuth 等待 flight 才移除并下发 Cancelled。
    match cancel_oauth_flight_if_present(&adapter.auth_flights, id) {
        Some(true) => {
            adapter.bus.publish_provider_auth(
                adapter.instance.clone(),
                provider_id,
                AuthChangeState::Cancelled,
            );
        }
        Some(false) => {
            return Err(GuiHostAdapter::host_error(
                "unsupported",
                format!(
                    "auth_cancel only cancels OAuth waits; provider {id} api_key verification cannot be cancelled"
                ),
            ));
        }
        None => {}
    }
    // 无进行中操作时幂等 Accepted、不发事件。
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: None,
    })
}

pub(crate) async fn auth_remove(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let (provider_id, remove_id, select_id, selection_mode) = match command {
        AppCommand::AuthRemove { provider_id } => (provider_id, None, None, None),
        AppCommand::AuthAccountRemove {
            provider_id,
            credential_id,
        } => (provider_id, Some(credential_id.as_str()), None, None),
        AppCommand::AuthAccountSelect {
            provider_id,
            credential_id,
        } => (provider_id, None, Some(credential_id.as_str()), None),
        AppCommand::AuthAccountSetSelectionMode { provider_id, mode } => {
            (provider_id, None, None, Some(*mode))
        }
        _ => unreachable!("account mutation handler"),
    };
    let backend = adapter.core.read().await.auth_backend().clone();
    let id = provider_id.as_str();
    let flight = flight_begin(&adapter.auth_flights, id, false)?;
    let outcome = (|| {
        let inventory = pawork_auth::list_provider_accounts(backend.as_ref(), provider_id)
            .map_err(|e| GuiHostAdapter::app_error(e.into()))?;
        if let Some(mode) = selection_mode {
            let stored_mode = match mode {
                pawork_protocol::ProviderAccountSelectionMode::Manual => {
                    pawork_auth::ProviderAccountSelectionMode::Manual
                }
                pawork_protocol::ProviderAccountSelectionMode::WhenExhausted => {
                    pawork_auth::ProviderAccountSelectionMode::WhenExhausted
                }
            };
            pawork_auth::set_provider_account_selection_mode(
                backend.as_ref(),
                provider_id,
                stored_mode,
            )
            .map_err(|e| GuiHostAdapter::app_error(e.into()))?;
            let after = pawork_auth::list_provider_accounts(backend.as_ref(), provider_id)
                .map_err(|e| GuiHostAdapter::app_error(e.into()))?;
            let state = after
                .selected()
                .map_or(AuthChangeState::Removed, |account| {
                    AuthChangeState::Succeeded {
                        method: account.kind.as_str().into(),
                        masked_credential: account.stored.masked.as_str().into(),
                    }
                });
            adapter
                .bus
                .publish_provider_auth(adapter.instance.clone(), provider_id, state);
            return Ok(AppResponse::Data(
                json!({"provider_id": id, "selection_mode": mode,
                "selected_credential_id": after.selected_credential_id}),
            ));
        }
        if let Some(selected) = select_id {
            let account = inventory
                .accounts
                .iter()
                .find(|account| account.credential_id == selected)
                .ok_or_else(|| GuiHostAdapter::host_error("not_found", "account not found"))?;
            let methods = channels::first_party_channel(id)
                .map(|channel| channel.auth_methods())
                .unwrap_or(&["api_key"]);
            if !methods.contains(&account.kind.as_str()) {
                return Err(GuiHostAdapter::host_error(
                    "unsupported",
                    "account auth method is not supported by this provider",
                ));
            }
            pawork_auth::select_provider_account(backend.as_ref(), provider_id, selected)
                .map_err(|e| GuiHostAdapter::app_error(e.into()))?;
            adapter.bus.publish_provider_auth(
                adapter.instance.clone(),
                provider_id,
                AuthChangeState::Succeeded {
                    method: account.kind.as_str().into(),
                    masked_credential: account.stored.masked.as_str().into(),
                },
            );
            return Ok(AppResponse::Data(
                json!({"provider_id": id, "selected_credential_id": selected}),
            ));
        }
        if let Some(removed) = remove_id {
            let effective = crate::auth::effective_provider_account(backend.as_ref(), provider_id)
                .map_err(GuiHostAdapter::app_error)?;
            pawork_auth::remove_provider_account(
                backend.as_ref(),
                provider_id,
                removed,
                effective
                    .as_ref()
                    .map(|account| account.credential_id.as_str()),
            )
            .map_err(|e| GuiHostAdapter::app_error(e.into()))?;
        } else {
            if inventory.accounts.is_empty() {
                return Err(if crate::auth::activate_first_account(id) {
                    GuiHostAdapter::host_error(
                        "not_found",
                        format!("provider {id} has no stored credential"),
                    )
                } else {
                    GuiHostAdapter::host_error("unsupported", "credential comes from PAWORK_API_KEY_* env; unset the variable to disconnect")
                });
            }
            pawork_auth::remove_all_provider_accounts(backend.as_ref(), provider_id)
                .map_err(|e| GuiHostAdapter::app_error(e.into()))?;
        }
        let remaining = if remove_id.is_some() {
            crate::auth::effective_provider_account(backend.as_ref(), provider_id)
                .map_err(GuiHostAdapter::app_error)?
        } else {
            None
        };
        let state = remaining
            .as_ref()
            .map_or(AuthChangeState::Removed, |account| {
                AuthChangeState::Succeeded {
                    method: account.kind.as_str().into(),
                    masked_credential: account.stored.masked.as_str().into(),
                }
            });
        adapter
            .bus
            .publish_provider_auth(adapter.instance.clone(), provider_id, state);
        let mut data = json!({"provider_id": id, "removed": true});
        if let Some(removed) = remove_id {
            data["credential_id"] = json!(removed);
            data["selected_credential_id"] = json!(remaining.map(|account| account.credential_id));
        }
        Ok(AppResponse::Data(data))
    })();
    flight_end(&adapter.auth_flights, id, &flight);
    outcome
}
