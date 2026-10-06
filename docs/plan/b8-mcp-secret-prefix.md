# B8 · MCP Secret 前缀单一事实源裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决 workspace 与 auth 间的前缀副本及依赖方向。

## 当前线索与读取范围

原项为 `locator::is_mcp_secret_service` 消费收敛收尾；workspace 的 `MCP_SECRET_SERVICE_PREFIX` 前缀副本仍待彻底收敛，关联 RV-2026-10-B。

[auth Spec](../spec/crates/auth.md)、[workspace Spec](../spec/crates/workspace.md)、[条件性工程项](../spec/backlog.md#8-条件性工程项)。

## 实施范围

- 比较现有依赖方向允许的最小方案，不能仅为一个常量先建公共框架。
- 只有裁决后才改变依赖或删除副本；同时收敛对应消费者。

## 完成条件

- 前缀的单一事实源和依赖方案有明确裁决，凭证定位结果保持一致。
- 若实际改变依赖边，同步架构和包级 Spec，并验证受影响 Secret 定位行为。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

