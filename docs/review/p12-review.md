# Phase 12 Review：Multi-Agent 编排（orchestration）

- **审查范围**：P12-1～P12-6（Supervisor/Worker、TaskGraph、worktree 隔离、双层预算、patch merge、cancel tree）及 orchestration 实际消费的最小账号控制面契约（`provider-control` / `tenant-service` / `usage-ledger` / `agent-domain` 身份）；ADR-016（事件持久重放）、ADR-033（控制面分离）；`docs/features/multi-agent.md`、`docs/architecture/workspace-layout.md`。
- **事实源**：当前源码（orchestration 全部 8 个源文件逐行 + `rg` 全仓跨引用核对）、`plan/P12-*.md`、`ROADMAP.md` Phase 12 段、相关 ADR。
- **方式**：Commander 统筹复核 + 两个只读 `deepseek_explorer` 并行调查（账号控制面契约一致性 / 主流程集成状态与「复用 Agent Engine」落实情况）。**本次只 Review，不修改实现。**
- **审查日期**：2026-08-10。

---

## 0. 总评

Phase 12 的设计目标——parent/worker 生命周期可持久化可重放、双层并发、写入隔离、patch 合并需 parent 审批、取消树联动且不惩罚账号健康——**在库代码层面诚实落地**：lifecycle 状态机纯函数化、事件按 ADR-016 可序列化可重放、cancel tree 复用 `LeaseOutcome::Cancelled` 幂等释放且不累加 `consecutive_failures`、`PatchMerger` 绝不自动合冲突、worktree 经 `git-service` 的 `WorktreeService` 分配并以 RAII 守卫释放。账号控制面契约（`provider-control` / `tenant-service` / `usage-ledger` / `agent-domain` 身份）**最小且一致**：依赖方向单向、`agent-domain` 零业务依赖红线保持、没有重复实现健康/路由状态机。架构红线（纯 Rust、无 GUI 依赖、无循环依赖）全部满足。

但 Phase 12 最大的问题不是「缺功能」或「设计错误」，而是 **四个子系统之间的接线缺失与大量为未来预留但当前完全无人调用的死抽象**：

1. **四个 P12 子系统彼此不接线**：`AgentSupervisor` 只 `use` 了 `budget` / `identity` / `lifecycle` 三个模块，**完全不引用 `TaskGraph` / `WorktreeAllocator` / `PatchMerger`**。spawn 出来的 worker 没有进入 TaskGraph 调度，没有分配 worktree，没有 patch 合并管道——TaskGraph / worktree / merge 是三个互不关联的独立库。P12-2～P12-5 名义上是 P12-1「之上」的任务，但代码里它们之间没有任何调用关系。
2. **全仓零消费者**：没有任何 crate 的 `[dependencies]` 引用 `orchestration`，没有 `use orchestration::` 出现在 crate 之外。orchestration 是仅登记为 workspace member 的独立库，从未接入 agent-engine / app-service / CLI / tool。这与文档「再由 tool 或外部 Client 触发 spawn」「大规模接入延后」一致，并非交付缺陷，但评审必须显式记录，避免误以为「multi-agent 已可跑」。
3. **P12-1 第 3 步「与 Agent Engine 复用 / 不重复实现循环」在代码中完全未体现**：orchestration 不依赖 `agent-engine`，`start_worker` 只是状态迁移 + emit `WorkerRunning`，**不运行任何 agent loop**。计划把这一步写进去了，但验收标准没要求接入，实现也没做——属于「计划声称做了、代码没做」的脱节。
4. **为未来预留的死枚举 / 死状态机**：`TaskCreated/Ready/Assigned/Completed/Failed/Retried/Cancelled` 七个 Task* 事件**没有任何生产者**（唯一构造点在 lifecycle.rs 测试模块）；`PatchProposed/PatchMerged/PatchConflict` 同理；`ConcurrencyDenied` 同理；`AgentConcurrency` / `ConcurrencyGuard` 独立并发计数器只在 budget.rs 自测中使用，Supervisor 实际用 `active_worker_count` 计数——模块文档宣称的「独立 agent 并发计数器」与实现不符。
5. **预算控制器空转**：`WorkerBudgetController` 在 spawn 时被注册，但**整个生命周期从未被喂过用量**（没有 `record_tokens` / `record_cost` / `check()` 的生产调用），因此 `BudgetExceeded` 事件永远不会触发；`complete()` 把累计用量 flush 到 ledger 时用了 `account_id: "unknown"`、`provider_id: "unknown"`、`model_id: "unknown"`——归属维度断裂，写进 ledger 的记录无法按 account/provider/model 归因。

