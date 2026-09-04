# pawork-workflow

> Plan / Task 纯 reducer：在内存中折叠 canonical `PlanEvent` / `TaskEvent` 并提供命令面校验，事件返回给调用方持久化。依赖仅 `pawork-domain`；装配在 `pawork-app`。

## 1. 职责与边界

- **做什么**：Plan 聚合（步骤状态机、版本替换与修订链、评审状态机、行锚点评审意见、审批 gate）与 Background Task 聚合（process / agent / monitor / automation 四类任务的注册、状态迁移、事件日志、取消传播、断连恢复）。命令面先校验合法性再 `apply` 折叠，返回 canonical 事件供调用方经 `AgentEvent::Plan` / `AgentEvent::Task` 持久化；`apply` / `replay` 为崩溃恢复唯一入口。
- **不做什么**：不执行进程、不持有 SandboxBackend、不清理进程树、不落库、不依赖 `pawork-exec` / `pawork-orchestration`。Plan 是只读建议——`PlanService` 不暴露任何 spawn / exec / write / 文件 / 网络 API，审批仅作为执行 gate 放行，不扩权。
- **事件类型归属**：`PlanEvent` / `TaskEvent` 及各 Id / 状态枚举定义在 `pawork-domain`，本包只消费不重定义。Goal / Automation / Monitor 三域 reducer 已随 V2 归档（tag `v2-final`），domain 事件类型保留以便重放；`TaskKind::{Process, Agent, Monitor, Automation}` 是 domain 的任务种类枚举，与已归档 reducer 无关。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~10 | 仅 `pub mod plan` / `pub mod task`，无 glob re-export |
| `src/plan/mod.rs` | ~35 | plan 门面与 re-export；模块级文档（只读约束、event-sourcing 设计） |
| `src/plan/state.rs` | ~235 | `PlanState` 聚合（title / steps / 当前与父版本 / 修订链 history / 评审状态 / comments / 审批 checkpoint）；`apply` 纯折叠（事件视为已校验事实，防御性忽略未知步骤）；`replay`；`is_legal_step_transition`；`PlanComment` |
| `src/plan/service.rs` | ~485 | `PlanService` 命令面 + 查询面（`Mutex<Inner>`，毒锁 into_inner 继续）；id 生成（`plan_N` / `planver_N` / `step_N`）；`from_events` 重放重建并 seed 计数器防 id 碰撞 |
| `src/plan/snapshot.rs` | ~30 | 查询面 DTO：`PlanSnapshot`、`PlanVersionInfo`（serde 可序列化，供 CLI/GUI 经 GUI Connection Protocol 消费） |
| `src/plan/error.rs` | ~60 | `PlanError`（非法步骤 / 评审转移、版本与 plan_id 不匹配、空 plan / 空步骤文本 / 空理由 / 空评论、重复版本等 13 变体） |
| `src/task/mod.rs` | ~40 | task 门面与 re-export；模块级文档（统一抽象、断连续存、取消传播、执行所有权边界） |
| `src/task/state.rs` | ~315 | `TaskManagerState` 纯聚合（任务表 `BTreeMap` + 只追加事件日志）；`apply` 唯一折叠入口（`Started` 幂等，`Finished` 校验前置状态与终态合法性）；`TaskSnapshot` / `TaskManagerSnapshot`；`is_active_status` / `is_terminal_status`；`subtree` 后代收集；id 分配与重放推进 |
| `src/task/manager.rs` | ~265 | `TaskManager` 命令面 / 查询面 / `broadcast` 实时事件（容量默认 256，Lagged 后走 snapshot + events_since 恢复） |
| `src/task/error.rs` | ~30 | `TaskManagerError`（UnknownTask / UnknownParent / InvalidTransition / InvalidFinishedStatus） |
| `tests/plan_service.rs` | ~785 | Plan 全流程集成测试（见 §7） |
| `tests/state_and_replay.rs` | ~340 | Task 状态机与重放集成测试（见 §7） |

