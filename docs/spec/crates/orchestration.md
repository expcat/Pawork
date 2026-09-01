# pawork-orchestration

> 多 Agent 编排：在控制面（租约 / 用量 / 租户策略）之上运行 `AgentSupervisor`——spawn worker、预算闸门、取消整棵子树、任务依赖图、可选 git worktree 隔离与 patch merge。依赖 `pawork-domain` + `pawork-control-plane`（关 default features），不依赖 `pawork-workflow`。

## 1. 职责与边界

- **做什么**：worker 生命周期状态机与事件溯源（`lifecycle`）；Supervisor 集中拥有 spawn / start / complete / fail / cancel_tree / 恢复诊断（`supervisor/`）；worker 级 token / cost 预算度量与 ledger flush（`budget`）；任务依赖 DAG（`task_graph`）；worker 身份（`identity`）；worktree 分配抽象与 RAII 守卫（`worktree`）；patch 收集 / 冲突检测 / Parent 审批合并（`merge`）。
- **不做什么**：不写 SQLite（预算超限走注入的控制面 `UsageLedger`）；不自动合并冲突（Parent 审批门）；不自动重试任务；不依赖 `pawork-workflow`（plan / task 装配在 app）；`OrchestrationEvent` 独立于 `pawork-domain::AgentEvent`，持久化由宿主决定。
- **注入面**：`CredentialPool`（租约）、`TenantPolicyEngine`（策略闸门）、`UsageLedger`（用量账本）来自 `pawork-control-plane`；`WorktreeAllocator` / `PatchMerger` / `TaskGraph` / parent workspace 经 builder 方法可选注入。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~20 | 7 个私有模块全部 glob re-export 到 crate 根（无 `pub mod` 子命名空间） |
| `src/supervisor/mod.rs` | ~2750（非测试 ~440） | `AgentSupervisor` 结构（workers / cancel_tokens / children / reservations / budget / event_log / pending_patches / flush_ctx / flush_in_flight）；`SupervisorConfig`（并发 16、池并发建议 4、默认预算、`max_worker_depth`）；`SupervisorError`（10 变体）；builder 注入；`complete` / `fail` / `retry_task` / `propose_patch` / `approve_patch`；约 2300 行内联测试 |
| `src/supervisor/spawn.rs` | ~720 | `SpawnRequest`；`spawn` 全流程（parent 准入 → 原子并发预约 → 策略闸门 → worktree → lease → 注册）；`ConcurrencyReservation`（RAII 槽位）；lease 作用域校验 `validate_lease_scope`；策略决策审计记录 |
| `src/supervisor/cancel_tree.rs` | ~175 | `cancel_tree`（BFS 取消整棵子树）；`CancelTreeReceipt` |
| `src/supervisor/budget_gate.rs` | ~260 | `record_usage`（预算检查 + `BudgetExceeded` 去重发射）；`flush_usage`（终态 flush 显式重试）；`flush_terminal_usage`（pub(crate) 终态路径）；`FlushTicket`（在途标记 RAII） |
| `src/supervisor/recovery.rs` | ~50 | `recover_report`（report-only 崩溃诊断）；`RecoveryReport` |
| `src/supervisor/registry.rs` | ~180 | `WorkerEntry`（instance + 状态机 + lease / worktree 守卫 + model）；`start_worker`；`cancel_token` / `state` / `events` 查询；`emit` / `remove_child`；`apply_terminal_and_take`（终态锁内取守卫） |
| `src/budget.rs` | ~900（非测试 ~415） | `WorkerBudgetLimits`（None = 不限）；`UsageAccumulator`（原子累加）；`BudgetReport`；维度常量 `DIM_INPUT_TOKENS` / `DIM_OUTPUT_TOKENS` / `DIM_COST_MICROS`；`DEFAULT_SOFT_RATIO = 0.8`（ppm 整数比较避免 f64 误差）；`WorkerBudgetController`（check / diff_hard_exceeded / flush_to_ledger 幂等提交游标）；`LedgerContext` |
| `src/lifecycle.rs` | ~630（非测试 ~435） | `WorkerState` 九态；`WorkerTransition` / `transition` 纯函数 / `WorkerStateMachine`（apply 返回 `EventHint`）；`LifecycleError`；`OrchestrationEvent`（21 变体，serde tag/content snake_case）；`replay_workers` 容错重放 |
| `src/task_graph.rs` | ~540（非测试 ~370） | `TaskGraph` 线程安全 DAG（`Arc<Mutex<BTreeMap>>`，锁不跨 await）；`TaskId` / `TaskState` 八态 / `AgentTask`；add_task（拒环、拒跨租户依赖、允许前向引用）；ready / assign / start / complete（幂等）/ fail / cancel / retry / ready_tasks / detect_cycle |
| `src/worktree.rs` | ~360（非测试 ~185） | `WorktreeAllocator` trait；`WorkerWorktree`；`WorktreeGuard`（显式 `release` 消费；Drop 只告警不释放）；`GitWorktreeAllocator`（仅 feature `git`，委托 pawork-git `WorktreeService`，释放绝不删用户数据） |
| `src/merge.rs` | ~620（非测试 ~355） | `DiffProvider` trait（changed_files / file_content / base_content）；`GitDiffProvider`（仅 feature `git`）；`WorkerPatch` / `PatchProposal` / `ConflictReport` / `MergeOutcome` / `MergeDecision`；`PatchMerger`（collect / detect_conflicts / merge）；`resolve_relative` 路径越界防护；`atomic_write`（tmp + rename） |
| `src/identity.rs` | ~155 | `WorkerRole::{Parent, Worker}`（serde snake_case）；`AgentInstance` 不可变身份（tenant / principal / parent / session / worktree_path / created_at_ms）与 `new_parent` / `new_worker` 构造 |

