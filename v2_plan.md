# Pawork V2 任务开启编排

> 本文是 V2 开发的**指定开启文件**。每次开新对话，提示词指向本文即可；主代理按本文编排**一个波次**（研究 → 设计 → 实现 → 收尾）。
>
> 本文只负责「选哪一波、怎么搜、怎么设计、怎么派子代理」。过程纪律（架构红线、迁移、测试、凭证、收尾清单）以 [docs/task-guide.md](docs/task-guide.md) 为准，不在此重复展开。

---

## 1. 文档地图

| 文档 | 读它做什么 |
| --- | --- |
| 本文 `v2_plan.md` | 开启编排、当前指针、统一提示词、子代理模型约定、S8+ 并行与前置注解（§11） |
| [ROADMAP.md](ROADMAP.md) | 阶段总索引、依赖、状态、阶段外任务 |
| [docs/design.md](docs/design.md) | 包激活映射、冻结契约、本阶段功能与参照项目映射 |
| [docs/gui-design.md](docs/gui-design.md) | Desktop GUI 设计：S7 先锁定再实现；后续阶段只按该文加面 |
| [docs/task-guide.md](docs/task-guide.md) | 开启核对、红线、测试通道、并行纪律、收尾与报告 |
| [plan/](plan/) | 本阶段任务书：目标、包与 V1 资产、冒烟、退出标准、**并行拆分（波次）** |
| [docs/references.md](docs/references.md) | 参照项目手册（公开文档入口） |
| [docs/v1-migration-reference.md](docs/v1-migration-reference.md) | V1→V2 唯一迁移词典 |
| [plan/archive/](plan/archive/README.md) | 归档索引；M0–M8 正文当前未落仓，迁移事实以 `v1-migration-reference.md` §4.1 为准 |
| [AGENTS.md](AGENTS.md) | 仓库级红线；V2 开发期验证放宽见 task-guide §6 |

工作区根即 Cargo workspace 根（2026-08-17 已将原 `Pawork_v2/` 摊平到仓库根）。V1 crate 目录若仍存在则只读，不在 V1 上继续加功能。

---

## 2. 开启提示词（用户侧）

**子代理模型必填。** 未写模型时，主代理只完成「读指针 / 提议下一波」，然后提问并停止，不启动研究或实现。

```text
按 v2_plan.md 开始。
子代理模型：〈必填：当前宿主可接受的模型标识，见 §7〉
范围覆盖：〈可选。例：S0 波 B。不写则按 §4 自动选下一波〉
凭证：〈可选。PAWORK_API_KEY_* 已设 / 本波无需真实 key；本地冒烟默认 source .env〉
临时约束：〈可选。例：跳过真实冒烟、只设计不实现——默认不要用〉
```

同一条消息里的「范围覆盖」优先于自动选择。不要在提示词里粘贴 task-guide 全文。

---

## 3. 当前指针（每波收尾由主代理更新）

