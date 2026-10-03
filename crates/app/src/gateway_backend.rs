//! Stateless, provider-neutral model access for the local HTTP gateway.
use crate::provider_assembly::{assemble_provider, resolve_provider_model};
use crate::{AdapterProtocol, AppCore, AppError};
use async_trait::async_trait;
use pawork_control_plane::credential::{AcquireRequest, LeaseOutcome};
use pawork_domain::*;
use pawork_gateway::{
    GatewayBackend, GatewayChatRequest, GatewayContent, GatewayContentPart, GatewayError,
    GatewayModel, GatewayVideoRequest,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

impl From<AppError> for GatewayError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::UnknownModel { .. }
            | AppError::UnknownProvider { .. }
            | AppError::ModelDisabled { .. }
            | AppError::ModelBelongsToProvider { .. } => {
                Self::new(404, "model_not_found", "Model is unknown or disabled.")
            }
            AppError::Provider(error) => Self::from(error),
            _ => Self::new(
                503,
                "provider_unavailable",
                "Provider or credentials unavailable. Check Pawork settings.",
            ),
        }
    }
}

/// Gateway v1.1 多模态正文 → canonical ContentPart。
///
/// URL 只做语法边界校验（长度 / 无空白控制符 / 凭证拒绝），Host 不下载、
/// 不解码、不转存；media_type 按扩展名 / data URL 前缀推断，未知图像
/// 扩展回落 image/png（所有已接通道的公共白名单项）。
fn gateway_content_parts(content: GatewayContent) -> Result<Vec<ContentPart>, GatewayError> {
    let parts = match content {
        GatewayContent::Text(text) => return Ok(vec![ContentPart::Text(TextContent { text })]),
        GatewayContent::Parts(parts) => parts,
    };
    if parts.is_empty() {
        return Err(GatewayError::invalid());
    }
    let mut converted = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            GatewayContentPart::Text { text } => {
                converted.push(ContentPart::Text(TextContent { text }));
            }
            GatewayContentPart::ImageUrl { image_url } => {
                converted.push(ContentPart::Image(image_part(image_url.url)?));
            }
            GatewayContentPart::VideoUrl { video_url } => {
                let video = VideoContent {
                    url: video_url.url,
                    media_type: "video/mp4".into(),
                };
                video.validate().map_err(|_| GatewayError::invalid())?;
                converted.push(ContentPart::Video(video));
            }
        }
    }
    Ok(converted)
}

fn image_part(url: String) -> Result<ImageContent, GatewayError> {
    let invalid = || GatewayError::invalid();
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid());
    }
    if let Some(rest) = url.strip_prefix("data:") {
        let (header, data) = rest.split_once(',').ok_or_else(invalid)?;
        let media_type = header.strip_suffix(";base64").ok_or_else(invalid)?;
        if !media_type.starts_with("image/")
            || media_type.contains(';')
            || data.is_empty()
            || !data
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
        {
            return Err(invalid());
        }
        return Ok(ImageContent {
            source: ImageSource::Base64(data.to_string()),
            media_type: media_type.to_string(),
            alt_text: None,
        });
    }
    // 内嵌图片受 HTTP 请求体 2 MiB 上限约束；8 KiB 只限制远程 URL。
    if url.len() > 8192 {
        return Err(invalid());
    }
    url.strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|host| !host.is_empty() && !host.contains(['@', '%']))
        .ok_or_else(invalid)?;
    let media_type = match url
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    };
    Ok(ImageContent {
        source: ImageSource::Url(url),
        media_type: media_type.into(),
        alt_text: None,
    })
}
pub struct GatewayCompletion {
    provider: Arc<dyn ModelProvider>,
    request: CanonicalModelRequest,
    provider_id: ProviderId,
    account_id: String,
    credential_id: Option<String>,
}

const VIDEO_PROVIDER: &str = "qwen-token-plan";

fn gateway_request_id() -> Result<RequestId, GatewayError> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| {
        GatewayError::new(
            500,
            "entropy_unavailable",
            "Cannot create request identity.",
        )
    })?;
    Ok(RequestId::new(format!(
        "gateway-{}",
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )))
}

