# Provider Runtime

## 职责

将不同模型供应商抽象为统一接口。所有 Provider 转换成统一请求（`CanonicalModelRequest`）和流式事件（`ProviderStreamEvent`），Agent Engine 只依赖 canonical domain，不感知具体 Provider。

Provider Runtime 只负责“如何调用某个 Provider”。账号池、并发租约、优先级/权重、session affinity、tenant policy 与跨 Provider fallback 由 [Provider Account Control Plane](provider-control-plane.md) 承担，不扩张 `ModelProvider`。

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

### Canonical Embedding（Phase 16 / P16-7）

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    async fn list_embedding_models(
        &self,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<EmbeddingModelDefinition>, ProviderError>;

    async fn embed(
        &self,
        request: EmbeddingRequest,
        cancel: CancellationToken,
    ) -> Result<EmbeddingResponse, ProviderError>;
}
```

embedding 是 Provider 的另一项 canonical 能力，与 `ModelProvider` 平级落在 `provider-api`（不新增独立 crate）。`memory-service` 只依赖该 trait，**禁止按 Provider 名调用不同 API、禁止用 `provider_options` 绕过 canonical、禁止私自实现 Provider-specific 请求**；各 `provider-*` 实现 `EmbeddingProvider`。每个 `EmbeddingModelDefinition` 携带自己的 `EmbeddingCapabilities`（维度 / `max_input_tokens` / `max_batch_size` 等），能力不假定在同一 Provider 的所有 embedding model 间相同。

## 统一请求能力

System Prompt；历史消息；Text 和 Image 输入；Tool Schema；Tool Choice；Temperature；Max Output Tokens；Stop Sequence；Thinking/Reasoning Level；JSON 或结构化输出；Provider-specific options；Prompt Cache；自定义 HTTP Header；Proxy；超时；请求取消；Trace ID。

> reasoning effort 不属于「Provider-specific options」：`ReasoningEffort { None, Low, Medium, High, XHigh, Max }` 是 canonical 一等字段，经 P15-8 `CapabilityNegotiator` 协商翻译（P17-5 `AgentProfile.effort` 走此路径，不经 `provider_options`）。

## 统一流式事件

```rust
pub enum ProviderStreamEvent {
    ResponseStarted,
    TextDelta(String),
    ThinkingDelta(String),
    ToolCallStarted { id: String, name: String },
    ToolCallArgumentsDelta { id: String, json: String },
    ToolCallCompleted { id: String },
    ServerTool(ServerToolEvent),
    CitationAdded(Citation),
    SourceAdded(Source),
    ReasoningItemCommitted(ReasoningItem),
    UsageUpdated(TokenUsage),
    ResponseCompleted(StopReason),
    ProviderMetadata(serde_json::Value),
    Error(ProviderError),
}
```

`ProviderEventSink` 是 push-based 异步 sink：adapter 逐事件 `emit`，上层可在实现中施加有界背压；`stream` 返回最终 stop reason、usage 与脱敏 metadata 摘要。

需正确处理：SSE；JSON Lines；chunked HTTP；Partial JSON；Tool Arguments 跨多 Chunk；Unicode 边界；Provider 提前断开；Provider 返回错误事件；多个 Tool Call 并行流式返回。

### 流式传输语义

- HTTP `timeout` 同时约束建连阶段与按单次读操作重置的 inactivity timeout；流持续产出 chunk 时不设总时长上限，连续无数据才归一为 `Timeout`。
- SSE / JSONL 单条缓冲上限为 1 MiB；超限产生解析错误并重置，确定非法的 UTF-8 字节按线性扫描批量移除。
- OpenAI-compatible 请求固定发送 `stream_options.include_usage = true`；末尾 `choices: []` 的 usage-only chunk 仍归一为 `UsageUpdated`。
- 协议正常以 `[DONE]` 收尾但没有 `finish_reason` 时归一为 `Completed`，不把成功响应误记为 `Error`。
- `provider_options` 中的非保留键在 canonical 翻译后合并，因此可覆盖同名的非关键 wire 字段；但不得覆盖 `model`、`messages`、`stream`、`stream_options`、`tools`、`tool_choice` 或认证字段，命中保留键时忽略并记录告警。
- Adapter 还必须保护会破坏协议约束的字段；Anthropic 的 `max_tokens`、`thinking`、`temperature`、`stop_sequences` 均不可由 `provider_options` 覆盖，避免绕过 thinking budget 钳制与 canonical 参数。
- 远端模型目录请求与流式请求复用同一认证头，受保护的 `/models` 端点不得匿名访问。

## Provider 优先级

- **P0（初始主要供应商）**：OpenAI（GPT）；Anthropic（Claude）；xAI Grok（API Key + OAuth 订阅）；智谱 GLM；阿里 Qwen（DashScope）；Moonshot Kimi；OpenAI-compatible；本地兼容服务（Ollama、vLLM、LM Studio）。
- **P1（已实现或次要）**：Google Gemini（已实现 `provider-google`，2026-08-08 降级为次要）；AWS Bedrock；Mistral；Azure OpenAI；Google Vertex AI；GitHub Models；自定义 Endpoint。
- **P2**：Provider WASM Plugin；动态模型发现；Provider 路由；多 Provider fallback；自动成本和延迟路由。

其中 Provider routing / fallback 的确定性基础在 Phase 18 实现；自动成本和延迟路由仍为 P2，必须等 Usage/Cost Ledger 与 Health 数据稳定后再启用。

## 已实现 Provider（Phase 6）

| Provider | crate | 协议 | 关键能力 |
| --- | --- | --- | --- |
| OpenAI（原生） | `provider-openai` | Chat Completions | reasoning 流、图片输入、结构化输出、provider options 透传、prompt cache 自动命中 |
| OpenAI-compatible / 本地服务 | `provider-openai-compatible` | Chat Completions | 覆盖云端兼容接口与 Ollama / vLLM / LM Studio；图片输入、reasoning、options 透传 |
| Anthropic | `provider-anthropic` | Messages API | thinking budget 与 `max_tokens` 约束、tool_use、有界 prompt cache 断点、图片；JSON/Schema 通过 system 约束注入 |
| Google Gemini | `provider-google` | `generateContent`（`alt=sse`） | `functionCall`、`thought` 流、`responseSchema`、`thinkingConfig`、cache usage；并行工具保留 id/name/ordinal 元数据（已降级次要 P1） |
| xAI Grok | `provider-xai` | Chat Completions（`api.x.ai`） | API Key / OAuth bearer 双鉴权、reasoning → `ThinkingDelta`、独立错误归一 |
| 智谱 GLM | `provider-zhipu` | BigModel OpenAI-compatible | API Key Bearer、`reasoning_content` → `ThinkingDelta`、余额/内容审核错误归一 |
| 阿里 Qwen | `provider-qwen` | DashScope OpenAI-compatible | API Key Bearer；canonical thinking 仅对能力目录中的模型映射 `enable_thinking`；reasoning 归一 |
| Moonshot Kimi | `provider-moonshot` | OpenAI-compatible | API Key Bearer、Kimi reasoning → `ThinkingDelta`、限流/余额/内容安全错误归一 |

跨切能力（Phase 6）：thinking/reasoning level（P6-5）、图片输入（P6-6）、prompt cache 控制+命中（P6-7）、结构化输出（P6-8）、provider-specific options 透传+raw metadata（P6-9）。Agent Core 经 [`agent-engine/tests/no_provider_branch.rs`](../../crates/agent-engine/tests/no_provider_branch.rs) 回归守护，禁止按 provider 名走分支。Moonshot 原生余额查询不属于 Provider 基线，由 P14-2 与额度适配器承接。

### 内置模型目录新鲜度

OpenAI、Anthropic、Google、Qwen 与 Moonshot 的 `builtin_models()` 数据快照日期为 **2026-08-09**，源码入口也标注同一日期。这些目录优先表达能力而非仅列名；发现新模型与远端 `/models` + 能力探测属于后续显式跟踪项。xAI 与智谱当前复用带鉴权的远端目录。

## 认证

API Key 与 OS Keychain 见 [auth](auth.md)；OAuth（PKCE / Device Flow / auto refresh / callback）由 `auth-service::oauth` 提供，明文 token 只存 SecretBackend。xAI 订阅模式在组合层先调用 `resolve_oauth_credential_for_request` 完成刷新与轮换 token 回写，再以 `OAuthBearer` 构造 `provider-xai`；鉴于消费级端点不是稳定公开契约，授权/token endpoint 由 host 配置注入，API Key 仍为默认路径。

## 错误模型

统一错误类别：Authentication / Authorization / RateLimited / QuotaExceeded / InvalidRequest / ModelNotFound / ContextTooLarge / ContentFiltered / Network / Timeout / ProviderUnavailable / StreamInterrupted / MalformedResponse / Cancelled / Unknown。

错误携带：是否可重试；建议重试时间；Provider request ID；HTTP 状态；脱敏后的错误内容；用户可读消息；诊断信息。

`provider-runtime` 负责协议/transport 错误归一与可重试性判断；生产环境的 bounded retry 与退避由 `agent-engine` 统一执行。P18 `ErrorClassifier` 再把 `UpstreamFailure` 分类为 failure class/scope/health impact，并决定是否允许 credential/model/provider/protocol failover。HTTP status 只是分类输入：`ClientCancelled`、`InvalidRequest`、`ContextTooLarge` 与 `ProtocolIncompatible` 不默认轮换 credential。

## 验收标准

- 每个 Provider 通过统一 Contract Tests（见 [testing](../quality/testing.md) §contract）
- Agent Core 不含 Provider 特例（禁止按 Provider 名走分支）
- 重试与错误归一化覆盖各类错误
- `ModelProvider` 不承担 account scheduling；credential lease 与 fallback 决策有独立 contract/property tests
- embedding model 目录及逐模型 capabilities 与实际 Provider 行为一致；memory-service 只经 `EmbeddingProvider` 调用

## 现代能力分层（Phase 6 vs Phase 15）

| 层 | 范围 | 能力 |
| --- | --- | --- |
| **Phase 6 = 基线兼容** | Chat Completions / Messages 基础协议 | text/image 输入、tool call、thinking/reasoning level、structured output、prompt cache、provider options 透传、raw metadata |
| **Phase 15 = 现代原生** | Canonical Tool v2 + Responses / Modern Messages | Responses 传输、Provider Hosted Tools（web_search / file_search / code_execution）、Provider Extension、reasoning continuation（加密凭证）、Citation / Source、Capability negotiation、Computer Use |

Phase 15 不替换 Phase 2/6 的兼容路径：旧路径保留，现代能力经 P15-8 协商降级时退回基线。Agent Core 始终不感知 Provider 名（`no_provider_branch`）。embedding（P16-7）经 `provider-api` 的 canonical `EmbeddingProvider` 暴露。

## 相关文档

- [models](models.md) · [auth](auth.md) · [provider-control-plane](provider-control-plane.md) · [tenant-audit](tenant-audit.md) · [agent-engine](agent-engine.md)
- [ADR-002 解耦](../adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../adr/ADR-015-provider-contract-tests.md)
- [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md) · [ROADMAP Phase 2 / Phase 6 / Phase 18](../../ROADMAP.md)
