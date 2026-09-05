//! Provider 装配层：通道协议解析、目录装配、凭证链、模型切换与聚合目录。
//!
//! R4 波 A 自 lib.rs 平移（行为零变化）：本模块是 host 装配层唯一的
//! Provider 选择点；Engine 仍只看 trait 对象。

use std::sync::Arc;
use std::time::Duration;

use pawork_auth::locator::api_key_env_name;
use pawork_auth::{
    load_default_oauth_credential, load_default_oauth_meta,
    refresh_default_oauth_credential_if_needed, resolve_oauth_credential,
    resolve_provider_credential, ApiKeyCredential, AuthError, CredentialSource, OAuthRefreshConfig,
    SecretBackend,
};
use pawork_domain::{
    AgentEvent, CanonicalModelRequest, CancellationToken, ContentPart, Message, MessageId,
    MessageRole, ModelDefinition, ModelId, ModelProvider, ProviderError, ProviderErrorKind,
    ProviderEventSink, ProviderStreamEvent, ProviderId, RequestId, ResolvedCredential, RunId,
    SessionId, TextContent,
};
use pawork_providers::ReasoningProtector;
use pawork_providers::{
    AnthropicConfig, AnthropicProvider, ApiKeyChannelConfig, ApiKeyChannelProvider, CatalogEntry,
    ModelRegistry, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use pawork_workspace::config::{PaworkConfig, ProviderConfig};

use async_trait::async_trait;

use crate::channels::{self, ChannelKind};
use crate::protocol::{resolve_adapter_protocol, AdapterProtocol};
use crate::{AppCore, AppError};

/// 自动命名一次性补全的兜底超时（ADR-054 D4：超时保留占位名）。
const NAMING_TIMEOUT: Duration = Duration::from_secs(20);
/// 送入命名模型的首条用户消息截断上限（字符）。
const NAMING_INPUT_MAX_CHARS: usize = 4_000;
/// 命名补全输出上限（token）：标题本身只取单行，64 已远超所需。
const NAMING_MAX_OUTPUT_TOKENS: u64 = 64;

/// 构造无工具的命名补全请求：system 指令 + 截断后的首条用户消息。
fn naming_request(model: ModelId, first_user_text: &str) -> CanonicalModelRequest {
    let truncated: String = first_user_text.chars().take(NAMING_INPUT_MAX_CHARS).collect();
    let mut request = pawork_engine::assemble_request(
        RequestId::from(format!(
            "req-naming-{}",
            pawork_engine::now_timestamp().as_unix_millis()
        )),
        model,
        vec![
            Message {
                id: MessageId::from("naming-system"),
                role: MessageRole::System,
                content: vec![ContentPart::Text(TextContent {
                    text: "你在为用户会话生成标题。根据用户消息生成一个简短标题：不超过 20 个字；只输出标题本身，不要引号、编号、前后缀或解释。".into(),
                })],
                metadata: Default::default(),
            },
            Message {
                id: MessageId::from("naming-user"),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent { text: truncated })],
                metadata: Default::default(),
            },
        ],
    );
    request.max_output_tokens = Some(NAMING_MAX_OUTPUT_TOKENS);
    request
}

/// 收集命名补全的文本增量；完成后取首个非空行并按既有标题规则限长。
#[derive(Default)]
struct TitleTextSink {
    text: std::sync::Mutex<String>,
}

impl TitleTextSink {
    fn single_line_title(&self) -> Option<String> {
        let joined = self.text.lock().expect("title sink mutex").clone();
        let line = joined
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())?;
        let title = crate::session_title_from_text(line);
        (title != crate::app_core::PLACEHOLDER_SESSION_TITLE).then_some(title)
    }
}

#[async_trait]
impl ProviderEventSink for TitleTextSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        if let ProviderStreamEvent::TextDelta(delta) = event {
            self.text
                .lock()
                .expect("title sink mutex")
                .push_str(&delta);
        }
        Ok(())
    }
}

/// 目录命令（models/auth/sessions）装配时容忍的「凭证缺失」错误族；
/// 其余错误（配置、协议、provider 未知）仍然 fail-closed。
pub(crate) fn is_credential_pending(err: &AppError) -> bool {
    matches!(
        err,
        AppError::MissingCredential { .. }
            | AppError::OAuthLoginRequired(_)
            | AppError::OAuthLogin(_)
            | AppError::Auth(_)
    )
}

impl AppCore {
    /// 当前 provider 在 registry 的静态目录（REPL /model 列表用）。
    pub fn provider_models(&self) -> Vec<CatalogEntry> {
        self.registry
            .list()
            .into_iter()
            .filter(|entry| entry.provider == self.provider_id)
            .cloned()
            .collect()
    }

    /// 会话中途切换模型：后续轮走新模型；有活动 session 时事件流记录变更。
    pub async fn switch_model(
        &mut self,
        session: Option<&SessionId>,
        model: &str,
    ) -> Result<(), AppError> {
        let provider = Arc::clone(&self.provider);
        let credential = self.credential.clone();
        let provider_id = self.provider_id.clone();
        let entry = resolve_provider_model(
            Arc::make_mut(&mut self.registry),
            provider.as_ref(),
            credential.as_ref(),
            &provider_id,
            model,
        )
        .await?;
        // ADR-055 D4：会话内模型切换同闸——目标模型禁用时结构化
        // fail-closed，不静默回退其他模型。
        if !self
            .config
            .is_model_enabled(provider_id.as_str(), entry.id.as_str())
        {
            return Err(AppError::ModelDisabled {
                provider: provider_id.as_str().to_string(),
                model: entry.id.as_str().to_string(),
            });
        }
        let from = (self.provider_id.clone(), self.model.clone());
        self.model = entry.id.clone();
        let to = (self.provider_id.clone(), self.model.clone());
        if let Some(session) = session {
            self.record_model_switch(session, from, to).await?;
        }
        Ok(())
    }