enum VideoOperation<'a> {
    Submit(&'a str),
    Query(&'a str),
}

impl AppCore {
    fn gateway_video_client(
        &self,
        account: Option<&str>,
    ) -> Result<
        (
            pawork_providers::token_plan_video::TokenPlanVideoClient,
            String,
        ),
        AppError,
    > {
        use pawork_auth::{ApiKeyCredential, CredentialSource};
        // 账号身份与 Secret 在同一个后端事务中读取，不装配无关的 Chat adapter。
        let provider = ProviderId::new(VIDEO_PROVIDER);
        let mut resolved = None;
        self.backend.transaction(&mut |backend| {
            let source = match account {
                None => pawork_auth::resolve_provider_credential(backend, VIDEO_PROVIDER)?,
                Some("environment") => {
                    match pawork_auth::locator::read_api_key_from_env(VIDEO_PROVIDER) {
                        Some(secret) => CredentialSource::EnvFallback(ResolvedCredential::new(
                            CredentialKind::ApiKey,
                            secret,
                        )),
                        None => CredentialSource::None,
                    }
                }
                Some(id) => {
                    let inventory = pawork_auth::list_provider_accounts(backend, &provider)?;
                    let entry = inventory
                        .accounts
                        .into_iter()
                        .find(|entry| {
                            entry.credential_id == id
                                && entry.kind == pawork_auth::ProviderAccountKind::ApiKey
                        })
                        .ok_or(pawork_auth::AuthError::NotFound)?;
                    CredentialSource::AuthFile(entry.stored)
                }
            };
            resolved = match source {
                CredentialSource::AuthFile(stored) => {
                    let id = if stored.secret_account == "default" {
                        pawork_auth::LEGACY_API_KEY_ID.to_string()
                    } else {
                        stored.id.to_string()
                    };
                    Some((ApiKeyCredential::from_stored(stored)?.resolve(backend)?, id))
                }
                CredentialSource::EnvFallback(credential) => {
                    Some((credential, "environment".into()))
                }
                CredentialSource::None => None,
            };
            Ok(())
        })?;
        let (credential, account) = resolved.ok_or_else(|| AppError::MissingCredential {
            provider: VIDEO_PROVIDER.into(),
            env_name: pawork_auth::locator::api_key_env_name(VIDEO_PROVIDER),
        })?;
        let config =
            crate::provider_assembly::api_key_channel_config(&self.config, VIDEO_PROVIDER)?;
        Ok((
            pawork_providers::token_plan_video::TokenPlanVideoClient::new(config, credential)?,
            account,
        ))
    }

    async fn execute_gateway_video(
        &self,
        client: &str,
        provider: pawork_providers::token_plan_video::TokenPlanVideoClient,
        account: String,
        operation: VideoOperation<'_>,
        cancel: CancellationToken,
    ) -> Result<VideoGenerationTask, GatewayError> {
        let request_id = gateway_request_id()?;
        let mut lease = self
            .usage
            .control
            .pool
            .acquire_guard(AcquireRequest {
                tenant_id: TenantId::new(format!("thirdparty/{client}")),
                principal_id: PrincipalId::new(client),
                session_id: SessionId::new(request_id.as_str()),
                agent_id: AgentId::new("gateway"),
                provider_id: Some(ProviderId::new(VIDEO_PROVIDER)),
                account_id: Some(AccountId::new(format!("{VIDEO_PROVIDER}/{account}"))),
                trace_id: None,
            })
            .await
            .map_err(|_| {
                GatewayError::new(429, "concurrency_limit", "Too many concurrent requests.")
            })?;
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(GatewayError::cancelled()),
            result = tokio::time::timeout(std::time::Duration::from_secs(60), async {
                match operation {
                    VideoOperation::Submit(prompt) => provider.submit(prompt, cancel.clone()).await,
                    VideoOperation::Query(id) => provider.query(id, cancel.clone()).await,
                }
            }) => match result {
                Ok(result) => result.map_err(GatewayError::from),
                Err(_) => { cancel.cancel(); Err(GatewayError::timeout()) }
            }
        };
        let outcome = match &result {
            Ok(_) => LeaseOutcome::Completed,
            Err(error) if error.code == "cancelled" => LeaseOutcome::Cancelled,
            Err(_) => LeaseOutcome::Failed,
        };
        *lease.outcome_mut() = outcome;
        if let Some(lease) = lease.into_lease() {
            self.usage
                .control
                .pool
                .release(lease.lease_id, outcome)
                .await
                .map_err(|_| {
                    GatewayError::new(500, "lease_unavailable", "Cannot release model lease.")
                })?;
        }
        result.map(|mut task| {
            // 可持久保存的账号限定 ID；客户端原样保存，不解析内部组成。
            task.id = format!("{account}.{}", task.id);
            task.model = format!("{VIDEO_PROVIDER}/{}", task.model);
            task
        })
    }
}

