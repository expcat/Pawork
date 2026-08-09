# P11-9：Phase 11 评审修复（REVIEW remediation）

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P11-1 ~ P11-8

**最终目的**：按 [docs/review/p11-review.md](../docs/review/p11-review.md) §2/§3 收敛「三处重复映射 + 两处集成边界」类问题——删除未接线的归一化 From impl、统一跨平台路径与 env/secret 清单、删除 Windows 死代码、合并 Linux 系统路径常量、补 PTY 丢弃可观测性与 attach_external 契约文档，同时在 sandbox.md / process.md 显式标注「沙箱与 PTY 仅在工具自身/单测内被证明，主流程接线属 P4/Phase 13」。无新增抽象，全部为「删除/合并/标注」。

**涉及范围**：`sandbox-runtime`（lib.rs、backends/{linux,windows,macos}.rs）、`builtin-tools`（run_command.rs）、`policy-engine`（path.rs、lib.rs）、`resource-loader`（Cargo.toml、io.rs、agents.rs、templates.rs）、`process-runtime`（lib.rs）、`pty-service`（lib.rs）、`docs/features/sandbox.md`、`docs/features/process.md`

## 处置策略（按评审 §5 矩阵）

- **现在修复（落地）**：§2.1 删 From impl、§2.2 env/secret 单一来源、§2.3 Linux 路径常量合并、§2.4 跨平台路径统一、§2.5 删 Windows 死代码、§2.6 删 needs_network/kill + network_allow_hosts 标注、§3.2 attach_external 契约文档、§3.3 PTY 丢弃可观测性、§3.1 集成边界文档标注。
- **显式延后**：§2.6 NetworkMode::Off/Hint 合并（属 P11-1.E1 多维 guarantee 重设计）、§3.4 process-runtime 文件拆分（纯组织性，下次触碰时顺手）。

## 细分步骤（分组）

### A. 归一化映射收敛（§2.1）

1. **删两个 From impl + 单测**：`sandbox-runtime/src/lib.rs` 的 `impl From<&ExecutionConstraints> for SandboxPolicy` 与 `for ResourceLimits` 仅被单测消费，生产路径 `run_command` 手工构造。删除两 impl 及 `execution_constraints_map_to_sandbox_policy` 测试。归一化统一映射属 P11-1.E2 Policy-aware Sandbox Planning。
2. **更新 sandbox.md**：「统一契约」段对 `ExecutionConstraints` 归一化的描述改为反映现状（手工构造基线，统一映射在 P11-1.E2 设计）。

### B. env/secret 单一来源（§2.2）

3. **sandbox-runtime 导出权威清单**：`default_env_allowlist` / `default_secret_paths` 改 `pub`，补全为两处并集超集（env 加 TMPDIR/SYSTEMROOT/TEMP/TMP/USERPROFILE/COMSPEC/PATHEXT；secret 加 .aws/.azure/.kube/gcloud）。
4. **run_command 复用**：删本地 `ENV_ALLOWLIST` 常量与本地 `default_secret_paths()`，改调 sandbox-runtime 导出版本（run_command 已依赖 sandbox-runtime）。
5. **超集回归测试**：新增 `default_allowlists_are_authoritative_supersets` 断言单一来源为权威超集。

### C. Linux 系统路径常量合并（§2.3）

6. **共享 const**：`backends/linux.rs` 提取 `SYSTEM_READ_PATHS`，bwrap `generate_bwrap_argv` 的 ro-bind 列表与 Landlock `SYSTEM_READ_PATHS` 共用同一来源；bwrap 按需跳过 `/proc`、`/dev/*`（bwrap 不需要这些路径 bind）。

### D. 跨平台路径统一（§2.4）

7. **policy-engine 开放权威符号**：`path.rs` 的 `relative_to_root`（两 cfg 分支）改 `pub`；`lib.rs` 加入 `pub use` 重导出。
8. **resource-loader 复用**：加 `policy-engine` 依赖；删 io.rs 本地 `path_is_within` + 两 cfg `relative_to_root`（与 policy-engine 逐行同构），改调 `policy_engine::path_within_root` / `relative_to_root`；`canonical_within` 与 agents.rs/templates.rs 共 6 处 `dunce::canonicalize` 改调 `policy_engine::canonicalize_platform`。
9. **移除冗余直接依赖**：resource-loader 的 `dunce` 直接依赖已无残留调用，从 Cargo.toml 移除。git-service 的 `dunce::simplified`（语义不同，review 允许保留）未动。

### E. Windows 死代码清理（§2.5）

