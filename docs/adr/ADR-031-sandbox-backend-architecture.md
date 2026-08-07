# ADR-031：沙箱后端分层架构（NativeRestricted 兜底 + 平台原生硬隔离 + 探测回退）

- **状态**：Proposed
- **日期**：2026-08-07

## 背景

Agent 调度的子进程（run_command、MCP stdio、Git、外部工具）具备完整 OS 能力。policy-engine 只决定「是否执行」，process-runtime 只提供进程启停，二者都不构成执行隔离——一个被批准执行的 sh -c "cat ~/.ssh/id_rsa" 仍可越权读 Secret。需要一层执行隔离边界，覆盖文件/网络/进程/环境/Secret，且要同时满足：

1. 三平台一致（Linux/macOS/Windows）；
2. 无平台原生沙箱可用时也不能拒绝运行（开发机/受限 CI 常缺 bwrap/AppContainer）；
3. 对抗性场景（不可信代码）需要真正的硬隔离。

单一后端无法同时满足：纯软沙箱（env/路径白名单）挡不住已授权命令内部越权；硬隔离后端（bwrap/sandbox-exec/AppContainer）依赖平台能力且常不可用。

## 决策

采用分层后端 + 探测回退架构，封装在 sandbox-runtime crate：

1. 统一 trait：所有后端实现 SandboxBackend（id / available / spawn(SandboxProcessSpec, SandboxPolicy)），调用方只感知 policy 到 spawn，平台差异封装在后端内部。
2. NativeRestricted 永远可用：纯 Rust 软沙箱（env 清洗、cwd 锁定、Secret 目录拒绝、rlimit/输出上限），作为基线与兜底，不依赖任何平台特性。
3. 平台原生硬隔离后端叠加：Linux bwrap/landlock、macOS sandbox-exec、Windows AppContainer/Job Object，在 NativeRestricted 之上叠加，提供对抗性文件/网络/进程隔离。
4. SandboxSelector 探测与回退：启动时探测平台后端可用性（可执行文件/API 权限），失败自动回退到下一档，最终回到 NativeRestricted；回退必须可观测（写入审计/诊断），绝不静默降级。
5. 进程树一致性：所有后端的进程树终止复用 process-runtime 统一路径（Unix killpg / Windows Job Object），不另起实现。

集成：policy-engine 的 ExecutionConstraints 归一化为 SandboxPolicy.resources；ToolCapability::Network 映射 network.mode；run_command 经 SandboxBackend::spawn 执行。

## 后果

- MVP 只需 NativeRestricted：满足「未信任工作区默认限制」与「三平台子进程测试」的最低安全基线，纯 Rust、无外部依赖。
- 安全差距显式化：仅有 NativeRestricted 时，对抗性越权（已授权命令读 Secret）无法阻断——这是软沙箱的固有局限，必须在文档/告警说明，并以补齐硬隔离（P11-2/3/4）为 Phase 11 核心目标。
- 探测结果影响安全姿态，须纳入审计与诊断包。
- 容器/VM（P11-5）作为更强一档延后。

## 相关

- [sandbox](../features/sandbox.md) · [process](../features/process.md) · [policy](../features/policy.md)
- [ADR-009 默认 Workspace Trust](ADR-009-default-workspace-trust.md) · [ADR-012 WASM capability](ADR-012-wasm-first-plugin.md) · [ADR-020 安全是发布门槛](ADR-020-performance-security-gate.md)
- [ROADMAP Phase 11](../../ROADMAP.md)
