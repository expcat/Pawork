# Pawork 设计文档

> 本文档是**设计事实源**：包布局与激活映射、冻结契约与「追加不重写」三道保险、各阶段功能设计及其到参照项目的映射、已确认扩展功能族与候选功能、发布策略。
>
> **V3 状态说明（2026-08-19）**：V2（S0–S13）已收官（总结见 [v2-summary.md](v2-summary.md)），当前执行 V3 重构线 R0–R9（[../ROADMAP.md](../ROADMAP.md)）。R1 已收口（ADR-039，2026-08-19）：本文 §2 已重写为 **V3 定稿布局**（21 成员：19 库平铺 `crates/<短名>` + 2 应用 `apps/`）；§4 的阶段映射仍记录 **V2 交付史**（S0–S13，收官时 39 成员），是「功能 ↔ 参照项目」的事实源，不再代表现行包布局。§3 冻结契约在 V3 期间继续有效（R6/R7 的版本化演进除外）。原 S0–S13 阶段任务书已删除，历史见 [v2-summary.md](v2-summary.md) 与 git 历史。
>
> 关联文档：[../ROADMAP.md](../ROADMAP.md)（任务总索引）· [../v3_plan.md](../v3_plan.md)（任务开启编排）· [../plan/](../plan/)（阶段任务书 R0–R9）· [gui-design.md](gui-design.md)（Desktop GUI 设计）· [references.md](references.md)（参照项目手册）· [task-guide.md](task-guide.md)（任务实现规范）· [v2-summary.md](v2-summary.md)（V2 归档总结）· [v1-migration-reference.md](v1-migration-reference.md)（V1 Review 与迁移词典全文）。

---

## 1. 目标与原则

（全文见 [v1-migration-reference.md](v1-migration-reference.md) §2，此处为执行摘要。）

1. 把 V1 的 88 crate / 约 23.6 万行重组为 **40 包 + 3 应用**（`pawork`、`protocol-probe`、S7 起的 `apps/desktop`；独立 Cargo workspace `Pawork_v2/`，2026-08-17 已摊平为仓库根），可独立发布，约 15 个高外部价值包按 W1–W4 波次发布 crates.io（§7）。插件四包目录预留、本轮不激活。
2. **纵向优先**：先交付内置工具真实接线、能在真实仓库完成编码任务的 CLI Coding Agent，再长出最小 Agent GUI，其后按同一窗口增量加面；WASM 插件等扩展生态移出本轮排期（[../ROADMAP.md](../ROADMAP.md) §4）——直接矫正 V1「组件齐全、主干未通电」病灶（[v1-migration-reference.md](v1-migration-reference.md) §1.2）。
3. **架构红线不变**：纯 Rust、CLI 与 Core 同进程同二进制（`pawork` 唯一正式宿主）、GUI 独立进程走 GUI Connection Protocol；canonical domain 纯净；事件可持久化可重放；Secret 不落库不入日志；Engine 无 Provider 名称特例分支；禁止循环依赖（详见 [../AGENTS.md](../AGENTS.md) §2）。
4. **针对 V1 病灶的新增规则**：无消费者不合入（否则 experimental feature + [../ROADMAP.md](../ROADMAP.md) §4 登记）；注册表自动化（依赖图由 `cargo metadata` 派生）；依赖方向执法放宽为「包内模块 + feature 门」。S12 只审查依赖方向与 feature 实态，若需 workspace lint 则作为 finding 另立实现任务。
5. **开发期放宽**：无 L0–L3 分级、无门禁、允许 feature 门控的残缺合入、文档同步降为里程碑级（见 [task-guide.md](task-guide.md) §6）。

---

## 2. 包布局与依赖方向（21 包，ADR-039 定稿）

R1 收口（2026-08-19）后 workspace 定稿为 **21 成员（19 库 + 2 应用）**：19 个库平铺 `crates/<短名>`（目录 = 包名去 `pawork-` 前缀，包名保持 `pawork-` 前缀不变），2 个应用维持 `apps/{pawork,desktop}`。布局决策、合并映射与不合并清单见 [adr/ADR-039](adr/ADR-039-package-layout-and-no-merge-list.md)；V2 时代的激活阶段史（S0–S13 各包何时接入、能力级核对）见 [v2-summary.md](v2-summary.md) 与 [v1-migration-reference.md](v1-migration-reference.md) §3/§4。

