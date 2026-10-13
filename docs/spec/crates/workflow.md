# pawork-workflow

> Plan / Task 纯 reducer：在内存中折叠 canonical `PlanEvent` / `TaskEvent` 并提供命令面校验，事件返回给调用方持久化。依赖仅 `pawork-domain`；装配在 `pawork-app`。

## 1. 职责与边界

- **做什么**：Plan 聚合（版本替换、评审提交、审批 gate，以及历史 `PlanEvent` 的折叠）与 Background Task 聚合（process / agent / monitor / automation 四类任务的注册、开始、完成、取消传播、事件日志、断连恢复）。命令面先校验合法性再 `apply` 折叠，返回 canonical 事件供调用方经 `AgentEvent::Plan` / `AgentEvent::Task` 持久化；`apply` / `replay` 为崩溃恢复唯一入口，并继续折叠全部 `PlanEvent` / `TaskEvent` 变体（含历史行里的 `StepUpdated` / `Revised` / `CommentAdded` / `Suspended` / `Resumed`）。
- **不做什么**：不执行进程、不持有 SandboxBackend、不清理进程树、不落库、不依赖 `pawork-exec` / `pawork-orchestration`。Plan 是只读建议——`PlanService` 不暴露任何 spawn / exec / write / 文件 / 网络 API，审批仅作为执行 gate 放行，不扩权。
- **事件类型归属**：`PlanEvent` / `TaskEvent` 及各 Id / 状态枚举定义在 `pawork-domain`，本包只消费不重定义。Goal / Automation / Monitor 三域 reducer 已随 V2 归档（tag `v2-final`），domain 事件类型保留以便重放；`TaskKind::{Process, Agent, Monitor, Automation}` 是 domain 的任务种类枚举，与已归档 reducer 无关。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~10 | 仅 `pub mod plan` / `pub mod task`，无 glob re-export |
| `src/plan/mod.rs` | ~35 | plan 门面与 re-export；模块级文档（只读约束、event-sourcing 设计） |
| `src/plan/state.rs` | ~220 | `PlanState` 聚合（title / steps / 当前与父版本 / 修订链 history / 评审状态 / comments / 审批 checkpoint）；`apply` 纯折叠全部 `PlanEvent`（事件视为已校验事实，未知步骤的 `StepUpdated` 防御性忽略）；`replay`；`PlanComment` |
| `src/plan/service.rs` | ~350 | `PlanService` Host 命令面 + 查询面（`Mutex<Inner>`，毒锁 into_inner 继续）；id 生成（`plan_N` / `planver_N` / `step_N`）；`from_events` 重放重建并 seed 计数器（含历史 `Revised` / `CommentAdded` 的版本号）防 id 碰撞 |
| `src/plan/snapshot.rs` | ~30 | 查询面 DTO：`PlanSnapshot`、`PlanVersionInfo`（serde 可序列化；修订链节点由 `PlanState::history` 持有） |
| `src/plan/error.rs` | ~40 | `PlanError`：`AlreadyExists` / `NotCreated` / `EmptyPlan` / `EmptyStepText` / `IllegalReviewTransition` / `VersionMismatch` / `PlanIdMismatch` / `EmptyReason` |
| `src/task/mod.rs` | ~40 | task 门面与 re-export；模块级文档（统一抽象、断连续存、取消传播、执行所有权边界） |
| `src/task/state.rs` | ~300 | `TaskManagerState` 纯聚合（任务表 `BTreeMap` + 只追加事件日志）；`apply` 唯一折叠入口（`Started` 幂等，`Suspended` / `Resumed` / `Finished` 校验前置状态）；`TaskSnapshot` / `TaskManagerSnapshot`；`is_terminal_status`；`subtree` 后代收集；id 分配（进程唯一命名空间） |
| `src/task/manager.rs` | ~230 | `TaskManager` 命令面（register / start / finish / cancel）与查询面（task / tasks / snapshot / event_log / replay） |
| `src/task/error.rs` | ~30 | `TaskManagerError`（UnknownTask / UnknownParent / InvalidTransition / InvalidFinishedStatus） |
| `tests/plan_service.rs` | ~450 | Plan 命令面与历史事件重放（见 §7） |
| `tests/state_and_replay.rs` | ~360 | Task 状态机与重放，含历史 `Suspended` / `Resumed`（见 §7） |

