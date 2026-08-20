# 参照项目手册

> **用途**：任务开启阶段快速查阅各参照项目的目标、功能面与文档入口。本手册是**目录/索引层**，不展开机制细节：机制调研全文见 [research/](research/) 下各文档（深入处以「详见 research §N」跳转），各阶段功能 → 参照项目的映射见 [design.md](design.md) §4，**参照项目 → 功能规划**的反向分类见本文 §6，**V3 阶段（R0–R9）参照指引**见本文 §7。文中 star 数与项目事实为 **2026-08-18** 复核快照（GitHub API 全量复核；收录标准与历次移除记录见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §8——最近一次为 2026-08-18 功能重叠二次清理，移除 5 项，见 §3.6 注）。实现前应复核最新实态。

---

## 1. 总览

三类参照项目：**A** = 主要对标编码 Agent；**B** = 多账户、网关与路由专题；**C** = 其他编码 Agent、协议/标准与专项库（GUI 组件 / 沙箱）。star 为数量级快照。

| 项目 | 类别 / 形态 | 一句话定位 | 主链接 |
| --- | --- | --- | --- |
| OpenCode（199k） | A / TUI 编码 Agent（TS/Bun） | 多形态（TUI / Desktop beta / Web / IDE）编码 Agent，自营 Zen/Go 托管模型 | [anomalyco/opencode](https://github.com/anomalyco/opencode) |
| Pi（93k） | A / TUI 编码 Agent（TS/Bun monorepo） | provider 无关 Context 与 Pi Packages 能力包生态 | [earendil-works/pi](https://github.com/earendil-works/pi) |
| Codex（107k） | A / CLI + Desktop + Cloud | OpenAI 官方编码 Agent 产品线，SDK / MCP server 等集成面最广；CLI 主体为 Rust workspace（codex-rs） | [developers.openai.com/codex](https://developers.openai.com/codex) |
| DeepSeek Harness（157k） | A / Web + headless 编码 Agent（TS/Node） | DeepSeek 官方开源 harness：一切皆插件；append-only 会话事件为 SSOT（只读发布仓，不收 issue/PR） | [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) |
| opencodex（11k） | B / 本地代理 + dashboard（Bun） | Codex 协议翻译（40+ provider）+ ChatGPT 账户池三窗口配额路由 | [lidge-jun/opencodex](https://github.com/lidge-jun/opencodex) |
| Codex Router（2.5k） | B / 本地路由器 + 托盘（JS / LiteLLM） | 一安装多客户端：把外部模型并入 Codex / DeepSeek Harness / Gemini CLI 目录，凭证隔离转发 | [duolahypercho/codex-router](https://github.com/duolahypercho/codex-router) |
| cc-switch（128k） | B / Tauri 桌面应用 | 多工具供应商**配置级**切换（SSOT SQLite 原子写回） | [farion1231/cc-switch](https://github.com/farion1231/cc-switch) |
| CLIProxyAPI（48k） | B / Go 守护进程 | 多 OAuth 订阅账户封装为兼容 API（轮询 + 冷却 + 亲和） | [router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) |
| claude-relay-service（12.5k） | B / Claude 订阅池中继（Node） | 内容 hash sticky session 保 prompt cache（增长停滞，作者重心转向 sub2api；保留观察） | [Wei-Shaw/claude-relay-service](https://github.com/Wei-Shaw/claude-relay-service) |
| claude-code-router（37k） | B / Claude Code 本地网关（TS） | 场景化路由 + transformer 链改写 | [musistudio/claude-code-router](https://github.com/musistudio/claude-code-router) |
| LiteLLM（57k） | B / Proxy/Router（Rust core + Python SDK） | 层级预算 + 多策略路由 + 缓存感知路由 | [BerriAI/litellm](https://github.com/BerriAI/litellm) |
| new-api（46k） | B / 计费网关（Go，AGPL-3.0） | 渠道-账户-令牌三层 quota 折算计费 | [QuantumNous/new-api](https://github.com/QuantumNous/new-api) |
| OmniRoute（50k） | B / 自托管网关（TS） | 19 种策略 + cacheAffinity 因子钉热缓存账号（「免费聚合」画像与 9router 同质，关注安全面） | [diegosouzapw/OmniRoute](https://github.com/diegosouzapw/OmniRoute) |
| Bifrost（7.4k） | B / Go 网关 | 统一 API + 每 provider 多 key 治理，高性能叙事 | [maximhq/bifrost](https://github.com/maximhq/bifrost) |
| Envoy AI Gateway（1.9k） | B / K8s 网关（CNCF v1.0 GA） | 统一 cache_control API 跨厂商翻译 + 内建 MCP 网关 | [envoyproxy/ai-gateway](https://github.com/envoyproxy/ai-gateway) |
| sub2api（38k） | B / 订阅池网关（Go + 管理台，LGPL-3.0） | 订阅池 + key 分发 + 拼车计费（CRS 同作者二代） | [Wei-Shaw/sub2api](https://github.com/Wei-Shaw/sub2api) |
| 9router（26k） | B / 本地代理 | 40+ provider 多账号 + 三级 fallback；安全通告选型反面警示 | [decolua/9router](https://github.com/decolua/9router) |
| Cline（66k） | C / VS Code 编码 Agent | BYOK 手动切换 + Plan/Act 双模型绑定 | [cline/cline](https://github.com/cline/cline) |
| Kilo Code（27k） | C / VS Code 编码 Agent + 自营网关 | 难度分类路由与缓存命中协同设计 | [Kilo-Org/kilocode](https://github.com/Kilo-Org/kilocode) |
| MCP | C / 协议/标准 | Model Context Protocol；research 中以 MCP 工具管理、MCP 网关、Codex as MCP server 形式出现 | — |
| ACP（agent-client-protocol） | C / 协议/标准 | 编辑器 ↔ Agent 协议（Zed 生态）：capability ↔ 方法组一一映射、schema 单源派生多语言 SDK；已迁 agentclientprotocol 组织 | [agent-client-protocol](https://github.com/zed-industries/agent-client-protocol) |
| models.dev | C / 模型目录注册表 | OpenCode 同团队维护的中心模型元数据目录 | [models.dev](https://models.dev) |
| gpui-component（13k） | C / GPUI 组件库（Rust，Apache-2.0） | 60+ 组件 + ~140 语义 token 主题 + VirtualList 变高虚拟化；v0.5.1 适配 crates.io gpui ^0.2.2 | [longbridge/gpui-component](https://github.com/longbridge/gpui-component) |
| Zed `ui`/`theme` crates | C / GPUI 官方组件层（GPL-3.0） | ButtonLike/ContextMenu 等 ~40 组件与 theme token 组织；**只参 API 形状，不抄代码**（gpui 本体 Apache-2.0 除外） | [zed-industries/zed](https://github.com/zed-industries/zed/tree/main/crates/ui) |
| sandbox-runtime（srt） | C / 沙箱运行时库（TS，Apache-2.0） | Claude Code 官方沙箱隔离层：Seatbelt/bubblewrap profile 生成 + egress 本地代理域名白名单 | [anthropic-experimental/sandbox-runtime](https://github.com/anthropic-experimental/sandbox-runtime) |

---

## 2. 主要对标项目

Pawork 的候选功能对照基于四家的公开功能面（功能对照见 [design.md](design.md) §6，转正登记见 [../ROADMAP.md](../ROADMAP.md) §3.3）。通用红线：纯 Rust 不引入 JS 运行时（排除 JS 插件生态路线）；无 TUI（CLI 交互模式 + S7 起的 GPUI Desktop，设计见 [gui-design.md](gui-design.md)）。

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

### 2.4 DeepSeek Harness

- **定位与目标**：DeepSeek AI 官方开源 agent harness（`dsh`，MIT，developer preview）。口号是 Agent = Model + Harness、**一切皆插件**：模型、工具、技能、会话、沙箱、存储、循环、调度与 UI 均由 [Cordis](https://github.com/cordiverse/cordis) 插件组合，配置层可替换。默认形态是本地 Web UI（`npx @deepseek-ai/dsh web`），另有 headless profile 与 Python SDK。本文所述均指 [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)，与同名非官方适配器项目无关。
- **核心功能**：四套 preset——Standard（文件编辑 / Shell / 文件与网页检索 / Skills / 计划 / goals / 子代理 / 工作流）、PTC/Code Mode（经 Code Mode SDK 用一段 TypeScript 组合多步工具）、Minimal（持久 `bash` + `str_replace_editor`，用于基准）、Creator（在 Standard 上加运行时检查与 preset 创作）。仅追加 `SessionEvent` 日志是模型可见上下文的 SSOT（fork / resume / Trajectory 回放同源）；沙箱模式与审批策略是两个独立 knob，经 `workspace-write` / `danger-full-access` 等 permission preset 捆绑。工具面含 `tool-ask-user`、`tool-todo`、`tool-web`、`tool-skill`、`tool-subagent`、`tool-terminal`、MCP；LLM 适配覆盖 DeepSeek 与 Anthropic / OpenAI / Bedrock / Azure / Vertex。
- **与 Pawork 的关系**：参照——仅追加会话事件作为模型可见输入的重建源（对照 Pawork `AgentEventEnvelope` + append-only，是目前最接近的外部同形）；沙箱与审批分 knob（对照 S3/S4）；`ctx.sessions.fork`、headless、Python SDK（对照 S10）；Skills / plan / 子代理 / 工作流（对照 S9/S11）。红线排除——Cordis/JS「一切皆插件」、以 Web UI 为默认壳、Code Mode 生成并执行 TypeScript（JS 运行时）。Developer preview，官方声明会有破坏性变更；实现前复核实态，不把其插件 API 当冻结契约。
- **关键链接**：[deepseek.com/harness](https://deepseek.com/harness) · [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) · [架构](https://deepseek-harness.github.io/deepseek-harness/en/reference/) · [权限预设](https://deepseek-harness.github.io/deepseek-harness/en/reference/subsystems/permission-presets) · [Python SDK](https://deepseek-harness.github.io/deepseek-harness/en/guide/python-sdk)。本仓暂无独立 research 专章（2026-08-17 按公开功能面登记）。

---

## 3. 多账户与路由专题项目

本节项目对应 [research/multi-account-quota-proposals.md](research/multi-account-quota-proposals.md) 的 F1–F6 方案（已确认）：F1 多账户模型与凭证、F2 额度感知与预算控制、F3 切换与路由策略、F4 子 Agent 跨供应商调用、F5 输入缓存策略控制、F6 对外账户池网关模式。

### 3.1 opencodex

- **定位与形态**：本地代理守护进程（Bun，默认端口 10100）+ Web dashboard + `ocx` CLI；把 Codex Responses API 翻译到 40+ provider，另向 Claude Code 提供 `/v1/messages` 网关。
- **核心机制**：① ChatGPT 账户池：5h / 周 / 30d 三窗口配额**主动探测**，`quota`（默认）/ round-robin / fill-first 三种池策略；② thread affinity：既有会话钉在原账户保 prompt cache，仅 failover / 亲和过期等触发 rebind；③ 429 → cooldown failover，401/403 → fail-closed（不静默换凭据）；④ Design B 注入：只改 `~/.codex/config.toml` 的 `openai_base_url` 一个字段。
- **与 Pawork 的关系**：F2-B 被动配额信号捕获与 F3-B「配额余量优先」策略、会话-账户亲和的直接参照；F6-A 下可作 openai-compatible 上游网关；config 布局是 G6 只读导入源候选；其本地凭证文件是导入参照，Pawork 额外要求 0600、原子写、损坏 fail-closed、掩码展示与日志脱敏。
- **链接**：[lidge-jun/opencodex](https://github.com/lidge-jun/opencodex) · [opencodex.me](https://opencodex.me) · [configuration](https://opencodex.me/reference/configuration/) · [How It Works](https://opencodex.me/getting-started/how-it-works/)。详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §3.1。

### 3.2 Codex Router

- **定位与形态**：本地路由器（JS + 内嵌 LiteLLM，默认 `127.0.0.1:4202`）+ 本机托盘；社区项目，与 OpenAI / opencodex 均无隶属。一次安装、一套凭证，把外部模型并入 [Codex](https://developers.openai.com/codex) 原生 picker，并同样发布到 DeepSeek Harness 与 Gemini CLI。宿主仍拥有 Agent 循环、工具、权限、MCP 与会话；路由器只做推理转发与协议翻译。
- **核心机制**：① Design B 注入：托管改写 `~/.codex/config.toml` 的 `openai_base_url` + `model_catalog_json`，把外部条目并入 Codex 原生目录；② 凭证隔离：丢弃入站 Codex 凭据，只向所选上游注入对应 OAuth/API key（Kimi Code / Grok CLI 会话复用，不读 Copilot 官方凭据库）；③ 注册表驱动：`config/` 校验过的 provider/model 才进 picker，凭证感知（无凭据不展示）；④ 额度耗尽 failover（默认开）：仅 402 / 余额耗尽 / 需等待 >1min 的 429 才换到已启用的下一模型，坏 key / 未知模型 / 宕机仍原样报错；提供商声明的复位窗口会冷却（上限 6h）；⑤ 可选旧工具结果老化与外部模型 compaction 摘要；⑥ 文本模型的 vision bridge（把粘贴图交给已启用视觉模型再代换成证据文本）。
- **与 Pawork 的关系**：与 opencodex 同属「截 Codex `base_url` 的本地路由器」，但重点是**多客户端共享的凭证隔离目录**，不是 ChatGPT 账户池。参照——S0/S6 openai-compatible 上游与六条首发通道的端点/凭证形态（GLM Coding Plan、OpenCode Go、Qwen Token Plan、DeepSeek、xAI OAuth）；S5 工具结果老化 / 外部 compaction 对照；S9 G6 导入源候选（托管 `config.toml` 块 + `~/.codex/codex-router` 状态目录）；S11 F2/F3 的窄错误分类 failover 与冷却（对照，不是 sticky 账户池）；S11 F4 的「仅注册表验证过的模型可作子代理」；F6-A 下可作 openai-compatible 上游。红线排除——JS/LiteLLM 运行时、login-free 把外部模型别名到原生 GPT slug、匿名免费网关、身份伪装。本仓暂无独立 research 专章（2026-08-18 按公开 README / HOW-IT-WORKS 登记）。
- **链接**：[duolahypercho/codex-router](https://github.com/duolahypercho/codex-router) · [How it works](https://github.com/duolahypercho/codex-router/blob/main/docs/HOW-IT-WORKS.md) · [Install](https://github.com/duolahypercho/codex-router/blob/main/docs/INSTALL.md) · [Compatible apps](https://github.com/duolahypercho/codex-router/blob/main/docs/COMPATIBLE-APPS.md)。

### 3.3 cc-switch

- **定位与形态**：跨平台桌面 GUI（Tauri 2，另有 Web/CLI 形态），统一管理 8 个工具（Claude Code、Codex、Gemini CLI 等）的供应商配置，50+ provider 预设。
- **核心机制**：① SSOT：provider 集中存 `~/.cc-switch/cc-switch.db`（SQLite），切换时原子写回各工具 live 配置文件（临时文件 + rename + 失败回滚 + backfill 回读）；② 切换粒度为全局配置级、手动为主（Claude Code 支持热切换），另有本地代理模式（auto-failover、circuit breaker）；③ 额度侧仅本地记账 dashboard 与可配置余额查询脚本，无配额驱动自动换号。
- **与 Pawork 的关系**：G6（F1 附属）导入源候选（cc-switch SQLite 布局）；「配置级切换 + 无 sticky」是 F3-B 的反面对照（切换即缓存作废）；导入后的 secret 直接写入 Pawork auth 文件，不落仓库或中间文件。
- **链接**：[farion1231/cc-switch](https://github.com/farion1231/cc-switch) · [cc-switch.cc](https://cc-switch.cc/) · [README_ZH](https://github.com/farion1231/cc-switch/blob/HEAD/README_ZH.md)。详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §3.2。

### 3.4 CLIProxyAPI

- **定位与形态**：Go 守护进程（默认端口 8317，支持 Docker / TLS / Go SDK 嵌入），把 Gemini CLI、Codex、Claude Code、Qwen Code 等 OAuth 订阅账户封装为 OpenAI / Gemini / Claude 兼容 API。
- **核心机制**：① 账户池：auth-dir 内一账户一 JSON token 文件，round-robin / 加权 / fill-first 轮询；② 额度耗尽被动检测：429 → 指数退避冷却（1s→30min）自动换凭据重试，另有降级链（switch-project / switch-preview-model）；③ session-affinity（v6.9.27+，默认关）：多来源 session ID + TTL SessionCache，明确以 prompt cache 命中率为目标；④ OAuth 后台自动刷新（过期前刷新、401 即时刷新重试）。
- **与 Pawork 的关系**：sticky session 与错误分类冷却是 F3-B 同构参照（V1 `ErrorClassifier` 语义更细）；auth-dir 是 G6 导入源候选；其 `codex.identity-confuse`（按所选账户重写 `prompt_cache_key` 与安装身份）属身份伪装，Pawork **明确不采纳**。
- **链接**：[router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) · [authentication](https://router-for-me-cliproxyapi.mintlify.app/concepts/authentication) · [routing](https://mintlify.wiki/router-for-me/CLIProxyAPI/concepts/routing) · [configuration options](https://help.router-for.me/configuration/options)。详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §3.3。

### 3.5 claude-relay-service

- **定位与形态**：Claude 订阅账户池中继（Node），在自发 API Key（`cr_` 前缀）层做限速、并发与模型黑名单控制。
- **核心机制**：① **内容 hash sticky session**：对可缓存前缀做 SHA-256，Redis 存 hash→账户映射（带 TTL），同会话固定账户保 prompt cache（作者明示频繁切换毁缓存且可能增加封号风险）；② 429/529 标记排除、5xx 临时暂停，并发用 Redis Sorted Set 排队；③ 每账户独立代理 IP，OAuth token AES 加密存 Redis。
- **与 Pawork 的关系**：sticky 保缓存路线的代表实现（F3-B 参照；Pawork 绑定键用自有 session_id，无需内容 hash）；「非限流 429（Extra usage is required）应透传而非锁账户」的错误分类教训已被 V1 错误表覆盖。
- **链接**：[Wei-Shaw/claude-relay-service](https://github.com/Wei-Shaw/claude-relay-service)。详见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §4.4。

### 3.6 其余专题项目速查表

下表「详见」列均指 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) 对应章节。

| 项目 | 形态 | 与本仓相关的核心机制 | 详见 |
| --- | --- | --- | --- |
| [claude-code-router](https://github.com/musistudio/claude-code-router) | Claude Code 本地网关（TS） | 场景化路由（default / background / think / longContext）+ transformer 链（`cleancache` 剥除 cache_control）；in-band 子代理标签是 F4-C 不采纳反例 | §4.1 |
| [LiteLLM](https://github.com/BerriAI/litellm) | Proxy/Router（Rust core + Python SDK） | 层级预算（org/team/user/key）、cooldown/fallback、`PromptCachingDeploymentCheck` + session_affinity 缓存感知路由、缓存差价计费 | §4.2 |
| [new-api](https://github.com/QuantumNous/new-api) | 计费网关（Go，AGPL-3.0） | 渠道-账户-令牌三层 quota 折算、渠道优先级/权重 + 渠道内多 key 轮询、失败自动禁用与换渠道重试 | §4.3 |
| [OmniRoute](https://github.com/diegosouzapw/OmniRoute) | 自托管网关（TS） | 19 种策略 + Auto-Combo 14 因子（含配额 headroom）、cacheAffinity 钉热缓存账号 | §8 |
| [Bifrost](https://github.com/maximhq/bifrost) | Go 网关 | 每 provider 多 key 权重随机 + 失败/限流切换、cache_control 透传 + 语义缓存插件 | §8 |
| [Envoy AI Gateway](https://github.com/envoyproxy/ai-gateway) | K8s 网关（CNCF v1.0 GA） | 统一 cache_control API 跨厂商翻译（F5-B adapter 映射层的同构先例）、内建 MCP 网关 | §8 |
| [sub2api](https://github.com/Wei-Shaw/sub2api) | 订阅池网关（Go + 管理台，LGPL-3.0） | 订阅池 + key 分发 + 限额 + 拼车计费；CRS 同作者二代，ToS 风险最重 | §8 |
| [9router](https://github.com/decolua/9router) | 本地代理 | 40+ provider 多账号、订阅→低价→免费三级 fallback；**19 份安全通告（6 critical），选型反面警示** | §8 |
| [Cline](https://github.com/cline/cline) | VS Code 编码 Agent | BYOK 配置档手动切换；按模型清单在 system + 末 1–2 user 打 `cache_control`，粘滞交给 OpenRouter；Plan/Act 双模型绑定 | §8 |
| [Kilo Code](https://github.com/Kilo-Org/kilocode) | VS Code 编码 Agent + 自营网关 | 沿 Cline 谱系断点、`kilo-auto` 会话亲和分层路由、难度分类路由与缓存命中协同设计 | §8 |

> **收录标准**（沿用 research §8）：仅收录活跃维护、且在表内承担**不可替代角色**的项目。历次移除：① 2026-08-14 按 pushed_at 复核活跃度，移除 TensorZero、Roo Code、Helicone AI Gateway、Arch/archgw、Portkey、one-api、gemini-balance 共 7 项；② 2026-08-18 GitHub API 全量复核后按「同功能与实现思路可由表内更强项目替代 + star 停滞或活跃不足」二次清理，移除 gpt-load、uni-api、claude-code-hub、meridian、antigravity-claude-proxy 共 5 项。逐项理由、替代关系与「外部网关存续风险 → 自持进程内能力（F6-A）更稳」结论见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §8；被移除项的机制原文仍保留在 research §4.5/§4.6/§5.4（历史快照）。同日复核另记：claude-relay-service 增长停滞（作者重心转向 sub2api），仍为 G3 sticky 主参照，保留观察；meridian 仓库无 LICENSE（移除的附加原因：不可参考其代码）。

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

F1–F6 与 [design.md](design.md) §5 已确认扩展功能族（G1–G7）的对应关系见 proposals 文档 §7。后续新增专题调研继续放入 [research/](research/) 目录，并在本手册（§1 总览 + 对应章节 + §6 反向分类）登记。

---

## 6. 参照项目按功能规划分类

> 正向映射（功能 → 参照）以 [design.md](design.md) §4 / §5 为准；本节是**反向索引**：打开某个参照项目时，它在当前规划里参与哪些功能。标「主」= 实现时优先对照；「对照」= 取舍或形态参考；「反例」= 明确不采纳。S12 / S13 是工程审查与整改，无外部功能对标。**V3 阶段轴（R0–R9 → 参照）见 §7。**

### 6.1 按规划轴

| 规划轴 | 主参照 | 对照 | 反例 / 排除 |
| --- | --- | --- | --- |
| **S0** 对话 CLI / 模型目录 / openai-compatible `base_url` | OpenCode、Codex、models.dev | Pi；opencodex / Codex Router / CLIProxyAPI（自建网关上游） | OpenCode/Pi TUI |
| **S1** 事件流落盘 / resume / headless JSONL | DeepSeek Harness、Codex | Pi session tree、OpenCode SQLite | — |
| **S2** 只读工具 / 工具循环 / 双协议 | OpenCode、Codex | Pi Anthropic tools；本文 §4 缓存协议 | — |
| **S3** 写入工具 / 审批档 | Codex approval、OpenCode permission | DeepSeek Harness 沙箱与审批分 knob、Pi Project Trust | — |
| **S4** 命令执行 / 沙箱 | Codex sandbox、V1 exec | DeepSeek Harness `ctx.sandbox`、OpenCode `permission.bash` | — |
| **S5** 上下文预算 / 用量 / registry | OpenCode compaction、models.dev、Pi | LiteLLM 缓存差价；Codex Router 旧工具结果老化与外部 compaction 摘要 | 把 compaction 当免费缓存续命 |
| **S6** 六通道 / 文件凭证 / OAuth / `/model` | Codex auth 形态、各厂商官方 API | OpenCode/Pi 切换与 handoff；Codex Router 的 GLM / OpenCode Go / Qwen / DeepSeek / xAI 端点与凭证形态 | Pi Anthropic OAuth 伪装；CLIProxyAPI `identity-confuse` |
| **S7** 最小 Agent GUI | Codex Desktop、OpenCode Desktop/Web | DeepSeek Harness Trajectory（默认壳不吸收） | 把 Web UI 当默认壳 |
| **S8** Git / checkpoint | V1 checkpoint；OpenCode `/undo` `/redo`（粒度对照） | — | 把 turn 级 undo 当成 Run 级 rollback |
| **S9** MCP / Skills / 兼容导入 | MCP 官方、OpenCode/Codex/DeepSeek Harness | G6 导入源：cc-switch、CLIProxyAPI auth-dir、opencodex config、Codex Router 托管 `config.toml` + 状态目录 | 覆盖用户自有 skills |
| **S10** headless / SDK / fork / 服务化 | Codex SDK、OpenCode serve、Pi `createAgentSession`、DeepSeek Harness headless | Codex Router 的「一安装多客户端」是 F6 对照，不是 Pawork 对外网关 | — |
| **S11** 多 Agent / 账户池 / 额度 / 路由 | OpenCode `task` 子代理；opencodex / CLIProxyAPI / CRS（F1–F3）；LiteLLM 预算 | Codex Router 窄错误 failover 与「仅注册表验证模型可作子代理」；Pi「核心不内置子代理」 | CCR in-band 子代理标签（F4-C）；请求级默认轮换（F3-C） |
| **G1 / F1** 同 Provider 多账户与 plan 凭证 | opencodex 账户池、CLIProxyAPI auth-dir | Codex / Pi 订阅 OAuth；Codex Router 复用 Kimi/Grok CLI 会话（单凭证隔离，不是池） | 身份伪装换号 |
| **G2 / F2** 额度感知与预算 | opencodex 三窗口探测、LiteLLM 层级预算 | CLIProxyAPI-Plus 阈值停用；Codex Router 托盘用量 + 仅信提供商复位窗口 | 主动刷配额接口（F2-C/D 冻结） |
| **G3 / F3** 缓存感知亲和路由 | CRS sticky、CLIProxyAPI session-affinity、opencodex thread affinity | OmniRoute cacheAffinity、LiteLLM `session_affinity`；Codex Router 只做额度耗尽换**模型**，不做会话-账户钉扎 | cc-switch 配置级切换（缓存作废）；请求级轮换 |
| **G4 / F4** 子 Agent 声明式绑定 | OpenCode `agent.model` + 权限派生 | DeepSeek Harness `tool-subagent`；Codex Router 仅 registry-proven 模型可作 v2 spawn | CCR `<CCR-SUBAGENT-MODEL>` 标签 |
| **G5 / F5** canonical 输入缓存 | Anthropic / OpenAI 官方；Pi / OpenCode 断点实践 | Envoy AI Gateway 跨厂商 `cache_control` 翻译；Cline/Kilo 断点 | CCR `cleancache` 作为默认；响应缓存（F5-C） |
| **G6** 账户/端点只读导入 | cc-switch SQLite、CLIProxyAPI auth-dir、opencodex config、官方 Codex/Claude 布局 | Codex Router 托管 config 块与 `~/.codex/codex-router` 状态目录 | 导入 secret 落仓库或中间文件 |
| **G7 / F6** 对外账户池网关 | —（已确认不内建） | opencodex / CLIProxyAPI / Codex Router 均可当 openai-compatible **上游** | 独立网关 app（F6-C）；订阅转售（sub2api） |

### 6.2 按项目

| 项目 | 类别 | 参与的功能规划（打开它时看这些） |
| --- | --- | --- |
| OpenCode | A | S0 对话/目录/配置分离；S2–S4 工具与权限；S5 compaction/用量；S6 `/models`；S7 流式 GUI；S8 turn undo 对照；S9 MCP/rules；S10 SDK/子 session；S11 `task` 子代理（G4）；候选 A1–A4、B1–B4、D1–D5 |
| Pi | A | S0 `auth.json`；S1 树形 session；S3 Project Trust；S5 精细断点/长 TTL（G5）；S6 跨厂商 handoff 与订阅 OAuth；S9/S10 profiles 与 fork；S11「核心不内置子代理」对照；候选 C1、D3、D8 |
| Codex | A | S0/S1 CLI 与 resume；S3/S4 approval + sandbox；S6 ChatGPT OAuth 与文件凭证形态；S7 Desktop 壳；S9 AGENTS.md / MCP；S10 SDK / app-server；G5 `prompt_cache_key` 亲和；候选 B1/B5–B7、C2/C3、D1/D2/D6/D7 |
| DeepSeek Harness | A | S1 append-only `SessionEvent` SSOT；S3/S4 沙箱与审批分轨；S7 Trajectory（不吸收默认壳）；S9 skills；S10 headless/fork；S11 子代理与工作流；候选 B2/B3/B8/B9、D4 |
| opencodex | B | S0/F6-A 上游网关；G1 账户池；G2 三窗口配额；G3 thread affinity；G6 导入源；S11 主体 |
| **Codex Router** | B | S0/S6 六通道端点与凭证形态、openai-compatible 上游；S5 工具结果老化 / 外部 compaction；S9 G6 导入源（托管 `config.toml` + 状态目录）；S11 F2/F3 **窄**额度 failover（换模型不换账户池）；S11 F4 仅验证过的模型可作子代理；G7/F6-A 上游。**不**作 G1 ChatGPT 账户池或 G3 sticky 主参照 |
| cc-switch | B | G6 导入源主参照；G3 反面对照（配置级切换毁缓存） |
| CLIProxyAPI | B | G1 auth-dir；G3 session-affinity；G6 导入；S11 冷却/降级；F6-A 上游。`identity-confuse` 反例 |
| claude-relay-service | B | G3 sticky 保缓存主参照 |
| claude-code-router | B | S11/G4 场景路由对照；F4-C in-band 标签反例；G5 `cleancache` 对照 |
| LiteLLM | B | G2 层级预算；G3 缓存感知路由；S5 缓存差价计费。Codex Router 把它当翻译层，Pawork 不引入该运行时 |
| new-api | B | G2 三层 quota 折算；渠道内多 key 轮询与失败自动禁用/恢复（原 gpt-load / uni-api 参照面并入此项与 CLIProxyAPI） |
| OmniRoute | B | G3 cacheAffinity / 配额 headroom（原 antigravity「缓存命中一等权衡」参照面并入此项与 CRS） |
| Bifrost | B | G1 每 provider 多 key |
| Envoy AI Gateway | B | G5 统一 `cache_control` 跨厂商翻译 |
| sub2api | B | G7 ToS / 拼车反例 |
| 9router | B | 选型安全反例 |
| Cline | C | G5 断点摆放；Plan/Act 双模型对照 |
| Kilo Code | C | G3 难度分类路由与缓存协同 |
| MCP | C | S9 MCP client；候选 B7（Pawork 作 MCP server）；R3 capabilities 协商措辞对照 |
| ACP | C | S10 `acp serve` 协议事实源；R3 capability ↔ 方法组映射与 schema 单源派生对照 |
| models.dev | C | S0/S5 模型 registry；R5 通道注册表数据化对照 |
| gpui-component | C | R8 组件库 / theme token / VirtualList 主参照（Apache-2.0，可借鉴实现） |
| Zed `ui`/`theme` | C | R8 组件 API 形状与 token 组织对照（GPL-3.0：只参形状不抄代码） |
| sandbox-runtime（srt） | C | R7 沙箱策略语义与 egress 架构主参照（写 allow-only / 读挖洞 / 域名白名单 + 双代理） |

---

## 7. V3 阶段参照指引（R0–R9）

> 与 [../ROADMAP.md](../ROADMAP.md) §2 阶段表对应，开工核对时与阶段任务书（[../plan/](../plan/)）、[design.md](design.md) §4（V2 功能 ↔ 参照映射，仍是功能面事实源）配合使用。本节为 **2026-08-18** 调研快照（GitHub API 复核 + 三路专项调研），执行各波前按 [../v3_plan.md](../v3_plan.md) §5.2 重验外部实态。「主参照」= 设计时优先对照；「对照 / 反例」= 取舍参考或明确不采纳。R0/R2/R4/R9 以仓库内证据驱动，无外部主参照。

### 7.1 阶段 → 参照映射

| 阶段 | 主参照 | 对照 / 反例 | 关键参照点 |
| --- | --- | --- | --- |
| **R0** 决策收口与库存裁决 | —（仓库内消费面证据驱动） | Pi「核心不内置」哲学（裁剪心态）；LiteLLM org/team/user/key 多租户层级（D1 单机决议的反面形态） | 归档判据全部来自本仓扫描；不引外部 |
| **R1** 包合并 39→21 | Codex [codex-rs workspace](https://github.com/openai/codex/tree/main/codex-rs)（布局纪律） | Pi 三层 monorepo（ai / agent-core / coding-agent ↔ domain / engine / host 分层对照）；DeepSeek Harness「一切皆插件」拆包形态（不采纳） | codex-rs：统一 `codex-` 前缀、扁平布局 + 少量分组子目录、`workspace.dependencies` 集中声明、每个域「protocol → 宿主 → client」三层切分。**反面教材**：其 134 成员微 crate 增殖——Pawork 方向相反（39→21），只抄布局纪律不抄粒度 |
| **R2** 依赖治理 | —（依赖用面审计驱动） | rmcp 官方仓库 [modelcontextprotocol/rust-sdk](https://github.com/modelcontextprotocol/rust-sdk)（波 C 专项） | rmcp 3.x wire 兼容性以官方 changelog + 本仓 64 条 MCP 契约测试（S13A 整改后实态计数） + 真实 server 冒烟为准；兼容则升，破坏则锁 `=2.2.0` 登记。✅ 升级落地（波 C 2026-08-20：`=3.1.3`，冒烟与基线逐字节一致，MSRV 1.85→1.88，lock 830→826） |
| **R3** 协议与投影同源化 | Codex [app-server-protocol](https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol) 宏 registry；ACP capability 模型 | MCP capabilities 协商措辞；DeepSeek Harness append-only 事件 → Trajectory 投影同源（既有 S1 参照的延伸） | codex：四个宏单点登记（variant / wire 名 / 参数响应类型 / experimental 标记），同一展开派生 dispatch enum、TS/JSON schema、experimental 门控名单——「宣告 = 授权 = dispatch = schema 同源」的直接同形；ACP：capability ↔ 方法组（`session/*`、`fs/*`、`terminal/*`）一一映射、capability 缺省 false 即禁用、`schema/v1/schema.json` 单源派生多语言 SDK；MCP 仅有「MUST only use capabilities that were successfully negotiated」措辞，**无字面「未宣告即拒绝」条款——fail-closed 语义须 Pawork 自建为结构保证** |
| **R4** 宿主拆解与可靠性内核 | —（内部契约工程） | codex-rs core / cli 职责边界（拆解规模感）；DeepSeek Harness 插件化服务切分（形态反例：拆服务不拆插件） | 幂等 CommandLedger、K-02 审批落盘、降级事件化、ACP actor 化均无外部同形；依托 R3 registry 分发与本仓 88+ 条 app 契约测试护航 |
| **R5** Provider 中立化与凭证收口 | Codex Router 注册表驱动目录；[models.dev](https://models.dev)；Envoy AI Gateway 统一 cache_control 翻译 | Pi 自维护模型目录 + compat 矩阵、`auth.json` 解析优先级（CLI > auth.json > env > models.json）；OpenCode transform 归一化与 auth/config 分离 | 通道 preset 数据化 ↔ Codex Router「仅注册表验证过的 provider/model 进 picker」（本文 §3.2）；K-10 能力收口 ↔ Envoy「统一 cache API → 各家 cachePoint 翻译」（F5-B adapter 映射同构先例）；credential locator 合一 ↔ Pi/OpenCode 单文件凭证 + 解析链实践 |
| **R6** 会话分支模型原生化 | DeepSeek Harness `ctx.sessions.fork`（turn 边界 + lineage 元数据） | Pi per-entry `parentId` 树（GUI 分支导航交互语义）；OpenCode 子 session（`parentID`）；**反面教材**：Claude Code 跨文件 DAG 重建（昂贵且脆弱） | DSH 不变量「fork 只许切在 turn 边界，越界即拒绝」+ `(parentSession, seedLength)` lineage 直接翻译为 Pawork `(parent_branch_id, fork_point_seq)`；差别：DSH 深拷贝 seed 事件，Pawork 单表 `branch_id` 引用零拷贝（单表方案优势）。**K-05 导入映射要点**：Claude `~/.claude/projects/**/*.jsonl`——`uuid/parentUuid` 链（同 parentUuid 多子 = 分叉点）、`isSidechain`/`agent-*.jsonl` = 子代理支线、`system.compact_boundary`+`logicalParentUuid` = 压缩边界、`tool_use{id}`↔`tool_result{tool_use_id}` 配对；Codex rollout `{timestamp,type,payload}`——首行 `session_meta`（含 `forked_from_id` 跨会话 lineage）、`turn_context` = turn 边界与模型、`function_call{call_id}`↔`function_call_output{call_id}` 配对、`reasoning.encrypted_content` 不可解只能存占位。两格式均非稳定契约：导入器逐行容错，未知 type 落为不透明扩展事件 |
| **R7** 执行面真隔离 | Codex sandboxing（[docs/sandbox.md](https://github.com/openai/codex/blob/main/docs/sandbox.md)，deny-default Seatbelt，Rust 直接可抄结构）；[sandbox-runtime（srt）](https://github.com/anthropic-experimental/sandbox-runtime)（Claude Code 沙箱层，策略语义事实源） | [codex-network-proxy](https://github.com/openai/codex/blob/main/codex-rs/network-proxy/README.md)（egress 纯 Rust 实现）；DeepSeek Harness 沙箱/审批分 knob（既有 S3/S4 参照） | codex：`(deny default)` base sbpl + 可写根参数化组装 + `.git`/metadata `require-not` 挖洞 + 网络 fail-closed + `seatbelt_tests.rs` 回归；Linux 为 Landlock + seccomp（+bwrap）。srt 策略语义：写 = allow-only（默认全拒 + `allowWrite` 白名单）、读 = deny-then-allow（默认可读 + `denyRead` 挖洞——**两家读侧都不是全 deny**）、`.bashrc`/`.git/hooks`/`.env` 永久禁写、egress = 域名白名单（deny 优先 + 面向模型的拒绝理由）。K-09 若做 egress（ADR-041 选项 a）：采「本地策略代理 + 沙箱内仅放行 loopback 代理端口 + 域名白名单」两层模型（srt 架构 + codex-network-proxy 实现；注意两家均如实标注 DNS rebinding 局限） |
| **R8** GUI 组件化与 Desktop 收口 | [gpui-component](https://github.com/longbridge/gpui-component) **v0.5.1 tag**（Apache-2.0；该版依赖 crates.io gpui ^0.2.2 与本仓 ADR-035 锁定一致，主干已改跟 Zed git 主干，勿参主干） | Zed [`crates/ui`](https://github.com/zed-industries/zed/tree/main/crates/ui)/`crates/theme`（**GPL-3.0：只参 API 形状不抄代码**）；Codex Desktop / OpenCode Desktop 壳形态（既有 S7 参照） | gpui-component：60+ 组件全覆盖 R8 十一组件清单、`ThemeColor` ~140 语义 token + `ActiveTheme` trait（对照 theme.rs ~20 token 的收敛目标）、`VirtualList` 变高虚拟化（对照 Timeline `list()` 改造）；Zed ui：`ButtonLike` 基座 + `ButtonCommon` trait（id/style/size/tooltip builder）、enum 型 `ContextMenu`（Header/Entry/Separator + `anchored()` + FocusHandle）——组件组织方式参照 |
| **R9** 一致性收口 | —（内部核对） | — | 本节使用记录纳入 R9「文档三处一致」核查；参照快照过期项按 §3.6 收录标准复核 |

### 7.2 使用纪律

- **许可证红线**：GPL 系（Zed `ui`/`theme`）与无 LICENSE 仓库只参照 API 形状与机制思路，禁止复制代码；Apache-2.0 / MIT 系（codex-rs、gpui-component、srt）可借鉴实现但仍以自写为主，引入片段须记录出处。
- **参照不改契约**：对照外部设计时，本仓冻结契约（[design.md](design.md) §3.2、[v2-summary.md](v2-summary.md) §4）优先；外部形状与冻结契约冲突的，走 ADR 而不是「顺手对齐」。
- **快照时效**：本节结论为 2026-08-18 快照；R6/R7/R8 等距今较远的阶段开工时，按 [../v3_plan.md](../v3_plan.md) §5.2 重验参照项目实态（版本、许可证、API 形状），漂移即回写本节。
- **登记约定**：2026-08-18 随本节新入册 ACP、gpui-component、Zed `ui`/`theme`、sandbox-runtime 四项（§1 总览与 §6.2 已同步）；R8 任务书引用的「Zed ui 与 gpui-component API 形状」自此在本手册有落点。后续 V3 专项调研继续按 §5 约定登记。

