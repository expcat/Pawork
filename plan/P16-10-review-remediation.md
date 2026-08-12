# P16-10：Phase 16 评审修复（REVIEW remediation）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：canonical event / reducer / 兼容导入已修复并定向门禁通过；生产宿主接线延后）· 依赖：P16-1 ~ P16-9

**最终目的**：按 [docs/review/p16-review.md](../docs/review/p16-review.md) §3/§4/§7 的改进优先级，先恢复 Phase 16 破坏的正式依赖链编译闭包，再把「canonical event 可完整重放实际可运行状态」从过度声明收敛为事实——为 Goal / Memory / Review 修复重放时丢失的状态（criterion 满足位事件化、embedding / confidence、finding 富字段与 fingerprint），把 Automation / Monitor 的重复计数、注册 / start 分叉与无执行器的假路径收缩为「Automation 只调度、Monitor 判定（保留独立 Running / Stopped）」，并把 P16-9 兼容导入的原子性、派生 ID 全局唯一性、参数保真与稳定 import identity 修到位。同时以一份可复跑的 [scripts/p16-gate.sh](../scripts/p16-gate.sh) 取代 plan 中仅有的 PowerShell 示例，把正式依赖链（`cargo check -p app-service`）与两条 workflow 事件折叠回归纳入门禁。生产宿主装配（host / core-api / EventHub）、真实 driver / executor、旧 Memory 事件向量、Goal `achieve` 校验全部 criteria、Automation / Monitor 完整 runtime replay 与 compat 顶层 unknown_fields 持久化按评审结论显式延后，不为死路径继续扩充 schema 或新增 crate / 抽象。

**涉及范围**：`agent-domain::workflow`（Goal `CriterionSatisfied`、Memory `embedding/confidence`、Review 富字段 + fingerprint 进事件）、`agent-engine::recovery` 与 `app-service::supervisor`（7 类 P16 事件显式折叠）、`goal-service`（criterion 事件化 + 人审入口）、`memory-service`（replay 从事件取 embedding / confidence）、`review-engine`（富字段进 `FindingOpened`）、`automation-service`（删 `external.rs` 与内置 dispatcher、`fired_count` 单源 + 结果归属）、`monitor-service`（重复注册 / start 顺序修复、删零消费者 `driver.rs`、保留纯 `evaluate` 与独立 Running / Stopped lifecycle）、`session-store`（compat 单事务原子导入 + ID session scope + 参数保真 + `compat_import_identity` 去重）、`task-manager`（配合 Automation / Monitor 收缩调整 lifecycle 引用）；新增 [docs/review/p16-review.md](../docs/review/p16-review.md)、[docs/features/workflow.md](../docs/features/workflow.md)、[scripts/p16-gate.sh](../scripts/p16-gate.sh) 与本任务 plan，并修正 [docs/features/sessions.md](../docs/features/sessions.md)。

## 处置策略（按评审 §7 矩阵）

