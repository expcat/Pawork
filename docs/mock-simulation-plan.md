# 本地 Provider 模拟仿真系列任务规划（MOCK-1～MOCK-8）

> 基线日期：2026-09-09；基线分支 `main`。本文是「本地模拟大模型响应」系列的规划文档与接口事实清单，目标是让 Core 与 GUI 在本地连接模拟程序完成登录、额度、模型目录、流式对话的全流程测试，不调用真实大模型，避免频繁错误、误触限流与成本消耗；同批纳入测试优化（合并相似、删除低效冗余）与快速门禁两项，减少测试时间、提高测试效果。接口事实由四个并行侦察代理对照当前源码核实（出处逐条标注），冲突时以源码为准。

## 1. 目标与非目标

**目标**：一个本地 mock server + 一套录制/注入工具，覆盖程序当前实际使用的全部外部接口。测试时 Host/Desktop 经配置切到 mock，即可仿真真实场景：OAuth 登录、API key 写前验证、额度查询、模型目录拉取、Chat Completions / Responses / Messages 三种流式对话、错误与限流归一。固定响应内容优先来自真实 API 录制（保存后少许修改），其次按 wiremock 契约测试形状合成。

**非目标**：不替代真实 Provider 冒烟与真实模型口径（[verification.md §2.1](spec/verification.md) 的 `opencode-go / glm-5.3-flash` 约定仍然约束「真实验证」语义；mock 证据不得写成真实冒烟）；不改 agent loop / engine 行为；不引入新的生产依赖；不做通用 LLM 网关；不设置全量门禁（维持仓库现状），不补历史覆盖率。

**可测账号口径**（用户确认当前可用）：xAI OAuth、OpenCode Go（API key）、GLM Coding Plan（API key）、Kimi Coding Plan（`kimi-code`，OAuth）、阿里 Coding Plan（`qwen-token-plan`，API key）、DeepSeek（API key）。ChatGPT、Anthropic 与 kimi-platform 无可用账号，其 fixture 按契约测试形状合成并如实标注「未经真实录制」，优先级最低。

## 2. 接口事实清单（mock 必须覆盖的全部端点）

### 2.1 OAuth 端点（3 通道；预设见 [channels/registry.rs](../crates/providers/src/channels/registry.rs)，实现见 [auth/oauth.rs](../crates/auth/src/oauth.rs)）

| 通道 | 流程 | 端点 | 说明 |
| --- | --- | --- | --- |
| xai | Device Flow | `POST https://auth.x.ai/oauth2/device/code`、`POST https://auth.x.ai/oauth2/token` | device/code 响应取 `device_code/user_code/verification_uri(_complete)/expires_in/interval`；token 轮询兼容 `authorization_pending` / `slow_down` / `expired_token`；refresh 复用 token 端点 |
| kimi-code | Device Flow | `POST https://auth.kimi.com/api/oauth/device_authorization`、`POST https://auth.kimi.com/api/oauth/token` | 与 xAI 共用同一实现，仅端点与 client_id/scope 不同 |
| chatgpt | PKCE | `POST https://auth.openai.com/oauth/token`（唯一出站） | authorize 页与 `localhost:1455/auth/callback` 回调在浏览器/本机侧，程序不发 authorize 请求；无 userinfo 端点，account_id 从 id_token JWT claim 本地提取（不验签） |

token 端点共用形状：form-urlencoded 请求；响应 JSON 出现 `error` 字段即失败（HTTP 200 也算）；成功必填 `access_token`，可选 `refresh_token/id_token/expires_in/scope/token_type`（缺省 Bearer）；refresh 响应缺 `refresh_token` 保留旧值、缺 `expires_in` 保留旧到期时间。

### 2.2 业务端点矩阵（默认 base_url 见 [channels/registry.rs](../crates/providers/src/channels/registry.rs)；逐模型 transport 表见 [channels/api_key.rs](../crates/providers/src/channels/api_key.rs)）