## 3. 对外 API 面

### 3.1 `plan` 模块

- **命令面 `PlanService`**（每个方法校验后 `apply` 并返回对应 `PlanEvent`，错误为 `PlanError`）：
  - `create_plan(title, step_texts) -> Created`：拒绝空 plan / 空白步骤文本 / 已存在 plan（单 Plan 聚合）。
  - `replace_plan(title, step_texts) -> Replaced`：整版替换，新版本 `parent_version` 指向旧版本；评审状态复位 `Draft`、comments 清空、审批 checkpoint 清空。
  - `update_step(step_id, new_status, note) -> StepUpdated`：须经合法步骤状态机（见 §5）。
  - `request_review(version) -> ReviewRequested`：`Draft → InReview`；`request_changes(version) -> ReviewRequested`：`InReview → ChangesRequested`（同一事件变体按当前状态折叠推进「评审回合」）。
  - `revise(version, parent_version, title, steps) -> Revised`：仅 `ChangesRequested` 可修订；校验 parent 等于当前版本、新版本异于 parent 且不与 history 重复；折叠后回到 `Draft`。
  - `approve(plan_id, version, checkpoint_id) -> Approved`：`InReview | ChangesRequested → Approved`，可携带批准点 checkpoint（可回滚）；`reject(plan_id, version, reason) -> Rejected`：同源状态 → `Rejected`，reason 必填非空。
  - `add_comment(plan_id, version, anchor, body) -> CommentAdded`：行锚点（`anchor.step_id` 必须是当前版本既有步骤）+ 非空正文。
  - `from_events(events)`：重放重建 service，并从既有 id 后缀 seed 计数器，保证重放后新发 id 不与历史碰撞。
- **查询面**：`plan_snapshot() -> Option<PlanSnapshot>`（未创建为 None）；`version_history() -> Vec<PlanVersionInfo>`（修订链按创建顺序）；**审批 gate** `is_approved_for_execution(plan_id, version) -> bool`——仅当 plan_id 与当前版本都匹配且评审状态为 `Approved` 才返回 true，其余（未创建 / 不匹配 / 任何未批准状态）一律 false；只读判定，不授予任何写 / 执行能力。
- **纯函数**：`apply(&mut PlanState, &PlanEvent)`（不重复校验，命令面已把关）；`replay(events) -> PlanState`；`is_legal_step_transition(from, to)`。

### 3.2 `task` 模块

- **命令面 `TaskManager`**（`Clone` 共享同一内部状态）：`register(task_kind, parent_task_id) -> BackgroundTaskId`（Queued，不发事件，parent 必须存在）；`start` (Queued→Running，`Started`)；`suspend` / `resume`（Running↔Suspended，逻辑挂起，OS 级暂停由 adapter 落地）；`finish(task_id, status, detail)`（Running|Suspended → Completed|Failed，`Canceled` 必须走 `cancel`，否则 `InvalidFinishedStatus`）；`cancel(task_id) -> Vec<TaskEvent>`（沿 `parent_task_id` 链传播到全部后代：Running/Suspended 发 `Finished{Canceled}` 并触发 domain `CancellationToken`，Queued 静默移除，终态跳过）。
- **查询与恢复**：`task` / `tasks` / `snapshot`（任务视图 + 完整事件日志）/ `event_log` / `events_since(seq)`（日志下标切片续读增量）/ `replay(events) -> usize`（重建视图，不重复广播）；`subscribe() -> broadcast::Receiver<AgentEvent>` 实时流（收到 `Lagged` 后先 `snapshot()` 再 `events_since` 续读）。
- **纯状态**：`TaskManagerState::apply(&TaskEvent)`（可失败：未知任务 / 非法转移即 `Err`，与 Plan 的不可失败 apply 不同）；`is_active_status`（Queued/Running/Suspended）/ `is_terminal_status`（Completed/Failed/Canceled）。

### 3.3 事件与命令对照（事件定义在 `pawork-domain`）

