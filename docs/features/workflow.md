# Workflow 系统（Modern Agent Workflow）

## 职责

Phase 16 引入的 Modern Agent Workflow 领域：Plan / Goal / Background Task / Automation / Monitor / Long-term Memory / Review 七组能力。它们以 canonical event 表达状态演进，目标是为 Agent Loop、CLI 与 GUI 提供可审阅意图（Plan）、可持久目标（Goal）、统一后台执行（Task）、调度（Automation）、监视（Monitor）、跨会话记忆（Memory）与代码评审（Review）的领域基础。

本文档只描述**当前真实接线边界**：已实现的领域类型与宿主折叠行为、以及明确延期（未接入生产宿主）的接线，不把未接线的能力写成闭环。

## 设计要点

### Canonical event 与重放边界

- 领域类型全部落在 `agent-domain::workflow`（`Plan`/`Goal`/`Task`/`Automation`/`Monitor`/`Memory`/`Review` 各域的状态枚举、事件枚举与快照结构），零外部 IO 依赖；`agent-events::AgentEvent` 新增 7 个 wrapping 变体（`Plan`/`Goal`/`Task`/`Automation`/`Monitor`/`Memory`/`Review`）统一承载。
- 宿主恢复链对 7 类 P16 事件**显式折叠**：`agent-engine::recovery::replay_run` 与 `app-service::supervisor`（`event_state` / `translate_payload`）把它们折叠为「不改变 Run 状态、不产生 AppEvent」的审计保留事件，并使用穷举 `match`（不用通配 `_`）——新增 canonical event 会破坏编译，强制显式处理。折叠行为有定向回归测试（`workflow_events_*`）。
- **重放边界**：P16 事件目前只有 crate 内 reducer 级重放（各 service 的事件折叠 / `apply` 入口，由各自单测覆盖）；不存在生产持久化与发布路径，因此「宿主重启后恢复 P16 状态」尚未成立（见下）。

### Host wiring 边界（当前事实）

- `core-api` 的 `AppCommand` / `AppQuery` / `AppEvent` 无 Plan / Goal / Task / Automation / Monitor / Memory / Review / Compat 入口；`app-service`、`core-runtime` 与正式宿主（`apps/pawork`）不依赖 7 个 Phase 16 service crate。
- 各 service 自持进程内状态与 broadcast，未桥接统一 Event Hub（[ADR-024](../adr/ADR-024-shared-app-service-event-hub.md)）；P16 事件不进入 `session-store` 持久化，也不经 EventHub 发布到 CLI/GUI。
- 结论：P16 能力当前只在 crate 层可调用（库 API + 各自测试），**无生产命令 / 查询 / 订阅入口**；正式依赖链（`app-service` 编译闭包）已恢复编译并通过折叠回归，纳入 [P16 门禁](../../scripts/p16-gate.sh)。

### 已明确延期的生产接线

| 能力 | 已实现 | 明确延期 / 未接线 |
| --- | --- | --- |
| Plan（P16-1/2） | Plan 状态机、版本链、Review/Approval、只读 snapshot | 不绑定 Session/Run；approval 不进入 Agent Loop gate；无 core-api 查询/订阅 |
| Goal（P16-3） | durable objective、success criteria、Auto/Human 命令面 | 不关联 Session/Run/Plan；单项 criterion 满足位由 `CriterionSatisfied { goal_id, criterion_id }`（无 `satisfied` 字段）事件折叠恢复并可完整 replay（`satisfy_criterion` 仅允许 Auto、`Human` 强制走 `mark_human_satisfied` 人审）；`achieve` 校验全部 criteria 未实现（延期）；steering 不进入 context |
| Background Task（P16-4） | 统一四 kind 后台任务管理；`start_process` 经注入 Sandbox backend 真实执行、取消沿 parent 链传播 | Agent / Monitor / Automation kind 无 executor；Queued 与输出不进 canonical event/artifact，断进程后不可恢复；内部 broadcast 不接 Event Hub |
| Automation（P16-5） | cron/interval/once/event 确定性计算、inbox、失败退避 | 无 timer/event-loop 调用者（`dispatch_due` 依赖外部驱动）；`AutomationDispatcher` 收敛为对象安全 trait，crate 不提供内置 TaskManager adapter（避免无执行器创建幽灵任务），真实 executor 由调用方注入；`ExternalTrigger` 五 variant 与 `external.rs` 已删除，外部 trigger 接线推迟；`fired_count` 单源（`AutomationState` 为唯一权威）与结果归属（`record_result` 校验任务确由该 automation 触发）已修；`next_at` / `failure_streak` / inbox 等进程内状态的完整 runtime replay 延期 |
| Monitor（P16-6） | 确定性 `evaluate` 纯函数、输出节流、独立 Running / Stopped 状态折叠 | 重复注册（配置锁内查重 + task 注册先于配置插入）与 start 顺序（task start 先于 `Started` 广播）已修；零消费者 FileWatchDriver（`driver.rs`）已删除，观测样本由宿主/未来 driver 注入；独立 Running / Stopped lifecycle 与 `Started` / `Stopped` 事件保留，task-manager 仅镜像簿记；config / task mapping 仅进程内、事件 replay 不可完整恢复（完整 runtime replay 延期）；ProcessExit / RegexMatch / PortState 仅纯函数判定；PersistentProcess attach/detach/reconnect 未实现 |
| Memory（P16-7） | canonical `EmbeddingProvider` 契约（`provider-api`，Provider 无关）、只读提炼、相似度检索 | 无生产 EmbeddingProvider 实现；存储为进程内 `BTreeMap`，新流 `Recorded` 已携带 embedding/confidence 并可完整 replay，**历史旧 Memory 事件缺向量、物理不可恢复**（检索过滤，须重新嵌入）；无 context consumer；当前为实验性 scaffold |
| Review（P16-8） | 行锚点约束、fingerprint re-anchor、resolution reducer、aggregate、PatchValidator | 富字段（evidence/assignee/patch/fingerprint）已进 `FindingOpened` 事件并可完整 replay（不再事件外补写）；无 core-api 查询与 inline UI；Forge 只有 Generic 适配（生成 `export_comments` ≠ 发布 `publish_comment`，后者仅显式调用产生本地合成 ID），无真实 GitHub/GitLab adapter |
| Compat Import（P16-9） | Claude / Codex / Grok / Cursor 四来源只读解析 → canonical event、blake3 指纹去重、批内结构校验、Secret 拒绝，append-only 不破坏既有事件 | 无 core-api / CLI 入口（`ImportPi` 仍走 cli-host placeholder）；`validate_structure` 仅做批内结构校验（sequence 连续 / parent 存在 / 首尾事件 / tool 引用），不是状态机 replay；**原子导入（单 transaction 写 Session + identity + event + projection，失败整批回滚）、派生 ID 以目标 session 为 scope（`scope_tool_id`）、外部 tool arguments 映射 `ToolCallArgumentsDelta`（参数保真）、`compat_import_identity` 稳定去重均已修复**；顶层 `unknown_fields` 仅进 `CompatImportReport`（报告、未持久化进事件），仅逐条无法映射的 Raw 记录进 `Diagnostic` raw metadata，「全部 raw metadata 入事件」不成立（明确延期）；正确性边界见 [P16 评审](../review/p16-review.md) |