- **现在修复（落地）**
  - §3.1 / §7-P0-1 正式依赖链编译闭包：`agent-engine::recovery::replay_run` 与 `app-service::supervisor::{event_state, translate_payload}` 对 7 个 Phase 16 `AgentEvent` 变体显式折叠为「不改变 Run 状态、不翻译为 `AppEvent`」，使用穷举 `match`（不用通配 `_`）；`cargo check -p app-service` 恢复编译并纳入 [P16 门禁](../scripts/p16-gate.sh)。
  - §3.5 / §7-P0-3 Goal：新增 `GoalEvent::CriterionSatisfied { goal_id, criterion_id }`（无 `satisfied` 字段），单项 criterion 满足位由 `apply` 折叠恢复；`satisfy_criterion` 仅允许 `Auto` 类、`Human` 类拒绝（必须走 `mark_human_satisfied` 显式人审入口）；`achieve` 校验全部 criteria 未实现，显式延期。
  - §3.5 / §7-P0-3 Memory：`MemoryEvent::Recorded` 携带 `embedding: Vec<f32>` 与 `confidence: f32`，`MemoryStore::apply` 从事件取值重建；实时写入路径与 replay 路径字段一致（`apply_replay_matches_direct_validity` 回归）。
  - §3.5 / §7-P1-8 Review：`ReviewEvent::FindingOpened` 携带 `evidence / assignee / suggested_patch / fingerprint`，富字段与 fingerprint 不再事件外补写，replay 后 finding 不退化为 stale。
  - §4.2 / §4.3 / §7-P1-6 Automation：删 `automation-service/src/external.rs`（`ExternalTrigger` 五 variant）与内置 `TaskManagerDispatcher`；`AutomationDispatcher` 收敛为对象安全 trait，crate 不再提供「register/start 即执行」的假 adapter，真实 executor 由调用方注入；只修 `fired_count` 单源（`AutomationState` 为唯一权威，取消命令侧重复计数）与结果归属（`record_result` 校验任务确由该 automation 触发）；完整 runtime replay（`next_at` / `failure_streak` / inbox 等进程内状态）显式延期。
  - §4.2 / §7-P1-6 / §7-P1-9 Monitor：修重复注册（配置锁内查重、task 注册先于配置插入）与 start 顺序（task start 先于 `Started` 广播）；删零消费者 `monitor-service/src/driver.rs`（FileWatchDriver）；保留确定性纯函数 `evaluate` 与独立 Running / Stopped lifecycle（task-manager 仅镜像簿记）；config / task mapping 仅进程内、事件 replay 不可完整恢复（显式延期）。
  - §3.2 / §3.3 / §3.4 / §7-P0-2 Compat Import：`import_compat_inner` 改为单一 SQLite transaction（`TransactionBehavior::Immediate`）一次写入 Session、`compat_import_identity`、branch、全部 event 与 projection，任一错误整批回滚；`run_id` / `message_id` / `tool_call_id` 以目标 `session_id` 为 scope（`scope_tool_id`），消除同来源第二会话与跨来源 tool ID 冲突；外部 tool `arguments` 映射既有 `ToolCallArgumentsDelta`（projection 累积到 `tool_calls.arguments_json`），无 / 空值不发空 delta；`(source, original_id)`（无 `original_id` 时用 content fingerprint）作为稳定 identity 与 content fingerprint 同事务持久化，同 identity 同 fingerprint 幂等、不同 fingerprint 明确冲突；校验明确为批内结构校验（sequence 连续 / parent 无悬空 / tool result 有前置 tool call / Secret 拒绝），文档与注释写明「这不是状态机 replay」。
  - §6 / §7-P2-13 文档与门禁：新增事实型 [workflow.md](../docs/features/workflow.md) 与 [p16-review.md](../docs/review/p16-review.md)，提供可复跑的 [p16-gate.sh](../scripts/p16-gate.sh)（独立 `target/gates`、trap 清理、四类门禁、覆盖正式链），取代 plan 中仅有的 PowerShell 示例与「集中门禁已全绿」的不可复核声明。

- **状态 / 事实纠正（评审 §0 / §6 过度声明）**
  - Phase 16 不再以「9/9 最小闭环」自述：功能文档与评审把当前状态写为「canonical 领域 / 纯算法 / 兼容导入已修复并定向门禁通过，生产宿主接线延后」的有界事实；`validate_batch` 的「replay 校验」声明收敛为「批内结构校验」，名实相符。
  - 评审 §3.4「`validate_batch` 不是状态机 replay」、§3.5「Goal / Memory / Review 重放后字段丢失」、§3.2/§3.3「compat 非事务 + ID 冲突」均为本次修复的直接对象，修复后以定向回归固化。

- **保留（评审建议但判定不采纳）**
  - `automation-service` / `monitor-service` 保留独立 crate（不并入 task-manager）：调度 / 判定 / 执行所有权分离符合 ADR-024 职责边界，且 `monitor-service` 已规划作为 P17-2 Plugin Package Monitor 的 contract / evaluator 入口。
  - compat 导入期不反向依赖 `diff-service` / `review-engine` 生成锚点：`session-store` 是底层存储 crate，反向依赖破坏分层；导入期原样保留外部 diff / comment raw data，待 Review consumer 出现后由上层复用 Review core 锚定。

