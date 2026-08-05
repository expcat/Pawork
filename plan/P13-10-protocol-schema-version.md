# P13-10：GUI Protocol schema 版本化

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P13-3

**最终目的**：完成 GUI Connection Protocol schema 版本化与向后兼容策略，保证 GUI（独立进程、可能独立发布）与 CLI/Core 可独立演进。

**涉及范围**：`schemas/gui-protocol`、`core-api`、`gui-protocol`

## 细分步骤

1. **API version 机制与协商** —— 目的：版本可协商。
2. **向后兼容策略** —— 目的：演进不破坏。
3. **废弃与迁移流程** —— 目的：可控演进。
4. **测试** —— 目的：兼容可验证。

## 主要产出物

- GUI Protocol schema 版本化与兼容策略

## 验收标准

- [ ] GUI Protocol schema 完成版本化与兼容策略
- [ ] GUI API 有版本与 Contract Tests

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md) · [ROADMAP](../ROADMAP.md)
