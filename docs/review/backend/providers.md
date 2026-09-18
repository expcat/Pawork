# pawork-providers Review

> 首发模型渠道适配层：OpenAI-compatible / Responses / Anthropic Messages 三套 wire transport、八通道静态注册表、三源能力证据与协商（含发网前 capability gate）、token 计量与计价、Go /usage 额度读取。共 35 个 .rs 文件 / 15,237 行（src 12,725 / 28 文件 + tests 2,512 / 7 文件）；主要模块：`net`（http/sse/retry）、`channels`（八通道与 CHANNEL_REGISTRY 单点登记）、`registry`/`negotiate`（能力证据与协商）、`responses`/`request`/`stream`（wire 翻译与流组装）、`pricing`/`usage`（计量）。

## 1. 职责与边界

- **做什么**：canonical `CanonicalModelRequest` → 三种厂商 wire（Chat Completions / Responses / Anthropic Messages）；SSE 字节流 → `ProviderStreamEvent`；凭证校验与认证头注入（`ResolvedCredential` 唯一入口，构造期拒绝固定凭证头）；模型目录三源证据（静态/probe/override）+ `CapabilityNegotiator` 协商 + `capability_gate` 发网前统一闸门；通道 preset 单点登记（`CHANNEL_REGISTRY` 八行 + `is_enabled` 唯一 cfg 求值点）；reasoning continuation 保护边界（trait + 内存实现）；usage 归一/聚合与 micros 整数计价；HTTP 状态与厂商错误正文归一；Go /usage 三窗额度读取与 `verify_api_key` 写前验证。
- **不做什么**：不做重试编排（`retry` 只分类与解析 Retry-After，策略归上层）；OAuth 流程本身（PKCE/Device/refresh）在 `pawork-auth`，本包只提供端点预设数据；不持久化 Secret（明文只在构造 header 与 protector 调用边界短暂存在）；不按 Provider 名称做能力分支——一切差异经 registry 证据 + negotiate 表达（adapter 内按 preset.id 的分支是装配数据差异，不是能力特判）。
- **模块纪律**：core 模块（registry/pricing/usage/negotiate/reasoning/error）零 `net` 引用（`lib.rs` 内 `module_discipline` 测试强制，`lib.rs:122-138`）；`responses_reasoning` 为 crate 私有模块，行为只能经 `responses` 观察。
- **feature 门**：9 个空 feature（anthropic 默认开）；Chat Completions 基线始终编译；各通道适配器按 feature 编译；`CHANNEL_REGISTRY` 与 `channels::registry` 不受任何 feature 门控（八行数据恒定）。

### Spec 与源码差异（以源码为准）

- Spec §2 文件地图写「共 27 个 .rs 文件，约 10.4k 行」；实际 **28 文件 / 12,725 行**（tests 另 7 文件 2,512 行）。多行量级过期：request ~470→674（computer-use 工具结果图片续接）、responses ~860→1141（SEARCH-1 web_search_call/url_citation）、negotiate ~500→727（capability_gate）、provider ~330→496、kimi ~260→421、api_key ~550→605（fetch_go_usage + Go/Qwen 官方 transport 表）。
- 通道数量本身无漂移：spec 全文已按八通道表述，`CHANNEL_REGISTRY` 实际八行（chatgpt/xai/glm-coding/opencode-go/qwen-token-plan/deepseek/kimi-platform/kimi-code，`channels/registry.rs:110-236`）。**源码注释层仍有六处滞后**：`lib.rs:5`（只列六通道、写「四条 API-key 通道」）、`error_table.rs:4`（「本期六个渠道」）、`channels/mod.rs:1`（「首发六通道」）、`channels/registry.rs:4` 与 `:254`（「六行语义」）、`registry.rs:11`（ChannelKind::ApiKey 注释「四条」实为五条）。不影响行为。
- `VENDOR_ERROR_RULES` 实为 **13 条**（chatgpt 2 / xai 3 / qwen-token-plan 4 / glm-coding 3）；spec §3.8 的叙述按语义合并了 qwen 两种拼写与 glm 的 1301/敏感，无实质冲突。

