# P11-2：macOS Sandbox profile

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P11-1

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

- [x] Seatbelt profile 编译 read/write/deny、network Enforce 与 process/resource 语义；不支持的 hostname allowlist/max_procs 有诚实降级说明
- [x] 无 sandbox-exec 或 smoke 失败时优雅回退 NativeRestricted 且可观测
- [x] profile 转义、路径规则、网络规则与 argv 包装有 L0 测试

## 验证记录（2026-08-09）

- `sandbox-runtime` L0 覆盖 Seatbelt 字符串转义、read/write/deny 与网络 profile；spawn 使用 `sandbox-exec -p`，不产生策略临时文件。
- `cargo check -p process-runtime -p sandbox-runtime -p pty-service --target aarch64-apple-darwin` 通过。
- 交付成熟度为 TargetVerified；真实 macOS Seatbelt 越权读/联网/进程 L2 仍须在 macOS runner 执行后才能升级为 MaintenanceGated，不以交叉编译冒充运行证明。

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
