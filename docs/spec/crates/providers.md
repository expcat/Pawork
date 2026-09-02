# pawork-providers

> 首发模型渠道适配器：把 canonical domain 请求（`CanonicalModelRequest`）翻译成各厂商 wire 协议并把响应流映射回 `ProviderStreamEvent`；只依赖 `pawork-domain`（见 [domain.md](domain.md)），被 `pawork-app` 生产依赖、被 `pawork-engine` 仅 dev 依赖。

## 1. 职责与边界

- **职责**：承载四种 transport 形态（OpenAI-compatible Chat Completions、Responses、Anthropic Messages、xAI 按模型能力二选一）、模型目录与能力证据（`ModelRegistry`）、能力协商（`CapabilityNegotiator`）、计价与用量归一（pricing/usage）、厂商错误归一（error_table）、reasoning 续传保护接口（`ReasoningProtector`）、八条首发通道的静态注册（`CHANNEL_REGISTRY`，SET-4 新增 Kimi Platform / Kimi Code）与网络层（HTTP/SSE/错误分类）。
- **不做**：凭证的存取与解析（`pawork-auth`，见 [auth.md](auth.md)）；事件持久化；Agent loop 编排；GUI。装配（把 preset + 凭证 + registry 组装成 Provider 实例）由 `pawork-app` 承载。
- **模块纪律**：core 纯逻辑模块（`registry` / `pricing` / `usage` / `negotiate` / `reasoning` / `error`）不得引用 `net` 模块，由 `lib.rs` 内 `module_discipline` 测试护航。
- **engine 不认厂商名**：能力差异一律走 registry/capability 证据；`pawork-engine` 仅以 dev-dependency 引用本包，用 `CHANNEL_REGISTRY` 派生 no-provider-branch 守护测试名单。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~120 | crate 门面：模块声明与 feature 门控 re-export；`is_credential_header`（五个凭证头小写匹配）；`module_discipline` 测试 |
| `src/error.rs` | ~20 | `RegistryError`（`NotFound` / `DuplicateAlias` / `DuplicateModelId`） |
| `src/error_table.rs` | ~160 | `VENDOR_ERROR_RULES` 数据表 + `normalize_vendor_error`：按厂商子串把错误改判为更精确的 `ProviderErrorKind`（如 ChatGPT usage limit、xAI live_search quota） |
| `src/provider.rs` | ~330 | `OpenAiCompatibleConfig` / `OpenAiCompatibleProvider`：Chat Completions transport 的 `ModelProvider` 实现；构造期拒绝 config 头携带凭证头 |
| `src/request.rs` | ~470 | `to_chat_completions_body`：canonical → Chat Completions 请求体；`provider_options` 保留键忽略并 `tracing` 警告 |
| `src/stream.rs` | ~230 | `chunk_to_events` / `is_done` / `ChunkState`：Chat Completions SSE chunk → `ProviderStreamEvent`（文本/工具调用增量、usage、finish_reason） |
| `src/usage.rs` | ~300 | `normalize_usage`（多厂商字段名归一为 `TokenUsage`）、`map_stop_reason`、`UsageAccumulator`（会话级累计） |
| `src/pricing.rs` | ~200 | `ModelPricing` / `estimate_cost`（micro-unit 定点算费，`MILLION` 基数）、`BUILTIN_RATE_CARD`（`"builtin"`）与 `BUILTIN_RATE_VERSION`（`"2026-08-15"`） |
| `src/registry.rs` | ~1.9k | `ModelRegistry`（目录 + 别名 + 三源能力证据 + 动态发现合并）、`CatalogEntry`、`CapabilityEvidence` / `CapabilitySource`、`merge_capabilities`、`ProviderProbe` / `ProbeError` / `ProviderCapabilitySource`、`caps` 构造 helper |
| `src/negotiate.rs` | ~500 | `CapabilityNegotiator::negotiate`（纯函数协商）与 `clamp_reasoning_to_thinking` |
| `src/reasoning.rs` | ~100 | `ReasoningProtector` trait（protect/recover 不透明 payload）与 `ReasoningProtectError`（`Unavailable` / `Corrupted` 判别） |
| `src/memory_protector.rs` | ~110 | `InMemoryReasoningProtector`：HashMap 存不透明字节，测试/内存场景用 |
| `src/responses.rs` | ~860 | Responses transport 共享件：`ResponsesTransport(Config)` / `ResponsesWireOptions` / `to_responses_body` / `ResponsesStreamAssembler` / `ResponsesAssemblyEvent` / `ResponsesFinalState`；保留键防覆盖；凭证头拒绝 |
| `src/responses_reasoning.rs` | ~250 | crate 私有：Responses reasoning item → canonical `ReasoningItem`（提取 `encrypted_content` 交 protector；容忍历史 hint 键拼写） |
| `src/net/mod.rs` | ~10 | re-export `http` / `sse` / `retry` |
| `src/net/http.rs` | ~400 | `HttpClient` / `HttpClientConfig`（builder：timeout/proxy/user_agent/自定义头/禁系统代理）、`is_local_target` / `loopback_aware_proxy`（本地目标绕过代理）；Debug 输出对凭证头脱敏 |
| `src/net/sse.rs` | ~450 | 增量 `SseParser`（feed/finish）、`SseEvent` / `SseParseError`、`MAX_BUFFER_BYTES`（1 MiB 缓冲上限，UTF-8 安全跨 chunk） |
| `src/net/retry.rs` | ~220 | `classify_status` / `classify_request_error`（HTTP 状态与 reqwest 错误 → `ProviderError`，解析 `Retry-After`，消息脱敏）、`parse_retry_after` |
| `src/channels/mod.rs` | ~60 | 八通道 feature 门控的模块声明与 re-export |
| `src/channels/registry.rs` | ~360 | `CHANNEL_REGISTRY`（八行静态 preset）、`ChannelPreset`（含 `display_name` 与 `auth_methods` 数据字段，SET-4 起不再按 kind 派生）/ `ChannelKind`、`OAuthPreset(Data)` / `OAuthFlow(Data)`、`channel_preset`、`is_enabled`（唯一 cfg 求值点） |
| `src/channels/api_key.rs` | ~230 | `ApiKeyChannelConfig` / `ApiKeyChannelProvider`：API-key 通道共用适配器（五行，含 kimi-platform；xAI 双认证亦复用 `verify_api_key`）；默认 Chat Completions，逐模型显式声明才走 Responses；`verify_api_key` 用候选 key 发一次性 `GET /models` 做写前验证（不持久化） |
| `src/channels/chatgpt.rs` | ~280 | `ChatGptConfig` / `ChatGptProvider`：ChatGPT OAuth 通道（Responses transport、`chatgpt-account-id` / `originator` 头、`client_version` 校验、`DEFAULT_BASE_URL`） |
| `src/channels/xai.rs` | ~280 | `XaiConfig` / `XaiProvider`：xAI Grok OAuth 通道，按模型 capability 声明选 Responses 或 Chat Completions；`xai_builtin_models` / `DEFAULT_BASE_URL` |
| `src/channels/kimi.rs` | ~200 | `KimiCodeConfig` / `KimiCodeProvider`：Kimi Code OAuth 通道（SET-4 A2），只接受 OAuth bearer、只走 Chat Completions（`https://api.kimi.com/coding/v1`）；`builtin_models` 版本固定目录（id 取自官方 kimi-cli / Models.dev，能力未知不推断） |
| `src/channels/anthropic/mod.rs` | ~20 | re-export 与 `ANTHROPIC_VERSION`（`anthropic-version` 头值） |
| `src/channels/anthropic/provider.rs` | ~1.1k | `AnthropicProvider(Config)`：Messages transport；`prepare_request` 能力收口（§4.3）；`builtin_models` 静态目录（claude-3-5-sonnet / haiku） |
| `src/channels/anthropic/request.rs` | ~790 | `to_messages_body(_with_plan)` / `MessagesWirePlan`：system 提升、`tool_use` 块、`thinking` 与 `cache_control` 按 plan 写 wire |
| `src/channels/anthropic/stream.rs` | ~690 | `parse_event` / `event_to_events` / `AnthropicStreamState` / `StreamOutput`：Anthropic SSE 事件 → canonical 事件；thinking signature 以 `PendingSignature` 输出待 protect |

