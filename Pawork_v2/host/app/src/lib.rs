//! 最小应用门面：读配置 → env key → provider → `chat_turn` / `list_models`。
//!
//! 不落库、不跑工具循环、不按 Provider 名称分支、不改写 `ProviderStreamEvent`。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use pawork_api::{
    CredentialKind, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderEventSink, ResolvedCredential,
};
use pawork_config::{
    api_key_env_name, read_api_key_from_env, ConfigError, Loader, PaworkConfig, ProviderConfig,
};
use pawork_domain::{CancellationToken, Message, ModelId, ProviderId, RequestId};
use pawork_engine::{assemble_request, run_turn};
use pawork_providers::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use thiserror::Error;

/// 从配置文件与 CLI 覆盖构造 [`AppCore`] 的选项。
#[derive(Clone, Debug, Default)]
pub struct AppLoadOptions {
    pub workspace_root: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

impl AppLoadOptions {
    pub fn from_cli(provider: Option<String>, model: Option<String>) -> Self {
        Self {
            workspace_root: std::env::current_dir().ok(),
            provider,
            model,
        }
    }
}

/// 装配期错误。明文 key 不得进入任何变体。
#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("未配置 default_provider，请在 config.toml 中设置或使用 --provider")]
    MissingDefaultProvider,
    #[error("未配置 default_model，请在 config.toml 中设置或使用 --model")]
    MissingDefaultModel,
    #[error("配置中找不到 provider `{id}`")]
    UnknownProvider { id: String },
    #[error("provider `{id}` 未配置 base_url")]
    MissingBaseUrl { id: String },
    #[error("缺少 API key：请设置环境变量 {env_name}")]
    MissingCredential { env_name: String },
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// 已装配的最小 Core：持有一个 openai-compatible provider 与默认 model。
pub struct AppCore {
    provider: Arc<dyn ModelProvider>,
    credential: Option<ResolvedCredential>,
    model: ModelId,
    provider_id: ProviderId,
    next_request: AtomicU64,
}

impl std::fmt::Debug for AppCore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppCore")
            .field("provider_id", &self.provider_id)
            .field("model", &self.model)
            .field("credential", &self.credential)
            .finish()
    }
}

impl AppCore {
    /// 发现 Builtin + Global + Workspace，再套用 CLI `--provider` / `--model`。
    pub fn load(options: AppLoadOptions) -> Result<Self, AppError> {
        let resolved = Loader::discover(options.workspace_root.as_deref()).resolve()?;
        Self::from_resolved(
            resolved.config,
            options.provider.as_deref(),
            options.model.as_deref(),
        )
    }

    /// 与 [`Self::load`] 相同，但配置发现路径由调用方注入（测试用）。
    pub fn load_from(
        global_file: Option<&Path>,
        workspace_file: Option<&Path>,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<Self, AppError> {
        let resolved = Loader::discover_from(global_file, workspace_file).resolve()?;
        Self::from_resolved(resolved.config, provider, model)
    }

    /// 用已合并的 [`PaworkConfig`] 装配。CLI 覆盖优先于配置默认值。
    pub fn from_resolved(
        mut config: PaworkConfig,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<Self, AppError> {
        if let Some(provider) = provider {
            config.default_provider = Some(provider.to_string());
        }
        if let Some(model) = model {
            config.default_model = Some(model.to_string());
        }

        let provider_id = config
            .default_provider
            .clone()
            .ok_or(AppError::MissingDefaultProvider)?;
        let model_id = config
            .default_model
            .clone()
            .ok_or(AppError::MissingDefaultModel)?;
        let provider_cfg = find_provider(&config.providers, &provider_id)?;
        let base_url = provider_cfg
            .base_url
            .clone()
            .ok_or_else(|| AppError::MissingBaseUrl {
                id: provider_id.clone(),
            })?;

        let env_name = api_key_env_name(&provider_id);
        let secret = read_api_key_from_env(&provider_id).ok_or(AppError::MissingCredential {
            env_name,
        })?;
        let credential = ResolvedCredential::new(CredentialKind::ApiKey, secret);

        let adapter = OpenAiCompatibleProvider::new(
            OpenAiCompatibleConfig::new(base_url).with_provider_id(provider_id.clone()),
            Some(credential.clone()),
        )?;

        Ok(Self::from_parts(
            Arc::new(adapter),
            Some(credential),
            ModelId::from(model_id.as_str()),
            ProviderId::from(provider_id.as_str()),
        ))
    }

    pub fn from_parts(
        provider: Arc<dyn ModelProvider>,
        credential: Option<ResolvedCredential>,
        model: ModelId,
        provider_id: ProviderId,
    ) -> Self {
        Self {
            provider,
            credential,
            model,
            provider_id,
            next_request: AtomicU64::new(1),
        }
    }

    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    pub fn model(&self) -> &ModelId {
        &self.model
    }

    /// 组装 canonical 请求并调用 `run_turn`。13 变体原样交给 sink。
    pub async fn chat_turn(
        &self,
        messages: Vec<Message>,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let n = self.next_request.fetch_add(1, Ordering::Relaxed);
        let request = assemble_request(
            RequestId::from(format!("req-{n}")),
            self.model.clone(),
            messages,
        );
        run_turn(self.provider.as_ref(), request, sink, cancel).await
    }

    pub async fn list_models(&self) -> Result<Vec<ModelDefinition>, ProviderError> {
        self.provider
            .list_models(self.credential.as_ref())
            .await
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use pawork_api::{
        CanonicalModelRequest, ModelCapabilities, ProviderStreamEvent,
    };
    use pawork_domain::{
        ContentPart, MessageId, MessageRole, StopReason, TextContent, TokenUsage, ToolCallId,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<ProviderStreamEvent>>);

    impl RecordingSink {
        fn snapshot(&self) -> Vec<ProviderStreamEvent> {
            self.0.lock().expect("sink mutex").clone()
        }
    }

    #[async_trait]
    impl ProviderEventSink for RecordingSink {
        async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
            self.0.lock().expect("sink mutex").push(event);
            Ok(())
        }
    }

    struct ScriptedProvider {
        events: Vec<ProviderStreamEvent>,
        summary: ModelResponseSummary,
        models: Vec<ModelDefinition>,
    }

    #[async_trait]
    impl ModelProvider for ScriptedProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(self.models.clone())
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            for event in &self.events {
                sink.emit(event.clone()).await?;
            }
            Ok(self.summary.clone())
        }
    }

