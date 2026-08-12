# P16-3：Goal Mode（目标、成功标准与转向）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：library/core verified，host composition deferred） · 依赖：P0-8、P1-4、P3-1、P3-6；建议 P16-1

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

- [ ] 部分达成：Auto/Human 区分且 Human 项仅能经显式人审入口满足（测试支撑：`human_criterion_cannot_be_satisfied_by_agent` / `human_criterion_satisfied_via_explicit_human_entry`）；`achieve` 不校验全部标准、无 actor 身份，命令面无法证明「Agent 不能自行宣布成功」→ 未达成，登记 **P16-10 #T2**
- [ ] 部分达成：progress 按 criteria 命中率计算且 `CriterionSatisfied` / `ProgressUpdated` 事件可追溯、重放完整（测试支撑：`progress_is_hit_rate_of_criteria` / `replay_rebuilds_reconstructible_state_identical_to_stepwise_apply`）；未消费 Plan 步骤（P16-1）→ 未达成，登记 **P16-10 #T2**
- [ ] 部分达成：pause/resume 状态保留/恢复已验（`pause_resume_preserves_state_and_resume_recomputes_budget`）；`resume` 接收调用方直接传入的预算值，预算组件复算未接 → 未达成，登记 **P16-10 #T2**
- [x] steering 作为 canonical event（`GoalEvent::Steered`）可重放回溯（测试支撑：`steering_is_recorded_and_replayable`；注入 Agent context 属主流程接入，归 P16-10 #T2）

**相关文档**：[plan-mode (P16-1)](P16-1-plan-mode.md) · [agent-engine](../docs/features/agent-engine.md) · [context（预算）](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `agent-engine` 运行状态机与预算（P3-1/P3-6）。新 crate `goal-service` 依赖方向：`agent-domain → goal-service → app-service`。

## 校准记录（2026-08-12）

依据 [p16-review](../docs/review/p16-review.md) 评审结论与当前工作区 remediation 状态校准：Goal 状态机、Auto/Human 判定、criterion 满足位事件化、progress/steering canonical 事件与完整重放（live→fresh snapshot 相等）属库级实现且有测试支撑，保留 **TargetVerified（library/core）**；「achieve 校验全部标准、actor 身份、progress 消费 Plan 步骤、resume 预算由预算组件复算、steering 进入 Agent context」未达成，登记 **P16-10** 并映射后续任务 #T2。

验证记录：`scripts/p16-gate.sh` 四类全 PASS（独立 `target/gates` 跑完已清理）；goal-service 定向测试 15 passed（含 serde 兼容回归）。

```text
Validation Level: L2（P16 簇门禁脚本，文档校准任务附带复跑）
Affected crates: none（本任务仅改 plan/P16-3 文档）
Validated: scripts/p16-gate.sh；goal-service 定向测试
Targeted regressions: Goal replay 完整性（criterion 满足位）+ 旧流 serde 兼容
Full workspace gate: NOT RUN（未命中升级条件）
```