| 包 | 目录 | 依赖方向 | 备注 |
| --- | --- | --- | --- |
| `pawork-domain` | `crates/domain` | 无内部依赖 | canonical 纯净红线；含 `provider_api/`（ModelProvider、CanonicalModelRequest、ProviderStreamEvent 13 变体、ProviderError、ResolvedCredential，R1 波 A 自 api 并入）与 `tool_api/`（AgentTool、ToolResult）；事件信封 v1 与契约字节 golden 在本包 tests/ |
| `pawork-protocol` | `crates/protocol` | → domain | GUI 帧 / headless-json / core-api / typegen（检入 `schemas/` 三产物） |
| `pawork-testkit` | `crates/testkit` | → domain | dev-only：MockProvider/MockTool/契约断言 |
| `pawork-policy` | `crates/policy` | → domain | 安全内核；`PolicyDecision`/`ApprovalMode` 冻结契约与红线回归锚 |
| `pawork-exec` | `crates/exec` | 无内部依赖 | process/sandbox/pty；R7 沙箱演进承载 |
| `pawork-tools` | `crates/tools` | → domain、exec、policy、workspace、auth | 八工具 + scheduler + `mcp/`（R1 波 C 并入；rmcp 隔离断言为模块级测试） |
| `pawork-workspace` | `crates/workspace` | → domain、policy | `service/`+`path/`+`file_index/`、`resources/`、`config/`（六层矩阵）、`import/`（原 compat 五来源，R1 波 B 并入） |
| `pawork-storage` | `crates/storage` | → domain | `sqlite/`（Actor+migration 框架）、`session/`（DDL/迁移/export）、`blob/`（PWB1+checkpoint/protected，R1 波 B 三合）；`default = ["session","blob"]`，compaction/checkpoint/protected opt-in |
| `pawork-providers` | `crates/providers` | → domain | `net/`（http/sse/retry）+ `registry/`/`pricing/`/`usage/`/`negotiate/`/`reasoning/`（原 provider-core）+ `channels/`（六通道，feature 门控）（R1 波 B 三合）；core 不依赖 net 降级为模块纪律 + 源扫描测试 |
| `pawork-auth` | `crates/auth` | → domain | Secret 后端/OAuth/脱敏/解析链（Secret 审计边界） |
| `pawork-git` | `crates/git` | → domain、exec | Diff/Status/GitService/GitRunner/HunkStage/worktree/merge（R0 已裁剪） |
| `pawork-engine` | `crates/engine` | → domain（唯一 pawork-* 生产依赖，`tests/domain_only.rs` 断言护航） | tool_loop/session_turn/context/cancel/appender |
| `pawork-workflow` | `crates/workflow` | → domain | plan/task 纯 reducer |
| `pawork-orchestration` | `crates/orchestration` | → domain、control-plane（default-features = false）、git(opt) | supervisor/budget/lifecycle/merge/task_graph/worktree/identity；不依赖 workflow（装配在 app） |
| `pawork-control-plane` | `crates/control-plane` | → domain（rusqlite optional，自开连接） | 控制面 core + `quota/` + `credential/`（lease/pool，R1 波 C 并入）；usage `dedup_key`/audit JSONL golden |
| `pawork-transport` | `crates/transport` | 无内部依赖（帧长度常量与 protocol 对齐，但不依赖该 crate） | local（UDS/named pipe）+ memory |
| `pawork-app` | `crates/app` | 领域宿主依赖 + transport | 装配宿主 + `gui_server/`（GuiServer/ConnectionManager/GuiHost trait，R1 波 D 并入） |
| `pawork-cli` | `crates/cli` | 原 cli 依赖（GuiHost 经 app） | 21 子命令 + `channels/acp/`（AcpHost 四件套，R1 波 D 并入） |
| `pawork-client` | `crates/client` | → domain、protocol、transport | framed 连接面 + `headless/`（原 sdk，R1 波 D 并入）；probe 9 场景为本包 tests/，live 模式 `examples/probe.rs` |
| `pawork`（bin） | `apps/pawork` | → cli | composition root + `redact.rs`（Redactor/RedactingFmtLayer，R1 波 A 自 diagnostics 迁入） |
| `pawork-desktop`（bin） | `apps/desktop` | → client、gpui | 四层 ui/projection/controller/platform；业务依赖仅 pawork-client（deny-list 断言） |

**不合并清单**（ADR-039 D2 固化）：`policy`、`exec`、`auth`、`git`、`engine`、`protocol`、`testkit`、`transport`、`orchestration`、`workflow` 保持独立包。R1 解散的 16 包与 protocol-probe 为**平移**语义（git 历史 + tag `v2-final` 兜底），模块现址见上表备注列。V3 期间不新增包，后续阶段只往既有包加模块；包布局变更须先过 ADR。

---

## 3. 冻结契约与「追加不重写」三道保险

增量式最大的风险是「最小实现长歪，后期推翻重来」。以下三道保险从 S0 起生效。

### 3.1 终局包布局先行

- 现行终局布局为 **21 成员（19 库 + 2 应用）**：19 库平铺 `crates/<短名>`，2 应用 `apps/{pawork,desktop}`（R1 收口定稿，ADR-039 D1）。V2 时期按 [v1-migration-reference.md](v1-migration-reference.md) §3 的 40 包布局逐阶段**激活**（建 crate、迁入/新写最小模块），不新造层级、不临时安置代码，该史见 §4 与 [v2-summary.md](v2-summary.md)；未激活域不预建空 crate。
- 新能力 = 已有包内新模块（V3 不新增包）；**禁止**「先写在 bin 里、以后再抽包」——`crates/cli`、`crates/app`、`crates/engine` 这些终局包从 S0 起就以最小形态存在，后续阶段只往里加模块。
- 包间依赖方向遵守 §2 表与 ADR-039 不合并清单；canonical 纯净红线（`pawork-domain` 不依赖 GUI/SQLite/HTTP/Keychain/Git/具体 Provider）不变。

### 3.2 冻结契约先行（激活即采用 V1 完整形状）

V1 的磁盘/线上契约与核心 trait 是全部后期迁移的兼容性锚点。**每个契约在其激活阶段直接采用 V1 完整形状，宁可字段暂时闲置，也不做「先简后改」**；golden 测试先于消费实现迁移。

