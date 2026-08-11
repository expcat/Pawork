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

实现落地（P15-8）：canonical vocabulary 落在 `provider-api`——`ModelTransport { Responses, Messages, ChatCompletions }`、`ReasoningEffort { None, Low, Medium, High, XHigh, Max }`、`ReasoningConfig { effort, state }`（state 只表达 signature / encrypted / interleaved 是否需要，不存明文），`ModelCapabilities` v2 增补 `transport` / `hosted_tool_tags` / `citations` / `reasoning`（逐项 `#[serde(default)]`，旧目录 fail-closed）。`provider_runtime::negotiate::CapabilityNegotiator::negotiate(evidence, requirements) -> ResolvedCapabilities` 是纯函数入口（不触网、不读 Provider 名、不读 wall-clock）；`provider_runtime::negotiate::clamp_reasoning_to_thinking` 把 `XHigh / Max` clamp 为 `High` 供旧 P6 adapter 复用。协商结果随 `ProviderLoopConfig.reasoning → CanonicalModelRequest.reasoning` 流动，并在每轮 Provider 请求前以稳定 `provider_capability_negotiated` Diagnostic 事件落入观测通道（含 `chosen_transport` / `supported` / `unsupported` / `fallback`），可解释「为何降级」。

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

### Anthropic Modern Messages（P15-3）

`provider-anthropic` 的现代路径与 P6-2 基线同 crate 并存，由
[`modern::resolve`](../../crates/provider-anthropic/src/modern.rs) 纯函数按
「请求现代字段 × 模型能力声明」选择，不读 Provider 名：

- **触发与降级**：`reasoning` / `hosted_tools` / `extensions` /
  `response_format`（Json/JsonSchema）任一存在时触发现代路径；模型须在
  adapter contract（`builtin_models()`，如 `claude-sonnet-4-5`）中声明
  `transport: Messages`，否则显式降级到 P6-2 基线并发出可观察的
  `ProviderMetadata.degradation` note（不静默、不报错）。
- **Structured Output**：`output_config.format.json_schema` 原生映射，不再注入
  system 指令；通用 `Json`（无 schema）与模型未声明 structured output 时显式
  降级回 system 约束并记录 note。
- **effort / adaptive / interleaved thinking**：`ReasoningConfig.effort` →
  `output_config.effort`（low/medium/high/xhigh/max），
  `thinking: {"type":"adaptive"}`；模型不支持 reasoning 时经
  `clamp_reasoning_to_thinking` clamp 回 P6-2 budget 模式（XHigh/Max → High）。
- **server tools**：按 capability 映射 `web_search_20250305` /
  `web_fetch_20250521` / `code_execution_20250522` / `bash_20250124` /
  `text_editor_20250124` / `computer_20250124` / `mcp_connector` /
  `tool_search` / `memory` / `advisor`（advisor 尚无 canonical tag，经 canonical
  名称回退映射）。客户端 function 工具仍走 `name/input_schema`，两种位点不混用；
  无法表达的 capability（XSearch / ImageGeneration / ProgrammaticToolCalling 等）
  或模型未声明的 server tool 显式降级为 function calling（可观察 note）。
- **server tool 结果归一**：`server_tool_use` → `Started`；`<name>_tool_result`
  与 `<name>_tool_result_error` → SourceAdded / ProgramOutput /
  ComputerScreenshot / Completed / Failed；text 块上的 citations →
  `CitationAdded`。大输出只留 Artifact 引用（ADR-018）。轮末以
  `ProviderTranscript` 信封续接；只有客户端 `tool_use` 的 Core 结果映射
  `tool_result`，server tool 永不经该路径。
- **thinking signature 往返**：`content_block_stop` 捕获 thinking /
  redacted_thinking 块，经 `ReasoningContinuationStore`（接线方以
  `ReasoningStateBridge` + `BlobScope` 实现）保护为不透明 blob，事件只携带
  `protected_blob_ref`；回灌时取回载荷重建原块。未配置 store 时 fail-closed。
