# Sandbox Runtime

## 职责

为 Agent 调度的工具与子进程提供**受控执行环境**：按 capability 约束文件、网络、进程、环境变量与 Secret 访问，保证未授权行为（越权读 `~/.ssh`、外联网络、fork 炸弹、读取剪贴板/Keychain 等）在**执行层**被阻断或受限，而不只是停留在策略层。

与 [policy-engine](policy.md)（决定「是否执行」）和 [process-runtime](process.md)（提供进程启停/IO/取消）正交：sandbox-runtime 在两者之上叠加**执行隔离**，把「允许执行」细化为「以何种隔离边界执行」。

## 设计要点

- **分层后端**：`NativeRestricted`（纯 Rust 软沙箱，永远可用）→ 平台原生硬隔离（Linux bwrap/landlock、macOS sandbox-exec、Windows AppContainer/Job Object）→ 容器/VM（推迟）。后者在前者之上叠加，不替换。
- **探测与回退**：启动时探测平台原生后端可用性（可执行文件存在、API 权限），结果缓存并写入诊断/审计；不可用时**自动回退**到 NativeRestricted，绝不因沙箱缺失而拒绝运行，也不静默降级（回退必须可观测）。
- **trait 统一**：所有后端实现 `SandboxBackend`，调用方只感知 `SandboxPolicy → spawn`，平台差异封装在后端内部。
- **进程树一致性**：所有后端的进程树终止复用 `process-runtime` 的统一路径（Unix `killpg(-pgid)` / Windows Job Object），沙箱后端不另起 kill 实现。
- **默认安全**：未信任工作区 + 未显式配置时，策略为**最小权限**（仅 workspace 只读、禁网络、禁越权 Secret）。

## 数据模型

### SandboxBackend trait

```rust
#[async_trait]
pub trait SandboxBackend: Send + Sync {
    /// 后端标识（如 "native_restricted"、"bwrap"、"sandbox_exec"、"appcontainer"）。
    fn id(&self) -> &'static str;

    /// 该后端在当前系统是否可用（探测结果，不应在 spawn 路径上重算）。
    fn available(&self) -> bool;

    /// 按策略 spawn 一个受控进程；返回的句柄可读流、kill 整树。
    async fn spawn(
        &self,
        spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError>;
}
```

### SandboxPolicy

声明式策略，后端据此构造平台原生约束。**最终语义以后端实际能力为准**（NativeRestricted 无法保证网络硬隔离时，`network.mode` 退化为 `Hint`）。

```text
SandboxPolicy {
  filesystem:
    read_roots:  [workspace..., 只读依赖目录]
    write_roots: [workspace..., 临时目录]
    deny:        [~/.ssh, ~/.aws, .git?, /etc, %APPDATA%\.. 密钥目录]
  network:
    mode:        Off | Hint | Enforce     # 默认 Enforce（禁出站）；NativeRestricted 仅 Hint
    allow_hosts: [...]                    # mode=Enforce 时的白名单
  process:
    allow_spawn: bool
    max_procs:   u32                      # 防止 fork 炸弹
  environment:
    allowlist:   [PATH, HOME, LANG, ...]
    denylist:    [*TOKEN*, *KEY*, *SECRET*]  # 前缀/子串匹配，覆盖 allowlist
  resources:
    cpu_seconds, memory_mb, open_fds, wall_time
  secrets:   DenyAll                      # 永远默认拒绝 Keychain/凭据访问
  clipboard: Deny
  browser:   Deny
}
```

### SandboxProcessSpec

在 `process_runtime::CommandSpec` 之上增加沙箱注解，供后端决定绑定方式：

```rust
pub struct SandboxProcessSpec {
    pub command: CommandSpec,            // 复用 program/args/cwd/env/timeout/max_output
    pub workspace_roots: Vec<PathBuf>,   // 文件系统隔离的根
    pub needs_network: bool,             // 工具声明（Network capability）
}
```

### SandboxProcess

复用 `process_runtime::ProcessEvent`（Stdout/Stderr/Exit）与进程树句柄；沙箱后端负责把受限进程的 IO 桥接到该事件流，调用方 API 与 `ProcessRuntime::spawn_stream` 一致。

## 后端优先级与选择

| 优先级 | 后端 | 隔离强度 | MVP |
| --- | --- | --- | --- |
| P0 | `NativeRestricted` | 软（env 清洗 + cwd/路径白名单 + rlimit + 进程组） | ✅ 必须 |
| P1 | Linux `Bubblewrap`（bwrap）/ `landlock` | 硬（mount namespace / LSM） | MVP 后优先补 |
| P1 | macOS `sandbox-exec`（Seatbelt） | 硬（系统级 profile） | MVP 后优先补 |
| P1 | Windows `AppContainer` + `Job Object` | 硬（AppContainer SID + Capabilities + Job 限额） | MVP 后优先补 |
| P2 | Docker / Podman | 容器级 | 推迟（P11-5） |
| P2 | 轻量 VM / 远程 Sandbox / 用户定义 Provider | 强隔离 | 推迟 |

**选择器**（`SandboxSelector::pick()`）按平台尝试硬隔离，失败回退 NativeRestricted：

```text
Linux:   bwrap 可执行 + 内核支持 → bwrap；否则 landlock 可用 → landlock；否则 NativeRestricted
macOS:   sandbox-exec 可执行 → sandbox-exec；否则 NativeRestricted
Windows: AppContainer API 可用 → AppContainer(含 Job Object)；否则 Job Object-only；否则 NativeRestricted
```

## 三平台技术选型

