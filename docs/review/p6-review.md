# Phase 6 Review：OpenAI / Anthropic / Google 三家 Provider 适配

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树仅含未跟踪的 REVIEW 文档，无代码改动）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置）
- **范围**：ROADMAP.md Phase 6 的 9 个任务（P6-1 ~ P6-9）的完成情况、所引入包是否合适、是否存在更优替代或自实现替换的必要；附基线偏差、漏洞与优化点。

### 1. 结论摘要

1. **三大 Provider 适配质量可信**：provider-openai / provider-anthropic / provider-google 各自通过统一 Contract Tests（10 / 13 / 14 项，全程 wiremock 不触网），覆盖文本流、单/并行 tool call、usage+stop、cancel、429 限流、流中断归一。P6-1/2/3 🟢 属实。
2. **跨切能力（P6-5/6/7/9）落实良好**：Thinking / Image / Prompt Cache / provider_options 的 canonical 表达落在 provider-api，三家适配器各自正确映射；cache token（Anthropic `cache_read_input_tokens` / `cache_creation_input_tokens`、Gemini `cachedContentTokenCount`）已归一到 usage。**ADR-002 解耦红线成立**：`rg` 全量扫描 agent-engine / context-engine / agent-domain 无 provider 名特例分支（仅 context-engine 按 model 名选 tiktoken/启发式估算器，与基线一致）。
3. **两个完成度存疑项**：
   - **P6-8 结构化输出对 Anthropic 是空操作**：[request.rs:109-114](../../crates/provider-anthropic/src/request.rs) 的 `ResponseFormat::Json | JsonSchema` 分支只有注释、无任何指令注入或 schema 透传，schema 被静默丢弃；与 OpenAI（`json_schema`）/ Google（`responseSchema`）形成行为不对称。
   - **P6-4 OAuth 为「库已完成、零接线」**：PKCE / Device Flow / refresh / callback primitives 与脱敏红线都到位，但 `needs_refresh` / `refresh_access_token` / `store_oauth_token` / `resolve_oauth_credential` 在 auth-service 之外**无任何消费者**，「auto refresh」未进入请求路径，刷新后轮换的 refresh token 也无处回写。任务标 🟢 与实际集成状态有偏差。
4. **基线偏差集中在 OAuth**：workspace 基线 `oauth2 = "5"`（[Cargo.toml:96](../../Cargo.toml)）**全仓库零引用**——实现选择了手写而非基线声明的 `oauth2` crate；同时手写引入的 `base64` / `rand` / `sha2` / `url`（[auth-service/Cargo.toml:14-22](../../crates/auth-service/Cargo.toml)）未回填基线。需对「oauth2 基线去留」做一次明确决策。
5. **三个应处理的风险**：(V1) Google 把 API key 放进 URL query 而非 `x-goog-api-key` 头；(V2) Anthropic thinking 默认 budget（High=8192）大于默认 max_tokens（4096），真实 API 会 400 拒绝，且被 mock 测试漏过；(V4) OAuth 刷新令牌轮换未持久化。

### 2. P6 任务完成情况核对表

