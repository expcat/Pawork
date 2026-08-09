# P5-7：Tool Result 裁剪

> Phase 5 · Session、Branch 与 Compaction · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-6、P3-2

**最终目的**：实现 tool result 分级裁剪（小/中/大/超大）与 artifact 引用，避免超大输出无限进入上下文。

**涉及范围**：`context-engine`

## 细分步骤

1. **小/中/大/超大分级策略** —— 目的：按体量裁剪。
2. **大输出转 artifact 引用** —— 目的：上下文轻量。
3. **可回溯完整内容** —— 目的：按需读取。
4. **裁剪测试** —— 目的：边界正确。

## 主要产出物

- Tool Result 裁剪策略

## 验收标准

- [x] 超大输出不无限进入上下文（转 artifact 引用）

**相关文档**：[context](../docs/features/context.md) · [artifacts](../docs/features/artifacts.md) · [ADR-018](../docs/adr/ADR-018-large-payload-artifact-id.md) · [ROADMAP](../ROADMAP.md)