| 字段 | 值 |
| --- | --- |
| 当前阶段 | S13（[plan/S13-s12-remediation.md](plan/S13-s12-remediation.md)）已收口（波 A–C ✅）；S12（[plan/S12-project-code-review.md](plan/S12-project-code-review.md)）已收口 |
| 阶段状态 | S13 🟢；S12 🟢；S11 🟢；S10 🟢；S9 🟢；S8 🟢；S6 仍 🔵（OAuth 临期 refresh 挂账） |
| 已完成波次 | S0 波 A–D（含 2026-08-14 两通道真实冒烟）；S1 波 A–C（含 2026-08-14 两通道真实冒烟：`sessions` / `--resume` / `run --json` / `kill -9` 恢复）；S2 波 A–C；S2 波 D（2026-08-14：cli `⚙` 工具行 + `host/app` 装配 `run_session`/scheduler/四只读工具；三通道冒烟 GLM OpenAI / GLM Anthropic / OpenCode Go）；S3 波 A（2026-08-15：`pawork-policy` 整包 + `pawork-tools` 写三件 + scheduler `check_gate`）；S3 波 B（2026-08-15：engine `ApprovalGate` + app 写三件/审批宿主/resume 封口 + cli `--approval-mode`/`y`/`a`/`n`/`--json` fail-closed）；S3 波 C（2026-08-15：两通道真实冒烟 + 提示注入评估；S3 收口）；S4 波 A（2026-08-15：`pawork-exec` process+sandbox；`run_command` + policy 认 `argv`；fail-closed 守 ADR-031 可观测回退）；S4 波 B（2026-08-15：engine `CancelHandle` + 工具中取消不发 `ToolExecutionCompleted`；cli 命令流式渲染/stderr 着色/Ctrl-C 走 `cancel(User)`；app 注册第八件 `run_command`）；S4 波 C（2026-08-15：两通道「读-改-跑」闭环 + Ctrl-C `RunCancelled` + `git push --force` Dangerous 拒绝；S4 收口）；S5 波 A（2026-08-15：`pawork-provider-core` usage/registry/pricing/negotiate/reasoning；`pawork-session` compaction feature + `TokenEstimator` 注入）；S5 波 B（2026-08-15：engine `context` 四模块迁入并接 `run_session`（ContextPrepared 估算/软限压缩/硬限截断）；app registry 装配 + session usage/cost + 手动压缩；cli `/compact` + 每轮用量行 + `models` 目录（window/定价）；修 trigger 消息 id 同毫秒撞主键与压缩折叠水位误删保留尾部）；S5 波 C（2026-08-15：两通道真实冒烟——长对话软限压缩/`--resume`/`/compact`/token 对账 1:1/`pawork models` + 评估记录；修复 `last_run_usage` 冻结最早轮；S5 收口） |
| S6 波次进度 | 波 A（2026-08-15）：六条首发通道 adapter、共享 Responses、credential fail-closed、wiremock 契约完成；未做真实凭证冒烟。波 B（2026-08-15）：`pawork-auth` 整包迁移 + Keychain→env→无凭证解析链（41 测试）；`pawork-diagnostics` 全局脱敏 layer + `RedactingFmtLayer`（metrics/bundle 门控 `experimental`）；workspace manifest 修复 glob 命中占位目录的加载错误。波 C（2026-08-15）：六通道正式装配 + `pawork models` 聚合 + `pawork auth` 四子命令 + REPL `/model` `/provider` 切换事件 + 宿主全局脱敏挂载；glm-coding/opencode-go 完成 set-key→清 env→Keychain 流式工具任务，GLM 双协议通道切换实测，trace 日志 0 泄漏；修复 keyring 平台后端缺失（原默认 mock 存储）；ChatGPT/xAI OAuth 与 Qwen/DeepSeek 凭证按 fail-closed 登记。收口（本地实现，2026-08-16）：六通道首次真实冒烟已齐；生产 default OAuth 请求路径接入进程内 singleflight；正式 auth 文件后端新增跨进程 write/refresh 锁、锁内重读、独立临时文件与 access/轮换 refresh/meta 批量原子回写；双进程旧快照只触发一次 token exchange，空 refresh 拒写，`invalid_grant` 明确要求重登；活动文档统一 auth 文件术语并保留历史/冻结兼容名；S6 定向自动化全绿 |
| S7 波次进度 | 波 0（2026-08-16）：锁定 `docs/gui-design.md` 的信息架构、交互状态、官方参照取舍、分页 Timeline 恢复、协议最小切片、四层依赖 deny list 与 S8–S12 增量图；修复任务书对缺失 M0/M5 正文的死链；未创建 Desktop crate、未进入 UI 实现。波 A（2026-08-16）：激活 `pawork-protocol`（V1 gui-protocol 帧 + core-api App 类型零裁剪、SessionGet 分页字段 + minor 1.1 + golden 先行）、`pawork-transport`（local UDS/Named pipe）、`pawork-gui-server`（单客户端握手→Snapshot→订阅→帧循环、Resume 三态、断线不取消 Run）；`pawork-app` 增 GuiHost 适配 + 事件扇出 + Run 注册表 + Timeline 投影，`pawork-cli` 增 `gui serve`（单实例探测）；六包定向测试全绿；`pawork-client`/`apps/desktop` 未建（波 B） |
| S7 波次进度（续） | 波 B（2026-08-16）：激活 `pawork-client`（V1 gui-client 整包平移至 clients/gui-client，authentication 改 `Option<ClientAuthentication>`、artifact 读取 `experimental` 门控、7 契约测试随迁，补 desktop 用 protocol/transport re-export）；新建 `apps/desktop`（gpui =0.2.2 锁定，ADR-035 审计过；四层 ui/projection/controller/platform，Sessions 侧栏 + Timeline + Composer + IME 输入，`--probe` 连接验证模式）；修复 `gui serve` accept 后丢弃 SessionHandle 致握手 Broken pipe 的波 A 装配缺陷；三包定向测试全绿（client 3+7 / desktop 4 / cli 17），依赖红线断言通过（desktop 直接业务依赖仅 pawork-client），真实 socket probe 正向冒烟通过（握手 + snapshot） |
| S7 波次进度（波 C） | 波 C（2026-08-17）：Desktop 恢复 Snapshot `ActiveRuns`/待审批、诚实 ContextMeter/RunStatusBar、默认窗 1440×1024、`--probe-smoke`；Host `ModelList` 改 `models_overview`，`RunStart.model` 未知 id 可按 overview 切通道。真实冒烟：`glm-coding`/`glm-4.7` 流式、`ask-for-writes` 审批通过、取消、重连 `persisted=12`、断线 `ActiveRuns` 仍在。`deepseek-v4-flash` 不在 GUI 聚合目录，跨通道切换 skipped；v3 TaskRail 未做 |
| S7 波次进度（波 D） | 波 D（2026-08-17）：v3 TaskRail（GroupingMenuButton、All projects 范围、日期→项目→Task / Projects、项目定向新建）；Host 消费 `SessionCreate.workspace_id`（进程内绑定，不改 sessions DDL）；`models_overview` 探测所有可装配通道。`--probe-smoke`：`first=glm-4.7` / `second=deepseek-v4-flash` / `second_turn=switched` / `approval=approved` / `cancelled=1` / `persisted=21` / `disconnect_survive=running`。S7 收口。人工 IME 窗口与 1440×1024 对照定稿图未做 |
| S8 波次进度 | 波 A（2026-08-17）：并行激活 `pawork-git` 与 `pawork-blob-store`（golden 先行；`cargo tree -p pawork-git` 无 `pawork-workspace`）。波 B（2026-08-17）：engine `snapshot_write_tools` → `CheckpointCreated`；app 装配 blob/git、写前快照、会话 diff/rollback、审批 hunk、`DiffListFiles`/`DiffGet`；cli `pawork diff` / `rollback`。定向测试 `pawork-engine`/`pawork-app`/`pawork-cli` 全绿。两通道真实冒烟：`glm-coding`/`glm-4.7`（git 仓库写三文件→diff→rollback 干净）+ `opencode-go`/`deepseek-v4-flash`（非 git 快照闭环）；中文路径、审批 hunk、worktree 不串主区、`--force`/`-o xx` 注入防御、回滚后模型确认文件已撤销。Desktop Changes 面未做（ROADMAP §4）。S8 收口 |
| S9 波次进度 | 波 A（2026-08-17）：并行激活 `pawork-mcp`（rmcp `=2.2.0` 锁进内部 `codec`，公开 `McpPeer` 无 SDK 类型；stdio fail-closed；MCP 工具与内置工具共用 `ToolRegistry`）、`pawork-resources`（AGENTS.md / Skills / profiles，hooks/lsp/templates/watch 不迁）、`pawork-config`（`Loader::with_session` / `with_run`，六层矩阵 + Profile 文件切换；未重写 merge）。定向测试 `pawork-mcp` 59 / `pawork-resources` 61 / `pawork-config` 44 全绿。波 B（2026-08-17）：并行激活 `pawork-compat`（五来源只读导入；MCP/Hook 本地薄类型，`cargo tree` 无 rmcp）、`pawork-session` `import::formats`（compat 四来源 + export v3 + pi；外部源 Secret 前缀拒绝；三路 Immediate 单事务）、`pawork-workspace` file-index（构建/查询/去抖 watch）。定向测试 `pawork-compat` 16 / `pawork-session` 53 / `pawork-workspace` 12 全绿。波 C（2026-08-17）：engine `InjectedLayer` + `resources.injected` Diagnostic + System 计入预算；`host/app` 装配 MCP/resources/file-index/compat；CLI `mcp list/test`、`import <tool>`、`sessions import/export`、REPL `@file`。定向测试 `pawork-engine` 60 / `pawork-app` 59 / `pawork-cli` 20 全绿。真实冒烟：`glm-coding`/`glm-4.7` + MCP filesystem（`npx @modelcontextprotocol/server-filesystem`）混用内置/`fs.*`；AGENTS.md「收到」+ Skill `greeter` 注入；`@ROADMAP` 独立 ContentPart 且答对第一行；`import claude --yes` 源 mtime 不变；export v3 → 新 data dir import → resume。Desktop Composer `@` / Resources 未做（ROADMAP §4）。S9 收口 |
| S10 波次进度 | 10a 波 A（2026-08-17，串行）：`pawork-protocol` 收口。`app.rs` 拆 version/command/query/event/quota/limits；并入 headless（`HeadlessError`）/ adapter（`AdapterWireFrame`）/ `client_auth`；`SUPPORTED_API_VERSIONS` 含 1.0 与 1.1；`PROTOCOL_CRATE_COMPATIBILITY` 表；typegen `[[bin]]` + 检入 `schemas/{core-api,gui-protocol,headless-json}`；GUI 帧 golden 补 17 条（原 9 条字节未改）；headless fixture 随迁。`--json` 映射表见 [docs/headless-json-migration.md](docs/headless-json-migration.md)（未切 CLI 输出）。定向测试 `pawork-protocol` 113 全绿。archive/M0 正文仍缺失，细则回退词典 §4.1 |
| S10 波次进度（续） | 10a 波 B（2026-08-17，并行 ×3）：`pawork-app` 将 `GuiEventBus` 演进为 V1 EventHub（容量 4096、ring/replay/`Lagged`、`SeqCst` 重写 `global_sequence`）；`subscribe()` 仍返回 `broadcast::Receiver`（gui-server 未改）。`IdempotencyStore` 接到 `GuiHostAdapter::command`（tenant 固定 `local/default`）。`RateLimiter` 模块+单测已迁、热路径未接。`pawork-transport` 补 `memory`/`remote` feature；Remote trait 上移；rustls 只在 `--features remote`；placeholder 不迁，Mock 落入 memory。`pawork-sdk` 从 exclude 转正（V1 agent-sdk 改名，fixture 升 1.1）；`ide` 因缠 LSP 只留空门控。未改 `--json`、未加 `headless` 子命令。定向测试 `pawork-app` 83 / transport 8·23·37 / `pawork-sdk` 22+3 skip 形态全绿；`cargo check` gui-server/cli/client 通过 |
| S10 波次进度（10b） | 10b（2026-08-17，并行 ×4 + 主代理装配）：`pawork-gui-server` ConnectionManager，Resume 走 host 共源、队列满丢新；Desktop Replay 三态 / 时间线 Fork / Inspector Terminal 文本面，`connect_with_resume`；`pawork-channels` 仅 `acp`；`pawork-exec` PTY + `pawork-session` lifecycle/`fork_from_event`。Host 接 `SessionFork`/`Terminal*` 与 Hub replay；幂等按 GUI `client_id` 作用域（避免多客户端 `gui-cmd-N` 撞车把 RunCancel 重放成 SessionCreate）。未做 CLI 六模式、protocol-probe、`--json` 切输出、本机 GPUI 多窗、VT100。S10 保持 🔵 |
| S10 波次进度（收口） | 收口（2026-08-17，串行）：`pawork-cli` 接六模式 + 运维。`--instance`（默认 `default`）解开数据路径；`headless --json-stdio` / `acp serve` / `service install\|start\|stop`（默认 dry-run，`--apply` 才改系统；注册命令是 `pawork --instance X gui serve`）/ `status`/`watch`/`shutdown`/`doctor`；`sessions fork`（新 CLI UX，V1 无此子命令）+ `chat --branch`；`run`/`chat --prompt --json` 升到 `HeadlessResponse`（去掉 unstable；不是恢复 V1 裸信封）。`GuiHostAdapter` 回 `WorkspaceAdd`/`SessionCreate`/`SessionOpen`/`SessionFork` `Data`，无凭证 `RunStart` 回 `Error`。激活 `apps/protocol-probe`（V1 9 场景名，V2 包名；无 golden）。定向测试 `pawork-app` 86 / `pawork-cli` 22 / `protocol-probe` 9 场景 / `pawork-sdk` spawn_e2e 3 全绿。未做：真实 ACP 编辑器、两 GUI 对真实 `gui serve` 的人工 Replay、凭证下 `--json`/fork resume、darwin launchd `--apply`；Windows Service 本机无法验收（降级登记）。S10 保持 🔵 |
| S10 波次进度（冒烟） | 冒烟补齐（2026-08-17）：两 GUI 真实 serve Replay 过（`protocol-probe --live-two-gui` `replay 14-61 n=48`）；PTY `echo` + 断线后 Hub Replay；`--json` 人工对照（`type=event\|response`，无 hello）；`sessions fork --no-switch` 后 main / fork 两分支 `chat --resume` 均回到指定词（`glm-coding`/`glm-4.7`）；darwin `service --apply` 实例 `s10svc`：install→start→`doctor handshake=ok`→stop（当时只 unload，**未删 plist**，S12-F39；删除定义由 S13 F03 补齐）。ACP stdio `initialize`+`session/new` 回到 `sessionId`。本机无 Zed，真实编辑器 fail-closed 不打勾；S0–S9 历史冒烟未复跑；Windows Service 仍不打勾。最小修复：`WorkspaceList` 补 `roots`；`gui serve` 握手声明 `TerminalStreaming`/`ArtifactStreaming`；`PersistThenRender` 写入 active branch（fork 上不再写死 `main`）；`RunStart` 失败会广播 `RunChanged(Failed)`；`--json --apply` 真正执行。定向：`pawork-app` 88、`desktop_direct_deps_stay_on_client_deny_list`、`pawork-cli` parse。S10 保持 🔵 |
| S10 波次进度（冒烟剩余） | 2026-08-17：S0–S9 已勾核心项复跑（§1.1 `opencode-go`/`deepseek-v4-flash` + S7 `glm-4.7`）。修复 `RunStart` 续聊漏历史（定向 `run_start_second_turn_includes_session_history`；真实 `--json --resume` 第二轮 `message_count` 3、第三轮 5，口令 `BLUE-PINE`）。修复 `models_overview` 单通道 4s 探测超时（ChatGPT 探活不再拖死 Desktop 10s `ModelList`）。S7 `--probe-smoke`：`first=glm-4.7` / `second=deepseek-v4-flash` / `second_turn=switched` / `approval=approved` / `cancelled=1` / `persisted=11` / `disconnect_survive=running`。 |
| S10 波次进度（Zed ACP） | 2026-08-17：Zed 1.15 Agent Panel 真实走完 `initialize` → `session/new` → `session/prompt`。`ses-1786972257220-1` 回 `session/update` 文本 `pong` + `stopReason=end_turn`。修复 `HostSessionResolver` 忽略 `EventStream::Session`（`GuiEventBus` 生产形态）导致 prompt 永不收口；定向 `pump_events_session_stream_run_changed_resolves_prompt`。S10 收口为 🟢。不自动开 S11。 |
| S11 波次进度 | 波 A（2026-08-17）：并行激活 `pawork-control-plane`（tenant/identity/`UsageLedger`/audit JSONL + `fixtures/audit/event-v1.jsonl`）、`pawork-quota`（核心 LocalLedger/`QuotaService`，远端六厂商+WebScrape+refresh **未迁**）、`pawork-provider-control`（lease/binding/pool + feature 实名 `account-control-v1`；`app-database` 的 control_plane/lease schema 收回本包，identity schema 归 control-plane）。`archive/M7` 正文缺失，回退词典 §4.1。未改 domain/api/protocol，未接 host/cli/`pawork usage`。定向：control-plane 79 / `--no-default-features` 59；quota 100；provider-control 202+10 / `--no-default-features` 88。OTel exporter 与 account/routing/health/factory 无宿主，ROADMAP §4 登记。 |
| S11 波次进度（波 B） | 波 B（2026-08-17）：并行激活 `pawork-workflow`（plan/goal/task/automation/monitor 五合一；`process-exec` 默认关，`TaskManager::new()` 无 backend；`start_process` 仅 feature 档，domain/exec 双取消令牌）、`pawork-memory`（Provider 无关抽象 + 测试 `FixedEmbedder` 写入→召回；无真实 embedder）、`pawork-review`（re-anchor + resolution + `ForgeAdapter`/`GenericForgeAdapter`；`parse_unified` 走 `pawork_git::diff`）。不迁 `throttle.rs`、不改 domain serde、不接 host/cli/engine。`archive/M7` 仍缺失，回退词典 §4.1 第 26–28 行。定向：workflow 91（默认；`process_and_policy` 0）/ `--features process-exec` 102（含进程 11）；memory 17；review 18。`cargo tree -p pawork-workflow -i pawork-exec` 默认未匹配。 |
| S11 波次进度（波 C） | 波 C（2026-08-17）：激活 `pawork-orchestration`（`agents/orchestration`；workspace 追加 `agents/*`）。V1 `orchestration` + `teams` 整包迁入；`supervisor` 拆 spawn/registry/cancel-tree/recovery/budget-gate；budget 接波 A `UsageLedger` / `TenantPolicyEngine` / `CredentialPool`（两控制面包 `default-features = false`）。`teams` 复用波 B `pawork-workflow::plan`（与原「只依赖波 A」字面冲突，按词典整包迁）。`GitWorktreeAllocator` / `GitDiffProvider` 仅 `git` feature。`TeamEvent` 18 变体留在本包，不改 domain/protocol。有意增量：`SupervisorConfig.max_worker_depth` 默认 `None`。不接 host/cli/engine。定向：`cargo test -p pawork-orchestration` 123（lib 96 + durability 5 + teams_service 22）；`cargo check --features git` 通过；默认 `cargo tree` 无 `pawork-git` / `pawork-exec`。`archive/M7` 仍缺失，回退词典 §4.1 第 29 行。 |
| S11 波次进度（波 D） | 波 D（2026-08-17，单一 owner 串行）：host 接线四消费点。Plan gate 在 `run_session` 前（整版 `Approved`，无 plan 放行；engine 不依赖 workflow）。`pawork usage` 接 SqliteUsageLedger + LocalLedger quota，与 S5 会话行同源。`pawork tasks list/status/cancel/register` 快照 `{instance}/tasks.json`（`process-exec` 仍关）。`pawork agents demo [--cancel] [--budget-tokens]` 装配 Supervisor（GLM + OpenCode Go AcquireRequest、cancel-tree、Mock ledger budget-gate）。CLI 另有 `pawork plan` 与 REPL `/plan`。修 usage 默认 `USD`（账本校验）与 replay 后 `task_N` 分配器。定向：`pawork-app` 94 / `pawork-cli` 22 / `pawork-workflow` 默认档全绿。真实冒烟：plan 拦截→批准→`glm-4.7` `PING`→replace 再拦；usage 对账 GLM 3192/35、OpenCode 3383/19；tasks 跨进程 `task_0`/`task_1`；demo 完成/cancel 三节点/budget-gate。Desktop Workflow 面未做。S11 收口为 🟢。不自动开 S12。 |
| S12 波次进度 | 2026-08-17～18 一次波次完成（任务书未分波）：主代理开启核对时登记补充主审范围（任务书未列 S9/S11 新增包：resources→CR-02、provider-control→CR-04、control-plane core/quota→CR-05、workflow×3+orchestration+testkit→CR-06）。CR-01～CR-08 按任务书模型分工并行审查（GLM 包：01/05/06/08，Grok 包：02/03/04/07），18 条 High 全部由另一模型交叉复核（5 份裁定表）：3 条降 Medium（CR03-02/CR05-02/CR07-03）、4 处证据/表述修正回写主报告；CR-09 最后做需求追踪与跨报告一致性收口。九份报告 + 五份裁定见 [docs/reviews/s12/](docs/reviews/s12/)。共 60 条 finding（全部 Confirmed，H15/M27/L18），按回写规则合并为 57 项 S12-F01～F57 登记 ROADMAP §3.2（CR04-06 链接 K-10、CR09-05 随 F01、CR02-01/02 同根因合并）。未改生产代码、未跑测试/构建/冒烟。S12 收口为 🟢 |
| S13 波次进度 | 波 A（2026-08-18）：安全与数据风险 F01–F14 + 随动 F45/F50/F52。拍板：F01 读写均拒 `.git`（无审计开关）；F02 支 B 诚实标签 `HardWritesAndNetwork`，保留 `(allow file-read* (subpath "/"))` 并扩 `default_secret_paths`；F05 SecretRef 仅 `pawork.mcp.*` + 独立 `mcp-auth.json` + stdio `env_clear` 拒 `PAWORK_API_KEY_*`；F06 剥离 workspace `proxy_url`/非回环 `base_url` 且 `redirect(Policy::none())`；F07 `classify_status` 只留 `HTTP {status}`；F08 剥离 workspace `trusted`/`auto_start`；F09 支 A schema v10 + `ancestor_lineage`（信封 v1 / append-only 未改）；F11 Lagged 发 `ReplayUnavailable`（F32 Snapshot 归一仍属波 B）。路径内核统一 `policy::path`；生产 `gui serve` 强制 token（UDS 0600）；Timeline 锚点改 `event_id`/`sequence`；`RunStart.provider` + API 1.2。F14 主路径 keybinding/tab_stop 已落地，真实窗口键盘走查并入 K-03；F03 Windows SCM 按 S10 降级未验。定向测试全绿。不自动开波 B |
| S13 波次进度（波 B） | 波 B（2026-08-18）：功能缺口与 Bug F15–F40 + 随动 Low。契约五项 ADR-037（F15 trait 归 domain；F19 维持 ADR-031 可观测回退；F24 `artifacts`；F26 `Revised` title/steps；F28 `ResultArchived.task_id`）。F32 客户端收齐附带 Snapshot；F33 未映射 headless fail-closed；F42 typegen 条件化（`cargo tree -p pawork -i ts-rs` 无结果）。Desktop：底部跟随/多行 Composer/All projects 确认 workspace/Inspector 440px/Fork 收进 ···。F37/F40/F49 走 §4 登记；F57 并入 K-04。真实窗口证据并入 K-03。不自动开波 C |
| S13 波次进度（波 C） | 波 C（2026-08-18）：§3.2 的 57 项压缩进 §3.1「S13 收口」行（链任务书 + [docs/reviews/s12/](docs/reviews/s12/) + [ADR-037](docs/adr/ADR-037-s13-wave-b-contracts.md)）；文档一致性复核 README / ROADMAP / v2_plan / AGENTS / task-guide；F40 三行补 `S12-F40`；F57 在 §4 指向 K-04。S13 收口为 🟢。不自动开后续任务 |
| **下一波次** | 无自动下一阶段。K-01～K-10 与「多账户并入 plan」仍按阶段外任务、须用户点名；发布/全量门禁/三平台矩阵须另立任务并明确授权 |
| 阻塞 | S6 的 ChatGPT/xAI 自然临期真实 refresh 人工验收继续挂账，S6 保持 🔵 |



