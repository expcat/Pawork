# Pawork 历史存档

> 本文是 Pawork 全部**已完成工作**的压缩存档：V1 世代（归档裁决与迁移词典）、V2 世代（S0–S13 交付、S12 审查与 S13 整改、扩展生态资产、headless 迁移史）、V3 世代（R0–R9 各已收口阶段、ADR-037~041 摘要、已闭环登记项、疑难问题根因志）。
>
> **使用约定**：本文只增不删、按世代追加；不承载未完成工作（当前任务见 [../ROADMAP.md](../ROADMAP.md)）。事实源优先级不变——冻结契约以 [architecture.md](architecture.md) 与源码/golden 为准，本文中的契约描述是历史快照。逐字节细节与被删除文档原文以 git 历史为准；V1 资产归档于仓库外 `../Pawork_v1/`（含 ADR-001~036）；V2 终态代码以 git tag `v2-final`（088b539）兜底。
>
> 目录：第一部分 V1/V2 世代（含 S12/S13、扩展生态、headless 迁移史）· 第二部分 V3 世代（R0–R9、ADR 摘要、已闭环登记、阶段外任务、根因志）。

## 第一部分:V1/V2 世代

> 提取日期 2026-08-25。来源:`docs/v2-summary.md`、`docs/v1-migration-reference.md`、`docs/reviews/s12/`(九报告 + 五交叉复核)、`plan/archive/`、`docs/headless-json-migration.md`。本文保留后续仍可能被引用的事实(冻结契约位置、安全拍板语义、复活条件、资产清单),压缩过程性叙述;逐字节细节与逐包映射如需考古,以 git 历史中的原始文档为准。V1 资产归档于仓库外 `../Pawork_v1/`(移出 git 管理);V1 期 ADR-001~036 归档于 `../Pawork_v1/docs/adr/`,原则继续有效。

### V1(2026-08-17 归档)

#### 1.1 项目形态(2026-08-14 全量 Review 快照)

| 维度 | 数据 |
| --- | --- |
| workspace 成员 | 86 crate + 2 app + benches;另有 `client-codex-app-server`、`client-claude-gateway` 两个磁盘存在、经 path 依赖参与构建但未登记 members 的 crate,实际 88 crate |
| 代码量 | 572 个 `.rs` 文件,236,177 行 Rust(约 23.6 万行) |
| 头部集中 | `app-service` 21.5k 行(29 个内部依赖,全仓组装枢纽)、`quota-service` 14.1k、`provider-control` 13.5k、`session-store` 9.7k |
| 尾部碎片 | 12 个 crate 不足 500 行(如 `transport-api` 165、`cli-renderer` 171、`tool-api` 253) |
| 完成度 | ROADMAP 计数 219 任务 / 188 完成,但大量为「有界完成」——library 层交付、生产宿主接线登记延期 |

#### 1.2 病灶结论(四类系统性问题)

1. **crate 过度碎片化**:88 crate 平均 2.7k 行(中位数约 1.4k),大量 crate 仅为表达依赖方向而存在;跨域改动动辄触碰 5–10 个 crate。
2. **组件齐全、主干未通电(最严重)**:全 workspace 依赖扫描确认约 4.5 万行「测试全绿但系统不可用」的零消费者库存——`builtin-tools`(3,655 行,八个内置文件工具,**正式二进制 Agent Loop 从未接入**)、`pty-service`(1,289)、`compaction-engine`(1,301)、`file-index`(930)、`context-engine`(1,469,主循环未接)、Phase 16 五件套 goal/automation/monitor/memory/review(6,904)、Phase 17 扩展生态批量(约 28,000,14 crate 仅 5 个有生产接线)。根本失衡:横向铺功能面优先于纵向打通可用主干。
3. **文档注册表漂移**:手工 crate 注册表(`workspace-layout.md`)与源码实态多处不符,含从未创建的幽灵登记项(`agent-api`/`provider-bedrock`/`provider-mistral`),已不可信。
4. **验证流程过重**:L0–L3 分级验证、每 Phase review+remediation 循环、4 个专用门禁脚本;流程消耗大量时间,却验证「库正确」而非「系统可用」,没能阻止问题二。

其余结构性发现:重复实现(`provider-openai`/`provider-xai` 各一份约 1.3k 行 Responses 流组装器;Ed25519+blake3 验签三处重复;SQLite schema 散布 5+ 处)、类型泄漏(`McpPeer` 直接用 `rmcp::model::*` 签名;`session-store` 反向依赖 `client-adapter-api`)、`http-runtime` 规划已久未抽离。

#### 1.3 可复用资产盘点(四档)

V1 代码质量整体过硬(每 Phase 经评审修复、安全红线有回归),V2 是**重组而非重写**:①高外部价值(发布主打):进程执行链 process/sandbox/pty、SSE/JSONL/partial-JSON 解析器、async LSP client、SQLite Actor、层级配置合并、接入 SDK、多厂商 Provider 适配层;②平台核心(随平台走):agent-engine、app-service、session-store、git/diff、policy、tool 链、GUI 协议栈;③冻结候审(见 §1.6);④不迁移:`transport-remote-placeholder`、no-op benches、注册表幽灵项、各 Phase 门禁脚本、TEMP 临时代码。

Review 方法:主代理完成依赖图扫描、逐 crate 行数统计、零消费者验证与生产装配链核查;7 个并行子代理按功能域完成 88 crate 逐个审查,主代理交叉验证成文(原 `ROADMAP_V2.md`,后为 `docs/v1-migration-reference.md`,冻结参考)。

#### 1.4 V1→V2 迁移方法

- **方式**:「复制 + 合并 + 改名」按需搬运,不整体平移;88 crate 重组为约 40 包 + 2 应用(后 Desktop 入列成 3 应用),统一 `pawork-` 前缀。
- **三道保险**:终局包布局先行;冻结契约先行(golden 先于消费实现,「只动代码组织,不动 wire/存储格式」);迁移词典 +「**无消费者不合入**」(合入即接到 `pawork` 装配链真实调用点,否则 `experimental` feature 并显式登记)。
- 配套新规:注册表自动化(依赖图以 `cargo metadata` 派生,不再手工维护);依赖方向执法从 crate 边界放宽为「包内模块 + feature 门」。开发期明确放宽:无 L0–L3、无每簇 review 循环、允许 feature 残缺合入(须编译通过且不在默认路径)。
- 保留架构红线:CLI 与 Core 同进程同二进制、纯 Rust;canonical domain 纯净;事件可持久化可重放;Secret 不落库不入日志;Engine 无 Provider 名称特例;禁循环依赖。

#### 1.5 V1→V2 映射(域级压缩;逐包细节见 git 历史中 v1-migration-reference §4.1)

| 域 | V2 包 ← 合并自(V1) | 关键动作 |
| --- | --- | --- |
| foundation(7 包) | domain←agent-domain+agent-events;api←provider-api+tool-api+plugin-api;protocol←core-api+gui-protocol+client-adapter-api+client-auth+headless-json+schema-typegen;sqlite←app-database(纯化);config←config-service;diagnostics;testkit←test-support | events 并入 domain 保留 `schema_version`/serde 形状;协议六合一为单一 schema source,core-api 单文件 2k 行拆六模块;app-database 三套业务 schema 移交 owner 包 |
| net(1) | net←provider-runtime 的 http/retry/sse/jsonl/partial_json | 补齐规划已久的 http-runtime;`parsers`(默认零重依赖)/`http` 两 feature |
| providers(3) | provider-core←provider-runtime 剩余+model-registry;providers←八厂商 adapter 合一;auth←auth-service | protector 实现移交宿主组装层;两份 Responses 组装器下沉共享模块;每厂商一 feature;Keychain+OAuth(PKCE/Device/refresh)通用化 |
| storage(2) | blob-store←artifact-store+protected-blob-store+checkpoint-service;session←session-store+compaction-engine | ADR-032 blob 语义不破;compaction 以 `TokenEstimator` trait 注入;四来源导入解析器收为 `import::formats`;对 client-adapter-api 的反向依赖后由 S13-F15/ADR-037 以「记录类型 + `SessionRegistryStore` 归 domain」落地 |
| workspace(2) | workspace←workspace-service+file-index;resources←resource-loader | file-index 首次接线;loader 基础设施与 profiles+skills 格式契约分层 |
| execution(3) | exec←process-runtime+sandbox-runtime+pty-service;policy←policy-engine;tools←tool-runtime+builtin-tools | exec 为对外发布主打(进程树+三平台沙箱+PTY 重连),按 `os/{linux,macos,windows}` 重排;policy 为独立安全内核;**tools 接入正式装配链是 V1 最大缺口的修复**;tool_search 冻结候审 |
| vcs(1) | git←git-service+diff-service | roots 参数化解开 workspace 依赖;async git+worktree+结构化 diff |
| engine(1) | engine←agent-engine+context-engine | context-engine 正式接入主循环(V1 未接);`provider_loop`(3,539 行单文件)拆 turn 组装/工具派发/流事件/审批暂停恢复四子模块 |
| extensions(5) | mcp←mcp-client;wasm-host←wasm-plugin-host+hook-runtime;plugin←plugin-package+marketplace;hooks←user-hooks;lsp←lsp-runtime | `McpPeer` 泄漏的 rmcp 类型 canonical 化,rmcp 收口内部模块;wasm-host/plugin/hooks/lsp 整族最终未激活(见第 4 章) |
| workflow(3) | workflow←plan+goal+task-manager+automation+monitor 五合一;memory←memory-service;review←review-engine | 各域独立模块与 reducer;`process-exec` feature 默认关;memory 等真实 EmbeddingProvider;review 保留 re-anchor+resolution 生命周期 |
| agents(1) | orchestration←orchestration+teams | `supervisor`(3,440 行)拆 spawn/registry/cancel-tree/recovery/budget-gate;budget 依赖 trait 化注入 |
| control-plane(3) | control-plane←tenant-service+usage-ledger+audit-log;provider-control;quota←quota-service 核心 | `dedup_key` 索引与 JSONL 审计格式保持;`account-control` feature 边界保留;quota 只迁 domain/service/ledger+LocalLedger,约 8k 行远端适配器冻结候审 |
| host(5) | app←app-service+core-runtime+subscription-hub;transport←transport-api+local+memory+remote;gui-server←gui-server+connection-manager+snapshot-service;channels←acp-host+remote-control-adapter+client-codex-app-server+client-claude-gateway;cli←cli-host+cli-command+cli-renderer | 应用门面+生命周期+Event Hub 合一;`transport-remote-placeholder` 删除、Remote trait 上移;Resume/Replay 与慢客户端隔离语义原样迁;两个未登记 members 的 crate 借 channels 四合一转正 |
| clients(3) | client←gui-client;sdk←agent-sdk+ide-host-adapter;compat←compat-loader | client 是外部 GUI 唯一接入 SDK;sdk 连接 `headless --json-stdio`(`ide` feature);compat 五来源只读导入、薄类型依赖摆脱 rmcp 拖带 |
| apps(2) | pawork←apps/pawork;protocol-probe←protocol-test-gui | composition root 清理 TEMP-P17-7-VERIFY、补 builtin 工具注册;协议契约自检工具不发布 |

#### 1.6 冻结候审清单(留在 V1 归档目录,不迁移,按需激活)

| 资产 | 行数 | 激活条件 |
| --- | --- | --- |
| quota 六厂商远端适配器 + WebScrape + refresh scheduler | 约 8k | 远端额度监控有真实用户需求且账号归属落地 |
| `browser-computer-runtime`(driver 全 Stub) | 3.5k | 真实 Local/Playwright driver 落地 |
| `tool-runtime::tool_search`(feature 门控) | 1.2k | 工具目录规模达到需要动态发现的量级 |

#### 1.7 发布波次历史候选(从未执行)

W1 exec/net/sqlite/config/domain(零内部前置,先占名)→ W2 api/protocol/policy/diagnostics → W3 provider-core/providers/auth/git/lsp/blob-store/transport(「用 Pawork 的件搭自己的 Agent」最小材料包)→ W4 client/sdk/mcp/wasm-host/hooks/testkit;License 与 crates.io 占名是任何发布的硬前置,至今未决。

### V2(S0–S13,2026-08-14 ~ 2026-08-18)

#### 2.1 方法论与结果

- **增量式重建**:S0 起 `pawork` 二进制始终可编译、可运行、可被真实使用,每阶段以「新增用户可见能力」定义;三道保险同 §1.4。
- **结果**:S0–S13 全部收口(唯一非 🟢 主干项:S6 ChatGPT/xAI OAuth 自然临期真实 refresh 人工验收挂账)。规模:v2-summary 记「38 crate ≈ 19 万行」(CR-01 于 2026-08-17 实测活跃 workspace 成员 39 个、含 3 应用,两处计数口径未对齐)。交付:CLI 完整闭环(对话/工具/审批/沙箱/用量/多通道)+ GPUI Desktop v3 三栏工作台 + 服务化(多客户端/SDK/ACP/PTY)+ 工作流与控制面。
- **验证方式**:真实冒烟(低消耗模型矩阵)+ 定向自动化(契约 golden、安全红线回归、解析器种子);开发期无 clippy/fmt/Workspace Full Gate;**未发布**。

#### 2.2 阶段交付总览(全部已真实冒烟验收)

| 阶段 | 主题 | 关键交付 |
| --- | --- | --- |
| S0 | 最小可对话 CLI | `pawork chat` 流式多轮、Ctrl-C 取消、`pawork models`、TOML 配置 + env key;401/429/超时可读呈现 |
| S1 | 会话持久化 | SQLite 落盘、`sessions list/show`、`--resume`、`--json` 事件流;envelope golden 与 append-only 契约生效;`kill -9` 后可恢复 |
| S2 | Agent Loop 与只读工具 | read/list/search/find 四工具自主循环;OpenAI/Anthropic 双协议 tool-calling;MockProvider 测试基座 |
| S3 | 写入工具与审批 | write/edit/apply_patch + `--approval-mode` 终端审批;路径越界/symlink 拒绝;deny 后会话可续 |
| S4 | 命令执行与沙箱 | run_command(进程树清理 + Seatbelt/Landlock 沙箱 + 输出截断);「读-改-跑」编码闭环;fail-closed(ADR-031 可观测回退) |
| S5 | 上下文预算与用量 | 软限压缩/硬限截断、token 与费用统计、`/compact`、`models` 目录;token 计量与厂商侧对账 1:1 |
| S6 | 首发 Provider 与认证 | 六通道适配(DeepSeek/GLM/OpenCode Go/ChatGPT/xAI/Qwen)、auth 文件后端 + env 降级、OAuth(singleflight + 跨进程锁)、全局脱敏 layer,trace 0 泄漏 |
| S7 | 最小 Agent GUI | v3 三栏工作台(TaskRail/流式对话/内嵌审批/取消/ContextMeter/RunStatusBar);`gui serve`(UDS 单实例);GPUI 锁 `=0.2.2`(ADR-035);关窗不杀 Run |
| S8 | Git、Diff 与 Checkpoint | 会话 diff、编辑前快照、`pawork rollback`、审批 hunk 预览;blob store(`PWB1` + protected AEAD,ADR-032);git 注入防护 |
| S9 | MCP、资源与兼容导入 | rmcp `=2.2.0` 锁入内部 codec(V3 R2 已升 `=3.1.3`);MCP 与内置共用 ToolRegistry;AGENTS.md/Skills 注入;`@file`;config 六层 + Profile;五来源只读导入 |
| S10 | 服务化与客户端 | protocol 收口(typegen 检入 `schemas/`)、EventHub(ring/replay/Lagged)、多客户端 gui-server、`pawork-sdk`、ACP(Zed 1.15 实测)、PTY、`service install`、headless `--json-stdio`、`sessions fork`、protocol-probe 9 场景 |
| S11 | 工作流、多 Agent 与控制面 | Plan 整版审批 gate、`pawork tasks/usage/agents demo`;control-plane(UsageLedger/audit JSONL)、quota(LocalLedger)、provider-control(lease/binding/pool)、orchestration(Supervisor) |
| S12 | 全项目 Code Review | 只读审查 CR-01~CR-09,60 finding(全 Confirmed:H15/M27/L18)→ 57 项任务;不改代码 |
| S13 | S12 finding 整改 | 57 项三波收口(波 A 安全 → 波 B Bug → 波 C 文档);契约变更 ADR-037;安全红线回归全绿 |