## 3. 对外 API 面

### 3.1 `plan` 模块

- **命令面 `PlanService`**（Host 实际调用的方法；每个方法校验后 `apply` 并返回对应 `PlanEvent`，错误为 `PlanError`）：
  - `create_plan(title, step_texts) -> Created`：拒绝空 plan / 空白步骤文本 / 已存在 plan（单 Plan 聚合）。
  - `replace_plan(title, step_texts) -> Replaced`：整版替换，新版本 `parent_version` 指向旧版本；评审状态复位 `Draft`、comments 清空、审批 checkpoint 清空。
  - `request_review(version) -> ReviewRequested`：仅 `Draft → InReview`。
  - `approve(plan_id, version, checkpoint_id) -> Approved`：`InReview | ChangesRequested → Approved`，可携带批准点 checkpoint（可回滚）；`reject(plan_id, version, reason) -> Rejected`：同源状态 → `Rejected`，reason 必填非空。`ChangesRequested` 只来自历史事件重放，命令面仍接受该前置状态。
  - `from_events(events)`：重放重建 service，并从既有 id 后缀 seed 计数器（含历史 `Revised` 的版本号与步骤号），保证重放后新发 id 不与历史碰撞。
- **查询面**：`plan_snapshot() -> Option<PlanSnapshot>`（未创建为 None）；**审批 gate** `is_approved_for_execution(plan_id, version) -> bool`——仅当 plan_id 与当前版本都匹配且评审状态为 `Approved` 才返回 true，其余（未创建 / 不匹配 / 任何未批准状态）一律 false；只读判定，不授予任何写 / 执行能力。修订链在 `PlanState::history()`，不从 `PlanService` 另开查询方法。
- **纯函数**：`apply(&mut PlanState, &PlanEvent)` 折叠全部变体，不重复命令面校验；`replay(events) -> PlanState`。

### 3.2 `task` 模块

- **命令面 `TaskManager`**（`Clone` 共享同一内部状态）：`register(task_kind, parent_task_id) -> BackgroundTaskId`（Queued，不发事件，parent 必须存在）；`register_with_cancel_token(..., cancel_token)`（R-05：任务记录共享执行体的真实取消令牌，`cancel` 经此令牌停止真实执行体而非只推进状态机）；`start` (Queued→Running，`Started`)；`finish(task_id, status, detail)`（Running|Suspended → Completed|Failed，`Canceled` 必须走 `cancel`，否则 `InvalidFinishedStatus`）；`cancel(task_id) -> Vec<TaskEvent>`（沿 `parent_task_id` 链传播到全部后代：Running/Suspended 发 `Finished{Canceled}` 并触发 domain `CancellationToken`，Queued 静默移除，终态跳过）。`Suspended` 只由历史事件折叠而来，`finish` / `cancel` 仍接受该前置状态。
- **查询与恢复**：`task` / `tasks` / `snapshot`（任务视图 + 完整事件日志）/ `event_log` / `replay(events) -> usize`（重建视图与日志）。Host 以快照文件为持久化事实源，不订阅进程内广播。
- **纯状态**：`TaskManagerState::apply(&TaskEvent)` 折叠全部变体（可失败：未知任务 / 非法转移即 `Err`，与 Plan 的不可失败 apply 不同）；`is_terminal_status`（Completed/Failed/Canceled）。

### 3.3 事件与命令对照（事件定义在 `pawork-domain`）

| 事件 | 产生命令 | 折叠效果 |
| --- | --- | --- |
| `PlanEvent::Created` | `create_plan` | 建聚合、首版入 history、评审 Draft |
| `PlanEvent::Replaced` | `replace_plan` | 换当前版本、history 追加、评审复位 Draft、清 comments 与 checkpoint |
| `PlanEvent::Revised` | 无现行命令 | 换当前版本、history 按 version 去重追加、评审复位 Draft、清 checkpoint；comments 保留 |
| `PlanEvent::StepUpdated` | 无现行命令 | 改单步状态（未知步骤防御性忽略，不校验转移合法性） |
| `PlanEvent::ReviewRequested` | `request_review`（仅 Draft→InReview） | 版本匹配时：Draft→InReview，或 InReview→ChangesRequested |
| `PlanEvent::Approved` / `Rejected` | `approve` / `reject` | 仅版本匹配时置终评状态；Approved 记 checkpoint |
| `PlanEvent::CommentAdded` | 无现行命令 | 追加行锚点意见 |
| `TaskEvent::Started` | `start` | 建 / 刷新任务为 Running（幂等） |
| `TaskEvent::Suspended` / `Resumed` | 无现行命令 | Running→Suspended / Suspended→Running（前置不符则 `Err`） |
| `TaskEvent::Finished` | `finish` / `cancel` | 收敛 Completed / Failed / Canceled + detail |

