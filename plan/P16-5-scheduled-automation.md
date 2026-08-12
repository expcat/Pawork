# P16-5：Scheduled Automation（定时与事件触发自动化 + 结果收件箱）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：library/core verified，host composition deferred） · 依赖：P16-4、P0-8、P3-4

**最终目的**：让用户可声明式地安排自动化：按 `cron` / `interval` / `once` / `event` 四种触发器在指定时机派发一次 Agent/Prompt/工具执行，结果落入可检索的 result inbox，使 Pawork 能承担「每日报告」「事件触发的修复」「一次性定时任务」等无人值守场景，且全程受既有 policy/预算约束。

**涉及范围**：新增 `automation-service`；复用 `task-manager`（P16-4）、`agent-events`、`core-api`、`policy-engine`、`artifact-store`（inbox 载体）、`agent-engine`（执行派发）

## 细分步骤

1. **Automation 定义** —— 目的：`Automation` 描述触发器（`cron` / `interval` / `once` / `event`）与动作（prompt / goal / tool call / 启动 background task），纯领域类型，可序列化持久。
2. **触发器引擎** —— 目的：`cron` 解析五/六字段表达式（参考 cron 语义自实现最小子集，不引入调度框架）；`interval` 用 `tokio` time；`once` 为一次性延时；`event` 订阅 canonical event 流做模式匹配触发。统一调度、可启停、可手动触发。
3. **外部 Trigger Adapter（P2 扩展）** —— 目的：`ExternalTrigger` 枚举与 `external.rs` 输入信封已删除（评审 remediation），本 crate 不再保留任何平台输入 envelope；`automation-service` 只匹配调用方注入的 canonical event payload（`match_event` / `dispatch_event` 对 payload 做正则命中，无平台名称分支）。认证、签名校验、速率限制、重放防护与平台 payload → canonical event 映射的 adapter 延期到 Core 边界实现。
4. **执行派发** —— 目的：触发后经 `task-manager`（P16-4）派发为 background task，受 `policy-engine` 审批/`agent-engine` 预算（P3-4/P3-6）约束，自动化不享受特权。
5. **Result Inbox** —— 目的：每次自动化执行的产出归档为 artifact（复用 `artifact-store`）并登记进 result inbox（按 automation / 时间 / 状态检索），inbox 项为 canonical event `AutomationResultArchived`，可重放。
6. **失败与重试** —— 目的：执行失败按退避重试（复用通用 `http-runtime` 的退避策略语义），连续失败暂停该 automation 并告警，不静默吞错。
7. **查询面** —— 目的：`core-api` 暴露 automation CRUD、手动触发、inbox 查询与订阅，GUI/CLI 呈现调度面板与收件箱。
8. **定向 / Mock 测试** —— 目的：cron/interval/once/event 四触发器命中、event 模式匹配、外部 adapter 认证/去重、inbox 归档与检索、失败退避。用 Mock 时间/事件，仅定向 + Mock smoke，不要求 workspace 全量门禁。

## 主要产出物

- `automation-service`：四类触发器 + 执行派发 + result inbox
- cron 表达式最小子集解析
- automation / inbox 相关 canonical event 与查询面
- 定向测试

## 验收标准

- [ ] 部分达成：四触发器确定性调度与派发计算已验（测试支撑：`four_triggers_check_due_timing` / `event_trigger_matches_and_dispatches` / `interval_advances_and_once_fires_once`，cron 最小子集 16 例）；仓库无 timer/event-loop 调用者，真实触发执行未接线 → 未达成，登记 **P16-10 #T4**
- [ ] 未达成：本 crate 已删除会伪造执行状态的 TaskManager adapter，`AutomationDispatcher` 为注入 trait，无生产 executor；policy/预算约束未接 → 登记 **P16-10 #T4**
- [ ] 部分达成：`ResultInbox` 内存归档与按 automation/时间/状态检索已验（`result_inbox_searchable_by_automation_status_time` / `record_result_rejects_task_not_triggered_by_automation`）；artifact-store 持久化未接 → 未达成，登记 **P16-10 #T4**
- [ ] 部分达成：触发事件 canonical 可重放（`events_round_trip_via_agent_event_and_replay` / `fired_count_is_sourced_from_canonical_state`）、连续失败发 `Suspended` 不静默吞错（`consecutive_failures_suspend_and_alert`）；完整配置/schedule/failure streak/inbox 状态为命令侧视图不随事件重放，真实执行重试/退避未接 → 未达成，登记 **P16-10 #T4**
- [ ] 部分达成：Automation Core 无平台分支（`ExternalTrigger` 五 variant 已按评审删除）；认证 adapter（Webhook/HTTP/GitHub/GitLab/External MCP）未实现 → 未达成，登记 **P16-10 #T4**

**相关文档**：[background-task-manager (P16-4)](P16-4-background-task-manager.md) · [artifacts](../docs/features/artifacts.md) · [policy](../docs/features/policy.md) · [observability](../docs/features/observability.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；cron 语义自实现最小子集（参考标准 cron 表达式），调度用基线内 `tokio` time。新 crate `automation-service` 依赖方向：`agent-domain → automation-service → app-service`。

## 校准记录（2026-08-12）

依据 [p16-review](../docs/review/p16-review.md) 评审结论与当前工作区 remediation 状态校准：cron/interval/once/event 确定性调度、事件模式匹配、派发抽象与 inbox 检索、连续失败暂停告警属库级实现且有测试支撑，保留 **TargetVerified（library/core）**；「真实 timer loop、生产 executor、policy/预算约束、artifact 归档、可重放完整配置/inbox、retry/backoff、认证 adapter」未达成，登记 **P16-10** 并映射后续任务 #T4。

验证记录：`scripts/p16-gate.sh` 四类全 PASS（独立 `target/gates` 跑完已清理）；automation-service 测试 27 passed（lib 16 + 集成 11）。

```text
Validation Level: L2（P16 簇门禁脚本，文档校准任务附带复跑）
Affected crates: none（本任务仅改 plan/P16-5 文档）
Validated: scripts/p16-gate.sh；automation-service 定向测试
Targeted regressions: cron 调度与事件重放
Full workspace gate: NOT RUN（未命中升级条件）
```