自动选择以本表为准，再用 ROADMAP / 任务书 / 工作区实态交叉校验（§4）。三者冲突时：**工作区实态 > 本表 > ROADMAP 状态列**；更新本表使三者一致后再开工。

---

## 4. 选任务规则

一次开启只做 **一个波次**（任务书「并行拆分建议」里的 A/B/C/…）。做完即收尾，不自动跨入下一波。

1. 读 ROADMAP §2。依赖阶段必须为 🟢；若当前阶段为 ⚠️，停止并报告阻塞。
2. 取第一个非 🟢 的主干阶段（S0→S13）。**S7 现为最小 Agent GUI（先锁定 [gui-design.md](docs/gui-design.md) 再实现）**；S12 是只读 Code Review，按其任务书审查和登记 finding，不进入实现/测试流程。S13 已收口（57 项压缩进 ROADMAP §3.1）；不要重开整改波。剩余非 🟢 主干只有 S6（OAuth 临期 refresh 挂账），**不得**因自动选择而重开 S6，除非用户在「范围覆盖」里点名。WASM 插件 / Hooks / LSP / 市场不占阶段号，见 ROADMAP §4。阶段外任务（ROADMAP §3.2）仅当用户在「范围覆盖」里点名时才做。
3. 读该阶段任务书的「并行拆分建议」，结合 §3 指针与工作区（`**/Cargo.toml`（workspace 根）、成员 crate 是否已激活、任务书勾选）选出**最早未落地的波次**。
4. 用户覆盖（「做 S2 波 B」「先做阶段外：多账户并入 plan」）立即生效。
5. 在聊天里用三行声明后立刻进入 §5（不必等确认）：
   - 本次：`S<N> 波 <X>` + 任务书中该波一句话；
   - 子代理模型：用户指定值；
   - 写入集：该波允许触碰的包/目录。