- **显式延后（生产宿主接线，不在本任务）**
  - §2.1 / §7-P0-4 / §7-P1-5 生产宿主装配：`core-api` 的 `AppCommand` / `AppQuery` / `AppEvent` 无 Plan / Goal / Task / Automation / Monitor / Memory / Review / Compat 入口；`app-service` / `core-runtime` / 正式宿主不依赖 7 个 Phase 16 service crate；P16 事件不进入 `session-store` 持久化，也不经 EventHub 发布到 CLI / GUI。「宿主重启后恢复 P16 状态」尚未成立，留待最小纵向闭环（Plan create/review/approve → Agent Loop gate → SessionStore / EventHub → core-api）接入。
  - §2.3 / §7-P1-6 真实 executor / driver：Agent / Monitor / Automation kind 的 TaskManager executor、Automation 的 timer / event-loop 调用者、Monitor 的 ProcessExit / RegexMatch / PortState 真实 driver 与 PersistentProcess attach/detach/reconnect 均未实现，按调度 / 判定 / 执行分离的设计延后。
  - §3.5 旧 Memory 事件向量：新流 `Recorded` 已携带 embedding / confidence 并可完整 replay；但历史已落库的旧 Memory 事件缺向量字段，serde default 为空、检索时被过滤，**不可恢复**，须在生产 EmbeddingProvider + 持久化 + context consumer 接入时重新嵌入（不在本任务）。

## 细分步骤（分组）

### A. 正式事件链编译闭包（评审 §3.1）

1. **recovery 折叠**：在 `agent-engine::recovery::replay_run` 对 `AgentEvent` 的穷举 `match` 显式处理 `Plan` / `Goal` / `Task` / `Automation` / `Monitor` / `Memory` / `Review` 七个变体，返回「不改变 Run 状态」的空转换。目的：消除 `E0004`，恢复 `app-service` 编译闭包。
2. **supervisor 折叠**：在 `app-service::supervisor::{event_state, translate_payload}` 同样显式折叠为 `None`（不改 `RunState`、不翻译为 `AppEvent`）。目的：补齐 recovery 之下的第二层穷举，避免新增 canonical event 时静默漏处理。
3. **回归固化**：`agent-engine::recovery` 与 `app-service::supervisor` 各加 `workflow_events` 回归，断言七类事件折叠后不产生 Run 状态变更 / AppEvent。目的：把「显式折叠」锁进测试。

### B. Goal / Memory / Review 完整新事件回放（评审 §3.5）

4. **Goal criterion 事件化**：`agent-domain::workflow` 新增 `GoalEvent::CriterionSatisfied { goal_id, criterion_id }`（无 `satisfied` 字段）；`goal-service::state::apply` 据此折叠恢复单项 criterion 满足位；`satisfy_criterion` 仅允许 `Auto` 类、拒绝 `Human` 类（强制 `mark_human_satisfied` 人审入口）；`achieve` 校验全部 criteria 未实现（显式延期）。目的：消除「progress=1 但全部 criteria=false」的矛盾。
5. **Memory 向量进事件**：`MemoryEvent::Recorded` 增 `embedding` / `confidence` 字段；`MemoryStore::apply` 从事件取值，实时路径与 replay 路径字段一致；旧流缺字段 serde default 空 / 0.0（检索过滤，需重新嵌入）。目的：新流记忆可完整重放与检索。
6. **Review 富字段进事件**：`ReviewEvent::FindingOpened` 增 `evidence / assignee / suggested_patch / fingerprint`；replay 后 finding 富字段与 fingerprint 不丢失、不退化为 stale。目的：人审事实与去重指纹可重放。

### C. Automation / Monitor 收缩（评审 §4.2 / §4.3）

7. **Automation 调度 / 执行分离**：删 `external.rs`（`ExternalTrigger` 五 variant）；`AutomationDispatcher` 收敛为对象安全 trait，crate 不提供内置 TaskManager adapter，避免无执行器创建 / 终结幽灵任务；`fired_count` 以 `AutomationState` 为唯一权威（取消命令侧重复计数），`record_result` 校验结果归属（任务须确由该 automation 触发），`fire` / `dispatch_due` 经注入 dispatcher 派发。目的：消除「register/start 即执行」的假路径与计数 / 结果归属分叉；完整 runtime replay（`next_at` / `failure_streak` / inbox）显式延期。
8. **Monitor 判定 / 执行分离**：修重复注册（配置锁内查重 + task 注册先于配置插入）与 start 顺序（task start 先于 `Started` 广播，避免「先广播再失败」分叉）；删零消费者 `driver.rs`（FileWatchDriver）；保留确定性纯函数 `evaluate` 与输出节流；Observation 由宿主 / 未来 driver 注入；独立 Running / Stopped lifecycle 与 `Started` / `Stopped` 事件保留，task-manager 仅作镜像簿记。目的：消除重复注册竞态与 start 分叉；config / task mapping 仅进程内、事件 replay 不可完整恢复，完整 runtime replay 显式延期。

