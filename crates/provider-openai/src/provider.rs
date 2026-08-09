//! OpenAI 原生 [`ModelProvider`](provider_api::ModelProvider) 实现。
//!
//! OpenAI 协议即 Chat Completions 协议，故本 provider 复用
//! [`OpenAiCompatibleProvider`](provider_openai_compatible::OpenAiCompatibleProvider)
//! 作为流式引擎，固定为 OpenAI 官方端点与 `openai` provider 标识，并提供能力更
//! 完整的内置模型目录（远端 `/models` 不返回能力信息）。

use std::time::Duration;

use agent_domain::{CancellationToken, ModelId, ProviderId};
use async_trait::async_trait;
use provider_api::{
    CanonicalModelRequest, ModelCapabilities, ModelDefinition, ModelProvider, ModelResponseSummary,
    ProviderError, ProviderEventSink, ResolvedCredential,
};
use provider_openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use provider_runtime::http::HttpClientConfig;

use crate::DEFAULT_BASE_URL;

/// OpenAI 适配器配置。
#[derive(Clone, Debug)]
pub struct OpenAiConfig {
    /// 基础 URL（默认 `https://api.openai.com/v1`）。
    pub base_url: String,
    /// Provider 标识（默认 `openai`）。
    pub provider_id: ProviderId,
    /// HTTP 客户端配置。
    pub http: HttpClientConfig,
    /// 建连及流式读取无数据超时（覆盖 `http.timeout`）。
    pub request_timeout: Option<Duration>,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            provider_id: ProviderId::new("openai"),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl OpenAiConfig {
    /// 以自定义基础 URL 构造（如 Azure OpenAI 风格端点），provider_id 仍为 `openai`。
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Default::default()
        }
    }

    /// 覆盖 provider 标识。
    pub fn with_provider_id(mut self, id: impl Into<String>) -> Self {
        self.provider_id = ProviderId::new(id.into());
        self
    }
}

/// OpenAI 原生 Provider。
pub struct OpenAiProvider {
    inner: OpenAiCompatibleProvider,
}

impl OpenAiProvider {
    /// 构造适配器。`credential` 为 None 时不带认证头。
    pub fn new(
        config: OpenAiConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let http_config = match config.request_timeout {
            Some(timeout) => {
                let mut c = config.http.clone();
                c.timeout = Some(timeout);
                c
            }
            None => config.http.clone(),
        };
        let compat = OpenAiCompatibleConfig {
            base_url: config.base_url,
            provider_id: config.provider_id,
            http: http_config,
            request_timeout: None,
        };
        let inner = OpenAiCompatibleProvider::new(compat, credential)?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        self.inner.id()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        // OpenAI 远端 /models 不返回能力信息，直接返回能力完整的内置目录。
        Ok(builtin_models())
    }

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        self.inner.stream(request, sink, cancel).await
    }
}

/// OpenAI 内置模型目录（含 reasoning / image / tool 能力）。
/// 数据快照：2026-08-09；目录更新作为显式跟踪项手动执行。
pub fn builtin_models() -> Vec<ModelDefinition> {
    fn caps(
        text: bool,
        image_input: bool,
        tool_calls: bool,
        parallel_tool_calls: bool,
        thinking: bool,
        structured_output: bool,
        prompt_cache: bool,
    ) -> ModelCapabilities {
        ModelCapabilities {
            text,
            image_input,
            tool_calls,
            parallel_tool_calls,
            thinking,
            structured_output,
            prompt_cache,
        }
    }

    fn def(
        id: &str,
        display: &str,
        context_window_tokens: u64,
        max_output_tokens: u64,
        capabilities: ModelCapabilities,
    ) -> ModelDefinition {
        ModelDefinition {
            id: ModelId::new(id),
            display_name: display.into(),
            context_window_tokens,
            max_output_tokens,
            capabilities,
        }
    }

    // GPT-4o 系：文本 + 视觉 + 工具，无 reasoning。
    let vision_tools = caps(true, true, true, true, false, true, true);
    // o 系：reasoning 模型，支持 thinking + 视觉 + 工具。
    let reasoning = caps(true, true, true, true, true, true, true);
    // GPT-3.5：无视觉、无 reasoning。
    let legacy = caps(true, false, true, true, false, true, true);

    vec![
        def("gpt-4o", "GPT-4o", 128_000, 16_384, vision_tools.clone()),
        def("gpt-4o-mini", "GPT-4o mini", 128_000, 16_384, vision_tools),
        def("o1", "o1", 200_000, 100_000, reasoning.clone()),
        def("o1-mini", "o1-mini", 128_000, 65_536, reasoning),
        def("gpt-3.5-turbo", "GPT-3.5 Turbo", 16_385, 4_096, legacy),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_has_reasoning_and_vision_models() {
        let models = builtin_models();
        let o1 = models
            .iter()
            .find(|m| m.id == ModelId::new("o1"))
            .expect("o1 present");
        assert!(o1.capabilities.thinking);
        assert!(o1.capabilities.image_input);
        let gpt4o = models
            .iter()
            .find(|m| m.id == ModelId::new("gpt-4o"))
            .expect("gpt-4o present");
        assert!(!gpt4o.capabilities.thinking);
        assert!(gpt4o.capabilities.image_input);
    }

    #[test]
    fn config_defaults_to_openai_endpoint() {
        let config = OpenAiConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.provider_id, ProviderId::new("openai"));
    }
}
