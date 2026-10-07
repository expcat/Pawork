# B1 · Policy / Exec 公开面与安全回归裁决

> 状态：已裁决并实现；本机定向门禁、Windows 交叉类型检查与独立 review 通过，Windows 原生回归待验。更新：2026-10-08。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决策略与 PTY 公开 API 的保留范围，以及五项 Exec 安全缺口的回归范围。

## 当前线索与读取范围

原 ROADMAP 登记 `PolicyEngine.mode` 字段、`classify_command` 公开面和 PTY `read_output` 系列零消费，以及 Exec 五项安全行为缺口；实施前重新核对调用者和五项缺口的具体内容。

[policy Spec](../spec/crates/policy.md)、[exec Spec](../spec/crates/exec.md)、[安全规格](../spec/security.md)。

## 实施范围

- 逐项选择保留、收窄或删除，说明公开契约和真实消费者。
- 若保留或修改安全行为，只补对应必要定向回归；不借此扩大测试体系。

## 完成条件

- 每个 API 和安全缺口有可追溯裁决，不能仅凭检索未命中删除。
- 受影响的 Policy / Sandbox / PTY 拒绝、取消和恢复语义不回退；同步对应 Spec。

## 完成记录

### 事实源与历史定位

本轮基线为 `main / 6abeb709`，开始时工作区干净。先用 CodeGraph 定位，再核对当前源码、包内回归与生产调用点；当前 PTY 实现在 `crates/exec/src/pty/mod.rs`，不存在 `pty/manager.rs`。

原 ROADMAP 的五项登记来自 `9b8acc3f`（2026-10-04 R-02）。该提交同步修正 exec Spec 原先高估的测试覆盖，在「已知缺口」中逐项列出：spawn 未授权 Denied、Sandbox 交互启动、Process stdin 写入/close、外部进程组长拒绝、Windows Job 限额映射。`31729171`（2026-10-06）把登记拆到本计划；本轮不根据概括后的「五项」猜测缺口。

### 公开 API 裁决

