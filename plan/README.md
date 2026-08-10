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
- 交付成熟度：`Designed` · `Implemented` · `Wired` · `TargetVerified` · `MaintenanceGated`。状态符号表示排期，成熟度表示证据；新任务和被重新打开的历史任务必须在元信息头记录两者。

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

测试要求按任务风险选择：普通功能至少有存在性/差异检查与受影响 crate 的定向 smoke；安全红线、持久化/重放、路径和进程清理必须随实现补定向回归。不要在每个任务的「验收标准」机械复制 workspace 全量 build/test/clippy。

`🟢` 与交付成熟度不互相替代：新任务至少完成实现、生产主干接线与定向验证（`TargetVerified`）后才能标绿；功能簇通过 L2/L3 后再记 `MaintenanceGated`。现有历史 `🟢` 可能只表示模块实现，若源码/运行证据显示未接线，保留历史状态并由 remediation 补线，不能仅凭勾选项推断 Production Ready。

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
| L1 定向 smoke | 单个任务收尾 | 受影响 crate 的单元/Mock/最小 contract；必要时 `cargo check -p <crate>` | 是 |
| L2 功能簇门禁 | 一组高变更功能基本收尾 | 相关 crates 集成/contract/golden/schema；一次性 clippy/fmt | 不阻塞组内未收尾任务 |
| L3 维护 / 发布门禁 | 发布候选、依赖/协议升级、主干合并前 | workspace 全量、三平台、安全、性能、fuzz/chaos/差分 | 是 |

例外：Secret 不落库、Policy/路径越界、事件持久化与重放、破坏性文件/进程清理、协议向后兼容属于高风险不变量，修改时立即执行对应定向回归，不等待 L2/L3。

功能簇门禁集中在专门的收尾任务（例如 P15-9），Phase 1～7 的七个 remediation 全部结束后再执行一次 Core 主干 L2；不再要求每个 remediation 单独跑 `cargo test --workspace`。具体测试类型与运行频率见 [测试体系](../docs/quality/testing.md)。

本地 L2/L3 使用隔离目标目录，确保无论通过或失败都能清理：

```powershell
$env:CARGO_TARGET_DIR = "target/gates"
try {
    cargo fmt --all -- --check
    cargo build --workspace --all-targets
    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
    cargo run -p schema-typegen -- --check
} finally {
    cargo clean --target-dir "target/gates"
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
}
```

- L0/L1 继续复用默认 `target/`，避免每次重编；任务结束只清理测试创建的临时目录、fixture 副本、日志、coverage/快照临时输出，测试本身优先用 RAII/tempfile 做失败路径清理。
- L2/L3 的 `target/gates` 必须在 `finally` 清理；CI runner 若为一次性环境可跳过本地清理步骤。
- 默认 `target/` 在功能簇收尾时检查体积；达到团队配置阈值或磁盘压力告警时执行一次 `cargo clean`，不要把全量清理放到每个定向测试后，否则会放大后续编译时间。
- Fuzz corpus、Golden 基线与可复核失败样本是版本化证据，不属于缓存；只清理生成缓存和临时输出，不删除人工确认的回归样本。

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
