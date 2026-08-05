# P1-3：数据库 schema 与迁移

> Phase 1 · 基础设施 · 状态：🟢已完成 · 依赖：P1-2

**最终目的**：建立核心表与向前迁移框架，保证 schema 可随版本演进而无损升级，升级前自动备份，迁移失败可回退。

**涉及范围**：`session-store`、`app-database`

## 细分步骤

1. **定义核心表** —— sessions/events/messages/runs/tool_calls 等。目的：Event Store 与投影落点。
2. **实现迁移框架** —— 版本号 + 顺序迁移。目的：向前迁移可恢复。
3. **升级前自动备份** —— 目的：迁移失败可回退。
4. **迁移往返测试** —— 目的：保证迁移正确。

## 主要产出物

- schema + 迁移框架 + 升级前备份

## 验收标准

- [x] 迁移可恢复
- [x] 升级前自动备份

**相关文档**：[sessions](../docs/features/sessions.md) · [ADR-003](../docs/adr/ADR-003-sqlite-event-store.md) · [ROADMAP](../ROADMAP.md)