共 27 个 `.rs` 文件，约 10.4k 行。

## 3. 对外 API 面

### 3.1 Provider adapters（`pawork_domain::ModelProvider` 实现）

trait 面为 `id()` / `list_models(credential)` / `stream(request, sink, cancel)`；所有实现把事件逐个 `sink.emit(ProviderStreamEvent)`，`stream` 返回 `ModelResponseSummary`（`stop_reason` / `usage` / `response_id` / `provider_metadata`），错误统一 `ProviderError`。`CancellationToken` 在发请求前与流式循环内多点检查：预取消不发 HTTP，流中取消报 `Cancelled`。

- `OpenAiCompatibleProvider::new(config, credential)`：`OpenAiCompatibleConfig::new(base_url)` 默认 `provider_id = "openai-compatible"`，可 `with_provider_id`。构造期若 config 自定义头含凭证头则拒绝（凭证只能经 `ResolvedCredential` 注入为 `Authorization: Bearer`）。
- `AnthropicProvider`（feature `anthropic`，默认开启）：认证头 `x-api-key` + `anthropic-version`；可 `with_registry(Arc<ModelRegistry>)` 注入能力证据、`with_reasoning_protector` 注入续传保护。
- `ChatGptProvider`（feature `chatgpt-oauth`）：内部复用 `ResponsesTransport`；OAuth Bearer + `chatgpt-account-id`（构造入参或从 id_token JWT claim 提取）+ `originator: codex_cli_rs` 头；`client_version` 字符集校验，`/models?client_version=` 过滤目录。
- `XaiProvider`（feature `xai-oauth`）：OAuth Bearer 或 API key（SET-4 A3 双认证，Bearer 用法相同）；按模型 capability 的 `transport` 声明路由 Responses / Chat Completions。
- `KimiCodeProvider`（feature `kimi-code`）：OAuth Bearer；固定 Chat Completions，`list_models` 返回版本固定 builtin。
- `ApiKeyChannelProvider`（任一 API-key feature）：以 `&'static ChannelPreset` 构造，构造期 fail-closed——preset 必须声明 api_key 认证方法（`auth_methods` 数据字段）且 `is_enabled`，凭证必须存在且为 API key 形态，config 固定头不得含凭证头。
- `verify_api_key(config, candidate_key)`（async，任一 API-key feature）：SET-2 写前验证入口——用候选 key 构造一次性 adapter 发 `GET /models`，只返回 `Ok(())` / `ProviderError`；key 只在内存短暂停留、不落任何后端，供宿主 `auth_set_api_key` 在 `store_default_api_key` 之前校验。