没有发现需要新增抽象的设计缺口；几乎所有建议都是「删除/合并/接线/补归属」，方向与本次「优先减少代码与概念」一致。**核心判断：Phase 12 的正确形态不是「六个独立库」，而是「Supervisor 把 TaskGraph / worktree / budget / merge 串起来的编排器」——当前只交付了零件，没有交付编排。**

---

## 1. 设计符合度（正面结论）

| 设计目标 | 实现事实 | 证据 | 判定 |
| --- | --- | --- | --- |
| lifecycle 事件化、可重放、终态拒绝一切转换 | `transition` 纯函数 + `WorkerStateMachine` 封装；终态 `FromTerminal`；`replay_workers` 容错重放，终态后迟到事件忽略 | lifecycle.rs:97-140, 432-484 | 符合 |
| 所有 worker 经 Supervisor 创建，禁止脱离监督的 `tokio::spawn` | `spawn` 是唯一创建路径，集中持有 `workers`/`cancel_tokens`/`children` 注册表 | supervisor.rs:128-137, 165-310 | 符合（库内） |
| Worker 不直接拿 API key，只经 `AcquireRequest` / lease | `spawn` 经 `pool.acquire_guard`，`WorkerEntry.lease` 持有 `LeaseGuard`；不读 key | supervisor.rs:22-25, 244-262 | 符合 |
| 取消树递归联动全部后代 | `cancel_tree` BFS 遍历 `children` 图，逐节点 Cancel + 释放 lease | supervisor.rs:399-475 | 符合 |
| 取消以 `LeaseOutcome::Cancelled` 幂等释放、不降健康 | `*guard.outcome_mut() = Cancelled`；`provider-control` 只加 `cancelled_count`，不动 `consecutive_failures` | supervisor.rs:455-463；provider-control lib.rs:372-373 | 符合（契约侧正确） |
| 崩溃恢复无悬挂 worker | `recover` 把重放后仍活动态的 worker 一律标记 Failed | supervisor.rs:495-518 | 符合 |
| 双层并发上限用独立计数器/状态机 | agent 并发走 `TenantPolicyEngine::check_agent_concurrency` + 本地 `active_worker_count`；request/lease 并发走 `CredentialPool`；两层完全互不读写 | supervisor.rs:175-194；budget.rs:1-13；provider-control lib.rs | 符合（语义正确，但见 §2.2） |
| TaskGraph 拒绝环 / 拒绝跨租户依赖 | `add_task` 调 `detect_cycle`（DFS）+ 跨租户比对；允许前向引用 | task_graph.rs:107-149, 215-247 | 符合 |
| TaskGraph 幂等 complete + 重试 attempt 计数 | `complete` 对已完成返回 Ok；`retry` 复位到 Created 并递增计数，超 max 拒绝 | task_graph.rs:151-186, 296-320 | 符合 |
| worktree 写入隔离、释放不删用户数据 | `WorktreeAllocator` trait + `GitWorktreeAllocator` 委托 `git-service::WorktreeService`；`Drop` best-effort release；`into_inner` 转移所有权 | worktree.rs:60-103, 158-187 | 符合 |
| patch 合并需 parent 审批、绝不自动合冲突 | `PatchMerger::merge` 遇冲突返回 `ConflictUnresolved`；`Reject` / `NeedsConflictResolution` 不写任何文件 | merge.rs:280-316 | 符合 |
| 冲突检测用 fork 点基准内容 | `base_content` 默认 `git show HEAD:<rel>`，父侧哈希与基准不一致才判冲突 | merge.rs:124-138, 219-252 | 符合 |
| 相对路径拒绝绝对/`..` 穿越 | `resolve_relative` 逐 component 校验 | merge.rs:319-339 | 符合 |
| 账号控制面契约最小且一致 | 四个 crate 的公开 API 与 P12 brief 逐项吻合，无越界实现 | 见 §1.1 | 符合 |

**结论**：单看每个子系统的内部行为，与 P12-1～P12-6 的库级验收标准基本一致；账号控制面契约（§1.1）的最小化与依赖方向红线全部保持。问题集中在「子系统之间」与「子系统与外部」（§2、§3）。

### 1.1 账号控制面契约最小且一致（deepseek 核查）