## 4. 核心行为与数据流

2026-09-24：Task ID 改为进程 PID + 纳秒命名空间 + 进程级原子序号，各 TaskManager 与 Host 不再复用裸计数 ID；旧事件 ID 继续原样重放，不改快照字段。`Started` 的历史重放边界仍按现行 reducer，未新增容错重放承诺。

### 4.1 Plan 版本演进与审批 gate

1. `create_plan` 产出首版（`plan_1` / `planver_1`，全部步骤 `Pending`，评审 `Draft`）。
2. Host 评审路径：`request_review`（Draft→InReview）后 `approve` 或 `reject`。`replace_plan` 整版替换并回到 Draft。
3. 历史 `PlanEvent`（`StepUpdated` / 第二次 `ReviewRequested` 折成 `ChangesRequested` / `CommentAdded` / `Revised`）没有现行命令，由 `apply` 按上表折叠。`Revised` 回 Draft 并清 checkpoint，comments 保留；`Replaced` 额外清空 comments。
4. **审批 gate**：宿主在执行前调用 `is_approved_for_execution`；只有「当前版本 + Approved」组合放行。Approved 时可关联 `checkpoint_id` 作为批准点，供回滚。重放得到的 `ChangesRequested` 仍可由 `approve` / `reject` 收敛。
5. 崩溃恢复：持久化的 `PlanEvent` 序列 `replay` 逐条 `apply` 即重建状态；`ReviewRequested` 按当前状态确定性推进，重放序列一致即状态一致。

### 4.2 Task 生命周期与取消传播

1. `register` 建 Queued 记录（**持久化前瞬态**：不发事件，重放不可见；取消 Queued 任务静默移除）。
2. `start` 发 `Started`（携带 kind 与 parent），任务自此进入可重放事件流；`apply` 对 `Started` 幂等（已存在则刷新为 Running）。
3. `finish` 从 Running 收敛到 Completed / Failed。历史 `Suspended` / `Resumed` 由 `apply` 折叠；重放后的 Suspended 任务仍可 `finish` 或 `cancel`。
4. `cancel(root)`：`subtree` 沿 parent 链自根向下栈式遍历（深度优先序）收集全部后代 → 逐个按状态处理（见 §3.2）→ 先释放锁再触发全部取消令牌，无孤儿。
5. 断连恢复：调用方持久化事件后用 `snapshot()`（视图 + 日志）或 `event_log()` + `replay(events)` 重建；重放只重建视图与日志，新 id 由进程唯一命名空间分配器生成（见本节开头 2026-09-24 说明），与重放恢复的历史 id 天然不碰撞。

## 5. 契约与不变量