四种 transport 形态对照：

| transport | 请求组装 | 流解析 | 认证头 | 使用方 |
| --- | --- | --- | --- | --- |
| Chat Completions | `to_chat_completions_body` | `SseParser` + `chunk_to_events` | `Authorization: Bearer` | OpenAiCompatible、API-key 通道（五行）、xAI Chat 模型、Kimi Code |
| Responses | `to_responses_body` | `SseParser` + `ResponsesStreamAssembler` | Bearer（ChatGPT 另加 account/originator 头） | ChatGPT、xAI Responses 模型、API-key 通道逐模型声明时 |
| Anthropic Messages | `to_messages_body_with_plan` | `SseParser` + `parse_event` | `x-api-key` + `anthropic-version` | AnthropicProvider |
| 模型能力路由 | —（复用上两行） | — | — | XaiProvider / ApiKeyChannelProvider 按 `ModelCapabilities::transport` 选择 |

### 3.2 通道注册表（channels/registry）

`CHANNEL_REGISTRY: &[ChannelPreset]` 八行（顺序即 `pawork models` / `auth list` 展示顺序）；行本身**不带 cfg**，feature 是数据字段，`is_enabled(preset)` 是唯一的 `cfg!` 求值点（未知 feature 名返回 false，fail-closed）。`channel_preset(id)` 按 id 查行；`ChannelPreset::oauth_preset()` 把 const 镜像 `OAuthPresetData` 转运行期 `OAuthPreset { client_id, token_url, scopes, flow }`（与 config `[oauth.<id>]` 覆盖共用同一形状）。`ChannelPreset` 另携带 `display_name`（品牌展示名）与 `auth_methods` 数据字段（SET-4 起不再按 kind 派生：纯 API-key 行为 `["api_key"]`、纯 OAuth 行为 `["oauth"]`、xAI 双认证为 `["oauth","api_key"]`），SET-2 GUI Settings 的通道 descriptor 与认证方式列表直接由此派生，宿主 / Desktop 不自建品牌表。

| provider_id | 凭证形态（ChannelKind） | 默认协议 / endpoint | feature | OAuth 流 |
| --- | --- | --- | --- | --- |
| `chatgpt` | ChatGptOAuth（Bearer + account 头） | Responses / `https://chatgpt.com/backend-api/codex` | `chatgpt-oauth` | PKCE（auth.openai.com；redirect 固定 `http://localhost:1455/auth/callback`；scopes 含 `api.connectors.read/invoke`；附加 `codex_cli_simplified_flow` 等参数） |
| `xai` | XaiOAuth（OAuth Bearer 或 API key，SET-4 双认证） | 按模型能力选 Responses/Chat / `https://api.x.ai/v1` | `xai-oauth` | Device Flow（auth.x.ai；scopes 含 `grok-cli:access`、`api:access`） |
| `glm-coding` | ApiKey | Chat Completions / `https://api.z.ai/api/coding/paas/v4` | `glm-coding` | — |
| `opencode-go` | ApiKey | Chat Completions / `https://opencode.ai/zen/go/v1` | `opencode-go` | — |
| `qwen-token-plan` | ApiKey | Chat Completions / `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | `qwen-token-plan` | — |
| `deepseek` | ApiKey | Chat Completions / `https://api.deepseek.com` | `deepseek` | — |
| `kimi-platform` | ApiKey | Chat Completions / `https://api.moonshot.ai/v1` | `kimi-platform` | — |
| `kimi-code` | KimiOAuth（Bearer） | Chat Completions / `https://api.kimi.com/coding/v1` | `kimi-code` | Device Flow（auth.kimi.com；client_id `17e5f671-d194-4dfb-9706-5516cb48c098`；scope `kimi-code`；端点与 MoonshotAI/kimi-cli 官方源一致） |

补充语义：

