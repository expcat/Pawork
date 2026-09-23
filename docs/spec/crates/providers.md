# pawork-providers

> 首发模型渠道适配器：把 canonical domain 请求（`CanonicalModelRequest`）翻译成各厂商 wire 协议并把响应流映射回 `ProviderStreamEvent`；只依赖 `pawork-domain`（见 [domain.md](domain.md)），被 `pawork-app` 生产依赖、被 `pawork-engine` 仅 dev 依赖。

## 1. 职责与边界

- **职责**：承载四种 transport 形态（OpenAI-compatible Chat Completions、Responses、Anthropic Messages、xAI 按模型能力二选一）、模型目录与能力证据（`ModelRegistry`）、逐 model_id 默认能力表（推理强度 / 图像输入 / 图像生成 / hosted WebSearch，实测口径统一）、能力协商（`CapabilityNegotiator`）、计价与用量归一（pricing/usage）、厂商错误归一（error_table）、reasoning 续传保护接口（`ReasoningProtector`）、八条首发通道的静态注册（`CHANNEL_REGISTRY`，SET-4 新增 Kimi Platform / Kimi Code）与网络层（HTTP/SSE/错误分类）。
- **不做**：凭证的存取与解析（`pawork-auth`，见 [auth.md](auth.md)）；事件持久化；Agent loop 编排；GUI。装配（把 preset + 凭证 + registry 组装成 Provider 实例）由 `pawork-app` 承载。
- **模块纪律**：core 纯逻辑模块（`registry` / `pricing` / `usage` / `negotiate` / `reasoning` / `error`）不得引用 `net` 模块，由 `lib.rs` 内 `module_discipline` 测试护航。
- **engine 不认厂商名**：能力差异一律走 registry/capability 证据；`pawork-engine` 仅以 dev-dependency 引用本包，用 `CHANNEL_REGISTRY` 派生 no-provider-branch 守护测试名单。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~120 | crate 门面：模块声明与 feature 门控 re-export；`is_credential_header`（五个凭证头小写匹配）；`module_discipline` 测试 |
| `src/error.rs` | ~20 | `RegistryError`（`NotFound` / `DuplicateAlias` / `DuplicateModelId`） |
| `src/error_table.rs` | ~160 | `VENDOR_ERROR_RULES` 数据表 + `normalize_vendor_error`：按厂商子串把错误改判为更精确的 `ProviderErrorKind`（如 ChatGPT usage limit、xAI live_search quota） |
| `src/provider.rs` | ~500 | `OpenAiCompatibleConfig` / `OpenAiCompatibleProvider`：Chat Completions transport 的 `ModelProvider` 实现；构造期拒绝 config 头携带凭证头 |
| `src/request.rs` | ~670 | `to_chat_completions_body`：canonical → Chat Completions 请求体；`provider_options` 保留键忽略并 `tracing` 警告 |
| `src/stream.rs` | ~230 | `chunk_to_events` / `is_done` / `ChunkState`：Chat Completions SSE chunk → `ProviderStreamEvent`（文本/工具调用增量、usage、finish_reason） |
| `src/usage.rs` | ~300 | `normalize_usage`（多厂商字段名归一为 `TokenUsage`）、`map_stop_reason`、`UsageAccumulator`（会话级累计） |
| `src/pricing.rs` | ~200 | `ModelPricing` / `estimate_cost`（micro-unit 定点算费，`MILLION` 基数）、`BUILTIN_RATE_CARD`（`"builtin"`）与 `BUILTIN_RATE_VERSION`（`"2026-08-15"`） |
| `src/registry.rs` | ~2.1k | `ModelRegistry`（目录 + 别名 + 三源能力证据 + 动态发现合并）、`CatalogEntry`、`CapabilityEvidence` / `CapabilitySource`、`merge_capabilities`、`ProviderProbe` / `ProbeError` / `ProviderCapabilitySource`、`caps` 构造 helper、四张逐 model_id 默认能力表（`default_supported_efforts` / `default_image_input` / `default_image_output` / `default_hosted_web_search`）与对应 `apply_default_*` 回填 |
| `src/negotiate.rs` | ~730 | `CapabilityNegotiator::negotiate`（纯函数协商）与 `clamp_reasoning_to_thinking` |
| `src/reasoning.rs` | ~100 | `ReasoningProtector` trait（protect/recover 不透明 payload）与 `ReasoningProtectError`（`Unavailable` / `Corrupted` 判别） |
| `src/memory_protector.rs` | ~110 | `InMemoryReasoningProtector`：HashMap 存不透明字节，测试/内存场景用 |
| `src/responses.rs` | ~1140 | Responses transport 共享件：`ResponsesTransport(Config)` / `ResponsesWireOptions` / `to_responses_body` / `ResponsesStreamAssembler` / `ResponsesAssemblyEvent` / `ResponsesFinalState`；保留键防覆盖；凭证头拒绝 |
| `src/responses_reasoning.rs` | ~250 | crate 私有：Responses reasoning item → canonical `ReasoningItem`（提取 `encrypted_content` 交 protector；容忍历史 hint 键拼写） |
| `src/net/mod.rs` | ~10 | re-export `http` / `sse` / `retry` |
| `src/net/http.rs` | ~400 | `HttpClient` / `HttpClientConfig`（builder：timeout/proxy/user_agent/自定义头/禁系统代理）、`is_local_target` / `loopback_aware_proxy`（本地目标绕过代理）；Debug 输出对凭证头脱敏 |
| `src/net/sse.rs` | ~450 | 增量 `SseParser`（feed/finish）、`SseEvent` / `SseParseError`、`MAX_BUFFER_BYTES`（1 MiB 缓冲上限，UTF-8 安全跨 chunk） |
| `src/net/retry.rs` | ~220 | `classify_status` / `classify_request_error`（HTTP 状态与 reqwest 错误 → `ProviderError`，解析 `Retry-After`，消息脱敏）、`parse_retry_after` |
| `src/channels/mod.rs` | ~60 | 八通道 feature 门控的模块声明与 re-export |
| `src/channels/registry.rs` | ~360 | `CHANNEL_REGISTRY`（八行静态 preset）、`ChannelPreset`（含 `display_name` 与 `auth_methods` 数据字段，SET-4 起不再按 kind 派生）/ `ChannelKind`、`OAuthPreset(Data)` / `OAuthFlow(Data)`、`channel_preset`、`is_enabled`（唯一 cfg 求值点） |
| `src/channels/api_key.rs` | ~610 | `ApiKeyChannelConfig` / `ApiKeyChannelProvider`：API-key 通道共用适配器（五行，含 kimi-platform；xAI / Kimi Code 双认证亦复用 `verify_api_key`）；默认 Chat Completions，官方表 / 家族回退决定 Responses 或 Messages（list_models 解析 transport 后回填默认能力表：efforts / image_output / hosted WebSearch，后者仅 Responses 生效）；未登记聊天 ID 不丢弃；`verify_api_key` 用候选 key 发已认证 GET 做写前验证（Go `/usage`，其余 `/models`，不持久化） |
| `src/channels/chatgpt.rs` | ~280 | `ChatGptConfig` / `ChatGptProvider`：ChatGPT OAuth 通道（Responses transport、`chatgpt-account-id` / `originator` 头、`client_version` 校验、`DEFAULT_BASE_URL`） |
| `src/channels/xai.rs` | ~600 | `XaiConfig` / `XaiProvider`：xAI Grok OAuth 通道，按模型 capability 声明选 Responses 或 Chat Completions；选择目录只认远端：OAuth 为 CLI proxy `/models`，API key 为 `/language-models`（output_modalities 含 "text" 才入目录），不预填静态 grok；`xai_builtin_models` 仅给已知 id（`grok-4` / `grok-4-fast` / `grok-3` / `grok-2`）补 transport / 能力，未知 id 保守默认（text + Chat Completions + 窗口 0，订阅远端可指定 Chat/Responses，远端字段可覆盖窗口与图像）；`DEFAULT_BASE_URL` |
| `src/channels/kimi.rs` | ~420 | `KimiCodeConfig` / `KimiCodeProvider`：Kimi Code 通道（SET-4 A2），接受 OAuth bearer 或 Coding Plan API key、只走 Chat Completions（`https://api.kimi.com/coding/v1`）；SET-5 起 `list_models` 走远端 `GET {base}/models`（OpenAI 风格 `data[]`，已知 id 沿用 `builtin_models` 元数据，未知 id 给保守默认；`builtin_models` 仅作元数据来源与静态兜底） |
| `src/channels/anthropic/mod.rs` | ~20 | re-export 与 `ANTHROPIC_VERSION`（`anthropic-version` 头值） |
| `src/channels/anthropic/provider.rs` | ~1.1k | `AnthropicProvider(Config)`：Messages transport；`prepare_request` 能力收口（§4.3）；`builtin_models` 静态目录（claude-3-5-sonnet / haiku） |
| `src/channels/anthropic/request.rs` | ~790 | `to_messages_body(_with_plan)` / `MessagesWirePlan`：system 提升、`tool_use` 块、`thinking` 与 `cache_control` 按 plan 写 wire |
| `src/channels/anthropic/stream.rs` | ~690 | `parse_event` / `event_to_events` / `AnthropicStreamState` / `StreamOutput`：Anthropic SSE 事件 → canonical 事件；thinking signature 以 `PendingSignature` 输出待 protect |

