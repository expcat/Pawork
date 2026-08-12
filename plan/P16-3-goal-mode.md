# P16-3：Goal Mode（目标、成功标准与转向）

> Phase 16 · Modern Agent Workflow · 状态：🟢已实现 · 交付成熟度：TargetVerified · 依赖：P0-8、P1-4、P3-1、P3-6；建议 P16-1

**最终目的**：引入「Goal」作为一次 Agent 工作的长期锚点：一个 Goal 携带可验证的 success criteria、可度量的 progress，支持 pause/resume 与运行中 steering（转向/纠偏），使 Agent 在长任务中始终对齐用户目标，而非被中途漂移带偏。

**涉及范围**：`agent-domain`（Goal 领域类型）、新增 `goal-service`；复用 `agent-events`、`core-api`、`agent-engine`（运行状态机/预算）、`session-store`

## 细分步骤

1. **Goal 领域模型** —— 目的：`agent-domain` 增加 `Goal` / `GoalId` / `SuccessCriterion`（可机检或可人判）/ `GoalStatus`（`active` / `paused` / `achieved` / `abandoned`）；Goal 关联一个或多个 Session/Run，跨 Run 存续。
2. **可验证成功标准** —— 目的：success criteria 区分「自动可检」（如测试通过、文件存在、diff 为空）与「需人确认」，并在 progress 计算中分别给出客观进度与人审项，避免 Agent 自行宣布「完成」。
3. **进度度量** —— 目的：基于已 completed 的 Plan 步骤（P16-1）与 success criteria 命中率计算 progress，progress 变更发 `GoalProgressUpdated` canonical event。
4. **pause / resume** —— 目的：`paused` 暂停新 Run 的派发但保留状态，`resume` 从断点继续；与运行状态机（P3-1）和预算（P3-6）联动，resume 时复算剩余预算而非沿用旧值。
5. **Steering（转向）** —— 目的：运行中允许用户向当前 Goal 注入 steering 输入（修正方向/约束/新优先级），Agent 将其纳入上下文而非丢入普通消息流；steering 记为 `GoalSteered` event，可在事后回溯「哪一步被纠偏」。
6. **查询面与订阅** —— 目的：`core-api` 暴露 Goal 查询/订阅，GUI/CLI 呈现目标、成功标准、进度与转向历史。
7. **定向 / Mock 测试** —— 目的：成功标准可检性、progress 计算、pause/resume 预算复算、steering 注入与回溯。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `agent-domain` 的 Goal 类型
- `goal-service`：成功标准 / 进度 / pause-resume / steering + canonical event
- 查询面与定向测试

## 验收标准

- [ ] Goal 携带可验证 success criteria，自动可检项不被 Agent 自行判定为达成
- [ ] progress 基于 Plan 步骤与 criteria 命中率，变更可追溯
- [ ] pause/resume 正确保留/恢复状态，resume 时复算剩余预算
- [ ] steering 注入作为 canonical event，可在重放中回溯

**相关文档**：[plan-mode (P16-1)](P16-1-plan-mode.md) · [agent-engine](../docs/features/agent-engine.md) · [context（预算）](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `agent-engine` 运行状态机与预算（P3-1/P3-6）。新 crate `goal-service` 依赖方向：`agent-domain → goal-service → app-service`。
