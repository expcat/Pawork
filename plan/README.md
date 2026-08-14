# Pawork 任务计划（plan/）

本目录存放 ROADMAP 中每个任务的**具体细节**。`ROADMAP.md` 是目录索引（进度表 + 任务简介 + 链接），本目录是每个任务的展开实现说明。

## 如何使用

1. 打开 [ROADMAP.md](../ROADMAP.md)，查看顶部的「进度总览」与「下一个推荐任务」。
2. Phase 内按任务 ID 与依赖执行；跨 Phase 按 [ROADMAP「实施波次与门禁节奏」](../ROADMAP.md#实施波次与门禁节奏) 推进，不机械追求 Phase 数字顺序。
3. 点开对应 `plan/<id>-<slug>.md` 获取该任务的最终目的、细分步骤、产出与验收标准。
4. 完成任务后：勾选该文件内的功能验收与定向验证项，并把 `ROADMAP.md` 中对应行的状态改为 `🟢`、更新进度总览与「下一个推荐任务」。全量 workspace 门禁由功能簇收尾任务负责，不在每个细节任务重复执行。
5. 任务粒度目标：数小时内可独立完成、独立验收、写入集收敛到单一 crate 或一组紧相关文件。

## 命名约定

- 文件名：`<任务ID>-<英文短名>.md`，例如 `P0-1-workspace-skeleton.md`。
- 任务 ID 沿用 ROADMAP 的 `P{n}-{seq}`：`P{n}` 是 Phase 序号，`{seq}` 是 Phase 内顺序；与优先级记号 P0–P2（见术语表）无关。
- 状态符号：`🟡未开始` · `🔵进行中` · `🟢已完成` · `⚪已归档/推迟`。
- 交付成熟度：`Designed` · `Implemented` · `LibraryBuilt` · `AdapterBuilt` · `HostSeam` · `PartialWired` · `HostWired` · `TargetVerified` · `MaintenanceGated`。状态符号表示排期，成熟度表示证据；前五个接线态用于区分「库/适配器存在」与「正式宿主已消费」，新任务和被重新打开的历史任务必须在元信息头记录两者。

## 依赖选型

引入任何第三方包前，先对照 [ROADMAP「依赖选型基线」](../ROADMAP.md#依赖选型基线)：列为「直接采用」的按表使用；列为「参考 + 自实现」的只实现所需最小子集；新增依赖必须同步回基线一节。任务文件带有「依赖建议」小节时，以任务文件为准。

## 单个任务文件结构

每个 `plan/<id>-<slug>.md` 包含以下小节（务必齐全）：

- 元信息头：`> Phase {n} · {Phase 名} · 状态 · 交付成熟度 · 依赖`。
- **最终目的**：一段话，说明这一步完成后解锁什么能力、为什么它处于关键路径上。
- **涉及范围**：涉及的 crate / 目录。
- **细分步骤**：编号列表，每一步写清「做什么」与「目的」，最后由「最终目的」串起整步交付意义。
- **主要产出物**：代码 / 文档 / 配置 / 测试的具体清单。
- **验收标准**：可勾选、可复核的条件。
- **相关文档**：仓库内相对路径链接。

测试要求按任务实际 diff 与风险选择：普通功能至少有存在性/差异检查、changed crates、必要关键 reverse dependents 与定向 smoke；安全红线、持久化/重放、路径和进程清理必须随实现补定向回归。不要在每个任务的「验收标准」机械复制 workspace 全量 build/test/clippy，也不要固定复制 `check + build + test + clippy`。

`🟢` 与交付成熟度不互相替代：新任务至少完成实现、生产主干接线与定向验证（`TargetVerified`）后才能标绿；`TargetVerified` 不要求 Workspace Full Gate。功能簇通过 L2/L3 后再记 `MaintenanceGated`。现有历史 `🟢` 可能只表示模块实现，若源码/运行证据显示未接线，保留历史状态并由 remediation 补线，不能仅凭勾选项推断 Production Ready。

> 「细分步骤」中每一步都要写清**任务**与**目的**，避免只罗列动作而不交代它服务哪个目标。

---

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
          → GPUI Desktop Gate / State Sync → Timeline / Composer / Diff / Terminal
          → Settings / Workflow UI → Signed Desktop Release

在核心 Coding Agent 能可靠完成真实仓库任务、Provider v2 canonical 语义与 Phase 18 账号控制面基础稳定前，不进入 Multi-Agent 与外部 Agent Client 大规模接入。Phase 编号是文档索引，不是机械串行顺序；实际波次见 ROADMAP。

> CLI Host 与多 GUI 协议（Phase 13）是 Core 的正式运行入口与 GUI 接入边界；协议冻结部分（GUI Connection Protocol / Transport 抽象类型）随 [P0-8](P0-8-core-api.md) 提前完成，Remote Transport 真实内网穿透库可推迟。

**Phase 15 显式延后的接线项登记**（[P15-10](P15-10-review-remediation.md) 收口，验收项已落入 Phase 18 计划，不只在 P15-10 链接）：① 生产 `ProtectedKeyResolver` + 持久 `ProtectedBlobStoreProtector` 注入宿主（ADR-032 兑现），并按真实 Session/run `BlobScope` 构造或选择、禁止跨 Session 复用 scoped protector → [P18-3](P18-3-provider-account.md) / [P18-4](P18-4-credential-lease.md) / [P18-14](P18-14-pool-reconciliation.md) 验收标准；② 宿主经 Provider factory / `register_provider` 装配真实 Provider 并消费 `builtin_models()` → P18-3 验收标准；③ provider v2 能力 catalog 统一进入共享 model-registry `caps()` / 协商证据 → P18-14 验收标准。

**Phase 16 延期落点登记**（[P16-10](P16-10-review-remediation.md) 收口，验收项已落入 Phase 17/19 既有计划）：Phase 16 的 10/10 是**有界**计数——library/core 领域 reducer、纯算法、Process 后台执行与 canonical event 包装已交付；[P16-10](P16-10-review-remediation.md) 在 library 层进一步修复正式链编译闭包、Goal/Memory/Review 重放字段与 compat 单事务原子导入（详见 [p16-review](../docs/review/p16-review.md)）。**生产宿主接线闭环延期**，以下六项不声称生产已装配，避免虚假完成：

- **① monitor 包驱动** → [P17-2](P17-2-plugin-package-format.md)（`monitors` 子段定义稳定 Monitor driver/evaluator 入口契约）+ [P17-3](P17-3-plugin-marketplace.md)（package-owned Monitor 的 install/uninstall/停止）。P16-10 已删 `monitor-service` 内置 driver、执行状态统一引用 `task-manager`；但 package 可声明的 Monitor 驱动入口与归属生命周期仍缺，由本组任务落地。
- **② Plan/Goal host** → [P19-12](P19-12-workflow-control.md)（host 经 core-api/EventHub 暴露 Plan/Goal 并被控制面消费；Plan 审批 gate、Goal 人审可证由 core 侧满足）。P16-10 已让 Goal criterion 满足位事件化；但 Plan/Goal 未经 host/core-api/EventHub 暴露、Plan 审批不进 Agent Loop gate、steer 不入 context，仍缺。
- **③ workflow core-api/EventHub** → [P17-6](P17-6-agent-teams.md)（team task board/mailbox/presence 经 `app-service` 唯一 Event Hub 派发可重放，automation 执行权威统一归 `task-manager`，不另建 broadcast）。P16-10 已删 automation 内置 dispatcher 与 `external.rs`、收敛为只调度；但无生产 executor/timer loop、事件不经统一 EventHub 发布到 CLI/GUI，仍缺。
- **④ Memory provider/SQLite/context** → [P17-5](P17-5-agent-profile-v2.md)（`profile.memory` 接真实 `EmbeddingProvider` + SQLite + `context-engine` 消费，或保留 contract 下 default-off）+ [P19-2](P19-2-client-state-projection.md)（memory context projection slice）。P16-10 已让 embedding/confidence 进事件可重放；但无生产 EmbeddingProvider、SQLite 持久化与 context 消费者，仍缺。
- **⑤ Review Forge/UI** → [P19-8](P19-8-diff-git-review.md)（finding/anchor/suggested-patch UI + 真实 Forge host 接线、SuggestedPatch 待 checkpoint/policy 后脱离 dry-run）。P16-10 已让 finding 富字段与 fingerprint 进事件可重放；但无 core-api/UI 消费、Generic Forge 为假副作用且丢弃远端 comment ID，仍缺。
- **⑥ compat 命令入口** → [P17-8](P17-8-agent-sdk.md)（headless/SDK 经 core-api/CLI 暴露 `import_compat` 入口与历史查询）+ [P19-2](P19-2-client-state-projection.md)（compat 会话 projection slice）。P16-10 已修 compat 单事务原子导入、session-scoped ID、参数保真与 import identity；但无 core-api/CLI 命令入口（CLI 仍走 placeholder），仍缺。

> 与 Phase 15 登记同样：以上验收项已写入对应 plan 文件的「验收标准」，不在 P16-10 单独重复实现。P16-10 是 Phase 16 的 review-remediation（library 层修复正式链编译 / 重放字段 / 兼容导入 + 生产宿主接线显式延后），见 [plan/P16-10-review-remediation](P16-10-review-remediation.md)；library 层修复面不计入下列生产延期项。

**Phase 17 延期落点登记**（[P17-14](P17-14-review-remediation.md) 已收口 · 状态：**🟢已完成 · TargetVerified**）：按 [p17-review](../docs/review/p17-review.md) §3/§4/§5/§7，代码层修复已落地八项——① **remote 长驻生命周期 + token 清理**：loopback publish 进程长驻 + 跨进程 connect/reconnect，SIGINT 触发 token 清理、同名再发布不冲突；独立 unpublish/revoke 无共享控制面时 fail-closed（跨进程 e2e 固化），外部可达/共享控制面延 P19-14；② **fail-closed**：placeholder 命令 / PluginList / McpList / 隔离 no-op 工具一律失败语义，CLI 非零退出码 + `--json` `ok=false`；③ **Profile refs fail-closed**：skills / mcp / permissions / hooks 任一非空即 run 解析拒绝；④ **Teams 不持久装配**：正式宿主启动不再无条件打开 teams.sqlite（team_db_path 默认 None）；⑤ **contract 归 transport-api**：remote 契约收回 transport-api，placeholder 收缩为 re-export + Mock；⑥ **Browser 别名删除**：`reject_hosted_for_local` 等纯别名 helper 移除；⑦ **Compat export**：`apply` 更名收缩为 `export_plan`（只写计划、不应用资源）；⑧ **JSON 日志契约**：日志一律 stderr，`--json` / ACP / Headless stdout 只承载协议帧；另**门禁衍生可靠性修复两项**——⑨ **RateLimiter 自动冲刷不丢**：新增 `enqueue`，窗口到期 / 容量触发的自动冲刷结果重排入就绪队列、由下一次 `flush` 发出（不丢不重、队列与合并缓冲同界，过载丢最旧计入 dropped_events）；⑩ **remote 认证同步 subscribe + carrier 顺序无关**：remote-control-adapter 首次认证成功后同步 `subscribe()` 再 spawn 通知泵（消除认证与订阅间的 spawn 调度窗口，认证后 Core 发布的事件不再错过），transport-remote carrier e2e 以响应/通知联合捕获允许任意到达顺序（压力复跑 0 失败）。**不声称生产闭环**，以下五项纵向能力显式延期：

- **① ACP 降级能力审计** → [P18-13](P18-13-audit-otel.md)（handshake 降级结果写入 canonical audit event / structured trace，不新增协商服务）。
- **② host-wired 成熟度再认定与功能簇门禁** → [P18-15](P18-15-control-plane-gate.md)（模型 Run 前置条件经 P18-3 Provider 注册闭合后，P17-1/7/8 Product usable 再认定与跨 crate 不变量集中验证；P18-15 依赖含 P17-7）。
- **③ Marketplace / Plugin 真实纵向 + Profile 引用维度消费** → [P19-11](P19-11-resources-extensions.md)（一个真实 source + 一种资源最小闭环、Plugin/MCP 真实列表接线、profile.skills/mcp/permissions/hooks 经 ResourceLoader 映射到既有入口；Compat export_plan 的真实应用一并落此）。
- **④ Teams canonical ingress** → [P19-13](P19-13-multi-agent-teams.md)（最薄 TeamCommand / TeamQuery + worker presence 桥，有真实 ingress 后再收敛 18 变体 public event 镜像）。
- **⑤ Remote 外部可达 + Remote Control pairing** → [P19-14](P19-14-multi-window-remote.md)（外部可达地址 / relay 作为 transport 配置，loopback 保留测试/开发模式；pairing 复用 client-auth / auth-service 与长驻 transport owner，adapter 只做 capability gate）。

> 与 Phase 15 / 16 登记同样：以上延后项的验收责任写入对应落点 plan；P17-14 是 Phase 17 的 review-remediation（成功语义 / 生命周期 / 状态失真 / 冗余收缩 + 纵向闭环显式延后），见 [plan/P17-14-review-remediation](P17-14-review-remediation.md)。代码门禁数字已回填，任务 🟢已完成 · TargetVerified。

**Phase 18 评审后续落点登记**（[P18-16](P18-16-review-remediation.md) 收口第一条 account route → lease 竖切）：P18-1～P18-15 的历史代码交付计数保留，但成熟度按 HostWired / PartialWired / LibraryBuilt / AdapterBuilt / HostSeam / MaintenanceGated 重新校准；**生产控制环仍有三项明确未完成任务**，不再用「Phase 18 已全部装配」概括：

- **① 持久 Provider composition → [P18-17](P18-17-production-provider-composition.md)**：account/credential 管理事务写回；`BackendCredentialResolver` 被 `ProviderFactory` 真实消费；正式 Provider 经 `register_provider` 注册；`builtin_models()` 合并共享 model registry。
- **② Route / Health / Binding control loop → [P18-18](P18-18-runtime-control-loop.md)**：真实 model capability、Health feedback、route winner credential 单次透传、Session Binding/LeaseRebound、Reconciler/Probe/Quota scheduler 生命周期。
- **③ External Client / Observability host → [P18-19](P18-19-client-observability-host.md)**：Codex/Claude `pawork` 入口、完整 durable audit coverage、WebScrape audit sink 与 OTel collector/exporter 生命周期。

> P18-15 仍表示专用功能簇 L2（`scripts/p18-gate.sh`），不等于 Workspace Full Gate 或 Product Ready；P18-16 只在定向 tests/clippy/rustfmt/gate 与独立复核通过后记 TargetVerified。P19-10 必须依赖 P18-17/P18-18，Desktop 不得用本地状态伪造缺失的 Core 能力。

---

## MVP 范围

首个可发布 Core 的能力边界。详细依据见 [性能目标](../docs/quality/performance-targets.md) 与各 Phase 退出标准。

**必须具备**：纯 Rust 无 Node/Bun；OpenAI-compatible / OpenAI（GPT）/ Anthropic（Claude）/ xAI Grok / 智谱 GLM / 阿里 Qwen / Moonshot Kimi；API Key + OAuth 订阅；Agent Loop + Streaming；Text/Image/Tool Call；read/write/edit/apply_patch；shell/search/find/list；Policy 与 Approval；Workspace Trust；SQLite Session；Fork/Resume/Branch；Compaction；Git status/diff/stage/unstage；Worktree；Checkpoint 与 Rollback；Skills 与 `AGENTS.md`；Pi JSONL 导入；CLI Host（`pawork`，CLI=Core 宿主，可脱离 GUI 运行）；GUI Connection Protocol 与多 GUI 连接（含本地 Transport、Snapshot/重放、Remote 占位接口）；macOS/Windows/Linux 基础支持。

**可推迟**：Bedrock、Mistral、Vertex、Google Gemini（已实现但降级次要）；MCP OAuth；WASM Provider；高级 Sandbox；Multi-Agent；Hunk/Line Stage；真实 Remote Transport（内网穿透库）；远程开发；云同步；长期语义记忆。

## 长期覆盖目标

在首个可发布 Core 之后，Pawork 的目标是用同一 canonical domain 承载 GPT / Claude / Grok 的现代原生能力，并覆盖主流 Coding Agent 工作流的合理并集：

- **Provider Native**：Client Function / Provider Hosted / Provider Extension；Responses / Modern Messages；Web/X/File/Collection Search、Code Execution、Hosted Shell、Computer Use、Image Generation、server-side MCP、Tool Search、citation/source、reasoning continuation 与 capability negotiation。
- **Agent Workflow**：Plan review、Goal、Background Task、Scheduled Automation、Persistent Monitor、Review Engine、跨产品 Session Import；Long-term Memory 为 P2，不阻塞前述闭环。
- **Ecosystem / Host**：用户 Shell/HTTP/Prompt Hooks、Plugin Package/Marketplace、LSP、完整 Agent Profile、Agent Teams、ACP、Rust/JSON SDK、IDE adapter、Browser/Computer、真实 Remote Transport 与受限移动端控制。
- **Account / Client Control Plane**：Tenant/Principal、ProviderAccount/Credential Lease、scope-aware ErrorClassifier、确定性 RoutingPolicy、Session Affinity、多维 Usage/Cost Ledger、统一 ClientAdapter，以及 Codex App-Server / Claude Gateway / ACP adapter。
- **Desktop Client**：独立 GPUI Rust GUI、可重建 Snapshot/Event 投影、Timeline/Composer/Approval/Diff/Terminal、资源与 Workflow 控制面、多窗口/远程连接，以及三平台签名分发。

这些目标分别落在 Phase 15～19；不得用 `provider_options`、HTTP status 直译或 Provider/Client 名称分支绕过 canonical domain，也不得破坏 CLI/Core 同进程同二进制、Core 单一事实源与 GUI 只经 GUI Connection Protocol 接入的架构红线。Phase 19 的本地状态只允许保存 UI preference 与可丢弃投影，不能成为第二事实源。

## MVP 验收清单

1. Core 启动不加载任何 JavaScript Runtime
2. 可从空工作区创建 Session
3. 可流式调用至少三个主要 Provider（初始集合：OpenAI / Anthropic / xAI / 智谱 / 阿里 / Moonshot）
4. 可接收多个 Tool Call
5. 可取消 Provider 和 Tool
6. 可安全读取和修改文件
7. 可运行测试命令并流式返回输出
8. 所有文件修改可查看 Diff
9. 所有 Agent 修改可回滚
10. 可从任意消息 Fork
11. 可自动和手动 Compaction
12. Core 崩溃后 Session 可恢复
13. Secret 不写入数据库和日志
14. 未信任 Workspace 默认限制写入和命令
15. 可导入 Pi JSONL Session
16. 大型 Tool Output 使用 Artifact 引用
17. 100,000 行 Diff 不需要一次复制到 GUI
18. GUI Connection Protocol 有版本和 Contract Tests
19. CLI 与 Core 编译为同一二进制，CLI 可脱离 GUI 运行
20. 一个 CLI/Core 实例可同时连接多个本地/远程 GUI，GUI 断线不取消任务
21. 三个平台的路径和子进程测试通过
22. 性能指标达到 Core 目标

## 测试节奏与缓存清理

目标是让开发期优先形成真实功能闭环，把昂贵、容易受频繁接口变化影响的门禁后移到功能簇收尾和维护升级阶段；后移的是重复的全量执行，不是放弃关键不变量。

| 层级 | 触发时机 | 默认内容 | 是否阻塞细节任务 |
| --- | --- | --- | --- |
| L0 存在性 / 差异 | 每次编辑 | 文件、链接、生成物与 diff 检查 | 是 |
| L1 定向 smoke | 单个任务收尾 | changed crates + 必要关键 reverse dependents + 定向 regression | 是 |
| L2 功能簇门禁 | 一组高变更功能基本收尾 | 相关 crates 集成/contract/golden/schema；定向 clippy/fmt；必要时明确升级 Full Gate | 不阻塞组内未收尾任务 |
| L3 维护 / 发布门禁 | 发布候选、Maintenance/Release Gate、重大依赖/协议升级 | workspace 全量、三平台、安全、性能、fuzz/chaos/差分 | 是 |

例外：Secret 不落库、Policy/路径越界、事件持久化与重放、破坏性文件/进程清理、协议向后兼容属于高风险不变量，修改时立即执行对应定向回归，不等待 L2/L3；高风险不等于自动执行 workspace 全量。

任务作者与执行 Agent 按以下方式收敛 L1：

1. 从本任务 committed/staged/unstaged/new-file diff 映射 changed crates；脏工作区排除用户原有无关改动。用 `cargo metadata --format-version 1 --no-deps` 校准 path → package。
2. crate 私有实现通常只验证该 crate。改变 `pub` API、feature、shared/canonical domain、GUI Connection Protocol、序列化/持久化格式或 schema 时，用 `cargo tree --workspace --invert <crate> --depth 1` 或 metadata dependency graph 选择实际关键消费者。
3. canonical domain/protocol 加主要 producer、consumer、serializer/typegen 与 contract；Provider、GUI、平台代码只加实际受影响 adapter/runtime/projection/controller/target/harness，不按类别扩成 workspace。
4. 最终集合使用多个 `-p`。相关 crate 多、修改公共 API 或需要定向高风险 regression，都不是切换 `--workspace` 的充分理由。

普通任务从以下模板中选择必要项，不按固定顺序全跑：

```bash
cargo check -p <crate-a> -p <crate-b>
cargo test -p <crate-a> -p <crate-b>
cargo clippy -p <crate-a> -p <crate-b> --all-targets -- -D warnings
```

`cargo test` 已覆盖所需编译时，没有 binary/link/build script、特定 target/profile 或产物行为需要验证就不再追加 `cargo build`。文档或不影响构建行为的配置任务可以不运行 Cargo 编译。

功能簇门禁集中在专门的收尾任务（例如 P15-9），Phase 1～7 的七个 remediation 全部结束后再执行一次 Core 主干 L2；不再要求每个 remediation 单独跑 `cargo test --workspace`。具体 affected-crate 算法、测试类型与运行频率见 [测试体系](../docs/quality/testing.md)。

Workspace Full Gate 仅在以下条件之一明确成立时执行：功能簇整体收尾/专门 Gate；大规模跨 crate 重构；workspace/resolver/toolchain/关键依赖重大变化；canonical protocol/domain 大范围变化且关键消费者集合不足；Maintenance/Release Gate；用户明确要求。“保险”“最终确认”“确保没有回归”“改动较多”或任务到达收尾阶段不是升级理由。

定向 L2 继续对相关 crates 使用多个 `-p`，可放到隔离 `target/gates`。只有命中上述条件时，本地 L2/L3 才使用以下 **Workspace Full Gate**；三个 workspace 命令保持为维护/发布入口：

```powershell
$env:CARGO_TARGET_DIR = "target/gates"
$env:CARGO_INCREMENTAL = "0"
try {
    cargo build --workspace --all-targets
    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
} finally {
    cargo clean --target-dir "target/gates"
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    Remove-Item Env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue
}
```

- L0/L1 继续复用默认 `target/`，避免每次重编；任务结束只清理测试创建的临时目录、fixture 副本、日志、coverage/快照临时输出，测试本身优先用 RAII/tempfile 做失败路径清理。
- L2/L3 的 `target/gates` 必须在 `finally` 清理；CI runner 若为一次性环境可跳过本地清理步骤。
- 本地 Gate 仅在 Rust 格式或 schema/typegen 可能受影响时加入 `cargo fmt --all -- --check` 或 `cargo run -p schema-typegen -- --check`；手动三平台 L3 CI 作为固定 Maintenance/Release Gate 始终包含两项。
- 默认 `target/` 只在达到团队配置阈值、磁盘压力告警或用户明确要求时执行 `cargo clean`；普通任务结束与功能簇收尾本身都不是清理触发器。
- Fuzz corpus、Golden 基线与可复核失败样本是版本化证据，不属于缓存；只清理生成缓存和临时输出，不删除人工确认的回归样本。

每个任务的验收记录使用以下字段；未运行 Full Gate 是普通任务的正常完成状态：

```text
Validation Level: L1
Affected crates: <changed + selected reverse dependents，或 none>
Validated: <实际命令 / tests / checks>
Targeted regressions: <实际覆盖，或 none>
Full workspace gate: NOT RUN (<未命中升级条件>)
```

发布前仍须对照 [性能目标](../docs/quality/performance-targets.md) 与 [安全验收](../docs/quality/security-acceptance.md)；这些是 L3 维护门槛，不是开发期每个 Phase 的重复前置。

## 风险监控

| 风险 | 影响 | 对策 | 相关 |
| --- | --- | --- | --- |
| Provider 适配工作量 | 高 | Provider 只依赖 canonical domain；保存 raw metadata；Contract Tests；禁止在 Agent Engine 判断 Provider 名 | [ADR-002](../docs/adr/ADR-002-agent-engine-provider-decoupled.md)、[ADR-015](../docs/adr/ADR-015-provider-contract-tests.md) |
| Compaction 品质 | 高 | 保留结构化状态/目标/未完成任务/修改文件；Golden Sessions；可检查摘要；可恢复压缩前 branch | [context](../docs/features/context.md) |
| Shell 跨平台 | 中 | Process Runtime 独立 crate；平台特定实现；三平台 CI | [process](../docs/features/process.md) |
| 插件生态重建 | 中 | Skills 优先；MCP 优先于 WASM；WASM API 小而稳定；不过早支持 native plugin | [ADR-011](../docs/adr/ADR-011-mcp-first-extension.md)、[ADR-012](../docs/adr/ADR-012-wasm-first-plugin.md) |
| GPUI pre-1.0 与平台/发布成熟度 | 高 | 精确 pin 已过三平台 Gate 的版本/revision；P19-1 先验证 Windows standalone、IME、Terminal、a11y、打包与签名更新，失败时保持协议不变并回退已记录路线；P19-16 用真实 Windows/macOS/Linux 壳验收 | [ADR-035](../docs/adr/ADR-035-gpui-desktop.md)、[Desktop GUI](../docs/features/desktop-gui.md)、[P19-1](P19-1-desktop-shell.md)、[P19-16](P19-16-desktop-gate.md) |
| 范围过大 | 高 | 严格按关键路径推进；Coding Agent 可靠完成真实任务前不进入 Multi-Agent/复杂插件 | [ROADMAP 关键路径](../ROADMAP.md) |
