//! Stateless, provider-neutral model access for the local HTTP gateway.
use crate::provider_assembly::{assemble_provider, resolve_provider_model};
use crate::{AdapterProtocol, AppCore, AppError};
use async_trait::async_trait;
use pawork_control_plane::credential::{AcquireRequest, LeaseOutcome};
use pawork_domain::*;
use pawork_gateway::{
    GatewayBackend, GatewayChatRequest, GatewayContent, GatewayContentPart, GatewayError,
    GatewayModel, GatewayUsageLink, GatewayVideoRequest, GatewayVideoResponse,
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
    usage_context: Option<TaskUsageContext>,
    operation: TaskUsageOperation,
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

fn usage_error(error: pawork_control_plane::UsageLedgerError) -> GatewayError {
    use pawork_control_plane::UsageLedgerError::*;
    match error {
        InvalidRecord { .. } => GatewayError::invalid(),
        Conflict { .. } => GatewayError::new(
            409,
            "usage_conflict",
            "Usage report conflicts with an existing record.",
        ),
        _ => GatewayError::new(
            500,
            "usage_unavailable",
            "Cannot access task usage records.",
        ),
    }
}
fn gateway_record(
    id: &str,
    client: &str,
    context: Option<TaskUsageContext>,
    operation: TaskUsageOperation,
    provider: Option<String>,
    model: Option<String>,
) -> TaskUsageRecord {
    TaskUsageRecord {
        id: id.into(),
        client: client.into(),
        source: TaskUsageSource::Gateway,
        context,
        operation,
        provider,
        model,
        started_at_ms: pawork_engine::now_timestamp().as_unix_millis(),
        finished_at_ms: None,
        status: TaskUsageStatus::Running,
        tokens: None,
        cost: None,
        output_images: None,
        planned_video_seconds: None,
        upstream_task_id: None,
        upstream_status: None,
        related_call_id: None,
        error_code: None,
    }
}
fn complete_record<T>(record: &mut TaskUsageRecord, result: &Result<T, GatewayError>) {
    record.finished_at_ms = Some(
        pawork_engine::now_timestamp()
            .as_unix_millis()
            .max(record.started_at_ms),
    );
    record.status = match result {
        Ok(_) => TaskUsageStatus::Succeeded,
        Err(e) if e.code == "cancelled" => TaskUsageStatus::Cancelled,
        Err(_) => TaskUsageStatus::Failed,
    };
    record.error_code = result.as_ref().err().map(|e| e.code.into());
}
fn video_status(status: VideoTaskStatus) -> TaskUsageStatus {
    match status {
        VideoTaskStatus::Pending | VideoTaskStatus::Running => TaskUsageStatus::Submitted,
        VideoTaskStatus::Succeeded => TaskUsageStatus::Succeeded,
        VideoTaskStatus::Failed => TaskUsageStatus::Failed,
        VideoTaskStatus::Canceled => TaskUsageStatus::Cancelled,
        VideoTaskStatus::Unknown => TaskUsageStatus::Unknown,
    }
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
        context: Option<TaskUsageContext>,
        original: Option<TaskUsageRecord>,
        cancel: CancellationToken,
    ) -> Result<GatewayVideoResponse, GatewayError> {
        let request_id = gateway_request_id()?;
        let submitting = matches!(&operation, VideoOperation::Submit(_));
        let mut record = gateway_record(
            request_id.as_str(),
            client,
            context,
            if submitting {
                TaskUsageOperation::Video
            } else {
                TaskUsageOperation::Query
            },
            Some(VIDEO_PROVIDER.into()),
            Some(pawork_providers::token_plan_video::TOKEN_PLAN_VIDEO_MODEL.into()),
        );
        record.related_call_id = original.as_ref().map(|r| r.id.clone());
        if submitting {
            record.planned_video_seconds = Some(5);
        }
        self.usage
            .control
            .ledger
            .start_task_usage(record.clone())
            .await
            .map_err(usage_error)?;
        let mut release_result = Ok(());
        let result: Result<VideoGenerationTask, GatewayError> = async {
            let mut lease = self.usage.control.pool.acquire_guard(AcquireRequest {
                tenant_id: TenantId::new(format!("thirdparty/{client}")),
                principal_id: PrincipalId::new(client),
                session_id: SessionId::new(request_id.as_str()),
                agent_id: AgentId::new("gateway"),
                provider_id: Some(ProviderId::new(VIDEO_PROVIDER)),
                account_id: Some(AccountId::new(format!("{VIDEO_PROVIDER}/{account}"))),
                trace_id: None,
            }).await.map_err(|_|GatewayError::new(429,"concurrency_limit","Too many concurrent requests."))?;
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
            let outcome=match &result {Ok(_)=>LeaseOutcome::Completed,Err(e) if e.code=="cancelled"=>LeaseOutcome::Cancelled,Err(_)=>LeaseOutcome::Failed};
            *lease.outcome_mut()=outcome;
            if let Some(lease)=lease.into_lease() {
                release_result = self.usage.control.pool.release(lease.lease_id,outcome).await.map(|_| ())
                    .map_err(|_|GatewayError::new(500,"lease_unavailable","Cannot release model lease."));
            }
            result.map(|mut task| {
                task.id=format!("{account}.{}",task.id);
                task.model=format!("{VIDEO_PROVIDER}/{}",task.model);
                task
            })
        }.await;
        complete_record(&mut record, &result);
        let mut original_result = Ok(());
        if let Ok(task) = &result {
            record.upstream_task_id = Some(task.id.clone());
            record.upstream_status = Some(task.status);
            if submitting {
                record.status = video_status(task.status);
            }
            if let Some(mut original) = original {
                if matches!(
                    original.status,
                    TaskUsageStatus::Submitted
                        | TaskUsageStatus::Running
                        | TaskUsageStatus::Unknown
                ) {
                    original.status = video_status(task.status);
                    original.upstream_status = Some(task.status);
                    original.error_code = matches!(task.status, VideoTaskStatus::Failed)
                        .then(|| "video_failed".into());
                    let original_id = original.id.clone();
                    original_result = self.usage.control.ledger.finish_task_usage(original).await;
                    if matches!(
                        original_result,
                        Err(pawork_control_plane::UsageLedgerError::Conflict { .. })
                    ) {
                        // 另一次并发轮询已确认终态时，保留该结果，不让迟到响应倒退状态。
                        match self
                            .usage
                            .control
                            .ledger
                            .get_task_usage(client, &original_id)
                            .await
                        {
                            Ok(Some(latest))
                                if matches!(
                                    latest.status,
                                    TaskUsageStatus::Succeeded
                                        | TaskUsageStatus::Failed
                                        | TaskUsageStatus::Cancelled
                                ) =>
                            {
                                original_result = Ok(())
                            }
                            Err(error) => original_result = Err(error),
                            _ => {}
                        }
                    }
                }
            }
        } else if submitting
            && result
                .as_ref()
                .is_err_and(|e| matches!(e.code, "timeout" | "cancelled" | "upstream_error"))
        {
            // 请求可能已被供应商接受；没有回执不能断言没有生成或扣费。
            record.status = TaskUsageStatus::Unknown;
        }
        let journal_result = self
            .usage
            .control
            .ledger
            .finish_task_usage(record.clone())
            .await;
        let with_id = |mut error: GatewayError| {
            error.call_id = Some(record.id.clone());
            error
        };
        journal_result.map_err(usage_error).map_err(with_id)?;
        original_result.map_err(usage_error).map_err(with_id)?;
        release_result.map_err(with_id)?;
        result
            .map(|task| GatewayVideoResponse {
                task,
                pawork_usage: GatewayUsageLink {
                    call_id: record.id.clone(),
                    related_call_id: record.related_call_id,
                },
            })
            .map_err(|mut e| {
                e.call_id = Some(record.id);
                e
            })
    }
}