## 2. 依赖关系

| 方向 | 包 | 用途 |
| --- | --- | --- |
| 上游（生产） | pawork-domain | canonical 类型、`ModelProvider`/`ProviderEventSink` trait、`ProviderError`、`CancellationToken`、`ProtectedBlobRef`/`ReasoningItem`、`ServerToolEvent`/`Citation`、`ResolvedCredential`（Debug 脱敏、无 Serialize，domain/src/provider_api.rs:465-496） |
| 下游 | pawork-app（生产，开全部 9 个 feature） | 通道装配、registry 合并、transport 选择、fetch_go_usage/verify_api_key 宿主接线 |
| 下游 | pawork-engine（仅 dev-dependency） | 守护测试名单从 `CHANNEL_REGISTRY` 派生 |
| 协作 | pawork-auth | OAuth 授权流本体（本包 registry 只提供端点预设数据） |

| 外部 crate（生产） | 用途 |
| --- | --- |
| reqwest | HTTP 客户端（重定向 Policy::none、逐 chunk 读超时） |
| tokio / futures | 异步运行时与 Stream 消费、`tokio::select` 取消竞争；生产 tokio features 为 `sync` / `rt` / `time` / `macros` |
| serde / serde_json | wire body 构造与流式 JSON 解析、`ModelPricing` 冻结形状 |
| thiserror | `RegistryError`/`ReasoningProtectError`/`SseParseError` |
| tracing | 保留键忽略告警、reasoning continuation 降级 debug 日志 |
| bytes / async-trait | 字节流类型（`ByteStream`）与 async trait |

**dev**：`wiremock`（HTTP mock）、`proptest`（SSE 随机字节）、`tokio`（macros / rt-multi-thread / time）。

## 3. 文件清单

