# Pawork 开发路线图

本路线图是**目录式索引**：顶部是任务进度表与「下一个推荐任务」，下方按 Phase 列出每个任务的简短介绍与详情链接。每个任务的**最终目的、细分步骤、产出与验收标准**见 `plan/` 目录下对应文件。

> 范围说明、MVP 边界、分层验证与延后门禁、测试缓存清理和风险监控见 [plan/README.md](plan/README.md)。

## 如何使用

1. 查看「进度总览」了解各 Phase 进度，查看「下一个推荐任务」获取当前应执行的任务。
2. Phase 内按任务 ID 与依赖执行；跨 Phase 按「实施波次」推进，不机械追求数字顺序。
3. 点开 `plan/<id>-<slug>.md` 获取该任务的细分步骤与最终目的。
4. 完成任务后：把对应行的状态改为 `🟢`、更新「进度总览」计数与「下一个推荐任务」。新完成任务须在 plan 元信息记录交付成熟度，至少达到 `TargetVerified`；不要求每个任务重复跑 workspace 全量门禁。历史 `🟢` 只代表既有完成记录，是否已接线仍以源码、运行证据和 remediation 复核为准。
5. 任务粒度：数小时内可独立完成、独立验收、写入集收敛到单一 crate 或一组紧相关文件。
6. 引入任何第三方依赖前先对照「依赖选型基线」一节；新增依赖必须同步回该节与对应 plan 任务。
7. 功能簇开发期只做与写入集直接相关的快速验证；workspace 全量 build/test/clippy、跨平台、完整 contract/fuzz/chaos 在功能簇收尾、发布候选或维护升级时集中执行，门禁后按 [plan/README](plan/README.md#测试节奏与缓存清理) 清理隔离缓存。

状态符号：`🟡未开始` · `🔵进行中` · `🟢已完成` · `⚪已归档/推迟`。架构红线见 [AGENTS.md](AGENTS.md) §2 与各 [ADR](docs/adr/)。

## 进度总览

| Phase | 主题 | 任务数 | 已完成 | 状态 |
| --- | --- | --- | --- | --- |
| 0 | 架构与协议冻结 | 12 | 12 | 🟢已完成 |
| 1 | 基础设施 | 13 | 13 | 🟢已完成 |
| 2 | 首个真实 Provider | 12 | 12 | 🟢已完成 |
| 3 | Agent Loop | 11 | 11 | 🟢已完成（P3-11 TargetVerified） |
| 4 | 核心工具与权限 | 13 | 13 | 🟢已完成（P4-13 TargetVerified） |
| 5 | Session、Branch 与 Compaction | 10 | 10 | 🟢已完成（P5-10 TargetVerified） |
| 6 | 主要 Provider | 14 | 9 | 🔵进行中（P6-10~13、P6-14 评审修复待启动） |
| 7 | Git、Diff 与 Worktree | 9 | 8 | 🔵进行中（P7-9 评审修复待启动） |
| 8 | Skills、Prompts 与 Instructions | 8 | 0 | 🟡未开始 |
| 9 | MCP | 7 | 0 | 🟡未开始 |
| 10 | WASM Plugin | 6 | 0 | 🟡未开始 |
| 11 | Sandbox 与跨平台强化 | 8 | 0 | 🔵进行中（P11-1 骨架） |
| 12 | Multi-Agent | 6 | 0 | 🟡未开始 |
| 13 | CLI Host 与多 GUI 协议 | 10 | 0 | 🟡未开始 |
| 14 | 模型用量与额度监控 | 9 | 0 | 🟡未开始 |
| 15 | Provider Native Capabilities | 9 | 0 | 🟡未开始 |
| 16 | Modern Agent Workflow | 9 | 0 | 🟡未开始 |
| 17 | Ecosystem & Host Compatibility | 13 | 0 | 🟡未开始 |
| 18 | Account Control Plane & Client Adapters | 15 | 0 | 🟡未开始 |
| 19 | Desktop GUI | 16 | 0 | 🟡未开始 |
| **合计** | — | **210** | **88** | — |

> 计数口径：任务数与已完成数均包含 ⚪（归档/推迟）任务。
>
> Phase 0 与 Phase 1 已有历史本地构建、测试和 Clippy 记录；自本次规划起不再把「每个 Phase 重跑 workspace 全量门禁」作为任务完成前提。三平台 GitHub Actions 仍按 `workflow_dispatch` 手动触发，其远程结果不计入本地完成状态。

## 下一个推荐任务

> 🎯 **P6-14 Phase 6 评审修复** —— P1-13 / P2-12 / P3-11 / P4-13 / P5-10 已完成，Phase 1–5 连续达到完成状态；剩余汇总评审任务为 P6-14 / P7-9。下一项处理 Provider 安全/正确性、OAuth 接线与依赖基线，详情见 [plan/P6-14-review-remediation.md](plan/P6-14-review-remediation.md)。

## 实施波次与门禁节奏

Phase 编号保留架构与文档索引意义，实际开发按结构性依赖分波推进：

1. **主干补线**：完成 P1-13 / P2-12 / P3-11 / P4-13 / P5-10 / P6-14 / P7-9，优先消除安全红线与“模块存在但未接入 Agent Loop”的缺口；每项只跑受影响 crate 的定向测试。
2. **Provider v2 前置**：完成 P15-1～P15-8，再由 P15-9 一次性执行 GPT / Claude / Grok 的完整 Provider Contract v2 门禁。P6-1 / P6-2 / P6-10 保留基础协议路径，Responses / modern server tools 在 Phase 15 扩展。
3. **账号控制面基础**：完成 P18-1～P18-9。先建立 `Tenant/Principal`、`ProviderAccount/Credential`、Lease、错误/健康状态机、确定性路由、Session Affinity 与多维 Usage Ledger；`ModelProvider` 保持不变，旧配置映射到 `local/default + SingleCandidate`。Phase 12、Phase 14 与外部 Client Adapter 不得各自复制账号选择逻辑。
4. **资源与 Host 基础**：推进 Phase 8、Phase 9、P13-1 / P13-2、P16-1 / P16-2 与 P18-10，先形成确定性资源、MCP、CLI/Core 正式宿主、可审批 Plan 和统一 Client Adapter 契约的最小闭环。
5. **Process 与扩展生态**：完成 Phase 10、Phase 11，再推进 P16-4 / P16-6 与 P17-1～P17-4；可安装插件、用户 Hook、LSP 与后台进程闭环后执行一次相关 crates 的 L2，不跑无关功能簇。
6. **Workflow、额度与编排**：推进 Phase 14、Phase 12、P16-3 / P16-5 / P16-7～P16-9、P17-5 / P17-6 与 P18-13 / P18-14；Quota 视图消费 P18-8 账本，Agent 只经 Lease 获取 Provider 资源，稳定后执行 workflow/orchestration L2。
7. **公共 Client 与远程能力**：完成 P18-11 / P18-12、P17-7～P17-13，最后由 P18-15 集中执行账号池属性/并发、迁移、跨租户隔离、Codex/Claude/ACP golden 与故障注入门禁；发布候选再执行 workspace 全量、三平台、安全、性能、fuzz/chaos 与协议兼容 L3。
8. **Desktop GUI**：P13-2～P13-10 稳定后先完成 P19-1～P19-9，用 Mock/Protocol Client 打通独立 Desktop Shell、状态投影与 Coding Agent 主交互；P19-10～P19-14 随 Phase 8～18 对应能力接线，P19-15 负责签名分发，最后由 P19-16 集中执行 Desktop contract、三平台 E2E、visual、accessibility、性能与安全门禁。GUI 不反向成为 Core 前置。

开发期不得为追求“全绿”阻塞快速迭代，但安全红线、事件可重放、Secret 不落库、路径越界和破坏性进程清理必须随改动立即定向验证。完整策略与清理命令见 [plan/README](plan/README.md#测试节奏与缓存清理)。

## 关键路径

    Domain → Mock Provider → Event Store → OpenAI-compatible
          → Agent Loop 主干补线 → Built-in Tools / Policy
          → Sessions/Compaction → Git/Diff → Canonical Tool v2
          → OpenAI / Anthropic / xAI Native APIs
          → Tenant/Principal → ProviderAccount/Credential Lease
          → ErrorClassifier / RoutingPolicy / Usage Ledger
          → Skills / MCP → Plan / Background Task → ClientAdapter
          → Hooks / Multi-Agent / Agent Profile → Codex / Claude / ACP
          → Marketplace / LSP → Goal / Automation / Memory → SDK / Remote / Browser
          → Desktop Shell / State Sync → Timeline / Composer / Diff / Terminal
          → Settings / Workflow UI → Signed Desktop Release

在核心 Coding Agent 能可靠完成真实仓库任务、Provider v2 语义与 Phase 18 账号控制面基础稳定前，不进入 Multi-Agent 与外部 Agent Client 大规模接入；Phase 15 与 P18-1～P18-9 是 Phase 8～18 扩展前的结构性前置，不按 Phase 编号机械串行。Phase 19 的 Shell/协议投影可在 Phase 13 后并行，但业务页面只能消费已交付的 Core 契约，不能用前端本地状态伪造未实现能力。

> CLI Host 与多 GUI 协议（Phase 13）是 Core 的正式运行入口与 GUI 接入边界；其协议冻结部分（GUI Connection Protocol / Transport 抽象类型）随 [P0-8](plan/P0-8-core-api.md) 提前完成，运行时实现按依赖关系推进。

## 依赖选型基线

> 2026-08 文档 review 结论。选型准则：**高采用率、文档好、能最小子集使用**；包功能太杂乱时只参考其中需要的部分自己实现。Rust 依赖表是 `[workspace.dependencies]` 的基线（落地见 [P0-1](plan/P0-1-workspace-skeleton.md)）；Desktop GUI 依赖单独锁定在 `apps/desktop`，不得进入 `pawork` Host 运行时。新增依赖必须先更新本节。

**判断准则**

- 采用条件（须同时满足）：采用率高；活跃维护（近 12 个月有发布）；docs.rs 文档质量好；只需最小子集即可用；属于自实现正确性风险高的领域（加密、编码、OS 绑定、协议编解码）。
- 自实现条件（满足其一）：生态碎片化、无明确赢家；与 canonical domain / 架构红线冲突；安全关键路径、需完整 fuzz 与审计；集成成本高于自实现成本。
- 中间态：参考其设计与字段清单，并用差分测试对照参考实现行为（见「参考 + 自实现」表）。

### 直接采用

| 类别 | 包 | 关联任务 | 理由与使用范围 |
| --- | --- | --- | --- |
| 异步运行时 | tokio | P0-1 | 事实标准；按需启用 feature |
| 异步 Trait | async-trait | P0-4、P0-5、P0-6、P0-8 | 稳定 Rust 上统一对象安全的异步接口，协议 crate 不绑定具体 runtime |
| 异步流 / 字节 | futures、bytes | P2-1、P2-5 | Provider 流式字节传输与异步消费的基础抽象 |
| 序列化 | serde / serde_json | P0-1 | 生态统一 |
| 错误 | thiserror（库）+ anyhow（应用层） | P0-7 | Rust 惯用分工 |
| 哈希 / 版本 | blake3、semver | P0-6、P1-6、P4-11 | blake3 用于 Blob 内容寻址与 checkpoint 完整性；semver 用于 Plugin API 版本 |
| 配置解析 | toml | P1-1 | 与 serde 配合 |
| CLI | clap | P1-12 | derive 宏最小化胶水 |
| 结构化日志 | tracing + tracing-subscriber | P1-9 | 脱敏（redaction）规则仍自实现；暂无线性日志文件 appender 需求 |
| SQLite 绑定 | rusqlite | P1-2 | 契合「SQLite Actor 单连接」设计；sqlx 亦活跃，但其异步池 + 编译期 SQL 检查与该设计不匹配，集成成本更高 |
| HTTP 客户端 | reqwest（rustls + stream） | P2-1、P9-2 | Provider 与 MCP Streamable HTTP 所需 |
| OS Keychain | keyring（v3） | P2-6 | Secret 不落库不入日志 |
| OAuth 基础 | oauth2 | P6-4 | 只用 PKCE + Device Flow 子集 |
| MCP SDK | rmcp | P9-1、P9-2 | 官方 SDK、跟进 MCP 2026-07-28 规范；只用 transport + codec 层；锁定小版本（2.x→3.0 有 breaking） |
| WASM 宿主 | wasmtime + wit-bindgen | P10-2、P10-5 | Component Model 成熟；fuel / 内存上限对应 ADR-012 |
| 文件遍历 | ignore + globset | P1-8、P4-6、P4-7 | ripgrep 同源，性能经过验证 |
| 正则 | regex | P4-6 | 线性时间匹配、无 ReDoS 风险 |
| 文件监听 | notify + notify-debouncer-full | P1-8、P7-6、P8-8 | file-index 以 notify + 有界通道做扫描级合并；git-service/P7-6 使用 debouncer 做缓存失效；P8-8 复用统一事件语义 |
| 路径规范化 | dunce | P1-13、P11-8 | Workspace 出口移除 Windows verbatim 前缀；后续统一短路径 / UNC 语义 |
| 编码检测 | chardetng + encoding_rs | P4-1 | Mozilla 系；二进制探测由内置启发式完成 |
| Token 计数 | tiktoken-rs | P3-2 | 仅对 OpenAI 系精确；其它 Provider 用启发式估算 |
| TS 类型导出 | ts-rs | P0-10、P13-7 | GUI Contract 类型生成，比 typeshare / specta 轻 |
| 系统目录 | directories | P1-12 | 配置 / 数据目录标准路径 |
| Linux 沙箱 | landlock | P11-1 | 基于 LSM，活跃维护 |
| Windows 绑定 | windows-rs（+ windows-service） | P11-4、P1-12 | 官方绑定 |
| Diff 生成 | similar | P7-3 | word-level diff，纯 Rust |
| PTY 基础 | portable-pty（或维护 fork） | P11-6 | 上游迭代慢，开工前先评估 fork |
| 签名 | ed25519-dalek | P10-1 | 插件 manifest 签名 |
| 测试与基准 | criterion、proptest、wiremock、insta、assert_cmd | P0-12、P2-11 等 | 基准 / 属性 / HTTP mock / 快照 / CLI e2e；解析器 fuzz 当前由 proptest 覆盖 |
| HTML 解析 | scraper | P14-4 | html5ever + selectors；仅最小子集（解析 + 选择器匹配），用于额度控制台页面抓取 |

### Desktop GUI 单独依赖基线

> Node 仅用于 `apps/desktop` 的前端构建与测试，版本以仓库 pin 的当前 LTS 为准；产物由 Tauri WebView 运行，`pawork` Host 不嵌入 Node/Bun/V8，也不依赖前端工具链。使用 lockfile 固定完整依赖图，开工时锁定 Tauri 2.x 小版本并记录升级门禁。

| 类别 | 包 / 工具 | 关联任务 | 理由与使用范围 |
| --- | --- | --- | --- |
| Desktop Shell | Tauri 2 + `@tauri-apps/api` | P19-1、P19-14、P19-15 | 仓库已冻结 Tauri 独立进程边界；只启用 window/dialog/clipboard/notification/updater 等最小官方 capability，禁止通用 shell/fs/http/sql 暴露 |
| UI | React + TypeScript + Vite | P19-1～P19-14 | Tauri 官方推荐 SPA 使用 Vite；静态前端不引入 SSR/本地 Web Server |
| 长列表 | `@tanstack/react-virtual` | P19-5、P19-8、P19-11 | Timeline、Diff、日志按 viewport 虚拟化；headless API 保留 Pawork 自有渲染与滚动语义 |
| Terminal | `@xterm/xterm` + `@xterm/addon-fit` | P19-9 | 仅渲染 P11/P13 提供的 PTY stream，不获得直接 spawn 权限 |
| Markdown | `react-markdown` + `remark-gfm` | P19-5 | 禁用 raw HTML，链接/图片 scheme 走 Pawork allowlist；不加载 CDN 脚本 |
| Renderer 测试 | Vitest + Testing Library | P19-2～P19-14 | reducer、组件、键盘与 accessibility 的快速 L1 |
| Desktop E2E | WebdriverIO + `@wdio/tauri-service` | P19-16 | Tauri 官方推荐的三平台 WebDriver 路径；浏览器模式跑快速 Mock，原生壳跑发布门禁 |

Desktop authoritative projection、`global_sequence` 去重/补洞、Snapshot/Event 合并、optimistic command reconciliation 与跨窗口选主语义由 Pawork 自实现并做状态机测试；不引入第二套服务端状态框架。官方依据：[Tauri Frontend](https://v2.tauri.app/start/frontend/)、[Tauri Capabilities](https://v2.tauri.app/security/capabilities/)、[Tauri WebDriver](https://v2.tauri.app/develop/tests/webdriver/)。

### 参考 + 自实现（第三方包仅作设计参照，只实现所需最小子集）

| 功能 | 参考对象 | 关联任务 | 不直接采用的原因 |
| --- | --- | --- | --- |
| SSE 解析 | eventsource-stream 状态机；TS eventsource-parser（Pi / Vercel 使用） | P2-2 | eventsource-stream 约 1.5k dependents 但维护停滞；SSE 规范小，fuzz 与畸形输入要求明确，自实现更可控 |
| Provider 适配 | async-openai 类型定义（字段清单）；rig-core 分层；Pi 行为 | P2-5、P6-1~3 | 整套 SDK 与 canonical domain 冲突（ADR-002）；Anthropic / Gemini 的 Rust 生态碎片化；以类型定义做字段清单 + 差分测试 |
| Partial JSON 拼接 | llmx、tool-parser、Vercel partial-json | P2-4 | 现有 crate 采用率低、测试弱；流式 Tool Call 需要确定性修复语义 |
| unified diff 解析 | unidiff 的 PatchSet 模型 | P7-3 | 优先 git 结构化输出（--raw -z --numstat）；仅参考数据模型 |
| apply_patch / edit_file | patch-apply-rs、mpatch | P4-3、P4-4 | 安全关键路径，需完整 fuzz 与审计；精确匹配与上下文校验语义必须完全可控 |
| 配置合并 | config-rs | P1-1 | 全局 / 工作区 / session / CLI 合并要求确定性优先级语义，比通用解析更重要 |
| Metrics 采集 | metrics crate 命名与标签约定 | P1-10 | 采集留在 SQLite Actor 内自实现，只参考命名 |
| OAuth Device Flow | oauth-device-flows；RFC 8628 | P6-4 | RFC 本身很小，可在 oauth2 之上实现 |
| 本地 Transport | tokio 原生 UDS / Named Pipe；interprocess（可选） | P13-4 | 无需引入整套框架 |
| OAuth 回调服务器 | tiny_http / hyper | P6-4 | 一次性本地临时监听，不引入 axum |

### 完全自实现（架构红线或安全关键）

Agent Loop / 状态机 / Tool Scheduler / 预算 / 消息队列（P3-*）；Event Store 与 Projection 语义（P1-4、P1-5，rusqlite 只是绑定层）；Policy Engine / Workspace Trust / 路径与 shell 安全（P4-9、P4-10）；Checkpoint / 回滚编排（P4-11）；Compaction 引擎（P5-5、P5-6）；JSONL 流式解析（P2-3、P5-9，serde_json 逐行即可）；沙箱编排：macOS sandbox-exec、bwrap、Windows AppContainer / Job Object 与进程树清理（P11-1~4、P11-7）；PTY 会话层：重连 / 有界缓冲 / 归属（P11-6，在 portable-pty 之上）；GUI Connection Protocol 编解码 / 快照 / 订阅 / 慢客户端隔离（P13-3、P13-5）；Credential Lease / 路由策略 / 错误健康状态机 / Tenant 隔离（P18-2～P18-9）；Desktop 状态投影 / 序列补洞 / command reconciliation（P19-2）；日志 redaction 规则（P1-9）。

### 行为参照（不作为依赖）

Pi（TS，差分测试对象，P5-9）；goose（Block → Linux Foundation，MCP-first 的 Rust 参照实现）；rig-core（Provider 中立抽象设计参照）；ripgrep / wezterm（性能与 PTY 设计参照）。

### 重点风险

- rmcp 是唯一「官方协议 SDK」级依赖：锁定小版本、跟进官方迁移指南，在 mcp-client 内封装以隔离 breaking change。
- portable-pty 上游缓慢：P11-6 开工前评估维护中的 fork（如 xpy/portable-pty-psmux）或 vendor 兜底。
- tiktoken-rs 仅对 OpenAI 精确：其它 Provider 统一启发式估算 + 容差，不依赖精确 token 数。
- Desktop 依赖 OS WebView，三平台排版、输入法、WebDriver 与内存表现存在差异；P19-16 必须使用真实平台矩阵，不以浏览器模式替代原生壳门禁。

## 遗留待决项（2026-08 review）

| 事项 | 说明 | 解决时点 |
| --- | --- | --- |
| agent-api 职责边界 | 评估与 core-api / app-service 的重叠；workspace-layout §6 依赖图仅画主干链（完整清单以其 §2 为准，含 agent-api / app-database / transport-memory / hook-runtime） | Phase 13 前 |
| provider-bedrock / provider-mistral | 已在 workspace-layout 登记但 ROADMAP 无对应任务（MVP 可推迟） | 启动时补任务 |
| 六初始供应商 crate 登记 | P6-10~13 新增 `provider-xai` / `provider-zhipu` / `provider-qwen` / `provider-moonshot`，需在 workspace-layout §2 登记并在 `ProviderId` 枚举补 Xai / Zhipu / Qwen / Moonshot | P6-10 启动时 |
| Google Gemini 降级次要 | 初始供应商集合调整为 OpenAI / Anthropic / xAI Grok / 智谱 GLM / 阿里 Qwen / Moonshot Kimi；Gemini 保留已实现的 `provider-google` 但不纳入初始范围 | 已确认（2026-08-08） |
| 缺失功能文档 | audit-log、client-auth 尚无独立 `docs/features/` 文档 | 对应 crate 实现时 |
| Phase 15–17 架构基线同步（2026-08-08 完成） | P17-4 重定位为 LSP Client Runtime；P15-7 reasoning 走 Protected Blob Store（ADR-032，不入 Keychain）；P16-7 embedding 扩展 `provider-api`（不新增 crate）；P17-1 hooks 拆 6 类 handler；P17-2 Plugin Package 增 Monitors；P17-5 effort 走 canonical；P17-9/SDK 经 pawork Host；P17-10 服从三执行位点。新 crate 已登记 workspace-layout §2.1，领域类型登记 domain-model §5 | 已确认 |
| Embedding canonical 决策 | 不新增独立 `embedding-api`/`embedding-runtime` crate；扩展 `provider-api`（`EmbeddingProvider` trait），memory-service 依赖 provider-api 保持 Provider 无关 | 已冻结（2026-08-08） |
| Protected Blob Store | reasoning 凭证等敏感制品加密落盘，ADR-032；ADR-014 收窄为小型凭证；新增 `protected-blob-store` crate（登记 workspace-layout §2.1） | 已确认（2026-08-08） |
| http-runtime 收敛（次优先级） | Marketplace / User Hooks / Forge Adapter 等应复用通用 `http-runtime`，避免反向依赖 `provider-runtime`；具体抽离时机随相关 Phase 推进 | 启动 P17-3 / Review Forge Adapter 时 |
| Automation 外部 Trigger（次优先级） | 预留 Webhook / HTTP API / GitHub / GitLab / External MCP 经 adapter 接入，不塞进 Automation Core | 随 P16-5 / 16-8 推进 |
| Review Forge Adapter（次优先级） | Review Engine 本体平台无关；预留 GitHub / GitLab / Generic forge adapter 发布评论 | 随 P16-8 后续推进 |
| 深度研究控制面缺口（2026-08-08） | Credential Pool、RoutePolicy/ErrorClassifier、Tenant/Principal、统一 ClientAdapter、Codex/Claude adapter 与多维 Usage/Audit 已拆入 Phase 18；P12 Supervisor/TaskGraph 与 P14 Quota 保留并补依赖，不重复立项 | 已映射到 P18-1～P18-15 |

---

## 任务目录

### Phase 0：架构与协议冻结

冻结所有协议与领域类型，用 Mock Provider 跑通最小链路，确保无 Tauri 依赖进入 Agent Core。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P0-1 | 🟢 | 仓库与 workspace 骨架 | 建立 workspace 根与目录骨架、CI 占位 | [详情](plan/P0-1-workspace-skeleton.md) |
| P0-2 | 🟢 | 领域类型基线 | 冻结消息/角色/内容块/元数据/ID 领域类型 | [详情](plan/P0-2-domain-types.md) |
| P0-3 | 🟢 | 事件模型 | 可持久化、可重放的事件与 schema version | [详情](plan/P0-3-event-model.md) |
| P0-4 | 🟢 | Provider 协议 | canonical 请求/流式事件/错误统一契约 | [详情](plan/P0-4-provider-api.md) |
| P0-5 | 🟢 | Tool 协议 | AgentTool/描述/结果/capability/取消 | [详情](plan/P0-5-tool-api.md) |
| P0-6 | 🟢 | 插件协议骨架 | manifest/生命周期事件接口（不实现宿主） | [详情](plan/P0-6-plugin-api.md) |
| P0-7 | 🟢 | 错误与取消模型 | 跨 crate 统一错误类别与取消语义 | [详情](plan/P0-7-error-cancel.md) |
| P0-8 | 🟢 | Core Command/Event 协议 | 面向 GUI/CLI 的稳定 Core API | [详情](plan/P0-8-core-api.md) |
| P0-9 | 🟢 | Mock Provider / Mock Tool | 可编程 mock，跑通最小链路 | [详情](plan/P0-9-mock-provider-tool.md) |
| P0-10 | 🟢 | TS 类型生成脚手架 | Rust→TS 生成管线占位 | [详情](plan/P0-10-ts-typegen.md) |
| P0-11 | 🟢 | ADR 与文档基线 | ADR-001~030 定稿与链接校验（含 CLI Host 架构修正） | [详情](plan/P0-11-adr-docs.md) |
| P0-12 | 🟢 | 基准框架骨架 | benches 目录与计时口径 | [详情](plan/P0-12-bench-skeleton.md) |

### Phase 1：基础设施

奠定存储、工作区、文件索引与可观测性，支撑后续 Session 与 Agent Loop。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P1-1 | 🟢 | 配置系统 | 确定性配置层级与优先级合并 | [详情](plan/P1-1-config.md) |
| P1-2 | 🟢 | SQLite Actor | 串行化 DB 访问、WAL | [详情](plan/P1-2-sqlite-actor.md) |
| P1-3 | 🟢 | 数据库 schema 与迁移 | 核心表与向前迁移框架 | [详情](plan/P1-3-db-schema-migration.md) |
| P1-4 | 🟢 | Event Store | 事件 append 与按 sequence 重放 | [详情](plan/P1-4-event-store.md) |
| P1-5 | 🟢 | Projection | 可重建投影 | [详情](plan/P1-5-projection.md) |
| P1-6 | 🟢 | Blob Store | BLAKE3 寻址+引用计数+GC | [详情](plan/P1-6-blob-store.md) |
| P1-7 | 🟢 | Workspace 服务 | 增删改/多 root/Git 检测 | [详情](plan/P1-7-workspace-service.md) |
| P1-8 | 🟢 | 文件索引 | 异步扫描+ignore+去抖 | [详情](plan/P1-8-file-index.md) |
| P1-9 | 🟢 | 结构化日志 | 规范字段+自动脱敏 | [详情](plan/P1-9-structured-logging.md) |
| P1-10 | 🟢 | Metrics | 关键指标采集 | [详情](plan/P1-10-metrics.md) |
| P1-11 | 🟢 | 诊断包导出 | 脱敏可分享诊断包 | [详情](plan/P1-11-diagnostics-export.md) |
| P1-12 | 🟢 | CLI Host 骨架（pawork） | serve/run/shell/watch 子命令骨架（CLI=Core 宿主） | [详情](plan/P1-12-cli-skeleton.md) |
| P1-13 | 🟢 | Phase 1 评审修复 | 安全红线（V1/V2）+ 健壮性（V3~V8）+ 基线清理 | [详情](plan/P1-13-review-remediation.md) |

### Phase 2：首个真实 Provider

先实现 OpenAI-compatible，可同时覆盖云端兼容接口与多数本地服务。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P2-1 | 🟢 | HTTP 运行时 | 超时/代理/cancel/trace | [详情](plan/P2-1-http-runtime.md) |
| P2-2 | 🟢 | SSE 解析器 | 跨 chunk/Unicode/fuzz | [详情](plan/P2-2-sse-parser.md) |
| P2-3 | 🟢 | JSON Lines 解析器 | 提前断开/错误事件/fuzz | [详情](plan/P2-3-jsonl-parser.md) |
| P2-4 | 🟢 | Partial JSON 拼接 | 跨 chunk tool arguments | [详情](plan/P2-4-partial-json.md) |
| P2-5 | 🟢 | OpenAI-compatible 适配 | canonical 转换+流式组装 | [详情](plan/P2-5-openai-compatible.md) |
| P2-6 | 🟢 | API Key 认证 | OS Keychain 存取不落库 | [详情](plan/P2-6-apikey-auth.md) |
| P2-7 | 🟢 | Model Registry | 目录/别名/能力/费用 | [详情](plan/P2-7-model-registry.md) |
| P2-8 | 🟢 | 流式组装 | 事件→领域消息 | [详情](plan/P2-8-stream-assembly.md) |
| P2-9 | 🟢 | Usage 与 stop reason | token/费用/完成原因归一 | [详情](plan/P2-9-usage-stopreason.md) |
| P2-10 | 🟢 | 重试与错误归一化 | 可重试判定/退避 | [详情](plan/P2-10-retry-error.md) |
| P2-11 | 🟢 | Provider Contract Tests | 统一测试套件 | [详情](plan/P2-11-contract-tests.md) |
| P2-12 | 🟢 | Phase 2 评审修复 | 正确性高危（V1~V4）+ 退避死代码（V8）+ 契约/文档漂移 | [详情](plan/P2-12-review-remediation.md) |

### Phase 3：Agent Loop

跑通完整 Agent 循环（含多轮工具、预算、取消、中断恢复）。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P3-1 | 🟢 | Run 状态机 | 全状态转换+事件化 | [详情](plan/P3-1-run-state-machine.md) |
| P3-2 | 🟢 | 上下文构建与预算 | 来源优先级+token 预算 | [详情](plan/P3-2-context-budget.md) |
| P3-3 | 🟢 | Provider Loop | 流式提交/解析 tool call/多轮 | [详情](plan/P3-3-provider-loop.md) |
| P3-4 | 🟢 | Tool Scheduler | 并发/串行/审批暂停 | [详情](plan/P3-4-tool-scheduler.md) |
| P3-5 | 🟢 | 消息队列 | 排队/replace queued | [详情](plan/P3-5-message-queue.md) |
| P3-6 | 🟢 | 预算控制 | 多维预算+事件不静默停 | [详情](plan/P3-6-budget-control.md) |
| P3-7 | 🟢 | 重试 | 断流重试/retry last call/run | [详情](plan/P3-7-retry.md) |
| P3-8 | 🟢 | 取消 | 取消 provider/tool+进程清理 | [详情](plan/P3-8-cancel.md) |
| P3-9 | 🟢 | 事件流式分发 | 广播+背压+<2ms | [详情](plan/P3-9-event-broadcast.md) |
| P3-10 | 🟢 | Interrupted Run 恢复 | 崩溃后 <1s 恢复 | [详情](plan/P3-10-interrupted-run-recovery.md) |
| P3-11 | 🟢 | Phase 3 评审修复 | 主干接线（V1~V9）+ 预算/重放（V4~V6）+ 文档漂移 | [详情](plan/P3-11-review-remediation.md) |

### Phase 4：核心工具与权限

具备最小可用 Coding Agent 能力（读写编辑搜索命令 + 权限审批 + 可回滚）。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P4-1 | 🟢 | read_file | offset/limit/编码/二进制/路径安全 | [详情](plan/P4-1-read-file.md) |
| P4-2 | 🟢 | write_file | 原子写/审批/checkpoint | [详情](plan/P4-2-write-file.md) |
| P4-3 | 🟢 | edit_file | 精确替换/unified patch/模糊匹配 | [详情](plan/P4-3-edit-file.md) |
| P4-4 | 🟢 | apply_patch | 多文件/dry run/原子/回滚 | [详情](plan/P4-4-apply-patch.md) |
| P4-5 | 🟢 | run_command | 流式/cwd/env/timeout/cancel | [详情](plan/P4-5-run-command.md) |
| P4-6 | 🟢 | search_text | 正则/ignore/上下文行 | [详情](plan/P4-6-search-text.md) |
| P4-7 | 🟢 | find_files | glob/ignore/排序 | [详情](plan/P4-7-find-files.md) |
| P4-8 | 🟢 | list_directory | 类型/symlink/分页 | [详情](plan/P4-8-list-directory.md) |
| P4-9 | 🟢 | Policy Engine | 审批/路径安全/Shell 风险 | [详情](plan/P4-9-policy-engine.md) |
| P4-10 | 🟢 | Workspace Trust | 默认受限/信任放宽 | [详情](plan/P4-10-workspace-trust.md) |
| P4-11 | 🟢 | Checkpoint 与回滚 | 单次/整 run 回滚+冲突检测 | [详情](plan/P4-11-checkpoint-rollback.md) |
| P4-12 | 🟢 | Process Runtime | 进程组/Job/无死锁 IO/cancel | [详情](plan/P4-12-process-runtime.md) |
| P4-13 | 🟢 | Phase 4 评审修复 | 策略接线（V1/V4）+ 数据完整性（V3）+ checkpoint 持久化（V9）+ 基线/fuzz | [详情](plan/P4-13-review-remediation.md) |

### Phase 5：Session、Branch 与 Compaction

完善会话树、分支、压缩与 Pi 导入。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P5-1 | 🟢 | Session Tree / Fork | 从任意事件分叉 | [详情](plan/P5-1-session-fork.md) |
| P5-2 | 🟢 | Branch 切换 | 切换+并发写保护 | [详情](plan/P5-2-branch-switch.md) |
| P5-3 | 🟢 | Resume/归档/删除/重命名 | lease+损坏检测 | [详情](plan/P5-3-session-lifecycle.md) |
| P5-4 | 🟢 | 搜索 / 标签 | session 搜索与标签 | [详情](plan/P5-4-session-search.md) |
| P5-5 | 🟢 | Compaction 引擎 | 自动/手动压缩+快照 | [详情](plan/P5-5-compaction-engine.md) |
| P5-6 | 🟢 | 压缩保留策略 | 保留约束/任务/待处理 | [详情](plan/P5-6-compaction-retention.md) |
| P5-7 | 🟢 | Tool Result 裁剪 | 分级裁剪+artifact 引用 | [详情](plan/P5-7-toolresult-trim.md) |
| P5-8 | 🟢 | Export / Import | 稳定 schema 往返 | [详情](plan/P5-8-session-export-import.md) |
| P5-9 | 🟢 | Pi JSONL Importer | 解析/未知字段/不改原文件 | [详情](plan/P5-9-pi-jsonl-import.md) |
| P5-10 | 🟢 | Phase 5 评审修复 | 多分支正确性（V1/V2/V8）+ Pi 导入（V3~V5）+ CJK token（V6） | [详情](plan/P5-10-review-remediation.md) |

### Phase 6：主要 Provider

覆盖初始主要 Provider（OpenAI / Anthropic / xAI Grok / 智谱 GLM / 阿里 Qwen / Moonshot Kimi）与高级能力，Agent Core 不含 Provider 特例。Google Gemini 已实现但降级为次要（P1）供应商，见「遗留待决项」。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P6-1 | 🟢 | OpenAI 适配 | 适配+contract tests | [详情](plan/P6-1-openai.md) |
| P6-2 | 🟢 | Anthropic 适配 | 适配+contract tests | [详情](plan/P6-2-anthropic.md) |
| P6-3 | 🟢 | Google Gemini 适配 | 适配+contract tests（已降级次要 P1） | [详情](plan/P6-3-gemini.md) |
| P6-4 | 🟢 | OAuth | PKCE/Device/refresh/callback | [详情](plan/P6-4-oauth.md) |
| P6-5 | 🟢 | Thinking / Reasoning | level+stream delta | [详情](plan/P6-5-thinking.md) |
| P6-6 | 🟢 | 图片输入 | image content part | [详情](plan/P6-6-image-input.md) |
| P6-7 | 🟢 | Prompt Cache | 缓存控制+命中 | [详情](plan/P6-7-prompt-cache.md) |
| P6-8 | 🟢 | 结构化输出 | JSON/structured | [详情](plan/P6-8-structured-output.md) |
| P6-9 | 🟢 | Provider-specific options | 透传+raw metadata | [详情](plan/P6-9-provider-options.md) |
| P6-10 | 🟡 | xAI Grok 适配 | API Key 直连 + OAuth 订阅+reasoning | [详情](plan/P6-10-xai-grok.md) |
| P6-11 | 🟡 | 智谱 GLM 适配 | API Key 直连+reasoning_content | [详情](plan/P6-11-zhipu-glm.md) |
| P6-12 | 🟡 | 阿里 Qwen 适配 | DashScope API Key+thinking | [详情](plan/P6-12-qwen.md) |
| P6-13 | 🟡 | Moonshot Kimi 适配 | API Key 直连+reasoning | [详情](plan/P6-13-moonshot-kimi.md) |
| P6-14 | 🟡 | Phase 6 评审修复 | 安全/正确性（V1~V3）+ OAuth 接线（V4）+ 基线（oauth2/四依赖） | [详情](plan/P6-14-review-remediation.md) |

### Phase 7：Git、Diff 与 Worktree

结构化 Git/Diff，支持 worktree 与大规模 diff。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P7-1 | 🟢 | Repo 检测 / branch / HEAD | 系统 Git 封装 | [详情](plan/P7-1-git-repo.md) |
| P7-2 | 🟢 | status / changed files | staged/unstaged/untracked | [详情](plan/P7-2-git-status.md) |
| P7-3 | 🟢 | 结构化 Diff | DiffFile/Hunk/分页/100k 行 | [详情](plan/P7-3-structured-diff.md) |
| P7-4 | 🟢 | stage / unstage / discard | 暂存操作 | [详情](plan/P7-4-git-stage.md) |
| P7-5 | 🟢 | Worktree | 创建/删除/不删用户数据 | [详情](plan/P7-5-worktree.md) |
| P7-6 | 🟢 | Git 缓存 / watcher | status 缓存+切换<50ms | [详情](plan/P7-6-git-cache.md) |
| P7-7 | 🟢 | Hunk / Line stage（优先级 P1） | 块/行暂存 | [详情](plan/P7-7-hunk-stage.md) |
| P7-8 | 🟢 | commit / branch / ...（优先级 P1） | P1 Git 操作 | [详情](plan/P7-8-git-operations.md) |
| P7-9 | 🟡 | Phase 7 评审修复 | 安全（V1/V2）+ 语义（V3/V4）+ 基线（similar/依赖）+ 文档漂移 | [详情](plan/P7-9-review-remediation.md) |

### Phase 8：Skills、Prompts 与 Instructions

确定性上下文来源，资源加载错误不崩溃。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P8-1 | 🟡 | Resource Loader | 加载 AGENTS.md/Skills/Prompt | [详情](plan/P8-1-resource-loader.md) |
| P8-2 | 🟡 | AGENTS.md 层级 | 根+路径层级聚合 | [详情](plan/P8-2-agents-md.md) |
| P8-3 | 🟡 | Skills | manifest/激活/冲突/热重载 | [详情](plan/P8-3-skills.md) |
| P8-4 | 🟡 | Prompt Templates | 参数/默认配置/覆盖 | [详情](plan/P8-4-prompt-templates.md) |
| P8-5 | 🟡 | Profiles / Agent Profile | 运行期 instructions | [详情](plan/P8-5-profiles.md) |
| P8-6 | 🟡 | 配置优先级（确定性） | 确定性合并 | [详情](plan/P8-6-config-priority.md) |
| P8-7 | 🟡 | Resource Diagnostics | 显示生效来源 | [详情](plan/P8-7-resource-diagnostics.md) |
| P8-8 | 🟡 | Hot Reload | 变更去抖重载 | [详情](plan/P8-8-hot-reload.md) |

### Phase 9：MCP

MCP 作为第一外部扩展机制，server 故障隔离。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P9-1 | 🟡 | stdio Transport | 本地进程接入 | [详情](plan/P9-1-mcp-stdio.md) |
| P9-2 | 🟡 | Streamable HTTP Transport | 远程接入+timeout/restart | [详情](plan/P9-2-mcp-http.md) |
| P9-3 | 🟡 | Tools / Resources / Prompts | 能力发现+注册 | [详情](plan/P9-3-mcp-capabilities.md) |
| P9-4 | 🟡 | Health / restart / cancel / logging | 故障隔离 | [详情](plan/P9-4-mcp-health.md) |
| P9-5 | 🟡 | Approval / 输出限制 / Secret 注入 | 每 server 独立权限 | [详情](plan/P9-5-mcp-approval.md) |
| P9-6 | 🟡 | MCP Config | workspace/global | [详情](plan/P9-6-mcp-config.md) |
| P9-7 | ⚪ | OAuth（优先级 P1） | 保护型 server 鉴权 | [详情](plan/P9-7-mcp-oauth.md) |

### Phase 10：WASM Plugin

WASM 作为第一代码插件机制，能力受控。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P10-1 | 🟡 | Plugin Manifest + signature | 元数据+签名校验 | [详情](plan/P10-1-plugin-manifest.md) |
| P10-2 | 🟡 | WASM Host | component 宿主+加载卸载 | [详情](plan/P10-2-wasm-host.md) |
| P10-3 | 🟡 | Tool / command + hooks | 注册+生命周期 hook | [详情](plan/P10-3-plugin-registration.md) |
| P10-4 | 🟡 | Plugin state | 状态保存 | [详情](plan/P10-4-plugin-state.md) |
| P10-5 | 🟡 | Capability / fuel / 内存 / 时间 | 默认无文件/网络/进程 | [详情](plan/P10-5-plugin-capability.md) |
| P10-6 | 🟡 | API version 兼容测试 | 版本兼容套件 | [详情](plan/P10-6-plugin-apiversion.md) |

### Phase 11：Sandbox 与跨平台强化

三平台核心可用，沙箱可控，进程树可清理。架构见 [ADR-031](docs/adr/ADR-031-sandbox-backend-architecture.md)：NativeRestricted 软沙箱（P11-1，骨架已建）永远可用作兜底，平台原生硬隔离（bwrap/sandbox-exec/AppContainer，P11-2/3/4）在其上叠加并探测回退。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P11-1 | 🔵 | NativeRestricted backend | trait+软沙箱+探测回退骨架 | [详情](plan/P11-1-sandbox-native-restricted.md) |
| P11-2 | 🟡 | macOS Sandbox profile | 系统 sandbox | [详情](plan/P11-2-sandbox-macos.md) |
| P11-3 | 🟡 | Linux Bubblewrap | bwrap 隔离 | [详情](plan/P11-3-sandbox-linux.md) |
| P11-4 | 🟡 | Windows AppContainer / Job | 进程级隔离 | [详情](plan/P11-4-sandbox-windows.md) |
| P11-5 | ⚪ | Docker / Podman（优先级 P1） | 容器沙箱 | [详情](plan/P11-5-sandbox-docker.md) |
| P11-6 | 🟡 | PTY Service | 终端/重连/自动清理 | [详情](plan/P11-6-pty-service.md) |
| P11-7 | 🟡 | 进程树清理 | 三平台取消清理 | [详情](plan/P11-7-process-tree-cleanup.md) |
| P11-8 | 🟡 | 跨平台路径 | 规范化/symlink/junction | [详情](plan/P11-8-cross-platform-path.md) |

### Phase 12：Multi-Agent

Parent/Worker 编排，写入隔离，取消传播。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P12-1 | 🟡 | Supervisor / Worker | parent/worker 抽象 | [详情](plan/P12-1-supervisor-worker.md) |
| P12-2 | 🟡 | 任务分解 / 任务图 | 依赖图调度 | [详情](plan/P12-2-task-graph.md) |
| P12-3 | 🟡 | 子 session / 独立 worktree | 写入隔离 | [详情](plan/P12-3-worker-worktree.md) |
| P12-4 | 🟡 | Worker 预算 / 模型 / 并发 | 预算可控 | [详情](plan/P12-4-worker-budget.md) |
| P12-5 | 🟡 | 结果聚合 / patch merge | 合并+冲突检测 | [详情](plan/P12-5-result-merge.md) |
| P12-6 | 🟡 | 取消树 | parent 取消联动 workers | [详情](plan/P12-6-cancel-tree.md) |

### Phase 13：CLI Host 与多 GUI 协议

完成 CLI/Core 一体化装配与多 GUI 连接协议：CLI 既能独立运行，也能同时服务多个本地与远程 GUI；GUI 经 GUI Connection Protocol 连接 CLI，不嵌入 Core。不开发真实 GUI，用协议测试客户端验证全流程。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P13-1 | 🟡 | app-service 完整化 + 统一 Command Source | CLI/GUI 共享入口与来源记录 | [详情](plan/P13-1-app-service.md) |
| P13-2 | 🟡 | CLI Host 装配与运行模式 | core-runtime + cli-host + pawork + 运行模式 + Event Hub | [详情](plan/P13-2-cli-host.md) |
| P13-3 | 🟡 | GUI Connection Protocol | 协议契约（Command/Query/Event/Snapshot） | [详情](plan/P13-3-gui-protocol.md) |
| P13-4 | 🟡 | GUI Server 与 Local Transport | CLI 内部协议服务器 + Unix Socket/Named Pipe | [详情](plan/P13-4-gui-server-local-transport.md) |
| P13-5 | 🟡 | 多 GUI 运行时 | Connection Manager + Subscription Hub + Snapshot/Event Replay + 慢客户端隔离 | [详情](plan/P13-5-multi-gui-runtime.md) |
| P13-6 | 🟡 | Remote Transport 占位与可替换 Adapter | 占位接口 + Mock 端到端 | [详情](plan/P13-6-remote-transport.md) |
| P13-7 | 🟡 | TS 类型生成落地 | 自动生成一致 | [详情](plan/P13-7-ts-typegen-final.md) |
| P13-8 | 🟡 | 大型 payload Artifact API | 按 ID 传递 | [详情](plan/P13-8-artifact-api.md) |
| P13-9 | 🟡 | 测试 GUI Client 与 API Contract Tests | 协议测试端 + 契约套件 | [详情](plan/P13-9-gui-client-contract-tests.md) |
| P13-10 | 🟡 | GUI Protocol schema 版本化 | 版本 + 兼容策略 | [详情](plan/P13-10-protocol-schema-version.md) |

### Phase 14：模型用量与额度监控

显示每个绑定模型的用量与剩余额度，支持 API Key 直连 / OAuth 订阅授权 / 网页抓取三种适配器，按六个初始供应商（OpenAI / Anthropic / xAI / 智谱 / 阿里 / Moonshot）各自真实的额度机制落地，覆盖整体 / 5 小时滚动 / 周 / 月等额度窗口。依赖 P2-9、P2-7、P6-4、P3-6，建议在 Phase 13 之后推进。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P14-1 | 🟡 | Quota 领域模型与适配器 Trait | 快照/窗口/适配器种类/Trait | [详情](plan/P14-1-quota-domain-adapter.md) |
| P14-2 | 🟡 | API Key 直连适配器 | key+REST billing/usage | [详情](plan/P14-2-quota-apikey-adapter.md) |
| P14-3 | 🟡 | OAuth 登录授权适配器 | 复用 OAuth 取 console API | [详情](plan/P14-3-quota-oauth-adapter.md) |
| P14-4 | 🟡 | 网页抓取适配器 | 无 API 平台页面解析 | [详情](plan/P14-4-quota-webscrape-adapter.md) |
| P14-5 | 🟡 | 具体供应商实现 | 六初始供应商额度适配 | [详情](plan/P14-5-quota-provider-implementations.md) |
| P14-6 | 🟡 | 多窗口额度聚合与归一 | 5h/周/月/整体+倒计时+缓存 | [详情](plan/P14-6-quota-window-aggregation.md) |
| P14-7 | 🟡 | 本地用量累计与预算联动 | 对照远端+触限推算+预算 | [详情](plan/P14-7-quota-local-usage-budget.md) |
| P14-8 | 🟡 | Quota 查询 API 与展示 | core-api/CLI/GUI 脱敏 | [详情](plan/P14-8-quota-query-api-display.md) |
| P14-9 | 🟡 | 刷新调度与限额告警 | 定时刷新/退避/告警建议 | [详情](plan/P14-9-quota-refresh-alerting.md) |

### Phase 15：Provider Native Capabilities

把三家现代 API 的 hosted tools、reasoning state 与生命周期事件提升为 canonical domain。OpenAI Responses、Anthropic Modern Messages 与 xAI Responses 分别适配，不替换 Phase 2/6 的兼容路径；P15-9 是本功能簇的集中维护门禁。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P15-1 | 🟡 | Canonical Tool v2 | Client Function / Provider Hosted / Extension | [详情](plan/P15-1-canonical-tool-v2.md) |
| P15-2 | 🟡 | OpenAI Responses API | hosted tools + continuation | [详情](plan/P15-2-openai-responses.md) |
| P15-3 | 🟡 | Anthropic Modern Messages | effort/adaptive thinking/server tools | [详情](plan/P15-3-anthropic-modern-messages.md) |
| P15-4 | 🟡 | xAI Responses API | Web/X/Code/Collections/MCP | [详情](plan/P15-4-xai-responses.md) |
| P15-5 | 🟡 | Server Tool Events | citation/source/search/execution/computer events | [详情](plan/P15-5-server-tool-events.md) |
| P15-6 | 🟡 | Tool Search | 动态发现与 lazy schema loading | [详情](plan/P15-6-tool-search.md) |
| P15-7 | 🟡 | Reasoning State | effort levels/encrypted continuation | [详情](plan/P15-7-reasoning-state.md) |
| P15-8 | 🟡 | Capability Discovery | ModelCapabilities v2 + negotiation | [详情](plan/P15-8-capability-discovery.md) |
| P15-9 | 🟡 | Provider Contract v2 | 三家集中 contract/golden/兼容门禁 | [详情](plan/P15-9-provider-contract-v2.md) |

### Phase 16：Modern Agent Workflow

把 Agent 从单次前台 run 扩展为可审阅计划、持久目标、后台任务与自动化工作流；状态必须事件化、可恢复，GUI 断连不得取消任务。Long-term Memory 保持 P2，不阻塞 Plan / Background / Automation 的首轮交付。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P16-1 | 🟡 | Plan Mode | 只读规划状态与 PlanArtifact | [详情](plan/P16-1-plan-mode.md) |
| P16-2 | 🟡 | Plan Review / Revision / Approval | 评论、修订、批准与拒绝 | [详情](plan/P16-2-plan-review-approval.md) |
| P16-3 | 🟡 | Goal Mode | durable objective + success criteria | [详情](plan/P16-3-goal-mode.md) |
| P16-4 | 🟡 | Background Task Manager | process/agent/monitor/automation 统一管理 | [详情](plan/P16-4-background-task-manager.md) |
| P16-5 | 🟡 | Scheduled Automation | cron/interval/once/event trigger + inbox | [详情](plan/P16-5-scheduled-automation.md) |
| P16-6 | 🟡 | Persistent Process / Monitor | attach/detach/restart/notification | [详情](plan/P16-6-persistent-process-monitor.md) |
| P16-7 | 🟡 | Long-term Memory（优先级 P2） | canonical EmbeddingProvider + 检索注入 | [详情](plan/P16-7-long-term-memory.md) |
| P16-8 | 🟡 | Review Engine | finding/line anchor/suggested patch/resolution | [详情](plan/P16-8-review-engine.md) |
| P16-9 | 🟡 | Session Compatibility Import | Claude/Codex/Grok/Cursor 无损导入 | [详情](plan/P16-9-session-compat-import.md) |

### Phase 17：Ecosystem & Host Compatibility

补齐用户 Hook、Marketplace、LSP、Agent Profile/Teams 与公共 Host/SDK，并把浏览器、真实 Remote Transport 和远程控制作为可替换 Adapter 接入；不得绕过 Core 单一事实源或让 GUI 直连 Provider/Tool。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P17-1 | 🟡 | User Hooks | Command/Http/PromptTransform/PromptEval/AgentEval/McpTool + 扩展 trigger | [详情](plan/P17-1-user-hooks.md) |
| P17-2 | 🟡 | Plugin Package Format | Skills/Agents/Hooks/MCP/LSP/Monitors 统一包 | [详情](plan/P17-2-plugin-package-format.md) |
| P17-3 | 🟡 | Plugin Marketplace / Registry | install/update/remove/trust/policy | [详情](plan/P17-3-plugin-marketplace.md) |
| P17-4 | 🟡 | LSP Client Runtime | 启动/管理/调用现有 Language Server（Client，非 Server） | [详情](plan/P17-4-lsp-runtime.md) |
| P17-5 | 🟡 | Agent Profile v2 | prompt/model/canonical-effort/tools/skills/MCP/permission/memory | [详情](plan/P17-5-agent-profile-v2.md) |
| P17-6 | 🟡 | Agent Teams / Peer Messaging | shared task board/mailbox/presence | [详情](plan/P17-6-agent-teams.md) |
| P17-7 | 🟡 | ACP Host | 公共 Agent Client Protocol adapter | [详情](plan/P17-7-acp-host.md) |
| P17-8 | 🟡 | Rust / JSON Agent SDK | client/headless stable API；只连接 pawork Host | [详情](plan/P17-8-agent-sdk.md) |
| P17-9 | 🟡 | IDE Host Adapter | 经 SDK/Headless 连 pawork Host；可选 LSP Server 输出 | [详情](plan/P17-9-ide-host-adapter.md) |
| P17-10 | 🟡 | Browser / Computer Runtime | capability facade，服从三执行位点 | [详情](plan/P17-10-browser-computer-runtime.md) |
| P17-11 | 🟡 | Real Remote Transport | 安全远程发布/连接/重连 | [详情](plan/P17-11-real-remote-transport.md) |
| P17-12 | 🟡 | Mobile / Remote Control Protocol | 受限控制、审批与通知 | [详情](plan/P17-12-mobile-remote-control.md) |
| P17-13 | 🟡 | Cross-Agent Compatibility Loader | Claude/Codex/Grok/Cursor/Pi 配置兼容 | [详情](plan/P17-13-compatibility-loader.md) |

### Phase 18：Account Control Plane & Client Adapters

在现有 `ModelProvider`、`auth-service`、Event Store 与 `app-service` 之间补齐账号资源治理和外部 Agent Client 接入层。Provider routing、Credential Pool、Agent scheduling、Client protocol 是不同状态机；所有新持久化实体带版本与 tenant boundary，未配置的新旧单用户统一映射到 `local/default`，默认路由保持 `SingleCandidate`。架构决策见 [ADR-033](docs/adr/ADR-033-control-plane-separation.md)。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P18-1 | 🟡 | Control Plane 契约与迁移基线 | 分层、状态机、版本、feature flag、回滚 | [详情](plan/P18-1-control-plane-contract.md) |
| P18-2 | 🟡 | Tenant / Principal 身份基线 | `local/default`、身份传播、versioned migration | [详情](plan/P18-2-tenant-principal.md) |
| P18-3 | 🟡 | ProviderAccount / Credential 模型 | account 与 secret metadata 分离、legacy synthetic account | [详情](plan/P18-3-provider-account.md) |
| P18-4 | 🟡 | CredentialPool / Lease | acquire/release、并发准入、幂等回收 | [详情](plan/P18-4-credential-lease.md) |
| P18-5 | 🟡 | ErrorClassifier / Health | scope-aware 分类、cooldown、circuit breaker | [详情](plan/P18-5-error-health.md) |
| P18-6 | 🟡 | RoutingPolicy Chain | capability/tenant/health 过滤、priority/weight/fill-first | [详情](plan/P18-6-routing-policy.md) |
| P18-7 | 🟡 | Session Affinity / Binding | 粘性、rebind、revision/ownership epoch | [详情](plan/P18-7-session-affinity.md) |
| P18-8 | 🟡 | Usage / Cost Ledger | tenant/account/session/agent 多维账本 | [详情](plan/P18-8-usage-cost-ledger.md) |
| P18-9 | 🟡 | Tenant Policy / RBAC | provider/model/account ACL、并发/预算/保留策略 | [详情](plan/P18-9-tenant-policy.md) |
| P18-10 | 🟡 | ClientAdapter Framework | adapter/factory、capability snapshot、Session Registry | [详情](plan/P18-10-client-adapter-framework.md) |
| P18-11 | 🟡 | Codex App-Server Adapter | Thread/Turn/Item、approval、subagent、interrupt | [详情](plan/P18-11-codex-app-server.md) |
| P18-12 | 🟡 | Claude Gateway Adapter | session/agent headers、Messages stream、usage attribution | [详情](plan/P18-12-claude-gateway.md) |
| P18-13 | 🟡 | Canonical Audit / OTel | 控制面审计、脱敏导出、trace 维度 | [详情](plan/P18-13-audit-otel.md) |
| P18-14 | 🟡 | Provider Registry / Pool Reconciliation | factory 注册、主动健康、lease 回收、事务式热切换 | [详情](plan/P18-14-pool-reconciliation.md) |
| P18-15 | 🟡 | Control Plane Contract Gate | property/concurrency/golden/migration/isolation/chaos | [详情](plan/P18-15-control-plane-gate.md) |

### Phase 19：Desktop GUI

实现真正的 Tauri + React Desktop Client。GUI 是连接 `pawork` Host 的独立进程和可重建视图：只经 GUI Connection Protocol 获取 Snapshot/Event、发送 AppCommand，不链接 `core-runtime`、不访问数据库/Provider/Tool/Git；Node 只用于前端构建，不进入 Host。基础壳与主交互依赖 Phase 13，扩展页面按 Phase 8～18 的生产契约逐项解锁，架构决策见 [ADR-034](docs/adr/ADR-034-desktop-gui-client-boundary.md)。

| ID | 状态 | 任务 | 简介 | 详情 |
| --- | --- | --- | --- | --- |
| P19-1 | 🟡 | Desktop Shell 与安全边界 | Tauri/React/Vite 骨架、最小 capabilities、Host 启动/连接 | [详情](plan/P19-1-desktop-shell.md) |
| P19-2 | 🟡 | GUI Client Bridge 与状态投影 | typed bridge、Snapshot/Event reducer、补洞/重连 | [详情](plan/P19-2-client-state-projection.md) |
| P19-3 | 🟡 | Design System、Accessibility 与本地化 | tokens、组件原语、键盘/读屏、zh-CN/en-US | [详情](plan/P19-3-design-system-a11y.md) |
| P19-4 | 🟡 | Workspace / Session 导航 | 实例、工作区、Session/Branch 树、搜索与恢复 | [详情](plan/P19-4-workspace-session-navigation.md) |
| P19-5 | 🟡 | Timeline / Streaming 渲染 | message/thinking/tool/server event、虚拟滚动、Artifact | [详情](plan/P19-5-timeline-streaming.md) |
| P19-6 | 🟡 | Composer / Context 输入 | prompt、@file、附件、模型/Profile、发送/取消 | [详情](plan/P19-6-composer-context.md) |
| P19-7 | 🟡 | Approval / Policy / Workspace Trust | 风险解释、审批竞争、revision、信任状态 | [详情](plan/P19-7-approval-policy-trust.md) |
| P19-8 | 🟡 | Diff / Git / Checkpoint / Review | 大 Diff、暂存、回滚、finding 与行锚点 | [详情](plan/P19-8-diff-git-review.md) |
| P19-9 | 🟡 | Terminal / Process | PTY stream、resize、重连、backpressure | [详情](plan/P19-9-terminal-process.md) |
| P19-10 | 🟡 | Provider / Account / Auth / Quota | 模型、账号、OAuth、Lease 健康、Usage/Quota | [详情](plan/P19-10-provider-account-settings.md) |
| P19-11 | 🟡 | Resources / MCP / Plugins / Diagnostics | 生效来源、扩展管理、健康与脱敏诊断 | [详情](plan/P19-11-resources-extensions.md) |
| P19-12 | 🟡 | Plan / Goal / Background / Automation | 计划评审、目标、后台任务、定时与 Monitor | [详情](plan/P19-12-workflow-control.md) |
| P19-13 | 🟡 | Multi-Agent / Teams / Task Graph | worker、依赖图、预算、patch merge、团队消息 | [详情](plan/P19-13-multi-agent-teams.md) |
| P19-14 | 🟡 | 多窗口、远程连接与系统通知 | instance/window 路由、presence、断线与通知 | [详情](plan/P19-14-multi-window-remote.md) |
| P19-15 | 🟡 | 打包、签名与自动更新 | 三平台 bundle、code sign/notarize、签名更新 | [详情](plan/P19-15-packaging-updater.md) |
| P19-16 | 🟡 | Desktop Contract / E2E / Visual / Performance Gate | reducer contract、三平台 E2E、a11y/visual/perf/security | [详情](plan/P19-16-desktop-gate.md) |

---

**范围、MVP、分层验证与延后门禁、缓存清理和风险监控**：见 [plan/README.md](plan/README.md)。