- **能力协商**：协商在 P15-8 `CapabilityNegotiator`（engine 层）；adapter
  contract 只负责声明能力，双方均不按 Provider 名分支。

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
| OpenAI Responses（P15-2） | `provider-openai`（`/v1/responses`） | Responses | 与 Chat Completions 并存；transport 由 P15-8 协商（`o3`/`gpt-4.1` 走 Responses，基线模型降级 Chat Completions）；原生 reasoning items、web/file search、code interpreter、image generation、local shell、apply patch、computer use、server-side MCP；citations 经 P15-5，encrypted reasoning 经 Protected Blob 边界（P15-7/ADR-032） |
| OpenAI-compatible / 本地服务 | `provider-openai-compatible` | Chat Completions | 覆盖云端兼容接口与 Ollama / vLLM / LM Studio；图片输入、reasoning、options 透传 |
| Anthropic | `provider-anthropic` | Messages API（现代 Messages 见 P15-3） | thinking budget 与 `max_tokens` 约束、tool_use、有界 prompt cache 断点、图片；JSON/Schema 通过 system 约束注入；P15-3 增加 `output_config.format` / effort / adaptive thinking / server tools / citations / thinking signature 往返 |
| Google Gemini | `provider-google` | `generateContent`（`alt=sse`） | `functionCall`、`thought` 流、`responseSchema`、`thinkingConfig`、cache usage；并行工具保留 id/name/ordinal 元数据（已降级次要 P1） |
| xAI Grok | `provider-xai` | Chat Completions + xAI Responses（P15-4 双传输） | API Key / OAuth bearer 双鉴权；`grok-4`/`grok-4-fast` 声明 Responses transport，其余模型降级 Chat Completions；Live Search（Web/X）、Collection Search、Code Execution、server-side MCP；reasoning → `ThinkingDelta`/`ReasoningItem`；独立错误归一 |
| 智谱 GLM | `provider-zhipu` | BigModel OpenAI-compatible | API Key Bearer、`reasoning_content` → `ThinkingDelta`、余额/内容审核错误归一 |
| 阿里 Qwen | `provider-qwen` | DashScope OpenAI-compatible | API Key Bearer；canonical thinking 仅对能力目录中的模型映射 `enable_thinking`；reasoning 归一 |
| Moonshot Kimi | `provider-moonshot` | OpenAI-compatible | API Key Bearer、Kimi reasoning → `ThinkingDelta`、限流/余额/内容安全错误归一 |

跨切能力（Phase 6）：thinking/reasoning level（P6-5）、图片输入（P6-6）、prompt cache 控制+命中（P6-7）、结构化输出（P6-8）、provider-specific options 透传+raw metadata（P6-9）。Agent Core 经 [`agent-engine/tests/no_provider_branch.rs`](../../crates/agent-engine/tests/no_provider_branch.rs) 回归守护，禁止按 provider 名走分支。Moonshot 原生余额查询不属于 Provider 基线，由 P14-2 与额度适配器承接。

### OpenAI Responses 传输路径（P15-2）

`provider-openai` 与 P6-1 Chat Completions 双传输并存：transport 选择由 P15-8 `CapabilityNegotiator`（纯函数，读 `ModelCapabilities.transport`，不读 Provider 名）驱动。内置目录中 `o3` / `gpt-4.1` 声明 `transport = Responses`，协商后走 `/v1/responses`；其余模型降级到 `/chat/completions` 并记录 `LegacyTransport` fallback。请求层未声明支持的 hosted tool 进入 `Reject`（fail-closed），不静默丢弃也不伪装成客户端 function。

- **请求转换**：canonical messages → Responses `input[]`（`message` / `function_call` / `function_call_output`）；system 消息抽到顶层 `instructions`；hosted capability → `web_search_preview` / `file_search` / `code_interpreter` / `image_generation` / `local_shell` / `apply_patch` / `computer_use_preview` / `mcp`（仅放行协商通过的类别）；客户端 function 工具声明为 Responses `function`。`reasoning.effort` 来自现代 `ReasoningConfig`（`XHigh/Max` clamp 为 `high`），`previous_response_id` 经 `provider_options` 续接。
- **output item → ProviderStreamEvent**：`response.output_text.delta` → `TextDelta`；`reasoning_summary_text.delta` → `ThinkingDelta`；`function_call` 流式 → `ToolCallStarted/ArgumentsDelta/Completed`；`reasoning` / `web_search_call` / `file_search_call` / `code_interpreter_call` / `computer_call` / `image_generation_call` / `mcp_call` / `local_shell_call` / `custom_tool_call` → `ServerTool` 生命周期事件，大输出走 `ArtifactId`（ProgramOutput / ComputerScreenshot）。
- **citations**：`web_search_call.action.sources[]` → `SourceAdded`；message `output_text.annotations[].url_citation` → `CitationAdded`，归属到产生它的 web_search call（可重放）。
- **reasoning 往返（ADR-032）**：wire `reasoning.encrypted_content` 只经 `ReasoningProtector` 边界（默认 `InMemoryReasoningProtector`，host 经 `OpenAiProvider::with_reasoning_protector` 注入 P15-7 `ReasoningStateBridge`）；canonical 事件只携带 `protected_blob_ref`，明文不入 Event / 日志 / GUI / Keychain。回灌时 `resolve_reasoning_inputs` 取回明文重建 input reasoning item。
- **续接**：只有客户端 `ContentPart::ToolResult`（CoreSuppliedResult）映射 `function_call_output`；server tool 永不经该路径，下一轮以 `response.id` 作为 `previous_response_id` 续接。
- **错误归一**：`normalize_responses_error` 把 vector store 未就绪 / code_interpreter 与 hosted shell 超时 / computer_use 需确认 / MCP 与 skill 不可用归一为统一 `ProviderError`（重试建议与 P2-10 一致）。
- **降级可观察**：基线模型（如 `gpt-4o`）命中 `/chat/completions`；未声明 hosted tool 不进入任何请求体。Mock smoke 在 `provider-openai/tests/responses.rs` 覆盖 item→event、citations、reasoning 往返、降级与 `no_provider_branch` 断言。