主干 S0–S4 按阶段串行。跨阶段并行只在 ROADMAP §2 依赖已满足、且用户明确要求时才开第二条线：S5/S6 可并行，S8（git）可与它们并行；S7 GUI 设计波不依赖 S6，实现波建议 S1–S5 已绿。

---

## 5. 主代理执行流程

未指定子代理模型 → **停在 §4 第 5 步之前**，向用户要 §2 模板中的那一行。

### 5.1 开启核对（主代理亲自读，不派发）

按 [task-guide.md](docs/task-guide.md) §2：任务书全文、ROADMAP 依赖、[design.md](docs/design.md) §3.2 本波相关冻结契约、§4 本阶段功能表。本波若需要真实 API，缺 key 则 fail-closed（task-guide §5），不改用 mock 顶替冒烟。

### 5.2 并行研究（只读，2–3 路同时派发）

在写设计、改代码之前，用 **§8.1 同一骨架** 并行派出研究子代理（全部使用 §7 指定的模型）。默认三路：

| 路 | 搜什么 | 目的 |
| --- | --- | --- |
| R1 V1 资产 | 本仓库对应 crate / 测试 / `docs/features/` | 定位要迁的代码、golden、已知死代码与 deferred API |
| R2 参照项目 | design.md §4 本阶段已映射项 + references.md 链接；只深挖本波功能，不扫整仓 | 行为对标与取舍（红线排除项只记「不采纳」） |
| R3 迁移词典 | v1-migration-reference.md 映射总表；plan/archive 目标存在时再读本波包级细则 | 合并来源、行数、关键动作、冻结候审清单；缺失引用须回报，不得臆造 |