#[async_trait]
impl GatewayBackend for AppCore {
    type Completion = GatewayCompletion;

    async fn gateway_video_models(&self) -> Result<Vec<VideoGenerationModel>, GatewayError> {
        use pawork_providers::token_plan_video::TOKEN_PLAN_VIDEO_MODEL;
        if !self
            .config
            .is_model_enabled(VIDEO_PROVIDER, TOKEN_PLAN_VIDEO_MODEL)
        {
            return Ok(Vec::new());
        }
        match self.gateway_video_client(None) {
            Ok(_) => Ok(vec![VideoGenerationModel {
                id: format!("{VIDEO_PROVIDER}/{TOKEN_PLAN_VIDEO_MODEL}"),
                display_name: "HappyHorse 1.1 · 5 秒 / 720P / 16:9".into(),
            }]),
            Err(AppError::MissingCredential { .. }) => Ok(Vec::new()),
            Err(error) => Err(error.into()),
        }
    }

    async fn submit_gateway_video(
        &self,
        client: &str,
        input: GatewayVideoRequest,
        cancel: CancellationToken,
    ) -> Result<VideoGenerationTask, GatewayError> {
        use pawork_providers::token_plan_video::TOKEN_PLAN_VIDEO_MODEL;
        if !pawork_gateway::tokens::valid_client(client)
            || input.model != format!("{VIDEO_PROVIDER}/{TOKEN_PLAN_VIDEO_MODEL}")
        {
            return Err(GatewayError::invalid());
        }
        if !self
            .config
            .is_model_enabled(VIDEO_PROVIDER, TOKEN_PLAN_VIDEO_MODEL)
        {
            return Err(GatewayError::new(
                404,
                "model_not_found",
                "Video model is disabled.",
            ));
        }
        let (provider, account) = self.gateway_video_client(None)?;
        self.execute_gateway_video(
            client,
            provider,
            account,
            VideoOperation::Submit(&input.prompt),
            cancel,
        )
        .await
    }

