//! Stateless, provider-neutral model access for the local HTTP gateway.
use crate::provider_assembly::{assemble_provider, resolve_provider_model};
use crate::{AdapterProtocol, AppCore, AppError};
use async_trait::async_trait;
use pawork_control_plane::credential::{AcquireRequest, LeaseOutcome};
use pawork_domain::*;
use pawork_gateway::{GatewayBackend, GatewayChatRequest, GatewayError, GatewayModel};
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
pub struct GatewayCompletion {
    provider: Arc<dyn ModelProvider>,
    request: CanonicalModelRequest,
    provider_id: ProviderId,
    account_id: String,
    credential_id: Option<String>,
}

#[async_trait]
impl GatewayBackend for AppCore {
    type Completion = GatewayCompletion;
    async fn gateway_models(&self) -> Result<Vec<GatewayModel>, GatewayError> {
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
            .map(|e| GatewayModel {
                id: format!("{}/{}", e.provider, e.id),
                object: "model",
                owned_by: e.provider.to_string(),
                display_name: e.display_name,
                context_window: e.context_window_tokens,
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
                content: vec![ContentPart::Text(TextContent { text: m.content })],
                metadata: Default::default(),
            });
        }
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
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| {
            GatewayError::new(
                500,
                "entropy_unavailable",
                "Cannot create request identity.",
            )
        })?;
        let id: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        let mut request = pawork_engine::assemble_request(
            RequestId::new(format!("gateway-{id}")),
            entry.id.clone(),
            messages,
        );
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