无独立 `tests/` 目录：全部测试内联于各模块 `#[cfg(test)]`。

## 3. 对外 API 面

### 3.1 `AgentSupervisor`

- 构造：`new(pool: Arc<dyn CredentialPool>, policy: Arc<dyn TenantPolicyEngine>, ledger: Arc<dyn UsageLedger>, config: SupervisorConfig)`；builder：`with_parent_workspace(PathBuf)` / `with_worktree_allocator` / `with_task_graph` / `with_patch_merger`。
- `SupervisorConfig` 字段：`max_agent_concurrency`（默认 16，本地全局闸门）；`default_pool_concurrency`（默认 4，建池建议值，本包不建池）；`budget`（spawn 未携带预算时的默认 `WorkerBudgetLimits`）；`max_worker_depth`（`None` 不限）。
- `SupervisorError` 变体一览：`UnknownAgent` / `IllegalLifecycle` / `PolicyDenied` / `PoolAcquire` / `LeaseError` / `Merge` / `WorkerTerminal`（终态拒绝 record_usage）/ `UsageFlushPending`（flush 失败或在途）/ `FlushNotTerminal` / `FlushContextMissing` / `CancelTreeFlushPending { receipt, pending }`。
- `spawn(SpawnRequest) -> Result<AgentId, SupervisorError>`：创建并启动 worker（流程见 §4.1）。`SpawnRequest` 携带 tenant / principal / parent_id（None = 根 Parent）/ session / 可选 worktree_path / 可选预算覆盖 / 可选 model（过租户模型白名单）/ 可选 `AcquireRequest`（申请 lease）/ 任务图参数（deps / description / max_retries）。
- `start_worker(&AgentId)`：Starting → Running，发 `WorkerRunning`。
- `complete(&AgentId)` / `fail(&AgentId, reason)`：终态收口（流程见 §4.3）。
- `cancel_tree(&AgentId) -> Result<CancelTreeReceipt, SupervisorError>`：递归取消整棵子树（见 §4.2）；receipt 含 `cancelled_ids` 与 `leases_released`。
- `record_usage(&AgentId, input, output, cost_micros)`：累加用量并对「新进入硬超限」维度发 `BudgetExceeded`（持续超限去重、回落后可再告警）；终态 worker 拒绝（`WorkerTerminal`）。
- `flush_usage(&AgentId)`：终态用量 flush 的显式重试入口；活动 worker 拒绝（`FlushNotTerminal`）；在途并发拒绝（`UsageFlushPending`）；controller 在而归属 ctx 缺失 → `FlushContextMissing`（保留 controller 不吞 pending）。
- `retry_task(&AgentId) -> Result<u32>`：仅复位 TaskGraph 状态（Failed→Created，attempt 递增）并发 `TaskRetried`；worker 生命周期仍是 Failed 终态，重跑需新 spawn。
- `propose_patch(&AgentId, WorkerPatch) -> ConflictReport`：收集 worker 变更入待审批表，发 `PatchProposed`（有冲突另发 `PatchConflict`）；`approve_patch(&AgentId, MergeDecision) -> MergeOutcome`：按 Parent 决策执行（见 §4.4）。两者都要求已注入 `PatchMerger` 与 parent workspace，否则 `PolicyDenied`。
- 查询：`state(&AgentId) -> Option<WorkerState>`；`cancel_token(&AgentId)`；`events() -> Vec<OrchestrationEvent>`（事件快照，重放输入）。
- `recover_report(&[OrchestrationEvent]) -> RecoveryReport`：**report-only** 崩溃诊断——重放后把仍处活动态的孤儿在报告中推演为 Failed；不重建 WorkerEntry / children / cancel token，不 emit 事件，报告不可作为继续操作的恢复态。

