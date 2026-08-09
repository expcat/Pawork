# Phase 11 Review：Sandbox 与跨平台强化

- **审查范围**：P11-1～P11-8（主任务）及相关 `sandbox-runtime` / `process-runtime` / `pty-service` / `policy-engine` / `builtin-tools` / 跨平台路径实现；ADR-031 及其 Amendment；`docs/features/sandbox.md`、`docs/features/process.md`。
- **事实源**：当前源码（逐行 + `rg` 跨引用核对）、`plan/P11-*.md`、`ROADMAP.md` Phase 11 段、ADR-031。
- **方式**：Commander 统筹 + 三个只读 `deepseek_explorer` 并行调查（sandbox-runtime / process+pty+path / policy+run_command），主代理复核关键论断。**本次只 Review，不修改实现**。
- **审查日期**：2026-08-09。

---

## 0. 总评

Phase 11 的核心设计目标——三层沙箱（NativeRestricted 兜底 + 平台原生硬隔离 + 探测回退）、统一进程树终止、PTY 会话层、跨平台路径——**在代码层面已真实落地且与 ADR-031 一致**：四个后端全部接入 `SandboxSelector::pick()`，无死后端；`IsolationLevel` 四值各有真实产出路径（非为未来留空）；`run_command` 确实经 `SandboxBackend::spawn` 执行而非直连 `ProcessRuntime`；网络固定 `Enforce` fail-closed；Windows Job-only 诚实报告 `degraded`，不伪称硬隔离。架构红线（纯 Rust、依赖方向单向、agent-domain 不依赖实现）全部满足。

主要问题不是「缺功能」，而是 **三处可收敛的重复映射 + 两处集成边界**：

1. 「ExecutionConstraints → SandboxPolicy 归一化」设计了却没接入主流程，导致同一映射被写三份（设计意图与实现脱节，最值得收敛）。
2. env 白名单 / secret deny 清单 / Linux 系统路径 / 跨平台路径规范化 在两到三处各写一份，存在漂移风险。
3. `builtin-tools`（`RunCommandTool`）与 `pty-service`（`PtyService`）在整个 workspace 中**没有生产消费方**——沙箱路径与 PTY 仅在工具自身和单测内被证明，尚未接入 agent 循环 / CLI host / GUI Connection Protocol。这不是 Phase 11 交付缺陷（工具注册属 P4、GUI/PTY 接线属 Phase 13），但评审须把它显式记录，避免误以为「沙箱已保护真实运行」。

没有发现需要新增抽象的设计缺口；绝大多数建议是「删除/合并/接线」，方向与本次「优先减少代码与概念」一致。

---

## 1. 设计符合度（正面结论）

