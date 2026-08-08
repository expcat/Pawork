# Provider Runtime

## 职责

将不同模型供应商抽象为统一接口。所有 Provider 转换成统一请求（`CanonicalModelRequest`）和流式事件（`ProviderStreamEvent`），Agent Engine 只依赖 canonical domain，不感知具体 Provider。

## 接口

```rust
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    async fn list_models(
        &self,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError>;

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError>;
}
```

## 统一请求能力

System Prompt；历史消息；Text 和 Image 输入；Tool Schema；Tool Choice；Temperature；Max Output Tokens；Stop Sequence；Thinking/Reasoning Level；JSON 或结构化输出；Provider-specific options；Prompt Cache；自定义 HTTP Header；Proxy；超时；请求取消；Trace ID。

## 统一流式事件

```rust
pub enum ProviderStreamEvent {
    ResponseStarted,
    TextDelta(String),
    ThinkingDelta(String),
    ToolCallStarted { id: String, name: String },
    ToolCallArgumentsDelta { id: String, json: String },
    ToolCallCompleted { id: String },
    UsageUpdated(TokenUsage),
    ResponseCompleted(StopReason),
    ProviderMetadata(serde_json::Value),
    Error(ProviderError),
}
```

`ProviderEventSink` 是 push-based 异步 sink：adapter 逐事件 `emit`，上层可在实现中施加有界背压；`stream` 返回最终 stop reason、usage 与脱敏 metadata 摘要。

需正确处理：SSE；JSON Lines；chunked HTTP；Partial JSON；Tool Arguments 跨多 Chunk；Unicode 边界；Provider 提前断开；Provider 返回错误事件；多个 Tool Call 并行流式返回。

## Provider 优先级

- **P0（初始主要供应商）**：OpenAI（GPT）；Anthropic（Claude）；xAI Grok（API Key + OAuth 订阅）；智谱 GLM；阿里 Qwen（DashScope）；Moonshot Kimi；OpenAI-compatible；本地兼容服务（Ollama、vLLM、LM Studio）。
- **P1（已实现或次要）**：Google Gemini（已实现 `provider-google`，2026-08-08 降级为次要）；AWS Bedrock；Mistral；Azure OpenAI；Google Vertex AI；GitHub Models；自定义 Endpoint。
- **P2**：Provider WASM Plugin；动态模型发现；Provider 路由；多 Provider fallback；自动成本和延迟路由。

## 已实现 Provider（Phase 6）

| Provider | crate | 协议 | 关键能力 |
| --- | --- | --- | --- |
| OpenAI（原生） | `provider-openai` | Chat Completions | reasoning 流、图片输入、结构化输出、provider options 透传、prompt cache 自动命中 |
| OpenAI-compatible / 本地服务 | `provider-openai-compatible` | Chat Completions | 覆盖云端兼容接口与 Ollama / vLLM / LM Studio；图片输入、reasoning、options 透传 |
| Anthropic | `provider-anthropic` | Messages API | thinking、tool_use、prompt cache（`cache_control`）、图片、`top_k` 等透传 |
| Google Gemini | `provider-google` | `generateContent`（`alt=sse`） | `function_call`、`thought` 流、`responseSchema`、`thinkingConfig`、`cachedContentTokenCount`（已降级次要 P1） |

跨切能力（Phase 6）：thinking/reasoning level（P6-5）、图片输入（P6-6）、prompt cache 控制+命中（P6-7）、结构化输出（P6-8）、provider-specific options 透传+raw metadata（P6-9）。Agent Core 经 [`agent-engine/tests/no_provider_branch.rs`](../../crates/agent-engine/tests/no_provider_branch.rs) 回归守护，禁止按 provider 名走分支。

## 计划中 Provider（Phase 6，P6-10~13）

| Provider | crate | 鉴权 | 协议 | 关键能力 | 状态 |
| --- | --- | --- | --- | --- | --- |
| xAI Grok | `provider-xai`（计划） | API Key / OAuth 订阅 | Chat Completions（`api.x.ai`） | reasoning 流归一；OAuth 订阅按订阅配额而非 token 计费 | 🟡 P6-10 |
| 智谱 GLM | `provider-zhipu`（计划） | API Key | OpenAI-compatible（`open.bigmodel.cn/api/paas/v4`） | GLM-4.6 `reasoning_content` 归一 | 🟡 P6-11 |
| 阿里 Qwen | `provider-qwen`（计划） | API Key（DashScope） | OpenAI-compatible（`compatible-mode/v1`） | Qwen3 `enable_thinking` 归一 | 🟡 P6-12 |
| Moonshot Kimi | `provider-moonshot`（计划） | API Key | OpenAI-compatible（`api.moonshot.cn/v1`） | Kimi K2 reasoning 归一；原生余额查询 | 🟡 P6-13 |

## 认证

API Key 与 OS Keychain 见 [auth](auth.md)；OAuth（PKCE / Device Flow / auto refresh / callback）由 `auth-service::oauth` 提供，明文 token 只存 SecretBackend。

## 错误模型

统一错误类别：Authentication / Authorization / RateLimited / QuotaExceeded / InvalidRequest / ModelNotFound / ContextTooLarge / ContentFiltered / Network / Timeout / ProviderUnavailable / StreamInterrupted / MalformedResponse / Cancelled / Unknown。

错误携带：是否可重试；建议重试时间；Provider request ID；HTTP 状态；脱敏后的错误内容；用户可读消息；诊断信息。

## 验收标准

- 每个 Provider 通过统一 Contract Tests（见 [testing](../quality/testing.md) §contract）
- Agent Core 不含 Provider 特例（禁止按 Provider 名走分支）
- 重试与错误归一化覆盖各类错误

## 相关文档

- [models](models.md) · [auth](auth.md) · [agent-engine](agent-engine.md)
- [ADR-002 解耦](../adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../adr/ADR-015-provider-contract-tests.md)
- [ROADMAP Phase 2 / Phase 6](../../ROADMAP.md)
