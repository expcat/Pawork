# P11-4：Windows AppContainer / Job Object

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟢已完成 · 交付成熟度：TargetVerified（Job-only 降级） · 依赖：P11-1、P11-7

**最终目的**：实现 Windows 硬隔离沙箱后端——以 AppContainer（受限令牌 + Capabilities SID）提供进程级文件/网络/能力隔离，以 Job Object 提供资源限额与整树生命周期控制，在 NativeRestricted 之上叠加对抗性边界。完成后 Windows 达到进程级隔离基线。

**涉及范围**：`sandbox-runtime`（Windows 后端）

## 细分步骤

1. **AppContainer 封装** —— 目的：经 `windows` crate 创建 AppContainer profile/SID + 受限令牌 spawn 子进程；按 `SandboxPolicy` 授予最小 Capabilities（默认不授予 `Internet`，实现网络隔离）；路径访问经 broker/显式 ACL 授予 read/write_roots。
2. **Job Object 资源与进程控制** —— 目的：用 Job Object 绑定子进程树，设置 `JOB_OBJECT_LIMIT_*`（CPU/memory/fd）与 `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`（防 fork 炸弹）；复用 P11-7 的进程树清理（句柄关闭即整树终止）。
3. **能力/路径限制映射** —— 目的：`filesystem` → AppContainer 隔离 + ACL；`network` → 不授予 Internet capability（WFP 兜底）；`process.max_procs` → Job active process limit；`secrets` → 默认拒绝对 `%APPDATA%` 密钥目录的访问。
4. **可用性探测与回退** —— 目的：探测 AppContainer API 可用性（权限/版本）；不可用时降级为 Job Object-only；再不可用回退 NativeRestricted；记审计。
5. **测试** —— 目的：L0 令牌/Job 配置生成；L1 探测程序在 AppContainer 下无法越权读/联网/fork；L2 在 Windows CI 跑，权限不足则 skip 并标记。

## 主要产出物

- Windows AppContainer + Job Object 后端
- 受限令牌与 ACL 授予逻辑
- 探测回退

## 验收标准

- [x] AppContainer policy/capability/Job 配置生成与 API 探测已冻结；受限令牌 spawn 不可用时不虚报文件/网络硬隔离
- [x] Job Object CPU、memory、active-process limit 与 `KILL_ON_JOB_CLOSE` 生效；子进程 suspended 创建、绑定成功后再恢复
- [x] 当前 Windows 选择 Job Object-only，结构化报告 `degraded`；AppContainer 探测失败原因可见

## 交付边界与验证记录（2026-08-09）

- Windows 原生 `process-runtime` 8 tests、`sandbox-runtime` 29 tests、`pty-service` 11 tests、`builtin-tools` 46 tests 通过，含深层后代清理、kill 幂等、PTY 后代回收、资源默认/上界与 run_command metadata；PTY 后代清理另做 30/30 重复压力验证。
- 本任务按既定 fallback 契约交付 Job-only 路径。`STARTUPINFOEX + SECURITY_CAPABILITIES` 受限令牌及可撤销 workspace ACL/broker 尚未接入，因此 Secret 路径与网络仍是软限制；文档、probe 与 `IsolationLevel::Degraded` 均明确该边界。
- 只有补齐 AppContainer 真实 spawn 并在 Windows L2 验证越权读/联网拒绝后，才能把该后端升级为 `hard` / MaintenanceGated。

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [P11-7 进程树清理](P11-7-process-tree-cleanup.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