- OAuth 行的公开 client_id / 端点预置在注册表源码中（各厂商公开 client 参数，非 Secret）；`OAuthPresetData` 是 static 初始化友好的 `&'static str` 镜像，运行期 `to_preset()` 转 String 形态后与 config `[oauth.<id>]` 覆盖走同一形状。
- `ChannelKind` 四变体即四种装配形态：`ApiKey`（五行通道复用 OpenAI-compatible transport，可逐模型切 Responses）、`ChatGptOAuth`（固定 Responses）、`XaiOAuth`（按模型 capability 选 Chat/Responses；SET-4 起凭证可为 OAuth 或 API key）、`KimiOAuth`（固定 Chat Completions）。
- feature `anthropic`（默认开）承载 Messages transport 适配器，不属于 CHANNEL_REGISTRY 八行——它是 transport 基线而非首发通道行。

### 3.3 模型目录与能力证据（registry）

- `ModelRegistry`：`empty()` / `builtin()` 构造；`register` / `try_register`（`RegistryError::DuplicateModelId` / `DuplicateAlias` 拒绝重复登记，`resolve` 未命中对应 `NotFound` 语义）；`extend_with`（批量合并）；`merge_provider_models` / `merge_provider_source`（把 Provider `list_models` 发现结果并入，静态已有时逐字段交集收窄）；`resolve(id_or_alias)`（模型 id 与别名同一命名空间查找）；`list` / `filter(required)`（按能力子集筛选）；`validate_context(id, input_tokens)` / `estimate_cost(id, usage)`（经 `CatalogEntry` 的 pricing）。`CatalogEntry::to_definition()` 转 domain `ModelDefinition`。
- 能力三源：`CapabilitySource::{Static, Probe, Override}`；`CapabilityEvidence { static_declared, probe_declared, override_declared }`，`merged()` 对已出现的来源逐字段取交集（缺失来源不约束）。运行期可 `set_override` / `remove_override`（override 只能收窄）、`record_probe` / `clear_probe`（探测结果按 provider 记录）。`capability_evidence(model)` / `capability_snapshot()` 导出证据。
- `ProviderCapabilitySource` trait / `ProviderProbe` / `ProbeError`：动态探测抽象。`caps(...)` 是测试与静态目录用的 `ModelCapabilities` 构造 helper。

### 3.4 能力协商（negotiate）

`CapabilityNegotiator::negotiate(&CapabilityEvidence, &CapabilityRequirements) -> ResolvedCapabilities`：无状态纯函数，不触网、不读 Provider 名、不读 wall-clock。

- 输入：证据快照（三源）× 请求要求（`transport_pref` / `required_tools` / `reasoning` / `citations`）。
- 输出 `ResolvedCapabilities` 字段：`chosen_transport`、`requested` / `supported` / `unsupported` 三集合（保证 `requested == supported ∪ unsupported`）、`fallback` map（键 → `CapabilityFallback`）。
- `CapabilityFallback` 变体语义：`Reject(原因)`（adapter 必须在发 HTTP 前拒绝）、`LegacyTransport`（请求现代 transport 但模型只有 Chat Completions 基线，降级记录）、`ClampedEffort`（reasoning `XHigh`/`Max` 在模型不支持细粒度 effort 时 clamp 为 `High`）。
- transport 选择顺序：请求偏好 ∈ 模型声明 → 采用偏好；否则用模型声明的 transport；模型只有基线时退 ChatCompletions（必要时记 `LegacyTransport`）。
- reasoning：显式 `ReasoningConfig` 优先于旧 `ThinkingConfig.level`；请求 reasoning 但模型 `thinking == false` 时整项进 `unsupported` + `Reject`。
- `clamp_reasoning_to_thinking(reasoning, thinking)` 供 adapter 复用（Anthropic 把 effort 翻成 thinking budget），避免形成第二套 clamp 双轨。

### 3.5 计价与用量

- `normalize_usage(&Value) -> TokenUsage`：usage 视图可在顶层或嵌套 `"usage"` 键下；字段名兼容 OpenAI（`prompt_tokens` / `completion_tokens`、嵌套 `prompt_tokens_details.cached_tokens`）与 Anthropic（`input_tokens` / `output_tokens`、`cache_read_input_tokens` / `cache_creation_input_tokens`）等拼写；缺失按 0，绝不 panic。
- `map_stop_reason(finish, has_tool_calls) -> StopReason`：`has_tool_calls` 为真直接 `ToolUse`；`stop`/`end_turn`/`ended` → `Completed`；`length`/`max_tokens`/`max_output_tokens` → `MaxTokens`；`tool_calls`/`tool_use`/`function_call` 等 → `ToolUse`；`content_filter`/`safety` → `ContentFiltered`；`cancelled` → `Cancelled`；`None`（协议正常收尾但未给 finish）→ `Completed`；其余 → `Other(原文)`。
- `UsageAccumulator`：请求内「最新快照覆盖」、跨请求「累加」——`record(request_id, usage)` 同请求覆盖，请求 id 变化或 `finish_request()` 时把上一请求终值并入 `total()`；`current()` 读进行中快照。假设同一时刻只有一个进行中请求，交错回放不在支持范围。
- `estimate_cost(&TokenUsage, &ModelPricing) -> Cost`：micro-unit（`MILLION` 基数）定点计算避免浮点误差；`ModelPricing` 区分 input/output/cache 读写费率。