| 通道 | 对话（SSE） | 模型目录 | 额度/验证 | 特殊头 |
| --- | --- | --- | --- | --- |
| chatgpt | `POST /responses` | `GET /models?client_version=0.153.0`（`models[].slug`，`visibility != "list"` 剔除） | — | Bearer + `ChatGPT-Account-Id` + `originator: codex_cli_rs` + UA `codex_cli_rs/0.153.0` |
| xai | 按模型路由 `POST /responses`（grok-4 系）或 `POST /chat/completions` | `GET /language-models`（`models[]`，`output_modalities` 须含 `"text"`） | — | Bearer（OAuth 与 API key 同形态） |
| glm-coding | `POST /chat/completions` | `GET /models`（OpenAI `data[]`） | `GET /models`（verify_api_key） | Bearer |
| opencode-go | 逐模型表：grok-4.6 等 → `POST /responses`；glm-\*/kimi-k\*/deepseek-v4-\* 等 → `POST /chat/completions` | `GET /models`（须回表内 Chat/Responses 组 id，Messages-only 与未登记 id 被过滤） | `GET /usage`（三窗，严格校验） | Bearer + `x-opencode-session`（仅此通道） |
| qwen-token-plan | `POST /chat/completions`（表内 10 模型；未登记模型 HTTP 前拒绝） | `GET /models` | `GET /models` | Bearer |
| deepseek | `POST /chat/completions` | `GET /models` | `GET /models` | Bearer |
| kimi-platform | `POST /chat/completions` | `GET /models` | `GET /models` | Bearer |
| kimi-code | `POST /chat/completions`（固定） | `GET /models`（OpenAI `data[]`） | —（纯 OAuth，无 key 验证路径） | Bearer（仅 OAuth） |
| anthropic（transport 基线，非注册表通道） | `POST /v1/messages` | 静态目录，无 HTTP | — | `x-api-key` + `anthropic-version: 2023-06-01` |

补充：所有请求带 `x-trace-id`（有 trace_id 时）与 UA `pawork`；重定向策略 none（跨域 3xx fail-closed）；`has_more: true` 的目录响应一律拒绝（不支持翻页）；空数组合法。

### 2.3 Go 额度 `GET {base}/usage` 精确形状与校验红线（[channels/api_key.rs](../crates/providers/src/channels/api_key.rs)）

`{"usage":{"rolling":{...},"weekly":{...},"monthly":{...}}}`，每窗 `{"status":"ok"|"rate-limited","percent":0-100,"resetsAt":"YYYY-MM-DDTHH:mm:ss.sssZ"}`。红线：顶层 `usage` 必须是 object（否则整体失败）；三窗独立成败；`percent` 必须是整数且 `ok` 仅配 0–99、`rate-limited` 仅配 100；`resetsAt` 严格 24 字符真实日历（含闰年）；错误消息固定 `Go returned an invalid usage response`，不回显上游字段。写前验证要求三窗全成；`rate-limited`（100%）算合法凭证。全仓仅此一个远端额度端点。

### 2.4 三种 transport 的 SSE 形状（样例可直接取自 [tests/contract.rs](../crates/providers/tests/contract.rs)、[tests/chatgpt.rs](../crates/providers/tests/chatgpt.rs)、[tests/xai.rs](../crates/providers/tests/xai.rs)）

- **Chat Completions**：`data: {"choices":[{"delta":{"content":...}}]}`；工具调用首片带 `id+name`、后续片仅 `index+arguments` 增量；`reasoning_content`（兼容 `reasoning`）→ ThinkingDelta；`finish_reason` 与独立 usage chunk；`[DONE]` 哨兵收尾（无 finish_reason 按 Completed；既无 [DONE] 又无 finish → StreamInterrupted）。
- **Responses**：`response.created / output_text.delta / reasoning_summary_text.delta / output_item.added / function_call_arguments.delta / output_item.done / completed / incomplete / failed / error`；非 JSON 事件立即 MalformedResponse 且不可救回；ChatGPT 通道请求体带 `store:false`。
- **Anthropic Messages**：`message_start → content_block_start/delta/stop → message_delta → message_stop`（缺 `message_stop` 即 StreamInterrupted；`ping` 忽略）。