### 3.2 预算（`budget`）

- `WorkerBudgetController::new(limits)` / `.with_soft_ratio(0.0..=1.0)`；`record_tokens` / `record_cost` / `usage()` / `limits()`。`Clone` 是同一逻辑控制器的共享句柄（累加器、提交游标、去重记忆全共享）。
- `check() -> BudgetReport`：软告警（`used >= ratio × limit`，ppm 整数比较）与硬超限（`used >= limit`）分维度报告；`diff_hard_exceeded(&report)` 返回新进入硬超限的维度并维护「已告警」记忆（恢复后遗忘、可再告警）。
- `flush_to_ledger(&dyn UsageLedger, &LedgerContext)`：把 `目标快照 - last_committed` 的增量写为一条 `UsageRecord`（record_id 含控制器 id 与目标 totals，幂等键）；async mutex 序列化，失败 / 取消保留完全相同的 pending record 供重试，Ok 后才推进游标；无增量为空操作。
- `UsageAccumulator`：无锁原子多维累加器。`LedgerContext`：flush 归属（tenant / principal / account / session / agent / provider / model / 可选 run 与 credential_id）。

### 3.3 生命周期与事件（`lifecycle`）

- `WorkerState`：`Created → Admitted → Starting → Running ↔ Waiting`，活动态可 `→ Cancelling → Cancelled` 或 `→ Failed`，`Starting|Running|Waiting → Completed`；`is_terminal` / `is_active`。终态拒绝一切转换（`LifecycleError::FromTerminal`）。
- `transition(from, t)` 纯函数；`WorkerStateMachine::apply` 返回 `(新状态, EventHint)` 供调用方发事件。
- `OrchestrationEvent`（serde `tag = "type", content = "data"`）：Worker 九件套（Created/Admitted/Started/Running/Waiting/Completed/Cancelling/Cancelled/Failed）+ Task 七件套（Created/Ready/Assigned/Completed/Failed/Retried/Cancelled）+ `BudgetExceeded` + `ConcurrencyDenied` + Patch 三件套（Proposed/Merged/Conflict）。
- `replay_workers(&[OrchestrationEvent]) -> BTreeMap<AgentId, WorkerState>`：事件溯源重建；终态事件（Complete/Cancel/Fail）直接落终态、不要求中间事件完整（日志可能因崩溃截断），非终态事件走严格状态机、非法静默跳过，终态后迟到事件忽略。

### 3.4 任务图 / worktree / merge / identity