### D. Compat 导入原子性 / 去重 / ID / 参数保真（评审 §3.2 / §3.3 / §3.4）

9. **单事务原子导入**：`import_compat_inner` 用 `TransactionBehavior::Immediate` 开启单一 transaction，依次写 Session、`compat_import_identity`、branch、全部 event（经 `persist_event_in_transaction`）与 projection，任一步失败由同一 transaction 整批回滚。目的：消除空 Session / 半截事件 / 半导入被误判 deduplicated。
10. **派生 ID session scope**：`run_id = compat-run-{session_id}`、trigger `message_id = compat-trigger-{session}`、`tool_call_id` 经 `scope_tool_id(session, external)` 派生，result 与 call 共享同 scope 以保证配对。目的：消除同来源第二会话 RunStarted projection 冲突与跨来源 message / tool ID 撞键。
11. **参数保真**：外部 tool `arguments` 映射既有 `ToolCallArgumentsDelta`（projection 累积到 `tool_calls.arguments_json`），无 / 空值不发空 delta。目的：修复评审 §P16-9「tool arguments 丢弃」。
12. **稳定 identity 与去重**：新增 `compat_import_identity(source, original_id, content_fingerprint, session_id)` 表与索引；`(source, original_id)` 为稳定 identity（无 `original_id` 用 content fingerprint），与 content fingerprint 同事务持久化：同 identity 同 fingerprint 幂等、不同 fingerprint 明确冲突。目的：取代「content 进 Session ID」导致内容变化即绕过去重的脆弱去重。
13. **回归固化**：连续导入两个不同外部会话、跨来源相同 tool ID、故意中途失败后零残留三类回归。目的：覆盖评审 §3.3 指出测试只导入一次的盲区。

### E. 文档与可复跑门禁（评审 §6 / §7-P2-13）

14. **事实型功能文档**：新增 [workflow.md](../docs/features/workflow.md)，只描述当前真实接线边界（已实现 / 明确延期），不把未接线能力写成闭环；修正 [sessions.md](../docs/features/sessions.md) Compat Import 段落。目的：兑现模块文档约定。
15. **评审文档**：新增 [p16-review.md](../docs/review/p16-review.md)，记录发现、证据与改进优先级。目的：状态事实可追溯。
16. **可复跑门禁**：新增 [p16-gate.sh](../scripts/p16-gate.sh)（独立 `target/gates`、trap 清理、四类门禁：crates-test / crates-clippy / official-chain / schema-check），把 `cargo check -p app-service` 与两条 `workflow_events` 回归纳入 official-chain 类。目的：取代 plan 中不可复跑的 PowerShell 示例。

## 主要产出物

- `agent-domain::workflow`：`GoalEvent::CriterionSatisfied { goal_id, criterion_id }`、`MemoryEvent::Recorded` 增 `embedding/confidence`、`ReviewEvent::FindingOpened` 增富字段 + fingerprint。
- `agent-engine::recovery` + `app-service::supervisor`：7 类 P16 事件显式折叠 + `workflow_events` 回归。
- `automation-service`：删 `external.rs`、dispatcher 收敛为 trait、调度 / 执行分离。
- `monitor-service`：重复注册 / start 顺序修复、删零消费者 `driver.rs`（FileWatchDriver）、保留纯 `evaluate` 与独立 Running / Stopped lifecycle。
- `session-store`：compat 单事务原子导入 + ID session scope + 参数保真 + `compat_import_identity` 去重 + 三类回归。
- [scripts/p16-gate.sh](../scripts/p16-gate.sh) + [docs/features/workflow.md](../docs/features/workflow.md) + [docs/review/p16-review.md](../docs/review/p16-review.md) + 本 plan + sessions.md 修正。

## 验收标准（保留 REVIEW 追踪编号）