### 2.5 错误归一的两条路径（mock 复现要点，[net/retry.rs](../crates/providers/src/net/retry.rs)、[error_table.rs](../crates/providers/src/error_table.rs)）

1. **HTTP 状态码路径**：401 Authentication、403 Authorization、404 ModelNotFound、408 Timeout、413 ContextTooLarge、429 RateLimited、400 InvalidRequest、451 ContentFiltered、402 QuotaExceeded、500/502/503/504 ProviderUnavailable；`Retry-After`（秒或 IMF-fixdate）仅 retryable 类采纳。错误体被读取但**不解析**（message 恒为 `HTTP <code>`），mock 只需发状态码 + 可选 Retry-After 头。
2. **厂商文案路径**：`normalize_vendor_error` 按 adapter id 扫 needle，命中则改 kind。HTTP `classify_status` 的 message 恒为 `HTTP <code>`，**不把错误体写入 message**，needle 放进 HTTP 错误体无效。Chat Completions SSE（[stream.rs](../crates/providers/src/stream.rs) `chunk_to_events`）只认 `choices`/`usage`，不抽 `error.message`。当前能把厂商原文送进 `ProviderError.message` 的生产路径，是 Responses 流的 `error` / `response.failed` 事件（chatgpt、xAI grok-4 系、opencode-go 的 Responses 模型）。glm-coding / qwen-token-plan 走 Chat Completions，表内 `1113`/`1301`/`throttling`/`quota_exhausted` 等 needle **没有对应 wire 命中路径**；mock 测这两条通道只走 HTTP 状态码路径，不要误发 Responses 事件。

### 2.6 凭证解析链（mock 登录注入依据，[auth/resolve.rs](../crates/auth/src/resolve.rs)、[auth/file_backend.rs](../crates/auth/src/file_backend.rs)）

解析顺序：账号索引选中项 → legacy 主条目 → env `PAWORK_API_KEY_<ID 大写，- 变 _>` → None（fail-closed）。auth 文件 `$PAWORK_HOME/auth.json`（缺省 `~/.pawork/auth.json`），形状 `{"version":1,"entries":{"<service>":{"<account>":"<明文>"}}}`；OAuth 条目为 service `pawork.<provider>.oauth` 下 `default.access` / `default.refresh` / `default.meta`（含 `expires_at_ms`，**必须设远期**，否则每次请求触发 refresh）+ `accounts.meta` 索引。直接 seed 该文件即可跳过真实 OAuth HTTP 流，是「模拟登录」的最小路径。

## 3. 接入方式：三个零代码切点 + 一个缺口

程序从 GUI 到 Provider 的链路为 Desktop → UDS → Host（gui_server）→ app 装配（[provider_assembly.rs](../crates/app/src/provider_assembly.rs)）→ providers adapter → HTTP。mock 在最后一跳之前注入，三个切点均已在生产中支持、互不冲突：

1. **base_url 覆盖（主切点）**：config `[[providers]]` 的 `id + base_url` 对全部通道生效（含 chatgpt/xai/kimi-code），wiremock 测试即用同款 `with_base_url(server.uri())` 范式。**硬限制：base_url 只在 Builtin/Global 层生效，Workspace `.pawork/config.toml` 中写入会被剥离并记 `ProviderBaseUrlIgnored`**（[config/loader.rs](../crates/workspace/src/config/loader.rs)）——mock 注入必须写 Global config（macOS `~/Library/Application Support/dev.pawork.pawork/config.toml`）。
2. **OAuth 端点覆盖**：config `[oauth.<id>]`（`client_id` + `token_url` 必填，Device Flow 加 `device_auth_url`，PKCE 加 `auth_url` + `redirect_uri`），登录 `oauth_begin` 与 refresh 全走覆盖端点。
3. **凭证注入**：API key 通道用 env `PAWORK_API_KEY_<ID>` 或 seed 临时 `auth.json`；OAuth 通道 seed 含远期 `expires_at_ms` 的 auth.json（跳过 OAuth HTTP），或经切点 2 跑完整 mock Device Flow。

