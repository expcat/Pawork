# R4–R6 — 核心 Agent 工作流

> 状态：🟢 R4 已收口（Wave A 🟢 2026-08-28 / Wave B 🟢 2026-08-28；拍板 1：State A/B 分区 SSIM ≥0.99 移交 R8；wire 演进仍开放）· 🟢 R5 已收口（Wave A 🟢 2026-08-28 / Wave B + U2 🟢 2026-08-29；Composer SSIM 经用户确认移交 R8）· 🔵 R6 进行中（Wave A 🟢 2026-08-29；Wave B ⚪）
> 前置：R2、R3 依次通过。R4 → R5 → R6 串行推进，每阶段同时完成视觉、真实交互与对应 UI 场景测试。

## R4 — Workspace、Timeline 与 Agent 状态

> 波次状态：
> - **Wave A（🟢 2026-08-28）**：F-05 Header / F-06 Top 对齐 / F-07 消息层级 / F-08 tool group 与 Run 摘要视觉与结构；评审 P0–P3 全修复；107/107 定向测试；State A 结构门禁通过（证据 [docs/ui-review/r4-wave-a/](../docs/ui-review/r4-wave-a/)）。State A/B 区域 SSIM 记录值未达 0.99（fixture 内容差），2026-08-28 拍板 1 同 R3 先例移交 R8。
> - **Wave B（🟢 2026-08-28）**：审批流 / error 原因 / 取消 / 流式 follow / 千级虚拟化 / 断线重放一致 U2 九场景 + Failed 摘要原因显示（WS-1）+ 用户消息乐观回显（WS-4a）+ entry-compare v2（WS-4b）；修复两个真 bug——种子审批决议补广播（WS-3a，wire 契约零变更）与合成终态闸门（WS-5，terminal_reported 去重）；评审 P2（合成 seq-0 压回显 → 2^60 合成序号段）同批修复、P3（早死回显重选消失）登记已知限制；app 156 + desktop 110 定向测试全绿；State B shell 回归 r4b-shell-1 与 U2 九场景 r4b-6 全 PASS（证据与收口记录 [docs/ui-review/r4-wave-b/](../docs/ui-review/r4-wave-b/)）。

### 工作范围

- Workspace Header：真实 task 标题、branch/workspace、运行终态与右侧动作；缺字段只隐藏该项，不删除整体骨架。
- Timeline：user/assistant 的段落、列表、代码、长行、时间/身份层级与 readable width；短会话从 Header 下开始，流式 follow 与用户回看互不抢滚动。
- Tool activity：按真实事件组合 pending/running/succeeded/failed/cancelled，参数/结果可展开；不将日志字符串伪装为结构化状态。
- Approval：在 transcript 内显示动作、目标、越界原因、风险与允许/拒绝选择；进入等待时 TaskRail 与 Activity 同步为 Needs input，决策可回放。
- Error/cancel/completion：明确失败原因、retry 条件、取消终态与 Ready for review 摘要；不用不可验证百分比。
- 大规模 Timeline：千级事件虚拟化、选择、展开、追加、断线重放与 window close 后恢复。

### 关键场景

短会话、长会话、流式追加、用户上滚后新消息、tool 全状态、approval allow/deny/cancel、Host error/retry、断线重连、完成摘要与多 task 状态同步。

### R4 退出标准（2026-08-28 拍板 1）

- [x] Header、消息、tool group、approval、错误/取消和完成摘要均与设计层级一致且由权威事件驱动（Wave A 结构 + Wave B U2 九场景）。
- [x] State A/B Timeline **结构门禁**通过；区域 SSIM `≥0.99` 按拍板 1 移交 R8 终局门禁（Wave A 记录值 timeline 0.665 / header-left 0.940 / header-right 0.883，主因 fixture 内容形状差；条款见 [R7–R8 任务书](R7-R8-ui-quality-gates.md) §3），不阻塞 R4 退出。
- [x] 每种 Agent 状态都有 U0/U1/U2 场景；重放后 UI 与实时执行一致（r4b-6 九场景 + entry-compare 35==35）。
- [x] 长会话无巨幅空白、无限行宽、截断、滚动抢夺或明显输入延迟（S5 虚拟化 barrier 64 / AX 窗口切片卸载；滚轮抢夺留 U1 登记）。

## R5 — Composer 与运行控制

> 波次状态：
> - **Wave A（🟢 2026-08-28）**：F-09 收口——真窗口 Composer 常态总高 156→91（合同 88–94，blocking PASS）；两行结构（输入区 + footer：model 28 truncate / workspace 只读 Label / ContextMeter / 32×32 动作槽）；Send/Cancel 同槽互换 + 单 composer_action_focus（无幽灵 tab stop）；提示行拆除（placeholder 状态机 + footer 瞬态 status_hint）；TextInput 参数化解耦 Terminal（terminal-input，28–220 独立 clamp）；诚实缺省（reasoning / 附件 / 进度条 / queue 不画，workspace 无点击面）；119/119 定向测试；评审 P1×3 + P2×2 全修复；State A 结构门禁三轮全 PASS（证据 [docs/ui-review/r5-wave-a/](../docs/ui-review/r5-wave-a/)）。composer SSIM 记录值 0.423/0.619，按先例移交 R8（待用户拍板）。
> - **Wave B（🟢 2026-08-29）**：输入交互落地——shift 选择 / 鼠标点选拖选 / Copy/Cut/SelectAll / Undo Redo / overflow scroll（TextElement 全内容高 + 父容器视口，caret 滚进视口）/ IME composing 闸门 / can_send trim / per-session 草稿 / Terminal 解耦；U2 九场景与 driver 18 用例交付。cargo test 129 绿、python 40 绿、warnings 15 持平、零 wire 变更。两轮评审：F1–F5（P0×2+P1×2+P2）全修复；第二轮 P1 鼠标/IME 坐标映射根因修复（prepaint 归一化 content_bounds，帧时序无关）+ IME 行高取 last_line_height。解锁后 U2 九场景 22 份断言全 PASS（含输入源 pin/restore 与 model 场景隔离），证据与处置见 [docs/ui-review/r5-wave-b/](../docs/ui-review/r5-wave-b/)。

