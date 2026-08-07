# P11-2：macOS Sandbox profile

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P11-1

**最终目的**：实现 macOS `sandbox-exec`（Seatbelt）硬隔离后端，为 macOS 提供系统级命令沙箱——在 NativeRestricted 软沙箱之上叠加真正的文件/网络/进程隔离，使被批准执行的命令也无法越权读 `~/.ssh`、外联网络、无限 fork。完成后 macOS 达到对抗性隔离基线。

**涉及范围**：`sandbox-runtime`（macOS 后端 + profile 生成）

## 细分步骤

1. **sandbox profile 生成器（纯函数）** —— 目的：把 `SandboxPolicy` 编译为 Seatbelt profile 文本（`(version …)`/`(allow …)`/`(deny …)` s-expression），可脱离 OS 单测（L0）。
2. **sandbox-exec 调用封装** —— 目的：以 `sandbox-exec -p <profile> -- <command>` spawn，复用 process-runtime 的进程组/IO/cancel；profile 经临时文件或 `-p` 传入，不落盘含密内容。
3. **路径/网络/进程策略映射** —— 目的：`filesystem.read_roots/write_roots/deny` → `file-read*`/`file-write*`/`(deny file*)`；`network.mode=Enforce` → `(deny network*)`（或按 allow_hosts 放行）；`process.max_procs` → `process-fork`/`signal` 限制；`secrets`/`clipboard`/`browser` → 显式 deny。
4. **可用性探测与回退** —— 目的：检测 `/usr/bin/sandbox-exec` 存在且可执行；不可用时 `SandboxSelector` 回退 NativeRestricted，并记审计（见 sandbox.md「探测与回退」）。
5. **测试** —— 目的：L0 profile 文本快照；L1 探测程序在 sandbox-exec 下无法越权读/联网/fork；L2 在 macOS CI 跑，CI 无权限则 skip。

## 主要产出物

- macOS `sandbox-exec` 后端
- profile 生成器 + Seatbelt profile 模板
- 可用性探测与回退

## 验收标准

- [ ] macOS 下沙箱限制生效：探测程序无法读 deny 路径、无法联网（Enforce）、无法 fork 炸弹
- [ ] 无 sandbox-exec 时优雅回退 NativeRestricted 且可观测
- [ ] profile 生成有 L0 快照测试

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)