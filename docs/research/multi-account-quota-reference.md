# 多账户额度管理、切换与输入缓存 — 开源项目参考说明

> 调研日期 **2026-08-14**。目的：为 Pawork V2 规划「多账户间额度控制与切换、子智能体（sub-agent）跨供应商调用、输入/提示缓存（prompt caching）策略控制」提供外部实现逻辑参考。
>
> 配套文档：[multi-account-quota-proposals.md](multi-account-quota-proposals.md)（实施方案与推荐，**待确认**）· [../design.md](../design.md) §5（候选登记 G1–G7）。
>
> 调研方式：5 路并行子代理（web 检索 + 官方文档/源码抓取，含 2026-08-14 流行度与新兴项目补充调研）产出分域报告，主代理独立复核项目身份、关键机制与 star 抽样后综合成文。所有论断附来源链接；无法核实处标「未证实」。内容为撰写时点快照，实现前应复核最新实态。

---

## 1. 项目总览

| 项目 | 仓库 | 形态 | 与本调研相关的核心机制 |
| --- | --- | --- | --- |
| OpenCode | [anomalyco/opencode](https://github.com/anomalyco/opencode)（原 sst/opencode） | TypeScript/Bun 终端 Coding Agent | models.dev 模型目录、`task` 工具子代理（子 session + 权限派生）、Anthropic 缓存断点、429 重试策略 |
| Pi | [earendil-works/pi](https://github.com/earendil-works/pi)（原 badlogic/pi-mono，2026-05 迁移） | TypeScript monorepo（pi-ai / pi-agent-core / pi-coding-agent） | provider 无关 Context + 跨厂商 handoff、订阅 OAuth 全线可用、精细缓存断点与长 TTL、扩展式子代理 |
| opencodex | [lidge-jun/opencodex](https://github.com/lidge-jun/opencodex)（npm `@bitkyc08/opencodex`，命令 `ocx`，文档站 [opencodex.me](https://opencodex.me)） | 本地代理守护进程 + Web dashboard（Bun，默认端口 10100） | Codex Responses 协议翻译（40+ provider）、**ChatGPT 账户池**（5h/周/30d 三窗口配额路由 + 线程亲和） |
| cc-switch | [farion1231/cc-switch](https://github.com/farion1231/cc-switch)（官网 ccswitch.io） | Tauri 桌面应用（另有 Web/CLI 形态） | **配置级**供应商/账户切换（SSOT SQLite → 原子写回各工具 live 配置文件）、本地代理模式下的故障转移 |
| CLIProxyAPI | [router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)（原 luispater） | Go 守护进程（默认端口 8317） | 多 OAuth 订阅账户封装为兼容 API、round-robin/加权/fill-first、429 指数退避冷却、session-affinity |
| claude-code-router（CCR） | [musistudio/claude-code-router](https://github.com/musistudio/claude-code-router) | 本地网关 | 场景化路由（default/background/think/longContext）、子代理标签路由、transformer 体系（含 `cleancache`） |
| LiteLLM | [BerriAI/litellm](https://github.com/BerriAI/litellm) | Proxy/Router | 层级预算（org/team/user/key）、TPM/RPM 限流、6 种路由策略、cooldown/fallback、缓存差价计费 |
| new-api | [QuantumNous/new-api](https://github.com/QuantumNous/new-api)（承自 one-api，其上游已于 2026-01 停更） | 计费网关 | 渠道-账户-令牌三层 quota 折算、渠道优先级/权重、渠道内多 key 轮询、失败自动禁用与换渠道重试 |
| claude-relay-service（CRS） | [Wei-Shaw/claude-relay-service](https://github.com/Wei-Shaw/claude-relay-service) | Claude 订阅账户池中继 | **内容 hash sticky session 保 prompt cache**、429/529 标记排除、每账户独立代理 IP |
| gpt-load | [tbphp/gpt-load](https://github.com/tbphp/gpt-load) | Go key 池透明代理 | 累计失败拉黑 + 定时验证恢复、failover 状态码可配置 |
| 新兴项目（2025–2026） | claude-code-hub、CLIProxyAPI-Plus、meridian、antigravity-claude-proxy | 见 §4.6 | 账户池 + sticky session 已成标配设计 |
| Codex Router | [duolahypercho/codex-router](https://github.com/duolahypercho/codex-router) | 本地路由器 + 托盘（JS / LiteLLM，默认 127.0.0.1:4202） | 一安装多客户端（Codex / DeepSeek Harness / Gemini CLI）；凭证隔离转发；额度耗尽换模型（非账户池 sticky） |

**同名项目辨析**（避免张冠李戴）：

- `opencodex` 另有两个不相关同名项目：[ymichael/open-codex](https://github.com/ymichael/open-codex)（Codex CLI 多 Provider fork，改用 Chat Completions，疑似停更，未证实）与 [codingmoh/open-codex](https://github.com/codingmoh/open-codex)（Python 本地模型 CLI，与本主题无关）。本文所述均指 lidge-jun/opencodex。
- `codex-router` 指 [duolahypercho/codex-router](https://github.com/duolahypercho/codex-router)（2026-08-18 按公开 README 登记，约 2.4k stars），与 musistudio/claude-code-router、lidge-jun/opencodex 均不是同一项目。
- `ccswitch` 另有基于 CLIProxyAPI 的包装 CLI（如 kaitranntt/ccs）；本文所述均指 farion1231/cc-switch。

全部参考项目（含本表之外 2026-08 补充的流行项目）的功能对照总表见 §8。

---

## 2. 多供应商 Coding Agent（OpenCode / Pi）

### 2.1 OpenCode

**多 Provider / 多账户**

- 模型目录：基于 [models.dev](https://models.dev) 中心注册表（同团队维护）+ Vercel AI SDK，声称 75+ provider。发现顺序：models.dev → 已装 plugin 注册 → `opencode.json` 自定义（`provider.<id>` 指定 `npm` 包、`baseURL`、`models`）。来源：[providers 文档](https://opencode.ai/docs/providers/)。
- 凭证存储：`opencode auth login` 写 `~/.local/share/opencode/auth.json`，**按 providerID 单条凭证**，三类：`api`（明文 key）/`oauth`（access/refresh/expires，自动刷新）/`wellknown`。
- 订阅登录现状：Anthropic 2026-01 起技术封锁第三方使用 Claude Pro/Max OAuth token，OpenCode 2026-03 应法律要求移除内置 Anthropic OAuth（[PR #18186](https://github.com/anomalyco/opencode/pull/18186)）；仍零配置支持 ChatGPT Plus/Pro（浏览器 OAuth）、GitHub Copilot（device-code）、GitLab Duo；另有自营网关 OpenCode Zen（按量）与 OpenCode Go（月费订阅）。
- 同 Provider 多账户：**原生不支持**——重复登录即覆盖（开放 issue [#5391](https://github.com/anomalyco/opencode/issues/5391)、[#6217](https://github.com/sst/opencode/issues/6217)）。绕法：API key 型在 config 定义别名 provider；OAuth 型换 `XDG_DATA_HOME` 或社区插件（opencode-anthropic-profiles、opencode-claude-multiauth）。
- 用量统计：每条 assistant message 落库 `cost` + `tokens{input,output,reasoning,cache{read,write}}`（SQLite 列），按 models.dev 单价计成本，cache read/write 单独计价（`packages/opencode/src/session/session.ts`）。
- 429 行为（`packages/opencode/src/session/retry.ts`）：最多 5 次重试，指数退避（初始 2s、倍率 2、jitter 0.25），优先遵循 `retry-after-ms`/`retry-after` 头（支持秒/HTTP 日期），无头封顶 30s；context overflow 不重试而走自动 compaction；**不自动切换 model/账户**。

**子代理**

- 统一 `agent` 概念，`mode: primary | subagent | all`；JSON（`opencode.json` `agent` 段）或 Markdown（`~/.config/opencode/agents/*.md`，frontmatter 承载字段）。字段：`description`（必填）、`model`（`provider/model-id`）、`prompt`、`temperature/top_p`、`steps`、`permission`（read/edit/bash/task/webfetch 每项 allow/ask/deny）、`hidden`。来源：[agents 文档](https://opencode.ai/docs/agents/)。
- 派发机制（`packages/opencode/src/tool/task.ts`）：主 agent 调 `task` 工具 → 创建子 session（`parentID` 指向父，**权限从父 + 子 agent 派生**，默认 deny 子的 todowrite/task）；`subagent_depth` 限制嵌套（默认 1）；`task_id` 可复用子 session 续跑。
- 模型绑定：子 agent 可配独立 `model`（任意 provider），未配则继承父消息的 providerID/modelID；**账户随 provider 全局凭证，无 per-agent 账户**。
- 并发/回传：前台阻塞等待，取子 session 最后一条 text part 回传；`background=true`（实验性环境变量门控）异步执行，完成后以 synthetic user part 注入父 session。

**Prompt caching**

- Anthropic 系显式管理（`packages/opencode/src/provider/transform.ts` `applyCaching`）：断点打在**前 2 条 system + 最后 2 条非 system 消息**（至多 4 个 `cache_control: {type:"ephemeral"}`，滚动前移）；未见 1h TTL 管理。
- OpenAI 系：不打断点，自动缓存 + **`prompt_cache_key = sessionID`**（openai/azure/xai/mistral 等）做会话亲和。
- 前缀稳定性工程（[PR #14203](https://github.com/anomalyco/opencode/pull/14203)、[PR #14743](https://github.com/anomalyco/opencode/pull/14743)、[PR #29949](https://github.com/anomalyco/opencode/pull/29949)）：系统提示拆「静态 header + 动态 rest」两块（provider prompt/全局 AGENTS.md 进静态，env/项目 AGENTS.md 进动态）；修复工具 schema 含 per-repo cwd、技能枚举排序非确定等「前缀污染」。

**切换与会话连续性**：会话中途 `/models` 随时换 model/provider，每条消息落库自己的 `modelID/providerID`；发请求时 transform 层对整段历史按目标 provider 归一化（不支持的 part 替换为占位文本、reasoning 键重映射、缓存断点重打）。**无缓存失效补偿**——换 provider 即冷启动重建缓存。

### 2.2 Pi

**多 Provider / 多账户**

- 模型目录：**自维护内置目录**（不用 models.dev）：`packages/ai/src/providers/*.models.ts` 生成式元数据（价格、context、reasoning 能力、compat 标志），缓存到 `~/.pi/agent/models-store.json`；用户扩展走 `~/.pi/agent/models.json`（4 种 API 形态：openai-completions / openai-responses / anthropic-messages / google-generative-ai，可覆写 baseUrl、`modelOverrides` 改价格/context）。来源：[docs/providers.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/providers.md)、[docs/models.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md)。
- 凭证：`/login` 写 `~/.pi/agent/auth.json`（0600），按 providerID 单条；`key` 支持 `!command`（keychain/1Password）、`$ENV` 插值、字面量。解析优先级：CLI `--api-key` > auth.json > 环境变量 > models.json。
- OAuth 订阅：内置 ChatGPT Plus/Pro（Codex，[OpenAI 官方背书](https://developers.openai.com/community/codex-for-oss)）、**Claude Pro/Max 仍可用**（2026 年起走 Anthropic「extra usage」按 token 计费）、GitHub Copilot、xAI、OpenRouter（PKCE）。Anthropic OAuth 实现为 **Claude Code 伪装模式**（`packages/ai/src/api/anthropic-messages.ts`：注入 "You are Claude Code" system 块、工具名重映射、模拟 claude-cli UA/beta headers）。
- 多账户：原生不支持同 provider 多账户；官方模式是把不同计划/区域拆成**独立 providerID**（如 `zai` vs `zai-coding-cn`），或换 `PI_CODING_AGENT_DIR`。
- 用量：流事件采集 `input/output/cacheRead/cacheWrite(/cacheWrite1h)`，按模型四元单价（+分级 tiers）计成本；TUI footer 实时显示缓存命中率与累计成本。
- 429：SDK 层重试默认禁用（曾因 SDK 睡满多天 Retry-After 出 bug，[#3671](https://github.com/earendil-works/pi/issues/3671)、[#6911](https://github.com/earendil-works/pi/issues/6911)）；agent 层 auto-retry（[#157](https://github.com/earendil-works/pi/issues/157)）：429/5xx 指数退避 2s/4s/8s 默认 3 次、Esc 可中断；配额/余额耗尽判为终态不重试。**不自动降级/换 provider**。

**子代理**

- **哲学：核心不内置**。README 明言 "No sub-agents"——官方立场是 tmux 开多实例、或用 extension 自建（[Philosophy](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md)）。
- 官方示例 `examples/extensions/subagent/`：extension 经 `pi.registerTool()` 注册派发工具，内部用 SDK（`createAgentSession()` + `SessionManager.inMemory()`）起隔离子会话；pi-ai 的 provider 无关性使子会话可绑任意 provider/model（账户仍取全局 auth.json）。无内置权限/并发框架。相关还有 `handoff.ts`（转移上下文到新聚焦 session）。

**Prompt caching**

- Anthropic 显式断点（`anthropic-messages.ts`）：(a) system prompt 块、(b) 最后一个 tool 定义、(c) 最后一条 user message 的最后一个 block——共 3~4 个断点随对话滚动。**TTL 显式管理**：`PI_CACHE_RETENTION=long` 时发 `cache_control.ttl: "1h"`（OpenAI 则 `prompt_cache_retention: "24h"`）。
- OpenAI/其它：自动缓存 + 会话亲和体系：`prompt_cache_key`、`session_id`/`x-session-affinity`/`x-client-request-id` 头按 provider 自适配（`sessionAffinityFormat`）；OpenAI 兼容代理可用 `compat.cacheControlFormat: "anthropic"` 打 Anthropic 风格断点；Bedrock 自动 cachePoint。
- cacheRead/cacheWrite（含 1h 写入）单独计价入账，footer 展示命中率。

**切换与会话连续性**：跨厂商 handoff 是一等能力（[pi-ai README](https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)）：Context 为 provider 无关格式（JSONL 树形 session，`id/parentId` 原地分支）；发往新 provider 时自动转换——同 provider 的 assistant 消息保留原生结构（thinking 签名可回放），**异 provider 的 thinking 块降级为 `<thinking>` 标签文本**；配套 compat 矩阵处理各家怪癖。缓存失效无特殊补偿（切换即冷启动）。

### 2.3 两项目对比要点

| 维度 | OpenCode | Pi |
| --- | --- | --- |
| 模型目录 | 外置 models.dev 注册表 + AI SDK 包 | 仓内生成式目录 + models-store.json 缓存 + models.json 扩展 |
| 订阅 OAuth | Claude 已被禁并移除；ChatGPT/Copilot 内置 | 全线可用（Claude 走 extra-usage 计费，Claude Code 伪装实现） |
| 同 Provider 多账户 | 不原生支持（插件/改 XDG 目录绕行） | 不原生支持（拆 providerID/改配置目录绕行） |
| 子代理 | 内置 task 工具 + 子 session + 权限派生 + 深度限制 | 核心零内置，extension/SDK 自建（官方示例） |
| Anthropic 缓存断点 | 前 2 system + 末 2 消息（≤4 断点），无 1h TTL | system + 末 tool + 末 user（3~4 断点），支持 1h/24h 长 TTL |
| OpenAI 缓存 | `prompt_cache_key = sessionID` | prompt_cache_key + 多种 session-affinity 头 |
| 429 | ≤5 次重试、遵循 Retry-After、封顶 30s | agent 层 3 次可中断重试；终态配额不重试 |
| 换 provider 连续性 | transform 层按目标 SDK 归一化历史 | pi-ai 一等 handoff（thinking 降级文本） |

共同点：缓存 token 全额计入成本、按消息级落库；都**不做 429 自动跨 provider/账户降级**；都靠「前缀追加 + 固定断点」维持缓存命中；**同 provider 多账户都是空白**（外部代理/配置切换工具因此存在）。

---

## 3. 账户额度管理与切换工具（opencodex / cc-switch / CLIProxyAPI）

### 3.1 opencodex（lidge-jun/opencodex）

- **定位与形态**：本地代理守护进程（Bun，端口 10100）+ Web dashboard + `ocx` CLI；可注册为系统服务或按需 shim 启动。把 Codex 的 Responses API 翻译到任意上游协议（Anthropic Messages/Gemini/Azure/OpenAI-compatible），也向 Claude Code 提供 `/v1/messages` 网关。核心是七个协议适配器组成的 parser → router → adapter → bridge 管线，`router.ts` 用七层优先级把模型 id 映射到 provider（[第三方源码解读](https://wangruofeng007.com/blog/2026-07/opencodex-codex-claude-any-llm/)）。
- **注入方式（Design B）**：对 Codex 只改 `~/.codex/config.toml` 的 `openai_base_url` 一个字段、不替换 provider 标签，对话历史 provider 标签保持原生 `openai`，卸载无需迁移。
- **账户模型**：一个 provider = config.json 一个条目（adapter + baseUrl + apiKey/OAuth）；OpenAI 侧另有 **ChatGPT 账户池**（主 Codex login + 追加账户）与 API-key 池，`ocx account list/use/add-key` 管理（[configuration 文档](https://opencodex.me/reference/configuration/)）。
- **切换粒度**：三层——请求级模型路由（`provider/model` 前缀、combo 虚拟模型 `--strategy failover`，含 sticky routing）；**会话级账户切换**（新会话自动选账户、已有 thread 固定）；无全局配置改写。
- **额度感知**：**主动探测** ChatGPT 账户 5h/weekly/30d 配额窗口（对应 OpenAI `primary/secondary/tertiary_window` 机制，`src/codex/quota.ts` 追踪各窗口使用率；dashboard 一键刷新，`GET /api/codex-auth/accounts?refresh=1`）；**成功响应捕获配额头**；同时被动处理 429。
- **切换策略**：`accountPoolStrategy` 三种——`quota`（默认，比较最热配额窗口，活跃账户越过 `autoSwitchThreshold` 时为新会话挑低用量健康账户）/`round-robin`/`fill-first`；可给账户设 selection order 作后备顺序（[How It Works](https://opencodex.me/getting-started/how-it-works/)）。
- **失败处理**：429 → 账户 cooldown 并 failover；401/403 → 标记需重新认证、**fail-closed**（不静默换凭据）。
- **缓存友好性**：显式优化——**thread affinity 把既有会话钉在原账户**（长 SSH/tmux/移动会话不中途跳号），仅 quota 再评估/failover/账户排除/亲和过期/401/403/429 恢复可触发 rebind；请求日志显示 cached/cache-write token。
- **密钥存储**：`~/.opencodex/config.json`（API key 明文或 `${ENV}` 引用）+ `~/.opencodex/auth.json`（OAuth 凭据，自动刷新）；本地明文文件而非 OS keychain（未见 keychain 说明，未证实）。
- **合规声明**：README 明示与各厂商无关联、第三方代理可能违反服务条款，"Use at your own risk"。

### 3.2 cc-switch（farion1231/cc-switch）

- **定位与形态**：跨平台桌面 GUI（Tauri 2，MIT；另有 Web/CLI 形态），管理 8 个工具：Claude Code、Claude Desktop、Codex、Gemini CLI、Grok Build、OpenCode、OpenClaw、Hermes；50+ provider 预设、MCP/Prompts/Skills 统一管理、系统托盘快切。
- **配置文件与切换动作**：SSOT + 双向同步——provider 集中存 `~/.cc-switch/cc-switch.db`（SQLite，v3.6 前为 config.json），切换时**写回各应用 live 配置文件**：Claude Code 写 `~/.claude/settings.json`（`env.ANTHROPIC_AUTH_TOKEN`/base url）；Codex 写 `~/.codex/auth.json`（`OPENAI_API_KEY`）+ `config.toml`（实时 TOML 校验）；MCP 投影到 `~/.claude.json` 与 `~/.codex/config.toml`。**原子写（临时文件 + rename）+ 失败回滚 + 写后 backfill 回读**（[README](https://github.com/farion1231/cc-switch/blob/HEAD/README_ZH.md)）。
- **切换粒度**：默认**全局配置级**——每个应用同时只有一个 active provider，切换后需重启终端（Claude Code 例外：热切换，配置热重载即时生效）。另有**本地代理模式**（路由接管）：格式转换、auto-failover、circuit breaker、健康监测，此模式下具备请求级故障转移（[官网](https://cc-switch.cc/)）。
- **额度感知**：配置切换本身**纯手动**，无配额驱动自动换号；辅以本地记账 usage dashboard（花费/请求/tokens、自定义单价）与可配置余额查询脚本（JS Usage Script）；托盘子菜单显示当前供应商与用量摘要。
- **多账户组织**：全部 provider 存 SQLite，自动备份（保留 10 份）、支持 WebDAV/网盘云同步；Codex 可在多个官方 Plus/Team 账户间切换（切到 Official Login 预设走官方 OAuth）。
- **缓存友好性**：未见任何 prompt cache 缓解机制说明；全局切换 base_url/key 即换上游，缓存自然失效（推断，未证实）。
- **密钥存储**：本地 SQLite 与 live 配置文件明文（云同步会同步含 key 数据）；未见加密/keychain 说明（未证实）。

### 3.3 CLIProxyAPI（router-for-me/CLIProxyAPI）

- **定位与形态**：Go 守护进程（端口 8317，支持 Docker/TLS/Go SDK 嵌入），把 Gemini CLI、Antigravity、Codex（ChatGPT plan）、Claude Code、Grok Build、Qwen Code、Kimi、iFlow 等 OAuth 订阅账户封装为 OpenAI（chat/responses）/Gemini/Claude 兼容 API；配套桌面壳与多个第三方管理面板。
- **账户模型**：一个账户 = auth-dir（默认 `~/.cli-proxy-api`）里一个 JSON token 文件（如 `codex_oauth_<email>.json`，含 access/refresh token/expiry），经 `--login` OAuth 流程生成；客户端用本地 `api-keys` 列表鉴权，真实凭据由代理持有（[authentication 文档](https://router-for-me-cliproxyapi.mintlify.app/concepts/authentication)）。
- **轮询实现**：`routing.strategy` 支持 `round-robin`（默认，同优先级内轮转）、`weighted-round-robin`（凭据 `weight`）、`fill-first`（按 priority 取第一个可用，烧完换下一个）；同一 alias 可映射多个上游模型构成池，池内轮询、失败续跑（[routing 文档](https://mintlify.wiki/router-for-me/CLIProxyAPI/concepts/routing)）。
- **额度耗尽处理**：**被动检测**——429/quota-exceeded → 凭据 cooldown（指数退避 1s→30min 上限），自动用下一凭据重试；瞬时错误（408/5xx）默认 60s 冷却；`request-retry`（默认 3）、`max-retry-credentials`、`max-retry-interval` 可调；`save-cooldown-status` 把冷却态持久化；Management API `POST /reset-quota` 手动复位。**降级策略**：`quota-exceeded.switch-project`（Gemini 换 GCP project）、`switch-preview-model`（自动降 preview 模型）、`antigravity-credits`（免费层耗尽用 Google One credits 兜底）（[configuration options](https://help.router-for.me/configuration/options)）。
- **OAuth 刷新**：后台调度器每 5s 扫描，过期前自动刷新（默认 16 worker），失败退避 5min；请求遇 401 先即时刷新再重试。Token 为本地明文 JSON，无 keychain（[token 生命周期](https://deepwiki.com/router-for-me/CLIProxyAPI/7.4-token-refresh-and-lifecycle)）。
- **模型映射与改写**：每凭据 `models[].name→alias` 映射（`force-mapping` 把响应模型名改写回 alias）、按 OAuth channel 的全局 `oauth-model-alias`、凭据 `prefix` 定向、`excluded-models` 通配过滤、`payload` 规则（default/override/filter 按模型/协议改写请求 JSON）。
- **会话粘滞**：`routing.session-affinity: true`（v6.9.27 引入，默认关）启用 SessionAffinitySelector + TTL 内存 SessionCache（默认 1h），**明确以提高上游 prompt cache 命中率为目标**；session ID 依次取 Claude Code `metadata.user_id`、`X-Session-ID`、Codex `Session_id`、`X-Amp-Thread-Id`、`conversation_id`、`prompt_cache_key`、Responses 会话 ID，兜底用首消息 FNV hash；绑定优先于凭据 priority，绑定凭据不可用时自动 failover 并重绑（[PR #2816](https://github.com/router-for-me/CLIProxyAPI/pull/2816)、[issue #2594](https://github.com/router-for-me/CLIProxyAPI/issues/2594)）。另有 `codex.identity-confuse`：在 fill-first/affinity 下按所选账户重写 `prompt_cache_key` 与安装身份。
- **配置**：config.yaml 含 host/port/tls、auth-dir、api-keys、remote-management、各 provider key 列表、routing/quota-exceeded/payload 等，支持热重载。

### 3.4 三工具横向小结

- **切换层次**：cc-switch 改「客户端配置」（全局级、手动为主）；opencodex 与 CLIProxyAPI 是「代理层持有凭据」（请求/会话级、自动）。
- **额度感知**：opencodex 独有主动配额窗口探测 + 低用量优选；CLIProxyAPI 为被动 429 + 指数退避冷却 + 降级链；cc-switch 仅本地记账与手动查询。
- **缓存保护**：opencodex（thread 固定账户）与 CLIProxyAPI（session-affinity）都有显式 sticky 机制；cc-switch 无。
- **密钥存储**：三者均为本地明文文件/库（无 OS Keychain）——与 Pawork 的 Secret 红线形成直接差异点。

---

## 4. 网关与路由类项目

### 4.1 musistudio/claude-code-router（CCR）

- **额度模型**：本地 gateway/control plane，不做上游计费；提供 CCR client keys（过期时间 + 本地 request/token/image 限额），Observability 按请求记录 tokens 与估算成本（[README](https://github.com/musistudio/claude-code-router)）。
- **路由决策点**：三层管线（[DeepWiki routing-rules](https://deepwiki.com/musistudio/claude-code-router/6.1-routing-rules)）：① 内置 agent 逻辑（识别 Claude Code/Codex，注入工具、剥离计费头）；② 可选 custom router JS（`CUSTOM_ROUTER_PATH` 导出 async 函数，返回 `"provider,model"` 或 `null` 回落）；③ 配置化 RouterRule（按 header/body 条件匹配，首个命中生效）。经典场景键：`default` / `background`（小模型省钱）/ `think`（Plan Mode 推理）/ `longContext`（token 超 `longContextThreshold`，默认 60000）/ `webSearch` / `image`；`/model provider,model` 会话内动态切换。
- **子代理路由**：若 Models 页给模型填了 Description（启用开关），CCR 把模型目录注入 `Agent`/`Task`/`Workflow` 工具描述；主 agent 在 subagent prompt 开头嵌 **`<CCR-SUBAGENT-MODEL>provider,model</CCR-SUBAGENT-MODEL>`** 标签；子代理请求到达后由 `extractAndRemoveClaudeCodeSubagentModelTag` 从 system prompt 或前两条 user message 提取并剥除标签再定向路由（[DeepWiki subagent-routing](https://deepwiki.com/musistudio/claude-code-router/6.2-subagent-and-workflow-routing)）。这是「改不了客户端」时的 in-band 补丁方案。
- **失败处理**：路由层 retries + ordered fallbacks。
- **缓存/transformer**：transformer 按 provider 全局或按模型挂载、可链式组合；内置 `Anthropic`（保留原始参数直连，即保留 cache_control）、`openrouter`/`deepseek`/`gemini` 等格式改写、`maxtoken`、`reasoning`，以及 **`cleancache`：从请求中清除 `cache_control` 字段**（用于不认识 Anthropic 缓存标记、否则 400 的上游）。

### 4.2 BerriAI/litellm

- **额度模型**：层级 Organization → Team → User → Virtual Key → End-User，对应五张实体表，可共享 `LiteLLM_BudgetTable`（max_budget、soft budget、TPM/RPM、`budget_duration` 周期重置）；明细批量写 `LiteLLM_SpendLogs` 并聚合日表；每请求花费经 `completion_cost()` 折 USD 同时归集到 key/user/team/org，任一层超预算即拒绝；高并发有 budget reservation（[db_info](https://docs.litellm.ai/docs/proxy/db_info)、[multi_tenant_architecture](https://docs.litellm.ai/docs/proxy/multi_tenant_architecture)、[budget/spend](https://deepwiki.com/BerriAI/litellm/3.3-budget-and-spend-tracking)）。
- **配额执行**：key/user/team 各设 `tpm_limit`/`rpm_limit`；`token_rate_limit_type` 可按 input/output/total token 计（[users](https://docs.litellm.ai/docs/proxy/users)）。
- **路由策略**：`simple-shuffle`（默认，rpm/tpm/weight 加权随机——即同模型多 deployment 的额度分配方式）、`least-busy`、`usage-based-routing-v2`（Redis 原子跨实例 TPM/RPM 统计，生产推荐）、`latency-based`、`cost-based`（[routing](https://docs.litellm.ai/docs/routing)、[load_balancing](https://docs.litellm.ai/docs/proxy/load_balancing)）。
- **失败处理**：`allowed_fails` 后进 `cooldown_time` 冷却池；429 立即 cooldown；`num_retries` + 按异常类型 `retry_policy`；`fallbacks` 模型组级 failover 与 `context_window_fallbacks`；deployment 可标 `order` 分级。
- **缓存支持**：透传 `cache_control`；usage 同时上报 OpenAI `cached_tokens` 与 Anthropic `cache_creation/read_input_tokens`；价格表含缓存读/写单价（可覆盖），`completion_cost()` 按差价计费（[prompt_caching](https://docs.litellm.ai/docs/completion/prompt_caching)、[custom_pricing](https://docs.litellm.ai/docs/proxy/custom_pricing)）。历史上有 cache_creation 双计 bug（[issue #9812](https://github.com/BerriAI/litellm/issues/9812)，修复状态未逐版核实，未证实）。
- **缓存感知路由**：`PromptCachingDeploymentCheck`（`optional_pre_call_checks: [prompt_caching]`）记住发生 cache write 的 deployment，后续同前缀请求路由回同一部署（[教程](https://docs.litellm.ai/docs/tutorials/claude_code_prompt_cache_routing)）；另有 `session_affinity`/`deployment_affinity`（session_id → deployment 映射，默认 TTL 1h，[PR #21763](https://github.com/BerriAI/litellm/pull/21763)）。

### 4.3 new-api（QuantumNous，承自 one-api）

> 注：本节机制原型来自 songquanpeng/one-api，该上游 2026-01-09 后停止更新，已按活跃度标准从参考清单移除（见 §8 收录标准）；以下以活跃维护、渠道体系兼容的 new-api（AGPL，45k+ stars）为参考载体。

- **三层模型**：渠道（channel，一个上游 key/baseURL/模型列表）→ 用户账户额度 → 令牌额度，令牌与账户额度双重扣减。quota 基准 1 unit ≈ $0.002；流程为请求前预扣 + 按实际 usage 结算（[new-api FAQ](https://doc.newapi.pro/support/faq/)）。
- **计费公式**：额度 = 分组倍率 × 模型倍率 × (prompt tokens + completion tokens × 补全倍率)。
- **渠道调度**：优先级（大者优先）+ 同优先级按权重随机；连续失败达阈值自动禁用渠道；失败自动重试换渠道（[new-api channel 文档](https://github.com/QuantumNous/new-api-docs-v1/blob/main/content/docs/zh/guide/feature-guide/admin/channel.mdx)）；管理员令牌可 `Bearer KEY-CHANNEL_ID` 指定渠道。
- **多 key 与限流增强**：① 渠道多 Key 模式——一渠道挂多 key，Round Robin/加权随机，单 key 失败自动跳过、恢复重新启用；② 渠道级限流——Redis/内存 token bucket，作用域整渠道或单 key，重试排除被限渠道/key（[PR #5067](https://github.com/QuantumNous/new-api/pull/5067)）；③ 模型固定价格、上游倍率同步、模型映射/参数覆盖。Claude 缓存 token 差价计费细节未确认（未证实）。

### 4.4 Wei-Shaw/claude-relay-service（CRS）

- **额度模型**：控制在自发 API Key（`cr_` 前缀）层：每 key 可设时间窗口内请求数/token 量限速、并发限制、模型黑名单、客户端限制；usage 从流式响应实时捕获经 pricingService 计成本。订阅账户 5h 窗口在 UI/统计接口展示（窗口长度、剩余时间、窗口内用量），但 CRS 是透明中继、不改变 Anthropic 侧窗口；账户级 5h 进度内部估算方式未证实。
- **Sticky session（核心机制）**：请求链 auth 中间件 → 统一调度器（选账户 + 粘性会话）→ Token 检查/刷新 → 经代理转发。**对请求可缓存前缀（cache_control 标记内容，回退 system/首消息）做 SHA-256 哈希作为会话键，Redis 存 hash→账户映射（带 TTL），同一会话固定命中同一账户以保住 prompt cache**（CLAUDE.md、[issue #1](https://github.com/Wei-Shaw/claude-relay-service/issues/1)）。作者明确「频繁切换会导致 token 缓存使用量增大，以及可能增加封号风险」（[issue #165](https://github.com/Wei-Shaw/claude-relay-service/issues/165)）。Nginx 反代需 `underscores_in_headers on` 防会话头被丢。
- **失败处理**：429 → `markAccountRateLimited()` 从池排除；529 过载 → 配置时长内排除；503/5xx → 临时暂停。粘滞绑定账户不可用时自动切换（曾有死抱坏账户 bug，[issue #1007](https://github.com/Wei-Shaw/claude-relay-service/issues/1007) 已修复）；`429 "Extra usage is required"` 属非限流 429，需透传响应体而非锁账户（issue #1000）。并发控制用 Redis Sorted Set 排队而非直接回 429。
- **隔离与安全**：每账户可配独立静态 HTTP/SOCKS5 代理 IP（防多账户共用 IP 被封）；OAuth token AES 加密存 Redis。

### 4.5 tbphp/gpt-load

> 2026-08-18 已按功能重叠标准移出参照表（见 §8 移除记录）；本节保留为机制快照。

- Go 透明代理，完整保留 OpenAI/Gemini/Claude 原生格式——`cache_control` 等字段原样透传（由透明代理特性推断，未证实）；key 池负载均衡器，无 per-user quota 计费。
- **key 生命周期**：分组 key 池 + 自动轮换；`blacklist_threshold`（默认 3）累计失败拉黑；后台按 `key_validation_interval_minutes`（默认 60）定时验证黑名单 key，通过即恢复。
- **失败处理**：`max_retries`（默认 3）单请求换 key 重试；`failover_status_codes` 可配置触发 failover 的状态码列表（支持区间语法），分组可覆盖。

### 4.6 新兴同类项目（2025–2026）

- **[ding113/claude-code-hub](https://github.com/ding113/claude-code-hub)**（2026-08-18 移出参照表，见 §8）：Claude Code & Codex 代理（Next.js+Hono+PostgreSQL+Redis）。权重+优先级+分组调度、熔断器、最多 3 次故障转移；RPM/金额（5h/周/月）/并发 session 多维限流用 Redis Lua 保原子、Redis 挂了 Fail-Open 降级；session 绑定 provider 用 Redis `SET NX` 原子首成锁，复用前查健康度、支持向高优先级 provider 迁移（[DeepWiki session-binding](https://deepwiki.com/ding113/claude-code-hub/4.2-session-binding-and-provider-stickiness)）。
- **[ztx888/CLIProxyAPI-Plus](https://github.com/ztx888/CLIProxyAPI-Plus)**：CLIProxyAPI 社区强化版，Codex 配额运营：展示 5h/周额度与恢复窗口、`usage_limit_reached` 后自动持久停用、周额度低于阈值（如 3%）提前停用、按剩余额度排序。
- **[rynfar/meridian](https://github.com/rynfar/meridian)**（约 1.8k stars；2026-08-18 移出参照表，见 §8）：经 Claude Agent SDK 把 Claude 订阅桥接为标准 Anthropic/OpenAI 协议（不做 OAuth 拦截）；多 profile 账户即时切换 + 可选 sticky session routing，会话分散到多账户的同时保持每账户 prompt cache 温热。
- **[badrisnarayanan/antigravity-claude-proxy](https://www.mintlify.com/badrisnarayanan/antigravity-claude-proxy/guides/load-balancing)**（2026-08-18 移出参照表，见 §8）：Google 账户池代理，策略三选一：Hybrid（健康度/余量/恢复期综合）、**Sticky（缓存最优：session ID = 首条 user message 的 SHA256，限流 <2min 时等待不切换）**、Round-Robin（吞吐最优、缓存最差）——把「缓存命中」作为调度策略的一等权衡维度。
- **[duolahypercho/codex-router](https://github.com/duolahypercho/codex-router)**（约 2.4k stars，2026-08-18 登记）：本地路由器 + 托盘，把外部模型并入 Codex / DeepSeek Harness / Gemini CLI 的原生目录。Design B 只改 `openai_base_url` + `model_catalog_json`；入站 Codex 凭据丢弃，只向所选上游注入对应 OAuth/API key。Failover 默认开但窗口极窄（402 / 余额耗尽 / 需等待 >1min 的 429 才换**已启用的下一模型**，坏 key 与宕机仍 fail-closed）。**不是** ChatGPT 账户池，也没有会话-账户 sticky；作 F2/F3 窄错误分类与 F6-A 上游对照，不作 G1/G3 主参照。机制详见 [references.md](../references.md) §3.2。

---

## 5. 输入/提示缓存（prompt caching）机制

### 5.1 厂商机制对照表（2026-08 各官方文档现行版本）

| 厂商 | 触发方式 | 最小可缓存长度 | TTL | 计价（写/读/存储） | 缓存键与隔离 | 用量字段 |
| --- | --- | --- | --- | --- | --- | --- |
| **Anthropic Claude**（[docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)，GA 无需 beta header） | 显式：块级 `cache_control:{type:"ephemeral"}` 断点，最多 **4 个**；另有 top-level 自动模式（断点放最后可缓存块并随对话前移） | 按模型 512–4096 tok，不足静默不缓存 | 5m 默认 / `ttl:"1h"` 可选，命中即免费刷新 | 写 1.25×（5m）/ 2×（1h）；读 0.1×；无存储费 | 按 `tools→system→messages` 渲染序累积前缀哈希；断点前任一字节变化即失效；命中查找从断点回看 20 块。组织间隔离；2026-02 起 Claude API 为 workspace 级隔离，Bedrock/Vertex 仍组织级 | `cache_creation_input_tokens`、`cache_read_input_tokens`（总输入 = 三者之和） |
| **OpenAI**（[docs](https://developers.openai.com/api/docs/guides/prompt-caching)） | 旧模型隐式自动（最长前缀匹配）；GPT-5.6+ 新增显式 `prompt_cache_breakpoint` 与 `prompt_cache_options.mode` | 1024 tok（128-token 步进） | 旧：5–10min 不活跃清除、≤1h，`prompt_cache_retention` 可延长（如 24h）；GPT-5.6+：`ttl` 仅 `"30m"`（命中刷新） | 旧写免费、读 0.5×→0.1×；GPT-5.6+ 写 1.25×、读 0.1× | 前缀哈希 + **`prompt_cache_key`**（建议 session/user ID）组合参与机器路由，GPT-5.6 起为可靠匹配所必须；每 key 建议 ≤15 rpm；不跨组织共享 | `prompt_tokens_details.cached_tokens`（Responses 为 `input_tokens_details`）；GPT-5.6+ 增 `cache_write_tokens` |
| **Google Gemini**（[docs](https://ai.google.dev/gemini-api/docs/generate-content/caching)） | 隐式（2.5+ 默认开启不可关）+ 显式 `CachedContent` API（资源引用，折扣有保证） | 2.5 Flash 1024 / 2.5 Pro 2048 / Gemini 3 家族 4096 | 显式默认 1h（TTL 可设/可更新）；隐式不保证（≤24h 清除） | 命中 0.1×（2.5+）；显式创建按标准输入价一次性 + 存储费（$/1M tok·h）；隐式无写入溢价 | 隐式前缀匹配；显式按资源名；按 Google Cloud 项目隔离 | `usageMetadata.cached_content_token_count` |
| **DeepSeek**（[docs](https://api-docs.deepseek.com/guides/kv_cache)） | 隐式：硬盘 KV Cache，默认开启不可关 | 前缀单元整体匹配（需从 token 0 完整命中） | 数小时至数天自动清理，best-effort | 命中约 0.02×；写无溢价、无存储费 | 从 token 0 起完整前缀单元匹配；按账户隔离 | `prompt_cache_hit_tokens`、`prompt_cache_miss_tokens` |
| **智谱 GLM**（[docs.bigmodel.cn](https://docs.bigmodel.cn/cn/guide/capabilities/cache)） | 隐式自动识别重复前缀 | 未公布（第三方实测约 1024，未证实） | 未公布（第三方称约 10min，未证实） | 命中约标准输入价 50%（Z.ai 国际站 GLM-5.2 约 0.19×）；写无溢价。**仅标准 API 计费适用，GLM Coding Plan 套餐不适用** | 前缀自动匹配；按账户隔离（未详述，未证实） | `usage.prompt_tokens_details.cached_tokens` |
| **阿里 Qwen / DashScope**（[docs](https://help.aliyun.com/zh/model-studio/context-cache)） | 三种：显式缓存（确定命中）/ 隐式（自动不可关）/ 会话缓存（header `x-dashscope-session-cache: enable`） | 显式/会话 1024；隐式 256 | 显式/会话 5min（命中重置）；隐式不保证 | 显式/会话：写 125%、命中 10%；隐式：写 100%、命中 20% | 前缀匹配；显式与隐式在 Chat Completions 下互斥；按阿里云账户隔离 | `prompt_tokens_details.cached_tokens`；会话缓存另返回 `cache_creation_input_tokens` |
| **Moonshot Kimi**（[docs](https://platform.kimi.com/docs/guide/use-context-caching-feature-of-kimi-api)） | 隐式全自动（旧显式 Context Caching API + 按分钟存储费模式已不再描述，是否彻底下线未证实） | 前一请求 prompt tokens >256 | 系统自动管理，未公布 | 命中 0.1×；无写入溢价、无存储费 | 前缀匹配；按账户隔离 | `usage.cached_tokens` |
| **AWS Bedrock**（[docs](https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html)） | 显式：Converse 用 `cachePoint` 块（放被缓存内容之后）；InvokeModel 用原生 `cache_control` | 按模型 1024–4096 | 5m 默认；2026-01 起 Claude 4.5+/Nova 支持 `ttl:"1h"` | 同 Anthropic 结构 | 按 AWS 账户（组织级）隔离 | Converse：`cacheReadInputTokens`、`cacheWriteInputTokens` |
| **OpenRouter**（[docs](https://openrouter.ai/docs/guides/best-practices/prompt-caching)） | 转发/翻译：透传块级 `cache_control`；top-level 自动模式翻译为 Bedrock 尾部断点 | 随上游 | 随上游 | 随上游；响应含 `cache_discount` | **Provider sticky routing**：缓存请求自动粘到同一上游端点；`session_id`（回退 `prompt_cache_key`）显式指定粘性键，10min 不活跃过期，故障自动 fallback | `cached_tokens`、`cache_write_tokens`、`cache_discount` |

### 5.2 Coding Agent 客户端的断点摆放实践

- **Claude Code**（[第三方逆向分析](https://notes.tsukino.dev/99-%E5%B7%A5%E5%85%B7%E4%B8%8E%E5%8F%82%E8%80%83/repos/how-claude-code-works/en/docs/03-context-engineering)）：3 类断点——① system prompt 在静态/动态边界处（`splitSysPromptPrefix()`），核心指令跨用户共享；② 工具数组最后一个常规工具打断点，可选工具放断点**之后**（开关不毁前缀），MCP 工具延迟加载；③ 消息数组最后一条打滑动断点。fire-and-forget 辅助请求把断点打在倒数第二条消息，避免临时请求污染主对话缓存链。
- **opencode**：前 2 system + 末 2 消息（见 §2.1）；系统提示静态/动态拆块。
- **pi**：system + 末 tool + 末 user（见 §2.2）；社区双标记策略（末 assistant `tool_use` 块 + 末 user 块）适配 MiniMax/Kimi 式缓存窗口，命中率 80%+（[PR #1737](https://github.com/badlogic/pi-mono/pull/1737)）。
- **Codex CLI**（[client.rs](https://github.com/openai/codex/blob/d807d44a/codex-rs/core/src/client.rs)）：Responses API，`prompt_cache_key = conversation_id`（会话内稳定跨轮不变）；跨会话共享相同启动前缀目前做不到（[issue #21796](https://github.com/openai/codex/issues/21796)）。

### 5.3 保持前缀稳定的技巧

- **静态在前、动态在后**：system prompt 固定；时间戳、环境信息、TODO 状态等易变内容放断点之后或对话末尾。
- **工具列表确定性**：排序稳定（opencode 曾修复 `Object.values()` 非确定序）、schema 不嵌 per-repo/per-run 值；JSON 键序也算字节，一字节差即从该处起全失效。
- **历史 append-only**：不改写/删除/重排早期消息；注意 reasoning 内容透传要求（GLM `clear_thinking=false` 时须原样透传，否则等效改写历史）。
- **Compaction/摘要 = 重写前缀 = 缓存全失效**，且按写入价重建。折中：低频触发、在任务自然边界压缩、压缩后立即发一次请求预热新前缀；长会话考虑 1h TTL 摊薄重写成本。
- **厂商特有**：OpenAI 固定 `prompt_cache_key` 且每 key ≤15 rpm，超量要分片；Anthropic 断点回看仅 20 块，单轮新增 >20 块（多工具调用）需每 ~15 块补中间断点。

### 5.4 多账户/多上游切换对缓存的破坏与网关缓解

**为什么换账户 = 缓存全失效**：所有厂商的缓存按组织/账户命名空间隔离——Anthropic 组织间隔离 + workspace 级隔离；OpenAI 声明「prompt caches are not shared between organizations」（[公告](https://openai.com/index/api-prompt-caching/)）；Bedrock 按 AWS 账户。网关把下一请求路由到另一账户，等于在空命名空间从零重建，重付全部写入费且延迟上升。

**网关侧缓解**（详见 §3/§4 各项目）：CRS 内容 hash 粘滞；CLIProxyAPI session-affinity；opencodex thread affinity；LiteLLM `PromptCachingDeploymentCheck` + `session_affinity`；OpenRouter provider sticky routing。**通用原则：cache-aware routing——只在新会话做账户轮换/负载均衡，会话中途绝不换；持续统计 `cache_read` 占比作为路由健康指标。**

### 5.5 子代理（sub-agent）场景的缓存取舍

- **复用父前缀 vs 独立上下文**：同模型同账户下，子代理以父上下文为前缀追加任务可直接读父缓存（注意 Anthropic 并发限制：缓存条目要等首个响应开始后才可用，并行 fan-out 前先等一发种子请求）。独立上下文（新 system+tools）缓存从零建：重付写入费，换更小上下文与更干净注意力——多数编排框架（如 Claude Code subagents 独立 context window）选择后者。
- **实测数据**：Codex fork 出的子会话缓存命中率从 62% 掉到 9.6%，因 cache key 绑死 thread id（[issue #21796](https://github.com/openai/codex/issues/21796)）——理想方案是子代理家族共享一个稳定 `prompt_cache_key`/缓存命名空间。
- **便宜模型/不同账户的取舍**：缓存不跨模型也不跨账户，子代理换绑即放弃父缓存复用。判断标准：子代理任务上下文短、调用少 → 写入费损失小于模型差价，值得换；子代理与父共享大前缀且高频往返 → 保持同模型同账户更省。
- pi 的立场：拒绝内置 sub-agent，主张 context gathering 在独立 session 完成并产出 artifact，兼顾可观测性与缓存友好（[Mario Zechner 博文](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)）。

---

## 6. 模式归纳（供方案决策引用）

1. **切换层次四档**：① 配置级（cc-switch：改客户端配置文件，全局、手动）；② 客户端内路由（OpenCode/Pi 的 `/model` 切换、CCR 场景规则：请求级、规则驱动）；③ 代理层会话级（opencodex thread affinity、CRS/CLIProxyAPI sticky session：新会话再平衡 + 会话内锁定）；④ 代理层请求级（round-robin/加权：吞吐最优、缓存最差）。业界共识是 ③ 为默认、④ 仅在无缓存诉求时用。
2. **额度感知三形态**：本地记账（litellm 预算、new-api quota、cc-switch dashboard）→ 被动信号（429/Retry-After/`usage_limit_reached`/成功响应配额头，CLIProxyAPI/CRS/opencodex 均用）→ 主动探测（opencodex 三窗口刷新、CLIProxyAPI-Plus 阈值停用；成本与 ToS 面最大）。成熟实现是三者叠加、可信度分级。
3. **缓存保护两路径**：改写层（CCR `cleancache` 剥除、OpenRouter 翻译）解决「上游不认识 cache 标记」；调度层（sticky session/亲和）解决「换账户毁缓存」。**订阅池代理已把 sticky session 做成标配**（CRS、CLIProxyAPI、opencodex、claude-code-hub、meridian、antigravity 全部实现）。
4. **子代理路由三模式**：声明式绑定（opencode agent.model + 权限派生——客户端可控时的正解）；in-band 标签（CCR `<CCR-SUBAGENT-MODEL>`——改不了客户端时的补丁）；模型即子代理槽位（opencodex 把每个上游模型暴露为一个可指派的子代理）。
5. **共性短板**：Agent 内核层普遍不做同 provider 多账户（OpenCode/Pi 均空白，靠外部代理/配置切换工具补位）——「内核单凭证 + 外部池化」是当前生态分层，但也意味着内核原生多账户是差异化机会。
6. **失败分类共识**：429（限流，可冷却恢复）≠ quota exceeded（窗口耗尽，需等 reset）≠ 401/403（凭证问题，refresh 或人工介入）≠ 5xx（临时故障）；错误分类错了就会误惩罚账户（CRS issue #1000 的 "Extra usage is required" 教训）。
7. **合规风险共识**：第三方代理接订阅账户有 ToS/封号风险（opencodex 免责声明、CRS 作者提示、Anthropic 对 OpenCode 的封锁先例）；缓解手段（每账户独立代理 IP、身份伪装如 `identity-confuse`）本身加重合规问题。

---

## 7. 与 Pawork 现有资产的对照

V1 已有大量同构资产（详见 [provider-control-plane](../../../Pawork_v1/docs/features/provider-control-plane.md)、[usage-quota](../../../Pawork_v1/docs/features/usage-quota.md)、[context](../../../Pawork_v1/docs/features/context.md)），V2 在 S11 激活（见 [../../ROADMAP.md](../../ROADMAP.md) §2 阶段表与 [../design.md](../design.md) §2 包激活映射）：

| 外部模式 | Pawork V1 对应资产 | 状态 |
| --- | --- | --- |
| 账户池 + 租约 + 并发上限（opencodex/CLIProxyAPI） | `provider-control`：`ProviderAccount`（priority/weight/max-concurrency/lifecycle）、`CredentialLease`（有期限可回收）、`CredentialPool` trait | 13.5k 行库级完整，S11 激活 |
| sticky session / thread affinity（CRS/CLIProxyAPI/opencodex） | `provider-control`：`SessionBinding` 独立状态机（Unbound→Bound→Rebinding→Bound） | 已有词表与实现，缺「默认开 + 缓存指标联动」 |
| 路由策略（quota/round-robin/fill-first） | `RoutingPolicy`：SingleCandidate、严格 Priority、Round-Robin、Smooth Weighted Round-Robin、Fill-First；固定过滤链 capability → tenant → health → priority → affinity → weighted/fill-first → concurrency | 已有；缺「配额余量优先」策略 |
| 错误分类驱动切换（429 冷却、401 refresh-once、quota failover） | `ErrorClassifier` + 错误表（AuthRejected/RateLimited/QuotaExceeded/ProviderUnavailable…按 credential/account/model/provider 四 scope 分离 cooldown 与 circuit） | 已有，语义比多数开源实现更细 |
| 配额窗口（5h/周/30d） | `quota-service`：`QuotaWindow{Overall,Rolling5h,Weekly,Monthly}`、`Exact/Derived/Scraped` 可信度分级、LocalLedger 派生、耗尽预测、阈值告警 | 核心 S11 激活；六厂商远端适配器 + WebScrape（约 8k 行）冻结候审 |
| 预算执行（litellm budget） | `usage-ledger`（dedup_key 幂等）+ orchestration budget-gate；「仅 fresh Exact 硬停止」规则 | S11 激活 |
| 子代理绑定不同通道 | S11 多 Agent demo 即「GLM 与 OpenCode Go 各驱动一个子 Agent」；`TeamEvent` 双通道语义 | 已规划，缺声明式绑定契约 |
| 缓存断点/亲和键 | `provider-anthropic` 完整版含 prompt cache（S6 迁移项）；context-engine 分级裁剪与 compaction | **缺 canonical 缓存策略层**（注解、能力表、用量入账、命中率观测） |
| 账户/端点配置导入（cc-switch SSOT、CLIProxyAPI auth-dir） | `compat-loader`：Claude/Codex/Grok/Cursor/Pi 五来源只读导入（S9） | 已有框架，缺账户/端点维度 |

主要缺口（即候选功能，登记于 [../design.md](../design.md) §5，方案见 [multi-account-quota-proposals.md](multi-account-quota-proposals.md)）：同 Provider 多账户的用户面（UX/凭证组织）、订阅 plan 凭证类型、被动配额信号捕获、缓存感知亲和默认化、子 Agent 声明式绑定契约、canonical 输入缓存策略控制。

---

## 8. 参考项目对照总表（2026-08-14 快照 · 2026-08-18 复核清理）

> 覆盖 §2–§4 详查项目 + 2026-08-14 补充调研发现的流行项目。star 为当日数量级快照（GitHub API 抽样复核：cc-switch 127,132、opencode 197,254、OmniRoute 47,446 与调研一致）；「缓存策略与公开效果」列仅录**公开**数据——绝大多数项目不公布命中率，仅有的公开口径（Pi 社区双标记 80%+、Codex 会话级 62%）与 Pawork 的 95/97/99 目标口径（排除冷启动的会话级聚合，见 [multi-account-quota-plan-merge.md](multi-account-quota-plan-merge.md) §1.3）不同，不可直接对比。
>
> **收录标准与移除记录（2026-08-14 按 pushed_at 复核）**：仅收录活跃维护项目——已归档或约 3 个月以上无提交者不作参考。据此移除 7 项：TensorZero（2026-06 归档停运）、Roo Code（2026-05 停运归档）、Helicone AI Gateway（被 Mintlify 收购转维护模式，2025-11 后无提交）、Arch/archgw→Plano（被 DigitalOcean 收购，2026-04 后无提交）、Portkey（被 Palo Alto Networks 收购，2026-05-25 后无提交）、one-api（2026-01-09 后无提交，机制由 new-api 继承）、gemini-balance（2025-09-30 后无提交）。2026 年商业开源网关整合潮（多起收购/停运）本身是重要事实：**依赖外部网关的方案有存续风险，自持进程内能力（F6-A 路线）因此更稳**。
>
> **2026-08-18 二次清理（功能重叠去重，GitHub API 全量复核）**：按「同功能与实现思路可由表内更强项目替代 + star 停滞或活跃不足」移除 5 项，对应行已从下表删除，机制原文保留于 §4.5/§4.6/§5.4（历史快照）——① **gpt-load**（key 池拉黑 + 定时验证恢复：由 CLIProxyAPI 冷却/自动恢复链与 new-api 失败自动禁用/恢复覆盖，V1 `ErrorClassifier` 语义更细；6.3k 完全停滞）；② **uni-api**（channel 加权 + key 轮询：由 new-api / CLIProxyAPI 覆盖；1.3k 零增长、个人项目流量稀疏）；③ **claude-code-hub**（`SET NX` 首成锁与 Redis Lua 多维限流：sticky 由 CRS / CLIProxyAPI 覆盖，Redis 集中式形态与 Pawork 单机产品不匹配；3.3k 停滞、提交放缓）；④ **meridian**（「不拦 OAuth 合规 sticky」立场已内化为 F1-B/F3-B 已确认决议；1.9k，且仓库无 LICENSE，代码不可参考）；⑤ **antigravity-claude-proxy**（2026-06-08 后停更 71 天，越过上轮「暂保留观察」；「缓存命中为调度一等权衡」由 OmniRoute cacheAffinity 与 CRS sticky 承载；已确认其仓库为 badrisnarayanan/antigravity-claude-proxy，约 3.9k）。同日复核另记：claude-relay-service 增长停滞于 12.5k（作者重心转向 sub2api，topics 自标 "crs2"），仍为 G3 sticky 主参照，保留观察；OmniRoute（50k，+3k/4 天）与 9router 同属「免费聚合 + token 压缩」画像，持续关注安全面；9router 19 份安全通告确认为 6 critical / 11 high / 2 medium（最新 2026-07-16）；LiteLLM 已重写为 Rust core + Python SDK；许可证注记——new-api AGPL-3.0、sub2api LGPL-3.0（open issues 2.7k 积压）、LiteLLM 混合授权（MIT 主体 + enterprise 目录）。子代理另建议移除 Codex Router（2.5k、功能面窄），**不采纳**：该项目 2026-07-19 新建非「较老」，且承担与 opencodex 不同的「凭证隔离多客户端 + 注册表驱动目录」角色（S6 通道端点形态 / S9 G6 导入源 / S11 F2-F3 窄 failover / R5 通道注册表数据化），表内无替代。

| 项目（star≈） | 类别 / 形态 | 账号 / 凭证切换 | 缓存策略与公开效果 | 反代 / 协议处理 | 差异与借鉴（含 2026 状态） |
| --- | --- | --- | --- | --- | --- |
| [OpenCode](https://github.com/anomalyco/opencode)（197k） | 编码 Agent（TS/Bun 终端） | provider 单凭证；多账户靠插件或改 XDG 目录 | 前 2 system + 末 2 消息断点；OpenAI `prompt_cache_key`=sessionID；命中未公开 | 无反代，客户端直连；transform 层按目标 SDK 归一化 | 内置 task 子代理 + 权限派生；429 重试完整；多账户空白 |
| [Pi](https://github.com/earendil-works/pi)（90k） | 编码 Agent（TS monorepo） | provider 单凭证；拆 providerID 绕行 | system + 末 tool + 末 user 断点 + 1h/24h 长 TTL + 亲和头；社区双标记 80%+ | 无反代；compat 矩阵跨厂商 handoff | provider 无关 Context；订阅 OAuth 全线可用；核心零子代理 |
| [Cline](https://github.com/cline/cline)（66k） | 编码 Agent（VS Code） | BYOK 配置档手动切换，无轮询 | 按模型清单在 system + 末 1–2 user 打 `cache_control`；粘滞交给 OpenRouter | 无反代 | Plan/Act 双模型绑定 |
| [Kilo Code](https://github.com/Kilo-Org/kilocode)（27k） | 编码 Agent（VS Code） | 30+ BYOK + 自营网关 | 沿 Cline 谱系断点；网关回传 cache 用量；`kilo-auto` 会话亲和分层路由 | 自营网关 api.kilo.ai | 难度分类路由与缓存命中协同设计 |
| [cc-switch](https://github.com/farion1231/cc-switch)（127k） | 配置切换工具（Tauri 桌面） | **全局配置级手动切换**（SQLite SSOT 原子写回 8 工具）；代理模式有 failover | 无缓存机制（切换即缓存作废） | 可选本地代理（格式转换/熔断/健康监测） | 多工具统一管理 + 云同步；额度仅本地记账 |
| [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)（47k） | 账户池代理（Go） | OAuth 池 RR/加权/fill-first；指数退避冷却 + 降级链（switch-project/preview） | session-affinity（v6.9.27+，默认关）；identity-confuse 重写 cache key（Pawork 不采纳） | 兼容 API 网关（OpenAI/Gemini/Claude 入口）协议翻译 | token 本地明文；Management API 手动复位配额 |
| [sub2api](https://github.com/Wei-Shaw/sub2api)（37k） | 订阅池网关（Go + 管理台） | Claude/OpenAI/Gemini 订阅池 + key 分发 + 限额 + 拼车计费 | 未证实（订阅反代按理透传 cache_control） | OpenAI/Anthropic 双入口 + 订阅上游适配 | CRS 同作者二代；商业拼车形态，ToS 风险最重 |
| [9router](https://github.com/decolua/9router)（25k） | 本地代理 | 40+ provider 多账号；订阅→低价→免费三级 fallback | token 压缩卖点；缓存处理未证实 | BYOK 本地代理 + 协议转换 | **19 份安全通告（6 critical）**——选型反面警示 |
| [claude-relay-service](https://github.com/Wei-Shaw/claude-relay-service)（13k） | Claude 订阅池中继（Node） | 账户池 + 429/529 标记排除 + Redis 排队 | **内容 hash sticky session**（作者明示切换毁缓存 + 封号风险） | Claude 专用透明中继；每账户独立代理 IP | OAuth AES 加密存 Redis；5h 窗口展示 |
| [opencodex](https://github.com/lidge-jun/opencodex)（9.9k） | 本地代理 + dashboard（Bun） | ChatGPT 账户池 quota/RR/fill-first；会话级切换 | **thread affinity 钉账户**；日志显示 cache 读写 | base_url 截持（只改一个字段）+ 七适配器协议翻译 | 5h/周/30d 主动配额窗口探测；401/403 fail-closed |
| [Codex Router](https://github.com/duolahypercho/codex-router)（2.4k，2026-08-18） | 本地路由器 + 托盘（JS / LiteLLM） | 单凭证隔离，非账户池；额度耗尽才换**模型** | 无会话-账户 sticky；外部 compaction 用自有 `kcr1:` 摘要 | Codex Responses → LiteLLM 翻译；托管改 `openai_base_url` + catalog | 一安装服务 Codex / Harness / Gemini CLI；login-free 别名与匿名免费网关 Pawork 不采纳 |
| [LiteLLM](https://github.com/BerriAI/litellm)（56k） | Proxy/Router（Rust core + Python SDK，2026-08 复核） | 多 deployment 加权 / least-busy / usage-based v2；cooldown/fallback | **PromptCachingDeploymentCheck** + session_affinity（TTL 1h）缓存感知路由；缓存差价计费 | OpenAI 兼容翻译 100+ 上游 | 层级预算（org/team/user/key）最完整 |
| [OmniRoute](https://github.com/diegosouzapw/OmniRoute)（47k） | 自托管网关（TS） | 多账号/多 provider 池；19 种策略 + Auto-Combo 14 因子（含配额 headroom/reset-aware） | **cache-optimized 策略 + cacheAffinity 因子**钉热缓存账号；`X-OmniRoute-Decision` 决策头 | OpenAI 兼容 + 330+ provider 转换 + 15–95% token 压缩 | 2026 token-saver 爆款；免费/OAuth 池化 ToS 风险 |
| [new-api](https://github.com/QuantumNous/new-api)（45k） | 计费网关（Go，承自 one-api） | 渠道优先级 + 权重；渠道内多 key RR/加权 + 渠道级限流 | 缓存感知路由未见；缓存差价计费未证实 | OpenAI 兼容翻译 | quota 折算三层计费（渠道/账户/令牌） |
| [claude-code-router](https://github.com/musistudio/claude-code-router)（37k） | Claude Code 网关（TS） | provider 级路由（无账户池） | `cleancache` 剥除 cache_control；Anthropic transformer 保留断点 | Claude Code 专用 + transformer 链改写 | 场景路由（default/background/think/longContext）；子代理 in-band 标签（反例） |
| [Bifrost](https://github.com/maximhq/bifrost)（7.3k） | Go 网关 | 每 provider 多 key 权重随机 + 失败/限流切换；虚拟 key 治理 | cache_control 透传 + 语义缓存插件；prompt cache 指标未证实 | 统一 API + 各家 SDK drop-in 双向翻译 | 11µs@5k RPS 性能叙事，LiteLLM 位竞争者 |
| [Envoy AI Gateway](https://github.com/envoyproxy/ai-gateway)（1.9k，CNCF v1.0 GA） | K8s 网关（Go/Envoy） | provider failover + token 级限流 | **统一 cache_control API 跨厂商翻译**（Vertex/Bedrock cachePoint，≤4 断点）+ cached_tokens 回传 | K8s Gateway API 协议翻译 | 首个产线 GA 的 CNCF 系 AI 网关，内建 MCP 网关 |
邻层项目（另一品类，不单列）：Kong AI Gateway（~44k）与 Higress（~9.1k）为通用 API 网关加 AI 插件/语义缓存；NVIDIA Dynamo（~7.7k）为推理集群内 KV-cache-aware 路由（serving 层）；metapi（~3.2k）为聚合 new-api 等的「路由器之路由器」。行业目录：[awesome-ai-gateway](https://github.com/cuihuan/awesome-ai-gateway)。

**对 Pawork 实现的三点结论**：

1. **缓存亲和已从「订阅池特色」扩散为通用网关标配**（OmniRoute cacheAffinity 因子、Envoy AI Gateway 跨厂商 cache_control 翻译、LiteLLM 缓存感知路由）——F3-B「亲和默认开」与 F5-B「统一缓存注解 + 能力表映射」与行业方向一致，且 Envoy AI Gateway 的「统一 cache API → 各家 cachePoint 翻译」正是 F5-B adapter 映射层的同构先例。
2. **行业整合期风险**：2026 年内多个商业网关被收购或停运、头部编码 Agent Roo Code 归档（相关项目已按收录标准移出总表，名单见表前移除记录）——外部网关作依赖有存续风险，进程内自持能力（F6-A）与「集成而非依赖」的立场得到事实支撑。
3. **命中率公开数据稀少**：仅 Pi（80%+，双标记口径）与 Codex（62%，会话口径）可查——Pawork 的 95/97/99 目标（排除冷启动的会话级聚合口径）无外部直接可比基线，达标判断以自建三场景真实测试为准（[multi-account-quota-plan-merge.md](multi-account-quota-plan-merge.md) §1.3）。
