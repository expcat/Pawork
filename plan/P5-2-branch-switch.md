# P5-2：Branch 切换

> Phase 5 · Session、Branch 与 Compaction · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P5-1

**最终目的**：实现 branch 切换与 tree 浏览，并保护并发写，让多分支工作流顺畅。

**涉及范围**：`session-store`

## 细分步骤

1. **branch 切换** —— 目的：在分支间切换。
2. **tree 浏览** —— 目的：可视化分支。
3. **并发写保护** —— 目的：多写不冲突。
4. **切换测试** —— 目的：状态一致。

## 主要产出物

- Branch 切换

## 验收标准

- [x] 切换后状态正确
- [x] 并发写受保护

**相关文档**：[sessions](../docs/features/sessions.md) · [ROADMAP](../ROADMAP.md)