实例隔离：`PAWORK_DATA_DIR=<tmp>` + `--instance mock` 得独立 socket/token/session.db；`PAWORK_HOME=<tmp>` 隔离 auth.json；全局 `proxy_url` 对 127.0.0.1 目标自动直连，mock 不受代理影响。

**缺口（决策点 D2）**：Global config 路径由 `directories::ProjectDirs` 推导、无 env 重定向，写 mock base_url 会暂时影响本机所有实例。两个方案：A）脚本备份/恢复 Global config（零代码，但有中途崩溃残留风险）；B）为 global config 增加 env 重定向（如 `PAWORK_CONFIG`，改动约数十行，位于 [config/paths.rs](../crates/workspace/src/config/paths.rs) 与 loader），触碰配置六层语义，按冻结契约流程需用户确认后 golden 先行。规划默认 A 起步、B 作为独立候选任务。

## 4. 任务序列

每任务数小时内可独立完成与验收；写入集收敛；文档/脚本任务不跑 Cargo，代码任务按 [AGENTS.md §5](../AGENTS.md) 定向验证。

### 测试现状盘点（MOCK-7 / MOCK-8 依据）

- providers 测试面：6 个集成测试文件 2285 行 41 个测试（contract.rs 16 个 / 741 行、api_key_channels.rs 9 个 / 582 行、anthropic.rs 11 个 / 568 行、xai.rs / chatgpt.rs / responses.rs 合计 5 个 / 394 行）。feature 门控使全量覆盖需多次 cargo 调用——默认 features 只编译 contract.rs 与 anthropic.rs，chatgpt / xai / responses / api_key_channels 需显式 `--features`，每次调用独立编译链接。
- 形状重复：`sse_body` helper 在 [contract.rs](../crates/providers/tests/contract.rs) 与 [api_key_channels.rs](../crates/providers/tests/api_key_channels.rs) 逐字节重复；文本流 / 工具调用 / usage 的 SSE 样例在 contract、api_key_channels、chatgpt、xai 多处各自手写。
- OAuth 端点 mock 重复：token 端点 wiremock 形状分散在 4 个包 9 个文件、约 20 处 `MockServer::start`（auth / app / gui_host tests / tools mcp）。
- scripts/ 当前没有任何测试门禁入口（仅 UI fixture 与增量清理脚本）。

### MOCK-1 接口盘点与系列规划（本文档）

- 范围：四个侦察代理核实的接口事实 + 本规划。状态：**已实现（文档）**。
- 验收：链接/格式检查；`git diff --check`。

### MOCK-2 真实响应录制工具与 fixture 格式

- 范围：`scripts/mock/capture.py`（stdlib，无新依赖）：按通道用真实凭证请求 models / usage / chat 流式 / responses 流式端点，原始 SSE 字节落盘，脱敏（剥离 Authorization、account id、邮箱等）后存 `fixtures/mock/<channel>/`，附 `meta.json`（录制时间、真实模型 id、端点、transport）。覆盖六条可测通道；chatgpt / anthropic / kimi-platform 无可用账号，fixture 按契约测试形状合成并标注「未经真实录制」。
- 写入集：`scripts/mock/`、`fixtures/mock/`；不动 Cargo。
- 验收：每个通道至少 1 条 models、1 条文本流、1 条工具调用流；Go 加 1 条 usage。用仓库现有 SSE 解析器（或 providers 契约测试同款形状）回放校验形状合法。真实账号操作需用户在场提供凭证或确认已登录态。
- 依赖：MOCK-1。

### MOCK-3 mock server 本体

- 范围：`scripts/mock/server.py`（stdlib `http.server`，线程模式）：单端口加载 fixtures，路由 §2.2 全端点矩阵（含 `/usage`、`/language-models`、`/models?client_version=`）。按 Bearer token 区分通道 persona（如 `mock-glm-coding` / `mock-opencode-go`），解决多通道共享 `/models` 路径的歧义；SSE 按录制字节定速回放（可配 chunk 间隔）；无匹配 fixture 时按通道 transport 回最小合法兜底响应。
- 落点决策（D1）：默认 scripts/ 下 Python stdlib 方案——零 Cargo 变更、不触碰「当前不新增包」红线，仓库已有 python3 脚本先例。若用户偏好 Rust，则备选 `crates/testkit` 增加 bin target（dev-only 定位不变，同批更新 [testkit.md](spec/crates/testkit.md)）。
- 写入集：`scripts/mock/`、`fixtures/mock/`。
- 验收：curl 对照各端点形状；与 §2.3/§2.5 校验红线逐条核对（故意喂畸形响应确认被拒）。
- 依赖：MOCK-2（可先用合成 fixture 并行开发）。

