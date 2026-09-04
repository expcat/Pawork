use std::sync::Arc;

use pawork_auth::CredentialSource;
use pawork_domain::ProviderId;
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppQuery, AppResponse, DefaultModelPair, ProviderAuthState,
    ProviderAuthStatusData, ProviderAuthStatusEntry, ProviderCatalogState,
};
use pawork_providers::ReasoningProtector;

use crate::gui_host::GuiHostAdapter;
use crate::gui_server::GuiHostError;
use crate::provider_assembly::{assemble_provider, assemble_registry, channel_protocol};
use crate::{channels, AppCore};

use super::{flight_active, iso8601_utc, now_millis, settings_data, AuthFlights};

/// 单通道目录探测上限（与 models_overview 的探测窗口一致）。
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);
/// 固定回退目录的快照标签：静态目录随 pawork-providers 版本发布。
const STATIC_CATALOG_LABEL: &str = concat!("pawork-providers/", env!("CARGO_PKG_VERSION"));

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
) -> ProviderAuthState {
    if flight_active(flights, channel.id) {
        return ProviderAuthState::Connecting;
    }
    let provider = ProviderId::new(channel.id);
    // SET-4 A3：按 auth_methods 数据判定（不按 kind 猜）。声明 api_key 的
    // 通道先查 api key 凭证，再查 OAuth meta——双认证通道显示 method 与
    // 实际存储凭证一致。
    let methods = channel.auth_methods();
    if methods.contains(&"api_key") {
        match pawork_auth::resolve_provider_credential(core.auth_backend().as_ref(), channel.id) {
            Ok(CredentialSource::AuthFile(stored)) => {
                return ProviderAuthState::Connected {
                    method: "api_key".into(),
                    masked_credential: Some(stored.masked.as_str().to_string()),
                };
            }
            // env 命中同样是可运行连接，但按脱敏规则不展示任何值片段。
            Ok(CredentialSource::EnvFallback(_)) => {
                return ProviderAuthState::Connected {
                    method: "api_key".into(),
                    masked_credential: None,
                };
            }
            Ok(CredentialSource::None) => {}
            Err(error) => {
                return ProviderAuthState::Error {
                    message: error.to_string(),
                }
            }
        }
    }
    if methods.contains(&"oauth") {
        return match pawork_auth::load_default_oauth_meta(core.auth_backend().as_ref(), &provider) {
            Ok(Some(meta)) => ProviderAuthState::Connected {
                method: "oauth".into(),
                masked_credential: Some(meta.masked.as_str().to_string()),
            },
            Ok(None) => ProviderAuthState::None,
            Err(error) => ProviderAuthState::Error {
                message: error.to_string(),
            },
        };
    }
    ProviderAuthState::None
}

/// 目录三态：探测成功 remote / 探测失败但有静态条目 fixed_fallback / 否则
/// unavailable（复用 models_overview 的装配 + 探测机制，不新增缓存）。
async fn catalog_state(
    core: &AppCore,
    channel: &channels::FirstPartyChannel,
) -> ProviderCatalogState {
    let id = ProviderId::new(channel.id);
    let protocol = match channel_protocol(Some(channel), core.config(), channel.id) {
        Ok(protocol) => protocol,
        Err(error) => {
            return ProviderCatalogState::Unavailable {
                error: error.to_string(),
                fetched_at: None,
            };
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
                    return ProviderCatalogState::Remote {
                        fetched_at: iso8601_utc(now_millis()),
                    };
                }
                Ok(Err(error)) => error.to_string(),
                Err(_) => "runtime model probe timed out".to_string(),
            }
        }
        Err(error) => error.to_string(),
    };
    if has_static {
        ProviderCatalogState::FixedFallback {
            snapshot_label: STATIC_CATALOG_LABEL.to_string(),
            fetched_at: None,
        }
    } else {
        ProviderCatalogState::Unavailable {
            error: probe_error,
            fetched_at: None,
        }
    }
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
    let providers: Vec<ProviderAuthStatusEntry> = selected
        .iter()
        .zip(catalog_states)
        .map(|(channel, catalog)| ProviderAuthStatusEntry {
            provider_id: channel.id.to_string(),
            display_name: channel.display_name.to_string(),
            endpoint_label: endpoint_label(&core, channel),
            auth_methods: channel
                .auth_methods()
                .iter()
                .map(|method| (*method).to_string())
                .collect(),
            auth: auth_state(&core, &adapter.auth_flights, channel),
            catalog,
        })
        .collect();
    // SET-5：顶层透出生效配置（分层合并后）的持久化默认项；
    // provider/model 任一缺失时诚实输出 null，不虚构半配对。
    let config = core.config();
    let default = match (&config.default_provider, &config.default_model) {
        (Some(default_provider), Some(default_model)) => Some(DefaultModelPair {
            provider_id: default_provider.clone(),
            model_id: default_model.clone(),
        }),
        _ => None,
    };
    Ok(settings_data(ProviderAuthStatusData { providers, default }))
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
    // SET-5：写盘成功即同步内存生效配置（短写锁，校验读锁已释放），
    // 保证同会话重查 provider_auth_status 的 default 即为新值。
    {
        let mut core = adapter.core.write().await;
        core.set_default_model_pair(id, model_id);
    }
    Ok(settings_data(DefaultModelPair {
        provider_id: id.to_string(),
        model_id: model_id.clone(),
    }))
}
