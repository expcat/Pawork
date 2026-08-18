# R0 — 决策收口与休眠库存裁决

> 对应 [ROADMAP.md](../ROADMAP.md) §2 R0 行。V3 的闸门阶段:先以 ADR-038 一次性拍板产品形态与全部休眠库存去留,再按决议归档/删除约 3.3–3.8 万行零消费者代码(近 20% src)。本阶段之后,仓库里不允许再存在「无消费者、无激活排期」的库存代码。
>
> 证据来源:2026-08-18 五路只读分析(补丁式实现全仓扫描、两路包合并分析、依赖用面审计);所有行数与调用点结论在执行时须按 [v3_plan.md](../v3_plan.md) §5.2 重验。

## 1. 目标与非目标

**目标**

1. ADR-038 拍板:单机 vs 多租户产品形态(T10)、每一项休眠资产的去留(T1)。
2. 按决议执行归档/删除;workspace members 收缩;`cargo tree -p pawork` 闭包瘦身。
3. 清理死 feature 声明、死依赖声明、唯一 deprecated 项。
4. K-07(rate_limit)与 K-08(ArtifactStreaming 宣告)裁决落地。

**非目标**:不做包合并(R1)、不升级依赖(R2)、不改任何冻结契约形状、不动 domain 事件类型(重放红线)。

## 2. 前置动作(波 0 开工前)

- 打 git tag `v2-final`(归档兜底;此后任何删除均可从 tag 找回)。
- 确认 S13 拍板([docs/v2-summary.md](../docs/v2-summary.md) §5)不因归档回退。

## 3. ADR-038 决策清单(波 0;推荐决议已列,须用户确认)

| # | 决策项 | 推荐决议 | 依据 |
| --- | --- | --- | --- |
| D1 | 产品形态(T10) | **单机优先**:身份维度收缩为可选扩展点,`local/default` 哨兵宇宙不再扩张;多账户 factory 转候选(激活时按新装配面重写) | control-plane 三包 28.7k 行中生产链路只用 ledger/audit/policy/lease/pool;哨兵派生(`control-plane/core/src/usage.rs:26,105`)从未接真实 lease |
| D2 | `provider-control` account-control-v1 九模块(8,476 行,feature 默认开、宿主零装配) | 归档 | `host/app/Cargo.toml:40` `default-features=false`;demo 只走 lease/pool |
| D3 | `provider-control` binding.rs + schema/ + legacy.rs(5,473 行未门控零引用) | binding/schema 归档;legacy.rs 删除 | 逐符号 rg 零外部消费;`legacy::` crate 内亦无引用;`session_bindings` 孤儿表留表登记(append-only) |
| D4 | `workflow` goal/automation/monitor 三域(3,603 行) | 归档(domain 事件保留) | 全仓无 `pawork_workflow::(goal\|automation\|monitor)` 引用;`process-exec` feature 随之移除 |
| D5 | `orchestration` teams(2,984 行) | 归档 | `AppEvent::TeamEvent` 在 host/app 零生产点(唯一引用是 ACP 标签映射 `host/channels/src/acp/map.rs:298`);supervisor(有 CLI demo 消费)保留 |
| D6 | `pawork-memory`(1,134)/ `pawork-review`(1,467)整包 | 归档出树 | workspace 零反向依赖;`EmbeddingProvider` 唯一 impl 是包内测试 Mock → trait 一并删除(`foundation/api/src/lib.rs:567`) |
| D7 | `transport` remote 模块(3,721 行 TLS)+ `memory/mock.rs` MockRemote(731 行) | 归档;rustls/tokio-rustls/rcgen 三依赖随之退出 | feature `remote` 全仓无启用方;cli 仅用 `LocalTransport`(`host/cli/src/ops.rs:113`) |
| D8 | `diagnostics` experimental(bundle 494 + metrics 225)与零消费 logging 组件 | 删除;`RedactingFmtLayer`/`Redactor`(~200 行 + 测试)保留待 R1 迁宿主 | experimental 无任何 Cargo.toml 激活;宿主只用两个符号(`apps/pawork/src/main.rs:6`) |
| D9 | `host/app/src/rate_limit.rs`(532 行,K-07) | 删除;`hub.rs` 为其预留的序列补洞逻辑(`hub.rs:3-5`)随 R4 简化 | 迁入以来无生产调用 |
| D10 | `storage/session/src/lifecycle.rs`(697 行 lease/integrity API) | 删除 | host/clients/apps 零调用(注意:`fork_from_event` 等生产路径不在此文件,C1 核查确认) |
| D11 | `net` jsonl.rs(285)+ partial_json.rs(535) | 删除 | 全仓零调用点 |
| D12 | `engine` `run_turn`(`engine/engine/src/lib.rs:84`) | 删除(测试改走 tool_loop 入口) | 生产零调用(app 走 `run_session`) |
| D13 | K-08 `ArtifactStreaming` 宣告 | 双端停止宣告 + client `experimental` 门控删除;artifact 流式转候选,R3 registry 就位后接线 | `host/gui-server/src/session.rs:462` 固定 unsupported 而握手宣告能力;desktop/probe 均未启用门控 |
| D14 | 死声明与死 feature | 删除:`foundation/api` features `provider/tool/plugin`(src 零门控;`plugin` 空数组保留为 F41 语义则显式注释)、`execution/tools` encoding_rs、`host/gui-server` futures | rg 验证零引用 |
| D15 | `control-plane/core` OTel 三 exporter(~200 行)、identity_schema(484 行)、rbac 的 PermissionProfile/PrincipalRole(~150 行) | OTel/identity_schema 归档;rbac 保留 `Permission`(orchestration `spawn.rs:11` 在用)其余归档 | 外部零调用 |
| D16 | `vcs/git` 六休眠服务(约 2.7k 行) | Branch/Stash/Conflict/History/CachedStatus+StatusCache+spawn_invalidator 归档;**保留** Diff/Status/GitService/GitRunner/Head/HunkId + HunkStageService(R8 K-04 消费)+ worktree/merge(orchestration `git` feature) | app 只用前六者;GUI git 面板转候选 |
| D17 | `providers/core` negotiate.rs CapabilityNegotiator(465 行)+ registry `caps()`/`ProviderProbe` | **保留**,R5 波 C 接线(K-10 能力收口的载体);包外可见性降 `pub(crate)` 待 R5 定 | P15-8 设计件,有明确消费计划 |
| D18 | `storage/blob` protected.rs PWB1(1,456 行) | **保留**(冻结契约 + golden);R5 波 C 接 ReasoningProtector 成为首个生产消费者 | [ROADMAP](../ROADMAP.md) §4「PWB1 protected 消费者」行 |
| D19 | `foundation/config` `Loader::with_session/with_run` | **保留**(六层合并冻结契约,S9 承诺) | [v2-summary](../docs/v2-summary.md) §4 config 行 |
| D20 | `clients/sdk` ide.rs 空占位(8 行) | 删除(sdk 本体 R1 并入 client) | 占位无内容 |
| D21 | `exec` 包外零消费的 pub 平台函数(`execution/exec/src/lib.rs:34-44`、`LinuxLandlockPolicy`) | 降 `pub(crate)`(集成测试需要的保留并注释) | 包外零调用 |
| D22 | `orchestration/recovery` deprecated `recover`(S13-F30) | 删除 deprecated 别名,保留 `recover_report` | 全仓唯一 deprecated |

