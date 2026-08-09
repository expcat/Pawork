# P11-7：进程树清理

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P4-12

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

- [x] 取消/timeout/强杀后整树在 5 秒契约内终止；Windows 与 Linux 深层后代回归通过
- [x] Windows 子进程在执行前绑定带 `KILL_ON_JOB_CLOSE` 的 Job，宿主句柄关闭即回收整树
- [x] `ProcessHandle::kill` 幂等且限时返回，run_command/Sandbox/PTY 共用 `ProcessTreeGuard`

## 验证记录（2026-08-09）

- Windows 原生覆盖 suspended attach、child/grandchild、timeout/cancel、重复 kill 与 PTY descendant cleanup；外部 PID 绑定会收编绑定窗口内已产生的后代，PTY 清理压力验证 30/30。
- Linux WSL/musl 10/10 覆盖 process group 深层后代、真实 `setsid` 离组逃逸与 PTY session tree cleanup；终止路径用 start-time 排除 PID 复用，冻结 process group 后递归冻结/终止 `/proc` 后代。`killpg` 使用正的 pgid（API 自身表达进程组）。
- macOS process/PTY 路径通过 aarch64 target 编译；真实 macOS chaos 属 MaintenanceGated 平台门禁。

**相关文档**：[process](../docs/features/process.md) · [sandbox](../docs/features/sandbox.md) · [P4-12 Process Runtime](P4-12-process-runtime.md) · [安全验收](../docs/quality/security-acceptance.md) · [测试体系](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)
