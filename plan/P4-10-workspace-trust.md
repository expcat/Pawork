# P4-10：Workspace Trust

> Phase 4 · 核心工具与权限 · 状态：🟢已完成 · 依赖：P4-9

**最终目的**：实现 Workspace Trust（默认受限、信任后放宽），让未信任工作区默认限制写与命令，满足默认安全（ADR-009）。

**涉及范围**：`workspace-service`、`policy-engine`

## 细分步骤

1. **信任状态与流程** —— 目的：明确信任交互。
2. **默认受限（写/命令）** —— 目的：默认安全。
3. **信任后放宽** —— 目的：信任工作区可用。
4. **未信任工作区测试** —— 目的：安全验收项。

## 主要产出物

- Workspace Trust 集成

## 验收标准

- [x] 未信任工作区默认限制写/命令

**相关文档**：[policy](../docs/features/policy.md) · [sandbox](../docs/features/sandbox.md) · [ADR-009 默认信任](../docs/adr/ADR-009-default-workspace-trust.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