### 工作范围

- 常态总高、增长上限、输入区、footer controls、ContextMeter 与 Send/Cancel 几何严格按定稿图；`88–94px` 是常态总高，不是输入框最小高。
- 完整输入：mouse/keyboard focus、多行、选择、复制粘贴、撤销重做、中文/日文 IME composition、超长文本与拖放/附件（仅 capability 已实现时）。
- 真实控制：model/provider、reasoning、workspace/context chip、`@` 引用、发送、取消与 disabled/loading/error；选择项从 registry/capability 派生。
- 运行中 follow-up/queue 只有 Host 支持才显示；否则保持可解释禁用，不模仿 Codex 的云端/后台能力。
- 快捷键可发现；Enter/Shift-Enter、Esc、审批键与菜单导航不能互相冲突。
- 状态切换不得移动 Composer 锚点、拉伸按钮、吞掉草稿或让 Popover 覆盖主操作。

### 关键场景

空输入、普通发送、多行/IME、粘贴大文本、切换模型/reasoning/workspace、`@` 选择与移除、发送中取消、错误重试、断线草稿保留、task 切换草稿策略、窄窗和键盘全路径。

### R5 退出标准

- [x] Composer 所有可见控件真实可用或诚实不可用，生产路径无演示数据与空点击面。
- [ ] State A/B 常态与 running Composer 几何/视觉在容差内，区域 SSIM `≥0.99`。
- [x] IME、paste、多行、草稿、send/cancel、菜单和 focus 恢复场景稳定通过。
- [x] 任何状态均无按钮拉伸、遮挡、布局跳动或输入丢失。

> R5 于 2026-08-29 按用户指令退出：自动门禁均已通过；Wave A 区域 SSIM 记录 0.423 / 0.619 同 R3/R4 先例移交 R8 终局视觉门禁，不追认为通过。

## R6 — Inspector、Changes、Terminal 与 Activity

> 波次状态：
> - **Wave A（🟢 2026-08-29）**：实现侧已收敛 Inspector 顶层 strip 与 Changes Files/Summary 二级 strip 的层级/默认落点，并将折叠态 Activity 从 StatusBar 迁到 Workspace Header 右上向下展开约 320px；render/AX/U1、长内容不挤窄 Inspector 与 Header Activity AX 锚点公式回归 132/132 通过，只展示权威 Changes 摘要，不伪造 Add tool / Agent capability。真窗口阻塞根因定位为 macOS 26.6.2 对无 bundle debug 二进制的 AX server 注册 flake（非代码回归），driver 层以 AXWindows 回退 + desktop-restart ≤3 兜底（fail-closed）绕过；Connected State A/B 三相位结构断言全过（label r6a-connected，git_head=d793999），分区 SSIM 记录值同 R3–R5 先例移交 R8；收口审查 P1×1 + P2×2 已整改并回归全绿。证据见 [r6-wave-a](../docs/ui-review/r6-wave-a/notes.md)。
> - **Wave B（⚪）**：覆盖 Changes/Terminal/Resources 真实生命周期、tab/二级 tab/滚动/focus/session 恢复、键盘与断线重连，完成 R6 U2 矩阵与阶段收口。

### 工作范围

1. **Inspector 壳**：顶层 tab 与 Files/Summary 二级 tab 层级清楚；展开/折叠后选择、scroll、terminal session 和 focus 可恢复。
2. **Changes**：明确 working tree / last turn 等真实 scope；文件摘要、选中行、DiffView、行语义色、长行横滚和底部动作。stage/unstage/hunk 若协议未定义，不出现假可用按钮。
3. **Terminal**：绑定当前 workspace/session，覆盖创建、输入、输出、resize、停止、失败与重连；Policy/approval 语义真实，不能绕过 Host。
4. **Resources / Add tool**：仅展示 Host 已提供的 MCP/resource/capability；与设计稿 `+` 的关系按 R1 决议，缺少候选查询时不伪造完整选择器。
5. **Activity**：触发器固定 Workspace 右上；折叠 Inspector 后向下打开约 320px Popover，显示权威摘要与恢复入口，不覆盖 Composer。
6. **状态一致**：TaskRail、Header、Timeline、Inspector/Popover 对 Running/Needs input/Ready/Blocked/Disconnected 使用同一语义。

### 关键场景

Changes 空态与真实 diff、文件切换、Summary/DiffView、长行横滚；Terminal 全生命周期；Resources 空/可用/失败；Inspector 折叠/恢复；Activity 打开/关闭/窗口边界；断线重连与 task 切换。

### R6 退出标准

- [ ] State A Inspector 与 State B ActivityPopover 结构、锚点、层级和内容密度对齐；各区域 SSIM `≥0.99`。
- [ ] Changes/Terminal/Resources 的展示范围与真实 capability 一致，无越权、假动作或错误 scope。
- [ ] 所有 tab、二级 tab、菜单、滚动、折叠、恢复和键盘路径有 U1/U2 场景。
- [ ] Terminal/approval/断线不会破坏 Run 生命周期、协议边界或安全决策。
