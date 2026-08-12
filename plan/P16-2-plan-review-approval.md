# P16-2：Plan Review / Revision / Approval（计划评审与审批）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：library/core verified，host composition deferred） · 依赖：P16-1、P0-8、P1-4、P4-9

**最终目的**：在 Plan Mode 之上建立「评审—修订—批准」闭环：用户或 Reviewer 可对 Plan 提出行级评审意见、要求修订，并在批准前作为 gate 阻断 Agent 进入执行，使「先规划后执行」成为可控、可审计的流程，而非 Agent 自行启动写入。

**涉及范围**：`plan-service`（评审状态、`PlanCommentAnchor` 与 gate）；复用 `agent-events`、`core-api`、`policy-engine`、`checkpoint-service`（审批即检查点）。P16-8 可在后续把通用 Review Finding 适配为 Plan comment，但不是本任务前置。

## 细分步骤

1. **评审状态机** —— 目的：为 Plan 增加 `draft → in_review → changes_requested → approved/rejected` 状态，状态转移均为 canonical event（`PlanReviewRequested` / `PlanRevised` / `PlanApproved` / `PlanRejected`）。
2. **Plan comment 锚点** —— 目的：用稳定 `plan_version + step_id + line_offset` 锚定计划正文，引用文件时附可选 `file:line`；该最小领域类型属于 plan-service，不等待 P16-8，后续可无损转换为通用 Review Finding。
3. **修订回合** —— 目的：`changes_requested` 触发 Agent 生成 Plan 新版本（经 P16-1 替换语义），保留修订链与每轮评审快照，可追溯「为什么从 A 改到 B」。
4. **审批 Gate** —— 目的：Agent Loop 在进入执行阶段前检查 Plan 审批状态，未 `approved` 则暂停（复用 P3-1 运行状态机的 `paused`），审批通过后以 `checkpoint-service` 落检查点再继续，保证可回滚到批准点。
5. **只读策略延续** —— 目的：审批前的 Plan 仍受 P16-1 只读约束；审批本身不扩大权限，仅放行执行 gate，防止「批准」被曲解为「授权任意写」。
6. **查询面与通知** —— 目的：`core-api` 暴露评审/审批查询与事件，GUI/CLI 可展示待审项与修订链。
7. **定向 / Mock 测试** —— 目的：评审状态机、修订回合、审批 gate 阻断/放行、检查点落点。仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- Plan 评审状态机与审批 Gate
- 修订链与评审快照
- 审批相关 canonical event 与查询面
- 定向测试

## 验收标准

- [ ] 未达成：`is_approved_for_execution` 零消费者，Agent Loop gate 未接（`approval_gate_closed_until_approved` 仅验证库内状态，不阻断真实执行）→ 登记 **P16-10 #T2（Agent Loop 纵向链：审批 gate）**
- [x] 评审意见带 `plan_version + step_id` 行锚点，修订生成带 `parent_id` 的新 Plan 版本且保留历史（测试支撑：`comments_carry_line_anchors` / `version_history_forms_chain` / `revise_validates_version_chain`）
- [ ] 未达成：`approve` 只接受调用方传入的可选 `checkpoint_id`，不创建检查点；`checkpoint-service` 未接 → 登记 **P16-10 #T2**
- [x] 评审/审批全流程为 canonical event 且 service 级可重放（测试支撑：`review_flow_replays_identically` / `review_events_round_trip_through_agent_event`；持久化链路同 P16-1，归 P16-10 #T1）

**相关文档**：[plan-mode (P16-1)](P16-1-plan-mode.md) · [P16-8 Review Engine](P16-8-review-engine.md) · [policy](../docs/features/policy.md) · [checkpoint](../docs/features/checkpoint.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；依赖 P16-1 的 `plan-service`，Plan comment 使用最小稳定锚点，P16-8 后续提供通用 Review adapter。`policy-engine` 仅做 gate 判定，不扩权。

## 校准记录（2026-08-12）

依据 [p16-review](../docs/review/p16-review.md) 评审结论与当前工作区 remediation 状态校准：评审状态机、行锚点评论、修订链、评审/审批 canonical 事件化属库级实现且有测试支撑，保留 **TargetVerified（library/core）**；「审批 gate 进入 Agent Loop、审批落 checkpoint、policy/查询面接线」未达成，登记 **P16-10** 并映射后续任务 #T1（宿主装配与 core-api/EventHub 接线）、#T2（Agent Loop 纵向链：审批 gate 与 checkpoint 落点）。已知残余：`request_review` / `request_changes` 均发 `ReviewRequested`（事件语义依赖折叠前状态），未列入本项验收但由 P16-10 收口时复核。

验证记录：`scripts/p16-gate.sh` 四类全 PASS（独立 `target/gates` 跑完已清理）；plan-service 定向测试 18 passed。

```text
Validation Level: L2（P16 簇门禁脚本，文档校准任务附带复跑）
Affected crates: none（本任务仅改 plan/P16-2 文档）
Validated: scripts/p16-gate.sh；plan-service 定向测试
Targeted regressions: none
Full workspace gate: NOT RUN（未命中升级条件）
```
