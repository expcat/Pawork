# ADR-039:V3 包合并布局(37→21)与不合并清单

- **状态**:Accepted(用户 2026-08-19 确认)
- **日期**:2026-08-19

## 背景

V2 收官 39 成员([v2-summary.md](../v2-summary.md)),R0 裁决后 37 成员([ADR-038](ADR-038-inventory-and-product-shape.md))。V3 计划原则要求结构收敛至 **21 成员(19 库 + 2 应用)**([ROADMAP.md](../../ROADMAP.md) §1);终局包清单与来源映射见任务书 [plan/R1](../../plan/R1-package-consolidation.md) §1,结构性判定依据见其 §2。任务书证据经 2026-08-18 波 A 三路只读核查重验,漂移已回写任务书:

- api 消费者实为 **11 个**(原估 ~13),全部同时依赖 domain;api features 已收敛为空锚点 `plugin = []`,src 零门控,`async-trait` 已在用,`ReasoningEffort` 定义在 domain、api 仅重导出。
- diagnostics 对外仅 `Redactor`/`RedactingFmtLayer` 两活符号,唯一消费者 `apps/pawork`;两测试在位可随迁。
- 现有红线断言(desktop deny-list、rmcp 隔离、no_provider_branch)均不点名 api/diagnostics 包名;engine domain-only 断言尚不存在,波 E 建立。

参照:codex-rs 布局纪律(扁平 + 统一前缀 + 集中依赖声明,[references.md](../references.md) §7.1 R1 行)——**只抄纪律不抄粒度**(其 134 成员微 crate 增殖为反面教材,Pawork 方向相反)。

## 决策

### D1 — 目录布局:扁平 `crates/<短名>` + `apps/<name>`

19 个库平铺 `crates/`,目录用去 `pawork-` 前缀的短名(`crates/domain`、`crates/storage`、`crates/control-plane` …);包名保持 `pawork-` 前缀不变——`use` 路径零变更,仅 Cargo.toml 的 path 变化。2 个应用维持 `apps/pawork`、`apps/desktop`(现状即终局)。

- 否决支:保留 13 个功能域目录——21 成员规模下多数域只剩 1–2 包,域目录成纯噪音。
- **执行方式(对任务书 §3「顺势 git mv」字面的修正)**:`git mv` 目录迁移**集中在波 E 一次完成**,与 members glob 切换、design.md §2 重写同步;波 A–D 只做内容级合并(解散包内容平移进幸存包现有目录)。理由:集中迁移每条 path 引用只改一次,避免跨波反复抖动与「两 glob 并存」过渡态;波 A–D 写集合不含根 Cargo.toml,收口边界干净。`git log --follow` 抽查在波 E 收口执行。

### D2 — 不合并清单(固化,以下保持独立包)

`policy`、`exec`、`auth`、`git`、`engine`、`protocol`、`testkit`、`transport`、`orchestration`、`workflow`。

结构性理由:

- **policy**:tools→workspace→policy 依赖链存在,并入任何含 tools 的包即成环;`PolicyDecision`/`ApprovalMode` 是冻结契约与安全红线回归锚。
- **exec**:零内部依赖自含,5 个消费者,平台执行面独立演进(R7 沙箱)。
- **auth**:Secret 审计边界;mcp/app 两个异质消费者。
- **engine**:纯执行核,R1 收口后只依赖 domain,波 E 建立依赖断言。
- **protocol**:冻结帧契约 + typegen 链载体。
- **testkit**:dev-only,不进生产二进制闭包。
- **git / transport / orchestration / workflow**:各自独立消费面与 feature 组合(orchestration 的 `git` feature、transport 的 `protocol` feature),并入即拖泥带水。

### D3 — api→domain 的 GUI 侧口径

api 并入 domain 后,desktop 的编译闭包(经 client→domain)将包含 provider/tool 契约**类型**。**不违反「GUI 不加载 Core」红线**:红线指 GUI 进程不在运行时装配/执行 Core 服务、不直接访问 Provider/数据库/工具(运行语义);纯类型出现在编译闭包不构成运行时加载,且 domain 纯净红线(不依赖 GUI/SQLite/HTTP/Keychain/Git/具体 Provider)对迁入类型继续成立(依赖树可断言)。serde 形状(tag=`type`/content=`data`/snake_case)逐字节不变。

### D4 — 合并映射与解散清单

终局 21 包与来源映射以任务书 §1 表为准。解散 16 包:`pawork-api`、`pawork-net`、`pawork-provider-core`、`pawork-sqlite`、`pawork-session`、`pawork-blob-store`、`pawork-resources`、`pawork-config`、`pawork-compat`、`pawork-mcp`、`pawork-quota`、`pawork-provider-control`、`pawork-gui-server`、`pawork-channels`、`pawork-sdk`、`pawork-diagnostics`,外加 `apps/protocol-probe`(转 client 测试/example)。波 A 执行其中 api→domain 与 diagnostics→apps/pawork 两项;解散性质为**平移**(git 历史 + tag `v2-final` 兜底),非删除语义。

### D5 — 编译粒度代价(明示取舍)