- **步骤事件**：现行命令面不推进步骤。`StepUpdated` 由 `apply` 写入目标状态，未知 `step_id` 忽略，不在折叠时拒绝所谓非法转移。
- **评审状态机**：命令面 `request_review` 只接受 Draft→InReview，`approve` / `reject` 接受 InReview 或历史重放得到的 ChangesRequested；其余命令转移为 `IllegalReviewTransition`。`apply` 对 `ReviewRequested` 另按当前状态把 InReview 推进到 ChangesRequested，对 `Revised` 复位 Draft。
- **任务状态机**：命令面 `Queued → Running → Completed | Failed | Canceled`。`apply` 另折叠历史 `Suspended` / `Resumed`（Running→Suspended、Suspended→Running，前置不符即 `Err`）。终态不可再转移；`Finished` 事件只接受终态 status。`finish` / `cancel` 接受 Running 与重放后的 Suspended。
- **Plan/Review 不授予写权限**：步骤文本是惰性数据，绝不作为命令通道；审批 gate 只做只读判定（`plan_with_write_action_descriptions_is_inert` 守护运行期不执行）。本包不暴露 spawn / exec / 写文件 / 网络 API；依赖方向见 [../../architecture.md](../../architecture.md)，不靠源码文本扫描证明。
- **replay 是纯折叠**：Plan 的 `apply` 不做再校验（非法输入由命令面拒绝）；Task 的 `apply` 保留状态机校验（重放坏事件显式报错而非静默损坏）。
- **事件所有权**：本包只产出事件；封装为 `AgentEvent::Plan` / `AgentEvent::Task` 并落盘由 session-store 侧负责（round-trip 由测试守护）。
- **无平台 / Provider 名分支**；不依赖 exec / orchestration（依赖方向红线，装配职责在 app）。

## 6. 依赖关系

- **生产依赖**：`pawork-domain`；`serde` / `thiserror`。无 tokio。dev-dependency 仅 `serde_json`（重放与 round-trip 测试）。
- **features**：`default = []`，无任何具名 feature（早期 `process-exec` feature 已随执行路径归档，现行不存在）。
- **被依赖**：仅 `pawork-app`（plan host 与 tasks host / services）。`pawork-orchestration` 不依赖本包。

## 7. 测试与验证资产

| 资产 | 覆盖点 |
| --- | --- |
| `tests/plan_service.rs` | Host 命令错误矩阵；`host_commands_replay_matches_manual_apply` 与 `replace_plan_forms_version_chain`；**红线** `plan_with_write_action_descriptions_is_inert`（写动作描述文本不产生任何执行，`StepUpdated` 只改状态）；`historical_plan_events_replay_identically`（`StepUpdated` / `CommentAdded` / 第二次 `ReviewRequested` / `Revised` / `Approved` 的 apply、replay、`from_events` 一致，未知步骤忽略）；`from_events_seeds_counters_from_historical_revision`；重放后的 `ChangesRequested` 仍可 approve/reject 且 gate 在批准前关闭；非法评审转移；`PlanEvent` 全变体经 `AgentEvent` round-trip。2026-09-20 重构删除了两条 `include_str!` 源码文本扫描；「纯 reducer 无 IO/spawn」的约束以 [../../architecture.md](../../architecture.md) 的依赖方向与评审为准 |
| `tests/state_and_replay.rs` | 四类 kind 注册查询；合法 start/finish 事件序；非法转移矩阵；`snapshot_and_replay_rebuild_view`；`pure_state_apply_folds_events`（含 `Suspended` / `Resumed`）；`cancel_propagates_to_descendants_without_orphans`；取消跳过终态并移除 Queued；`replayed_suspend_and_resume_rebuild_and_host_can_finish_or_cancel`（历史挂起事件重放后 Host 仍可 finish / cancel）；`replay_then_register_keeps_ids_unique`；R-05 `shared_cancel_token_stops_real_executor` |

默认验证命令：`cargo test -p pawork-workflow --offline --lib --tests`。

## 8. 注意事项与已知限制

- Cargo description 已收敛为 Plan / Task；现行有预算持续目标由 app Host 承载，未复活 V2 reducer。
- 单 Plan 聚合：一个 `PlanService` 只承载一个 Plan（重复 `create_plan` 返回 `AlreadyExists`），多 Plan 由宿主开多实例。
- `TaskSnapshot.output_seq` / `output_bytes` 在当前纯状态机档恒为 0（无输出缓冲；输出通道由执行 adapter 承载）。
- Plan 的 `apply` 对未知 `step_id` 的 `StepUpdated` 静默忽略（事件是已校验事实，防御性折叠）；Task 的 `apply` 则显式报错——两域容错策略不同，重放时注意区分。
- 一致性事实源是事件日志与 Host 快照文件。本包不提供进程内实时广播。
- 相关文档：[architecture](../../architecture.md) · [design](../../design.md) §2 · [flows](../flows.md) · [Spec 总览](../README.md) · [产品候选](../backlog.md)（goal / automation / monitor 复活条件）；相邻包：[domain.md](domain.md) · [app.md](app.md)。
