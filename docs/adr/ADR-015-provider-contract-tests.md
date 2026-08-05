# ADR-015：Provider 使用统一 Contract Tests

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

各 Provider 行为差异大，若无统一测试套件，难以保证 canonical 抽象的一致性与回归可控。

## 决策

每个 Provider 使用相同测试套件：text、tool call、multiple tool calls、image、thinking、usage、stop reason、cancel、timeout、rate limit、malformed stream、partial JSON、reconnect、context overflow。

## 后果

- Provider 行为可横向对比、可回归。
- 新增 Provider 须过套件才能合并。
- 与 [ADR-002](ADR-002-agent-engine-provider-decoupled.md) 共同保证 Agent Engine 无 Provider 特例。

## 相关

- [providers](../features/providers.md) · [测试体系](../quality/testing.md) · [ADR-002 解耦](ADR-002-agent-engine-provider-decoupled.md)
