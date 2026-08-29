# R6 Wave B — Inspector 生命周期、键盘与重连

> 状态：⏭️ 未收口，按用户指令跳过（2026-08-29 开启；同日停止）

用户明确指令「跳过并忽略 R6，直接进行 R7」。因此本波停止，不继续补门禁、不勾选 R6 退出标准，也不把当前未提交工作树或局部验证追记为完成；现有资产原样保留供后续事实核对。

## 范围与事实基线

- Changes：保留 Files / Summary、真实 `diff_list_files` / `diff_get`、DiffView 横滚与 latest-session mismatch 诚实标注；补 workspace/scope 失效、断线 stale、重连刷新和键盘路径。
- Terminal：只经 Host `terminal_create` / `terminal_write` / `terminal_resize` 与 `TerminalOutput`；补 workspace 归属、snapshot 选择/尺寸/状态恢复、失败/stale 展示、输入不丢与重连验证。
- Resources：只读 Host `mcp_list`，不伪造 Add tool 或「已加载规则」；补 stale、重连刷新和键盘路径。
- Inspector：顶层 tab、Changes 二级 tab、collapse/restore、refresh、Terminal 控件和文件行纳入普通 Tab/Enter/Space/方向键与 AX 同源验证；各 surface 独立滚动状态保持。

## 冻结边界与闸门

- Desktop 仍只依赖 `pawork-client`，不直连 Git、PTY、MCP、Provider、数据库或 Policy。
- GUI wire 目前没有 terminal stop/close 命令，也没有 live exit/failure 事件。不得以写入 `exit`、本地 kill 或假按钮冒充通用停止能力；专用 Stop / live 终态若作为 R6 退出硬条件，须先立 ADR。
- stage/unstage/hunk 与 Add tool surface registry 仍无冻结协议，不进入本波实现。

## 计划门禁

1. `cargo test -p pawork-desktop --offline --tests`：状态机、焦点/键盘、stale/reconnect、snapshot terminal 选择与现有回归。
2. Python driver unittest：R6 Wave B AX 断言/状态机脚本。
3. U2 真进程矩阵：Changes 文件/摘要/长行横滚；Terminal create/write/output/resize/reconnect 与 policy 失败；Resources empty/ready/failed；tab/二级 tab/折叠/focus/任务切换/断线重连。
4. 只读模型审查读取 diff 与既有测试日志，不重复 Cargo。

## 开启时已知缺口

- Inspector 可见 tab 与多数按钮只有 mouse/AX press，无普通键盘 tab stop 与焦点恢复。
- `TerminalState` 是全局单例，snapshot 只解析首个 terminal id；多 workspace、终端失效及尺寸/状态恢复不确定。
- Changes / Resources 的旧成功数据断线后仍看似在线，Replay/UpToDate 重连不自动刷新；失败事件未携 epoch，旧失败可覆盖新请求。
- Changes scope 由 workspace query + Host latest session 决定；本波必须显式显示真实范围，不能把它写成当前 turn 精确 diff。
