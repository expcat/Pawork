# P12-3：子 session / 独立 worktree

> Phase 12 · Multi-Agent · 状态：🟡未开始 · 依赖：P12-1、P7-5

**最终目的**：为 Worker 提供子 session 与独立 worktree，实现写入隔离——Worker 不直接改 Parent 工作区。

**涉及范围**：`orchestration`、`session-store`、`git-service`

## 细分步骤

1. **worker 子 session** —— 目的：独立会话上下文。
2. **独立 worktree 分配** —— 目的：写入隔离。
3. **worker 不直接改 parent** —— 目的：安全边界。
4. **隔离测试** —— 目的：parent 工作区不被污染。

## 主要产出物

- worker 隔离工作区

## 验收标准

- [ ] worker 写操作不直接污染 parent workspace

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [sessions](../docs/features/sessions.md) · [git-diff](../docs/features/git-diff.md) · [ROADMAP](../ROADMAP.md)
