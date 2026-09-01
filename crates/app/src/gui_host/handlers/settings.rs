//! SET-2 Host Settings 门面（ADR-046）：provider_auth_status 查询与五个
//! 认证 / 默认项命令。Secret 红线：api_key 明文只在 handler 栈上与验证
//! 请求的 Authorization 头中短暂停留，绝不进入 tracing / 事件 / ledger。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pawork_auth::{AuthError, CredentialSource};
use pawork_domain::{CancellationToken, ProviderId};
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppQuery, AppResponse, AuthChangeState};
use pawork_providers::ReasoningProtector;
use serde_json::{json, Value};

use crate::gui_server::GuiHostError;
use crate::provider_assembly::{assemble_provider, assemble_registry, channel_protocol};
use crate::{channels, AppCore, AppError, OAuthLogin};

use super::super::GuiHostAdapter;

/// 认证单飞注册表（按 provider_id；Arc 身份用于安全移除自己的 flight）。
pub(super) type AuthFlights = Arc<Mutex<HashMap<String, Arc<CancellationToken>>>>;

/// OAuth 授权等待上限：设备码 / PKCE 回调超时后下发 Expired / Failed。
const OAUTH_WAIT_TIMEOUT: Duration = Duration::from_secs(600);
/// 单通道目录探测上限（与 models_overview 的探测窗口一致）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
/// API key 验证请求超时。
const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);
/// 固定回退目录的快照标签：静态目录随 pawork-providers 版本发布。
const STATIC_CATALOG_LABEL: &str = concat!("pawork-providers/", env!("CARGO_PKG_VERSION"));

fn flight_begin(
    flights: &AuthFlights,
    provider: &str,
) -> Result<Arc<CancellationToken>, GuiHostError> {
    let mut flights = flights.lock().expect("auth flights poisoned");
    if flights.contains_key(provider) {
        return Err(GuiHostAdapter::host_error(
            "busy",
            format!("an auth operation for provider {provider} is already in progress"),
        ));
    }
    let token = Arc::new(CancellationToken::new());
    flights.insert(provider.to_string(), Arc::clone(&token));
    Ok(token)
}

/// 仅当注册表中仍是同一 flight 时移除，避免误删后来者。
fn flight_end(flights: &AuthFlights, provider: &str, token: &Arc<CancellationToken>) {
    let mut flights = flights.lock().expect("auth flights poisoned");
    if flights
        .get(provider)
        .is_some_and(|current| Arc::ptr_eq(current, token))
    {
        flights.remove(provider);
    }
}

fn flight_active(flights: &AuthFlights, provider: &str) -> bool {
    flights
        .lock()
        .expect("auth flights poisoned")
        .contains_key(provider)
}

/// unix 毫秒 → UTC ISO-8601（无时区依赖；仅供 wire 展示字段使用）。
fn iso8601_utc(millis: u64) -> String {
    let secs = millis / 1000;
    let days = (secs / 86_400) as i64;
    let time = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

/// Howard Hinnant civil_from_days：epoch 天数 → (年, 月, 日)。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn endpoint_label(core: &AppCore, channel: &channels::FirstPartyChannel) -> String {
    core.config()
        .providers
        .iter()
        .find(|provider| provider.id == channel.id)
        .and_then(|provider| provider.base_url.clone())
        .unwrap_or_else(|| channel.default_base_url.to_string())
}

fn auth_state(
    core: &AppCore,
    flights: &AuthFlights,
    channel: &channels::FirstPartyChannel,
) -> Value {
    if flight_active(flights, channel.id) {
        return json!({ "type": "connecting" });
    }
    let provider = ProviderId::new(channel.id);
    match channel.kind {
        channels::ChannelKind::ApiKey => {
            match pawork_auth::resolve_provider_credential(core.auth_backend().as_ref(), channel.id)
            {
                Ok(CredentialSource::AuthFile(stored)) => json!({
                    "type": "connected",
                    "method": "api_key",
                    "masked_credential": stored.masked.as_str(),
                }),
                // env 命中同样是可运行连接，但按脱敏规则不展示任何值片段。
                Ok(CredentialSource::EnvFallback(_)) => json!({
                    "type": "connected",
                    "method": "api_key",
                    "masked_credential": Value::Null,
                }),
                Ok(CredentialSource::None) => json!({ "type": "none" }),
                Err(error) => json!({ "type": "error", "message": error.to_string() }),
            }
        }
        channels::ChannelKind::ChatGptOAuth | channels::ChannelKind::XaiOAuth => {
            match pawork_auth::load_default_oauth_meta(core.auth_backend().as_ref(), &provider) {
                Ok(Some(meta)) => json!({
                    "type": "connected",
                    "method": "oauth",
                    "masked_credential": meta.masked.as_str(),
                }),
                Ok(None) => json!({ "type": "none" }),
                Err(error) => json!({ "type": "error", "message": error.to_string() }),
            }
        }
    }
}

