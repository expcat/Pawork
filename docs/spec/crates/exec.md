# pawork-exec

> 执行内核：跨平台 Process Runtime（进程组/Job、超时、输出上限、协作取消、整树回收）、Sandbox Runtime（macOS Seatbelt / Linux bwrap+Landlock / Windows Job、软沙箱 NativeRestricted、可观测回退选择器）与 PTY 服务。**不依赖任何 `pawork-*` 包**（W1 自含，含 domain 与 policy），被 `pawork-tools` 与 app 宿主消费。

## 1. 职责与边界

- **Process Runtime**：`run`（缓冲）/ `spawn_stream`（流式）/ `spawn_interactive`（带 stdin），统一超时、stdout+stderr 合计输出预算、cancel/kill 整树回收（5s 有界）。
- **Sandbox Runtime**：把声明式 `SandboxPolicy` 翻译为平台隔离原语；`SandboxSelector::pick` 按平台选最强可用后端，探测失败**可观测回退**（fail-closed = ADR-031 可观测回退语义：不静默降级，也不因后端缺失拒跑）。
- **PTY 服务**：多会话终端（create/write/resize/订阅/快照重连/kill/清理），输出走有界 ring buffer + broadcast，**不写入 Agent Event Store**。
- **不做**：不做审批与风险分类（[policy.md](policy.md)）；不解析 workspace 相对路径（调用方传入已解析的绝对 roots）。本包 `CancellationToken` 与 `pawork_domain::CancellationToken` 是两个类型，桥接在消费方（见 [tools.md](tools.md) §4.2）。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~50 | 门面：`pub mod cancel`，其余模块私有 + 全量 re-export；平台函数（Seatbelt profile / bwrap argv / Landlock 探测 / AppContainer 映射）以 `pub(crate) use` 引入（R0 D21 包外零消费，R7 再评估是否公开）。 |
| `src/cancel.rs` | ~165（含测试） | 协作取消原语 `CancellationToken` / `CancellationFuture`：`Arc<{AtomicBool, Mutex<Vec<Waker>>}>`，克隆共享，`cancelled().await` 挂起至取消，`cancel()` 幂等唤醒全部 waiter。 |
| `src/path.rs` | ~37 | `canonicalize_platform` / `path_within_root` / `relative_to_root` 的 **crate 内小复制**（与 pawork-policy 同名函数同语义，避免 exec→policy 依赖边）；全部 `pub(crate)`。 |
| `src/process.rs` | ~1060（逻辑 ~600 + 测试） | `CommandSpec`（含 Linux 专属 `landlock` 字段）/ `ProcessLimits` / `ProcessRuntime` / `ProcessEvent` / `ProcessOutput` / `ProcessInput` / `ProcessHandle` / `ProcessError` / `LinuxLandlockPolicy`（pub(crate)）；Unix `pre_exec`（setpgid + setrlimit + Linux PDEATHSIG + Landlock restrict）；Windows `CREATE_SUSPENDED` → Job attach → `NtResumeProcess`；监督循环与 `kill_child_tree`。 |
| `src/sandbox.rs` | ~1210（逻辑 ~700 + 测试） | `SandboxPolicy` / `FilesystemPolicy` / `NetworkMode` / `ResourceLimits` / `IsolationLevel` / `SandboxBackend` trait / `SandboxProcessSpec` / `SandboxProcess` / `SandboxInteractiveProcess` / `SandboxError` / `ProbeOutcome` / `BackendSelection`；软沙箱 `NativeRestricted` 与全后端共用第一层 `apply_soft_restrictions`；`SandboxSelector`；`default_secret_paths` / `default_env_allowlist`。 |
| `src/tree.rs` | ~185 | `ProcessTreeGuard`：`attach`（pub(crate)，spawn 后立即挂载）与公开的 `attach_external(pid, limits)`（PTY 场景；Unix 要求目标为进程组长）、幂等 `terminate()`；`PROCESS_TREE_KILL_TIMEOUT = 5s`；`kill_child_tree` 收口函数。 |
| `src/os/mod.rs` | ~8 | 平台模块声明（linux / macos / windows，cfg 门控）。 |
| `src/os/linux.rs` | ~1150（逻辑 ~700 + 测试） | `generate_bwrap_argv`（系统 ro-bind + `--dev/--proc` + roots 绑定 + deny tmpfs 覆盖 + `--unshare-net/pid/ipc/uts` + `--die-with-parent --new-session`）；`BwrapBackend` / `LandlockBackend`；`compile_policy`（Landlock 读写集合编译）；`probe_landlock_support`（OnceLock 缓存，真实 ruleset 创建探测）；`prepare_linux_landlock` / `restrict_linux_landlock`（父进程编 ruleset FD、子进程 `pre_exec` 只做 no_new_privs + restrict_self）；`linux_process_tree`（`/proc` 快照冻结-倒序杀）。 |
| `src/os/macos.rs` | ~995（逻辑 ~500 + 测试） | `SANDBOX_EXEC_PATH = /usr/bin/sandbox-exec`；`escape_seatbelt_string`；`generate_seatbelt_profile`（全平台编译便于单测）；`SandboxExecBackend`（OnceLock 缓存 smoke 探测：真实跑 `sandbox-exec -p <默认 profile> /usr/bin/true`；实现 `spawn` 与 `spawn_interactive`）；`macos_process_tree`（`proc_listpids`/`proc_pidinfo` 快照，语义与 Linux 版一致）。 |
| `src/os/windows.rs` | ~545（逻辑 ~400 + 测试） | `job` 模块：Job Object 创建（`KILL_ON_JOB_CLOSE`）、`attach`/`attach_pid`、限额映射（active process / job memory / CPU time）、收养既有后代（Toolhelp32 快照，≤16 轮）；`resume`（`NtResumeProcess`）；`probe_appcontainer_job`（**冻结为 available:false**）与 `AppContainerCapability` / `AppContainerConfig` / `policy_to_appcontainer_config`（R7 预留，未启用）；`WindowsJobBackend`。 |
| `src/pty/mod.rs` | ~1525（逻辑 ~900 + 测试） | `PtyService` 与全部 PTY 公开类型；portable-pty 之上的会话管理：`PTY_SPAWN_LOCK` 短临界区 spawn、reader/waiter 专用 OS 线程、broadcast（容量 256）+ 丢弃计数、owner 归属校验、幂等清理（`CLEANUP_GRACE = 5s`）。 |
| `src/pty/buffer.rs` | ~140（含测试） | `RingBuffer`：有界字节环，单调 `OutputCursor`（`start`/`end`），`read_since` 增量读；`RingReadError::{Stale, Future}` 区分被截断的旧游标与伪造的未来游标。 |