| 对象 | 裁决与实际修改 | 源码 / 消费者 / 契约依据 |
| --- | --- | --- |
| `PolicyEngine.mode` / `.mode()` / `new(mode)` | 删除冗余字段、getter 与构造参数；`PolicyEngine::new()` / `Default` 无状态构造。迁移 scheduler、两处 Run 装配与 Terminal gate。 | [engine](../../crates/policy/src/engine.rs) 的 `decide` 只读取 `PolicyInput::approval_mode`，字段仅被 getter 读取；[scheduler](../../crates/tools/src/scheduler.rs) 从 config 组装 PolicyInput，[Run](../../crates/app/src/services/run.rs) 与 [loop_ctx](../../crates/app/src/loop_ctx.rs) 持有并传入 Run 档位，[Terminal gate](../../crates/app/src/gui_host/handlers/terminal.rs) 直接传入档位。历史 `731d67b5` 只删过 getter 镜像测试，没有让字段参与决策。调用方审批 / trust 快照继续保留。 |
| `classify_command` / `CommandRisk` | 保留 tokenizer 与全部分类 / 灾难地板回归；收窄为 `pub(crate)`，删除 crate 门面重导出。 | [shell](../../crates/policy/src/shell.rs) 是真实安全实现，[engine::command_risk](../../crates/policy/src/engine.rs) 是生产消费者；包外按信任、档位、能力调用 `decide`。二者不属于 [架构 §3.2](../architecture.md#32-冻结契约激活即采用完整形状golden-先于实现改动) 的冻结 Policy 类型；公开 `RiskLevel`、`ApprovalPrompt`、`PolicyDecision`、`ApprovalMode` 原样保留。 |
| PTY `read_output` 系列 | 删除 `read_output`、`PtyOutputChunk`、两种游标错误、`read_since` 与 `RingReadError`；`RingBuffer` 收为 PTY 内部实现。保留 `OutputCursor`、快照、广播与生命周期 API。 | [Agent Terminal](../../crates/app/src/gui_host/terminal_tool.rs) 读取 `snapshot`，[GUI forwarder](../../crates/app/src/gui_host/handlers/terminal.rs) 消费 `subscribe`；GUI Snapshot 只列终端元数据。没有接受输出游标的生产命令 / wire；旧续读只由包内测试调用，Spec 的「snapshot + read_output 重连协议」没有生产接线。删除依据还包括已工作的替代消费链与冻结 GUI 契约核对，不以零命中单独裁决。 |

被删除的 Stale / Future 测试只锚定随之删除的外部游标输入入口，不再有该拒绝边界。保留 `ring_drops_oldest_when_full`，把旧续读测试改为 `reconnect_snapshot_retains_output_and_exit_state`，并保留 owner、broadcast 覆写、kill / cleanup / shutdown 回归。没有新增依赖、模块、配置层或兼容双轨，schema / wire 与安全红线不变。

### 五项 Exec 安全缺口裁决

| 原缺口 | 裁决与最小回归 | 实际证据 / 验证边界 |
| --- | --- | --- |
| `allow_spawn=false` 的 Denied 硬门 | 保留安全硬门；新增 `native_restricted_rejects_both_spawn_paths_before_execution`，普通与交互入口都必须在 OS spawn 前 Denied。 | [apply_soft_restrictions](../../crates/exec/src/sandbox.rs) 是全部已实现后端的首层；不存在的 program 若走到 OS 会报 Process 而非 Denied。允许侧复用真实子进程环境过滤与 MCP 交互测试。macOS 新回归通过。 |
| `SandboxBackend::spawn_interactive`（含 NativeRestricted）冒烟 | 收窄「需要新增冒烟」的登记，复用已有 `mcp::sandbox::tests::sandboxed_spawner_round_trips_stdio_through_sandbox`，不复制到 exec。 | [MCP stdio](../../crates/mcp/src/sandbox.rs) 经 `Arc<dyn SandboxBackend>` → NativeRestricted → ProcessRuntime 真实 spawn 与 stdin/stdout 往返。缺交互实现的 trait 默认仍显式 BackendUnavailable，不静默降级。 |
| `ProcessRuntime::spawn_interactive` stdin 写入 / close | 写入已有 MCP 消费与回归；只补关闭边界。新增 `interactive_stdin_close_delivers_eof_and_rejects_later_writes`。 | [ProcessInput](../../crates/exec/src/process.rs) 的写入被 MCP writer 消费，`close` 负责显式 EOF，当前无生产调用。新回归通过真实交互进程验证输出、共享幂等关闭、关闭后 BrokenPipe、EOF 后正常退出；进程生命周期仍由 handle 管理。macOS 通过。 |
| `attach_external` 非进程组长拒绝 | 保留前置拒绝；新增 Unix `attach_external_rejects_non_group_leader`。 | [tree](../../crates/exec/src/tree.rs) 拒绝 `pgid != pid`，避免把宿主组绑定为回收目标；真实普通 child 返回 InvalidInput。允许侧继续由 PTY `setsid` 创建与整树 cleanup 回归承接。macOS 通过。 |
| Windows Job 限额映射 | 保留生产映射；回归收窄为真实 Job 的内核查询，不扩为 Windows 全平台安全矩阵。新增 Windows 专属 `job_limits_are_installed_in_kernel`。 | [Job::create](../../crates/exec/src/os/windows.rs) 设置进程数、Job 内存、CPU 100ns 单位与 KILL_ON_JOB_CLOSE；新回归经 QueryInformationJobObject 读取内核值。Windows 原生执行待验，macOS 用例不编译该测试，不记假绿。 |

### 实际验证与交付状态

验证环境为 2026-10-08、macOS 26.6.2 / arm64；全部 Cargo 串行执行，经仓库 kache wrapper。日志只在本机 `/tmp` 保存，不作为检入的跨平台验收资产。

| 实际命令 / 检查 | 结果 | 当次日志 |
| --- | --- | --- |
| `bash scripts/test.sh --log /tmp/pawork-b1-policy-exec.log policy exec` | Policy 73 项、Exec 63 项通过；含真实 Seatbelt Secret 拒绝、写白名单 / 禁写洞、超时 / 取消 / 整树回收与 PTY owner / 快照 / 清理。 | `/tmp/pawork-b1-policy-exec.log` |
| `cargo test -p pawork-exec --offline --lib reconnect_snapshot_retains_output_and_exit_state` | 1 项通过；修正断言允许等待退出期间追加尾部输出，已有内容与游标必须保留。 | `/tmp/pawork-b1-pty-snapshot.log` |
| `cargo test -p pawork-tools --offline --lib scheduler::tests` | 18 项通过；审批快照、档位 / trust 拒绝、执行约束、审批与取消。 | `/tmp/pawork-b1-scheduler.log` |
| `cargo test -p pawork-mcp --offline --lib sandbox::tests` | 7 项通过；真实 Sandbox 交互 stdio、Secret 环境剥离、输出预算拒绝。 | `/tmp/pawork-b1-mcp-sandbox.log` |
| `cargo test -p pawork-app --offline --lib --features ui-fixture terminal` | 19 项通过；含真实 PTY 创建 / 写入 / 中断 / 关闭、owner 与策略拒绝、退出广播及注销。 | `/tmp/pawork-b1-app-terminal.log` |
| `cargo test -p pawork-app --offline --lib --features ui-fixture services::approval::tests` | 8 项通过；审批拒绝 / 一次批准、真实文件与事件持久化、回滚、崩溃恢复。 | `/tmp/pawork-b1-app-approval-service.log` |
| `cargo test -p pawork-app --offline --lib --features ui-fixture gui_host::tests::approval` | 6 项通过；待审批快照、重启恢复、非 live 决策持久化与 live 不重复封口。 | `/tmp/pawork-b1-app-approval-recovery.log` |
| `cargo test -p pawork-app --offline --lib --features ui-fixture set_approval_mode_updates_permissions_settings_within_same_session` | 1 项通过；审批设置写盘 / 重启、非法值与写盘失败拒绝、scheduler 快照同步。 | `/tmp/pawork-b1-app-approval-mode.log` |
| `cargo check -p pawork-exec --offline --tests --target x86_64-pc-windows-msvc` | Windows 测试代码交叉类型检查通过，含新增 Job 查询；未在 Windows 执行。 | `/tmp/pawork-b1-windows-check.log` |
| `cargo tree -p pawork --offline` | 默认 Host 闭包解析通过，含 Host 共 25 个 Pawork 包；所有 Cargo.toml / Cargo.lock 与基线零差异，无新增包或依赖边。 | `/tmp/pawork-b1-dependency-tree.log` |
| `git diff --check`、改动源文件 rustfmt、六个改动文档的相对链接目标检查 | 通过；冻结 schema / wire 与 Cargo 配置零差异。 | 本机确定性检查 |

写入集为 policy / exec，以及迁移 `PolicyEngine::new` 所必需的 tools scheduler、app Run / Terminal gate 与审批快照注释；同批维护四包 Spec、本计划与 ROADMAP。不修改安全决策实现。2026-10-08 独立 review（gpt-6.1-sol / max）PASS，审查者只读核查源码、公开契约、消费者、日志与文档，未编译或运行测试；本批仅作本地提交，不推送。当前缺 Windows 原生环境，须在 Windows 执行 `cargo test -p pawork-exec --offline --lib job_limits_are_installed_in_kernel` 后补运行结果。本任务尚不标为 ✅ 完成，不声称三平台发布证明。

Full workspace gate: NOT RUN（当前未设置全量门禁）