共 28 个 `.rs` 文件，约 12.7k 行。

## 3. 对外 API 面

### 3.1 Provider adapters（`pawork_domain::ModelProvider` 实现）

trait 面为 `id()` / `list_models(credential)` / `stream(&request, sink, cancel)`（请求按引用借用，R-07：engine 工具循环每轮免全历史深拷贝）；所有实现把事件逐个 `sink.emit(ProviderStreamEvent)`，`stream` 返回 `ModelResponseSummary`（`stop_reason` / `usage` / `response_id` / `provider_metadata`），错误统一 `ProviderError`。`CancellationToken` 在发请求前与流式循环内多点检查：预取消不发 HTTP，流中取消报 `Cancelled`。

- `OpenAiCompatibleProvider::new(config, credential)`：`OpenAiCompatibleConfig::new(base_url)` 默认 `provider_id = "openai-compatible"`，可 `with_provider_id`。构造期若 config 自定义头含凭证头则拒绝（凭证只能经 `ResolvedCredential` 注入为 `Authorization: Bearer`）。
- `AnthropicProvider`（feature `anthropic`，默认开启）：认证头 `x-api-key` + `anthropic-version`；可 `with_registry(Arc<ModelRegistry>)` 注入能力证据、`with_reasoning_protector` 注入续传保护。
- `ChatGptProvider`（feature `chatgpt-oauth`）：内部复用 `ResponsesTransport`；OAuth Bearer + `chatgpt-account-id`（构造入参或从 id_token JWT claim 提取）+ `originator: codex_cli_rs` 头；`client_version` 字符集校验，`/models?client_version=` 过滤目录。SEARCH-1 / VISION-1：codex 后端模型统一声明 hosted `WebSearch`（Responses `web_search` 内置工具对全部模型可用，wire 选项 `hosted_web_search = true`）与 `image_input`（Responses `input_image` content part）。ADR-063：目录 `supported_reasoning_levels`（字符串或 `{effort}` 对象，minimal→low、xhigh→x_high）解析归一为 canonical `supported_efforts`，缺键或全部不可识别为 None（未知，不约束）。
- `XaiProvider`（feature `xai-oauth`）：OAuth Bearer 或 API key（SET-4 A3 双认证，Bearer 用法相同）；按模型 capability 的 `transport` 声明路由 Responses / Chat Completions。订阅 OAuth 使用 `https://cli-chat-proxy.grok.com/v1/models`（`data[]`），目录与推理均带 `X-XAI-Token-Auth: xai-grok-cli` 与 `x-grok-client-version`（2026-09-23 起订阅代理强制校验，缺失即 426，需 ≥0.1.202；当前对齐官方 lockstep 1.0.41），推理另带当前模型的 `x-grok-model-override`；API key 仍使用 `https://api.x.ai/v1/language-models`（`models[]`，仅保留 `output_modalities` 含 `"text"` 的模型）。默认 API base 在 OAuth 构造期切到订阅 base，自定义 base 保留。订阅目录按 `model` / `modelId` / `id` 取实际推理 ID，读取 `name`、`contextWindow` / `context_window` / `_meta.contextWindow` / `_meta.totalContextTokens`、`maxCompletionTokens` / `max_completion_tokens`，`apiBackend` / `api_backend` 指定 Chat 或 Responses（缺省 Chat，其余协议过滤），成功解析后保存协议映射供流式请求使用；已知 id 沿用 builtin 元数据（display_name / 窗口 / transport），未知 id 只给保守默认（text 声明 + Chat Completions 基线 + 窗口 0）；无凭证（构造即失败）、请求失败或响应缺对应目录数组一律 `Err`。Host 选择目录不预填静态 grok：探测成功才列出远端结果，失败为 unavailable，不落 fixed_fallback。SEARCH-1 审查修正（2026-09-15）：Responses transport 模型（builtin 与远端目录同规则，2026-09-22 起）声明 hosted `WebSearch`，经 `hosted_web_search = true` 写 `tools: [{"type":"web_search"}]`；Chat 与未知模型不声明 hosted search，并在发 HTTP 前拒绝。已删除旧 `ChatSearchWire` / `chat_search` / `search_parameters` 实现；远端别名落到 Chat 时也清除继承的 hosted 标签。依据：[xAI 当前协议对照](https://docs.x.ai/developers/model-capabilities/text/comparison)与 [Web Search](https://docs.x.ai/developers/tools/web-search)。
- `KimiCodeProvider`（feature `kimi-code`）：OAuth Bearer 或 Coding Plan API key（Bearer 用法相同）；固定 Chat Completions。SET-5 起 `list_models` 请求 `GET {base}/models`（与官方 kimi-cli 同端点，证据见 MoonshotAI/kimi-cli 源码与 repo issue 中的真实请求实例）：OpenAI 风格 `data[].id` 解析，已知 id 沿用 builtin 元数据，未知 id 只给保守默认；形状不符（缺 `data` 数组）、请求失败或无凭证一律 `Err`，禁止猜测兼容。VISION-1：builtin 条目声明 `image_input`（platform.kimi.ai 官方 models 文档，2026-09-15）；Kimi `$web_search` 需客户端回显 arguments 的两段流程，本通道未接线，**不声明** WebSearch（fail-closed）。 MM-1：发 HTTP 前拒绝外部图片 URL（官方视觉指南只收 base64 或 `ms://` 文件 ID）；`kimi-platform` 的 Chat Completions 路径使用同一拒绝。
- `ApiKeyChannelProvider`（任一 API-key feature）：以 `&'static ChannelPreset` 构造，构造期 fail-closed——preset 必须声明 api_key 认证方法（`auth_methods` 数据字段）且 `is_enabled`，凭证必须存在且为 API key 形态，config 固定头不得含凭证头。
- `verify_api_key(config, candidate_key)`（async，任一 API-key feature）：SET-2 写前验证入口——用候选 key 构造一次性 adapter 校验凭证边界；Go 请求 `/usage` 验证 key/订阅，其余请求严格 `/models`，只返回 `Ok(())` / `ProviderError`；key 只在内存短暂停留、不落任何后端，供宿主 `auth_set_api_key` 在 `store_default_api_key` 之前校验。

Grok 订阅端点与协议依据：[官方 CLI 目录实现](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/remote/model_source/oai.rs)、[目录解析](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/remote/client.rs)、[订阅请求头](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/README.md#using-authjson-for-api-access)。403 保留真实 Authorization 错误，不用静态列表掩盖失败。

四种 transport 形态对照：

| transport | 请求组装 | 流解析 | 认证头 | 使用方 |
| --- | --- | --- | --- | --- |
| Chat Completions | `to_chat_completions_body` | `SseParser` + `chunk_to_events` | `Authorization: Bearer` | OpenAiCompatible、API-key 通道（五行）、xAI Chat 模型、Kimi Code |
| Responses | `to_responses_body` | `SseParser` + `ResponsesStreamAssembler` | Bearer（ChatGPT 另加 account/originator 头） | ChatGPT、xAI Responses 模型、API-key 通道逐模型声明时 |
| Anthropic Messages | `to_messages_body_with_plan` | `SseParser` + `parse_event` | `x-api-key` + `anthropic-version` | AnthropicProvider |
| 模型能力路由 | —（复用上两行） | — | — | XaiProvider / ApiKeyChannelProvider 按 `ModelCapabilities::transport` 选择 |

### 3.2 通道注册表（channels/registry）

`CHANNEL_REGISTRY: &[ChannelPreset]` 八行（顺序即 `pawork models` / `auth list` 展示顺序）；行本身**不带 cfg**，feature 是数据字段，`is_enabled(preset)` 是唯一的 `cfg!` 求值点（未知 feature 名返回 false，fail-closed）。`channel_preset(id)` 按 id 查行；`ChannelPreset::oauth_preset()` 把 const 镜像 `OAuthPresetData` 转运行期 `OAuthPreset { client_id, token_url, scopes, flow }`（与 config `[oauth.<id>]` 覆盖共用同一形状）。`ChannelPreset` 另携带 `display_name`（品牌展示名）与 `auth_methods` 数据字段（SET-4 起不再按 kind 派生：纯 API-key 行为 `["api_key"]`、纯 OAuth 行为 `["oauth"]`、xAI / Kimi Code 双认证为 `["oauth","api_key"]`），SET-2 GUI Settings 的通道 descriptor 与认证方式列表直接由此派生，宿主 / Desktop 不自建品牌表。

| provider_id | 凭证形态（ChannelKind） | 默认协议 / endpoint | feature | OAuth 流 |
| --- | --- | --- | --- | --- |
| `chatgpt` | ChatGptOAuth（Bearer + account 头） | Responses / `https://chatgpt.com/backend-api/codex` | `chatgpt-oauth` | PKCE（auth.openai.com；redirect 固定 `http://localhost:1455/auth/callback`；scopes 含 `api.connectors.read/invoke`；附加 `codex_cli_simplified_flow` 等参数） |
| `xai` | XaiOAuth（OAuth Bearer 或 API key，SET-4 双认证） | 按模型能力选 Responses/Chat；OAuth `https://cli-chat-proxy.grok.com/v1`，API key `https://api.x.ai/v1` | `xai-oauth` | Device Flow（auth.x.ai；scopes 含 `grok-cli:access`、`api:access`） |
| `glm-coding` | ApiKey | Chat Completions / `https://api.z.ai/api/coding/paas/v4` | `glm-coding` | — |
| `opencode-go` | ApiKey | Chat Completions / `https://opencode.ai/zen/go/v1` | `opencode-go` | — |
| `qwen-token-plan` | ApiKey | Chat Completions / `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | `qwen-token-plan` | — |
| `deepseek` | ApiKey | Chat Completions / `https://api.deepseek.com` | `deepseek` | — |
| `kimi-platform` | ApiKey | Chat Completions / `https://api.moonshot.ai/v1` | `kimi-platform` | — |
| `kimi-code` | KimiOAuth（OAuth Bearer 或 Coding Plan API key） | Chat Completions / `https://api.kimi.com/coding/v1` | `kimi-code` | Device Flow（auth.kimi.com；client_id `17e5f671-d194-4dfb-9706-5516cb48c098`；scope `kimi-code`；端点与 MoonshotAI/kimi-cli 官方源一致）。`auth_methods` 为 `["oauth","api_key"]`，与 xAI 同序 |

补充语义：

- glm-coding 的远端目录端点为 `GET https://api.z.ai/api/coding/paas/v4/models`（OpenAI 风格，走通用 API-key 通道实现；Z.AI Coding Plan 官方文档证据：<https://docs.z.ai/devpack/tool/others>）。
- OAuth 行的公开 client_id / 端点预置在注册表源码中（各厂商公开 client 参数，非 Secret）；`OAuthPresetData` 是 static 初始化友好的 `&'static str` 镜像，运行期 `to_preset()` 转 String 形态后与 config `[oauth.<id>]` 覆盖走同一形状。
- `ChannelKind` 四变体即四种装配形态：`ApiKey`（五行通道复用 OpenAI-compatible transport，可逐模型切 Responses）、`ChatGptOAuth`（固定 Responses）、`XaiOAuth`（按模型 capability 选 Chat/Responses；SET-4 起凭证可为 OAuth 或 API key）、`KimiOAuth`（固定 Chat Completions；Coding Plan API key 与 OAuth 双认证）。
- feature `anthropic`（默认开）承载 Messages transport 适配器，不属于 CHANNEL_REGISTRY 八行——它是 transport 基线而非首发通道行。

### 3.3 模型目录与能力证据（registry）

- `ModelRegistry`：`empty()` / `builtin()` 构造；`register` / `try_register`（`RegistryError::DuplicateModelId` / `DuplicateAlias` 拒绝重复登记，`resolve` 未命中对应 `NotFound` 语义）；`extend_with`（批量合并）；`merge_provider_models` / `merge_provider_source`（把 Provider `list_models` 发现结果并入，静态已有时逐字段交集收窄）；`resolve(id_or_alias)`（模型 id 与别名同一命名空间查找）；`list` / `filter(required)`（按能力子集筛选）；`validate_context(id, input_tokens)` / `estimate_cost(id, usage)`（经 `CatalogEntry` 的 pricing）。`CatalogEntry::to_definition()` 转 domain `ModelDefinition`。
- 能力三源：`CapabilitySource::{Static, Probe, Override}`；`CapabilityEvidence { static_declared, probe_declared, override_declared }`，`merged()` 对已出现的来源逐字段取交集（缺失来源不约束）。运行期可 `set_override` / `remove_override`（override 只能收窄）、`record_probe` / `clear_probe`（探测结果按 provider 记录）。`capability_evidence(model)` / `capability_snapshot()` 导出证据。
- `ProviderCapabilitySource` trait / `ProviderProbe` / `ProbeError`：动态探测抽象。`caps(...)` 是测试与静态目录用的 `ModelCapabilities` 构造 helper。

UI-6a / ADR-058：`ApiKeyChannelConfig::transport_for` 与 adapter 共用协议解析，Host 静态 / 配置回退同样过滤不可运行模型并同步 transport，显式覆盖仍优先。通用、ChatGPT、Kimi、xAI 目录使用严格形状与非空 ID 校验，合法空数组仍成功。通用未知窗口/输出为 0、未知工具能力 false；已知 ID 仅从同 provider 静态条目补证据，远端实际字段优先。Kimi 消费 display_name/context_length/supports_reasoning/supports_image_in；xAI 消费输入模态和窗口，aliases 仅辅助找静态证据；ChatGPT 空/null/none-only reasoning levels 不算思考。通用 `has_more=true` 显式拒绝，尚未实现翻页。

VISION-1 / SEARCH-1（2026-09-15 八家官方调研）：通用远端目录消费 `supports_image_in` 显式布尔值，缺失时再读 `input_modalities`；纯文本模态或显式 false 会撤销静态视觉能力，两者均缺失才保留已有静态证据。通用 Chat / API-key Responses 没有 hosted search wire，不因远端 `supports_web_search` 而宣称可用；builtin 静态目录增至 8 条——新增 `glm-5.3`（text-only）/ `glm-5.3-flash`（多模态）@glm-coding、`deepseek-flash`（视觉 + thinking，1M / 384K）@deepseek，`qwen3.8-max` 标 `image_input`；能力按型号而非按 provider 声明（GLM-5.3 text / 5.3-Flash 多模态；deepseek-flash 视觉 / v4-pro 无视觉）。

VISION-2（2026-09-22 官方文档调研）：新增 `default_image_input` 逐 model_id 默认表（registry.rs，口径与 ADR-063 强度表相同：实测 > 官方目录 > 官方文档，竞品仅作线索），远端目录（通用 / Kimi / xAI）未声明模态时按表回填 true，显式 false 与纯文本模态仍覆盖一切静态声明。登记为支持图像：glm-5.3-flash、qwen3.8/3.7/3.6 系、minimax-m3、mimo-v2.5、deepseek-flash 系、kimi k3/k2.6/k2.7-code 系（含 coding 计划 id）、grok-4.5+；登记为官方证实 text-only：glm-5.3/5.2、deepseek-v4-pro、hy3；omen-alpha 等无官方证据保持未知不声明。Kimi 外部图片 URL 在 `reject_kimi_external_image_urls` 发 HTTP 前拒绝（`ms://` 与 base64 放行）。同期搜索调研结论：Chat 通道原生搜索（z.ai `web_search` 工具、百炼 `enable_search`、MiniMax `web_search`、Kimi `$web_search` 及 coding 端点 REST `/search`）均未接线，保持不声明 WebSearch；DeepSeek 官方明确 Responses built-in tools 一律 Ignored（无 hosted 搜索）；xAI Responses API 有 API 级 `web_search` 且订阅 CLI proxy 经 `/responses` 调用（grok-build 源码佐证），故 xAI Responses transport 模型（含远端目录）均声明 hosted WebSearch，Chat 模型不声明。

CAP-PROBE（2026-09-23 五通道端点实测，glm-coding / opencode-go / qwen-token-plan / kimi-code / xai 订阅）：经 provider 适配器对当前可访问模型逐一发送 64x64 纯红 PNG 图片请求与 hosted `web_search` 请求（opencode-go 必须带 `x-opencode-session`，否则 400 MissingSessionID），结论按「实测 > 目录 > 文档」回写默认表——
- `default_image_input` 补登 omen-alpha / mimo-v2.6-flash / mimo-v2.6-pro / deepseek-v4-flash-vision-exp（opencode-go）与 qwen `auto`；补登 text-only：glm-4.5 / 4.5-air / 4.6 / 4.7 / 5 / 5-turbo / 5.1、muse-spark 1.2/1.3、longcat-2.0；实测翻转 deepseek-v4-flash（legacy id 在 go 对图片 400、文本 200）与 qwen3.7-max（token-plan 对图片 400、文本 200）。glm-5.3-flashx（429）、gpt-5.6-luna（403）、hy3 / hy4-preview / mimo-v2.5-pro（推理 404）未测，保持未知。
- 新增 `default_image_output`：qwen token-plan compatible-mode 实测 wan2.7-image / wan2.7-image-pro 文生图（content 数组入、`{"type":"image","image":<url>}` 出、image_count 用量）；两款模型仍被 `non_text_model` 过滤在聊天目录外，本表作为目录级声明保留给图像生成接线。其余四通道目录无图像生成模型。
- 新增 `default_hosted_web_search`：xai 订阅（grok-4.5/4.6/4.7/4.7-build-fast）与 opencode-go Responses（grok-4.6/4.7）实测透传 `{"type":"web_search"}` 并返回 web_search_call 事件与 url_citation；`apply_default_hosted_web_search` 仅在 Responses 传输插标签，Chat 通道仍不声明（fail-closed）。
- 同批产品修复：xAI 订阅代理 2026-09 起强制 `x-grok-client-version`（缺失即 426 "CLI version (none) is outdated"，需 ≥0.1.202），适配器订阅路径统一附带该头（当前对齐官方 lockstep 版本 1.0.41，目录与推理共用）；opencode-go Responses 的 `hosted_web_search` wire 由恒 false 改为按通道放开（见 §5 Responses transport）。

`ApiKeyChannelConfig` 初始化 Go/Qwen 官方逐模型 transport 表；显式 `with_model_transport` 覆盖单项。混合通道的目录过滤与 `stream` 共用 `transport_for`：官方表命中用表内协议；未命中先过共用 `non_text_model` 谓词（图片 / 音频 / TTS / realtime 等非文本 ID 为 `None`，Go 与 Qwen Token Plan 同规则），再按官方 endpoint 家族回退（Go：`grok-*`/`gpt-*`/`muse-spark-*` → Responses，`qwen*`/`minimax-*` → Messages，其余 Chat Completions）。只保留 ChatCompletions/Responses 进入可运行目录；Messages-only 与非文本 ID 的直接请求也在 HTTP 前拒绝。远端成功返回的新聊天 ID 不得因缺表被丢弃。其它 Chat 通道仍采用兼容文本基线。来源与协议/认证边界见 [ADR-058](../settings.md#adr-058ui-6a-目录权威与凭证验证2026-09-08)。

### 3.4 能力协商（negotiate）

`CapabilityNegotiator::negotiate(&CapabilityEvidence, &CapabilityRequirements) -> ResolvedCapabilities`：无状态纯函数，不触网、不读 Provider 名、不读 wall-clock。

- 输入：证据快照（三源）× 请求要求（`transport_pref` / `required_tools` / `reasoning` / `citations` / `image_input`——VISION-1 起，请求消息含图片内容时置位，模型未声明图片输入则 `Reject`）。
- 输出 `ResolvedCapabilities` 字段：`chosen_transport`、`requested` / `supported` / `unsupported` 三集合（保证 `requested == supported ∪ unsupported`）、`fallback` map（键 → `CapabilityFallback`）。
- `CapabilityFallback` 变体语义：`Reject(原因)`（adapter 必须在发 HTTP 前拒绝）、`LegacyTransport`（请求现代 transport 但模型只有 Chat Completions 基线，降级记录）、`ClampedEffort`（reasoning `XHigh`/`Max` 在模型不支持细粒度 effort 时 clamp 为 `High`）。
- transport 选择顺序：请求偏好 ∈ 模型声明 → 采用偏好；否则用模型声明的 transport；模型只有基线时退 ChatCompletions（必要时记 `LegacyTransport`）。
- reasoning：显式 `ReasoningConfig` 优先于旧 `ThinkingConfig.level`；请求 reasoning 但模型 `thinking == false` 时整项进 `unsupported` + `Reject`。
- `clamp_reasoning_to_thinking(reasoning, thinking)` 供 adapter 复用（Anthropic 把 effort 翻成 thinking budget），避免形成第二套 clamp 双轨。
- `capability_gate(&CapabilityEvidence, &CanonicalModelRequest)`（VISION-1 / SEARCH-1）：发 HTTP 前的统一前置闸门——从请求派生 `required_tools`（hosted + extension）与 `image_input`，走同一 `negotiate`，任一 `Reject` 即 `InvalidRequest`，不触网。Anthropic 经 `prepare_request` 完整 negotiate 收口；其余通道由装配 / Run 服务在派发前统一调用（按证据判定，不按厂商分支）。无任何证据（未知模型）按空证据处理：纯文本放行，带图片 / hosted 工具 fail-closed。

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

- `HttpClientConfig::builder()`：`timeout` / `no_timeout` / `proxy` / `user_agent` / `header` / `disable_system_proxy`；`HttpClient::new(config)`（代理不合法等报 `ProviderError`）。凭证头在 Debug 输出中显示为脱敏值；proxy URL 的 Debug / 解析错误只保留 scheme/host/port。`is_local_target(host)` / `loopback_aware_proxy(proxy)`：loopback 目标绕过代理。
- `SseParser::new()` / `feed(&[u8]) -> Vec<Result<SseEvent, SseParseError>>` / `finish()`：增量解析，容忍 CRLF/LF、注释行、跨 chunk UTF-8 截断；缓冲超 `MAX_BUFFER_BYTES`（1 MiB）报错。
- `classify_status(status, retry_after, body_snippet)`：状态码 → `ProviderErrorKind` 固定映射——401 `Authentication`、403 `Authorization`、404 `ModelNotFound`、408 `Timeout`、413 `ContextTooLarge`、429 `RateLimited`、400 `InvalidRequest`、451 `ContentFiltered`、402 `QuotaExceeded`、500/502/503/504 `ProviderUnavailable`，其余 4xx/5xx 按类别兜底。`message` 固定为 `HTTP <code>`——`body_snippet` **不写入** message（上游正文可能回显 token）；`Retry-After`（秒数或 RFC 7231 IMF-fixdate，`parse_retry_after`）仅对 retryable 类采纳为 `retry_after_ms`。
- `classify_request_error(reqwest::Error)`：timeout → `Timeout`、connect → `Network`、body/decode → `StreamInterrupted`、request 构造 → `InvalidRequest`、其余 → `Network`；消息只保留错误类别与 origin，不含 userinfo/path/query。

### 3.8 wire 纯函数与错误表

- Chat Completions：`to_chat_completions_body`（`provider_options` 中保留键如 `model`/`messages`/`stream` 被忽略并告警）；`chunk_to_events(data, &mut ChunkState)` / `is_done`。
- Responses：`to_responses_body(request, reasoning_inputs, ResponsesWireOptions)`（同样拦截保留键覆盖）；`ResponsesStreamAssembler::feed/finish` 产出 `ResponsesAssemblyEvent` 与 `ResponsesFinalState`。
- Anthropic：`to_messages_body` / `to_messages_body_with_plan(request, &MessagesWirePlan)`；`parse_event(data, &mut AnthropicStreamState) -> Vec<StreamOutput>`（`Event` / `MappingError` / `ReasoningError` / `PendingSignature`）；`event_to_events` 兼容入口；`ANTHROPIC_VERSION` 常量。
- `normalize_vendor_error(vendor, error)`：按 `VENDOR_ERROR_RULES` 细化——`VendorErrorRule { vendor, needles, kind, retryable, detail, diagnostic_key }`，消息小写后须命中该厂商规则的**全部** needles 才改判 kind/retryable 并写入 diagnostics；未命中原样返回。表内只登记本期渠道的稳定标记：chatgpt（usage limit → `QuotaExceeded`、account deactivated → `Authorization`）、xai（live_search quota → `RateLimited`、collection not_ready → `ProviderUnavailable`、insufficient_quota → `QuotaExceeded`）、qwen-token-plan（数据检查 → `ContentFiltered`、throttling → `RateLimited`、quota_exhausted → `QuotaExceeded`）、glm-coding（错误码 1113 → `QuotaExceeded`、1301/敏感 → `ContentFiltered`）。

## 4. 核心行为与数据流

UI-6b G2：`fetch_go_usage(config, &ResolvedCredential, cancel)` 单次认证 GET 返回 `GoUsage{rolling,weekly,monthly: Result<GoUsageWindow, ProviderError>}`；窗口包含 `used_percent` 与 `resets_at: Timestamp`。严格校验整数百分比/状态及 `YYYY-MM-DDTHH:mm:ss.sssZ` 日历；单窗失败独立，顶层畸形整体失败。`verify_api_key` Go 分支复用并要求三窗均成功；GET 与正文读取共用总期限（request_timeout → http.timeout → 60s），均可取消，错误不回显上游字段；Host 额度读取设为 10s。无新增依赖；必要回归位于既有 API-key 测试文件。

### 4.1 一次 Chat Completions stream 请求（OpenAiCompatible / ApiKeyChannel）

1. 调用方（`pawork-app` 装配层）把 `ResolvedCredential` 注入 Provider 构造；构造期校验固定头无凭证头（含凭证头直接构造失败）。
2. `stream(&request, sink, cancel)`：先查 `cancel`——预取消不发任何 HTTP 请求。
3. `to_chat_completions_body` 生成请求体：messages / tools / response_format 按 canonical 语义翻译，`provider_options` 白名单透传（保留键忽略并 `tracing` 告警）。
4. `HttpClient` POST `{base_url}/chat/completions`，凭证经 `Authorization: Bearer` 头注入；请求阶段错误走 `classify_request_error`，非 2xx 走 `classify_status`，再经 `normalize_vendor_error` 按渠道细化。
5. 响应字节流喂 `SseParser::feed`；每个 SSE event 的 data 经 `chunk_to_events`（`ChunkState` 跨 chunk 组装工具调用 id/name/参数增量）映射为 `ProviderStreamEvent`，逐个 `sink.emit`。
6. usage chunk 经 `normalize_usage` 发 `UsageUpdated`；`finish_reason` 经 `map_stop_reason` 发 `ResponseCompleted`；`[DONE]` 到达而无 finish_reason 时按 `Completed` 收尾。
7. 每收到一个 chunk 重置读超时（长流不误杀）；流中断（未见完成信号）报 `StreamInterrupted`；取消点贯穿字节循环与事件循环。
8. `ApiKeyChannelProvider` 额外一步：若模型 capability 显式声明 Responses transport，则路由到共享 `ResponsesTransport`——按能力数据路由，不按通道名分支。

SEARCH-1 hosted 工具门控（`apply_hosted_tool_wire`，纯函数）：Chat Completions 未声明 hosted / extension 工具 wire，携带此类工具的请求一律在发 HTTP 前 `InvalidRequest`，不静默丢弃。xAI 的搜索走 Responses；其它通道尚未接线的原生搜索保持拒绝。

MM-1 图像限制（`reject_channel_image_limits`）：Chat Completions 与 Responses 在组请求体前按已登记的 `(provider, model)` 校验。glm-5.3-flash 只收 png/jpeg 且 base64 解码后不超过 5MB；百炼 qwen3.8/3.7/3.6 收 png/jpeg/webp/bmp/gif、不超过 20MB；DeepSeek flash 收 png/jpeg/gif/webp、不超过 32MiB；MiniMax M3 只收 jpeg/png/webp（官方未给可执行字节上限，不另造）；xAI `grok-*` 收 png/jpeg/gif/webp、不超过 20MB。URL 只校验媒体类型。未登记的通道与模型不经过本表。

ADR-057：`ApiKeyChannelProvider` 仅为 `opencode-go` 启用内部会话头映射，Chat 与下述 Responses 共用 `opencode_session_header` 校验；每次从 `CanonicalModelRequest.session_id` 取 `x-opencode-session`，None 不发送、其他通道不发送。非法 header 值在发网前返回固定脱敏 `InvalidRequest`；不写入 body、trace_id 或持久配置。

### 4.2 Responses transport（ChatGPT / xAI Responses 模型）

1. `requirements_from_request` → `CapabilityNegotiator::negotiate`；被拒能力（`Reject`）在发 HTTP 前变成 `InvalidRequest` 错误。
2. 请求中的历史 `ReasoningItem` 经 `ReasoningProtector::recover` 还原成 reasoning input（`encrypted_content`），交给 `to_responses_body(request, reasoning_inputs, wire)`。
3. `ResponsesWireOptions` 控制 wire 细节：`store`（ChatGPT OAuth 默认 `Some(false)`，xAI 与 API-key 通道不设）、`include_encrypted_reasoning`（请求返回加密 reasoning continuation）与 SEARCH-1 的 `hosted_web_search`（`true` 时 canonical hosted `WebSearch` 写成 `tools: [{"type":"web_search"}]` 内置工具；`false` 时 hosted tools 在流入口发 HTTP 前拒绝——API-key 通道默认 `false`，opencode-go 自 2026-09-23 实测 grok 家族透传后按通道放开 `true`，未登记 `default_hosted_web_search` 的模型仍由协商层 fail-closed）；保留键防覆盖同样生效。
4. 认证头：ChatGPT 为 Bearer + `chatgpt-account-id`（构造入参或 id_token JWT claim 提取）+ `originator: codex_cli_rs`；xAI 仅 Bearer。
5. SSE → `ResponsesStreamAssembler::feed`：文本增量、工具调用、reasoning item（`encrypted_content` 经 `protect` 换 `protected_blob_ref` 后发 `ReasoningItem` 事件）、usage 与完成信号。SEARCH-1：`web_search_call` item 的 added 映射为 `ServerTool::Started`；done 仅 `status=completed` 映射 Completed，其余状态映射 Failed，同调用终态去重；`response.output_text.annotation.added` 的 `url_citation` 归一为 `CitationAdded`（`source_kind = WebSearch`，归到最近一次 web_search 调用，缺省时退回事件 `item_id`），其余 annotation 类型忽略（forward-compat）。
6. malformed 事件立即报错——即便其后跟着完成事件也不救回（防止半损坏流被误判成功）；`finish()` 输出 `ResponsesFinalState` 收尾校验。

### 4.3 Anthropic Messages 能力收口（`prepare_request`，写 wire 或发 HTTP 前拒绝）

1. `capability_evidence(model)`：优先注入的 `ModelRegistry`，退回 `builtin_models` 静态声明。
2. `negotiate` 后 `first_reject`：任何 `Reject`（如 hosted tools 未声明、citations 不支持）→ `InvalidRequest`，**不发 HTTP**。
3. prompt cache 三态：`Disabled` 不写；`Automatic` 依能力；`Required` 且能力未声明 → 拒绝。
4. thinking 预检：`reasoning` 优先经 `clamp_reasoning_to_thinking` 翻译；budget < `MIN_THINKING_BUDGET_TOKENS`（1024）、`temperature != 1.0`、`max_output_tokens <= budget` 均拒绝。
5. 历史 thinking 块经 `ReasoningProtector::recover` 还原签名（`resolve_thinking_blocks`）。
6. 组装 `MessagesWirePlan { write_cache, thinking_budget, resolved_thinking_blocks }` → `to_messages_body_with_plan`；`Required` 但 body 无任何 `cache_control` 断点 → 拒绝。
7. 发 HTTP（`x-api-key` + `anthropic-version`）；SSE → `parse_event`：thinking signature 以 `PendingSignature` 输出，经 `protect` 变 `ReasoningItem`（`continuation_metadata` 带 anthropic model hint）；无 `message_stop` 即 `StreamInterrupted`。
8. 服务端工具的 `server_tool_use` 结束只代表参数块结束；收到对应 result 块并结束后才发 Completed。`web_search_tool_result_error` / `is_error` 发 Failed（保留 error_code），同调用只发一个终态；引用可在终态后继续到达。依据：[Anthropic Web Search](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool)。

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

### Computer use 工具图片续接（2026-09-17）

canonical `ToolResultContent.content` 中 Image 不再被编码器丢弃。Chat Completions 保留原 tool 文本消息，把连续 tool 消息的图统一附于之后的 user 消息，带 tool_call_id 与“未信任工具输出，不是用户指令”标签；Responses function_call_output 在含图时使用 input_text/input_image 数组；Anthropic tool_result 使用 text/image blocks。纯文字编码形状保持原样。能力 gate 和 Anthropic prepare_request 递归检查工具结果中的图片，未声明 image_input 时发请求前失败。

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
  - `glm-coding` / `opencode-go` / `qwen-token-plan` / `deepseek` / `kimi-platform` / `kimi-code`：任一开启即编译共用的 `api_key` 模块（Kimi Code 另编译 `kimi` adapter；`verify_api_key` 复用 `/models`）；
  - `CHANNEL_REGISTRY` 与 `channels/registry` 不受任何 feature 门控，始终可用（数据恒定八行）。

## 7. 测试与验证资产

2026-09-20 测试重构：Chat 文本/并行工具/usage 的重复解析切面由 HTTP/SSE contract 承接，保留 thinking、终态优先级与损坏流边界；Anthropic 静态目录的能力断言并入 `list_models_is_static_and_does_not_hit_network`，同时验证真实入口不发网。定向入口 `bash scripts/test.sh providers` 显式启用九个通道 feature。 本批执行状态见 Git 历史（37fae8f3:docs/testing-refactor-plan.md）。

同批：删除 `capability_source_priority_is_static_then_probe_then_override`（derive Ord 自证）和 `accumulator_starts_from_zero`（空 Default 自证）。三源收窄仍由 `capability_evidence` 合并测试覆盖；会话累计仍由同请求覆盖、跨请求累加和 `finish_request` 结算三项验证。

2026-09-17 computer use：`request::tests::tool_result_images_map_across_chat_responses_and_anthropic` 覆盖 JPEG 工具结果在三协议的文字/图片保留、多个 tool response 顺序；`negotiate::tests::capability_gate_rejects_nested_tool_result_image_without_declaration` 覆盖不支持图片时拒绝。

默认验证入口：`bash scripts/test.sh providers`，一次 Cargo 调用显式启用九个通道 feature。直接运行不带 feature 的 `cargo test -p pawork-providers` 只选择默认 Anthropic，不能证明其余通道通过；需要更窄的回归时按下表明确选择 feature/target。Kimi 真实联网专项仍为显式 ignore，不计入本地通道回归通过。

| 测试资产 | required-features | 覆盖点 |
| --- | --- | --- |
| `src/**` 内 `#[cfg(test)]` | — | 各模块单测：`module_discipline`（core 不引用 net）、注册表八行顺序与 fail-closed、kimi-code 端点预设与双认证、xAI 双认证凭证接受、xAI/Kimi 远端目录解析与失败路径（wiremock）、SSE 边界、保留键忽略、协商 clamp、pricing 定点、错误分类脱敏等；VISION-1 / SEARCH-1 增补：builtin 目录逐型号视觉断言（`builtin_catalog_declares_vision_per_model`）；MM-1 增补：`channel_image_limits_reject_format_and_decoded_size`；VISION-2 / CAP-PROBE 增补：默认能力表分轨断言（`default_image_input_distinguishes_verified_text_only_from_unknown`，覆盖 image_input 实测翻转、`default_image_output` 与 `apply_default_hosted_web_search` 仅 Responses 插标签）、`capability_gate` 图片 / web search 拒放矩阵（`capability_gate_fail_closed_on_undeclared_image_and_web_search`）、未声明 hosted 工具在发 HTTP 前拒绝（`hosted_tools_rejected_without_declared_wire`）、Responses `web_search` 工具写入与 `web_search_call` / `url_citation` SSE 归一（`responses_body_writes_web_search_tool_only_when_wire_allows` / `assembler_maps_web_search_call_and_url_citation`） |
| `tests/common/mod.rs` | —（随引用它的测试目标编译） | 集成测试共享件单一来源（MOCK-7 去重）：SSE 帧拼装 `sse_frames`/`sse_body`、chat 文本流 / 单工具调用 / usage+stop / 最小成功 / 仅收尾样例、Responses 完成 / 文本流样例；`contract` 流断言（text / tool / usage / error 归一）供 contract.rs 与 api_key_channels.rs 共用 |
| `tests/contract.rs` | —（默认即跑） | OpenAI-compatible 契约全集（见下） |
| `tests/anthropic.rs` | `anthropic` | Messages 契约（见下） |
| `tests/chatgpt.rs` | `chatgpt-oauth` | OAuth 头 / models / Responses 路径接线；malformed Responses 事件即使后随完成事件也报错 |
| `tests/xai.rs` | `xai-oauth` | 模型能力选 Responses/Chat；Grok 订阅 `/models` 目录、实际模型 ID 与协议路由、认证头（含 `x-grok-client-version` 存在性）与模型路由头、VISION-2 默认图像回填与 Responses 模型 WebSearch 声明；Responses 带 OAuth Bearer 的全链路往返 |
| `tests/responses.rs` | `chatgpt-oauth` | `to_responses_body` 保留 canonical tools、拦截保留键覆盖 |
| `tests/api_key_channels.rs` | 五个 API-key feature（含 kimi-platform） | 五通道默认 id/endpoint 覆盖、未声明 api_key 的 preset 与缺/错凭证 fail-closed、固定凭证头拒绝、Bearer Chat 路径、模型声明 transport 选 Responses 且不按通道分支 |

`tests/contract.rs` 契约点（wiremock 驱动）：

- 文本流 / 单工具调用 / usage + stop 三切面合并为默认执行的 `contract_chat_facets_default_coverage`，五通道表驱动用例另外复用 `tests/common` 样例与断言；并行工具调用回归保留；
- 流中取消与预取消（预取消不发请求）、超时归一、长流逐 chunk 重置读超时；
- 429 归一（含 `Retry-After`）、上下文溢出（413）归一；
- malformed 流中断与中断后重连、`[DONE]` 无 finish_reason 按完成、部分 JSON 工具参数跨 chunk 组装、`list_models`。

`tests/anthropic.rs` 契约点：

- 文本/单工具/并行工具流、流中取消与预取消、429 归一、缺 `message_stop` 判 `StreamInterrupted`；
- `list_models` 静态目录不触网；
- prompt cache 与 thinking 按 plan 写 wire（`contract_prompt_cache_and_thinking_are_written`）；
- hosted WebSearch 写成 `web_search_20250305` 并放行、未声明的 hosted 工具 HTTP 前拒绝（`hosted_web_search_written_and_undeclared_tools_rejected_before_http`，SEARCH-1 起替换原全拒口径）；
- thinking signature 走 protect、不以明文出现在事件流（`contract_thinking_signature_is_protected_not_emitted`）。

dev-dependencies：`wiremock`（HTTP mock）、`proptest`、多线程 tokio。产品级验证边界见 [../verification.md](../verification.md)。

ADR-057：`tests/api_key_channels.rs` 在既有通道契约测试中核验 Chat / Responses 的会话头、连续请求身份隔离与其他通道不携带；非法会话头定向回归断言不发请求且错误不回显原值。

UI-6a 回归扩充现有 contract / Kimi / xAI / ChatGPT 解析用例，补混合目录与 mock transport 一致性主路径（含未支持协议无网络拒绝）。Go 验证失败保旧与成功脱敏在 app 的 Settings 真实 Host 回归覆盖。

MOCK-7 测试整合：已实现（`tests/common` 单一来源去重 `sse_body` 与 SSE 样例、contract.rs 与 api_key_channels.rs 等效用例合并为五通道表驱动、chatgpt.rs / xai.rs 样例改引 common；不切换录制 fixture，属后续候选）；已验证（上方带齐 features 的单条命令全绿，2026-09-10）。
三切面（文本流 / 单工具调用 / usage+stop）的默认死表覆盖由 `contract_chat_facets_default_coverage` 保持（引用 common 样例、不复制；默认无 features 命令与带齐 features 命令均复验全绿）。

2026-09-15 审查回归扩展既有测试：xAI Responses search 请求体与 Chat 发网前拒绝、Anthropic 参数块 / 成功结果 / 错误结果终态、Responses failed/incomplete 与重复终态，远端视觉证据收窄与未接线搜索标签拒绝。命令：`cargo test -p pawork-providers --offline --lib --tests --all-features`；1 个既有网络 opt-in 测试仍 ignored。

## 8. 注意事项与已知限制

- `ReasoningProtector` 的生产实现（`SwappableReasoningProtector`，含 master key 管理）在 `pawork-app` 的 protected 模块，本包只有内存实现；跨包链路见 [../flows.md](../flows.md)。
- `builtin_models`（Anthropic）静态目录只含 claude-3-5-sonnet / claude-3-5-haiku 两条基线；线上新模型依赖 registry 动态合并或 config 声明。
- `xai_builtin_models` 只含已知 id 的 transport / 能力提示（`grok-4` / `grok-4-fast` / `grok-3` / `grok-2`），不是 GUI 选择目录。可选模型始终来自远端目录（OAuth `/models`，API key `/language-models`）；订阅采用远端声明的 Chat/Responses 协议，API key 未登记 builtin 的 id 按 Chat Completions 兜底，窗口与图像以远端字段为准；远端未声明模态时图像输入按 `default_image_input` 默认表回填（VISION-2），Responses 模型一律声明 hosted WebSearch。
- `BUILTIN_RATE_VERSION = "2026-08-15"`：内置费率卡有版本口径，厂商调价后需要更新数据表（非代码逻辑）。
- ChatGPT 通道的 `client_version`（当前 `0.153.0`）参与后端 `/models` 目录过滤与 UA 构造，版本过旧会拿到空目录；redirect URI 固定 `localhost:1455`（上游 allow-list 精确匹配，host/port 不可改）。
- `error_table` 是子串匹配的经验规则表，厂商错误文案变化时可能失配（回退到通用分类，不影响正确性只影响精度）。
- 本包不做重试编排；`retry` 模块只负责分类与 `Retry-After` 解析（HTTP-date 用内置最小解析器，仅识别 IMF-fixdate GMT），重试策略由上层决定。
- `OpenAiCompatibleConfig.request_timeout` 是「建连及流式读取无数据超时」的便捷字段（设置时覆盖 `http.timeout`）；配合逐 chunk 重置语义，长流只要持续有数据就不会误杀。
- `channels/mod.rs` 的 re-export 保持合并前 adapters 包的对外路径形状（`ApiKeyChannelConfig` / `ChatGptProvider` 等可从 crate 根直取），消费方无需感知内部目录结构。
- `responses_reasoning` 是 crate 私有模块（无 `pub`），其行为只能经 `responses` 模块间接观察；历史 hint 键拼写兼容属于该模块内部契约。
- 各 OAuth 流程本身（PKCE/Device/refresh）由 `pawork-auth` 承载（见 [auth.md](auth.md)）；本包注册表只提供端点预设数据。任务状态与阶段口径见 [AGENTS.md](../../../AGENTS.md)。