| 任务 | 交付 crate | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P6-1 OpenAI 适配 | `provider-openai`（复用 `provider-openai-compatible`） | 🟢 | [provider.rs](../../crates/provider-openai/src/provider.rs)：OpenAI 协议即 Chat Completions，复用兼容引擎；contract 10 项全过 |
| P6-2 Anthropic 适配 | `provider-anthropic` | 🟢 | [request.rs](../../crates/provider-anthropic/src/request.rs) + [stream.rs](../../crates/provider-anthropic/src/stream.rs)；contract 13 项全过 |
| P6-3 Google Gemini 适配 | `provider-google` | 🟢 | [request.rs](../../crates/provider-google/src/request.rs) + [stream.rs](../../crates/provider-google/src/stream.rs)；contract 14 项全过 |
| P6-4 OAuth | `auth-service`（oauth.rs） | 🟡 | PKCE/Device/refresh/callback 均实现且测试通过，但**无外部消费者**，auto-refresh 未接线 |
| P6-5 Thinking / Reasoning | `provider-api` + 三家 | 🟢 | canonical `ThinkingConfig`/`ThinkingLevel`（[provider-api/lib.rs](../../crates/provider-api/src/lib.rs)）；Anthropic budget / OpenAI `reasoning_effort` / Gemini `thinkingBudget` 各自映射 |
| P6-6 图片输入 | `agent-domain` + 三家 | 🟢 | `ImageContent`/`ImageSource`（[message.rs:43-57](../../crates/agent-domain/src/message.rs)）；三家 image block 映射均有 contract 覆盖 |
| P6-7 Prompt Cache | `provider-api` + Anthropic/OpenAI | 🟢 | `PromptCachePreference`；Anthropic `cache_control` 标记（[request.rs:44](../../crates/provider-anthropic/src/request.rs)、[request.rs:192-195](../../crates/provider-anthropic/src/request.rs)）；cache token 归一到 usage（[stream.rs:175-185](../../crates/provider-anthropic/src/stream.rs)） |
| P6-8 结构化输出 | `provider-api` + 三家 | 🟡 | OpenAI `json_schema`（[request.rs:75-85](../../crates/provider-openai-compatible/src/request.rs)）、Google `responseSchema`（[request.rs:96-100](../../crates/provider-google/src/request.rs)）OK；**Anthropic 静默丢弃**（[request.rs:109-114](../../crates/provider-anthropic/src/request.rs)） |
| P6-9 Provider-specific options | `provider-api` + 三家 | 🟢 | `provider_options: BTreeMap` 透传；agent core 无 provider 名分支（见 §3.3） |

**门禁证据（2026-08-08 复核）**：

- `cargo fmt --all -- --check`：干净（exit 0）。
- `cargo clippy -p provider-api -p provider-runtime -p provider-openai-compatible -p provider-openai -p provider-anthropic -p provider-google -p auth-service -p model-registry -p agent-domain --all-targets -- -D warnings`：**Finished，无告警**。
- `cargo test`（上述 9 crate）：**187 passed / 0 failed**。Phase-6 自有 crate 合计 94 项（provider-openai 2+10、provider-anthropic 20+13、provider-google 8+14、auth-service 27）；共享层 provider-runtime 54、provider-openai-compatible 12+10、provider-api 4、model-registry 10、agent-domain 3。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本 | 使用点 | 评估 | 结论 |
| --- | --- | --- | --- | --- |
| `reqwest`（rustls+stream） | 0.12 | 三家 provider HTTP 底座 | Provider 流式与 list_models 的唯一网络层，feature 子集精确 | **保留** |
| `serde` / `serde_json` | 1 | 全部请求/响应编解码 | 基础设施 | **保留** |
| `tokio` / `futures` / `bytes` | 1 / 0.3 / 1 | 流式组装、回调服务器、取消竞争 | 异步与字节流核心 | **保留** |
| `thiserror` | 2 | `ProviderError` / `AuthError` | 库错误类型分工 | **保留** |
| `async-trait` | 0.1 | `ModelProvider` / `ProviderEventSink` | 稳定 Rust 对象安全异步接口 | **保留** |
| `keyring` | 3 | `KeychainBackend`（[backend.rs](../../crates/auth-service/src/backend.rs)） | OS Keychain 绑定，Secret 不落库红线依赖 | **保留** |
| `backon` | 1 | provider-runtime 重试退避 | 退避策略完整 | **保留** |
| `wiremock` / `proptest` | 0.6 / 1 | contract 与 fuzz 测试 | 三家 provider 契约套件基座 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 选项 | 建议 |
| --- | --- | --- | --- |
| `oauth2 = "5"` | 基线声明（[Cargo.toml:96](../../Cargo.toml)）且 plan P6-4 写明「基于 oauth2 crate 实现 PKCE / refresh」，但实现**手写**（[oauth.rs:6](../../crates/auth-service/src/oauth.rs) 注释「不引入整套 oauth2 SDK」），全仓库零引用 | a) 采纳 oauth2 crate 重写 token 交换/PKCE 原语，手写层只留 Device Flow 编排；b) 维持手写，**更新基线与 plan 说明自实现理由并移除 oauth2** | **建议 b**。手写质量合格（PKCE S256 经 RFC 7636 测试向量验证、state CSRF 校验、错误归一不含 token、Secret 红线到位）；oauth2 crate 在「PKCE + refresh」子集上的增量价值不足以抵消重写+回归成本。但必须补齐 §3.3 所列缺口（refresh 轮换回写、auto-refresh 接线）并同步基线文档 |
| `base64` 0.22 / `rand` 0.8 / `sha2` 0.10 / `url` 2 | auth-service 手写 OAuth 引入（[auth-service/Cargo.toml:14-22](../../crates/auth-service/Cargo.toml)），**均未登记基线** | 回填基线或改用已有等价物 | **回填**。`base64`/`sha2`/`rand` 是加密/编码自实现高风险区，采用成熟 crate 正确；`url` 已是 `reqwest` 间接依赖，直接引用合理。一并写入基线「直接采用」表并标注 P6-4 |
| Anthropic `response_format` 处理 | 无原生 response_format，当前空实现（V3） | a) 注入 system 指令 + 工具约束 schema；b) 显式返回不支持错误；c) 透传到 provider_options 让上层决策 | **建议 a 或 c**，至少不能静默丢弃（见 V3） |

