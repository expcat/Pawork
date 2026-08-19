# R1 — 包合并 37→21

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R1 行。把 R0 裁决后的 workspace 从 37 成员收敛到 **21 成员(19 库 + 2 应用)**（V2 收官时 39 成员,R0 归档 memory/review 并裁剪多包后余 37）,并以 ADR-039 定稿目录布局。只动代码组织,不动任何 wire/磁盘格式;golden 与测试随模块平移。
>
> 证据来源:两路包合并分析(基础核心域 21→12、宿主控制面域 18→9,2026-08-18);执行时按 [v3_plan.md](../v3_plan.md) §5.2 重验消费者与依赖。

## 1. 目标包清单(终局)

| # | 目标包 | 来源(V2 包) | 关键模块布局 | 依赖方向 |
| --- | --- | --- | --- | --- |
| 1 | `pawork-domain` | domain + **api** | 现有模块 + `provider_api/`(ModelProvider、CanonicalModelRequest、ProviderStreamEvent 13 变体、ProviderError、ResolvedCredential)+ `tool_api/`(AgentTool、ToolResult);删除死 feature,保留 `typegen` | 无内部依赖(新增 async-trait) |
| 2 | `pawork-protocol` | 不变 | app/codec/handshake/headless/adapter/client_auth/typegen;R3 增 `projection/` | → domain |
| 3 | `pawork-testkit` | 不变(dev-only) | MockProvider/MockTool/契约断言 | → domain |
| 4 | `pawork-policy` | 不变 | decision/engine/mode/path/shell(安全内核) | → domain |
| 5 | `pawork-exec` | 不变 | process/sandbox/pty/os/tree/cancel(零内部依赖自含) | 无内部依赖 |
| 6 | `pawork-tools` | tools + **mcp** | 八工具 + scheduler + `mcp/`(capabilities/codec/config/manager/oauth/sandbox/security/transport) | → domain、exec、policy、workspace、auth |
| 7 | `pawork-workspace` | workspace/core + **resources** + **config** + **compat** | `service/`+`path/`+`file_index/`、`resources/`、`config/`、`import/`(原 compat 五来源) | → domain、policy |
| 8 | `pawork-storage` | **sqlite** + **session** + **blob** | `sqlite/`(Actor+migration 框架)、`session/`、`blob/{artifact,checkpoint,protected}` | → domain |
| 9 | `pawork-providers` | **net** + **provider-core** + **adapters** | `net/`(http/sse/retry)、`registry/`+`pricing/`+`usage/`+`negotiate/`+`reasoning/`、`channels/`(六通道,feature 保留) | → domain |
| 10 | `pawork-auth` | 不变 | Secret 后端/OAuth/脱敏/解析链(Secret 审计边界) | → domain |
| 11 | `pawork-git` | 不变(R0 已裁剪) | Diff/Status/GitService/GitRunner/HunkStage/worktree/merge | → domain、exec |
| 12 | `pawork-engine` | 不变 | tool_loop/session_turn/context/cancel/appender(只依赖 domain,保持) | → domain |
| 13 | `pawork-workflow` | 不变(R0 后仅剩 plan+task ≈1.75k) | plan/task 纯 reducer | → domain |
| 14 | `pawork-orchestration` | 不变(R0 已去 teams) | supervisor/budget/lifecycle/merge/task_graph/worktree/identity | → domain、workflow、control-plane、git(opt) |
| 15 | `pawork-control-plane` | core + **quota** + **provider-control 被消费核心** | 现有六模块(R0 裁剪后)+ `quota/`(整包平移)+ `credential/`(lease/pool ≈3.3k) | → domain；包内可选 `sqlite` feature，不依赖 storage |
| 16 | `pawork-transport` | 不变(R0 已去 remote) | local(UDS/named pipe)+ memory | → protocol(opt) |
| 17 | `pawork-app` | app + **gui-server** | 领域宿主 + `gui_server/`(GuiServer/ConnectionManager/GuiHost trait) | 原 app 依赖 + transport |
| 18 | `pawork-cli` | cli + **channels** | 21 子命令 + `channels/acp/`(AcpHost 四件套) | 原 cli 依赖(gui-server 改经 app) |
| 19 | `pawork-client` | gui-client + **sdk** | framed 连接面 + `headless/`(原 sdk);probe 9 场景改为本包 `tests/` + live 模式 `examples/probe.rs` | → domain、protocol、transport |
| 20 | `pawork`(bin) | apps/pawork + **diagnostics 活符号** | composition root + `redact.rs`(RedactingFmtLayer/Redactor 随迁,测试随迁) | → cli |
| 21 | `pawork-desktop`(bin) | 不变 | 四层 ui/projection/controller/platform(R8 再组件化) | → client、gpui |