| 契约 | V1 事实源 | 激活阶段 | 形状要点 |
| --- | --- | --- | --- |
| Provider 契约 | `provider-api`：`ModelProvider`（`id`/`list_models`/`stream`）、`CanonicalModelRequest`、`ProviderStreamEvent`（13 变体，tag=`type`/content=`data`）、`ModelResponseSummary`、`ResolvedCredential`（Debug 脱敏）、`ProviderError` | S0 | 整包迁移，不裁剪字段；S0 只消费其中一部分 |
| 事件信封 | `agent-events`：`AgentEventEnvelope`（`schema_version = 1`、`event_id/session_id/run_id/sequence/timestamp/parent_event_id/payload`）、`AgentEvent` 32 变体（含 `Diagnostic`） | S1 | 整枚举一次迁入 `domain::events`，serde golden 先行（V1 无独立夹具，S1 波 A 补建）；后期各域事件（Plan/Goal/Task/…）变体已在位 |
| 会话存储 | `session-store`：`session_events` DDL（`UNIQUE(session_id, sequence)`、`CHECK(sequence > 0)`）、append-only 双触发器、`AppendReceipt`、migration 序列（DB `CURRENT_SCHEMA_VERSION = 10`，`messages.branch_id` 为投影附加列，与信封版本 1 相互独立） | S1 | 直接复用 V1 migration 序列全量建库，保证 V1 库文件可打开升级 |
| 工具契约 | `tool-api` 执行面 + `agent-domain` 描述符：`AgentTool`（`descriptor`/`execute`）、`ToolEventSink`、`ToolExecutionContext`（`workspace_id` + 相对 `working_directory`）原在 `pawork-api` `tool` feature（R1 起并入 `pawork-domain::tool_api`）；`ToolDescriptor`（含 `requires_approval`/`read_only`/`allowed_in_untrusted_workspace`）定义在 `pawork-domain`（S0 已迁入，S2 不复制） | S2 | 执行契约整组迁移、零裁剪；descriptor 的审批/只读语义为 S3 审批直接铺路 |
| Policy 契约 | `policy-engine`：`PolicyDecision`（`Allow/Deny/AskUser/AllowWithConstraints`）、`ApprovalPrompt`+`RiskLevel`、`ApprovalMode`（默认 `ReadOnly`） | S3 | 整包迁移（V1 实现成熟，安全红线回归随迁） |
| 配置 schema | `config-service`：TOML、`ConfigTier`（Builtin<Global<Profile<Workspace<Session<Run）、`PaworkConfig`/`ProviderConfig{id, base_url}`（**无 api_key 字段**） | S0 最小 / S9 完整 | S0 只实现 Builtin+Global+Workspace 三层读取与合并，但 schema 字段与文件位置照抄 V1；Profile/Session/Run 层 S9 补齐 |
| 引擎语义 | `agent-engine`：审批经 `ApprovalResolver` await（`ToolApprovalRequested/Responded` 事件对）、`CancelHandle`+`CancelReason`、`LoopContext` 工具执行注入点 | S2–S3 | engine 实现增量长出，但事件语义、审批/取消语义与 V1 对齐（对应模块迁移时以 V1 测试为准绳） |
| blob 格式 | `PWB1` + protected AEAD 边界（ADR-032） | S8 | golden 先行 |
| GUI 协议 | `gui-protocol` 帧（ADR-036）、headless-json、core-api | S7 最小激活 / S10 收口 / S13-F13 minor 1.2 | 激活即用 V1 完整形状；S7 只消费对话子集。S1 起 `--json` 标 **unstable**，S10 对齐正式 headless。10a 波 A 已补齐 golden/typegen/`schemas/`；S13-F13 将 `API_VERSION` 升为 1.2（`RunStart.provider` 可选字段，`SUPPORTED` 含 1.0/1.1/1.2）；映射表见 [headless-json-migration.md](headless-json-migration.md)；CLI 切输出在收口 |
| 控制面契约 | usage `dedup_key`、audit JSONL | S11 | golden 先行 |
| 缓存注解（新增，已确认 D4） | `CanonicalModelRequest` 缓存策略枚举（`Off/Auto/Explicit{retention}`）+ 前缀分段标注；`ModelResponseSummary`/usage 增 `cache_read`/`cache_write` | S2 占位 / S5 分段 / S6 全量 | **附加式**可选字段，serde 向后兼容；golden 先行；详见 §5（F5）与 [research/multi-account-quota-proposals.md](research/multi-account-quota-proposals.md) F5-B |

### 3.3 迁移词典与「无消费者不合入」

- [v1-migration-reference.md](v1-migration-reference.md) §4.1 映射总表仍是 V1→V2 的**唯一迁移词典**（合并来源、行数、关键动作）；归档的旧里程碑文档（[../plan/archive/](../plan/archive/README.md)）保留包级迁移细则，各阶段计划直接引用。
- 「无消费者不合入」在增量计划下天然成立：每个包在**被 `pawork` 装配链真实消费的那个阶段**才激活。仍需 feature 门控合入的（如 S5 的 `compaction`），照旧显式登记。
- 冻结候审清单不变（quota 远端适配器约 8k 行、browser-computer-runtime、tool_search，见 [v1-migration-reference.md](v1-migration-reference.md) §4.4），留在 V1 目录按需激活。

---

## 4. 各阶段功能设计与参照项目映射

> 本节按 V2 阶段（S0–S13，均已交付）记录**用户可见功能**与参照映射，是「功能 ↔ 参照项目」的持续事实源；原逐阶段任务书已删除，交付细节见 [v2-summary.md](v2-summary.md) §2。「参照」列给出该功能在参照项目中的对应实现与资料入口——项目背景见 [references.md](references.md)，**参照项目 → 功能规划**的反向分类见同文 §6，机制细节见 [research/](research/) 调研文档（记作 research §N，指 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md)）。V1 资产来源统一见 [v1-migration-reference.md](v1-migration-reference.md) §4.1，不在表内重复。

### S0 最小可对话 CLI

