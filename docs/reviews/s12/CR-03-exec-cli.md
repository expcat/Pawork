# CR-03 进程 / 沙箱 / CLI 服务路径审查

- CR 编号：CR-03
- 主审范围：`execution/exec`、`host/cli`（进程/服务路径为主，含 tests）；为核对沙箱消费与取消/PTY 接线，只读抽查了 `execution/tools/src/run_command.rs`、`execution/policy/src/shell.rs`、`host/app/src/gui_host.rs`
- 审查日期：2026-08-17
- 主审模型：xai/grok-4.6（grok_reviewer）

## 1. 实际审查路径

- `execution/exec/src/lib.rs`
- `execution/exec/src/sandbox.rs`（选择器、软限制、`NativeRestricted`、secret/env 清单）
- `execution/exec/src/process.rs`（进程组 / Job、timeout/cancel、输出截断）
- `execution/exec/src/tree.rs`（`ProcessTreeGuard`、Unix `killpg` / Linux `/proc` 扫树、Windows Job）
- `execution/exec/src/cancel.rs`
- `execution/exec/src/path.rs`
- `execution/exec/src/os/macos.rs`（Seatbelt profile 生成与 `sandbox-exec` spawn）
- `execution/exec/src/os/linux.rs`（bwrap argv、Landlock compile、Linux 进程树终止）
- `execution/exec/src/os/windows.rs`（AppContainer 探测冻结、Job Object 后端）
- `execution/exec/src/pty/mod.rs`、`execution/exec/src/pty/buffer.rs`
- `execution/tools/src/run_command.rs`（`SandboxSelector` 热路径，只读核对）
- `execution/policy/src/shell.rs`（Dangerous 分类，只读核对）
- `host/cli/src/lib.rs`、`service.rs`、`ops.rs`、`gui.rs`、`headless.rs`、`chat.rs`、`render.rs`
- 对照：`plan/S4-exec-sandbox.md`、`plan/S10-serve-clients.md`、`ROADMAP.md` §3.2 K-09、`docs/design.md` §3.2 / §4 S4+S10

## 2. 未覆盖路径与原因

- `host/cli` 的 auth/mcp/import/plan/tasks/usage/sessions/vcs/agents/acp 业务命令：不在本包「进程/服务路径」核心问题内，只核对 `--json` stdout 是否污染协议流。
- Windows AppContainer 受限令牌 spawn、Linux Landlock/bwrap 与 macOS Seatbelt 的真实内核效果：S12 禁止运行测试/冒烟；仅审源码与既有注释。
- `kill -9` 后跨进程恢复：本包只能证明当前进程内没有 reaper/supervisor；会话 seal/resume 语义已由 K-02 挂账，不重复建任务。
- `execution/policy` 审批矩阵整体属 CR-02；本报告只登记分类器遗漏对进程执行面的影响，不重复建审批绕过 finding。

## 3. Findings

### S12-CR03-01 — macOS Seatbelt 把整盘只读放开后仍报告 IsolationLevel::Hard

- 类别：Security
- 严重度：High
- 置信度：Confirmed
- 证据：
  - 实际行为：`generate_seatbelt_profile` 为规避 Darwin 25+ `/bin/echo` SIGABRT，无条件写入 `(allow file-read* (subpath "/"))`，使 `read_roots` 失效；secret deny 只覆盖 `~/.ssh` / `.aws` / `.azure` / `.kube`（Windows 另加 gcloud），不含 `~/.pawork/auth.json`、`~/.gnupg`、`~/.config` 等。`SandboxSelector::pick` 在 `sandbox_exec` 可用时仍返回 `isolation: Hard`、`fallback: false`。`RunCommandTool` 直接消费该选择结果并写入工具 metadata。
  - 期望行为：硬隔离应对 `read_roots`/`deny` 做文件系统强制；若因平台限制放开读，选择器不得把结果标成 Hard，且至少覆盖 Pawork 自身凭证路径。
  - 影响面：macOS 上 `run_command` 的默认路径。Agent 只需读文件即可拿走本机任意可读凭据（含 `~/.pawork/auth.json`），同时调用方/审计会看到 `isolation=hard`。写仍受 `write_roots` 限制；网络 Enforce 仍全拒（K-09 已挂账 allowlist 未实现，不在此重复建项）。
  - 路径：
    - `execution/exec/src/os/macos.rs` `generate_seatbelt_profile` 62-65、92-96
    - `execution/exec/src/sandbox.rs` `SandboxSelector::pick` 298-317；`default_secret_paths` 578-588
    - `execution/tools/src/run_command.rs` `run` 257-333