/// 目录三态：探测成功 remote / 探测失败但有静态条目 fixed_fallback / 否则
/// unavailable（复用 models_overview 的装配 + 探测机制，不新增缓存）。
async fn catalog_state(core: &AppCore, channel: &channels::FirstPartyChannel) -> Value {
    let id = ProviderId::new(channel.id);
    let protocol = match channel_protocol(Some(channel), core.config(), channel.id) {
        Ok(protocol) => protocol,
        Err(error) => {
            return json!({
                "type": "unavailable",
                "error": error.to_string(),
                "fetched_at": Value::Null,
            })
        }
    };
    let registry = assemble_registry(core.config(), &id, protocol, Some(channel));
    let has_static = registry.list().iter().any(|entry| entry.provider == id);
    let assembled = assemble_provider(
        core.config(),
        &id,
        core.auth_backend(),
        false,
        Arc::clone(&core.reasoning_protector) as Arc<dyn ReasoningProtector>,
    )
    .await;
    let probe_error = match assembled {
        Ok(assembled) => {
            match tokio::time::timeout(
                PROBE_TIMEOUT,
                assembled.adapter.list_models(assembled.credential.as_ref()),
            )
            .await
            {
                Ok(Ok(_)) => {
                    return json!({
                        "type": "remote",
                        "fetched_at": iso8601_utc(now_millis()),
                    })
                }
                Ok(Err(error)) => error.to_string(),
                Err(_) => "runtime model probe timed out".to_string(),
            }
        }
        Err(error) => error.to_string(),
    };
    if has_static {
        json!({
            "type": "fixed_fallback",
            "snapshot_label": STATIC_CATALOG_LABEL,
            "fetched_at": Value::Null,
        })
    } else {
        json!({
            "type": "unavailable",
            "error": probe_error,
            "fetched_at": Value::Null,
        })
    }
}

fn now_millis() -> u64 {
    pawork_engine::now_timestamp().as_unix_millis()
}