- `TaskGraph`：`add_task`（重复 id / 已知依赖跨租户 / 成环拒绝；允许前向引用；初始态按依赖完成度 Ready 或 Blocked）、`mark_ready` / `assign` / `start` / `complete`（幂等）/ `fail` / `cancel`（任意非终态）/ `retry`（仅 Failed 且未达 max_retries）/ `ready_tasks`（Blocked 且依赖全完成）/ `state_of` / `tenant_of` / `detect_cycle`（公开 DFS 辅助）。
- `WorktreeAllocator::{allocate(parent_path, branch, start_point), release(path)}`；`WorktreeGuard::release()` 显式消费释放（幂等，失败也置 unmanaged），`into_inner()` 转移所有权不释放，Drop 只告警。`GitWorktreeAllocator` 仅 feature `git`。
- `PatchMerger::new(Arc<dyn DiffProvider>)`：`collect`（变更清单 + 最终内容）→ `detect_conflicts`（父侧当前内容 blake3 哈希 vs 基准；父侧无此文件 = 干净）→ `merge`（见 §4.4）。`GitDiffProvider` 仅 feature `git`（基准走 `git show HEAD:<rel>`，失败退化为父侧当前内容）。
- `AgentInstance` / `WorkerRole::{Parent, Worker}`：不可变身份记录，Agent 与账号状态机隔离。

## 4. 核心行为与数据流

### 4.1 spawn 全流程（deny-first，任一层拒绝不可被上层覆盖）

1. **parent 准入**：parent 存在、同 tenant、同 session、状态活动且非 Cancelling，否则 `PolicyDenied`（不写任何注册表）。
2. 生成 canonical `agent-N` id；深度闸门（`max_worker_depth`，超限发 `ConcurrencyDenied{kind:"depth"}`）。
3. **原子并发预约**：单一临界区合并「活动 worker + 在途预约」计数，先全局本地上限（拒绝发 `ConcurrencyDenied{kind:"agents"}`）再租户 `max_concurrent_agents`；通过后插入 RAII `ConcurrencyReservation`——后续任一步失败自动归还槽位，杜绝 check-then-act 超配。
4. **策略闸门**：`AgentSpawn` 权限 → 模型白名单 → （携带 acquire 时）AcquireRequest 外层一致性校验 + `LeaseAcquire` 权限 + provider / account 白名单 → 日 token/cost 预算（配置任一维度才查账本；账本不可用 fail-closed 拒绝）。每次允许 / 拒绝都记 versioned `PolicyDecisionEvent` 审计。
5. 创建 `AgentInstance`（Parent / Worker 按 parent_id）；状态机 Admit。
6. **worktree 分配**（配置了 allocator + parent workspace 且请求未自带路径时）：失败则标记 Failed、发 `WorkerFailed`、注册条目后返回 `PoolAcquire` 错误（事件流一致，恢复不留悬挂 worker）。
7. 发 `WorkerCreated`（含分配后真实 worktree 路径）→ `WorkerAdmitted`。
8. **lease 申请**（可选）：AcquireRequest 的 agent/tenant/principal/session 由 supervisor 用 canonical 值覆写（不信任调用方拼接）；acquire 成功后再校验 pool 返回的 lease 作用域（`validate_lease_scope`，不信任 pool），错配 fail-closed：显式以 `Released` 归还 lease（不惩罚账号健康）、释放 worktree、标记 Failed 注册后返回。acquire 失败同样释放 worktree、Failed 注册返回。
9. 状态机 Start，发 `WorkerStarted`；注册 children 边与该 worker 的 `CancellationToken`。
10. **TaskGraph 注册**（配置时）：`add_task`（初始 Ready / Blocked 由依赖决定）发 `TaskCreated`；Ready 任务立即 `TaskReady` → assign（`TaskAssigned`）→ start；Blocked 任务等依赖完成后经 `ready_tasks` + `mark_ready` + 外部推进。
11. 原子兑现预约：同一临界区从 reservations 移除并插入 `WorkerEntry`；随后注册 `WorkerBudgetController`（请求预算或 config 默认）。

### 4.2 cancel-tree

