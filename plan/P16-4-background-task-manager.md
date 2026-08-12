# P16-4：Background Task Manager（后台任务与断连续存）

> Phase 16 · Modern Agent Workflow · 状态：🟢已完成 · TargetVerified（有界：library/core verified，host composition deferred） · 依赖：P1-4、P3-1、P4-12、P11-6、P11-7、P13-2；Agent kind 与 P12-1 协调

**最终目的**：提供统一的后台任务管理器，把 `process` / `agent` / `monitor` / `automation` 四类长生命周期任务纳入同一注册、状态与事件模型，并在 CLI/GUI 断连后任务继续运行、重连可恢复查看，使「关掉终端任务就没了」不再是 Pawork 的体验缺陷。

**涉及范围**：新增 `task-manager`；复用 `process-runtime`、`agent-engine`、`sandbox-runtime`、`agent-events`、`app-service`、`session-store` 与 P13-2 Event Hub。Process/Monitor 先形成闭环；Agent kind 在 P12-1 落地后作为 adapter 接入，多 GUI 订阅在 P13-5 复用同一事件。

## 细分步骤

1. **统一任务抽象** —— 目的：定义 `BackgroundTask` 统一接口，按 `kind` 区分 `process`（子进程/PTY）/ `agent`（子 Agent/Worker）/ `monitor`（监视循环，见 P16-6）/ `automation`（定时触发，见 P16-5），统一 `TaskId`、状态、stdout/stderr/event 流。
2. **生命周期与状态机** —— 目的：`queued → running → suspended → completed/failed/canceled` 状态机，所有转移为 canonical event（`TaskStarted` / `TaskSuspended` / `TaskFinished` 等），可持久化可重放。
3. **断连续存** —— 目的：任务运行与连接生命周期解耦——CLI/GUI 断连不取消任务；重连后通过 Event Hub（P13-5）的 snapshot + replay 恢复任务视图与增量输出，复用慢客户端隔离避免回压拖垮 Core。
4. **资源与隔离** —— 目的：每类任务按 kind 套用既有限制——process 走 sandbox（P11）与进程树清理（P11-7），agent 走 worktree 写入隔离（P12-3）与预算（P12-4），automation 触发的执行同样受 policy 约束，不因「后台」而越权。
5. **取消与传播** —— 目的：取消一个 parent task 按取消树（P12-6）传播到其子任务/子进程，保证不留孤儿；复用进程树清理。
6. **查询面** —— 目的：`core-api` 暴露任务列表/详情/输出流订阅，GUI/CLI 呈现后台任务面板。
7. **定向 / Mock 测试** —— 目的：四类任务的状态机、断连后任务继续与重连 snapshot 恢复、取消传播无孤儿。仅定向 + Mock smoke（模拟断连/重连），不要求 workspace 全量门禁。

### Core-owned 进程统一 Sandbox/Process Runtime 所有权（显式约束）

> 对齐 `.codex-brief/ENHANCEMENT-BRIEF.md` §4「后续 Runtime 执行所有权」：所有 Core-owned 本地进程必须经 `Sandbox Runtime → Process Runtime` 统一执行，后台执行不构成例外。

- process / monitor 任务禁止直接 `tokio::process::Command` / `tokio::spawn` 绕过 Sandbox Runtime；即使任务在后台运行、CLI/GUI 已断连，也必须走同一受沙箱执行路径。
- 不自复制 Job Object / process-group cleanup：统一复用 P11-7 进程树清理，禁止在 `task-manager` 内另写一套清理逻辑造成双重管理或孤儿。
- 不自定另一套 filesystem / network policy：统一复用 `sandbox-runtime` 的 `SandboxPolicy`，后台任务不因「后台」获得额外越权。
- 不因 background 绕过 guarantee reporting：任务实际获得的 `SandboxGuarantees`（含各维度降级）照常上报、可持久化、可观测，与前台任务一致。

## 主要产出物

- `task-manager` 统一抽象（四 kind）+ 状态机 + canonical event
- 断连续存与重连恢复（接入 Event Hub snapshot/replay）
- 取消树接线与查询面
- 定向测试

## 验收标准

- [ ] 部分达成：四 kind 统一注册/状态机/事件化已验（测试支撑：`four_kinds_register_and_query` / `legal_lifecycle_emits_events` / `snapshot_and_replay_rebuild_view`）；agent/monitor/automation kind 的 `start` 仅状态转移，无真实 executor → 未达成，登记 **P16-10 #T3**
- [ ] 部分达成：同进程 lagged subscriber 的 snapshot+增量恢复已验（`lagged_subscriber_recovers_via_snapshot` / `disconnect_and_reconnect_semantics` / `output_cursor_resume_incremental`）；跨连接断连续存未接 EventHub/宿主，Queued 与输出不进 canonical event/artifact，Core 重启无法恢复 → 未达成，登记 **P16-10 #T3**
- [x] process 路径取消传播到子任务/子进程树，无孤儿残留（测试支撑：`process_parent_cancel_cascades` / `process_cancel_terminates_tree` / `cancel_propagates_to_descendants_without_orphans`；经注入 SandboxBackend → ProcessRuntime，无直接 spawn）
- [ ] 部分达成：process 路径 policy 原样透传不升级（`backend_receives_exact_policy_no_escalation` / `no_direct_process_spawn_or_self_made_cleanup`）；预算约束与其余 kind 的 sandbox/policy 接线未实现 → 未达成，登记 **P16-10 #T3**

**相关文档**：[persistent-process-monitor (P16-6)](P16-6-persistent-process-monitor.md) · [scheduled-automation (P16-5)](P16-5-scheduled-automation.md) · [multi-agent](../docs/features/multi-agent.md) · [gui-connection](../docs/features/gui-connection.md) · [process](../docs/features/process.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `process-runtime` / `sandbox-runtime` / `agent-engine`（Multi-Agent）与 P13-5 Event Hub。新 crate `task-manager` 依赖方向：`agent-domain → task-manager → app-service`。

## 校准记录（2026-08-12）

依据 [p16-review](../docs/review/p16-review.md) 评审结论与当前工作区 remediation 状态校准：统一任务抽象、生命周期状态机、取消树传播与真实 Process 执行路径（SandboxBackend → ProcessRuntime、取消令牌、输出驱动）属库级实现且有测试支撑，保留 **TargetVerified（library/core）**；「Agent/Monitor/Automation kind 真实 executor、跨连接断连续存（EventHub snapshot/replay）、Queued/output 事件化与 artifact、policy/预算接线」未达成，登记 **P16-10** 并映射后续任务 #T3。

验证记录：`scripts/p16-gate.sh` 四类全 PASS（独立 `target/gates` 跑完已清理）；task-manager 测试 19 passed（lib 0 + `process_and_policy` 11 + `state_and_replay` 8）。

```text
Validation Level: L2（P16 簇门禁脚本，文档校准任务附带复跑）
Affected crates: none（本任务仅改 plan/P16-4 文档）
Validated: scripts/p16-gate.sh；task-manager 定向测试（process/policy/replay）
Targeted regressions: process 取消树与 sandbox policy 透传
Full workspace gate: NOT RUN（未命中升级条件）
```