10. **删 JobLimitsConfig + policy_to_job_limits**：生产零消费（spawn 走 apply_soft_restrictions→ProcessLimits 通道），删除类型、映射函数及相关测试。
11. **AppContainer 生成器保留并标注 frozen**：`AppContainerConfig` / `AppContainerCapability` / `policy_to_appcontainer_config` 加 `// frozen, awaiting P11-4.E1` 注释保留（接口冻结未接入，与文档一致）。

### F. 小冗余清理（§2.6）

12. **删 SandboxProcessSpec.needs_network**：恒传 false，无后端门控，唯一消费是 NativeRestricted warn 日志；warn 改读 `policy.network_mode`。
13. **删 SandboxProcess::kill 方法**：零调用方。但实证发现私有 `_handle` 字段是 `ProcessHandle::Drop` 生命周期守卫（Drop 时 cancel kill token 杀整树），删除会导致 spawn 测试子进程瞬间被杀——故保留 `_handle` 字段（带 `#[allow(dead_code)]` 与文档说明），仅删 `kill()` 方法。
14. **run_command 同步**：删 `needs_network: false` 与审计 metadata 中 `sandbox.network.{requested,granted,mode}` 对象（三项全为常量，信息价值趋零）；删 `spec.timeout` 双写（仅保留 `resources.wall_time_ms` 路径，apply_soft_restrictions 会从 wall_time_ms 覆写）。
15. **network_allow_hosts 标注**：macos.rs 消费处加 `// not implemented, awaiting egress broker` 注释（与 sandbox.md 网络策略边界一致）。
16. **延后 NetworkMode::Off/Hint 合并**：三后端行为等价，但合并属 P11-1.E1 多维 guarantee 重设计范畴，不在本次。

### G. attach_external 契约文档（§3.2，纯文档）

17. **补全 doc-comment**：`ProcessTreeGuard::attach_external` 三处不对称——limits 仅 Windows 生效（Unix `let _ = limits`）、Unix 要求 leader（pgid==pid 否则 InvalidInput）、后代收养 spawn_blocking 内同步封顶 16 轮。行为零变化。

### H. PTY 丢弃可观测性（§3.3）

18. **dropped_events 计数 + snapshot 暴露**：`SessionInner` 增 `dropped_events: AtomicU64`，broadcast 槽位被覆写时递增，`PtySnapshot.dropped_events` 暴露。`PtyEvent` 不变（序列化兼容）。tokio 1.53.x broadcast 满后静默覆写，故检测在 producer 侧（`len() >= event_capacity` 时计数）。
19. **两个测试**：确定性单测（容量 4 channel 10 sends → counter==6）+ 端到端 PTY 泛洪测试（>256 chunks → snapshot.dropped_events>0）。

### I. 主流程集成边界标注（§3.1，纯文档）

20. **sandbox.md / process.md 新增「主流程集成边界」段**：显式写明 `builtin-tools` / `pty-service` 尚无生产消费方，沙箱与 PTY 的真实 agent 循环通电发生在工具注册（P4 接线）与 GUI Connection Protocol（Phase 13）之后；当前证据限于工具自身与定向测试。
21. **同步 sandbox.md metadata 字段**：删除 `sandbox.network.{requested,granted,mode}` 与 needs_network 审计描述。

## 主要产出物

- 删除：两个 `From<&ExecutionConstraints>` impl + 单测、`SandboxProcessSpec.needs_network`、`SandboxProcess::kill`、`JobLimitsConfig` + `policy_to_job_limits` + 测试、resource-loader 本地 `path_is_within` / `relative_to_root`、run_command 本地 `ENV_ALLOWLIST` / `default_secret_paths()` / `spec.timeout` 双写、metadata `sandbox.network` 对象。
- 合并：Linux `SYSTEM_READ_PATHS` 共享 const、env/secret 单一权威来源（sandbox-runtime 导出）、跨平台路径符号统一（resource-loader → policy-engine）。
- 新增：PTY `dropped_events` 可观测性、attach_external 契约文档、§3.1 集成边界文档段、超集回归测试。
- 标注：AppContainer frozen（P11-4.E1）、network_allow_hosts 未实现（egress broker）。
- 依赖变更：resource-loader 加 `policy-engine`、移除 `dunce`。

## 验收标准（保留 REVIEW 追踪章节）