    /// 会话中途切换 provider（可选同时切模型）：重建 adapter，后续轮生效。
    pub async fn switch_provider(
        &mut self,
        session: Option<&SessionId>,
        provider: &str,
        model: Option<&str>,
    ) -> Result<(), AppError> {
        let known = channels::is_first_party(provider)
            || self.config.providers.iter().any(|p| p.id == provider);
        if !known {
            return Err(AppError::UnknownProvider {
                id: provider.to_string(),
            });
        }
        let target = ProviderId::new(provider);
        let mut assembled = assemble_provider(
            &self.config,
            &target,
            &self.backend,
            true,
            Arc::clone(&self.reasoning_protector) as Arc<dyn ReasoningProtector>,
        )
        .await?;

        // 目标模型：显式参数 → 当前模型（若属于目标 provider）→ 目标 provider
        // 的第一个 registry 条目；都无则要求显式 /model。
        let target_model = if let Some(model) = model {
            resolve_provider_model(
                &mut assembled.registry,
                assembled.adapter.as_ref(),
                assembled.credential.as_ref(),
                &target,
                model,
            )
            .await?
            .id
        } else if self
            .registry
            .resolve(self.model.as_str())
            .is_some_and(|entry| entry.provider == target)
        {
            self.model.clone()
        } else {
            assembled
                .registry
                .list()
                .into_iter()
                .find(|entry| entry.provider == target)
                .map(|entry| entry.id.clone())
                .ok_or_else(|| AppError::UnknownModel {
                    model: "<any>".to_string(),
                    provider: provider.to_string(),
                })?
        };
        // ADR-055 D4：切换后生效对禁用时 fail-closed，不启动后续轮。
        if !self
            .config
            .is_model_enabled(target.as_str(), target_model.as_str())
        {
            return Err(AppError::ModelDisabled {
                provider: target.as_str().to_string(),
                model: target_model.as_str().to_string(),
            });
        }

        let from = (self.provider_id.clone(), self.model.clone());
        self.provider = assembled.adapter;
        self.credential = assembled.credential;
        self.adapter_protocol = assembled.protocol;
        self.registry = Arc::new(assembled.registry);
        self.provider_id = target;
        self.model = target_model;
        self.rebind_persistent_protector();
        let to = (self.provider_id.clone(), self.model.clone());
        if let Some(session) = session {
            self.record_model_switch(session, from, to).await?;
        }
        Ok(())
    }

    /// ADR-054 D4：用命名模型做一次无工具一次性补全，产出会话标题。
    ///
    /// 命名 provider 与当前已装配 provider 相同且凭证就绪时复用既有
    /// adapter / 凭证（避免重复装配）；否则经 assemble_provider 全量装配。
    /// 调用带超时；任何失败由调用方决定保留占位名，本方法不产生用户可见
    /// 错误。输出 trim 后取首个非空行并限长，空结果返回 None。
    pub(crate) async fn generate_session_title(
        &self,
        first_user_text: &str,
    ) -> Result<Option<String>, AppError> {
        let (provider, model) = match (
            self.config.naming_provider.as_deref(),
            self.config.naming_model.as_deref(),
        ) {
            (Some(provider), Some(model)) => (provider.to_string(), model.to_string()),
            _ => return Ok(None),
        };
        let provider_id = ProviderId::new(provider);
        let (adapter, credential, mut registry) =
            if provider_id == self.provider_id && !self.provider_pending {
                (
                    Arc::clone(&self.provider),
                    self.credential.clone(),
                    self.registry.as_ref().clone(),
                )
            } else {
                let assembled = assemble_provider(
                    &self.config,
                    &provider_id,
                    &self.backend,
                    true,
                    Arc::clone(&self.reasoning_protector) as Arc<dyn ReasoningProtector>,
                )
                .await?;
                (assembled.adapter, assembled.credential, assembled.registry)
            };
        let entry = resolve_provider_model(
            &mut registry,
            adapter.as_ref(),
            credential.as_ref(),
            &provider_id,
            &model,
        )
        .await?;
        let request = naming_request(entry.id.clone(), first_user_text);
        let sink = TitleTextSink::default();
        let streamed = tokio::time::timeout(
            NAMING_TIMEOUT,
            adapter.stream(request, &sink, CancellationToken::new()),
        )
        .await
        .map_err(|_| {
            AppError::from(ProviderError::new(
                ProviderErrorKind::Timeout,
                "session naming timed out",
            ))
        })??;
        let _ = streamed;
        Ok(sink.single_line_title())
    }