- `agent-domain`：`string_id!` 宏（ids.rs:54）生成 `TenantId` / `PrincipalId` / `AgentId` / `SessionId` / `RunId`，`CancellationToken`（cancel.rs:14）存在；仅依赖 serde/serde_json/可选 ts-rs，**零业务依赖红线保持**。语义默认值（`local/default`、`local/user`）由 tenant-service（`DEFAULT_TENANT`/`DEFAULT_PRINCIPAL`，lib.rs:23-31）与 provider-control（池默认账号 `local/default`，lib.rs:309）提供，身份类型自身 `Default` 是空串——这是合理的分层，非缺陷。
- `provider-control`：`CredentialPool` trait（lib.rs:175）含 `acquire`/`acquire_guard`/`release`/`active_count`/`account_health`；`LeaseOutcome`（lib.rs:75）四态齐全；release 对未知 lease 幂等返回 `already_released:true`（lib.rs:361），`Cancelled` 只加 `cancelled_count`、`Failed` 才加 `consecutive_failures`（lib.rs:372-373）。契约与设计逐项一致。
- `tenant-service`：`TenantPolicy`（lib.rs:41）含 `max_concurrent_agents`/`max_concurrent_requests` 独立字段；`TenantPolicyEngine`（lib.rs:126）的 `check_agent_concurrency`（lib.rs:210）与 `check_request_concurrency`（lib.rs:227）读各自字段，并有测试 `request_concurrency_independent_from_agent_concurrency`（lib.rs:428-442）验证双层独立。**agent 并发与 request 并发确实用独立计数器/状态机。**
- `usage-ledger`：`UsageRecord`（lib.rs:29）按 tenant/principal/account/session/agent/run/provider/model 多维归属；validate 拒绝空 tenant、总 token 为 0（lib.rs:214-216）与 `occurred_at_ms=0`（lib.rs:219-222）。
- **依赖单向**：这四个 crate 的 Cargo.toml 仅依赖 agent-domain 与标准库，不依赖 orchestration；orchestration 依赖它们。无循环。编排侧没有重复实现健康/路由状态机——Supervisor 只通过 `acquire_guard` + `outcome_mut`（supervisor.rs:244、406、485）消费并复用 `check_agent_concurrency` / `check_model` 闸门。

---

## 2. 冗余 / 死代码 / 过度预留

> 严重度：〔高〕= 影响正确性或架构判断；〔中〕= 明显可删的死代码或漂移；〔低〕= 清理收益有限。

### 2.1〔高〕四个子系统彼此不接线，Supervisor 是个「半成品编排器」

**事实**：`AgentSupervisor`（supervisor.rs:22-24）的 import 只有：

```rust
use crate::budget::{LedgerContext, WorkerBudgetController, WorkerBudgetLimits};
use crate::identity::{AgentInstance, WorkerRole};
use crate::lifecycle::{replay_workers, OrchestrationEvent, WorkerState, WorkerStateMachine, WorkerTransition};
```

**完全不引用 `TaskGraph` / `WorktreeAllocator` / `PatchMerger`**（全文件 `rg` 确认零命中）。后果：

- `spawn` 创建的 worker **不进任何 TaskGraph**——P12-2 的依赖调度与 P12-1 的 worker 注册表是两套互不感知的状态。
- `SpawnRequest.worktree_path` 只是被动接收一个 `PathBuf`，Supervisor **从不调用 `WorktreeAllocator::allocate`**——P12-3 的 worktree 分配器与 P12-1 的 worker 生命周期脱钩，worktree 完全靠外部传入路径。
- Supervisor **没有 patch 合并管道**——P12-5 的 `PatchMerger` 与 P12-1 的 worker 完成路径（`complete()`）没有交集。

P12-2～P12-5 的计划措辞是「在 P12-1 之上叠加」「复用 P12-1」「依赖 P12-3」，但代码里它们之间**没有任何调用关系**。这是 Phase 12 最本质的结构问题：交付了零件，没有交付编排。

**建议方向（减概念、接线优先）**：在 spawn 路径里接 `WorktreeAllocator`（把 `worktree_path` 从「外部传入」改成「Supervisor 按需分配」），把 `TaskGraph` 作为 Supervisor 的可选调度器注入，把 `PatchMerger` 接到 `complete()` 之前的 patch 收集环节。这会让 Supervisor 真正成为编排器，而不是「带 lease 的状态机注册表」。若短期不接线，至少应在 `multi-agent.md` 显式声明「当前为零件级交付，编排接线在 P13/集成阶段」——目前文档（multi-agent.md:24）的「P12 首先交付 `AgentSupervisor` / `TaskGraph` / `AgentTree`」措辞让人以为三者已经协同。