无 `tests/` 目录与 fixture 文件；全部回归内联（Seatbelt profile golden 为 `os/macos.rs` 内联字符串断言）。

## 3. 对外 API 面

### 3.1 取消原语

- `CancellationToken::new()` / `.cancel()` / `.is_cancelled()` / `.cancelled() -> CancellationFuture`。克隆共享同一状态；`cancel()` 幂等。

### 3.2 Process Runtime

- `CommandSpec::new(program)` + builder `arg`/`args`；字段：
  - `args: Vec<String>`、`cwd: Option<PathBuf>`。
  - `env_clear: bool`（先清空再注入）、`env: Vec<(String, String)>`。
  - `timeout: Option<Duration>`、`max_output_bytes: u64`（默认 8 MiB，stdout+stderr 合计预算）。
  - `limits: ProcessLimits`；Linux 专属 `landlock: Option<LinuxLandlockPolicy>`（类型 pub(crate)，包外不可构造，由 LandlockBackend 注入）。
- `ProcessLimits { cpu_time: Option<Duration>, memory_bytes: Option<u64>, open_files: Option<u64>, max_processes: Option<u32> }`：Unix 映射 `setrlimit`（`RLIMIT_CPU`/`AS`/`NOFILE`/`NPROC`；macOS 对 AS 的 EINVAL 容忍、NPROC 主动跳过——按 uid 计数会误伤宿主）；Windows 经 Job Object。
- `ProcessRuntime::new()`（Copy 零尺寸），三种执行形态：
  - `run(spec, cancel) -> Result<ProcessOutput, ProcessError>`：缓冲收集。`ProcessOutput { stdout: Vec<u8>, stderr: Vec<u8>, exit_code: Option<i32>, truncated: bool, timed_out: bool, killed: bool }`。
  - `spawn_stream(spec, cancel) -> (mpsc::Receiver<ProcessEvent>, ProcessHandle)`（通道容量 64）。
  - `spawn_interactive(spec, cancel) -> (Receiver, ProcessInput, ProcessHandle)`：LSP/MCP 等长驻 stdio 协议进程的唯一入口，Sandbox Runtime 在其上包装。