    /// 追加 model.switched 诊断事件（冻结的 Diagnostic 变体，不新增枚举形状）。
    async fn record_model_switch(
        &self,
        session: &SessionId,
        from: (ProviderId, ModelId),
        to: (ProviderId, ModelId),
    ) -> Result<(), AppError> {
        let mut sequence = self.next_sequence(session).await?;
        let run_id = RunId::from(format!(
            "run-switch-{}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
        self.append_payload(
            session,
            &run_id,
            &mut sequence,
            AgentEvent::Diagnostic {
                code: "model.switched".into(),
                details: serde_json::json!({
                    "from": {
                        "provider": from.0.as_str(),
                        "model": from.1.as_str(),
                    },
                    "to": {
                        "provider": to.0.as_str(),
                        "model": to.1.as_str(),
                    },
                }),
            },
        )
        .await
        .map(|_| ())
    }

    /// 模型目录（builtin + config 覆盖 + 运行期 /models 探测合并，探测失败退回静态）。
    pub async fn model_catalog(&self) -> Vec<CatalogEntry> {
        let mut catalog = self.registry.as_ref().clone();
        match catalog
            .probe_provider(self.provider.as_ref(), self.credential.as_ref())
            .await
        {
            Err(error) => {
                tracing::warn!(
                    provider = %self.provider_id,
                    error = %error,
                    "runtime model probe failed; falling back to static catalog"
                );
            }
            Ok(probe) => {
                for definition in &probe.definitions {
                    if catalog.resolve(definition.id.as_str()).is_none() {
                        catalog.extend_with(vec![CatalogEntry {
                            id: definition.id.clone(),
                            provider: self.provider_id.clone(),
                            display_name: definition.display_name.clone(),
                            context_window_tokens: definition.context_window_tokens,
                            max_output_tokens: definition.max_output_tokens,
                            capabilities: definition.capabilities.clone(),
                            pricing: None,
                            aliases: Vec::new(),
                        }]);
                    }
                }
            }
        }
        catalog.list().into_iter().cloned().collect()
    }

    /// pawork models 聚合目录：六通道静态条目 + config providers（Messages
    /// 静态目录与 models 覆盖）+ 所有能装配成功的通道的运行期探测（探测失败
    /// 静默退回该通道静态，与单通道目录一致）。未登记协议或无凭证的通道跳过探测。
    pub async fn models_overview(&self) -> Vec<CatalogEntry> {
        let mut provider_ids: Vec<ProviderId> = channels::FIRST_PARTY_CHANNELS
            .iter()
            .map(|channel| ProviderId::new(channel.id))
            .collect();
        for provider in &self.config.providers {
            let id = ProviderId::new(provider.id.as_str());
            if !provider_ids.contains(&id) {
                provider_ids.push(id);
            }
        }

        let mut catalog = ModelRegistry::empty();
        for id in &provider_ids {
            let channel = channels::first_party_channel(id.as_str());
            let protocol = match channel_protocol(channel, &self.config, id.as_str()) {
                Ok(protocol) => protocol,
                Err(_) => continue,
            };
            let registry = assemble_registry(&self.config, id, protocol, channel);
            for entry in registry.list() {
                if catalog.resolve(entry.id.as_str()).is_none() {
                    catalog.extend_with(vec![entry.clone()]);
                }
            }
        }
        let mut probe_jobs = Vec::new();
        for id in provider_ids {
            let assembled = if id.as_str() == self.provider_id.as_str() && !self.provider_pending {
                Some((Arc::clone(&self.provider), self.credential.clone()))
            } else {
                match assemble_provider(
                    &self.config,
                    &id,
                    &self.backend,
                    false,
                    Arc::clone(&self.reasoning_protector) as Arc<dyn ReasoningProtector>,
                )
                .await
                {
                    Ok(assembled) => Some((assembled.adapter, assembled.credential)),
                    Err(_) => None,
                }
            };
            if let Some((adapter, credential)) = assembled {
                probe_jobs.push((id, adapter, credential));
            }
        }
        let catalog_for_probe = catalog.clone();
        // 单通道探测若挂起（临期 OAuth / 不可达厂商），不得拖死 Desktop
        // ModelList：客户端默认 10s 超时，静态目录已含 §1.1 低消耗模型。
        const OVERVIEW_PROBE_TIMEOUT: Duration = Duration::from_secs(4);
        let probe_results =
            futures::future::join_all(probe_jobs.into_iter().map(|(id, adapter, credential)| {
                let catalog = catalog_for_probe.clone();
                async move {
                    let result = match tokio::time::timeout(
                        OVERVIEW_PROBE_TIMEOUT,
                        catalog.probe_provider(adapter.as_ref(), credential.as_ref()),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(pawork_providers::ProbeError::new(
                            "runtime model probe timed out",
                        )),
                    };
                    (id, result)
                }
            }))
            .await;
        for (id, result) in probe_results {
            match result {
                Err(error) => {
                    tracing::warn!(
                        provider = %id,
                        error = %error,
                        "runtime model probe failed; falling back to static catalog"
                    );
                }
                Ok(probe) => {
                    for definition in &probe.definitions {
                        if catalog.resolve(definition.id.as_str()).is_none() {
                            catalog.extend_with(vec![CatalogEntry {
                                id: definition.id.clone(),
                                provider: id.clone(),
                                display_name: definition.display_name.clone(),
                                context_window_tokens: definition.context_window_tokens,
                                max_output_tokens: definition.max_output_tokens,
                                capabilities: definition.capabilities.clone(),
                                pricing: None,
                                aliases: Vec::new(),
                            }]);
                        }
                    }
                }
            }
        }
        catalog.list().into_iter().cloned().collect()
    }
}

fn catalog_entry_from_definition(
    provider_id: &ProviderId,
    definition: &ModelDefinition,
) -> CatalogEntry {
    CatalogEntry {
        id: definition.id.clone(),
        provider: provider_id.clone(),
        display_name: definition.display_name.clone(),
        context_window_tokens: definition.context_window_tokens,
        max_output_tokens: definition.max_output_tokens,
        capabilities: definition.capabilities.clone(),
        pricing: None,
        aliases: Vec::new(),
    }
}

/// 按明确的 `(provider, model)` 解析模型。静态目录未命中目标 provider 时，
/// 对该 provider 探测一次并惰性合并；探测仍未命中则保持 fail-closed。
pub(crate) async fn resolve_provider_model(
    registry: &mut ModelRegistry,
    provider: &dyn ModelProvider,
    credential: Option<&ResolvedCredential>,
    provider_id: &ProviderId,
    model: &str,
) -> Result<CatalogEntry, AppError> {
    if let Some(entry) = registry
        .resolve(model)
        .filter(|entry| entry.provider == *provider_id)
        .cloned()
    {
        return Ok(entry);
    }

    let static_owner = registry
        .resolve(model)
        .map(|entry| entry.provider.as_str().to_string());
    let discovered = provider
        .list_models(credential)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|definition| definition.id.as_str() == model)
        .map(|definition| catalog_entry_from_definition(provider_id, &definition));
    if let Some(entry) = discovered {
        registry.extend_with(vec![entry.clone()]);
        return Ok(entry);
    }

    match static_owner {
        Some(owner) => Err(AppError::ModelBelongsToProvider {
            model: model.to_string(),
            owner,
            current: provider_id.as_str().to_string(),
        }),
        None => Err(AppError::UnknownModel {
            model: model.to_string(),
            provider: provider_id.as_str().to_string(),
        }),
    }
}

fn find_provider<'a>(
    providers: &'a [ProviderConfig],
    id: &str,
) -> Result<&'a ProviderConfig, AppError> {
    providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| AppError::UnknownProvider { id: id.to_string() })
}

