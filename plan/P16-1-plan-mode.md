# P16-1：Plan Mode（只读计划与状态）

> Phase 16 · Modern Agent Workflow · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-8、P1-4、P3-1、P4-9

**最终目的**：为 Agent 引入只读的 Plan 表示与状态机，使 Agent 在动手前产出一个有序、可勾选的计划，并在执行过程中只更新计划状态（`pending → in_progress → completed/blocked`），而不被当作写入指令通道。计划演进作为 canonical event 持久化、可重放，让 CLI/GUI 与用户可在执行前审阅意图并跟踪进度。

**涉及范围**：`agent-domain`（Plan 领域类型）、新增 `plan-service`；复用 `agent-events`、`core-api`、`session-store`、`policy-engine`

## 细分步骤

1. **Plan 领域模型** —— 目的：在 `agent-domain` 增加 `Plan` / `PlanStep` / `PlanStepStatus`（`pending` / `in_progress` / `completed` / `blocked`）/ `PlanId`，纯领域类型、不依赖任何 infra，统一「意图」的表示。
2. **只读策略约束** —— 目的：明确「Plan 不是写入指令」——Plan 内容由 Agent 生成后视为只读建议，不触发工具/文件变更，仅承载文本与状态；`policy-engine` 对 Plan 不授予额外写权限，防止 Plan 被当作命令通道绕过审批。
3. **状态机与事件** —— 目的：定义步骤状态机的合法转移，发出 `PlanCreated` / `PlanStepUpdated` / `PlanReplaced` 事件并写入 `agent-events`，使演进可追溯、崩溃后可重放恢复当前进度。
4. **版本与替换语义** —— 目的：Agent 可整体替换 Plan（新版本带 `parent_id` 指针），旧版本保留，支持 Plan 修订历史而不丢失，且替换本身也是 canonical event。
5. **查询面与订阅** —— 目的：`core-api` 暴露 `get_plan` / `plan snapshot`，经 GUI Connection Protocol 订阅，让 CLI/GUI 实时呈现当前 Plan 与步骤状态。
6. **定向 / Mock 测试** —— 目的：状态机合法/非法转移、事件持久化与重放恢复、只读策略不被绕过（构造「带写动作的 Plan」断言其不产生副作用）。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `agent-domain` 的 Plan 类型
- `plan-service`：状态机 + 事件 + 查询面
- 定向测试（状态转移 / 重放 / 只读断言）

## 验收标准

- [ ] Plan 步骤状态只能按状态机合法转移
- [ ] Plan 演进作为 canonical event 持久化，且可经重放恢复当前 Plan 与进度
- [ ] Plan 不携带任何写入/工具执行能力，`policy-engine` 不因 Plan 授予额外权限
- [ ] `core-api` 可查询与订阅 Plan，CLI/GUI 可呈现

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [policy](../docs/features/policy.md) · [gui-connection](../docs/features/gui-connection.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `agent-domain` / `agent-events` / `core-api` / `policy-engine`。新 crate `plan-service` 依赖方向：`agent-domain → plan-service → app-service`。
