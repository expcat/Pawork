# P7-5：Worktree

> Phase 7 · Git、Diff 与 Worktree · 状态：🟡未开始 · 依赖：P7-1

**最终目的**：实现 Git Worktree 创建/删除，并保证清理不删用户数据，为多分支并行与 Multi-Agent 隔离工作区提供基础。

**涉及范围**：`git-service`、`workspace-service`

## 细分步骤

1. **worktree 创建/删除** —— 目的：基础操作。
2. **清理安全性** —— 目的：不删用户数据。
3. **与 workspace-service 集成** —— 目的：作为工作区可被索引。
4. **清理测试** —— 目的：不删用户数据。

## 主要产出物

- Worktree 管理

## 验收标准

- [ ] 清理不删用户数据

**相关文档**：[git-diff](../docs/features/git-diff.md) · [ADR-007 系统 Git](../docs/adr/ADR-007-system-git.md) · [ROADMAP](../ROADMAP.md)