### MOCK-4 场景库：错误、限流、慢流与截断

- 范围：fixture 场景集 + 触发机制。触发用两种互补方式：prompt 关键字（GUI 输入 `MOCK:RATE_LIMIT` 等即可驱动，端到端最真）与 `POST /__control` 切换全局场景（脚本化用）。场景清单：HTTP 401/403/404/408/413/429（含 Retry-After）/400/451/402/500-504 全表；Responses 流内 error 事件携带 §2.5 needle（chatgpt `usage+limit`、xai `insufficient_quota` 各一条，可选 opencode-go Responses 模型一条）；glm-coding / qwen-token-plan 只走 HTTP 状态码路径，不构造 Responses 文案事件；慢流（测 30s 心跳与读超时重置）、流中截断（无 [DONE]）、缺 `message_stop`、usage 独立 chunk、跨 chunk 工具参数分片。
- 写入集：`fixtures/mock/scenarios/`、`scripts/mock/server.py`。
- 验收：脚本驱动 CLI headless 逐场景断言错误归一输出与事件序列。
- 依赖：MOCK-3。

### MOCK-5 OAuth 模拟层与凭证注入工具

- 范围：mock server 增加 §2.1 端点（xai/kimi-code 的 device_authorization + token，chatgpt 的 token；含 `authorization_pending` → 成功 的轮询剧本与 `invalid_grant` 失败剧本）；`scripts/mock/seed_auth.py` 生成含远期 `expires_at_ms` 的 auth.json（API key 与 OAuth 两种形态）到 `$PAWORK_HOME`；config 样例（`[oauth.xai]` 等覆盖）写入 `fixtures/mock/config.example.toml`。
- 写入集：`scripts/mock/`、`fixtures/mock/`。
- 验收：隔离实例经 mock 完成一次完整 device 登录（含 user_code 展示、轮询、落盘）与一次 refresh；seed 路径跳过 HTTP 直接可用。
- 依赖：MOCK-3。

### MOCK-6 端到端接入与真窗口验收

- 范围：`scripts/mock/run-instance.sh` 串起隔离环境（`PAWORK_DATA_DIR` / `PAWORK_HOME` / `--instance mock`）+ Global config base_url 注入（默认备份/恢复方案 A；若 D2 方案 B 获批则改 env 重定向）；GUI 真窗口连接 mock 实例走全流程：登录 → 模型目录 → Go 额度展示 → 流式对话 → 工具调用 → 取消 → 错误展示；CLI headless 同样跑通。结果按 [verification.md §6](spec/verification.md) 证据格式记录，如实标注「mock 环境」。
- 写入集：`scripts/mock/`、本文档状态回写、[capabilities.md](spec/capabilities.md)/[operations.md](spec/operations.md) 各加一段 mock 用法（用户可见运维边界变化，同批更新）。
- 验收：真窗口主路径人工可走通；检查记录与窗口外事实（SQLite、argv）齐全。
- 依赖：MOCK-3～MOCK-5。

### MOCK-7 测试整合与精简（合并相似、删除冗余）

