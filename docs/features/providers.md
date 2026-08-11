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

> reasoning effort 不属于「Provider-specific options」：`ReasoningConfig { effort: ReasoningEffort, state }` 是现代 canonical 一等字段，其中 effort 为 `None / Low / Medium / High / XHigh / Max`。显式 `ReasoningConfig` 优先；旧 `ThinkingConfig.level` 仅在缺省时派生，`XHigh/Max` 进入旧 adapter 时显式 clamp 为 `High`。P17-5 `AgentProfile.effort` 走 `ReasoningConfig → CapabilityNegotiator`，不经 `provider_options`。

### 能力协商（P15-8）

每次 Provider 请求前先以 `model × requested capabilities` 协商，不按 Provider 名称分支。canonical vocabulary 覆盖：Responses / Chat Completions / Messages transport；Web Search / Fetch、File 或 Collection Search、X Search、Code Execution、Hosted Shell、Provider Apply Patch、Computer Use、Image Generation、server-side MCP、Tool Search、Memory、Programmatic Tool Calling、server-side multi-agent；Citation / Source、Structured Output、Prompt Cache；以及 reasoning effort 与 state signature / encrypted / interleaved continuation。

能力来源依次来自 registry 静态目录、Provider 探测缓存与显式配置/fixture override，但有效支持始终取所有已出现来源的**交集**。override 只能禁用或收紧，不能声明远端未支持的能力。协商结果包含 requested / supported / unsupported、选定 transport 与逐项 fallback；记录随 `RunRequest → ProviderLoopConfig → CanonicalModelRequest` 保存，adapter 只消费该记录完成 wire 翻译。

fallback 必须显式：能由 Core 本地工具等价承接时记录为 Client Tool；不能安全等价时记录 Reject 并在请求前失败。不得静默丢工具、把 Hosted / Extension 伪装成 ClientFunction，也不得把能力记录塞进 `provider_options`。Phase 15 的真实 OpenAI Responses、Anthropic Modern Messages 与 xAI Responses 能力分别由 P15-2 / P15-3 / P15-4 的 adapter contract 声明；P15-8 不猜测具体模型支持。

## 统一流式事件

```rust
pub enum ProviderStreamEvent {
    ResponseStarted { response_id: Option<String> },
    TextDelta(String),
    ThinkingDelta(String),
    ReasoningItem(ReasoningItem),
    ToolCallStarted { id: ToolCallId, name: String },
    ToolCallArgumentsDelta { id: ToolCallId, json: String },
    ToolCallCompleted { id: ToolCallId },
    ServerTool(ServerToolEvent),
    TranscriptEnvelope(ProviderTranscriptEnvelope),
    UsageUpdated(TokenUsage),
    ResponseCompleted(StopReason),
    ProviderMetadata(serde_json::Value),
    Error(ProviderError),
}
```

`ProviderEventSink` 是 push-based 异步 sink：adapter 逐事件 `emit`，上层可在实现中施加有界背压；`stream` 返回最终 stop reason、usage 与脱敏 metadata 摘要。

需正确处理：SSE；JSON Lines；chunked HTTP；Partial JSON；Tool Arguments 跨多 Chunk；Unicode 边界；Provider 提前断开；Provider 返回错误事件；多个 Tool Call 并行流式返回。

### Server Tool / Citation 统一口径（P15-5）

Provider-owned 工具只经 `ServerTool(ServerToolEvent)` 与 `TranscriptEnvelope(ProviderTranscriptEnvelope)` 进入 Core。`ClientFunction` 的结果走 `ContinuationMode::CoreSuppliedResult`；`ProviderHosted` / `ProviderExtension` 的结果走 `ContinuationMode::ProviderTranscript`，不得伪装成本地 `ToolResult`，也不得触发 scheduler 本地执行。transcript envelope 不携带 Provider 名称；无法确定的 wire 字段返回 `Unsupported`，不猜值。

| wire 口径 | canonical 映射 | 缺省 / 不支持规则 |
| --- | --- | --- |
| OpenAI Responses `web_search_call.{id,status,action}` | `id → tool_call_id`；`action → Started.arguments`；`status → Started / Completed / Failed` | 只有请求显式 `include: ["web_search_call.action.sources"]` 时才存在完整 sources；未知 status/action 不推断 |
| OpenAI `web_search_call.action.sources[]` | `Source { url, title, raw_metadata }` | 未返回的 title/snippet 保持空；额外字段仅进待脱敏的 `raw_metadata` |
| OpenAI `output_text.annotations[].url_citation` | `Citation { index: start_index, url, title, source_kind: Url }` | `end_index` 当前无 canonical 字段，不复用为其他含义 |
| Anthropic `server_tool_use.{id,name,input}` | `Started { tool_call_id, name, arguments }` | 只接受 `server_tool_use`，普通 `tool_use` 始终属于客户端工具 |
| Anthropic `*_tool_result.tool_use_id` 与 `web_search_result` / citation block | result 与调用按 `tool_use_id` 配对；搜索结果映射 `Source`，可表达的 cited text/document index 映射 `Citation` | `pause_turn` 续传完整 assistant content；不能无损表达的 citation 定位口径返回 `Unsupported` |
| xAI Responses `web_search_call` / `x_search_call` / `code_interpreter_call` / `file_search_call` / `mcp_call` | 对应 `ServerToolEvent` 生命周期，保留 wire call id | 本地 `function_call` 不得进入此通道 |
| xAI response `citations[]` 与 `output_text.annotations[]` | URL 列表映射 `Citation { url, source_kind: Url }`；annotation 另映射 title/start index | `citations[]` 只有 URL 时不猜 title/snippet；collections URI 原样保留为 url |