providers/storage 单体化后,「改一个 adapter 重编整包」。接受该代价,换依赖治理、feature 收敛与纪律简化。**实测数据补录**:波 B/C 收口时以 touch-单文件 + `cargo check -p` 计时(合并前基线 vs 合并后),结果追加到本 ADR「落实记录」。

### D6 — 合并后包内纪律(原跨包纪律降级为模块纪律的清单)

- provider-core 不依赖 net → providers 包内模块可见性(`net/` 不对 `registry/` 等暴露)+ 定向测试。
- rmcp 隔离断言 `public_sources_do_not_mention_rmcp` 随迁为 tools 包 `mcp/` 模块级测试。
- storage feature 分层:`sqlite` 基座常开,`session`/`blob` default-on;control-plane 以 `default-features = false, features = ["sqlite"]` 只取 Actor 面。
- engine domain-only 断言、desktop deny-list 更新、`cargo tree` 无环与闭包断言:波 E 建立/更新。

### D7 — 契约保护(golden 先行)

- 波 A 前置:`ProviderStreamEvent` 13 变体、`ProviderError`、`CanonicalModelRequest`、`ToolResult` 目前仅有内存 roundtrip、**无检入字节级 golden**;先在 `pawork-api` 补 fixture 跑绿,再整组平移 domain,迁移前后 JSON 零 diff。`ResolvedCredential` 无 serde(仅 Debug 脱敏测),随迁。
- 迁入 domain 的类型**不加** ts-rs derive(不进 typegen 导出集);`pawork-domain/typegen` feature 链与 `schemas/` 三产物保持。
- 全阶段 serde/磁盘/线上形状零 diff;合并不裁剪(整组平移,字段可闲置);各冻结契约 golden(信封/帧/DDL/PWB1/config/audit/MCP)随模块平移并保绿。

## 后果

- 波 A 后 members 37→35(撤 api、diagnostics);波 B/C/D 继续合并;波 E 定稿 21 并完成目录迁移、design.md §2 重写、README 与 AGENTS.md 成员数回写。
- 每波 `cargo tree` 无环 + `-p pawork` 闭包只减不增;Cargo.lock 逐波刷新。
- 微包间编译器强制纪律下沉为模块纪律 + 定向测试(D6),强制力减弱——为收敛 21 成员目标接受的代价。
- 目录迁移集中波 E 后,波 A–D 期间「包位置 ≠ 终局位置」为已知过渡态,文档(design.md §2)以 V2 实态为准直至波 E 重写。

## 落实记录

### 波 A(2026-08-19,api→domain + diagnostics 撤包)

- **golden 先行(D7)**:`ProviderStreamEvent` 13 变体 / `ProviderError` / `CanonicalModelRequest` / `ToolResult` 字节级夹具先于迁移在 `pawork-api` 建立并跑绿,随后随类型整组平移至 `foundation/domain/tests/{contract_golden.rs,fixtures/}`;迁移前后 JSON 逐字节零 diff(迁移后同 fixture 全绿)。
- **api→domain**:lib.rs(951 行)+ tool.rs(254 行)→ `provider_api`/`tool_api` 两模块,domain 根 re-export 保持原符号路径(`pawork_domain::CanonicalModelRequest` 等);domain 增 `thiserror` 依赖与 `plugin = []` F41 复活锚(ADR-038 D14 语义随迁);未给迁入类型加 ts-rs derive,`pawork-domain/typegen` 链不变。11 个消费者(74 文件)`use`/Cargo.toml 切至 `pawork-domain`(net 的 `http` feature 只留 `dep:pawork-domain`);`pawork_api_key*` 环境变量名不受影响。`foundation/api` 撤包。
- **diagnostics 撤包**:`Redactor`/`RedactingFmtLayer` 与两测试迁 `apps/pawork/src/redact.rs`(宿主增 `regex` 依赖);`foundation/diagnostics` 删除。
- **验证**:`cargo test -p` domain(含迁移后 golden 4+信封 golden 3)/cli(100)/mcp(59,含 rmcp 隔离断言)/quota/auth/engine/testkit/app/tools/providers/provider-core/net/pawork(bin,脱敏 2)全绿;protocol 含 typegen 全绿;desktop 与 protocol-probe `cargo check` 绿;members 37→35;`cargo tree -p pawork` 闭包 817→800 行,无环、无 api/diagnostics。
- **遗留**:D5 编译粒度实测数据待波 B/C(providers/storage 单体化)收口补录;design.md §2、README、AGENTS.md 成员数回写在波 E。

## 相关

- [plan/R1-package-consolidation.md](../../plan/R1-package-consolidation.md)(任务书:目标包清单、波次拆分、退出标准)
- [ROADMAP.md](../../ROADMAP.md) §2 R1 行
- [ADR-038](ADR-038-inventory-and-product-shape.md)(R0 裁决;D8 已预告 RedactingFmtLayer/Redactor 迁宿主)
- [v2-summary.md](../v2-summary.md) §4(冻结契约)、§5(S13 拍板)
- [references.md](../references.md) §7.1 R1 行(codex-rs 布局纪律)
