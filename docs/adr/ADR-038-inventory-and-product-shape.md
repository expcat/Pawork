# ADR-038:V3 产品形态与休眠库存裁决

- **状态**:Accepted(用户 2026-08-18 确认,22 项决议按推荐决议执行;波 B 落实时三路实态核查改判 3 项实现细节,见文末「落实改判记录」)
- **日期**:2026-08-18
- **落实日期**:R0 波 A–C(本 ADR Accepted 后逐波落地)

## 背景

V2 收官(S0–S13,存档见 [history.md](../history.md) 第一部分)后,workspace 39 成员约 19 万行 src,其中约 3.3 万行为零消费者休眠库存(近 20%)。V3 计划原则要求「删除优先于门控,门控优先于库存」(原 ROADMAP V3 计划原则,存档见 [history.md](../history.md)),故 R0 一次性拍板产品形态(T10)与全部休眠资产去留(T1),避免后续阶段反复翻案。

证据基础:2026-08-18 五路只读分析(原任务书 plan/R0-inventory-decisions.md §3,已随收口删除,见 git 历史),并于同日经三路只读核查按实态重验;漂移处已回写任务书,本 ADR 以重验后实态为准。归档兜底:git tag `v2-final`(已打,指向 088b539),任何删除均可找回。

## 决策

### 产品形态

**D1 — 单机优先(T10)**。身份维度收缩为可选扩展点,`local/default` 哨兵宇宙不再扩张;多账户 factory 转候选(激活时按新装配面重写,调研结论仍有效,已并入 [references.md](../references.md) 附录 A–C)。依据:control-plane 三包 28.7k 行中生产链路只用 ledger/audit/policy/lease/pool(`host/app/src/control.rs` 装配 `SqliteUsageLedger`/`FileAuditStore`/`InMemoryTenantPolicyEngine`/`InMemoryCredentialPool`);哨兵派生(`control-plane/core/src/usage.rs:26,105`)从未接真实 lease,宿主入账写死 `LEDGER_ACCOUNT = "local/default"`(`control.rs:29`)。否决支:多租户层级(LiteLLM 形态)——无真实消费面,维持成本高。

### 归档(移出 workspace + 删除源目录,tag 兜底,ROADMAP 候选池登记复活条件)

- **D2 — provider-control account-control-v1 九模块(8,476 行)归档**。feature 默认开但宿主零装配(`host/app/Cargo.toml:40` `default-features=false`);包外只用 lease/pool。
- **D3 — provider-control binding.rs + schema/ 归档,legacy.rs 删除**(合计 5,473 行)。包外零引用;`legacy::` 唯一消费方是同批归档的 account-control-v1(`account.rs:340-375`)。`session_bindings` 孤儿表留表登记「预留」(append-only,DDL 不回滚)。
- **D4 — workflow goal/automation/monitor 三域(3,603 行)归档**;domain canonical 事件类型保留(重放红线);`process-exec` feature 随之移除。包外只消费 `pawork_workflow::plan`/`task`。
- **D5 — orchestration teams(2,985 行)归档**。生产侧唯一引用是 ACP 标签映射(`host/channels/src/acp/map.rs:298`);supervisor 有 CLI demo 消费,保留。
- **D6 — pawork-memory(1,134 行)/ pawork-review(1,467 行)整包归档出树**。零反向依赖;`EmbeddingProvider` trait(`foundation/api/src/lib.rs:567`)一并删除(唯一 impl 是包内测试 FixedEmbedder)。
- **D7 — transport remote 模块(3,721 行 TLS)+ MockRemote(731 行)归档**。feature `remote` 全仓无启用方;cli 仅装配 `LocalTransport`(`host/cli/src/ops.rs:113`)。rcgen 随之退出 lock;rustls/tokio-rustls 经 reqwest 保留属预期(2026-08-18 重验修正)。
- **D15 — control-plane/core OTel exporter 四实现、identity_schema(484 行)归档;rbac 保留 `Permission`(orchestration `spawn.rs:11` 在用),`PermissionProfile`/`PrincipalRole` 归档**并同步改 orchestration 测试引用(`supervisor/mod.rs:457`)【落实改判:rbac 三类型全部保留,见「落实改判记录」】。
- **D16 — vcs/git 六休眠服务(1,992 行)归档**:Branch/Stash/Conflict/History/CachedStatus+StatusCache+spawn_invalidator。**保留** Diff/Status/GitService/GitRunner(包内基建)/Head/HunkId + HunkStageService(R8 K-04 消费)+ worktree/merge(orchestration `git` feature)。GUI git 面板转候选。