    async fn query_gateway_video(
        &self,
        client: &str,
        id: &str,
        cancel: CancellationToken,
    ) -> Result<VideoGenerationTask, GatewayError> {
        let (account, id) = id
            .split_once('.')
            .filter(|(account, id)| {
                !account.is_empty() && pawork_providers::token_plan_video::valid_task_id(id)
            })
            .ok_or_else(GatewayError::invalid)?;
        if !pawork_gateway::tokens::valid_client(client) {
            return Err(GatewayError::invalid());
        }
        let (provider, account) = self.gateway_video_client(Some(account))?;
        self.execute_gateway_video(client, provider, account, VideoOperation::Query(id), cancel)
            .await
    }
    async fn gateway_models(
        &self,
        purposes: &[pawork_domain::ModelPurpose],
    ) -> Result<Vec<GatewayModel>, GatewayError> {
        let catalog = self.models_overview().await;
        // The regular settings catalog includes unconnected static choices; this API does not.
        let mut connected = std::collections::HashSet::new();
        let mut checked = std::collections::HashSet::new();
        for entry in &catalog {
            if checked.insert(entry.provider.clone())
                && (entry.provider == self.provider_id && !self.provider_needs_rebuild()
                    || assemble_provider(
                        &self.config,
                        &entry.provider,
                        &self.backend,
                        true,
                        self.reasoning_protector.clone(),
                    )
                    .await
                    .is_ok())
            {
                connected.insert(entry.provider.clone());
            }
        }
        Ok(catalog
            .into_iter()
            .filter(|e| {
                connected.contains(&e.provider)
                    && self
                        .config
                        .is_model_enabled(e.provider.as_str(), e.id.as_str())
            })
            // ADR-064（Gateway v1.1）：按 canonical 用途过滤；缺省保持
            // v1「可聊天」口径（仅 text 模型），第三方显式带用途才见
            // 图像生成等专用模型。
            .filter(|e| {
                let effective: &[pawork_domain::ModelPurpose] = if purposes.is_empty() {
                    &[pawork_domain::ModelPurpose::Text]
                } else {
                    purposes
                };
                effective.iter().all(|purpose| {
                    pawork_models::capabilities_support_purpose(&e.capabilities, *purpose)
                })
            })
            .map(|e| GatewayModel {
                id: format!("{}/{}", e.provider, e.id),
                object: "model",
                owned_by: e.provider.to_string(),
                display_name: e.display_name,
                context_window: e.context_window_tokens,
                capabilities: pawork_gateway::GatewayModelCapabilities {
                    text: e.capabilities.text,
                    image_input: e.capabilities.image_input,
                    image_output: e.capabilities.image_output,
                    video_input: e.capabilities.video_input,
                    web_search: e
                        .capabilities
                        .hosted_tool_tags
                        .contains(&pawork_domain::ToolCapabilityTag::WebSearch),
                },
            })
            .collect())
    }

