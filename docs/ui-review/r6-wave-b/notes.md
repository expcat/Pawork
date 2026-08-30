# R6 Wave B — Inspector 生命周期、键盘与重连

> 状态：🟢 已收口（2026-08-30）；审查后最终二进制的 U2 九场景已在 macOS AX 注册恢复后一次补录通过。同日用户确认将 R6 State A/B Inspector/Activity 分区 SSIM `≥0.99` 移交 R8，R6 总阶段已退出。

## 结果

R6 Wave B 已在冻结 GUI wire 内完成：Changes、Terminal、Resources 与 Inspector 的状态归属、键盘/AX、失败/stale、snapshot/replay/reconnect 路径通过定向测试和真 Host/Desktop 九场景。生产 Desktop 仍只依赖 `pawork-client`，未增加 Git/PTY/MCP/Policy 直连，也未伪造 stage/unstage/hunk、Add tool 或 Terminal Stop。

核心修复：

- Changes：`diff_list_files` / `diff_get` 回执以 epoch + path + Host session id 三重校验；latest-session 切换时 fail-closed；scope 文案明确不是 workspace filter；长行建立真实横向 extent。
- Terminal：多个 workspace 的 terminal、create 失败、草稿和 pending write/create 分别归属；output-before-create 先按 id 缓存，权威 workspace 回执前不上屏；成功回执清失败占位；UpToDate 合并非事件权威 snapshot，Replay 历史输出不会复活 exited/killed 终态。
- Resources/重连：断线旧数据保留但标 stale；Fresh、SnapshotRequired、Replay、UpToDate 后均刷新当前打开的查询面。
- Controller：terminal create 失败携 workspace；Changes/Resources 失败走可靠 channel；diff 内容携 Host 实际 session id。
- Client 根因：Host 进程重启会重新分配 `client-0`，旧的 `client_id + 本地序号` 自动命令 id 会撞上持久化幂等账本。本波为每个 `GuiClient` 连接实例加入 request namespace；显式 `command_envelope` 的幂等语义不变。

## 真实界面证据

审查后最终二进制的完整通过目录：[u2-reviewfix-pass-20260830](u2-reviewfix-pass-20260830/)：

- `scenario-matrix.json`：`c1/c2/c3/t1/i1/s1/d1/r1/t2` 九场景齐全。
- 19 份 `assert-*.json` 全部 `pass: true`；trace 以 `run done; all requested real-interface scenes passed` 结束。
- C3 由真实 CGEvent 横向滚动，AX 记录 offset `-720.0 / 2477.0`，证明不是只画 nowrap 文本。
- T2 在 Host 重启后真实命中 `ReadOnly` policy 拒绝，证明新连接请求未重放旧 `Accepted`。
- fixture secret scan：87 files、0 hits。
- 00:14–00:18 UTC 的完整运行一次通过；trace 未出现 `AX recursion`、`desktop-restart` 或 AXWindows fallback。

审查前完整矩阵形成于 [u2-rootfix-pass-20260830](u2-rootfix-pass-20260830/)。审查随后发现并修复两个矩阵未覆盖/诊断边角：create 回执前切 workspace 的 terminal 首段串屏，以及新 latest session 不含旧路径时空 `diff_get` 缺 session id。两项先由 app/desktop 定向回归覆盖；第一次审查后补录在任何业务场景前被 macOS AX 连续 3 次递归 `AXApplication` fail-closed，证据保留于 [u2-reviewfix-ax-blocked-20260830](u2-reviewfix-ax-blocked-20260830/)。外部状态恢复后先做一次临时 C1 预检，再执行且仅执行一次最终九场景补录，完整通过。

## 验证

- `cargo test -p pawork-app --offline --lib --tests`：178/178 通过。
- `cargo test -p pawork-desktop --offline --bins --features gpui/runtime_shaders`：144/144 通过；3 个既有 dead-code warning。
- `cargo test -p pawork-client --offline --lib --tests`：41/41 通过（lib 10、client_tests 22、contract 9）。
- `python3 -m unittest scripts/test_ui_r6_wave_b_states.py`：6/6 通过。
- `bash -n scripts/ui-r6-wave-b-states.sh`、`swiftc -typecheck scripts/ui-key-event.swift`、`git diff --check`：通过。
- 只读模型审查：发现 P1 terminal 首段串屏与 P2 空 diff scope 诊断；均已最小修复并通过上述定向门禁，其余检查 clean。
- 完整 U2 真进程矩阵：审查后最终二进制 9 场景/19 断言全部通过；87 文件 Secret 扫描 0 命中。

## 冻结边界与后续

- GUI wire 仍无 terminal stop/close 命令和 live exit/failure 事件；不得以写入 `exit`、本地 kill 或假按钮冒充。若要新增，先立 ADR 演进 wire。
- Changes 仍为 Host latest-session 只读 diff；stage/unstage/hunk 与 Add tool registry 没有冻结协议，不在本波。
- 三张定稿图分区 SSIM、完整 VoiceOver/hover/性能与用户视觉签字仍由 R7/R8 汇总；本波的结构/交互通过不等于视觉终局通过。