1. 根不存在 → `UnknownAgent`。BFS 沿 children 图收集整棵子树。
2. 逐节点：先触发取消令牌；锁内——终态节点跳过，活动节点 `BeginCancel → Cancel` 双转换、移除预算 controller（登记 flush 在途票据）、take lease / worktree 守卫。
3. 发 `WorkerCancelling` → `WorkerCancelled`；worktree best-effort 释放；TaskGraph 推进 Cancelled 并发 `TaskCancelled`。
4. lease 以 `LeaseOutcome::Cancelled` 幂等释放——只累加取消计数，**不惩罚账号健康**（不计连续失败）。
5. 终态用量 flush（与 complete / fail 同路径）；失败的节点进入 pending 列表。
6. 取消总是完成；若有 flush 失败，返回 `CancelTreeFlushPending { receipt, pending }`——错误携带完整 receipt 与待重试列表，调用方逐个 `flush_usage` 重试，不吞 pending。重复 cancel_tree 幂等（第二次不再取消、不重复释放）。

### 4.3 终态收口（complete / fail）与预算 flush

1. `apply_terminal_and_take`：锁内应用 Complete / Fail 转换并取走 lease / worktree / controller / 归属（含 flush 在途票据）。
2. worktree best-effort 释放；TaskGraph 推进（Completed / Failed）并发对应 Task 事件。
3. 归属取真实值：account / provider 读自 lease（释放前），model 取 spawn 请求；无 lease 回退 `local/default` + `local`。
4. lease 显式标记 outcome 后 Drop 触发同步幂等释放：`Completed`（正常）/ `Failed`（计入连续失败）。LeaseGuard 默认 outcome 为 Failed（fail-safe：未显式标记不得计作成功）。
5. `flush_terminal_usage`：controller 累计用量写入 ledger；失败把 controller + `LedgerContext` 放回表内并返回 `UsageFlushPending`（生命周期转换不回滚，仅保留用量可重试）；重试经 `flush_usage`，幂等键保证不重复计账。
6. 发 `WorkerCompleted` / `WorkerFailed`，从父的活跃 children 中移除。

### 4.4 patch 审批合并

`propose_patch`：`collect`（DiffProvider 取变更清单与内容）→ `detect_conflicts`（父侧 blake3 vs 基准）→ 存入 `pending_patches` → 发 `PatchProposed`（+冲突时 `PatchConflict`）。`approve_patch`：取出提案 → `MergeDecision::Merge` 时重新检测冲突，仍有冲突返回 `MergeError::ConflictUnresolved`（**绝不自动合并冲突**）；干净文件原子写（同目录 tmp + rename）合入 parent；`Reject` / `NeedsConflictResolution` 不写任何文件。发 `PatchMerged` / `PatchConflict`。

## 5. 契约与不变量

- **禁止依赖 `pawork-workflow`**（包依赖方向红线；plan/task 与编排的装配在 app）。
- **取消覆盖整棵 worker 树**：无孤儿；`Cancelled` 释放不惩罚账号健康。
- **预算超限走控制面 ledger**：本包不写 SQLite；`BudgetExceeded` 同一维度持续超限只发一次，回落后可再告警。
- **flush 不吞 pending**：终态 flush 失败必显式返回（`UsageFlushPending` / `CancelTreeFlushPending`），controller 与归属 ctx 成对保留；提交游标 + 幂等 record_id 保证重试不重复计账；在途标记防并发假成功。
- **spawn 原子并发**：预约与兑现在单一临界区，无 check-then-act 窗口；RAII 保证失败路径归还槽位。
- **不信任外部输入**：AcquireRequest 由 supervisor 覆写 canonical 身份；pool 返回的 lease 作用域必须校验，错配 fail-closed。
- **事件流一致**：spawn 中途失败也把 worker 标记 Failed 并注册（`WorkerFailed` 落日志），恢复重放不留悬挂 worker。
- **recover_report 是 report-only**：不重建可操作状态、不 emit 事件。
- **worktree 释放安全**：只经 allocator（git 侧校验受管 worktree），绝不递归删除用户数据；Guard Drop 不隐式释放（避免无 runtime panic），须显式 `release`。
- **merge 路径安全**：相对路径拒绝绝对分量与 `..` 穿越；写入原子（tmp + rename）。
- 状态机冻结：`WorkerState` 转换表、`TaskState` 转换表与 `OrchestrationEvent` serde 形状（`type`/`data` snake_case）是重放兼容面。

