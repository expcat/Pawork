# P5-1：Session Tree / Fork

> Phase 5 · Session、Branch 与 Compaction · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P1-5

**最终目的**：实现从任意事件创建新 branch 的 Session Tree，让用户可在任意节点分叉探索，且大 session 不需全量读。

**涉及范围**：`session-store`

## 细分步骤

1. **从任意事件 Fork 新 branch** —— 目的：任意节点分叉。
2. **session tree 视图** —— 目的：可浏览分支结构。
3. **大 session 惰性读取** —— 目的：不全量加载。
4. **Fork 一致性测试** —— 目的：分叉后事件链正确。

## 主要产出物

- Session Tree / Fork

## 验收标准

- [x] 可从任意事件 Fork
- [x] 大 session 不全量读

**相关文档**：[sessions](../docs/features/sessions.md) · [ROADMAP](../ROADMAP.md)