## 接口或数据模型

- 领域类型：`agent-domain::workflow` —— `PlanStepStatus` / `PlanReviewStatus` / `PlanEvent`、`GoalStatus` / `CriterionKind` / `GoalEvent`、`TaskKind` / `TaskStatus` / `TaskEvent`、`AutomationTriggerKind` / `AutomationEvent`、`MonitorSourceKind` / `MonitorEvent`、`MemoryPrivacy` / `MemoryEvent`、`ReviewSeverity` / `ReviewResolution` / `ReviewAnchor` / `ReviewEvent`。
- 事件：`agent-events::AgentEvent::{Plan, Goal, Task, Automation, Monitor, Memory, Review}`（7 个 wrapping 变体，schema version 随 `agent-events` 统一演进）。
- Embedding：`provider-api` 的 canonical `EmbeddingProvider` trait + `EmbeddingRequest` / `EmbeddingResponse` / `EmbeddingModelDefinition` / `EmbeddingCapabilities`（Provider 无关，各 `provider-*` 实现）。
- 宿主折叠：`agent-engine::recovery` 与 `app-service::supervisor` 对 7 类事件显式返回「无转换 / 不翻译」。

## 优先级

- **P0（已完成）**：canonical 领域类型与事件变体；宿主对 P16 事件显式折叠 + 编译闭包恢复；P16 定向门禁可复跑（`scripts/p16-gate.sh`，独立 `target/gates`，trap 清理，不跑 workspace 全量）。
- **P1（近期接入）**：最小纵向闭环（Plan create/review/approve → Agent Loop gate → SessionStore / EventHub → core-api 查询/订阅）；TaskManager 作为唯一后台执行生命周期，Automation 只调度、Monitor 只产 Observation；Goal 接入 Plan/context/budget。
- **P2（明确延期）**：Memory 生产化（真实 EmbeddingProvider + 持久化 + context consumer）；Review 真实 Forge adapter 与 UI；Automation 外部 trigger；Monitor 真实 driver；Compat Import 的 CLI/API 入口（原子导入 / 去重 / ID scope / 参数保真已修复，仅剩 CLI/API 入口延期）。

## 验收标准

- [x] `agent-domain::workflow` 保持纯领域（零 GUI / SQLite / HTTP / Git / Provider 依赖）
- [x] 7 类 P16 事件进入 `agent-events`；宿主恢复链显式折叠并有 `workflow_events` 回归测试
- [x] `cargo check -p app-service` 通过（正式依赖链编译闭包纳入 P16 门禁）
- [x] P16 门禁覆盖 P16 crates test/clippy、正式链、schema check，隔离 `target/gates` 且 trap 清理，不跑 workspace 全量
- [ ] P16 状态跨宿主重启恢复（未达成：无生产持久化/发布链路）
- [ ] Plan approval 进入 Agent Loop gate、Goal steering 进入 context（未达成，属 P1 接线）

## 相关文档

- [sessions（Event Store / Compat Import）](sessions.md) · [agent-engine](agent-engine.md) · [policy](policy.md) · [gui-connection](gui-connection.md)
- [ADR-016 事件可重放](../adr/ADR-016-core-event-persist-replay.md) · [ADR-024 Event Hub](../adr/ADR-024-shared-app-service-event-hub.md) · [ADR-025 CLI 唯一宿主](../adr/ADR-025-cli-is-sole-host.md) · [ADR-030 Core 唯一事实源](../adr/ADR-030-core-sole-source-of-truth.md)
- [Phase 16 评审（当前状态事实）](../review/p16-review.md) · [ROADMAP Phase 16](../../ROADMAP.md) · [plan/P16-*](../../plan/)