- `ProcessEvent`：`Stdout(Vec<u8>)` / `Stderr(Vec<u8>)` / `Exit { code: Option<i32>, truncated: bool }`（8 KiB 块推送；超预算后停止转发，最终 `Exit.truncated=true`）。
- `ProcessInput::write_all(bytes)`（写全 + flush）/ `close()`（幂等 shutdown stdin）。
- `ProcessHandle::kill()`（幂等，请求整树终止并等待退出）/ `id()`；**Drop 时自动 cancel 内部 kill token**——句柄丢弃即回收进程树。
- `ProcessError`：`Spawn{program, source}` / `ProcessTree{program, source}` / `Isolation{program, source}` / `KillTimeout{process_id}` / `Io`。
- 退出语义：正常退出 `exit_code=Some(n)`；超时 `timed_out=true, killed=true`；取消 `killed=true`；信号死亡 `exit_code=None`。

### 3.3 Sandbox Runtime

- `SandboxPolicy` 字段：
  - `filesystem: FilesystemPolicy { read_roots, write_roots, deny: Vec<PathBuf> }`（deny 优先于允许，洞语义）。
  - `network_mode: NetworkMode`：`Off`（不施加）/ `Hint`（仅记录）/ `Enforce`（default；仅 Seatbelt 与 bwrap 真正断网，其余后端显式降级为告警）。
  - `allow_spawn: bool`：**false 时任何 spawn 直接 `Denied`**——沙箱执行必须显式授权；`max_procs: Option<u32>`。
  - `env_clear: bool`、`env_allowlist` / `env_denylist: Vec<String>`（denylist 优先；`*` 通配大小写不敏感：两端含、首部后缀、尾部前缀）。
  - `resources: ResourceLimits { cpu_seconds, memory_mb, open_fds, wall_time_ms, max_output_bytes }`（全 `Option<u64>`）→ 映射 `ProcessLimits` + `CommandSpec.timeout/max_output_bytes`。
  - `Default`：全空 roots + `Enforce` + `allow_spawn=false` + `env_clear=false`。
- `SandboxPolicy::untrusted_default(workspace_roots)`：read_roots = workspace roots、**write_roots 空（只读）**、deny = `default_secret_paths()`、`Enforce`、`allow_spawn=false`、`env_clear=true`、allowlist = `default_env_allowlist()`、**denylist 空**。
- `IsolationLevel`（serde snake_case、`as_str`，仅 Serialize）：
  - `Soft`：NativeRestricted 软沙箱。
  - `Hard`：bwrap 全隔离（文件系统 + 网络 + PID/IPC/UTS namespace）。
  - `HardWritesAndNetwork`：Seatbelt（写白名单 + 断网，读不隔离）。
  - `HardFilesystemOnly`：Landlock（文件系统内核强制，网络未强制）。
  - `Degraded`：探测到部分能力但完整路径不可用（如 Windows Job：进程/资源限额真实生效，文件/网络软限制）。