| 设计目标 | 实现事实 | 证据 | 判定 |
| --- | --- | --- | --- |
| `SandboxBackend` trait + 统一类型骨架 | trait + `SandboxPolicy`/`SandboxProcessSpec`/`SandboxProcess`/`SandboxError`/`SandboxSelector`/`BackendSelection`/`IsolationLevel` 全部定义且被消费 | `sandbox-runtime/src/lib.rs:86-149`、`backends/{linux,macos,windows}.rs` | 符合 |
| NativeRestricted 永远可用、施加软限制 | `apply_soft_restrictions` 各后端共用：env 清洗、cwd 锁定、rlimit/资源上限；有端到端 secret 剥离测试 | `lib.rs:430-460`、`750-791` | 符合 |
| 平台原生后端 + 探测回退 | bwrap/landlock/seatbelt/job 四后端接入 pick；探测经 OnceLock 缓存，结果进 `attempted` | `lib.rs:277-369`、各 backend 探测 | 符合 |
| `BackendSelection` 不静默降级 | isolation/fallback/note/attempted 真实写入，run_command 同步输出 metadata | `run_command.rs:258-271,310-334` | 符合 |
| Windows AppContainer 诚实降级 | `probe_appcontainer_job` 恒 `available:false`，选择 Job-only 并报 `degraded` | `windows.rs:104-131`、`lib.rs:344` | 符合（与文档 degraded 声明一致） |
| 进程树终止三平台一致 + crash 安全 | Unix `killpg`+`/proc` 后代冻结；Windows `CREATE_SUSPENDED`→Job attach→`KILL_ON_JOB_CLOSE`→`NtResumeProcess`；`setsid` 离组有兜底测试 | `process-runtime/src/lib.rs:407,982-1264,767-828,1556-1608` | 符合 |
| `ProcessTreeGuard` 统一终止路径 | run_command/Sandbox/PTY 共用同一实现；sandbox 委托 `spawn_stream`，PTY 经 `attach_external` | `process-runtime/lib.rs:869,909,943`；`pty-service/lib.rs:776` | 符合 |
| kill 幂等 + 5s 限时 | `PROCESS_TREE_KILL_TIMEOUT=5s`；`kill` 先查 done 再 cancel 限时等待，Drop 兜底 | `process-runtime/lib.rs:22,144-172,976-980` | 符合 |
| `run_command` 经沙箱执行 | `SandboxSelector::with_runtime` → `pick` → `backend.spawn`，无直连 `ProcessRuntime::spawn` | `run_command.rs:258-268` | 符合 |
| 网络固定 Enforce fail-closed | `needs_network` 仅审计 `granted=false`；`network_mode=Enforce` 硬编码 | `run_command.rs:240,265,316-336` | 符合 |
| 资源默认值 + 双重上界 | schema maximum+default 与运行时 clamp 同源常量；默认 60s CPU/2048MiB/1024fd/64 进程/30s wall | `run_command.rs:53-62,113-125,171-233` | 符合 |
| `sandbox.*` metadata 全字段 | backend/isolation/fallback/note/attempted/network.{requested,granted,mode}/limits 逐项写入 `ToolResult.metadata` | `run_command.rs:310-334,347-350` | 符合 |
| 依赖方向合规 | sandbox-runtime → process-runtime + policy-engine + agent-domain；policy-engine 不反向依赖；纯 Rust | 各 `Cargo.toml` | 符合 |

**结论**：Phase 11 没有「声称已做但代码未做」的严重缺口；ADR-031 的分层架构在源码中得到忠实实现。

---

## 2. 冗余 / 重复 / 死代码

### 2.1〔中-高〕「归一化」未接入主流程，映射被写三份

设计目标（P11-1 步骤 3、`sandbox.md:17`）是 `policy-engine::ExecutionConstraints` 归一化为 `SandboxPolicy`（含 `ToolCapability::Network → network.mode`）。

事实：
- `impl From<&ExecutionConstraints> for SandboxPolicy`（`lib.rs:119`）与 `impl From<&ExecutionConstraints> for ResourceLimits`（`lib.rs:72`）**仅被单测调用**（`lib.rs:612`），生产路径零消费。
- 主流程实际走：`tool-runtime/scheduler.rs:268,373` 把约束注入为 `request.input` 字段 → `run_command.rs:215-252` 从 input 解析并**手工重建** `SandboxPolicy`。
- `ToolCapability::Network → network.mode` 映射在代码中不存在：run_command 不读 `ToolCapability`，硬编码 `NetworkMode::Enforce`。

同一份语义被写三处（tool-runtime / builtin-tools / sandbox-runtime From），且 From 的 `ResourceLimits::from` 与 run_command 手工构造默认值不同（前者无界 Option，后者带 schema 上界）。

**建议（二选一，方向是减少映射）**：
- 接线：让 `run_command` 改用 `From<&ExecutionConstraints>` 作基线再收紧，删除其手工映射；或
- 删除：若短期不接线，删除 `From` impl 与 `ResourceLimits::from`，消除「设计已实现」的假象与双写源。

### 2.2〔中〕env 白名单 / secret deny 清单双写并已漂移

- env 白名单：`run_command.rs:37-46`（unix 6 项 / windows 11 项）预填 `spec.env`；`sandbox-runtime/lib.rs:553-560` `default_env_allowlist()`（5 项）。两份内容不一致（`TMPDIR/SYSTEMROOT/TEMP/TMP/USERPROFILE/COMSPEC/PATHEXT` 仅前者有）。
- secret 路径：`run_command.rs:415-426`（`.ssh/.aws/.azure/.kube`+gcloud，实际生效且更完整）vs `sandbox-runtime/lib.rs:540-551` `default_secret_paths`（仅 `~/.ssh` 或 `%APPDATA%`）。

