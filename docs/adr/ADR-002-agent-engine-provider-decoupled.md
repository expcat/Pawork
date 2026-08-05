# ADR-002：Agent Engine 与 Provider 解耦

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

各 Provider 在 Tool Call、Thinking、Image、Cache、Stop Reason、Token Usage、Error、OAuth、Streaming 上差异明显。若 Agent Engine 直接判断 Provider 名称走特例，会形成难以维护的条件分支与隐性耦合。

## 决策

Provider 只依赖 canonical domain（统一请求/事件/错误），保存 Provider Raw Metadata，Agent Engine 禁止按 Provider 名称走分支逻辑。通过统一 Contract Tests 验证。

## 后果

- 新增 Provider 不需改动 Agent Engine。
- Provider 差异封装在各自 crate，行为可回归测试。
- 需维护 canonical 抽象的完备性，避免能力被「裁平」。

## 相关

- [providers](../features/providers.md) · [ADR-015 Contract Tests](ADR-015-provider-contract-tests.md) · [workspace-layout](../architecture/workspace-layout.md)
