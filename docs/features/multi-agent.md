# Multi-Agent 与编排

## 职责

提供 Parent/Worker 多 Agent 编排，隔离写入、传播取消、聚合结果。属 P2 能力，在核心 Coding Agent 稳定前不开发。

## 功能

Parent Agent；Worker Agent；任务分解；子 Session；独立 Worktree；Worker 模型选择；Worker Token 预算；并发上限；结果聚合；Worker 取消；Worker 重试；共享 Artifact；冲突检测；文件锁；状态汇总。

P12 首先交付 `AgentSupervisor` / `TaskGraph` / `AgentTree`，再由 tool 或外部 Client 触发 spawn；禁止把 `spawn_agent` 直接实现为脱离监督的 `tokio::spawn`。最低生命周期为 Created → Admitted → Starting → Running/Waiting → Completed，任何阶段可进入 Cancelling → Cancelled 或 Failed，状态变化均事件化并可重放。

## 调度规则

```text
同一 Worktree 写操作默认串行
不同 Worktree 可以并行
同一 Git Index 操作串行
Worker 不直接修改 Parent Workspace
Parent 决定是否合并 Worker Patch
```

Agent 与账号控制面只通过 `AcquireRequest` / `CredentialLease` 交互。Worker 不读取 API key，不实现 credential rotation；Agent cancel 产生 `LeaseOutcome::Cancelled`，不得降低 account health。所有 AgentInstance、TaskGraph、usage 与 audit 记录带 `tenant_id`，Parent/Worker 不可跨 tenant 委派。

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
- Agent lifecycle、TaskGraph 与 cancellation cascade 可重放；无脱离 Supervisor 的悬挂 Worker
- Worker 只经 CredentialPool lease 使用 Provider，cancel 不污染账号健康

## 相关文档

- [agent-engine](agent-engine.md) · [provider-control-plane](provider-control-plane.md) · [tenant-audit](tenant-audit.md) · [git-diff（worktree）](git-diff.md) · [checkpoint](checkpoint.md) · [sessions（子 session）](sessions.md)
- [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md) · [ROADMAP Phase 12 / Phase 18](../../ROADMAP.md)
