# CR-03 High Findings 交叉复核（GLM）

- 复核对象：[CR-03-exec-cli.md](CR-03-exec-cli.md) 中 4 条 High（S12-CR03-01 ~ 04）
- 复核人：zai/glm-5.3（glm_reviewer）
- 复核日期：2026-08-18
- 方法：不采信报告转述，逐条独立打开源码（路径+符号+行号）核对实际行为；遵守 S12 只读纪律，未运行任何构建/测试/二进制。

## 裁定表

| 编号 | 原严重度 | 裁定 | 一行理由 |
| --- | --- | --- | --- |
| S12-CR03-01 | High | uphold（维持 High） | 整盘只读 allow、Hard 标签、deny 清单缺 ~/.pawork/auth.json 三项均在源码逐行核实，属默认 macOS 路径上的凭据暴露。 |
| S12-CR03-02 | High | adjust-severity（降为 Medium） | 事实全部成立（无沙箱/审批/分类、退出不调 pty.shutdown()），但唯一写入方是本机用户的手工输入，未找到任何模型到 TerminalWrite 的调用路径，未跨权限边界，High 高估。 |
| S12-CR03-03 | High | uphold（维持 High） | --instance 经原始 format! 拼进 plist XML / systemd unit，可注入 RunAtLoad+KeepAlive 持久化定义；stop --apply 又不删除定义，与任务书声明不符。 |
| S12-CR03-04 | High | uphold（维持 High） | 分类表/灾难地板/审批放行链逐行核实：cmd/powershell/curl|sh 等全部落 Safe，AskForDangerous 下以 Moderate 直接 AllowWithConstraints，构成默认审批门绕过。 |

## 逐条复核记录

### S12-CR03-01 — macOS Seatbelt 整盘只读仍标 Hard（uphold）

- 整盘只读：execution/exec/src/os/macos.rs generate_seatbelt_profile 62-65 无条件写 (allow file-read* (subpath "/"))，注释自述是为 Darwin 25+ /bin/echo SIGABRT 放开；read_roots（66-72）被根 allow 完全覆盖，失去约束力。
- secret deny 覆盖面：execution/exec/src/sandbox.rs default_secret_paths 578-589 仅含 ~/.ssh/.aws/.azure/.kube（Windows 另加 gcloud）。providers/auth/src/file_backend.rs 39、337 证实 Pawork 自身凭据在 ~/.pawork/auth.json（$PAWORK_HOME 可改），不在 deny 清单；~/.gnupg、~/.config 同样不在。
- deny 仍生效但只剩清单内：macos.rs 92-96 在根 allow 之后追加 deny file-read*（Seatbelt v1 后规则优先），报告「deny 只覆盖那四个目录」的表述准确。
- Hard 标签：execution/exec/src/sandbox.rs SandboxSelector::pick 298-318，sandbox_exec 可用即返回 IsolationLevel::Hard + fallback: false；315 行 note 诚实披露 file-read 未收敛，但枚举语义（416、423 行「实际生效的隔离强度」）与 Hard 的注释定义（435 行）不符，且类型系统已有 HardFilesystemOnly 这类更弱级别的先例。
- 消费方：execution/tools/src/run_command.rs 232-237（deny: default_secret_paths()）、257-268（pick+spawn）、330-335（metadata 原样写 isolation/note）。
- 影响链核实：Enforce 下网络仍全拒（macos.rs 98-105，K-09 已挂账不重复建项），但 stdout 进模型上下文即构成凭据读出通道；写仍受 write_roots + deny default 约束，与报告一致。
- 裁定：三项核心主张全部成立，High 维持。

### S12-CR03-02 — GUI Terminal/PTY 绕过与退出不回收（adjust-severity → Medium）

- 事实全部核实：
  - host/app/src/gui_host.rs TerminalCreate 1246-1264 只构造 cwd/owner/size 的 PtyCreateSpec，不经 SandboxSelector/SandboxPolicy/shell 分类/审批；from_locked 383 持有 PtyService；shutdown 413-417 只 Arc::try_unwrap(core)，全仓库 rg 无任何 pty.shutdown() 调用点（PtyService::shutdown 在 execution/exec/src/pty/mod.rs 663-684 存在但零消费）。
  - execution/exec/src/pty/mod.rs open_and_spawn 823-883 直接 portable_pty 拉起，唯一防护是 ProcessTreeGuard::attach_external（864）；build_command 885-918 用 $SHELL（909），未 env_clear，继承父环境。
  - host/cli/src/gui.rs run_gui 82-93：Ctrl-C 只关 listener、删 pid、unwrap core，不碰 adapter 内 PtyService。
