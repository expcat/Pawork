# P11-7：进程树清理

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P4-12

**最终目的**：把 P4-12 的进程树终止从「尽力而为」升级为「三平台一致且 crash 安全」——Windows 用 Job Object 取代临时 `taskkill /T`，确保宿主崩溃/句柄关闭后整树仍被回收；Unix 收敛进程组语义；并提供 chaos 测试验证取消/异常退出后无残留进程。完成后取消命令不再遗留孤儿进程（满足安全验收 #11）。

**涉及范围**：`process-runtime`、`sandbox-runtime`

## 细分步骤

1. **Windows Job Object 完整实现** —— 目的：spawn 时把子进程及其后代绑定到一个 Job Object（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`），用 Job 终止整树，替换现有 `taskkill /T`；保证进程崩溃后 Core 仍能回收整树。
2. **Unix 进程组收敛** —— 目的：确认 `setpgid` + `killpg(-pgid)` 在沙箱后端（bwrap `--unshare-pid`、sandbox-exec）下仍一致；必要时由后端透出 pgid 给 process-runtime 统一 kill。
3. **孤儿进程检测兜底** —— 目的：在进程树终止后扫描（可选，按平台 pid/pgid 关系）确认无残留；检测到孤儿时记审计并尝试清理。
4. **chaos 测试** —— 目的：构造深层子进程树（shell → child → grandchild，含 sleep/僵尸），验证 cancel、timeout、Core 强杀三条路径下整树在限时内终止、无残留。
5. **三平台一致性** —— 目的：统一 `ProcessHandle::kill` 契约（idempotent、限时返回），保证 run_command/沙箱/PTY 共用同一路径。

## 主要产出物

- Windows Job Object 进程树绑定与终止
- 孤儿进程检测（可选）
- chaos 测试（三平台）

## 验收标准

- [ ] 取消/timeout/强杀后整树在限时内终止、无残留（chaos 测试通过，三平台）
- [ ] Windows 宿主崩溃后 Job 仍回收整树（句柄关闭即终止）
- [ ] kill 契约 idempotent 且限时返回

**相关文档**：[process](../docs/features/process.md) · [sandbox](../docs/features/sandbox.md) · [P4-12 Process Runtime](P4-12-process-runtime.md) · [安全验收](../docs/quality/security-acceptance.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)