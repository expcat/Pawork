# P11-4：Windows AppContainer / Job Object

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟡未开始 · 依赖：P11-1、P11-7

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

- [ ] Windows 下 AppContainer 沙箱限制生效：探测程序无法越权读 Secret 路径、联网、fork 炸弹
- [ ] Job Object 限额生效，且句柄关闭/进程崩溃后整树清理（与 P11-7 一致）
- [ ] 无 AppContainer 时优雅降级（Job Object-only 或 NativeRestricted）且可观测

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [P11-7 进程树清理](P11-7-process-tree-cleanup.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)