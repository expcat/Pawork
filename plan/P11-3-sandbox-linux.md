# P11-3：Linux Bubblewrap

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P11-1

**最终目的**：实现 Linux 硬隔离沙箱后端——以 Bubblewrap（`bwrap`）为主、`landlock`（LSM）为补充，为 Linux 提供容器级文件/网络/进程隔离，在 NativeRestricted 之上叠加对抗性边界。完成后 Linux 达到与容器隔离相当的安全基线。

**涉及范围**：`sandbox-runtime`（Linux 后端）

## 细分步骤

1. **bwrap 命令行生成器（纯函数）** —— 目的：把 `SandboxPolicy` 编译为 bwrap argv（`--ro-bind`/`--bind` 映射 read/write_roots、`--unshare-net` 网络、`--unshare-pid` 进程、`--die-with-parent` 生命周期），可脱离 OS 单测（L0）。
2. **landlock 补充后端** —— 目的：用 `landlock` crate 在进程内设置 LSM 规则（`access_ro/wo`）作为 bwrap 不可用时的文件系统硬隔离兜底；注意 landlock 0.4.x 已支持 direct TCP 端口规则（ABI v4+，Linux 6.7+；UDP 尚未稳定），当前实现尚未启用网络 rules，后续演进见 P11-3.E1。
3. **资源/进程限制** —— 目的：`rlimit`（CPU/内存/fd）+ 可选 cgroup v2；`max_procs` 经 `RLIMIT_NPROC`/prlimit 约束；seccomp 可选收紧系统调用。
4. **可用性探测与回退** —— 目的：探测 `bwrap --version` 可执行性与内核 unshare 支持；失败则尝试 landlock；再失败回退 NativeRestricted；全部记审计。
5. **测试** —— 目的：L0 argv 生成快照；L1 探测程序在 bwrap/landlock 下无法越权读/联网/fork；L2 在 Linux CI 跑（CI 容器需 `--privileged` 或 `SYS_ADMIN`，否则 skip 并标记）。

## 主要产出物

- Linux `bwrap` 后端
- `landlock` 补充后端
- 命令行/profile 生成器 + 探测回退

## 验收标准

- [x] Linux 下 bwrap namespace 隔离与 Landlock 文件系统回退均在真实内核运行通过
- [x] 无 bwrap/landlock 时优雅回退 NativeRestricted 且可观测；只有 Landlock 时标记 `hard_filesystem_only`
- [x] argv/ruleset 编译、deny overlap 与探测选择有 L0/L1 测试

## 验证记录（2026-08-09）

- WSL2 Linux 6.6（`CONFIG_SECURITY_LANDLOCK=y`）运行 musl 测试：`process-runtime` 10/10、`sandbox-runtime` 33/33 通过；Landlock 自定义 `PATH` 仅授权解析后的 executable 文件。
- 临时注入 Ubuntu bwrap 0.9 后再次运行：sandbox 32/32 通过，实际验证 workspace 可读与 sibling 拒绝；测试产物已清理，未把 bwrap 打包进仓库。
- Linux GNU 与 musl 目标编译通过；Landlock ruleset 在父进程构造，child `pre_exec` 只执行 no-new-privileges/restrict-self。

## 后续增强 / Maintenance Tasks

以下子任务为 Phase 11 之后的增量增强（Enhancement），不改变 P11-3 主任务 🟢 已完成状态；实现前需复核内核与 crate 版本事实（查询日期 2026-08-09，experimental 能力不得视为稳定契约）。

### P11-3.E1 Modern Landlock Capability Upgrade（🟡未开始 · Designed）

**最终目的**：让现代 Linux 上 Landlock 成为无外部 executable 的强隔离基础。重核最新 Landlock ABI（filesystem / TCP / UDP / UNIX socket scope / audit / inheritance），并同步核对锁定的 `landlock` crate 0.4.x 实际支持的 ABI（0.4.0=ABI4 TCP，0.4.7=ABI6 UNIX scope + ABI7 audit + ABI9 ResolveUnix，均在 0.4.x 小版本内；UDP v10 尚不在 crate）。比较方案 A 升级 crate 至 0.4.7 / B 极小 FFI / C 暂不采用，依赖选型服从 ROADMAP 基线；若可阻断 direct TCP，更新 "Landlock network always degraded" 长期规划。按运行时 ABI probe（`landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)`），老 ABI 按维度降级并可观测，不兼容非常旧 Linux。

