# P16-4：Background Task Manager（后台任务与断连续存）

> Phase 16 · Modern Agent Workflow · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P1-4、P3-1、P4-12、P11-6、P11-7、P13-2；Agent kind 与 P12-1 协调

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

## 主要产出物

- `task-manager` 统一抽象（四 kind）+ 状态机 + canonical event
- 断连续存与重连恢复（接入 Event Hub snapshot/replay）
- 取消树接线与查询面
- 定向测试

## 验收标准

- [ ] process / agent / monitor / automation 四类任务统一注册、状态可查
- [ ] CLI/GUI 断连后任务继续运行，重连可恢复任务视图与增量输出
- [ ] 取消 parent 传播到子任务/子进程，无孤儿残留
- [ ] 后台任务不因「后台」绕过 sandbox / policy / 预算约束

**相关文档**：[persistent-process-monitor (P16-6)](P16-6-persistent-process-monitor.md) · [scheduled-automation (P16-5)](P16-5-scheduled-automation.md) · [multi-agent](../docs/features/multi-agent.md) · [gui-connection](../docs/features/gui-connection.md) · [process](../docs/features/process.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08）**：不新增第三方依赖；复用 `process-runtime` / `sandbox-runtime` / `agent-engine`（Multi-Agent）与 P13-5 Event Hub。新 crate `task-manager` 依赖方向：`agent-domain → task-manager → app-service`。
