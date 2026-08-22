# pawork-workflow

plan / task 纯 reducer。依赖 `pawork-domain`。ADR-039 不合并清单成员。

## 职责

在内存里折叠 canonical `PlanEvent` / `TaskEvent`，并提供命令面（创建计划、评审、任务注册与状态迁移）。不执行进程、不依赖 `pawork-exec`、不依赖 `pawork-orchestration`（装配在 `pawork-app`）。Goal / Automation / Monitor **reducer 已归档**；domain 事件类型仍保留以便重放。

## 模块树

```
src/
  lib.rs
  plan/{mod,error,service,snapshot,state}.rs
  task/{mod,error,manager,state}.rs
tests/
  plan_service.rs  state_and_replay.rs
```

无 `goal/`、`automation/`、`monitor/`；无 `process-exec` feature。

## 对外入口/API 面

仅 `pub mod plan` / `pub mod task`，crate 根无 glob re-export。

- **plan**：`PlanService`（`create_plan` / `replace_plan` / `update_step` / `request_review` / `revise` / `approve` / `reject` / `add_comment` 等）、`PlanSnapshot`、`apply` / `replay`（纯 fold）、`is_legal_step_transition`。步骤：`Pending → InProgress → Completed | Blocked`。
- **task**：`TaskManager`（`register` / `start` / `suspend` / `resume` / `finish` / `cancel`）、`TaskManagerState::apply`、`is_active_status` / `is_terminal_status`。`TaskKind::{Process, Agent, Monitor, Automation}` 是 **domain 枚举**（任务种类），不是已归档的三域 reducer。

事件类型定义在 `pawork-domain`，本包不重新定义。

## 依赖与被依赖

- **依赖**：`pawork-domain`。`serde` / `regex` / `tokio` / `tracing`。`default = []`。
- **被依赖**：仅 `pawork-app`（`plan_host.rs`、`tasks_host.rs` / `services/tasks.rs`）。orchestration **不**依赖本包。

## 红线与注意事项

- 归档、现行不存在：`goal` / `automation` / `monitor` 模块与 `process-exec` feature（ADR-038 D4，tag `v2-final`）。Cargo 包描述仍写「五合一」，以本树为准。
- Plan/Review 不授予写权限；core 无平台名 / Provider 名分支。
- `replay` 是纯折叠、不做再校验；非法迁移由命令面 `PlanError` / `TaskManagerError` 拒绝。

## 相关文档

- [docs/design.md](../../docs/design.md) §2
- [ROADMAP.md](../../ROADMAP.md) §3.3（teams / goal / automation / monitor 复活）
- [代码地图总索引](../../docs/code-map/README.md)
