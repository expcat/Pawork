# ADR-009：默认启用 Workspace Trust

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

Pi 默认以启动用户的系统权限运行，没有内置完整权限隔离，存在破坏性命令与越权写入风险。

## 决策

新核心默认对未信任工作区限制写入与命令，需用户显式信任后才放宽。Workspace Trust 状态参与 Policy 决策。

## 后果

- 未信任工作区默认安全。
- 用户体验上需明确信任流程，避免误判。
- 安全验收包含「未信任工作区测试」。

## 相关

- [policy](../features/policy.md) · [sandbox](../features/sandbox.md) · [安全验收](../quality/security-acceptance.md)