约束：

- 只读。禁止改文件、禁止开始实现。
- 不克隆参照项目到本仓库；公开文档用已有 research / 链接抓取。
- 回传要带路径 + 行号（或稳定符号名），不要空泛摘要。
- 本波任务书已写明「直接迁移 / 新写」的，R1+R3 仍要跑（核对事实源）；R2 若 §4 本阶段没有对应行可省略。

### 5.3 本波实现设计（主代理写，不改冻结契约）

研究齐了之后，主代理在**本会话**写出「本波实现设计」（结构化消息，默认不新建 markdown）。子代理不得自行改设计。需要 ADR 或与任务书/冻结契约冲突时，**先问用户再实现**。

设计至少包含：

1. **目标 / 非目标**：对应该波与任务书「为后续阶段预留 / 明确不做」。
2. **V1 事实源**：复制 / 合并 / 改名 / 测试随迁的路径；明确不搬的死代码。
3. **参照取舍**：采纳的行为 vs 红线排除（无 TUI、无 JS runtime 等）。
4. **包与模块**：激活还是增强；关键类型 / trait / 文件；依赖方向。
5. **冻结契约**：本波涉及的表项；字段宁可闲置，禁止「先简后改」。
6. **写入集**：允许触碰的目录/包；契约文件（`foundation/domain`、`foundation/api`）单一 owner。
7. **验证**：本波 `cargo test -p …` / `cargo check -p …`；是否需要真实 key。
8. **派发图**：本波若标「并行 ×N」，列出每路写入集；若标「串行 / 单一 owner」，主代理自做或只派 **一个** 实现子代理。