| 维度 | Linux | macOS | Windows |
| --- | --- | --- | --- |
| 文件系统 | bwrap bind-mount（只读/读写 root）+ landlock `access_ro/wo` 兜底 | sandbox-exec `file-read*` / `file-write*` | AppContainer SID + 受限令牌 + Job Object；路径经 broker 授予 |
| 网络 | bwrap `--unshare-net`；landlock 无法控网络 | sandbox-exec `(deny network*)` / `(allow network-outbound ...)` | AppContainer 不授予 `Internet` capability；WFP/防火墙策略兜底 |
| 进程 | bwrap `--unshare-pid` + seccomp | sandbox-exec 限制 `process-fork` / `signal` | Job Object `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` |
| 资源 | `rlimit` + cgroup v2（可选） | `rlimit` | Job Object `JOB_OBJECT_LIMIT_*`（CPU/memory/fd） |
| 进程树清理 | `killpg(-pgid)` | `killpg(-pgid)` | Job Object：进程退出/句柄关闭即整树终止（crash 安全） |

## 与现有层集成

1. **policy-engine → sandbox-runtime**：`PolicyDecision::AllowWithConstraints { ExecutionConstraints }`（timeout_ms / max_output_bytes）映射为 `SandboxPolicy.resources.wall_time` 与输出上限；`ToolCapability::Network` 映射 `network.mode`；未信任工作区映射最小权限策略。新增 `From<ExecutionConstraints>` 的归一化函数。
2. **sandbox-runtime → process-runtime**：后端在内部调用 process-runtime 的 spawn 与 IO 收集，确保 `killpg`/Job 一致、超大输出/timeout/cancel 行为与 `run_command` 既有语义不回退。
3. **builtin-tools（run_command）**：改经 `SandboxBackend::spawn` 而非直接 `ProcessRuntime`；env 白名单逻辑下沉为 `SandboxPolicy.environment`。其他带副作用的工具（apply_patch 写盘、search 遍历）复用 `SandboxPolicy.filesystem` 做路径收敛。

### Phase 15–17 执行所有权

Sandbox 只对 `ExecutionOwner::Core` 的本地执行给出隔离保证，包括 ClientFunction、Command Hook、LSP language server、本地 MCP、Local/Playwright Browser/Computer 与 Agent Worker 子进程。`ProviderHosted` / `ProviderExtension` 在外部 trust boundary 执行，Core 只做 Policy、审批、审计与 transcript 归一，**不得**将其标记为经过本地 `SandboxBackend`。跨边界 fallback 必须重新审批并在事件中记录实际 owner / backend / isolation level。

## 测试分层

- **L0 单元（三平台共享，无 OS 依赖）**：`SandboxPolicy` 构造/合并/序列化；各后端的 profile/profile 文本/命令行生成（纯函数）；`From<ExecutionConstraints>` 归一化。
- **L1 契约（平台无关）**：`SandboxBackend` trait 的统一 contract test——给定策略，运行一个「探测程序」，断言其：无法读 `deny` 路径、无法联网（Enforce 时）、无法超过 `max_procs`、env 中 Secret 已清除。探测程序自身是平台无关的小二进制。
- **L2 平台（真实 OS）**：每个硬隔离后端在对应平台 CI 上跑隔离测试；探测失败（如 CI 无 bwrap/AppContainer 权限）则 skip 并在报告中标记，不判失败。
- **L3 chaos（P11-7）**：取消/宿主 crash 后整树无残留（Windows 验证 Job Object 在进程崩溃后仍清理后代）。

## 验收标准

- [ ] 未信任工作区默认走最小权限策略（满足 MVP 验收 #14）
- [ ] NativeRestricted 永远可用：无 bwrap/sandbox-exec/AppContainer 时仍能运行命令并施加软限制
- [ ] 各硬隔离后端可用时，「探测程序」无法越权读 Secret 路径 / 联网 / fork 炸弹（满足安全验收 #15 Sandbox 逃逸测试）
- [ ] 回退可观测：后端选择/回退写入审计与诊断包
- [ ] 取消能清理整树（三平台，含 Windows Job Object crash 路径，满足安全验收 #11）
- [ ] L0/L1 在三平台 CI 通过；L2 在具备能力的平台 CI 通过、其余 skip 可见
- [ ] 只有 Core-owned 进程可声明本地 sandbox 级别；hosted/extension 事件不会伪造本地隔离证明

## MVP 边界与安全差距

- **MVP 必须**：`NativeRestricted`（软沙箱）——它是「未信任工作区默认限制」与「三平台子进程测试」的最低安全基线，纯 Rust、无外部依赖、永远可用。
- **MVP 后优先**：Linux bwrap/landlock、macOS sandbox-exec、Windows AppContainer/Job Object——**硬隔离**。在仅有 NativeRestricted 时，一个被批准执行的 `sh -c "cat ~/.ssh/id_rsa"` 仍能越权读 Secret：软沙箱挡不住已授权命令内部的越权行为。这是 NativeRestricted 的固有局限，必须在文档/告警中显式说明，并以补齐硬隔离为 Phase 11 的核心目标。
- **推迟**：Docker/Podman（P11-5）、轻量 VM、远程 Sandbox。

> 安全判断：`NativeRestricted` 提供的是**纵深防御的第一层（降低误伤与意外）**，不是**对抗性隔离边界**。真正的「不可信代码执行」必须等待硬隔离后端或容器/VM。

## 相关文档

- [process](process.md) · [policy](policy.md) · [tools](tools.md) · [plugins（capability）](plugins.md)
- [安全验收](../quality/security-acceptance.md) · [性能目标](../quality/performance-targets.md)
- [ADR-031 沙箱后端架构](../adr/ADR-031-sandbox-backend-architecture.md)
- [ROADMAP Phase 11](../../ROADMAP.md)