- `SandboxBackend` trait：`id() -> &'static str`（`"native_restricted"` / `"bwrap"` / `"landlock"` / `"sandbox_exec"` / `"windows_job"`）、`available() -> bool`（探测缓存，不在热路径重算）、`spawn(spec, policy, cancel) -> SandboxProcess`、`spawn_interactive(...) -> SandboxInteractiveProcess`（**默认实现显式拒绝** `BackendUnavailable`，防第三方后端静默降级为裸进程；NativeRestricted 与 Seatbelt 实现了它）。
- `SandboxProcessSpec { command: CommandSpec, workspace_roots: Vec<PathBuf> }`；`SandboxProcess { events, _handle }`（handle 私持，drop 即回收）；`SandboxInteractiveProcess::into_parts() -> (events, input, handle)`。
- `SandboxError`：`Denied(String)`（策略拒绝：spawn 未授权 / cwd 在 deny / Landlock 无法减洞）/ `PathEscape(String)`（cwd 越界）/ `BackendUnavailable(&'static str)` / `Process(ProcessError)`。
- `SandboxSelector::new()` / `with_runtime(runtime)`；`pick() -> (Box<dyn SandboxBackend>, BackendSelection)`。
- `BackendSelection { id: &'static str, fallback: bool, note: String, isolation: IsolationLevel, attempted: Vec<ProbeOutcome> }`；`ProbeOutcome { backend, available, reason }`——可观测回退的证据形状（Serialize，直接进工具 metadata）。
- `default_secret_paths()`（`untrusted_default` 与 tools `run_command` 共用的单一事实源）：
  - `$HOME` 下：`.ssh` `.aws` `.azure` `.kube` `.pawork`（整目录）`.pawork/auth.json` `.pawork/mcp-auth.json` `.gnupg` `.config` `.netrc` `.git-credentials` `.docker` `.npmrc` `.pypirc` `.cargo/credentials.toml`。
  - 环境覆盖：`$PAWORK_HOME`（目录 + auth.json + mcp-auth.json）与 `$PAWORK_DATA_DIR`。
  - Windows 另加：`%APPDATA%\gcloud`；无 PAWORK_DATA_DIR 时 `%LOCALAPPDATA%\pawork`。
- `default_env_allowlist()`（12 项，unix/Windows 并集）：`PATH HOME LANG LC_ALL TERM TMPDIR SYSTEMROOT TEMP TMP USERPROFILE COMSPEC PATHEXT`。

### 3.4 进程树守卫与 PTY

- `ProcessTreeGuard::attach_external(process_id: u32, limits: ProcessLimits)`：绑定外部启动的进程。Unix 要求目标是进程组长（`pgid==pid`，否则 InvalidInput；limits 被忽略）；Windows 创建 Job、写入限额并收养既有后代。`terminate()` 幂等整树终止。守卫本身无 Drop 杀树；生命周期兜底靠 `ProcessHandle` Drop（Unix）与 Job 句柄 `KILL_ON_JOB_CLOSE`（Windows）。
- `PtyService::new()`；方法（除 `subscribe` 外均校验 owner，错配 `PtyError::Ownership`）：
  - `create(spec: PtyCreateSpec) -> Result<TerminalId, PtyError>`（async）。`PtyCreateSpec { owner_session: OwnerSessionId, shell: Option<String>（None → 平台默认 shell）, args, cwd, env, size: PtyWindowSize（默认 24×80）, buffer_capacity: usize（默认 DEFAULT_BUFFER_CAPACITY = 256 KiB） }`。
  - async 方法：`resize` / `write` / `kill` / `wait_exit` / `cleanup` / `cleanup_owner`（返回清理数）/ `shutdown`。
  - 同步方法：`subscribe`（broadcast 接收器）/ `snapshot` / `read_output(cursor)` / `state` / `list_for_owner` / `session_count`。
  - `PtySnapshot { terminal_id, owner_session, state, size, buffer_start, buffer_end, buffered, exit_code, exit_signal, dropped_events }`。重连协议 = `snapshot()` 拿基线 + `read_output(cursor)` 增量：游标过旧 → `PtyError::StaleCursor{requested, available_from}`（应重新 snapshot）；未来游标 → `FutureCursor`（输入非法）。
  - `PtyEvent`：`Output { data, cursor_end }` / `Exit { code, signal, state }`（`state` 是 waiter 写入退出事实后的权威 `PtySessionState`，显式 cleanup 与订阅消费并发时仍可无竞态地区分 Exited/Killed；broadcast 容量 256，慢消费者被覆写的事件计入 `dropped_events`）。
  - `PtySessionState`：`Running / Exited / Killed`；`PtyError` 10 变体：`NotFound / Ownership / Closed / StaleCursor / FutureCursor / Create / Spawn / ProcessTree / Io / ShuttingDown`。
  - `OwnerSessionId` / `TerminalId`：字符串 newtype（本包不依赖 domain 的 ID 类型）。
