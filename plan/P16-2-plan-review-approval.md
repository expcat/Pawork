# P16-2：Plan Review / Revision / Approval（计划评审与审批）

> Phase 16 · Modern Agent Workflow · 状态：🟢已实现 · 交付成熟度：TargetVerified · 依赖：P16-1、P0-8、P1-4、P4-9

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

- [ ] 未 `approved` 的 Plan 不能触发 Agent 写入/工具执行（gate 生效）
- [ ] 评审意见带行锚点，修订生成带 `parent_id` 的新 Plan 版本且保留历史
- [ ] 审批通过后落检查点，可回滚到批准点
- [ ] 评审/审批全流程为 canonical event，可重放

**相关文档**：[plan-mode (P16-1)](P16-1-plan-mode.md) · [P16-8 Review Engine](P16-8-review-engine.md) · [policy](../docs/features/policy.md) · [checkpoint](../docs/features/checkpoint.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；依赖 P16-1 的 `plan-service`，Plan comment 使用最小稳定锚点，P16-8 后续提供通用 Review adapter。`policy-engine` 仅做 gate 判定，不扩权。
