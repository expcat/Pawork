# P7-1：Repository 检测 / branch / HEAD

> Phase 7 · Git、Diff 与 Worktree · 状态：🟡未开始 · 依赖：P4-12

**最终目的**：封装系统 Git（repo 检测、branch、HEAD、错误归一化），为后续 Git 操作提供统一入口（ADR-007）。

**涉及范围**：`git-service`

## 细分步骤

1. **系统 Git 封装（进程监督 + 超时/取消）** —— 目的：统一调用入口。
2. **repo 检测 / branch / HEAD** —— 目的：基本仓库信息。
3. **错误归一化** —— 目的：Git 错误可处理。
4. **版本差异容错** —— 目的：不同 Git 版本输出解析稳定。

## 主要产出物

- `git-service` 基础封装

## 验收标准

- [ ] 可检测 repo 并读取 branch/HEAD

**相关文档**：[git-diff](../docs/features/git-diff.md) · [process](../docs/features/process.md) · [ADR-007 系统 Git](../docs/adr/ADR-007-system-git.md) · [ROADMAP](../ROADMAP.md)
