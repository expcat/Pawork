# 参照项目手册

> **用途**：任务开启阶段快速查阅各参照项目的目标、功能面与文档入口。本手册是**目录/索引层**，不展开机制细节：机制调研全文见 [research/](research/) 下各文档（深入处以「详见 research §N」跳转），各阶段功能 → 参照项目的映射见 [design.md](design.md) §4。文中 star 数与项目事实均为 **2026-08-14** 快照（复核口径见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §8），实现前应复核最新实态。

---

## 1. 总览

三类参照项目：**A** = 主要对标编码 Agent；**B** = 多账户、网关与路由专题；**C** = 其他编码 Agent 与协议/标准。star 为数量级快照。

| 项目 | 类别 / 形态 | 一句话定位 | 主链接 |
| --- | --- | --- | --- |
| OpenCode（197k） | A / TUI 编码 Agent（TS/Bun） | 多形态（TUI / Desktop beta / Web / IDE）编码 Agent，自营 Zen/Go 托管模型 | [anomalyco/opencode](https://github.com/anomalyco/opencode) |
| Pi（90k） | A / TUI 编码 Agent（TS/Bun monorepo） | provider 无关 Context 与 Pi Packages 能力包生态 | [earendil-works/pi](https://github.com/earendil-works/pi) |
| Codex | A / CLI + Desktop + Cloud | OpenAI 官方编码 Agent 产品线，SDK / MCP server 等集成面最广 | [developers.openai.com/codex](https://developers.openai.com/codex) |
| opencodex（9.9k） | B / 本地代理 + dashboard（Bun） | Codex 协议翻译（40+ provider）+ ChatGPT 账户池三窗口配额路由 | [lidge-jun/opencodex](https://github.com/lidge-jun/opencodex) |
| cc-switch（127k） | B / Tauri 桌面应用 | 多工具供应商**配置级**切换（SSOT SQLite 原子写回） | [farion1231/cc-switch](https://github.com/farion1231/cc-switch) |
| CLIProxyAPI（47k） | B / Go 守护进程 | 多 OAuth 订阅账户封装为兼容 API（轮询 + 冷却 + 亲和） | [router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) |
| claude-relay-service（13k） | B / Claude 订阅池中继（Node） | 内容 hash sticky session 保 prompt cache | [Wei-Shaw/claude-relay-service](https://github.com/Wei-Shaw/claude-relay-service) |
| claude-code-router（37k） | B / Claude Code 本地网关（TS） | 场景化路由 + transformer 链改写 | [musistudio/claude-code-router](https://github.com/musistudio/claude-code-router) |
| LiteLLM（56k） | B / Proxy/Router（Python） | 层级预算 + 多策略路由 + 缓存感知路由 | [BerriAI/litellm](https://github.com/BerriAI/litellm) |
| new-api（45k） | B / 计费网关（Go） | 渠道-账户-令牌三层 quota 折算计费 | [QuantumNous/new-api](https://github.com/QuantumNous/new-api) |
| gpt-load（6.3k） | B / key 池透明代理（Go） | key 轮换 + 失败拉黑 + 定时验证恢复 | [tbphp/gpt-load](https://github.com/tbphp/gpt-load) |
| claude-code-hub（3.3k） | B / 代理（Next.js/Hono） | Redis Lua 多维限流 + session 绑定首成锁 | [ding113/claude-code-hub](https://github.com/ding113/claude-code-hub) |
| meridian（1.8k） | B / 订阅桥（Claude Agent SDK） | 多 profile 切换 + sticky routing；不拦 OAuth 的合规路线 | [rynfar/meridian](https://github.com/rynfar/meridian) |
| antigravity-claude-proxy | B / Google 账户池代理 | Hybrid / Sticky / RR 三策略，缓存命中是调度一等权衡 | [docs（load-balancing）](https://www.mintlify.com/badrisnarayanan/antigravity-claude-proxy/guides/load-balancing) |
| OmniRoute（47k） | B / 自托管网关（TS） | 19 种策略 + cacheAffinity 因子钉热缓存账号 | [diegosouzapw/OmniRoute](https://github.com/diegosouzapw/OmniRoute) |
| Bifrost（7.3k） | B / Go 网关 | 统一 API + 每 provider 多 key 治理，高性能叙事 | [maximhq/bifrost](https://github.com/maximhq/bifrost) |
| Envoy AI Gateway（1.9k） | B / K8s 网关（CNCF v1.0 GA） | 统一 cache_control API 跨厂商翻译 + 内建 MCP 网关 | [envoyproxy/ai-gateway](https://github.com/envoyproxy/ai-gateway) |
| uni-api（1.3k） | B / Python 网关 | 单 YAML 极简派，channel 加权 + key 轮询 | [yym68686/uni-api](https://github.com/yym68686/uni-api) |
| sub2api（37k） | B / 订阅池网关（Go + 管理台） | 订阅池 + key 分发 + 拼车计费（CRS 同作者二代） | [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api) |
| 9router（25k） | B / 本地代理 | 40+ provider 多账号 + 三级 fallback；安全通告选型反面警示 | [decolua/9router](https://github.com/decolua/9router) |
| Cline（66k） | C / VS Code 编码 Agent | BYOK 手动切换 + Plan/Act 双模型绑定 | [cline/cline](https://github.com/cline/cline) |
| Kilo Code（27k） | C / VS Code 编码 Agent + 自营网关 | 难度分类路由与缓存命中协同设计 | [Kilo-Org/kilocode](https://github.com/Kilo-Org/kilocode) |
| MCP | C / 协议/标准 | Model Context Protocol；research 中以 MCP 工具管理、MCP 网关、Codex as MCP server 形式出现 | — |
| models.dev | C / 模型目录注册表 | OpenCode 同团队维护的中心模型元数据目录 | [models.dev](https://models.dev) |

---

## 2. 主要对标项目

Pawork 的候选功能对照基于三家的公开功能面（功能对照见 [design.md](design.md) §6，转正登记见 [../ROADMAP.md](../ROADMAP.md) §3.3）。通用红线：纯 Rust 不引入 JS 运行时（排除 JS 插件生态路线）；无 TUI（CLI 交互模式 + S7 起的 GPUI Desktop，设计见 [gui-design.md](gui-design.md)）。

### 2.1 OpenCode

- **定位与目标**：TypeScript/Bun 的 TUI 编码 Agent，形态最全（Desktop beta、Web UI、IDE 扩展），自营 OpenCode Zen（按量）与 OpenCode Go（订阅）托管模型。
- **核心功能**：GitHub/GitLab CI bot；自定义命令、undo/redo、post-edit formatters；webfetch / websearch / question / todowrite 工具；References；JS 插件生态；会话分享；models.dev 模型目录（75+ provider）；内置 `task` 子代理（子 session + 权限派生 + 深度限制）。
- **与 Pawork 的关系**：参照——`task` 子代理与权限派生（F4 声明式绑定方向）、Anthropic 缓存断点摆放与前缀稳定性工程（F5）、429 重试策略（遵循 Retry-After、封顶 30s）；红线排除——TUI 形态、JS 插件生态；其同 provider 多账户空白正是 Pawork 的差异化机会（F1）。
- **关键链接**：[opencode.ai/docs](https://opencode.ai/docs/) · [anomalyco/opencode](https://github.com/anomalyco/opencode)（原 sst/opencode）· [providers](https://opencode.ai/docs/providers/) · [agents](https://opencode.ai/docs/agents/)。机制详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §2.1、§5.2。

### 2.2 Pi

- **定位与目标**：TypeScript/Bun monorepo（pi-ai / pi-agent-core / pi-coding-agent）的 TUI 编码 Agent；provider 无关 Context 与跨厂商 handoff 为一等能力。
- **核心功能**：Prompt Templates；Pi Packages（能力包打包 + npm/git 分发）；Project Trust；Message Queue（steering / follow-up）；thinking-level 用户控制；session tree / clone；llama.cpp 本地模型；订阅登录（Claude / OpenAI / Copilot plan）；OSS session 分享。
- **与 Pawork 的关系**：参照——provider 无关 Context（canonical domain 思路同构）、订阅 OAuth（F1-B plan 凭证）、精细缓存断点与 1h/24h 长 TTL（F5-B 显式族实践）、「核心不内置子代理」哲学（F4 取舍对照）；红线排除——TUI、npm/git 能力包分发（JS 生态路线）；其 Anthropic OAuth 的 Claude Code 伪装实现属身份伪装，Pawork 明确不采纳。
- **关键链接**：[pi.dev](https://pi.dev) · [earendil-works/pi](https://github.com/earendil-works/pi) · [docs/providers.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/providers.md) · [docs/models.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md) · [pi-ai README](https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)。机制详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §2.2、§5.2。

### 2.3 Codex

- **定位与目标**：OpenAI 官方编码 Agent 产品线：CLI + Desktop app + Cloud，配 IDE 扩展；产品与生态集成面最广的对标项。
- **核心功能**：图片输入 / web search / image generation / voice；Computer Use、Browser、Chrome 扩展；`/review` + GitHub PR 自动审查；GitHub Action；Slack / Linear 集成；Codex as MCP server；TS/Python SDK；本地 memories；scheduled tasks 产品；插件目录（连接器）；Bedrock 模型源。
- **与 Pawork 的关系**：参照——approval/sandbox 体系（对照 Pawork 的 policy / sandbox）、SDK 与 MCP server 对外集成形态、`prompt_cache_key = conversation_id` 会话亲和（F5-B 隐式族亲和键实践；其子会话 fork 缓存命中率 62%→9.6% 是子代理缓存取舍的直接反例）。
- **关键链接**：[developers.openai.com/codex](https://developers.openai.com/codex) · [client.rs（亲和键实现）](https://github.com/openai/codex/blob/d807d44a/codex-rs/core/src/client.rs) · [issue #21796](https://github.com/openai/codex/issues/21796)。机制详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §5.2、§5.5。

---

## 3. 多账户与路由专题项目

本节项目对应 [research/multi-account-quota-proposals.md](research/multi-account-quota-proposals.md) 的 F1–F6 方案（已确认）：F1 多账户模型与凭证、F2 额度感知与预算控制、F3 切换与路由策略、F4 子 Agent 跨供应商调用、F5 输入缓存策略控制、F6 对外账户池网关模式。

### 3.1 opencodex

- **定位与形态**：本地代理守护进程（Bun，默认端口 10100）+ Web dashboard + `ocx` CLI；把 Codex Responses API 翻译到 40+ provider，另向 Claude Code 提供 `/v1/messages` 网关。
- **核心机制**：① ChatGPT 账户池：5h / 周 / 30d 三窗口配额**主动探测**，`quota`（默认）/ round-robin / fill-first 三种池策略；② thread affinity：既有会话钉在原账户保 prompt cache，仅 failover / 亲和过期等触发 rebind；③ 429 → cooldown failover，401/403 → fail-closed（不静默换凭据）；④ Design B 注入：只改 `~/.codex/config.toml` 的 `openai_base_url` 一个字段。
- **与 Pawork 的关系**：F2-B 被动配额信号捕获与 F3-B「配额余量优先」策略、会话-账户亲和的直接参照；F6-A 下可作 openai-compatible 上游网关；config 布局是 G6 只读导入源候选；其本地凭证文件是导入参照，Pawork 额外要求 0600、原子写、损坏 fail-closed、掩码展示与日志脱敏。
- **链接**：[lidge-jun/opencodex](https://github.com/lidge-jun/opencodex) · [opencodex.me](https://opencodex.me) · [configuration](https://opencodex.me/reference/configuration/) · [How It Works](https://opencodex.me/getting-started/how-it-works/)。详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §3.1。

### 3.2 cc-switch

- **定位与形态**：跨平台桌面 GUI（Tauri 2，另有 Web/CLI 形态），统一管理 8 个工具（Claude Code、Codex、Gemini CLI 等）的供应商配置，50+ provider 预设。
- **核心机制**：① SSOT：provider 集中存 `~/.cc-switch/cc-switch.db`（SQLite），切换时原子写回各工具 live 配置文件（临时文件 + rename + 失败回滚 + backfill 回读）；② 切换粒度为全局配置级、手动为主（Claude Code 支持热切换），另有本地代理模式（auto-failover、circuit breaker）；③ 额度侧仅本地记账 dashboard 与可配置余额查询脚本，无配额驱动自动换号。
- **与 Pawork 的关系**：G6（F1 附属）导入源候选（cc-switch SQLite 布局）；「配置级切换 + 无 sticky」是 F3-B 的反面对照（切换即缓存作废）；导入后的 secret 直接写入 Pawork auth 文件，不落仓库或中间文件。
- **链接**：[farion1231/cc-switch](https://github.com/farion1231/cc-switch) · [cc-switch.cc](https://cc-switch.cc/) · [README_ZH](https://github.com/farion1231/cc-switch/blob/HEAD/README_ZH.md)。详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §3.2。

### 3.3 CLIProxyAPI

- **定位与形态**：Go 守护进程（默认端口 8317，支持 Docker / TLS / Go SDK 嵌入），把 Gemini CLI、Codex、Claude Code、Qwen Code 等 OAuth 订阅账户封装为 OpenAI / Gemini / Claude 兼容 API。
- **核心机制**：① 账户池：auth-dir 内一账户一 JSON token 文件，round-robin / 加权 / fill-first 轮询；② 额度耗尽被动检测：429 → 指数退避冷却（1s→30min）自动换凭据重试，另有降级链（switch-project / switch-preview-model）；③ session-affinity（v6.9.27+，默认关）：多来源 session ID + TTL SessionCache，明确以 prompt cache 命中率为目标；④ OAuth 后台自动刷新（过期前刷新、401 即时刷新重试）。
- **与 Pawork 的关系**：sticky session 与错误分类冷却是 F3-B 同构参照（V1 `ErrorClassifier` 语义更细）；auth-dir 是 G6 导入源候选；其 `codex.identity-confuse`（按所选账户重写 `prompt_cache_key` 与安装身份）属身份伪装，Pawork **明确不采纳**。
- **链接**：[router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) · [authentication](https://router-for-me-cliproxyapi.mintlify.app/concepts/authentication) · [routing](https://mintlify.wiki/router-for-me/CLIProxyAPI/concepts/routing) · [configuration options](https://help.router-for.me/configuration/options)。详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §3.3。

### 3.4 claude-relay-service

- **定位与形态**：Claude 订阅账户池中继（Node），在自发 API Key（`cr_` 前缀）层做限速、并发与模型黑名单控制。
- **核心机制**：① **内容 hash sticky session**：对可缓存前缀做 SHA-256，Redis 存 hash→账户映射（带 TTL），同会话固定账户保 prompt cache（作者明示频繁切换毁缓存且可能增加封号风险）；② 429/529 标记排除、5xx 临时暂停，并发用 Redis Sorted Set 排队；③ 每账户独立代理 IP，OAuth token AES 加密存 Redis。
- **与 Pawork 的关系**：sticky 保缓存路线的代表实现（F3-B 参照；Pawork 绑定键用自有 session_id，无需内容 hash）；「非限流 429（Extra usage is required）应透传而非锁账户」的错误分类教训已被 V1 错误表覆盖。
- **链接**：[Wei-Shaw/claude-relay-service](https://github.com/Wei-Shaw/claude-relay-service)。详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §4.4。

### 3.5 其余专题项目速查表

下表「详见」列均指 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) 对应章节。

| 项目 | 形态 | 与本仓相关的核心机制 | 详见 |
| --- | --- | --- | --- |
| [claude-code-router](https://github.com/musistudio/claude-code-router) | Claude Code 本地网关（TS） | 场景化路由（default / background / think / longContext）+ transformer 链（`cleancache` 剥除 cache_control）；in-band 子代理标签是 F4-C 不采纳反例 | §4.1 |
| [LiteLLM](https://github.com/BerriAI/litellm) | Proxy/Router（Python） | 层级预算（org/team/user/key）、cooldown/fallback、`PromptCachingDeploymentCheck` + session_affinity 缓存感知路由、缓存差价计费 | §4.2 |
| [new-api](https://github.com/QuantumNous/new-api) | 计费网关（Go） | 渠道-账户-令牌三层 quota 折算、渠道优先级/权重 + 渠道内多 key 轮询、失败自动禁用与换渠道重试 | §4.3 |
| [gpt-load](https://github.com/tbphp/gpt-load) | key 池透明代理（Go） | 累计失败拉黑 + 定时验证恢复、failover 状态码可配置 | §4.5 |
| [claude-code-hub](https://github.com/ding113/claude-code-hub) | 代理（Next.js/Hono） | Redis Lua 多维限流、session 绑定 `SET NX` 首成锁 + 健康度迁移、Redis 故障 Fail-Open 降级 | §4.6 |
| [meridian](https://github.com/rynfar/meridian) | 订阅桥（Claude Agent SDK） | 多 profile 即时切换 + 可选 sticky routing 保每账户缓存温热；不拦 OAuth 的合规路线 | §4.6 |
| [antigravity-claude-proxy](https://www.mintlify.com/badrisnarayanan/antigravity-claude-proxy/guides/load-balancing) | Google 账户池代理 | Hybrid / Sticky / Round-Robin 三策略；Sticky = 首条 user 消息 SHA256，限流 <2min 等待不切换 | §4.6 |
| [OmniRoute](https://github.com/diegosouzapw/OmniRoute) | 自托管网关（TS） | 19 种策略 + Auto-Combo 14 因子（含配额 headroom）、cacheAffinity 钉热缓存账号 | §8 |
| [Bifrost](https://github.com/maximhq/bifrost) | Go 网关 | 每 provider 多 key 权重随机 + 失败/限流切换、cache_control 透传 + 语义缓存插件 | §8 |
| [Envoy AI Gateway](https://github.com/envoyproxy/ai-gateway) | K8s 网关（CNCF v1.0 GA） | 统一 cache_control API 跨厂商翻译（F5-B adapter 映射层的同构先例）、内建 MCP 网关 | §8 |
| [uni-api](https://github.com/yym68686/uni-api) | Python 网关 | channel 加权 + channel 内 key 轮询（smart_round_robin）、单 YAML 极简派 | §8 |
| [sub2api](https://github.com/Wei-Shaw/sub2api) | 订阅池网关（Go + 管理台） | 订阅池 + key 分发 + 限额 + 拼车计费；CRS 同作者二代，ToS 风险最重 | §8 |
| [9router](https://github.com/decolua/9router) | 本地代理 | 40+ provider 多账号、订阅→低价→免费三级 fallback；**19 份安全通告（6 critical），选型反面警示** | §8 |
| [Cline](https://github.com/cline/cline) | VS Code 编码 Agent | BYOK 配置档手动切换；按模型清单在 system + 末 1–2 user 打 `cache_control`，粘滞交给 OpenRouter；Plan/Act 双模型绑定 | §8 |
| [Kilo Code](https://github.com/Kilo-Org/kilocode) | VS Code 编码 Agent + 自营网关 | 沿 Cline 谱系断点、`kilo-auto` 会话亲和分层路由、难度分类路由与缓存命中协同设计 | §8 |

> **收录标准**（沿用 research §8，2026-08-14 按 pushed_at 复核）：仅收录活跃维护项目，已归档或约 3 个月以上无提交者不作参考——TensorZero、Roo Code、Helicone AI Gateway、Arch/archgw、Portkey、one-api、gemini-balance 共 7 项已按此标准移除；移除记录与「外部网关存续风险 → 自持进程内能力（F6-A）更稳」结论见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §8。

---

## 4. 厂商 prompt caching 机制速查

| 厂商 | 触发方式 | 官方文档 |
| --- | --- | --- |
| Anthropic Claude | 显式块级 `cache_control: {type:"ephemeral"}` 断点（最多 4 个），另有 top-level 自动模式 | [prompt-caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) |
| OpenAI | 旧模型隐式自动（最长前缀匹配）；GPT-5.6+ 新增显式 `prompt_cache_breakpoint`，`prompt_cache_key` 参与缓存路由 | [prompt-caching](https://developers.openai.com/api/docs/guides/prompt-caching) |
| Google Gemini | 隐式（2.5+ 默认开启不可关）+ 显式 `CachedContent` API（资源引用，折扣有保证） | [caching](https://ai.google.dev/gemini-api/docs/generate-content/caching) |
| DeepSeek | 隐式硬盘 KV Cache，默认开启不可关（前缀单元需从 token 0 完整命中） | [kv_cache](https://api-docs.deepseek.com/guides/kv_cache) |
| 智谱 GLM | 隐式自动识别重复前缀（缓存计价仅标准 API 适用，GLM Coding Plan 套餐不适用） | [cache](https://docs.bigmodel.cn/cn/guide/capabilities/cache) |
| 阿里 Qwen / DashScope | 显式缓存 / 隐式 / 会话缓存（header `x-dashscope-session-cache: enable`）三种并存 | [context-cache](https://help.aliyun.com/zh/model-studio/context-cache) |
| Moonshot Kimi | 隐式全自动（前一请求 prompt tokens >256 触发） | [context-caching](https://platform.kimi.com/docs/guide/use-context-caching-feature-of-kimi-api) |
| AWS Bedrock | 显式：Converse 用 `cachePoint` 块，InvokeModel 用原生 `cache_control` | [prompt-caching](https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html) |
| OpenRouter | 转发/翻译上游 `cache_control`；provider sticky routing 自动粘同一上游 | [prompt-caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching) |

完整对照表（最小可缓存长度 / TTL / 计价 / 缓存键与隔离 / 用量字段）见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §5.1；Coding Agent 客户端的断点摆放实践见 §5.2；前缀稳定技巧见 §5.3。

---

## 5. 调研文档索引

| 文档 | 用途 |
| --- | --- |
| [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) | 外部实现逻辑调研全文：项目机制详查（§2–§4）、厂商缓存机制对照（§5）、模式归纳（§6）、与 V1 资产对照（§7）、参照项目对照总表与收录标准（§8） |
| [research/multi-account-quota-proposals.md](research/multi-account-quota-proposals.md) | F1–F6 实施方案与推荐（**已确认**）：多账户凭证、额度感知、切换路由、子 Agent 绑定、输入缓存、网关模式；含分阶段落地图 |
| [research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) | 决策记录 D1–D8 与并入 plan 任务书（决策唯一入口，含工作约定与疑问解答归档） |

F1–F6 与 [design.md](design.md) §5 已确认扩展功能族（G1–G7）的对应关系见 proposals 文档 §7。后续新增专题调研继续放入 [research/](research/) 目录，并在本手册（§1 总览 + 对应章节）登记。