| 相对路径 | 行数 | 职责 |
| --- | ---: | --- |
| src/lib.rs | 139 | crate 门面、`is_credential_header`（五头）、feature 门控 re-export、module_discipline 测试 |
| src/error.rs | 19 | `RegistryError`（NotFound/DuplicateAlias/DuplicateModelId） |
| src/error_table.rs | 165 | 厂商错误细化表 `VENDOR_ERROR_RULES`（13 条）+ `normalize_vendor_error` |
| src/provider.rs | 496 | `OpenAiCompatibleProvider`：Chat Completions 基线适配器 + 共享 catalog 解析 helper |
| src/request.rs | 674 | canonical → Chat body（`to_chat_completions_body`）；工具结果图片续接到后续 user 消息（2026-09-17） |
| src/stream.rs | 234 | Chat chunk → 事件（`chunk_to_events`/`is_done`/`ChunkState`） |
| src/responses.rs | 1,141 | 共享 Responses transport + wire 构造 + `ResponsesStreamAssembler`（含 web_search_call / url_citation 归一） |
| src/responses_reasoning.rs | 247 | Responses reasoning item 与 canonical 安全映射（crate 私有） |
| src/registry.rs | 2,009 | `ModelRegistry` 三源证据、probe 状态机、别名解析、内置目录（8 条 builtin） |
| src/negotiate.rs | 727 | `CapabilityNegotiator` 纯函数协商 + `clamp_reasoning_to_thinking` + `capability_gate`（:229） |
| src/pricing.rs | 199 | `ModelPricing` micros 定点计价、`BUILTIN_RATE_CARD` |
| src/usage.rs | 299 | usage/stop reason 归一、`UsageAccumulator` 会话聚合 |
| src/reasoning.rs | 98 | `ReasoningProtector` trait 与错误类型 |
| src/memory_protector.rs | 106 | `InMemoryReasoningProtector` 进程内实现 |
| src/net/mod.rs | 9 | net 门面（http/retry/sse） |
| src/net/http.rs | 451 | `HttpClient`：超时/代理/trace/取消、脱敏 Debug |
| src/net/retry.rs | 284 | `classify_status`/`classify_request_error`/`parse_retry_after` |
| src/net/sse.rs | 449 | 增量 SSE 解析器（1 MiB 有界缓冲、跨 chunk UTF-8 安全） |
| src/channels/mod.rs | 64 | channels 门面与 re-export（保持合并前对外路径形状） |
| src/channels/registry.rs | 380 | `CHANNEL_REGISTRY` 八行 + `is_enabled` 唯一 cfg 求值点 |
| src/channels/api_key.rs | 605 | 五条 API-key 通道共用适配器 + `verify_api_key` + `fetch_go_usage` + Go/Qwen 官方 transport 表与家族回退 |
| src/channels/chatgpt.rs | 319 | ChatGPT OAuth Responses 适配器（/models 目录解析，client_version 默认 0.153.0） |
| src/channels/xai.rs | 483 | xAI 双认证适配器（Chat/Responses 按模型选）+ 远端 language-models 目录 + 静态目录 |
| src/channels/kimi.rs | 421 | Kimi Code OAuth/API-key 适配器（固定 Chat Completions）+ 远端 /models 目录 |
| src/channels/anthropic/mod.rs | 18 | Anthropic 门面、`ANTHROPIC_VERSION` |
| src/channels/anthropic/provider.rs | 1,118 | AnthropicProvider：协商收口、thinking 保护、流泵 |
| src/channels/anthropic/request.rs | 836 | canonical → Messages body（`MessagesWirePlan`，tool_result 图片 blocks） |
| src/channels/anthropic/stream.rs | 735 | Anthropic SSE → 事件（`parse_event`/`StreamOutput`，server tool 终态去重） |
| tests/contract.rs | 645 | OpenAI-compatible 契约全集（wiremock，含 `contract_chat_facets_default_coverage` 默认死表覆盖） |
| tests/anthropic.rs | 612 | Anthropic Messages 契约（wiremock，含 hosted WebSearch 写 wire 与 HTTP 前拒绝） |
| tests/api_key_channels.rs | 650 | 五通道默认值、fail-closed、Go usage 解析与 verify、混合 transport 路由 |
| tests/xai.rs | 198 | xAI 模型能力选 transport + OAuth Responses 往返 |
| tests/chatgpt.rs | 155 | ChatGPT 头/models/Responses 接线 + malformed 立即报错 |
| tests/responses.rs | 66 | `to_responses_body` 保留 canonical tools、拦截保留键覆盖 |
| tests/common/mod.rs | 186 | 集成测试共享件单一来源（SSE 帧/样例/流断言，MOCK-7 去重） |

## 4. 类型与方法功能列表

### lib.rs（门面）

`is_credential_header(name)`（pub(crate)）：五头名单 `authorization`/`proxy-authorization`/`api-key`/`x-api-key`/`x-goog-api-key`，大小写不敏感——所有适配器构造期据此拒绝固定凭证头。其余为模块声明与按 feature 的 re-export（含 `fetch_go_usage`/`verify_api_key`/`GoUsage`/`GoUsageWindow`，任一 API-key feature）；`module_discipline` 内联测试扫描 core 六模块源码禁止出现 `net` 标识符。

### provider.rs（Chat Completions 基线）

| 名称 | 种类 | 功能/语义 |
| --- | --- | --- |
| `OpenAiCompatibleConfig` | pub struct | `base_url`/`provider_id`（默认 openai-compatible）/`http`/`request_timeout`（便捷字段，设置时覆盖 http.timeout）；`chat_url()`/`models_url()` 拼接并去尾部斜杠 |
| `OpenAiCompatibleProvider` | pub struct | `new(config, credential)`：extra_headers 含凭证头 → 构造失败；`with_opencode_session()`（ADR-057，仅 Go 启用会话头校验）；`drive_stream`：to_body → POST（Bearer 头）→ `ResponseStarted` → SseParser + ChunkState 循环 → `sse.finish()` 冲刷尾事件 → `[DONE]` 无 finish_reason 按 Completed；未见完成信号 → `StreamInterrupted`；`list_models`：GET /models，data[] 保守映射；预取消在 HttpClient 层竞争（不发请求） |
| `catalog_entries` / `catalog_model_id` | pub(crate) fn | 各通道 list_models 共用的形状校验与 id 提取（严格形状，缺失/非数组报 InvalidRequest） |

