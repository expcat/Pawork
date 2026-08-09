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

## 后续增强 / Maintenance Tasks

> 以下为后续增强任务（child task，ID 形如 `P11-{n}.E{k}`，状态符号一律 `🟡未开始`，交付成熟度 `Designed`），不改变 P11-2 主任务 🟢 状态。macOS App Sandbox 等随系统演进的能力记录查询日期 2026-08-09，避免把实验/演进中能力误认为稳定契约。

### P11-2.E1 macOS Real L2 Verification

状态：🟡未开始 · 交付成熟度：Designed（平台门禁：MaintenanceGated） · 依赖：P11-2、macOS CI/runner

- **最终目的**：在真实 macOS runner 上验证 Seatbelt 后端的 L2 语义，将交付成熟度从 TargetVerified 升级为 MaintenanceGated；不以交叉编译冒充运行证明。
- **涉及范围**：`sandbox-runtime` macOS 测试。
- **依赖**：P11-2；macOS CI/runner（平台门禁）。
- **产出物**：真实 macOS 环境可重复运行的 L2 验证用例；验证通过记录；backend 交付成熟度升级。
- **验收标准**：allowed workspace / sibling deny / Secret deny / network deny / fork 限制 / timeout-cancel / 进程树清理 / probe-fallback / sandbox metadata 均在真实 macOS 环境验证通过；target compile 不作为运行证明。
- **相关文档**：[sandbox](../docs/features/sandbox.md) · [P11-2 主任务](P11-2-sandbox-macos.md)

### P11-2.E2 Desktop App Sandbox / XPC Feasibility（研究任务）

状态：🟡未开始 · 交付成熟度：Designed · 依赖：Phase 19、P11-2

- **最终目的**：结合 Phase 19 Desktop 方向，调查 App Sandbox / sandboxed helper / XPC service / security-scoped workspace 是否适合 Desktop 长期隔离，明确采用或拒绝及理由。
- **涉及范围**：`docs/features/desktop-gui.md` 研究 + ADR 交叉引用；不改 Phase 11 CLI backend。
- **原则**：CLI/Core 不依赖 Desktop App Sandbox；GUI 不成 Core Sandbox 前置；Desktop helper 仍消费 Core policy；不复制第二套 policy model。
- **依赖**：Phase 19、P11-2。
- **产出物**：desktop-gui.md 中的可行性研究章节与 ADR 交叉引用记录（查询日期 2026-08-09）。
- **验收标准**：平台事实准确记录——security-scoped bookmarks（implicit，需 entitlement `files.bookmarks.app-scope`）传 workspace 访问；XPC helper 不自动继承 sandbox extension；明确 Desktop App Sandbox/XPC 只能是额外 host-level defense，不替代 Phase 11 CLI Seatbelt backend。
- **相关文档**：[desktop-gui](../docs/features/desktop-gui.md) · [ADR-031](../docs/adr/ADR-031-sandbox-backend-architecture.md) · [ADR-034](../docs/adr/ADR-034-desktop-gui-client-boundary.md) · [P11-2 主任务](P11-2-sandbox-macos.md)

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