设计默认留在会话里。仅当发现任务书或 design.md 的缺口/错误时，由主代理改现有文档，不另开设计文件。

### 5.4 按波次实现

- **研究已并行结束**，再进入实现；不要边搜边写。
- 实现并行度 **严格按该波标注**：并行波一次派齐互不重叠的实现子代理；串行波不拆。
- 契约文件不并行。装配收口（`host/app`、`apps/pawork`）与真实 key 冒烟由主代理做。
- 每个实现子代理使用 **§8.1 同一骨架**（角色=实现）+ 主代理设计中属于它的切片 + 写入集边界。
- 子代理写完后主代理做：冲突检查、定向测试、必要接线。子代理之间禁止改同一文件。

### 5.5 本波收尾（主代理）

1. 跑本波写入集对应的 `cargo check/test -p <crate>`（多个 `-p`，不用 `--workspace`）。开发期无 clippy/fmt/Full Gate（task-guide §6）。
2. 更新本文 §3 指针：已完成波次、下一波次；本阶段仍有剩余波次则 ROADMAP 标 🔵。
3. 最后一波才跑任务书冒烟清单、勾退出标准、ROADMAP 标 🟢（需真实 key 则 fail-closed）。
4. 简式报告（task-guide §4 第 5 条）：写入集、验证、登记项、说明未跑全量门禁为正常。
5. 不提交、不推送，除非用户当场要求。

---

## 6. 并行与子代理纪律

- 文档、指针、设计、ROADMAP/任务书勾选：**主代理写**。
- 研究可并行；实现按波次并行。两阶段不要叠成「有的包还在搜、有的包已经在写」。
- 写入集以包/目录为界，互不重叠。
- 一次开启只派 **本波** 的实现子代理，不预派下一波。
- 并发保持小：研究最多 3 路；实现按任务书该波的并行度（通常 1–3）。不要为加速拆碎契约波。
- 子代理同样受 task-guide 全文约束；提示词里写明「禁止越写入集、禁止改冻结契约形状、禁止 git commit」。

---

## 7. 子代理模型

开启提示词里的「子代理模型」作用于**所有** `Task` 子代理（研究 + 实现）。主代理用当前对话模型，不要擅自换成别的。

本文**不映射具体模型**：不维护型号清单，也不做「用户写法 → `Task` 参数」的翻译表。用户写的模型标识由主代理**原样**落入 `Task`（落在哪个参数、取什么值以当前宿主为准），不猜测、不替换、不查表。

写法：

- 直接写当前宿主 `Task` 可接受的模型标识（如宿主文档/可选列表中的 model slug、subagent 类型等），一行一个值；研究与实现共用同一值。
- 想与主代理同模型：写 `inherit`（或宿主的等价写法）。

规则：