当前因 `untrusted_default` 无生产调用方未爆雷，但两份清单已发散，未来若 `untrusted_default` 被接线会产生安全语义差异。

**建议**：导出单一权威清单（建议放 sandbox-runtime，供 `untrusted_default` 与 run_command 共用），消除漂移。

### 2.3〔中〕Linux 系统路径白名单双写

`sandbox-runtime/backends/linux.rs` 内两份高度重叠的常量：bwrap 内联 ro-bind 列表（`55-69`，13 项）与 Landlock `SYSTEM_READ_PATHS`（`234-255`，19 项），`/usr /lib /lib64 /bin /sbin /nix /etc/ssl` 等 8+ 项重复，需同步演进。

**建议**：提取共享 `const`，两处按需筛选。

### 2.4〔中〕跨平台路径规范化未真正统一

P11-8 验收称「policy-engine、git-service、resource-loader 与 sandbox-runtime 路径消费者统一使用该边界」，但事实：

- 权威符号只有 `policy_engine::canonicalize_platform`（`path.rs:156`）与 `policy_engine::path_within_root`（`path.rs:163`），实际消费方仅 policy-engine 自身与 sandbox-runtime。
- **`resource-loader` 重写了一份**：`io.rs:119` `path_is_within` + `io.rs:128-142` `relative_to_root` 与 `policy-engine/path.rs:168-186` **逐行同构**（含 Windows 分支）；另在 `agents.rs/io.rs/templates.rs` 直接调 `dunce::canonicalize` 4 处。
- `git-service` 直接用 `dunce::simplified`（11 处）+ `dunce::canonicalize`（多处）。

**建议**：`resource-loader` 改为复用 `policy_engine` 两符号（依赖方向允许）；`git-service` 的 `simplified`（去 verbatim+lexical，与 canonical 语义不同）可保留，但收口到单一 `simplified_cwd` helper。这是 P11-8 「统一消费」验收标准的真实缺口。

### 2.5〔中-高〕Windows 后端存在死代码 + 平行映射

`sandbox-runtime/backends/windows.rs` 中：
- `JobLimitsConfig`（`38-56`）+ `policy_to_job_limits`（`59-68`）：生产零消费，`WindowsJobBackend::spawn`（`176-196`）走 `apply_soft_restrictions`→`ProcessLimits`→`process-runtime` `windows_job::Job::create`，Job 限额实际经 `ProcessLimits` 通道生效。同一 policy 字段（memory_mb→bytes、max_procs）存在两条映射。
- `AppContainerConfig`（`27-35`）+ `AppContainerCapability`（7 变体，仅 `InternetClient` 被用）+ `policy_to_appcontainer_config`（`74-87`）：无任何 spawn 消费方，仅测试使用；与文档「接口冻结未接入」一致。

**建议**：删除 `policy_to_job_limits`/`JobLimitsConfig`（约 31 行 + 测试）；AppContainer 生成器若 P11-4.E1 无近期排期则整体删除（约 50 行），或明确标注 `// frozen, awaiting P11-4.E1`。

### 2.6〔低〕其他小冗余

- `SandboxProcessSpec.needs_network`（`lib.rs:134`）：仅被 NativeRestricted 的 warn 日志读取（`lib.rs:225`），无后端以其门控，run_command 恒传 false。网络语义已由 `policy.network_mode` 表达，字段冗余 → 建议删除。
- `SandboxProcess::kill` + 私有 `handle`（`lib.rs:137-149`）：workspace 内无调用方（PTY 走 `ProcessTreeGuard`），实际退化为「事件流 + 不可达句柄」→ 建议删除或接入消费方。
- `NetworkMode::Off` 与 `Hint` 在所有后端行为等价（bwrap 仅 `Enforce` unshare-net；macOS 把 Off|Hint 同编译为 allow；NativeRestricted 仅对 `Enforce` 告警；AppContainer 仅 `Enforce` 不发 Internet）→ 建议合并为单值（保留 `Hint`，删 `Off`，或反之）。
- `network_allow_hosts` 字段近乎死字段：仅 `macos.rs:98-103` 以注释消费（明确不编译 host 过滤），其余后端忽略 → 建议删除或标注未实现（避免调用方误以为 `Enforce+allow_hosts` 生效）。
- run_command timeout 双写：`spec.timeout`（`170-171`）与 `resources.wall_time_ms`（`249`）值相同，后者由 `apply_soft_restrictions` 重写 → 建议删前者，仅保留 policy 路径。