### request.rs（Chat wire 翻译）

`to_chat_completions_body`：model/stream=true/`stream_options.include_usage`；messages 经 `message_to_openai`（ToolResult 展开为独立 role=tool 消息；文本 join；图片 → `image_url` 数组（Url 透传、Base64 拼 data: URI、Artifact 跳过）；ToolCall → `tool_calls[]`；Thinking/Reasoning 不回传）；**工具结果图片续接（2026-09-17）**：Chat 保留原 tool 文本消息，连续 tool 消息的图统一附于之后的 user 消息（带 tool_call_id 与「未信任工具输出」标签，`deferred_tool_result_images`/`collect_deferred_tool_result_images`/`flush_tool_result_images`，:262-330）；tools → function 形态 + tool_choice 映射；response_format；thinking.level → `reasoning_effort`；provider_options 白名单透传（`is_reserved_provider_option` 十四键拦截并 tracing 告警，:122-140）。

### stream.rs（Chat 流解析）

`chunk_to_events(data, &mut ChunkState)`：usage → `normalize_usage` → UsageUpdated（非零才发）；delta.content → TextDelta；delta.reasoning_content|reasoning → ThinkingDelta；delta.tool_calls 按 index 组装（首段带 id/name → ToolCallStarted + 参数增量；无 id 合成 `call-{index}`）；finish_reason → 对已开始调用发 ToolCallCompleted + `map_stop_reason` → ResponseCompleted。`is_done`：trim 后 == `[DONE]`。

### usage.rs / pricing.rs（计量与计价）

`normalize_usage` 兼容 OpenAI 与 Anthropic 命名（usage 容器优先、顶层回退，缺字段按 0）；`map_stop_reason` 全映射（None → Completed，未知 → Other）；`UsageAccumulator` 请求内快照覆盖、跨请求累加（saturating）。`BUILTIN_RATE_CARD="builtin"`、`BUILTIN_RATE_VERSION="2026-08-15"`；`estimate_cost` micros 定点（u128 中间、MILLION 基数），计价单轨。

### error_table.rs（厂商错误细化）

`VendorErrorRule{vendor, needles, kind, retryable, detail, diagnostic_key}`；`VENDOR_ERROR_RULES` **13 条**：chatgpt 2（usage+limit → QuotaExceeded；account+deactivated → Authorization）、xai 3（live_search+quota → RateLimited；collection+not_ready → ProviderUnavailable；insufficient_quota → QuotaExceeded）、qwen-token-plan 4（datainspectionfailed 与 data_inspection_failed 两条 → ContentFiltered；throttling → RateLimited；quota_exhausted → QuotaExceeded）、glm-coding 3（1113 → QuotaExceeded；1301 → ContentFiltered；敏感 → ContentFiltered）。`normalize_vendor_error`：消息小写后须命中该厂商规则**全部** needles 才改判；未命中原样返回。

### registry.rs（目录与三源证据）

| 名称 | 种类 | 功能/语义 |
| --- | --- | --- |
| `CatalogEntry` | pub struct | id/provider/display_name/窗口/输出/capabilities/pricing?/aliases；`to_definition()` 转协议定义 |
| `CapabilityEvidence` | pub struct | 三源快照（Static/Probe/Override）；`merged()` = present 来源逐字段交集 |
| `ProviderProbe` / `ProbeError` | pub | 探测结果与失败缓存；`ProviderCapabilitySource` trait 数据驱动，Core 不做名称匹配 |
| `merge_capabilities` | pub fn | serde Value 层字段级合并：bool 取 AND、数组取交集、冲突移除键（fail-closed） |
| `ModelRegistry` | pub struct | `empty`/`builtin`（8 条内置，VISION-1 起按型号声明视觉）/`register`/`try_register`/`extend_with`/`merge_provider_models(source)`/`resolve`（真实 id 优先）/`list`/`filter`/`validate_context`/`estimate_cost`/`set_override`（只收窄）/`record_probe`/`probe_provider`（Idle→InFlight→Done 状态机、不持锁跨 await）/`capability_evidence`/`capability_snapshot` |
| `caps` | pub fn | 布尔便捷构造（测试与静态目录用） |