| 事件 | 产生命令 | 折叠效果 |
| --- | --- | --- |
| `PlanEvent::Created` | `create_plan` | 建聚合、首版入 history、评审 Draft |
| `PlanEvent::Replaced` / `Revised` | `replace_plan` / `revise` | 换当前版本、history 追加、评审复位 Draft、清 comments 与 checkpoint |
| `PlanEvent::StepUpdated` | `update_step` | 改单步状态（未知步骤防御性忽略） |
| `PlanEvent::ReviewRequested` | `request_review` / `request_changes` | Draft→InReview 或 InReview→ChangesRequested（按当前状态推进） |
| `PlanEvent::Approved` / `Rejected` | `approve` / `reject` | 仅版本匹配时置终评状态；Approved 记 checkpoint |
| `PlanEvent::CommentAdded` | `add_comment` | 追加行锚点意见 |
| `TaskEvent::Started` | `start` | 建 / 刷新任务为 Running（幂等） |
| `TaskEvent::Suspended` / `Resumed` | `suspend` / `resume` | Running ↔ Suspended |
| `TaskEvent::Finished` | `finish` / `cancel` | 收敛 Completed / Failed / Canceled + detail |

## 4. 核心行为与数据流

### 4.1 Plan 版本演进与审批 gate

1. `create_plan` 产出首版（`plan_1` / `planver_1`，全部步骤 `Pending`，评审 `Draft`）。
2. 步骤推进：`update_step` 沿 `Pending → InProgress → Completed | Blocked`，`Blocked → InProgress`；非法转移（自环、终态跳出、回退 Pending）被 `IllegalStepTransition` 拒绝。
3. 评审回合：`request_review`（Draft→InReview）→ 评审方 `add_comment`（行锚点）→ `request_changes`（InReview→ChangesRequested）→ `revise` 产出新版本（回 Draft，parent 指向被修订版本）→ 再次 `request_review`……直至 `approve` 或 `reject`。
4. 每次 `Replaced` / `Revised` 都把评审状态复位 Draft、清空当前版本 comments 与审批 checkpoint；history 追加修订链节点（`Revised` 重放时按 version 去重）。
5. **审批 gate**：宿主在执行 Plan 步骤前调用 `is_approved_for_execution`；只有「当前版本 + Approved」组合放行。Approved 时可关联 `checkpoint_id` 作为批准点，供回滚。
6. 崩溃恢复：持久化的 `PlanEvent` 序列 `replay` 逐条 `apply` 即重建状态；`ReviewRequested` 按当前状态确定性推进，重放序列一致即状态一致。

### 4.2 Task 生命周期与取消传播

1. `register` 建 Queued 记录（**持久化前瞬态**：不发事件，重放不可见；取消 Queued 任务静默移除）。
2. `start` 发 `Started`（携带 kind 与 parent），任务自此进入可重放事件流；`apply` 对 `Started` 幂等（已存在则刷新为 Running）。
3. Running ↔ Suspended（`Suspended` / `Resumed` 事件）；`finish` 收敛到 Completed / Failed。
4. `cancel(root)`：`subtree` 沿 parent 链 BFS 收集全部后代 → 逐个按状态处理（见 §3.2）→ 先在锁外触发全部取消令牌、再广播事件，无孤儿。
5. 断连恢复：调用方持久化事件后可用 `snapshot()`（视图 + 日志）或 `replay(events)` 重建，`events_since(seq)` 续读增量；重放同时推进 id 分配器（`task_N` 后缀取 max+1），避免恢复后新 id 碰撞。

## 5. 契约与不变量