#### 3.3 「自实现替换包」总体判断

Phase 6 范围内**没有发现应被自实现替换的已引包**——reqwest/serde/keyring 等使用面都覆盖核心价值区。唯一需要决策的是反向问题：**oauth2 crate 基线虚置**。手写 OAuth 在「PKCE + token 交换 + Device Flow」子集上是基线「参考 + 自实现」表的合理延伸（与 SSE 自实现同源），保留手写可行，但需补三个缺口：

1. **refresh token 轮换回写**：`refresh_access_token`（[oauth.rs:307](../../crates/auth-service/src/oauth.rs)）返回的 `TokenSet` 可能携带**新** refresh_token（部分 Provider 每次刷新轮换），但无 `update_oauth_token` 之类函数把它写回 backend；旧 refresh token 失效后用户被迫重新授权。
2. **auto-refresh 编排缺失**：`needs_refresh` + `refresh_access_token` 只是原语，没有任何调用方在发请求前检查并刷新（见 §2 P6-4、§5 V4）。
3. **PKCE verifier 取模偏差**（[oauth.rs:86](../../crates/auth-service/src/oauth.rs)）：`UNRESERVED[(*b % 66)]` 对 66 字符表有轻微偏差（256 非 66 整数倍）。PKCE verifier 只需高熵不可猜，64 字符 ≈ 390 bit，偏差不构成可利用风险，但若决定长期手写，建议改用拒绝采样或直接 base64url(random 48 bytes)。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填、声明须被引用。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用 | `oauth2 = "5"` | [Cargo.toml:96](../../Cargo.toml) | P6-4 改手写，零引用（见 §3.2） |
| 引入未登记 | `base64 = "0.22"` | [auth-service/Cargo.toml:14](../../crates/auth-service/Cargo.toml) | OAuth 手写引入 |
| 引入未登记 | `rand = "0.8"` | [auth-service/Cargo.toml:15](../../crates/auth-service/Cargo.toml) | 同上 |
| 引入未登记 | `sha2 = "0.10"` | [auth-service/Cargo.toml:18](../../crates/auth-service/Cargo.toml) | PKCE S256 |
| 引入未登记 | `url = "2"` | [auth-service/Cargo.toml:22](../../crates/auth-service/Cargo.toml) | 授权 URL 构造 |

> 附注（非本阶段新增，已在 REVIEW.md 记录）：`uuid`、`tracing-appender`、`anyhow`、`similar` 仍为声明未引用；`rmcp`/`wasmtime`/`wit-bindgen`/`landlock`/`windows`/`windows-service`/`portable-pty`/`ed25519-dalek` 属未来 Phase（9/10/11）的预声明，可在对应阶段开工时再评估是否提前引用。

**建议**：一次小型清理——按 §3.2 决策 oauth2 去留（倾向移除并补文档），回填 base64/rand/sha2/url 四项，同步 ROADMAP 基线表。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V8）。

#### V1 [安全·中] Google API key 写入 URL query

