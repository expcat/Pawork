use std::sync::Arc;

use pawork_auth::CredentialSource;
use pawork_domain::ProviderId;
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppQuery, AppResponse, DefaultModelPair, ProviderAuthState,
    ProviderAuthStatusData, ProviderAuthStatusEntry, ProviderCatalogState, ProviderUseProxyData,
    RoleDefaultsData, SetDefaultRoleModelData, SetModelEnabledData, SetProviderModelsEnabledData,
};
use pawork_providers::ReasoningProtector;

use crate::app_core::RoleModelKind;
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

/// provider 已知性（首方通道或生效配置 `[[providers]]` 条目）。
fn known_provider(core: &AppCore, id: &str) -> bool {
    channels::is_first_party(id)
        || core
            .config()
            .providers
            .iter()
            .any(|provider| provider.id == id)
}

/// provider/model 校验（`set_default_model` 的 models_overview 口径）+
/// ADR-055 D4 禁用校验：拒绝把禁用模型设为默认/角色默认对。
async fn validate_runnable_model(
    core: &AppCore,
    id: &str,
    model_id: &str,
) -> Result<(), GuiHostError> {
    if !known_provider(core, id) {
        return Err(GuiHostAdapter::host_error(
            "unknown_provider",
            format!("provider {id} is unknown"),
        ));
    }
    let overview = core.models_overview().await;
    if !overview
        .iter()
        .any(|entry| entry.provider.as_str() == id && entry.id.as_str() == model_id)
    {
        return Err(GuiHostAdapter::host_error(
            "unknown_model",
            format!("model {model_id} is not in the runnable catalog of provider {id}"),
        ));
    }
    if !core.config().is_model_enabled(id, model_id) {
        return Err(GuiHostAdapter::host_error(
            "model_disabled",
            format!("model {model_id} of provider {id} is disabled"),
        ));
    }
    Ok(())
}

/// Global 配置路径（不可用即 `config_unavailable`）。
fn global_config_file() -> Result<std::path::PathBuf, GuiHostError> {
    pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "global config directory is not available on this platform",
        )
    })
}

