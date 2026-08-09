# Sandbox Runtime

## 职责

`sandbox-runtime` 位于 [policy-engine](policy.md) 与 [process-runtime](process.md) 之间：Policy 决定「是否允许执行」，Sandbox 决定「以什么隔离等级执行」，Process 负责 spawn、IO、取消和进程树生命周期。Sandbox 只为 Core-owned 本地进程提供保证，不给 ProviderHosted / ProviderExtension 外部执行伪造本地隔离证明。

## 统一契约

所有后端实现 `SandboxBackend`，接收 `SandboxProcessSpec + SandboxPolicy` 并返回与 `ProcessRuntime::spawn_stream` 一致的事件流和 kill 句柄。策略包含：

- 文件系统 `read_roots` / `write_roots` / `deny`；
- 网络 `Off` / `Hint` / `Enforce` 与可选 host allowlist；
- spawn 与最大进程数；
- `env_clear`、环境白名单与 Secret denylist；
- CPU、内存、fd、wall time 与输出预算。

`ExecutionConstraints` 的 timeout/output budget 可直接归一化为 `SandboxPolicy` 资源基线，workspace、capability 与 trust posture 再由调用方收紧。未信任工作区默认只读、禁 spawn、清洗环境并要求禁网；`run_command` 本身也不允许在未信任工作区静默执行。

## 后端与真实保证

| 平台/后端 | 文件系统 | 网络 | 进程与资源 | 对外隔离等级 |
| --- | --- | --- | --- | --- |
| `NativeRestricted`（全平台） | 只校验 cwd/授权根并清洗环境；不能拦截已启动命令内部的任意文件访问 | 仅提示，不能硬阻断 | Process Runtime 资源限额与整树清理 | `soft` |
| Linux `bwrap` | workspace/读根只读 bind，写根可写 bind，deny 子树以空 tmpfs 覆盖 | `Enforce` 使用独立 network namespace；当前不实现按 hostname 放行 | PID/IPC/UTS/cgroup namespace + Unix rlimit + die-with-parent | `hard` |
| Linux Landlock | ruleset 白名单在 child `pre_exec` 生效；不能表达 allow 根内再减 deny 子树时拒绝启动 | 不支持 | Unix rlimit + process group + `/proc` 后代清理 | `hard_filesystem_only` |
| macOS `sandbox-exec` | Seatbelt `file-read*` / `file-write*` / deny profile | `Enforce` 全部拒绝；hostname allowlist 不安全解析时保持拒绝 | Seatbelt + Unix rlimit/process group | `hard` |
| Windows Job-only | 路径校验仍是软限制；无 AppContainer 文件边界 | 无硬网络隔离 | suspended spawn 后先绑定 Job；CPU、memory、active-process limit 与 `KILL_ON_JOB_CLOSE` | `degraded` |

`NativeRestricted` 是降低误伤的纵深防御，不是对抗性边界。例如获准执行的 shell 仍可能读取 workspace 外文件；调用方必须根据 `BackendSelection.isolation` 判断是否允许运行不可信代码。

Windows 已交付 AppContainer policy/capability 配置生成器与真实能力探测槽位，但当前 `process-runtime` 尚未提供 `STARTUPINFOEX + SECURITY_CAPABILITIES` 受限令牌 spawn，选择器因此明确选用 Job-only 并报告 `degraded`。它不声称 Secret 路径或网络已被硬隔离。AppContainer 后续接线不得通过宽泛永久 ACL 授权 workspace；需要可撤销 broker/ACL 生命周期设计。

Docker/Podman（P11-5）保持 P1 归档，不属于本次运行时选择链。

## 探测与选择

平台探测执行最小真实 smoke，而不只检查文件名：

```text
Linux:   bwrap namespace smoke → Landlock 空 ruleset restrict-self → NativeRestricted
macOS:   sandbox-exec 执行 /usr/bin/true → NativeRestricted
Windows: AppContainer spawn 能力（当前不可用）→ Job Object-only
```

探测结果在进程内缓存。`BackendSelection` 记录最终 backend、`isolation`、是否 fallback、说明和全部 `attempted` 结果；`run_command` 把这些字段写入 Tool Result metadata，降级同时通过 tracing 可见。没有硬后端时不会静默把软限制标成 hard。

## 平台实现要点

### Linux