---

## 3. 架构与集成问题

### 3.1〔高〕沙箱与 PTY 仅在工具自身/单测内被证明，未接入主流程

全仓 `rg` 核实：`builtin-tools` 与 `pty-service` 在整个 workspace 中**没有任何生产 crate 依赖它们**（仅在各自 `Cargo.toml` 自声明与根 `Cargo.toml` members 列表出现）。`RunCommandTool` 未被 agent-engine / app-service / cli-host / tool-runtime 注册；`PtyService` 无消费方（app-service 仅一处 mock 字符串引用）。

含义：
- 「`run_command` 经沙箱执行」「进程树清理」「PTY 会话」这些 Phase 11 能力**当前只在工具实现和定向测试中被验证**，真实 agent 循环尚未通过它们执行模型发起的命令。
- 这**不是** Phase 11 的交付缺陷（工具注册是 P4 范畴、GUI/PTY 接线属 Phase 13），但 ROADMAP/sandbox.md 的措辞易让人误读为「沙箱已保护真实运行」。

**建议**（不改实现，仅澄清）：
- 在 `sandbox.md` / `process.md` / `ROADMAP` Phase 11 段显式标注「沙箱与 PTY 的主流程接线在工具注册（P4）与 GUI Connection Protocol（Phase 13）完成；当前证据限于工具自身与定向测试」。
- 不建议在本次为接线创建新抽象；接线属后续 Phase 任务。

### 3.2〔中〕`attach_external` 契约不对称、无文档保证

`ProcessTreeGuard::attach_external`（`process-runtime/lib.rs:869`）是 PTY 接入进程树终止的公开契约，但存在两处不对称：
- Unix 分支忽略 `limits`（`872` `let _ = limits`），Windows 分支用它建 Job——契约注释未说明「limits 仅 Windows 生效」。
- Unix 硬性要求目标已是进程组 leader（`876-885`，非 leader 返回 `InvalidInput`）；PTY 靠 portable-pty 的 `setsid` 满足（注释 `867`），但前置条件无类型/文档级保证。
- Windows 后代收养（`1129-1166`）在 `spawn_blocking` 内同步执行 16 轮，最坏耗时无界。

**建议**：补全契约文档（limits 语义、leader 前置条件、收养耗时上界），或显式参数化 pgid；属文档/契约完善，不改行为。

### 3.3〔中〕PTY 输出双通道静默丢弃

`pty-service` 同一 chunk 同时入 `RingBuffer`（`lib.rs:325-327`）与 broadcast（`329-332`，容量 256 事件）。慢消费者丢广播事件时 ring 保留，但 broadcast 满后**静默丢旧数据且无 `truncated` 标志**。与 process-runtime 的 `reserve_output_bytes`（字节预算截断）是不同语义。

**建议**：在快照/事件中显式暴露丢弃事实（如 `truncated: bool` 或 dropped 计数），避免重连消费者无感知丢数据。属可观测性改进。

### 3.4〔低〕process-runtime 单文件职责面宽

`process-runtime/src/lib.rs` 58KB / 1671 行，横跨 spec/limits 定义、spawn 组装、Unix pre_exec、Landlock ruleset 构建、`linux_process_tree`（`616-849`，234 行）、`windows_job`（`982-1264`，283 行）、两类输出收集。内部已按平台模块隔离，行为正确，但文件过大影响导航。

