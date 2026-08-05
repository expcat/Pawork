# P1-9：结构化日志

> Phase 1 · 基础设施 · 状态：🟢已完成 · 依赖：P0-8

**最终目的**：实现结构化日志与自动脱敏，保证明文 secret 不进日志（ADR-014），为可观测性与安全验收奠基。

**涉及范围**：`diagnostics`

## 细分步骤

1. **规范日志字段** —— 目的：结构化可查询。
2. **自动脱敏** —— API Key / Token / Cookie / OAuth / Authorization。目的：不泄漏 secret。
3. **日志级别与采样** —— 目的：可控开销。
4. **脱敏回归测试** —— 目的：保证不漏。

## 主要产出物

- `diagnostics` 日志模块 + 脱敏规则

## 验收标准

- [x] 日志不含明文 secret（含 OAuth/Authorization/Cookie）

**相关文档**：[observability](../docs/features/observability.md) · [ADR-014 Secret Keychain](../docs/adr/ADR-014-secret-os-keychain.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