### 删除

- **D8 — diagnostics experimental(bundle 494 + metrics 225)与零消费 logging 组件(StructuredLogLayer/LogBuffer)删除**;`RedactingFmtLayer`/`Redactor` 保留(宿主消费,`apps/pawork/src/main.rs:6`),R1 迁宿主。
- **D9 — host/app `rate_limit.rs`(532 行,K-07)删除**;`hub.rs:3` 为其预留的序列补洞逻辑随 R4 简化。
- **D10 — storage/session `lifecycle.rs`(697 行)删除**。生产 fork 路径在 `session_tree.rs:117`,不受影响(2026-08-18 重验确认)。
- **D11 — net `jsonl.rs`(285)+ `partial_json.rs`(535)删除**。零调用点。
- **D12 — engine `run_turn` 公开入口删除**(`engine/engine/src/lib.rs:84`),测试改走 tool_loop 入口;生产走 `run_session`。【落实改判:函数本体保留、仅 `pub` 降 `pub(crate)`,见「落实改判记录」】
- **D14 — 死声明清理**:foundation/api features `provider`/`tool`/`plugin`(src 零门控;`plugin` 空数组保留为 F41 语义则显式注释)、execution/tools `encoding_rs`、host/gui-server `futures`。【落实改判:`encoding_rs` 为类型级隐性直接依赖,保留,见「落实改判记录」】
- **D20 — clients/sdk `ide.rs` 空占位(8 行)删除**。
- **D22 — orchestration/recovery deprecated `recover` 别名删除**(全仓唯一 deprecated),保留 `recover_report`。

### 宣告修正

- **D13 — K-08 `ArtifactStreaming` 双端停止宣告**。服务端 `host/gui-server/src/session.rs:462` `ArtifactRead` 固定 unsupported,而握手在 `host/cli/src/gui.rs:71` 宣告能力、`clients/gui-client` `capabilities`:79 无条件包含;desktop/protocol-probe 均未启用 `experimental` 门控。停止宣告 + 门控删除;artifact 流式转候选,R3 registry 就位后低成本接线。

### 保留(消费计划明确或冻结契约)

- **D17 — `CapabilityNegotiator`(465 行)与 registry `caps()`/`ProviderProbe` 保留**,R5 波 C 接线(K-10 载体);包外可见性降 `pub(crate)` 待 R5 定。
- **D18 — `protected.rs` PWB1(1,456 行)保留**:冻结契约 + golden(`storage/blob/tests/golden/pwb1_valid.hex`);R5 波 C 接 ReasoningProtector 成为首个生产消费者。
- **D19 — `Loader::with_session/with_run` 保留**(六层合并冻结契约,S9 承诺;消费在 crate 内测试,`loader.rs:172`)。

### 可见性降级

- **D21 — exec 包外零消费的 pub 平台函数降 `pub(crate)`**(`execution/exec/src/lib.rs:34-44`、`LinuxLandlockPolicy` 在 20-21);集成测试需要的保留并注释。

## 后果