### 3.6 reasoning 续传保护

`ReasoningProtector`（`Send + Sync`）异步 trait：

- `protect(payload) -> blob_ref`：把厂商返回的 reasoning 载荷（thinking signature / encrypted_content 等不透明字节）封存，返回可安全入事件流的引用；
- `recover(blob_ref) -> payload`：续传时还原原始载荷交回 wire 组装；
- 实现只负责加解密与存取，**不解释内容**；错误 `ReasoningProtectError` 提供 `is_unavailable`（后端不可用）/ `is_corrupted`（数据损坏）判别，供上层决定降级或报错。
- 本包只带测试实现 `InMemoryReasoningProtector`；生产实现 `SwappableReasoningProtector`（含 master key 管理）在 `pawork-app`（见 [app.md](app.md)）。

### 3.7 网络层（net，feature 无关）

- `HttpClientConfig::builder()`：`timeout` / `no_timeout` / `proxy` / `user_agent` / `header` / `disable_system_proxy`；`HttpClient::new(config)`（代理不合法等报 `ProviderError`）。凭证头在 Debug 输出中显示为脱敏值。`is_local_target(host)` / `loopback_aware_proxy(proxy)`：loopback 目标绕过代理。
- `SseParser::new()` / `feed(&[u8]) -> Vec<Result<SseEvent, SseParseError>>` / `finish()`：增量解析，容忍 CRLF/LF、注释行、跨 chunk UTF-8 截断；缓冲超 `MAX_BUFFER_BYTES`（1 MiB）报错。
- `classify_status(status, retry_after, body_snippet)`：状态码 → `ProviderErrorKind` 固定映射——401 `Authentication`、403 `Authorization`、404 `ModelNotFound`、408 `Timeout`、413 `ContextTooLarge`、429 `RateLimited`、400 `InvalidRequest`、451 `ContentFiltered`、402 `QuotaExceeded`、500/502/503/504 `ProviderUnavailable`，其余 4xx/5xx 按类别兜底。`message` 固定为 `HTTP <code>`——`body_snippet` **不写入** message（上游正文可能回显 token）；`Retry-After`（秒数或 RFC 7231 IMF-fixdate，`parse_retry_after`）仅对 retryable 类采纳为 `retry_after_ms`。
- `classify_request_error(reqwest::Error)`：timeout → `Timeout`、connect → `Network`、body/decode → `StreamInterrupted`、request 构造 → `InvalidRequest`、其余 → `Network`。

### 3.8 wire 纯函数与错误表

- Chat Completions：`to_chat_completions_body`（`provider_options` 中保留键如 `model`/`messages`/`stream` 被忽略并告警）；`chunk_to_events(data, &mut ChunkState)` / `is_done`。
- Responses：`to_responses_body(request, reasoning_inputs, ResponsesWireOptions)`（同样拦截保留键覆盖）；`ResponsesStreamAssembler::feed/finish` 产出 `ResponsesAssemblyEvent` 与 `ResponsesFinalState`。
- Anthropic：`to_messages_body` / `to_messages_body_with_plan(request, &MessagesWirePlan)`；`parse_event(data, &mut AnthropicStreamState) -> Vec<StreamOutput>`（`Event` / `MappingError` / `ReasoningError` / `PendingSignature`）；`event_to_events` 兼容入口；`ANTHROPIC_VERSION` 常量。
- `normalize_vendor_error(vendor, error)`：按 `VENDOR_ERROR_RULES` 细化——`VendorErrorRule { vendor, needles, kind, retryable, detail, diagnostic_key }`，消息小写后须命中该厂商规则的**全部** needles 才改判 kind/retryable 并写入 diagnostics；未命中原样返回。表内只登记本期渠道的稳定标记：chatgpt（usage limit → `QuotaExceeded`、account deactivated → `Authorization`）、xai（live_search quota → `RateLimited`、collection not_ready → `ProviderUnavailable`、insufficient_quota → `QuotaExceeded`）、qwen-token-plan（数据检查 → `ContentFiltered`、throttling → `RateLimited`、quota_exhausted → `QuotaExceeded`）、glm-coding（错误码 1113 → `QuotaExceeded`、1301/敏感 → `ContentFiltered`）。

## 4. 核心行为与数据流

### 4.1 一次 Chat Completions stream 请求（OpenAiCompatible / ApiKeyChannel）

