# ADR-008：Built-in Tools 使用 capability 分类

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

工具并发/串行策略、权限与审批需要统一依据，否则难以判断只读可并发、写串行、Git Index 串行等约束。

## 决策

每个工具声明 capability 类别：ReadOnly / WorkspaceWrite / GitWrite / Process / Network / UserInteraction / ExternalPlugin。Tool Scheduler 与 Policy Engine 依据类别决定调度与审批。

## 后果

- 调度与权限规则可表达、可审计。
- 新增工具须显式声明类别。
- 同文件操作、Git Index 等串行约束由调度器统一保证。

## 相关

- [tools](../features/tools.md) · [policy](../features/policy.md) · [控制流](../architecture/control-flow.md)