#### 2.3 真实测试模型矩阵(V2 约定,V3 沿用输入)

| 通道(provider_id) | 默认测试模型 | 凭证形态 |
| --- | --- | --- |
| DeepSeek(`deepseek`) | `deepseek-v4-flash` | API key |
| GLM Coding Plan(`glm-coding`) | `glm-4.7` | API key |
| OpenCode Go(`opencode-go`) | `deepseek-v4-flash` | API key |
| xAI Grok 订阅(`xai`) | `grok-4.3` | OAuth bearer |

规则:常规冒烟/回归只用矩阵内组合;高级模型(`deepseek-v4-pro`、`glm-5.x`、`grok-4.6`、ChatGPT/Codex 系列)仅限一次性接通验证或用户指定专项。凭证在 `~/.pawork/auth.json`(env 降级 fallback),缺失即 fail-closed 不静默 mock;key/token 不入日志、事件、配置样例与可提交文件。

#### 2.4 冻结契约与 ADR(V2 定形,当前事实源在 [architecture.md](architecture.md) §3.2 与 [spec/contracts.md](spec/contracts.md))

V2 收口时定形的契约族:事件信封 envelope v1(append-only)· 会话 SQLite DDL 与 import/export v3 · blob `PWB1` + protected AEAD · GUI 协议帧(ADR-036,S13 后支持 1.0/1.1/1.2)· headless `HeadlessResponse` · config 六层合并 + 凭证解析链 · usage `dedup_key` 与 audit JSONL · `PROTOCOL_CRATE_COMPATIBILITY`。typegen 检入 `schemas/`(core-api/gui-protocol/headless-json)。

本仓库 ADR(编号续接 V1):ADR-031 沙箱不可用时可观测回退 · ADR-032 blob 格式 · ADR-035 gpui 锁 `=0.2.2` · ADR-036 GUI 协议版本协商 · ADR-037 S13 波 B 五项契约(见 §3.4)。

#### 2.5 历史里程碑

| 日期 | 事件 |
| --- | --- |
| 2026-08-14 | V1 全量 Review 定稿;按域计划 M0–M8 登记后即被增量式取代;多账户调研 D1–D8 确认;五文档体系成型 |
| 2026-08-14~15 | S0–S5 收口(对话→持久化→工具→审批→沙箱→上下文) |
| 2026-08-15~16 | S6 六通道与认证(OAuth 收口本地实现) |
| 2026-08-16~17 | GUI v3 视觉基准定稿;S7 四波收口 |
| 2026-08-17 | V1 归档至 `../Pawork_v1` + V2 摊平为仓库根(1280 D/267 R);AGENTS/README 重建、84 断链修复;S8/S9/S10 收口(Zed ACP 实测);S11 波 A–C |
| 2026-08-18 | S11 收口;S12 九包审查(60 finding);S13 三波整改收口(57 项);参照项目补 Codex Router |

#### 2.6 旧「按域迁移」M0–M8 计划为何被废弃

- **教训**:按域整体迁移要到原 M4(第 5 个里程碑)才产出第一个可运行物,M0–M3 全部「库先行、无真实消费者」,无法逐步做真实测试与评估——正是 V1「组件齐全、主干未通电」病灶在计划层面的重演。2026-08-14 登记后即被增量式 S0–S13 取代;2026-08-16 又把原 S10 扩展生态移出排期(见第 4 章),S7 改为最小 Agent GUI。
- **正文从未落仓**:当前仓库与可见 git 历史中均无 M0–M8 正文,`plan/archive/README.md` 只存编号索引。回退规则:M0–M8 只作历史编号,不得把概括扩写成不存在的细则;包级迁移细则事实源是 v1-migration-reference §4.1(域级压缩见本文 §1.5)。
- 历史编号 → 实际承接阶段(压缩):M0 骨架基座→S0/S1/S6/S7/S10;M1 执行与安全→S2–S4/S8;M2 providers→S0/S2/S5/S6;M3 存储会话→S1/S5/S8/S9;M4 引擎闭环→S2–S5/S10;M5 连接与客户端→S7/S10;M6 扩展→S9(mcp/resources/compat),其余见第 4 章;M7 工作流控制面→S11;M8 Release Hardening→无活动映射,历史门禁/发布清单思路:workspace 全量 build/test/clippy/fmt、三平台矩阵、fuzz 扩展、schema/typegen CI、依赖卫生、license inventory、crates.io dry-run——未来发布任务须另立并重新裁剪。

#### 2.7 已删除文档与考古说明

`v2_plan.md`、V2 版 `ROADMAP.md` 与阶段任务书 `plan/S0–S13` 已随 V3 规划删除;各阶段逐波实现记录(写入集、测试数、冒烟口令)原载 `v2_plan.md` §3 指针表,历史价值有限未随迁。S12 报告内引用的 `plan/S*.md`、`v2_plan.md` 行号均指审查时点(2026-08-18)版本。考古一律以 git 历史为准。

#### 2.8 V2 收口遗留债务(已转入 V3 规划输入,状态以 ROADMAP 为准)

待执行任务(原 K-01~K-10):

| 编号 | 内容 |
| --- | --- |
| K-01 | 仓库根迁移后 `foundation/config` 路径闭环核对 |
| K-02 | `ToolApprovalRequested` 等待前持久化(崩溃后 seal/resume/不重复执行)——✅ 已由 R4 波 B 落地(2026-08-21) |
| K-03 | S7 Desktop 人工验收:中文 IME、多行粘贴、1440×1024 对照定稿图、键盘走查(F14/F34/F35/F36/F53–F56 证据) |
| K-04 | S8 Desktop Changes 面(Inspector Files/Summary + ActivityPopover;并入 `HunkStageService` 消费,S12-F57) |
| K-05 | S9 本机会话格式导入(`~/.claude/projects/**/*.jsonl` 与 Codex rollout,待脱敏样本) |
| K-06 | S9 Desktop `@`/Resources 面 |
| K-07 | `rate_limit.rs` 有实现无生产调用:接入或删除 |
| K-08 | `ArtifactStreaming` 能力宣告与实际 unsupported 不一致:接线或停止宣告 |
| K-09 | macOS sandbox `network_allow_hosts` 全拒未实现:egress broker 或收窄配置 |
| K-10 | Anthropic Messages 能力收口(prompt cache/thinking/hosted tools 等逐项定夺) |

另:S6 ChatGPT/xAI OAuth 自然临期真实 refresh 人工验收挂账(唯一非 🟢 主干项);F03 Windows Service 本机无法验收(降级登记);F10 两 GUI 冒烟未复跑;「多账户功能族并入 plan」(F1–F5+G6,决策 D1–D8 已确认)未执行。

休眠能力与激活条件(已迁入但无生产消费者):

| 能力 | 状态 | 激活条件 |
| --- | --- | --- |
| `pawork-diagnostics` metrics/bundle | `experimental` feature 门控 | 真实诊断导出/指标消费方 |
| control-plane OTel audit exporter | 类型已迁,无 collector | 真实审计导出消费者 |
| provider-control account/routing/health/factory | feature `account-control-v1`;demo 只走 lease | 真实多账户 factory 装配 |
| `pawork-workflow` `process-exec` | feature 默认关 | 后台任务需要真实进程 |
| workflow goal/automation/monitor 三域 | V2 收口时状态机+测试已迁、无宿主消费面(S12-F40);V3 R0 已随归档裁出,git tag `v2-final` 兜底 | 对应产品面立项(复活条件登记 ROADMAP 候选池) |
| `pawork-memory` | Mock 召回,无真实 EmbeddingProvider | 真实 embedder + 宿主 `memory_available` |
| `pawork-review` Forge | Generic 占位,无 GitHub/GitLab 实现 | 会话内评审接线 |
| orchestration teams / 真实双子 run_session | demo 级 | teams 面或真实并行子 Agent |
| tool result 分级裁剪 | `ToolResultContent.artifacts` 已扩,engine 未接线(S12-F49) | 超大 tool output 需分级 |
| S10 本机 GPUI 多窗口 | 单 `open_window`(S12-F37) | 产品定义每窗策略 |
| 对外账户池网关(F6-B) | 不内建(F6-A 已确认) | `pawork-channels` 扩展 feature 长期评估 |

未决决策:License 与 crates.io 占名(任何发布的硬前置);冻结候审资产砍留(§1.6 三项);扩展生态整族(第 4 章);全量门禁、三平台验证与发布(须用户明确决定后另立任务)。

### S12 全项目审查与 S13 整改(2026-08-18)

#### 3.1 形式与总量

S12 为只读审查:禁止运行构建/测试/二进制/GUI,全部结论基于源码与 git 静态证据;产出九份包域报告(CR-01~CR-09,主审 GLM 与 Grok 混编)+ 五份交叉复核,存档于 `docs/reviews/s12/`(报告为审查时点冻结证据,不随后续重构回写)。8 份主报告 55 finding + CR-09 自身 5 条 = 60,全部 Confirmed、零 Needs Verification;裁定后 High 15 / Medium 27 / Low 18。

#### 3.2 九报告主题与关键结论

- **CR-01 manifest 与包布局**(GLM;M1/L4):39 活跃成员的布局、命名与红线全部核对通过(domain/api 纯净、Desktop 不链 Core、无循环依赖、engine 不依赖 workflow)。唯一 Medium:session→protocol 的 client-adapter 反向依赖未按迁移词典 trait 倒置(后由 S13-F15/ADR-037 归 domain 解决);其余为 api `plugin` feature 缺位、protocol 无条件把 ts-rs 拉进生产图、workspace 依赖集中化被绕过、词典措辞漂移。
- **CR-02 写入/审批/git 边界**(Grok;H2/M2/L1):写路径安全内核健全,但只读工具走 S2 词法门,workspace 内 symlink 可读出 root(可读宿主凭据);S3「单一 policy 路径内核」为假完成,`.git` 只读面敞开。另:`OnFailure` 审批档与 `NeverAsk` 同实现、未信任 workspace 无条件注入 AGENTS.md/Skills 正文、`list_directory` 回传 symlink 宿主绝对目标。
- **CR-03 进程/沙箱/CLI 服务**(Grok;H4/M2/L1):macOS Seatbelt 为规避平台缺陷整盘只读放开却仍标 `Hard`,secret deny 不含 `~/.pawork/auth.json`;GUI PTY 绕过沙箱/审批且宿主退出不回收(裁定降 Medium:无模型→TerminalWrite 路径);`service stop --apply` 不删 plist/unit 且 `--instance` 可注入标识;Dangerous 分类漏 PowerShell/cmd/`curl|sh`;沙箱回退与 S4 任务书字面冲突(拍板维持 ADR-031);macOS `setsid` 后代不回收;遗留 `JsonlSink` 打非协议行。
- **CR-04 Secret/网络/MCP**(Grok;H4/M2):恶意 workspace 可用 MCP SecretRef 指向主 Provider auth 域窃取全局 token;workspace 可设 `proxy_url`/`base_url` 且 redirect 默认跟随可截凭证(含 `x-api-key`);HTTP 错误 body 512 字节明文进入可持久化事件流;workspace MCP 可自封 `trusted`+`auto_start`(复核补充:stdio 子进程完整继承宿主环境、不经沙箱硬隔离);Anthropic 认证头无覆盖闸门;Anthropic 能力静默丢弃(归 K-10)。
- **CR-05 持久化/事件/账本**(GLM;H2/M1/L2):`messages` 投影无 branch 维度,fork 后 resume/compaction 混入或误删跨分支消息(维持 High);失败/取消 run 已发生用量不入 usage ledger(降 Medium:计量低估而非控制面绕过);重复写前快照 blob 引用计数泄漏;无定价静默标 USD;崩溃窗口 final blob 孤儿不被 gc。append-only 双触发器、PWB1 AAD、dedup 语义等核对通过。
- **CR-06 Engine/Workflow/编排**(GLM;M7/L3):无 High,7 条 Medium 集中在契约完整性——tool artifacts 在 engine 数据面被丢弃、Plan `Revised` 事件无法携带修订内容、Automation `record_result` 不幂等(三者进 ADR-037);审批 gate 数组短缺时 fail-open;Memory replay 后 ID 从 0 重发覆盖历史;Supervisor spawn 不校验 parent、`recover()` 只诊断不重建。另有 tool result 分级裁剪零消费、review anchor symlink 逃逸、Provider 禁用名单过期。
- **CR-07 协议/宿主/客户端**(Grok;H3/M2/L1):生产 `gui serve` 握手无认证,同用户任意进程可驱动 Run/PTY/审批;连接队列/broadcast Lagged 只丢事件不通知客户端;服务端 SnapshotRequired 附带 Snapshot 而客户端按「只回 disposition」提前结束,契约分裂;IdempotencyStore check/record 非原子(降 Medium:现实触发面窄);headless 未映射命令默认放行;探针 auth scheme 与生产常量不一致。
- **CR-08 Desktop GUI**(GLM;H3/M4/L4):Timeline 锚点存数组 index,分页与直播交错时改错/复制条目;`RunStart` 无 provider 字段,同名模型(两通道各有 `deepseek-v4-flash`)会切错通道/凭证;主路径不可全键盘操作、无 tooltip/accessible name。Medium/Low:无条件抢滚、Composer 单行不增长、All projects 新建静默绑定首 workspace、S10「本机多窗口」假完成、事件保真度缺口、运行时长不实时、render 全量克隆、视觉基准未登记漂移。
- **CR-09 追踪/死代码/文档一致性**(GLM;M3/L2):S0–S11 主承诺逐项核到生产调用点,除已登记延期外无其他假完成/零消费者主路径。发现:README/AGENTS 状态与结构清单滞后;S10「stop --apply 删 plist」验收记录与源码从未相符(`git log -S` 证实从未实现);workflow goal/automation/monitor 三域零生产消费;`HunkStageService` 零消费;工作区路径校验四处分叉(policy/workspace/resources/review 各一套)。并完成跨报告收口。

各报告 finding 数(严重度为主报告原判):CR-01 5(M1/L4)· CR-02 5(H2/M2/L1)· CR-03 7(H4/M2/L1)· CR-04 6(H4/M2)· CR-05 5(H2/M1/L2)· CR-06 10(M7/L3)· CR-07 6(H3/M2/L1)· CR-08 11(H3/M4/L4)· CR-09 5(M3/L2)。

#### 3.3 交叉复核与裁定

五份复核文件(CR-02/03/04/07 各一份 GLM + CR-05-08 一份 Grok;派发口径曾误计 4 份,收口按 5 计)。18 条 High 全数独立复核,7 项裁定差异:

| 编号 | 裁定 | 理由(压缩) |
| --- | --- | --- |
| S12-CR03-02 | High → Medium | 全仓唯一 PTY 写入方是 Desktop 用户输入框,无模型→TerminalWrite 路径;若未来出现模型驱动写入应回升 High |
| S12-CR05-02 | High → Medium | chat 热路径未用该 ledger 做 quota/budget 硬门禁,影响是计量低估而非控制面绕过 |
| S12-CR07-03 | High → Medium | GUI 单连接串行、通道各持内存表,现实触发面仅 ACP 同 id 并发重试一条窄路径 |
| S12-CR07-01 | 维持 High,修正括注 | 「umask 022 → 他人可 connect」证伪(0755 无写权限);同用户任意进程可驱动 Run/PTY/审批已足够支撑 High |
| S12-CR08-02 | 维持 High,修正方向 | channels 顺序 opencode-go 在 deepseek 之前,用户选 deepseek 时 find() 落到 opencode-go(与原文相反);结论不变 |
| S12-CR08-03 | 维持 High,校正行号 | 审批三按钮实际 1246-1268、全局新建 1119-1131;控件与缺口仍在 |
| S12-CR04-04 | 维持 High,修正口径 | `trusted=true` 只绕过未信任 workspace 硬门,写入类 MCP 工具单次调用仍触发审批;另补两项加重事实(env 全继承、stdio 不经硬隔离) |

