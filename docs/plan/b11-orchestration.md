# B11 · Orchestration 注入与恢复面裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

核实未由 App 消费的图、工作树、合并与恢复 API。

## 当前线索与读取范围

登记项为 `with_task_graph` / `with_worktree_allocator` / `with_patch_merger` / `retry_task` / `recover_report` 在 app 零调用。

[orchestration Spec](../spec/crates/orchestration.md)、[app Spec](../spec/crates/app.md)、[候选激活条件](../spec/backlog.md)。

## 实施范围

- 区分生产调度、测试注入与尚未激活的图/合并能力。
- 逐项裁决保留、收窄或归档；不顺势激活完整任务 DAG 或自动合并。

## 完成条件

- 裁决有真实消费者和恢复用途依据，现行 worker 生命周期不回退。
- 必要变更同步 Spec；若触及恢复、取消或合并边界，运行对应定向回归。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

