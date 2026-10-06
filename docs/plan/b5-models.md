# B5 · Models 四个预留 API 裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

核实模型库四个 Spec 记录 API 的使用情况并裁决。

## 当前线索与读取范围

登记项为 `validate_context` / `merge_provider_source` / `capability_snapshot` / `filter_by_purpose`；原 ROADMAP 标为零消费者。

[models Spec](../spec/crates/models.md)、[模型网关](../spec/model-gateway.md)。

## 实施范围

- 重新核对生产调用、公开用途和测试锚点，逐项决定保留、收窄或删除。
- 与 [CU-11 模型图像门控](cu-11-model-vision-gate.md) 的消费需求对齐，避免清理后再造同一能力。

## 完成条件

- API 与 Spec 同步，没有破坏模型目录、能力协商或用途筛选。
- 只验证实际受影响行为，不为保留接口补镜像测试。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