**建议**（纯组织性，不改行为）：把 `linux_process_tree`、`windows_job` 提为 `tree/linux.rs`/`tree/windows.rs`，Landlock 构建拆出。优先级低，与「减少代码」目标关系不大，仅在下次触碰该 crate 时顺手做。

---

## 4. 不建议改动的部分（已最小/符合设计）

- **`IsolationLevel` 四值 enum**：`Soft`/`Hard`/`HardFilesystemOnly`/`Degraded` 全部由真实路径产出，无死值；多维 `SandboxGuarantees` 在 ADR-031 Amendment 与 P11-1.E1 明确为「演进」，IsolationLevel 定位为摘要——**现状已最小，不为未来留空**。
- **`backends/mod.rs`（13 行）**：纯路由 + 边界文档，非空壳，保留。
- **Landlock enforcement 切分**：ruleset 创建 + `restrict_self` 在 process-runtime（`528-614`），policy→ruleset 编译在 sandbox-runtime（`linux.rs:307-361`），职责正确，不重复。
- **四个后端的保证差异与结构化回退**：各有真实保证差异且可观测，符合 ADR-031。
- **`PTY_SPAWN_LOCK` 短临界区**（`pty-service/lib.rs:41,736-784`）：为规避 musl 并发 fork/`pre_exec` 崩溃而真实存在，保留。
- **Docker/Podman 归档决策**（P11-5）：与「不为 OCI/VM 提前抽象」一致，ADR-031 Amendment 的 Sandbox-vs-Execution-Environment 边界清晰，不应回退。

---

## 5. 改进优先级

> 全部为「删除/合并/接线/澄清」类，无新增抽象。本次只 Review，下列为后续可选收敛建议，按收益/成本排序。

### P1（高收益、低成本、直接减少重复）

1. **归一化二选一**（§2.1）：接线 `From<&ExecutionConstraints>` 或删除之，消除三份映射与「设计已实现」假象。
2. **跨平台路径真正统一**（§2.4）：`resource-loader` 复用 `policy_engine` 符号，删除重写的 `relative_to_root`/`path_is_within`；满足 P11-8 的「统一消费」验收。
3. **env / secret 清单单一来源**（§2.2）：消除已漂移的双写。

### P2（中等收益、清理死代码/契约）

4. **删除 Windows 死代码**（§2.5）：`policy_to_job_limits`/`JobLimitsConfig`；AppContainer 生成器标注 frozen 或删除。
5. **Linux 系统路径常量合并**（§2.3）。
6. **主流程集成边界显式标注**（§3.1）：在 sandbox.md / process.md / ROADMAP 写明沙箱与 PTY 的接线依赖 P4/Phase 13，避免误读。

### P3（低优先、小冗余/可观测性/组织）

7. 删除 `SandboxProcessSpec.needs_network`、`SandboxProcess::kill`+`handle`、合并 `NetworkMode::Off`/`Hint`、处理 `network_allow_hosts` 死字段、简化 run_command timeout 双写（§2.6）。
8. PTY 广播丢弃可观测性（§3.3）：补 `truncated` 标志。
9. `attach_external` 契约文档完善（§3.2）。
10. process-runtime 文件拆分（§3.4，纯组织性）。

---

## 6. 方法与证据可信度

- 三个 `deepseek_explorer` 并行逐行阅读 `sandbox-runtime`（lib.rs 816 / linux.rs 758 / macos.rs 295 / windows.rs 278）、`process-runtime`（lib.rs 1671）、`pty-service`（lib.rs 1199 / buffer.rs 142）、policy-engine path.rs、builtin-tools run_command.rs，跨引用用 `rg` 核对消费方。
- 主代理独立复核三项关键论断并全部确认：(a) `builtin-tools`/`pty-service` 无生产消费方（`rg` 全仓 toml + rust）；(b) `resource-loader` 重写 `relative_to_root`；(c) `From<&ExecutionConstraints>` 仅单测调用。
- 未运行构建（本次为只读审查）；所有行号与符号名基于当前工作区源码，未使用记忆中的旧版本。
- 不修改任何实现；本文件为 Review 结论。

---