**涉及范围**：`sandbox-runtime` Linux 后端

**依赖**：P11-3、P11-1.E1

**产出物**：A/B/C 选型决策记录（含拒绝理由）；升级至 0.4.7 或极小 FFI 方案；运行时 ABI probe 与按维度（filesystem / TCP / UNIX scope / audit）降级设计；长期规划更新（Landlock 网络能力描述）。

**验收标准**：现代内核（ABI v4+，Linux 6.7+）下 direct TCP deny 与文件系统隔离均由 Landlock 独立达成，无需 bwrap；老内核按维度降级且 metadata 可观测；升级或 FFI 不改变 ROADMAP 依赖基线小版本；查询日期与 experimental 状态已记录。

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ROADMAP 依赖选型：landlock](../ROADMAP.md) · [ADR-031](../docs/adr/ADR-031-sandbox-backend-architecture.md)

### P11-3.E2 Bubblewrap Role Clarification（🟡未开始 · Designed）

**最终目的**：重新定义 Landlock 与 bwrap 的职责边界，消除"同义 backend"误解：Landlock = daemonless / unprivileged / 文件系统 +（目标 ABI 可用时）TCP / UNIX socket scope / IPC scope；bwrap = namespace（mount / PID / 独立 /proc / IPC / UTS / network）+ 更强宿主视图隔离。明确 `SandboxSelector` 如何表达二者差异，并把 "自实现 mini-bwrap / mini-container runtime" 列为默认拒绝（除非未来有充分证据再评估）。

**涉及范围**：`sandbox-runtime` + `docs/features/sandbox.md`

**依赖**：P11-3、P11-3.E1

**产出物**：Landlock vs bwrap 保证维度对照与职责边界说明（写入 sandbox.md）；selector 表达差异的设计；"自实现 mini-bwrap" 默认拒绝的决策记录。

**验收标准**：sandbox.md 与代码注释中两者保证维度描述一致；selector 对 Landlock/bwrap 的能力差异可解释、可观测；拒绝自实现 mini-bwrap / mini-container runtime 的结论明确记录；不引入 OCI 或完整 container runtime 作为前置。

**相关文档**：[sandbox](../docs/features/sandbox.md) · [ADR-031](../docs/adr/ADR-031-sandbox-backend-architecture.md) · [ROADMAP](../ROADMAP.md)

### P11-3.E3 Linux Negative Security Tests（🟡未开始 · MaintenanceGated）

**最终目的**：在真实 Linux 内核上验证负向安全保证：workspace allowed / sibling deny / Secret deny / direct TCP outbound deny / UDP（目标 ABI）/ UNIX socket scope（目标 ABI）/ fork-process limit / child 继承 / process escape cleanup / executable PATH 最小授权 / ABI downgrade metadata。优先使用本地 listener/helper 构造对抗场景，不依赖外网。

**涉及范围**：`sandbox-runtime` Linux 测试

**依赖**：P11-3.E1

**产出物**：负向测试用例清单与实现（本地 listener/helper）；真实内核 runner 接入与平台门禁（MaintenanceGated，缺 SYS_ADMIN/privilege 时 skip 并标记）；ABI downgrade metadata 断言。

**验收标准**：在真实 Linux 内核（如 WSL2 / CI runner）逐项通过，或按运行时 ABI 降级断言；direct TCP deny 与文件系统 deny 确由 OS 层拒绝而非软限制；ABI downgrade metadata 可观测；测试不依赖外网。

**相关文档**：[sandbox](../docs/features/sandbox.md) · [安全验收](../docs/quality/security-acceptance.md) · [测试](../docs/quality/testing.md) · [ROADMAP](../ROADMAP.md)

**相关文档**：[sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [安全验收](../docs/quality/security-acceptance.md) · [ROADMAP 依赖选型：landlock](../ROADMAP.md) · [ROADMAP](../ROADMAP.md)