### negotiate.rs（协商与发网前闸门）

`CapabilityNegotiator::negotiate(evidence, requirements)` 纯函数（不触网、不读 Provider 名、不读时钟）：transport 选择、hosted tools 逐 tag、citations、reasoning 三态独立判定；不变式 `requested == supported ∪ unsupported`。`clamp_reasoning_to_thinking` adapter 复用。`capability_gate(&CapabilityEvidence, &CanonicalModelRequest)`（:229，VISION-1/SEARCH-1）：从请求派生 required_tools（hosted + extension）与 image_input（含嵌套工具结果图片递归检查），走同一 negotiate，任一 Reject 即 InvalidRequest 不触网；无证据按空证据处理（纯文本放行、图片/hosted 工具 fail-closed）。

### reasoning.rs / memory_protector.rs（reasoning 保护）

`ReasoningProtector` trait：async protect/resolve 不透明载荷互转，不解释明文；`ReasoningProtectError` Unavailable/Corrupted 判别。`InMemoryReasoningProtector`：namespace 由计数器 + RandomState 派生，Debug 只输出 entry_count。生产实现 `SwappableReasoningProtector` 在 pawork-app。

### responses.rs（共享 Responses transport）

| 名称 | 种类 | 功能/语义 |
| --- | --- | --- |
| `ResponsesWireOptions` | pub struct | `store` + `include_encrypted_reasoning` + `hosted_web_search`（SEARCH-1：true 时 hosted WebSearch 写 `tools:[{"type":"web_search"}]`；false 时 hosted 工具在流入口发 HTTP 前拒绝） |
| `ResponsesTransportConfig` | pub struct | base_url/provider_id/http/request_timeout/request_headers/wire；认证头禁止出现在固定头（两处构造期检查） |
| `ResponsesTransport` | pub struct | `new` fail-closed + 默认 InMemory protector；`with_reasoning_protector`/`with_opencode_session`；`request_headers()` 追加 Bearer；`get_json`（经 normalize_vendor_error）；`stream`：hosted/extension 工具先过 wire 声明检查 → resolve_reasoning_inputs → to_responses_body → POST → SSE 喂 assembler → finish 合并终态；无完成事件 → `StreamInterrupted` |
| `to_responses_body` | pub fn | System → instructions；input 数组（message/function_call/function_call_output；含图时 function_call_output 用 input_text/input_image 数组）；reasoning effort（XHigh/Max 压 High）；include encrypted_content；store；`previous_response_id` 白名单 + 其余 provider_options（十七键拦截） |
| `ResponsesStreamAssembler` | pub struct | 流组装状态机：function_calls item_id→call_id、completed_calls 去重、web_search_call added → ServerTool::Started / done 按 status 映射终态并去重；`response.output_text.annotation.added` 的 url_citation → CitationAdded（归到最近 web_search_call，其余 annotation 忽略）；malformed JSON 立即 Error（后随完成事件不救回）；`finish()` → `ResponsesFinalState` |

### responses_reasoning.rs（crate 私有）

`EncryptedContent` Debug 只输出 byte_len；`to_canonical`/`to_input` 双向映射（summary entries 校验拒绝多余字段）；历史 hint 键拼写兼容（LEGACY_HINT_KEY_MAP，生产只写规范键）。

### net/（http / sse / retry）

`HttpClientConfig`：timeout（逐 chunk 读超时重置）/proxy/user_agent/extra_headers/system_proxy；Debug 对 header 值与 proxy URL 脱敏。`HttpClient::new`：重定向 Policy::none（跨源 fail-closed）、loopback 直连。`post_stream(_with_headers)`：头合并、x-trace-id、biased select 预取消竞争；非 2xx → classify_status。`get_json(_with_headers)` 同构。`SseParser`：1 MiB 有界缓冲、跨 chunk UTF-8 安全、超限丢弃到边界再恢复、`finish()` 必调。`classify_status`：固定状态映射，message 固定 `HTTP {code}`（正文不入 message）；`classify_request_error`：URL 脱敏到 origin；`parse_retry_after`：秒数或 IMF-fixdate。包内无重试编排，通道也不自行重试（无双重重试路径）。