- [x] **§3.1 / P0-1 正式链**：`cargo check -p app-service` 通过；recovery 与 supervisor 对 7 类 P16 事件显式折叠（穷举 match，无通配 `_`），`workflow_events` 回归通过
- [x] **§3.5 Goal**：`CriterionSatisfied { goal_id, criterion_id }` 进事件，replay 后单项 criterion 满足位可恢复；`satisfy_criterion` 仅允许 Auto、Human 强制 `mark_human_satisfied` 人审
- [x] **§3.5 Memory（新流）**：`Recorded` 携带 embedding / confidence，replay 与实时路径字段一致
- [x] **§3.5 Review**：`FindingOpened` 携带 evidence / assignee / suggested_patch / fingerprint，replay 后 finding 不退化为 stale
- [x] **§4.2 / §4.3 Automation**：`external.rs` 删除、dispatcher 收敛为 trait、无内置「register/start 即执行」假路径、`fired_count` 单源 + 结果归属
- [x] **§4.2 Monitor**：重复注册 / start 顺序修复、`driver.rs`（FileWatchDriver）删除、保留纯 `evaluate` 与独立 Running / Stopped lifecycle
- [x] **§3.2 Compat 原子性**：单 transaction 写 Session + identity + event + projection，中途失败整批回滚、零残留
- [x] **§3.3 Compat ID**：run / message / tool ID 以目标 session 为 scope；连续两会话、跨来源重复 tool ID 回归通过
- [x] **§3.4 / §P16-9 参数保真**：外部 tool arguments 映射 `ToolCallArgumentsDelta` 并持久化
- [x] **§3.3 Compat 去重**：`(source, original_id)` + content fingerprint 同事务持久化，幂等 / 冲突语义正确
- [x] **§6 / §P2-13 门禁**：[p16-gate.sh](../scripts/p16-gate.sh) 可复跑，四类门禁覆盖 crates-test / clippy / 正式链 / schema，独立 `target/gates` 且 trap 清理
- [ ] **§2.1 生产宿主装配**（未达成，显式延后）：core-api / app-service / EventHub / 持久化接线
- [ ] **§2.3 真实 executor / driver**（未达成，显式延后）：Agent/Monitor/Automation executor、timer/event-loop、真实 driver、PersistentProcess
- [ ] **§3.5 旧 Memory 事件向量**（不可恢复，显式延后）：历史旧流缺向量，须生产 EmbeddingProvider 接入后重新嵌入
- [ ] **§3.5 Goal `achieve` 校验全部 criteria**（未达成，显式延后）：`achieve` 仅校验 Active 状态，不校验 criteria
- [ ] **§4.2 Monitor 完整 runtime replay**（未达成，显式延后）：config / task mapping 仅进程内，事件 replay 不可恢复
- [ ] **§4.3 Automation 完整 runtime replay**（未达成，显式延后）：`next_at` / `failure_streak` / inbox 等进程内状态
- [ ] **§3.2 / §3.3 compat 顶层 unknown_fields 持久化**（未达成，显式延后）：顶层 unknown_fields 仅进 `CompatImportReport`，未持久化进事件
- [x] **快速验证**：只运行本任务涉及 crate 的定向门禁与必要 `cargo check -p app-service`；Phase 16 remediation 收尾后不重复 workspace 全量门禁

## 验证记录（2026-08-12）

- **最终独立复核（deepseek_reviewer）：VERDICT: PASS**——独立复跑 `scripts/p16-gate.sh` 全类别 PASS（crates-test 225 + official-chain 2 = **227 tests / 0 failed**；11 crate clippy `--all-targets -D warnings` 0 warning；`cargo check -p app-service` PASS；schema-typegen `--check` PASS；改动 Rust 文件 rustfmt `--check` 与 `git diff --check` PASS）。唯一低严重度 `docs/features/workflow.md` 注解已校正，**无代码 finding**。
- `scripts/p16-gate.sh`：四类门禁全部 PASS——
  - **crates-test**：`agent-domain / agent-events / provider-api / plan-service / goal-service / task-manager / automation-service / monitor-service / memory-service / review-engine / session-store`，**225 tests passed / 0 failed**（覆盖 Goal criterion 事件回放、Memory embedding/confidence replay 一致、Review 富字段 + fingerprint 回放、compat 单事务原子导入 / 连续两会话 / 跨来源重复 tool ID / 中途失败零残留 / identity 幂等与冲突）；
  - **crates-clippy**：上述 11 crate `--all-targets -- -D warnings`，**0 warning**；
  - **official-chain**：`cargo check -p app-service`（正式依赖链编译闭包恢复）+ 两条 `workflow_events` 回归（`agent-engine::recovery::workflow_events` + `app-service::supervisor::workflow_events`，7 类 P16 事件折叠），全部 PASS——合计 227 = crates-test 225 + official-chain 2；
  - **schema-check**：`cargo run -p schema-typegen -- --check`，PASS（只比对已提交声明，门禁内不改写 `schemas/`）。
