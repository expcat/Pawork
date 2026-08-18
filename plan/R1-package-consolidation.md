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
| 15 | `pawork-control-plane` | core + **quota** + **provider-control 被消费核心** | 现有六模块(R0 裁剪后)+ `quota/`(整包平移)+ `credential/`(lease/pool ≈3.3k) | → domain、storage(`sqlite` feature) |
| 16 | `pawork-transport` | 不变(R0 已去 remote) | local(UDS/named pipe)+ memory | → protocol(opt) |
| 17 | `pawork-app` | app + **gui-server** | 领域宿主 + `gui_server/`(GuiServer/ConnectionManager/GuiHost trait) | 原 app 依赖 + transport |
| 18 | `pawork-cli` | cli + **channels** | 14 子命令 + `channels/acp/`(AcpHost 四件套) | 原 cli 依赖(gui-server 改经 app) |
| 19 | `pawork-client` | gui-client + **sdk** | framed 连接面 + `headless/`(原 sdk);probe 9 场景改为本包 `tests/` + live 模式 `examples/probe.rs` | → domain、protocol、transport |
| 20 | `pawork`(bin) | apps/pawork + **diagnostics 活符号** | composition root + `redact.rs`(RedactingFmtLayer/Redactor 随迁,测试随迁) | → cli |
| 21 | `pawork-desktop`(bin) | 不变 | 四层 ui/projection/controller/platform(R8 再组件化) | → client、gpui |

**解散/摘除**:`pawork-api`、`pawork-net`、`pawork-provider-core`、`pawork-sqlite`、`pawork-session`、`pawork-blob-store`、`pawork-resources`、`pawork-config`、`pawork-compat`、`pawork-mcp`、`pawork-quota`、`pawork-provider-control`、`pawork-gui-server`、`pawork-channels`、`pawork-sdk`、`pawork-diagnostics`、`apps/protocol-probe`(memory/review/三域等已在 R0 归档)。

## 2. 关键判定依据(合并 / 不合并的结构性理由)

- **api→domain**:api 的 11 个消费者(2026-08-18 波 A 核查重验,原估 ~13)全部已依赖 domain;features 已收敛为空锚点 `plugin = []`、src 零门控(`provider`/`tool` 已于 R0 D14 移除);`ReasoningEffort` 本就在 domain 由 api 重导出。desktop 将编译 provider trait 纯类型——不违反「GUI 不加载 Core」(红线指 Core 运行时);ADR-039 里明示该取舍。
- **policy 必须独立**:tools→workspace→policy 链存在(`workspace/core/src/path.rs` 5 处),policy 若并入含 tools 的包即成环;且 `PolicyDecision` 是冻结契约、安全回归锚。
- **exec/auth 必须独立**:exec 零内部依赖自含(`execution/exec/src/lib.rs:3`)、5 个消费者;auth 是 Secret 审计边界且有 mcp/app 两个异质消费者。**写入 ADR-039「不合并清单」:policy、exec、auth、git、engine、protocol、testkit、transport、orchestration、workflow。**
- **gui-server→app 而非 cli**:app 实现 GuiHost trait,并入 cli 会造成 cli→app→cli 循环。
- **compat→workspace**:compat 依赖 config+policy+resources+domain,全是宿主侧;放 clients/ 属目录错位。
- **storage 三合一带 feature 分层**:`sqlite` 基座常开,`session`/`blob` default-on,`checkpoint`/`compaction`/`protected` 沿用;control-plane 以 `default-features=false, features=["sqlite"]` 只取 Actor 面,避免拖 15k 无关代码。
- **probe→client tests**:`--self-test` 的进程内装配与 client dev-deps 完全同构;live 模式保留为 example bin,不占 members。不能并入 cli:会把 testkit 泄漏进生产二进制。

## 3. ADR-039 决策点(波 A)

1. **目录布局**:推荐扁平 `crates/<name>` + `apps/<name>`(21 成员规模下 13 个功能域目录成噪音;对齐 Rust 社区惯例)。备选:保留域目录。执行方式:每波合并时顺势 `git mv` 到新位置,「先 mv 后改」两步提交,`git log --follow` 抽查。
2. **api 并入 domain 的 GUI 侧口径**(见 §2)。
3. **编译粒度代价**:providers/storage 单体化后「改一个 adapter 重编整包」;增量构建实测数据记入 ADR。
4. **合并后包内纪律**:原跨包纪律降级为模块纪律的清单(provider-core 不依赖 net → 模块可见性 + 定向测试;rmcp 隔离断言 `public_sources_do_not_mention_rmcp` 随迁为 tools 模块级测试)。