## 7. 修复记录（review-remediation）

**修复任务**：[P11-9](../../plan/P11-9-review-remediation.md) · 状态：🟢已完成 · TargetVerified · 修复日期：2026-08-10

Commander 统筹 + 4 个 `deepseek_explorer` 并行核对（§2.1/§2.2/§2.6、§2.3/§2.5/§3.1、§2.4、§3.2/§3.3/§3.4）全部确认成立（§2.1 核心成立，两处细节已过时并记录）+ 4 个 `deepseek_worker` 并行执行（写集互不重叠）+ `deepseek_reviewer` 独立复核。

### 已修复（§2/§3）

| 章节 | 问题 | 处置 |
| --- | --- | --- |
| §2.1 | `From<&ExecutionConstraints>` 两 impl 仅单测消费，映射写三份 | 删两 impl + 单测；归一化统一映射延后 P11-1.E2；sandbox.md 描述同步 |
| §2.2 | env 白名单 / secret deny 清单双写并漂移 | sandbox-runtime 导出权威超集（pub），run_command 删本地副本复用；超集回归测试 |
| §2.3 | Linux 系统路径白名单双写（13 项重叠） | 提取共享 `SYSTEM_READ_PATHS` const，bwrap/Landlock 共用 |
| §2.4 | 跨平台路径规范化未真正统一 | policy-engine 开放 `relative_to_root` pub，resource-loader 复用，删本地同构副本，移除 dunce 直接依赖 |
| §2.5 | Windows 死代码（JobLimitsConfig/policy_to_job_limits）+ AppContainer 平行映射 | 删 JobLimitsConfig + policy_to_job_limits + 测试；AppContainer 生成器 frozen 标注（P11-4.E1） |
| §2.6 | needs_network/kill+handle/NetworkMode Off=Hint/network_allow_hosts/timeout 双写 | 删 needs_network + kill()（`_handle` 保留为 Drop 生命周期守卫）；network_allow_hosts 标注；timeout 双写消除；NetworkMode 合并延后 P11-1.E1 |
| §3.1 | 沙箱与 PTY 仅工具自身/单测证明，未接入主流程 | sandbox.md/process.md 新增「主流程集成边界」段；不改实现（接线属 P4/Phase 13） |
| §3.2 | attach_external 契约不对称、无文档 | doc-comment 补全（limits 仅 Windows/leader 前置/收养 16 轮上限） |
| §3.3 | PTY 输出双通道静默丢弃 | `dropped_events: AtomicU64` + `PtySnapshot` 暴露；PtyEvent 不变；2 个测试 |

### 显式延后

- **§2.6 NetworkMode::Off/Hint 合并** → P11-1.E1（多维 SandboxGuarantees 重设计）
- **§3.4 process-runtime 文件拆分** → 下次触碰该 crate 时顺手（纯组织性）
- **§3.1 主流程接线** → P4 工具注册 + Phase 13 CLI Host + P19-9 Terminal/Process

### 验证记录（2026-08-10）

- `cargo test -p sandbox-runtime -p builtin-tools -p policy-engine -p resource-loader -p process-runtime -p pty-service`：206 passed / 0 failed
- `cargo clippy`（同 6 crate，`--all-targets -- -D warnings`）：通过
- `cargo fmt`（同 6 crate，`--check`）：通过
- 跨 crate 引用一致性 `rg` 复核：`needs_network`（仅 input schema 测试）、`From<&ExecutionConstraints>`（零残留）、`ENV_ALLOWLIST`（零残留）、`JobLimitsConfig`/`policy_to_job_limits`（零残留）

### 关键实证修正

review §2.6(b) 称私有 `handle` 零消费方建议删除，但实测它是 `ProcessHandle::Drop` 生命周期守卫——Drop 时 cancel kill token 杀整棵进程树，删除字段会导致 spawn 端到端测试子进程瞬间被杀（`Exit{code:None}`）。故保留 `_handle` 字段（`#[allow(dead_code)]` + 文档说明），仅删零调用方的 `kill()` 方法。review 在该字段上的「冗余」判断不成立，已如实记录。