### 2.2〔中〕`AgentConcurrency` / `ConcurrencyGuard` 是未接线的死状态机，且与文档不符

**事实**：budget.rs:238-292 实现了一个完整的 `AgentConcurrency` 原子计数器 + `ConcurrencyGuard` RAII 守卫，模块文档（budget.rs:1-13）把它描述为「Agent 并发闸门（与 lease/请求并发完全独立的计数器）」。但 `rg "AgentConcurrency"` 全仓命中只在 budget.rs 自身（定义 + 单测），**Supervisor 的 agent 并发闸门实际用的是 `active_worker_count`（supervisor.rs:572）+ `config.max_agent_concurrency`（supervisor.rs:194）**，不是 `AgentConcurrency`。

后果：

- 一个完整的状态机（含 CAS acquire 循环、RAII 守卫）是死代码，只被自己的测试引用。
- 模块文档宣称的「独立 agent 并发计数器」与实现不符——实现用的是遍历 `workers` 注册表数活动 worker，不是原子计数器。

**建议**：删除 `AgentConcurrency` / `ConcurrencyGuard`（budget.rs:236-292，约 60 行 + 测试），或反过来——若原子计数器比遍历注册表更合适，就用它替换 `active_worker_count`。两者不能并存。当前是「为双层并发预留的抽象 + 实际没用它」。

### 2.3〔中〕Task* / Patch* / ConcurrencyDenied 事件全是死枚举

**事实**：`OrchestrationEvent` 有 7 个 Task* 变体（`TaskCreated/Ready/Assigned/Completed/Failed/Retried/Cancelled`，lifecycle.rs:284-324）、3 个 Patch* 变体（`PatchProposed/PatchMerged/PatchConflict`，lifecycle.rs:349-368）、1 个 `ConcurrencyDenied`（lifecycle.rs:340）。

- `rg` 全仓：Task* 的唯一构造点是 lifecycle.rs:583，**在测试模块内**（`mod tests` 始于 :436），手工构造用来验证 `replay_workers` 忽略无关事件——非生产路径。
- `task_graph.rs` 的 `mark_ready`/`assign`/`start`/`complete`/`fail`/`cancel`/`retry`（task_graph.rs:178-320）**零 `OrchestrationEvent` 引用、零 emit**，只改内部 `TaskState`。
- merge.rs:7 的模块文档自己写「`PatchProposed`/`PatchMerged`/`PatchConflict` 由调用方（编排宿主）依据本模块结果发出；本模块本身无事件日志」——但没有「编排宿主」，所以这些事件无人发。
- `ConcurrencyDenied` 同理无生产者。

后果：`OrchestrationEvent` 的 26 个变体里，**Task*（7）+ Patch*（3）+ ConcurrencyDenied（1）= 11 个变体是死的**，占 42%。它们在 `replay_workers` 里被显式「忽略」（lifecycle.rs 的 `_ => {}` 分支），但本身永远不会出现在事件流里。这既增加了维护面（每个变体带 doc comment），也制造了「事件已建模」的假象。

**建议**：在接线（§2.1）之前，把 Task* / Patch* / ConcurrencyDenied 变体从 `OrchestrationEvent` 移除，或降级为 `task_graph.rs` / `merge.rs` 内部的、不被 `OrchestrationEvent` 承载的局部返回类型。接线时再加回 `OrchestrationEvent`。当前是「为未来事件源预留的事件类型，但没有任何产生者」。

### 2.4〔中〕预算控制器空转，ledger flush 用 `"unknown"` 归属

**事实**：

- `WorkerBudgetController` 在 spawn 时注册（supervisor.rs:308），但**整个 worker 生命周期内没有任何 `record_tokens` / `record_cost` / `check()` 的生产调用**（`rg` 确认 supervisor.rs 零命中）。因此 `BudgetReport` 永远是空的，`BudgetExceeded` 事件永远不触发，P12-4 的「达预算行为（pause/cancel/reassign/fallback）」实际上没有触发源。
- `complete()` 在 flush 到 ledger 时构造 `LedgerContext`（supervisor.rs:365-374）用的是：
  ```rust
  account_id: "unknown".to_string(),
  provider_id: ProviderId::new("unknown"),
  model_id: ModelId::new("unknown"),
  ```
  即把归属三维度（account / provider / model）全部写成 `"unknown"`。usage-ledger 的 `UsageRecord` 设计上按这八个维度归属（usage-ledger lib.rs:29），写进 `"unknown"` 后记录**无法按 account/provider/model 归因**——P12-4 验收标准「usage 可归属 tenant/session/agent/account」里 account 这一维事实上断了。

