# pawork-orchestration

多 Agent 编排：supervisor、预算、生命周期、任务图、worktree、patch merge。依赖 domain 与 control-plane（`default-features = false`）。ADR-039 不合并清单成员。

## 职责

在控制面租约 / 用量 / 租户策略之上跑 Supervisor：spawn worker、取消整棵、预算门、任务图、可选 git worktree 与 patch merge。**不**依赖 `pawork-workflow`（plan/task 装配在 app）。R0 已归档 Agent Teams。

## 模块树

```
src/
  lib.rs
  supervisor/{mod,budget_gate,cancel_tree,recovery,registry,spawn}.rs
  budget.rs  lifecycle.rs  task_graph.rs  worktree.rs  merge.rs  identity.rs
```

无 `tests/` 目录。无私有 `pub mod`：全部 glob re-export 到 crate 根。

## 对外入口/API 面

- `AgentSupervisor`：`spawn` / `start_worker` / `complete` / `fail` / `cancel_tree` / `record_usage` / `recover_report` / `propose_patch` / `approve_patch`；`SupervisorConfig`（并发、深度、预算）。
- 预算：`WorkerBudgetController`、`UsageAccumulator`、维度常量 `DIM_INPUT_TOKENS` / `DIM_OUTPUT_TOKENS` / `DIM_COST_MICROS`。
- 生命周期：`WorkerState`、`WorkerStateMachine`、`OrchestrationEvent`、`replay_workers`。
- 任务图：`TaskGraph`（`add_task` / `assign` / `detect_cycle` / `retry`…）。
- worktree：`WorktreeAllocator`；`GitWorktreeAllocator` 仅 feature `git`。
- merge：`PatchMerger`、`MergeDecision`；`GitDiffProvider` 仅 feature `git`。
- identity：`AgentInstance`、`WorkerRole::{Parent,Worker}`。

当前 workspace **没有**成员打开本包 `git` feature（`pawork-app` 以 `default-features = false` 依赖）。

## 依赖与被依赖

- **依赖**：`pawork-domain`；`pawork-control-plane`（关 default，不拉 rusqlite）；optional `pawork-git`。
- **features**：`default = []`；`git`。
- **被依赖**：仅 `pawork-app`（`orchestration_host.rs`）。

## 红线与注意事项

- 禁止加 `pawork-workflow` 依赖边（design.md §2）。
- Teams 模块归档（ADR-038 D5）；不要把 `AppEvent::TeamEvent` 当成现行编排面。
- 取消必须覆盖 worker 树；预算超限走控制面 ledger，不在本包写 SQLite。
- `Cargo.toml` 描述仍写「Agent Teams」，以本树为准。

## 相关文档

- [docs/design.md](../../docs/design.md) §2
- [ROADMAP.md](../../ROADMAP.md) §3.3
- [代码地图总索引](../../docs/code-map/README.md)
