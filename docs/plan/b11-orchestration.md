# B11 · Orchestration 注入与恢复面裁决

> 状态：✅ 完成。完成日期：2026-10-08。裁决、必要自动验证与独立 review 已完成；未推送、未发布。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决 `with_task_graph`、`with_worktree_allocator`、`with_patch_merger`、`retry_task`、`recover_report`。区分 App 模型子代理、S11 demo、包内测试注入和未激活库能力。不激活 Host DAG、自动合并或热恢复。

## 事实

基线 `main` / `427399b30a4ed3e3fc8e61a1ea7f3c280e9de13a`，开工时工作区干净。

App 模型子代理在 [subagents.rs](../../crates/app/src/subagents.rs) 的 `supervisor()` 里用控制面 pool / policy / ledger 构造 `AgentSupervisor`，然后 spawn、start、complete、fail、cancel_tree。`SpawnRequest` 的 `worktree_path` 为空，`task_deps` 为空，不注入图、分配器或合并器。子代理与父工作区共享文件。失败后的重跑是新的 `spawn_agent`，恢复走会话事件。

[orchestration_host.rs](../../crates/app/src/orchestration_host.rs) 的 `run_multi_agent_demo` 同样只做 spawn、complete、cancel_tree 和预算记录。它是 S11 demo，由 CLI `run_agents_demo` 调用，不是模型 runtime。

五个入口在改动前只有 `crates/orchestration` 的测试调用。`TaskGraph`、`WorktreeAllocator`、`PatchMerger` 类型仍公开。spawn、complete、fail、cancel_tree 在正式库里仍会在已注入时推进任务图并释放 worktree。`GitDiffProvider::base_content` 在 `git show HEAD:<rel>` 失败时仍回退父侧当前文件。`OrchestrationEvent`、`WorkerState`、`TaskState` 未删。`replay_workers` 仍是公开重放原语。

## 逐项裁决

| 入口 | 裁决 | 依据 |
| --- | --- | --- |
| `with_task_graph` | 收到 `#[cfg(test)]` | 没有生产调用。正式库保留图字段和 spawn / 终态 / 取消里的条件分支。O-06 的双顺序反向依赖校验未做，不激活 Host DAG。 |
| `with_worktree_allocator` | 收到 `#[cfg(test)]` | 子代理明确共享父工作区，demo 也不分配。正式库保留分配失败收口和 complete / fail / cancel_tree 的显式释放。git feature 测试仍覆盖真实隔离。 |
| `with_patch_merger` | 收到 `#[cfg(test)]` | 没有生产调用。`propose_patch` / `approve_patch` 和 parent workspace 注入一并只留在测试构建。O-07 未改，不激活合并，冲突仍拒绝自动写入。 |
| `retry_task` | 收到 `#[cfg(test)]` | 只把任务图从 Failed 复位到 Created 并增加 attempt，发出 `TaskRetried`。同一 worker 仍是 Failed，不 spawn。现有测试补上这条断言。 |
| `recover_report` | 收到 `#[cfg(test)]` | 重放后把活动孤儿记为 Failed，不重建 WorkerEntry、children、cancel token、lease，不 emit。App 不持久化 `OrchestrationEvent`。正式库不保留这个热恢复外形。 |

`with_parent_workspace` 不是独立产品入口，只给测试中的分配器和合并器提供基准目录。

## 实际改动

可见性改在 `crates/orchestration/src/supervisor/mod.rs` 与 `recovery.rs`。App 生产调用未改，[app Spec](../spec/crates/app.md) 不需要同批修改。同步 [orchestration Spec](../spec/crates/orchestration.md)、[backlog](../spec/backlog.md) 的 O-06 / O-07 和本文件、ROADMAP。O-06 / O-07 的激活条件没有关闭，也没有补校验或 fork commit。

## 验证

日志目录 `/tmp/pawork-b11.VnArnn`。Cargo 串行，沿用仓库 kache。中间一次把入口收成 `pub(crate)` 后，`cargo check -p pawork --offline` 对正式库报 dead_code；最终改成 `#[cfg(test)]` 后再查，编排包无警告。

| 命令 | 结果 | 日志 |
| --- | --- | --- |
| `bash scripts/test.sh --print orchestration` | `cargo test --offline --tests -p pawork-orchestration --features pawork-orchestration/git` | 终端记录 |
| `bash scripts/test.sh --log /tmp/pawork-b11.VnArnn/orchestration-git-cfg-test.log orchestration` | 87 passed，0 failed，exit 0，Elapsed 18s。实际 Cargo：`cargo test --offline --tests -p pawork-orchestration --features pawork-orchestration/git` | `orchestration-git-cfg-test.log` |
| `cargo check -p pawork --offline` | exit 0；编排包无 dead_code。仅既有 `block` future-incompat 提示 | `pawork-host-check-final.log` |
| `git diff --check` | exit 0 | 无 |

更早的 `orchestration-git.log` 是去掉未使用重导出之前的 87 通过，带一条 unused import。`orchestration-git-final.log` 是 `pub(crate)` 形态的 87 通过。`pawork-host-check.log` 是该形态的 dead_code 警告，不作为最终结果。

最终检查后，仅测试模块内 `RecoveryReport` / `recover_report` 的 `pub` 收到 `pub(crate)`，并修正两处文档文本，未重跑 Cargo 或 rustdoc。Sol max 独立审查与同线程复审接受这一同 crate 可见性收窄的证据边界；文档小修后的 diff 与 119 个本地链接检查通过。整文件 rustfmt 仍有基线既存样式差异，本任务未扩改，也不宣称格式全绿。

未跑真实 Provider、GUI 或人工验收。自动回归不表示产品人工验收。

```text
Validated: bash scripts/test.sh --log /tmp/pawork-b11.VnArnn/orchestration-git-cfg-test.log orchestration（实际 cargo test --offline --tests -p pawork-orchestration --features pawork-orchestration/git，87 passed，exit 0，证据 orchestration-git-cfg-test.log）；cargo check -p pawork --offline exit 0；git diff --check exit 0
Targeted regressions: orchestration lib 87，含 git feature 的 worktree 隔离与释放、cancel_tree、lease、retry 后 worker 仍 Failed、recover_report 不重建、propose/approve 不自动合并冲突
Full workspace gate: NOT RUN（当前未设置全量门禁）
```

## 完成记录

2026-10-08 已实现裁决并完成上表必要自动验证。独立审查与同线程复审均由 `gpt-6.1-sol` / `max` 完成，最终结论 PASS、无剩余问题；两处 P3 文档问题已修复。指挥核对 HEAD、原定 7 文件写入集及 `final.diff` 字节一致后同步完成状态。O-06 / O-07 激活前置仍未解决，本任务不代表完整 DAG、自动合并或 Host 热恢复已实现；不推送、不发布。
