# B3 · Control-plane 读面与未接线能力裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决额度读面、租约恢复和未消费的凭证、预算及保留策略类型。

## 当前线索与读取范围

原 ROADMAP 登记 QuotaService `read` / `read_cache_only` / `overview_cache_only` / `invalidate` 零生产消费者；`overview` 已核实有消费者。另有 LeaseProjection 崩溃恢复机器在 app 零接线，以及 `CredentialPicker` / `BudgetCap` / `PolicyGate::Retention` 变体。

[control-plane Spec](../spec/crates/control-plane.md)、[app Spec](../spec/crates/app.md)。

## 实施范围

- 重新核对各接口生产读面，不把 `overview` 纳入零消费者清理。
- 对租约恢复、选择器、预算和保留策略逐项裁决消费、保留或归档，避免提前接入整套框架。

## 完成条件

- 所有登记项有裁决和证据，真实额度、租约及账本消费者保持完整。
- 改动影响账本幂等或恢复时保留对应定向回归，并更新 Spec。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