- 归档/删除合计约 3.3 万行(D2–D8、D15、D16 为主);workspace members 39 → 37(memory/review 整包出树),其余为包内模块级裁减。`cargo tree -p pawork` 闭包只减不增,波 C 收口前后快照对比归档。
- 「归档」= 移出 workspace members + 删除源目录;不复制到仓库其它位置;复活条件现登记于 [产品候选](../spec/backlog.md)。domain canonical 事件类型(Plan/Goal/Task/Team 等)一律保留,历史事件可重放。
- `session_bindings` 孤儿表:留表 + 注释登记「预留」,不回滚 DDL(append-only)。
- S13 拍板([architecture.md](../architecture.md) §4)不回退;安全红线定向回归、持久化/重放 golden、协议 golden 在每波收口保持绿。
- 破坏式内部改动(删 public API、feature)允许;磁盘/线上冻结契约形状零变更([architecture.md](../architecture.md) §3.2 清单)。
- 波 A 并行度 ×3(控制面 / workflow+orchestration / transport),波 B ×3,波 C 串行收口——写入集边界见任务书 §4。

## 落实改判记录(2026-08-18 波 B,实态核查驱动)

波 B 执行前三路只读核查(C1/C2/C3)重验证据,以下三项按实态改判,不改变本 ADR 的产品形态与归档方向:

1. **D12 `run_turn`**:任务书原判「生产零调用,删除」不成立——生产内部经 `session_turn.rs:124`、`tool_loop.rs:206`(run_session)、`tool_loop.rs:719`(compaction)调用。落实为:公开入口删除(`pub` 降 `pub(crate)`),函数本体保留。决议意图(公开面无死入口)达成。
2. **D15 rbac**:`PermissionProfile`/`PrincipalRole` 非「仅测试引用」——`TenantPolicy.permission_profile` 是生产字段,`check_permission` deny-first 热路径(`tenant.rs` `principal_role()`,orchestration `supervisor/spawn.rs:137,166` 消费)在用。落实为:rbac 三类型(`Permission`/`PrincipalRole`/`PermissionProfile`)全部保留;OTel exporter 四件与其依赖缝(`export_tenant`/`ExportRecord`,外部零消费)及 `identity_schema` 照原判归档。orchestration 测试零改动。
3. **D14 `encoding_rs`**:非死依赖——chardetng `EncodingDetector::guess()` 返回 `&'static encoding_rs::Encoding`,`execution/tools/src/read_file.rs:205` 的方法解析要求 encoding_rs 为直接依赖(删除后 E0599 实证)。落实为:保留并在 manifest 注释原因。

## 落实改判记录(2026-08-18 波 C,实态核查驱动)

4. **D16 `commit.rs` 补入归档集**:任务书 D16 点名归档 Branch/Stash/Conflict/History/CachedStatus+StatusCache+spawn_invalidator,未提及 `CommitService`/`CommitOptions`(270 行)。波 C 实态核查:crate 外零消费(仅自身测试),保留面不依赖它。按本 ADR「零消费者即归档」总原则一并归档删除;复活条件随 GUI git 面板一并登记 ROADMAP 候选池。
5. **D16 收口验证**:删除 branch/stash/conflict/history/cache/commit 六文件(合计 2,262 行),`pawork-git` 定向测试 57+5 golden 全绿;`cargo tree -p pawork` 闭包 833→817 行,只减不增——notify-debouncer-full 与 notify 8 专属传递依赖(file-id/inotify 0.11/notify-types 2.1 等)随之退出闭包;`ignore`/`parking_lot` 仍被闭包内其它消费者使用,仅退出 `pawork-git` 直接依赖。前后快照存 `/tmp/pawork-r0c/`(易失),对比结论固化于本行。

另:D13 握手宣告点实为五处(cli/gui-client 默认/desktop/probe/契约测试),写入集相应扩及 `host/cli`、`apps/desktop`、`apps/protocol-probe`;冻结面(`GuiCapability` 枚举、schemas/ typegen、`SUPPORTED_API_VERSIONS`)未触碰。

## 相关

- 原任务书 plan/R0-inventory-decisions.md 已随收口删除(git 历史);R0 交付存档见 [history.md](../history.md) R0 节
- [history.md](../history.md)（当时 ROADMAP 未决/候选记录）· [产品候选](../spec/backlog.md)
- 冻结契约与 S13 拍板现行事实源:[architecture.md](../architecture.md) §3.2/§4(原 v2-summary §4/§5 存档见 [history.md](../history.md))
- ADR-033 控制面分离(随 V1 归档,原则继续有效)