- **步骤状态机（冻结）**：合法转移仅 `Pending→InProgress`、`InProgress→Completed|Blocked`、`Blocked→InProgress`。
- **评审状态机（冻结）**：`Draft → InReview → ChangesRequested → (Revised 回 Draft)`；`InReview | ChangesRequested → Approved | Rejected`；其余转移一律 `IllegalReviewTransition`。
- **任务状态机（冻结）**：`Queued → Running ↔ Suspended → Completed | Failed | Canceled`；终态不可再转移；`Finished` 事件只接受终态 status。
- **Plan/Review 不授予写权限**：步骤文本是惰性数据，绝不作为命令通道；审批 gate 只做只读判定（`plan_with_write_action_descriptions_is_inert`、`source_has_no_io_or_spawn_api` 测试守护）。
- **replay 是纯折叠**：Plan 的 `apply` 不做再校验（非法输入由命令面拒绝）；Task 的 `apply` 保留状态机校验（重放坏事件显式报错而非静默损坏）。
- **事件所有权**：本包只产出事件；封装为 `AgentEvent::Plan` / `AgentEvent::Task` 并落盘由 session-store 侧负责（round-trip 由测试守护）。
- **无平台 / Provider 名分支**；不依赖 exec / orchestration（依赖方向红线，装配职责在 app）。

## 6. 依赖关系

- **生产依赖**：`pawork-domain`；`serde` / `thiserror` / `tokio`（rt, sync；实际仅用 `broadcast`）。
- **features**：`default = []`，无任何具名 feature（早期 `process-exec` feature 已随执行路径归档，现行不存在）。
- **被依赖**：仅 `pawork-app`（plan host 与 tasks host / services）。`pawork-orchestration` 不依赖本包。

## 7. 测试与验证资产

| 资产 | 覆盖点 |
| --- | --- |
| `tests/plan_service.rs` | 步骤合法 / 非法转移；`replay_matches_live_service_and_manual_apply`（重放与实况一致）；版本修订链成链；命令错误矩阵；**红线**：`plan_with_write_action_descriptions_is_inert`（写动作描述文本不产生任何执行）、`source_has_no_io_or_spawn_api`（扫源码断言无 IO/spawn API）、`review_surface_adds_no_write_or_exec_api`；`PlanEvent` 经 `AgentEvent` round-trip；评审全流程（review→comment→changes→revise→approve 带 checkpoint）；`approval_gate_closed_until_approved`（gate 未批准恒关）；非法评审转移矩阵；直接 approve/reject；行锚点评论；revise 版本链校验与重复版本拒绝；评审流重放一致性 |
| `tests/state_and_replay.rs` | 四类 kind 注册查询；合法生命周期事件序；非法转移矩阵；`snapshot_and_replay_rebuild_view`；`pure_state_apply_folds_events`；`cancel_propagates_to_descendants_without_orphans`（取消树无孤儿）；取消跳过终态并移除 Queued；`events_since` 增量；`replay_advances_id_allocator` |

默认验证命令：`cargo test -p pawork-workflow --offline --lib --tests`。

## 8. 注意事项与已知限制

- Cargo.toml 的 package description 仍写「plan/goal/task/automation/monitor 五合一 reducer」，为 V2 归档前的过期描述，以源码树（仅 plan / task）为准。
- 单 Plan 聚合：一个 `PlanService` 只承载一个 Plan（重复 `create_plan` 返回 `AlreadyExists`），多 Plan 由宿主开多实例。
- `TaskSnapshot.output_seq` / `output_bytes` 在当前纯状态机档恒为 0（无输出缓冲；输出通道由执行 adapter 承载）。
- Plan 的 `apply` 对未知 `step_id` 的 `StepUpdated` 静默忽略（事件是已校验事实，防御性折叠）；Task 的 `apply` 则显式报错——两域容错策略不同，重放时注意区分。
- `TaskManager` 的广播为 best-effort（无订阅者时 send 失败被忽略）；一致性事实源是事件日志而非广播流。
- 相关文档：[architecture](../../architecture.md) · [design](../../design.md) §2 · [flows](../flows.md) · [Spec 总览](../README.md) · [产品候选](../backlog.md)（goal / automation / monitor 复活条件）；相邻包：[domain.md](domain.md) · [app.md](app.md)。