主报告正文保留审查原文,裁定以各报告头部注记与 CR-09 §4.3 差异表为准。

#### 3.4 S13 三波整改与关键安全拍板

60 finding 收敛为 57 项任务(F01–F57),三波:**波 A 安全(F01–F14)→ 波 B Bug(F15–F40)→ 波 C 文档(F41–F57)**;收口时安全红线回归全绿。关键拍板(安全语义,后续重构须保持):

- **F01**:读写工具均拒 `.git`(无审计开关)。
- **F02**:macOS Seatbelt 写+网模式诚实标签 `HardWritesAndNetwork`;`default_secret_paths` 扩充。
- **F05**:MCP 凭证走 SecretRef(仅 `pawork.mcp.*` 命名空间)+ 独立 `mcp-auth.json`;stdio 子进程 `env_clear` 且拒绝透传 `PAWORK_API_KEY_*`。
- **F06/F07**:workspace 级配置剥离 `proxy_url`/非回环 `base_url`;HTTP 错误只留 `HTTP {status}`;`redirect(Policy::none())`。
- **F08**:workspace 级配置剥离 MCP `trusted`/`auto_start`。
- **F11/F32**:EventHub Lagged → `ReplayUnavailable`;客户端收齐附带 Snapshot。
- **F33**:未映射 headless 命令 fail-closed。
- 路径检查统一 `policy::path` 内核(读路径 symlink 同内核);生产 `gui serve` 强制 token(UDS 0600);Timeline 锚点用 `event_id`/`sequence`。

#### 3.5 契约变更

[ADR-037](adr/ADR-037-s13-wave-b-contracts.md)(S13 波 B 五项):①session registry trait 与记录类型归 domain(F15);②沙箱回退维持 ADR-031 可观测回退(F19);③`ToolResultContent.artifacts` 扩展;④Plan `Revised` 携带 title+steps;⑤`ResultArchived` 增 `task_id`。另 S13-F09:sessions schema 升 v10 增 `ancestor_lineage`;GUI 协议 `RunStart.provider` 升 API 1.2(支持 1.0/1.1/1.2)。

### 扩展生态资产存档(S10 extensions deferred)

**决议**:2026-08-16 起 WASM 插件/市场/用户 Hooks/LSP 整族移出排期,待设计与决策;原 S10 让位,S7 改为最小 Agent GUI。四包不激活实现,不产生零消费者库存,「合入即接线」原则对该族同样成立。

**保留的预留锚**(现行代码在位,不随移出排期删除):`PluginId`(domain ids)、`ToolCapability::ExternalPlugin`(domain tool,policy 审批文案在位)、`plugin` feature 空数组占位(V2 期在 `pawork-api`,S12-CR01-02 曾指出 manifest 缺位,V2 收口确认保留;R1 合并后现位于 `pawork-domain`)、GUI 对未知 capability 的容忍。

**已实现资产清单**(V1 归档目录内,复制式激活;行数为 V1 实测约数):

| V2 包(计划) | V1 来源 | 行数 | 激活时关键动作 |
| --- | --- | --- | --- |
| `pawork-plugin` | plugin-package + marketplace(feature `market`) | 7.5k | 三处 Ed25519+blake3 验签收敛为单一 `signing` 模块(golden 向量一致);market HTTP 走 pawork-net;安装/撤销路径操作经 pawork-policy |
| `pawork-wasm-host` | wasm-plugin-host + hook-runtime | 3.9k | hook-runtime 降级为包内 `lifecycle` 模块;wasmtime(27)锁本包、默认构建路径不含;验签经 `signing` trait 注入 |
| `pawork-hooks` | user-hooks | 3.7k | 注入式执行器;与 wasm lifecycle 信任域分离(宿主进程内 vs 沙箱内),不同注册位点;hook 决策事件化入 envelope |
| `pawork-lsp` | lsp-runtime | 5.4k | async LSP client(生态稀缺,高发布价值);resource/sandbox 依赖改注入;LSP 能力注册为工具 |

**激活草案要点**(原冒烟/退出标准压缩):插件「安装(验签→写盘→注册 ToolRegistry/Hook 链)→调用→撤销(注销→精确清理无残留)」全闭环;未签名/篡改包拒装(fail-closed);hooks pre-tool 返回值短路语义、事件可重放(如「阻止对 *.lock 写入」被拦截且事件可见);LSP definition/references/diagnostics 三查询工具化并与 grep 对比评估,server 进程经 exec 沙箱启动;wasmtime/rmcp 不进默认构建路径(`cargo tree` 断言);`pawork-api` 三 feature(provider/tool/plugin)集齐;CLI 预期入口 `pawork plugin install/list/remove`、`pawork hooks list`。

**复活条件**:纳入排期须先过产品决策(V2 期登记于当时 ROADMAP §4「扩展生态整族」);V3 期间不新增包,任何包布局变更须先过 ADR;市场正式源待运营决策。插件可声明的能力面(工具之外的 hook/资源源)按 plugin-api 契约预留在位。明确不做:`tool_search` 与 browser/computer 驱动(冻结候审,见 §1.6,不随本族激活)。

原激活草案的并行拆分建议(波 A:plugin[signing 先行定 trait]+api plugin feature、lsp;波 B:wasm-host、hooks;波 C:app 注册位点接线+cli+冒烟)仅作历史参考,重启时须按当时布局重排。

### headless `--json` 迁移史

- **V1**:`run --json` 打**裸 `AppEventEnvelope`**(无 `{type:event}` 包装、无 hello),与 V2 各阶段形状均不同;老 V1 脚本不能假设「已接近正式协议」。
- **V2 S1**:`--json` 以 unstable 落地,stdout 逐行打**裸磁盘信封 `AgentEventEnvelope`**(顶层带 `schema_version`/`sequence`,即三层中的磁盘层)。
- **S10 10a 波 A(2026-08-17)**:headless 帧 + 翻译 + stdio 循环落 `pawork-protocol::headless`(V1 `headless-json` 整组迁入);`JSON_TO_HEADLESS_EVENT_MAP` 对照表建立;typegen `.d.ts` 检入 `schemas/`;GUI 帧 golden 补齐。
- **S10 收口(2026-08-17)**:兑现计划内唯一一次 `--json` breaking——`run`/`chat --prompt --json` 的 stdout 从裸 `AgentEventEnvelope`(磁盘层)升为 `HeadlessResponse` 包装的 `AppEventEnvelope`(应用层+传输层),`type=event|response|error`,去掉 unstable;序号语义从 session 内 `sequence` 改为 `global_sequence`。
- **同波新增** `pawork headless --json-stdio` 子命令:双向 stdin/stdout JSONL,握手强制(`hello`→`hello_ack`)、单帧上限 4 MiB、能力门 Sessions/Runs/Streaming/Compat*;单向 `--json` 与双向 headless 共用同一套 `HeadlessResponse` 词表(Codex `exec --json` vs `app-server` 的 Pawork 版,不是两套协议)。SDK `spawn_e2e` 不再 skip;`--json`/`watch --json` 消费 EventHub 扇出。
- `sessions`/`auth`/`models`/`diff`/`mcp`/`import` 等子命令的 `--json` 自始至终是各自快照 JSON,从未属于协议帧,不在上述 breaking 范围。
- **S13-F33**:未映射到 capability 的 headless 命令改为 fail-closed(`UnsupportedCapability`),修复 S12-CR07-05 的默认放行。
- 现行有效的对照映射(三层信封、字段与事件 tag、stdio 纪律)已提炼至 [docs/spec/contracts.md](spec/contracts.md) §6;逐条历史迁移指引(老脚本改法等)以 git 历史中的 `docs/headless-json-migration.md` 为准。

## 第二部分:V3 世代(R0–R9)

> 提取时点 2026-08-25。来源:plan/R0–R9 十份任务书、plan/out-of-band/code-map.md、ROADMAP.md、v3_plan.md、docs/adr/ADR-037~041。ADR 原文保留于 docs/adr/ 不删,本文仅作摘要索引。提取时实态:R0–R7 🟢、R8 🔵(仅余 K-03 人工签字)、R9 🔵(波 A1 🟢);未完成部分(R8 K-03 签字、R9 A2/B/C 及各人工验收项)不收入本文,继续以 ROADMAP 为事实源。

### V3 目标与主线

V3(R0–R9)不是新功能扩张,而是 V2 增量交付完成(S0–S13,见本文第一部分)后的一次**全仓结构重构**:清偿「先通电、后收敛」策略遗留的结构债。允许破坏式重设计,但磁盘/线上冻结契约的演进必须经 ADR 版本化,不允许静默破坏。立项依据为 2026-08-18 五路只读分析(两路包合并、依赖用面审计、GUI 组件分析、补丁式实现全仓扫描)。

四条目标:

1. **结构收敛**:workspace 39 成员 → **21 成员**(19 库 + 2 应用);约 3.3–3.8 万行零消费者休眠代码(近 20% src)裁决归档;确立「消费面先行」硬门——每个模块要么在产品面有真实装配点,要么移出主干。
2. **依赖治理**:本地化 3 项(rand→getrandom、parking_lot→std::sync、base64→本地 base64url)、死声明清理、版本升级九项(消除 Cargo.lock 多版本共存)、rmcp 3.x 专项评估。
3. **补丁根因重构**:12 个聚类主题 T1–T12(映射见下节)——协议三通道同源化、宿主单体拆解、幂等持久化、降级可观测、Provider 中立层渗漏封堵、凭证词汇净化、会话分支原生化、沙箱真隔离。
4. **GUI 工程化**:apps/desktop 建立 `ui/theme.rs` tokens 与 `ui/components/` 组件库(硬编码色全量 token 化、手写按钮/复制菜单组件化),菜单改 gpui `anchored()/deferred()`,并收口 V2 遗留的 Changes / `@` / Resources 面与人工验收(K-03/K-04/K-06)。

计划原则:

1. **每阶段保持可用**:任意波次收口时 `pawork` 二进制可编译、可运行、既有冒烟行为不回退;重构不断主干。
2. **冻结契约不静默破坏**:事件信封、session DDL、blob `PWB1`、GUI 帧、headless JSON、config 层级、usage `dedup_key`、audit JSONL 只经 ADR 版本化演进(R6 分支模型、R7 沙箱为已知仅有的两处);golden 先于实现改动。
3. **删除优先于门控,门控优先于库存**:零消费者代码默认归档,git tag `v2-final` 兜底可找回;不再以「experimental feature + 登记」方式库存。
4. **决策先行**:四个 ADR 闸门——R0 波 0(ADR-038 库存与产品形态)、R1 波 A(ADR-039 布局)、R6 波 0(ADR-040 分支模型)、R7 波 0(ADR-041 沙箱);破坏式改动须用户确认 Accepted 后执行,主代理不代替用户拍板。

阶段依赖:R0→R1→R2 串行主干(裁决→合并→治理);R3–R7 在 R1 后开启,推荐 R3→R4→R5→R6 串行(四者都触 host,避免写入集冲突),R2 与 R3、R7 与 R3–R6 写入集不相交可并行;R8 依赖 R3(共享投影 reducer)与 R2(gpui 传递树);R9 收口全部阶段。真实测试默认低消耗矩阵:deepseek(`deepseek-v4-flash`)/ glm-coding(`glm-4.7`)/ opencode-go(`deepseek-v4-flash`)/ xai OAuth(`grok-4.3`);凭证缺失即 fail-closed,Secret 不入日志/事件/可提交文件。

### 补丁主题映射 T1–T12

来自补丁式实现全仓扫描(2026-08-18,60 处原始补丁聚类;逐项证据在各任务书):

| 主题 | 内容 | 归属 |
| --- | --- | --- |
| T1 休眠库存大清仓 | 约 3.3–3.8 万行零消费者代码裁决 | R0 |
| T2 host/app 单体拆解 | `lib.rs` 4,057 行 + `gui_host.rs` 2,594 行巨 match → 领域服务 | R4 |
| T3 三通道协议面归一 | GUI/headless/ACP 三套 mapping/授权 → 单一 registry | R3 |
| T4 会话分支模型原生化 | 后补 `branch_id` 列 + 反查回填 → 原生 lineage | R6 |
| T5 Timeline 投影单一事实源 | host/desktop/client 三处手搓投影 → 共享 reducer | R3 |
| T6 Provider 扩展元数据契约化 | 存储层 provider 键名清单、通道三处硬编码 → 命名空间契约 + 注册表 | R5 |
| T7 沙箱与执行面真隔离 | 诚实标签 → 真隔离(profile 重设计、PTY 入闸) | R7 |
| T8 降级与吞错可观测契约 | 323 处 `let _`、HOME→temp 静默回退 → 降级事件化 | R4 |
| T9 幂等与占用原语统一 | 内存 CAS、9 张 Mutex map、序列补洞 → 持久化 ledger + actor | R4 |
| T10 控制面多租户对齐单机现实 | `local/default` 哨兵宇宙裁决 | R0(拍板)+ R1(收编) |
| T11 凭证/配置解析去重与词汇净化 | env 双实现、keychain 兼容名、mcp-auth 前缀白名单 → 单一 locator | R5 |
| T12 Desktop UI 工程化 | 单文件 UI、零组件、97 硬编码色 → theme + components | R8 |

### R0–R7 阶段存档

#### R0 — 决策收口与休眠库存裁决(🟢 2026-08-18,波 0/A/B/C)

- **目标**:ADR-038 一次性拍板产品形态(T10)与全部休眠资产去留(T1);归档/删除零消费者代码(计划口径 3.3–3.8 万行,ADR-038 后果实录约 3.3 万行);清死 feature/死声明与唯一 deprecated;K-07(rate_limit)删除、K-08(ArtifactStreaming)停止宣告。不做包合并、不升依赖、不动冻结契约与 domain 事件类型。
- **关键决策**:ADR-038 Accepted(用户 2026-08-18 确认,22 项 D1–D22 按推荐决议执行)。D1 单机优先:`local/default` 哨兵宇宙不扩张,多账户 factory 转候选;否决多租户层级形态。执行前打 git tag `v2-final`(指向 088b539)兜底,「归档 = 移出 members + 删除源目录」,复活条件登记 ROADMAP 候选池。

| 波 | 交付要点 |
| --- | --- |
| 0 | tag `v2-final` + ADR-038 起草与用户确认 |
| A(并行×3)| 大块归档 D2–D7:provider-control account-control-v1 九模块 8,476 行、binding.rs+schema/ 5,473 行(legacy.rs 删除)、workflow goal/automation/monitor 三域 3,603 行、orchestration teams 2,985 行、memory 1,134 / review 1,467 整包出树、transport remote TLS 3,721 行 + MockRemote 731 行(rcgen 退出 lock) |
| B(并行×3)| 小块删除与降级 D8–D15/D20–D22:diagnostics experimental(bundle 494+metrics 225)、rate_limit.rs 532 行(K-07)、session lifecycle.rs 697 行、net jsonl 285+partial_json 535、sdk ide.rs 占位、exec pub 函数降 pub(crate)、deprecated `recover` 别名删除;K-08 双端停止宣告(宣告点实为五处:cli/gui-client 默认/desktop/probe/契约测试;`GuiCapability` 枚举与 schemas/ 冻结面不动) |
| C(串行)| D16 git 六休眠服务归档(Branch/Stash/Conflict/History/CachedStatus+StatusCache+spawn_invalidator,补判 commit.rs,合计 2,262 行;保留 Diff/Status/GitService/GitRunner/Head/HunkId+HunkStageService+worktree/merge);收口断言与登记 |