- 范围：SSE/JSON 形状收敛到单一来源——MOCK-3 录制完成后优先引用 `fixtures/mock/`，此前先在 providers tests 内建 common 模块去重（`sse_body`、chat 文本流、工具调用样例）；五通道重复用例参数化（沿用 `api_key_presets()` 表驱动样式）；合并 contract.rs 与 api_key_channels.rs 中等效的 chat 文本流 / 工具调用 / usage 用例；OAuth token 端点 mock 形状收敛为一处 dev-only helper 供 auth / app 共用。
- 删除准则：同一行为已被另一测试等效覆盖（同切面、同断言强度）才删；三类关键回归（安全红线、持久化与重放、协议与解析）不删、不降断言；逐条删除在任务报告中给理由。不补历史覆盖率、不引入新测试框架。
- 写入集：crates/providers/tests/、crates/auth 与 crates/app 的 dev 测试；不动生产代码。
- 验收：`cargo test -p pawork-providers --offline --lib --tests`（带齐 features 的单条调用）+ auth / app 定向命令通过；任务报告给出测试数量与耗时的前后对比。
- 依赖：去重与合并不依赖 mock server，可先行；切换到录制 fixture 依赖 MOCK-3。

### MOCK-8 mock 快速门禁

- 范围：`scripts/mock/gate.sh` 单入口四级：L0 文档链接 / 格式 / `git diff --check`（秒级）→ L1 单条 cargo 命令带齐 features 跑 providers 全套，并按参数追加写入集定向包（遵守单 Cargo 进程纪律）→ L2 启动 mock server 回放冒烟，断言 §2.2 关键端点形状与 §2.3 额度形状（秒级、不触网）→ L3 真实 Provider 冒烟**不入门禁**，保持手动并按 [verification.md §2.1](spec/verification.md) 口径执行。
- 目标：减少测试时间（单次入口、一次编译链接、无真实网络等待）并提高测试效果（形状单一来源、场景库覆盖错误归一）。明确**不是全量门禁**——仓库「当前未设置全量门禁」的约定不变。
- 写入集：`scripts/mock/`。
- 验收：干净工作区一次 `./scripts/mock/gate.sh` 通过并输出分级耗时；故意注入畸形 fixture 时 L2 失败。
- 依赖：MOCK-3；L1 整合依赖 MOCK-7。

### MOCK-0b（候选，需用户确认）：Global config env 重定向

- 范围：`PAWORK_CONFIG`（或等价命名）env 覆盖 global config 路径；golden/契约先行，同批更新 [contracts.md](spec/contracts.md) 配置层级与 [workspace.md](spec/crates/workspace.md)。
- 不获批时的兜底：MOCK-6 用方案 A（备份/恢复脚本），并在脚本内做崩溃后恢复的兜底提示。

## 5. 决策点汇总（实施前需用户拍板）

| ID | 问题 | 默认建议 |
| --- | --- | --- |
| D1 | mock server 落点：scripts/ Python stdlib（默认，零 Cargo 变更）vs testkit bin target（Rust） | Python stdlib |
| D2 | 是否新增 `PAWORK_CONFIG` env 重定向（触碰配置六层语义，golden 先行） | 先备份/恢复脚本，重定向作为候选 |
| D3 | ChatGPT / Anthropic / kimi-platform 无真实账号：fixture 用契约测试形状合成并标注来源 | 同意 |
| D4 | MOCK-7 删除测试的确认方式：逐条列出拟删 / 拟并清单随当批 diff 一起审，不单独请示；三类关键回归不动 | 同意 |

## 6. 验证口径

- 文档与脚本任务：链接/格式/`git diff --check`，不跑 Cargo。
- mock 门禁（MOCK-8）是快速定向门禁：L0/L1/L2 必须全绿才允许任务收口；它不改变「当前未设置全量门禁」的仓库约定，L3 真实冒烟仍手动触发并单独记录。
- mock 链路的测试证据属于「本地仿真」新层级，写入证据记录时必须与 E3（真实 Provider/真窗口）区分表述；[verification.md §2.1](spec/verification.md) 的真实模型口径不变，真实冒烟缺口不因 mock 通过而关闭。
- 触及配置语义（D2）时按三类关键回归中的「协议与解析」处理：配置六层 golden 先行。
- 本系列不新增生产依赖、不改包布局；若实施中出现新增包/新依赖的必要性，先回到本文档登记并经用户确认。

## 7. 实施状态与证据回写（2026-09-10）