### channels/registry.rs（通道单点登记）

`ChannelKind`：ApiKey/ChatGptOAuth/XaiOAuth/KimiOAuth。`OAuthPreset(Data)`/`OAuthFlow(Data)` const 镜像 + to_preset。`ChannelPreset{id, kind, default_base_url, display_name, feature, oauth, auth_methods}`。`CHANNEL_REGISTRY` **八行**（chatgpt PKCE、xai Device+双认证、glm-coding、opencode-go、qwen-token-plan、deepseek、kimi-platform 五条 API-key、kimi-code Device+双认证）；`is_enabled` 唯一 cfg! 求值点（未知 feature fail-closed）。新增通道 = 加一行 + is_enabled 加一分支。

### channels/api_key.rs（五通道共用）

`ApiKeyChannelConfig`：preset 必须声明 api_key 认证方法且 feature 已启用（双 fail-closed）；`model_transports: BTreeMap` 由 `documented_model_transports(preset.id)` 初始化（Go/Qwen 官方逐模型表）；`transport_for` 与目录筛选、stream 共用：表命中用表内协议，未命中先过 `non_text_model`（图片/音频/TTS/realtime → None），再按家族回退（Go：grok-*/gpt-*/muse-spark-* → Responses，qwen*/minimax-* → Messages，其余 Chat；Qwen 同 non_text 规则）。`ApiKeyChannelProvider`：chat + responses（store=None + include_encrypted_reasoning + hosted_web_search=false）双 transport；Messages-only 直接请求 HTTP 前拒绝；仅 opencode-go 启用会话头映射。`verify_api_key`：Go 分支 `fetch_go_usage` 三窗均成功才过，其余通道构造一次性 adapter GET /models。`fetch_go_usage`（UI-6b G2）：一次认证 GET /usage 返回 `GoUsage{rolling,weekly,monthly}`；整数百分比 + toISOString 重置时刻严格校验；单窗失败独立、顶层畸形整体失败；总期限默认 60s、可取消、错误不回显上游字段。

### channels/chatgpt.rs / xai.rs / kimi.rs

**chatgpt**：仅 OAuthBearer；固定头 ChatGPT-Account-Id + originator codex_cli_rs；client_version 默认 **0.153.0**（字符集校验，/models 过滤与 UA）；`chatgpt_models` 消费 models[].slug/visibility/context_window|max_context_window/supported_reasoning_levels（none-only 不算思考）/supports_parallel_tool_calls/input_modalities；全模型声明 hosted WebSearch + image_input（SEARCH-1/VISION-1）。

**xai**：OAuthBearer | ApiKey 双认证；chat + responses（hosted_web_search=true）双 transport + 独立 models 客户端 GET {base}/language-models；仅保留 output_modalities 含 text 的模型；已知 id（grok-4/grok-4-fast/grok-3/grok-2）沿用 builtin 元数据，未知保守默认（text + Chat + 窗口 0）；非 Responses transport 清除 hosted 标签（Chat 不声明 hosted search，发网前拒绝）；aliases 仅辅助找静态证据。

**kimi**：OAuthBearer | ApiKey 双认证；固定 Chat Completions；GET {base}/models data[] 严格形状；消费 display_name/context_length/supports_reasoning/supports_image_in；builtin 四条（kimi-for-coding/-highspeed、k3、k3-256k）声明 image_input，不声明 WebSearch（$web_search 两段流程未接线，fail-closed）。

### channels/anthropic/*（Messages 基线）

