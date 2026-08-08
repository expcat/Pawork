# P16-5：Scheduled Automation（定时与事件触发自动化 + 结果收件箱）

> Phase 16 · Modern Agent Workflow · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P16-4、P0-8、P3-4

**最终目的**：让用户可声明式地安排自动化：按 `cron` / `interval` / `once` / `event` 四种触发器在指定时机派发一次 Agent/Prompt/工具执行，结果落入可检索的 result inbox，使 Pawork 能承担「每日报告」「事件触发的修复」「一次性定时任务」等无人值守场景，且全程受既有 policy/预算约束。

**涉及范围**：新增 `automation-service`；复用 `task-manager`（P16-4）、`agent-events`、`core-api`、`policy-engine`、`artifact-store`（inbox 载体）、`agent-engine`（执行派发）

## 细分步骤

1. **Automation 定义** —— 目的：`Automation` 描述触发器（`cron` / `interval` / `once` / `event`）与动作（prompt / goal / tool call / 启动 background task），纯领域类型，可序列化持久。
2. **触发器引擎** —— 目的：`cron` 解析五/六字段表达式（参考 cron 语义自实现最小子集，不引入调度框架）；`interval` 用 `tokio` time；`once` 为一次性延时；`event` 订阅 canonical event 流做模式匹配触发。统一调度、可启停、可手动触发。
3. **执行派发** —— 目的：触发后经 `task-manager`（P16-4）派发为 background task，受 `policy-engine` 审批/`agent-engine` 预算（P3-4/P3-6）约束，自动化不享受特权。
4. **Result Inbox** —— 目的：每次自动化执行的产出归档为 artifact（复用 `artifact-store`）并登记进 result inbox（按 automation / 时间 / 状态检索），inbox 项为 canonical event `AutomationResultArchived`，可重放。
5. **失败与重试** —— 目的：执行失败按退避重试（复用 `provider-runtime` 退避思路），连续失败暂停该 automation 并告警，不静默吞错。
6. **查询面** —— 目的：`core-api` 暴露 automation CRUD、手动触发、inbox 查询与订阅，GUI/CLI 呈现调度面板与收件箱。
7. **定向 / Mock 测试** —— 目的：cron/interval/once/event 四触发器命中、event 模式匹配、inbox 归档与检索、失败退避。用 Mock 时间/事件，仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `automation-service`：四类触发器 + 执行派发 + result inbox
- cron 表达式最小子集解析
- automation / inbox 相关 canonical event 与查询面
- 定向测试

## 验收标准

- [ ] cron / interval / once / event 四类触发器可按声明时机派发执行
- [ ] 自动化执行经 `task-manager` 派发并受 policy/预算约束，无特权
- [ ] 每次产出归档进 result inbox 且可按 automation/时间/状态检索
- [ ] 触发与结果均为 canonical event，可重放；失败退避且不静默吞错

**相关文档**：[background-task-manager (P16-4)](P16-4-background-task-manager.md) · [artifacts](../docs/features/artifacts.md) · [policy](../docs/features/policy.md) · [observability](../docs/features/observability.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；cron 语义自实现最小子集（参考标准 cron 表达式），调度用基线内 `tokio` time。新 crate `automation-service` 依赖方向：`agent-domain → automation-service → app-service`。
