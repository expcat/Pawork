# B10 · Engine 预留入口与上下文 API 裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决未消费入口、工具结果裁剪模块和跨 Storage 小 API。

## 当前线索与读取范围

登记项为 `run_session_turn`、`tool_result_trim` 模块、`ContextBudget::reserved_tokens` 等小 API 和跨 storage 的 `TokenEstimator::estimator_kind`。原 ROADMAP 的零消费者判断须实施前重核。

[engine Spec](../spec/crates/engine.md)、[storage Spec](../spec/crates/storage.md)。

## 实施范围

- 核对现行 Agent loop，而非仅测试装配；裁决入口和模块的必要性。
- 与 [CU-12 截图上下文预算](cu-12-screenshot-context.md) 协调，只在请求侧裁剪，不改持久化证据。
- RV-2026-10-A 测试骨架转发收敛仍按 backlog 触发，不单独扩大本任务。

## 完成条件

- 裁决不删除正在使用或当前计划明确需要的生产行为。
- 事件顺序、工具结果和持久化/重放的受影响回归通过，engine / storage Spec 同步。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

