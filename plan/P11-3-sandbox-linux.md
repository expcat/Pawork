# P11-3：Linux Bubblewrap

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P11-1

**最终目的**：实现 Linux 硬隔离沙箱后端——以 Bubblewrap（`bwrap`）为主、`landlock`（LSM）为补充，为 Linux 提供容器级文件/网络/进程隔离，在 NativeRestricted 之上叠加对抗性边界。完成后 Linux 达到与容器隔离相当的安全基线。

**涉及范围**：`sandbox-runtime`（Linux 后端）

## 细分步骤

1. **bwrap 命令行生成器（纯函数）** —— 目的：把 `SandboxPolicy` 编译为 bwrap argv（`--ro-bind`/`--bind` 映射 read/write_roots、`--unshare-net` 网络、`--unshare-pid` 进程、`--die-with-parent` 生命周期），可脱离 OS 单测（L0）。
2. **landlock 补充后端** —— 目的：用 `landlock` crate 在进程内设置 LSM 规则（`access_ro/wo`）作为 bwrap 不可用时的文件系统硬隔离兜底；注意 landlock 无法控网络，需与网络策略降级配合。
3. **资源/进程限制** —— 目的：`rlimit`（CPU/内存/fd）+ 可选 cgroup v2；`max_procs` 经 `RLIMIT_NPROC`/prlimit 约束；seccomp 可选收紧系统调用。
4. **可用性探测与回退** —— 目的：探测 `bwrap --version` 可执行性与内核 unshare 支持；失败则尝试 landlock；再失败回退 NativeRestricted；全部记审计。
5. **测试** —— 目的：L0 argv 生成快照；L1 探测程序在 bwrap/landlock 下无法越权读/联网/fork；L2 在 Linux CI 跑（CI 容器需 `--privileged` 或 `SYS_ADMIN`，否则 skip 并标记）。

## 主要产出物

- Linux `bwrap` 后端
- `landlock` 补充后端
- 命令行/profile 生成器 + 探测回退

## 验收标准

- [ ] Linux 下 bwrap 沙箱限制生效（或 landlock 文件系统隔离生效）
- [ ] 无 bwrap/landlock 时优雅回退 NativeRestricted 且可观测
- [ ] argv 生成有 L0 快照测试

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP 依赖选型：landlock](../ROADMAP.md) · [ROADMAP](../ROADMAP.md)