## 4. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| A ✅(2026-08-19) | ADR-039(布局 + 不合并清单 + 取舍)用户确认;api→domain(契约包,golden 先行);diagnostics 活符号迁 `apps/pawork` 并撤包 | docs/adr/、foundation/{domain,api,diagnostics}、apps/pawork、全部引用 api 的 Cargo.toml/use | 串行(契约包单一 owner) |
| B | 三大合并:storage(sqlite+session+blob)∥ providers(net+core+adapters)∥ workspace(core+resources+config+compat) | storage/*、foundation/sqlite、providers/*、net/、workspace/*、clients/compat | 并行 ×3(写入集不相交;下游 use 修复各自负责) |
| C | tools(+mcp)∥ control-plane(core+quota+provider-control 核心) | execution/tools、extensions/mcp、control-plane/* | 并行 ×2 |
| D | host 与 clients:app(+gui-server)∥ cli(+channels)∥ client(+sdk、probe→tests/example) | host/{app,gui-server}、host/{cli,channels}、clients/{gui-client,sdk}、apps/protocol-probe | 并行 ×3(app 与 cli 的接缝——cli 改经 app 取 GuiHost——由 app 路先定 trait 位置,cli 路后接;若冲突改串行) |
| E | 收口:members 定稿 21;剩余未动包 `git mv` 到新布局;design.md §2 重写为 V3 布局;README 仓库结构更新;依赖红线断言更新(desktop deny-list、`cargo tree` 无环);全量受影响包定向测试 | 根 Cargo.toml、全目录 mv、docs/design.md、README.md、各红线测试 | 串行(主代理) |

## 5. 契约与 golden 随迁清单(改动前先确认 golden 在位)

| 契约 | golden 位置(迁移后) |
| --- | --- |
| 事件信封 v1 / schema v10 | domain(类型)+ storage `session/`(DDL/迁移/信封 golden) |
| Provider 契约 13 变体 serde | domain `provider_api`(**形状不变,tag/content 不动**) |
| blob `PWB1` + checkpoint | storage `blob/` golden |
| GUI 帧 / headless-json / `schemas/` typegen | protocol(不动;typegen 链 `pawork-domain/typegen` 保持) |
| `PolicyDecision` / `ApprovalMode` | policy(不动) |
| config 六层矩阵 | workspace `config/`(44 测试随迁) |
| usage `dedup_key` / audit JSONL | control-plane(fixtures 随迁) |
| MCP 契约(59 测试)+ rmcp 隔离断言 | tools `mcp/` |

> 2026-08-18 波 A 核查补注(golden 缺口,已于 2026-08-19 波 A 落实):`ProviderStreamEvent` 13 变体、`ProviderError`、`CanonicalModelRequest`、`ToolResult` 原仅有内存 roundtrip 覆盖;波 A 已按「golden 先行」补齐字节级夹具并随类型整组平移 `foundation/domain/tests/{contract_golden.rs,fixtures/}`,迁移前后零 diff。diagnostics 两测试(Redactor/RedactingFmtLayer)已随迁 `apps/pawork/src/redact.rs`。

## 6. 验证

- 每波:被合并包与全部下游 `cargo check -p` / `cargo test -p`;`cargo tree -p pawork` 闭包不膨胀;`cargo tree` 无循环。
- 波 E:desktop 依赖断言(业务依赖仅 pawork-client)、engine 只依赖 domain 断言、`cargo test -p` 全 21 包冒烟级通过。
- 真实冒烟(矩阵一组):chat 流式 + 工具 + 审批 + `gui serve` + desktop `--probe-smoke`(证明协议/装配未回退)。

## 7. 退出标准

- [ ] ADR-039 Accepted;members = 21;目录布局定稿并完成迁移
- [ ] §5 全部 golden 随迁且绿;serde/磁盘/线上形状零 diff(信封、帧、DDL 逐项核对)
- [ ] 无循环依赖;红线断言(desktop deny-list / engine domain-only / rmcp 隔离)全绿
- [ ] design.md §2 已重写为 V3 布局;README 结构图同步
- [ ] 冒烟通过;v3_plan §3 指针更新