    fn sample_config(id: &str) -> PaworkConfig {
        PaworkConfig {
            default_provider: Some(id.into()),
            default_model: Some("glm-5.2".into()),
            providers: vec![ProviderConfig {
                id: id.into(),
                base_url: Some("https://example.test/v1".into()),
                ..ProviderConfig::default()
            }],
            ..PaworkConfig::default()
        }
    }

    fn set_env(key: &str, value: &str) {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(key, value);
        }
    }

    fn remove_env(key: &str) {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(key);
        }
    }

    fn user_hello() -> Message {
        Message {
            id: MessageId::from("message-1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "hello".into(),
            })],
            metadata: Default::default(),
        }
    }

    #[test]
    fn from_resolved_requires_provider_and_model() {
        let err = AppCore::from_resolved(PaworkConfig::default(), None, None)
            .expect_err("empty config");
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

    #[test]
    fn from_resolved_fail_closed_without_env_key() {
        let id = "app-core-missing-key";
        remove_env(&api_key_env_name(id));
        let err = AppCore::from_resolved(sample_config(id), None, None).expect_err("no key");
        let display = format!("{err}");
        match err {
            AppError::MissingCredential { env_name } => {
                assert_eq!(env_name, "PAWORK_API_KEY_APP_CORE_MISSING_KEY");
                assert!(display.contains("PAWORK_API_KEY_APP_CORE_MISSING_KEY"));
            }
            other => panic!("unexpected {other:?}"),
        }
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
        let debug = format!("{core:?}");
        assert!(
            !debug.contains(secret),
            "secret leaked in Debug: {debug}"
        );
        assert!(debug.contains("[REDACTED]"), "{debug}");
    }

    #[tokio::test]
    async fn chat_turn_forwards_events_from_mock_provider() {
        let events = vec![
            ProviderStreamEvent::TextDelta("hi".into()),
            ProviderStreamEvent::ThinkingDelta("think".into()),
            ProviderStreamEvent::ToolCallStarted {
                id: ToolCallId::from("call-1"),
                name: "read_file".into(),
            },
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ];
        let summary = ModelResponseSummary {
            stop_reason: StopReason::Completed,
            usage: TokenUsage::default(),
            response_id: Some("resp-1".into()),
            provider_metadata: Default::default(),
        };
        let core = AppCore::from_parts(
            Arc::new(ScriptedProvider {
                events: events.clone(),
                summary: summary.clone(),
                models: vec![ModelDefinition {
                    id: ModelId::from("glm-5.2"),
                    display_name: "glm-5.2".into(),
                    context_window_tokens: 0,
                    max_output_tokens: 0,
                    capabilities: ModelCapabilities::default(),
                }],
            }),
            None,
            ModelId::from("glm-5.2"),
            ProviderId::from("mock"),
        );
        let sink = RecordingSink::default();
        let result = core
            .chat_turn(vec![user_hello()], &sink, CancellationToken::new())
            .await
            .expect("turn");
        assert_eq!(result, summary);
        assert_eq!(sink.snapshot(), events);

        let models = core.list_models().await.expect("models");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id.as_str(), "glm-5.2");
    }
}