口径依据：[OpenAI Web search](https://developers.openai.com/api/docs/guides/tools-web-search)、[Anthropic Server tools](https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools)、[xAI Tool usage details](https://docs.x.ai/developers/tools/tool-usage-details) 与 [xAI Citations](https://docs.x.ai/developers/tools/citations)。

### Reasoning continuation 安全边界（P15-7）

`ThinkingDelta` / `ContentPart::Thinking` 只表达可见 thinking 文本；跨轮回灌凭证使用独立的 `ReasoningItem { id, summary, protected_blob_ref, opaque_metadata, continuation_metadata }`。`ProviderStreamEvent::ReasoningItem` 到达后按顺序写入 assistant `MessageCommitted` 的 `ContentPart::Reasoning`，因此普通事件重放即可恢复 canonical 推理链。`ThinkingContent.reasoning_item_id` 只关联同消息最近一项 reasoning，不再保存历史 `signature` 字段；旧事件中的 `signature` 反序列化时被丢弃。

`protected_blob_ref` 是稳定逻辑引用。凭证原文只进入独立 `protected-blob-store`：XChaCha20-Poly1305 随机 nonce、Provider + Session scope、版本化注入式 key resolver、密文摘要物理寻址、可恢复的 `pending / ready / deleting` 状态、引用计数 + retention GC、完整性校验与在线轮换。首次保护得到的引用归首个持久化事件所有，append 失败必须回滚该引用；Event/Projection/日志/GUI/导出/OS Keychain 均不得收到原文。密钥不可用或 AEAD 校验失败时显式失败，禁止回退明文。

| Provider wire | canonical 安全映射 | 回灌规则 |
| --- | --- | --- |
| OpenAI Responses reasoning item `id` / `summary[]` / `encrypted_content` | `id` 与安全 summary 进入 `ReasoningItem`；`encrypted_content` 原文加密入 Protected Blob | Responses adapter 解析 blob ref 后重建 reasoning input item；缺 `encrypted_content` 时不伪造 continuation |
| Anthropic `thinking { thinking, signature }` | thinking 文本留在关联的 `ThinkingContent`；`signature` 加密入 Protected Blob；metadata 只记 `thinking` block kind | Modern Messages adapter 必须按关联 id 重建原 thinking block；缺文本、签名或关联时返回 `Unsupported` |
| Anthropic `redacted_thinking { data }` | `data` 加密入 Protected Blob；metadata 只记 `redacted_thinking` kind | 原样重建 redacted block，不解码 `data` |
| xAI Responses reasoning `id` / `summary[]` / `encrypted_content`（兼容路径可返回 `reasoning_content`） | id/summary 进入 canonical；加密 continuation 原文只入 Protected Blob | xAI adapter 按实际 transport 重建；字段对不上返回 `Unsupported`，不把 OpenAI 口径当作猜测默认值 |

口径依据：[OpenAI reasoning items](https://platform.openai.com/docs/api-reference/responses-streaming/response/refusal?lang=python)、[Anthropic extended thinking](https://platform.claude.com/docs/en/docs/build-with-claude/extended-thinking)、[xAI multi-turn prompt caching](https://docs.x.ai/developers/advanced-api-usage/prompt-caching/multi-turn) 与 [xAI Responses advanced usage](https://docs.x.ai/developers/tools/advanced-usage)。三家 wire 翻译只存在于各自 provider crate，Agent Core 不按 Provider 名称分支。

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

## Quota adapter 边界

Provider 推理 crate 不承担额度累计或控制台抓取。Phase 14 的 `quota-service` 以独立 adapter 读取官方 billing/quota endpoint、OAuth console API 或显式启用的 WebScrape，并归一为 canonical `QuotaSnapshot`；供应商特例不得进入 Agent Engine。普通 inference key 与 Admin/Management/AccessKey 的权限口径必须分开，远端不支持时返回 `Unsupported`，不得根据推理响应猜测总额度。

当前六供应商的 exact/derived/scraped 能力、endpoint 与凭据要求见 [usage-quota](usage-quota.md#供应商能力矩阵)。本地用量统一来自 P18-8 Ledger，远端 billing 读数不创建第二套累计账本。

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