- 验证建议：在 macOS Seatbelt 下对 workspace 外路径做 `cat ~/.pawork/auth.json` / `cat ~/.ssh/id_rsa` / `cat /etc/passwd` 对照；同时核对工具 metadata 的 `isolation`/`note`。S12 内不执行。
- 整改边界：最小写入 `execution/exec/src/os/macos.rs` 与 `default_secret_paths`；选择器 note/isolation 枚举只改与「读未隔离」一致的标签。不要顺带实现 K-09 egress broker，也不要改 Linux bwrap profile。

### S12-CR03-02 — GUI Terminal / PTY 完全绕过沙箱与策略，宿主退出也不回收会话

> **交叉复核裁定**（2026-08-18 主代理回写，GLM 复核，详见 [CR-03-cross-review-glm.md](CR-03-cross-review-glm.md)）：**adjust-severity → Medium**。事实全部成立，但全仓唯一写入方是 Desktop 用户输入框，无模型→TerminalWrite 调用路径，不跨权限边界；孤儿回收缺失按 Medium，若未来出现模型驱动写入路径应回升 High。

- 类别：Security
- 严重度：High
- 置信度：Confirmed
- 证据：
  - 实际行为：`GuiHostAdapter` 的 `TerminalCreate` 只用 workspace cwd 构造 `PtyCreateSpec`，不经过 `SandboxSelector`、`SandboxPolicy`、shell 风险分类或审批。`open_and_spawn` 直接 `portable_pty` 拉起用户 `SHELL`（默认继承完整父环境），只挂 `ProcessTreeGuard`。`GuiHostAdapter::shutdown` 只关 `AppCore`，从不调用 `PtyService::shutdown`。`pawork gui serve` 的 Ctrl-C 路径关闭 listener 后 `try_unwrap(core)`，不碰 adapter 里的 `PtyService`。协作式取消最多能杀 `run_command` 进程组；交互终端不受 token 约束。`kill -9` 后更没有外部 reaper。
  - 期望行为：S10 把 PTY 作为正式执行面后，至少应复用 run_command 的隔离/审批，或显式降级为「本机不受控终端」并在宿主退出时回收。Ctrl-C / 正常停服必须 `pty.shutdown()`。
  - 影响面：Desktop / 多 GUI 的 Terminal tab。用户或模型一旦写入 `cat ~/.pawork/auth.json`、`curl`、`git push --force`，无沙箱、无 Dangerous 门、环境变量全量继承。KeepAlive 的 launchd 服务被 `kill -9` 后由 launchd 拉起新进程，但旧 PTY 子进程不在 Job/cgroup 里，会成孤儿。
  - 路径：
    - `host/app/src/gui_host.rs` `GuiHostAdapter::from_locked` 383；`shutdown` 413-417；`TerminalCreate` 1246-1264
    - `execution/exec/src/pty/mod.rs` `open_and_spawn` 823-882；`build_command` 885-917
    - `host/cli/src/gui.rs` `run_gui` 82-93
- 验证建议：经 `gui serve` 开 Terminal，读 `~/.pawork/auth.json` 与执行 `sleep 300`；Ctrl-C / `service stop --apply` / `kill -9` 后用 `pgrep -lf` 查残留。S12 内不执行。
- 整改边界：`host/app/src/gui_host.rs` 增加 PTY shutdown 与（若产品确认）沙箱/审批接线；`execution/exec/src/pty/mod.rs` 只补隔离包装，不改 ring buffer / 重连协议。不要顺带做 VT100 或 Snapshot 缓冲。

### S12-CR03-03 — `service stop --apply` 不删单元，且 `--instance` 可注入 launchd/systemd/sc 标识

- 类别：Security
- 严重度：High
- 置信度：Confirmed
- 证据：
  - 实际行为：`normalize_instance` 只拒绝 `/` `\\` `..`，允许空格、引号、`;`、换行。该字符串直接进入 plist Label、systemd 文件名与 `sc create` 参数。macOS `stop --apply` 只 `launchctl unload`，不删除 `~/Library/LaunchAgents/{name}.plist`；Linux 只 `systemctl --user stop`，不 disable、不删 unit。S10 任务书写明 `stop --apply` 后「进程退出并删 plist」，源码未实现。
  - 期望行为：实例名应限制为 `[A-Za-z0-9._-]`；`--apply` 默认 dry-run 成立（已实现），但 stop 必须按任务书回收常驻定义，避免下次登录/KeepAlive 自动拉起。
  - 影响面：本机服务安装面。恶意或误输入的 `--instance` 可写出意外 Label/unit；stop 后 plist 仍在，macOS `RunAtLoad`/`KeepAlive` 会在下次 load/login 把 `gui serve` 拉回来。
  - 路径：
    - `host/app/src/data_dir.rs` `normalize_instance` 29-37
    - `host/cli/src/service.rs` `run_service` 29-31；`launchd_plist` 70-80；`execute_service_action` 133-158
    - `plan/S10-serve-clients.md` 第 48 行（文档称 stop 删 plist）