根因：Supervisor 不跑 agent loop（§3.1），所以拿不到真实 usage 与真实 account/provider/model——这些信息只能在 agent-engine run loop 里产生。预算控制器被设计成「跑的时候喂数据」，但没有「跑」的环节。

**建议**：要么明确「预算度量在 agent-engine 接入后才生效」（文档声明 + 把 `BudgetExceeded` / ledger flush 标记为待接线），要么把 `LedgerContext` 的 account/provider/model 从 spawn 的 `AcquireRequest` / lease 上取（lease 持有后可拿到 account id），不要硬编码 `"unknown"`。当前是「写了完整的预算 + ledger flush 管道，但没有数据源」。

### 2.5〔低-中〕`checkpoint-service` 与 `agent-events` 是死依赖

**事实**：orchestration 的 Cargo.toml 声明依赖 `checkpoint-service`（Cargo.toml:14）和 `agent-events`（Cargo.toml:13），但 `rg` 全 crate 确认：**没有任何 `use checkpoint_service` / `use agent_events`**。两者从未被任何 `.rs` 文件 import。

- `checkpoint-service`：P12-5 计划（plan/P12-5-result-merge.md:3）写「涉及范围：orchestration、checkpoint-service」，但 merge.rs 实际只用 `diff-service` + `std::fs` + `git-service::GitRunner`，没有 checkpoint。冲突检测走的是「父侧当前内容 vs `git show HEAD:<rel>` 基准」，不涉及 checkpoint 快照。
- `agent-events`：lifecycle.rs 自己定义了 `OrchestrationEvent`（独立的 enum，不复用 `agent_events` 的 canonical 事件类型），P12 没有把编排事件并入 core event store。

后果：两个依赖白白编译、白白进入 `Cargo.lock`，且 `workspace-layout.md` 把它们登记为 orchestration 的依赖方向，制造了「orchestration 会落 checkpoint / 会发 canonical event」的错觉。

**建议**：从 orchestration 的 Cargo.toml 删除 `checkpoint-service` 和 `agent-events`（除非短期要接线）。同步更新 `workspace-layout.md` §2 的 orchestration 行。

### 2.6〔低〕`parent_id()` / `AgentTree` / `WorktreeAllocator` 的文档-代码漂移

- `AgentSupervisor::parent_id(&self) -> AgentId`（supervisor.rs:155）恒返回 `AgentId::new("supervisor")`，全仓无调用点（`rg "\.parent_id\(\)"` 仅命中定义行）。这是一个「看起来像 owner identity，实际是硬编码常量且无人读」的死 API。
- `docs/features/multi-agent.md:24` 写「P12 首先交付 `AgentSupervisor` / `TaskGraph` / `AgentTree`」，但 `rg "AgentTree"` 全仓零命中——**`AgentTree` 这个类型根本不存在**。文档在描述一个未实现的抽象。
- `WorktreeAllocator` trait + `GitWorktreeAllocator` + `WorktreeGuard`（worktree.rs:42-187）是一套完整实现，但 §2.1 指出 Supervisor 不引用它——它只在自己和测试里被用。

**建议**：删除 `parent_id()`（或改成有意义的设计）；把 `multi-agent.md` 的 `AgentTree` 改成实际存在的类型名（`AgentSupervisor` 的 children 注册表，或显式说明 AgentTree 待后续）。

### 2.7〔低〕`WorktreeGuard::drop` 在 Drop 中 `tokio::spawn`

**事实**：worktree.rs:170-185 的 `Drop` 实现里 `tokio::spawn` 一个 async release 任务。模块文档承认「Drop 中无法 await」。这意味着：

- 如果 Drop 发生在没有 tokio runtime 的上下文，`tokio::spawn` 会 panic。
- best-effort release 失败只 `tracing::warn`，没有重试或上报机制——worktree 可能泄漏（git worktree 残留）。

当前没有生产调用方所以未爆雷，但这是 Drop + async 的经典反模式。