| 任务 | 状态 | 说明 |
| --- | --- | --- |
| MOCK-1 接口盘点与规划 | 已实现（文档） | 本文档。 |
| MOCK-2 录制工具与 fixture | 已实现 | [capture.py](../scripts/mock/capture.py) + `fixtures/mock/` 九通道 fixture；无真实账号的通道（chatgpt / anthropic / kimi-platform 等）按契约形状合成并标注来源。 |
| MOCK-3 mock server | 已实现、已验证 | [server.py](../scripts/mock/server.py)（场景与 OAuth 端点内置）+ `server_smoke.py` 全绿。 |
| MOCK-4 场景库 | 已实现、已验证 | `fixtures/mock/scenarios/` + `server_scenarios_smoke.py` 全绿；关键字（`MOCK:RATE_LIMIT` 等）与 `POST /__control` 双触发。CLI headless 场景为抽测（RATE_LIMIT / 401 / 404 / 截断 / QUOTA）；HTTP 层全量 47/47 由 server_scenarios_smoke 覆盖。 |
| MOCK-5 OAuth 模拟层与 seed | 已实现、已验证 | `oauth_selftest.py` 全绿；fixture 覆盖九通道，`run-instance.sh seed` 落盘八通道（API key 5 + OAuth 3，无 anthropic——它只作为 transport fixture，未接入编排的注册通道）；真机完成一次 device 登录（user_code 展示、轮询、落盘）。 |
| MOCK-6 端到端接入与真窗口验收 | 已实现；CLI 全流程已验证，GUI 主路径已验证、取消受阻（BUG-GUI-01） | [run-instance.sh](../scripts/mock/run-instance.sh)（start/run/stop/status/seed/env/desktop）+ `quota_probe.py`；证据见下。 |
| MOCK-7 测试整合与精简 | 已实现、已验证（review 通过） | providers 测试收敛与 OAuth mock helper 去重。 |
| MOCK-8 mock 快速门禁 | 已实现、已验证 | [gate.sh](../scripts/mock/gate.sh) L0/L1/L2；历史阶段收口：L0 0.1s、L1 52.8s（providers 208 测试）、L2 1.4s（冒烟 45 + 47 与 usage 回放）。提交前 L2 另纳入 fixture verify、OAuth 与配置恢复回归，耗时以新运行输出为准。 |
| MOCK-0b Global config env 重定向 | 候选，未获批 | 本系列用方案 A（备份/恢复）兜底，见 §3 缺口。 |

### MOCK-6 证据记录（mock 环境/本地仿真，非 E3 真实 Provider）