[provider.rs:92-97](../../crates/provider-google/src/provider.rs) 把 secret 拼成 `?alt=sse&key=<secret>`，且该请求不附任何认证头（[provider.rs:112](../../crates/provider-google/src/provider.rs) 传 `&[]`）。query 参数会进入：代理访问日志（`HttpClientConfig.proxy` 启用时）、Google 服务端日志、潜在的重定向目标、以及任何诊断/抓包。HTTP 运行时本身不记录 URL（[http.rs](../../crates/provider-runtime/src/http.rs) 仅把 url 作参数、无 tracing 宏输出），但「key 在 URL」是 Google 已明确不推荐的旧式做法。**建议**：改用 `x-goog-api-key: <secret>` 请求头（Google 现行推荐），key 从 URL 移除；这是少量改动且与 Anthropic/OpenAI 的「头携带 secret」模式一致。

#### V2 [正确性·中] Anthropic thinking budget 与 max_tokens 默认冲突

[request.rs:16](../../crates/provider-anthropic/src/request.rs) `max_tokens = max_output_tokens.unwrap_or(4096)`，而 [request.rs:216-225](../../crates/provider-anthropic/src/request.rs) 的 `thinking_budget` 默认 Low=1024 / Medium=4096 / High=8192。Anthropic 扩展思考要求 `thinking.budget_tokens < max_tokens`，因此默认 max（4096）+ High（8192）或 Medium（4096，等于非小于）真实请求会被 API 以 400 拒绝。现有 mock 测试不触网故未暴露（`thinking_maps_to_budget` 用例 budget=8192、max=128 仍断言通过）。**建议**：构造请求体时将 `budget_tokens` 钳制为 `< max_tokens`（留余量），并对「未显式设 max_output_tokens 但开 thinking」补一条默认提升或告警。

#### V3 [正确性/功能·中] Anthropic 结构化输出静默丢弃

[request.rs:109-114](../../crates/provider-anthropic/src/request.rs) 的 `ResponseFormat::Json | JsonSchema` 分支仅有注释（「退化为 system 指令」），实际**无任何动作**——既不注入 schema 指令，也不透传到 body，更不报错。用户请求 JsonSchema 时 schema 与 name 被完全丢弃，与 OpenAI（`response_format: json_schema`）/Google（`responseSchema`）行为不对称，且 P6-8 验收「可要求并校验 JSON 结构化输出」对 Anthropic 实际未达成。**建议**：至少注入一条 system/tool 约束把 schema 喂给模型，或在 `ModelCapabilities` 标注 Anthropic 此模型不支持后由上层回退；不应静默。

#### V4 [功能完整性·中] OAuth auto-refresh 未接线 + refresh 轮换不回写

`needs_refresh` / `refresh_access_token` / `store_oauth_token` / `resolve_oauth_credential` 在 auth-service 之外**零消费者**（`rg` 全仓确认）。P6-4 步骤 2「auto refresh — token 自动续期」只交付了原语，没有在任何请求路径前置刷新检查；`refresh_access_token`（[oauth.rs:307](../../crates/auth-service/src/oauth.rs)）可能返回轮换的新 refresh token，但无函数将其回写 backend，轮换型 Provider 会在下一次刷新失败。**建议**：在 provider 构造/请求前置处接入「检查 `needs_refresh` → 刷新 → 回写 access/refresh → 更新 `expires_at`」的编排（可放 app-service，Phase 13 顺手完成），并补 `update_oauth_token` 写回函数与对应测试。

#### V5 [健壮性·低] Anthropic cache_control 标注每条 user 消息

`cache_enabled` 默认为真（`PromptCachePreference::Automatic`），[request.rs:192-195](../../crates/provider-anthropic/src/request.rs) 在 `message_to_anthropic`（逐条消息调用）内对**每条** role=user 消息的末 block 加 `cache_control`。多轮长对话会累积远超 Anthropic 缓存断点上限的标记 → 触发 400。**建议**：仅在「可缓存前缀」的稳定边界（system、首个稳定 user turn、工具定义末尾）标记，或受断点计数约束；参考 Anthropic 当前断点上限动态钳制。

#### V6 [健壮性·低] OAuth 回调服务器单次读取 + redirect_uri 未绑定监听