- 用户写的模型标识当前宿主无法识别 → 提问，不猜测、不替换。
- 禁止对研究用快模型、对实现用另一个模型，除非用户在「临时约束」里写明。
- 用户指定的模型不在当前宿主能力范围内时：告诉用户不可用项与可用项，等回复。
- 具体型号与 `Task` 参数映射随宿主（Cursor / opencodex 路由等）变化，以宿主当前可选值为准；本文件不维护、不钉死。

---

## 8. 统一提示词

所有子代理用同一骨架，只替换「角色 / 范围 / 产出 / 禁止」四段。主代理把用户指定的模型按 §7 传入 `Task`，**不要把模型名写进 prompt 指望子代理自己切换**。

### 8.1 骨架

```text
你是 Pawork V2 的〈角色：研究 | 实现〉子代理。只做本提示词里的范围。

规范（纪律全文，必须遵守）：
- docs/task-guide.md
- 仓库根 AGENTS.md（V2 开发期验证放宽以 task-guide §6 为准）

任务：
- 阶段任务书：plan/S<N>-*.md
- 波次：〈波 X：一句话〉
- 设计切片：〈实现角色必填——粘贴主代理本波设计中属于本路的部分；研究角色写「无，先于设计」〉

范围：
- 〈研究：只读路径/关键词；实现：允许写入的包/目录清单〉

产出（完成后一次性报告）：
- 〈见下方角色特化〉

禁止：
- 超出范围的文件改动或无关重构
- 改变冻结契约的 serde/磁盘/线上形状（字段可闲置，不可删减「图省事」）
- git commit / push / 改 git config
- 把 Secret 写入仓库或日志
- 运行 cargo --workspace / clippy 门禁 / cargo clean
- 研究角色：任何写入；实现角色：开始前改设计、碰契约包（除非写入集明确包含）
```

### 8.2 角色特化 — 研究 R1（V1 资产）

在骨架「产出」处填：

```text
- 本波相关 V1 crate 路径、关键类型/函数（文件+行号或符号名）
- 必须随迁的测试 / golden / 种子
- 评审已标的死代码、deferred-consumer API（建议接线或删除）
- 不要搬的内容（冻结候审、明显死代码）
- 与任务书「V1 来源与方式」不一致之处（事实源优先）
```

范围示例（V1 已归档于 `../Pawork_v1/`，以下路径相对该目录）：`crates/agent-domain`、`crates/provider-api`、对应 `docs/features/`、crate 内 `tests/`。

### 8.3 角色特化 — 研究 R2（参照项目）

```text
- 仅针对 design.md §4 本阶段、且属于本波的功能行
- 每个功能：参照项目中的对应行为、文档 URL、与 Pawork 红线冲突的部分
- 建议采纳的行为要点（短）；明确不采纳的（TUI / JS 插件 / 身份伪装等）
- 不要写实现代码
```

### 8.4 角色特化 — 研究 R3（迁移词典）

```text
- v1-migration-reference.md 映射表中本波各包的来源、行数、关键动作
- plan/archive 里实际存在且可直接引用的包级细则段落链接；目标缺失则明确报告并回退到迁移词典 §4.1
- 本波激活 vs 后续增强的边界
- 冻结候审资产是否被任务书误列入
```

### 8.5 角色特化 — 实现

```text
- 按设计切片在写入集内完成迁移或新写
- 关键测试随迁并在本包 `cargo test -p <crate>`（或 check）通过
- 报告：实际写入文件、验证命令与结果、未做项、发现的计划偏差
- 接不上装配链的能力不要静默合入（experimental + 登记，见 task-guide）
```

主代理派发实现时，把 §5.3 设计中该路的第 4–7 条原样贴进「设计切片」，避免子代理重新设计。

---

## 9. 与 task-guide 的分工

| | `v2_plan.md`（本文） | `task-guide.md` |
| --- | --- | --- |
| 何时读 | 每次开聊最先读 | 核对、进行中、收尾时遵守 |
| 选哪一波 | §3–§4 | 不负责 |
| 研究 → 设计 → 派发 | §5–§8 | §7 只给并行原则 |
| 红线 / 迁移 / 测试 / key / 报告格式 | 引用 | 事实源 |
| 最小启动提示词 | §2（含必填模型） | §1 仍可用于「已选波次、不走编排」的窄任务 |

窄任务（例如「只修 pawork-net 一条 golden」）可以继续用 task-guide §1，不必走本文三段编排。**阶段波次开发默认走本文。**

---

## 10. 主代理自检清单（派发前）

- [ ] 子代理模型已由用户指定，且按 §7 能落到 `Task` 参数
- [ ] 本次恰好一个波次，写入集已写清
- [ ] 依赖阶段为 🟢；本波契约已对照 design.md §3.2
- [ ] 研究三路（或可省略的 R2）已回传后再写设计
- [ ] 设计未改冻结契约形状；冲突已升级而不是自行拍板
- [ ] 实现并行度与任务书该波一致；契约/装配未被拆并行
- [ ] 收尾会更新本文 §3，且不会顺手开下一波

---

## 11. S8+ 并行与前置注解（2026-08-17 分析）

> 本节基于 `plan/` S8–S12 任务书、[ROADMAP.md](ROADMAP.md)、[design.md](docs/design.md) §3.2、[gui-design.md](docs/gui-design.md) §5 与 [multi-account-quota-plan-merge.md](docs/research/multi-account-quota-plan-merge.md) §4 的当前实态分析，只加注解，不改 §3 指针与 §4 自动选择。各任务书均无「可在前置阶段完成前开工」的授权句，「可前置」项为结构推断；启用时走 §2「范围覆盖」（§4 规则 4）并由用户逐项确认。

### 11.1 硬前置与并行 / 前置总表