/// 该 provider 出站应使用的代理（ADR-052 SET-6h）：Global `proxy_url`
/// 生效，除非该 provider 显式 `use_proxy = false`。未按 id 特判，
/// 仅查配置。
fn provider_proxy(config: &PaworkConfig, provider_id: &str) -> Option<String> {
    let bypass = config
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .is_some_and(|provider| provider.use_proxy == Some(false));
    if bypass {
        None
    } else {
        config.proxy_url.clone()
    }
}

/// 通道协议解析（无凭证依赖）：首发通道固定，其余走 config provider_protocols。
pub(crate) fn channel_protocol(
    channel: Option<&channels::FirstPartyChannel>,
    config: &PaworkConfig,
    id: &str,
) -> Result<AdapterProtocol, AppError> {
    match channel.map(|channel| channel.kind.clone()) {
        Some(ChannelKind::ChatGptOAuth) | Some(ChannelKind::XaiOAuth) => {
            Ok(AdapterProtocol::Responses)
        }
        Some(ChannelKind::ApiKey) | Some(ChannelKind::KimiOAuth) => {
            Ok(AdapterProtocol::ChatCompletions)
        }
        None => Ok(resolve_adapter_protocol(config, id)?),
    }
}

/// 目录装配（无凭证依赖）：builtin + 协议静态目录 + config 覆盖 + transport。
pub(crate) fn assemble_registry(
    config: &PaworkConfig,
    provider_id: &ProviderId,
    protocol: AdapterProtocol,
    channel: Option<&channels::FirstPartyChannel>,
) -> ModelRegistry {
    let mut registry = ModelRegistry::builtin();
    if protocol == AdapterProtocol::Messages {
        registry.merge_provider_models(provider_id, &pawork_providers::builtin_models());
    }
    if channel.is_some_and(|channel| channel.kind == ChannelKind::XaiOAuth) {
        registry.merge_provider_models(provider_id, &pawork_providers::xai_builtin_models());
    }
    if channel.is_some_and(|channel| channel.kind == ChannelKind::KimiOAuth) {
        registry.merge_provider_models(provider_id, &pawork_providers::kimi_code_builtin_models());
    }
    apply_config_models(&mut registry, &config.models, provider_id);
    apply_transport_overrides(&mut registry, config);
    registry
}

/// 装配产物：adapter + 凭证 + 协议标记 + 全量 registry。
pub(crate) struct AssembledProvider {
    pub(crate) adapter: Arc<dyn ModelProvider>,
    pub(crate) credential: Option<ResolvedCredential>,
    pub(crate) protocol: AdapterProtocol,
    pub(crate) registry: ModelRegistry,
}

