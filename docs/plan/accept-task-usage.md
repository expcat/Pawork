# AC-05 · Pawork 任务消耗窗口人工验收

> 状态：等待人工验收。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#等待人工验收)

## 目标

闭合 Pawork 统计窗口的用户验收；两个外部消费者拆为独立任务。

## 当前依据与读取范围

迁移前 ROADMAP 已登记相关功能实现和自动门禁结论，但人工验收及消费者接入独立跟踪。本次仅迁移任务，不重新声明产品验证通过。

[任务消耗 UI](../spec/task-usage-ui.md)、[control-plane Spec](../spec/crates/control-plane.md)、[GUI 设计](../gui-design.md)。

## 验收范围

- 按统计窗口现行口径核对范围、读数、未知/缺失和错误显示。
- 外部消费者分别跟踪 [AC-08 MoMai](accept-momai-task-usage.md) 与 [AC-09 YingMai](accept-yingmai-task-usage.md)，不把本窗口通过算作消费者已接入。

## 完成条件

- 真实账本/Host 数据与窗口结果一致，缺失不伪装为零。
- 用户验收有记录，消费端状态独立保留。

## 完成记录

待填写实际环境、输入、结果、日期与证据。前置或验收缺失时保持当前待办状态；满足全部完成条件后同步本文件，并将 ROADMAP 对应项标为 ✅。