- `RingBuffer::new(capacity)` / `push` / `read_since(cursor)` / `start()` / `end()`；`OutputCursor` 单调递增，容量满丢最老字节。

## 4. 核心行为与数据流

### 4.1 一次 spawn（process.rs `spawn_child`）

1. 构造 `tokio::process::Command`：按 `env_clear` 清环境、注入 `env`、设 cwd、stdout/stderr 管道（stdin 按需）、`kill_on_drop(true)`。
2. Unix：`pre_exec` 内 `setpgid(0,0)` 自立进程组 → `setrlimit`（CPU/AS/NOFILE/NPROC，macOS 特例见 §3.2）→ Linux 再 `PR_SET_PDEATHSIG(SIGKILL)` + `getppid()==1` 检查（关父死竞态窗口）→ 若带 Landlock ruleset FD 则 `restrict_linux_landlock`（no_new_privs + restrict_self；失败即 spawn 失败，fail-closed）。Landlock ruleset 由父进程在 spawn 前编译（`prepare_linux_landlock`），子进程 `pre_exec` 内不 open 任何文件（async-signal-safety）。
3. Windows：`CREATE_SUSPENDED` 创建 → `ProcessTreeGuard::attach`（建 Job + 限额 + 收养）→ `resume`（`NtResumeProcess`）；attach 或 resume 失败立即 kill 子进程再报 `ProcessTree`/`Isolation` 错——杜绝无守卫窗口。
4. 监督任务（tokio）：并发读 stdout/stderr 至合计预算，`select!`（biased）等待 cancel / handle.kill / timeout / 退出；非正常路径走 `kill_child_tree`（guard.terminate + start_kill + 5s 有界等待，超时报 `KillTimeout`）；先置 done 再收尾输出任务、最后发 `Exit` 事件（句柄等待不被输出背压阻塞）。

### 4.2 `SandboxSelector::pick` 探测与回退

1. 平台候选序：macOS `sandbox_exec → native_restricted`；Linux `bwrap → landlock → native_restricted`；Windows `appcontainer（冻结不可用）→ windows_job`（无条件选中，`Degraded` + `fallback=true`）；其它平台直接 `native_restricted`（`Soft`）。
2. 每个候选的探测结果（含 reason 文案）推进 `attempted`；第一个可用者即选中。探测都是真实验证而非存在性检查：macOS 跑一次 `sandbox-exec /usr/bin/true` smoke（OnceLock 缓存）；Linux bwrap 探测可执行 + userns，Landlock 真实创建 ruleset；Windows AppContainer 恒 `available:false`。
3. 非首选即 `fallback=true`；全部硬后端失败（非 Windows）落 `NativeRestricted`（永真可用）。消费方必须把 `BackendSelection` 写入结果 metadata（[tools.md](tools.md) §4.2），CLI/GUI 必须展示 fallback——这是「可观测回退」的完整闭环。