## 6. 依赖关系

- **生产依赖**：`pawork-domain`；`pawork-control-plane`（`default-features = false`，不拉 rusqlite）；optional `pawork-git`（仅 feature `git`）；`async-trait` / `serde` / `serde_json` / `thiserror` / `tokio`（sync, rt, macros）/ `tracing` / `blake3`。
- **features**：`default = []`；`git = ["dep:pawork-git"]`（启用 `GitWorktreeAllocator` 与 `GitDiffProvider`）。当前 workspace 无成员打开 `git` feature（`pawork-app` 以 default-features = false 依赖本包）。
- **被依赖**：仅 `pawork-app`（orchestration host）。

## 7. 测试与验证资产

无 `tests/` 目录，全部内联 `#[cfg(test)]`：

| 位置 | 覆盖点 |
| --- | --- |
| `supervisor/mod.rs`（~40 个用例） | spawn 生命周期事件序（created→admitted→started）；lease 持有到 complete / fail 的 outcome 与账号健康；cancel_tree 递归取消、幂等、lease Cancelled 释放、flush pending 上抛；并发闸门（全局 / 租户 / 深度，`ConcurrencyDenied`）；策略闸门（角色 / 模型 / provider / account 白名单、日预算 fail-closed、AcquireRequest 错配、恶意 pool lease 作用域校验）；`record_usage` 终态拒绝与 `BudgetExceeded` 去重；`flush_usage` 重试矩阵（not-terminal / context-missing / in-flight / 幂等重放）；worktree 分配失败路径与显式释放；TaskGraph 联动（Ready 直启 / Blocked 等待 / retry）；patch propose→approve 全流程；`recover_report` 孤儿推演 |
| `lifecycle.rs` | 状态机合法 / 非法转换矩阵；终态拒绝；`replay_workers` 容错重放（截断日志、迟到事件）；事件 serde round-trip |
| `budget.rs` | 软 / 硬阈值判定与 ppm 精度；`diff_hard_exceeded` 去重与恢复再告警；flush 幂等游标（失败重放同 record、cost-only 增量、并发 clone 句柄共享游标） |
| `task_graph.rs` | 拒环 / 跨租户依赖 / 重复 id；前向引用与 ready_tasks；转换矩阵；retry 上限 |
| `worktree.rs` / `merge.rs` / `identity.rs` | Guard 显式释放与 Drop 告警语义；冲突检测（基准 vs 父侧）、Merge 拒绝未解决冲突、原子写、路径穿越拒绝；身份构造与 serde |

默认验证命令：`cargo test -p pawork-orchestration --offline --lib --tests`。

## 8. 注意事项与已知限制

- Cargo.toml 的 package description 仍含「Agent Teams」，Teams 已随 V2 归档（tag `v2-final`），以源码树为准；不要把 `AppEvent::TeamEvent` 当成现行编排面。
- `OrchestrationEvent` 是本包私有事件模型（内存 `event_log`），不并入 `pawork-domain::AgentEvent`；持久化与对外投影由宿主负责。
- 预算度量当前只贯通 input / output / cost 三维；`UsageRecord` 的 cache_read / cache_write 恒写 0（cache 通路未贯通，完整贯通单独排期，不写误导值）。
- `recover_report` 不能作为热恢复：崩溃后要继续操作需重新 spawn；报告仅用于诊断孤儿。
- `retry_task` 只复位任务图状态，不会复活 Failed worker。
- `WorktreeGuard` 未显式 release 而 Drop 时 worktree 保持存在（仅告警）——长驻进程需保证终态路径都走显式释放（complete / fail / cancel_tree 已覆盖）。
- 每个 worker 的 `CancellationToken` 由 spawn 注册，但令牌与实际执行体的绑定由宿主完成；本包只保证取消信号可查询、可触发。
- 相关文档：[architecture](../../architecture.md) · [design](../../design.md) §2 · [flows](../flows.md) · [Spec 总览](../README.md) · [产品候选](../backlog.md)；相邻包：[domain.md](domain.md) · [control-plane.md](control-plane.md) · [git.md](git.md) · [app.md](app.md)。