- **数字结论**:members 39→37(memory/review 出树);`cargo tree -p pawork` 闭包 833→817 行只减不增;rcgen 退出 lock(rustls/tokio-rustls 经 reqwest 保留属预期)。
- **验证要点**:受影响包定向 check/test;安全红线(policy/tools/exec)全绿;重放 golden(信封/迁移)全绿证明 domain 事件保留;真实冒烟 deepseek-v4-flash(chat 流式 + 工具 + sessions list)。
- **偏差与改判(4 项,实态核查驱动)**:D12 `run_turn` 生产内部在用(session_turn/tool_loop/compaction 调用)→ 仅 `pub` 降 `pub(crate)`,函数保留;D15 rbac 三类型(Permission/PrincipalRole/PermissionProfile)全部保留(deny-first 热路径在用),OTel/identity_schema 照归档;D14 `encoding_rs` 为类型级隐性直接依赖(chardetng `guess()` 透出 `&'static Encoding`,删后 E0599 实证)保留;D16 commit.rs 零消费补判归档。另:两个既有测试失败(sdk handshake 夹具、workflow plan_service)裁决为既有、留 R1 波 E 窄修;usage 幂等键冲突冒烟发现登记为阶段外窄任务。

#### R1 — 包合并 37→21(🟢 2026-08-19,波 A–E)

- **目标**:workspace 37 成员收敛至 21(19 库平铺 `crates/<短名>` + `apps/{pawork,desktop}`);ADR-039 定稿布局与不合并清单;只动代码组织,wire/磁盘形状零 diff,golden 与测试随模块平移。
- **关键决策**:ADR-039 Accepted(2026-08-19)。D1 扁平布局,`git mv` 集中波 E 一次完成(波 A–D 只做内容级合并);D2 不合并清单固化:policy/exec/auth/git/engine/protocol/testkit/transport/orchestration/workflow 保持独立(policy 并入含 tools 的包即成环;exec 零内部依赖自含;auth 是 Secret 审计边界);D3 api→domain 的 GUI 侧口径(纯类型进编译闭包不违反「GUI 不加载 Core」,红线指运行时装配);D5 编译粒度代价明示并实测;D6 跨包纪律降级为模块纪律 + 定向测试;D7 golden 先行。

| 波 | 交付要点(members 变化) |
| --- | --- |
| A | api→domain(golden 先行:ProviderStreamEvent 13 变体/ProviderError/CanonicalModelRequest/ToolResult 原仅内存 roundtrip,先补字节级夹具再整组平移,零 diff)+ diagnostics 撤包(Redactor/RedactingFmtLayer 迁 `apps/pawork/src/redact.rs`);37→35 |
| B(并行×3)| storage(sqlite+session+blob,feature 分层 `default=["session","blob"]`,compaction/checkpoint/protected opt-in)∥ providers(net+core+adapters,`channels/` 内聚,core→net 降为模块纪律+源扫描)∥ workspace(core+resources+config+compat→`import/`);control-plane 对 sqlite Actor 零引用的死依赖直接移除;35→28 |
| C(并行×2)| mcp→tools `mcp/`(64 测零裁剪,rmcp 隔离断言随迁)∥ quota+provider-control→control-plane(`quota/` 100 测 + `credential/` 35 测;orchestration 保持 default-features=false 防 rusqlite 传染);28→25 |
| D(并行×3)| gui-server→app(GuiHost trait 平移,cli 改经 app)∥ channels→cli `channels/acp/` ∥ sdk→client `headless/` + probe 9 场景→client tests/、live 模式→examples/probe.rs;25→21 |
| E(串行)| 19 库 `git mv` 扁平 `crates/`;members 定稿 21;design.md §2 重写为 V3 布局;红线断言建立/随迁(desktop client-only、engine domain-only、providers core→net、rmcp 隔离、`cargo tree` 无环) |

- **数字结论**:members 37→35→28→25→21;`cargo tree -p pawork` 闭包 817→800→751→724→711 行;16 解散包名在闭包与 Cargo.lock 零残留。D5 实测:providers touch-单文件增量 check 合并前 ≈0.14–0.16s → 合并后 ≈3.7–4.7s(tools ≈10.4→11.5s,control-plane 在噪声内),代价成立但秒级,维持取舍。
- **验证要点**:波 E 全 21 包 check 绿、73 测试二进制 1644 测绿;真实冒烟 chat 流式/工具/审批 fail-closed/`gui serve`/desktop `--probe-smoke`。
- **偏差与修复**:整阶段审查修复两个 probe 暴露的**既有**生产缺陷——① ModelList(运行期探测合并)与 switch_provider(静态注册表)目录不对称致 UnknownModel:新增按 `(provider, model)` 动态探测/惰性合并,未知模型仍 fail-closed;② client `FrameWant::Event` 抢占命令错误帧(见根因志)。红线断言收紧为 allow-only(覆盖 target-specific 表与 package alias)。窄修复两项既有测试失败:hello_ack.json 夹具 negotiated 1.1→1.2、workflow 测试携现有步骤修订。

#### R2 — 依赖治理(🟢 2026-08-19~20,波 A–D + 整阶段复核)

- **目标**:依据 38 crate 逐调用点用面审计:本地化 3 项、死声明清理、九项升级去重、rmcp 3.x 专项;其余依赖(tokio/reqwest/blake3/chacha20poly1305 等)经逐调用点核查保留。
- **关键交付**:

| 波 | 交付要点 |
| --- | --- |
| A(并行×2)| L1 rand→`getrandom::fill()`(6 生产调用点:client_auth 32B token、oauth PKCE、protected nonce/盲化/抖动);L2 parking_lot→std::sync(52 处,毒锁统一 `unwrap_or_else(PoisonError::into_inner)`;orchestration 死声明清除);L3 base64→auth 本地 `base64url` 模块(先与 base64 0.22.1 逐字节对拍,绿后固化 13 组固定向量再删依赖) |
| B(串行)| 九项升级:notify 8.2(debouncer 死声明删)、windows 0.61.3(0.58 退出,msvc 交叉 check 绿)、portable-pty 0.9(官方 `signal()` 替 Display 解析 hack,nix 0.25 老栈退出)、ts-rs 12.0.1(7 个 .d.ts 索引签名去 `?` 属形状变化,**用户拍板 A 接受**:wire golden 不变、仓内无 TS 消费方)、reqwest 0.13.4(上游强制 TLS 信任栈 webpki-roots→rustls-platform-verifier,系统信任库/吊销策略生效,aws-lc-sys 引入 cmake;redirect/回环代理语义实测不变)、toml 1.1.4、rusqlite 0.40.2(SQLite 3.46→3.53.2 安全修复)、sha2 0.11(RFC 7636 golden 字节不变)、directories 6.0.0(macOS `dev.pawork.pawork` 快照 golden×2 v5/v6 逐字节一致) |
| C(串行)| rmcp `=2.2.0`→`=3.1.3` 升级决议落地:65 条 MCP 契约测试 + 隔离断言绿;codec.rs 适配 `#[non_exhaustive]` 与 `InputRequiredResult` 明确 fail-closed;**MSRV 1.85→1.88**(rmcp 3.x 为 edition 2024);精确锁定策略保留,复评条件=下个 major 或 wire 变化 |
| D(串行)| 收口断言:默认目标 `cargo tree --duplicates` 归档(plan/R2-cargo-tree-duplicates-2026-08-20.txt),notify/reqwest 单版本可复现;CLI 直控面多版本清零;闭包传递残留登记(base64 0.22/0.23、syn 2/3、thiserror 1.x 均上游传递,随上游自然消除);默认 desktop 例外 sha2/toml/thiserror,windows 0.57 仅为可选 screen-capture 的 lock 残留 |

- **数字结论**:Cargo.lock 836→830(波 B)→826(波 C),净 -10;MSRV 1.85→1.88;tools 130/130 绿。
- **验证口径拍板**:历史 xAI OAuth/MCP stdio 冒烟与编译数字当时通过,但原始输出未归档——统一降级为「历史人工记录」,不作为仓内可复现门禁(2.2.0 基线亦未归档,「逐字节一致」仅保留为历史结论)。
- **整阶段复核修复**:notify 8 `Flag::Rescan` fail-safe(后端溢出/MUST_SCAN_SUBDIRS 空路径事件转每 workspace root 的 Upsert 全量重扫,补回归);directories 关闭测试环境短路(auth 路径抽纯函数,确定性覆盖 override/fallback);rmcp InputRequiredResult 回归缺口;PTY 毒锁注释与 Windows 路径注释校正(`%APPDATA%\pawork\pawork\config`)。

#### R3 — 协议与投影同源化(T3+T5;🟢 2026-08-20,波 A–D + 整阶段审计 08-20~21)

- **目标**:GUI/headless/ACP 三套命令 mapping 与能力/授权模型收敛到单一 Command/Capability Registry(宣告=授权=实现,未登记天然 fail-closed);Timeline 投影 reducer 下沉 protocol 共享模块(host/desktop 同源);OnFailure 审批档位裁决。协议 wire 形状不变,变的是实现组织。

| 波 | 交付要点 |
| --- | --- |
| A(串行)| protocol 新模块 `app/registry.rs` 表驱动登记 19 command + 11 query(wire 名/三通道可用性/所需 capability/幂等/引入版本);GUI 宣告改 registry 派生(向量 {Events,Snapshots,TerminalStreaming,Approvals} 新 golden 钉死,K-08 编码为数据);app/gui_server 新增逐命令授权门,未登记/未授予 fail-closed(拒绝先于进入 host);gui_host 删 wire 名硬编码镜像改查 registry(巨 match 留 R4) |
| B(并行×2)| headless 删 `command_capability` 手写表改查 registry(文案逐字保留);ACP 新增 `admit_acp_command` 查 acp 列 fail-closed(acp: true 恰为 {session_create,run_start,run_cancel,tool_approve};decode 四臂属协议路由保留);HOST_CAPABILITIES 快照钉死 + headless 列 ⊆ HOST_CAPABILITIES 一致性测试 |
| C(串行)| 投影 reducer 下沉 `protocol::projection`(805 行纯模块:project_event 逐字平移 + TimelineProjection 合并核——seen 去重/partition_point 有序插入/双键 tool 锚 + resume 基线语义);desktop projection.rs 2346→1542 行只剩渲染适配(<800 目标偏差登记);三种子 golden(分页交错/Lagged→Snapshot/fork 切换)+ host/desktop 两端对拍;CR08-08 根治(live `run started` 文案统一 + run/diagnostic 有序插入);删 gui_server SessionGet 丢弃结果的重复 timeline() 预调用 |
| D(串行)| OnFailure 变体删除 + NeverAsk `#[serde(alias = "on_failure")]`「接受旧值、不再产出」;compat 导入 codex "on-failure" 与 claude "acceptEdits" 映射 NeverAsk + CompatIssue warning(decision 保持 Ask + requires_review 不放宽);S13-F16 三处收窄注释清除;ApprovalMode 不在 wire golden 面,无需协议 minor 版本 |

- **验证要点**:26 帧 golden、events_golden、schemas/ 全程零 diff;五包定向全绿;三通道真实冒烟(desktop probe-smoke / headless --json-stdio / ACP initialize/session/new/prompt)。
- **整阶段审计修复(grok-4.6 四路分域)**:registry 与生产 host 可用面失真(GuiHostAdapter 未实现的 8 command + 4 query 改 unavailable);GUI 帧能力泄漏(Events/Snapshots/TerminalStreaming/ArtifactStreaming 从只宣告改为覆盖首帧 Snapshot、Subscribe/Unsubscribe、Resume replay/fallback、terminal live/replay 过滤);订阅拒绝污染后续收帧(Heartbeat 作有序屏障);TerminalSessions snapshot 泄漏;不可投影空展示页提前截断游标(complete/next_sequence 按原始持久化 envelopes 计算);assistant committed 失序/跨轮/live-history 锚点(移除后重插、tombstone 吞迟到 delta);并发工具输出串线(既有 `detail` 承载内部 tool_call_id,不新增冻结 wire 字段)。headless/ACP 与 OnFailure 复核无缺陷。

#### R4 — 宿主拆解与可靠性内核(T2+T8+T9;🟢 2026-08-21,整阶段审计 08-22)

- **目标**:AppCore 单体(lib.rs 4,057 行 + gui_host.rs 2,594 行)拆领域服务;幂等 CommandLedger 持久化 + K-02 审批等待前落盘;ACP host actor 化;降级可观测契约(消灭静默 `let _`/回退)。

| 波 | 交付要点 |
| --- | --- |
| A(串行)| 七服务拆分(Session/Run/Approval/Usage/Task/Import/Extension)+ provider_assembly;gui_host 目录化(mod.rs+bus/events/handlers/tests),巨 match 改 QUERY_HANDLERS 7 / COMMAND_HANDLERS 10 静态分发表(与 registry gui.available 双射,pin 测试锁定);lib.rs 4131→1413(<1500)、gui_host/mod.rs 679(<800);AppCore 对外 pub API 形状不变 |
| B(串行)| CommandLedger 入 SQLite:storage v11 `command_ledger` 迁移(CURRENT_SCHEMA_VERSION 10→11,纯新增表不动 v1–v10 DDL;**v11 编号被本波占用,R6 迁移顺延 v12**);作用域列式 `(tenant, client_scope, command_id)`;open 后 reclaim inflight(open_read_only 不动);容量淘汰全局 4096;record 失败 tracing::error + 计数不吞错。K-02:engine `request_approval` 加 emitter,`ToolApprovalRequested` 在阻塞等待(含 batch 短路)前 emit 落盘;GUI resume 改 keep-pending 不 seal、决策落盘不重跑(durable resolve 工具不重跑);CLI resume 维持 seal Denied |
| C(并行×2)| ACP actor 化:单 actor(独立 OS 线程 + current_thread runtime)+ mpsc 信箱独占 5 map/negotiated/outbox,std Mutex 与 35 处毒锁 expect 清零,prompt 串行语义与 HEAD 一致,urgent cancel select! 插队 ∥ DegradeEvent 契约:domain `degrade.rs`(DegradeKind 六类:HomeDirFallback/MissingCredential/EventStreamLagged/TasksFinishFailed/IdempotencyConflict/AcpState),复用 AgentEvent::Diagnostic + AppEvent::Diagnostic 双通道,serde 零 diff,code 命名空间 `degrade.*` pin 冻结;五接点接线(HOME 回退真实 warn、无凭证兜底、Lagged 删 seq-0 旁路、tasks_finish persist-first、幂等 record 失败只 tracing 不发客户端帧) |
| D(串行)| host 域非测试 `let _` 58 处清零(常态竞态 debug、fail-closed 升 warn、弃绑定改定义处命名);HOME 回退单一 `consume_data_dir_outcome` 结构化告警出口(load_with/ops 消费);usage 哨兵按 ADR-038 D1 doc+pin 钉死(LEDGER_ACCOUNT="local/default"、upstream_attempt=Some(1)、trace_id=None,账本写入值零变化);hub 简化(RingInner 拆除、死 API 删);acp map.rs 死码删除 |

- **验证要点**:app/cli/storage/engine/domain/protocol/client 定向全绿;26 帧 golden 与 events_golden 零 diff;审计后独立门禁 355 passed / 0 failed;`cargo check -p pawork` 绿。
- **整阶段审计修复(7 项)**:InFlight 同键不同 command_id 占位挂死与丢唤醒(见根因志);record 失败 inflight 不释放(DB 类错误幂等重试一次再 release);tasks_start_agent `.ok()` 吞错升 warn;lib.rs compact_session 内联 63 行搬 RunService(1458<1500);cli/acp.rs 三处 flush_outbox 补 warn;wait_std 无界 recv 改 recv_timeout(2s) 区分 Timeout/Disconnected;open_read_only 不 reclaim 补回归。驳回虚构路径等误报(B 初版 P0 引用不存在的 crates/storage/src/command_ledger.rs 等)。行数漂移登记:审计后 lib.rs 1458、gui_host/mod.rs 812(略超 800,为有界等待/重试增量,接受)。