**建议**：接线时若 Supervisor 持有 worktree，改成「显式 `async fn release()` + 注册表在 Supervisor Drop 时统一 await 释放」，避免 Drop 内 spawn。本次仅记录，不改。

---

## 3. 主流程接入与设计-实现一致性

### 3.1〔高〕「复用 Agent Engine 循环」未实现，Supervisor 是纯状态机

**事实**：

- P12-1 计划第 3 步（plan/P12-1-supervisor-worker.md:13）明确写「与 Agent Engine 复用——目的：不重复实现循环」。
- 但 orchestration 的 Cargo.toml **不依赖 `agent-engine`**；agent-engine 的 Cargo.toml 也不依赖 orchestration（双向无依赖）。
- `start_worker`（supervisor.rs:314）只是 `BeginRunning` 状态迁移 + emit `WorkerRunning`，**不运行任何 agent loop**。
- agent-engine 的入口是 `ProviderLoop::run`（provider_loop.rs:190）+ 单轮 `run_turn`（provider_loop.rs:291），由 `LoopContext` trait 驱动——但 `ProviderLoop::new/.run` 的全部调用点都在 agent-engine 自己的 `#[cfg(test)]` 模块内（provider_loop.rs:935 起）。**生产代码里连 agent-engine 自己都还没被任何 crate 调用。**

后果：Supervisor spawn 出来的 worker 是一个「注册表里的状态机条目」，不是一个真正在跑的 agent。P12-1 第 3 步的「不重复实现循环」实际上退化为「不实现任何循环」。

**判定**：这是计划与实现的脱节——计划声称复用 Agent Engine，实现既没复用也没接入。考虑到 ROADMAP.md:86 明确「在核心 Coding Agent 能可靠完成真实仓库任务前，不进入 Multi-Agent 大规模接入」，且 agent-engine 自己也尚未被生产消费，**当前的「不接入」与 ROADMAP 节奏一致**，并非交付缺陷。但 P12-1 计划的第 3 步应当要么删除（承认本轮不接 agent-engine），要么改写为「保留 agent-engine 接入点，接线在 P13/集成阶段」——当前措辞具有误导性。

### 3.2〔高〕全仓零消费者，与文档一致但须显式记录

**事实**：`rg "orchestration"` 全仓命中，除 orchestration 自身外只有：

- workspace members 声明（Cargo.toml:57）
- 文档（ROADMAP / multi-agent.md / workspace-layout.md / 各 P12 plan）
- 未来计划（P17-6 的 `teams` crate 会依赖 orchestration，P17-6-agent-teams.md:33，尚未实现）

没有任何 crate 的 `[dependencies]` 引用 orchestration；没有 `use orchestration::` / `AgentSupervisor` / `TaskGraph` 出现在 orchestration 之外。app-service（仅 core-api）、cli-host（app-service/cli-command/cli-renderer/core-api）、`pawork` 二进制（apps/pawork/Cargo.toml）均不涉及。

**判定**：**未接入主流程**。这与 `multi-agent.md:24`「再由 tool 或外部 Client 触发 spawn」、`ROADMAP.md:66`「稳定后执行 orchestration L2」、`ROADMAP.md:86`「不进入 Multi-Agent 大规模接入」一致——是显式的延后，不是遗漏。但评审必须把它写清楚，避免「Phase 12 已完成（🟢 TargetVerified）」被误读为「multi-agent 已可跑」。Phase 11 review（p11-review.md §0.3）对 sandbox/PTY 同类问题有同样记录，可对齐口径。

### 3.3〔中〕`OrchestrationEvent` 与 core canonical event 的关系未定

**事实**：lifecycle.rs 定义了自己的 `OrchestrationEvent`（26 变体，独立 enum），不复用 `agent-events` 的 canonical 事件类型。ADR-016（事件持久重放）要求「所有 Agent 事件必须可持久化、可重放」，`OrchestrationEvent` 自己实现了 `Serialize/Deserialize` 并有 `replay_workers`——但它是 orchestration 内部的独立事件流，没有接入 session-store / core event store。

后果：