fn config_write_error(error: pawork_workspace::config::ConfigError) -> GuiHostError {
    GuiHostAdapter::host_error("config_write", error.to_string())
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
            // ADR-052 SET-6h：生效值 = 未显式 `use_proxy = false`。
            use_proxy: core
                .config()
                .providers
                .iter()
                .find(|provider| provider.id == channel.id)
                .and_then(|provider| provider.use_proxy)
                != Some(false),
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
    // ADR-055 D5：三辅助角色默认对（半配对输出 null，同顶层 default 口径）；
    // conversation 仍由既有顶层 default 透出，不在此重复。
    let wire_pair = |kind: RoleModelKind| {
        kind.pair_in_config(config)
            .map(|(provider_id, model_id)| DefaultModelPair {
                provider_id,
                model_id,
            })
    };
    let role_defaults = RoleDefaultsData {
        naming: wire_pair(RoleModelKind::Naming),
        vision: wire_pair(RoleModelKind::Vision),
        search: wire_pair(RoleModelKind::Search),
    };
    Ok(settings_data(ProviderAuthStatusData {
        providers,
        default,
        role_defaults,
    }))
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
        // 校验复用 models_overview 口径；ADR-055 D4 起含禁用校验。
        validate_runnable_model(&core, id, model_id).await?;
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

/// ADR-052 SET-6h：切换供应商级代理开关。`use_proxy = false` 表示该
/// provider 出站绕过 Global `proxy_url`；未设置或 `true` 跟随全局代理。
/// 与 set_default_model 一致：写盘成功即同步内存生效配置。
pub(crate) async fn set_provider_use_proxy(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetProviderUseProxy {
        provider_id,
        use_proxy,
    } = command
    else {
        unreachable!("set_provider_use_proxy handler receives SetProviderUseProxy")
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
    }
    let path = pawork_workspace::config::global_config_path().ok_or_else(|| {
        GuiHostAdapter::host_error(
            "config_unavailable",
            "global config directory is not available on this platform",
        )
    })?;
    pawork_workspace::config::write_provider_use_proxy(&path, id, *use_proxy)
        .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
    {
        let mut core = adapter.core.write().await;
        core.set_provider_use_proxy(id, *use_proxy);
    }
    Ok(settings_data(ProviderUseProxyData {
        provider_id: id.to_string(),
        use_proxy: *use_proxy,
    }))
}

/// ADR-055 OPT-3a：单模型启用/禁用。禁用命中任一角色默认对时同批清除
/// 该键对（D3：禁止静默换绑），回执 `cleared_roles` 按 wire 名列出；
/// 启用恒为空。写盘成功即同步内存生效配置（同 set_provider_use_proxy）。
pub(crate) async fn set_model_enabled(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetModelEnabled {
        provider_id,
        model_id,
        enabled,
    } = command
    else {
        unreachable!("set_model_enabled handler receives SetModelEnabled")
    };
    let id = provider_id.as_str();
    let model = model_id.as_str();
    let (disabled, cleared) = {
        let core = adapter.core.read().await;
        if !known_provider(&core, id) {
            return Err(GuiHostAdapter::host_error(
                "unknown_provider",
                format!("provider {id} is unknown"),
            ));
        }
        // 校验口径同 set_default_model：模型必须在该 provider 当前可运行目录。
        let overview = core.models_overview().await;
        if !overview
            .iter()
            .any(|entry| entry.provider.as_str() == id && entry.id.as_str() == model)
        {
            return Err(GuiHostAdapter::host_error(
                "unknown_model",
                format!("model {model} is not in the runnable catalog of provider {id}"),
            ));
        }
        let mut disabled: Vec<String> = core
            .config()
            .providers
            .iter()
            .find(|provider| provider.id == id)
            .map(|provider| provider.disabled_models.clone())
            .unwrap_or_default();
        let mut cleared = Vec::new();
        if *enabled {
            disabled.retain(|entry| entry != model);
        } else {
            // 幂等：重复同态写为最终覆盖语义。
            if !disabled.iter().any(|entry| entry == model) {
                disabled.push(model.to_string());
            }
            for kind in RoleModelKind::ALL {
                if kind.pair_in_config(core.config()) == Some((id.to_string(), model.to_string())) {
                    cleared.push(kind);
                }
            }
        }
        (disabled, cleared)
    };
    let path = global_config_file()?;
    pawork_workspace::config::write_provider_disabled_models(&path, id, &disabled)
        .map_err(config_write_error)?;
    for kind in &cleared {
        pawork_workspace::config::write_model_pair(
            &path,
            kind.provider_key(),
            kind.model_key(),
            None,
        )
        .map_err(config_write_error)?;
    }
    {
        let mut core = adapter.core.write().await;
        core.set_provider_disabled_models(id, disabled);
        for kind in &cleared {
            core.set_role_model_pair(*kind, None);
        }
    }
    Ok(settings_data(SetModelEnabledData {
        provider_id: id.to_string(),
        model_id: model.to_string(),
        enabled: *enabled,
        cleared_roles: cleared
            .iter()
            .map(|kind| kind.wire_name().to_string())
            .collect(),
    }))
}

/// ADR-055 OPT-3a：provider 全量模型启用/禁用。全开 = 清空 denylist；
/// 全关 = 按当前聚合目录展开全部模型写 denylist（目录为空
/// `catalog_unavailable` fail-closed 不写盘，防空展开退化为全开）。
pub(crate) async fn set_provider_models_enabled(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetProviderModelsEnabled {
        provider_id,
        enabled,
    } = command
    else {
        unreachable!("set_provider_models_enabled handler receives SetProviderModelsEnabled")
    };
    let id = provider_id.as_str();
    let (disabled, cleared) = {
        let core = adapter.core.read().await;
        if !known_provider(&core, id) {
            return Err(GuiHostAdapter::host_error(
                "unknown_provider",
                format!("provider {id} is unknown"),
            ));
        }
        if *enabled {
            (Vec::new(), Vec::new())
        } else {
            let mut models: Vec<String> = core
                .models_overview()
                .await
                .iter()
                .filter(|entry| entry.provider.as_str() == id)
                .map(|entry| entry.id.as_str().to_string())
                .collect();
            models.sort();
            models.dedup();
            if models.is_empty() {
                return Err(GuiHostAdapter::host_error(
                    "catalog_unavailable",
                    format!("provider {id} has no runnable catalog to disable"),
                ));
            }
            // 全关展开使命中该 provider 的任一角色默认对整体失效。
            let cleared = RoleModelKind::ALL
                .into_iter()
                .filter(|kind| {
                    kind.pair_in_config(core.config())
                        .is_some_and(|(provider, _)| provider == id)
                })
                .collect::<Vec<_>>();
            (models, cleared)
        }
    };
    let path = global_config_file()?;
    pawork_workspace::config::write_provider_disabled_models(&path, id, &disabled)
        .map_err(config_write_error)?;
    for kind in &cleared {
        pawork_workspace::config::write_model_pair(
            &path,
            kind.provider_key(),
            kind.model_key(),
            None,
        )
        .map_err(config_write_error)?;
    }
    {
        let mut core = adapter.core.write().await;
        core.set_provider_disabled_models(id, disabled);
        for kind in &cleared {
            core.set_role_model_pair(*kind, None);
        }
    }
    Ok(settings_data(SetProviderModelsEnabledData {
        provider_id: id.to_string(),
        enabled: *enabled,
        cleared_roles: cleared
            .iter()
            .map(|kind| kind.wire_name().to_string())
            .collect(),
    }))
}

/// ADR-055 OPT-3b：四默认角色读写。未知 role `unknown_role` fail-closed；
/// `value = null` 清除键对；设置校验 = 已知 provider + 可运行目录 + 未禁用。
pub(crate) async fn set_default_role_model(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetDefaultRoleModel { role, value } = command else {
        unreachable!("set_default_role_model handler receives SetDefaultRoleModel")
    };
    let Some(kind) = RoleModelKind::from_wire(role) else {
        return Err(GuiHostAdapter::host_error(
            "unknown_role",
            format!("role {role} is not one of conversation/naming/vision/search"),
        ));
    };
    // 清除（null）：半配对只移除存在的键，两键都不存在时写盘为 no-op。
    let pair = match value {
        Some(pair) => {
            let (provider_id, model_id) = (pair.provider_id.clone(), pair.model_id.clone());
            {
                let core = adapter.core.read().await;
                validate_runnable_model(&core, &provider_id, &model_id).await?;
            }
            Some((provider_id, model_id))
        }
        None => None,
    };
    let path = global_config_file()?;
    let write_pair = match &pair {
        Some((provider_id, model_id)) => Some((provider_id.as_str(), model_id.as_str())),
        None => None,
    };
    pawork_workspace::config::write_model_pair(
        &path,
        kind.provider_key(),
        kind.model_key(),
        write_pair,
    )
    .map_err(config_write_error)?;
    {
        let mut core = adapter.core.write().await;
        core.set_role_model_pair(kind, write_pair);
    }
    Ok(settings_data(SetDefaultRoleModelData {
        role: kind.wire_name().to_string(),
        value: pair.map(|(provider_id, model_id)| DefaultModelPair {
            provider_id,
            model_id,
        }),
    }))
}