「归档」= 从 workspace members 移除并删除源目录(tag `v2-final` 可找回),同时在 ROADMAP §3.3 登记复活条件;不复制到仓库其它位置。domain 中的 canonical 事件类型(Plan/Goal/Task/Team 等)**一律保留**,保证历史事件可重放。

## 4. 波次拆分

| 波 | 内容 | 写入集 | 并行度 |
| --- | --- | --- | --- |
| 0 | 打 tag `v2-final`;起草 ADR-038(按 §3 清单逐项决议 + 证据);**用户确认后**才进波 A | docs/adr/ADR-038-*.md、ROADMAP §4 | 串行(主代理) |
| A | 大块归档:D2/D3/D4/D5/D6/D7(provider-control、workflow 三域、teams、memory、review、transport remote);workspace members 同步收缩;受影响包的 use/测试修复 | control-plane/provider-control、workflow/*、agents/orchestration、host/transport、workspace 根 Cargo.toml | 并行 ×3(控制面 / workflow+orchestration / transport) |
| B | 小块删除与降级:D8–D12、D14、D15、D20、D21、D22;K-08 停止宣告(D13,触 gui-server/client/desktop 能力表) | foundation/diagnostics、net/net、engine、storage/session、host/app(rate_limit)、host/gui-server、clients/{gui-client,sdk}、execution/{tools,exec}、control-plane/core、foundation/api | 并行 ×3(foundation+net+engine / host+clients / control-plane+exec) |
| C | 收口:D16 git 服务裁剪;`cargo check/test -p` 全部受影响包;`cargo tree -p pawork` 闭包对比(前后快照);ROADMAP §3.3 复活条件登记;§4 孤儿表登记 | vcs/git、ROADMAP、本任务书 | 串行(主代理) |

## 5. 验证

- 每个受影响包 `cargo check -p <crate>` + `cargo test -p <crate>`(受影响 = 被删包的全部反向依赖,C1 核查产出清单)。
- `cargo tree -p pawork` 前后对比:闭包只减不增;rustls/rcgen/tokio-rustls 退出 lock。
- 安全红线定向回归不推迟:policy/tools/exec 测试全绿(删除不得触碰红线语义)。
- 重放 golden 全绿:`pawork-session` envelope/迁移测试(domain 事件保留的证明)。
- 真实冒烟(§1.1 矩阵一组即可):`pawork chat` 流式 + 工具调用 + `pawork sessions list`,证明主干未回退。

## 6. 退出标准

- [ ] ADR-038 Accepted(用户确认),22 项决议逐项落地或显式改判
- [ ] tag `v2-final` 已打;归档项在 ROADMAP §3.3 登记复活条件
- [ ] workspace members 收缩完成;全部受影响包定向测试绿
- [ ] `cargo tree -p pawork` 快照对比归档;lock 中 rustls/rcgen/tokio-rustls 消失
- [ ] 冒烟通过;v3_plan §3 指针更新