[oauth.rs](../../crates/auth-service/src/oauth.rs) `CallbackServer`：`handle_callback_connection` 只做一次 `read(&mut [0u8;4096])`，浏览器回调若分片或携带大 cookie 可能解析不全；`PkceFlowConfig.redirect_uri` 是独立字符串，未与 `local_addr()` 校验一致，配置错误时仍生成指向错误地址的授权 URL。**建议**：循环读到请求头结束或限长后解析；`start()` 用实际绑定端口回填 redirect_uri 或校验一致。

#### V7 [安全·低] PKCE verifier 取模偏差

[oauth.rs:86](../../crates/auth-service/src/oauth.rs) `UNRESERVED[(*b % 66)]`，66 非 256 因数，存在轻微偏差。不降低实际熵（64×log2(66)≈390 bit），无可利用性，但若长期手写建议改拒绝采样或 `base64url(rand 48B)`，并加一条均匀性属性测试。

#### V8 [健壮性·低] Gemini 工具调用 id 为合成序号

[stream.rs](../../crates/provider-google/src/stream.rs) `chunk_to_events` 用 `call-{tool_counter}`（`call-0`/`call-1`…）作为 ToolCallId，因 Gemini 不在响应中返回调用 id。后续 tool result 回填只能靠顺序/名称匹配，多工具并发或重放场景下 id 不稳定。**建议**：在 `ModelResponseSummary.provider_metadata` 或 ToolCall 元数据中保留 Gemini 原始顺序，由上层在回写 functionResponse 时按 name 对齐，避免依赖合成 id 跨轮稳定。

### 6. 优化建议（按优先级）

#### P0（建议尽快处理）

1. **V2**：Anthropic thinking budget 钳制到 `< max_tokens` + 默认值重算；补一条触网 mock 校验（断言 `budget_tokens < max_tokens`）。
2. **V3**：Anthropic 结构化输出至少注入 schema 指令或显式不支持，消除静默丢弃。
3. **V1**：Google key 改 `x-goog-api-key` 头，移出 URL query。

#### P1（近期排期）

4. **V4**：补 OAuth auto-refresh 编排 + `update_oauth_token` 回写，并加「刷新后轮换 token 被持久化」的契约测试；明确 P6-4 验收口径（库完成 vs 端到端）。
5. **基线清理**（§4）：决策 oauth2 去留（建议移除并补自实现说明）、回填 base64/rand/sha2/url、同步 ROADMAP 基线表。
6. **V5**：收敛 Anthropic `cache_control` 标注点，受断点上限约束。

#### P2（顺手/评估项）

7. **V6/V7/V8**：回调服务器读取与 redirect_uri 绑定；PKCE 均匀性；Gemini 工具 id 稳定性。
8. **内置模型目录新鲜度**：三家 `builtin_models()` 硬编码（OpenAI 含 o1/gpt-4o、Anthropic 仅 claude-3.5/3、Gemini 至 2.5）。模型迭代快，建议把目录外置为可更新数据或补一个远端 `/models`（带能力探测）的渐进路径，避免目录与线上脱节。
9. **list_models 全静态**：三家均返回内置目录、`models_url()` 标 `#[allow(dead_code)]`（Anthropic）。评估是否提供「远端目录 + 能力推断」开关，至少用于发现新模型。
10. **provider_options 语义统一**：Anthropic 与 OpenAI 把 `provider_options` 合并到顶层、Google 合并到 `generationConfig`（各自正确），但「同名覆盖 canonical」的语义只在 OpenAI 注释中写明（[request.rs](../../crates/provider-openai-compatible/src/request.rs)）。建议在 provider-api 文档统一声明该「覆盖」语义，避免上游误用。

### 7. 附录：相关「优先级 P1」与遗留项

| 事项 | 状态 | 说明 |
| --- | --- | --- |
| P9-7 MCP OAuth | ⚪ 未开始 | 复用本阶段 `auth-service` OAuth primitives；开工前确认 callback 服务器复用与 redirect_uri 一致性（V6） |
| agent-api 职责边界 | 遗留 | ROADMAP 遗留项；Phase 6 不涉及，Phase 13 前评估 |
| provider-bedrock / provider-mistral | 遗留 | workspace-layout 已登记但无任务；与本阶段三家原生适配同构，启动时补任务 |

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V2/V3/V1 立项（正确性 + 安全优先，改动面集中在三个 provider crate）。
2. V4 的 OAuth 接线方案讨论（落点在 app-service 还是 provider 构造），并据此最终判定 P6-4 验收。
3. 基线清理小任务（§4），一次提交完成。
4. 决定 oauth2 crate 去留（建议移除 + 文档化自实现理由 + 补 §3.3 三缺口）。
5. 内置模型目录的更新机制评估（§6 P2.8）。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 9 个 Phase-6 相关 crate 的测试与静态门禁；ADR-002 解耦红线经全仓 `rg` 验证。文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*