1. 调用方（`pawork-app` 装配层）把 `ResolvedCredential` 注入 Provider 构造；构造期校验固定头无凭证头（含凭证头直接构造失败）。
2. `stream(request, sink, cancel)`：先查 `cancel`——预取消不发任何 HTTP 请求。
3. `to_chat_completions_body` 生成请求体：messages / tools / response_format 按 canonical 语义翻译，`provider_options` 白名单透传（保留键忽略并 `tracing` 告警）。
4. `HttpClient` POST `{base_url}/chat/completions`，凭证经 `Authorization: Bearer` 头注入；请求阶段错误走 `classify_request_error`，非 2xx 走 `classify_status`，再经 `normalize_vendor_error` 按渠道细化。
5. 响应字节流喂 `SseParser::feed`；每个 SSE event 的 data 经 `chunk_to_events`（`ChunkState` 跨 chunk 组装工具调用 id/name/参数增量）映射为 `ProviderStreamEvent`，逐个 `sink.emit`。
6. usage chunk 经 `normalize_usage` 发 `UsageUpdated`；`finish_reason` 经 `map_stop_reason` 发 `ResponseCompleted`；`[DONE]` 到达而无 finish_reason 时按 `Completed` 收尾。
7. 每收到一个 chunk 重置读超时（长流不误杀）；流中断（未见完成信号）报 `StreamInterrupted`；取消点贯穿字节循环与事件循环。
8. `ApiKeyChannelProvider` 额外一步：若模型 capability 显式声明 Responses transport，则路由到共享 `ResponsesTransport`——按能力数据路由，不按通道名分支。

### 4.2 Responses transport（ChatGPT / xAI Responses 模型）

1. `requirements_from_request` → `CapabilityNegotiator::negotiate`；被拒能力（`Reject`）在发 HTTP 前变成 `InvalidRequest` 错误。
2. 请求中的历史 `ReasoningItem` 经 `ReasoningProtector::recover` 还原成 reasoning input（`encrypted_content`），交给 `to_responses_body(request, reasoning_inputs, wire)`。
3. `ResponsesWireOptions` 控制 wire 细节：`store`（ChatGPT OAuth 默认 `Some(false)`，xAI 与 API-key 通道不设）与 `include_encrypted_reasoning`（请求返回加密 reasoning continuation）；保留键防覆盖同样生效。
4. 认证头：ChatGPT 为 Bearer + `chatgpt-account-id`（构造入参或 id_token JWT claim 提取）+ `originator: codex_cli_rs`；xAI 仅 Bearer。
5. SSE → `ResponsesStreamAssembler::feed`：文本增量、工具调用、reasoning item（`encrypted_content` 经 `protect` 换 `protected_blob_ref` 后发 `ReasoningItem` 事件）、usage 与完成信号。
6. malformed 事件立即报错——即便其后跟着完成事件也不救回（防止半损坏流被误判成功）；`finish()` 输出 `ResponsesFinalState` 收尾校验。

### 4.3 Anthropic Messages 能力收口（`prepare_request`，写 wire 或发 HTTP 前拒绝）

1. `capability_evidence(model)`：优先注入的 `ModelRegistry`，退回 `builtin_models` 静态声明。
2. `negotiate` 后 `first_reject`：任何 `Reject`（如 hosted tools 未声明、citations 不支持）→ `InvalidRequest`，**不发 HTTP**。
3. prompt cache 三态：`Disabled` 不写；`Automatic` 依能力；`Required` 且能力未声明 → 拒绝。
4. thinking 预检：`reasoning` 优先经 `clamp_reasoning_to_thinking` 翻译；budget < `MIN_THINKING_BUDGET_TOKENS`（1024）、`temperature != 1.0`、`max_output_tokens <= budget` 均拒绝。
5. 历史 thinking 块经 `ReasoningProtector::recover` 还原签名（`resolve_thinking_blocks`）。
6. 组装 `MessagesWirePlan { write_cache, thinking_budget, resolved_thinking_blocks }` → `to_messages_body_with_plan`；`Required` 但 body 无任何 `cache_control` 断点 → 拒绝。
7. 发 HTTP（`x-api-key` + `anthropic-version`）；SSE → `parse_event`：thinking signature 以 `PendingSignature` 输出，经 `protect` 变 `ReasoningItem`（`continuation_metadata` 带 anthropic model hint）；无 `message_stop` 即 `StreamInterrupted`。

### 4.4 usage / pricing 计量

1. 每个 usage chunk → `normalize_usage` → `UsageUpdated` 事件（含 cache 读写 token）。
2. 会话层 `UsageAccumulator::record` 以 `RequestId` 维度覆盖式累计，`finish_request` 后进入 `total`。
3. 费用 `ModelRegistry::estimate_cost(id_or_alias, usage)` 或直接 `estimate_cost(usage, pricing)`；内置费率卡 `BUILTIN_RATE_CARD`/`BUILTIN_RATE_VERSION` 标注来源与版本。

### 4.5 目录合并与能力证据流

