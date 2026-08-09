# ADR-031：沙箱后端分层架构（NativeRestricted 兜底 + 平台原生硬隔离 + 探测回退）

- **状态**：Accepted
- **日期**：2026-08-07
- **落实日期**：2026-08-09

## 背景

Agent 调度的子进程（run_command、MCP stdio、Git、外部工具）具备完整 OS 能力。policy-engine 只决定「是否执行」，process-runtime 只提供进程启停，二者都不构成执行隔离——一个被批准执行的 sh -c "cat ~/.ssh/id_rsa" 仍可越权读 Secret。需要一层执行隔离边界，覆盖文件/网络/进程/环境/Secret，且要同时满足：

1. 三平台一致（Linux/macOS/Windows）；
2. 无平台原生沙箱可用时也不能拒绝运行（开发机/受限 CI 常缺 bwrap/AppContainer）；
3. 对抗性场景（不可信代码）需要真正的硬隔离。

单一后端无法同时满足：纯软沙箱（env/路径白名单）挡不住已授权命令内部越权；硬隔离后端（bwrap/sandbox-exec/AppContainer）依赖平台能力且常不可用。

## 决策

采用分层后端 + 探测回退架构，封装在 `sandbox-runtime` crate：

1. 统一 trait：所有后端实现 SandboxBackend（id / available / spawn(SandboxProcessSpec, SandboxPolicy)），调用方只感知 policy 到 spawn，平台差异封装在后端内部。
2. NativeRestricted 永远可用：纯 Rust 软沙箱（env 清洗、cwd 锁定、Secret 目录拒绝、rlimit/输出上限），作为基线与兜底，不依赖任何平台特性。
3. 平台原生硬隔离后端叠加：Linux bwrap/landlock、macOS sandbox-exec、Windows AppContainer/Job Object，在 NativeRestricted 之上叠加，提供对抗性文件/网络/进程隔离。
4. SandboxSelector 探测与回退：执行最小真实 smoke 并缓存结果，失败自动回退到下一档；`BackendSelection` 必须携带实际隔离等级、全部尝试与失败原因，绝不静默降级。
5. 进程树一致性：所有后端的进程树终止复用 `process-runtime` 统一路径（Unix `killpg(pgid)` / Windows Job Object），PTY 通过公开的 `ProcessTreeGuard` 接入，不另起实现。
6. 能力按实际生效情况分级：`soft`、`hard`、`hard_filesystem_only`、`degraded`。调用方不得因“选择了某平台后端”就推断文件、网络、进程三类保证都已具备。

集成：policy-engine 的 ExecutionConstraints 归一化为 SandboxPolicy.resources；ToolCapability::Network 映射 network.mode；run_command 经 SandboxBackend::spawn 执行。

## 平台落地边界

- Linux：bwrap 提供 namespace 级文件/网络/进程隔离；不可用时 Landlock 提供文件系统硬隔离，网络明确降级。
- macOS：Seatbelt profile 与 `sandbox-exec` backend 已实现；真实隔离证明由 macOS L2 runner 提供。
- Windows：Job Object 的 suspended-attach、资源限额与 `KILL_ON_JOB_CLOSE` 已落地。AppContainer policy/capability 生成与探测接口已冻结，但受限令牌 spawn 尚未接入；当前选择器固定报告 Job-only `degraded`，不得声称文件/网络已硬隔离。后续 AppContainer 接线必须保持统一 Process Runtime 生命周期，并避免不可撤销的宽泛 ACL。
- Docker/Podman（P11-5）维持归档，不进入选择链。

## 后果

- NativeRestricted 满足「未信任工作区默认限制」与三平台基础可用性的最低安全基线，纯 Rust、无外部 sandbox executable 依赖。
- 安全差距显式化：仅有 NativeRestricted 时，对抗性越权（已授权命令读 Secret）无法阻断——这是软沙箱的固有局限，必须在文档/告警说明，并以补齐硬隔离（P11-2/3/4）为 Phase 11 核心目标。
- 探测结果影响安全姿态，须进入 Tool metadata、tracing，并在审计层接线后纳入诊断包。
- 容器/VM（P11-5）作为更强一档延后。

## 相关

- [sandbox](../features/sandbox.md) · [process](../features/process.md) · [policy](../features/policy.md)
- [ADR-009 默认 Workspace Trust](ADR-009-default-workspace-trust.md) · [ADR-012 WASM capability](ADR-012-wasm-first-plugin.md) · [ADR-020 安全是发布门槛](ADR-020-performance-security-gate.md)
- [ROADMAP Phase 11](../../ROADMAP.md)