---

## 修复记录（review-remediation）

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 交付成熟度：Implemented · 依赖：P6-1 ~ P6-9

**最终目的**：消除 [REVIEW.md](../../REVIEW.md) §6（Phase 6）评审发现的安全与正确性缺陷、OAuth 未接线与基线/文档漂移——让 Google API key 不进 URL、Anthropic thinking budget 与 max_tokens 不冲突、结构化输出不静默丢弃、OAuth auto-refresh 与 refresh token 轮换回写进入请求路径，并按评审结论处置 `oauth2` 基线虚置。

**涉及范围**：`provider-google`、`provider-anthropic`、`provider-openai`/`provider-openai-compatible`、`auth-service`、根 `Cargo.toml`、ROADMAP「依赖选型基线」、`docs/features/providers.md`

### 细分步骤（分组）

#### A. 安全与正确性（V1 / V2 / V3）

1. **V1 Google key 出 URL**：`provider-google` 把 API key 从 `?key=` query 改为 `x-goog-api-key` 请求头，URL 移除 key。目的：secret 不进代理/服务端日志与重定向面，与 Anthropic/OpenAI「头携带 secret」一致。
2. **V2 Anthropic thinking budget 钳制**：构造请求体时将 `thinking.budget_tokens` 钳制为 `< max_tokens`（留余量），并对「未显式设 max_output_tokens 但开 thinking」补默认提升或告警；补触网 mock 断言 `budget_tokens < max_tokens`。目的：默认 max（4096）+ High（8192）不再被 API 400 拒绝。
3. **V3 Anthropic 结构化输出**：`ResponseFormat::Json | JsonSchema` 至少注入一条 system/tool 约束把 schema 喂给模型，或在 `ModelCapabilities` 标注不支持后由上层回退，不再静默丢弃。目的：与 OpenAI/Google 行为对称，P6-8 验收对 Anthropic 真正达成。

#### B. OAuth 接线（V4）

4. **V4 auto-refresh 编排**：在 provider 构造/请求前置处接入「检查 `needs_refresh` → 刷新 → 回写 access/refresh → 更新 `expires_at`」，补 `update_oauth_token` 写回函数与「刷新后轮换 token 被持久化」契约测试。目的：P6-4「auto refresh」从原语升级为端到端，轮换型 Provider 不在下次刷新失败。

#### C. 健壮性（V5 / V6 / V7 / V8）

5. **V5 cache_control 收敛**：仅在可缓存前缀的稳定边界（system、首个稳定 user turn、工具定义末尾）标记 `cache_control`，受 Anthropic 断点上限约束。目的：多轮长对话不累积超限标记触发 400。
6. **V6 回调服务器/redirect_uri**：`CallbackServer` 循环读到请求头结束或限长后解析；`start()` 用实际绑定端口回填/校验 `redirect_uri`。目的：分片/大 cookie 不解析失败，配置错误不生成错误授权 URL。
7. **V7 PKCE 均匀性**：verifier 生成改拒绝采样或 `base64url(rand 48B)`，补均匀性属性测试。目的：消除 `*b % 66` 取模偏差。
8. **V8 Gemini 工具 id 稳定性**：在 `provider_metadata`/ToolCall 元数据保留 Gemini 原始顺序，回写 functionResponse 时按 name 对齐，不依赖合成 id 跨轮稳定。目的：多工具并发/重放场景 id 稳定。

#### D. 基线/包清理

9. **oauth2 决策**：按评审结论维持手写 OAuth，移除根 `Cargo.toml` 的 `oauth2 = "5"`（零引用），在基线与 plan 补「手写自实现理由」说明。目的：基线不再虚置。
10. **回填**：根 `Cargo.toml` 回填 `base64`/`rand`/`sha2`/`url`（OAuth 手写引入，均未登记），同步 ROADMAP 基线表。目的：基线一致。