- orchestration 事件目前只在 `AgentSupervisor.event_log: Arc<Mutex<Vec<OrchestrationEvent>>>`（supervisor.rs:134）这个内存 Vec 里，**不落 SQLite、不进 core event store、不广播给 GUI**。
- 「可重放」目前只意味着「`replay_workers(&events)` 能从 Vec 重建状态」，不意味着「崩溃后从持久化恢复」——因为没有持久化。
- ADR-033 只决定 `AgentSupervisor`/`TaskGraph` 归属 orchestration、不在 Provider control plane 重建，没说 orchestration 事件如何并入 core event store。ADR-016 全文不提 orchestration。

**判定**：当前的「可持久化、可重放」是「可序列化、可从内存 Vec 重放」，不是「已持久化」。这与「先交付库」的定位一致，但 `multi-agent.md` 的「事件全部可序列化、可重放（ADR-016），崩溃恢复无悬挂 worker」措辞容易让人以为已经持久化。建议文档区分「事件可序列化（已做）」与「事件已持久化到 event store（接线后）」。

---

## 4. 合并 / 拆分 / 简化建议

> 优先级：P0 = 影响架构判断或正确性，应优先；P1 = 明显减码减概念；P2 = 清理收益有限。

| # | 建议 | 优先级 | 类型 | 预期收益 |
| --- | --- | --- | --- | --- |
| 1 | **接线：让 Supervisor 在 spawn 时调 `WorktreeAllocator::allocate`**，把 `worktree_path` 从外部传入改为 Supervisor 按需分配并持有 `WorktreeGuard` | P0 | 接线 | 消除 P12-1 与 P12-3 的脱节，worktree 真正进入生命周期 |
| 2 | **接线：把 `TaskGraph` 作为 Supervisor 的可选调度器**（spawn 时 register task，complete/fail 时推进图，emit Task* 事件） | P0 | 接线 | 让 7 个死 Task* 事件复活，P12-2 与 P12-1 协同 |
| 3 | **接线：把 `PatchMerger` 接到 `complete()` 之前的 patch 收集**（或至少在文档声明接线延后） | P0 | 接线 | 让 3 个死 Patch* 事件复活，P12-5 与 P12-1 协同 |
| 4 | **删 `AgentConcurrency` / `ConcurrencyGuard`**（budget.rs:236-292 + 测试），或用它替换 `active_worker_count` | P1 | 删死代码 | 减约 80 行 + 消除文档-实现不符 |
| 5 | **删 Task* / Patch* / ConcurrencyDenied 事件变体**（接线前），接线时再加回 | P1 | 删死枚举 | `OrchestrationEvent` 从 26 变体降到 15，消除「事件已建模」假象 |
| 6 | **删 `checkpoint-service` + `agent-events` 死依赖**（Cargo.toml:13-14），同步 workspace-layout.md | P1 | 删依赖 | 减编译面，消除「落 checkpoint / 发 canonical event」错觉 |
| 7 | **修 `LedgerContext` 归属**：从 `AcquireRequest`/lease 取 account_id/provider_id/model_id，不要硬编码 `"unknown"` | P1 | 修正确性 | 让 P12-4 的 account 维度归属真正生效 |
| 8 | **让 `WorkerBudgetController` 被喂数据**（接线后由 agent loop 报 usage），或显式标记「度量待接线」 | P1 | 接线/文档 | 让 `BudgetExceeded` 真正可触发 |
| 9 | **删 `AgentSupervisor::parent_id()`**（supervisor.rs:155，死 API） | P2 | 删死代码 | 减误导 |
| 10 | **修 `multi-agent.md` 的 `AgentTree`**：改成实际类型名或声明待实现 | P2 | 文档 | 消除文档-代码漂移 |
| 11 | **改 P12-1 计划第 3 步措辞**：「复用 Agent Engine」→「保留 agent-engine 接入点，接线在集成阶段；本轮 Supervisor 为纯状态机」 | P2 | 文档 | 消除计划-实现脱节 |
| 12 | **`WorktreeGuard::drop` 的 Drop 内 `tokio::spawn`** 改为显式 async release（接线时一并处理） | P2 | 反模式 | 避免 runtime 缺失 panic + worktree 泄漏 |

**核心方向**：P0 的三条接线（#1/#2/#3）是 Phase 12 从「零件」变成「编排器」的关键；P1 的四条删除/修复（#4/#5/#6/#7）能在不损失能力的前提下减少约 200+ 行代码与 11 个死事件变体、2 个死依赖。**不建议新增任何抽象**——当前的问题全部是「已写的抽象没被用」，不是「缺抽象」。

---

## 5. 架构符合度