- 定向 rustfmt check：本任务改动 Rust 文件 PASS；`git diff --check`：PASS。
- 独立隔离 `target/gates`，`trap` 清理生效，门禁结束后不残留（`P16_GATE_KEEP_TARGET` 未设置）；全程不跑 workspace 全量。
- 状态事实：`core-api` / `app-service` / `core-runtime` / 正式宿主对 7 个 Phase 16 service crate 零依赖、零命令 / 查询 / 事件入口（已核对源码与依赖图）；P16 事件未接 `session-store` 持久化与 EventHub 发布——生产宿主接线、真实 executor / driver、旧 Memory 事件向量按上述显式延后。
- 文档同步：`docs/features/workflow.md` 已随本任务新增并同步（P16 交付物，重放状态表述与修复后代码一致）；ROADMAP/plan 已同步——ROADMAP Phase 16 行 10/10、总计 **219/175**，plan checkbox 与「有界完成」表述一致，plan/README [Phase 16 延期落点登记](README.md) 六项映射（monitor 包驱动 → P17-2/P17-3、Plan/Goal host → P19-12、workflow core-api/EventHub → P17-6、Memory → P17-5/P19-2、Review → P19-8、compat 命令入口/历史查询 → P17-8/P19-2）。

```text
Validation Level: L2（P16 定向功能簇门禁）
Affected crates: agent-domain、agent-events、provider-api、plan-service、goal-service、task-manager、automation-service、monitor-service、memory-service、review-engine、session-store、agent-engine、app-service（changed + 正式依赖链 + 关键直接消费者）
Validated: scripts/p16-gate.sh 四类全 PASS（crates-test 225 passed / 0 failed，official-chain 两条 workflow_events，合计 227；crates-clippy 0 warning；official-chain app-service check + 两条 workflow_events 回归；schema-typegen --check）/ 定向 rustfmt + git diff --check
Targeted regressions: 正式链 7 事件折叠、Goal CriterionSatisfied 回放、Memory embedding/confidence replay 一致、Review 富字段 + fingerprint 回放、Automation fired_count 单源 / 结果归属、Monitor 重复注册 / start 顺序、compat 单事务原子导入 / 连续两会话 / 跨来源重复 tool ID / 中途失败零残留 / identity 幂等与冲突 / 参数保真
Full workspace gate: NOT RUN（P16 定向功能簇门禁已充分覆盖；未命中升级条件）
```

**相关文档**：[docs/review/p16-review.md](../docs/review/p16-review.md) · [docs/features/workflow.md](../docs/features/workflow.md) · [docs/features/sessions.md](../docs/features/sessions.md) · [scripts/p16-gate.sh](../scripts/p16-gate.sh) · [ADR-016 事件可重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [ADR-024 Event Hub](../docs/adr/ADR-024-shared-app-service-event-hub.md) · [ADR-025 CLI 唯一宿主](../docs/adr/ADR-025-cli-is-sole-host.md) · [ADR-030 Core 唯一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ROADMAP Phase 16](../ROADMAP.md)

> 延期决策（2026-08-12）：本任务只修复「canonical event 可完整重放 + 兼容导入正确性 + 正式链编译闭包 + 收缩假路径」；生产宿主装配（host / core-api / EventHub / 持久化）、真实 executor / driver、PersistentProcess、旧 Memory 事件向量、Goal `achieve` 校验全部 criteria、Automation / Monitor 完整 runtime replay（config / task mapping、`next_at` / `failure_streak` / inbox 等进程内状态）与 compat 顶层 unknown_fields 持久化不在本任务——生产宿主装配需要最小纵向闭环设计，旧 Memory 事件因历史事件缺向量物理不可恢复，其余为未实现的校验或进程内状态，须待生产接入后补齐。不为这些延后项在本任务扩充 schema 或新增 crate / 抽象。延后落点按 [plan/README Phase 16 延期落点登记](README.md) 六项映射：monitor 包驱动 → P17-2/P17-3、Plan/Goal host → P19-12、workflow core-api/EventHub → P17-6、Memory → P17-5/P19-2、Review → P19-8、compat 命令入口/历史查询 → P17-8/P19-2；ROADMAP/plan 已同步（Phase 16 10/10、总计 219/175）。