- [x] **§2.1**：两个 `From<&ExecutionConstraints>` impl 与单测已删；run_command 手工构造基线 SandboxPolicy 不变；归一化统一映射延后 P11-1.E2
- [x] **§2.2**：env/secret 单一权威来源（sandbox-runtime pub 导出，run_command 复用）；超集回归测试落地
- [x] **§2.3**：Linux `SYSTEM_READ_PATHS` 共享 const，bwrap/Landlock 共用
- [x] **§2.4**：resource-loader 复用 policy-engine 路径符号，本地同构副本删除，dunce 直接依赖移除
- [x] **§2.5**：`JobLimitsConfig` + `policy_to_job_limits` 删除；AppContainer 生成器 frozen 标注
- [x] **§2.6**：`needs_network` 删、`SandboxProcess::kill` 删（`_handle` 保留为生命周期守卫）、`network_allow_hosts` 标注、run_command timeout 双写消除；NetworkMode 合并延后 P11-1.E1
- [x] **§3.1**：sandbox.md / process.md 新增「主流程集成边界」段
- [x] **§3.2**：attach_external 契约 doc-comment 补全（limits 语义/leader 前置/收养耗时）
- [x] **§3.3**：PTY dropped_events 可观测性 + 两个测试
- [x] **§3.4 显式延后**：process-runtime 文件拆分（纯组织性，下次触碰时顺手）
- [x] **定向验证**：受影响 6 crate 联合 test（206 passed）/ clippy `-D warnings` / fmt `--check` 全绿

### Deferred items（建议/跟踪，本任务不做）

- **§2.6 NetworkMode::Off/Hint 合并**（P11-1.E1 多维 SandboxGuarantees 重设计）
- **§3.4 process-runtime 文件拆分**（纯组织性，下次触碰该 crate 时顺手）
- **§3.1 主流程接线**（P4 工具注册 + Phase 13 CLI Host + P19-9 Terminal/Process）
- **P11-4.E1 AppContainer 受限令牌 spawn 接线**（frozen 生成器待启用）
- **egress broker**（network_allow_hosts hostname/domain/URL policy 的可靠实现）

## 验证记录（2026-08-10）

- `cargo test -p sandbox-runtime -p builtin-tools -p policy-engine -p resource-loader -p process-runtime -p pty-service`：206 passed / 0 failed（sandbox-runtime 46、builtin-tools 28、policy-engine 57、resource-loader 54、process-runtime 8、pty-service 13）。
- `cargo clippy -p sandbox-runtime -p builtin-tools -p policy-engine -p resource-loader -p process-runtime -p pty-service --all-targets -- -D warnings`：通过。
- `cargo fmt -p sandbox-runtime -p builtin-tools -p policy-engine -p resource-loader -p process-runtime -p pty-service -- --check`：通过。
- 跨 crate 引用一致性：`rg needs_network`（仅 input schema 测试断言保留）、`From<&ExecutionConstraints>`（零残留）、`ENV_ALLOWLIST`（零残留）、`JobLimitsConfig`/`policy_to_job_limits`（零残留）全部确认。
- 按本任务门禁节奏只执行受影响 crate 的定向门禁；workspace 全量、三平台与发布门禁留待 Core 主干 L2/L3。
- **关键实证修正**（review §2.6(b)）：review 称私有 `handle` 零消费方，但实测它是 `ProcessHandle::Drop` 生命周期守卫（Drop cancel kill token 杀整树）；删除字段会导致 spawn 测试子进程瞬间被杀。故保留 `_handle` 字段，仅删 `kill()` 方法。review 的「字段冗余」判断在该字段上不成立。

**相关文档**：[REVIEW.md](../REVIEW.md) §P11 · [docs/review/p11-review.md](../docs/review/p11-review.md) · [sandbox](../docs/features/sandbox.md) · [process](../docs/features/process.md) · [ADR-031](../docs/adr/ADR-031-sandbox-backend-architecture.md) · [ROADMAP Phase 11](../ROADMAP.md)

> 跨任务协调（2026-08-10 review）：写入集覆盖 sandbox-runtime / builtin-tools / policy-engine / resource-loader / process-runtime / pty-service 六 crate 与 sandbox.md / process.md。resource-loader 的 io.rs 与 templates.rs 与 P8-9 任务存在文件级叠加（同文件不同改动）：本任务的改动是路径符号统一（io.rs 删 path_is_within/relative_to_root、改调 policy_engine；templates.rs 2 处 canonicalize 改调 policy_engine）；P8-9 的改动是校验合并 helper（io.rs 新增 is_valid_identifier/is_safe_relative_reference/parse_toml_resource + 单测）与 templates.rs 消费侧切换。两任务改动在文件内不冲突（206 测试全绿、clippy/fmt 通过），reviewer 独立复核确认无功能缺口。其余 resource-loader 文件（loader.rs/skills.rs/profiles.rs/source.rs/diagnostics.rs/request.rs/lib.rs/Cargo.toml 部分）属 P8 任务。Cargo.toml 的 policy-engine 依赖新增 + dunce 移除属本任务。
