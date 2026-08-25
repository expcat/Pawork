# 参照项目手册

> **用途**：任务开启阶段快速查阅各参照项目的目标、功能面与文档入口。本手册是**目录/索引层**，不展开机制细节：机制调研全文见本文附录 A–C（深入处以「详见附录 A §N」跳转），各阶段功能 → 参照项目的映射见 [design.md](design.md) §2，**参照项目 → 功能规划**的反向分类见本文 §6，旧 V3（R0–R9）参照快照见本文 §7；当前 UI 主线的 Codex/Zed/Cursor/Claude/VS Code 与测试方法见 [UI 参照调研](../plan/UI-reference-research.md)。多账户/配额/缓存调研已并入本文附录 A/B/C。文中 star 数与项目事实为 **2026-08-18** 快照；实现前应复核最新实态。

---

## 1. 总览

三类参照项目：**A** = 主要对标编码 Agent；**B** = 多账户、网关与路由专题；**C** = 其他编码 Agent、协议/标准与专项库（GUI 组件 / 沙箱）。star 为数量级快照。

| 项目 | 类别 / 形态 | 一句话定位 | 主链接 |
| --- | --- | --- | --- |
| OpenCode（199k） | A / TUI 编码 Agent（TS/Bun） | 多形态（TUI / Desktop beta / Web / IDE）编码 Agent，自营 Zen/Go 托管模型 | [anomalyco/opencode](https://github.com/anomalyco/opencode) |
| Pi（93k） | A / TUI 编码 Agent（TS/Bun monorepo） | provider 无关 Context 与 Pi Packages 能力包生态 | [earendil-works/pi](https://github.com/earendil-works/pi) |
| Codex（111k） | A / CLI + Desktop + Cloud（Rust） | OpenAI 官方编码 Agent；开源实现 [openai/codex](https://github.com/openai/codex)（Apache-2.0，`codex-rs` workspace），SDK / MCP server 等集成面最广 | [openai/codex](https://github.com/openai/codex) |
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
| MCP | C / 协议/标准 | Model Context Protocol；调研（附录 A）中以 MCP 工具管理、MCP 网关、Codex as MCP server 形式出现 | — |
| ACP（agent-client-protocol） | C / 协议/标准 | 编辑器 ↔ Agent 协议（Zed 生态）：capability ↔ 方法组一一映射、schema 单源派生多语言 SDK；已迁 agentclientprotocol 组织 | [agent-client-protocol](https://github.com/zed-industries/agent-client-protocol) |
| models.dev | C / 模型目录注册表 | OpenCode 同团队维护的中心模型元数据目录 | [models.dev](https://models.dev) |
| gpui-component（13k） | C / GPUI 组件库（Rust，Apache-2.0） | 60+ 组件 + ~140 语义 token 主题 + VirtualList 变高虚拟化；v0.5.1 适配 crates.io gpui ^0.2.2 | [longbridge/gpui-component](https://github.com/longbridge/gpui-component) |
| Zed `ui`/`theme` crates | C / GPUI 官方组件层（GPL-3.0） | ButtonLike/ContextMenu 等 ~40 组件与 theme token 组织；**只参 API 形状，不抄代码**（gpui 本体 Apache-2.0 除外） | [zed-industries/zed](https://github.com/zed-industries/zed/tree/main/crates/ui) |
| sandbox-runtime（srt） | C / 沙箱运行时库（TS，Apache-2.0） | Claude Code 官方沙箱隔离层：Seatbelt/bubblewrap profile 生成 + egress 本地代理域名白名单 | [anthropic-experimental/sandbox-runtime](https://github.com/anthropic-experimental/sandbox-runtime) |

---

## 2. 主要对标项目

Pawork 的候选功能对照基于四家的公开功能面（功能对照见 [design.md](design.md) §4，转正登记见 [../ROADMAP.md](../ROADMAP.md) §5 候选池）。通用红线：纯 Rust 不引入 JS 运行时（排除 JS 插件生态路线）；无 TUI（CLI 交互模式 + S7 起的 GPUI Desktop，设计见 [gui-design.md](gui-design.md)）。

### 2.1 OpenCode

- **定位与目标**：TypeScript/Bun 的 TUI 编码 Agent，形态最全（Desktop beta、Web UI、IDE 扩展），自营 OpenCode Zen（按量）与 OpenCode Go（订阅）托管模型。
- **核心功能**：GitHub/GitLab CI bot；自定义命令、undo/redo、post-edit formatters；webfetch / websearch / question / todowrite 工具；References；JS 插件生态；会话分享；models.dev 模型目录（75+ provider）；内置 `task` 子代理（子 session + 权限派生 + 深度限制）。
- **与 Pawork 的关系**：参照——`task` 子代理与权限派生（F4 声明式绑定方向）、Anthropic 缓存断点摆放与前缀稳定性工程（F5）、429 重试策略（遵循 Retry-After、封顶 30s）；红线排除——TUI 形态、JS 插件生态；其同 provider 多账户空白正是 Pawork 的差异化机会（F1）。
- **关键链接**：[opencode.ai/docs](https://opencode.ai/docs/) · [anomalyco/opencode](https://github.com/anomalyco/opencode)（原 sst/opencode）· [providers](https://opencode.ai/docs/providers/) · [agents](https://opencode.ai/docs/agents/)。机制详见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §2.1、§5.2。

### 2.2 Pi

- **定位与目标**：TypeScript/Bun monorepo（pi-ai / pi-agent-core / pi-coding-agent）的 TUI 编码 Agent；provider 无关 Context 与跨厂商 handoff 为一等能力。
- **核心功能**：Prompt Templates；Pi Packages（能力包打包 + npm/git 分发）；Project Trust；Message Queue（steering / follow-up）；thinking-level 用户控制；session tree / clone；llama.cpp 本地模型；订阅登录（Claude / OpenAI / Copilot plan）；OSS session 分享。
- **与 Pawork 的关系**：参照——provider 无关 Context（canonical domain 思路同构）、订阅 OAuth（F1-B plan 凭证）、精细缓存断点与 1h/24h 长 TTL（F5-B 显式族实践）、「核心不内置子代理」哲学（F4 取舍对照）；红线排除——TUI、npm/git 能力包分发（JS 生态路线）；其 Anthropic OAuth 的 Claude Code 伪装实现属身份伪装，Pawork 明确不采纳。
- **关键链接**：[pi.dev](https://pi.dev) · [earendil-works/pi](https://github.com/earendil-works/pi) · [docs/providers.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/providers.md) · [docs/models.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md) · [pi-ai README](https://github.com/earendil-works/pi/blob/main/packages/ai/README.md)。机制详见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §2.2、§5.2。

### 2.3 Codex（openai/codex）

- **定位与目标**：OpenAI 官方编码 Agent。开源实现在 [openai/codex](https://github.com/openai/codex)（Apache-2.0，Rust workspace `codex-rs`）；产品线覆盖 CLI + Desktop app + Cloud + IDE 扩展，是四家 A 类对标里集成面最广、也最接近 Pawork「纯 Rust CLI 宿主」形态的一项。与社区项目 [opencodex](https://github.com/lidge-jun/opencodex)、[Codex Router](https://github.com/duolahypercho/codex-router) 均无隶属，勿混用。
- **核心功能**：图片输入 / web search / image generation / voice；Computer Use、Browser、Chrome 扩展；`/review` + GitHub PR 自动审查；GitHub Action；Slack / Linear 集成；Codex as MCP server；TS/Python SDK；本地 memories；scheduled tasks 产品；插件目录（连接器）；Bedrock 模型源。仓库侧对照点：`codex-rs` 扁平 workspace、[app-server-protocol](https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol) 宏 registry、[sandbox.md](https://github.com/openai/codex/blob/main/docs/sandbox.md) Seatbelt/Landlock 结构。
- **与 Pawork 的关系**：参照——approval/sandbox 体系（对照 Pawork 的 policy / sandbox）、SDK 与 MCP server 对外集成形态、`prompt_cache_key = conversation_id` 会话亲和（F5-B 隐式族亲和键实践；其子会话 fork 缓存命中率 62%→9.6% 是子代理缓存取舍的直接反例）。V3 另对照其 workspace 布局纪律（R1）、协议 registry 同源（R3）、sandbox/egress（R7）；**反面教材**是 134 成员微 crate 增殖——只抄纪律不抄粒度。
- **关键链接**：[openai/codex](https://github.com/openai/codex) · [codex-rs](https://github.com/openai/codex/tree/main/codex-rs) · [developers.openai.com/codex](https://developers.openai.com/codex)（产品文档） · [client.rs（亲和键实现）](https://github.com/openai/codex/blob/d807d44a/codex-rs/core/src/client.rs) · [issue #21796](https://github.com/openai/codex/issues/21796)。机制详见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §5.2、§5.5。

### 2.4 DeepSeek Harness

- **定位与目标**：DeepSeek AI 官方开源 agent harness（`dsh`，MIT，developer preview）。口号是 Agent = Model + Harness、**一切皆插件**：模型、工具、技能、会话、沙箱、存储、循环、调度与 UI 均由 [Cordis](https://github.com/cordiverse/cordis) 插件组合，配置层可替换。默认形态是本地 Web UI（`npx @deepseek-ai/dsh web`），另有 headless profile 与 Python SDK。本文所述均指 [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)，与同名非官方适配器项目无关。
- **核心功能**：四套 preset——Standard（文件编辑 / Shell / 文件与网页检索 / Skills / 计划 / goals / 子代理 / 工作流）、PTC/Code Mode（经 Code Mode SDK 用一段 TypeScript 组合多步工具）、Minimal（持久 `bash` + `str_replace_editor`，用于基准）、Creator（在 Standard 上加运行时检查与 preset 创作）。仅追加 `SessionEvent` 日志是模型可见上下文的 SSOT（fork / resume / Trajectory 回放同源）；沙箱模式与审批策略是两个独立 knob，经 `workspace-write` / `danger-full-access` 等 permission preset 捆绑。工具面含 `tool-ask-user`、`tool-todo`、`tool-web`、`tool-skill`、`tool-subagent`、`tool-terminal`、MCP；LLM 适配覆盖 DeepSeek 与 Anthropic / OpenAI / Bedrock / Azure / Vertex。
- **与 Pawork 的关系**：参照——仅追加会话事件作为模型可见输入的重建源（对照 Pawork `AgentEventEnvelope` + append-only，是目前最接近的外部同形）；沙箱与审批分 knob（对照 S3/S4）；`ctx.sessions.fork`、headless、Python SDK（对照 S10）；Skills / plan / 子代理 / 工作流（对照 S9/S11）。红线排除——Cordis/JS「一切皆插件」、以 Web UI 为默认壳、Code Mode 生成并执行 TypeScript（JS 运行时）。Developer preview，官方声明会有破坏性变更；实现前复核实态，不把其插件 API 当冻结契约。
- **关键链接**：[deepseek.com/harness](https://deepseek.com/harness) · [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) · [架构](https://deepseek-harness.github.io/deepseek-harness/en/reference/) · [权限预设](https://deepseek-harness.github.io/deepseek-harness/en/reference/subsystems/permission-presets) · [Python SDK](https://deepseek-harness.github.io/deepseek-harness/en/guide/python-sdk)。本仓暂无独立调研专章（2026-08-17 按公开功能面登记）。

---

## 3. 多账户与路由专题项目

本节项目对应 [附录 B](#附录-b-分功能方案-f1f6原-researchmulti-account-quota-proposalsmd) 的 F1–F6 方案（已确认）：F1 多账户模型与凭证、F2 额度感知与预算控制、F3 切换与路由策略、F4 子 Agent 跨供应商调用、F5 输入缓存策略控制、F6 对外账户池网关模式。

### 3.1 opencodex

- **定位与形态**：本地代理守护进程（Bun，默认端口 10100）+ Web dashboard + `ocx` CLI；把 Codex Responses API 翻译到 40+ provider，另向 Claude Code 提供 `/v1/messages` 网关。
- **核心机制**：① ChatGPT 账户池：5h / 周 / 30d 三窗口配额**主动探测**，`quota`（默认）/ round-robin / fill-first 三种池策略；② thread affinity：既有会话钉在原账户保 prompt cache，仅 failover / 亲和过期等触发 rebind；③ 429 → cooldown failover，401/403 → fail-closed（不静默换凭据）；④ Design B 注入：只改 `~/.codex/config.toml` 的 `openai_base_url` 一个字段。
- **与 Pawork 的关系**：F2-B 被动配额信号捕获与 F3-B「配额余量优先」策略、会话-账户亲和的直接参照；F6-A 下可作 openai-compatible 上游网关；config 布局是 G6 只读导入源候选；其本地凭证文件是导入参照，Pawork 额外要求 0600、原子写、损坏 fail-closed、掩码展示与日志脱敏。
- **链接**：[lidge-jun/opencodex](https://github.com/lidge-jun/opencodex) · [opencodex.me](https://opencodex.me) · [configuration](https://opencodex.me/reference/configuration/) · [How It Works](https://opencodex.me/getting-started/how-it-works/)。详见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §3.1。

### 3.2 Codex Router

- **定位与形态**：本地路由器（JS + 内嵌 LiteLLM，默认 `127.0.0.1:4202`）+ 本机托盘；社区项目，与 OpenAI / opencodex 均无隶属。一次安装、一套凭证，把外部模型并入 [openai/codex](https://github.com/openai/codex) 原生 picker，并同样发布到 DeepSeek Harness 与 Gemini CLI。宿主仍拥有 Agent 循环、工具、权限、MCP 与会话；路由器只做推理转发与协议翻译。
- **核心机制**：① Design B 注入：托管改写 `~/.codex/config.toml` 的 `openai_base_url` + `model_catalog_json`，把外部条目并入 Codex 原生目录；② 凭证隔离：丢弃入站 Codex 凭据，只向所选上游注入对应 OAuth/API key（Kimi Code / Grok CLI 会话复用，不读 Copilot 官方凭据库）；③ 注册表驱动：`config/` 校验过的 provider/model 才进 picker，凭证感知（无凭据不展示）；④ 额度耗尽 failover（默认开）：仅 402 / 余额耗尽 / 需等待 >1min 的 429 才换到已启用的下一模型，坏 key / 未知模型 / 宕机仍原样报错；提供商声明的复位窗口会冷却（上限 6h）；⑤ 可选旧工具结果老化与外部模型 compaction 摘要；⑥ 文本模型的 vision bridge（把粘贴图交给已启用视觉模型再代换成证据文本）。
- **与 Pawork 的关系**：与 opencodex 同属「截 Codex `base_url` 的本地路由器」，但重点是**多客户端共享的凭证隔离目录**，不是 ChatGPT 账户池。参照——S0/S6 openai-compatible 上游与六条首发通道的端点/凭证形态（GLM Coding Plan、OpenCode Go、Qwen Token Plan、DeepSeek、xAI OAuth）；S5 工具结果老化 / 外部 compaction 对照；S9 G6 导入源候选（托管 `config.toml` 块 + `~/.codex/codex-router` 状态目录）；S11 F2/F3 的窄错误分类 failover 与冷却（对照，不是 sticky 账户池）；S11 F4 的「仅注册表验证过的模型可作子代理」；F6-A 下可作 openai-compatible 上游。红线排除——JS/LiteLLM 运行时、login-free 把外部模型别名到原生 GPT slug、匿名免费网关、身份伪装。本仓暂无独立调研专章（2026-08-18 按公开 README / HOW-IT-WORKS 登记）。
- **链接**：[duolahypercho/codex-router](https://github.com/duolahypercho/codex-router) · [How it works](https://github.com/duolahypercho/codex-router/blob/main/docs/HOW-IT-WORKS.md) · [Install](https://github.com/duolahypercho/codex-router/blob/main/docs/INSTALL.md) · [Compatible apps](https://github.com/duolahypercho/codex-router/blob/main/docs/COMPATIBLE-APPS.md)。

### 3.3 cc-switch

- **定位与形态**：跨平台桌面 GUI（Tauri 2，另有 Web/CLI 形态），统一管理 8 个工具（Claude Code、Codex、Gemini CLI 等）的供应商配置，50+ provider 预设。
- **核心机制**：① SSOT：provider 集中存 `~/.cc-switch/cc-switch.db`（SQLite），切换时原子写回各工具 live 配置文件（临时文件 + rename + 失败回滚 + backfill 回读）；② 切换粒度为全局配置级、手动为主（Claude Code 支持热切换），另有本地代理模式（auto-failover、circuit breaker）；③ 额度侧仅本地记账 dashboard 与可配置余额查询脚本，无配额驱动自动换号。
- **与 Pawork 的关系**：G6（F1 附属）导入源候选（cc-switch SQLite 布局）；「配置级切换 + 无 sticky」是 F3-B 的反面对照（切换即缓存作废）；导入后的 secret 直接写入 Pawork auth 文件，不落仓库或中间文件。
- **链接**：[farion1231/cc-switch](https://github.com/farion1231/cc-switch) · [cc-switch.cc](https://cc-switch.cc/) · [README_ZH](https://github.com/farion1231/cc-switch/blob/HEAD/README_ZH.md)。详见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §3.2。

### 3.4 CLIProxyAPI

- **定位与形态**：Go 守护进程（默认端口 8317，支持 Docker / TLS / Go SDK 嵌入），把 Gemini CLI、Codex、Claude Code、Qwen Code 等 OAuth 订阅账户封装为 OpenAI / Gemini / Claude 兼容 API。
- **核心机制**：① 账户池：auth-dir 内一账户一 JSON token 文件，round-robin / 加权 / fill-first 轮询；② 额度耗尽被动检测：429 → 指数退避冷却（1s→30min）自动换凭据重试，另有降级链（switch-project / switch-preview-model）；③ session-affinity（v6.9.27+，默认关）：多来源 session ID + TTL SessionCache，明确以 prompt cache 命中率为目标；④ OAuth 后台自动刷新（过期前刷新、401 即时刷新重试）。
- **与 Pawork 的关系**：sticky session 与错误分类冷却是 F3-B 同构参照（V1 `ErrorClassifier` 语义更细）；auth-dir 是 G6 导入源候选；其 `codex.identity-confuse`（按所选账户重写 `prompt_cache_key` 与安装身份）属身份伪装，Pawork **明确不采纳**。
- **链接**：[router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) · [authentication](https://router-for-me-cliproxyapi.mintlify.app/concepts/authentication) · [routing](https://mintlify.wiki/router-for-me/CLIProxyAPI/concepts/routing) · [configuration options](https://help.router-for.me/configuration/options)。详见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §3.3。

### 3.5 claude-relay-service

- **定位与形态**：Claude 订阅账户池中继（Node），在自发 API Key（`cr_` 前缀）层做限速、并发与模型黑名单控制。
- **核心机制**：① **内容 hash sticky session**：对可缓存前缀做 SHA-256，Redis 存 hash→账户映射（带 TTL），同会话固定账户保 prompt cache（作者明示频繁切换毁缓存且可能增加封号风险）；② 429/529 标记排除、5xx 临时暂停，并发用 Redis Sorted Set 排队；③ 每账户独立代理 IP，OAuth token AES 加密存 Redis。
- **与 Pawork 的关系**：sticky 保缓存路线的代表实现（F3-B 参照；Pawork 绑定键用自有 session_id，无需内容 hash）；「非限流 429（Extra usage is required）应透传而非锁账户」的错误分类教训已被 V1 错误表覆盖。
- **链接**：[Wei-Shaw/claude-relay-service](https://github.com/Wei-Shaw/claude-relay-service)。详见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §4.4。

### 3.6 其余专题项目速查表

下表「详见」列均指 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) 对应章节。

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

> **收录标准**（沿用附录 A §8）：仅收录活跃维护、且在表内承担**不可替代角色**的项目。历次移除：① 2026-08-14 按 pushed_at 复核活跃度，移除 TensorZero、Roo Code、Helicone AI Gateway、Arch/archgw、Portkey、one-api、gemini-balance 共 7 项；② 2026-08-18 GitHub API 全量复核后按「同功能与实现思路可由表内更强项目替代 + star 停滞或活跃不足」二次清理，移除 gpt-load、uni-api、claude-code-hub、meridian、antigravity-claude-proxy 共 5 项。逐项理由、替代关系与「外部网关存续风险 → 自持进程内能力（F6-A）更稳」结论见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §8；被移除项的机制原文仍保留在附录 A §4.5/§4.6/§5.4（历史快照）。同日复核另记：claude-relay-service 增长停滞（作者重心转向 sub2api），仍为 G3 sticky 主参照，保留观察；meridian 仓库无 LICENSE（移除的附加原因：不可参考其代码）。

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

完整对照表（最小可缓存长度 / TTL / 计价 / 缓存键与隔离 / 用量字段）见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) §5.1；Coding Agent 客户端的断点摆放实践见附录 A §5.2；前缀稳定技巧见附录 A §5.3。

---

## 5. 调研附录索引

原 docs/research/ 三份调研已于 **2026-08-25** 并入本文文末附录（压缩保留结论，原文全文见 git 历史）：

| 附录 | 用途 |
| --- | --- |
| [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd) | 外部实现逻辑调研：项目机制详查（A §2–§4）、厂商缓存机制对照（A §5）、模式归纳（A §6）、与 V1 资产对照（A §7）、参照项目对照总表与收录标准（A §8） |
| [附录 B](#附录-b-分功能方案-f1f6原-researchmulti-account-quota-proposalsmd) | F1–F6 实施方案与推荐（**已确认**）：多账户凭证、额度感知、切换路由、子 Agent 绑定、输入缓存、网关模式；含分阶段落地图（B §7） |
| [附录 C](#附录-c-决策记录-d1d8-与并入约定原-researchmulti-account-quota-plan-mergemd) | 决策记录 D1–D8 与并入约定（决策唯一入口）：执行期凭证 fail-closed、少测试无门禁、缓存命中率 95/97/99 目标 |

F1–F6 与 [design.md](design.md) §3 已确认扩展功能族（G1–G7）的对应关系见附录 B §7。后续新增专题调研直接以附录形式并入本手册，并在 §1 总览、对应章节与 §6 反向分类登记。

---

## 6. 参照项目按功能规划分类

> 正向映射（功能 → 参照）以 [design.md](design.md) §2 / §3 为准；本节是**反向索引**。标「主」= 实现时优先对照；「对照 / 反例」= 取舍参考或明确不采纳。旧 V3 阶段轴见 §7；当前 UI 阶段轴见 [UI 参照调研](../plan/UI-reference-research.md)。

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
| Codex（[openai/codex](https://github.com/openai/codex)） | A | S0/S1 CLI 与 resume；S3/S4 approval + sandbox；S6 ChatGPT OAuth 与文件凭证形态；S7 Desktop 壳；S9 AGENTS.md / MCP；S10 SDK / app-server；G5 `prompt_cache_key` 亲和；R1 workspace 布局纪律、R3 app-server-protocol registry、R7 sandbox；候选 B1/B5–B7、C2/C3、D1/D2/D6/D7 |
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

## 7. 历史 V3 阶段参照指引（R0–R9）

> 本节是 **2026-08-18** 的旧 V3 调研快照，不再与当前 ROADMAP 阶段一一对应。当前 R1–R8 的外部行为与测试方法以 [UI 参照调研](../plan/UI-reference-research.md) 为准；以下内容只供历史选型考证。

### 7.1 阶段 → 参照映射

| 阶段 | 主参照 | 对照 / 反例 | 关键参照点 |
| --- | --- | --- | --- |
| **旧 R8** GUI 组件化与 Desktop 收口 | [gpui-component](https://github.com/longbridge/gpui-component) **v0.5.1 tag**（Apache-2.0；该版依赖 crates.io gpui ^0.2.2 与本仓 ADR-035 锁定一致，主干已改跟 Zed git 主干，勿参主干） | Zed [`crates/ui`](https://github.com/zed-industries/zed/tree/main/crates/ui)/`crates/theme`（**GPL-3.0：只参 API 形状不抄代码**）；Codex Desktop / OpenCode Desktop 壳形态（既有 S7 参照） | gpui-component：60+ 组件、`ThemeColor` 语义 token、`VirtualList` 与 Zed `ButtonLike`/`ContextMenu` 只作历史组件组织参照 |
| **旧 R9** 一致性收口 | —（内部核对） | — | 已由当前 R9/R10 任务书重新整理未完成部分 |

### 7.2 使用纪律

- **许可证红线**：GPL 系（Zed `ui`/`theme`）与无 LICENSE 仓库只参照 API 形状与机制思路，禁止复制代码；Apache-2.0 / MIT 系（codex-rs、gpui-component、srt）可借鉴实现但仍以自写为主，引入片段须记录出处。
- **参照不改契约**：对照外部设计时，本仓冻结契约（[architecture.md](architecture.md) §3.2）优先；外部形状与冻结契约冲突的，走 ADR 而不是「顺手对齐」。
- **快照时效**：本节结论为 2026-08-18 快照；后续任务开工时按 [../ROADMAP.md](../ROADMAP.md) §7.1 核查约定重验参照项目实态（版本、许可证、API 形状），漂移即回写本节。
- **登记约定**：2026-08-18 随本节新入册 ACP、gpui-component、Zed `ui`/`theme`、sandbox-runtime 四项（§1 总览与 §6.2 已同步）；R8 任务书引用的「Zed ui 与 gpui-component API 形状」自此在本手册有落点。**2026-08-21**：Codex 主入口改为官方仓 [openai/codex](https://github.com/openai/codex)（此前 §1 只挂产品文档站）；附录 A §8 同步补行。后续 V3 专项调研继续按 §5 约定登记。

---

## 附录 A 多账户/配额/缓存机制调研（原 research/multi-account-quota-reference.md）

> 调研日期 **2026-08-14**，2026-08-18 复核清理；**2026-08-25 并入本手册**（约六成压缩版：机制对照表、各项目结论与 A §8 参照移除记录完整保留，过程叙述压缩；原文全文见 git 历史 `docs/research/multi-account-quota-reference.md`）。目的：为「多账户间额度控制与切换、子 Agent 跨供应商调用、输入/提示缓存策略控制」（[design.md](design.md) §3 已确认扩展功能族 G1–G7）提供外部实现逻辑参考。论断附来源链接，无法核实处标「未证实」；内容为撰写时点快照，实现前应复核最新实态。配套：[附录 B](#附录-b-分功能方案-f1f6原-researchmulti-account-quota-proposalsmd)（F1–F6 方案）· [附录 C](#附录-c-决策记录-d1d8-与并入约定原-researchmulti-account-quota-plan-mergemd)（决策 D1–D8）。附录小节保留原文编号，引用记作「A §N」。

### A §1 项目总览

| 项目 | 仓库 | 形态 | 与本调研相关的核心机制 |
| --- | --- | --- | --- |
| OpenCode | [anomalyco/opencode](https://github.com/anomalyco/opencode)（原 sst/opencode） | TypeScript/Bun 终端 Coding Agent | models.dev 模型目录、`task` 工具子代理（子 session + 权限派生）、Anthropic 缓存断点、429 重试策略 |
| Pi | [earendil-works/pi](https://github.com/earendil-works/pi)（原 badlogic/pi-mono，2026-05 迁移） | TypeScript monorepo（pi-ai / pi-agent-core / pi-coding-agent） | provider 无关 Context + 跨厂商 handoff、订阅 OAuth 全线可用、精细缓存断点与长 TTL、扩展式子代理 |
| Codex | [openai/codex](https://github.com/openai/codex)（Apache-2.0，Rust `codex-rs`） | 官方 CLI + Desktop + Cloud 编码 Agent | 会话亲和 `prompt_cache_key = conversation_id`；approval / sandbox（Seatbelt/Landlock）；**不是**账户池项目，与 opencodex / Codex Router 无隶属 |
| opencodex | [lidge-jun/opencodex](https://github.com/lidge-jun/opencodex)（npm `@bitkyc08/opencodex`，命令 `ocx`，文档站 [opencodex.me](https://opencodex.me)） | 本地代理守护进程 + Web dashboard（Bun，默认端口 10100） | Codex Responses 协议翻译（40+ provider）、**ChatGPT 账户池**（5h/周/30d 三窗口配额路由 + 线程亲和） |
| cc-switch | [farion1231/cc-switch](https://github.com/farion1231/cc-switch)（官网 ccswitch.io） | Tauri 桌面应用（另有 Web/CLI 形态） | **配置级**供应商/账户切换（SSOT SQLite → 原子写回各工具 live 配置文件）、本地代理模式下的故障转移 |
| CLIProxyAPI | [router-for-me/CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)（原 luispater） | Go 守护进程（默认端口 8317） | 多 OAuth 订阅账户封装为兼容 API、round-robin/加权/fill-first、429 指数退避冷却、session-affinity |
| claude-code-router（CCR） | [musistudio/claude-code-router](https://github.com/musistudio/claude-code-router) | 本地网关 | 场景化路由（default/background/think/longContext）、子代理标签路由、transformer 体系（含 `cleancache`） |
| LiteLLM | [BerriAI/litellm](https://github.com/BerriAI/litellm) | Proxy/Router | 层级预算（org/team/user/key）、TPM/RPM 限流、6 种路由策略、cooldown/fallback、缓存差价计费 |
| new-api | [QuantumNous/new-api](https://github.com/QuantumNous/new-api)（承自 one-api，其上游已于 2026-01 停更） | 计费网关 | 渠道-账户-令牌三层 quota 折算、渠道优先级/权重、渠道内多 key 轮询、失败自动禁用与换渠道重试 |
| claude-relay-service（CRS） | [Wei-Shaw/claude-relay-service](https://github.com/Wei-Shaw/claude-relay-service) | Claude 订阅账户池中继 | **内容 hash sticky session 保 prompt cache**、429/529 标记排除、每账户独立代理 IP |
| gpt-load | [tbphp/gpt-load](https://github.com/tbphp/gpt-load) | Go key 池透明代理 | 累计失败拉黑 + 定时验证恢复、failover 状态码可配置 |
| 新兴项目（2025–2026） | claude-code-hub、CLIProxyAPI-Plus、meridian、antigravity-claude-proxy | 见 A §4.6 | 账户池 + sticky session 已成标配设计 |
| Codex Router | [duolahypercho/codex-router](https://github.com/duolahypercho/codex-router) | 本地路由器 + 托盘（JS / LiteLLM，默认 127.0.0.1:4202） | 一安装多客户端（Codex / DeepSeek Harness / Gemini CLI）；凭证隔离转发；额度耗尽换模型（非账户池 sticky） |

**同名项目辨析**（避免张冠李戴）：`opencodex` 另有两个不相关同名项目（[ymichael/open-codex](https://github.com/ymichael/open-codex) Codex CLI 多 Provider fork、[codingmoh/open-codex](https://github.com/codingmoh/open-codex) Python 本地模型 CLI），本文所述均指 lidge-jun/opencodex；`codex` / `openai/codex` 指官方仓（A 类对标，见本文 §2.3），与 lidge-jun/opencodex、duolahypercho/codex-router 均不是同一项目；`codex-router` 指 [duolahypercho/codex-router](https://github.com/duolahypercho/codex-router)，与 musistudio/claude-code-router 不是同一项目；`ccswitch` 另有基于 CLIProxyAPI 的包装 CLI（如 kaitranntt/ccs），本文所述均指 farion1231/cc-switch。

### A §2 多供应商 Coding Agent（OpenCode / Pi）

#### A §2.1 OpenCode

- **多 Provider / 多账户**：models.dev 中心目录 + Vercel AI SDK（75+ provider）；`opencode auth login` 写 `~/.local/share/opencode/auth.json`，按 providerID 单条凭证（`api` / `oauth` 自动刷新 / `wellknown`）。Anthropic 2026-01 起技术封锁第三方 Claude OAuth，OpenCode 2026-03 应法律要求移除内置 Anthropic OAuth（[PR #18186](https://github.com/anomalyco/opencode/pull/18186)）；仍零配置支持 ChatGPT Plus/Pro、GitHub Copilot、GitLab Duo，另有自营 Zen（按量）/ Go（订阅）。**同 Provider 多账户原生不支持**——重复登录即覆盖（[#5391](https://github.com/anomalyco/opencode/issues/5391)）；绕法：API key 型配置别名 provider，OAuth 型换 `XDG_DATA_HOME` 或社区插件。
- **用量与 429**：每条 assistant message 落库 `cost` + `tokens{input,output,reasoning,cache{read,write}}`（SQLite 列），按 models.dev 单价计成本、cache read/write 单独计价；429 最多 5 次指数退避重试（初始 2s、倍率 2、jitter 0.25），优先遵循 `retry-after` 头、无头封顶 30s；context overflow 走自动 compaction 不重试；**不自动切换 model/账户**。
- **子代理**：统一 `agent` 概念（`mode: primary|subagent|all`，JSON 或 Markdown frontmatter 定义，字段含 `model`、`prompt`、`permission`、`hidden` 等）；主 agent 调 `task` 工具创建子 session（`parentID` 指父、**权限从父 + 子 agent 派生**、`subagent_depth` 默认 1、`task_id` 可续跑）；子 agent 可配独立 `model`（任意 provider），未配则继承父；账户随 provider 全局凭证，无 per-agent 账户。
- **Prompt caching**：Anthropic 断点打在前 2 条 system + 最后 2 条非 system 消息（≤4 个 `cache_control: {type:"ephemeral"}`，滚动前移），未见 1h TTL 管理；OpenAI 系不打断点，自动缓存 + **`prompt_cache_key = sessionID`** 会话亲和。前缀稳定性工程：系统提示拆「静态 header + 动态 rest」两块，修复工具 schema 含 per-repo cwd、技能枚举排序非确定等「前缀污染」（PR #14203/#14743/#29949）。
- **切换连续性**：`/models` 会话中随时换 model/provider，transform 层按目标 provider 归一化整段历史（不支持的 part 替换占位、断点重打）；**无缓存失效补偿**——换 provider 即冷启动。

#### A §2.2 Pi

- **多 Provider / 多账户**：自维护内置模型目录（`*.models.ts` 生成式元数据，缓存 `~/.pi/agent/models-store.json`；用户扩展 `models.json`，四种 API 形态，可覆写 baseUrl/价格/context）；`/login` 写 `~/.pi/agent/auth.json`（0600），`key` 支持 `!command`（keychain/1Password）/ `$ENV` / 字面量，解析优先级 CLI > auth.json > env > models.json。OAuth 订阅全线可用：ChatGPT Plus/Pro（OpenAI 官方背书）、Claude Pro/Max（2026 起走 extra-usage 计费；实现为 **Claude Code 伪装模式**——注入 "You are Claude Code" system 块、模拟 claude-cli UA/beta headers，Pawork 明确不采纳）、Copilot、xAI、OpenRouter。同 provider 多账户不原生支持：拆独立 providerID（如 `zai` vs `zai-coding-cn`）或换 `PI_CODING_AGENT_DIR`。
- **用量与 429**：流事件采集 `input/output/cacheRead/cacheWrite(/cacheWrite1h)` 按四元单价（+tiers）计成本，TUI footer 实时显示命中率与累计成本；SDK 层重试默认禁用（曾因睡满多天 Retry-After 出 bug），agent 层 429/5xx 指数退避 3 次可中断；配额/余额耗尽判终态不重试；**不自动降级/换 provider**。
- **子代理**：**核心不内置**（README 明言 "No sub-agents"，官方立场是 tmux 多实例或 extension 自建）；官方示例经 `pi.registerTool()` + SDK `createAgentSession()` 起隔离子会话，可绑任意 provider/model（账户仍取全局 auth.json），无内置权限/并发框架；另有 `handoff.ts` 转移上下文到新聚焦 session。
- **Prompt caching**：Anthropic 显式断点 system 块 + 末 tool 定义 + 末 user message 末 block（3~4 个滚动）；**TTL 显式管理**：`PI_CACHE_RETENTION=long` 发 `cache_control.ttl:"1h"`（OpenAI 则 `prompt_cache_retention:"24h"`）；OpenAI/其它自动缓存 + `prompt_cache_key` 与 `session_id`/`x-session-affinity` 等亲和头按 provider 自适配；兼容代理可配 `compat.cacheControlFormat:"anthropic"`；Bedrock 自动 cachePoint。cacheRead/cacheWrite（含 1h 写入）单独计价入账。
- **切换连续性**：跨厂商 handoff 一等能力——Context 为 provider 无关格式（JSONL 树形 session，`id/parentId` 原地分支），发往新 provider 自动转换（同 provider assistant 消息保留原生结构，**异 provider thinking 块降级为 `<thinking>` 标签文本**）；配套 compat 矩阵。缓存失效无特殊补偿。

#### A §2.3 两项目对比要点

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

### A §3 账户额度管理与切换工具（opencodex / cc-switch / CLIProxyAPI）

#### A §3.1 opencodex（lidge-jun/opencodex）

- **定位**：本地代理守护进程（Bun，端口 10100）+ Web dashboard + `ocx` CLI；七个协议适配器组成 parser → router → adapter → bridge 管线，把 Codex Responses API 翻译到 40+ provider，另向 Claude Code 提供 `/v1/messages` 网关。
- **注入方式（Design B）**：对 Codex 只改 `~/.codex/config.toml` 的 `openai_base_url` 一个字段、不替换 provider 标签，卸载无需迁移。
- **账户模型与切换**：一个 provider = config.json 一个条目（adapter + baseUrl + apiKey/OAuth）；OpenAI 侧另有 **ChatGPT 账户池**与 API-key 池（`ocx account list/use/add-key` 管理）。三层切换——请求级模型路由（`provider/model` 前缀、combo 虚拟模型 `--strategy failover` 含 sticky）；**会话级账户切换**（新会话自动选账户、已有 thread 固定）；无全局配置改写。
- **额度感知**：**主动探测** ChatGPT 账户 5h/weekly/30d 三配额窗口（对应 OpenAI `primary/secondary/tertiary_window`，dashboard 一键刷新）+ **成功响应捕获配额头** + 被动 429。
- **池策略**：`accountPoolStrategy` 三种——`quota`（默认：比较最热配额窗口，活跃账户越过 `autoSwitchThreshold` 时为新会话挑低用量健康账户）/ `round-robin` / `fill-first`；可设账户 selection order 作后备顺序。
- **失败处理**：429 → 账户 cooldown 并 failover；401/403 → 标记需重新认证、**fail-closed**（不静默换凭据）。
- **缓存友好性**：**thread affinity 把既有会话钉在原账户**（长 SSH/tmux/移动会话不中途跳号），仅 quota 再评估/failover/账户排除/亲和过期/401/403/429 恢复可触发 rebind；请求日志显示 cached/cache-write token。
- **密钥与合规**：`~/.opencodex/config.json`（key 明文或 `${ENV}` 引用）+ `auth.json`（OAuth 自动刷新），本地明文文件无 keychain（未证实）；README 明示第三方代理可能违反 ToS、"Use at your own risk"。

#### A §3.2 cc-switch（farion1231/cc-switch）

- **定位**：跨平台桌面 GUI（Tauri 2，MIT；另有 Web/CLI 形态），管理 8 个工具（Claude Code、Claude Desktop、Codex、Gemini CLI、Grok Build、OpenCode、OpenClaw、Hermes）；50+ provider 预设、MCP/Prompts/Skills 统一管理、托盘快切。
- **SSOT 与切换动作**：provider 集中存 `~/.cc-switch/cc-switch.db`（SQLite），切换时写回各应用 live 配置文件（Claude Code `~/.claude/settings.json`、Codex `~/.codex/auth.json` + `config.toml` 实时 TOML 校验；MCP 投影到 `~/.claude.json` 与 `~/.codex/config.toml`）；**原子写（临时文件 + rename）+ 失败回滚 + 写后 backfill 回读**。
- **切换粒度**：默认**全局配置级**、手动为主（每应用同时只有一个 active provider，切换后需重启终端；Claude Code 例外热切换）；另有本地代理模式（格式转换、auto-failover、circuit breaker、健康监测）。
- **额度感知**：纯手动切换，无配额驱动自动换号；辅以本地记账 usage dashboard 与可配置余额查询脚本（JS Usage Script）。
- **多账户组织 / 缓存 / 密钥**：全部 provider 存 SQLite（自动备份 10 份、WebDAV/网盘云同步；Codex 可多官方账户切换）；未见 prompt cache 缓解机制（全局切换即缓存失效，推断未证实）；SQLite 与 live 配置明文、云同步会同步含 key 数据（未见加密/keychain 说明，未证实）。

#### A §3.3 CLIProxyAPI（router-for-me/CLIProxyAPI）

- **定位**：Go 守护进程（端口 8317，支持 Docker/TLS/Go SDK 嵌入），把 Gemini CLI、Antigravity、Codex（ChatGPT plan）、Claude Code、Grok Build、Qwen Code、Kimi、iFlow 等 OAuth 订阅账户封装为 OpenAI/Gemini/Claude 兼容 API。
- **账户模型**：auth-dir（默认 `~/.cli-proxy-api`）一账户一 JSON token 文件（含 access/refresh token/expiry），经 `--login` OAuth 生成；客户端用本地 `api-keys` 鉴权，真实凭据由代理持有。
- **轮询**：`routing.strategy` = `round-robin`（默认，同优先级内轮转）/ `weighted-round-robin`（凭据 `weight`）/ `fill-first`（按 priority 烧完换下一个）；同一 alias 可映射多上游模型构成池，池内轮询、失败续跑。
- **额度耗尽**：**被动检测**——429/quota-exceeded → 凭据 cooldown（指数退避 1s→30min）自动换凭据重试；瞬时错误（408/5xx）默认 60s 冷却；`request-retry`/`max-retry-credentials` 可调、冷却态可持久化、Management API `POST /reset-quota` 手动复位；降级链 `switch-project`（Gemini 换 GCP project）/ `switch-preview-model` / `antigravity-credits`。
- **OAuth 刷新**：后台每 5s 扫描、过期前自动刷新（默认 16 worker），401 即时刷新重试；token 本地明文 JSON 无 keychain。
- **模型映射与改写**：每凭据 `models[].name→alias`（`force-mapping` 回写响应模型名）、全局 `oauth-model-alias`、凭据 `prefix` 定向、`excluded-models` 过滤、`payload` 规则按模型/协议改写请求 JSON。
- **会话粘滞**：`routing.session-affinity: true`（v6.9.27+，默认关）——SessionAffinitySelector + TTL 内存 SessionCache（默认 1h），**明确以提高上游 prompt cache 命中率为目标**；session ID 依次取 Claude Code `metadata.user_id`、`X-Session-ID`、Codex `Session_id`、`X-Amp-Thread-Id`、`conversation_id`、`prompt_cache_key`、Responses 会话 ID，兜底首消息 FNV hash；绑定优先于凭据 priority，绑定不可用自动 failover 并重绑。另有 `codex.identity-confuse`（按所选账户重写 `prompt_cache_key` 与安装身份）——属身份伪装，Pawork **明确不采纳**。

#### A §3.4 三工具横向小结

- **切换层次**：cc-switch 改「客户端配置」（全局级、手动为主）；opencodex 与 CLIProxyAPI 是「代理层持有凭据」（请求/会话级、自动）。
- **额度感知**：opencodex 独有主动配额窗口探测 + 低用量优选；CLIProxyAPI 为被动 429 + 指数退避冷却 + 降级链；cc-switch 仅本地记账与手动查询。
- **缓存保护**：opencodex（thread 固定账户）与 CLIProxyAPI（session-affinity）都有显式 sticky 机制；cc-switch 无。
- **密钥存储**：三者均为本地明文文件/库（无 OS Keychain）——与 Pawork 的 Secret 红线形成直接差异点。

### A §4 网关与路由类项目

#### A §4.1 musistudio/claude-code-router（CCR）

- 本地 gateway/control plane，不做上游计费；CCR client keys（过期时间 + 本地 request/token/image 限额），Observability 按请求记录 tokens 与估算成本。
- **路由三层管线**：① 内置 agent 逻辑（识别 Claude Code/Codex，注入工具、剥离计费头）；② 可选 custom router JS（返回 `"provider,model"` 或 `null` 回落）；③ 配置化 RouterRule（按 header/body 条件首个命中）。场景键 `default` / `background`（小模型省钱）/ `think`（Plan Mode）/ `longContext`（默认阈值 60000）/ `webSearch` / `image`；`/model provider,model` 会话内动态切换。
- **子代理路由**：主 agent 在 subagent prompt 开头嵌 **`<CCR-SUBAGENT-MODEL>provider,model</CCR-SUBAGENT-MODEL>`** 标签，请求到达后提取剥除再定向路由——「改不了客户端」时的 in-band 补丁（Pawork F4-C 不采纳反例）。
- **失败与缓存**：路由层 retries + ordered fallbacks；transformer 按 provider/模型挂载可链式——内置 `Anthropic`（直连保留 cache_control）、`openrouter`/`deepseek`/`gemini` 格式改写、`maxtoken`、`reasoning`，以及 **`cleancache`（从请求中清除 `cache_control`，适配不认识缓存标记否则 400 的上游）**。

#### A §4.2 BerriAI/litellm

- **层级预算**：Organization → Team → User → Virtual Key → End-User 五级实体可共享 `LiteLLM_BudgetTable`（max/soft budget、TPM/RPM、`budget_duration` 周期重置）；每请求 `completion_cost()` 折 USD 归集各层，任一层超预算即拒绝；明细写 `LiteLLM_SpendLogs`，高并发有 budget reservation。
- **配额执行**：key/user/team 各设 `tpm_limit`/`rpm_limit`，`token_rate_limit_type` 可按 input/output/total 计。
- **路由策略**：`simple-shuffle`（默认，rpm/tpm/weight 加权随机）、`least-busy`、`usage-based-routing-v2`（Redis 原子跨实例统计，生产推荐）、`latency-based`、`cost-based`。
- **失败处理**：`allowed_fails` 后进 `cooldown_time` 冷却池、429 立即 cooldown；`num_retries` + 按异常类型 `retry_policy`；模型组级 `fallbacks` 与 `context_window_fallbacks`；deployment 可标 `order` 分级。
- **缓存**：透传 `cache_control`；usage 同时上报 OpenAI `cached_tokens` 与 Anthropic `cache_creation/read_input_tokens`；价格表含缓存读写单价按差价计费（历史有 cache_creation 双计 bug #9812，修复状态未证实）；**缓存感知路由**——`PromptCachingDeploymentCheck` 记住发生 cache write 的 deployment、同前缀路由回同一部署，另有 `session_affinity`/`deployment_affinity`（session_id → deployment，默认 TTL 1h）。

#### A §4.3 new-api（QuantumNous，承自 one-api）

- 机制原型来自 songquanpeng/one-api（上游 2026-01-09 停更，已按活跃度移除，见 A §8）；以活跃维护、渠道体系兼容的 new-api（AGPL-3.0）为参考载体。
- **三层模型**：渠道（上游 key/baseURL/模型列表）→ 用户账户额度 → 令牌额度双重扣减；quota 基准 1 unit ≈ $0.002，请求前预扣 + 按实际 usage 结算；计费 = 分组倍率 × 模型倍率 × (prompt tokens + completion tokens × 补全倍率)。
- **渠道调度**：优先级（大者优先）+ 同优先级按权重随机；连续失败达阈值自动禁用；失败自动重试换渠道；管理员令牌可 `Bearer KEY-CHANNEL_ID` 指定渠道。
- **多 key 与限流**：渠道内多 key RR/加权随机、单 key 失败跳过恢复重启用；渠道级 token bucket 限流（作用域整渠道或单 key）；模型固定价格、上游倍率同步、模型映射/参数覆盖。Claude 缓存差价计费未确认（未证实）。

#### A §4.4 Wei-Shaw/claude-relay-service（CRS）

- **额度模型**：控制在自发 API Key（`cr_` 前缀）层——时间窗口请求/token 限速、并发限制、模型黑名单、客户端限制；usage 从流式响应实时捕获计成本；订阅 5h 窗口在 UI 展示（透明中继不改变 Anthropic 侧窗口，内部估算方式未证实）。
- **Sticky session（核心机制）**：**对请求可缓存前缀（cache_control 标记内容，回退 system/首消息）做 SHA-256 作为会话键，Redis 存 hash→账户映射（带 TTL），同一会话固定命中同一账户以保住 prompt cache**；作者明确「频繁切换会导致 token 缓存使用量增大，以及可能增加封号风险」。
- **失败处理**：429 → `markAccountRateLimited()` 从池排除；529 过载 → 配置时长排除；503/5xx → 临时暂停；粘滞绑定账户不可用自动切换（死抱坏账户 bug #1007 已修复）；`429 "Extra usage is required"` 属非限流 429，需透传响应体而非锁账户（issue #1000 教训）；并发用 Redis Sorted Set 排队。
- **隔离与安全**：每账户可配独立静态 HTTP/SOCKS5 代理 IP（防共用 IP 被封）；OAuth token AES 加密存 Redis。

#### A §4.5 tbphp/gpt-load

> 2026-08-18 已按功能重叠标准移出参照表（见 A §8 移除记录）；本节保留为机制快照。

- Go 透明代理，完整保留 OpenAI/Gemini/Claude 原生格式——`cache_control` 等字段原样透传（推断，未证实）；分组 key 池负载均衡，无 per-user quota 计费。
- **key 生命周期**：分组 key 池 + 自动轮换；`blacklist_threshold`（默认 3）累计失败拉黑；后台按 `key_validation_interval_minutes`（默认 60）定时验证黑名单 key，通过即恢复。
- **失败处理**：`max_retries`（默认 3）单请求换 key 重试；`failover_status_codes` 可配置触发 failover 的状态码列表（支持区间语法）。

#### A §4.6 新兴同类项目（2025–2026）

- **[ding113/claude-code-hub](https://github.com/ding113/claude-code-hub)**（2026-08-18 移出参照表，见 A §8）：Claude Code & Codex 代理（Next.js+Hono+PostgreSQL+Redis）；权重+优先级+分组调度、熔断器、最多 3 次故障转移；RPM/金额（5h/周/月）/并发 session 多维限流用 Redis Lua 保原子、Redis 挂了 Fail-Open 降级；session 绑定 provider 用 Redis `SET NX` 原子首成锁，复用前查健康度、支持向高优先级 provider 迁移。
- **[ztx888/CLIProxyAPI-Plus](https://github.com/ztx888/CLIProxyAPI-Plus)**：CLIProxyAPI 社区强化版，Codex 配额运营：展示 5h/周额度与恢复窗口、`usage_limit_reached` 后自动持久停用、周额度低于阈值（如 3%）提前停用、按剩余额度排序。
- **[rynfar/meridian](https://github.com/rynfar/meridian)**（约 1.8k stars；2026-08-18 移出参照表，见 A §8）：经 Claude Agent SDK 把 Claude 订阅桥接为标准 Anthropic/OpenAI 协议（不做 OAuth 拦截）；多 profile 账户即时切换 + 可选 sticky session routing——多账户分散的同时保持每账户 prompt cache 温热。
- **[badrisnarayanan/antigravity-claude-proxy](https://www.mintlify.com/badrisnarayanan/antigravity-claude-proxy/guides/load-balancing)**（2026-08-18 移出参照表，见 A §8）：Google 账户池代理，策略三选一：Hybrid（健康度/余量/恢复期综合）、**Sticky（缓存最优：session ID = 首条 user message 的 SHA256，限流 <2min 时等待不切换）**、Round-Robin（吞吐最优、缓存最差）——把「缓存命中」作为调度策略的一等权衡维度。
- **[duolahypercho/codex-router](https://github.com/duolahypercho/codex-router)**（约 2.4k stars，2026-08-18 登记）：本地路由器 + 托盘，把外部模型并入 Codex / DeepSeek Harness / Gemini CLI 原生目录。Design B 只改 `openai_base_url` + `model_catalog_json`；入站 Codex 凭据丢弃，只向所选上游注入对应 OAuth/API key。Failover 默认开但窗口极窄（402 / 余额耗尽 / 需等待 >1min 的 429 才换**已启用的下一模型**，坏 key 与宕机仍 fail-closed）。**不是** ChatGPT 账户池、无会话-账户 sticky；作 F2/F3 窄错误分类与 F6-A 上游对照，不作 G1/G3 主参照。机制详见本文 §3.2。

### A §5 输入/提示缓存（prompt caching）机制

#### A §5.1 厂商机制对照表（2026-08 各官方文档现行版本）

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

#### A §5.2 Coding Agent 客户端的断点摆放实践

- **Claude Code**（[第三方逆向分析](https://notes.tsukino.dev/99-%E5%B7%A5%E5%85%B7%E4%B8%8E%E5%8F%82%E8%80%83/repos/how-claude-code-works/en/docs/03-context-engineering)）：3 类断点——① system prompt 在静态/动态边界处（`splitSysPromptPrefix()`）；② 工具数组最后一个常规工具打断点，可选工具放断点**之后**（开关不毁前缀），MCP 工具延迟加载；③ 消息数组最后一条打滑动断点。fire-and-forget 辅助请求把断点打在倒数第二条消息，避免污染主对话缓存链。
- **opencode**：前 2 system + 末 2 消息（见 A §2.1）；系统提示静态/动态拆块。
- **pi**：system + 末 tool + 末 user（见 A §2.2）；社区双标记策略（末 assistant `tool_use` 块 + 末 user 块）适配 MiniMax/Kimi 式缓存窗口，命中率 80%+（[PR #1737](https://github.com/badlogic/pi-mono/pull/1737)）。
- **Codex CLI**（[client.rs](https://github.com/openai/codex/blob/d807d44a/codex-rs/core/src/client.rs)）：Responses API，`prompt_cache_key = conversation_id`（会话内稳定跨轮不变）；跨会话共享相同启动前缀目前做不到（[issue #21796](https://github.com/openai/codex/issues/21796)）。

#### A §5.3 保持前缀稳定的技巧

- **静态在前、动态在后**：system prompt 固定；时间戳、环境信息、TODO 状态等易变内容放断点之后或对话末尾。
- **工具列表确定性**：排序稳定（opencode 曾修复 `Object.values()` 非确定序）、schema 不嵌 per-repo/per-run 值；JSON 键序也算字节，一字节差即从该处起全失效。
- **历史 append-only**：不改写/删除/重排早期消息；注意 reasoning 内容透传要求（GLM `clear_thinking=false` 时须原样透传，否则等效改写历史）。
- **Compaction/摘要 = 重写前缀 = 缓存全失效**，且按写入价重建。折中：低频触发、在任务自然边界压缩、压缩后立即发一次请求预热新前缀；长会话考虑 1h TTL 摊薄重写成本。
- **厂商特有**：OpenAI 固定 `prompt_cache_key` 且每 key ≤15 rpm，超量要分片；Anthropic 断点回看仅 20 块，单轮新增 >20 块（多工具调用）需每 ~15 块补中间断点。

#### A §5.4 多账户/多上游切换对缓存的破坏与网关缓解

**为什么换账户 = 缓存全失效**：所有厂商的缓存按组织/账户命名空间隔离——Anthropic 组织间隔离 + workspace 级隔离；OpenAI 声明「prompt caches are not shared between organizations」（[公告](https://openai.com/index/api-prompt-caching/)）；Bedrock 按 AWS 账户。网关把下一请求路由到另一账户，等于在空命名空间从零重建，重付全部写入费且延迟上升。

**网关侧缓解**（详见 A §3/§4 各项目）：CRS 内容 hash 粘滞；CLIProxyAPI session-affinity；opencodex thread affinity；LiteLLM `PromptCachingDeploymentCheck` + `session_affinity`；OpenRouter provider sticky routing。**通用原则：cache-aware routing——只在新会话做账户轮换/负载均衡，会话中途绝不换；持续统计 `cache_read` 占比作为路由健康指标。**

#### A §5.5 子代理（sub-agent）场景的缓存取舍

- **复用父前缀 vs 独立上下文**：同模型同账户下，子代理以父上下文为前缀追加任务可直接读父缓存（注意 Anthropic 并发限制：缓存条目要等首个响应开始后才可用，并行 fan-out 前先等一发种子请求）。独立上下文缓存从零建：重付写入费，换更小上下文与更干净注意力——多数编排框架（如 Claude Code subagents 独立 context window）选择后者。
- **实测数据**：Codex fork 出的子会话缓存命中率从 62% 掉到 9.6%，因 cache key 绑死 thread id（[issue #21796](https://github.com/openai/codex/issues/21796)）——理想方案是子代理家族共享一个稳定 `prompt_cache_key`/缓存命名空间。
- **便宜模型/不同账户的取舍**：缓存不跨模型也不跨账户，子代理换绑即放弃父缓存复用。判断标准：子代理任务上下文短、调用少 → 写入费损失小于模型差价，值得换；与父共享大前缀且高频往返 → 保持同模型同账户更省。
- **pi 的立场**：拒绝内置 sub-agent，主张 context gathering 在独立 session 完成并产出 artifact，兼顾可观测性与缓存友好（[Mario Zechner 博文](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/)）。

### A §6 模式归纳（供方案决策引用）

1. **切换层次四档**：① 配置级（cc-switch：改客户端配置文件，全局、手动）；② 客户端内路由（OpenCode/Pi 的 `/model` 切换、CCR 场景规则：请求级、规则驱动）；③ 代理层会话级（opencodex thread affinity、CRS/CLIProxyAPI sticky session：新会话再平衡 + 会话内锁定）；④ 代理层请求级（round-robin/加权：吞吐最优、缓存最差）。业界共识是 ③ 为默认、④ 仅在无缓存诉求时用。
2. **额度感知三形态**：本地记账（litellm 预算、new-api quota、cc-switch dashboard）→ 被动信号（429/Retry-After/`usage_limit_reached`/成功响应配额头）→ 主动探测（opencodex 三窗口刷新、CLIProxyAPI-Plus 阈值停用；成本与 ToS 面最大）。成熟实现是三者叠加、可信度分级。
3. **缓存保护两路径**：改写层（CCR `cleancache` 剥除、OpenRouter 翻译）解决「上游不认识 cache 标记」；调度层（sticky session/亲和）解决「换账户毁缓存」。**订阅池代理已把 sticky session 做成标配**（CRS、CLIProxyAPI、opencodex、claude-code-hub、meridian、antigravity 全部实现）。
4. **子代理路由三模式**：声明式绑定（opencode agent.model + 权限派生——客户端可控时的正解）；in-band 标签（CCR `<CCR-SUBAGENT-MODEL>`——改不了客户端时的补丁）；模型即子代理槽位（opencodex 把每个上游模型暴露为一个可指派的子代理）。
5. **共性短板**：Agent 内核层普遍不做同 provider 多账户（OpenCode/Pi 均空白，靠外部代理/配置切换工具补位）——「内核单凭证 + 外部池化」是当前生态分层，但也意味着内核原生多账户是差异化机会。
6. **失败分类共识**：429（限流，可冷却恢复）≠ quota exceeded（窗口耗尽，需等 reset）≠ 401/403（凭证问题，refresh 或人工介入）≠ 5xx（临时故障）；错误分类错了就会误惩罚账户（CRS issue #1000 的 "Extra usage is required" 教训）。
7. **合规风险共识**：第三方代理接订阅账户有 ToS/封号风险（opencodex 免责声明、CRS 作者提示、Anthropic 对 OpenCode 的封锁先例）；缓解手段（每账户独立代理 IP、身份伪装如 `identity-confuse`）本身加重合规问题。

### A §7 与 Pawork 现有资产的对照

V1 已有大量同构资产（详见 V1 归档 [provider-control-plane](../../Pawork_v1/docs/features/provider-control-plane.md)、[usage-quota](../../Pawork_v1/docs/features/usage-quota.md)、[context](../../Pawork_v1/docs/features/context.md)），撰写时点规划为 V2 S11 激活（S 阶段为 V2 语境，历史）：

| 外部模式 | Pawork V1 对应资产 | 状态（撰写时点） |
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

主要缺口（即候选功能，登记于 [design.md](design.md) §3，方案见 [附录 B](#附录-b-分功能方案-f1f6原-researchmulti-account-quota-proposalsmd)）：同 Provider 多账户的用户面（UX/凭证组织）、订阅 plan 凭证类型、被动配额信号捕获、缓存感知亲和默认化、子 Agent 声明式绑定契约、canonical 输入缓存策略控制。

### A §8 参考项目对照总表（2026-08-14 快照 · 2026-08-18 复核清理）

> 覆盖 A §2–§4 详查项目 + 2026-08-14 补充调研发现的流行项目。star 为当日数量级快照（GitHub API 抽样复核：cc-switch 127,132、opencode 197,254、OmniRoute 47,446 与调研一致）；「缓存策略与公开效果」列仅录**公开**数据——绝大多数项目不公布命中率，仅有的公开口径（Pi 社区双标记 80%+、Codex 会话级 62%）与 Pawork 的 95/97/99 目标口径（排除冷启动的会话级聚合，见 [附录 C](#附录-c-决策记录-d1d8-与并入约定原-researchmulti-account-quota-plan-mergemd) §1.3）不同，不可直接对比。
>
> **收录标准与移除记录（2026-08-14 按 pushed_at 复核）**：仅收录活跃维护项目——已归档或约 3 个月以上无提交者不作参考。据此移除 7 项：TensorZero（2026-06 归档停运）、Roo Code（2026-05 停运归档）、Helicone AI Gateway（被 Mintlify 收购转维护模式，2025-11 后无提交）、Arch/archgw→Plano（被 DigitalOcean 收购，2026-04 后无提交）、Portkey（被 Palo Alto Networks 收购，2026-05-25 后无提交）、one-api（2026-01-09 后无提交，机制由 new-api 继承）、gemini-balance（2025-09-30 后无提交）。2026 年商业开源网关整合潮（多起收购/停运）本身是重要事实：**依赖外部网关的方案有存续风险，自持进程内能力（F6-A 路线）因此更稳**。
>
> **2026-08-18 二次清理（功能重叠去重，GitHub API 全量复核）**：按「同功能与实现思路可由表内更强项目替代 + star 停滞或活跃不足」移除 5 项，对应行已从下表删除，机制原文保留于 A §4.5/§4.6/§5.4（历史快照）——① **gpt-load**（key 池拉黑 + 定时验证恢复：由 CLIProxyAPI 冷却/自动恢复链与 new-api 失败自动禁用/恢复覆盖，V1 `ErrorClassifier` 语义更细；6.3k 完全停滞）；② **uni-api**（channel 加权 + key 轮询：由 new-api / CLIProxyAPI 覆盖；1.3k 零增长、个人项目流量稀疏）；③ **claude-code-hub**（`SET NX` 首成锁与 Redis Lua 多维限流：sticky 由 CRS / CLIProxyAPI 覆盖，Redis 集中式形态与 Pawork 单机产品不匹配；3.3k 停滞、提交放缓）；④ **meridian**（「不拦 OAuth 合规 sticky」立场已内化为 F1-B/F3-B 已确认决议；1.9k，且仓库无 LICENSE，代码不可参考）；⑤ **antigravity-claude-proxy**（2026-06-08 后停更 71 天，越过上轮「暂保留观察」；「缓存命中为调度一等权衡」由 OmniRoute cacheAffinity 与 CRS sticky 承载；已确认其仓库为 badrisnarayanan/antigravity-claude-proxy，约 3.9k）。同日复核另记：claude-relay-service 增长停滞于 12.5k（作者重心转向 sub2api，topics 自标 "crs2"），仍为 G3 sticky 主参照，保留观察；OmniRoute（50k，+3k/4 天）与 9router 同属「免费聚合 + token 压缩」画像，持续关注安全面；9router 19 份安全通告确认为 6 critical / 11 high / 2 medium（最新 2026-07-16）；LiteLLM 已重写为 Rust core + Python SDK；许可证注记——new-api AGPL-3.0、sub2api LGPL-3.0（open issues 2.7k 积压）、LiteLLM 混合授权（MIT 主体 + enterprise 目录）。子代理另建议移除 Codex Router（2.5k、功能面窄），**不采纳**：该项目 2026-07-19 新建非「较老」，且承担与 opencodex 不同的「凭证隔离多客户端 + 注册表驱动目录」角色（S6 通道端点形态 / S9 G6 导入源 / S11 F2-F3 窄 failover / R5 通道注册表数据化），表内无替代。

| 项目（star≈） | 类别 / 形态 | 账号 / 凭证切换 | 缓存策略与公开效果 | 反代 / 协议处理 | 差异与借鉴（含 2026 状态） |
| --- | --- | --- | --- | --- | --- |
| [OpenCode](https://github.com/anomalyco/opencode)（197k） | 编码 Agent（TS/Bun 终端） | provider 单凭证；多账户靠插件或改 XDG 目录 | 前 2 system + 末 2 消息断点；OpenAI `prompt_cache_key`=sessionID；命中未公开 | 无反代，客户端直连；transform 层按目标 SDK 归一化 | 内置 task 子代理 + 权限派生；429 重试完整；多账户空白 |
| [Pi](https://github.com/earendil-works/pi)（90k） | 编码 Agent（TS monorepo） | provider 单凭证；拆 providerID 绕行 | system + 末 tool + 末 user 断点 + 1h/24h 长 TTL + 亲和头；社区双标记 80%+ | 无反代；compat 矩阵跨厂商 handoff | provider 无关 Context；订阅 OAuth 全线可用；核心零子代理 |
| [Codex](https://github.com/openai/codex)（111k，2026-08-21） | 官方编码 Agent（Rust CLI + Desktop + Cloud） | ChatGPT 订阅 / API key 单账户形态；**不是**账户池 | `prompt_cache_key = conversation_id`；fork 子会话命中 62%→9.6%（issue #21796） | 无反代，客户端直连 Responses API | A 类主对标；approval/sandbox/app-server 参照；与 opencodex / Codex Router 勿混 |
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
3. **命中率公开数据稀少**：仅 Pi（80%+，双标记口径）与 Codex（62%，会话口径）可查——Pawork 的 95/97/99 目标（排除冷启动的会话级聚合口径）无外部直接可比基线，达标判断以自建三场景真实测试为准（[附录 C](#附录-c-决策记录-d1d8-与并入约定原-researchmulti-account-quota-plan-mergemd) §1.3）。

---

## 附录 B 分功能方案 F1–F6（原 research/multi-account-quota-proposals.md）

> 状态：**已确认**（2026-08-14 用户按推荐通过全部方案 F1–F6，决策原则：**减少实现复杂度、优先缓存命中**）。**2026-08-25 并入本手册**：每项选定方案与理由完整保留，对比过程压缩；原文全文见 git 历史 `docs/research/multi-account-quota-proposals.md`。决策记录见 [附录 C](#附录-c-决策记录-d1d8-与并入约定原-researchmulti-account-quota-plan-mergemd)；外部依据见 [附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd)（下文引用记作「A §N」）+ Pawork V1 既有资产（`provider-control` 13.5k 行、`quota-service` 核心约 6k 行、`usage-ledger`、orchestration budget-gate，对照见 A §7）。文中 S0–S13 阶段编号为撰写时点 V2 规划语境（历史）；转正落点按 [../ROADMAP.md](../ROADMAP.md) §5 候选池重新登记（见附录 C §4）。
>
> 全文适用的架构红线：Agent Engine 不按 Provider 名走特例（能力差异一律经 registry/capability 数据表达）；Secret 不落数据库、不入日志/事件流/仓库（账户凭证走 `pawork-auth` 的仓库外 `auth.json`，0600、原子写、损坏 fail-closed）；canonical domain 纯净（厂商字段不进 `pawork-domain`/`pawork-api` 核心类型）；所有路由/切换决策事件化、可持久化、可重放。

### B §0 功能域划分与结论一览

| 功能域 | 推荐方案 | 主要落点（V2 语境） | 性质 |
| --- | --- | --- | --- |
| F1 多账户模型与凭证 | F1-B：激活 V1 账户层 + 订阅 plan 凭证类型 + auth 文件多凭证 | S6 铺垫 / S11 主体 | 沿用 + 扩展 |
| F2 额度感知与预算控制 | F2-A+B：LocalLedger 派生 + 被动信号捕获；远端适配器保持冻结 | S11 | 沿用 + 小新增 |
| F3 切换与路由策略 | F3-B：会话-账户亲和默认开 + 新会话再平衡 + 分类错误 rebind | S11 | 沿用 + 策略新增 |
| F4 子 Agent 跨供应商调用 | F4-A+B：声明式绑定（默认继承、显式覆盖）+ budget-gate 预算分配 | S9 铺垫 / S11 主体 | 沿用 + 新增契约 |
| F5 输入缓存策略控制 | F5-B：canonical cache 注解 + registry 能力表 + adapter 映射 + 用量入账 | S2 占位 / S5 分段 / S6 全量 | 新增（附加式契约扩展） |
| F6 对外账户池网关模式 | F6-A：近期不做；以 openai-compatible 上游接外部网关；长期按需评估 channels 扩展 | 暂不排期 | 决策登记 |

### B F1 多账户模型与凭证

**目标**：同一 Provider 下管理多个账户（API key 或订阅 plan OAuth），账户携带优先级/权重/并发上限/生命周期状态，凭证安全存储，供路由层取用。

**模式**（A §2.3、§3、§6）：内核层「一 providerID 一凭证」是共性空白，代理层账户池与配置切换工具补位——「内核单凭证 + 外部池化」是生态分层，内核原生多账户是差异化机会。

**选项**：F1-A（最小：配置别名 providerID——零改动，但账户不是一等实体，无优先级/健康度/并发语义，F2/F3 无从谈起）；**F1-B（选定）**；F1-C（对齐 opencodex dashboard 的完整 UX——GUI 范畴，推迟到 Desktop Settings 增量）。

**F1-B 内容**：
1. **账户实体沿用 V1**：`ProviderAccount`（priority、weight、max concurrency、lifecycle：Active→CoolingDown→Active / BillingBlocked / Disabled）+ `CredentialMetadata`（仅 `secret_ref`、kind、expiry、refresh state）——契约已冻结（account-control schema v2），直接迁移。
2. **新增凭证 kind：订阅 plan OAuth**（ChatGPT plan / Claude plan / Copilot 等，对应 [design.md](design.md) §4.5 候选 D8）：refresh token 入 Pawork auth 文件（`pawork-auth` 已有 OAuth PKCE/Device/refresh 全流程），凭证解析链沿用 auth 文件 → env fallback；多账户 secret_ref 命名规约 `<provider>/<account_id>`。
3. **CLI 用户面**：`pawork accounts list/add/remove/enable/disable`（与 `pawork usage` 同批），`auth set-key` 扩展 `--account` 维度。

**选定理由**：词表与状态机已是 V1 冻结资产（A §7 对照表），激活成本远低于新造；auth 文件已有 0600、原子写、损坏 fail-closed、掩码展示与日志脱敏基线；plan OAuth 是两条真实测试通道（GLM Coding Plan、OpenCode Go）之后最现实的账户形态。

**契约影响与开放问题**：plan-credential kind 为 account-control schema 的**附加**变体（unknown-field fail-closed 契约下需登记 schema 迁移）；ToS/封号风险需在文档显著声明（A §6 第 7 条——Anthropic 已封锁第三方 OAuth 的先例）；**不做**身份伪装类手段（Claude Code UA 伪装、`identity-confuse`），宁可少接一家。附属候选 G6：`pawork-compat` 增加账户/端点只读导入源（`~/.codex/auth.json`、cc-switch SQLite、CLIProxyAPI auth-dir、opencodex config、Codex Router 托管 `config.toml` 块与 `~/.codex/codex-router` 状态目录），导入的 secret 直接转存 Pawork auth 文件、不落仓库或中间文件。

### B F2 额度感知与预算控制

**目标**：回答「这个账户还剩多少额度、下一个任务该派给谁、什么时候必须停」。

**模式**（A §3、§4.2、§6 第 2 条）：本地记账 → 被动信号 → 主动探测三形态叠加、可信度分级是共同做法。

**选项与选定（F2-A+B 组合）**：
- **F2-A（基线，选定）**：LocalLedger 派生——V1 `usage-ledger`（dedup_key 幂等）+ `LedgerQuotaAdapter` 按 Rolling5h/Weekly/Monthly 滚动派生 `Derived` 快照；orchestration budget-gate 消费投影。零网络请求、零 ToS 面，但只见自己消耗。
- **F2-B（叠加，选定）**：**被动配额信号捕获**——Provider adapter 在正常请求的响应头/错误体中捕获配额信息（Anthropic `anthropic-ratelimit-*`、OpenAI `x-ratelimit-*` 与 plan 窗口字段、`Retry-After`、`usage_limit_reached` 类错误体），归一为 `QuotaSnapshot`（confidence 按来源定 `Exact`/`Derived`）写入 quota 缓存与账户健康状态；不新增任何请求；归一化放 provider adapter（厂商差异不进 core，符合红线）。
- **F2-C / F2-D（保持冻结）**：六厂商远端适配器 + `RefreshScheduler`（约 8k 行主动轮询）与 WebScrape 兜底——维持冻结候审不变（激活条件见 [history.md](history.md) §1.6 冻结候审清单；原 v1-migration-reference.md §4.4 见 git 历史），不因本批候选自动解冻。

**预算执行规则沿用 V1**：仅 fresh `Exact` 且明确耗尽的信号可触发硬停止；`Derived`/`Scraped`/stale 只产软告警；budget-gate 按窗口余量为子 Agent 分配预算（联动 F4）。

**选定理由**：A 是既定项；B 增量小（adapter 内解析 + 一条快照写入路径），把额度感知从纯本地估算升级为「用真实信号校准」，且完全被动。

**契约影响与开放问题**：`QuotaSnapshot`/`QuotaProvenance` 契约已有，B 只新增来源枚举值（附加式）；plan 窗口 reset 时间的不确定性用 V1 `QuotaReset::uncertain` 表达；各厂商配额头覆盖面在迁移各 adapter 时逐家登记。

### B F3 多账户切换与路由策略

**目标**：新会话选对账户、会话中不乱跳、账户出问题时正确切换，且一切可解释、可重放。

**模式**（A §3.4、§4.4、§6 第 1/3/6 条）：sticky session 是订阅池代理标配；新会话才再平衡；错误分类驱动 failover（分类错了会误惩罚账户）。

**选项与选定（F3-B）**：F3-A（手动切换 `/provider` `/model`——必要但不满足池化）为既有基线；**F3-B（选定）：缓存感知亲和 + 新会话再平衡**，全部落在 V1 `provider-control` 既有机制上：
1. **会话-账户亲和默认开**：`SessionBinding`（Unbound→Bound→Rebinding→Bound）作为默认行为；绑定键 = session_id（Pawork 自有会话体系，无需像代理们那样对请求内容做 hash）。
2. **新会话再平衡**：新 session 首次 `AcquireRequest` 走 `RoutingPolicy` 完整过滤链（capability → tenant → health → priority → affinity → weighted/fill-first → concurrency），**新增「配额余量优先」策略**（对齐 opencodex `quota` 策略与 CLIProxyAPI-Plus 排序：比较各账户最紧窗口剩余比例，消费 F2 快照），与既有 SWRR/Fill-First 并列可选。
3. **Rebind 仅由 `ErrorClassifier` 触发**：沿用 V1 错误表（RateLimited → scope-aware cooldown、QuotaExceeded hard → failover、AuthRejected → refresh-once、BillingBlocked → 显式恢复、ClientCancelled/ContextTooLarge/ProtocolIncompatible 不轮换）——已覆盖 CRS issue #1000「非限流 429」教训。
4. **决策可观测**：每次选择/淘汰/rebind 进 `RouteDecision`（不含 Secret）并事件化；缓存命中率（F5 用量数据）纳入账户健康视图，作为「亲和值不值得保」的量化依据。

F3-C（请求级轮换）**不作默认**——破坏 prompt cache（A §5.4），仅保留为 RoutingPolicy 可选策略，文档标注适用场景（无缓存诉求的批量吞吐）。

**选定理由**：与外部最佳实践收敛一致，且 V1 的 binding/routing/health 三件套已具备全部骨架，实际新增只有「配额余量优先策略 + 亲和默认开 + 命中率指标」三点。

**契约影响与开放问题**：`RouteDecision`/binding 事件已在 V1 词表；绑定粒度默认 session（run 级可配）；亲和过期时长对齐上游 cache TTL，默认 1h 可配；**不做** `identity-confuse` 类身份重写（合规红线，见 F1）。

### B F4 子 Agent 跨供应商调用

**目标**：编排（supervisor）派发的每个子 Agent 可声明自己的 provider/model/账户约束与预算，路由层据此供给，事件流可区分归属。

**模式**（A §2、§4.1、§6 第 4 条）：三模式——声明式绑定（opencode `agent.model` + 权限派生）、in-band 标签（CCR，「改不了客户端」的补丁）、模型即子代理槽位（opencodex）。Pawork 同时控制引擎与编排两侧，**in-band 标签无存在理由**。缓存取舍见 A §5.5。

**选定（F4-A+B 组合）**：
- **F4-A（主体）**：**声明式绑定**——Agent Profile（profiles 契约）与编排 spawn 参数中声明 `provider` / `model` /（可选）`account_hint` / `budget`；supervisor spawn 时写入 `RouteContext`，由 `provider-control` 完成账户选择（子 Agent 不直接接触凭证，符合「Agent 只提交 AcquireRequest」红线）；budget-gate 按声明为子 Agent 划预算。多 Agent demo（GLM + OpenCode Go 双子 Agent）即最小验收场景。
- **F4-B（默认行为）**：**默认继承、显式覆盖**——未声明绑定的子 Agent 继承父的 provider/model/账户绑定（同账户共享缓存前缀、行为可预期），声明了则覆盖。对应 opencode 继承语义。
- F4-C（不采纳）：CCR 式 prompt 内标签路由——引擎两侧皆可控时属多余间接层，且污染 prompt、不可类型化审计。

**契约影响与开放问题**：Agent Profile schema 增加绑定字段（随 profiles 契约激活一并定型，避免后补破坏冻结契约）；`TeamEvent`/子 Agent 事件已含归属，需确认 `ProviderRequestStarted` 携带 account 维度（脱敏 hint，不含 secret）；子 Agent 并发对单账户 max concurrency 的挤占沿用 lease 并发上限 + fill-first 下沉，不为子 Agent 特设通道。

### B F5 输入缓存策略控制

**目标**：把 prompt caching 从「各 adapter 自行其是」升级为 canonical 可配置、可观测的一等能力：断点/TTL/亲和键统一策略化，缓存用量入账，命中率可查。

**模式**（A §5）：厂商机制分显式断点与隐式前缀两族 + 亲和键；客户端实践收敛为「静态前缀 + 少量滑动断点 + 会话稳定亲和键」；compaction 与缓存天然冲突需折中；用量字段可归一为 cache_read/cache_write 二元。

**选项与选定（F5-B）**：F5-A（各 adapter 硬编码断点——能用，但策略不可配、不可跨厂商观测、compaction 联动无从挂接）为现状延伸；**F5-B（选定）三层设计**：
1. **canonical 注解层**（canonical request）：`CanonicalModelRequest` 增加缓存策略字段（枚举 `Off` / `Auto` / `Explicit { retention: Default | Long }`）与「前缀稳定性分段」标注——context 产出按（static system｜tools｜history｜dynamic tail）分段标记可缓存边界。**不含任何厂商字段**（cache_control 不进 canonical 类型）。
2. **adapter 映射层**（providers + model registry）：缓存能力进 registry 数据表——`cache_kind`（explicit/implicit/none）、`min_cacheable_tokens`、`supports_ttl`、`affinity_key_kind` 等；显式族映射为断点（缺省策略对齐 pi/opencode 收敛实践：system 尾 + 末 tool 定义 + 滑动末 user；TTL 按 retention 映射 5m/1h 或 24h retention 参数）；隐式族映射为亲和键（`prompt_cache_key` / session 头 = Pawork session_id）。Engine 全程零厂商分支（红线），一切查表。
3. **用量与观测层**：cache_read/cache_write token 归一进 usage（`ModelResponseSummary` 与 usage-ledger 记录增列），计价按 registry 单价（写入溢价/命中折扣）；`pawork usage` 与事件流展示命中率；命中率喂给 F3 账户健康视图。
4. **配套纪律（context/compaction，不新增包）**：static-first 排序、工具列表确定性排序、历史 append-only；**compaction 视为缓存重置事件**——触发时机偏向任务自然边界、压缩后首请求即预热新前缀、`CompactionCompleted` 事件附缓存影响标注。

F5-C（网关式响应缓存/语义缓存）不属于本域——那是输出缓存，不做。

**选定理由**：这是唯一「V1 没有对应资产」的净新增，但外部实践已高度收敛（A §5.2 四家客户端做法一致到细节），可低风险抄收敛解；分层设计保住两条红线（canonical 纯净、engine 零厂商分支）。

**契约影响与开放问题**：`CanonicalModelRequest`/`ModelResponseSummary` 字段新增为**附加式**，serde 向后兼容 + golden 先行（[architecture.md](architecture.md) §3.2 原则）；分阶段：契约占位 → context 分段产出 → adapter 映射与用量入账全量（见 B §7）。开放问题——GLM Coding Plan 套餐不参与缓存计费（A §5.1），计价表需按「计费模式」区分套餐/按量；OpenAI `prompt_cache_key` 每键 ~15 rpm 限制在高并发编排下的分片策略（子 Agent 家族共享 key 时，参照 A §5.5 Codex 命中率 62%→9.6% 反例）。

### B F6 对外账户池网关模式（决策项）

**问题**：是否让 `pawork` 像 opencodex/CLIProxyAPI 那样对外暴露 OpenAI/Anthropic 兼容端点，把自己的账户池服务给其他客户端？

**选定（F6-A：不内建）**：近期需求两条腿走——① Pawork 作为消费者，经 openai-compatible adapter 把外部网关（opencodex、CLIProxyAPI、Codex Router 等）当上游（仅需 base_url，已支持）；② Pawork 自身多账户能力对内服务（F1–F4）。
- F6-B（长期候选，P3）：以 channels 扩展 feature 评估——V1 `client-claude-gateway` / `client-codex-app-server`（14.4k 行 channels 资产）已有「外部客户端协议 → Pawork」翻译层，反向暴露「模型代理端点」是其邻接能力；若未来有真实需求（如团队共享账户池），按 [../ROADMAP.md](../ROADMAP.md) §5 候选池流程评估。
- F6-C（不做）：独立网关 app——偏离产品定位（Coding Agent 而非 API 网关），且订阅账户转售式代理的 ToS 风险最重（A §6 第 7 条）。

### B §7 分阶段落地图（确认时点的 V2 规划语境，历史对照）

| 阶段 | 并入内容 | 涉及包 |
| --- | --- | --- |
| S2 | F5-B-1 canonical 缓存注解占位（契约激活即完整形状，字段暂闲置） | api/provider-core（契约） |
| S5 | F5-B-1 context 前缀分段产出；缓存用量并入 token 统计路径 | engine、provider-core、session |
| S6 | F1-B-2 plan 凭证 kind 铺垫 + auth 文件多凭证命名；F5-B-2/3 adapter 缓存映射、registry 能力表、用量入账 | providers、auth、provider-core、config |
| S9 | G6 账户/端点导入源（Claude/Codex/opencodex/cc-switch/CLIProxyAPI/Codex Router 布局）；F4 Agent Profile 绑定字段随 profiles 契约定型 | compat、resources |
| S11 | F1-B 账户层激活与 CLI；F2-A+B 额度感知；F3-B 亲和 + 再平衡 + 配额余量策略；F4-A+B 子 Agent 绑定与预算 | provider-control、quota、control-plane、orchestration、cli |
| 冻结不变 | quota 六厂商远端适配器 + WebScrape（F2-C/D） | — |
| 明确不做 | 请求级默认轮换（F3-C）、in-band 子代理标签（F4-C）、身份伪装/identity-confuse、响应缓存（F5-C）、独立网关 app（F6-C） | — |

与 [design.md](design.md) §3 已确认扩展功能族的对应：G1↔F1、G2↔F2、G3↔F3、G4↔F4、G5↔F5、G6↔F1 附属、G7↔F6。

### B §8 决策清单

原 §8 所列 5 项待拍板决策（F1-B 订阅 plan OAuth、F3-B 亲和默认开、F5-B 契约扩展、F6-A 不内建网关、落地方式）已于 2026-08-14 全部按推荐确认，见 [附录 C](#附录-c-决策记录-d1d8-与并入约定原-researchmulti-account-quota-plan-mergemd) §2 决策状态表 D1–D8。

---

## 附录 C 决策记录 D1–D8 与并入约定（原 research/multi-account-quota-plan-merge.md）

> 用途：多账户功能族从调研走向排期的**决策唯一入口**。建立 2026-08-14；凭证存储已按 2026-08-15 用户决策更新（OS Keychain 方案由仓库外 `auth.json` 文件后端取代）；**2026-08-25 并入本手册**（D1–D8 决策、执行期凭证 fail-closed 约定与缓存命中率 95/97/99 目标完整保留；原文全文见 git 历史 `docs/research/multi-account-quota-plan-merge.md`）。上游：[附录 A](#附录-a-多账户配额缓存机制调研原-researchmulti-account-quota-referencemd)（调研）· [附录 B](#附录-b-分功能方案-f1f6原-researchmulti-account-quota-proposalsmd)（方案 F1–F6）· [design.md](design.md) §3（已确认扩展功能族 G1–G7）。

### C §1 已确认的工作约定（2026-08-14 用户确认）

> 决策原则：**减少实现复杂度、优先缓存命中**。后续方案取舍与实现细节冲突时以此为裁决基准。

#### C §1.1 执行期凭证（fail-closed）

适用于本功能族全部后续任务（含并入 plan 后的开发任务），与架构红线「Secret 不落库、不入日志」同源：

1. **供给方式**：执行任务所需 API key 由用户在任务开始时临场提供；写入**本地环境变量**（会话级或用户级），或经 `pawork auth` 存入 `$PAWORK_HOME/auth.json` / `~/.pawork/auth.json`（0600、原子写、损坏 fail-closed）。二者均为本机存储。
2. **不落仓库**：凭证不写入任何可能被提交到远程仓库的文件——源码、配置、fixture、脚本、文档一律使用占位符或 env 引用；`.env` 类文件即使使用也必须在 `.gitignore` 内且不作为推荐通道；明文 key 不出现在日志与任务报告中。
3. **缺失即终止（fail-closed）**：任务执行中发现凭证缺失或失效（更换环境、环境变量丢失、key 过期等）→ **立即终止当前任务并明确提示用户重新提供**；不静默跳过、不自行换用其他凭证、不降级为 mock 继续。（对应 opencodex 对 401/403 的 fail-closed 立场。）

#### C §1.2 开发期测试策略：少测试、无门禁

- 开发期**尽量少测试**：每阶段只保留**关键模块级测试**（能证明该阶段核心行为的最小集合）；不设阶段全量门禁（与 V1 时期历史原则一致，原 v1-migration-reference.md §6 见 git 历史）。未来若决定发布再另立任务。
- 并入 plan 的任务执行时，按此原则**核减**各阶段计划中已写入的非关键测试项：只删不加，保留冒烟、关键路径与契约 golden。

#### C §1.3 缓存命中率目标与真实命中测试（95 / 97 / 99）

- **指标定义**：会话级聚合命中率 = Σ cache_read ÷ Σ(cache_read + cache_write + 未缓存 input token)。自会话第 2 个请求起统计（首请求为冷启动写入）；compaction 后的首个请求同样计为冷启动，不计入达标口径但原始值一并记录。
- **前提**：会话-账户亲和开启（D3）、会话内同账户、目标 provider 支持 prompt caching；数据来源为 F5 契约的 cache_read/cache_write 用量字段（D4）。
- **场景**：① 多轮对话；② Agent 工具循环（含子 Agent 派发）；③ 长任务（触发 compaction 的长会话）。

| 档位 | 数值 | 口径 |
| --- | --- | --- |
| 低目标（下限） | ≥ 95% | 任一场景低于即测试失败，先修缓存策略/前缀稳定性再继续 |
| 平均目标 | ≥ 97% | 三场景均值，开发期常态要求 |
| release 目标 | ≥ 99% | 未来发布验证任务：三场景均值 ≥99%，单场景仍不得低于低目标 |

- **执行方式**：真实 provider 打点（凭证遵守 C §1.1，缺失即终止）；默认不进 CI，本地/手动触发；结果随 `pawork usage` 与事件流可查。
- **落点**：首个对话场景命中测试随凭证/缓存映射落地；agent/长任务/多 Agent 场景随多账户激活补全；release 目标只作为未来发布任务输入。

### C §2 决策状态表 D1–D8

| # | 事项 | 状态 | 说明 |
| --- | --- | --- | --- |
| D1 | 执行期凭证约定 | ✅ 已确认（2026-08-15 更新） | 见 C §1.1。env 为开发期/headless 供给通道，仓库外 auth 文件为产品存储层，共同底线是不落仓库、数据库、日志与事件流 |
| D2 | F1-B 订阅 plan OAuth 凭证 kind | ✅ 已确认（2026-08-14 按推荐） | 纳入范围，凭证契约定型时实施；「不做身份伪装」立场一并确认 |
| D3 | F3-B 会话-账户亲和默认开 + 「配额余量优先」策略 | ✅ 已确认（2026-08-14 按推荐） | 作为多账户场景默认行为（直接服务「优先缓存命中」原则）；效果与前后差异见 C §3.1 |
| D4 | F5-B canonical 缓存契约扩展（附加式字段） | ✅ 已确认（2026-08-14 按推荐） | 契约激活时字段一次就位，golden 先行；差异见 C §3.2 |
| D5 | F6 网关形态 | ✅ 已确认（F6-A） | 不内建独立网关：能力以进程内库实现；不兼容供应商处理见 C §3.3 |
| D6 | 并入 plan 的方式 | ✅ 已确认 | 由后续独立任务执行，C §4 为其任务书（落点已随 V2 收官失效，见该节） |
| D7 | 开发期测试策略：少测试、无门禁 | ✅ 已确认（2026-08-14） | 见 C §1.2；并入 plan 时核减非关键测试项 |
| D8 | 缓存命中率目标 95/97/99 与真实命中测试 | ✅ 已确认（2026-08-14） | 见 C §1.3；对话场景起步，多账户激活后补全场景，99% 目标保留给未来发布验证任务 |

### C §3 疑问解答归档（2026-08-14）

#### C §3.1 F3-B「会话-账户亲和默认开」：效果、目标与前后差异

**行为**：多账户池启用后，每个会话首次请求时路由层经完整过滤链选定账户并绑定（`SessionBinding`）；会话内所有后续请求固定走该账户；仅 `ErrorClassifier` 分类的账户级错误（429 硬限流、配额窗口耗尽、凭证失效且 refresh 失败、计费封锁）触发换绑；新会话才重新做负载均衡。「配额余量优先」= 新会话选账户时比较各账户最紧配额窗口的剩余比例，选余量最大者（对齐 opencodex `quota` 策略）。

**目标**：① 保住 prompt cache——厂商缓存按账户/组织命名空间隔离，换账户 = 会话前缀缓存整体作废；② 切换可预测、可解释——绑定与换绑均事件化、可重放；③ 降低订阅账户风控面——同一会话在多账户间跳动是可识别特征（claude-relay-service 作者明确提示封号风险，附录 A §4.4）。

**前后差异**：

| 维度 | 不开亲和（每请求轮换） | 开启亲和（会话绑定） |
| --- | --- | --- |
| 缓存命中 | 趋近 0（每次换号冷启动重建） | 会话内最高，只受 TTL / compaction 影响 |
| 成本 | 长前缀会话每轮按全价重算 + 写入溢价（Anthropic 写 1.25–2×） | 命中部分按 ~0.1× 计价 |
| 首 token 延迟 | 每轮明显变慢 | 稳定 |
| 负载分布 | 各账户最均匀 | 长会话集中占用单账户；绑定账户被限流时有一次性 rebind 损失 |
| 对当前状态影响 | —— | **零**：单 provider 单凭证下亲和是空操作，真正生效在多账户激活后 |

轮换（RoundRobin/SWRR）不删除，保留为显式可选策略（适用无缓存诉求的批量吞吐场景）。

#### C §3.2 F5-B canonical 缓存契约扩展：批准与否的差异

**改动本体**（全部附加式可选字段，serde 默认值向后兼容，老数据不受影响）：① `CanonicalModelRequest` 增缓存策略枚举（`Off / Auto / Explicit{retention}`）；② context 产出带前缀稳定性分段标注；③ `ModelResponseSummary` / usage 记录增 `cache_read` / `cache_write` 计数。

| 维度 | 批准（F5-B） | 不批准（维持 F5-A） |
| --- | --- | --- |
| 策略配置 | 统一可配：换 1h TTL、关缓存 = 改配置 | 各 adapter 硬编码，改策略 = 改代码 |
| 基础功能 | 全厂商按能力表映射 | Anthropic 缓存仍可用（adapter 内置），但一家一个形状 |
| 成本核算 | 命中按 ~0.1×、写入含溢价入账，`pawork usage` 与预算 gate 数字准确 | 缓存 token 按全价或不计 → **成本系统性高估**，F2 余量判断随之失真 |
| 观测联动 | 命中率进账户健康视图（支撑 F3 亲和决策）；compaction 有缓存重置挂接点 | 均无挂接点 |
| 时机成本 | 契约激活时字段一次就位 | 之后补加需对已冻结契约走变更流程（golden 重录 + schema 迁移登记），更贵 |
| 风险 | 净新增字段，动「激活即 V1 完整形状」原则 → 需明确批准 + golden 先行 | 无契约风险 |

#### C §3.3 多供应商/多账户切换是否需要网关；不兼容供应商如何处理

**结论：需要网关的能力，不需要网关的形态。** opencodex / CLIProxyAPI 做成独立代理进程，是因为它们改不了所服务的客户端（固定协议黑盒），只能在进程外截 base_url。Pawork 的引擎、编排、CLI 全部自持，账户池、路由、亲和、冷却做成**进程内库**（V1 `provider-control` 已有全套骨架），调度能力等同网关，且：少一个常驻进程/端口/运维面；凭证只从仓库外 auth 文件解析为进程内租约；没有本地 HTTP 明文一跳。多账号切换 = 换一张内部 `CredentialLease`，Engine 无感知。

**不兼容供应商三层处理**：

1. **协议形态不兼容**（Anthropic Messages / OpenAI Chat Completions / OpenAI Responses / Gemini 四形态）：每形态一个 adapter 做 canonical ↔ 厂商翻译；新增 OpenAI 兼容形态供应商 = 填配置（base_url + 模型表），全新协议形态才需新写 adapter，核心不动。
2. **能力不兼容**（不支持工具 / 图片 / 缓存 / thinking / 结构化输出）：model registry 能力表声明 + Engine 查表降级（thinking → 标签文本、图片 → 占位、缓存注解 → 忽略、工具 → 禁用并提示）；红线禁止按厂商名写特例分支。Pi 的 compat 矩阵为成熟先例（附录 A §2.2）。
3. **完全接不进**（无 API、OAuth 被厂商封锁如 Claude plan）：不硬接、不做身份伪装（UA 伪装 / identity-confuse 均排除）；用户自愿时把外部网关（opencodex、Codex Router 等）当一个 openai-compatible 上游接入，风险外置。**注意**：外部网关与 Pawork 双层账户池并存时，同一 provider 的轮换只在一层启用，否则双层轮换互相毁缓存。

### C §4 并入计划任务书（历史，落点已失效）

原 §4 为「把 F/G 方案并入 `plan/S*.md`」的任务书：前置条件 D1–D8 已于 2026-08-14 全部确认。V3 更新（2026-08-18）：所列落点文件 `plan/S*.md` 已随 V2 收官删除，且 R0 已归档 account-control-v1 装配面（原 plan/R0 任务书 D2）；本节保留为**方案内容清单**（F/G/D 决议仍有效）——多账户任务转正时按 [../ROADMAP.md](../ROADMAP.md) §5 候选池重新登记落点（V3 布局下另立任务书），方案 → 阶段映射见 [附录 B](#附录-b-分功能方案-f1f6原-researchmulti-account-quota-proposalsmd) §7 落地图（历史对照）。执行约束沿用 C §1 凭证约定；明确不做项沿用 B §7（请求级默认轮换、in-band 子代理标签、身份伪装、响应缓存、独立网关 app）；99% release 目标保留为未来发布任务输入。原逐文件写入内容清单见 git 历史。