**解散/摘除**:`pawork-api`、`pawork-net`、`pawork-provider-core`、`pawork-sqlite`、`pawork-session`、`pawork-blob-store`、`pawork-resources`、`pawork-config`、`pawork-compat`、`pawork-mcp`、`pawork-quota`、`pawork-provider-control`、`pawork-gui-server`、`pawork-channels`、`pawork-sdk`、`pawork-diagnostics`、`apps/protocol-probe`(memory/review/三域等已在 R0 归档)。

## 2. 关键判定依据(合并 / 不合并的结构性理由)

- **api→domain**:api 的 11 个消费者(2026-08-18 波 A 核查重验,原估 ~13)全部已依赖 domain;features 已收敛为空锚点 `plugin = []`、src 零门控(`provider`/`tool` 已于 R0 D14 移除);`ReasoningEffort` 本就在 domain 由 api 重导出。desktop 将编译 provider trait 纯类型——不违反「GUI 不加载 Core」(红线指 Core 运行时);ADR-039 里明示该取舍。
- **policy 必须独立**:tools→workspace→policy 链存在(`crates/workspace/src/path.rs` 生产引用 3 处、测试 2 处),policy 若并入含 tools 的包即成环;且 `PolicyDecision` 是冻结契约、安全回归锚。
- **exec/auth 必须独立**:exec 零内部依赖自含(`crates/exec/src/lib.rs:3`)、5 个消费者;auth 是 Secret 审计边界且有 mcp/app 两个异质消费者。**写入 ADR-039「不合并清单」:policy、exec、auth、git、engine、protocol、testkit、transport、orchestration、workflow。**
- **gui-server→app 而非 cli**:app 实现 GuiHost trait,并入 cli 会造成 cli→app→cli 循环。
- **compat→workspace**:compat 依赖 config+policy+resources+domain,全是宿主侧;放 clients/ 属目录错位。
- **storage 三合一带 feature 分层**:`sqlite` 基座常开,`session`/`blob` default-on,`checkpoint`/`compaction`/`protected` 沿用;control-plane 经实态核查对 storage Actor 零引用，已移除死依赖；orchestration 消费 control-plane 时保持 `default-features=false`，避免引入 rusqlite。
- **probe→client tests**:`--self-test` 的进程内装配与 client dev-deps 完全同构;live 模式保留为 example bin,不占 members。不能并入 cli:会把 testkit 泄漏进生产二进制。

## 3. ADR-039 决策点(波 A)

1. **目录布局**:采用扁平 `crates/<name>` + `apps/<name>`(21 成员规模下 13 个功能域目录成噪音;对齐 Rust 社区惯例)。目录 `git mv` 按 ADR-039 D1 的落实修正集中在波 E 一次完成，并以 `git log --follow` 抽查。
2. **api 并入 domain 的 GUI 侧口径**(见 §2)。
3. **编译粒度代价**:providers/storage 单体化后「改一个 adapter 重编整包」;增量构建实测数据记入 ADR。
4. **合并后包内纪律**:原跨包纪律降级为模块纪律的清单(provider-core 不依赖 net → 模块可见性 + 定向测试;rmcp 隔离断言 `public_sources_do_not_mention_rmcp` 随迁为 tools 模块级测试)。

## 4. 波次拆分