### 4.3 沙箱 spawn（硬后端共构）

1. `apply_soft_restrictions`（全后端共用第一层）：`allow_spawn` 硬门 → cwd 三重校验（在 workspace_roots 内 + 不在 deny 洞 + 在 read/write roots 内；违规 `PathEscape` / `Denied`）→ env 清洗（启用过滤即强制 `env_clear`，denylist 优先于 allowlist）→ `ResourceLimits` 映射（wall_time→timeout、max_output_bytes 覆写、cpu/memory/fds→`ProcessLimits`、max_procs→`max_processes`）。
2. 平台翻译：
   - Seatbelt：生成 profile 文本（§5 golden），argv 改写为 `sandbox-exec -p <profile> <prog> <args>`。
   - bwrap：生成 `generate_bwrap_argv` 前缀包装（`--ro-bind /` 起底、写 roots `--bind`、deny 洞 tmpfs 覆盖、`--unshare-net/pid/ipc/uts`、`--die-with-parent --new-session`）。
   - Landlock：`compile_policy` 产出 `LinuxLandlockPolicy`（read = read_roots + workspace + cwd + 已解析 executable + 存在的系统只读路径；write = write_roots + `/dev/null` + `/dev/zero`；**deny 与 allow 根重叠时直接拒绝**——Landlock 纯 allow 模型无法减洞）注入 `CommandSpec`，§4.1 步骤 2 内核态强制。
   - Windows Job：进程/资源限额真实生效，文件/网络保持软限制（`Degraded`）。
3. `NativeRestricted` 在 `Enforce` 下显式 `tracing::warn`（target `pawork.sandbox`）降级为 Hint——绝不静默声称已强制。
4. 走 §4.1 的 `spawn_stream` / `spawn_interactive`，返回 `SandboxProcess` / `SandboxInteractiveProcess`。

### 4.4 进程树终止（Unix 共通模式）

1. 对进程组 SIGSTOP 冻结，抑制「kill 竞速下重新 fork」。
2. 快照式列举后代（Linux `/proc/<pid>/stat` PPID 图；macOS `proc_listpids` + `proc_pidinfo`），迭代至固定点，上限 16 轮；凭 PPID 图捕获 `setsid` 逃逸出进程组的后代。
3. 按深度倒序 SIGKILL（先叶后根），再 `killpg(SIGKILL)` 兜底；以进程 start_time 复核防 PID 复用误杀。
4. Windows：`TerminateJobObject` 一击整树；Job 句柄关闭即 `KILL_ON_JOB_CLOSE` 兜底宿主崩溃场景。

### 4.5 PTY 会话生命周期

1. `create`：拒绝 shutdown 中的服务 → `spawn_blocking` 内持全局 `PTY_SPAWN_LOCK` 短临界区 openpty + spawn + `attach_external`（portable-pty 的 Unix spawn 在 `pre_exec` 配置 session/TTY，并发进入是未定义行为；子进程经 `setsid` 成组长满足 attach 前置）→ 注册 `SessionInner`。
2. Reader 专用 OS 线程：8 KiB 块阻塞读 master → `RingBuffer.push`（容量满丢最老）→ broadcast `Output{data, cursor_end}`。Waiter 专用 OS 线程：`child.wait()` → 置 `Exited`（或保持 `Killed`）+ exit_code/signal → 发 `Exit{code, signal, state}`（终态随事件携带，不依赖仍在 service map 的快照）→ 幂等释放句柄并通知 `closed`。
3. `write` / `resize` 经 `spawn_blocking` 转阻塞句柄；`kill` → guard.terminate 整树 + 状态置 `Killed`。
4. 会话退出后条目**保留**（供重连读缓冲与退出态），由显式 `cleanup` / `cleanup_owner` / `shutdown` 移除；清理等待 waiter 回收子进程至多 `CLEANUP_GRACE = 5s`。
5. 输出只存在于有界 ring buffer；PTY 原始输出不属于 Agent 事件，不持久化、不重放。