> 完整 Responses 能力矩阵（真实 API 端到端、Programmatic Tool Calling、API Multi-Agent、持久化 protector）在 P15-9 集中验收；本段描述 P15-2 定向实现的可观察行为。

### xAI Responses 传输路径（P15-4）

`provider-xai` 与 P6-10 Chat Completions 双传输并存：transport 选择由 P15-8 `CapabilityNegotiator`（纯函数，读 `ModelCapabilities.transport`，不读 Provider 名）驱动。内置目录中 `grok-4` / `grok-4-fast` 声明 `transport = Responses`，协商后走 `/v1/responses`；`grok-3` / `grok-2` 等基线模型降级到 `/chat/completions` 并记录 `LegacyTransport` fallback。鉴权复用 P6-10 的 API Key / OAuth bearer（订阅模式由组合层以 `OAuthBearer` 构造 adapter）。

- **请求转换**：canonical messages → Responses `input[]`（`message` / `function_call` / `function_call_output`）；system 消息抽到顶层 `instructions`；hosted capability → `web_search` / `x_search` / `file_search`（Collection Search，`collection_ids`/`vector_store_ids` 透传）/ `code_interpreter`（Code Execution）/ `mcp`（server-side MCP，仅放行协商通过的类别）；客户端 function 工具声明为 Responses `function`。`reasoning.effort` 来自现代 `ReasoningConfig`（`XHigh/Max` clamp 为 `high`），`previous_response_id` 经 `provider_options` 续接。
- **output item → ProviderStreamEvent**：`response.output_text.delta` → `TextDelta`；`reasoning_summary_text.delta` → `ThinkingDelta`；`function_call` 流式 → `ToolCallStarted/ArgumentsDelta/Completed`；`reasoning` / `web_search_call` / `x_search_call` / `file_search_call` / `code_interpreter_call` / `mcp_call` → `ServerTool` 生命周期事件，大输出走 `ArtifactId`（ProgramOutput / MCP output_file）。
- **Live Search / Collection 结果归一**：Web/X Search 的 `sources[]`（`type: url`/`x`/`post`）与 Collection 的 `results[]`（`type: document`）经 `live_search_source_to_source` 归一为 `SourceAdded`，保留 url/title/snippet/text/document_index 与原始 `raw_metadata`；message `output_text.annotations[].url_citation` → `CitationAdded`。后续轮次以 `response.id` 作为 `previous_response_id` 续接（`ProviderTranscript` 通道），不经过客户端 `function_call_output`。
- **reasoning 往返（ADR-032）**：wire `reasoning.encrypted_content` 只经 `ReasoningProtector` 边界（默认 `InMemoryReasoningProtector`，host 经 `XaiProvider::with_reasoning_protector` 注入 P15-7 `ReasoningStateBridge`）；canonical 事件只携带 `protected_blob_ref`，明文不入 Event / 日志 / GUI / Keychain。回灌时 `resolve_reasoning_inputs` 取回明文重建 input reasoning item（复用 P15-7 `parse_responses_reasoning` / `to_reasoning_item` / `to_responses_input_reasoning`）。
- **错误归一**：`normalize_responses_error` 把 Live Search / Web / X Search 配额、Collection 未就绪/未找到、code_interpreter 超时、MCP 不可用/未授权、billing/insufficient_quota 归一为统一 `ProviderError`（重试建议与 P2-10 一致）。
- **降级可观察**：基线模型（`grok-2`/`grok-3`）命中 `/chat/completions`；未声明 hosted tool 不进入任何请求体（fail-closed）。Mock smoke 在 `provider-xai/tests/responses.rs` 覆盖 Responses+reasoning item→event、Live Search sources、Web/X/Collection/Code/MCP 事件、reasoning 往返、双鉴权、降级与 `no_provider_branch` 断言。

> 完整 xAI Responses 能力矩阵（真实 API 端到端、订阅模式 token endpoint、持久化 protector）在 P15-9 集中验收；本段描述 P15-4 定向实现的可观察行为。

### 内置模型目录新鲜度

OpenAI、Anthropic、Google、Qwen 与 Moonshot 的 `builtin_models()` 数据快照日期为 **2026-08-09**；xAI 的目录快照日期为 **2026-08-12**（P15-4：`grok-4`/`grok-4-fast` 声明 Responses transport，`grok-3`/`grok-2` 声明 Chat Completions）。源码入口也标注同一日期。这些目录优先表达能力而非仅列名；发现新模型与远端 `/models` + 能力探测属于后续显式跟踪项。智谱当前复用带鉴权的远端目录。

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