    /// Validate and freeze routing before sending HTTP/SSE success headers.
    async fn prepare_gateway_completion(
        &self,
        input: GatewayChatRequest,
    ) -> Result<GatewayCompletion, GatewayError> {
        let (provider, model) = input
            .model
            .split_once('/')
            .filter(|(p, m)| !p.is_empty() && !m.is_empty())
            .ok_or_else(GatewayError::invalid)?;
        if !self.config.is_model_enabled(provider, model) {
            return Err(GatewayError::new(
                404,
                "model_not_found",
                "Model is unknown or disabled.",
            ));
        }
        if input.messages.is_empty()
            || input.messages.len() > 1024
            || input
                .temperature
                .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
            || input.max_tokens == Some(0)
            || input.max_completion_tokens == Some(0)
            || (input.max_tokens.is_some() && input.max_completion_tokens.is_some())
            || (!input.stream && input.stream_options.is_some())
        {
            return Err(GatewayError::invalid());
        }
        let mut messages = Vec::with_capacity(input.messages.len());
        for (i, m) in input.messages.into_iter().enumerate() {
            let role = match m.role.as_str() {
                "system" => MessageRole::System,
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                _ => return Err(GatewayError::invalid()),
            };
            messages.push(Message {
                id: MessageId::new(format!("gateway-{i}")),
                role,
                content: gateway_content_parts(m.content)?,
                metadata: Default::default(),
            });
        }
        let web_search = input.web_search;
        let provider_id = ProviderId::new(provider);
        self.select_account_for_provider(&provider_id, &CancellationToken::new())
            .await?;
        let revision = pawork_auth::provider_accounts_revision(self.backend.as_ref(), &provider_id)
            .map_err(AppError::from)?;
        let (adapter, credential, mut registry, protocol) =
            if provider_id == self.provider_id && !self.provider_needs_rebuild() {
                (
                    self.provider.clone(),
                    self.credential.clone(),
                    self.registry.as_ref().clone(),
                    self.adapter_protocol,
                )
            } else {
                let assembled = assemble_provider(
                    &self.config,
                    &provider_id,
                    &self.backend,
                    true,
                    self.reasoning_protector.clone(),
                )
                .await?;
                (
                    assembled.adapter,
                    assembled.credential,
                    assembled.registry,
                    assembled.protocol,
                )
            };
        let entry = resolve_provider_model(
            &mut registry,
            adapter.as_ref(),
            credential.as_ref(),
            &provider_id,
            model,
            &self.config,
        )
        .await?;
        let account = crate::auth::effective_provider_account(self.backend.as_ref(), &provider_id)?;
        if revision
            != pawork_auth::provider_accounts_revision(self.backend.as_ref(), &provider_id)
                .map_err(AppError::from)?
        {
            return Err(GatewayError::new(
                409,
                "account_changed",
                "Provider account changed. Retry the request.",
            ));
        }
        let request_id = gateway_request_id()?;
        let session_id = SessionId::new(request_id.as_str());
        let mut request = pawork_engine::assemble_request(request_id, entry.id.clone(), messages);
        request.session_id = Some(session_id);
        request.max_output_tokens = input.max_completion_tokens.or(input.max_tokens);
        request.temperature = input.temperature;
        if let Some(format) = input.response_format {
            let kind = format
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(GatewayError::invalid)?;
            request.response_format = match kind {
                "text" => ResponseFormat::Text,
                "json_object" => ResponseFormat::Json,
                "json_schema" => {
                    let schema = format
                        .get("json_schema")
                        .ok_or_else(GatewayError::invalid)?;
                    let name = schema
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(GatewayError::invalid)?;
                    let value = schema
                        .get("schema")
                        .filter(|v| v.is_object())
                        .ok_or_else(GatewayError::invalid)?;
                    ResponseFormat::JsonSchema {
                        name: name.into(),
                        schema: value.clone(),
                    }
                }
                _ => return Err(GatewayError::invalid()),
            };
            // Messages currently implements structured output as a prompt hint: never claim strict support.
            if (protocol == AdapterProtocol::Messages
                || entry.capabilities.transport == ModelTransport::Messages)
                && kind != "text"
            {
                return Err(GatewayError::new(
                    400,
                    "unsupported_response_format",
                    "This model transport does not support structured output.",
                ));
            }
            // Preserve strict/description instead of dropping them in the canonical adapter.
            let responses = protocol == AdapterProtocol::Responses
                || entry.capabilities.transport == ModelTransport::Responses;
            if responses {
                let mut translated = if kind == "json_schema" {
                    format["json_schema"].clone()
                } else {
                    serde_json::json!({})
                };
                translated["type"] = Value::String(kind.into());
                request
                    .provider_options
                    .insert("text".into(), serde_json::json!({"format":translated}));
            } else if kind != "text" {
                request
                    .provider_options
                    .insert("response_format".into(), format);
            }
        }
        let credential_id = account.map(|a| a.credential_id);
        // ADR-064（Gateway v1.1）：请求级 hosted web search 与多模态输入
        // 都走既有 capability_gate——按目录证据 fail-closed，不支持即
        // 400，不静默降级、不触网。
        if web_search {
            request.hosted_tools.push(pawork_domain::HostedToolRequest {
                name: "web_search".into(),
                kind: pawork_domain::ToolCapabilityTag::WebSearch,
                description: "Provider-hosted web search".into(),
                capabilities: Vec::new(),
                config: None,
            });
        }
        let evidence = registry
            .capability_evidence(entry.id.as_str())
            .filter(|evidence| evidence.provider.as_ref() == Some(&provider_id))
            .unwrap_or_else(|| pawork_models::registry::CapabilityEvidence {
                model: entry.id.clone(),
                provider: None,
                static_declared: None,
                probe_declared: None,
                override_declared: None,
            });
        pawork_models::negotiate::capability_gate(&evidence, &request)
            .map_err(GatewayError::from)?;
        let account_id = format!(
            "{}/{}",
            provider,
            credential_id.as_deref().unwrap_or("environment")
        );
        Ok(GatewayCompletion {
            provider: adapter,
            request,
            provider_id,
            account_id,
            credential_id,
        })
    }