1. 装配期：`ModelRegistry::builtin()` 或空表起步，`extend_with` 并入各通道静态目录（如 `anthropic_builtin_models` / `xai_builtin_models`）。
2. 运行期：`merge_provider_models` / `merge_provider_source` 并入 `list_models` 发现结果——静态没有的模型新增，已有的逐字段交集收窄（动态声明不能放宽静态口径）。
3. probe：`record_probe(provider, ProviderProbe)` 按 provider 记录探测能力，`clear_probe` 清除；override：`set_override` / `remove_override` 运行期人工修正（只可收窄）。
4. `capability_evidence(model)` 输出三源快照（`static_declared` / `probe_declared` / `override_declared`），`merged()` 对已出现来源逐字段交集后供 negotiate 使用；`capability_snapshot()` 全量导出（诊断/展示）。
5. adapter 侧兜底：如 `AnthropicProvider` 未注入 registry 时退回 `builtin_models` 静态声明，未知模型退回 Messages 基线能力——证据永远存在，不出现「无证据直接放行」。

## 5. 契约与不变量

- **凭证只经 `ResolvedCredential` 注入**：`is_credential_header` 列出的五个头（`authorization` / `proxy-authorization` / `api-key` / `x-api-key` / `x-goog-api-key`）不得出现在任何通道的固定自定义头里，构造期 fail-closed。`ResolvedCredential` 由 domain 定义：Debug 脱敏、无 `Serialize`。
- **Secret 不入日志/事件**：`HttpClient` Debug 输出与 `classify_*` 错误消息对凭证脱敏；`ProviderStreamEvent` 不携带明文凭证。
- **reasoning 载荷不明文外泄**：`encrypted_content` / thinking signature 只经 `ReasoningProtector::protect` 换成 `protected_blob_ref` 后进入事件流；provider hint 走 `ReasoningItem.continuation_metadata` 命名空间键（如 anthropic model hint），engine 不解释其内容。
- **engine 不按厂商名分支**：能力差异一律经 registry 证据 + negotiate 结果表达；`pawork-engine` 的守护测试名单从 `CHANNEL_REGISTRY` 派生（dev-only 依赖方向）。
- **注册表单点登记**：`CHANNEL_REGISTRY` 行不带 cfg（`pawork models` 八行数据语义恒定）；`is_enabled` 是唯一 `cfg!` 求值点，未知 feature fail-closed；新增通道 = 加一行。
- **协商完备性**：`requested == supported ∪ unsupported`；不支持能力绝不静默丢弃——要么 `Reject` 要么显式 fallback 记录。
- **Anthropic 收口**：prompt cache / thinking / hosted tools 的约束在写 wire 或发 HTTP 之前判定并拒绝（§4.3），不把不满足的请求发给厂商。
- **保留键防覆盖**：`provider_options` 不能覆盖 `model` / `messages` / `stream` 等 wire 保留键（Chat 与 Responses 两处独立拦截）。
- **SSE 有界缓冲与收尾**：单事件缓冲上限 1 MiB（`MAX_BUFFER_BYTES`），防恶意/异常流占满内存；流结束后必须调 `finish()` 取出无终止空行的最后一个事件，否则尾事件丢失。
- **`[DONE]` 哨兵**：Chat Completions 路径用 `is_done`（容忍首尾空白）判定收尾；Responses 路径把 `[DONE]` 与空 data 一并忽略后按自身完成事件收尾。
- **流完成信号必需**：Anthropic 无 `message_stop`、Responses 流 malformed 均按 `StreamInterrupted`/错误处理，不伪造成功；Chat Completions 的 `[DONE]` 正常收尾但缺 finish_reason 时按 `Completed`（协议允许）。
- **错误消息不携带响应正文**：`classify_status` 的 message 固定 `HTTP <code>`，body_snippet 不入 message（上游正文可能回显 token）；`Retry-After` 仅 retryable 错误采纳。
- **计价单轨**：`usage` 模块不含任何计价逻辑，定价统一走 `pricing`（micro-unit 定点），避免双轨口径。
- **模块纪律**：core 模块（registry/pricing/usage/negotiate/reasoning/error）零 `net` 引用，测试强制。

## 6. 依赖关系

- **上游**：仅 `pawork-domain`（canonical 类型、`ModelProvider` / `ProviderEventSink` trait、`ProviderError`）。三方：`reqwest`（HTTP）、`tokio` / `futures`（异步）、`serde(_json)`、`thiserror`、`tracing`、`bytes`、`async-trait`。
- **下游**：`pawork-app` 生产依赖并开启全部九个 feature（`anthropic` + 八通道）；`pawork-engine` 仅 dev-dependency（守护测试名单）。依赖方向与包布局见 [../../design.md](../../design.md) §2。
- **features**（全部为空依赖集、只控制条件编译，互不依赖）：
  - `anthropic`（默认开）：Messages transport 适配器与 `builtin_models`；
  - `chatgpt-oauth` / `xai-oauth`：两条 OAuth 通道适配器；
  - `glm-coding` / `opencode-go` / `qwen-token-plan` / `deepseek`：任一开启即编译共用的 `api_key` 模块；
  - `CHANNEL_REGISTRY` 与 `channels/registry` 不受任何 feature 门控，始终可用（数据恒定八行）。

