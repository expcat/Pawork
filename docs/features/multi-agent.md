# Multi-Agent 与编排

## 职责

提供 Parent/Worker 多 Agent 编排，隔离写入、传播取消、聚合结果。属 P2 能力，在核心 Coding Agent 稳定前不开发。

## 功能

Parent Agent；Worker Agent；任务分解；子 Session；独立 Worktree；Worker 模型选择；Worker Token 预算；并发上限；结果聚合；Worker 取消；Worker 重试；共享 Artifact；冲突检测；文件锁；状态汇总。

## 调度规则

```text
同一 Worktree 写操作默认串行
不同 Worktree 可以并行
同一 Git Index 操作串行
Worker 不直接修改 Parent Workspace
Parent 决定是否合并 Worker Patch
```

## Phase 15–17 边界

- Pawork 本地编排使用 `AgentTeam`、Parent/Worker、子 Session 与 Worktree；Provider-side Multi-Agent 只是 `ProviderHosted` capability / `ServerToolEvent`，不得伪装成 P12 Worker、占用本地 worktree 或绕过本地并发预算。
- Team peer message、shared task board、任务指派与状态转移均为 canonical event，可持久化、可重放。
- User Hooks 的 `SubagentStart` / `SubagentStop` / `TaskStarted` / `TaskCompleted` 订阅这些 canonical event；Hook 失败不能篡改编排事实，阻断语义必须由 Policy 明确授权。
- 取消、预算与审批从 Parent 传播到本地 Worker；Provider-hosted 内部 worker 只能通过 Provider cancellation / transcript 观察，不声称具备本地隔离保证。

## 验收标准

- Worker 写操作不直接污染 Parent Workspace
- Parent 可审查 Worker Patch
- 取消 Parent 会取消所有 Worker
- 并发预算可控
- Provider-side Multi-Agent 与本地 AgentTeam 的身份、预算、事件和执行所有权不混用

## 相关文档

- [agent-engine](agent-engine.md) · [git-diff（worktree）](git-diff.md) · [checkpoint](checkpoint.md) · [sessions（子 session）](sessions.md)
- [ROADMAP Phase 12](../../ROADMAP.md)