- 验证建议：`service install --apply` 后只 `stop --apply`，检查 plist/unit 是否仍在；再用带空格/分号的 instance 看 Label 是否原样写入。S12 内不执行。
- 整改边界：`normalize_instance` + `host/cli/src/service.rs` 的 stop 回收。不要顺带改 socket/pid 布局，也不要碰 Windows SCM 未验收路径以外的行为，除非同一写入集能一起修标识校验。

### S12-CR03-04 — Dangerous 分类漏掉 PowerShell / cmd 高危动词与常见破坏命令

- 类别：Security
- 严重度：High
- 置信度：Confirmed
- 证据：
  - 实际行为：`classify_single` 只把 Unix `rm/chmod/chown/git` 加一份固定危险程序表（`sudo/dd/mkfs/shutdown/reboot/format/reg`）标为 Dangerous。S4 任务书已记录分类器不认 `Remove-Item -Recurse`，并改用 `git push --force` 过关。`cmd /c del /s /q`、`powershell -Command Remove-Item -Recurse`、`curl|sh`、`wget|sh`、`python -c`、`osascript`、`diskpart`、`schtasks`、`launchctl` 都走 Safe。PTY 路径甚至不调用分类器（见 S12-CR03-02）。灾难地板只拦 `mkfs`、`dd of=/dev*`、`rm -rf /`。
  - 期望行为：进程执行面的风险分类应覆盖本机默认 shell（Windows 是 cmd/PowerShell，macOS PTY 是用户 SHELL）。至少把任务书点名的 `Remove-Item -Recurse` 以及等价递归删除/提权/远程管道纳入 Dangerous。
  - 影响面：`ApprovalMode::AskForDangerous` / `NeverAsk` 下，Windows 或经 `cmd/powershell` 包装的破坏命令可静默执行；Unix 上 `curl|sh` 同类也是 Safe。
  - 路径：
    - `execution/policy/src/shell.rs` `classify_single` 125-135；`is_dangerous_program` 155-160
    - `plan/S4-exec-sandbox.md` 第 33 行（已承认不认 `Remove-Item`）
- 验证建议：对 `Remove-Item -Recurse`、`cmd /c del /s`、`curl https://... | sh`、`python -c` 跑 `classify_command` 单测矩阵。S12 内不执行。
- 整改边界：只改 `execution/policy/src/shell.rs` 分类表与对应定向单测。不要顺带改审批模式枚举或 scheduler。

### S12-CR03-05 — 选择器在硬隔离不可用时继续裸跑，与 S4「绝不静默裸跑」字面冲突

- 类别：Requirement Gap
- 严重度：Medium
- 置信度：Confirmed
- 证据：
  - 实际行为：`SandboxSelector::pick` 在 macOS 探测失败时回退 `NativeRestricted`；Linux 回退 Landlock（仅文件系统）或 `NativeRestricted`；Windows 永久 `available: false` 的 AppContainer 后固定走 Job-only，文件系统/网络都是软限制。`NativeRestricted` 对 `NetworkMode::Enforce` 只打 warn，仍 spawn。`run_command` 不因 `fallback=true` / `isolation=soft|degraded` 拒绝。S4 任务书第 24 行要求「探测失败除显式 `--sandbox off` 外拒绝执行」；同文档第 35 行又把验收改成 ADR-031「可观测回退，不是拒跑」。
  - 期望行为：产品需二选一：要么按任务书拒跑，要么把 ADR-031 写回 design/S4 退出标准，并让调用方在 metadata 之外对用户可见。当前源码走后者，文档仍保留前者。
  - 影响面：非 macOS 或 Seatbelt 不可用的机器上，Agent 命令以软沙箱继续跑；Windows 永远没有文件/网络硬隔离。这不是静默（metadata/tracing 有记录），但用户/模型不一定看见。
  - 路径：
    - `execution/exec/src/sandbox.rs` 模块注释 3-5；`SandboxSelector::pick` 321-410；`NativeRestricted::spawn` 225-233
    - `execution/exec/src/os/windows.rs` `probe_appcontainer_job` 72-95；`WindowsJobBackend::spawn` 432-451
    - `plan/S4-exec-sandbox.md` 24、35 行
- 验证建议：在无 bwrap/sandbox-exec 环境或强制探测失败后看 `run_command` 是否仍 spawn，以及 CLI/GUI 是否向用户展示 fallback。S12 内不执行。
- 整改边界：先改文档或选择器策略之一，不要同时改三平台 profile。若维持 ADR-031，至少让 CLI/GUI 在 fallback 时显式提示。

