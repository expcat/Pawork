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

## 后续增强 / Maintenance Tasks

以下子任务为 P11-4 的后续增强，主任务 P11-4 保持 🟢 已完成，状态不变。各子任务按统一约束标记 🟡未开始；涉及 Windows 实验性 API 的能力事实以 2026-08-09 查询的官方资料为准，不得视为稳定契约。

### P11-4.E1 Classic AppContainer + Job Completion

> 状态：🟡未开始 · 交付成熟度：Designed

**最终目的**：完成已冻结未执行的 AppContainer 真实接入——以 `STARTUPINFOEX + SECURITY_CAPABILITIES` + restricted/AppContainer SID 受限令牌 spawn 子进程，与现有 Job Object 生命周期整合，将 Windows 从 Job-only 软限制升级为进程级文件/网络硬隔离；Secret 路径与网络由 OS 拒绝而非仅靠文档声明。

**涉及范围**：`process-runtime`（受限令牌 spawn 接线）+ `sandbox-runtime`（Windows 后端整合）

**依赖**：P11-4、P11-1.E1（Sandbox Guarantee Model）、[P11-7 进程树清理](P11-7-process-tree-cleanup.md)

**产出物**：

- restricted/AppContainer SID 受限令牌 spawn 路径（STARTUPINFOEX + SECURITY_CAPABILITIES + 最小 capabilities）
- workspace filesystem read/write root 授权生命周期：优先 temporary/reversible ACL / broker / OS-native scoped policy，禁止宽泛永久 ACL；明确 cleanup / crash recovery / permission rollback
- network deny-by-default（不授予 Internet capability）与 Secret boundary（默认拒绝密钥目录访问）
- 与现有 Job Object 生命周期整合及降级观测

**验收标准**：

- [ ] AppContainer 受限令牌 spawn 真实生效；workspace 授权为可撤销/临时 ACL，cleanup、crash recovery 与 permission rollback 可验证
- [ ] 网络 deny-by-default 与 Secret 路径拒绝由 OS 强制，probe 失败原因可见且 fallback 顺序可观测
- [ ] Windows backend 仅在实际获得对应 OS 强制保证后升级为 hard，与 P11-1.E1 guarantee 模型一致

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [安全验收](../docs/quality/security-acceptance.md) · [ADR-031](../docs/adr/ADR-031-sandbox-backend-architecture.md)

### P11-4.E2 Windows CreateProcessInSandbox Experimental Probe

> 状态：🟡未开始 · 交付成熟度：MaintenanceGated（experimental）

**最终目的**：调查 Windows 实验性 API `Experimental_CreateProcessInSandbox` / `Experimental_CreateProcessAsUserInSandbox`（Microsoft Learn 标注 experimental，DLL 为 `processmodel.dll`，无公开 SDK header），评估其能否降低 Pawork 自管 ACL / AppContainer profile / proxy 的复杂度；结论作为 capability-gated 的增强选项，不作产品硬基线，不假定 Win11 一定存在。

**涉及范围**：`sandbox-runtime`（Windows 研究 + probe 设计）

**依赖**：P11-4、P11-4.E1

**产出物**：

- 动态探测方案：`LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_SYSTEM32)` + `GetProcAddress`，探测不可用时不阻塞 classic AppContainer 路径
- fallback 链明确：CreateProcessInSandbox（实验）→ Classic AppContainer + Job → Job-only → NativeRestricted
- FlatBuffer spec 决策记录：API 接受编译后的 FlatBuffer spec（标识 "SBOX"、版本 "0.1.0"），但 Microsoft 未公开 `SandboxSpec.fbs` schema 源——倾向暂不引入 FlatBuffers 依赖，schema 未公开时自实现 encoder 风险高；若评估确需新增依赖，须登记 ROADMAP
- 探测结果与 experimental 状态记录（查询日期 2026-08-09）

**验收标准**：

- [ ] API 可用/不可用两条路径均有明确探测结果与可观测 fallback，不因探测失败影响 classic AppContainer + Job 稳定路径
- [ ] FlatBuffer 引入与否的结论有书面依据；如新增依赖已登记 ROADMAP
- [ ] 文档明确 experimental / capability-gated / MaintenanceGated，不把该 API 表述为稳定契约

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ADR-031](../docs/adr/ADR-031-sandbox-backend-architecture.md) · [ROADMAP](../ROADMAP.md)

### P11-4.E3 Windows Hard Isolation L2

> 状态：🟡未开始 · 交付成熟度：MaintenanceGated

**最终目的**：在真实 Windows 环境完成 L2 验收，确认文件系统/网络隔离由 OS 实际拒绝；只有 filesystem/network 真由 OS 拒绝后，才允许把 Windows backend 从 `degraded` 升级为相应维度的 hard guarantee。

**涉及范围**：`sandbox-runtime`（Windows 测试）

**依赖**：P11-4.E1、P11-4.E2

**产出物**：

- Windows L2 验收用例集：workspace read/write / sibling deny / Secret deny / direct network deny / child spawn limit / Job cleanup / AppContainer child behavior / crash cleanup / temporary permission cleanup
- backend probe-fallback metadata 与 guarantee 升级判定记录

**验收标准**：

- [ ] 上述用例在真实 Windows runner 通过；权限不足时 skip 并显式标记，不以 target compile 冒充运行证明
- [ ] 仅当 filesystem/network 拒绝由 OS 强制执行后，Windows backend 相应维度升级为 hard；其余维度保持 degraded 且可观测
- [ ] 临时 ACL/权限的 crash 后 cleanup 与 rollback 已验证

**相关文档**：[sandbox](../docs/features/sandbox.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [P11-7 进程树清理](P11-7-process-tree-cleanup.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP](../ROADMAP.md)