| 功能 | 参照 |
| --- | --- |
| `pawork chat` 流式多轮 REPL、Ctrl-C 取消当轮 | [Codex CLI](https://developers.openai.com/codex)；OpenCode/Pi 的终端交互语义（仅对标行为——Pawork 无 TUI，见 §6.1 红线排除） |
| `pawork models` 模型目录 | OpenCode 外置 [models.dev](https://models.dev) 注册表 vs Pi 自维护内置目录（research §2.2 对比表）——Pawork 走 registry + config 覆盖（S5 完整化） |
| TOML 配置 + env key（配置**无 api_key 字段**） | OpenCode `opencode.json` 与 `auth.json` 分离（research §2.1）；Pi `auth.json`（0600）与 `!command`/`$ENV` 插值（research §2.2） |
| openai-compatible 适配器（可配 `base_url`） | GLM Coding Plan / OpenCode Go / 自建网关（opencodex、[Codex Router](https://github.com/duolahypercho/codex-router) 等）均为此形态；通道端点见 [task-guide.md](task-guide.md) §5 |
| 可读错误呈现（401/429/超时/断网） | OpenCode ≤5 次重试、遵循 Retry-After（research §2.1）；Pi agent 层退避（research §2.2）。S0 只做呈现；自动冷却/换号是 S11 F3 的范畴 |

### S1 会话持久化与恢复

| 功能 | 参照 |
| --- | --- |
| 事件流落盘（`AgentEventEnvelope` + append-only 存储） | V1 冻结契约（§3.2）；最接近的外部同形：DeepSeek Harness 仅追加 `SessionEvent` 日志（模型可见输入必须可从日志重建，fork/resume/Trajectory 同源，[references.md](references.md) §2.4）；相邻实现：Pi JSONL 树形 session（`id/parentId` 原地分支，research §2.2）、OpenCode 消息级 SQLite 落库（research §2.1） |
| `pawork sessions list/show`、`--resume` 续聊 | [Codex](https://developers.openai.com/codex) sessions/resume；OpenCode/Pi 会话恢复；DeepSeek Harness 从同一事件流 resume |
| `pawork run`（非交互单次）+ `--json` JSONL 事件流（unstable） | Codex exec / headless 输出形态；DeepSeek Harness `dsh-headless` + JSONL session；S10 对齐正式 headless 协议；S7 GUI 不走这条输出 |

### S2 Agent Loop 与只读工具

| 功能 | 参照 |
| --- | --- |
| 只读四工具 read_file/list_directory/search_text/find_files | [OpenCode](https://opencode.ai/docs/) 内置工具族；Codex 工具面 |
| 引擎多轮工具循环（每 run 轮数上限防失控） | OpenCode agent `steps` 上限（research §2.1）；协议中立红线：工具映射在 adapter 侧完成，engine 零厂商分支 |
| OpenAI tools / Anthropic tool_use 双协议 | OpenAI 与 Anthropic 官方 API（缓存与协议文档入口见 [references.md](references.md) §4）；Pi `anthropic-messages` 实现（research §2.2） |
| workspace roots + `workspace_id + relative_path` 输入红线 | V1 tool-api 类型化路径红线；OpenCode permission 边界（[agents 文档](https://opencode.ai/docs/agents/)） |
| MockProvider 确定性测试 | 工程实践（testkit） |
| **已确认待并入**：F5 canonical 缓存注解占位（§5 G5，契约见 §3.2 末行） | — |

### S3 写入工具与审批

| 功能 | 参照 |
| --- | --- |
| write_file/edit_file/apply_patch 写三件 | OpenCode edit/write/patch 工具；Codex apply_patch |
| 终端审批（一次/本运行/拒绝）+ `--approval-mode` 六档（默认 ReadOnly） | [Codex approval modes](https://developers.openai.com/codex)；OpenCode `permission`（read/edit/bash/task 每项 allow/ask/deny，research §2.1）；DeepSeek Harness 把 `sandbox/mode` 与 `approval/policy` 做成独立 knob，再经 permission preset 捆绑（[权限预设](https://deepseek-harness.github.io/deepseek-harness/en/reference/subsystems/permission-presets)）；V1 policy-engine 契约（§3.2） |
| 未信任 workspace 强制询问 | Pi Project Trust（[earendil-works/pi](https://github.com/earendil-works/pi)） |
| 路径越界/symlink/TOCTOU 红线 + 提示注入回归 | V1 安全红线资产（policy 整包随迁） |

### S4 命令执行与沙箱

| 功能 | 参照 |
| --- | --- |
| run_command + 沙箱（AppContainer/Landlock/Seatbelt）+ fail-closed（ADR-031：**可观测回退**，不是拒跑；CLI/GUI 必须展示 fallback） | [Codex](https://github.com/openai/codex) sandbox（Landlock/Seatbelt 路线）；DeepSeek Harness `ctx.sandbox` 与审批分轨（`workspace-write` / `danger-full-access`）；V1 exec 链（Windows Job Object + AppContainer，Rust 生态稀缺资产，发布主打包）；[ADR-031](../../Pawork_v1/docs/adr/ADR-031-sandbox-backend-architecture.md)（归档）· [ADR-037](adr/ADR-037-s13-wave-b-contracts.md) |
| shell 风险分类 → 审批（Dangerous 必询） | V1 policy `shell` 分类；OpenCode `permission.bash` 语义 |
| 取消 = 清理整棵进程树 | V1 `cancel.rs` + 进程树管理（Job Object/进程组） |
| 输出截断 + 完整输出落工件 | 上下文预算纪律（S5 铺垫、S8 artifact 接管）；对照 research §5.3 前缀稳定技巧 |

### S5 上下文预算与用量

| 功能 | 参照 |
| --- | --- |
| 上下文预算（软限压缩 / 硬限截断）+ `/compact` 手动触发 | OpenCode context overflow 自动 compaction（research §2.1）；**compaction=重写前缀=缓存全失效**的折中纪律（research §5.3）；Codex Router 可选旧工具结果老化与外部模型 continuation 摘要（宿主仍拥有 compact，路由器只翻译） |
| token 与费用统计（micros 定价、无定价不编造） | OpenCode 消息级 cost/tokens 落库（research §2.1）；Pi footer 实时命中率与成本（research §2.2）；LiteLLM 缓存差价计费（research §4.2） |
| 模型 registry（context window / 定价 / 别名） | [models.dev](https://models.dev)（OpenCode 路线）；Pi `models-store.json` + `models.json` 扩展（research §2.2） |
| **已确认待并入**：F5 前缀稳定性分段产出、缓存用量并入统计（§5 G5） | — |

### S6 多 Provider 与认证

| 功能 | 参照 |
| --- | --- |
| 六条首发通道：ChatGPT OAuth、xAI Grok OAuth、Z.AI GLM Coding Plan、OpenCode Go、Qwen Token Plan、DeepSeek；其它厂商延期 | 各厂商官方 API；范围与 credential/transport 冻结见任务书。端点/凭证形态对照：[Codex Router](https://github.com/duolahypercho/codex-router) 注册表（zai-coding / opencode-go / qwen-plan / deepseek / grok-oauth） |
| ChatGPT/xAI 共用 Responses transport；xAI 与 API-key 混合通道按模型 capability 选 Chat/Responses | canonical 保持 provider-neutral，Engine 不按厂商名分支 |
| `auth.json` 文件凭证 + `pawork auth` 子命令 | 形态对齐 Codex CLI；Pawork 额外锁定 0600、跨进程 write/refresh 锁、独立临时文件 + rename 原子写、损坏 fail-closed、掩码展示与全链日志脱敏。env 仅作 headless/CI fallback |
| ChatGPT/xAI OAuth（PKCE/Device/refresh/callback） | Codex Sign in with ChatGPT；xAI 登录兼容性需真实账号验收；OAuth client secret 不进入 adapter/仓库 |
| REPL `/model` `/provider` 切换（事件流记录变更） | OpenCode `/models` 切换 + transform 归一化历史（research §2.1）；Pi 跨厂商 handoff 一等能力（research §2.2） |
| Z.AI GLM Coding Plan 端点预设 | 国际站 Coding Plan 专属端点 `https://api.z.ai/api/coding/paas/v4`；中国区旧测试通道继续显式配置，不作为首发默认值 |
| **已确认待并入**：plan 凭证 kind（D2）、adapter 缓存映射 + registry 能力表、F2-B 被动配额信号 per-adapter 登记、首个缓存命中测试 ≥95%（§5） | — |

### S7 最小 Agent GUI（[设计](gui-design.md)）

| 功能 | 参照 |
| --- | --- |
| 先锁定最小 Agent 信息架构，再实现本机单窗口 | [gui-design.md](gui-design.md)；Codex Desktop 主对话壳；OpenCode Desktop/Web 的流式+工具行；DeepSeek Harness Web UI 的 Trajectory / 工具+审批同对话（默认壳不吸收）；根仓 [desktop-gui.md](../../Pawork_v1/docs/features/desktop-gui.md) 四层，**不**搬 P19 全量 Surface |
| `pawork gui serve` + GPUI Desktop：会话 / Timeline / Composer / 取消 / 模型切换 / 时间线内审批 | 独立进程 + GUI Connection Protocol（ADR-022/023/035）；S7 只做单客户端本机 |
| 协议帧完整形状、只消费对话子集 | V1 gui-protocol（ADR-036）；`--json` 仍 unstable，正式 headless 收口在 S10 |

### S8 Git、Diff 与 Checkpoint

| 功能 | 参照 |
| --- | --- |
| `pawork diff` 结构化 diff（分页、CRLF/中文文件名） | V1 diff-service（unified diff 状态机 parser）；IDE in-place review 为候选形态（§6.5 D1） |
| 写前 checkpoint + `pawork rollback` | OpenCode `/undo` `/redo`（turn 级、经 Git——与 Pawork Run/Tool 级快照的粒度对比见 §6.2 A3）；V1 checkpoint-service |
| git 状态感知（status/branch/worktree）+ 注入防护 | V1 git-service（`validate_position_arg` 等防御随迁） |
| 审批 UX 升级为 diff 预览 | S3 预留升级点的兑现 |

### S9 MCP、资源与兼容导入

| 功能 | 参照 |
| --- | --- |
| MCP client（rmcp 收口）+ 与内置工具共存注册 | [MCP 官方](https://modelcontextprotocol.io)；OpenCode/Codex/Claude Code/DeepSeek Harness 均支持 MCP；「Pawork 作为 MCP server」为候选反向形态（§6.3 B7） |
| AGENTS.md / Skills / profiles 加载注入 | [AGENTS.md 开放约定](https://agents.md)；OpenCode rules、Codex AGENTS.md、Claude Code 同类机制；DeepSeek Harness `tool-skill` + agent preset；Skills 对标 Claude/Cursor 的 SKILL.md 机制 |
| `@file` 引用 + file-index 模糊补全 | 各家 `@` 语义；OpenCode References（工作区外引用）为候选扩展（§6.3 B4） |
| 一键导入本机 Claude/Codex/Grok/Cursor/Pi 配置（只读） | 各工具本机配置布局；账户/端点导入源（G6）：cc-switch SQLite SSOT（research §3.2）、CLIProxyAPI auth-dir（research §3.3）、opencodex config（research §3.1）、Codex Router 托管 `config.toml` 块 + `~/.codex/codex-router` 状态目录（[references.md](references.md) §3.2） |
| config 完整六层 + Profile | V1 config-service 层级合并引擎 |
| **已确认待并入**：G6 账户/端点导入源、F4 Agent Profile 绑定字段随 profiles 契约定型（§5） | — |

### S10 服务化与客户端补齐

| 功能 | 参照 |
| --- | --- |
| `pawork headless --json-stdio` + SDK 编程驱动 | [Codex](https://developers.openai.com/codex) TS/Python SDK 与 app-server；OpenCode SDK/serve（[opencode.ai/docs](https://opencode.ai/docs/)）；Pi SDK `createAgentSession()`（research §2.2）；DeepSeek Harness headless + [Python SDK](https://deepseek-harness.github.io/deepseek-harness/en/guide/python-sdk)。`--json` 对照见 [headless-json-migration.md](headless-json-migration.md) |
| `gui serve` 从 S7 单客户端升级为多客户端 + 断线 Replay + 慢客户端隔离 | V1 gui-server 资产；Desktop 增量见 [gui-design.md](gui-design.md) §5 |
| `pawork acp serve` 接入 ACP 编辑器 | [Agent Client Protocol](https://github.com/zed-industries/agent-client-protocol)（Zed 生态） |
| 会话分支 / `pawork session fork`（任意消息处分叉） | Pi session tree/clone（JSONL 树形，research §2.2）；OpenCode 子 session（research §2.1 task 工具）；DeepSeek Harness `ctx.sessions.fork`（从同一 `SessionEvent` 流切边界） |
| `pawork service install/start/stop` + 运维子命令（status/watch/shutdown/doctor） | V1 六运行模式（外部无直接对标） |
| PTY 交互式命令 + GUI Terminal | V1 pty-service（PTY 重连语义）；DeepSeek Harness `tool-terminal` + 持久 bash |

### S11 工作流、多 Agent 与控制面

| 功能 | 参照 |
| --- | --- |
| Plan 审批 gate（未批准整版拦截 turn；host 在 `run_session` 前校验，无 plan 放行） | V1 plan-service；相邻机制：OpenCode question/todowrite、DeepSeek Harness planning / `tool-todo` / `ctx.goals` 为模型侧轻量形态（候选 §6.3 B2/B3/B9） |
| 多 Agent 编排（spawn/registry/cancel-tree/recovery/budget-gate） | OpenCode `task` 子代理 + 权限派生 + `subagent_depth`（research §2.1）；Pi「核心不内置子代理」哲学与 extension 自建（research §2.2）；DeepSeek Harness `tool-subagent` + workflows；CCR in-band 标签为**明确不采纳**的反例（F4-C，research §4.1） |
| 子 Agent 声明式 provider/model/账户绑定 + 预算分配（F4） | opencode `agent.model` 声明式绑定（research §2.1）；Codex Router 仅 registry-proven 模型可作 v2 spawn（本地设置不能提升未验证模型）；方案 [research/multi-account-quota-proposals.md](research/multi-account-quota-proposals.md) F4-A+B |
| 多账户池 / 租约 / 路由 / 会话-账户亲和（F1/F3） | opencodex 账户池 + 三窗口配额 + thread affinity（research §3.1）；CLIProxyAPI RR/加权/fill-first + 冷却 + session-affinity（research §3.3）；claude-relay-service 内容 hash sticky（research §4.4）；Codex Router 仅额度耗尽换**模型**（窄 402/长 429，非账户池 sticky，[references.md](references.md) §3.2）；V1 provider-control 资产对照（research §7） |
| 额度感知与预算 gate（F2）+ `pawork usage` | opencodex 主动配额窗口探测（research §3.1）；LiteLLM 层级预算（research §4.2）；Codex Router 托盘用量 + 只信提供商复位窗口（上限 6h）；V1 quota-service/usage-ledger |
| audit / tenant 控制面 | LiteLLM org/team/user/key 层级（research §4.2）；V1 `dedup_key`/audit JSONL 冻结契约（§3.2） |
| 评审（re-anchor/resolution）与记忆抽象 | V1 review-engine / memory-service（memory 无 EmbeddingProvider 则 experimental 登记） |
| **已确认待并入**：F1–F4 全部 + 命中测试补全场景（§5） | — |

### S12 全项目 Code Review 与整改拆分

| 功能 | 参照 |
| --- | --- |
| 全包与跨包接口只读审查：安全、Bug、持久化/并发、性能、维护性、需求追踪与假完成 | 工程审查，无外部功能对标；按 CR-01～CR-09 独立产出 finding（报告存 [reviews/s12/](reviews/s12/)），Confirmed 项经 S13 三波整改收口（见 [v2-summary.md](v2-summary.md) §5–§6）；该阶段不实现、不测试、不发布 |
| **已确认待并入**：缓存命中 ≥99% 纳入 Release Gate（§5；[research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) §1.3） | — |

---

## 5. 已确认扩展功能族：多账户额度、切换、子 Agent 路由与输入缓存（G1–G7）

> 2026-08-14 调研并经用户确认（决策原则：**减少实现复杂度、优先缓存命中**；决策记录 D1–D8 见 [research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md)）。调研全文见 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md)；分功能方案（F1–F6，**已确认**）见 [research/multi-account-quota-proposals.md](research/multi-account-quota-proposals.md)。
>
> 对照来源（第二批调研）：opencodex（lidge-jun/opencodex）、cc-switch（farion1231/cc-switch）、CLIProxyAPI（router-for-me）、claude-code-router、LiteLLM、new-api、claude-relay-service、gpt-load、claude-code-hub 等，以及 OpenCode/Pi 在多账户与缓存维度的补充调研（项目手册见 [references.md](references.md) §3）。2026-08-18 补入 Codex Router（duolahypercho/codex-router）：凭证隔离的多客户端本地路由器，不作账户池主参照。同日参照表二次清理：gpt-load、claude-code-hub 已移出手册（机制调研留档 [research/multi-account-quota-reference.md](research/multi-account-quota-reference.md) §4.5 / §4.6），本节结论不受影响。

| ID | 功能 | 来源参照 | 说明 | 优先级 | 落点 |
| --- | --- | --- | --- | --- | --- |
| G1 | 同 Provider 多账户池与订阅 plan 凭证 | opencodex 账户池、CLIProxyAPI auth-dir；OpenCode/Pi 多账户缺位（差异化机会） | 激活 V1 provider-control 账户层（ProviderAccount/CredentialLease）+ 新增订阅 plan OAuth 凭证 kind + 扩展 `auth.json` 多账户命名（0600、原子写、损坏 fail-closed）+ `pawork accounts` CLI | P1 | S6 铺垫 / S11 主体（方案 F1-B） |
| G2 | 额度窗口跟踪与预算 gate 增强 | opencodex 5h/周/30d 窗口探测、CLIProxyAPI-Plus 阈值停用、litellm 层级预算 | LocalLedger 派生（已规划）+ 响应头/错误体被动配额信号捕获归一为 QuotaSnapshot；远端适配器与 WebScrape 保持冻结候审 | P1 | S11（方案 F2-A+B） |
| G3 | 缓存感知的会话-账户亲和路由 | claude-relay-service sticky session、CLIProxyAPI session-affinity、opencodex thread affinity | SessionBinding 亲和默认开 + 新会话再平衡 + 新增「配额余量优先」路由策略 + 分类错误 rebind；请求级轮换不作默认 | P1 | S11（方案 F3-B） |
| G4 | 子 Agent 声明式 provider/model/账户绑定 | opencode agent.model + 权限派生；CCR 子代理标签（反例，不采纳）；opencodex 模型即子代理 | Agent Profile/spawn 参数声明绑定 → RouteContext → provider-control 选账户；默认继承父绑定、显式覆盖；预算经 budget-gate 分配 | P1 | S9 profile 铺垫 / S11（方案 F4-A+B） |
| G5 | canonical 输入缓存策略控制 | Anthropic cache_control、OpenAI prompt_cache_key、pi/opencode/Claude Code 断点收敛实践 | cache 注解（canonical，无厂商字段）+ registry 缓存能力表 + adapter 断点/亲和键映射 + 缓存用量入账与命中率观测 + compaction 联动 | P1 | S2 占位 / S5 分段 / S6 全量（方案 F5-B） |
| G6 | 账户/端点配置导入 | cc-switch SQLite SSOT、CLIProxyAPI auth-dir、opencodex config、Codex Router 托管 config 块、Claude/Codex 官方布局 | `pawork-workspace::import`（原 pawork-compat）增加账户与端点只读导入源，secret 直接转存 Pawork auth 文件，不落仓库或中间文件 | P2 | S9（方案 F1 附属） |
| G7 | 对外账户池网关模式 | opencodex / CLIProxyAPI / Codex Router 网关形态 | 近期不内建：以 openai-compatible 上游接外部网关 + 对内账户池；长期按需评估 channels 扩展 feature | P3 | 暂不排期（方案 F6，决策项；登记于 [../ROADMAP.md](../ROADMAP.md) §4） |

**状态**：G1–G6 已确认、待由独立任务并入对应 `plan/S*.md`（任务书见 [research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) §4，已登记于 [../ROADMAP.md](../ROADMAP.md) §3.2；按 ROADMAP §6 状态回写约定执行）；G7 维持不做。其中 G5 涉及冻结契约的附加式字段扩展（CanonicalModelRequest/ModelResponseSummary），须遵守 §3.2 golden 先行原则。配套工作约定（执行期凭证 fail-closed / 少测试无门禁 / 缓存命中 95-97-99 目标）见 [research/multi-account-quota-plan-merge.md](research/multi-account-quota-plan-merge.md) §1 与 [task-guide.md](task-guide.md)。

---

## 6. 候选功能对照（未排期；对照 OpenCode / Pi / Codex / DeepSeek Harness）

> 本节先于 2026-08-14 对照 OpenCode / Pi / Codex 的公开功能面，再于 2026-08-17 补入 DeepSeek Harness，与 Pawork V2 S0–S12 已规划范围对照后识别**尚未规划**的功能缺口。每项标注来源、是否违反架构红线、建议优先级（P0 最高）。已在 S0–S12 覆盖或冻结候审的不在此列。四家项目的背景与功能全貌见 [references.md](references.md) §2。候选纳入排期的流程见 [../ROADMAP.md](../ROADMAP.md) §3.3。

### 6.1 架构红线排除项（不实现）

以下功能因违反 Pawork 架构红线（[ADR-001](../../Pawork_v1/docs/adr/ADR-001-pure-rust-core.md) 纯 Rust、[ADR-019](../../Pawork_v1/docs/adr/ADR-019-no-tui.md) 无 TUI）**不纳入路线图**，仅记录排除理由以备回溯。

| 功能 | 来源 | 排除理由 |
| --- | --- | --- |
| 交互式全屏 TUI（themes/keybinds/sounds/Ctrl+G 编辑器） | OpenCode / Pi | ADR-019 明确不实现 TUI；Pawork 以 CLI 交互模式 + GPUI Desktop 为用户界面 |
| JS/TS 插件运行时（Bun/Node 扩展、hot-reload、`tool.execute.before/after` JS hooks） | OpenCode / Pi / DeepSeek Harness（Cordis「一切皆插件」） | 纯 Rust 红线（ADR-001）；若未来做代码插件，只评估 WASM + in-process hooks（当前整族待决策，见 ROADMAP §4） |
| npm 生态传输（`@opencode-ai/sdk`、npm 插件安装、Bun/Node runtime） | OpenCode / Pi / DeepSeek Harness（`@deepseek-ai/dsh`、pnpm 插件） | 同上；即使未来做插件也不走 npm |

### 6.2 CLI 交互与命令体验

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| A1 | 自定义 slash 命令 / Prompt Templates | OpenCode `/commands`、Pi Prompt Templates | 用户定义 Markdown prompt 片段 + `$ARGUMENTS`/`{{var}}` 变量替换，作为 `/name` 命令调用；可绑定 agent/model/subtask。S0 CLI 只有内置命令，S9 resources 有 profiles 但无此轻量 prompt-snippet 机制 | P1 |
| A2 | `pawork init` AGENTS.md 生成器 | OpenCode `/init` | 扫描仓库结构 → 交互式问答 → 生成/更新 AGENTS.md。S9 只 *加载* AGENTS.md，不生成 | P1 |
| A3 | Turn 级 undo/redo | OpenCode `/undo` `/redo` | 回退上一轮用户消息 *及其关联文件改动*（经 Git）。区别于 S8 的 Run 级 checkpoint/rollback——粒度是「对话轮次」而非「整个 Run」或「单个 Tool Call」 | P2 |
| A4 | 写后自动格式化（Post-edit formatters） | OpenCode | write/edit/apply_patch 成功后可选自动跑 `cargo fmt` / `prettier` / `gofmt` / biome 等；可在配置中按语言/工具开关 | P2 |

### 6.3 内置工具与上下文扩展

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| B1 | webfetch + websearch 内置工具 | OpenCode、Codex `--search`、DeepSeek Harness `tool-web` | 网页抓取（给定 URL → markdown）与网络搜索（关键词 → 结果摘要）；S2–S4 工具集只有 read/list/search/find/write/edit/apply_patch/run_command，无网络工具 | P1 |
| B2 | question 工具（模型侧多选问答） | OpenCode、DeepSeek Harness `tool-ask-user` | 模型在循环中主动调用结构化多选问题向用户提问，阻塞等待回答；区别于 policy 审批——这是模型侧信息获取，不是权限请求 | P2 |
| B3 | todowrite 工具（模型侧任务清单） | OpenCode `todowrite`、DeepSeek Harness `tool-todo` | 模型在循环中维护结构化任务清单（增删改状态），作为 tool 调用；区别于 S11 plan 域（plan 是审阅+审批工作流，todowrite 是模型自管理的轻量 checklist） | P2 |
| B4 | References（工作区外引用） | OpenCode | 将额外本地目录或克隆的 Git 仓库注册为 `@alias` 上下文源，模型可引用其文件。S9 `@file` 只在当前工作区 roots 内 | P2 |
| B5 | 图片输入与多模态 | Codex `--image` / `-i` | CLI 接受图片文件路径或 stdin 粘贴，作为 image content part 发送给 Provider。V1 P6-6 已实现；V2 S0–S12 未显式列为用户可见能力 | P1 |
| B6 | 图片生成工具 | Codex `$imagegen` | 作为内置工具或 skill，让模型在编码循环中生成图片（图标、mockup、diagram）。需接入 image generation Provider | P3 |
| B7 | `pawork mcp-server`（作为 MCP Server） | Codex `codex mcp-server` | Pawork 自身作为 MCP Server 暴露 `pawork` / `pawork-reply` 工具，让其他 MCP Client（IDE、其他 Agent）驱动 Pawork 会话。S9 只做 MCP Client | P2 |
| B8 | Code Mode / 单轮组合多步工具 | DeepSeek Harness PTC（Code Mode SDK） | 模型用一段程序在单轮内组合多步工具，减少往返。对标的是「少轮次组合」能力，**不得**引入 JS/TS runtime；若落地需另选 Rust/WASM 或结构化 DSL | P2 |
| B9 | 会话级 Goals（目标对象） | DeepSeek Harness `ctx.goals` | 同一会话内维护可续跑的目标对象，经 `agent/*` 事件推进。区别于 B3 checklist 与 S11 plan gate——Goals 是会话级目标状态，不是审批工作流 | P2 |

### 6.4 扩展生态

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| C1 | 能力包打包与 git 分发 | Pi Packages | 将 skills + prompt templates + hooks + themes 打包为单一 manifest，支持 git 仓库安装/分享。与待决策的 WASM 插件市场分开；这是更轻量的「资源包」分发（不含 JS/TS 代码） | P2 |
| C2 | 用户级 memories（`/memories`） | Codex local memories | 跨会话的用户级记忆存储 + `/memories` 管理命令（增删查）。区别于 S11 实验性 embedding memory——这是用户显式管理的轻量 preferences/facts 存储，不需要 embedding Provider | P2 |
| C3 | 连接器目录（Connector directory） | Codex plugins | 预置 MCP connector 目录（Gmail / Drive / Slack / Calendar / GitHub 等），一键安装 + OAuth 配置 UX。S9 可外接任意 MCP server，但没有预置目录和一键安装体验 | P2 |
| C4 | LSP 自动安装矩阵 + diagnostics 反馈 | OpenCode lsp | 内置常用语言服务器自动发现与安装，并把 diagnostics 反馈给模型。基础 LSP 工具化亦待决策（ROADMAP §4），本项是更重的自动安装矩阵 | P3 |

### 6.5 集成与分发

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| D1 | 第一方 IDE 扩展 | OpenCode、Codex | VS Code / Cursor / Windsurf / JetBrains 扩展，经 ACP 或 GUI Connection Protocol 连接 `pawork`（不嵌入 Core）；支持 open-file/selection context、in-place review。S10 有 ACP 协议但无第一方扩展产品 | P1 |
| D2 | GitHub / GitLab CI bot | OpenCode `/opencode`、Codex `@codex review` | 在 issue/PR 评论中触发 Pawork（`/pawork`），自动 triage / implement / open PR，在 CI runner 上执行。S11 review 有 Forge trait 但无平台 bot | P2 |
| D3 | 会话公开分享 | OpenCode `/share`、Pi session 分享 | 生成可分享的只读会话链接或导出（HTML/JSON/gist），支持 manual/auto/disabled 模式 | P2 |
| D4 | Web UI 浏览器客户端 | OpenCode `opencode web`、DeepSeek Harness `dsh web` | 作为 GUI Connection Protocol 的 Web client（本地 web app、LAN bind、basic-auth），与 Desktop client 并列。S7 起有本机 GPUI，S10 补齐 gui-server，无 Web client | P2 |
| D5 | 自更新与多渠道安装器 | OpenCode `opencode upgrade`、Codex installers | `pawork upgrade` 自更新命令 + 多渠道安装器（Homebrew / Scoop / Winget / curl / cargo install）。当前 S0–S12 无发布或运行时自更新流程 | P2 |
| D6 | Cloud 执行环境 | Codex Cloud | 隔离的远程执行环境，支持并行任务、结果本地应用（`pawork cloud`）。需 remote transport + 隔离沙箱 + 任务编排 | P3 |
| D7 | Slack / Linear 等 chat 平台集成 | Codex `@Codex` in Slack/Linear | 作为 S10 channels 的扩展 feature（Slack / Linear adapter），将 chat 平台消息映射到 Pawork 会话 | P3 |
| D8 | 订阅登录（plan credits 认证） | Pi `/login`、Codex SiwC | ChatGPT 与 xAI 的订阅 OAuth 已并入 S6 首发范围；Claude Pro/Max、GitHub Copilot 等其它 plan 登录仍为候选。§5 G1（F1-B）继续负责后续多账户/订阅凭证抽象 | P2 |

### 6.6 企业与安全

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| E1 | Enterprise SSO + 组织级集中配置 | OpenCode Enterprise | 企业 SSO 登录 + 组织级集中配置下发（model allowlist、workspace roles、internal gateway）。S11 control-plane 是本地 tenant/usage/quota，不含 org SSO | P3 |
| E2 | Bedrock / Vertex 作为显式模型源 | Codex Bedrock、Pi providers、DeepSeek Harness LLM 适配 | AWS Bedrock / GCP Vertex AI 作为模型接入端点；不在 S6 六条首发通道内，按后续需求排期 | P2 |

### 6.7 运维与产品体验

| ID | 功能 | 来源 | 说明 | 优先级 |
| --- | --- | --- | --- | --- |
| F1 | 版本自检 + 遥测 + 离线模式 | Pi telemetry | 启动时检查最新版本（可选匿名遥测 ping，opt-out），`--offline` / 环境变量禁用所有启动时网络请求。当前 S0–S12 不覆盖运行时产品运维 | P3 |

### 6.8 落地建议

上表共 **30 项**候选功能（排除 3 项架构红线排除项），按优先级分布：

- **P1（7 项）**：自定义命令、AGENTS.md 生成器、webfetch/websearch、图片输入、IDE 扩展——建议在 S2–S9 主干阶段择机并行追加（工具类在 S2/S4，CLI 体验类在 S0/S10，GUI 体验跟 S7 壳走）。
- **P2（16 项）**：核心功能补全，建议在对应阶段（S9 resources/MCP、S10 serve/clients、S11 workflow）作为增强项纳入；原 S10 扩展生态类改走 ROADMAP §4 待决策，或在 S12 审查与整改后独立迭代。DeepSeek Harness 补入的 B8/B9 属此类。
- **P3（7 项）**：Cloud、企业、Slack/Linear、voice 等重型或长尾功能，建议在 S12 审查与高优先级整改完成后再按用户需求排期。

**注意事项**：

 1. 工具类缺口（B1–B9）大多数可在 S2（工具注册面）或 S4（run_command 后的工具扩展）以新增 `AgentTool` 实现的方式低风险追加，不触及契约或架构。B8 除外：单轮组合多步工具会改 loop 形态，落地前需单独设计。
 2. CLI 体验类（A1–A4）在 S0 CLI 最小实现或 S10 CLI 正式化阶段追加，写入集限定在 `pawork-cli`。
 3. D1（IDE 扩展）虽列为 P1，但实现路径是独立的 IDE 扩展项目经 ACP 连接，不影响 Core 包；可与 S10 并行。
 4. 部分功能（D5 安装器、F1 遥测）是纯运维/产品层，无架构依赖，可随时插入。
 5. 每项落地时须遵守 §3.2 冻结契约先行原则——新工具的 `ToolDescriptor` 审批/只读语义在加入时就定义清楚。

---

## 7. 发布策略

W1–W4 波次与包清单保留为 [v1-migration-reference.md](v1-migration-reference.md) §5.2 的历史候选策略，不属于当前 S0–S13 执行范围。各包在激活阶段仍保持发布卫生（元数据、无类型泄漏、`publish = false` 默认）；S12 只审查其真实状态，不翻转 `publish`。只有在 S13 收口后且用户明确决定发布后，才另立发布任务并重新核对波次、License、全量门禁与三平台证据。
