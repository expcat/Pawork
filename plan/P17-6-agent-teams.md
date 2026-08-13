# P17-6：Agent Teams（智能体团队协作）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟢已验收 · 交付成熟度：Target-Verified · 依赖：P12-1~P12-6、P16-1、P16-2、P3-1、P1-4

**最终目的**：在 [P12](P12-1-supervisor-worker.md) supervisor / worker 基础上实现 Agent Teams——多 Agent 组队的协作层：shared task board（共享任务板）、mailbox（异步消息投递）、presence（在线 / 忙 / 空闲状态）、worker 互联（worker 间直接通信）、plan approval（parent 审批子计划）。让一组 Agent 像团队一样协作完成复杂任务，**复用 P12 编排与 P16 plan**，不重写底层 run loop。

**涉及范围**：新增 `teams`；复用 `orchestration`（P12）、`plan-service`（[P16-1](P16-1-plan-mode.md)）、`agent-events`、`event-store`

## 细分步骤

1. **Team 模型与生命周期** —— 目的：定义 team（成员、角色、创建 / 解散），成员复用 [P12-1](P12-1-supervisor-worker.md) worker 抽象；team 创建 / 成员变更 / 解散作为 canonical event 持久化、可重放。
2. **shared task board** —— 目的：团队级共享任务板（task = owner / status / 依赖），复用 [P12-2](P12-2-task-graph.md) task graph 图结构但提升为团队可见、可认领，状态变更发事件。
3. **mailbox** —— 目的：成员间异步 mailbox（点对点 / 广播投递），消息持久化可重放，与 run loop 解耦，worker 按需拉取。
4. **presence** —— 目的：成员 presence（online / busy / idle / offline），基于 worker 生命周期与 run 状态（[P3-1](P3-1-run-state-machine.md)）派生，供调度与 task board 分配决策。
5. **worker 互联** —— 目的：受控的 worker↔worker 直接通信（非只能经 parent），经 policy / capability 约束避免无限制 fan-out，复用 P12 编排通道。
6. **plan approval** —— 目的：worker 产出 Plan（[P16-1](P16-1-plan-mode.md)）后，team / parent 可审批（approve / reject / 改），审批通过才执行，复用 [P16-2](P16-2-plan-review-approval.md) plan review。
7. **定向 / Mock 测试** —— 目的：task board 认领与状态流转、mailbox 投递与重放、presence 派生、worker 互联受控、plan approval 阻断未批准执行。仅定向 + Mock。

## 主要产出物

- `teams`：team 生命周期 + shared task board + mailbox + presence + worker 互联 + plan approval
- 定向测试

## 验收标准

- [x] 支持 team 创建 / 成员管理 / 解散，全程 canonical event 可重放
- [x] shared task board 可认领流转，mailbox 可异步投递且持久化
- [x] presence 基于 run / worker 状态正确派生
- [x] worker 互联与 plan approval 经策略约束，未批准计划不执行
- [x] **（P16-10 延期接线）workflow 经统一 EventHub 暴露**：shared task board / mailbox / presence 经 `app-service` 唯一 Event Hub 派发与持久化、可重放，不另建 `tokio::broadcast`；automation 执行权威统一归 `task-manager`，`teams` 只拥有协作语义——修复 P16-5 无 timer/loop、`AutomationAction` 不执行、自持 broadcast 未接 ADR-024 统一 Event Hub。见 [p16-review §2.1/§2.3](../docs/review/p16-review.md) 与 [plan/README Phase 16 登记](README.md)。

## 验证记录（2026-08-12）

- `cargo test -p teams -p app-service --all-targets`：通过（`teams` 41 项；`app-service` 及集成回归 106 项）。
- `cargo clippy -p teams -p app-service --all-targets -- -D warnings`：通过。
- 回归覆盖：fan-out 恰好一次结算、mailbox/auto-retry/presence 批量事务原子性、严格 owner + supervisor override、最后 supervisor 保护、SQLite 重放、P12 worker projection 与唯一 EventHub typed 镜像。
- Validation Level：L1；Full workspace gate：NOT RUN（未命中升级条件）。

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [agent-engine](../docs/features/agent-engine.md) · [P16-1 Plan Mode](P16-1-plan-mode.md) · [P16-2 Plan Approval](P16-2-plan-review-approval.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `orchestration` / `plan-service` / `agent-events` / `event-store`。新 crate `teams` 依赖方向：`orchestration → teams → app-service`；不重写 P12 循环，仅在其上叠加团队协作语义。