#### R5 — Provider 中立化与凭证收口(T6+T11+K-10;🟢 2026-08-22)

- **目标**:provider_hints 命名空间契约替换存储层 provider 键名清单;通道 preset 数据化(新增通道=注册表加一行);credential locator 合一 + keychain 词汇迁移;K-10 Anthropic 能力收口(全仓唯一真 TODO);ReasoningProtector 持久化(PWB1 首个生产消费者)。

| 波 | 交付要点 |
| --- | --- |
| A(并行×2)| provider_hints 契约:domain `provider_hints.rs`(键名 `provider_hints.<provider>.<key>`、键 ≤128B/值 ≤64KiB、Secret 键扫描拒绝、冻结 LEGACY_HINT_KEY_MAP 三旧拼写);storage 删 OPAQUE/CONTINUATION 两 allowlist 常量,读取链经共享 decode_persisted_json 做 legacy→canonical 映射(旧拼写永不落盘);**键名错位根治**:唯一生产者写无前缀 `responses.summary_entries` 因不在 allowlist 被保形脱敏,改规范键 `provider_hints.openai.responses.summary_entries` ∥ 通道注册表:providers `channels/registry.rs`(CHANNEL_REGISTRY 六行,is_enabled 单一 cfg 求值点 fail-closed);ApiKeyChannel 枚举删除改 preset 驱动;engine 守护名单 = registry ids 派生 + 基线别名(S12-CR06-10 根治) |
| B(串行,Secret 面单一 owner)| auth 新模块 `locator.rs` 单一事实源(env 名推导、PROVIDER_SERVICE_PREFIX/oauth_secret_service、MCP_SERVICE_PREFIX/`pawork.mcp.*`/mcp-auth.json 常量);workspace config env.rs 过渡实现整文件删除(workspace 无 auth 依赖边,cargo tree 断言);keychain 词汇迁移:StoredCredential 字段 keychain_service/keychain_account→secret_service/secret_account(serde alias 读旧写新 + 迁移测试,兼容一个版本期;auth.json v1 落盘形状零变化)、CredentialSource::Keychain→AuthFile;F05 域隔离语义逐字节不变 |
| C(串行)| K-10:prompt cache(Automatic 有 cap 才写 `cache_control ephemeral`/Required 无 cap 即 InvalidRequest/Disabled 永不写)、thinking(`{type:enabled,budget_tokens}`,Low=1024/Medium=2048/High=4096;temperature≠1.0 或 max_tokens≤budget 拒绝)、hosted_tools/extensions nonempty 进 negotiator required_tools 未声明 HTTP 前拒绝、signature 经 protector 事件只带 ProtectedBlobRef——写 wire 或 HTTP 前显式拒绝,TODO 清除;CapabilityNegotiator 在 prepare_request 接线;ReasoningProtector 持久化:app `protected.rs` 注入 ProtectedBlobStore(同一 `Arc<SwappableReasoningProtector>` 注入四通道),storage `protected` feature 进 pawork 闭包(chacha20poly1305 仅随 feature);instance-level BlobScope `instance-reasoning` 为已接受偏差 |

- **验证要点**:domain/storage/providers/engine/app/auth/workspace/tools 定向全绿;信封/DDL/PWB1/26 帧 golden 零 diff;PWB1 golden + `cargo tree` 断言;真实 Anthropic 冒烟按 fail-closed 未发请求(本机无凭证),留人工验收。
- **整阶段审计修复**:provider_hints 深层 Secret 递归脱敏与旧键不再重导出;损坏 auth 文件 fail-closed(不静默降级 env,auth list 同样);兼容导入生成 `pawork.mcp.<server>` Secret service 与运行时 locator 一致;执行沙箱拒绝 auth/mcp-auth/`~/.pawork`/`PAWORK_HOME`/`PAWORK_DATA_DIR`;Anthropic Required cache 落点回退、thinking 预算下限 1024 且严格小于 max_tokens、signature/redacted payload 形状错误 HTTP 前失败、reasoning continuation 绑定产生它的模型(切换模型不复用);negotiator requested/unsupported 分区不变量;master.key 精确 32 字节读取、拒符号链接与组/其他权限、no-replace 原子首建 + 并发回归。

#### R6 — 会话分支模型原生化(T4,ADR-040;🟢 2026-08-23,波 0/A/B/C + 整阶段审计)

- **目标**:分支 lineage 一等建模,替换 S13-F09 的后补投影列 + 反查回填 + `ancestor_lineage` API 外挂;schema v12 迁移 + 旧库升级 golden;压缩按分支水位;K-05 本机会话导入。
- **关键决策(ADR-040 Accepted 2026-08-23)**:D1 原生化(否决删除 Fork——已交付产品能力);D2 事件账本 append-only 单表 + session 全局 sequence 保持(否决 per-branch 流);D3 lineage 单点收编,消灭 `DEFAULT 'main'` 静默回退;D4 **schema v12**(v11 已被 R4 波 B command_ledger 占用)——回填即校验、检入升级 golden、信封 v1 零 diff;D5 压缩按分支水位三处口径合一,fork 只许切 turn 边界;D6 K-05 并入波 C。回滚不可行:v12 被写入后旧版拒开,回滚路径 = 恢复迁移前备份。
- **波 0 核查修正**:`session_events.branch_id` 自 v1 即 NOT NULL 一等列(F09/v10 后补的是 messages 投影列);压缩三处语义不一致实态——host 用 lineage 算水位、storage CompactionEngine 按本支读取(且 filter_retention_inputs 二次过滤)、投影按事件 branch 删除。