| 架构红线 / 约定 | 状态 | 证据 |
| --- | --- | --- |
| 纯 Rust，无 Node/V8/JS Runtime | 符合 | orchestration 无相关依赖 |
| `agent-domain` 零业务依赖（无 GUI/SQLite/HTTP/Git/Provider） | 符合 | agent-domain 仅 serde/serde_json/ts-rs（deepseek 核查） |
| 无循环依赖 | 符合 | orchestration → 4 个契约 crate 单向；4 个 crate 不反向依赖 |
| Agent Engine 不按 Provider 名走特例 | 符合（不适用） | orchestration 不直接调 Provider，只经 lease |
| Secret 不入库不入日志 | 符合 | Worker 只持 `LeaseGuard`，不读 API key |
| 所有 Agent 事件可持久化、可重放（ADR-016） | 部分 | `OrchestrationEvent` 可序列化 + `replay_workers` 可重放，但**未持久化**（仅内存 Vec，未接 event store）——见 §3.3 |
| GUI 不直接访问 Provider/DB/Tool | 符合（不适用） | orchestration 无 GUI 消费方 |
| `workspace-layout.md` 登记的依赖方向 | 部分漂移 | 登记依赖 `checkpoint-service` / `agent-events` 实际未使用（§2.5） |
| Provider-side Multi-Agent 不伪装成本地 Worker | 符合（不适用） | 当前无 Provider-side multi-agent 接入 |

**架构层面无红线违反**。主要问题是「登记的依赖有 2 个没用」+「事件未持久化」+「四个子系统不接线」，都是实现/接线层面，不是架构决策错误。ADR-033（控制面分离）的「`AgentSupervisor`/`TaskGraph` 归属 orchestration、不在 Provider control plane 重建」被忠实遵守——编排没有重复实现健康/路由状态机。

---

## 6. 改进优先级（总结）

1. **P0 — 接线三件套**（#1 worktree 分配 / #2 TaskGraph 调度 / #3 patch 合并管道接入 Supervisor）：让 Phase 12 从「四个独立库」变成「一个编排器」。这是 Phase 12 价值兑现的前提；在接线前，P12-2/P12-3/P12-5 名义上「在 P12-1 之上」实际上与 P12-1 毫无调用关系。
2. **P1 — 删减死抽象**（#4 `AgentConcurrency` / #5 死事件变体 / #6 死依赖 / #7 ledger 归属 / #8 预算空转）：减约 200+ 行 + 11 个事件变体 + 2 个依赖，消除文档-实现脱节。**只删/修，不加。**
3. **P2 — 文档与低风险清理**（#9 `parent_id()` / #10 `AgentTree` 文档 / #11 P12-1 计划措辞 / #12 Drop 反模式）：对齐口径，避免后续误判。

**是否需要新增抽象**：**否**。当前所有问题都是「已实现的抽象未被消费」，不是「缺抽象」。Phase 12 的正确收敛方向是「接线 + 删死代码 + 修归属」，而非增加新的模块/接口/概念。

**与 ROADMAP 节奏的一致性**：当前的「未接入主流程」与 ROADMAP.md:86「核心 Coding Agent 稳定前不进入 Multi-Agent 大规模接入」一致，**不是交付缺陷**。Phase 12 标记为 🟢 TargetVerified 在「库级验收」层面成立（每个子系统的库级行为都通过测试）。问题在于「库级验收」与「编排级验收」之间的鸿沟没有被任何验收项覆盖——P12-1～P12-6 的验收标准全部是库级的，没有任何一条要求「Supervisor 把 TaskGraph/worktree/patch 串起来」。建议后续在集成阶段（P13 或专门的 orchestration 接线任务）补一个「编排级端到端」验收项：parent spawn worker → 分配 worktree → 进 TaskGraph → 跑（mock）agent loop → 收 patch → 检冲突 → parent 审批合并 → complete，全程事件可重放。

---

## 7. 相关文档

- [multi-agent](../features/multi-agent.md) · [workspace-layout](../architecture/workspace-layout.md) · [ADR-016 事件持久重放](../adr/ADR-016-core-event-persist-replay.md) · [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md)
- [ROADMAP Phase 12](../../ROADMAP.md)（第 442–453 行）· [P12-1](../../plan/P12-1-supervisor-worker.md) ～ [P12-6](../../plan/P12-6-cancel-tree.md)
- 历史口径参考：[p11-review §0.3](p11-review.md)（sandbox/PTY 同类「未接入主流程」记录）