    async fn run_gateway_completion(
        &self,
        client: &str,
        completion: GatewayCompletion,
        sink: Arc<dyn ProviderEventSink>,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, GatewayError> {
        if !pawork_gateway::tokens::valid_client(client) {
            return Err(GatewayError::invalid());
        }
        let tenant = TenantId::new(format!("thirdparty/{client}"));
        let session = SessionId::new(completion.request.request_id.as_str());
        let mut lease = self
            .usage
            .control
            .pool
            .acquire_guard(AcquireRequest {
                tenant_id: tenant.clone(),
                principal_id: PrincipalId::new(client),
                session_id: session.clone(),
                agent_id: AgentId::new("gateway"),
                provider_id: Some(completion.provider_id.clone()),
                account_id: Some(AccountId::new(&completion.account_id)),
                trace_id: None,
            })
            .await
            .map_err(|_| {
                GatewayError::new(
                    429,
                    "concurrency_limit",
                    "Too many concurrent model requests.",
                )
            })?;
        let sink = UsageSink {
            inner: sink,
            usage: Mutex::new(TokenUsage::default()),
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(GatewayError::cancelled()),
            result = tokio::time::timeout(std::time::Duration::from_secs(600),completion.provider.stream(&completion.request,&sink,cancel.clone())) => {
                match result { Ok(value)=>value.map_err(GatewayError::from),Err(_)=>{cancel.cancel();Err(GatewayError::timeout())} }
            }
        };
        let usage = match &result {
            Ok(s) => s.usage.clone(),
            Err(_) => sink.usage.lock().expect("usage").clone(),
        };
        let result = result.and_then(|summary| match summary.stop_reason {
            StopReason::Cancelled => Err(GatewayError::cancelled()),
            StopReason::Error | StopReason::ToolUse | StopReason::Other(_) => {
                Err(GatewayError::new(
                    502,
                    "upstream_error",
                    "Provider returned an unsupported completion outcome.",
                ))
            }
            _ => Ok(summary),
        });
        // ADR-064：图像生成模型经 chat 兼容端点可能不回传 usage；账本契约
        // 要求 token / cost 至少一项大于 0。零用量如实跳过记账：不伪造
        // token，也不因记账失败拒绝已成功的生成结果。
        let billable = usage
            .input_tokens
            .saturating_add(usage.output_tokens)
            .saturating_add(usage.cache_read_tokens)
            .saturating_add(usage.cache_write_tokens)
            > 0;
        if billable {
            let mut record = crate::control::usage_record(
                &session,
                &RunId::new(completion.request.request_id.as_str()),
                &completion.request.request_id,
                &completion.provider_id,
                &completion.request.model,
                &usage,
                0,
                "",
            );
            record.tenant_id = tenant;
            record.principal_id = PrincipalId::new(client);
            record.agent_id = AgentId::new("gateway");
            record.account_id = completion.account_id;
            record.credential_id = completion.credential_id;
            self.usage
                .control
                .ledger
                .record(record)
                .await
                .map_err(|_| {
                    GatewayError::new(500, "usage_unavailable", "Cannot persist model usage.")
                })?;
        }
        *lease.outcome_mut() = match &result {
            Ok(_) => LeaseOutcome::Completed,
            Err(e) if e.code == "cancelled" => LeaseOutcome::Cancelled,
            Err(_) => LeaseOutcome::Failed,
        };
        if let Some(value) = lease.into_lease() {
            self.usage
                .control
                .pool
                .release(
                    value.lease_id,
                    match &result {
                        Ok(_) => LeaseOutcome::Completed,
                        Err(e) if e.code == "cancelled" => LeaseOutcome::Cancelled,
                        Err(_) => LeaseOutcome::Failed,
                    },
                )
                .await
                .map_err(|_| {
                    GatewayError::new(500, "lease_unavailable", "Cannot release model lease.")
                })?;
        }
        result
    }
}
struct UsageSink {
    inner: Arc<dyn ProviderEventSink>,
    usage: Mutex<TokenUsage>,
}
#[async_trait]
impl ProviderEventSink for UsageSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        if let ProviderStreamEvent::UsageUpdated(usage) = &event {
            *self.usage.lock().expect("usage") = usage.clone();
        }
        if let ProviderStreamEvent::Error(error) = event {
            return Err(error);
        }
        self.inner.emit(event).await
    }
}