| 波 | 交付要点 |
| --- | --- |
| A(串行)| CURRENT_SCHEMA_VERSION 11→12:TEMP 触发器对无事件背书的 messages 行 fail-closed(RAISE ABORT 整批回滚)→ 按事件所属 branch 重建整表去 `DEFAULT 'main'`;检入 4 个升级 golden(v10 fork 树/v11 交错/v10 压缩折叠/v11 孤儿负例),fixture 由真实写入路径落盘字节生成、`PAWORK_WRITE_STORAGE_GOLDEN=1` 门控再生;删除公开 `ancestor_lineage`(零生产消费者);create_session 显式写 active_branch |
| B(并行×2)| compact 读取与 retention 过滤统一 active lineage;branch snapshot 从 append-only event ledger 重建消息、按 lineage 可见 CompactionCompleted 最大水位折叠(父支晚压缩/late-fork/兄弟隔离回归);host compact 错误显式上抛,无 outcome 水位 fail-safe 0;`fork_from_event` 白名单 = RunCompleted/RunCancelled/RunFailed + standalone CompactionCompleted,同 `(parent, fork point)` 幂等;Pi Branch marker 折叠为 main 上 `pi.branch_collapsed` Diagnostic 不造零事件 branch;protocol 非 wire `ForkBoundary` 单点判型 + Desktop 渲染/动作双 gate + 同 session 切支 reset baseline |
| C(串行)| K-05:compat 双形态解析——Claude Code 本地 JSONL(sidechain/thinking/噪声跳过计数、标题取 aiTitle/customTitle、未知行落 Raw)与 Codex rollout 信封 `{timestamp,type,payload}`(session_meta 取 identity、event_msg 仅 token_count→Usage);损坏文件零 record fail-closed,旧路径逐字节不变;workspace `session_scan` 只读发现(有界/不跟 symlink/**排除 Claude `agent-*.jsonl` sidecar**——与父会话共用 sessionId);CLI `sessions import --from claude|codex` 经 app facade(不加 cli→workspace 依赖边);fork 生产路径 export v3→import 往返回归;.jsonl 嗅探首行整行读取(8KiB 截断被真实大首行证伪) |

- **验证要点**:storage(compaction feature)125+5、四包定向、desktop runtime_shaders 28/28 全绿;信封 v1/export v3/DDL v1–v11/波 A fixture 字节零 diff;隔离数据目录真实样本导入 + export 还原 + `--from` 幂等通过;真实 Provider fork/compact 冒烟留人工验收。
- **偏差与登记**:v9 正例种子孤儿行 m-orphan 移出(旧「孤儿静默归 main」断言与 D4 fail-closed 冲突,孤儿负例由专项 golden 承接);host 先读 lineage 组装 + storage 再读同 lineage 权威校验为有意跨 crate 双检,非特判;整阶段审计无 P0–P2,P3×2 修复(claude 噪声 skipped_* 计数补齐、锚点回退注释定性);波 C 5 项 P3(多 text part 拼接、畸形 tool_use 回退、嗅探内存有界、部分损坏静默导入、扫描根 symlink)留 ROADMAP §4 待 R9。

#### R7 — 执行面真隔离(T7,ADR-041;🟢 2026-08-23,波 0/A/B/C + 整阶段审计)

- **目标**:执行面从「标签诚实」升级为「语义诚实」:macOS Seatbelt 写白名单正式化;PTY 入 policy 闸;shell 风险分类结构化;K-09 终局;fail-closed 只紧不松。
- **关键决策(ADR-041 Accepted 2026-08-23)**:D1 写侧 deny-default 白名单正式化 + 读侧整盘 allow 挖洞(**读白名单经 Darwin 25.6 实测不可行**:deny-default + 系统根全枚举下连 /bin/echo 都 SIGABRT 134);D2 PTY 创建动作入 policy 闸(会话内容不逐条审批但如实标注);D3 K-09 选删除 `network_allow_hosts` 字段(选项 b;字段无配置入口、无生产赋值、唯一消费者是注释行——egress broker 选项 a 转候选,否决维持全拒+文档的选项 c);D4 shell 手写轻量 tokenizer(不引入 tree-sitter-bash;分类只影响升档,灾难地板不动)。本机实测数据进 ADR:写白名单下 clang/git/cargo/brew 工具链全通;`(deny network*)` 有效;spawn 开销约 5.7ms/次。

| 波 | 交付要点 |
| --- | --- |
| A(串行,安全内核单一 owner)| golden 先行六面落钉(metadata.sandbox 键集+limits 六值、IsolationLevel 五词汇 as_str/serde 双钉、投影 sandbox_timeline_detail/fallback_label、CLI notice 分支、Seatbelt profile 整体输出、default_secret_paths 固定向量);macOS profile 正式化:读=整盘 allow+secret 挖洞(删冗余系统读枚举),写=write_roots+/tmp+/private/tmp+$TMPDIR(raw+canonical 双形态)+/dev,每可写根永久禁写 .git(subpath)/.env(literal 双形态);**symlink 根(/var→/private/var)deny 不命中缺陷修为双形态**;default_secret_paths 扩六项(.netrc/.git-credentials/.docker/.npmrc/.pypirc/.cargo/credentials.toml);Linux/Windows 复核零行为变更、标签如实(HardFilesystemOnly/Degraded) |
| B(并行×2)| PTY 创建入闸:terminal_create 经 PolicyEngine(capability=Process,信任语义镜像 loop_ctx),NeverAsk/ReadOnly 直拒,**AskUser 一律 fail-closed 落 Deny(用户拍板选项 A**——GUI 审批回路以 run 为键无命令级承载,命令级交互审批须另立 ADR 做 wire 演进,转候选;后果:AlwaysAsk/AskForWrites 档不能创建终端);响应以 sandboxed/policy/approval_mode/note 如实标注替换 `uncontrolled` 裸语义;Terminal 四帧+响应六 golden 先行 ∥ shell 手写 tokenizer(单双引号/转义/管道/重定向/`$()`/反引号/变量感知,`-lc` 组合簇闭环);引号拼接程序名、程序位变量、curl|wget 管道进 python/perl 收紧为 Dangerous;灾难地板集合不变;残余局限如实登记(nohup/env/xargs launcher 不解包,wrapper 升档变松接受) |
| C(串行)| K-09 按 D3:`SandboxPolicy.network_allow_hosts` 字段与 os/macos.rs 死分支删除(2 文件 9 行删除 0 新增);Enforce `(deny network*)` 全拒不变,profile 输出零 diff;NetworkMode 三档与 IsolationLevel 词汇不动;8 处构造点走 `..Default::default()` 编译安全 |

- **验证要点**:policy 73 / protocol+app 306 / exec 64(Seatbelt 真机逃逸种子无 SKIPPED,进程组回收回归)/ client probe 全绿;msvc 交叉 check 绿;schemas/registry/wire/波 A golden 面零 diff;Desktop PTY 面板冒烟留人工验收。
- **整阶段审计**:修 P1 反引号地板漏检(见根因志)与 P2 probe 零终端场景(波 B 收口所称「probe 保持终端场景」实态九场景无一含 TerminalCreate,新增 terminal-gate 场景放行/拒绝双路,probe 10 场景绿);否决 4 项与实态不符的 explorer 论断(SandboxMode 等符号不存在、回退文案实已统一、run_command.rs +88 全在 tests、policy 计数 73 正确);六包矩阵 658 passed / 0 failed;登记不修项(Seatbelt 真机探针补强、Windows Job 单测、终端闸 P3 集)入 ROADMAP §4。

### R8 阶段存档 — GUI 组件化与 Desktop 收口(T12;提取时点 🔵,仅余 K-03 人工签字)

背景基线:apps/desktop 四层边界干净(ui 不碰 socket、projection 纯函数、controller 唯一写者),但 ui/mod.rs 单文件承载全部渲染、零 hover、菜单为 in-flow child、Timeline 全量 eager 物化。开工核查修正 2026-08-18 快照多处漂移:硬编码色实态 78 行/92 调用/25 去重值(非 97 处),菜单实态 5 组(grouping/scope/model/entry fork/workspace confirm,非「model/mode/provider/session 四组」),手写按钮 21 处 on_click(非 15),`uniform_list`/`0x3ecf8e`/pty_view.rs 不存在,「F44 长标题」仓内不可溯源改实态登记。

| 波 | 交付要点 |
| --- | --- |
| A(2026-08-24)| theme.rs:Theme 六组 25 色 token(bg3/surface2/border2/text10/accent2/semantic6)+ 字阶 11/12/13 + metrics 14 常量;95 处消费点(92 rgb/rgba 调用 + 审批数组 3 裸 u32)逐值等价机械替换,rgb/rgba/0x 与数字 px 字面量零残留;前置修复 Desktop 真窗口启动崩溃(gpui 前台执行器无 tokio reactor,见根因志),写入集实态扩为 ui/3 文件 + controller.rs;desktop 28/28 绿 + probe-smoke EXIT=0 + 真窗口启动实证 |
| B(2026-08-24)| components 基础族 button/label/panel/status_bar/list_row + dropdown + follow_scroll;16 处非菜单 on_click 迁 Button;五组菜单全迁 `deferred(anchored())` 浮层——开合状态收敛 `Option<MenuKind>` 单开互斥修双开、Escape 根节点冒泡承接(面板 deferred 不可聚焦)、外点 on_mouse_down_out 以 (MenuKind, Point<Pixels>) 匹配同一物理点击、面板 occlude() 滚轮无穿透;hover/active 基准先行(design/README.md §8.1–8.3,theme 25→29 色:surface.hover/accent.hover/success_hover/danger_hover);FollowScroll + 回底控件 + follow_terminal 重置;审查修 2×P2(FollowScroll 滚轮双计见根因志;pending_outside_close 吞后续单击) |
| C(2026-08-24)| ui/ 拆六模块 timeline/timeline_entry/approval_card/input_area/inspector/task_rail,mod.rs 1950→824(<900 达标);Timeline 首次引入 gpui `list()` 变高虚拟化(ListAlignment::Bottom 钉底、timeline 变化统一 reset(new_count)、脱钩读史恢复 reset 前偏移、审批卡作末项、Entry 菜单 close-on-reset);TaskRail 长标题 `.truncate()`(本仓首个 TextOverflow 消费点);DiffView 无消费面按红线留波 D;FollowScroll 收窄终端专用;真窗口截图实证 Connected 渲染;P3-4 Entry 菜单滚动卸载短暂失联登记(后经 D4 拍板接受) |
| D(2026-08-24)| Inspector 三页签 Changes/Terminal/Resources。K-04 Changes 只读面(**拍板 Q1=A**):ui/changes.rs 705 行——Files 行点击经 diff_get 拉 hunks、Summary 七字段、DiffView 绿/红 hunk 语义着色 + font::MONO=Menlo 显式等宽 + 全仓首个 overflow_x_scroll 横滚、ActivityPopover(StatusBar 触发,「N files · +A/−D」摘要行点击定位)、session_mismatch banner;git_stage/HunkStageService 接线顺延 ADR 候选(git_stage wire 自 V1 仅文件级 stage-only),K-04 记部分交付。Resources:ui/resources.rs 210 行 MCP 只读表;protocol registry mcp_list gui.available 翻 true + app handler fail-soft(帧 golden/schemas 零 diff)。K-06「@」端到端(**拍板 Q2=A**):host run_start 接线 expand_at_refs,附件独立 Text part `[attached file: path (complete|truncated)]`,fail-closed 零 wire 变更;「@」补全 query 顺延候选;提交前主代理修 P1 幽灵 run(先展开再登记 ActiveGuiRun)。probe +3 场景 13 全 PASS;desktop 41/41 绿;真窗口逐项截图实证(Changes 四面/Resources 行/「@」bubble 双 part/断线 fail-soft) |
| E(2026-08-24,自动化部分;提交 528ab3d)| S12-CR09 五项复核:02(stop --apply 记录)/04(HunkStageService 登记)维持 ✅;01/03/05 口径漂移同波修复——README 状态表与结构图、v2-summary 归档注记、design.md §3.3 路径校验语义矩阵回写、io.rs canonical_within 残余登记;K-03 自动化取证真窗口截图 7+1 张(soak 为 D3 实证)落 gui-design.md §9 + 附录 A;用户四项拍板 D1–D4(见下);desktop 空闲心跳修复(soak >2min 与 33min 实证);grok_reviewer 一轮 4×P2 文档对齐修复;K-03 人工走查(附录 A.2 十一项)待用户签字后 R8 转 🟢 |

波 E 用户四项拍板(2026-08-24):

- **D1** mod.rs 波 D 回弹至 1031 行(>900 阶段目标;changes.rs 705/resources.rs 210 已拆出余量有限):接受 1031 为终态口径,任务书 §1.4/§4 退出标准同批修订,不再重瘦。
- **D2** 窄窗响应式未实现(基准规定 1080–1279 宽 TaskRail 收敛 240px + Inspector 默认折叠;实现为固定 288px,V2 起即如此、非 R8 回归):接受登记,固定宽维持现状转候选。
- **D3** 空闲约 30s 断连:实为 host `heartbeat_timeout` 30s + desktop 无周期心跳的机制性关闭(波 C「显示器休眠/App Nap」归因不准确;Reconnect 可恢复、run 不取消);已修复(见根因志)。
- **D4** P3-4 Entry 菜单滚动卸载(浮层随条目回收、滚回自现、Escape/外点仍有效):接受为虚拟化卸载语义下的有界差异。

整阶段审计(2026-08-25,四路 glm 分域只读审计 + 主代理逐条源码复核;13 项发现全部实证成立,无否决改判):

- **修 8 项**:P2 会话切换 Timeline 跟随态与读史偏移泄漏进新会话(open_session 漏重置 timeline_following,新会话开在旧会话偏移的历史中部且不跟随;修为与终端重置同点补置 true);P2 历史 flake 根治——tracing-core interest 缓存投毒(RecordingCapture 双注册钉住 + 确定性回归 + 14 burners×45 runs 负载全绿;历史归因「async 跨线程迁移」一并更正,见根因志);P3 hsla 占位色绕过 token 扫描(补 text.placeholder,theme 29→30 色,视觉零变化);P3 metrics 字面量归位(SUMMARY_LABEL_WIDTH/ZERO);P3「@」附件头 wire 格式钉死(split_once 精确断言头行);P3 文档/基准对齐四项(gui-design 仅 dark 基线、MODULE.md 补 testsupport、截图计数 7+1、基准 §8.6 增补)。
- **登记 6 项不修**:BackToBottom 滚轮死区(浮钮未 occlude);desktop 心跳泵自动测试缺口(内联魔数 15);泵错误路径 state.client=None 竞态(既有、窗口极窄);main.rs 窗口尺寸字面量(与基准一致仅形态未收 metrics);extension.rs mcp_list auto_start 死分支(既有,wire 中性);gpui 渲染面无自动门禁(既有缺口复证——菜单/滚动/虚拟化/hover 均靠真窗口截图 + K-03 人工走查)。
- **审查门异常如实记录**:glm_reviewer 与备用 zai/glm-5.3 均在任务执行前被路由基础设施拒绝,未读代码未给 verdict,不记 pass;以主代理逐项源码复核 + app 146 与 desktop 41/41 定向门、负载门全绿收口。

### R9 已完成部分(提取时点:波 A1 🟢 2026-08-25,A2/B/C 未开始)

- **编号谱系结论**:V1 历史任务书覆盖 P0–P19(共 224 个编号任务;P19-1~P19-16 全为 Designed/未开始,无 p19-review),已随 V1 归档;V2 用 S0–S13 且已收官;当前活动线为 V3 R0–R9。**不续造 P20**——三代编号不得混成一条虚假进度线;P0–P19 只供考古(git 历史、tag `v2-final`、../Pawork_v1/),不得复制回主干重新宣称为当前计划。
- **docs/spec/ 产品 Spec 文档集建立**:README 固化事实源优先级、状态词汇与 P→S→R 谱系;product/capabilities/contracts/security/desktop/verification/operations/backlog 八份现行 Spec + feature-template 一份受控模板(带激活闸门,小功能仍直接写任务书);不复制完整 API/DDL/包地图;「已实现/已验证/已人工验收/已发布」分开表述。常设导航同步:README、AGENTS、ROADMAP、v3_plan、task-guide、design、v2-summary。明确不增加:V1 P 文档副本、P20 占位、手抄 API 清单、与 MODULE.md/code-map 重复的包地图、批量空 Feature Spec。
- **候选计数纠正**:design/ROADMAP 旧汇总「30 项」按表内条目纠正为 **28 项**(P1 5、P2 17、P3 6)。
- **明确保留待办(不借文档任务宣称完成)**:R8 K-03 人工签字、R9 A2(其余文档/登记/断言一致性)/B(三类回归全量复跑 + 冒烟矩阵)/C(K-01 + S6 OAuth refresh 人工验收 + 收官登记)、docs/v3-summary.md。

### ADR-037~042 摘要索引

ADR 原文保留在 docs/adr/ 不删;编号续接 V1(ADR-001~035 随 V1 归档,原则继续有效)。

#### ADR-037 — S13 波 B 契约/红线决策

- 状态:Accepted(2026-08-18);落实:V2 S13 波 B(先于 V3 各阶段;本仓 docs/adr/ 自此文件起维护)。
- 五项决议:F15 `SessionRegistryStore` 及记录类型下沉 pawork-domain(session 实现去 protocol 依赖,protocol::adapter 保留映射与 re-export);F19 维持 ADR-031 可观测回退(硬隔离不可用回退 NativeRestricted,不拒跑,fallback 必须对用户可见);F24 `ToolResultContent.artifacts` 附加式扩展(空向量不改 32 变体夹具字节);F26 `PlanEvent::Revised` 携带 title/steps(附加式,保留修订链);F28 `ResultArchived` 加 task_id,幂等键 `(automation_id, task_id)`。
- 边界:事件载荷只允许附加式 serde 演进;信封 v1 与 `UNIQUE(session_id, sequence)` 不在其范围(F09 已钉死)。

#### ADR-038 — V3 产品形态与休眠库存裁决

- 状态:Accepted(用户 2026-08-18 确认);落地:R0 波 A–C。
- D1 单机优先(T10):`local/default` 哨兵宇宙不扩张,多账户 factory 转候选;否决多租户层级形态。D2–D22 共 22 项:大块归档(account-control-v1、binding/schema、workflow 三域、teams、memory/review、transport remote)、小块删除(K-07 rate_limit、lifecycle、net 死模块、deprecated)、K-08 停止宣告、保留项(PWB1、CapabilityNegotiator、config Loader 待 R5 接线)、可见性降级。
- 落实改判 4 项:D12 run_turn 仅 pub 降 pub(crate);D15 rbac 三类型全保留;D14 encoding_rs 保留(类型级隐性直接依赖);D16 commit.rs 补判归档。
- 兜底:git tag `v2-final`(088b539);domain canonical 事件类型一律保留(重放红线)。

#### ADR-039 — 包合并布局(37→21)与不合并清单

- 状态:Accepted(用户 2026-08-19 确认);落地:R1 波 A–E。
- D1 扁平 `crates/<短名>` + `apps/<name>`,git mv 集中波 E 一次完成;D2 不合并清单固化(policy/exec/auth/git/engine/protocol/testkit/transport/orchestration/workflow);D3 api→domain 的 GUI 侧口径(纯类型入编译闭包不违反「GUI 不加载 Core」运行时红线);D4 解散 16 包 + probe 转 client 测试;D5 编译粒度代价明示并实测补录(秒级,接受);D6 跨包纪律降级为模块纪律 + 定向测试;D7 golden 先行(Provider 契约 13 变体等字节级夹具先补后迁)。
- 参照:codex-rs 布局纪律「只抄纪律不抄粒度」(其 134 成员微 crate 增殖为反面教材)。

#### ADR-040 — 会话分支模型原生化(schema v12)

- 状态:Accepted(用户 2026-08-23 确认);落地:R6 波 A–C。
- D1 原生化(否决删除 Fork);D2 事件账本 append-only 单表 + session 全局 sequence 保持(否决 per-branch 独立流);D3 lineage 单点收编,消灭 `DEFAULT 'main'` 静默回退与公开 ancestor_lineage 外挂;D4 schema v12 迁移(v11 已被 R4 command_ledger 占用)——回填即校验(孤儿投影行使迁移失败)、检入升级 golden、信封 v1 wire 零 diff;D5 压缩按分支水位三处口径合一,投影删除不波及兄弟分支,fork 只许切 turn 边界(`(parent_branch_id, forked_from_event_id)` 对应 DSH `(parentSession, seedLength)` 的零拷贝形态);D6 K-05 导入并入波 C(单分支、源只读、Secret 前缀拒绝)。
- 回滚不可行:v12 被新版写入后旧版 open_read_only 拒开;回滚路径 = 恢复迁移前备份;append-only 事件本体不丢。

#### ADR-041 — 沙箱信任模型与执行面真隔离

- 状态:Accepted(用户 2026-08-23 确认);落地:R7 波 A–C。
- D1 macOS 写侧 deny-default 白名单正式化(workspace+tmp+$TMPDIR+/dev,.git/.env 永久禁写),读侧整盘 allow + default_secret_paths 挖洞——读白名单经 Darwin 25.6 本机实测不可行(/bin/echo SIGABRT 134;codex/srt 上游读侧同样非全 deny);D2 PTY 创建动作入 policy 闸(NeverAsk/ReadOnly 拒创建;会话内容不逐条审批但如实标注,替换 uncontrolled 裸语义);D3 K-09 删除 `network_allow_hosts` 字段(选项 b;网络只有 allow-all/deny-all 两档事实;egress broker 选项 a 转候选);D4 shell 手写轻量 tokenizer(分类只影响升档,灾难地板不动)。
- 本机实测进 ADR:写白名单下 clang/git/cargo/brew 全通;`(deny network*)` 下 curl 解析即失败;spawn 开销约 5.7ms/次。

#### ADR-042 — Desktop 原生 Accessibility bridge

- 状态:Accepted(用户 2026-08-26 确认);落地:R1 Wave C。
- 保持 `gpui = 0.2.2` 与 Desktop→client 唯一业务依赖不变;Desktop 以平台无关 `AxTree` 显式生成语义,macOS 用 AppKit 虚拟元素挂入 `GPUIView`,非 macOS 保留 no-op facade;AX action 回到既有 AppView handler / enable gate,未知请求 fail-closed。
- 真窗口补救前仅 7 个系统节点;补救后 75 节点、0 截断,稳定 identifier/role/value/action 可用,`AXPress` 选会话与 `AXValue` 写 Composer 均产生可观察状态变化;证据见 `docs/ui-review/wave-c/{ax-gate,ax-bridge}/`。Windows/Linux 平台 AX 与全量 VoiceOver 仍属后续范围。

### 已闭环登记项存档

自 ROADMAP §4(未决事项)与 §3.2(V2 遗留债务映射)提取:结论已闭环且无遗留未来动作的行。带「R9 复查/复跑」「人工验收」「另立 ADR 时」「触碰…时」「候选」「兼容期满」等未来动作的行仍留 ROADMAP,不收入本表。

| 事项 | 结论 | 时点 |
| --- | --- | --- |
| ADR-038 库存与产品形态 | Accepted;22 项决议全部落地或显式改判(改判 4 项见 ADR 落实记录);闸门解除 | 2026-08-18 确认;R0 落地 |
| ADR-039 目录布局 | Accepted;扁平 `crates/<短名>` + `apps/<name>` 与不合并清单固化;波 A–E 全落地(members 21);闸门解除 | 2026-08-19 确认;R1 落地 |
| ADR-040 分支模型 | Accepted;原生化 + append-only 单表全局 sequence + lineage 单点 + v12 回填即校验 + 压缩分支水位;波 0/A/B/C 全落地;闸门解除 | 2026-08-23 确认;R6 落地 |
| ADR-041 沙箱信任模型 | Accepted;D1 写白名单+读整盘挖洞 / D2 PTY 入闸 / D3 删 network_allow_hosts / D4 手写 tokenizer;波 A–C 全落地 | 2026-08-23 确认;R7 落地 |
| ADR-042 Desktop Accessibility bridge | Accepted;保持 GPUI 0.2.2,显式 AxTree + AppKit 虚拟元素 + action 复用既有 UI gate;真窗口 75 节点与两条语义 action 通过,AX 闸门解除 | 2026-08-26 确认;R1 Wave C 落地 |
| K-02 `ToolApprovalRequested` 等待前持久化 | 等待(含 batch 短路)前 emit 落盘;GUI resume keep-pending 呈现待审批、决策落盘不重跑;CLI resume 维持 seal Denied | R4 波 B,2026-08-21 |
| K-05 本机会话导入 | Claude Code 本地 JSONL + Codex rollout 双形态 compat 解析;session_scan 只读发现(排除 agent-*.jsonl sidecar);`sessions import --from` 批量;隔离目录真实样本冒烟通过 | R6 波 C,2026-08-23 |
| K-09 macOS `network_allow_hosts` | 按 ADR-041 D3 删除字段与死分支;网络维持 Enforce 全拒 / Off·Hint 放行两档事实;egress broker 转候选 | R7 波 C,2026-08-23 |
| K-10 Anthropic Messages 能力收口 | prompt cache/thinking/hosted tools/signature/server_tool/citations 写 wire 或 HTTP 前显式拒绝;全仓唯一真 TODO 清除 | R5 波 C,2026-08-22 |
| pawork-sdk handshake 契约测试既有失败 | 夹具 hello_ack.json negotiated 对齐 1.2(S13-F13 升 API_VERSION 未随更夹具) | R1 波 E,2026-08-19 |
| workflow plan_service 回放测试既有失败 | 测试侧改为携现有步骤修订(revise 空 steps 越界,基线 v2-final 可复现、与 R0 无关) | R1 波 E,2026-08-19 |
| ModelList 与 switch_provider 目录不对称 | 新增按 `(provider, model)` 动态探测/惰性合并解析,未知模型仍 fail-closed;desktop probe 实测切换通过 | R1 整阶段审查,2026-08-19 |
| client 事件泵抢占命令错误帧 | Response/Snapshot/Resume 错误按 request_id 归属;Event 只接 `request_id=None` 连接级错误 | R1 整阶段审查,2026-08-19 |
| directories 5→6 | 升 6.0.0;macOS `dev.pawork.pawork` 布局快照 golden×2 v5/v6 逐字节一致;auth 路径确定性覆盖 | R2 波 B,2026-08-20(整阶段复核补测) |
| `session_bindings` 孤儿表 | R0 归档 binding 后无读写方;迁移 append-only,留表 + v9 注释登记「预留」,不回滚 DDL | 2026-08-18 |
| PWB1 protected 消费者 | app protected.rs 注入 ProtectedBlobStore;storage `protected` feature 进 pawork 闭包(chacha20poly1305 仅随 feature);instance-level BlobScope `instance-reasoning` 接受偏差 | R5 波 C,2026-08-22 |
| gui_host record tracing 断言 flake | 根因非「future 跨线程迁移」,实为 tracing-core interest 缓存投毒;RecordingCapture 双注册钉住 + 确定性回归,14 burners 45/45 负载绿 | R8 整阶段审计,2026-08-25 |
| R8 mod.rs 行数回弹(1031>900) | 用户 D1 拍板:接受 1031 为终态口径,任务书 §1.4/§4 退出标准同批修订,不再重瘦 | R8 波 E,2026-08-24 |
| GUI 空闲 30s 断连机制 | 定性为 host 30s 心跳超时 + desktop 无周期心跳的机制性关闭(非环境性);D3 修复 desktop 15s 空闲心跳,真窗口 soak >2min 实证 | R8 波 E,2026-08-24 |

### 阶段外任务存档

ROADMAP §3.1 已完成阶段外任务:

| 任务 | 完成日期 | 产出 |
| --- | --- | --- |
| V3 立项分析(五路只读:包合并 ×2、依赖用面审计、GUI 组件分析、补丁式实现全仓扫描) | 2026-08-18 | 结论沉淀于 plan/R0–R9 各任务书与 ROADMAP §1/§2 |
| V2 文档归档(v2_plan / V2 ROADMAP / plan S0–S13 压缩为总结) | 2026-08-18 | docs/v2-summary.md;原文档删除,git 历史可溯 |
| 参照项目全面复核与 V3 参照指引(GitHub API 全量复核 + 功能重叠二次清理移除 5 项 + 新增 ACP/gpui-component/Zed ui/srt 四项 + R0–R9 阶段参照调研,三路子代理) | 2026-08-18 | docs/references.md §7 阶段参照指引;移除记录见 docs/research/multi-account-quota-reference.md §8 |
| 参照项目补官方仓 openai/codex(手册 §1 主链接从产品文档站改 GitHub 仓,五处文档引用同步) | 2026-08-21 | docs/references.md §1/§2.3、docs/research §1/§8、design.md §4、gui-design.md §2 |
| 三层代码地图 | 2026-08-22 | 任务书 plan/out-of-band/code-map.md;总索引 docs/code-map/README.md;21 份 crate/app MODULE.md;热点 docs/code-map/hotspots/ |

三层代码地图任务要点:纯文档任务(不改 .rs/Cargo.toml/golden/wire),三层结构——总索引(依赖自底向上列 21 成员)+ 每包 `MODULE.md`(固定六节:职责/模块树/对外入口/依赖与被依赖/红线/相关文档)+ 热点深描(Agent loop、GUI Connection Protocol、事件持久化与重放、凭证与脱敏四条跨包热路径);每模块独立 commit(`docs(code-map):` 风格),#22 收尾回写 AGENTS.md §6 与 README 导航。定位为按需导览、**非**布局/契约事实源:日常按写入集加载包级 MODULE.md,总索引仅定位包用,冲突以源码为准并回写。R9 波 A1 建立 docs/spec/ 产品汇总层后明确「不新增与 MODULE.md/code-map 重复的包地图」——产品级事实汇总由 spec 承担,包级导览职责由各包 MODULE.md 承担。**后记(2026-08-25 文档重构)**:MODULE.md 与 code-map(含 hotspots)已整体由 [spec/crates/](spec/README.md) 包级 Spec 与 [spec/flows.md](spec/flows.md) 取代并删除,导览职责随之移交;原文以 git 历史为准。

### 疑难问题根因志

从各阶段审计、R8 §6 与 ROADMAP §4 提炼的长期复用调试知识,按「现象 / 根因 / 修法」压缩。

**1. tracing-core interest 缓存投毒(测试 tracing 断言偶发失败)**
现象:gui_host record 失败 tracing 断言偶发失败,自 R5 波 A 登记、R7 审计再复现,历史归因「async 块跨线程迁移」不准确。
根因:tracing-core 0.1.36 每个 callsite 在全局注册表只缓存一份 Interest;`has_just_one=true` 时某 callsite 若在无 scoped default 的线程首次命中,走 JustOne→get_default→NONE 路径缓存 `Interest::never()`,此后**所有线程**该 callsite 的 emit 被宏门 `!interest.is_never()` 静默跳过,直到某次 `Dispatch::new` 的 Write 重建治愈——即与无 subscriber 的兄弟测试共享 callsite 即可被投毒。
修法:testsupport::RecordingCapture 对同一 subscriber 做两次 `Dispatch::new`(注册表推到 ≥2,`has_just_one=false`,Write 重建治愈既有 never 缓存);pin 存活期新 callsite 一律走 Read(vec) 路径,投毒窗口封闭;dismiss() 依序释放。确定性回归 + 14 burners×45 runs 负载验证。

**2. gpui 前台执行器无 tokio reactor 崩溃(Desktop 真窗口无法启动)**
现象:Desktop 真窗口自始无法启动(exit 134);probe-smoke 从不复现。
根因:controller.connect 握手后 ack/subscribe_all 在 gpui 前台执行器(无 tokio reactor)上 await,receive_frame 内 tokio::time 直接 panic;probe-smoke 走 platform.block_on 自带 runtime,该路径无自动门禁。
修法:握手/ack/subscribe_all 全部移入 runtime.spawn 任务(ack 四分支语义逐字节等价);「真窗口启动无自动门禁」单独登记。

**3. FollowScroll 滚轮双计(跟随态误判)**
现象:跟随滚动逻辑对用户滚轮方向/位移判断相反,跟随态错乱。
根因:gpui 0.2.2 Bubble 相监听按注册逆序分发,容器内部偏移应用先于用户监听执行;监听里再自行投影 delta 会把同一次滚动计两次。
修法:放弃 delta 投影,直读 `is_scrolled_to_bottom()` 判定跟随态。

**4. Seatbelt 读白名单在 Darwin 25+ 不可行**
现象:deny-default + 系统根全枚举(/usr /System /bin …)读白名单下,连 `/bin/echo` 都 SIGABRT(134)。
根因:Darwin 25+ firmlink/cryptex 磁盘布局使读侧枚举无法覆盖进程启动所需路径;上游 codex/srt 读侧同样不做全 deny。
修法:ADR-041 D1——读=整盘 `(allow file-read* (subpath "/"))` + default_secret_paths 读写双拒挖洞;写=deny-default 白名单;隔离强度靠写闸+网络闸承担。

**5. Seatbelt symlink 根 deny 不命中(raw+canonical 双形态)**
现象:$TMPDIR、/var 等 symlink 根下的 deny/allow 规则不生效。
根因:Seatbelt 按 canonical 路径(/private/var…)匹配,profile 只写 raw 形态(/var…)时规则落空。
修法:R7 波 A——tmp/$TMPDIR 白名单与 .git/.env 禁写洞一律 raw+canonical 双形态写入 profile,golden 钉死整体输出。

**6. InFlight 同键不同 command_id 占位挂死(幂等等待)**
现象:幂等命令等待方永不被唤醒挂死;record 失败后行滞留 inflight,同进程重试挂死、重启 reclaim 重入。
根因:waiter 按自身 command_id 注册 Notify,而占位行持有者是另一 command_id,叠加 notify_waiters 丢唤醒竞态;record 失败路径未释放 inflight 占位。
修法:R4 审计——50ms 有界等待(select notified/sleep)后回 loop 重查 SQLite 权威 CAS;record 遇 DB 类错误先幂等重试一次(UPDATE WHERE status='inflight'),仍失败或键冲突才 release;hazard1/hazard2 独立回归。

**7. EventHub Lagged 禁止 seq-0 旁路**
现象:订阅端 Lagged 后收到伪造起点的补发帧,序列不可信。
根因:gui_server 曾在 Lagged 时以 seq-0 旁路直发事件,绕开 hub 真序列。
修法:R4 波 C——删 seq-0 旁路,改经 hub 真序列取信封 + host_tx 直发受影响连接 + ReplayUnavailable;测试改断言递增序列帧。

**8. host 30s 心跳超时 × desktop 15s 空闲心跳**
现象:Desktop 空闲约 30s 即被断连(Reconnect 可恢复、run 不取消);曾误归因「显示器休眠/App Nap」。
根因:host gui_server `heartbeat_timeout` 30s、任意入站帧刷新;修复前 desktop 无周期心跳,纯空闲即触发机制性关闭。
修法:R8 波 E D3——desktop controller 泵循环连续 15 tick(≈15s<30s)空闲即发 `heartbeat()`(io AsyncMutex 支持泵内并发),心跳失败走既有断线路径;真窗口 soak >2min 实证。泵计数无自动测试已登记。

**9. shell 反引号灾难地板漏检**
现象:NeverAsk 档 `` echo `rm -rf /` `` 静默放行(同形 `$(rm -rf /)` 正确拒绝)。
根因:take_backtick 把收尾反引号推入 inner 后才 break(take_command_substitution 的收尾 `)` 不进 inner),inner 尾 token 变 `/`+反引号,灾难地板 `== "/"` 精确比较不命中。
修法:R7 审计——收尾符 break 前不入 inner(w.text 保留原文);command_substitution 递归分类补 danger+floor 断言;policy 73 绿。

**10. client 事件泵抢占命令错误帧**
现象:desktop 首发 send_message 确定性失败,等待方 10s 超时误报 Disconnected。
根因:`FrameWant::Event` 匹配所有 `ServerFrame::Error`,常驻事件泵抢走本属命令响应的带 request_id 错误帧。
修法:R1 整阶段审查——Response/Snapshot/Resume 只接同 request_id 的错误,Event 只接 `request_id=None` 的连接级错误;`frame_wants_route_errors_by_request_id` 回归钉死。

**11. K-05 .jsonl 嗅探截断误判**
现象:真实 Codex rollout 文件格式嗅探失败。
根因:嗅探读首行 8KiB 截断,而真实 session_meta 首行超 8KiB,截断后 JSON 解析失败误判格式。
修法:R6 波 C——首行整行读取做签名化嗅探;单行超大的内存占用判为有界接受(导入下一步本就整文件读入)。

**12. notify 8 Rescan 空路径事件漏扫**
现象:notify 升 8 后,后端溢出场景文件变更漏检。
根因:notify 后端溢出/MUST_SCAN_SUBDIRS 时发 `Flag::Rescan` 空路径事件,原转换逻辑按路径映射直接丢弃。
修法:R2 整阶段复核——Rescan 转换为每个 workspace root 的 Upsert 触发全量重扫,补回归测试。

**13. Claude subagent sidecar 与父会话共用 sessionId**
现象:全量 home `sessions import --from claude` 时同 sessionId 多文件互撞,CompatImportConflict。
根因:Claude Code 的 subagent sidecar `agent-*.jsonl` 复用父会话 sessionId(本机键级统计坐实)。
修法:R6 波 C——`session_scan` 扫描层排除 `agent-*.jsonl`;导入器不再见到 sidecar。

---

## 附:文档体系重构(2026-08-25,阶段外任务)

R8 K-03 签字与 R9 开启前,把常设文档从「导览地图 + 分散总结」改组为「包级 Spec + 单一存档」体系。本文(history.md)即该次重构的产物之一。新旧对照:

| 原文档 | 去向 |
| --- | --- |
| `docs/v2-summary.md` · `docs/v1-migration-reference.md` · `docs/headless-json-migration.md` | 压缩并入本文第一部分;原文见 git 历史 |
| `plan/R0–R7` 任务书 · `plan/archive/` · `plan/out-of-band/` | 交付结论并入本文第二部分;任务书原文见 git 历史 |
| `docs/reviews/s12/`(九报告+五裁定) | 结论摘要在本文 S12 节;报告全文见 git 历史 |
| `docs/research/` 三调研 | 压缩并入 `docs/references.md` 附录 A/B/C |
| `docs/task-guide.md` · `v3_plan.md` | 任务约定/编排/凭证矩阵并入 `ROADMAP.md` §7 |
| `docs/design.md`(旧,设计+架构混排) | 拆分:架构红线/包布局/冻结契约/S13 拍板 → `docs/architecture.md`;功能设计/候选池 → 新 `docs/design.md` |
| 21 份 `MODULE.md` + `docs/code-map/`(总索引+四热点) | 由 `docs/spec/crates/` 21 篇包级 Spec(八节模板,含 API 面/行为/契约/测试资产)与 `docs/spec/flows.md`(四条跨包链路)取代 |

终态文档集:README(入口)· AGENTS(工程约定)· ROADMAP(任务事实源+任务约定)· plan/(仅进行中任务书)· docs/{architecture,design,gui-design,references,history}.md · docs/adr/ · docs/spec/(产品篇+crates/+flows)· design/(GUI 视觉基准)。全仓 50 份 markdown 相对链接校验通过;五份 ADR 的死链已改指存档位置(决策内容未动)。

---

## 附：UI 优化路线重排（2026-08-25）

经用户确认，旧 V3 阶段编号停止作为当前任务指针，既有功能与结构任务视为历史完成面；`plan/R8-gui-components.md` 与 `plan/R9-consistency-closeout.md` 退出活动任务目录。旧任务书中仍有价值但尚未获得新证据的 UI 缺口，全部并入新 R1–R8 的视觉合同、组件实现与全功能模拟操作门禁；非 UI 剩余项重新编排为 R9 一致性/代码债务、R10 关键回归/真实环境，发布准备仅在用户另行授权后进入 R11。当前事实源以 [ROADMAP](../ROADMAP.md) 与 [plan](../plan/) 为准，本节只记录编号迁移，不把历史“待签字/待验收”表述改写为已验证。

2026-08-27 用户将尚未开始的 R11 从「发布准备（条件阶段）」改为「设计稿与实际 UI 终局比对」：只对照 `design/` v3 定稿图与已归档 UI 证据，把不符合的显示效果归纳为下一阶段完善任务；该阶段只改文档，不查询、不修改代码。原发布准备（License、供应链、安装/升级/回滚、三平台与全量门禁）退回 [ROADMAP §5](../ROADMAP.md) 候选池，需用户另行授权后立项。本节 2026-08-25 原文保留，以上为后续重定义。

---

## R1 — 视觉合同、固定 fixture 与 UI 测试基座（2026-08-25–27）

R1 四波完成后退出活动任务目录；原任务书全文以 git 历史为准，当前验证事实见 [UI 证据目录](ui-review/README.md) 与 [Wave D 收口记录](ui-review/wave-d/notes.md)。

- **Wave A · 视觉合同**：冻结 State A/B/C 的 1440×1024 reference、量图表、组件 manifest、mask/zones 与逐 RGB channel `SSIM ≥0.99` 分区门禁；TaskRail 288、Inspector 约 440、Composer 88–94、StatusBar 24 以文档合同为准，ImageGen 边差用 anchor/current rect 表达，不反改实现合同。
- **Wave B · 真实 fixture**：固定 3 workspace / 7 session / 263 event 与 diff/terminal/approval 等状态，经真实 Host、GUI Connection Protocol 与 Desktop projection 消费；数据目录、barrier 与 token 隔离，不向生产 UI 写死演示文案。
- **Wave C · U0–U3 路线**：U1 选定 GPUI `TestAppContext`；U2/U3 选定稳定 AX identifier + 文件 barrier + `screencapture`。GPUI 0.2.2 原生 AX 空树由 Accepted ADR-042 AppKit bridge 补救，真窗口语义 action、focus/value、TabGroup 与可复用 AX tree 已验证。
- **Wave D · State A 闭环**：`seed → serve → Desktop → timeline_stable → AXPress task → 三栏/几何/focus 断言 → ICC→sRGB 无 profile 截图 → zone diff → manifest/checklist` 从零运行两次；主显示器位置固定后两份 `current.png` 字节一致，zone/global 指纹完全一致。规范 State A `reference/current/overlay/diff/mask/checklist` 已成套保存。
- **反证与恢复**：临时把 `SIDEBAR_WIDTH` 288→320，初始/最终 rail 几何硬失败，驱动退出 4；恢复 288 后截图与基线再次字节一致，生产 token 无残留 diff。证据：[repeatability](ui-review/wave-d/repeatability.json) · [drift](ui-review/wave-d/drift-detection.json) · [recovery](ui-review/wave-d/recovery-compare.json)。
- **诚实边界**：R1 只证明合同与门禁可靠，不宣称 UI 已还原。State A 当前 0/9 zone 达到 0.99（global 辅助 SSIM 0.336185），Composer AX group 实测 156px；这些视觉差异转交 R2–R6。真实 IME、性能、完整 VoiceOver 与三状态全组件验收仍属 R7/R8。

收口验证：`scripts/test_ui_wave_d_tools.py` 8/8；`scripts/test_ui_visual_diff.py` 14/14；`pawork-desktop` 定向测试 66/66（Wave C 写入集）；Wave D baseline-1 / baseline-2 / recovery 结构通过，drift 按预期结构失败；未运行全 workspace gate（当前路线未设置）。下一任务转入 R2 Wave A：Window chrome 与根级 surface/token。

## R2 — Window shell 与全局视觉系统（2026-08-27）

R2 三波完成后退出活动任务目录；原任务书正文仍在 [plan/R2-R3-ui-shell-navigation.md](../plan/R2-R3-ui-shell-navigation.md)（R3 已于同日开启 Wave A）。验证事实见 [r2-wave-a](ui-review/r2-wave-a/notes.md)、[r2-wave-b](ui-review/r2-wave-b/notes.md)、[r2-wave-c](ui-review/r2-wave-c/notes.md)。

- **Wave A · Window chrome 与根 token**：F-01 透明 titlebar、F-02 三栏骨架、design/README §2.1 根 token、1440/1080 layout invariant、State A 壳层证据。U1 74/74。
- **Wave B · 窗口状态与 U2 driver**：空态引导、Reconnect 相位、F-13 StatusBar 居中与定稿语序、window_min_size 1080×720、U2 五相位（empty/focus-blur/narrow/restored/collapsed/resumed）。desktop 78/78 + 脚本 25/25；真窗口门禁通过。
- **Wave C · 连接失败重试**：drop-socket Disconnected 重试 + host 停机 ConnectFailed 重试双循环。脚本 42/42；真窗口五相位门禁通过（git_head=b744550）。
- **退出拍板 a（2026-08-27 用户确认）**：R2 以壳层结构门禁为准退出；State A/B 分区像素 SSIM ≥0.99 依赖 F-03/F-04（R3）、F-05（R4）、F-09（R5）、F-12（R6）内容组件，移交 R8 汇总，不阻塞 R2。Wave A 实测 9/9 zone <0.99（0.65–0.81）是预期中间态，不是回归。

收口验证：Wave A/B desktop 定向测试全绿；Wave B/C 真窗口 U2 通过；脚本 unittest 42/42（wave-c 15 + wave-b 17 + wave-d 8）。未运行全 workspace gate。下一任务转入 R3：TaskRail 顶部 F-03 与列表/底部 F-04。

## R3 — TaskRail 与任务导航（2026-08-27–28）

R3 已收口；Wave A/B 证据 [r3-wave-a](ui-review/r3-wave-a/notes.md) 与 [r3-wave-b](ui-review/r3-wave-b/notes.md)。任务书正文仍在 [plan/R2-R3-ui-shell-navigation.md](../plan/R2-R3-ui-shell-navigation.md)。

- **Wave A · TaskRail F-03/F-04**：顶部三行（Pawork 22 / scope 36 / 连接行 Ø10 + 全局 +）、日期桶→项目→44px 任务行、底部「Local」honest-hidden；状态点诚实语义 NeedsInput > Running > 空心灰（无终态绿点）。live `RunChanged` 与 `MessageSent` 乐观登记跨会话维护 `active_runs`；后台审批进出闸门前入账。AX 与 render 共享 `metrics::RAIL_*`。desktop 84/84；脚本 55/55。真窗口 State A taskrail SSIM 0.6941、State C 0.3543（当时登记 ROADMAP §5；2026-08-28 拍板 c 后移交 R8）。
- **Wave B · 导航状态与键盘 / Unread / Blocked**：`SessionLiveStatus::Blocked` live 派生（failed/interrupted，快照清空、Replay 再派生）与独立 unread 通道；断线保留 active/unread/blocked。键盘：Tab 链（AppKit NSEvent monitor，`BLOCK_IS_GLOBAL=1<<28`）、rail ↑/↓、Enter/Space 行级与按钮级激活、菜单打开即接管、Esc 回触发器、cmd-alt 循环（target==active 短路）与 next-needs-attention（NeedsInput > Blocked > Unread）。收口审查补 scope 焦点回退（`pending_scope_focus`）、空态/tooltip ASCII 快捷键（GPUI 默认字体无 ⌘/⌥ 级联）、`on_select_model` enable 门。U2 归档为 Slice 4 的 22 相位（label `r3-wave-b-u2-nav-slice4`，git_head 69d1fb3）；Slice 5 button-enter 相位写入驱动但按用户指示未复跑。Computer Use 真窗口截图见 [r3-wave-b/visual](ui-review/r3-wave-b/visual/)。
- **退出拍板 c（2026-08-28 用户确认）**：R3 以结构门禁为准退出（同 R2 拍板 a 先例）；三状态 TaskRail 分区 SSIM ≥0.99 连同 fixture 演示数据重塑、State C reference tone 归一（设计基准变更，届时需用户批准）移交 R8 终局门禁，条款见 [plan/R7-R8-ui-quality-gates.md](../plan/R7-R8-ui-quality-gates.md) §3。天花板量化分解（与归档 diff-report 一致 0.6941/0.3543）：State A ≈100% fixture 内容形状（标题/时间值已遮，行数/行位/密度/省略形状按合同须对齐，tone 校正上限 0.7490）；State C = tone 差 ≈50% + 内容形状 ≈50%（reference 中位 RGB (0,9,17) 比冻结 token base 0x07121a 更暗，非实现漂移；tone 校正后 0.6885）。遮罩调整方案合同不可行：§0.1 只允许遮「值本身」、禁止空白稀释，taskrail 遮罩已用 16.6%/14.9%（上限 35%），合法微调收益 ≤0.05。fixture 重塑估算 0.5–1 天（seed.json 数据形状 + golden + 约 18 文件断言同步 + 三状态重采集），最佳时机为 R8 重采集前一次完成。

收口验证：Wave A `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 84/84；Wave B 同命令 94/94；脚本 unittest 35/35（wave-b + wave-a + wave-d，bundled Python + Pillow）；U2 Slice 4 22 相位 PASS；Slice 5 button-enter 未复跑；ASCII 空态字形待支持 Computer Use 的模型复拍。未运行全 workspace gate。下一任务：R4 — Workspace、Timeline 与 Agent 状态。

## R4 — Workspace、Timeline 与 Agent 状态（2026-08-28）

R4 已收口；Wave A/B 证据 [r4-wave-a](ui-review/r4-wave-a/notes.md) 与 [r4-wave-b](ui-review/r4-wave-b/notes.md)。任务书正文仍在 [plan/R4-R6-ui-workflows.md](../plan/R4-R6-ui-workflows.md)。

- **Wave A · F-05–F-08**：Workspace Header 骨架常存（branch 仅 GitDiffInfo 诚实源、终态只画 live 可派生状态）；Timeline Top 对齐四合同（跟随态单一表达、滚动事件事实判定贴底，评审 P0 修 handler 内读 ListState 的 BorrowMutError）；消息层级 You/Pawork + 相对时间；连续同 run ToolCall 合组、紧邻终态吸收为 RunSummary（终态判定 = fork_boundary.is_some()）。desktop 107/107；State A 结构门禁 r4a-2 通过。
- **Wave B · Agent 状态 U2 九场景**：Failed 摘要真实原因（live 诚实兜底 "The run failed."）；种子审批决议补广播（WS-3a，仅 ToolCompleted 上 wire）；用户消息乐观回显（WS-4a，local-echo 不进 seen）；entry-compare v2 三重合同；合成终态闸门（WS-5，terminal_reported 去重，cancel 不再谎报 Failed）。评审 P2：合成 seq-0 压回显 → publish_raw 从 2^60 递增自取。P3：早死 run 回显重选消失，登记 desktop Spec §8。app 156 / desktop 110 / 脚本 22；U2 r4b-6 14 相位 + entry-compare 全 PASS；State B shell r4b-shell-1 结构 PASS（composer-height=F-09，R5）。
- **退出拍板 1（2026-08-28 用户确认）**：R4 以结构门禁与 U2 九场景为准退出（同 R2 拍板 a / R3 拍板 c）；State A/B 分区 SSIM ≥0.99 移交 R8 终局门禁，条款见 [plan/R7-R8-ui-quality-gates.md](../plan/R7-R8-ui-quality-gates.md) §3。Wave A 记录值 timeline 0.665 / header-left 0.940 / header-right 0.883 / global 0.648，主因 fixture 演示内容形状差（重塑已在 R3 拍板 c 移交，本条不另开数据任务）。仍开放：live RunChanged 无失败原因 / 无用户消息 wire 事件是否立 ADR。

收口验证：`cargo test -p pawork-app --offline --lib --tests` 156/156；`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 110/110；脚本 unittest 22/22；U2 r4b-6 全 PASS。未运行全 workspace gate。下一任务：R5 — Composer 与运行控制。

## R5 — Composer 与运行控制（2026-08-28–29）

R5 已收口；Wave A/B 证据分别见 [r5-wave-a](ui-review/r5-wave-a/notes.md) 与 [r5-wave-b](ui-review/r5-wave-b/notes.md)，任务书正文仍在 [plan/R4-R6-ui-workflows.md](../plan/R4-R6-ui-workflows.md)（R6 已开启）。

- **Wave A · F-09 Composer 结构**：真窗口常态总高 156→91，进入 88–94 合同；输入区 + footer 两行结构、model/workspace/ContextMeter 与 32×32 Send/Cancel 同槽互换落地；彻除常驻提示行与幽灵 tab stop，Terminal TextInput 参数化解耦；无权威 capability 的 reasoning/附件/队列不画。State A 结构三轮 PASS，desktop 119/119。
- **Wave B · 输入与 U2**：shift/鼠标选择、Copy/Cut/SelectAll、Undo/Redo、overflow scroll、IME composing 闸门、trim 发送、per-session 草稿与 Terminal 解耦落地；两轮评审发现的 P0–P2 均修复，含鼠标/IME 坐标映射根因（`content_bounds` 归一化）。desktop 129/129，Python 40/40，warnings 15 持平，零 wire 变更；U2 九场景 22 份断言全 PASS。
- **退出拍板（2026-08-29）**：用户指令按 ROADMAP 开启下一任务，确认将 State A/B Composer 分区 SSIM ≥0.99 同 R3/R4 先例移交 R8。Wave A 记录值 0.423 / 0.619 只是中间态，不追认为通过；R8 须在 fixture 演示数据重塑后重采 idle/running current 并重跑终局分区门禁。

收口验证：`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 129/129；脚本 unittest 40/40；U2 九场景 22 份断言全 PASS。未运行全 workspace gate。下一任务：R6 Wave A — Inspector/Changes 层级与 Workspace Header ActivityPopover。

## R6 — Inspector、Changes、Terminal 与 Activity（2026-08-29–30）

R6 已收口；Wave A/B 证据分别见 [r6-wave-a](ui-review/r6-wave-a/notes.md) 与 [r6-wave-b](ui-review/r6-wave-b/notes.md)，任务书正文仍在 [plan/R4-R6-ui-workflows.md](../plan/R4-R6-ui-workflows.md)。

- **Wave A · Inspector 层级与 Header Activity**：Inspector 顶层 Changes/Terminal/Resources 与 Files/Summary 二级 strip、默认 Changes、440px 固定栏和 320×320 Header ActivityPopover 收口；只展示权威 Changes 摘要，不伪造 Add tool/Agent capability。render/AX/U1 132/132，Connected State A/B 三相位结构断言全 PASS。macOS 26.6.2 AX 注册 flake 以 AXWindows 回退 + desktop-restart≤3 fail-closed 取证。
- **Wave B · 生命周期、键盘与重连**：Changes latest-session/scope/真实横滚，Terminal 多 workspace 草稿与 snapshot/replay/reconnect，Resources stale/刷新，Inspector 键盘/AX，以及 Host 重启后的 GuiClient request namespace 均在冻结 wire 内完成。审查发现的 terminal 首段串屏与空 diff scope 诊断已最小修复。
- **视觉证据更正**：State B 原 `current.png`/`diff/` 在 Popover 打开前采集，原 0.528/0.573 不能作为 ActivityPopover 分区分数，保留作审计。2026-08-30 以同次运行的正确 `shot-activity-popover.png` 补录 ICC→sRGB current/diff，popover-left/right 为 0.712/0.860；State A Inspector 中间态为 0.614/0.800，均未达到 0.99。
- **退出拍板（2026-08-30 用户确认）**：R6 以结构、交互、定向门禁与审查后最终二进制 U2 为准退出；State A/B Inspector/Activity 分区 SSIM ≥0.99 与 R3–R5 一致移交 R8，所有中间态记录值均不追认为通过。R8 须在 fixture 演示数据重塑后重新采集真正打开的 ActivityPopover 与 Inspector current。

收口验证：`cargo test -p pawork-app --offline --lib --tests` 178/178；`cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders` 144/144；`cargo test -p pawork-client --offline --lib --tests` 41/41；driver unittest 6/6；审查后最终二进制 U2 九场景/19 断言全 PASS；87 文件 Secret 扫描 0 命中。状态回写仅复核归档证据与补录正确 State B 视觉 diff，未重跑 Cargo。未运行全 workspace gate。下一任务：恢复 R7 Wave A VoiceOver/overlay 人工验收。

## 附：默认死表 opt-in 门控（2026-08-30，阶段外任务）

用户要求清理多余测试与门禁以缩短开发期测试/构建耗时。盘点结论：仓库无 CI / 全量门禁可撤；三类关键测试与 UI 波次脚本（被 Spec 与 R8/R11 证据链引用）保留；protocol 测试箱合并仍留 R9。实际收敛三项默认不跑但每次仍被编译的测试箱为 required-features opt-in（沿用 ui-fixture / provider 通道既有模式），默认死表不再编译：

- `pawork-client` `tests/probe.rs`（13 场景 self-test）→ feature `probe-self-test`；`tests/spawn_e2e.rs` → feature `spawn-e2e`。
- `pawork-app` `tests/smoke.rs`（env 门控真实 API 冒烟，`#[ignore]`）→ feature `live-smoke`。

复跑命令见 [client](spec/crates/client.md) / [app](spec/crates/app.md) 包级 Spec §7。验证：默认死表两包全绿且三箱不再出现在编译目标；opt-in 复跑 probe 13 场景绿、spawn_e2e 3 测试绿、smoke 编译通过。R9「probe flake 复查」与 R10 typed-client/headless 矩阵复跑时需显式启用对应 feature。未触碰生产代码与用户未提交改动。