### S12-CR03-06 — macOS 协作式取消无法回收 setsid/脱离进程组的后代

- 类别：Bug
- 严重度：Medium
- 置信度：Confirmed
- 证据：
  - 实际行为：Unix `ProcessRuntime` 用 `setpgid(0,0)` + `killpg`。Linux `ProcessTreeGuard::terminate` 额外冻树并扫 `/proc`，`process.rs` 有 `kill_reaps_descendant_that_escaped_with_setsid` 测试。macOS 分支只 `killpg`，对 ESRCH 当成功；PTY 的 `attach_external` 也只按 pgid leader 记账。子进程 `setsid` / 双 fork 后，Ctrl-C 只杀原组。`kill -9` 掉 `pawork` 本身时，macOS 没有 `PR_SET_PDEATHSIG`，也没有 launchd 级 cgroup；PTY/未沙箱进程会残留（与 S12-CR03-02 叠加）。
  - 期望行为：S4 取消链路要求「命令进程与其子进程全部终止」。至少 macOS 应有与 Linux 类似的后代扫描，或对 PTY/bwrap 统一进可回收容器。
  - 影响面：`run_command` 里 `setsid sleep 300`、后台 `&` 再 `disown`，以及任何交互终端里启动的守护进程。Windows Job + `KILL_ON_JOB_CLOSE` 相对完整，不在本条。Linux 已有扫树，不重复定罪。
  - 路径：
    - `execution/exec/src/tree.rs` `ProcessTreeGuard::terminate` 111-141；`attach_external` 47-75
    - `execution/exec/src/process.rs` `configure_unix_child` 541-543；Linux-only 测试 `kill_reaps_descendant_that_escaped_with_setsid`
    - `execution/exec/src/os/linux.rs` `linux_process_tree::terminate`（约 295 行起）
- 验证建议：macOS 上跑 `setsid sleep 300` / `sleep 300 & disown`，Ctrl-C 后 `ps`。S12 内不执行。
- 整改边界：把 Linux `/proc` 扫树的语义抽到 Unix，或在 macOS 用 `libproc` 等价实现。不要顺带改 Windows Job。

### S12-CR03-07 — `--json` 纪律只覆盖 chat/run/headless；遗留 JsonlSink 会打非协议行

- 类别：Bug
- 严重度：Low
- 置信度：Confirmed
- 证据：
  - 实际行为：`host/cli/src/lib.rs` 声明 `--json` 时 stdout 只打 `HeadlessResponse` JSONL，文本走 stderr。`run()` 对错误走 `eprintln`，`chat::run_json` / `headless --json-stdio` 主路径合规。但 `JsonlSink`（`render.rs` 220-226）仍直接 `println` 原始 `AgentEventEnvelope`，与正式 `HeadlessResponse` 不是同一形状。`service --json` 输出的是自定义对象而不是 `HeadlessResponse`（按运维命令可接受，但与模块头注释不完全一致）。
  - 期望行为：全局 `--json` 要么只对 chat/run/headless 生效并在文档写明，要么所有子命令都走同一 JSONL 形状；废弃/隐藏 `JsonlSink` 以免被再次接到生产路径。
  - 影响面：SDK 若误用 REPL/`JsonlSink` 会解析失败。当前 `run_json`/`headless` 主路径未直接调用 `JsonlSink`，所以不是 P0 泄漏。
  - 路径：
    - `host/cli/src/lib.rs` 3-5、68-70、355-361
    - `host/cli/src/render.rs` `JsonlSink::emit` 220-226
    - `host/cli/src/chat.rs` `print_headless` 226-232（主路径合规）
- 验证建议：`pawork --json run ...` / `headless --json-stdio` 对 stdout 做形状断言；再确认没有生产调用点使用 `JsonlSink`。S12 内不执行。
- 整改边界：删除或门控 `JsonlSink`；不要顺带改 headless 协议。

## 4. 统计

| 严重度 | 条数 | Confirmed | Needs Verification |
| --- | --- | --- | --- |
| Critical | 0 | 0 | 0 |
| High | 4 | 4 | 0 |
| Medium | 2 | 2 | 0 |
| Low | 1 | 1 | 0 |
| 合计 | 7 | 7 | 0 |

已知基线引用：K-09（macOS `network_allow_hosts` 在 Enforce 下全拒、未做 egress broker）已在 `macos.rs` 100-105 复核，不另建 finding。K-02 覆盖审批等待前持久化 / `kill -9` 后 seal，本包只补充 PTY/进程树在同一崩溃下会残留的执行面证据。