/// 统一装配入口（S6 波 C）：首发通道走通道表，其余走 config + 协议解析。
///
/// 这是 host 装配层唯一的 Provider 选择点；Engine 仍只看 trait 对象。
/// `refresh_oauth = true` 时 OAuth 凭证先走请求前刷新（网络）。
pub(crate) async fn assemble_provider(
    config: &PaworkConfig,
    provider_id: &ProviderId,
    backend: &Arc<dyn SecretBackend>,
    refresh_oauth: bool,
    reasoning_protector: Arc<dyn ReasoningProtector>,
) -> Result<AssembledProvider, AppError> {
    let id = provider_id.as_str();
    let channel = channels::first_party_channel(id);
    let config_base = find_provider(&config.providers, id)
        .ok()
        .and_then(|provider| provider.base_url.clone());
    let protocol = channel_protocol(channel, config, id)?;
    let registry = assemble_registry(config, provider_id, protocol, channel);
    let registry_arc = Arc::new(registry.clone());

    let (adapter, credential, protocol) = match channel.map(|channel| channel.kind.clone()) {
        Some(ChannelKind::ChatGptOAuth) => {
            let (credential, account_id) =
                oauth_credential(config, id, backend, refresh_oauth).await?;
            let account_id = account_id.ok_or_else(|| {
                AppError::OAuthLogin(
                    "ChatGPT account id missing; re-run pawork auth login chatgpt".into(),
                )
            })?;
            let base_url =
                config_base.unwrap_or_else(|| channel.expect("channel").default_base_url.into());
            let mut chatgpt_config =
                pawork_providers::ChatGptConfig::new(account_id).with_base_url(base_url);
            chatgpt_config.http.proxy = provider_proxy(config, id);
            let provider =
                pawork_providers::ChatGptProvider::new(chatgpt_config, Some(credential.clone()))?
                    .with_reasoning_protector(Arc::clone(&reasoning_protector));
            (
                Arc::new(provider) as Arc<dyn ModelProvider>,
                Some(credential),
                AdapterProtocol::Responses,
            )
        }
        Some(ChannelKind::XaiOAuth) => {
            // SET-4 A3 双认证：按实际存储形态解析——先 api key（auth 文件或
            // env fallback），无则走 OAuth（含请求前刷新）；都不在才 fail-closed。
            let credential = match try_api_key_credential(backend, id)? {
                Some((credential, _)) => credential,
                None => {
                    oauth_credential(config, id, backend, refresh_oauth)
                        .await?
                        .0
                }
            };
            let base_url =
                config_base.unwrap_or_else(|| channel.expect("channel").default_base_url.into());
            let mut xai_config = pawork_providers::XaiConfig::new(base_url);
            xai_config.http.proxy = provider_proxy(config, id);
            let provider =
                pawork_providers::XaiProvider::new(xai_config, Some(credential.clone()))?
                    .with_reasoning_protector(Arc::clone(&reasoning_protector));
            (
                Arc::new(provider) as Arc<dyn ModelProvider>,
                Some(credential),
                AdapterProtocol::Responses,
            )
        }
        Some(ChannelKind::KimiOAuth) => {
            let (credential, _) = oauth_credential(config, id, backend, refresh_oauth).await?;
            let base_url =
                config_base.unwrap_or_else(|| channel.expect("channel").default_base_url.into());
            let mut kimi_config = pawork_providers::KimiCodeConfig::new(base_url);
            kimi_config.http.proxy = provider_proxy(config, id);
            let provider =
                pawork_providers::KimiCodeProvider::new(kimi_config, Some(credential.clone()))?;
            (
                Arc::new(provider) as Arc<dyn ModelProvider>,
                Some(credential),
                AdapterProtocol::ChatCompletions,
            )
        }
        Some(ChannelKind::ApiKey) => {
            let preset = channels::api_key_channel(id)
                .filter(|preset| pawork_providers::is_enabled(*preset))
                .ok_or_else(|| AppError::UnknownProvider { id: id.to_string() })?;
            let (credential, _source) = resolve_api_key_credential(backend, id)?;
            let mut channel_config = ApiKeyChannelConfig::new(preset)?;
            channel_config.http.proxy = provider_proxy(config, id);
            if let Some(base_url) = config_base {
                channel_config = channel_config.with_base_url(base_url);
            }
            for (model, transport) in model_transport_overrides(config) {
                channel_config = channel_config.with_model_transport(model, transport);
            }
            let provider = ApiKeyChannelProvider::new(channel_config, Some(credential.clone()))?
                .with_reasoning_protector(Arc::clone(&reasoning_protector));
            (
                Arc::new(provider) as Arc<dyn ModelProvider>,
                Some(credential),
                AdapterProtocol::ChatCompletions,
            )
        }
        None => {
            let _provider = find_provider(&config.providers, id)?;
            let base_url =
                config_base.ok_or_else(|| AppError::MissingBaseUrl { id: id.to_string() })?;
            let (credential, _source) = resolve_api_key_credential(backend, id)?;
            let protocol = resolve_adapter_protocol(config, id)?;
            let adapter: Arc<dyn ModelProvider> = match protocol {
                AdapterProtocol::ChatCompletions => Arc::new(OpenAiCompatibleProvider::new(
                    {
                        let mut c = OpenAiCompatibleConfig::new(base_url)
                            .with_provider_id(provider_id.as_str().to_string());
                        c.http.proxy = provider_proxy(config, id);
                        c
                    },
                    Some(credential.clone()),
                )?),
                AdapterProtocol::Messages => Arc::new(
                    AnthropicProvider::new(
                        {
                            let mut c = AnthropicConfig::new(base_url)
                                .with_provider_id(provider_id.as_str().to_string());
                            c.http.proxy = provider_proxy(config, id);
                            c
                        },
                        Some(credential.clone()),
                    )?
                    .with_reasoning_protector(Arc::clone(&reasoning_protector))
                    .with_registry(registry_arc),
                ),
                AdapterProtocol::Responses => {
                    return Err(AppError::Protocol(crate::ProtocolError::Unknown {
                        provider: id.to_string(),
                        value: "responses".to_string(),
                    }))
                }
            };
            (adapter, Some(credential), protocol)
        }
    };

    Ok(AssembledProvider {
        adapter,
        credential,
        protocol,
        registry,
    })
}
/// API key 凭证链（可选形态）：auth 文件 → env fallback → None。
fn try_api_key_credential(
    backend: &Arc<dyn SecretBackend>,
    id: &str,
) -> Result<Option<(ResolvedCredential, crate::AuthSource)>, AppError> {
    match resolve_provider_credential(backend.as_ref(), id)? {
        CredentialSource::AuthFile(stored) => {
            let credential = ApiKeyCredential::from_stored(stored)?.resolve(backend.as_ref())?;
            Ok(Some((credential, crate::AuthSource::File)))
        }
        CredentialSource::EnvFallback(credential) => Ok(Some((credential, crate::AuthSource::Env))),
        CredentialSource::None => Ok(None),
    }
}

/// API key 凭证链：auth 文件 → env fallback → fail-closed。
fn resolve_api_key_credential(
    backend: &Arc<dyn SecretBackend>,
    id: &str,
) -> Result<(ResolvedCredential, crate::AuthSource), AppError> {
    try_api_key_credential(backend, id)?.ok_or_else(|| AppError::MissingCredential {
        provider: id.to_string(),
        env_name: api_key_env_name(id),
    })
}

/// OAuth 凭证解析：default 条目（meta）→（可选）请求前刷新 → bearer。
async fn oauth_credential(
    config: &PaworkConfig,
    id: &str,
    backend: &Arc<dyn SecretBackend>,
    refresh: bool,
) -> Result<(ResolvedCredential, Option<String>), AppError> {
    let provider = ProviderId::new(id);
    let Some(mut stored) = load_default_oauth_credential(backend.as_ref(), &provider)? else {
        return Err(AppError::OAuthLoginRequired(id.to_string()));
    };
    if refresh {
        let preset = oauth_refresh_endpoint(config, id)?;
        let http = AppCore::http_from_config(config)?;
        let refresh_config = OAuthRefreshConfig {
            token_url: preset.token_url,
            client_id: preset.client_id,
            refresh_skew: Duration::from_secs(30),
        };
        match refresh_default_oauth_credential_if_needed(
            &mut stored,
            backend.as_ref(),
            &refresh_config,
            &http,
        )
        .await
        {
            Ok(_) => {}
            Err(AuthError::TokenEndpoint { error, .. }) if error == "invalid_grant" => {
                return Err(AppError::OAuthLogin(format!(
                    "provider {id} 的 OAuth refresh token 已失效；请运行 pawork auth login {id} 重新登录"
                )))
            }
            Err(error) => return Err(AppError::Auth(error)),
        }
    }
    let account_id =
        load_default_oauth_meta(backend.as_ref(), &provider)?.and_then(|meta| meta.account_id);
    let credential = resolve_oauth_credential(&stored, backend.as_ref())?;
    Ok((credential, account_id))
}

/// OAuth 刷新端点：config [oauth.<id>] 覆盖 → 通道预设（xAI 无预设则报错）。
pub(crate) fn oauth_refresh_endpoint(
    config: &PaworkConfig,
    id: &str,
) -> Result<channels::OAuthPreset, AppError> {
    if let Some(preset) = channels::oauth_override(config, id) {
        return Ok(preset);
    }
    channels::first_party_channel(id)
        .and_then(|channel| channel.oauth_preset())
        .ok_or_else(|| {
            AppError::OAuthLogin(format!(
                "provider {id} has no OAuth endpoint preset; configure [oauth.{id}] first"
            ))
        })
}

