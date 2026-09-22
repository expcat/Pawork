use std::sync::Arc;

use pawork_domain::ProviderId;
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppQuery, AppResponse, DefaultModelPair, ProviderAuthState,
    ProviderAuthStatusData, ProviderAuthStatusEntry, ProviderCatalogState,
    ProviderCredentialStatus, ProviderUseProxyData, RoleDefaultsData, SetDefaultRoleModelData,
    SetModelEnabledData, SetProviderModelsEnabledData,
};
use pawork_providers::{CatalogEntry, ReasoningProtector};

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

fn catalog_has_model(catalog: &[CatalogEntry], id: &str, model: &str) -> bool {
    catalog
        .iter()
        .any(|entry| entry.provider.as_str() == id && entry.id.as_str() == model)
}

fn static_catalog_has_model(core: &AppCore, id: &str, model: &str) -> bool {
    let channel = channels::first_party_channel(id);
    let Ok(protocol) = channel_protocol(channel, core.config(), id) else {
        return false;
    };
    assemble_registry(core.config(), &ProviderId::new(id), protocol, channel)
        .list()
        .iter()
        .any(|entry| entry.provider.as_str() == id && entry.id.as_str() == model)
}

fn denylist_has_model(core: &AppCore, id: &str, model: &str) -> bool {
    core.config()
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .is_some_and(|provider| provider.disabled_models.iter().any(|entry| entry == model))
}

/// 启停校验：快照 / 静态回退 / 已在 denylist 的 ID 命中即过，未命中才探测。
/// 避免每次 Switch 再跑全通道 `models_overview`。
async fn require_known_runnable_model(
    core: &AppCore,
    id: &str,
    model: &str,
) -> Result<(), GuiHostError> {
    if core
        .last_runnable_catalog()
        .is_some_and(|catalog| catalog_has_model(&catalog, id, model))
        || static_catalog_has_model(core, id, model)
        || denylist_has_model(core, id, model)
    {
        return Ok(());
    }
    let overview = core.models_overview().await;
    if catalog_has_model(&overview, id, model) {
        Ok(())
    } else {
        Err(GuiHostAdapter::host_error(
            "unknown_model",
            format!("model {model} is not in the runnable catalog of provider {id}"),
        ))
    }
}

async fn current_runnable_catalog(core: &AppCore) -> Vec<CatalogEntry> {
    match core.last_runnable_catalog() {
        Some(cached) => cached,
        None => core.models_overview().await,
    }
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
    Ok(())
}