| 阶段 | 硬前置（实态） | 阶段内并行 | 可前置项 |
| --- | --- | --- | --- |
| S8 git checkpoint | S3 🟢（写工具在位，diff 预览本就留给 S8）；S1 事件已在位；run_command/S4 非前置；S7 不阻塞 CLI 验收 | 波 A 并行 ×2（`pawork-git` / `pawork-blob-store`，golden 先行）；波 B 串行（engine/app/cli 接线 + 冒烟） | **波 A 两包现在即可开工**（只依赖 S3）；CLI 路径功能上不等 S7 |
| S9 mcp/resources | S2 🟢 工具注册面；S6 config 凭证链（波 C 已接线；S6 挂账的 OAuth 临期 refresh 非前置） | 波 A 并行 ×3（mcp/resources/config 完整化）；波 B 并行 ×3（compat 依赖 mcp 薄类型；session 导入器、workspace file-index 不依赖）；波 C 串行（engine 注入 + cli + 冒烟） | **波 A 三包 + 波 B 的 session 导入器、workspace file-index** 可前置（不依赖 mcp 薄类型与 S7 壳） |
| S10 serve/clients | S7（本机 `gui serve` 已通）；协议收口（10a 波 A 串行）是硬前置；未把 S8/S9 列为依赖 | 10a 波 B 并行 ×3（app/transport/sdk）；10b 并行 ×4（多客户端 / Desktop 增量 / ACP / PTY+lifecycle）；收口串行 | 任务书无前置授权；`channels` 的 codex/claude/remote-control 明确留给后续阶段 |
| S11 workflow/control | S10 整阶段（app/cli 正式化 + Event Hub） | 波 A 控制面三包 ∥ 波 B 工作流三包；波 C orchestration（波 A trait + 波 B `plan`，因 teams）；波 D host 接线单一 owner 串行 | 任务书无前置授权；结构耦合弱的是波 A/B 库迁移 + golden 先行（`dedup_key` / audit JSONL），仍挂在 S11 内 |
| S12 全项目 Code Review | S0–S11 状态、延期项与证据已回写；无新功能 | CR-01～CR-09 按主审路径并行，CR-09 最后做跨报告一致性收口 | **只读准备可前置**（见 11.4）；不实现、不测试、不发布 |
| 阶段外：多账户并入 plan | 前置已满足（D1–D8 已于 2026-08-14 确认） | 纯文档，与任意阶段不冲突 | **可随时开启**；写入集 plan/S2/S5/S6/S9/S11 + design.md + ROADMAP，全部 plan 顺带只删不加核减测试 |

### 11.2 S8 ∥ S9：唯一的跨阶段并行窗口

- ROADMAP 只写 S8 依赖 S3、S9 依赖 S2 + S6；两份任务书互不列对方为前置，无阶段依赖边。
- **可并行**：S8 波 A（git/blob）∥ S9 波 A（mcp/resources/config）∥ S9 波 B 的 session/workspace，写集不相交。
- **不能整阶段并行**：两边收口都动 `pawork-cli`（S8 `diff`/`rollback` vs S9 `mcp`/`import`/`@`）；S8 波 B 与 S9 波 C 还都动 `pawork-engine`（不同位点）；GUI 同改 `apps/desktop` 不同面。
- 若真开双线：库包并行没问题，收口波（engine/cli 接线 + 冒烟）必须串行或先定文件级切分——任务书未写冲突仲裁。

### 11.3 GUI 跨阶段串行点（gui-design §5 同壳加面）

| 组件 | 触碰阶段 | 处理 |
| --- | --- | --- |
| `InspectorToolTabs` 顶层 tab strip | S8 Changes、S10 Terminal | 串行 |
| `ActivityPopover` 本体 | S8 Changes 分区、S11 Agent 分区 | 串行（分区语义可叠加） |
| `Composer` | S9 `@file`、S10（README §6 归 S9/S10 任务书） | 串行 |
| `RunStatusBar` | S7 已有权威字段、S11 补完整 quota | 串行 |
| `TaskRail` | S8+ 均未点名（S10 Fork 最像落点但未登记） | 无冲突 |

S8 Changes 的 CLI 路径可前置，但 **Changes GUI 应等 S7 波 C 壳收口**：gui-design 只保证 S7 留槽，加面以「该阶段 Core 投影 + Host capability」为准（前置投影依赖本阶段投影，见 `design/README.md` §5.1），壳未收口时文档未授权先加面。

### 11.4 S12 可前置的只读准备

1. **审查清单冻结**：按 [S12 任务书](plan/S12-project-code-review.md) 固定 CR-01～CR-09 的主审路径，避免跨包重复 finding。
2. **状态/证据索引**：整理 S0–S11 的计划条目、生产调用点与既有验收记录，只建索引，不重跑测试或冒烟。
3. **已知缺口归一**：以 ROADMAP §3.2 K-01～K-10 为基线，合并重复描述但不提前判定严重度。
4. **发布相关决策移出**：License、三平台矩阵、全量门禁、模型评估报告、benchmark 与发布波次不属于当前 S12；审查整改后若用户决定发布，再另立任务。

### 11.5 冻结契约激活时点（design.md §3.2 中 S8+ 相关）

| 契约 | 激活 | 是否提前激活 |
| --- | --- | --- |
| blob 格式（`PWB1` + protected AEAD，ADR-032） | S8 | 否 |
| GUI 协议（帧 ADR-036、headless-json、core-api） | S7 最小激活 / S10 收口 | 是（S7 已激活完整形状，当前只消费对话子集） |
| config schema | S0 最小 / S9 完整 | 部分（S0 已按 V1 字段读三层） |
| 控制面契约（usage `dedup_key`、audit JSONL） | S11 | 否 |

### 11.6 文档缺口（本次分析发现，未臆造，留待对应任务决策）

- S8/S9 未规定 Changes / `@` / Resources 的 GUI 协议命令与投影字段。
- 未写 S8 ∥ S9 整阶段并行许可与收口冲突仲裁。
- 未写 S6 仍 🔵 时能否改 `pawork-config`（S9 波 A 含 config 完整化；S6 挂账项未点名 config）。
- S9 包表未列 `pawork-app` / `apps/desktop` / `pawork-engine`（engine 只出现在波 C 一句）。
- gui-design.md 未写 S8+ 各面的前置投影清单（实态：依赖该阶段 Core 投影）。