`ANTHROPIC_VERSION = "2023-06-01"`。`request.rs`：`MessagesWirePlan{write_cache, thinking_budget, resolved_thinking_blocks}`；max_tokens 推导、system 提升、连续 Tool 角色合并、thinking 只回放 resolved 签名块、tool_result 用 text/image blocks（2026-09-17）、三段式 cache_control、二十一个保留键拦截。`stream.rs`：`StreamOutput` 四变体（Event/PendingSignature/ReasoningError/MappingError）；状态机处理 message_start/content_block_*/message_delta/message_stop；server_tool_use 终态只发一次（result 块结束才 Completed，error result 发 Failed 保留 error_code）；citations → CitationAdded。`provider.rs`：`prepare_request` 六步收口（证据→negotiate→prompt cache 三态→thinking 预检→resolve_thinking_blocks→Required 断点校验）；`pump_messages` protect 换 ReasoningItem；builtin 两条（claude-3-5-sonnet/haiku）。

## 5. 关键行为与契约

### 5.1 net/（http/sse/retry）

- HTTP：预取消竞争（biased select，不发请求）；重定向 Policy::none 跨源 fail-closed；逐 chunk 读超时重置；Debug/错误消息对 header 值、proxy userinfo、URL path/query 全脱敏。
- SSE：单事件 1 MiB 有界缓冲；超限报错后丢弃到边界再恢复；`finish()` 必须调用否则尾事件丢失。
- retry：只分类 + Retry-After 解析，不做重试编排；Retry-After 仅 retryable 错误采纳。通道层无自行重试，无双轨。

### 5.2 registry / pricing / usage / negotiate / reasoning（core，零 net 引用）

- 证据合并 fail-closed：present 来源逐字段交集，override 只能收窄；probe 状态机 last-write-wins 与 complete_claimed 仲裁。
- 协商不变式 `requested == supported ∪ unsupported`；`capability_gate` 是发网前统一闸门（Anthropic 经 prepare_request 完整收口，其余通道由装配/Run 服务在派发前调用）。
- 计价单轨 micros 整数；usage 请求内快照覆盖、跨请求累加。
- reasoning 载荷不明文外泄：只经 protector 换 `protected_blob_ref` 进事件流。

### 5.3 channels/ 八通道与 CHANNEL_REGISTRY 登记单点

- 注册表是纯数据：八行不带 cfg；`is_enabled` 唯一 cfg! 求值点，未知 feature fail-closed；新增通道 = 加一行。
- 各适配器构造期凭证契约：API-key 通道仅 ApiKey；ChatGPT 仅 OAuthBearer；xAI/Kimi Code 双接受（互斥替换由宿主保证）；固定头出现凭证头一律构造失败。
- transport 路由按数据：ApiKeyChannel 按 model_transports 表 + 家族回退；xAI 按 builtin/远端能力；ChatGPT 恒 Responses——不按通道名猜协议。
- 远端目录解析只信声明形状，未知 id 给保守默认（不推断窗口与能力），形状不符报 Err；远端成功返回的新聊天 ID 不丢弃。

### 5.4 Responses / Anthropic 流收尾

- Responses：malformed 事件立即 Error（后随 completed 不救回）；无完成事件 → StreamInterrupted。
- Chat：`[DONE]` 容忍首尾空白；无 finish_reason 按 Completed；未见完成信号 → StreamInterrupted。
- Anthropic：无 message_stop → StreamInterrupted；thinking signature/redacted data 缺失 → MalformedResponse，不伪造成功。

### 5.5 凭证与 Secret 契约

凭证只经 `ResolvedCredential` 注入（domain 定义：Debug `[REDACTED]`、无 Serialize）；`is_credential_header` 五头不得出现在任何通道固定头（构造期 fail-closed）；`classify_status` 不把响应正文写入 message；`fetch_go_usage` 错误不回显上游字段。Engine 侧无 Provider 名称特判——能力差异全部经 registry 证据 + negotiate 数据表达。

### 5.6 质量观察（冗余/低效）