// 目录探测之后、配置写锁之内复核，防止校验后被另一条命令禁用。
fn validate_model_enabled(core: &AppCore, id: &str, model_id: &str) -> Result<(), GuiHostError> {
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

/// 盘上持久化配置（Builtin + Global 文件，不含 Session/Run 覆盖）。
/// 角色默认对的清除判定以此为准：内存生效配置可能含 CLI
/// `--provider/--model` 覆盖（不落盘），据此清盘会误删真实默认对。
fn persisted_config(
    path: &std::path::Path,
) -> Result<pawork_workspace::config::PaworkConfig, GuiHostError> {
    pawork_workspace::config::Loader::discover_from(Some(path), None)
        .resolve()
        .map(|resolved| resolved.config)
        .map_err(config_write_error)
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
    match crate::auth::effective_provider_account(core.auth_backend().as_ref(), &provider) {
        Ok(Some(account)) => ProviderAuthState::Connected {
            method: account.kind.as_str().into(),
            masked_credential: Some(account.stored.masked.as_str().into()),
        },
        Ok(None)
            if channel.auth_methods().contains(&"api_key")
                && !crate::auth::activate_first_account(channel.id) =>
        {
            ProviderAuthState::Connected {
                method: "api_key".into(),
                masked_credential: None,
            }
        }
        Ok(None) => ProviderAuthState::None,
        Err(error) => ProviderAuthState::Error {
            message: error.to_string(),
        },
    }
}

fn stored_credentials(
    core: &AppCore,
    channel: &channels::FirstPartyChannel,
) -> Vec<ProviderCredentialStatus> {
    let backend = core.auth_backend();
    let provider = ProviderId::new(channel.id);
    let Ok(inventory) = pawork_auth::list_provider_accounts(backend.as_ref(), &provider) else {
        return Vec::new();
    };
    let effective = crate::auth::effective_provider_account(backend.as_ref(), &provider)
        .ok()
        .flatten();
    inventory
        .accounts
        .into_iter()
        .map(|account| ProviderCredentialStatus {
            selected: effective
                .as_ref()
                .is_some_and(|current| current.credential_id == account.credential_id),
            credential_id: account.credential_id,
            display_name: account.display_name,
            kind: account.kind.as_str().into(),
            masked_credential: account.stored.masked.as_str().into(),
            expired: account
                .stored
                .expires_at
                .is_some_and(|expires| expires.as_unix_millis() <= now_millis()),
            expires_at: account
                .stored
                .expires_at
                .map(|expires| iso8601_utc(expires.as_unix_millis())),
        })
        .collect()
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
    let probe_error = match tokio::time::timeout(PROBE_TIMEOUT, async {
        let assembled = assemble_provider(
            core.config(),
            &id,
            core.auth_backend(),
            true,
            Arc::clone(&core.reasoning_protector) as Arc<dyn ReasoningProtector>,
        )
        .await
        .map_err(|error| error.to_string())?;
        assembled
            .adapter
            .list_models(assembled.credential.as_ref())
            .await
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(Ok(_)) => {
            return ProviderCatalogState::Remote {
                fetched_at: iso8601_utc(now_millis()),
            };
        }
        Ok(Err(error)) => error,
        Err(_) => "runtime model probe timed out".to_string(),
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
            selection_mode: match pawork_auth::list_provider_accounts(
                core.auth_backend().as_ref(),
                &ProviderId::new(channel.id),
            )
            .map(|inventory| inventory.selection_mode)
            {
                Ok(pawork_auth::ProviderAccountSelectionMode::WhenExhausted) => {
                    pawork_protocol::ProviderAccountSelectionMode::WhenExhausted
                }
                _ => pawork_protocol::ProviderAccountSelectionMode::Manual,
            },
            provider_id: channel.id.to_string(),
            display_name: channel.display_name.to_string(),
            endpoint_label: endpoint_label(&core, channel),
            auth_methods: channel
                .auth_methods()
                .iter()
                .map(|method| (*method).to_string())
                .collect(),
            // ADR-056 D2：盘上存储凭证逐条列出；flight 不回写本列表。
            credentials: stored_credentials(&core, channel),
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
    let mut core = adapter.core.write().await;
    validate_model_enabled(&core, id, model_id)?;
    pawork_workspace::config::write_default_model_pair(&path, id, model_id)
        .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
    // SET-5：写盘成功即同步内存生效配置（短写锁，校验读锁已释放），
    // 保证同会话重查 provider_auth_status 的 default 即为新值。
    core.set_default_model_pair(id, model_id);
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
    let mut core = adapter.core.write().await;
    pawork_workspace::config::write_provider_use_proxy(&path, id, *use_proxy)
        .map_err(|error| GuiHostAdapter::host_error("config_write", error.to_string()))?;
    core.set_provider_use_proxy(id, *use_proxy);
    if core.provider_id().as_str() == id {
        core.provider_stale = true;
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
    {
        let core = adapter.core.read().await;
        if !known_provider(&core, id) {
            return Err(GuiHostAdapter::host_error(
                "unknown_provider",
                format!("provider {id} is unknown"),
            ));
        }
        require_known_runnable_model(&core, id, model).await?;
    }
    let path = global_config_file()?;
    // 写盘与内存同步共用写锁；单项修改基于最新磁盘，保留其他实例的选择。
    let mut core = adapter.core.write().await;
    let persisted = persisted_config(&path)?;
    let mut disabled = persisted
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .map(|provider| provider.disabled_models.clone())
        .unwrap_or_default();
    if *enabled {
        disabled.retain(|entry| entry != model);
    } else if !disabled.iter().any(|entry| entry == model) {
        disabled.push(model.to_string());
    }
    // 清除判定按盘上持久化配置：只有真实写盘的默认对才允许同批清除。
    let cleared: Vec<(RoleModelKind, (String, String))> = if *enabled {
        Vec::new()
    } else {
        RoleModelKind::ALL
            .into_iter()
            .filter_map(|kind| {
                let pair = kind.pair_in_config(&persisted)?;
                (pair.0 == id && pair.1 == model).then_some((kind, pair))
            })
            .collect()
    };
    let clear_pairs: Vec<_> = cleared
        .iter()
        .map(|(kind, _)| (kind.provider_key(), kind.model_key()))
        .collect();
    pawork_workspace::config::write_provider_model_preferences(&path, id, &disabled, &clear_pairs)
        .map_err(config_write_error)?;
    {
        core.set_provider_disabled_models(id, disabled);
        for (kind, pair) in &cleared {
            // 内存仅在与被清持久化对一致时同步清除，保留 CLI 覆盖的生效值。
            if kind.pair_in_config(core.config()).as_ref() == Some(pair) {
                core.set_role_model_pair(*kind, None);
            }
        }
    }
    Ok(settings_data(SetModelEnabledData {
        provider_id: id.to_string(),
        model_id: model.to_string(),
        enabled: *enabled,
        cleared_roles: cleared
            .iter()
            .map(|(kind, _)| kind.wire_name().to_string())
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
    let mut disabled = {
        let core = adapter.core.read().await;
        if !known_provider(&core, id) {
            return Err(GuiHostAdapter::host_error(
                "unknown_provider",
                format!("provider {id} is unknown"),
            ));
        }
        if *enabled {
            Vec::new()
        } else {
            let mut models: Vec<String> = current_runnable_catalog(&core)
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
            models
        }
    };
    let path = global_config_file()?;
    let mut core = adapter.core.write().await;
    let persisted = persisted_config(&path)?;
    if !*enabled {
        // 全关不能重新启用暂时退出目录、随后可能回来的已禁用模型。
        if let Some(provider) = persisted
            .providers
            .iter()
            .find(|provider| provider.id == id)
        {
            disabled.extend(provider.disabled_models.iter().cloned());
            disabled.sort();
            disabled.dedup();
        }
    }
    // 全关展开使命中该 provider 的任一角色默认对整体失效；清除判定
    // 按盘上持久化配置（内存可能含不落盘的 CLI 覆盖）。
    let cleared: Vec<(RoleModelKind, (String, String))> = if *enabled {
        Vec::new()
    } else {
        RoleModelKind::ALL
            .into_iter()
            .filter_map(|kind| {
                let pair = kind.pair_in_config(&persisted)?;
                (pair.0 == id).then_some((kind, pair))
            })
            .collect()
    };
    let clear_pairs: Vec<_> = cleared
        .iter()
        .map(|(kind, _)| (kind.provider_key(), kind.model_key()))
        .collect();
    pawork_workspace::config::write_provider_model_preferences(&path, id, &disabled, &clear_pairs)
        .map_err(config_write_error)?;
    {
        core.set_provider_disabled_models(id, disabled);
        for (kind, pair) in &cleared {
            // 内存仅在与被清持久化对一致时同步清除，保留 CLI 覆盖的生效值。
            if kind.pair_in_config(core.config()).as_ref() == Some(pair) {
                core.set_role_model_pair(*kind, None);
            }
        }
    }
    Ok(settings_data(SetProviderModelsEnabledData {
        provider_id: id.to_string(),
        enabled: *enabled,
        cleared_roles: cleared
            .iter()
            .map(|(kind, _)| kind.wire_name().to_string())
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
    let mut core = adapter.core.write().await;
    let write_pair = match &pair {
        Some((provider_id, model_id)) => Some((provider_id.as_str(), model_id.as_str())),
        None => None,
    };
    if let Some((provider_id, model_id)) = write_pair {
        validate_model_enabled(&core, provider_id, model_id)?;
    }
    pawork_workspace::config::write_model_pair(
        &path,
        kind.provider_key(),
        kind.model_key(),
        write_pair,
    )
    .map_err(config_write_error)?;
    core.set_role_model_pair(kind, write_pair);
    Ok(settings_data(SetDefaultRoleModelData {
        role: kind.wire_name().to_string(),
        value: pair.map(|(provider_id, model_id)| DefaultModelPair {
            provider_id,
            model_id,
        }),
    }))
}

/// ADR-063：单模型推理强度偏好写（Global `[reasoning]`）。
///
/// 全态语义：default_effort / supported_efforts 为 None 即清除该键，两者皆
/// None 移除条目。effort 名必须属 canonical 词汇（invalid_effort
/// fail-closed）；default 须落在已知的可选范围内——本次写入的手动范围、
/// 盘上既有手动范围、目录声明范围（按此序取首个已知），全未知时不约束。
/// 写盘成功即同步内存生效配置（同 set_model_enabled）。
pub(crate) async fn set_model_reasoning(
    adapter: &GuiHostAdapter,
    _envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let AppCommand::SetModelReasoning {
        provider_id,
        model_id,
        default_effort,
        supported_efforts,
    } = command
    else {
        unreachable!("set_model_reasoning handler receives SetModelReasoning")
    };
    let invalid_effort = || {
        GuiHostAdapter::host_error(
            "invalid_effort",
            "effort names must be canonical (low/medium/high/x_high/max) and the default must be inside the supported range",
        )
    };
    let default = match default_effort {
        Some(name) => Some(
            pawork_domain::ReasoningEffort::from_wire_name(name).ok_or_else(|| invalid_effort())?,
        ),
        None => None,
    };
    let manual = match supported_efforts {
        Some(names) => {
            let mut parsed = Vec::with_capacity(names.len());
            for name in names {
                parsed.push(
                    pawork_domain::ReasoningEffort::from_wire_name(name)
                        .ok_or_else(|| invalid_effort())?,
                );
            }
            Some(parsed)
        }
        None => None,
    };
    let id = provider_id.as_str();
    let model = model_id.as_str();
    {
        let core = adapter.core.read().await;
        if !known_provider(&core, id) {
            return Err(GuiHostAdapter::host_error(
                "unknown_provider",
                format!("provider {id} is unknown"),
            ));
        }
        let catalog = current_runnable_catalog(&core).await;
        if !catalog_has_model(&catalog, id, model) {
            // 静态目录 / denylist 回退判定（与启停写同口径）。
            require_known_runnable_model(&core, id, model).await?;
        }
        if let Some(default) = default {
            let known_range: Option<Vec<pawork_domain::ReasoningEffort>> = manual
                .clone()
                .or_else(|| persisted_manual_efforts(&core, model))
                .or_else(|| {
                    // 跨 Provider 合并：同 model_id 共用目录声明范围——优先本
                    // Provider 条目，其次任一 Provider 的同 id 声明。
                    catalog
                        .iter()
                        .find(|entry| entry.provider.as_str() == id && entry.id.as_str() == model)
                        .or_else(|| catalog.iter().find(|entry| entry.id.as_str() == model))
                        .and_then(|entry| entry.capabilities.supported_efforts.clone())
                });
            if let Some(range) = known_range {
                if !range.contains(&default) {
                    return Err(invalid_effort());
                }
            }
        }
    }
    let path = global_config_file()?;
    let mut core = adapter.core.write().await;
    let default_wire = default.map(|effort| effort.as_wire_name().to_string());
    let manual_wire = manual.map(|efforts| {
        efforts
            .iter()
            .map(|effort| effort.as_wire_name().to_string())
            .collect::<Vec<_>>()
    });
    pawork_workspace::config::write_model_reasoning(
        &path,
        id,
        model,
        default_wire.as_deref(),
        manual_wire.as_deref(),
    )
    .map_err(config_write_error)?;
    let reasoning = core.config.reasoning.get_or_insert_with(Default::default);
    if default_wire.is_none() && manual_wire.is_none() {
        reasoning.models.retain(|entry| entry.model_id != model);
    } else if let Some(entry) = reasoning
        .models
        .iter_mut()
        .find(|entry| entry.model_id == model)
    {
        entry.provider_id = id.to_string();
        entry.default_effort = default_wire.clone();
        entry.supported_efforts = manual_wire.clone();
    } else {
        reasoning
            .models
            .push(pawork_workspace::config::ModelReasoningConfig {
                provider_id: id.to_string(),
                model_id: model.to_string(),
                default_effort: default_wire.clone(),
                supported_efforts: manual_wire.clone(),
            });
    }
    Ok(settings_data(ModelReasoningData {
        provider_id: id.to_string(),
        model_id: model.to_string(),
        default_effort: default_wire,
        supported_efforts: manual_wire,
    }))
}

/// 盘上生效配置里该模型的既有手动范围（内存生效配置；ADR-063 校验用；
/// 跨 Provider 按 model_id 匹配合并）。
fn persisted_manual_efforts(
    core: &AppCore,
    model: &str,
) -> Option<Vec<pawork_domain::ReasoningEffort>> {
    core.config()
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.model(model))
        .and_then(|entry| entry.supported_efforts.clone())
        .map(|names| {
            names
                .iter()
                .filter_map(|name| pawork_domain::ReasoningEffort::from_wire_name(name))
                .collect()
        })
}

/// `set_model_reasoning` 回执（ADR-063；键恒在，null = 已清除）。
#[derive(serde::Serialize)]
struct ModelReasoningData {
    provider_id: String,
    model_id: String,
    default_effort: Option<String>,
    supported_efforts: Option<Vec<String>>,
}
