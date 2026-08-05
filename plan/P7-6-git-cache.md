# P7-6：Git 缓存 / watcher

> Phase 7 · Git、Diff 与 Worktree · 状态：🟡未开始 · 依赖：P7-2

**最终目的**：实现 Git status 缓存与文件监听，让已缓存 diff 切换快速，提升大仓库交互体验。

**涉及范围**：`git-service`

## 细分步骤

1. **status 缓存** —— 目的：减少重复 Git 调用。
2. **文件监听失效** —— 目的：缓存及时更新。
3. **diff 切换性能** —— 目的：已缓存切换 < 50ms。
4. **一致性** —— 目的：缓存与真实状态一致。

## 主要产出物

- Git 缓存 / watcher

## 验收标准

- [ ] 已缓存 diff 切换 < 50ms

**相关文档**：[git-diff](../docs/features/git-diff.md) · [ROADMAP](../ROADMAP.md)