- 主要传输路径已充分共享（chat/responses/SSE/错误归一/catalog helper 均单点），通道适配器是薄路由；2026-09-18 起 `require_bearer_credential` 已提炼为 channels 共享 helper（cfg 门控覆盖 xai-oauth/kimi-code 两使用方），xai.rs / kimi.rs 原逐字副本已删除。
- list_models 的 builtin 查表侧已于 2026-09-18 外提（xai.rs 与 kimi.rs 均循环外一次构建）；`transport_for` 每条目重建目录保留未动（builtin 仅数条、量级可忽略）。
- `fetch_go_usage` 每次调用新建 HttpClient（api_key.rs:443）；Settings 按需读频低影响小，若后续做周期轮询应复用客户端。
- 未发现每请求重建 client、流式路径无谓 String 拷贝等系统性低效；SSE 解析增量、无全量缓冲。

## 6. 测试资产

内联 `#[test]`/`#[tokio::test]` 注解 **179**（src），集成 **40**（tests）：

| 文件 | required-features | 验证点 |
| --- | --- | --- |
| src/** 内联（179） | — | module_discipline（core 不引用 net）、注册表八行顺序与 fail-closed、kimi-code 端点与双认证、xAI 双认证与远端目录（wiremock）、SSE 边界与随机字节 proptest、保留键忽略、协商 clamp 与 partition 不变式、capability_gate 图片/hosted 工具拒放矩阵、builtin 目录逐型号视觉断言、pricing 定点与饱和、错误分类脱敏、chatgpt 模型解析（含隐藏模型剔除）、anthropic URL/头/静态目录、registry probe 并发与缓存、merge 交集语义、工具结果图片三协议映射（`tool_result_images_map_across_chat_responses_and_anthropic`，request.rs:575）等 |
| tests/contract.rs | — | 文本流/单工具/并行工具/usage+stop（默认死表覆盖 `contract_chat_facets_default_coverage` + 五通道表驱动）、流中取消、预取消不发请求、超时归一、长流逐 chunk 重置、429+Retry-After、413、malformed 中断、[DONE] 无 finish、跨 chunk 部分 JSON 工具参数、list_models、无凭证不发 Authorization |
| tests/anthropic.rs | anthropic | 文本/单工具/并行工具流、取消、429、缺 message_stop 判中断、list_models 静态不触网、prompt cache 与 thinking 按 plan 写 wire、hosted WebSearch 写 `web_search_20250305` 与未声明工具 HTTP 前拒绝（:494）、thinking signature 走 protect 不明文出现 |
| tests/chatgpt.rs | chatgpt-oauth | OAuth 头/models/Responses 接线、malformed Responses 事件即使后随完成也报错 |
| tests/xai.rs | xai-oauth | 模型能力选 Responses/Chat、grok Responses 带 OAuth Bearer 全链路往返 |
| tests/responses.rs | chatgpt-oauth | to_responses_body 保留 canonical tools、拦截保留键覆盖 |
| tests/api_key_channels.rs | 五个 API-key feature（含 kimi-platform） | 五通道默认 id/endpoint、未声明 api_key 的 preset 与缺/错凭证 fail-closed、固定凭证头拒绝、Bearer Chat 路径、模型声明 transport 选 Responses 且不按通道分支、Go usage 解析/取消/错 key/verify 三窗、混合目录与家族回退、会话头与身份隔离（ADR-057） |

默认死表：`cargo test -p pawork-providers --offline --lib --tests`；MOCK-7 整合口径（一次编译链接跑全部目标）：`cargo test -p pawork-providers --offline --lib --tests --features anthropic,chatgpt-oauth,xai-oauth,glm-coding,opencode-go,qwen-token-plan,deepseek,kimi-platform,kimi-code`（本任务未跑）。

## 7. 协作关系

```mermaid
graph LR
  domain[pawork-domain<br/>canonical / ModelProvider trait] --> providers[pawork-providers<br/>wire 适配 + registry + negotiate]
  providers --> app[pawork-app<br/>通道装配 / registry 合并 / protector 注入 / Go usage]
  auth[pawork-auth<br/>OAuth PKCE/Device] -.端点预设由 CHANNEL_REGISTRY 提供.-> providers
  providers -.->|dev-only：CHANNEL_REGISTRY 守护名单| engine[pawork-engine]
  app -.->|ResolvedCredential| providers
```