#### E. 文档漂移

11. **providers.md 语义**：补 `include_usage`（随 P2-12 V3）、stop reason 语义、Anthropic 结构化输出（V3）说明；provider_options「覆盖 canonical」语义在 provider-api 文档统一声明。目的：文档与实现一致，避免上游误用。
12. **内置目录新鲜度**：标注三家 `builtin_models()` 数据日期，建立目录更新跟踪项（不在此任务实现远端 `/models`）。目的：目录与线上脱节可见。

#### F. 安全复审收口

13. **OAuth secret 与 callback**：为 `TokenSet`、PKCE session、Device Flow 临时凭据提供脱敏 `Debug`；callback 使用固定 `text/plain` 文案且不回显 query。目的：明文 secret 不进日志，loopback 回调不形成反射注入面。
14. **刷新一致性**：refresh 响应缺少 `expires_in` 时保留既有到期策略；同一 credential 的并发请求使用 singleflight gate，共享一次刷新结果。目的：避免误判为永不过期及轮换 refresh token 的并发消费竞态。
15. **Provider options 约束**：Anthropic 的 `max_tokens` / `thinking` / `temperature` / `stop_sequences` 纳入保留字段，不能覆盖 canonical 映射与 thinking clamp。目的：透传选项不能绕过安全/正确性约束。

### 主要产出物

- Google key 改头；Anthropic thinking budget 钳制 + 结构化输出注入；OAuth auto-refresh 编排 + 轮换回写 + 契约测试
- cache_control 收敛；回调服务器/redirect_uri；PKCE 均匀性；Gemini 工具 id 稳定性
- OAuth 临时 secret Debug 脱敏、callback 固定文本响应、refresh singleflight/TTL 保留；Anthropic canonical 字段防覆盖
- oauth2 移除 + 手写说明；base64/rand/sha2/url 回填；providers.md 语义补全 + 目录数据日期

### 验收标准（保留 REVIEW 追踪编号）

- [x] **V1**：Google 请求 key 在 `x-goog-api-key` 头、URL 不含 key（契约断言）
- [x] **V2**：`thinking.budget_tokens < max_tokens` 恒成立（含默认值，触网 mock 断言）
- [x] **V3**：Anthropic 结构化输出注入 schema 指令或显式不支持（不再静默丢弃，测试）
- [x] **V4**：请求前置触发 auto-refresh；轮换的 refresh token 被回写持久化（契约测试）
- [x] **V5**：多轮长对话 cache_control 标记不超 Anthropic 断点上限（用例）
- [x] **V6**：回调分片/大 cookie 可完整解析；redirect_uri 与监听端口一致（测试）
- [x] **V7**：PKCE verifier 生成无取模偏差（base64url 48B 随机输入属性测试）
- [x] **V8**：Gemini functionResponse 按 name/顺序对齐，不依赖合成 id 跨轮稳定（多工具测试）
- [x] **基线**：`oauth2` 移除并补自实现说明；`base64`/`rand`/`sha2`/`url` 回填，ROADMAP 基线表同步
- [x] **文档**：providers.md 含 include_usage/stop reason/结构化输出/provider_options 覆盖语义；内置目录标注数据日期
- [x] **安全复审**：OAuth 临时 secret 不出现在 Debug；callback 不反射 query；缺 TTL 不清空过期时间；并发刷新只交换一次；Anthropic canonical 字段不可被 options 覆盖
- [x] **快速验证**：只运行发生变化的 Provider/OAuth 路径与安全契约子集；仅在 schema 变化时检查生成物，完整三家 modern contract 由 P15-9 集中执行

**相关文档**：[REVIEW.md](../../REVIEW.md) §6 · [ADR-002 Agent Engine 与 Provider 解耦](../../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Provider 契约测试](../../docs/adr/ADR-015-provider-contract-tests.md) · [providers](../../docs/features/providers.md)

> 基线决策（2026-08 review）：手写 OAuth 在「PKCE + token 交换 + Device Flow」子集质量合格（S256 经 RFC 7636 测试向量验证），维持手写、移除 `oauth2`；前提是补齐 V4 三缺口（auto-refresh 接线、轮换回写、回调健壮性）。