## 5. 契约与不变量

- **`IsolationLevel` 词汇冻结**：`soft / hard / hard_writes_and_network / hard_filesystem_only / degraded`（`as_str` 与 serde snake_case 同形），内联 golden `isolation_level_vocabulary_golden` 钉死；GUI 协议与工具 metadata 复用该词表。
- **Seatbelt profile golden**：`profile_full_output_golden` 对固定输入断言 profile 全文；结构不变量另有单测：
  - `(version 1)` + `(deny default)` 起底。
  - 读：整盘 `(allow file-read* (subpath "/"))` 叠加 secret deny 挖洞（deny 必须用 `file-read*` 形态才盖得住整盘 allow）。
  - 写：deny-default 白名单（write_roots + `/tmp` + `/private/tmp` + `$TMPDIR` + `/dev` 明细），raw + canonical 双形态。
  - **每个 write_root ∪ workspace_root 下永久禁写 `<root>/.git`（subpath）与 `<root>/.env`（literal）**，亦为 raw + canonical 双形态。
  - 网络：`Enforce → (deny network*)`；`Hint` / `Off → (allow network*)`。
  - `max_procs` 仅注释行（Seatbelt 无进程数原语，依赖 RLIMIT_NPROC，诚实降级不假装强制）。
- **`default_secret_paths` 集合冻结**：§3.3 清单；`secret_paths_for_exact_vector_golden` / `default_secret_paths_controlled_env_golden` 钉死（含 PAWORK_HOME / PAWORK_DATA_DIR 覆盖形态）。`~/.pawork` 整目录在列（内含 `protected/master.key` 与加密 blob）。
- **`default_env_allowlist` 12 项冻结**（§3.3 清单）。
- **fail-closed = 可观测回退**：探测失败不阻断执行，但 `BackendSelection{fallback, attempted}` 必须如实上报；Landlock 编译失败则 spawn 失败（不静默裸奔）；Windows attach/resume 失败即杀子进程；NativeRestricted 的 Enforce 降级必须 warn。
- **spawn 默认拒绝**：`SandboxPolicy::default()` 与 `untrusted_default` 均 `allow_spawn=false`；`SandboxBackend::spawn_interactive` 默认实现拒绝。
- **kill 幂等且有界**：任何路径（超时/取消/显式 kill/句柄 Drop）5s 内整树回收，否则显式 `KillTimeout`。
- **PTY 归属强制**：非 owner 的会话操作一律 `PtyError::Ownership`；PTY 输出不写事件存储。
- 破坏以上形状须走 ADR（R7 沙箱窗口）；无独立 golden 文件，契约测试全部内联。

## 6. 依赖关系

- **workspace 内**：无（`path.rs` 为 policy 三函数的刻意复制，保持 W1 零依赖）。
- **外部**：`tokio`（full）、`portable-pty`、`async-trait`、`serde/serde_json`、`thiserror`、`tracing`、`dunce`、`libc`（Unix）；Linux 加 `landlock`；Windows 加 `windows`（Win32 Job/Threading/Security feature 集）。dev 依赖 `tempfile`。无 cargo feature。
- **被依赖**：`pawork-tools`（run_command + MCP stdio 托管）、app 宿主（PTY / 沙箱装配）。`pawork-engine` 刻意不依赖本包（进程操作由宿主注入）。

## 7. 测试与验证资产

默认验证命令：`cargo test -p pawork-exec --offline --lib --tests`（无 `tests/` 目录，用例全部在 `--lib`；平台 cfg 用例仅在对应 OS 编译执行）。