/// extra["model_transports"]：{"model-id": "responses"|"chat_completions"|"messages"}。
fn model_transport_overrides(
    config: &PaworkConfig,
) -> Vec<(String, pawork_domain::ModelTransport)> {
    let Some(table) = config.extra.get("model_transports") else {
        return Vec::new();
    };
    let Some(map) = table.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(model, value)| {
            let transport = match value.as_str()? {
                "responses" => pawork_domain::ModelTransport::Responses,
                "chat_completions" | "openai-compatible" => {
                    pawork_domain::ModelTransport::ChatCompletions
                }
                "messages" | "anthropic-messages" => pawork_domain::ModelTransport::Messages,
                _ => return None,
            };
            Some((model.clone(), transport))
        })
        .collect()
}

/// 把 transport 覆盖应用到 registry 条目（混合协议模型显式声明，不按渠道猜）。
fn apply_transport_overrides(registry: &mut ModelRegistry, config: &PaworkConfig) {
    for (model, transport) in model_transport_overrides(config) {
        if let Some(mut entry) = registry.resolve(&model).cloned() {
            entry.capabilities.transport = transport;
            registry.extend_with(vec![entry]);
        }
    }
}

/// 把 config `[[models]]` 覆盖并入 registry：已有条目只改 window / max_output
/// （能力、定价、别名保持目录权威），未知条目追加（provider 归当前 provider，
/// 能力 fail-closed 全 false，定价 None——不编造）。
pub(crate) fn apply_config_models(
    registry: &mut ModelRegistry,
    models: &[pawork_workspace::config::ModelConfig],
    provider_id: &ProviderId,
) {
    for config in models {
        let mut entry = match registry.resolve(&config.id) {
            Some(existing) => existing.clone(),
            None => CatalogEntry {
                id: pawork_domain::ModelId::new(&config.id),
                provider: provider_id.clone(),
                display_name: config.id.clone(),
                context_window_tokens: 0,
                max_output_tokens: 0,
                capabilities: Default::default(),
                pricing: None,
                aliases: Vec::new(),
            },
        };
        if let Some(window) = config.context_window {
            entry.context_window_tokens = window;
        }
        if let Some(max_output) = config.max_output {
            entry.max_output_tokens = max_output;
        }
        registry.extend_with(vec![entry]);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use pawork_auth::locator::api_key_env_name;
    use pawork_auth::SecretBackend;
    use pawork_domain::{
        AgentEvent, ModelId, ModelResponseSummary, ProviderId, StopReason, TokenUsage,
    };
    use pawork_providers::ModelRegistry;
    use pawork_storage::session::SessionStore;
    use pawork_workspace::config::{PaworkConfig, ProviderConfig};
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::testsupport::{
        core_with_registry, mock_core, remove_env, sample_config, set_env, ScriptedProvider,
    };
    use crate::{AdapterProtocol, AppCore, AppError};
    use pawork_providers::ReasoningProtector;

    use super::*;

    #[test]
    fn from_resolved_requires_provider_and_model() {
        let err =
            AppCore::from_resolved(PaworkConfig::default(), None, None).expect_err("empty config");
        assert!(matches!(err, AppError::MissingDefaultProvider));

        let err = AppCore::from_resolved(
            PaworkConfig {
                default_provider: Some("missing".into()),
                default_model: Some("m".into()),
                ..PaworkConfig::default()
            },
            None,
            None,
        )
        .expect_err("unknown provider");
        assert!(matches!(err, AppError::UnknownProvider { id } if id == "missing"));
    }

    #[tokio::test]
    async fn catalog_load_allows_zero_config_for_auth_and_models() {
        let core = AppCore::from_config_inner(
            PaworkConfig::default(),
            None,
            None,
            Arc::new(pawork_auth::MemoryBackend::new()),
            true,
        )
        .await
        .expect("catalog load tolerates missing defaults");
        assert_eq!(core.provider_id.as_str(), "catalog");
        // auth list 在零配置下可列出六通道（全部 none 来源）。
        let rows = core.auth_status().expect("auth status");
        assert!(rows.iter().any(|row| row.provider == "xai"));
    }

    #[tokio::test]
    async fn chat_load_still_fails_closed_without_defaults() {
        let err = AppCore::from_config_inner(
            PaworkConfig::default(),
            None,
            None,
            Arc::new(pawork_auth::MemoryBackend::new()),
            false,
        )
        .await
        .expect_err("chat load must fail");
        assert!(matches!(err, AppError::MissingDefaultProvider));
    }

    #[test]
    fn from_resolved_fail_closed_without_env_key() {
        let id = "app-core-missing-key";
        remove_env(&api_key_env_name(id));
        let err = AppCore::from_resolved(sample_config(id), None, None).expect_err("no key");
        let display = format!("{err}");
        match err {
            AppError::MissingCredential { provider, env_name } => {
                assert_eq!(provider, id);
                assert_eq!(env_name, "PAWORK_API_KEY_APP_CORE_MISSING_KEY");
                assert!(display.contains("PAWORK_API_KEY_APP_CORE_MISSING_KEY"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn models_overview_aggregates_six_channels() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let overview = core.models_overview().await;
        let providers: std::collections::BTreeSet<String> = overview
            .iter()
            .map(|entry| entry.provider.as_str().to_string())
            .collect();
        // chatgpt 无静态目录（Codex backend 模型只能登录后运行期探测）。
        for expected in [
            "xai",
            "glm-coding",
            "opencode-go",
            "qwen-token-plan",
            "deepseek",
        ] {
            assert!(
                providers.contains(expected),
                "missing provider {expected} in overview: {providers:?}"
            );
        }
        assert!(
            overview.iter().any(|entry| entry.id.as_str() == "grok-4"),
            "xai static models missing"
        );
    }

    #[tokio::test]
    async fn switch_provider_accepts_runtime_discovered_model() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "runtime-only-model"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider_id = ProviderId::from("runtime-catalog-provider");
        let backend = Arc::new(pawork_auth::MemoryBackend::new());
        pawork_auth::store_default_api_key(backend.as_ref(), &provider_id, "not-a-real-key")
            .expect("store test credential");
        let backend: Arc<dyn SecretBackend> = backend;
        let config = PaworkConfig {
            providers: vec![ProviderConfig {
                id: provider_id.as_str().into(),
                base_url: Some(format!("{}/v1", server.uri())),
                ..ProviderConfig::default()
            }],
            ..PaworkConfig::default()
        };
        let mut core =
            core_with_registry(ModelRegistry::empty(), "initial").with_state(config, backend);

        core.switch_provider(None, provider_id.as_str(), Some("runtime-only-model"))
            .await
            .expect("ModelList runtime entry must be selectable");

        assert_eq!(core.provider_id(), &provider_id);
        assert_eq!(core.model().as_str(), "runtime-only-model");
        assert_eq!(
            core.registry
                .resolve("runtime-only-model")
                .map(|entry| &entry.provider),
            Some(&provider_id)
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn switch_model_records_diagnostic_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(&dir.path().join("session.db"))
            .await
            .expect("store");
        let mut registry = ModelRegistry::empty();
        for id in ["m-a", "m-b"] {
            registry.extend_with(vec![CatalogEntry {
                id: ModelId::from(id),
                provider: ProviderId::from("mock"),
                display_name: id.into(),
                context_window_tokens: 8_000,
                max_output_tokens: 1_024,
                capabilities: Default::default(),
                pricing: None,
                aliases: Vec::new(),
            }]);
        }
        let mut core = AppCore::from_parts_with_protocol(
            Arc::new(ScriptedProvider {
                events: Vec::new(),
                summary: ModelResponseSummary {
                    stop_reason: StopReason::Completed,
                    usage: TokenUsage::default(),
                    response_id: None,
                    provider_metadata: Default::default(),
                },
                models: Vec::new(),
            }),
            None,
            ModelId::from("m-a"),
            ProviderId::from("mock"),
            AdapterProtocol::ChatCompletions,
            Some(store),
            registry,
        );
        let session = core.create_session("switch").await.expect("session");
        core.switch_model(Some(&session), "m-b")
            .await
            .expect("switch");
        let events = core
            .store()
            .expect("store")
            .replay_events(&session, 0, 100)
            .await
            .expect("replay");
        let switches: Vec<_> = events
            .iter()
            .filter(|envelope| {
                matches!(
                    &envelope.payload,
                    AgentEvent::Diagnostic { code, .. } if code == "model.switched"
                )
            })
            .collect();
        assert_eq!(switches.len(), 1, "model.switched event missing");
        match &switches[0].payload {
            AgentEvent::Diagnostic { details, .. } => {
                assert_eq!(details["from"]["model"], "m-a");
                assert_eq!(details["to"]["model"], "m-b");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn default_oauth_refresh_is_singleflight_and_persists_meta() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=old-refresh"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(serde_json::json!({
                        "access_token": "singleflight-access",
                        "refresh_token": "singleflight-refresh",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "scope": "openid profile"
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let backend: Arc<dyn SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
        let provider = ProviderId::new("xai");
        pawork_auth::store_default_oauth_token(
            backend.as_ref(),
            provider.clone(),
            &pawork_auth::TokenSet {
                access_token: "old-access".into(),
                refresh_token: Some("old-refresh".into()),
                id_token: None,
                expires_in: Some(0),
                token_type: "Bearer".into(),
                scope: Some("openid".into()),
            },
        )
        .expect("store default oauth");

        let mut config = PaworkConfig::default();
        config.extra.insert(
            "oauth".into(),
            serde_json::json!({
                "xai": {
                    "client_id": "client-id",
                    "device_auth_url": "https://example.test/device/code",
                    "token_url": format!("{}/token", server.uri()),
                    "scopes": ["openid", "profile"]
                }
            }),
        );

        let (first, second) = tokio::join!(
            oauth_credential(&config, "xai", &backend, true),
            oauth_credential(&config, "xai", &backend, true)
        );
        for result in [first, second] {
            let (credential, account_id) = result.expect("oauth credential");
            assert_eq!(credential.expose_secret(), "singleflight-access");
            assert!(account_id.is_none());
        }

        let stored = load_default_oauth_credential(backend.as_ref(), &provider)
            .expect("load default oauth")
            .expect("default oauth present");
        assert_eq!(
            pawork_auth::read_refresh_token(&stored, backend.as_ref()).expect("rotated refresh"),
            "singleflight-refresh"
        );
        let meta = load_default_oauth_meta(backend.as_ref(), &provider)
            .expect("load meta")
            .expect("meta present");
        assert_eq!(meta.masked, stored.masked);
        assert_eq!(
            meta.expires_at_ms,
            stored.expires_at.map(|value| value.as_unix_millis())
        );
        assert_eq!(meta.scopes, vec!["openid", "profile"]);
        server.verify().await;
    }

    #[tokio::test]
    async fn permanent_oauth_refresh_failure_requires_relogin_without_secret_leak() {
        let server = MockServer::start().await;
        let endpoint_description = "rotated credential is permanently invalid: secret-sentinel";
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=invalid-old-refresh"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": endpoint_description
            })))
            .expect(1)
            .mount(&server)
            .await;

        let backend: Arc<dyn SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
        let provider = ProviderId::new("xai");
        let stored = pawork_auth::store_default_oauth_token(
            backend.as_ref(),
            provider.clone(),
            &pawork_auth::TokenSet {
                access_token: "invalid-old-access".into(),
                refresh_token: Some("invalid-old-refresh".into()),
                id_token: None,
                expires_in: Some(0),
                token_type: "Bearer".into(),
                scope: Some("openid".into()),
            },
        )
        .expect("store default oauth");

        let mut config = PaworkConfig::default();
        config.extra.insert(
            "oauth".into(),
            serde_json::json!({
                "xai": {
                    "client_id": "client-id",
                    "device_auth_url": "https://example.test/device/code",
                    "token_url": format!("{}/token", server.uri()),
                    "scopes": ["openid"]
                }
            }),
        );

        let error = oauth_credential(&config, "xai", &backend, true)
            .await
            .expect_err("invalid_grant must fail closed");
        let message = error.to_string();
        assert!(message.contains("pawork auth login xai"));
        assert!(!message.contains(endpoint_description));
        assert!(!message.contains("secret-sentinel"));
        assert!(!message.contains("invalid-old-refresh"));
        assert_eq!(
            backend
                .get(&stored.secret_service, &stored.secret_account)
                .expect("access remains unchanged"),
            "invalid-old-access"
        );
        assert_eq!(
            pawork_auth::read_refresh_token(&stored, backend.as_ref())
                .expect("refresh remains unchanged"),
            "invalid-old-refresh"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn catalog_load_tolerates_missing_credential() {
        // 独立 provider id：避免与并行 env 测试共享同一环境变量。
        let id = "deepseek";
        remove_env(&api_key_env_name(id));
        let mut config = sample_config(id);
        config.default_model = Some("glm-5.2".into());
        let backend: Arc<dyn SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
        let strict = AppCore::from_config(config.clone(), None, None, backend.clone()).await;
        assert!(matches!(
            strict,
            Err(AppError::MissingCredential { provider, .. }) if provider == id
        ));
        // RecordingCapture 双注册钉住 interest 缓存：该 degrade callsite 亦被无
        // subscriber 的 from_config_inner 调用路径共享，裸 set_default 会间歇丢事件。
        let capture = crate::testsupport::RecordingCapture::install();
        let core = AppCore::from_config_inner(config, None, None, backend, true)
            .await
            .expect("catalog load");
        assert!(core.provider_pending(), "core should be pending");
        let events = capture.events();
        capture.dismiss();
        let emitted = events
            .iter()
            .find(|event| {
                event.fields.get("code").map(String::as_str) == Some("degrade.missing_credential")
            })
            .unwrap_or_else(|| panic!("missing credential must emit tracing: {events:?}"));
        assert_eq!(emitted.level, "WARN");
        assert_eq!(
            emitted.fields.get("provider_id").map(String::as_str),
            Some(id),
            "details must only contain provider_id: {emitted:?}"
        );
        let field_names: Vec<&str> = emitted.fields.keys().map(String::as_str).collect();
        assert!(
            field_names
                .iter()
                .all(|name| matches!(*name, "code" | "provider_id")),
            "details must only contain provider_id: {emitted:?}"
        );
        let encoded = format!("{emitted:?}").to_lowercase();
        assert!(!encoded.contains("secret"), "{encoded}");
        assert!(!encoded.contains("token"), "{encoded}");
        assert!(!emitted.message.to_lowercase().contains("secret"));
        let overview = core.models_overview().await;
        assert!(overview.iter().any(|entry| entry.id.as_str() == "glm-5.2"));
    }

    #[test]
    fn cli_overrides_win_and_secret_is_redacted() {
        let id = "app-core-redact";
        let env_name = api_key_env_name(id);
        let secret = "super-secret-key-value-not-for-logs";
        set_env(&env_name, secret);
        let core = AppCore::from_resolved(sample_config(id), Some(id), Some("deepseek-v4-pro"))
            .expect("load with key");
        remove_env(&env_name);

        assert_eq!(core.provider_id().as_str(), id);
        assert_eq!(core.model().as_str(), "deepseek-v4-pro");
        assert_eq!(core.adapter_protocol(), AdapterProtocol::ChatCompletions);
        let debug = format!("{core:?}");
        assert!(!debug.contains(secret), "secret leaked in Debug: {debug}");
        assert!(debug.contains("[REDACTED]"), "{debug}");
    }

    #[test]
    fn from_resolved_selects_messages_adapter_from_default_table() {
        let id = "glm-coding-anthropic";
        let env_name = api_key_env_name(id);
        set_env(&env_name, "not-a-real-key");
        let core = AppCore::from_resolved(sample_config(id), None, None).expect("load");
        remove_env(&env_name);
        assert_eq!(core.adapter_protocol(), AdapterProtocol::Messages);
        assert_eq!(core.provider_id().as_str(), id);
    }

    #[test]
    fn extra_protocol_overrides_default_and_rejects_unknown() {
        let id = "app-core-protocol-extra";
        let env_name = api_key_env_name(id);
        set_env(&env_name, "not-a-real-key");
        let mut config = sample_config(id);
        config.extra.insert(
            "provider_protocols".into(),
            serde_json::json!({ id: "messages" }),
        );
        let core = AppCore::from_resolved(config.clone(), None, None).expect("override");
        assert_eq!(core.adapter_protocol(), AdapterProtocol::Messages);

        config.extra.insert(
            "provider_protocols".into(),
            serde_json::json!({ id: "not-a-protocol" }),
        );
        let err = AppCore::from_resolved(config, None, None).expect_err("bad protocol");
        remove_env(&env_name);
        assert!(matches!(err, AppError::Protocol(_)));
    }

    #[tokio::test]
    async fn assemble_provider_injects_reasoning_protector_for_messages() {
        let id = "app-core-protector-inject";
        let env_name = api_key_env_name(id);
        set_env(&env_name, "not-a-real-key");
        let mut config = sample_config(id);
        config.extra.insert(
            "provider_protocols".into(),
            serde_json::json!({ id: "messages" }),
        );
        let backend: Arc<dyn SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
        let protector = Arc::new(crate::protected::SwappableReasoningProtector::in_memory());
        let assembled = assemble_provider(
            &config,
            &ProviderId::from(id),
            &backend,
            false,
            protector.clone() as Arc<dyn ReasoningProtector>,
        )
        .await
        .expect("assemble");
        assert_eq!(assembled.protocol, AdapterProtocol::Messages);
        let blob = protector.protect(b"sig").await.expect("protect");
        assert_eq!(protector.resolve(&blob).await.expect("resolve"), b"sig");
        remove_env(&env_name);
    }
}