| 波 | 内容 | 写入集（执行时路径） | 并行度 |
| --- | --- | --- | --- |
| A ✅(2026-08-19) | ADR-039(布局 + 不合并清单 + 取舍)用户确认;api→domain(契约包,golden 先行);diagnostics 活符号迁 `apps/pawork` 并撤包 | docs/adr/、foundation/{domain,api,diagnostics}、apps/pawork、全部引用 api 的 Cargo.toml/use | 串行(契约包单一 owner) |
| B ✅(2026-08-19) | 三大合并:storage(sqlite+session+blob)∥ providers(net+core+adapters)∥ workspace(core+resources+config+compat) | storage/*、foundation/sqlite、foundation/config、providers/*、net/、workspace/*、clients/compat;下游:storage 路修 cli/gui-client/protocol-probe/control-plane Cargo.toml,workspace 路修 mcp/tools,host/app 装配缝由主代理串行收口 | 并行 ×3(写入集不相交;下游 use 修复各自负责) |
| C ✅(2026-08-19) | tools(+mcp)∥ control-plane(core+quota+provider-control 核心) | execution/tools、extensions/mcp、control-plane/* | 并行 ×2 |
| D ✅(2026-08-19) | host 与 clients:app(+gui-server)∥ cli(+channels)∥ client(+sdk、probe→tests/example) | host/{app,gui-server}、host/{cli,channels}、clients/{gui-client,sdk}、apps/protocol-probe | 并行 ×3(app 与 cli 的接缝——cli 改经 app 取 GuiHost——由 app 路先定 trait 位置,cli 路后接;若冲突改串行) |
| E ✅(2026-08-19) | 收口:members 定稿 21;剩余未动包 `git mv` 到新布局;design.md §2 重写为 V3 布局;README 仓库结构更新;依赖红线断言更新(desktop deny-list、`cargo tree` 无环);全量受影响包定向测试 | 根 Cargo.toml、全目录 mv、docs/design.md、README.md、各红线测试 | 串行(主代理) |

## 5. 契约与 golden 随迁清单(改动前先确认 golden 在位)

| 契约 | golden 位置(迁移后) |
| --- | --- |
| 事件信封 v1 / schema v10 | domain(类型 + 信封字节 golden:`crates/domain/tests/events_golden.rs` + fixtures)+ storage `session/`(DDL/迁移/export/快照测试锚,无独立 SQL golden) |
| Provider 契约 13 变体 serde | domain `provider_api`(**形状不变,tag/content 不动**) |
| blob `PWB1` + checkpoint | storage `blob/` golden |
| GUI 帧 / headless-json / `schemas/` typegen | protocol(不动;typegen 链 `pawork-domain/typegen` 保持) |
| `PolicyDecision` / `ApprovalMode` | policy(不动) |
| config 六层矩阵 | workspace `config/`(47 测试随迁,2026-08-19 波 B 核查重数) |
| usage `dedup_key` / audit JSONL | control-plane(fixtures 随迁) |
| MCP 契约(64 测试)+ rmcp 隔离断言 | tools `mcp/` |

> 2026-08-18 波 A 核查补注(golden 缺口,已于 2026-08-19 波 A 落实):`ProviderStreamEvent` 13 变体、`ProviderError`、`CanonicalModelRequest`、`ToolResult` 原仅有内存 roundtrip 覆盖;波 A 已按「golden 先行」补齐字节级夹具并随类型整组平移，终态位于 `crates/domain/tests/{contract_golden.rs,fixtures/}`,迁移前后零 diff。diagnostics 两测试(Redactor/RedactingFmtLayer)已随迁 `apps/pawork/src/redact.rs`。

> 2026-08-19 波 B 三路核查补注(实态重验,已按实态执行):
> - 写入集补 `foundation/config`(config 实态在 foundation/,不在 workspace/);compat 实态在 clients/(目录错位,本波并入 workspace `import/`)。
> - `workspace/core` 对 policy 的生产依赖为 3 处(path.rs:11/45/87),另 2 处仅测试;任务书「5 处」以实态为准。
> - storage feature 实态:session/blob 原无独立 feature,blob `default = []` 且 protected/checkpoint opt-in,session compaction opt-in;control-plane 的 `pawork-sqlite` 为死依赖(零源码引用,`SqliteUsageLedger` 自开 rusqlite 连接)。本波落地 ADR-039 D6 分层:`default = ["session","blob"]`,compaction/checkpoint/protected 保持 opt-in;control-plane 直接移除死依赖(prod optional + dev),「只取 Actor」以不依赖达成。
> - providers:`channels/` = adapters 现有通道模块(anthropic/、chatgpt.rs、xai.rs、api_key.rs)内聚重组,cfg feature 门控原样保留;host 通道表(`host/app/src/channels.rs`)不在本波写入集。adapters 原 `usage.rs` 薄 re-export 与 core `usage` 合一。
> - 信封字节 golden 实态在 domain(非 session);v2-summary §4「golden 在 pawork-session」与 design.md:33,47 的过时表述由波 E 统一修正。desktop deny-list 与 gui-design.md 的包名引用(session/sqlite/provider-core)同样在波 E 更新。
> - host/app 同时是三路下游,装配缝(Cargo.toml + use)由主代理在三路完成后串行收口,不并行派发。

> 2026-08-19 波 C 三路核查补注(实态重验,已按实态执行):
> - MCP 测试实态 **64** 个(本任务书「59 测试」为 2026-08-18 快照),全部在 src 模块内(无 tests/);rmcp 隔离断言在 mcp/src/lib.rs,随迁 tools `mcp/` 后扫描根改 `src/mcp/`、仍只豁免 codec.rs。
> - provider-control 实态为 `lease.rs` + `lib.rs`(池/闸门在 lib.rs)共 3107 行,**无 `credential/` 目录**——`credential/` 是 control-plane 内的目标模块名;lease 状态机无外部消费者(外部只消费池/闸门根类型),按零裁剪整组平移。
> - control-plane core 自带 `rusqlite` optional feature(`default=["sqlite"]`,`SqliteUsageLedger` 自开连接),**不依赖 pawork-storage**;§1 #15 终态依赖已按实态回写。quota/provider-control 均不依赖 rusqlite/storage。
> - tools 现无 auth 依赖;#6 终态依赖里的 auth 随 mcp 并入获得。mcp/quota/provider-control 三包均无 [features] 段;provider-control 被 app/orchestration 以 `default-features = false` 引用为历史空操作,orchestration 切到 control-plane 后必须保持 `default-features = false`(防 rusqlite 传染编排闭包)。
> - 写入集外下游仅 host/app(三包)与 agents/orchestration(仅 provider-control);cli/desktop 不直连。usage `dedup_key` 与 audit JSONL golden 锚定在 core 自身,本波只新增模块不动该面。
> - 删除三源目录(extensions/mcp、control-plane/{quota,provider-control})与 host/app 装配缝、根 `extensions/*` glob 移除统一在主代理收口串行执行(保持两路并行期间 workspace 可解析)。

> 2026-08-19 波 D 三路核查补注(实态重验,已按实态执行):
> - workspace 实测 **25 members**(开篇「37」为 R0 后快照);本波解散 gui-server/channels/sdk/protocol-probe 后恰为 21。cli 顶层子命令实态 **21** 个(终态 `crates/cli/src/lib.rs:87`,§1 已回写)。
> - GuiHost 接缝实态:trait 在 `host/gui-server/src/lib.rs:43`,app 已实现 `GuiHostAdapter`(`gui_host.rs:570`)但**不 re-export trait、无 transport 依赖**;cli 四处直接 `use pawork_gui_server::GuiHost`(chat/gui/headless/adapter)。本波落地目标态:trait 随包平移为 `pawork_app::gui_server::GuiHost`,cli 改经 app;app 终态依赖 = 原依赖 + transport(吸收 gui-server 获得)。
> - probe 与 client dev-deps **不完全同构**(任务书§2 表述过时):probe 用 MemoryTransport 且 testkit/app/gui-server 在 prod 依赖;client `tests/contract.rs` 用 LocalTransport+UDS、同组依赖在 dev-deps。live 模式(`--connect`/`--live-two-gui`/`--live-pty`)在同一 bin 上,非 example。按目标态执行:9 场景(MemoryTransport harness 形态保留)→ client `tests/`,live → client `examples/probe.rs`。
> - channels 实态:`acp/{adapter,command_host,host,wire}` 四公开模块 + `pub(crate) map.rs`;workspace 唯一消费者为 cli。sdk 实态:**零外部消费者**;稳定面 PaworkClient/PaworkOptions/EventSubscription/SdkError{,Kind}/Transport/spawn_pawork/SDK_API_VERSION/SDK_VERSION + experimental::CompatOutcome + protocol reexport,平移为 `pawork_client::headless` 后保持 `pub`,夹具(clients/sdk/tests/fixtures 5 件 + client_tests 20 测 + spawn_e2e 3 测)随迁。
> - 契约面无新增缺口:GUI 帧/headless-json 字节 golden 在 protocol(本波不动);typegen 链不引用写入集内包,不改 typegen 输入集。行为锚点随迁:EventHub Snapshot/ReplayUnavailable(gui-server tests session.rs:586、multi_gui_runtime.rs:612 → app 侧;client contract.rs:255/382;probe 9 场景)、F33 fail-closed(cli headless.rs:371,包内移动不动)、ACP golden(channels fixtures → cli)。UDS 0600/token 锚点在 transport+protocol(写入集外,不动);cli `gui.rs:44-75` 强制 token 接线原样保留。
> - 写入集外本波只动 Cargo.lock(收口时由 cargo 重解析);根 Cargo.toml glob(host/*/clients/*/apps/*)删四源目录后仍可解析,members 定稿与目录扁平化在波 E;docs/design.md、README、AGENTS.md 成员数、desktop deny-list、engine domain-only 断言均属波 E。

## 6. 验证

- 每波:被合并包与全部下游 `cargo check -p` / `cargo test -p`;`cargo tree -p pawork` 闭包不膨胀;`cargo tree` 无循环。
- 波 E:desktop 依赖断言(业务依赖仅 pawork-client)、engine 只依赖 domain 断言、`cargo test -p` 全 21 包冒烟级通过。
- 真实冒烟(矩阵一组):chat 流式 + 工具 + 审批 + `gui serve` + desktop `--probe-smoke`(证明协议/装配未回退)。

## 7. 退出标准

- [x] ADR-039 Accepted;members = 21;目录布局定稿并完成迁移(波 E:19 库 `git mv` 扁平 `crates/`,`cargo metadata` 确认 21 成员)
- [x] §5 全部 golden 随迁且绿;serde/磁盘/线上形状零 diff(波 A–D golden 先行平移各自验收;波 E 73 测试二进制 1644 测绿含全部契约 golden;波 E 自身零序列化代码改动)
- [x] 无循环依赖;红线断言(desktop client-only / engine domain-only / providers core→net / rmcp 隔离)全绿(`cargo tree` 无环、`-p pawork` 闭包 711 行、16 解散包名零残留;生产依赖断言覆盖普通与 target-specific 表、含 package alias;`crates/engine/tests/domain_only.rs` 在位)
- [x] design.md §2 已重写为 V3 布局;README 结构图同步(AGENTS.md/gui-design/v2-summary/task-guide 包名引用同步修正)
- [x] 冒烟通过;v3_plan §3 指针更新(chat 流式/工具/审批/fail-closed 与 `gui serve` 装配全绿;波 E 暴露的 ModelList↔switch_provider 目录不对称、client 事件泵抢命令错误帧已于 2026-08-19 R1 整阶段审查修复并补回归;修复后 `pawork-desktop --probe-smoke` 实测首轮 glm-4.7 完成、切换 deepseek-v4-flash 后第二轮完成、取消/持久化/断线存活均通过)

> 2026-08-19 R1 整阶段审查补录:全 21 包 `cargo check` 与 `cargo test --no-fail-fast` 通过;storage `--all-features`、`--no-default-features` 与 providers `--all-features` 通过;`cargo tree -p pawork` 仍为 711 行且 16 个解散包名零残留,orchestration 闭包不含 rusqlite/storage。除上述两项生产缺陷外,审查还把 desktop/engine 生产依赖测试从有限扫描收紧为 allow-only,覆盖 target-specific、inline/nested `package` alias;providers core→net 扫描收紧为 `net` 标识符零引用。未改变冻结协议、serde、schema 或磁盘形状。
