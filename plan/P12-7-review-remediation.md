 # P12-7：Phase 12 评审修复（REVIEW remediation）
 
 > Phase 12 · Multi-Agent · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P12-1 ~ P12-6
 
 **最终目的**：按 [docs/review/p12-review.md](../docs/review/p12-review.md) §2/§3 收敛「四个子系统互不接线 + 死抽象 + 死依赖 + ledger 归属断裂 + 文档漂移」类问题——把 `WorktreeAllocator` / `TaskGraph` / `PatchMerger` 经可选 builder 接进 `AgentSupervisor`（spawn 分配 worktree、注册并推进 TaskGraph、complete/fail/cancel 联动；新增 `record_usage` / `retry_task` / `propose_patch` / `approve_patch`），让 11 个原本零生产者的 `Task*` / `Patch*` / `ConcurrencyDenied` / `BudgetExceeded` 事件全部获得真实 emit 点；删 `AgentConcurrency` / `ConcurrencyGuard` 死状态机与死 API `parent_id()`；删 `agent-events` / `checkpoint-service` 死依赖；修复 `LedgerContext` 把 account/provider/model 写成 `"unknown"` 的归属断裂（改从 lease 与 spawn 请求取真实值）；修 `WorktreeGuard::drop` 内 `tokio::spawn` 的 Drop 反模式（改显式 `async fn release()`，未显式释放的 Drop 仅告警）；同步 `multi-agent.md` 的 `AgentTree` 文档漂移、`workspace-layout.md` 的依赖登记、`P12-1` 第 3 步措辞。无新增抽象，全部为「接线 / 删除 / 修复 / 标注」。外部 agent-engine run loop 接线与主流程（CLI/GUI）接入按 ROADMAP 显式延后。
 
 **涉及范围**：`orchestration`（supervisor.rs、budget.rs、worktree.rs、Cargo.toml）、`docs/features/multi-agent.md`、`docs/architecture/workspace-layout.md`、`plan/P12-1-supervisor-worker.md`
 
 ## 处置策略（按评审 §4/§6 矩阵）
 
 - **现在修复（落地）**：§2.1 接线三件套（#1 worktree 分配 / #2 TaskGraph 调度 / #3 patch 合并管道）、§2.2 删 AgentConcurrency/ConcurrencyGuard、§2.3 给 11 个死事件变体生产者、§2.4 修 ledger 归属 + 喂预算控制器、§2.5 删 agent-events/checkpoint-service 死依赖、§2.6 删 parent_id + 修 AgentTree 文档、§2.7 修 WorktreeGuard Drop 反模式、§3.1 改 P12-1 第 3 步措辞。
 - **显式延后**：§3.1 agent-engine run loop 真实接线、§3.2 主流程（CLI/GUI）接入、§3.3 `OrchestrationEvent` 接入 core event store / SQLite 持久化、§4 #12（接线后 Supervisor 持有 worktree 时的统一 async release 已在本轮以 `WorktreeGuard::release()` + complete/fail/cancel_tree 显式调用形式落地，更激进的「Supervisor Drop 时统一 await 释放」留待主流程接线）。
 
 ## 细分步骤（分组）
 
 ### A. 接线：WorktreeAllocator 接入 Supervisor（§2.1 #1，W1）
 
 1. `AgentSupervisor` 新增 `parent_workspace` / `worktree_allocator` 字段 + `with_parent_workspace` / `with_worktree_allocator` builder；`WorkerEntry` 新增 `worktree: Option<WorktreeGuard>`。`spawn` 在 Admit 后、申请 lease 前，若配置了 allocator 且 spawn 请求未自带 worktree 路径，则 `allocate(parent_workspace, agent_id, None)`；分配失败走与 lease 失败同型的 Failed 路径（事件流一致、不留悬挂 worker）。`WorkerCreated` 携带分配后的真实路径。`complete` / `fail` / `cancel_tree` 显式 `WorktreeGuard::release().await`（best-effort，失败仅 warn）。
 
 ### B. 接线：TaskGraph 作为可选调度器（§2.1 #2，W2）
 
 2. `AgentSupervisor` 新增 `task_graph` 字段 + `with_task_graph` builder；`SpawnRequest` 新增 `task_deps` / `task_description` / `task_max_retries`。`spawn` 在注册 child/取消令牌后：`add_task` → emit `TaskCreated` → 无依赖任务直接 `Ready` 时 emit `TaskReady` → `assign` + emit `TaskAssigned` → `start`。`complete` 推进 `complete` + emit `TaskCompleted`；`fail` 推进 `fail` + emit `TaskFailed`；`cancel_tree` 每节点推进 `cancel` + emit `TaskCancelled`；新增 `retry_task`（`Failed→Created` 复位 + emit `TaskRetried`，仅复位任务、worker 仍终态）。
 
 ### C. 接线：PatchMerger 接入 complete 前的 patch 收集（§2.1 #3，W3）
 
 3. `AgentSupervisor` 新增 `patch_merger` / `pending_patches` 字段 + `with_patch_merger` builder；新增 `SupervisorError::Merge` 变体。`propose_patch(agent_id, patch)` → `collect` + `detect_conflicts` + emit `PatchProposed`（有冲突再 emit `PatchConflict`）+ 存入 `pending_patches`。`approve_patch(agent_id, decision)` → 取出 proposal + `merge` + emit `PatchMerged` / `PatchConflict`。
 
 ### D. 删死代码（§2.2 / §2.6）
 
 4. 删 `budget.rs` 的 `AgentConcurrency` / `ConcurrencyGuard` + 2 个测试；模块文档改为「Agent 并发闸门由 `AgentSupervisor` 活动 worker 计数 + `TenantPolicyEngine::check_agent_concurrency` 实现」（消除文档-实现不符）。
 5. 删 `supervisor.rs` 的 `pub fn parent_id()`（全仓零调用，硬编码 `AgentId::new("supervisor")` 的死 API）。
 
 ### E. 删死依赖（§2.5）
 
 6. 从 `orchestration/Cargo.toml` 删 `agent-events` / `checkpoint-service`（全 crate 零 `use`，merge 用 diff-service + git-service，事件用自建模的 `OrchestrationEvent`）；`workspace-layout.md` 同步依赖登记。
 
 ### F. 修 ledger 归属 + 喂预算控制器（§2.4，#7/#8）
 
 7. `complete()` 构造 `LedgerContext` 时 account_id / provider_id 改从刚 take 的 lease 经 `LeaseGuard::lease()` 取（默认 `local/default` / `local`），model_id 取自 spawn 请求的 `model`——不再硬编码 `"unknown"`，P12-4 的 account 维度归属真正生效。
 8. 新增 `record_usage(agent_id, input, output, cost_micros)`：累加到注册表内控制器，`check()` 硬超限的每个维度 emit `BudgetExceeded`。`BudgetExceeded` 事件获得真实触发源（接线后由 agent loop 报 usage 调用本方法）。
 
 ### G. 修 WorktreeGuard Drop 反模式（§2.7）
 
 9. `worktree.rs`：删 `Drop` 内的 `tokio::spawn`；新增显式 `pub async fn release(mut self)`（消费 self、释放并置 managed=false）；`Drop` 改为未显式释放时仅 `tracing::warn`（不 spawn、不 await，避免无 runtime 时 panic 与 worktree 静默泄漏）。测试 `guard_drop_releases_worktree` 改名 `guard_release_releases_worktree`（调显式 release）；新增 `guard_drop_without_release_does_not_spawn`（断言 Drop 不再触发释放）。
 
 ### H. 文档对齐（§2.6 / §3.1）
 
 10. `multi-agent.md`：`AgentTree`（不存在的类型）改为实际四件套 + builder 接线描述。
 11. `workspace-layout.md`：orchestration 依赖行删 `agent-events` / `checkpoint-service`。
 12. `P12-1-supervisor-worker.md` 第 3 步：「与 Agent Engine 复用」改为「Agent Engine 接入点（延后到集成阶段）」，承认本轮 Supervisor 为纯状态机编排器、agent loop 接线在 ROADMAP 集成阶段。
 
 ## 主要产出物
 
 - 接线：`AgentSupervisor` 的 4 个可选 builder（`with_parent_workspace` / `with_worktree_allocator` / `with_task_graph` / `with_patch_merger`）+ `WorkerEntry.worktree` / `SpawnRequest.{task_deps,task_description,task_max_retries}`；spawn/complete/fail/cancel_tree 全路径接入 worktree 分配/释放、TaskGraph 推进、patch 收集/合并；新增 `record_usage` / `retry_task` / `propose_patch` / `approve_patch` 方法。
 - 复活事件：`TaskCreated` / `TaskReady` / `TaskAssigned` / `TaskCompleted` / `TaskFailed` / `TaskRetried` / `TaskCancelled`（7）+ `PatchProposed` / `PatchMerged` / `PatchConflict`（3）+ `ConcurrencyDenied`（1）+ `BudgetExceeded`（已有变体，本轮补生产者）—— review §2.3 点名的 11 个零生产者变体 + BudgetExceeded 共 12 个变体现在全部有真实 emit 点。
 - 删除：`AgentConcurrency` / `ConcurrencyGuard`（约 60 行 + 2 测试）、`parent_id()`、`agent-events` / `checkpoint-service` 两个 crate 依赖。
 - 修复：`LedgerContext` 归属（lease 取 account/provider + 请求取 model，不再 unknown）、`WorktreeGuard` Drop 反模式（显式 async release）。
 - 文档：`AgentTree` → 四件套、依赖登记同步、P12-1 第 3 步措辞。
 
 ## 验收标准（保留 REVIEW 追踪章节）
 
 - [x] **§2.1**：四子系统接线落地——spawn 分配 worktree、注册并推进 TaskGraph（TaskCreated/Ready/Assigned）、complete/fail/cancel 联动；patch 经 propose_patch/approve_patch 走 collect/detect_conflicts/merge
 - [x] **§2.2**：`AgentConcurrency` / `ConcurrencyGuard` 已删；模块文档与实现一致；Supervisor 用 `active_worker_count` + TenantPolicyEngine 实现 agent 并发闸门
 - [x] **§2.3**：11 个死事件变体（Task*7 + Patch*3 + ConcurrencyDenied1）全部获得生产者；`OrchestrationEvent` 22 变体中 review §2.3 点名的 11 个 + BudgetExceeded 已全部获得生产者（WorkerWaiting 为 lifecycle.rs 既有无生产者变体，归 §3.1 延后）
 - [x] **§2.1 接线健壮性（review 复核追加）**：spawn 携带未完成前向依赖（`task_deps`）时任务保持 `Blocked`、不再强制 `assign`（原实现会让 spawn 在 emit Worker* 后因 assign 对 Blocked 报 IllegalState 而失败，导致事件流声称已 Started、注册表无条目、worktree 泄漏）；改为仅对 Ready 任务 assign+start，Blocked 任务等待依赖 complete 后由 ready_tasks+mark_ready+外部 assign/start 推进。测试 `spawn_with_unmet_task_deps_stays_blocked_and_consistent` 锁定该路径。
 - [x] **§2.4**：`LedgerContext` account/provider 改从 lease 取（默认 local/default / local）、model 取自 spawn 请求；`record_usage` 硬超限 emit `BudgetExceeded`；预算控制器有真实数据源入口
 - [x] **§2.5**：`agent-events` / `checkpoint-service` 从 orchestration Cargo.toml 删除；workspace-layout.md 同步
 - [x] **§2.6**：`parent_id()` 删除；multi-agent.md 的 `AgentTree` 改为四件套描述
 - [x] **§2.7**：`WorktreeGuard::drop` 不再 `tokio::spawn`；显式 `release()` + 未释放仅告警
 - [x] **§3.1**：P12-1 第 3 步措辞改为「延后到集成阶段」，消除计划-实现脱节
 - [x] **定向验证**：`cargo test -p orchestration --all-targets`（53 passed / 0 failed，含 9 个新测试）/ `cargo clippy -p orchestration --all-targets -- -D warnings`（通过）/ `cargo fmt -p orchestration -- --check`（通过）—— Commander 独立复跑确认
 
 ### Deferred items（建议/跟踪，本任务不做）
 
 - **§3.1 agent-engine run loop 真实接线**：本轮 Supervisor 为纯状态机编排器（不内嵌 agent loop）；agent-engine 自身生产消费亦尚未接入（provider_loop 生产调用点在 P3 测试内）。接线在 ROADMAP 明确的集成阶段（核心 Coding Agent 稳定后）落地。
 - **§3.2 主流程（CLI/GUI）接入**：orchestration 当前无 crate 直接依赖它（与 ROADMAP.md「核心 Coding Agent 稳定前不进入 Multi-Agent 大规模接入」一致）；未来 P17-6 `teams` crate 会依赖 orchestration。
 - **§3.3 `OrchestrationEvent` 接入 core event store / SQLite 持久化**：当前事件可序列化、可从内存 `Vec` 经 `replay_workers` 重放；持久化到 event store 与广播给 GUI 随主流程接线落地。
 - **§4 #12 Supervisor Drop 统一 await 释放**：本轮以 `WorktreeGuard::release()` + complete/fail/cancel_tree 显式调用形式落地；更激进的「Supervisor Drop 时统一 await 释放所有 worktree」留待持有持久 worktree 集合时实现。
 - **retry_task 语义闭环**：`retry_task` 把任务复位到 `Created` 并 emit `TaskRetried`（事件有生产者），但 task_id 由 agent_id 派生、新 spawn 生成新 id，Supervisor 当前无路径把复位后的旧任务再指派给新 worker——重试任务停在 `Created` 待外部 assign/start。完整的「失败 worker → 复位任务 → 新 spawn 接管 → 重跑」闭环随 agent loop 接线（§3.1）落地。
 - **编排级端到端验收**：建议后续集成阶段补「parent spawn worker → 分配 worktree → 进 TaskGraph → 跑（mock）agent loop → 收 patch → 检冲突 → parent 审批合并 → complete，全程事件可重放」的编排级端到端用例（P12-1～P12-6 库级验收已全部通过，本任务把四子系统接线打通使其可被编排级验收覆盖）。
 
 ## 验证记录（2026-08-10）
 
  - `cargo test -p orchestration --all-targets`：**53 passed / 0 failed**。9 个新测试全部通过：`spawn_with_allocator_assigns_isolated_worktree`、`spawn_without_allocator_preserves_old_behavior`、`task_graph_wiring_emits_task_events`、`retry_task_emits_task_retried`、`record_usage_emits_budget_exceeded`、`complete_flushes_ledger_with_real_attribution`（断言 `account_id == "local/default"` 非 `"unknown"`、model 取自请求）、`concurrency_denied_event_on_local_limit`、`propose_and_approve_patch_emits_events`、`spawn_with_unmet_task_deps_stays_blocked_and_consistent`（review 复核追加：前向依赖 spawn 一致性）；存量 44 个（lifecycle 8 / merge 7 / task_graph 7 / identity 3 / budget 3 / worktree 5 / supervisor 11）全过。
 - `cargo clippy -p orchestration --all-targets -- -D warnings`：通过。
 - `cargo fmt -p orchestration -- --check`：通过。
 - 残留核验（Commander）：`rg "AgentConcurrency|ConcurrencyGuard"` 在 orchestration 内零命中；`rg "fn parent_id"` 零命中；`rg "checkpoint_service|agent_events"` 零命中；review §2.3 点名的 11 个死事件变体 + BudgetExceeded 共 12 个经 spawn/complete/fail/cancel_tree/record_usage/retry_task/propose_patch/approve_patch 全部获得生产者。
 - 写集合核验（Commander）：仅 `crates/orchestration/{Cargo.toml,src/supervisor.rs,src/budget.rs,src/worktree.rs}` + `Cargo.lock`（自动同步）+ 3 个文档文件 + 本 plan，未触碰其它源码。diff stat：orchestration 4 文件 +903/-201；3 文档 +3/-3。
 - 按本任务门禁节奏只执行受影响 crate 的定向门禁；workspace 全量、三平台与发布门禁留待 Core 主干 L2/L3。
 
 **相关文档**：[REVIEW.md](../REVIEW.md) §P12 · [docs/review/p12-review.md](../docs/review/p12-review.md) · [multi-agent](../docs/features/multi-agent.md) · [workspace-layout](../docs/architecture/workspace-layout.md) · [ADR-016 事件持久重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [ADR-033 控制面分离](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP Phase 12](../ROADMAP.md)