- 降级理由（关键反证）：写入方只有用户。apps/desktop/src/ui/mod.rs send_terminal_input 505-526 从 terminal_input 文本框取用户键入并 terminal_write；engine/execution 无任何构造 AppCommand::TerminalWrite 的路径（全仓库仅 Desktop controller 与 protocol-probe）。按 S12 证据标准（未搜到调用点不能单独定罪），「模型一旦写入」缺乏调用链；用户在本机 App 终端里执行任意命令不跨权限边界（对齐 Zed/VS Code 集成终端不做沙箱的常态），「绕过沙箱与策略」对用户键入而言是产品取舍而非既成违约。
- 仍成立的 Medium 级缺陷：宿主退出/被 kill 不回收 PTY 子进程（shutdown() 零调用 + launchd KeepAlive 场景孤儿累积）；该终端未按「本机不受控终端」显式标注；全量环境继承。若未来出现任何模型可驱动的 terminal 写入路径，应立即回升 High。
- 裁定：事实 uphold、严重度调整为 Medium（Bug + Requirement Gap）。

### S12-CR03-03 — service stop 不删单元 + --instance 标识注入（uphold）

- 校验弱：host/app/src/data_dir.rs normalize_instance 29-38 仅拒空、/、反斜杠、..，空格、引号、分号、换行、尖括号均放行；host/cli/src/lib.rs 369-374 证实该返回值直接进入 service::run_service。
- 注入面：host/cli/src/ops.rs service_name 19-25 拼 pawork.{instance}；host/cli/src/service.rs launchd_plist 70-94 用 format! 原文插值 name/instance 进 XML（无转义，可闭合 string 标签改写 ProgramArguments）；Linux install_definition 62-64 的 ExecStart 行换行可注入新 unit 指令；Windows sc 116-127 为 argv 数组传递（注入面弱，但标识仍可含任意字符）。
- stop 不删：service.rs execute_service_action macOS 144 仅 launchctl unload、Linux 158 仅 systemctl --user stop、Windows 130 仅 sc stop，均无删除/disable；git log -p 证实 stop 自引入起就只有 unload。
- 文档漂移：plan/S10-serve-clients.md 48 行明称「stop --apply 后……删 plist」，与源码不符。
- 严重度判断：在「模型建议命令、用户粘贴执行」的核心威胁模型下，plist XML 注入 + RunAtLoad/KeepAlive（86-89）构成用户级持久化原语，而 stop --apply 恰好不清除该定义，两者叠加维持 High。
- 裁定：全部主张成立，High 维持（Windows 分支影响较弱，可在整改时一并收严字符集）。

### S12-CR03-04 — Dangerous 分类遗漏（uphold）

- 分类表：execution/policy/src/shell.rs classify_single 125-136 仅 rm/chmod/chown/git + is_dangerous_program 155-161（sudo/su/dd/shutdown/reboot/halt/poweroff/format/reg/mkfs*）。cmd、powershell/pwsh、python、osascript、diskpart、schtasks、launchctl、curl、wget 均落默认分支 false。
- shell 包装也漏：is_shell_program 217-222 不含 cmd/powershell，extract_shell_script 不拆其 -Command；即便经 sh -c 包装 curl 管道，classify_snippet 按管道分段后两段均 Safe，redirection_dangerous 238-268 只管重定向目标、danger_regexes 276-294 无对应模式——curl|sh 最终 Safe，与报告一致。
- 灾难地板：catastrophic_single 76-93 仅 mkfs*、dd of=/dev*、rm -rf /（root 仅字面 /）。
- 放行链：execution/policy/src/engine.rs effective_risk 123-131 把 Safe 命令映为 Moderate；decide 88-94 在 AskForDangerous 下 Moderate 走 allow_or_constrained（109-120）直接放行；NeverAsk 79 行同路径。即上述命令全部静默执行。
- 文档自认：plan/S4-exec-sandbox.md 33 行括注「分类器不认 PowerShell Remove-Item」。
- 裁定：全部主张成立。该遗漏直接打通模型到 run_command 的破坏性命令免审批路径，High 维持。

## 复核补充事实（不构成新 finding）

- IsolationLevel 枚举已含 HardFilesystemOnly/Degraded（sandbox.rs 429-441），CR03-01 的「标签应与实际强度一致」整改有现成语义先例，无需新枚举语义发明。
- PtyService::shutdown/cleanup_owner 均已实现且带测试（pty/mod.rs 653-684、1269），CR03-02 的整改只需在 GuiHostAdapter::shutdown / run_gui 收尾接线。
- Desktop 终端输入是纯用户驱动（ui/mod.rs 505-526），该事实同时是 CR03-02 降级依据与 CR03-04 影响面描述（PTY 不经分类器）的边界。