pub(crate) async fn provider_auth_status(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::ProviderAuthStatus { provider_id } = query else {
        unreachable!("provider_auth_status handler receives ProviderAuthStatus")
    };
    let core = adapter.core.read().await;
    let selected: Vec<&channels::FirstPartyChannel> = channels::FIRST_PARTY_CHANNELS
        .iter()
        .filter(|channel| {
            provider_id
                .as_ref()
                .map(|id| id.as_str() == channel.id)
                .unwrap_or(true)
        })
        .collect();
    if provider_id.is_some() && selected.is_empty() {
        let id = provider_id
            .as_ref()
            .expect("checked some")
            .as_str()
            .to_string();
        return Err(GuiHostAdapter::host_error(
            "unknown_provider",
            format!("provider {id} is not a first-party channel"),
        ));
    }
    let probes = selected.iter().map(|channel| catalog_state(&core, channel));
    let catalog_states = futures::future::join_all(probes).await;
    let providers: Vec<Value> = selected
        .iter()
        .zip(catalog_states)
        .map(|(channel, catalog)| {
            json!({
                "provider_id": channel.id,
                "display_name": channel.display_name,
                "endpoint_label": endpoint_label(&core, channel),
                "auth_methods": channel.auth_methods(),
                "auth": auth_state(&core, &adapter.auth_flights, channel),
                "catalog": catalog,
            })
        })
        .collect();
    Ok(AppResponse::Data(json!({ "providers": providers })))
}

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
    let token = flight_begin(&adapter.auth_flights, id)?;
    let outcome = verify_and_store(adapter, preset, &provider_id, candidate).await;
    flight_end(&adapter.auth_flights, id, &token);
    match outcome {
        Ok(masked) => {
            adapter.bus.publish_provider_auth(
                adapter.instance.clone(),
                &provider_id,
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
                &provider_id,
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
    let token = flight_begin(&adapter.auth_flights, id)?;
    let login = {
        let core = adapter.core.read().await;
        core.oauth_begin(id).await
    };
    let login = match login {
        Ok(login) => login,
        Err(error) => {
            flight_end(&adapter.auth_flights, id, &token);
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
        &provider_id,
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
            () = token.cancelled() => return,
        };
        flight_end(&flights, provider.as_str(), &token);
        bus.publish_provider_auth(instance, &provider, outcome);
    });

    Ok(AppResponse::Data(json!({
        "verification_url": verification_url,
        "user_code": user_code,
        "expires_at": expires_at,
    })))
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
    // 下发终态，既违背 Cancelled 语义又破坏单飞守卫。
    if flight_active(&adapter.auth_flights, id)
        && channels::first_party_channel(id)
            .is_some_and(|channel| channel.kind == channels::ChannelKind::ApiKey)
    {
        return Err(GuiHostAdapter::host_error(
            "unsupported",
            format!(
                "auth_cancel only cancels OAuth waits; provider {id} api_key verification cannot be cancelled"
            ),
        ));
    }
    let token = adapter
        .auth_flights
        .lock()
        .expect("auth flights poisoned")
        .remove(id);
    if let Some(token) = token {
        token.cancel();
        adapter.bus.publish_provider_auth(
            adapter.instance.clone(),
            &provider_id,
            AuthChangeState::Cancelled,
        );
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
    let is_oauth = channels::first_party_channel(id)
        .map(|channel| channel.kind != channels::ChannelKind::ApiKey)
        .unwrap_or(false);
    if is_oauth {
        match pawork_auth::load_default_oauth_meta(backend.as_ref(), &provider_id) {
            Ok(Some(_)) => {
                pawork_auth::delete_default_oauth_token(backend.as_ref(), &provider_id)
                    .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
            }
            Ok(None) => {
                return Err(GuiHostAdapter::host_error(
                    "not_found",
                    format!("provider {id} has no stored OAuth credential"),
                ));
            }
            Err(error) => return Err(GuiHostAdapter::app_error(error.into())),
        }
    } else {
        match pawork_auth::resolve_provider_credential(backend.as_ref(), id) {
            Ok(CredentialSource::AuthFile(_)) => {
                pawork_auth::delete_default_api_key(backend.as_ref(), &provider_id)
                    .map_err(|error| GuiHostAdapter::app_error(error.into()))?;
            }
            Ok(CredentialSource::EnvFallback(_)) => {
                return Err(GuiHostAdapter::host_error(
                    "unsupported",
                    format!(
                        "provider {id} credential comes from PAWORK_API_KEY_* env; unset the variable to disconnect"
                    ),
                ));
            }
            Ok(CredentialSource::None) => {
                return Err(GuiHostAdapter::host_error(
                    "not_found",
                    format!("provider {id} has no stored credential"),
                ));
            }
            Err(error) => return Err(GuiHostAdapter::app_error(error.into())),
        }
    }
    adapter.bus.publish_provider_auth(
        adapter.instance.clone(),
        &provider_id,
        AuthChangeState::Removed,
    );
    Ok(AppResponse::Data(json!({
        "provider_id": id,
        "removed": true,
    })))
}

pub(crate) async fn set_default_model(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetDefaultModel {
        provider_id,
        model_id,
    } = command
    else {
        unreachable!("set_default_model handler receives SetDefaultModel")
    };
    let id = provider_id.as_str();
    {
        let core = adapter.core.read().await;
        let known = channels::is_first_party(id)
            || core
                .config()
                .providers
                .iter()
                .any(|provider| provider.id == id);
        if !known {
            return Err(GuiHostAdapter::host_error(
                "unknown_provider",
                format!("provider {id} is unknown"),
            ));
        }
        let overview = core.models_overview().await;
        let runnable = overview
            .iter()
            .any(|entry| entry.provider.as_str() == id && entry.id.as_str() == model_id);
        if !runnable {
            return Err(GuiHostAdapter::host_error(
                "unknown_model",
                format!("model {model_id} is not in the runnable catalog of provider {id}"),
            ));
        }
    }
    let path = pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "global config directory is not available on this platform",
        )
    })?;
    pawork_workspace::config::write_default_model_pair(&path, id, model_id)
        .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
    Ok(AppResponse::Data(json!({
        "provider_id": id,
        "model_id": model_id,
    })))
}