```text
Implemented: scripts/mock/run-instance.sh（隔离环境编排 + Global config 方案 A 注入/恢复 +
  host/server/desktop 生命周期）、scripts/mock/quota_probe.py（协议层三窗额度探针）
Validated: CLI headless 全流程——seed 八通道凭证；各通道 models 命中 fixture；opencode-go
  GET /usage 三窗（rolling5h 12% / weekly 34% / monthly 57%）；glm-coding 文本流；xai
  Responses 流；工具流首轮成功；Ctrl-C 取消；MOCK:RATE_LIMIT/HTTP_401/HTTP_404/
  TRUNCATED_CHAT/QUOTA 错误归一展示；OAuth device 登录（user_code E5C0-F958 轮询落盘）。
  GUI 真窗口——连接与 16 会话持久化重放、模型目录与切换、文本流 Run 完成、MOCK:RATE_LIMIT
  错误卡片、Settings 八提供商全部已连接且模型数与 fixture 一致、工具流 scenario 命中。
  ./scripts/mock/gate.sh 全绿（L0 0.1s / L1 52.8s / L2 1.4s，总 54.5s，退出码 0）。
Targeted regressions: gate.sh L0（文档链接/格式/git diff --check）+ L1（providers 全套单 cargo
  进程）+ L2（mock server 回放冒烟）
Real-world evidence: mock 环境/本地仿真（127.0.0.1:8787，fixture 回放）；真窗口为预构建
  Pawork.app 平行 bundle + AX 快照/截图取证；不冒充 E3 真实 Provider 冒烟
Known gaps: 见下「已知缺口」
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

### 已知缺口

1. **GUI Run 事件路径静默断连（BUG-GUI-01）**：Run 启动约 1.5–10 秒连接被静默关闭，6 次复现；阻断真窗口取消主路径与断连期间的设置/额度刷新；CLI 取消已验证。已登记 [backlog.md §8](spec/backlog.md)。
2. **OAuth 请求前刷新缺陷（BUG-OAUTH-01）**：FileBackend 路径 metadata 比较恒真导致 refresh 被静默跳过；判别实验与复验配方见 [backlog.md §8](spec/backlog.md)。mock 环境的 device 登录与轮转不受影响。
3. **usage ledger request-id 跨进程撞车（BUG-USAGE-01）**：同 data dir 第二个 Host 进程跑对话报 `usage record id conflict`，第二程用量不落账；见 [backlog.md §8](spec/backlog.md)。
4. **GUI 三窗额度显示**：协议层已由 `quota_probe.py` 验证（同 socket 同 token 三窗 200）；本机预构建 bundle 早于 API 1.16 的 per-credential 额度 UI（二进制内无 `settings.quota.*` 字符串），设置页仅显示 ADR-056 的「Usage unavailable」诚实空态。待含 V1_16 UI 的 bundle 重建后人工复验。
5. **工具流固定 tool_call_id（mock 限制）**：mock fixture 的 tool_call_id 固定，同一 data dir 第二次工具 Run 撞事件 UNIQUE 约束（CLI 与 GUI 均复现）；首轮工具流正常。属 mock 数据限制，非产品缺陷。
6. **fixture 合成回退**：opencode-go 余额不足、xai token 过期、kimi-code 无凭证等场景的 fixture 为契约形状合成（无真实录制），已在 meta 标注。
7. **launchd 初连挂起**：`run-instance.sh desktop` 以 `open -na`（launchd 托管，env 经 `--env` 传递）封装；该方式启动的实例初次连接常挂起「连接中…」数分钟，直启二进制（带 env）秒连。缺口保留：自动化验收需要秒连时手工直启 `<state>/Pawork-mock.app/Contents/MacOS/Pawork --instance mock`（env 用 `run-instance.sh env`）。
8. **MOCK-0b 未获批**：Global config 注入采用方案 A，注入期间本机所有实例共享 mock base_url；崩溃残留由 `run-instance.sh stop` 按注入记录恢复。
9. **Global config 独占与恢复**：提交前 review 已补文件锁与跨 state 所有权记录，第二个 state 注入会拒绝；配置采用完整临时替换，支持原文已有 `[oauth.*]` 的情况。恢复失败或检测到用户编辑时保留备份与所有权记录，需核对 `<state>/config.backup.json` 后恢复。`stop` 不删除状态目录；移除递归 `clean` 和 Host wrapper 子命令。

### 提交前 review 修正

- 配置编排改为 stdlib Python + shell 入口：原文/权限恢复、跨 state 独占、用户编辑冲突保护、进程身份核对与启动失败清场。`review_selftest.py` 仅操作临时目录，用正常往返与关键失败路径验证；不覆盖真实 Global config。
- 录制工具禁止 HTTP 重定向，避免转发凭证；校验报错不回显敏感匹配内容；修正 kimi-code OAuth service 与 ChatGPT 特殊头；fixture 查找拒绝模型路径穿越和符号链接越界。
- 本节修正不补齐上方已登记的产品/旧 Desktop bundle 验收缺口，历史 GUI 证据不等于本次重新验收。

Validated（提交前）：`gate.sh --packages pawork-auth,pawork-app` 的 L1 完成 providers 208、auth 79（1 ignored）、app 243 项测试，共 530 passed；期间更新脚本导致后续 shell 读取中断，固定脚本后以 `gate.sh --level 0,2` 单独复验通过（L0 0.3s / L2 5.0s）。L2 含 fixture verify 9/9、OAuth 17、review 回归 2、server 45、scenarios 47 与 usage 回放。`git diff --cached --check` 通过；SSE 必需的末尾空行由 fixture 目录 `.gitattributes` 显式保留。Full workspace gate: NOT RUN。
