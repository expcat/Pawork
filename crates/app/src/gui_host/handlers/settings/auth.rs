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
    cancel_oauth_flight_if_present, flight_begin, flight_end, iso8601_utc, now_millis, settings_data,
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
    let AppCommand::AuthSetApiKey {
        provider_id,
        api_key,
    } = command
    else {
        unreachable!("auth_set_api_key handler receives AuthSetApiKey")
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
    let outcome = verify_and_store(adapter, preset, provider_id, candidate).await;
    flight_end(&adapter.auth_flights, id, &flight);
    match outcome {
        Ok(masked) => {
            adapter.bus.publish_provider_auth(
                adapter.instance.clone(),
                provider_id,
                AuthChangeState::Succeeded {
                    method: "api_key".into(),
                    masked_credential: masked.clone(),
                },
            );
            Ok(AppResponse::Data(json!({
                "provider_id": id,
                "method": "api_key",
                "masked_credential": masked,
                "verified_at": iso8601_utc(now_millis()),
            })))
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
) -> Result<String, GuiHostError> {
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
        channel_config.http.proxy = core.config().proxy_url.clone();
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
    let stored = pawork_auth::store_default_api_key(backend.as_ref(), provider_id, candidate)
        .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
    // 替换语义（SET-4 A3）：一切换认证方式 = 替换连接。声明 oauth 的通道
    // 写入 api key 后移除旧 OAuth 条目；删除失败 fail-closed 上报，不静默。
    if preset.auth_methods.contains(&"oauth") {
        pawork_auth::delete_default_oauth_token(backend.as_ref(), provider_id)
            .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
    }
    Ok(stored.masked.as_str().to_string())
}

pub(crate) async fn auth_start(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::AuthStart { provider_id, flow } = command else {
        unreachable!("auth_start handler receives AuthStart")
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
    let (backend, http) = {
        let core = adapter.core.read().await;
        (core.auth_backend().clone(), core.http_client().clone())
    };
    let bus = adapter.bus.clone();
    let instance = adapter.instance.clone();
    let flights = adapter.auth_flights.clone();
    let provider = provider_id.clone();
    tokio::spawn(async move {
        let outcome = tokio::select! {
            result = crate::auth::oauth_finish(login, backend.as_ref(), &http, OAUTH_WAIT_TIMEOUT) => {
                match result {
                    Ok(stored) => AuthChangeState::Succeeded {
                        method: "oauth".into(),
                        masked_credential: stored.masked.as_str().to_string(),
                    },
                    Err(AppError::Auth(AuthError::ExpiredToken)) => AuthChangeState::Expired,
                    Err(error) => AuthChangeState::Failed { error: error.to_string() },
                }
            }
            // 取消路径由 auth_cancel 负责移除 flight 并下发 Cancelled。
            () = flight.token.cancelled() => return,
        };
        flight_end(&flights, provider.as_str(), &flight);
        bus.publish_provider_auth(instance, &provider, outcome);
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
    let AppCommand::AuthRemove { provider_id } = command else {
        unreachable!("auth_remove handler receives AuthRemove")
    };
    let id = provider_id.as_str();
    let core = adapter.core.read().await;
    let backend = core.auth_backend();
    // SET-4 A3：按 auth_methods 数据判定；双认证通道（如 xai）依次清理
    // OAuth 与 api key 条目（删除幂等），无任何存储凭证时 not_found。
    let methods = channels::first_party_channel(id)
        .map(|channel| channel.auth_methods())
        .unwrap_or(&["api_key"]);
    // env 凭证无法从 Host 侧移除；命中时仍继续清理已存条目（SET-4 审查修复：
    // 双认证通道 env + 已存 OAuth 时应删掉 OAuth），仅最终无可删项时按 env 语义上报。
    let env_credential_active = methods.contains(&"api_key")
        && matches!(
            pawork_auth::resolve_provider_credential(backend.as_ref(), id),
            Ok(pawork_auth::CredentialSource::EnvFallback(_))
        );
    let mut removed = false;
    if methods.contains(&"oauth") {
        match pawork_auth::load_default_oauth_meta(backend.as_ref(), provider_id) {
            Ok(Some(_)) => {
                pawork_auth::delete_default_oauth_token(backend.as_ref(), provider_id)
                    .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
                removed = true;
            }
            Ok(None) => {}
            Err(error) => return Err(GuiHostAdapter::app_error(error.into())),
        }
    }
    if methods.contains(&"api_key") {
        match pawork_auth::resolve_provider_credential(backend.as_ref(), id) {
            Ok(pawork_auth::CredentialSource::AuthFile(_)) => {
                pawork_auth::delete_default_api_key(backend.as_ref(), provider_id)
                    .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
                removed = true;
            }
            Err(error) => return Err(GuiHostAdapter::app_error(error.into())),
            Ok(_) => {}
        }
    }
    if !removed {
        if env_credential_active {
            return Err(GuiHostAdapter::host_error(
                "unsupported",
                format!(
                    "provider {id} credential comes from PAWORK_API_KEY_* env; unset the variable to disconnect"
                ),
            ));
        }
        return Err(GuiHostAdapter::host_error(
            "not_found",
            format!("provider {id} has no stored credential"),
        ));
    }
    adapter.bus.publish_provider_auth(
        adapter.instance.clone(),
        provider_id,
        AuthChangeState::Removed,
    );
    Ok(AppResponse::Data(json!({
        "provider_id": id,
        "removed": true,
    })))
}