## 7. 测试与验证资产

默认验证命令：`cargo test -p pawork-providers --offline --lib --tests`（注意：仅编译 `default = ["anthropic"]`，feature 门控的集成测试目标见下表 required-features，需显式 `--features` 才运行）。

| 测试资产 | required-features | 覆盖点 |
| --- | --- | --- |
| `src/**` 内 `#[cfg(test)]` | — | 各模块单测：`module_discipline`（core 不引用 net）、注册表八行顺序与 fail-closed、kimi-code 端点预设、xAI 双认证凭证接受、SSE 边界、保留键忽略、协商 clamp、pricing 定点、错误分类脱敏等 |
| `tests/contract.rs` | —（默认即跑） | OpenAI-compatible 契约全集（见下） |
| `tests/anthropic.rs` | `anthropic` | Messages 契约（见下） |
| `tests/chatgpt.rs` | `chatgpt-oauth` | OAuth 头 / models / Responses 路径接线；malformed Responses 事件即使后随完成事件也报错 |
| `tests/xai.rs` | `xai-oauth` | 模型能力选 Responses/Chat；grok Responses 带 OAuth Bearer 的全链路往返 |
| `tests/responses.rs` | `chatgpt-oauth` | `to_responses_body` 保留 canonical tools、拦截保留键覆盖 |
| `tests/api_key_channels.rs` | 五个 API-key feature（含 kimi-platform） | 五通道默认 id/endpoint 覆盖、未声明 api_key 的 preset 与缺/错凭证 fail-closed、固定凭证头拒绝、Bearer Chat 路径、模型声明 transport 选 Responses 且不按通道分支 |

`tests/contract.rs` 契约点（wiremock 驱动）：

- 文本流、单工具调用、并行工具调用、usage + stop reason；
- 流中取消与预取消（预取消不发请求）、超时归一、长流逐 chunk 重置读超时；
- 429 归一（含 `Retry-After`）、上下文溢出（413）归一；
- malformed 流中断与中断后重连、`[DONE]` 无 finish_reason 按完成、部分 JSON 工具参数跨 chunk 组装、`list_models`。

`tests/anthropic.rs` 契约点：

- 文本/单工具/并行工具流、流中取消与预取消、429 归一、缺 `message_stop` 判 `StreamInterrupted`；
- `list_models` 静态目录不触网；
- prompt cache 与 thinking 按 plan 写 wire（`contract_prompt_cache_and_thinking_are_written`）；
- hosted tools 未声明时 HTTP 前拒绝（`hosted_tools_are_rejected_before_http`）；
- thinking signature 走 protect、不以明文出现在事件流（`contract_thinking_signature_is_protected_not_emitted`）。

dev-dependencies：`wiremock`（HTTP mock）、`proptest`、多线程 tokio。产品级验证边界见 [../verification.md](../verification.md)。

## 8. 注意事项与已知限制

- `ReasoningProtector` 的生产实现（`SwappableReasoningProtector`，含 master key 管理）在 `pawork-app` 的 protected 模块，本包只有内存实现；跨包链路见 [../flows.md](../flows.md)。
- `builtin_models`（Anthropic）静态目录只含 claude-3-5-sonnet / claude-3-5-haiku 两条基线；线上新模型依赖 registry 动态合并或 config 声明。
- `BUILTIN_RATE_VERSION = "2026-08-15"`：内置费率卡有版本口径，厂商调价后需要更新数据表（非代码逻辑）。
- ChatGPT 通道的 `client_version`（当前 `0.147.0`）参与后端 `/models` 目录过滤与 UA 构造，版本过旧会拿到空目录；redirect URI 固定 `localhost:1455`（上游 allow-list 精确匹配，host/port 不可改）。
- `error_table` 是子串匹配的经验规则表，厂商错误文案变化时可能失配（回退到通用分类，不影响正确性只影响精度）。
- 本包不做重试编排；`retry` 模块只负责分类与 `Retry-After` 解析（HTTP-date 用内置最小解析器，仅识别 IMF-fixdate GMT），重试策略由上层决定。
- `OpenAiCompatibleConfig.request_timeout` 是「建连及流式读取无数据超时」的便捷字段（设置时覆盖 `http.timeout`）；配合逐 chunk 重置语义，长流只要持续有数据就不会误杀。
- `channels/mod.rs` 的 re-export 保持合并前 adapters 包的对外路径形状（`ApiKeyChannelConfig` / `ChatGptProvider` 等可从 crate 根直取），消费方无需感知内部目录结构。
- `responses_reasoning` 是 crate 私有模块（无 `pub`），其行为只能经 `responses` 模块间接观察；历史 hint 键拼写兼容属于该模块内部契约。
- 各 OAuth 流程本身（PKCE/Device/refresh）由 `pawork-auth` 承载（见 [auth.md](auth.md)）；本包注册表只提供端点预设数据。任务状态与阶段口径见 [../../../ROADMAP.md](../../../ROADMAP.md)。