#[async_trait]
impl GatewayBackend for AppCore {
    type Completion = GatewayCompletion;
    fn gateway_completion_call_id(&self, completion: &GatewayCompletion) -> String {
        completion.request.request_id.as_str().into()
    }

    async fn gateway_task_usage(
        &self,
        client: &str,
        mut query: TaskUsageQuery,
    ) -> Result<TaskUsageReport, GatewayError> {
        if !pawork_gateway::tokens::valid_client(client) || !query.validate() {
            return Err(GatewayError::invalid());
        }
        if query.client.as_deref().is_some_and(|c| c != client)
            || query.cursor.as_ref().is_some_and(|c| c.client != client)
        {
            return Err(GatewayError::new(
                403,
                "usage_forbidden",
                "Usage is scoped to the authenticated client.",
            ));
        }
        query.client = Some(client.into());
        self.usage
            .control
            .ledger
            .task_usage_report(&query)
            .await
            .map_err(usage_error)
    }

    async fn report_gateway_operation(
        &self,
        client: &str,
        input: TaskUsageOperationReport,
    ) -> Result<TaskUsageRecord, GatewayError> {
        if !pawork_gateway::tokens::valid_client(client)
            || !input.context.validate()
            || !valid_usage_id(&input.report_id)
            || input.report_id.len() > 128
            || !input.context.operation.is_client_operation()
            || !matches!(
                input.status,
                TaskUsageStatus::Succeeded | TaskUsageStatus::Failed | TaskUsageStatus::Cancelled
            )
            || input.started_at_ms == 0
            || input.finished_at_ms < input.started_at_ms
        {
            return Err(GatewayError::invalid());
        }
        let id = format!("client-{}", input.report_id);
        let mut record = gateway_record(
            &id,
            client,
            Some(input.context.clone()),
            input.context.operation,
            None,
            None,
        );
        record.source = TaskUsageSource::ClientReport;
        record.started_at_ms = input.started_at_ms;
        record.finished_at_ms = Some(input.finished_at_ms);
        record.status = input.status;
        record.related_call_id = input.related_call_id;
        self.usage
            .control
            .ledger
            .start_task_usage(record.clone())
            .await
            .map_err(usage_error)?;
        Ok(record)
    }

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
    ) -> Result<GatewayVideoResponse, GatewayError> {
        use pawork_providers::token_plan_video::TOKEN_PLAN_VIDEO_MODEL;
        if !pawork_gateway::tokens::valid_client(client)
            || input.model != format!("{VIDEO_PROVIDER}/{TOKEN_PLAN_VIDEO_MODEL}")
            || input
                .pawork_usage
                .as_ref()
                .is_some_and(|c| !c.validate() || c.operation != TaskUsageOperation::Video)
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
            input.pawork_usage,
            None,
            cancel,
        )
        .await
    }

    async fn query_gateway_video(
        &self,
        client: &str,
        id: &str,
        cancel: CancellationToken,
    ) -> Result<GatewayVideoResponse, GatewayError> {
        let original = self
            .usage
            .control
            .ledger
            .find_task_usage_video(None, id)
            .await
            .map_err(usage_error)?;
        if original.as_ref().is_some_and(|r| r.client != client) {
            return Err(GatewayError::new(
                404,
                "not_found",
                "Video task is unavailable.",
            ));
        }
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
        let context = original
            .as_ref()
            .and_then(|r| r.context.clone())
            .map(|mut c| {
                c.operation = TaskUsageOperation::Query;
                c.retry_of = None;
                c
            });
        self.execute_gateway_video(
            client,
            provider,
            account,
            VideoOperation::Query(id),
            context,
            original,
            cancel,
        )
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
        if input.pawork_usage.as_ref().is_some_and(|c| {
            !c.validate()
                || !matches!(
                    c.operation,
                    TaskUsageOperation::Text
                        | TaskUsageOperation::Storyboard
                        | TaskUsageOperation::Image
                )
        }) {
            return Err(GatewayError::invalid());
        }
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
        let operation = input.pawork_usage.as_ref().map(|c| c.operation).unwrap_or(
            if entry.capabilities.image_output && !entry.capabilities.text {
                TaskUsageOperation::Image
            } else {
                TaskUsageOperation::Text
            },
        );
        if (operation == TaskUsageOperation::Image && !entry.capabilities.image_output)
            || (operation != TaskUsageOperation::Image && !entry.capabilities.text)
        {
            return Err(GatewayError::invalid());
        }
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
            usage_context: input.pawork_usage,
            operation,
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
        let mut log = gateway_record(
            completion.request.request_id.as_str(),
            client,
            completion.usage_context,
            completion.operation,
            Some(completion.provider_id.as_str().into()),
            Some(completion.request.model.as_str().into()),
        );
        let with_id = |mut e: GatewayError| {
            e.call_id = Some(log.id.clone());
            e
        };
        self.usage
            .control
            .ledger
            .start_task_usage(log.clone())
            .await
            .map_err(usage_error)
            .map_err(with_id)?;
        let lease = self
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
            .await;
        let mut lease = match lease {
            Ok(value) => value,
            Err(_) => {
                let result: Result<ModelResponseSummary, GatewayError> = Err(GatewayError::new(
                    429,
                    "concurrency_limit",
                    "Too many concurrent model requests.",
                ));
                complete_record(&mut log, &result);
                self.usage
                    .control
                    .ledger
                    .finish_task_usage(log.clone())
                    .await
                    .map_err(usage_error)?;
                return result.map_err(|mut e| {
                    e.call_id = Some(log.id);
                    e
                });
            }
        };
        let sink = UsageSink {
            inner: sink,
            usage: Mutex::new(None),
            images: std::sync::atomic::AtomicU64::new(0),
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(GatewayError::cancelled()),
            result = tokio::time::timeout(std::time::Duration::from_secs(600),completion.provider.stream(&completion.request,&sink,cancel.clone())) => {
                match result { Ok(value)=>value.map_err(GatewayError::from),Err(_)=>{cancel.cancel();Err(GatewayError::timeout())} }
            }
        };
        let usage = match &result {
            Ok(summary) if !summary.usage.is_zero() => Some(summary.usage.clone()),
            _ => sink.usage.lock().expect("usage").clone(),
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
        log.tokens = usage.clone();
        if log.operation == TaskUsageOperation::Image {
            let images = sink.images.load(std::sync::atomic::Ordering::Relaxed);
            // 输出事件是已知证据；失败且没有输出不冒充已确认 0 张。
            log.output_images = (images > 0 || result.is_ok()).then_some(images);
        }
        complete_record(&mut log, &result);
        let billable = usage.as_ref().filter(|u| !u.is_zero());
        let ledger_result = if let Some(usage) = billable {
            let mut record = crate::control::usage_record(
                &session,
                &RunId::new(completion.request.request_id.as_str()),
                &completion.request.request_id,
                &completion.provider_id,
                &completion.request.model,
                usage,
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
                .map_err(usage_error)
        } else {
            Ok(())
        };
        let journal_result = self
            .usage
            .control
            .ledger
            .finish_task_usage(log.clone())
            .await
            .map_err(usage_error);
        let outcome = match &result {
            Ok(_) => LeaseOutcome::Completed,
            Err(e) if e.code == "cancelled" => LeaseOutcome::Cancelled,
            Err(_) => LeaseOutcome::Failed,
        };
        *lease.outcome_mut() = outcome;
        let release_result = if let Some(value) = lease.into_lease() {
            self.usage
                .control
                .pool
                .release(value.lease_id, outcome)
                .await
                .map(|_| ())
                .map_err(|_| {
                    GatewayError::new(500, "lease_unavailable", "Cannot release model lease.")
                })
        } else {
            Ok(())
        };
        let with_id = |mut e: GatewayError| {
            e.call_id = Some(log.id.clone());
            e
        };
        journal_result.map_err(with_id)?;
        ledger_result.map_err(with_id)?;
        release_result.map_err(with_id)?;
        result.map_err(with_id)
    }
}
struct UsageSink {
    inner: Arc<dyn ProviderEventSink>,
    usage: Mutex<Option<TokenUsage>>,
    images: std::sync::atomic::AtomicU64,
}
#[async_trait]
impl ProviderEventSink for UsageSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        if let ProviderStreamEvent::UsageUpdated(usage) = &event {
            *self.usage.lock().expect("usage") = Some(usage.clone());
        }
        if matches!(&event, ProviderStreamEvent::ImageOutput { .. }) {
            self.images
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if let ProviderStreamEvent::Error(error) = event {
            return Err(error);
        }
        self.inner.emit(event).await
    }
}