- bwrap 默认只读暴露 workspace 与显式 read roots，仅 write roots 用 `--bind`；使用 `--unshare-net/pid/ipc/uts`、`--die-with-parent` 与 `--new-session`。
- Landlock ruleset FD 在父进程准备，child 的 fork 后路径只做 async-signal-safe 限制调用；bwrap 不可用时提供文件系统硬隔离回退。命令查找只把解析后的具体 executable 加入只读规则，不暴露宿主 `PATH` 目录整树。
- CPU/memory/fd/max-process 由 `RLIMIT_CPU/AS/NOFILE/NPROC` 施加。

### macOS

- profile 由纯函数生成并通过 `sandbox-exec -p` 传入，不写含策略内容的临时文件。
- write root 同时获得 read/write；deny 规则后置覆盖；`max_procs` 依赖 `RLIMIT_NPROC`，不伪装成 Seatbelt 原生能力。

### Windows

- Process Runtime 以 `CREATE_SUSPENDED` 创建进程，Job attach 成功后才恢复，消除「先运行、后绑定」窗口。
- Job 使用 `KILL_ON_JOB_CLOSE`、active-process、Job memory 与 Job CPU time；kill、timeout、取消和 host 句柄关闭都沿同一路径回收后代。
- `open_fds` 没有等价 Job 限制；文件/网络硬隔离也不在 Job-only 保证内，均通过结构化降级信息暴露。

## 与 run_command 集成

`builtin-tools::run_command` 总是解析真实 workspace roots，以 workspace 为默认 cwd，构造环境 allowlist/Secret denylist、文件 roots 与资源预算，再经 `SandboxSelector` spawn。该工具只声明 `Process` capability，因此模型输入不能自行关闭网络隔离：策略固定为 `Enforce`，旧 `needs_network` 输入只记审计且 `granted=false`。默认上限为 60 CPU 秒、2048 MiB、1024 fd、64 进程和 30 秒 wall time；所有可调值还有 schema 与运行时双重上界。stdout/stderr 继续实时发送 `OutputDelta`，最终 metadata 包含：

```text
sandbox.backend
sandbox.isolation
sandbox.fallback
sandbox.note
sandbox.attempted[]
sandbox.network.{requested,granted,mode}
sandbox.limits.{timeout_ms,cpu_seconds,memory_mb,open_fds,max_procs,max_output_bytes}
```

## 验证分层与当前证据

- L0：策略归一化、Seatbelt profile、bwrap argv、AppContainer/Job 配置与选择器结构化回退均有跨平台单元测试。
- Windows L1：NativeRestricted/run_command、Job 限额与深层后代清理、PTY Job 复用均在真实 Windows 运行通过。
- Linux L2：WSL 6.6 内核实际运行 bwrap 与 Landlock；允许 workspace、拒绝 sibling、收紧自定义 PATH executable 的边界测试通过，Process/PTY 进程树测试也通过。
- macOS：`process-runtime`、`sandbox-runtime`、`pty-service` 已通过 `aarch64-apple-darwin` 交叉编译；真实 Seatbelt L2 仍需 macOS runner 执行，不以交叉编译冒充运行证明。

## 验收状态

- [x] NativeRestricted 永远可选，施加 env/cwd/资源软限制并显式报告网络降级
- [x] Linux bwrap 与 Landlock 的真实文件边界测试通过；回退顺序可观测
- [x] macOS backend/profile/probe 已实现并通过目标编译与 L0；真实 macOS L2 留给平台门禁
- [x] Windows Job-only 资源/进程树隔离生效，AppContainer 缺口与降级等级明确可见
- [x] timeout/cancel/kill 复用统一进程树路径，`run_command` 流式行为与输出预算不回退
- [x] Docker/Podman 按 P11-5 决策维持归档

## Phase 15–17 执行所有权

Sandbox 只对 `ExecutionOwner::Core` 的本地 ClientFunction、Command Hook、LSP、本地 MCP、Local Browser/Computer 与 Agent Worker 子进程给出隔离等级。ProviderHosted / ProviderExtension 只做 Policy、审批、审计与 transcript 归一；跨 trust-boundary fallback 必须重新审批并记录实际 owner/backend/isolation。

## 相关文档

- [process](process.md) · [policy](policy.md) · [tools](tools.md) · [plugins](plugins.md)
- [安全验收](../quality/security-acceptance.md) · [ADR-031](../adr/ADR-031-sandbox-backend-architecture.md)
- [ROADMAP Phase 11](../../ROADMAP.md)