| 文件 | 覆盖点 |
| --- | --- |
| `cancel.rs` | 取消传播、克隆共享、waiter 唤醒、幂等。 |
| `process.rs` | stdout/stderr 捕获与 exit_code、超时（`timed_out+killed`）、取消 kill、输出截断（`Exit.truncated`）、交互式 stdin 写入/close、进程树整树回收（孙进程）、rlimit 应用（Unix）、句柄 Drop 回收。 |
| `sandbox.rs` | `untrusted_default` 形状（只读 + no spawn + Enforce + env_clear）、`isolation_level_vocabulary_golden`、`secret_paths_for_exact_vector_golden` / `default_secret_paths_controlled_env_golden`、env 清洗（denylist 优先、通配大小写）、cwd 越界与 deny 洞拒绝、spawn 未授权 `Denied`、资源映射、selector 回退与 `attempted` 记录、NativeRestricted spawn / spawn_interactive 冒烟。 |
| `tree.rs` | attach_external 组长前置校验、terminate 幂等。 |
| `os/linux.rs` | bwrap argv 生成（bind 顺序 / unshare / die-with-parent / deny tmpfs 覆盖）、Landlock 策略编译（读写集合、deny 重叠拒绝、executable 单文件授权）、探测降级 reason、进程树冻结-快照-倒序杀、setsid 逃逸捕获、start_time 防复用。 |
| `os/macos.rs` | **`profile_full_output_golden`** 及结构断言（`profile_denies_network_when_enforce` / `profile_allows_network_when_hint` / `profile_emits_deny_for_secret_paths` / `profile_emits_deny_for_default_secret_paths` / `profile_emits_canonical_deny_for_existing_path` / `profile_emits_write_roots_as_file_write` / `profile_notes_max_procs_unenforced` / `profile_includes_version_header`）、字符串转义、进程树冻结与逃逸回收。 |
| `os/windows.rs` | AppContainer 能力→SID 映射、`probe_appcontainer_job` 冻结输出、Job 限额映射与后代收养。 |
| `pty/mod.rs` | 输出捕获、owner 强制、快照重连（游标续读 / Stale 语义）、broadcast 覆写丢弃计数、kill 整树（后代收割）、多会话 cleanup_owner / shutdown、resize、退出状态与 signal 传播。 |
| `pty/buffer.rs` | 环形丢最老、游标单调、`read_since` 增量、`Stale` / `Future` 判定。 |

## 8. 注意事项与已知限制

- `NativeRestricted` 是**非对抗性**软沙箱：env 清洗 + cwd/资源约束，挡不住已授权命令内部的越权读（如无硬后端时 `sh -c "cat ~/.ssh/id_rsa"`）；仅作纵深防御第一层与最低回退。
- `NetworkMode::Enforce` 真正断网仅 Seatbelt（`deny network*`）与 bwrap（`--unshare-net`）；Landlock / Windows Job / NativeRestricted 下降级为告警注记，不拒跑。K-09 全平台闭合属 R7。
- Windows：AppContainer 类型与映射已备但探测冻结 `available:false`，选择器实际落 `WindowsJobBackend`（`Degraded`）；`attach_external` 收养既有后代非原子（Toolhelp32 快照竞态窗口，≤16 轮尽力收养）。
- macOS：`RLIMIT_AS` 的 EINVAL 容忍与 `RLIMIT_NPROC` 跳过是刻意行为；Seatbelt 无进程数原语，`max_procs` 仅 profile 注释。
- Landlock 是纯 allow 模型：deny 路径与 allow 根重叠时后端直接拒绝该次执行（`Denied`），不会静默放行。
- `max_output_bytes` 为 stdout+stderr 合计预算、8 KiB 块推送，截断点可能落在 UTF-8 字符中间（消费方需容忍半字符字节流）。
- `attach_external` 在 Unix 仅接受进程组长；PTY 路径由 portable-pty 的 `setsid` 保证；`ProcessLimits` 参数仅 Windows 生效。
- 相关文档：跨包执行链路 [../flows.md](../flows.md)；架构总览 [../../architecture.md](../../architecture.md)；布局与冻结契约 [../../design.md](../../design.md)；Spec 索引 [../README.md](../README.md)；R7 沙箱演进 [../../../ROADMAP.md](../../../ROADMAP.md)。
