# P5-8：Export / Import

> Phase 5 · Session、Branch 与 Compaction · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-5

**最终目的**：实现 session 导出/导入（schema 稳定），为迁移、备份与 Pi 导入提供基础格式。

**涉及范围**：`session-store`

## 细分步骤

1. **导出 schema 定义** —— 目的：稳定格式。
2. **导出实现** —— 目的：可导出完整 session。
3. **导入实现 + 校验** —— 目的：可导入并校验。
4. **往返测试** —— 目的：导出再导入等价。

## 主要产出物

- Export / Import

## 验收标准

- [x] 导出/导入 schema 稳定
- [x] 往返等价

**相关文档**：[sessions](../docs/features/sessions.md) · [ROADMAP](../ROADMAP.md)
